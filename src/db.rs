use chrono::TimeDelta;
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct User {
    pub id: teloxide::types::ChatId,
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
const DB_NAME: &str = "pjatkschedulebot";

pub async fn load_database(config: &Config) -> eyre::Result<mongodb::Database> {
    let mongo_session = mongodb::Client::with_uri_str(&config.mongodb_uri).await?;
    let db = mongo_session.database(DB_NAME);

    Ok(db)
}
