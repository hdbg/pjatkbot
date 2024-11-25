use std::{any::Any, str::FromStr, sync::Arc};

use eyre::OptionExt;
use rust_i18n::t;
use slog::Logger;
use strum::IntoEnumIterator;
use teloxide::{
    dispatching::{
        dialogue::{GetChatId, InMemStorage},
        HandlerExt, UpdateFilterExt, UpdateHandler,
    },
    dptree,
    payloads::{EditMessageTextSetters, SendMessageSetters},
    prelude::{DependencyMap, Requester},
    types::{
        CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup,
        MaybeInaccessibleMessage, Message, MessageId, ReplyMarkup, Update, UserId,
    },
    Bot,
};

use crate::{
    db::{Language, NotificationConstraint},
    parsing::types::Group,
};

use super::{BotDialogue, BotState, HandlerResult};

#[derive(strum::EnumIter, strum::Display, strum::EnumString, Clone)]
pub enum Notification {
    #[strum(serialize = "nothing")]
    No,
    #[strum(serialize = "_10mins")]
    _10Mins,
    #[strum(serialize = "_30mins")]
    _30Mins,
    #[strum(serialize = "_1hour")]
    _1Hour,
    #[strum(serialize = "_2hours")]
    _2Hours,
}

impl Notification {
    fn constraint(self) -> Option<NotificationConstraint> {
        let duration = match self {
            Notification::No => None,
            Notification::_10Mins => Some(std::time::Duration::from_secs(10 * 60)),
            Notification::_30Mins => Some(std::time::Duration::from_secs(30 * 60)),
            Notification::_1Hour => Some(std::time::Duration::from_secs(60 * 60)),
            Notification::_2Hours => Some(std::time::Duration::from_secs(120 * 60)),
        };

        duration.map(NotificationConstraint)
    }
}

#[derive(Default, Clone)]
pub enum Stages {
    #[default]
    Start,
    WaitingForLanguage,
    WaitingForGroups {
        language: Language,
    },
    WaitingForNotifications {
        groups: Vec<Group>,
        language: Language,
    },
    // ReceivedNotification {
    //     language: Language,
    //     groups: Vec<Group>,
    //     notifications: Notification,
    // },
}

pub fn deps() -> DependencyMap {
    teloxide::dptree::deps![super::create_storage::<Stages>()]
}

#[rustfmt::skip]
    pub fn handler() -> UpdateHandler<eyre::Report> {
        dptree::entry()
            .enter_dialogue::<Update, super::DialogueStorage<Stages>, Stages>()
            .branch(
                Update::filter_callback_query()
                    .branch(dptree::case![Stages::WaitingForLanguage].endpoint(handlers::handle_language_selection))
                    .branch(dptree::case![Stages::WaitingForNotifications { groups, language }].endpoint(handlers::handle_notifications_choice))
            )

            .branch(
                Update::filter_message()
                    .branch(dptree::case![Stages::Start].endpoint(entrypoint))
                    .branch(dptree::case![Stages::WaitingForGroups {language}].endpoint(handlers::handle_group_selection))    
            )
    }

fn format_notifications_keyboard() -> InlineKeyboardMarkup {
    let buttons = Notification::iter().map(|notification_type| {
        vec![InlineKeyboardButton {
            text: t!(format!("onboarding.notifications.{}", notification_type)).to_string(),
            kind: teloxide::types::InlineKeyboardButtonKind::CallbackData(
                notification_type.to_string(),
            ),
        }]
    });

    InlineKeyboardMarkup {
        inline_keyboard: buttons.collect(),
    }
}

fn format_languages_keyboard() -> InlineKeyboardMarkup {
    let buttons = Language::iter().map(|lang| {
        vec![InlineKeyboardButton {
            text: t!(format!("onboarding.language_{}", lang), locale = "en").to_string(),
            kind: teloxide::types::InlineKeyboardButtonKind::CallbackData(lang.to_string()),
        }]
    });

    InlineKeyboardMarkup {
        inline_keyboard: buttons.collect(),
    }
}

pub async fn entrypoint(
    bot: Bot,
    user_id: ChatId,
    dialogue: super::BotDialogue<Stages>,
    state: Arc<BotState>,
) -> super::HandlerResult {
    println!("{:#?}", format_languages_keyboard());
    bot.send_message(user_id, t!("onboarding.language.title", locale = "en"))
        .reply_markup(format_languages_keyboard())
        .await?;

    dialogue.update(Stages::WaitingForLanguage).await?;

    slog::info!(state.logger, "onboard.start"; "user" => ?user_id);

    Ok(())
}

mod senders {
    use teloxide::{
        payloads::{EditMessageTextSetters, SendMessageSetters},
        prelude::Requester,
        types::{ChatId, InlineKeyboardMarkup, MaybeInaccessibleMessage, UserId},
        Bot,
    };

    use crate::{bot::HandlerResult, db::Language};

