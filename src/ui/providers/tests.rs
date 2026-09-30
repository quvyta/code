//! The providers screen, driven from where a person's hand is: the menu row, the buttons, the
//! fields and the keys. Nothing here reaches the network, because the screen is given a web that
//! answers from a string, and nothing here touches the person's data folder, because it is given
//! a file of its own.

use std::sync::{Arc, Mutex};

use qframe::runtime::Harness;

use crate::provider::{Answer, Ask, AskError, Web};
use crate::{Page, QCode, testing};

/// A terminal wide enough for an address and a model's two windows on one row.
const SIZE: (u16, u16) = (110, 40);

/// Not anyone's key: the characters say so.
const MADE_UP: &str = "not-a-real-key-0000-wxyz";

/// A providers file of this test's own.
fn file(what: &str) -> std::path::PathBuf {
    let folder = std::env::temp_dir().join(format!("qcode-providers-ui-{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    folder.join("providers.toml")
}

/// A web that answers from `answers`, matched by what the address ends with, and keeps every
/// request it was ever handed.
fn canned(answers: Vec<(&'static str, &'static str)>) -> (Web, Arc<Mutex<Vec<Ask>>>) {
    let seen: Arc<Mutex<Vec<Ask>>> = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    let web = Web::new(move |ask| {
        kept.lock().expect("the list").push(ask.clone());
        match answers.iter().find(|(ending, _)| ask.url.ends_with(ending)) {
            Some((_, body)) => Ok(Answer { status: 200, body: (*body).to_owned() }),
            None => Err(AskError::Unreachable { url: ask.url.clone(), reason: "no route to host".to_owned() }),
        }
    });
    (web, seen)
}

/// An application on its home screen whose providers screen reads `path` and asks through `web`,
/// on a terminal of this test's own size.
fn sized(path: &std::path::Path, web: Web, size: (u16, u16)) -> Harness<QCode> {
    let store = testing::scratch("providers-store");
    let app = testing::app(testing::config(&store, &[]), &testing::settled(), None)
        .with_providers(Some(path.to_path_buf()), web);
    testing::harness(app, size.0, size.1)
}

/// The same, on the terminal these tests are read at.
fn app(path: &std::path::Path, web: Web) -> Harness<QCode> {
    sized(path, web, SIZE)
}

/// The same, already on the providers page, reached the way a person reaches it.
fn opened(path: &std::path::Path, web: Web) -> Harness<QCode> {
    let mut harness = app(path, web);
    harness.click_text("Providers").render();
    assert_eq!(harness.app().page(), Page::Providers, "the menu row opens the page:\n{}", harness.screen());
    harness
}

/// Opens the add dialog, picks `kind`, writes `tag` and moves the keyboard on to the address
/// field, from the controls a person uses.
fn to_the_address(harness: &mut Harness<QCode>, kind: &str, tag: &str) {
    harness.click_text("Add a provider").render();
    // Picking the kind moves the keyboard onto the choice, so the fields are walked from there
    // the way Tab walks them for anyone.
    harness.click_text(kind).render();
    harness.press("tab");
    harness.type_text(tag);
    harness.press("tab");
}

/// Fills the add dialog and presses Add, from the controls a person uses.
fn add(harness: &mut Harness<QCode>, kind: &str, tag: &str, base: &str, key: Option<&str>) {
    to_the_address(harness, kind, tag);
    // The address field comes with the kind's usual address already in it; it is cleared the way
    // a person clears a field before writing their own.
    for _ in 0..80 {
        harness.press("backspace");
    }
    harness.type_text(base);
    if let Some(key) = key {
        harness.press("tab");
        harness.type_text(key);
    }
    press_add(harness);
}

/// A providers file of this test's own holding one OpenRouter provider with forty models, some
/// free and some paid, as the service's own listing leaves them.
///
/// The three at the top are the ones the lineup tests reach for by writing a part of their name.
/// The rest stand in for the hundreds a real listing carries, so that both the filter and the
/// scrolling list are needed to get at one, and two of every three carry a price, because that
/// is what a listing that publishes prices looks like.
fn openrouter(what: &str) -> std::path::PathBuf {
    openrouter_with(what, &[])
}

/// The same, with the lineups the file already holds: each one named, and given its steps in the
/// order they are tried.
fn openrouter_with(what: &str, lineups: &[(&str, &[&str])]) -> std::path::PathBuf {
    let path = file(what);
    let folder = path.parent().expect("its folder");
    std::fs::create_dir_all(folder).expect("a folder");
    let mut text = String::from(
        "[[provider]]\ntag = \"yol\"\nkind = \"openrouter\"\nbase = \"https://openrouter.ai\"\nwire = \"openai\"\n",
    );
    let mut row = |id: String, price: Option<&str>| {
        text.push_str("\n[[model]]\ntag = \"yol\"\n");
        text.push_str(&format!("id = \"{id}\"\nclaimed-context = 262144\n"));
        if let Some(price) = price {
            text.push_str(&format!("price = \"{price}\"\n"));
        }
    };
    for (id, price) in
        [("qwen/qwen3-coder:free", Some("free")), ("z-ai/glm-4.6:free", Some("free")), ("z-ai/glm-4.6", Some("paid"))]
    {
        row(id.to_owned(), price);
    }
    for n in 4..=40 {
        row(
            format!("vendor/model-{n:02}"),
            match n % 3 {
                1 => Some("paid"),
                2 => Some("free"),
                _ => None,
            },
        );
    }
    for (name, steps) in lineups {
        for step in *steps {
            text.push_str("\n[[lineup]]\ntag = \"yol\"\n");
            text.push_str(&format!("name = \"{name}\"\nmodel = \"{step}\"\n"));
        }
    }
    std::fs::write(&path, text).expect("a file");
    // As QCode itself leaves it: the two permission warnings the page carries for a folder and a
    // file others can read would take four rows of a twenty-four row terminal away from the very
    // buttons these tests are about.
    #[cfg(unix)]
    {
        set_mode(folder, 0o700);
        set_mode(&path, 0o600);
    }
    path
}

/// The same, on the page, with the lineups of that provider open, the way a person opens them.
fn lineups(harness: &mut Harness<QCode>) {
    harness.click_text("Lineups…").render();
    assert!(harness.screen().contains("Lineups of yol"), "the dialog names the provider:\n{}", harness.screen());
}

/// The `[[lineup]]` rows of the file at `path`, in the order they stand in it: the name each row
/// is for and the model of that step, read off the disk.
fn lineups_in(path: &std::path::Path) -> Vec<(String, String)> {
    let written = std::fs::read_to_string(path).expect("the file is there");
    let value = |block: &str, key: &str| -> Option<String> {
        let prefix = format!("{key} = \"");
        block
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str()))
            .and_then(|rest| rest.strip_suffix('"'))
            .map(str::to_owned)
    };
    written
        .split("\n[[lineup]]\n")
        .skip(1)
        .filter_map(|block| Some((value(block, "name")?, value(block, "model")?)))
        .collect()
}

