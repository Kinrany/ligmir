use rocket::futures::TryFutureExt;
use telegram_bot::{ChatId, MessageId};

pub async fn send_message(token: &str, chat_id: ChatId, message: &str, reply_to: MessageId) {
	let result = reqwest::get(&format!(
		"https://api.telegram.org/bot{}/sendMessage?chat_id={}&text={}&reply_to_message_id={}",
		token, chat_id, message, reply_to
	))
	.and_then(|response| response.text())
	.await;

	if let Err(err) = result {
		println!(
			r#"Failed to send message "{}" to user {} in chat {}: {}"#,
			message, reply_to, chat_id, err
		);
	}
}
