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

/// An application on its home screen whose providers screen reads `path` and asks through `web`.
fn app(path: &std::path::Path, web: Web) -> Harness<QCode> {
    let store = testing::scratch("providers-store");
    let app = testing::app(testing::config(&store, &[]), &testing::settled(), None)
        .with_providers(Some(path.to_path_buf()), web);
    testing::harness(app, SIZE.0, SIZE.1)
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
    assert!(screen.contains("does not read as one address"), "the reason is beside the field:\n{screen}");
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
    assert!(screen.contains("too many requests right now"), "the person is told what 429 means:\n{screen}");
    assert!(screen.contains("Wait a minute"), "and what to do meanwhile:\n{screen}");
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
fn turkish_reads_as_turkish() {
    let path = file("turkish");
    let (web, _) = canned(Vec::new());
    let mut harness = opened(&path, web);
    harness.set_locale("tr").render();
    let screen = harness.screen();
    assert!(screen.contains("Sağlayıcılar"), "{screen}");
    assert!(screen.contains("düz metin"), "the key-file line is Turkish too:\n{screen}");
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
