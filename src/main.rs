use failure::{err_msg, Fallible};
use headless_chrome::{protocol::target::methods::CreateTarget, Browser};
use rocket::{get, http::Status, launch, post, routes, tokio::task::spawn_blocking, Rocket};
use rocket_contrib::json::Json;
use teloxide::types::Update;

fn download_skill_modifiers(url: &str) -> Fallible<Vec<(String, i32)>> {
	let browser = Browser::default()?;

	let tab = browser.new_tab_with_options(CreateTarget {
		url,
		width: None,
		height: None,
		browser_context_id: None,
		enable_begin_frame_control: None,
	})?;

	// Wait for network/javascript/dom to make the skill list available
	let element = tab
		.wait_for_element_with_custom_timeout("div.ct-skills", std::time::Duration::from_secs(10))?;

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
            let message = format!("Cannot parse string \"{}\" into skill name and modifier", s);
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

#[post(
	"/telegram/update/<_token>",
	format = "application/json",
	data = "<_update>"
)]
async fn telegram_update(_token: String, _update: Json<Update>) -> Result<(), Status> {
	match spawn_blocking(|| {
		download_skill_modifiers("https://www.dndbeyond.com/characters/27570282/JhoG2D")
	})
	.await
	{
		Ok(skill_modifiers) => {
			println!("Skill modifiers: {:?}", skill_modifiers);
			Ok(())
		}
		Err(error) => {
			println!("Fail: {}", error);
			Err(Status::InternalServerError)
		}
	}
}

#[get("/telegram/setwebhook?<token>&<host>")]
async fn telegram_setwebhook(token: String, host: String) -> Result<(), Status> {
	let url = format!(
		"https://api.telegram.org/bot{}/setWebhook?url=https://{}/telegram/update/{}",
		token, host, token
	);

	reqwest::get(&url)
		.await
		.map_err(|_| Status::InternalServerError)?;

	Ok(())
}

#[launch]
async fn rocket() -> Rocket {
	rocket::ignite().mount("/", routes![health, telegram_update, telegram_setwebhook])
}
