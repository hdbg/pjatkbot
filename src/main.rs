use std::convert::Infallible;

use mongodb::Database;
use parsing::pjatk::Parser;
use slog::Logger;
use tokio::task::JoinSet;

pub mod bot;
pub mod db;
pub mod notifications;
pub mod parsing;

pub mod channels {
    use eyre::Error;

    pub trait Tx<Item: Send + 'static>: 'static + Send {
        type Error: std::error::Error + std::fmt::Debug;
        fn send(&self, item: Item) -> impl std::future::Future<Output = Result<(), Error>> + Send;
    }

    pub trait Rx<Item: Send + 'static>: 'static + Send {
        type Error: std::error::Error + std::fmt::Debug;
        fn recv(&self) -> impl std::future::Future<Output = Result<Item, Error>> + Send;
    }

    impl<Item: Send + 'static> Tx<Item> for kanal::AsyncSender<Item> {
        type Error = kanal::SendError;

        async fn send(&self, item: Item) -> Result<(), Error> {
            <kanal::AsyncSender<Item>>::send(self, item).await?;
            Ok(())
        }
    }

    impl<Item: Send + 'static> Rx<Item> for kanal::AsyncReceiver<Item> {
        type Error = kanal::ReceiveError;

        async fn recv(&self) -> Result<Item, Error> {
            let item = <kanal::AsyncReceiver<Item>>::recv(self).await?;
            Ok(item)
        }
    }
}

#[macro_use]
extern crate rust_i18n;

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    mongodb_uri: String,
    database_name: String,
    pjatk: parsing::manager::Config,
    telegram: bot::BotConfig,

    notifications_manager: notifications::manager::Config,
    propagator: notifications::propagator::Config,
}

const BOT_TIMEZONE: chrono_tz::Tz = chrono_tz::Europe::Warsaw;
i18n!();

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let logger = setup_logger();
    let config = load_config()?;
    let db = db::load_database(config).await?;

    let _log_guard = slog_scope::set_global_logger(logger.clone());
    slog_stdlog::init_with_level(log::Level::Info)?;
    slog::info!(logger, "boot");
    let (notifications_tx, notifications_rx) = kanal::unbounded_async();

    let mut bot = bot::setup_bot(config, &logger, &db, notifications_rx);

    let mut tasks = setup_tasks(&db, &config, &logger, notifications_tx).await?;

    tokio::select! {
        Some(tasks) = tasks.join_next() => {
            // we only care about error
            let _ = tasks??;
        }

        err = bot.dispatch() => {
            println!("{:#?}", err);
        }
    };

    tasks.abort_all();

    Ok(())
}

async fn setup_tasks(
    db: &Database,
    config: &'static Config,
    logger: &Logger,
    notifications_tx: impl channels::Tx<notifications::NotificationEvents> + Clone,
) -> eyre::Result<JoinSet<Result<eyre::Result<Infallible>, tokio::task::JoinError>>> {
    let mut handle_set = JoinSet::new();
    let (updates_tx, updates_rx) = kanal::unbounded_async();

    let pjatk = Parser::new();
    let parser_manager = parsing::manager::ParserManager::new(&db, pjatk, &config.pjatk, &logger);

    handle_set.spawn(parser_manager.work(updates_tx));

    let notifications_manager = notifications::manager::NotificationManager::new(
        &config.notifications_manager,
        &db,
        &logger,
    );

    handle_set.spawn(
        notifications_manager
            .work(updates_rx, notifications_tx.clone())
            .await?,
    );

    let notifications_propagator =
        notifications::propagator::Propagator::new(&db, &config.propagator, &logger);

    handle_set.spawn(notifications_propagator.work(notifications_tx));

    Ok(handle_set)
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
fn load_config() -> eyre::Result<&'static Config> {
    let config_file = std::env::args().nth(1).unwrap_or("config.toml".to_string());

    let config: Config = toml::from_str(std::fs::read_to_string(config_file)?.as_ref())?;

    let config = Box::leak(Box::new(config));
    Ok(config)
}
