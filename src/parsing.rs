use types::Class;

pub trait IntoLocalized {
    fn localized(&self, locale: &str) -> &str;
}

pub trait ScheduleParser: Send + Sync + 'static {
    const NAME: &'static str;
    fn parse_day(
        &mut self,
        day: chrono::NaiveDate,
    ) -> impl std::future::Future<Output = eyre::Result<Vec<Class>>> + Send;
}

pub mod types;

pub mod manager {
    use std::{collections::HashSet, convert::Infallible, hash::RandomState};

    use bson::{doc, DateTime};
    use chrono::{NaiveDate, NaiveTime, TimeDelta, Utc};
    use eyre::OptionExt;
    use futures::StreamExt;
    use mongodb::Collection;
    use serde::Serialize;
    use slog::Logger;

    use crate::db::Model;

    use super::{types::Class, ScheduleParser};

    #[derive(serde::Deserialize, Debug)]
    pub struct Config {
        pub interval: std::time::Duration,
        pub days_ahead: u32,
    }
    #[derive(serde::Deserialize, Serialize, Default, Clone)]
    pub struct Data {
        pub name: String,
        pub last_day_reparsed: Option<NaiveDate>,
    }

    impl Model for Data {
        const COLLECTION_NAME: &'static str = "parsing_datas";
    }

    // each variant date denotes day which is parsed this time
    enum SelectorKind {
        ParsingNew,
        Refreshing,
    }

    struct DaySelector {
        date: NaiveDate,
        kind: SelectorKind,
    }

    pub struct ParserManager<Parser: ScheduleParser> {
        parser: Parser,
        class_collection: Collection<Class>,
        data_collection: Collection<Data>,
        config: &'static Config,
        logger: Logger,
    }

    impl<Parser: ScheduleParser> ParserManager<Parser> {
        pub fn new(
            db: &mongodb::Database,
            parser: Parser,
            config: &'static Config,
            logger: &Logger,
        ) -> Self {
            let class_collection = db.collection(Class::COLLECTION_NAME);
            let data_collection = db.collection(Data::COLLECTION_NAME);
            let logger =
                logger.new(slog::o! {"subsystem" => "parser.manager", "parser" => Parser::NAME});

            Self {
                class_collection,
                data_collection,
                parser,
                logger,
                config,
            }
        }

        async fn get_maximum_day_parsed(&self) -> eyre::Result<Option<NaiveDate>> {
            // query to get the latest class
            let max_class = self
                .class_collection
                .find_one(bson::doc! {})
                .sort(bson::doc! {"range.start": -1})
                .await?;

            Ok(max_class.map(|class| class.range.start.date_naive()))
        }

        async fn get_current_parser_data(&self) -> eyre::Result<Data> {
            let data_query = self
                .data_collection
                .find_one(doc! {"name": Parser::NAME})
                .await?;

            match data_query {
                Some(data) => return Ok(data),
                None => {
                    let new_data = Data {
                        name: Parser::NAME.to_owned(),
                        last_day_reparsed: None,
                    };
                    self.data_collection.insert_one(new_data.clone()).await?;

                    Ok(new_data)
                }
            }
        }

        async fn select_date(&self) -> eyre::Result<DaySelector> {
            let maximum_date_parsed = self.get_maximum_day_parsed().await?;
            let current_data = self.get_current_parser_data().await?;

            let today = Utc::now().date_naive();

            match maximum_date_parsed {
                Some(date_max)
                    if (date_max - today).num_days() <= self.config.days_ahead as i64 =>
                {
                    let delta = date_max - today;
                    let day = today + (delta + TimeDelta::days(1));
                    slog::info!(&self.logger, "selecting date"; "mode" => "ParsingNew", "date" => day.to_string());
                    return Ok(DaySelector {
                        date: day,
                        kind: SelectorKind::ParsingNew,
                    });
                }
                None => {
                    slog::info!(&self.logger, "selecting date"; "mode" => "ParsingNew", "date" => today.to_string());

                    return Ok(DaySelector {
                        date: today,
                        kind: SelectorKind::ParsingNew,
                    });
                }
                _ => (),
            }

            let next_date_reparse = current_data
                .last_day_reparsed
                .map(|last_reparsed| last_reparsed + TimeDelta::days(1))
                .unwrap_or(today);

            slog::info!(&self.logger, "selecting date"; "mode" => "reparsing", "date" => next_date_reparse.to_string());

            Ok(DaySelector {
                date: next_date_reparse,
                kind: SelectorKind::Refreshing,
            })
        }

        pub async fn parse_one(&mut self) -> eyre::Result<()> {
            let selector = self.select_date().await?;
            let parsed_day = self.parser.parse_day(selector.date.clone()).await?;
            replace_or_fill_day(&self.class_collection, parsed_day.into_iter()).await?;

            let data_update = match selector.kind {
                SelectorKind::ParsingNew => {
                    // need to reset reparsing counter, because we might have
                    None
                }
                SelectorKind::Refreshing => Some(selector.date),
            };

            self.data_collection
                .find_one_and_replace(
                    doc! {"name": Parser::NAME},
                    Data {
                        name: Parser::NAME.to_owned(),
                        last_day_reparsed: data_update,
                    },
                )
                .upsert(true)
                .await?;
            Ok(())
        }

        pub fn work(mut self) -> tokio::task::JoinHandle<eyre::Result<Infallible>> {
            let fut = async move {
                loop {
                    let result = self.parse_one().await;

                    tokio::time::sleep(self.config.interval).await;
                }
            };
            tokio::task::spawn(fut)
        }
    }
    pub async fn replace_or_fill_day(
        coll: &Collection<Class>,
        classes: impl Iterator<Item = Class>,
    ) -> eyre::Result<()> {
        let mut classes = classes.peekable();
        let first_class = classes.peek().ok_or_eyre("classes list is empty")?;

        println!("{:#?}", first_class);

        let min_class_start = first_class
            .range
            .start
            .with_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
            .unwrap();
        let max_class_end = min_class_start
            .with_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap())
            .unwrap();

        let db_stored_classes_query = doc! {"range.start": {"$gt": bson::DateTime::from(min_class_start), "$lt": bson::DateTime::from(max_class_end)}};
        println!("{}", db_stored_classes_query);
        let mut db_classes = coll.find(db_stored_classes_query).await?;
        let mut db_classes_set = HashSet::new();
        while let Some(class) = db_classes.next().await {
            db_classes_set.insert(class?);
        }
        // optimization to handle case when it's a new day parsed
        if db_classes_set.is_empty() {
            coll.insert_many(classes).await?;
            return Ok(());
        }

        let new_classes_set: HashSet<Class, RandomState> = HashSet::from_iter(classes);

        let diff_to_insert = new_classes_set.difference(&db_classes_set);
        let diff_to_remove = db_classes_set.difference(&new_classes_set);

        // if diff_to_insert.
        coll.insert_many(diff_to_insert.into_iter().cloned())
            .await?;

        for class in diff_to_remove {
            coll.find_one_and_delete(mongodb::bson::to_document(class)?)
                .await?;
        }

        Ok(())
    }
}

pub mod pjatk;
