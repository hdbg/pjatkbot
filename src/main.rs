use std::fs::read_to_string;

use chrono::Utc;
use db::Model;
use parsing::{
    pjatk::{Parser, PjatkClass},
    types::Class,
};
use slog::{info, o};

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    bot_token: String,
    mongodb_uri: String,

    admin_id: String,

    pjatk: parsing::manager::Config,
}

const DB_NAME: &str = "pjatkschedulebot";

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let config_file = std::env::args().nth(1).unwrap_or("config.toml".to_string());

    let config: Config = toml::from_str(std::fs::read_to_string(config_file)?.as_ref())?;

    let config = Box::leak(Box::new(config));

    let logger = setup_logger();
    info!(logger, "boot");

    let mongo_session = mongodb::Client::with_uri_str(&config.mongodb_uri).await?;
    let db = mongo_session.database(DB_NAME);

    let classes_coll: mongodb::Collection<Class> = db.collection(&Class::COLLECTION_NAME);

    let mut pjatk = Parser::new();
    let mut manager = parsing::manager::ParserManager::new(&db, pjatk, &config.pjatk, &logger);

    manager.work().await?;

    Ok(())
}

pub mod bot {}

pub mod db {
    use chrono::TimeDelta;
    use eyre::OptionExt;
    use mongodb::Collection;
    use serde::{Deserialize, Serialize};

    use crate::parsing::types::{Class, Group};

    #[derive(Serialize, Deserialize, Debug, Clone, strum::EnumString, strum::IntoStaticStr)]
    pub enum Language {
        English,
        Polish,
        Ukrainian,
    }

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct NotificationConstraint(std::time::Duration);

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
}
pub mod parsing;

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
