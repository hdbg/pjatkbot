use parsing::pjatk::Parser;

pub mod bot;
pub mod db;
pub mod notifications;
pub mod parsing;

#[macro_use]
extern crate rust_i18n;

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    mongodb_uri: String,
    database_name: String,
    pjatk: parsing::manager::Config,
    telegram: bot::BotConfig,
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

    let mut pjatk = Parser::new();
    let mut manager = parsing::manager::ParserManager::new(&db, pjatk, &config.pjatk, &logger);

    let handle = manager.work(futures::sink::drain());

    let mut bot = bot::setup_bot(config, &logger, &db);

    bot.dispatch().await;

    handle.abort();

    Ok(())
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
