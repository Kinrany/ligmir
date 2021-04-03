mod character_sheet;
mod telegram;

use character_sheet::Headless;
use rocket::{get, launch, post, routes, tokio, Rocket, State};
use rocket_contrib::json::Json;
use strsim::damerau_levenshtein as edit_distance;
use telegram_bot::{Message, MessageChat, MessageId, MessageKind, Update, UpdateKind};

struct SkillCheckRequest {
	chat: MessageChat,
	message_id: MessageId,
	skill: Option<String>,
	charsheet_url: Option<String>,
}

fn parse_update(update: Update, bot_name: &str) -> Option<SkillCheckRequest> {
	match update {
		Update {
			kind:
				UpdateKind::Message(Message {
					chat,
					id,
					kind: MessageKind::Text { data: text, .. },
					..
				}),
			..
		} => {
			let args = text
				.split_whitespace()
				.filter(|&s| s != bot_name)
				.take(2)
				.collect::<Vec<_>>();
			let skill = args.get(0).map(ToString::to_string);
			let charsheet_url = args.get(1).map(ToString::to_string);

			Some(SkillCheckRequest {
				chat,
				message_id: id,
				skill,
				charsheet_url,
			})
		}
		_ => None,
	}
}

static DNDBEYOND_HOST: &'static str = "https://www.dndbeyond.com/";

enum SkillCheckResponse {
	Ok { name: String, modifier: i32 },
	InvalidCharacterSheetUrl(String),
	EmptySkillList,
	DownloadError(failure::Error),
}

impl SkillCheckResponse {
	fn message(&self) -> String {
		use SkillCheckResponse::*;
		match self {
			Ok { name, modifier } => format!("{}: {}", name, modifier),
			InvalidCharacterSheetUrl(url) => format!(
				r#"I can't open "{}" as a charsheet link. It must start with "{}"."#,
				url, DNDBEYOND_HOST
			),
			EmptySkillList => "Internal error: skill list is empty".to_string(),
			DownloadError(err) => {
				format!("Failed to download modifiers: {}", err)
			}
		}
	}
}

async fn handle_skill_check_request(
	headless: &Headless,
	request: &SkillCheckRequest,
) -> SkillCheckResponse {
	let charsheet_url = match &request.charsheet_url {
		Some(url) => {
			if !url.starts_with(DNDBEYOND_HOST) {
				return SkillCheckResponse::InvalidCharacterSheetUrl(url.to_string());
			}
			url
		}
		None => "https://www.dndbeyond.com/characters/27570282/JhoG2D",
	};

	let character_sheet_result = headless
		.download_character_sheet(charsheet_url.to_string())
		.await;

	match character_sheet_result {
		Ok(character_sheet) => {
			let entered_skill_name = request.skill.as_deref().unwrap_or("Perception");
			let skill_with_closest_name = character_sheet
				.skills
				.into_iter()
				.min_by_key(|(name, _)| edit_distance(name, entered_skill_name));
			match skill_with_closest_name {
				None => SkillCheckResponse::EmptySkillList,
				Some((name, modifier)) => SkillCheckResponse::Ok { name, modifier },
			}
		}
		Err(err) => SkillCheckResponse::DownloadError(err),
	}
}

async fn handle_update(headless: &Headless, token: &str, update: Update) {
	if let Some(request) = parse_update(update, "@ligmir_bot") {
		let skill_check_response = handle_skill_check_request(headless, &request).await;
		telegram::send_message(
			token,
			request.chat.id(),
			&skill_check_response.message(),
			request.message_id,
		)
		.await;
	}
}

#[get("/health")]
fn health() -> &'static str {
	"OK"
}

#[post(
	"/telegram/update/<token>",
	format = "application/json",
	data = "<update>"
)]
async fn telegram_update<'a>(token: String, update: Json<Update>, headless: State<'a, Headless>) {
	let update = update.0;

	println!("Received update: {:?}", update);

	print!("Spawning thread...");
	let headless = (*headless).clone();
	tokio::spawn(async move {
		handle_update(&headless, &token, update).await;
	});
	println!("success.");
}

#[launch]
fn rocket() -> Rocket {
	rocket::ignite()
		.manage(Headless {
			service_url: std::env::var("LIGMIR_BROWSER_URL").expect("Expected LIGMIR_BROWSER_URL"),
			timeout: std::env::var("LIGMIR_BROWSER_TIMEOUT")
				.expect("Expected LIGMIR_BROWSER_TIMEOUT")
				.parse()
				.expect("Cannot parse LIGMIR_BROWSER_TIMEOUT"),
		})
		.mount("/", routes![health, telegram_update])
}