/// Clicks the dialog's own button, the lowest `Add` standing as a word of its own: the page's
/// "Add a provider" can still show beside the dialog once a provider is on the list.
fn press_add(harness: &mut Harness<QCode>) {
    let screen = harness.screen();
    let lines: Vec<&str> = screen.lines().collect();
    let place = lines.iter().enumerate().rev().find_map(|(y, line)| {
        let cells: Vec<char> = line.chars().collect();
        (0..cells.len().saturating_sub(2)).rev().find_map(|x| {
            let word = cells[x..x + 3].iter().collect::<String>() == "Add";
            let alone = (x == 0 || cells[x - 1] == ' ') && cells.get(x + 3).is_none_or(|next| *next == ' ');
            (word && alone).then_some((x, y))
        })
    });
    let (x, y) = place.unwrap_or_else(|| panic!("the dialog's Add button:\n{screen}"));
    let (x, y) = (i32::try_from(x).expect("a column"), i32::try_from(y).expect("a row"));
    harness.click(x, y).render();
}

#[test]
fn typing_an_address_into_the_offered_one_replaces_it() {
    let path = file("replaced");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    to_the_address(&mut harness, "ollama", "ev");
    // No clearing first: a person who tabs into a field holding a suggestion types over it.
    harness.type_text("http://192.168.122.1:11434");
    press_add(&mut harness);
    let written =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("the provider was added:\n{}", harness.screen()));
    assert!(written.contains("base = \"http://192.168.122.1:11434\""), "{written}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn an_address_typed_after_the_offered_one_is_refused_beside_the_field() {
    let path = file("run-together");
    let (web, seen) = canned(Vec::new());
    let mut harness = opened(&path, web);
    to_the_address(&mut harness, "ollama", "ev");
    // The person goes to the end on purpose and writes theirs after the one offered.
    harness.press("end");
    harness.type_text("http://192.168.122.1:11434");
    press_add(&mut harness);
    let screen = harness.screen();
    assert!(screen.contains("read as one address"), "the reason is beside the field:\n{screen}");
    assert!(!path.exists(), "nothing was written:\n{screen}");
    assert_eq!(seen.lock().expect("the list").len(), 0, "refusing it reached nothing");
}

#[test]
fn the_menu_has_a_row_for_providers_beside_profiles_and_it_opens_the_page() {
    let (web, seen) = canned(Vec::new());
    let mut harness = app(&file("menu"), web);
    let screen = harness.screen();
    assert!(screen.contains("Profiles") && screen.contains("Providers"), "{screen}");
    harness.click_text("Providers").render();
    assert_eq!(harness.app().page(), Page::Providers);
    assert!(harness.screen().contains("No provider of your own yet"), "{}", harness.screen());
    assert_eq!(seen.lock().expect("the list").len(), 0, "opening the page reached nothing");
}

#[test]
fn the_page_says_where_the_key_file_is_and_that_a_backup_carries_it() {
    let path = file("says-where");
    let (web, _) = canned(Vec::new());
    let harness = opened(&path, web);
    let screen = harness.screen();
    // The owner's condition for the whole feature: not a footnote, a line of the page.
    assert!(screen.contains("plain text"), "the words are on the page:\n{screen}");
    assert!(screen.contains("backup"), "what carries the key away is named:\n{screen}");
    let folder = path.parent().expect("its folder").file_name().expect("a name").to_string_lossy().into_owned();
    assert!(screen.contains(&folder), "the place itself is named:\n{screen}");
}

