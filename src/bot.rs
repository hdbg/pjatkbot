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
type BotDialogue<State> = teloxide::dispatching::dialogue::Dialogue<State, DialogueStorage<State>>;

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
