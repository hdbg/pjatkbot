use chrono::{DateTime, NaiveTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use eyre::OptionExt;
use mongodb::Collection;
use serde::{Deserialize, Serialize};

use crate::{
    parsing::types::{Class, Group},
    Config,
};

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    strum::EnumString,
    strum::IntoStaticStr,
    strum::Display,
    strum::EnumIter,
)]
pub enum Language {
    #[strum(serialize = "en")]
    English,
    #[strum(serialize = "pl")]
    Polish,
    #[strum(serialize = "ukr")]
    Ukrainian,
    #[strum(serialize = "ru")]
    Russian,
}

impl Language {
    pub fn code(&self) -> &'static str {
        self.into()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NotificationConstraint(pub std::time::Duration);

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Role {
    User,
    BetaTester,
    Admin,
}
use bson::serde_helpers::chrono_datetime_as_bson_datetime;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct User {
    pub id: teloxide::types::ChatId,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub join_date: DateTime<Utc>,
    pub role: Role,
    pub groups: Vec<Group>,
    pub language: Language,
    pub constraints: Vec<NotificationConstraint>,
}

impl Model for User {
    const COLLECTION_NAME: &'static str = "users";
}

pub trait Model {
    const COLLECTION_NAME: &'static str;
}

pub fn create_range_query<T: TimeZone>(
    date: &DateTime<T>,
    end_point: Option<DateTime<T>>,
) -> mongodb::bson::Document {
    let end = date
        .with_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap())
        .unwrap();

    let start_point = end_point.unwrap_or_else(|| date.with_time(NaiveTime::MIN).unwrap());

    mongodb::bson::doc! {"range.start": {"$gt": bson::DateTime::from(start_point), "$lt": bson::DateTime::from(end)}}
}

pub async fn load_database(config: &Config) -> eyre::Result<mongodb::Database> {
    let mongo_session = mongodb::Client::with_uri_str(&config.mongodb_uri).await?;
    let db = mongo_session.database(&config.database_name);

    Ok(db)
}
