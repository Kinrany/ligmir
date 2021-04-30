mod character_sheet;
mod telegram;

use std::{convert::TryFrom, env, fmt::Display};

use anyhow::anyhow;
use character_sheet::Headless;
use lazy_static::lazy_static;
use rand::Rng;
use redis::{AsyncCommands, Client as Redis, FromRedisValue, ToRedisArgs};
use regex::Regex;
use rocket::{get, launch, post, routes, tokio, Rocket, State};
use rocket_contrib::json::Json;
use strsim::damerau_levenshtein as edit_distance;
use telegram_bot::{ChatId, Message, MessageId, MessageKind, Update, UpdateKind, User, UserId};
use url::Url;

struct RequestSource {
	chat_id: ChatId,
	message_id: MessageId,
	user_id: UserId,
}

impl RequestSource {
	async fn respond(&self, token: &str, message: &str) {
		telegram::send_message(token, self.chat_id, message, self.message_id).await;
	}
}

struct SkillCheckRequest {
	source: RequestSource,
	skill: String,
}

struct SetCharacterRequest {
	source: RequestSource,
	character_id: CharacterId,
}

enum BotCommand {
	SkillCheck(SkillCheckRequest),
	SetCharacter(SetCharacterRequest),
	Unknown,
	Error {
		source: RequestSource,
		error: String,
	},
}

impl From<Update> for BotCommand {
	fn from(update: Update) -> Self {
		match update {
			Update {
				kind:
					UpdateKind::Message(Message {
						chat,
						id: message_id,
						from: User { id: user_id, .. },
						kind: MessageKind::Text { data, .. },
						..
					}),
				..
			} => {
				let source = RequestSource {
					chat_id: chat.id(),
					message_id,
					user_id,
				};
				if data.starts_with("/skill") {
					BotCommand::SkillCheck(SkillCheckRequest {
						source,
						// skip the first 7 characters matching "/skill "
						skill: data[7..data.len()].to_string(),
					})
				} else if data.starts_with("/character") {
					let character_id = match CharacterId::try_from(&data[11..data.len()]) {
						Ok(character_id) => character_id,
						Err(err) => {
							return BotCommand::Error {
								source,
								error: err.to_string(),
							}
						}
					};
					BotCommand::SetCharacter(SetCharacterRequest {
						source,
						character_id,
					})
				} else {
					BotCommand::Unknown
				}
			}
			_ => BotCommand::Unknown,
		}
	}
}

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct CharacterId(i64);

impl TryFrom<&str> for CharacterId {
	type Error = anyhow::Error;

	fn try_from(url: &str) -> Result<Self, Self::Error> {
		lazy_static! {
			// List of regexes that capture character ID in a group named "id"
			static ref PATTERNS: Vec<Regex> =
				vec![
					Regex::new(r"^https://www.dndbeyond.com/(?:profile/[[:alnum:]]+/)?characters/(?P<id>\d+)").unwrap(),
				];
		}

		for pattern in PATTERNS.iter() {
			if let Some(captures) = pattern.captures(url) {
				if let Some(id_match) = captures.name("id") {
					let character_id = id_match.as_str().parse()?;
					return Ok(CharacterId(character_id));
				}
			}
		}

		Err(anyhow!("Expected a character sheet URL."))
	}
}

impl ToRedisArgs for CharacterId {
	fn write_redis_args<W>(&self, out: &mut W)
	where
		W: ?Sized + redis::RedisWrite,
	{
		self.0.write_redis_args(out)
	}
}

impl FromRedisValue for CharacterId {
	fn from_redis_value(v: &redis::Value) -> redis::RedisResult<Self> {
		i64::from_redis_value(v).map(CharacterId)
	}
}

impl Display for CharacterId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
		self.0.fmt(f)
	}
}

/// Sample character: https://www.dndbeyond.com/characters/36535842
const DEFAULT_CHARACTER_ID: CharacterId = CharacterId(36535842);

static REDIS_KEY_TELEGRAM_USER_CHARSHEET_URL: &str = "TELEGRAM_USER_CHARSHEET_URL";

fn character_sheet_url(character_id: CharacterId) -> Url {
	let base = Url::parse("https://www.dndbeyond.com/characters/").unwrap();
	base.join(&character_id.to_string()).unwrap()
}

