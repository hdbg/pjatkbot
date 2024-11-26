use std::{collections::HashSet, convert::Infallible, hash::RandomState};

use bson::doc;
use chrono::{NaiveDate, NaiveTime, TimeDelta, Utc};

use futures::{Sink, SinkExt, StreamExt};
use mongodb::Collection;
use serde::Serialize;
use slog::Logger;

use crate::db::Model;

use super::{types::Class, ScheduleParser};

#[derive(Debug, Default)]
pub struct ClassDelta {
    pub removed_classes: Vec<Class>,
}

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    pub interval: std::time::Duration,
    pub days_ahead: u32,
}
#[derive(serde::Deserialize, Serialize, Default, Clone)]
pub struct Data {
    pub name: String,
    pub last_day_reparsed: Option<NaiveDate>,
    pub last_day_parsed: Option<NaiveDate>,
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

    async fn get_maximum_day_parsed(&self, data: &Data) -> eyre::Result<Option<NaiveDate>> {
        if let Some(date) = data.last_day_parsed {
            return Ok(Some(date.clone()));
        }

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
                    last_day_parsed: None,
                };
                self.data_collection.insert_one(new_data.clone()).await?;

                Ok(new_data)
            }
        }
    }

    async fn select_date(&self, data: &Data) -> eyre::Result<DaySelector> {
        let maximum_date_parsed = self.get_maximum_day_parsed(&data).await?;

        let today = Utc::now().date_naive();

        match maximum_date_parsed {
            Some(date_max) if (date_max - today).num_days() <= self.config.days_ahead as i64 => {
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

        let next_date_reparse = data
            .last_day_reparsed
            .map(|last_reparsed| last_reparsed + TimeDelta::days(1))
            .unwrap_or(today);

        slog::info!(&self.logger, "selecting date"; "mode" => "reparsing", "date" => next_date_reparse.to_string());

        Ok(DaySelector {
            date: next_date_reparse,
            kind: SelectorKind::Refreshing,
        })
    }

    pub async fn parse_next(&mut self) -> eyre::Result<ClassDelta> {
        let current_data = self.get_current_parser_data().await?;

        let selector = self.select_date(&current_data).await?;
        let parsed_day = self.parser.parse_day(selector.date.clone()).await?;
        let class_delta =
            replace_or_fill_day(&self.class_collection, parsed_day.into_iter()).await?;

        let data_update = match selector.kind {
            SelectorKind::ParsingNew => Data {
                last_day_parsed: Some(selector.date),
                ..current_data
            },
            SelectorKind::Refreshing => Data {
                last_day_reparsed: Some(selector.date),
                ..current_data
            },
        };

        self.data_collection
            .find_one_and_replace(doc! {"name": Parser::NAME}, data_update)
            .upsert(true)
            .await?;
        Ok(class_delta)
    }

    pub fn work(
        mut self,
        mut delta_consumer: impl Sink<ClassDelta> + Unpin + Send + 'static,
    ) -> tokio::task::JoinHandle<eyre::Result<Infallible>> {
        let fut = async move {
            loop {
                let result = self.parse_next().await;

                match result {
                    Ok(delta) if !delta.removed_classes.is_empty() => {
                        slog::info!(self.logger, "parser.got_removed_classes"; "removed_list" => ?delta.removed_classes);
                        if delta_consumer.send(delta).await.is_err() {
                            slog::error!(self.logger, "parser.delta_channel_err");
                        }
                    }
                    Err(err) => {
                        slog::error!(self.logger, "parser.errored"; "err" => ?err);
                        println!("{:#?}", err);
                    }

                    _ => (),
                }

                tokio::time::sleep(self.config.interval).await;
            }
        };
        tokio::task::spawn(fut)
    }
}

// In case db already contrains classes for this day,
// will return classes that were deleted
// e.g. user might want notification if class was cancelled
pub async fn replace_or_fill_day(
    coll: &Collection<Class>,
    classes: impl Iterator<Item = Class>,
) -> eyre::Result<ClassDelta> {
    let mut delta = ClassDelta::default();
    let removed_classes = &mut delta.removed_classes;
    let mut classes = classes.peekable();

    let Some(first_class) = classes.peek() else {
        // no work to do
        return Ok(delta);
    };

    let min_class_start = first_class
        .range
        .start
        .with_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
        .unwrap();
    let max_class_end = min_class_start
        .with_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap())
        .unwrap();

    let db_stored_classes_query = doc! {"range.start": {"$gt": bson::DateTime::from(min_class_start), "$lt": bson::DateTime::from(max_class_end)}};
    let mut db_classes = coll.find(db_stored_classes_query).await?;
    let mut db_classes_set = HashSet::new();
    while let Some(class) = db_classes.next().await {
        db_classes_set.insert(class?);
    }
    // optimization to handle case when it's a new day parsed
    if db_classes_set.is_empty() {
        coll.insert_many(classes).await?;
        return Ok(delta);
    }

    let new_classes_set: HashSet<Class, RandomState> = HashSet::from_iter(classes);

    let diff_to_insert = new_classes_set.difference(&db_classes_set);

    let diff_to_insert: Vec<_> = diff_to_insert.collect();
    if !diff_to_insert.is_empty() {
        coll.insert_many(diff_to_insert.into_iter().cloned())
            .await?;
    }

    let diff_to_remove = db_classes_set.difference(&new_classes_set);

    for class in diff_to_remove.into_iter() {
        coll.find_one_and_delete(mongodb::bson::to_document(class)?)
            .await?;
        removed_classes.push(class.clone());
    }

    Ok(delta)
}
