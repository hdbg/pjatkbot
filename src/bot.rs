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
    utils::command::{self, BotCommands},
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

type BotHandler = teloxide::dispatching::UpdateHandler<eyre::Report>;

#[derive(BotCommands, Debug, Clone, PartialEq)]
#[command(rename_rule = "snake_case")]
pub enum UserCommands {
    Start,
}

fn create_storage<State>() -> Arc<DialogueStorage<State>> {
    InMemStorage::new()
}

#[rustfmt::skip]
fn build_main_handler_tree() -> BotHandler {
    dptree::entry()
        .branch(
            Update::filter_message()
            .filter_command::<UserCommands>()
            .branch(dptree::case![UserCommands::Start].endpoint(user_path::main_menu))
        )
}

#[rustfmt::skip]
fn build_handler_tree() -> BotHandler {
    dptree::entry()
        // NOTE: this currently limits event handling only to user interactions
        // in case some other updates are required, the following line should be removed
        .filter_map(|update: Update| update.chat_id())
        .branch(
            dptree::filter_map_async(
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
        
            )
            // all redirect to actual handling happens here
            // note usage of `chain`
            .branch(build_main_handler_tree())
            .endpoint(|state: Arc<BotState>, update: Update| async move {
                 slog::warn!(state.logger, "unhandled_update"; "update" => ?update);       
                 Ok(())
            })
        )
        .chain(user_path::user_onboard_dialog::handler())
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

    Dispatcher::builder(bot, build_handler_tree())
        .enable_ctrlc_handler()
        .dependencies(dependencies)
        .build()
}

pub mod user_path {
    use std::sync::Arc;

    use bson::doc;
    use chrono::{DateTime, Days, Timelike, Utc};
    use futures::StreamExt;
    use teloxide::{payloads::SendMessageSetters, prelude::Requester, types::ParseMode, Bot};

    use crate::{db::User, parsing::types::Class};

    use super::{BotState, HandlerResult};

    pub mod user_onboard_dialog;

    async fn select_classes_for_user_and_date(
        date: &DateTime<Utc>,
        user: &User,
        state: &BotState,
        start_point: Option<DateTime<Utc>>,
    ) -> eyre::Result<Vec<Class>> {
        let mut final_query = mongodb::bson::Document::default();

        let group_constraints: Vec<_> = user
            .groups
            .iter()
            .map(|group| doc! {"groups.code": &group.code})
            .collect();
        final_query.extend(crate::db::create_range_query(&date, start_point).into_iter());
        final_query.extend(doc! {"$or": group_constraints}.into_iter());

        let mut class_query = state.classes_coll.find(final_query).await?;

        let mut selected_classes = Vec::default();

        while let Some(next_class) = class_query.next().await {
            selected_classes.push(next_class?);
        }

        selected_classes.sort_by(|first, second| first.range.start.cmp(&second.range.start));

        Ok(selected_classes)
    }

    fn format_shortform_class(user: &User, class: &Class) -> String {
        let localized_start = class.range.start.with_timezone(&crate::BOT_TIMEZONE);
        let localized_end = class.range.end.with_timezone(&crate::BOT_TIMEZONE);

        let start_time = localized_start.time().format("%H:%M").to_string();
        let end_time = localized_end.time().format("%H:%M").to_string();

        let duration = (localized_end - localized_start).num_minutes();

        t!(
            "lectures.format.short",
            locale = user.language.code(),
            code = class.code,
            from = start_time,
            until = end_time,
            duration_minutes = duration
        )
        .to_string()
    }

    fn format_shortform_classes(user: &User, classes: &[Class], kind: &str) -> String {
        let count_selector = format!("lectures.{kind}.ahead.");

        let count_line = match classes.is_empty() {
            true => t!(count_selector + "none", locale = user.language.code()),
            false => t!(
                count_selector + "some",
                count = classes.len(),
                locale = user.language.code()
            ),
        };

        let class_list = classes
            .iter()
            .map(|class| format_shortform_class(user, class))
            .fold(String::new(), |accum, current| {
                format!("{accum}\n{current}")
            });

        format!("{count_line}\n{class_list}")
    }

    async fn format_mainmenu(bot_state: &BotState, user: &User) -> eyre::Result<String> {
        let today_classes =
            select_classes_for_user_and_date(&Utc::now(), &user, &bot_state, Some(Utc::now()))
                .await?;
        let tomorrow_classes = select_classes_for_user_and_date(
            &(Utc::now().checked_add_days(Days::new(1)).unwrap()),
            &user,
            &bot_state,
            None,
        )
        .await?;

        let today_classes = format_shortform_classes(&user, &today_classes, "today");
        let tomorrow_classes = format_shortform_classes(&user, &tomorrow_classes, "tomorrow");

        let current_time = Utc::now().with_timezone(&crate::BOT_TIMEZONE).time();

        let greeting_kind = match current_time.hour() {
            12..18 => "afternoon",
            18..24 => "evening",
            _ => "morning",
        };

        let greeting = t!(
            format!("greeting.{}", greeting_kind),
            locale = user.language.code()
        );
        Ok(t!(
            "mainmenu.content",
            locale = user.language.code(),
            greeting = greeting,
            lectures_today = today_classes,
            lectures_tomorrow = tomorrow_classes
        )
        .to_string())
    }

    pub async fn main_menu(bot: Bot, bot_state: Arc<BotState>, user: User) -> HandlerResult {
        bot.send_message(user.id, format_mainmenu(&bot_state, &user).await?)
            .parse_mode(ParseMode::Html)
            .await?;
        Ok(())
    }
}
