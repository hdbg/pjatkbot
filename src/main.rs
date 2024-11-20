use std::fs::read_to_string;

use chrono::Utc;
use parsers::pjatk::{ParseRequest, Parser, PjatkClass};
use slog::{info, o};
use types::Class;
use utils::Model;

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    bot_token: String,
    mongodb_uri: String,

    admin_id: String,
}

const DB_NAME: &str = "pjatkschedulebot";

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let config_file = std::env::args().nth(1).unwrap_or("config.toml".to_string());

    let config: Config = toml::from_str(std::fs::read_to_string(config_file)?.as_ref())?;

    let logger = setup_logger();
    info!(logger, "boot");

    let mongo_session = mongodb::Client::with_uri_str(&config.mongodb_uri).await?;
    let db = mongo_session.database(DB_NAME);

    let classes_coll: mongodb::Collection<types::Class> = db.collection(&Class::COLLECTION_NAME);

    let mut pjatk = Parser::new();

    let day = pjatk.parse_day(ParseRequest { date: Utc::now() }).await?;

    classes_coll.insert_many(day.into_iter()).await?;

    Ok(())
}
pub mod db {
    use mongodb::Collection;

    use crate::types::Class;

    pub async fn replace_day(
        coll: &Collection<Class>,
        classes: impl Iterator<Item = Class>,
    ) -> eyre::Result<()> {
        Ok(())
    }
}
pub mod parsers;

pub mod utils {
    // a database model
    pub trait Model {
        const COLLECTION_NAME: &'static str;
    }
}
pub mod types {
    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub enum ClassKind {
        Lecture,
        Seminar,
        DiplomaThesis,
    }

    use chrono::{DateTime, Utc};

    use bson::serde_helpers::chrono_datetime_as_bson_datetime;
    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub struct TimeRange {
        #[serde(with = "chrono_datetime_as_bson_datetime")]
        pub start: chrono::DateTime<Utc>,
        #[serde(with = "chrono_datetime_as_bson_datetime")]
        pub end: chrono::DateTime<Utc>,
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub enum StudyMode {
        Online,
        OnSite,
        PartTime,
    }
    pub enum Language {
        English,
        Polish,
    }
    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub enum Degree {
        Bachelor,
        Master,
        Doctoral,
    }
    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub enum Semester {
        Number(u8),
        Retake,
    }
    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub struct Group {
        pub code: String,
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub enum ClassPlace {
        Online,
        OnSite { room: String },
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
    pub struct Class {
        pub name: String,
        pub code: String,
        pub kind: ClassKind,
        pub lecturer: String,
        pub range: TimeRange,
        pub place: ClassPlace,
        pub groups: Vec<Group>,
    }

    impl super::utils::Model for Class {
        const COLLECTION_NAME: &'static str = "classes";
    }
}

fn setup_logger() -> slog::Logger {
    use sloggers::terminal::{Destination, TerminalLoggerBuilder};
    use sloggers::types::Severity;
    use sloggers::Build;

    let mut builder = TerminalLoggerBuilder::new();
    builder.level(Severity::Debug);
    builder.format(sloggers::types::Format::Full);
    builder.destination(Destination::Stdout);

    let logger = builder.build().unwrap();
    logger
}