async fn handle_skill_check_request(
	context: &Context,
	request: &SkillCheckRequest,
) -> Result<SkillCheckResponse, anyhow::Error> {
	let mut redis_conn = context.redis.get_async_connection().await?;

	let saved_character_id: Option<CharacterId> = redis_conn
		.get((
			REDIS_KEY_TELEGRAM_USER_CHARSHEET_URL,
			&request.source.user_id.to_string(),
		))
		.await?;

	let character_id = saved_character_id.unwrap_or(DEFAULT_CHARACTER_ID);

	let character_sheet = context
		.headless
		.download_character_sheet(character_sheet_url(character_id))
		.await
		.map_err(|_| anyhow!("Failed to download modifiers"))?;

	let (skill, modifier) = character_sheet
		.skills
		.into_iter()
		.min_by_key(|(name, _)| edit_distance(name, &request.skill))
		.ok_or_else(|| anyhow!("Internal error: skill list is empty"))?;

	let d20 = rand::thread_rng().gen_range(1..21);

	Ok(SkillCheckResponse {
		skill,
		modifier,
		d20,
	})
}

struct SetCharacterResponse;

impl Display for SetCharacterResponse {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "Will do!")
	}
}

async fn handle_set_character_request(
	context: &Context,
	request: &SetCharacterRequest,
) -> Result<SetCharacterResponse, anyhow::Error> {
	let mut redis_conn = context.redis.get_async_connection().await?;

	let key = (
		REDIS_KEY_TELEGRAM_USER_CHARSHEET_URL,
		&request.source.user_id.to_string(),
	);
	redis_conn.set(key, request.character_id).await?;

	Ok(SetCharacterResponse)
}

fn response_to_string<T>(response: Result<T, anyhow::Error>) -> String
where
	T: Display,
{
	match response {
		Ok(ok) => ok.to_string(),
		Err(err) => {
			println!("Internal error: {}", err);
			"Sorry, boss, I can't do that.".to_string()
		}
	}
}

async fn handle_update(context: &Context, token: &str, update: Update) {
	let response = match update.into() {
		BotCommand::SkillCheck(request) => {
			let response = handle_skill_check_request(context, &request).await;
			Some((request.source, response_to_string(response)))
		}
		BotCommand::SetCharacter(request) => {
			let response = handle_set_character_request(context, &request).await;
			Some((request.source, response_to_string(response)))
		}
		BotCommand::Unknown => None,
		BotCommand::Error { source, error } => Some((source, error)),
	};

	if let Some((source, message)) = response {
		source.respond(token, &message).await;
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
async fn telegram_update<'a>(token: String, update: Json<Update>, context: State<'_, Context>) {
	let update = update.0;

	println!("Received update: {:?}", update);

	print!("Spawning thread...");
	let context = (*context).clone();
	tokio::spawn(async move {
		handle_update(&context, &token, update).await;
	});
	println!("success.");
}

#[derive(Clone, Debug)]
struct Context {
	redis: Redis,
	headless: Headless,
}

#[launch]
fn rocket() -> Rocket {
	rocket::ignite()
		.manage(Context {
			redis: Redis::open(env::var("LIGMIR_REDIS_URL").expect("Expected LIGMIR_REDIS_URL"))
				.expect("Failed to initialize Redis client"),
			headless: Headless {
				service_url: env::var("LIGMIR_BROWSER_URL").expect("Expected LIGMIR_BROWSER_URL"),
				timeout: env::var("LIGMIR_BROWSER_TIMEOUT")
					.expect("Expected LIGMIR_BROWSER_TIMEOUT")
					.parse()
					.expect("Cannot parse LIGMIR_BROWSER_TIMEOUT"),
			},
		})
		.mount("/", routes![health, telegram_update])
}

#[cfg(test)]
mod tests {
	use super::CharacterId;
	use std::convert::TryFrom;

	#[test]
	fn parse_character_id_from_str() {
		let url = "https://www.dndbeyond.com/characters/36535842/";
		assert_eq!(CharacterId::try_from(url).unwrap(), CharacterId(36535842));
	}
}
