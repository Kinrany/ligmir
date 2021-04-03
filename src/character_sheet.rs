use std::collections::HashMap;

use failure::{err_msg, Fallible};
use headless_chrome::{protocol::target::methods::CreateTarget, Browser};
use rocket::tokio;

pub struct CharacterSheet {
	pub skills: HashMap<String, i32>,
}

#[derive(Clone, Debug)]
pub struct Headless {
	pub service_url: String,
	pub timeout: u64,
}

impl Headless {
	fn download_character_sheet_sync(self: Headless, url: String) -> Fallible<CharacterSheet> {
		let browser = Browser::connect(self.service_url)?;

		let tab = browser.new_tab_with_options(CreateTarget {
			url: &url,
			width: None,
			height: None,
			browser_context_id: None,
			enable_begin_frame_control: None,
		})?;

		// Wait for network/javascript/dom to make the skill list available
		let element = tab.wait_for_element_with_custom_timeout(
			"div.ct-skills",
			std::time::Duration::from_secs(self.timeout),
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
			.collect::<Fallible<HashMap<String, i32>>>()?;

		Ok(CharacterSheet { skills })
	}

	pub async fn download_character_sheet(
		self: &Headless,
		url: String,
	) -> Fallible<CharacterSheet> {
		let headless = self.clone();
		let character_sheet = tokio::task::spawn_blocking(move || async move {
			headless.download_character_sheet_sync(url)
		})
		.await?
		.await?;

		Ok(character_sheet)
	}
}
