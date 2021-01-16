use failure::{err_msg, Fallible};
use headless_chrome::{protocol::target::methods::CreateTarget, Browser};
use rocket::{futures::TryFutureExt, get, http::Status, launch, post, routes, tokio, Rocket};
use rocket_contrib::json::Json;
use teloxide::types::{Message, Update, UpdateKind};

fn download_skill_modifiers(url: &str) -> Fallible<Vec<(String, i32)>> {
	let browser = if let Ok(url) = std::env::var("LIGMIR_BROWSER_URL") {
		Browser::connect(url)?
	} else {
		Browser::default()?
	};

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
		std::time::Duration::from_secs(10),
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

#[get("/health")]
fn health() -> &'static str {
	"OK"
}

#[tokio::main]
async fn handle_update(token: String, update: Update) {
	println!("Spawning thread");

	let (chat, reply_to) = match update {
		Update {
			kind: UpdateKind::Message(Message { chat, id, .. }),
			..
		} => (chat, id),
		_ => return,
	};

	let result = tokio::task::spawn_blocking(|| {
		println!("Beginning to download");
		let skills =
			download_skill_modifiers("https://www.dndbeyond.com/characters/27570282/JhoG2D");
		println!("Finished downloading");
		skills
	})
	.await;

	let message = match result {
		Ok(Ok(s)) => format!("{:?}", s),
		Ok(Err(err)) => format!("Failed to download modifiers: {}", err),
		Err(err) => format!("JoinError: {}", err),
	};

	let text = reqwest::get(&format!(
		"https://api.telegram.org/bot{}/sendMessage?chat_id={}&text={}&reply_to_message_id={}",
		token, chat.id, message, reply_to
	))
	.and_then(|response| response.text())
	.await;

	println!("Response from Telegram: {:?}", text);
}

#[post(
	"/telegram/update/<token>",
	format = "application/json",
	data = "<update>"
)]
async fn telegram_update(token: String, update: Json<Update>) {
	let update = update.0;

	println!("Received update: {:?}", update);

	std::thread::spawn(move || handle_update(token, update));
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
	rocket::ignite().mount("/", routes![health, telegram_update, telegram_setwebhook])
}
