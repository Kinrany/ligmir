mod character_sheet;
mod telegram;

use std::{error::Error, fmt::Display};

use character_sheet::Headless;
use rand::Rng;
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

#[derive(Debug)]
enum SkillCheckError {
	InvalidCharacterSheetUrl(String),
	EmptySkillList,
	DownloadError(failure::Error),
}

/// Used in error messages shown to the user.
impl Display for SkillCheckError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		use SkillCheckError::*;
		match self {
			InvalidCharacterSheetUrl(url) => write!(
				f,
				r#"I can't open "{}" as a charsheet link. It must start with "{}"."#,
				url, DNDBEYOND_HOST
			),
			EmptySkillList => write!(f, "Internal error: skill list is empty"),
			DownloadError(err) => {
				write!(f, "Failed to download modifiers: {}", err)
			}
		}
	}
}

impl Error for SkillCheckError {}

struct SkillCheckResponse {
	skill: String,
	modifier: i32,
	d20: i32,
}

impl Display for SkillCheckResponse {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let SkillCheckResponse {
			skill,
			modifier,
			d20,
		} = self;
		write!(
			f,
			"{} check: 🎲{} + {} = {}",
			skill,
			d20,
			modifier,
			d20 + modifier
		)
	}
}

static DEFAULT_CHARSHEET_URL: &'static str = "https://www.dndbeyond.com/characters/27570282/JhoG2D";

async fn handle_skill_check_request(
	headless: &Headless,
	request: &SkillCheckRequest,
) -> Result<SkillCheckResponse, SkillCheckError> {
	let charsheet_url = match request.charsheet_url {
		Some(ref url) if url.starts_with(DNDBEYOND_HOST) => Ok(url.as_str()),
		Some(ref url) => Err(SkillCheckError::InvalidCharacterSheetUrl(url.to_string())),
		None => Ok(DEFAULT_CHARSHEET_URL),
	}?;

	let character_sheet = headless
		.download_character_sheet(charsheet_url.to_string())
		.await
		.map_err(|err| SkillCheckError::DownloadError(err))?;

	let entered_skill_name = request.skill.as_deref().unwrap_or("Perception");

	let (skill, modifier) = character_sheet
		.skills
		.into_iter()
		.min_by_key(|(name, _)| edit_distance(name, entered_skill_name))
		.ok_or(SkillCheckError::EmptySkillList)?;

	let d20 = rand::thread_rng().gen_range(1..21);

	Ok(SkillCheckResponse {
		skill,
		modifier,
		d20,
	})
}

async fn handle_update(headless: &Headless, token: &str, update: Update) {
	if let Some(request) = parse_update(update, "@ligmir_bot") {
		let skill_check_response = handle_skill_check_request(headless, &request).await;
		let message = match skill_check_response {
			Ok(ok) => ok.to_string(),
			Err(err) => err.to_string(),
		};
		telegram::send_message(token, request.chat.id(), &message, request.message_id).await;
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
