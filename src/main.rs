use failure::{err_msg, Fallible};
use headless_chrome::{protocol::target::methods::CreateTarget, Browser};
use rocket::{
	futures::TryFutureExt, get, http::Status, launch, post, routes, tokio, Rocket, State,
};
use rocket_contrib::json::Json;
use strsim::damerau_levenshtein as edit_distance;
use teloxide::types::{
	Chat, MediaKind, MediaText, Message, MessageCommon, MessageKind, Update, UpdateKind,
};

struct SkillCheckRequest {
	chat: Chat,
	message_id: i32,
	skill: Option<String>,
	charsheet_url: Option<String>,
}

async fn send_message(token: &str, chat_id: i64, message: &str, reply_to: i32) -> Fallible<()> {
	reqwest::get(&format!(
		"https://api.telegram.org/bot{}/sendMessage?chat_id={}&text={}&reply_to_message_id={}",
		token, chat_id, message, reply_to
	))
	.and_then(|response| response.text())
	.await?;

	Ok(())
}

enum Request {
	SkillCheckRequest(SkillCheckRequest),
}

#[derive(Clone, Debug)]
struct Config {
	browser_url: String,
	browser_timeout: u64,
}

impl Config {
	fn download_skill_modifiers(self, url: &str) -> Fallible<Vec<(String, i32)>> {
		let browser = Browser::connect(self.browser_url)?;

		let tab = browser.new_tab_with_options(CreateTarget {
			url,
			width: None,
			height: None,
			browser_context_id: None,
			enable_begin_frame_control: None,
		})?;

		// Wait for network/javascript/dom to make the skill list available
		let element = tab.wait_for_element_with_custom_timeout(
			"div.ct-skills",
			std::time::Duration::from_secs(self.browser_timeout),
		)?;

		// Parse the skill list
		let skills = element
			.call_js_fn(
				r#"
				function() {
					const items = this.querySelectorAll(".ct-skills__item");
					const skillValues = [...items].map(item => {
						const skill = item.querySelector(".ct-skills__col--skill");
						const modifier = item.querySelector(".ct-skills__col--modifier");
						return [skill, modifier];
					});
					const text = skillValues
						.map(([skill, modifier]) => `${skill.innerText},${modifier.innerText.replace("\n", "")}`)
						.join(";");
					return text;
				}"#,
				true,
			)?
			.value
			.ok_or(err_msg("Function did not return a value"))?
			.to_string()
			.replace("\"", "")
			.split(";")
			.map(
				|s| match s.split(",").take(2).collect::<Vec<&str>>().as_slice() {
					[a, b, ..] => Ok(((*a).to_owned(), b.parse::<i32>()?)),
					_ => {
						let message =
							format!("Cannot parse string \"{}\" into skill name and modifier", s);
						Err(err_msg(message))
					}
				},
			)
			.collect::<Fallible<Vec<(String, i32)>>>()?;

		Ok(skills)
	}

	async fn handle_skill_check_request(self, token: &str, request: SkillCheckRequest) {
		let charsheet_url = match request.charsheet_url {
			Some(some_url) => {
				let origin = "https://www.dndbeyond.com/";
				if !some_url.starts_with(origin) {
					let message = format!(
						r#"I can't open "{}" as a charsheet link. It must start with "{}"."#,
						some_url, origin
					);
					if let Err(err) =
						send_message(token, request.chat.id, &message, request.message_id).await
					{
						println!("Failed to warn the user about invalid URL: {}", err);
					}
					return;
				}
				some_url
			}
			None => "https://www.dndbeyond.com/characters/27570282/JhoG2D".to_string(),
		};

		let result = tokio::task::spawn_blocking(move || {
			println!("Beginning to download");
			let skills = self.download_skill_modifiers(&charsheet_url);
			println!("Finished downloading");
			skills
		})
		.await;

		let message = match result {
			Ok(Ok(skills)) => {
				let entered_skill_name = request.skill.unwrap_or("Perception".to_string());
				let skill_with_closest_name = skills
					.into_iter()
					.min_by_key(|(name, _)| edit_distance(name, &entered_skill_name));
				match skill_with_closest_name {
					None => "Internal error: skill list is empty".to_string(),
					Some((name, modifier)) => format!("{}: {}", name, modifier),
				}
			}
			Ok(Err(err)) => format!("Failed to download modifiers: {}", err),
			Err(err) => format!("JoinError: {}", err),
		};

		if let Err(err) = send_message(token, request.chat.id, &message, request.message_id).await {
			println!("Failed to send telegram message: {}", err);
		}
	}

	async fn handle_update(self, token: String, update: Update) {
		let bot_name = "@ligmir_bot";

		let request = match update {
		Update {
			kind:
				UpdateKind::Message(Message {
					chat,
					id,
					kind:
						MessageKind::Common(MessageCommon {
							media_kind: MediaKind::Text(MediaText { text, .. }),
							..
						}),
					..
				}),
			..
		}
		// Negative `chat.id` = private message
		if text.starts_with(bot_name) || chat.id < 0 => {
			let args = text.split_whitespace().filter(|&s| s != bot_name).take(2).collect::<Vec<_>>();
			let skill = args.get(0).map(ToString::to_string);
			let charsheet_url = args.get(1).map(ToString::to_string);

			Request::SkillCheckRequest(SkillCheckRequest {
				chat,
				message_id: id,
				skill,
				charsheet_url,
			})
		}
		_ => {
			println!("Ignoring update.");
			return;
		},
	};

		match request {
			Request::SkillCheckRequest(skill_check_request) => {
				self.handle_skill_check_request(&token, skill_check_request)
					.await
			}
		}
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
async fn telegram_update<'a>(token: String, update: Json<Update>, config: State<'a, Config>) {
	let update = update.0;

	println!("Received update: {:?}", update);

	print!("Spawning thread...");
	let config = (*config).clone();
	tokio::spawn(async move {
		config.handle_update(token, update).await;
	});
	println!("success.");
}

#[get("/telegram/setwebhook?<token>&<host>")]
async fn telegram_setwebhook(token: String, host: String) -> Result<String, Status> {
	let update_url = format!("https://{}/telegram/update/{}", host, token);
	let telegram_setwebhook_request_url = format!(
		"https://api.telegram.org/bot{}/setWebhook?url={}",
		token, update_url
	);

	reqwest::get(&telegram_setwebhook_request_url)
		.and_then(|response| response.text())
		.await
		.map_err(|err| {
			println!("setwebhook error: {}", err);
			Status::InternalServerError
		})
}

#[launch]
fn rocket() -> Rocket {
	rocket::ignite()
		.manage(Config {
			browser_url: std::env::var("LIGMIR_BROWSER_URL").expect("Expected LIGMIR_BROWSER_URL"),
			browser_timeout: std::env::var("LIGMIR_BROWSER_TIMEOUT")
				.expect("Expected LIGMIR_BROWSER_TIMEOUT")
				.parse()
				.expect("Cannot parse LIGMIR_BROWSER_TIMEOUT"),
		})
		.mount("/", routes![health, telegram_update, telegram_setwebhook])
}
