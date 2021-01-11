use failure::{err_msg, Fallible};
use headless_chrome::Browser;

fn download_skill_modifiers(url: &str) -> Fallible<Vec<(String, i32)>> {
	let browser = Browser::default()?;

	let tab = browser.wait_for_initial_tab()?;

	// Navigate to dndbeyond
	tab.navigate_to(url)?;

	// Wait for network/javascript/dom to make the skill list available
	let element = tab.wait_for_element("div.ct-skills")?;

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

fn main() -> Fallible<()> {
	let modifiers = download_skill_modifiers("https://www.dndbeyond.com/characters/31859887/HUQLxj")?;
	println!("{:?}", modifiers);
	Ok(())
}