#[test]
fn a_provider_added_from_the_dialog_is_on_the_list_and_in_the_file() {
    let path = file("added");
    let (web, seen) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    let screen = harness.screen();
    assert!(screen.contains("ev"), "the tag is on the list:\n{screen}");
    assert!(screen.contains("http://192.168.122.1:11434"), "{screen}");
    // A value only a working connection to the file could produce: the address the person typed,
    // read back off the disk.
    let written = std::fs::read_to_string(&path).expect("the file was written");
    assert!(written.contains("tag = \"ev\""), "{written}");
    assert!(written.contains("base = \"http://192.168.122.1:11434\""), "{written}");
    assert_eq!(seen.lock().expect("the list").len(), 0, "adding a provider reached nothing");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_pasted_key_is_never_shown_again_and_only_its_last_four_characters_stand_on_the_page() {
    let path = file("key-hidden");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", Some(MADE_UP));
    harness.render();
    let screen = harness.screen();
    assert!(!screen.contains("not-a-real"), "the key is on the screen:\n{screen}");
    assert!(screen.contains("wxyz"), "its last four characters are:\n{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_tag_that_could_not_stand_in_front_of_a_model_keeps_the_dialog_open_and_says_why() {
    let path = file("bad-tag");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev/makine", "http://h:1", None);
    harness.render();
    let screen = harness.screen();
    assert!(screen.contains("cannot hold"), "the reason is beside the field:\n{screen}");
    assert!(harness.app().page() == Page::Providers && screen.contains("Add"), "the dialog is still open:\n{screen}");
    assert!(!std::path::Path::new(&path).exists(), "nothing was written");
}

#[test]
fn openrouter_is_not_added_without_a_key() {
    let path = file("no-key");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", None);
    harness.render();
    assert!(harness.screen().contains("reached with a key"), "{}", harness.screen());
}

#[test]
fn nothing_reaches_the_network_until_the_button_is_pressed_and_the_page_says_where_it_will_go() {
    let path = file("only-on-press");
    let (web, seen) = canned(vec![("/api/version", r#"{"version":"0.34.2"}"#)]);
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    let screen = harness.screen();
    // Where it will go, before it goes.
    assert!(screen.contains("GET http://192.168.122.1:11434/api/version"), "{screen}");
    let said = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(said.contains("nothing else here goes out"), "and that nothing else does:\n{screen}");
    assert_eq!(seen.lock().expect("the list").len(), 0, "nothing has gone out yet");

    harness.click_text("Try the connection").render();
    let asked = seen.lock().expect("the list");
    assert_eq!(asked.len(), 1, "one press, one request");
    assert_eq!(asked[0].line(), "GET http://192.168.122.1:11434/api/version", "and it went where the page said");
    drop(asked);
    // A value only a working connection could produce: the version the canned server named.
    assert!(harness.screen().contains("0.34.2"), "what answered is said:\n{}", harness.screen());
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_server_that_does_not_answer_is_said_out_loud_with_the_address_it_was_tried_at() {
    let path = file("unreachable");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    harness.click_text("Try the connection").render();
    let screen = harness.screen();
    assert!(screen.contains("no route to host"), "what went wrong is said:\n{screen}");
    assert!(screen.contains("192.168.122.1:11434"), "and where it was tried:\n{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn asking_a_provider_what_it_offers_puts_both_windows_on_the_row() {
    let path = file("both-windows");
    let (web, _) = canned(vec![
        ("/api/tags", r#"{"models":[{"name":"qwen3.8:latest"}]}"#),
        ("/api/show", r#"{"model_info":{"qwen35.context_length":262144}}"#),
        ("/v1/messages", r#"{"usage":{"input_tokens":3010}}"#),
    ]);
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    assert!(harness.screen().contains("Nothing has been asked"), "{}", harness.screen());

    harness.click_text("Ask what it offers").render();
    let screen = harness.screen();
    assert!(screen.contains("qwen3.8:latest"), "the model is on the list:\n{screen}");
    // Both figures, and neither of them invented: 262144 came from the record, and nothing has
    // been measured yet.
    assert!(screen.contains("262144"), "what the model claims:\n{screen}");
    assert!(screen.contains("not measured yet"), "and that nobody has measured it:\n{screen}");

    harness.click_text("Measure the real window").render();
    let screen = harness.screen();
    assert!(screen.contains("262144"), "what the model claims is still there:\n{screen}");
    assert!(screen.contains("3010"), "and what the server really gives:\n{screen}");
    // The whole point of the feature: a window this small is called what it is, in words, with
    // the way out on the person's own machine.
    assert!(screen.contains("too small for a coding agent"), "{screen}");
    assert!(screen.contains("num_ctx"), "the remedy is spelled out:\n{screen}");
    assert!(screen.contains("ollama create"), "{screen}");

    let written = std::fs::read_to_string(&path).expect("the file was written");
    assert!(written.contains("measured-context = 3010"), "the measurement is kept:\n{written}");
    assert!(written.contains("measured = \"about\""), "{written}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn measuring_an_openrouter_model_asks_its_api_and_not_its_website() {
    let path = file("openrouter-api");
    let (web, seen) = canned(vec![
        ("/api/v1/models", r#"{"data":[{"id":"nex-agi/nex-n2.5-mini:free","context_length":262144}]}"#),
        ("/v1/messages", r#"{"usage":{"input_tokens":3010}}"#),
    ]);
    let mut harness = opened(&path, web);
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", Some(MADE_UP));
    harness.render();
    harness.click_text("Ask what it offers").render();
    harness.click_text("Measure the real window").render();
    assert!(harness.screen().contains("3010"), "the measurement came back:\n{}", harness.screen());
    let asked: Vec<String> = seen.lock().expect("the list").iter().map(Ask::line).collect();
    // The same path without `/api` is OpenRouter's website, which answers a message with a page
    // of HTML and a 200.
    assert!(asked.iter().any(|line| line == "POST https://openrouter.ai/api/v1/messages"), "{asked:?}");
    assert!(!asked.iter().any(|line| line.contains("openrouter.ai/v1/")), "nothing went to the website: {asked:?}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

/// What OpenRouter really answered for a rate-limited free model on 2026-09-21, with its request
/// id shortened.
const RATE_LIMITED: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Provider returned error","error_type":"rate_limit_exceeded"},"request_id":"gen-1","metadata":{"raw":"google/gemma-4-26b-a4b-it:free is temporarily rate-limited upstream. Please retry shortly, or add your own key to accumulate your rate limits: https://openrouter.ai/settings/integrations","provider_name":"Google AI Studio","is_byok":false,"provider_error_code":"429","limit_source":"upstream_provider_shared_pool"}}"#;

#[test]
fn a_free_model_that_is_rate_limited_is_said_to_be_asking_for_a_wait_not_to_have_refused() {
    let path = file("rate-limited");
    let web = Web::new(|ask: &Ask| {
        if ask.url.ends_with("/api/v1/models") {
            let body = r#"{"data":[{"id":"google/gemma-4-26b-a4b-it:free","context_length":262144}]}"#;
            return Ok(Answer { status: 200, body: body.to_owned() });
        }
        Ok(Answer { status: 429, body: RATE_LIMITED.to_owned() })
    });
    let mut harness = opened(&path, web);
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", Some(MADE_UP));
    harness.render();
    harness.click_text("Ask what it offers").render();
    harness.click_text("Measure the real window").render();
    let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        screen.contains("is rate-limited (429): free models are shared"),
        "the person is told what 429 means:\n{screen}"
    );
    assert!(screen.contains("wait a minute and press again"), "and what to do meanwhile:\n{screen}");
    assert!(!screen.contains("refused with"), "a service asking for a wait did not refuse anything:\n{screen}");
    // The provider's own sentence, not the "Provider returned error" in front of it.
    assert!(screen.contains("temporarily rate-limited upstream"), "{screen}");
    assert!(!screen.contains("Provider returned error"), "{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_window_whose_edge_was_never_found_is_not_called_cramped_and_offers_no_remedy() {
    let path = file("roomy");
    // Every probe is read whole, so growth never stops.
    let web = Web::new(|ask: &Ask| {
        if ask.url.ends_with("/api/tags") {
            return Ok(Answer { status: 200, body: r#"{"models":[{"name":"qwen3.8-32k:latest"}]}"#.to_owned() });
        }
        if ask.url.ends_with("/api/show") {
            return Ok(Answer { status: 200, body: r#"{"model_info":{"qwen35.context_length":262144}}"#.to_owned() });
        }
        let body = ask.body.as_deref().unwrap_or_default();
        let sent: serde_json::Value = serde_json::from_str(body).expect("a probe is JSON");
        let words = sent["messages"][0]["content"].as_str().expect("a prompt").split_whitespace().count();
        Ok(Answer { status: 200, body: format!("{{\"usage\":{{\"input_tokens\":{words}}}}}") })
    });
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    harness.click_text("Ask what it offers").render();
    harness.click_text("Measure the real window").render();
    let screen = harness.screen();
    assert!(screen.contains("at least"), "no edge was found, so no edge is claimed:\n{screen}");
    assert!(!screen.contains("too small for a coding agent"), "{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn asking_again_what_a_provider_offers_keeps_a_measurement_the_person_waited_for() {
    let path = file("kept");
    let (web, _) = canned(vec![
        ("/api/tags", r#"{"models":[{"name":"qwen3.8:latest"}]}"#),
        ("/api/show", r#"{"model_info":{"qwen35.context_length":262144}}"#),
        ("/v1/messages", r#"{"usage":{"input_tokens":3010}}"#),
    ]);
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    harness.click_text("Ask what it offers").render();
    harness.click_text("Measure the real window").render();
    assert!(harness.screen().contains("3010"), "{}", harness.screen());
    harness.click_text("Ask what it offers").render();
    assert!(harness.screen().contains("3010"), "the measurement was not thrown away:\n{}", harness.screen());
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn forgetting_the_key_keeps_the_provider_and_takes_the_key_out_of_the_file() {
    let path = file("forget");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", Some(MADE_UP));
    harness.render();
    let written = std::fs::read_to_string(&path).expect("the file was written");
    assert!(written.contains(MADE_UP), "the key is in the file it belongs in");

    harness.click_text("Forget the key").render();
    let screen = harness.screen();
    assert!(screen.contains("yol"), "the provider stays:\n{screen}");
    assert!(screen.contains("No key"), "and it now has none:\n{screen}");
    let written = std::fs::read_to_string(&path).expect("the file is still there");
    assert!(!written.contains(MADE_UP), "the key is gone from the file:\n{written}");
    assert!(written.contains("tag = \"yol\""), "the provider is not:\n{written}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn deleting_a_provider_asks_first_and_takes_the_key_with_it() {
    let path = file("delete");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", Some(MADE_UP));
    harness.render();
    harness.click_text("Delete").render();
    let screen = harness.screen();
    assert!(screen.contains("Delete yol?"), "the question is asked first:\n{screen}");
    assert!(screen.contains("key goes with it"), "{screen}");
    harness.press("esc").render();
    assert!(harness.screen().contains("yol"), "the answer was no, so nothing went:\n{}", harness.screen());

    harness.click_text("Delete").render();
    harness.press("tab").press("enter").render();
    let written = std::fs::read_to_string(&path).expect("the file is still there");
    assert!(!written.contains("yol"), "the provider is gone:\n{written}");
    assert!(!written.contains(MADE_UP), "and the key with it:\n{written}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_providers_file_written_by_hand_and_broken_is_reported_with_its_place_and_not_panicked_over() {
    let path = file("broken");
    std::fs::create_dir_all(path.parent().expect("its folder")).expect("a folder");
    std::fs::write(&path, "[[provider]]\ntag = \"ev\"\nkind = \"ollamaa\"\nbase = \"http://h:1\"\n").expect("a file");
    let (web, _) = canned(Vec::new());
    let harness = opened(&path, web);
    let screen = harness.screen();
    assert!(screen.contains("providers.toml:3:8"), "the line and column are on the page:\n{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn permissions_that_let_others_read_the_key_are_said_on_the_page() {
    let path = file("wide");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("the mode is changed");
        harness.click_text("Providers").render();
        harness.press("esc").render();
        harness.click_text("Providers").render();
        assert!(harness.screen().contains("0644"), "the real mode is on the page:\n{}", harness.screen());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("the mode is changed");
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

/// The permissions of `place`, as the disk has them.
#[cfg(unix)]
fn mode(place: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(place).expect("it is there").permissions().mode() & 0o777
}

#[cfg(unix)]
fn set_mode(place: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(place, std::fs::Permissions::from_mode(mode)).expect("the mode is changed");
}

#[cfg(unix)]
#[test]
fn a_folder_qcode_made_before_any_key_is_not_warned_about_and_the_first_save_narrows_it() {
    let path = file("fresh-folder");
    let folder = path.parent().expect("its folder");
    // What the rest of QCode leaves on a fresh start: its data folder, made with the usual mode,
    // and no providers file in it.
    std::fs::create_dir_all(folder).expect("the folder is made");
    set_mode(folder, 0o755);
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    let screen = harness.screen();
    assert!(!screen.contains("0755"), "no key is at risk, so nothing is said:\n{screen}");

    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    harness.render();
    assert_eq!(mode(&path), 0o600, "the key file is narrow");
    assert_eq!(mode(folder), 0o700, "and so is the folder it went into, although it was there before");
    let screen = harness.screen();
    assert!(!screen.contains("0755") && !screen.contains("0700"), "nothing to warn about:\n{screen}");
    let _ = std::fs::remove_dir_all(folder);
}

#[cfg(unix)]
#[test]
fn a_warning_that_a_save_made_untrue_leaves_the_page() {
    let path = file("stale-warning");
    let folder = path.parent().expect("its folder");
    let mut providers = crate::provider::Providers::in_memory().at(&path);
    let entry = crate::provider::ProviderEntry::new(
        crate::provider::Tag::parse("ev").expect("a tag"),
        crate::provider::ProviderKind::Ollama,
        "http://192.168.122.1:11434",
    );
    providers.add(entry).expect("the tag is free");
    providers.save().expect("the file is written");
    set_mode(folder, 0o755);

    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    let screen = harness.screen();
    assert!(screen.contains("0755") && screen.contains("0700"), "the widened folder is said:\n{screen}");
    assert!(screen.contains("ev"), "and the providers are still read:\n{screen}");
    harness.set_locale("tr").render();
    let screen = harness.screen();
    assert!(screen.contains("izinleri 0755"), "in the person's language:\n{screen}");
    harness.set_locale("en").render();

    // A kind the list does not already show, so the dialog's own choice is the one clicked.
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", Some(MADE_UP));
    harness.render();
    assert_eq!(mode(folder), 0o700, "the save narrowed the folder:\n{}", harness.screen());
    let screen = harness.screen();
    assert!(!screen.contains("0755"), "and the warning it made untrue is gone:\n{screen}");
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_provider_with_many_models_keeps_its_buttons_and_its_answer_on_the_screen() {
    // A real ollama with fifteen models, on an ordinary terminal and on a small one.
    let tags = format!(
        r#"{{"models":[{}]}}"#,
        (1..=15).map(|n| format!(r#"{{"name":"model-{n:02}:latest"}}"#)).collect::<Vec<_>>().join(",")
    );
    let tags: &'static str = Box::leak(tags.into_boxed_str());
    for (width, height) in [(120, 36), (80, 24)] {
        let path = file(&format!("many-models-{width}"));
        let (web, _) = canned(vec![
            ("/api/tags", tags),
            ("/api/show", r#"{"model_info":{"qwen35.context_length":262144}}"#),
            ("/api/version", r#"{"version":"0.9.1"}"#),
        ]);
        let store = testing::scratch("providers-store");
        let app = testing::app(testing::config(&store, &[]), &testing::settled(), None)
            .with_providers(Some(path.clone()), web);
        let mut harness = testing::harness(app, width, height);
        harness.click_text("Providers").render();
        add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
        harness.click_text("Ask what it offers").render();
        let screen = harness.screen();
        assert!(screen.contains("model-01:latest"), "{width}x{height}: the models are listed:\n{screen}");
        for label in ["Try the connection", "Ask what it offers", "Delete", "Add a provider"] {
            assert!(screen.contains(label), "{width}x{height}: `{label}` is on the screen:\n{screen}");
        }

        harness.click_text("Try the connection").render();
        let screen = harness.screen();
        assert!(screen.contains("running version 0.9.1"), "{width}x{height}: the answer is read:\n{screen}");
        harness.click_text("Delete").render();
        assert!(harness.screen().contains("Delete ev?"), "{width}x{height}: Delete asks:\n{}", harness.screen());
        harness.press("esc").render();
        harness.click_text("Add a provider").render();
        assert!(harness.screen().contains("New provider"), "{width}x{height}: {}", harness.screen());
        harness.press("esc").render();

        // The last model is still there, inside the list.
        harness.click_text("model-01:latest").render();
        for _ in 0..14 {
            harness.press("down");
        }
        harness.render();
        assert!(harness.screen().contains("model-15:latest"), "{width}x{height}: {}", harness.screen());
        let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
    }
}

#[test]
fn a_lineup_written_in_the_dialog_lands_in_the_file_in_the_order_the_person_chose() {
    let path = openrouter("lineup-new");
    let (web, seen) = canned(Vec::new());
    let mut harness = opened(&path, web);
    assert!(harness.screen().contains("No lineups yet"), "the page says it has none:\n{}", harness.screen());
    lineups(&mut harness);
    assert!(harness.screen().contains("no lineups yet"), "and so does the dialog:\n{}", harness.screen());

    harness.click_text("New lineup").render();
    harness.type_text("coder");
    // The keyboard walks from the name on to the filter and on to the models, the way Tab walks
    // it for anyone.
    harness.press("tab");
    harness.type_text("qwen");
    harness.press("tab");
    assert!(harness.is_focused("lineup-models"), "the models are under the keyboard:\n{}", harness.screen());
    harness.press("enter");
    let screen = harness.screen();
    assert!(screen.contains("1. qwen/qwen3-coder:free"), "the model went in at the end:\n{screen}");

    // A second one, found by clearing the filter and writing another part of a name, and put in
    // by a click on its row.
    harness.press("shift+tab");
    for _ in 0..4 {
        harness.press("backspace");
    }
    harness.type_text("glm");
    harness.click_text("z-ai/glm-4.6:free").render();
    let screen = harness.screen();
    assert!(screen.contains("2. z-ai/glm-4.6:free"), "the row under the pointer went in too:\n{screen}");

    // The step that fell back is the one that goes first, so the second step is put above the
    // first and the person sees the order change where they chose it.
    harness.click_text("2. z-ai/glm-4.6:free").render();
    harness.click_text("Up").render();
    let screen = harness.screen();
    let step = |n: u8| {
        screen.lines().find(|line| line.contains(&format!("{n}. "))).unwrap_or_else(|| panic!("step {n}:\n{screen}"))
    };
    assert!(step(1).contains("z-ai/glm-4.6:free"), "the one chosen is first:\n{screen}");
    assert!(step(2).contains("qwen/qwen3-coder:free"), "and the one it was put above is second:\n{screen}");

    harness.click_text("Save").render();
    let written = lineups_in(&path);
    assert_eq!(
        written,
        [
            ("coder".to_owned(), "z-ai/glm-4.6:free".to_owned()),
            ("coder".to_owned(), "qwen/qwen3-coder:free".to_owned())
        ],
        "two rows in the file, in the order they are tried:"
    );
    let text = std::fs::read_to_string(&path).expect("the file is there");
    assert_eq!(text.matches("[[lineup]]").count(), 2, "and no other block of them: {text}");
    assert!(harness.screen().contains("coder"), "the dialog shows what was saved:\n{}", harness.screen());
    harness.press("esc").render();
    let screen = harness.screen();
    assert!(
        screen.contains("Lineups: coder (z-ai/glm-4.6:free → qwen/qwen3-coder:free)"),
        "the page names it and the models it tries:\n{screen}"
    );
    assert_eq!(seen.lock().expect("the list").len(), 0, "writing a lineup reached nothing");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_step_taken_out_of_a_lineup_in_the_editor_is_one_row_shorter_in_the_file() {
    let path = openrouter_with("lineup-edit", &[("coder", &["qwen/qwen3-coder:free", "z-ai/glm-4.6:free"])]);
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    assert!(
        harness.screen().contains("Lineups: coder (qwen/qwen3-coder:free → z-ai/glm-4.6:free)"),
        "the page reads the order out of the file:\n{}",
        harness.screen()
    );

    lineups(&mut harness);
    harness.click_text("Edit").render();
    let screen = harness.screen();
    assert!(screen.contains("1. qwen/qwen3-coder:free") && screen.contains("2. z-ai/glm-4.6:free"), "{screen}");
    harness.click_text("2. z-ai/glm-4.6:free").render();
    harness.click_text("Remove").render();
    let screen = harness.screen();
    assert!(screen.contains("1. qwen/qwen3-coder:free"), "what is left is the first row now:\n{screen}");
    assert!(!screen.contains("2. z-ai"), "and the one that was taken out is gone:\n{screen}");

    harness.click_text("Save").render();
    assert_eq!(
        lineups_in(&path),
        [("coder".to_owned(), "qwen/qwen3-coder:free".to_owned())],
        "one row left in the file"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_lineup_is_not_saved_without_a_name_it_can_hold_a_name_taken_or_a_model_in_it() {
    let path = openrouter_with("lineup-refused", &[("coder", &["qwen/qwen3-coder:free"])]);
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    lineups(&mut harness);
    harness.click_text("New lineup").render();

    // A name that could stand in front of a model is no use as a name a harness is told.
    harness.type_text("2coder");
    harness.press("tab");
    harness.press("tab");
    harness.press("enter");
    harness.click_text("Save").render();
    let screen = harness.screen();
    assert!(screen.contains("not with 2"), "the reason is beside the name field:\n{screen}");
    assert!(screen.contains("Name"), "which is still the field the editor opens on:\n{screen}");

    // A name this provider already answers to would leave nothing to tell two orders apart.
    harness.click_text("Name").render();
    for _ in 0..8 {
        harness.press("backspace");
    }
    harness.type_text("coder");
    harness.click_text("Save").render();
    let screen = harness.screen();
    assert!(screen.contains("already has a lineup called coder"), "and that one says which:\n{screen}");

    // Nothing in it is not an order: a request that failed on the first model would have nothing
    // to fall back to. The lineup is left and a new one started, so there is nothing in this one
    // either.
    harness.click_text("Cancel").render();
    harness.click_text("New lineup").render();
    harness.type_text("yeni");
    harness.click_text("Save").render();
    let screen = harness.screen();
    assert!(screen.contains("There is nothing in it"), "an empty one is refused in words:\n{screen}");
    assert!(!screen.contains("New lineup"), "and the editor is still open:\n{screen}");
    assert!(screen.contains("yeni"), "with what was written in the name field:\n{screen}");

    assert_eq!(lineups_in(&path), [("coder".to_owned(), "qwen/qwen3-coder:free".to_owned())], "nothing was written:");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_step_that_costs_money_is_said_where_the_order_is_chosen_and_stops_being_said_when_it_is_taken_out() {
    let path = openrouter("lineup-paid");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    lineups(&mut harness);
    harness.click_text("New lineup").render();
    harness.type_text("mali");
    harness.press("tab");
    harness.type_text("glm");
    harness.press("tab");
    // The paid model of a name stands under its free twin, so the row is the one below it.
    let (x, y) = harness.find("z-ai/glm-4.6:free").expect("the free twin is listed first");
    harness.click(x, y + 1).render();
    let screen = harness.screen();
    // The paid step beside the free one of the same name, and what a request that falls back to
    // it costs.
    assert!(screen.contains("paid"), "the row of the model says what it costs:\n{screen}");
    assert!(screen.contains("These steps cost money: z-ai/glm-4.6"), "and the warning names it:\n{screen}");
    assert!(screen.contains("OpenRouter balance"), "and what is spent:\n{screen}");

    // A free step beside it, so the order can still be saved once the paid one is taken out.
    harness.press("shift+tab");
    for _ in 0..3 {
        harness.press("backspace");
    }
    harness.type_text("qwen");
    harness.press("tab");
    harness.press("enter");
    let screen = harness.screen();
    assert!(screen.contains("These steps cost money: z-ai/glm-4.6"), "and it is the only one named:\n{screen}");

    harness.click_text("1. z-ai/glm-4.6").render();
    harness.click_text("Remove").render();
    let screen = harness.screen();
    assert!(!screen.contains("These steps cost money"), "nothing in the order costs money now:\n{screen}");
    harness.click_text("Save").render();
    assert_eq!(
        lineups_in(&path),
        [("mali".to_owned(), "qwen/qwen3-coder:free".to_owned())],
        "and what was kept is what is left of it:"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn deleting_a_lineup_asks_first_and_takes_its_rows_out_of_the_file() {
    let path = openrouter_with(
        "lineup-delete",
        &[("coder", &["qwen/qwen3-coder:free", "z-ai/glm-4.6:free"]), ("fast", &["vendor/model-04"])],
    );
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    lineups(&mut harness);
    harness.click_text("Delete").render();
    let screen = harness.screen();
    assert!(screen.contains("Delete the lineup coder?"), "the question is asked first:\n{screen}");
    // A profile that runs on this order would have nothing to run on, and the person is told it
    // before the rows go rather than when a tab refuses to open. The question is a window over the
    // list it was asked from, so it is read in the two halves its own width gives it.
    assert!(screen.contains("A profile that uses it will not open until it is"), "{screen}");
    assert!(screen.contains("given another order to run."), "{screen}");
    harness.press("esc").render();
    assert_eq!(lineups_in(&path).len(), 3, "the answer was no, so every row is where it was:\n{}", harness.screen());

    harness.click_text("coder").render();
    harness.click_text("Delete").render();
    harness.press("tab").press("enter").render();
    assert_eq!(
        lineups_in(&path),
        [("fast".to_owned(), "vendor/model-04".to_owned())],
        "the rows of that lineup are gone and the other one is not:"
    );
    let screen = harness.screen();
    // The page names what this provider has left, and the one that was deleted is not in it. Read
    // by its own beginning rather than by a word of it: a model of a provider may well be called
    // qwen3-coder.
    assert!(screen.contains("Lineups: fast (vendor/model-04)"), "and the page names what is left:\n{screen}");
    assert!(!screen.contains("Lineups: coder"), "the deleted one is not named:\n{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

/// Whether a model row says what the service published for it, and what a model it published
/// nothing for says instead.
#[test]
fn every_model_row_says_what_the_service_says_it_costs() {
    let path = openrouter("prices");
    let (web, _) = canned(Vec::new());
    let harness = opened(&path, web);
    let screen = harness.screen();
    // The row of one model, read by its own name and by where that name ends: `z-ai/glm-4.6` is
    // also the beginning of `z-ai/glm-4.6:free`, which is another model at another price.
    let row = |id: &str| {
        screen
            .lines()
            .find(|line| line.match_indices(id).any(|(at, _)| line[at + id.len()..].starts_with(' ')))
            .unwrap_or_else(|| panic!("the row of {id} is not on the screen:\n{screen}"))
            .to_owned()
    };
    assert!(row("qwen/qwen3-coder:free").contains("free"), "a free model says so:\n{screen}");
    assert!(row("z-ai/glm-4.6:free").contains("free"), "and the free one beside it too:\n{screen}");
    assert!(row("z-ai/glm-4.6").contains("paid"), "a paid one says what it is:\n{screen}");
    // A model the service published no price for is not given one, and its row is left as it was.
    let unpriced = row("vendor/model-06");
    assert!(!unpriced.contains("free") && !unpriced.contains("paid"), "{unpriced}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn the_lineups_dialog_keeps_its_buttons_and_scrolls_at_eighty_by_twenty_four() {
    use qframe::event::MouseKind;

    let path = openrouter("lineup-small");
    let (web, _) = canned(Vec::new());
    let mut harness = sized(&path, web, (80, 24));
    harness.click_text("Providers").render();
    lineups(&mut harness);
    harness.click_text("New lineup").render();
    harness.type_text("coder");
    let screen = harness.screen();
    for label in ["Up", "Down", "Remove", "Cancel", "Save"] {
        assert!(screen.contains(label), "80x24: `{label}` is on the screen:\n{screen}");
    }

    // The keyboard walks on to the filter the way Tab walks it for anyone, and what the filter
    // leaves is the list of models, not however many the provider offers.
    harness.press("tab");
    harness.type_text("model");
    let screen = harness.screen();
    assert!(screen.contains("vendor/model-04"), "80x24: the models it keeps are in the list:\n{screen}");
    assert!(!screen.contains("z-ai/glm-4.6"), "and the ones it does not are not:\n{screen}");

    // The wheel moves that list inside the dialog, and a model at the far end of forty comes into
    // it: a person who cannot see a model cannot pick it.
    let (x, y) = harness.find("vendor/model-04").expect("the row of a model");
    for _ in 0..40 {
        harness.mouse(MouseKind::ScrollDown, x, y);
    }
    let screen = harness.screen();
    assert!(screen.contains("vendor/model-40"), "80x24: the list scrolled to the end of them:\n{screen}");
    assert!(!screen.contains("vendor/model-04"), "and the first is out of it:\n{screen}");

    // The last of them is what Return puts in the lineup, the one the list has under the keyboard.
    harness.press("tab");
    assert!(harness.is_focused("lineup-models"), "80x24: the models are under the keyboard:\n{}", harness.screen());
    harness.press("end").press("enter");
    let screen = harness.screen();
    assert!(screen.contains("1. vendor/model-40"), "80x24: and it went in at the end:\n{screen}");

    // The lineup it is in scrolls as it grows, and the buttons under it are still on the screen.
    for _ in 0..8 {
        harness.press("down");
        harness.press("enter");
    }
    let screen = harness.screen();
    for label in ["Up", "Down", "Remove", "Cancel", "Save"] {
        assert!(screen.contains(label), "80x24: `{label}` is still on the screen:\n{screen}");
    }
    // A step of the order is told from a model of the provider by the number in front of it, so
    // the two rows the list has are the only lines on the screen that carry one.
    let steps: Vec<&str> = screen.lines().filter(|line| line.contains(". vendor/model-")).collect();
    assert_eq!(steps.len(), 2, "80x24: the order takes the rows the list has and scrolls:\n{screen}");
    assert!(steps[1].contains("9. vendor/model-11"), "80x24: with the last step under the hand in them:\n{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn turkish_reads_as_turkish() {
    let path = openrouter("turkish");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    harness.set_locale("tr").render();
    let screen = harness.screen();
    assert!(screen.contains("Sağlayıcılar"), "{screen}");
    assert!(screen.contains("düz metin"), "the key-file line is Turkish too:\n{screen}");
    // A lineup is called a sıra in Turkish, the word the owner used for it, and the dialog speaks
    // of the same thing rather than of a line of models.
    assert!(screen.contains("Sıralar"), "and the lineups are sıralar:\n{screen}");
    harness.click_text("Sıralar…").render();
    let screen = harness.screen();
    assert!(screen.contains("Henüz sıra yok"), "{screen}");
    assert!(screen.contains("Yeni sıra"), "and the button makes one:\n{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn nothing_is_bracketed_lined_or_framed() {
    use qframe::icons::GlyphMode;

    let path = file("plain");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "ev", "http://192.168.122.1:11434", None);
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        let screen = harness.screen();
        // `⟦` and `⟧` are what an icon or a text missing from its file is drawn as.
        for forbidden in ['[', ']', '{', '}', '┌', '│', '⟦', '⟧'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` in {mode:?}:\n{screen}");
        }
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

/// Every file under `folder` that holds `text`, for a test that has to show a key went to one
/// place and no other.
fn holding(folder: &std::path::Path, text: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(holding(&path, text));
        } else if std::fs::read(&path).is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(text)) {
            found.push(path);
        }
    }
    found
}

#[test]
fn picking_xiaomi_fills_everything_but_the_key_and_the_key_is_written_in_the_providers_file_alone() {
    let path = file("mimo");
    let (web, seen) = canned(Vec::new());
    let mut harness = opened(&path, web);
    harness.click_text("Add a provider").render();
    harness.click_text("Xiaomi MiMo Token Plan").render();
    let screen = harness.screen();
    // Before a key is typed the dialog already says where requests will go, the header the key
    // goes in and what the service offers.
    assert!(screen.contains("Anthropic shape: https://token-plan-ams.xiaomimimo.com/anthropic"), "{screen}");
    assert!(screen.contains("api-key"), "{screen}");
    assert!(screen.contains("mimo-v2.6-flash"), "{screen}");

    // The subscription page named the Singapore cluster; the person picks it.
    harness.click_text("Singapore").render();
    let screen = harness.screen();
    assert!(screen.contains("Anthropic shape: https://token-plan-sgp.xiaomimimo.com/anthropic"), "{screen}");
    assert!(screen.contains("OpenAI shape: https://token-plan-sgp.xiaomimimo.com/v1"), "{screen}");
    // Tag and address are filled; Tab walks past them to the key, the one thing typed.
    harness.press("tab");
    harness.press("tab");
    harness.press("tab");
    harness.type_text(MADE_UP);
    press_add(&mut harness);
    harness.render();

    let written =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("the provider was added:\n{}", harness.screen()));
    assert!(written.contains("tag = \"mimo\""), "{written}");
    assert!(written.contains("kind = \"mimo-token-plan\""), "{written}");
    assert!(written.contains("base = \"https://token-plan-sgp.xiaomimimo.com\""), "{written}");
    assert!(written.contains("id = \"mimo-v2.6-flash\"\nclaimed-context = 1048576"), "{written}");
    assert_eq!(written.matches(MADE_UP).count(), 1, "the key is written once: {written}");
    assert_eq!(
        holding(path.parent().expect("its folder"), MADE_UP),
        std::slice::from_ref(&path),
        "and in no other file there"
    );
    assert_eq!(holding(&testing::scratch("providers-store"), MADE_UP), Vec::<std::path::PathBuf>::new());
    let screen = harness.screen();
    assert!(!screen.contains("not-a-real"), "the key is on the screen:\n{screen}");
    assert!(screen.contains("mimo-v2.6-pro") && screen.contains("its maker publishes 1048576"), "{screen}");
    assert!(seen.lock().expect("the list").is_empty(), "adding a provider reached nothing");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn trying_kimi_asks_its_own_listing_with_the_key_in_the_header_kimi_reads() {
    let path = file("kimi");
    let (web, seen) =
        canned(vec![("/coding/v1/models", r#"{"data":[{"id":"kimi-for-coding","context_length":1048576}]}"#)]);
    let mut harness = opened(&path, web);
    harness.click_text("Add a provider").render();
    harness.click_text("Kimi Code").render();
    for _ in 0..4 {
        harness.press("tab");
    }
    harness.type_text(MADE_UP);
    press_add(&mut harness);
    harness.render();
    let screen = harness.screen();
    assert!(screen.contains("GET https://api.kimi.com/coding/v1/models"), "the page says where Try goes:\n{screen}");
    harness.click_text("Try the connection").render();
    harness.render();
    assert!(harness.screen().contains("The key was accepted."), "{}", harness.screen());
    let asked = seen.lock().expect("the list");
    assert_eq!(asked.len(), 1, "one press, one request");
    assert_eq!(asked[0].line(), "GET https://api.kimi.com/coding/v1/models");
    let secret = asked[0].secret.as_ref().expect("the key is what is tried");
    assert_eq!(secret.header, "x-api-key");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_ready_made_service_is_not_added_without_a_key_and_says_whose_key_is_missing() {
    let path = file("kimi-no-key");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    harness.click_text("Add a provider").render();
    harness.click_text("Kimi Code").render();
    press_add(&mut harness);
    harness.render();
    assert!(harness.screen().contains("Kimi Code is reached with a key."), "{}", harness.screen());
    assert!(!path.exists(), "nothing was written");
}

/// Whether any text of `screen` above its footer bar reaches past the column `right`.
fn reaches_past(screen: &str, right: usize) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let body = &lines[..lines.len().saturating_sub(1)];
    body.iter().any(|line| line.trim_end().chars().count() > right)
}

#[test]
fn the_page_stands_in_the_middle_at_a_readable_width() {
    let path = file("page");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    add(&mut harness, "ollama", "orchard", "http://192.168.122.1:11434", None);
    harness.resize(200, 40).render();
    let screen = harness.screen();
    // The heading at the page's left edge, the row just inside it, the row's kind and address at
    // the page's right edge, and nothing further right than that, on a terminal twice as wide.
    let left = (200 - i32::from(crate::ui::page::WIDTH)) / 2;
    let right = left + i32::from(crate::ui::page::WIDTH);
    let (heading, _) = harness.find("Providers").expect("the heading is drawn");
    assert!(heading.abs_diff(left) <= 1, "the heading is not at the page's edge ({heading}):\n{screen}");
    let (row, _) = harness.find("orchard").expect("the provider is listed");
    // A chosen row carries its mark in the cells in front of its name.
    assert!((left..=left + 3).contains(&row), "the row is not at the page's edge ({row}):\n{screen}");
    let address = "http://192.168.122.1:11434";
    let (at, _) = harness.find(address).expect("the address is on the row");
    let end = at + i32::try_from(address.len()).expect("a length");
    assert_eq!(end, right - 1, "the address does not end at the page's edge:\n{screen}");
    assert!(!reaches_past(&screen, usize::try_from(right).expect("a column")), "text past the page:\n{screen}");

    // Narrower than the page, the page takes the terminal and every label stays whole.
    harness.resize(70, 40).render();
    let screen = harness.screen();
    assert!(screen.contains("orchard") && screen.contains(address), "{screen}");
    let (heading, _) = harness.find("Providers").expect("the heading is drawn");
    assert!(heading < 4, "a narrow terminal keeps the page at its edge:\n{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn the_page_says_each_thing_in_few_words() {
    let path = file("few-words");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    harness.resize(120, 40).render();
    add(&mut harness, "ollama", "orchard", "http://192.168.122.1:11434", None);
    harness.render();
    let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    for said in ["Keys are in plain text in", "a backup carries them.", "nothing else here goes out."] {
        assert!(screen.contains(said), "`{said}` is missing:\n{screen}");
    }
    for gone in ["A backup of your home folder carries them with it", "Nothing else on this page reaches the network"] {
        assert!(!screen.contains(gone), "`{gone}` is still said:\n{screen}");
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn free_models_qcode_saw_work_come_first_with_a_mark_and_those_it_saw_fail_come_last_with_why() {
    let path = file("tried");
    // OpenRouter's own order, which puts a model that answers Claude Code with nothing on top.
    let (web, _) = canned(vec![(
        "/api/v1/models",
        r#"{"data":[
            {"id":"cohere/north-mini-code:free","context_length":256000},
            {"id":"acme/untried-7b:free","context_length":131072},
            {"id":"nex-agi/nex-n2.5-mini:free","context_length":262144},
            {"id":"acme/untried-70b:free","context_length":65536}
        ]}"#,
    )]);
    let mut harness = opened(&path, web);
    harness.resize(120, 40).render();
    add(&mut harness, "OpenRouter", "yol", "https://openrouter.ai", Some(MADE_UP));
    harness.render();
    harness.click_text("Ask what it offers").render();
    let screen = harness.screen();
    let row = |id: &str| screen.lines().position(|line| line.contains(id)).unwrap_or_else(|| panic!("{id}:\n{screen}"));
    let (tried, first, second, broken) = (
        row("nex-agi/nex-n2.5-mini:free"),
        row("acme/untried-7b:free"),
        row("acme/untried-70b:free"),
        row("cohere/north-mini-code:free"),
    );
    assert!(
        tried < first && first < second && second < broken,
        "tried first, the rest as listed, broken last:\n{screen}"
    );
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[tried].contains('✓'), "the tried model is marked:\n{screen}");
    assert!(!lines[first].contains('✓'), "a model nobody tried is not:\n{screen}");
    assert!(lines[broken].contains("Claude Code gets an empty answer"), "why it fails, on its row:\n{screen}");
    // The first row is the one chosen, and the line under the list says in which harnesses.
    assert!(screen.contains("Tried in Claude Code, opencode"), "{screen}");
    // A model the record knows but the service no longer offers is not made up.
    assert!(!screen.contains("dots-studio"), "{screen}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn a_providers_file_written_before_the_record_of_tried_models_opens_in_the_same_order() {
    let path = file("tried-file");
    std::fs::create_dir_all(path.parent().expect("its folder")).expect("a folder");
    let text = "[[provider]]\ntag = \"yol\"\nkind = \"openrouter\"\nbase = \"https://openrouter.ai\"\n\
                [[model]]\ntag = \"yol\"\nid = \"qwen/qwen3.8-27b:free\"\n\
                [[model]]\ntag = \"yol\"\nid = \"acme/untried-7b:free\"\n\
                [[model]]\ntag = \"yol\"\nid = \"poolside/laguna-s-2.1:free\"\n";
    std::fs::write(&path, text).expect("a file");
    let (web, seen) = canned(Vec::new());
    let harness = opened(&path, web);
    let screen = harness.screen();
    let row = |id: &str| screen.lines().position(|line| line.contains(id)).unwrap_or_else(|| panic!("{id}:\n{screen}"));
    assert!(row("poolside/laguna-s-2.1:free") < row("acme/untried-7b:free"), "{screen}");
    assert!(row("acme/untried-7b:free") < row("qwen/qwen3.8-27b:free"), "{screen}");
    assert!(screen.contains("refuses Claude Code's tools"), "{screen}");
    assert_eq!(seen.lock().expect("the list").len(), 0, "the order is QCode's own, asked of nobody");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}
