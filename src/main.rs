use std::{fs::read_to_string, sync::Arc};

use chrono::Utc;
use db::{Model, User};
use parsing::{
    pjatk::{Parser, PjatkClass},
    types::Class,
};
use slog::{info, o, Logger};
use teloxide::{
    dispatching::{dialogue::GetChatId, DefaultKey, UpdateFilterExt},
    dptree,
    prelude::Dispatcher,
    types::Update,
    Bot,
};

#[macro_use]
extern crate rust_i18n;

i18n!();

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    mongodb_uri: String,

    admin_id: String,

    pjatk: parsing::manager::Config,

    telegram: bot::BotConfig,
}

const DB_NAME: &str = "pjatkschedulebot";

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let logger = setup_logger();
    let config = load_config()?;
    let db = load_database(config).await?;

    let _log_guard = slog_scope::set_global_logger(logger.clone());
    slog_stdlog::init_with_level(log::Level::Info)?;
    slog::info!(logger, "boot");

    let mut pjatk = Parser::new();
    let mut manager = parsing::manager::ParserManager::new(&db, pjatk, &config.pjatk, &logger);

    // let handle = manager.work(futures::sink::drain());

    let mut bot = bot::setup_bot(config, &logger, &db);

    bot.dispatch().await;

    // handle.abort();

    Ok(())
}

pub mod bot {
    use std::sync::Arc;

    use dptree::filter_async;
    use mongodb::Collection;
    use slog::Logger;
    use teloxide::{
        dispatching::{
            dialogue::{GetChatId, InMemStorage},
            DefaultKey,
        },
        prelude::*,
    };

    use crate::{
        db::{Model, User},
        parsing::types::Class,
        Config,
    };

    pub mod utils;

    #[derive(serde::Serialize, serde::Deserialize, Debug)]
    pub struct BotConfig {
        pub bot_token: String,

        pub disappering_message_delay: std::time::Duration,
    }

    pub struct BotState {
        pub config: &'static BotConfig,
        pub users_coll: Collection<User>,
        pub classes_coll: Collection<Class>,
        pub logger: Logger,
    }
    type DialogueStorage<State> = teloxide::dispatching::dialogue::InMemStorage<State>;
    type BotDialogue<State> =
        teloxide::dispatching::dialogue::Dialogue<State, DialogueStorage<State>>;

    type HandlerResult = eyre::Result<()>;

    fn create_storage<State>() -> Arc<DialogueStorage<State>> {
        InMemStorage::new()
    }

    pub fn setup_bot(
        config: &'static Config,
        logger: &Logger,
        db: &mongodb::Database,
    ) -> Dispatcher<Bot, eyre::Report, DefaultKey> {
        let users_coll = db.collection(&User::COLLECTION_NAME);
        let classes_coll = db.collection(&Class::COLLECTION_NAME);

        let logger = logger.new(slog::o!("subsystem" => "bot"));

        let bot = Bot::new(config.telegram.bot_token.clone());

        let state = Arc::new(BotState {
            config: &config.telegram,
            users_coll,
            classes_coll,
            logger,
        });

        let mut dependencies = dptree::deps![state.clone()];
        dependencies.insert_container(user_path::user_onboard_dialog::deps());

        Dispatcher::builder(
            bot,
            dptree::entry()
                // NOTE: this currently limits event handling only to users interactions
                // in case some other updates are required, the following line should be removed
                .filter_map(|update: Update| update.chat_id())
                .branch(dptree::filter_map_async(
                    |id: ChatId, state: Arc<BotState>| async move {
                        // check if user is registered
                        let user_query = mongodb::bson::doc! {"id": id.0};

                        match state.users_coll.find_one(user_query).await {
                            Ok(result) => result,
                            Err(err) => {
                                slog::error!(state.logger, "reg_check.error"; "err" => ?err);
                                None
                            }
                        }
                    },
                ))
                .branch(user_path::user_onboard_dialog::handler()),
        )
        .enable_ctrlc_handler()
        .dependencies(dependencies)
        .build()
    }

    pub mod user_path {
        use std::sync::Arc;

        use teloxide::Bot;

        use crate::db::User;

        use super::{BotState, HandlerResult};

        pub mod user_onboard_dialog;

        async fn main_menu(bot: Bot, bot_state: Arc<BotState>, user: User) -> HandlerResult {
            todo!()
        }
    }
}

pub mod db {
    use chrono::TimeDelta;
    use eyre::OptionExt;
    use mongodb::Collection;
    use serde::{Deserialize, Serialize};

    use crate::parsing::types::{Class, Group};

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
fn load_config() -> eyre::Result<&'static Config> {
    let config_file = std::env::args().nth(1).unwrap_or("config.toml".to_string());

    let config: Config = toml::from_str(std::fs::read_to_string(config_file)?.as_ref())?;

    let config = Box::leak(Box::new(config));
    Ok(config)
}

async fn load_database(config: &Config) -> eyre::Result<mongodb::Database> {
    let mongo_session = mongodb::Client::with_uri_str(&config.mongodb_uri).await?;
    let db = mongo_session.database(DB_NAME);

    Ok(db)
}
