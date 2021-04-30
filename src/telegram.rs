use rocket::futures::TryFutureExt;
use serde::Serialize;
use telegram_bot::{ChatId, MessageId};
use url::Url;

#[derive(Serialize)]
struct SendMessage {
	chat_id: ChatId,
	text: String,
	reply_to_message_id: MessageId,
}

pub async fn send_message(token: &str, chat_id: ChatId, message: &str, reply_to: MessageId) {
	let query = serde_urlencoded::to_string(SendMessage {
		chat_id,
		text: message.to_string(),
		reply_to_message_id: reply_to,
	});
	let query = match query {
		Ok(query) => query,
		Err(err) => {
			println!("Failed to serialize message: {}", err);
			return;
		}
	};

	let mut url: Url = format!("https://api.telegram.org/bot{}/sendMessage", token)
		.parse()
		.unwrap();
	url.set_query(Some(&query));

	let response = reqwest::get(url).and_then(|response| response.text()).await;
	if let Err(err) = response {
		println!(
			r#"Failed to send message "{}" to user {} in chat {}: {}"#,
			message, reply_to, chat_id, err
		);
	}
}
