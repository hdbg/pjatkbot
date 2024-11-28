use std::sync::Arc;

use dptree::filter_async;
use mongodb::Collection;
use notifications_sender::notifications_sender;
use slog::Logger;
use teloxide::{
    adaptors::DefaultParseMode,
    dispatching::{
        dialogue::{GetChatId, InMemStorage},
        DefaultKey,
    },
    prelude::*,
    types::ParseMode,
    utils::command::{self, BotCommands},
};
use tokio::sync::Mutex;

use crate::{
    channels::{self, DynTx, DynamicTx},
    db::{Model, User},
    notifications::{NotificationEvents, UpdateEvents},
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
    bot: Mutex<OurBot>,
    update_tx: DynamicTx<UpdateEvents>,

    pub config: &'static BotConfig,
    pub users_coll: Collection<User>,
    pub classes_coll: Collection<Class>,
    pub logger: Logger,
}
type DialogueStorage<State> = teloxide::dispatching::dialogue::InMemStorage<State>;
type BotDialogue<State> = teloxide::dispatching::dialogue::Dialogue<State, DialogueStorage<State>>;

type OurBot = DefaultParseMode<Bot>;

type HandlerResult = eyre::Result<()>;

type BotHandler = teloxide::dispatching::UpdateHandler<eyre::Report>;

fn create_storage<State>() -> Arc<DialogueStorage<State>> {
    InMemStorage::new()
}