    use super::{format_languages_keyboard, format_notifications_keyboard, Stages};

    pub async fn send_groups_selection(
        bot: Bot,
        user_id: ChatId,
        msg_id: MaybeInaccessibleMessage,
        language: &Language,
    ) -> HandlerResult {
        let content = t!("onboarding.groups.prompt", locale = language.code());

        match msg_id {
            MaybeInaccessibleMessage::Inaccessible(_) => bot.send_message(user_id, content).await?,
            MaybeInaccessibleMessage::Regular(msg) => {
                bot.edit_message_text(user_id, msg.id, content)
                    .reply_markup(InlineKeyboardMarkup::default())
                    .await?
            }
        };

        Ok(())
    }

    pub async fn send_notifications_prompt(
        bot: Bot,
        user_id: ChatId,
        language: &Language,
    ) -> HandlerResult {
        let languages_keyboard = format_notifications_keyboard();

        let prompt = t!("onboarding.notifications.prompt", locale = language.code());

        bot.send_message(user_id, prompt)
            .reply_markup(languages_keyboard)
            .await?;

        Ok(())
    }
}

mod handlers {
    use std::{str::FromStr, sync::Arc};

    use bson::doc;
    use teloxide::{
        prelude::Requester,
        types::{CallbackQuery, Message},
        Bot,
    };

    use crate::{
        bot::{utils::send_disappering_message, BotDialogue, BotState, HandlerResult},
        db::{self, Language},
        parsing::types::Group,
    };

    use super::{senders, Notification, Stages};

    pub async fn handle_language_selection(
        bot: Bot,
        state: Arc<BotState>,
        answer: CallbackQuery,
        dialogue: BotDialogue<Stages>,
    ) -> super::HandlerResult {
        println!("loool");
        // let logger = Logger::new(&state.logger, slog::o!())
        let Some(callback_data) = answer.data else {
            slog::warn!(state.logger, "onboarding.handle_language_selection"; "error" => "received language selection answer without callback");
            return Ok(());
        };

        let Ok(language) = Language::from_str(&callback_data) else {
            slog::warn!(state.logger, "onboarding.handle_language_selection"; "error" => "couldn't parse selected language", "data" => callback_data);
            return Ok(());
        };

        let Some(message) = answer.message else {
            slog::warn!(state.logger, "onboarding.handle_language_selection"; "error" => "message wasn't present", );
            return Ok(());
        };
        super::senders::send_groups_selection(bot, answer.from.id.into(), message, &language)
            .await?;
        dialogue
            .update(Stages::WaitingForGroups { language })
            .await?;

        slog::trace!(state.logger, "onboarding.handle_language_selection"; "event" => "selected");

        Ok(())
    }

    pub async fn handle_group_selection(
        bot: Bot,
        dialogue: BotDialogue<Stages>,
        state: Arc<BotState>,
        message: Message,
        language: Language,
    ) -> super::HandlerResult {
        let Some(msg_text) = message.text() else {
            bot.send_message(message.chat.id, "Internal error").await?;
            return Ok(());
        };

        let group_chunks: Vec<_> = msg_text
            .split("\n")
            .map(|group_code| Group {
                code: group_code.to_owned(),
            })
            .collect();

        // check if such groups exist
        for group in group_chunks.iter() {
            let class_test_query = doc! {"groups.code": &group.code};
            let query = state.classes_coll.find_one(class_test_query).await?;

            if query.is_none() {
                bot.send_message(
                    message.chat.id,
                    t!(
                        "onboarding.groups.error",
                        group = &group.code,
                        locale = &language.code()
                    ),
                )
                .await?;
                return Ok(());
            }
        }

        senders::send_notifications_prompt(bot, message.chat.id, &language).await?;

        dialogue
            .update(Stages::WaitingForNotifications {
                groups: group_chunks,
                language,
            })
            .await?;

        Ok(())
    }

    pub async fn handle_notifications_choice(
        (groups, language): (Vec<Group>, Language),
        state: Arc<BotState>,
        answer: CallbackQuery,
        dialogue: BotDialogue<Stages>,
    ) -> HandlerResult {
        let Some(answer_data) = answer.data else {
            slog::error!(state.logger, "onboard.handle_notification_choice"; "err" => "haven't received callback data");
            return Ok(());
        };

        let Ok(notification_choice) = Notification::from_str(&answer_data) else {
            slog::error!(state.logger, "onboard.handle_notification_choice"; "err" => "couldn't parse choice");
            return Ok(());
        };

        let constraints = match notification_choice.constraint() {
            Some(constraint) => vec![constraint],
            None => vec![],
        };

        state
            .users_coll
            .insert_one(db::User {
                id: answer.from.id.into(),
                role: db::Role::User,
                groups,
                language,
                constraints,
            })
            .await?;

        slog::info!(state.logger, "onboard.succ_registered"; "userid" => ?answer.from.id);

        dialogue.exit().await?;

        Ok(())
    }
}
