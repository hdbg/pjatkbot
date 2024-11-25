use teloxide::{dispatching::dialogue::GetChatId, prelude::Requester, types::Message, Bot};

pub async fn send_disappering_message<'bot, Ret, Func>(
    bot: &'bot Bot,
    wait_delay: std::time::Duration,
    functor: Func,
) -> super::HandlerResult
where
    Ret: std::future::Future<Output = eyre::Result<Message>> + 'bot,
    Func: FnOnce(&'bot Bot) -> Ret,
{
    let sent_message = functor(bot).await?;

    tokio::time::sleep(wait_delay).await;

    bot.delete_message(sent_message.chat.id, sent_message.id)
        .await?;

    Ok(())
}