#[rustfmt::skip]
fn build_main_handler_tree() -> BotHandler {
    dptree::entry()
        .branch(
            commands::handler()
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
        .chain(gui::user_onboard_dialog::handler())
}

fn setup_sender(state: &Arc<BotState>, notification_rx: impl channels::Rx<NotificationEvents>) {
    notifications_sender(Arc::downgrade(state), notification_rx);
}

pub fn setup_bot(
    config: &'static Config,
    logger: &Logger,
    db: &mongodb::Database,
    notification_rx: impl channels::Rx<NotificationEvents>,
    update_tx: DynamicTx<UpdateEvents>,
) -> Dispatcher<OurBot, eyre::Report, DefaultKey> {
    let users_coll = db.collection(&User::COLLECTION_NAME);
    let classes_coll = db.collection(&Class::COLLECTION_NAME);

    let logger = logger.new(slog::o!("subsystem" => "bot"));

    let bot = Bot::new(config.telegram.bot_token.clone()).parse_mode(ParseMode::Html);

    let state = Arc::new(BotState {
        bot: Mutex::new(bot.clone()),
        config: &config.telegram,
        users_coll,
        classes_coll,
        update_tx,
        logger,
    });

    setup_sender(&state, notification_rx);

    let mut dependencies = dptree::deps![state.clone()];
    dependencies.insert_container(gui::user_onboard_dialog::deps());

    Dispatcher::builder(bot, build_handler_tree())
        .enable_ctrlc_handler()
        .dependencies(dependencies)
        .build()
}

pub mod commands {
    use teloxide::{
        dispatching::{HandlerExt, UpdateFilterExt},
        dptree,
        macros::BotCommands,
        types::Update,
    };

    use super::gui;

    #[derive(BotCommands, Debug, Clone, PartialEq)]
    #[command(rename_rule = "snake_case")]
    pub enum UserCommands {
        Start,
    }

    pub fn handler() -> super::BotHandler {
        Update::filter_message()
            .filter_command::<UserCommands>()
            .branch(dptree::case![UserCommands::Start].endpoint(gui::main_menu))
    }
}

pub mod notifications_sender {
    use std::{collections::HashSet, sync::Weak};

    use chrono::{Datelike, Utc};
    use eyre::bail;
    use slog::Logger;
    use teloxide::{
        adaptors::DefaultParseMode, payloads::SendMessageSetters, prelude::Requester,
        types::ParseMode, Bot,
    };

    use super::{common::formatters::format_class_long, BotState, OurBot};
    use crate::{channels, db::UserID, notifications::NotificationEvents, parsing::types::Class};

    const RESEND_ATTEMPTS: usize = 10;

    async fn send_message_safe(
        bot: &OurBot,
        user: UserID,
        logger: &Logger,
        message: String,
    ) -> eyre::Result<()> {
        for _ in 0..RESEND_ATTEMPTS {
            let result = bot
                .send_message(user, &message)
                .parse_mode(ParseMode::Html)
                .await;
            match result {
                Err(teloxide::RequestError::RetryAfter(seconds)) => {
                    tokio::time::sleep(seconds.duration()).await;
                }
                Err(err) => {
                    slog::error!(logger, "notifications.handle_scheduled.safe_send"; "err" => ?err);
                    return Ok(());
                }
                Ok(_) => {
                    return Ok(());
                }
            }
        }
        bail!("resend attempts reached")
    }

    async fn handle_scheduled(state: &BotState, class: Class, user: UserID) -> eyre::Result<()> {
        let Some(user) = state
            .users_coll
            .find_one(mongodb::bson::doc! {"id": &user.0})
            .await?
        else {
            slog::error!(state.logger, "notifications.handle_scheduled.user_not_found"; "id" => ?user);
            return Ok(());
        };

        let in_minutes = (class.range.start - Utc::now()).num_minutes();

        let content = format_class_long(&class, &user.language);
        let content = t!(
            "notifications.class.start",
            locale = user.language.code(),
            minutes = in_minutes,
            content = content
        )
        .to_string();

        let bot = state.bot.lock().await;

        send_message_safe(&bot, user.telegram_id, &state.logger, content).await?;

        Ok(())
    }
    async fn handle_deleted(
        state: &BotState,
        class: Class,
        users: HashSet<UserID>,
    ) -> eyre::Result<()> {
        for user in users {
            let Some(user) = state
                .users_coll
                .find_one(mongodb::bson::doc! {"id": &user.0})
                .await?
            else {
                slog::error!(state.logger, "notifications.handle_deleted.user_not_found"; "id" => ?user);
                return Ok(());
            };

            let content = format_class_long(&class, &user.language);
            let content = t!(
                "notifications.class.cancelled",
                locale = user.language.code(),
                content = content
            )
            .to_string();

            let bot = state.bot.lock().await;

            send_message_safe(&bot, user.telegram_id, &state.logger, content).await?;
        }

        Ok(())
    }

    pub fn notifications_sender(
        state: Weak<BotState>,
        notification_rx: impl channels::Rx<NotificationEvents>,
    ) -> tokio::task::JoinHandle<eyre::Result<()>> {
        let fut = async move {
            loop {
                let new_notifications = notification_rx.recv().await?;

                let Some(current_state) = state.upgrade() else {
                    // meaning all the other modules have exited
                    return Ok(());
                };

                for notification in new_notifications.into_iter() {
                    match notification {
                        crate::notifications::NotificationEvent::ClassDeleted {
                            class,
                            affected_users,
                        } => handle_deleted(&current_state, class, affected_users).await?,
                        crate::notifications::NotificationEvent::Scheduled { class, user_id } => {
                            handle_scheduled(&current_state, class, user_id).await?;
                        }
                    }
                }
            }
        };

        tokio::spawn(fut)
    }
}

pub mod common {

    pub mod formatters {
        use crate::{
            db::Language,
            parsing::types::{Class, ClassPlace},
        };

        fn format_place(place: &ClassPlace, lang: &Language) -> String {
            let place = match place {
                crate::parsing::types::ClassPlace::Online => {
                    t!("classes.place.online", locale = lang.code()).to_string()
                }
                crate::parsing::types::ClassPlace::OnSite { room } => room.trim().to_owned(),
            };

            format!("{:<7}", "(".to_owned() + &place + ")")
        }

        fn format_timerange(class: &Class) -> (String, String) {
            let localized_start = class.range.start.with_timezone(&crate::BOT_TIMEZONE);
            let localized_end = class.range.end.with_timezone(&crate::BOT_TIMEZONE);

            let start_time = localized_start.time().format("%H:%M").to_string();
            let end_time = localized_end.time().format("%H:%M").to_string();

            (start_time, end_time)
        }

        fn format_kind(class: &Class, lang: &Language) -> String {
            let kind = format!("classes.type.{}", class.kind.to_string());
            let kind = t!(kind, locale = lang.code());
            kind.to_string()
        }

        pub fn format_class_long(class: &Class, lang: &Language) -> String {
            let (from, to) = format_timerange(&class);
            t!(
                "classes.format.long",
                locale = lang.code(),
                name = &class.name,
                from = from,
                to = to,
                class_type = format_kind(class, lang),
                lecturer = &class.lecturer,
                place = format_place(&class.place, &lang)
            )
            .to_string()
        }

        pub fn format_class_short(class: &Class, lang: &Language) -> String {
            let (from, to) = format_timerange(class);
            t!(
                "classes.format.short",
                locale = lang.code(),
                code = format!("{:<4}", class.code),
                from = from,
                until = to,
                kind = format_kind(class, lang),
                place = format_place(&class.place, lang)
            )
            .to_string()
        }
    }
}

pub mod gui {
    use std::sync::Arc;

    use bson::doc;
    use chrono::{DateTime, Days, Timelike, Utc};
    use futures::StreamExt;
    use teloxide::{payloads::SendMessageSetters, prelude::Requester, types::ParseMode, Bot};

    use crate::{bot::common::formatters::format_class_short, db::User, parsing::types::Class};

    use super::{BotState, HandlerResult, OurBot};

    pub mod user_onboard_dialog;

    use crate::BOT_TIMEZONE;

    async fn select_classes_for_user_and_date(
        date: &DateTime<Utc>,
        user: &User,
        state: &BotState,
        end_point: Option<DateTime<Utc>>,
    ) -> eyre::Result<Vec<Class>> {
        // fix for considering days in user's timezone
        let date = date.with_timezone(&BOT_TIMEZONE);
        let start_point = end_point.map(|date| date.with_timezone(&BOT_TIMEZONE));

        let mut final_query = mongodb::bson::Document::default();

        let group_constraints: Vec<_> = user
            .groups
            .iter()
            .map(|group| doc! {"groups": &group.code})
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

    fn format_shortform_classes(user: &User, classes: &[Class], kind: &str) -> String {
        let count_selector = format!("classes.{kind}.ahead.");

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
            .map(|class| format_class_short(class, &user.language))
            .fold(String::new(), |accum, current| {
                format!("{accum}{current}\n")
            });

        format!("{count_line}\n<pre>{class_list}</pre>")
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

    pub async fn main_menu(bot: OurBot, bot_state: Arc<BotState>, user: User) -> HandlerResult {
        bot.send_message(user.telegram_id, format_mainmenu(&bot_state, &user).await?)
            .parse_mode(ParseMode::Html)
            .await?;
        Ok(())
    }
}
