use std::{collections::HashSet, convert::Infallible, hash::RandomState};

use bson::{doc, oid::ObjectId};
use chrono::{NaiveDate, NaiveTime, TimeDelta, Utc};

use futures::{Sink, SinkExt, StreamExt, TryStreamExt};
use mongodb::Collection;
use serde::Serialize;
use slog::Logger;
use smallvec::SmallVec;

use crate::{
    channels,
    db::{Model, OIDCollection, OID},
    notifications::UpdateEvent,
};

use super::{types::Class, ScheduleParser};

#[derive(Debug, Default)]
pub struct ClassDelta {
    pub added_classes: Vec<OID<Class>>,
    pub removed_classes: Vec<OID<Class>>,
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
            .filter(|last_reparsed| {
                last_reparsed
                    < &(Utc::now().date_naive() + TimeDelta::days(self.config.days_ahead as i64))
            })
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
        events_consumer: impl channels::Tx<crate::notifications::UpdateEvents>,
    ) -> tokio::task::JoinHandle<eyre::Result<Infallible>> {
        let fut = async move {
            loop {
                let result = self.parse_next().await;

                match result {
                    Ok(delta) => {
                        slog::info!(self.logger, "parser.got_delta"; "added" => delta.added_classes.len(), "removed" => delta.removed_classes.len());

                        let mut events = SmallVec::new();

                        // lol, de'morgan law in action
                        let should_send =
                            !delta.added_classes.is_empty() || !delta.removed_classes.is_empty();

                        if !should_send {
                            continue;
                        }

                        for added_class in delta.added_classes {
                            events.push(UpdateEvent::ClassAdded { class: added_class });
                        }
                        for removed_class in delta.removed_classes {
                            events.push(UpdateEvent::ClassRemoved {
                                class: removed_class,
                            });
                        }

                        if events_consumer.send(events).await.is_err() {
                            slog::error!(self.logger, "parser.delta_channel_err");
                        }
                    }

                    Err(err) => {
                        slog::error!(self.logger, "parser.errored"; "err" => ?err);
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
    let mut classes_new = classes.peekable();

    let Some(first_new_class) = classes_new.peek() else {
        return Ok(delta);
    };

    let coll = coll.clone_with_type::<OID<Class>>();
    let classes_in_db_query = crate::db::create_range_query(&first_new_class.range.start, None);

    let classes_in_db: Vec<_> = coll.find(classes_in_db_query).await?.try_collect().await?;

    // first we find classes that aren't present
    for class_new in classes_new.by_ref() {
        let does_db_have = classes_in_db
            .iter()
            .any(|db_class| db_class.data == class_new);

        if !does_db_have {
            let class_new = OID {
                id: ObjectId::new(),
                data: class_new.clone(),
            };

            delta.added_classes.push(class_new);
        }
    }

    let mut session = coll.client().start_session().await?;
    session.start_transaction().await?;

    // now we remove all classes from db that were cancelled
    for class_in_db in classes_in_db {
        let does_new_includes = classes_new.any(|new_class| &new_class == &class_in_db.data);

        if does_new_includes {
            coll.delete_one(doc! {"_id": &class_in_db.id}).await?;
            delta.removed_classes.push(class_in_db);
        }
    }

    // batch insert all classes that are new

    if !delta.added_classes.is_empty() {
        coll.insert_many(delta.added_classes.clone().into_iter())
            .await?;
    }

    session.commit_transaction().await?;

    Ok(delta)
}
