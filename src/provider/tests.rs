//! What the providers file promises, and what asking a provider promises.
//!
//! Nothing here reaches the network: every question is put to a [`Web`] built from a string, so
//! the whole of this file runs on a machine with no server anywhere. The keys written here are
//! made up, and the characters spell that they are.

use std::sync::{Arc, Mutex};

use qframe::diagnostics::Severity;

use super::ask::{self, Reached, listing, probing, trial};
use super::{
    AddError, Answer, Ask, AskError, Key, Measured, Model, PermissionProblem, ProviderEntry, ProviderKind, Providers,
    Tag, Web, Wire, permission_problems,
};

/// Not anyone's key: the characters say so.
const MADE_UP: &str = "not-a-real-key-0000-wxyz";

/// A file with one provider of each kind and a model apiece.
const COMPLETE: &str = "\
[[provider]]
tag = \"ev\"
kind = \"ollama\"
base = \"http://192.168.122.1:11434\"
wire = \"anthropic\"

[[provider]]
tag = \"yol\"
kind = \"openrouter\"
base = \"https://openrouter.ai\"
wire = \"anthropic\"
key = \"not-a-real-key-0000-wxyz\"

[[model]]
tag = \"ev\"
id = \"qwen3.8:latest\"
claimed-context = 262144
measured-context = 3012
measured = \"about\"

[[model]]
tag = \"yol\"
id = \"nex-agi/nex-n2.5-mini:free\"
claimed-context = 131072

";

fn tag(name: &str) -> Tag {
    Tag::parse(name).expect("a tag")
}

fn read(text: &str) -> Providers {
    let loaded = Providers::parse("providers.toml", text);
    assert_eq!(loaded.diagnostics, [], "the file is complete");
    loaded.value
}

/// A folder of this machine's temporary directory for this test alone.
fn scratch(what: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("qcode-providers-{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[test]
fn a_complete_file_reads_without_a_word_of_complaint() {
    let providers = read(COMPLETE);
    assert_eq!(providers.entries().len(), 2);
    let home = providers.get("ev").expect("the first provider");
    assert_eq!(home.kind, ProviderKind::Ollama);
    assert_eq!(home.base, "http://192.168.122.1:11434");
    assert_eq!(home.wire, Wire::Anthropic);
    assert_eq!(home.key, None, "a local server needs none");
    let model = home.model("qwen3.8:latest").expect("its model");
    // The two numbers that only mean something together: what the model claims, and what the
    // server was seen to give.
    assert_eq!(model.claimed, Some(262_144));
    assert_eq!(model.measured, Some(Measured::About(3_012)));
    assert!(model.is_short_changed(), "the server gives a fraction of what the model claims");

    let far = providers.get("yol").expect("the second provider");
    assert_eq!(far.kind, ProviderKind::OpenRouter);
    assert_eq!(far.key.as_ref().and_then(Key::last_four).as_deref(), Some("wxyz"));
    assert_eq!(far.model("nex-agi/nex-n2.5-mini:free").expect("its model").measured, None);
}

#[test]
fn what_is_written_reads_back_as_the_same_providers() {
    let providers = read(COMPLETE);
    assert_eq!(providers.to_toml(), COMPLETE);
    assert_eq!(read(&providers.to_toml()).entries(), providers.entries());
}

#[test]
fn a_broken_value_points_at_its_line_and_column_instead_of_panicking() {
    let text = "[[provider]]\ntag = \"ev\"\nkind = \"ollamaa\"\nbase = \"http://h:1\"\n";
    let loaded = Providers::parse("providers.toml", text);
    let at = loaded.diagnostics.iter().find_map(|d| d.location.clone()).expect("the value has a place in the file");
    assert_eq!(at.to_string(), "providers.toml:3:8");
    assert_eq!(loaded.value.entries(), [], "a provider whose kind cannot be read is not asked anything");
}

#[test]
fn a_file_that_is_not_toml_is_reported_not_panicked_over() {
    let loaded = Providers::parse("providers.toml", "[[provider]\ntag = \n");
    assert!(loaded.diagnostics.iter().any(|d| d.severity == Severity::Error), "{:?}", loaded.diagnostics);
    assert_eq!(loaded.value.entries(), []);
}

#[test]
fn a_tag_that_would_read_as_something_else_is_named_where_it_stands() {
    let text = "[[provider]]\ntag = \"ev/makine\"\nkind = \"ollama\"\nbase = \"http://h:1\"\n";
    let loaded = Providers::parse("providers.toml", text);
    assert_eq!(loaded.value.entries(), []);
    let refused = loaded.diagnostics.iter().find(|d| d.severity == Severity::Error).expect("the reason is given");
    assert!(refused.message.contains("ev/makine"), "{}", refused.message);
    assert_eq!(refused.location.as_ref().map(ToString::to_string).as_deref(), Some("providers.toml:2:7"));
}

#[test]
fn two_providers_of_one_tag_would_make_the_tag_mean_two_machines_so_the_second_is_refused() {
    let one = "[[provider]]\ntag = \"ev\"\nkind = \"ollama\"\nbase = \"http://one:1\"\n\n";
    let two = "[[provider]]\ntag = \"ev\"\nkind = \"ollama\"\nbase = \"http://two:2\"\n";
    let loaded = Providers::parse("providers.toml", &format!("{one}{two}"));
    assert_eq!(loaded.value.entries().len(), 1);
    assert_eq!(loaded.value.get("ev").expect("the first").base, "http://one:1", "the first one keeps the tag");
    assert!(loaded.diagnostics.iter().any(|d| d.severity == Severity::Error), "{:?}", loaded.diagnostics);
}

#[test]
fn a_model_of_a_provider_nobody_read_is_reported_and_dropped() {
    let text = "[[provider]]\ntag = \"ev\"\nkind = \"ollama\"\nbase = \"http://h:1\"\n\n\
                [[model]]\ntag = \"yok\"\nid = \"q\"\n";
    let loaded = Providers::parse("providers.toml", text);
    assert_eq!(loaded.value.get("ev").expect("the provider").models, []);
    let warning = loaded.diagnostics.iter().find(|d| d.severity == Severity::Warning).expect("it is said");
    assert!(warning.message.contains("yok"), "{}", warning.message);
}

#[test]
fn a_measured_window_without_its_kind_is_dropped_rather_than_guessed() {
    // A number alone cannot say whether it is the window's edge or only as far as anyone looked,
    // and guessing either way would put a figure on the page that nobody measured.
    let text = "[[provider]]\ntag = \"ev\"\nkind = \"ollama\"\nbase = \"http://h:1\"\n\n\
                [[model]]\ntag = \"ev\"\nid = \"q\"\nmeasured-context = 3012\n";
    let loaded = Providers::parse("providers.toml", text);
    assert_eq!(loaded.value.get("ev").expect("the provider").models[0].measured, None);
    assert!(loaded.diagnostics.iter().any(|d| d.severity == Severity::Warning), "{:?}", loaded.diagnostics);
}

#[test]
fn a_window_that_is_not_a_count_of_tokens_is_reported_and_left_out() {
    let text = "[[provider]]\ntag = \"ev\"\nkind = \"ollama\"\nbase = \"http://h:1\"\n\n\
                [[model]]\ntag = \"ev\"\nid = \"q\"\nclaimed-context = -5\n";
    let loaded = Providers::parse("providers.toml", text);
    assert_eq!(loaded.value.get("ev").expect("the provider").models[0].claimed, None);
    assert!(loaded.diagnostics.iter().any(|d| d.message.contains("-5")), "{:?}", loaded.diagnostics);
}

#[test]
fn nothing_that_prints_a_provider_prints_its_key() {
    let providers = read(COMPLETE);
    let printed = format!("{:?}", providers.entries());
    assert!(!printed.contains("not-a-real-key"), "the key is in what was printed:\n{printed}");
    assert!(printed.contains("…wxyz"), "only the last four characters are there:\n{printed}");
}

#[test]
fn deleting_a_provider_deletes_its_key_and_forgetting_the_key_keeps_the_provider() {
    let mut providers = read(COMPLETE);
    assert!(providers.forget_key("yol"), "there was a key to forget");
    let kept = providers.get("yol").expect("the provider itself stays");
    assert_eq!(kept.key, None);
    assert_eq!(kept.base, "https://openrouter.ai", "its address and its measurements stay");
    assert!(!providers.to_toml().contains("key ="), "nothing of the key is left in the file:\n{}", providers.to_toml());
    assert!(!providers.forget_key("yol"), "there is nothing left to forget");

    assert!(providers.remove("yol"));
    assert_eq!(providers.get("yol"), None);
    assert!(!providers.to_toml().contains("yol"), "{}", providers.to_toml());
    assert!(!providers.remove("yol"));
}

#[test]
fn a_tag_that_is_already_there_is_refused_rather_than_quietly_renamed() {
    let mut providers = read(COMPLETE);
    let same = ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://other:2");
    assert_eq!(providers.add(same), Err(AddError::Taken("ev".to_owned())));
    assert_eq!(providers.entries().len(), 2);
    assert!(providers.add(ProviderEntry::new(tag("baska"), ProviderKind::Ollama, "http://other:2")).is_ok());
    assert_eq!(providers.entries().len(), 3);
}

#[cfg(unix)]
#[test]
fn the_file_is_written_where_only_its_owner_can_open_it() {
    use std::os::unix::fs::PermissionsExt as _;

    let folder = scratch("modes");
    let path = folder.join("quvyta").join("code").join("providers.toml");
    let mut providers = Providers::in_memory().at(&path);
    providers.add(ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://h:1")).expect("a new tag");
    providers.save().expect("the file is written");

    let mode = |place: &std::path::Path| std::fs::metadata(place).expect("it is there").permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600, "nobody else on the machine can read the key");
    assert_eq!(mode(path.parent().expect("its folder")), 0o700);
    assert_eq!(Providers::open(&path).diagnostics, [], "a file written this way has nothing wrong with it");
    assert_eq!(permission_problems(&path), [], "and nothing too wide about it");
    let _ = std::fs::remove_dir_all(&folder);
}

#[cfg(unix)]
#[test]
fn permissions_that_let_others_read_the_key_are_said_out_loud() {
    use std::os::unix::fs::PermissionsExt as _;

    let folder = scratch("widened");
    let path = folder.join("quvyta").join("code").join("providers.toml");
    let mut providers = Providers::in_memory().at(&path);
    providers.add(ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://h:1")).expect("a new tag");
    providers.save().expect("the file is written");

    // What a backup program or an over-helpful `chmod -R` leaves behind.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("the mode is changed");
    let loaded = Providers::open(&path);
    assert_eq!(loaded.value.entries().len(), 1, "the providers are still read; the person is told, not refused");
    let problems = permission_problems(&path);
    assert_eq!(problems, [PermissionProblem { place: path.clone(), mode: 0o644, wanted: 0o600 }]);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("the mode is changed");
    let widened = path.parent().expect("its folder");
    std::fs::set_permissions(widened, std::fs::Permissions::from_mode(0o755)).expect("the mode is changed");
    let problems = permission_problems(&path);
    assert_eq!(problems, [PermissionProblem { place: widened.to_path_buf(), mode: 0o755, wanted: 0o700 }]);

    std::fs::set_permissions(widened, std::fs::Permissions::from_mode(0o700)).expect("the mode is changed");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_file_that_is_not_there_yet_is_an_empty_list_and_not_a_problem() {
    let folder = scratch("absent");
    let loaded = Providers::open(&folder.join("providers.toml"));
    assert_eq!(loaded.diagnostics, []);
    assert_eq!(loaded.value.entries(), []);
}

/// A web that answers every request from `answers`, matched by what the address ends with, and
/// keeps every request it was given.
fn canned(answers: Vec<(&'static str, &'static str)>) -> (Web, Arc<Mutex<Vec<Ask>>>) {
    let seen: Arc<Mutex<Vec<Ask>>> = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    let web = Web::new(move |ask| {
        kept.lock().expect("the list").push(ask.clone());
        let found = answers.iter().find(|(ending, _)| ask.url.ends_with(ending));
        match found {
            Some((_, body)) => Ok(Answer { status: 200, body: (*body).to_owned() }),
            None => Ok(Answer { status: 404, body: format!("nothing answers at {}", ask.url) }),
        }
    });
    (web, seen)
}

fn ollama() -> ProviderEntry {
    ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://192.168.122.1:11434")
}

#[test]
fn an_ollama_server_is_asked_what_it_has_and_what_each_model_claims() {
    let (web, seen) = canned(vec![
        ("/api/tags", r#"{"models":[{"name":"qwen3.8:latest"},{"name":"gemma:2b"}]}"#),
        ("/api/show", r#"{"model_info":{"qwen35.context_length":262144,"qwen35.block_count":48}}"#),
    ]);
    let models = ask::list_models(&web, &ollama()).expect("the server answered");
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<&str>>(), ["qwen3.8:latest", "gemma:2b"]);
    // The claimed window comes from the provider's own record, under whatever name the model's
    // architecture gives it.
    assert_eq!(models[0].claimed, Some(262_144));
    assert!(models.iter().all(|model| model.measured.is_none()), "listing measures nothing");
    let asked: Vec<String> = seen.lock().expect("the list").iter().map(Ask::line).collect();
    assert_eq!(asked[0], "GET http://192.168.122.1:11434/api/tags");
    assert!(asked[1].starts_with("POST http://192.168.122.1:11434/api/show"), "{asked:?}");
}

#[test]
fn openrouter_gives_its_models_and_their_windows_in_one_answer() {
    let body = r#"{"data":[{"id":"a/b","context_length":1048576},{"id":"c/d:free","context_length":131072}]}"#;
    let (web, seen) = canned(vec![("/api/v1/models", body)]);
    let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
    entry.key = Key::new(MADE_UP);
    let models = ask::list_models(&web, &entry).expect("the server answered");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].claimed, Some(1_048_576));
    let asked = seen.lock().expect("the list");
    assert_eq!(asked.len(), 1, "one answer holds every model's window");
    assert_eq!(asked[0].line(), "GET https://openrouter.ai/api/v1/models");
}

#[test]
fn a_model_whose_record_cannot_be_read_keeps_its_place_with_nothing_claimed() {
    let (web, _) = canned(vec![("/api/tags", r#"{"models":[{"name":"q"}]}"#)]);
    let models = ask::list_models(&web, &ollama()).expect("the listing answered");
    assert_eq!(models.len(), 1, "a model that is there is worth naming");
    assert_eq!(models[0].claimed, None, "and nothing is invented about it");
}

#[test]
fn a_server_that_refuses_says_where_and_what_it_said_and_never_the_key() {
    let web = Web::new(|_| Ok(Answer { status: 401, body: "no such key".to_owned() }));
    let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
    entry.key = Key::new(MADE_UP);
    let refused = ask::try_connection(&web, &entry).expect_err("a bad key is refused");
    assert_eq!(refused.url(), "https://openrouter.ai/api/v1/key");
    assert!(matches!(refused, AskError::Refused { status: 401, .. }));
    let printed = format!("{refused:?}");
    assert!(printed.contains("no such key"), "what the server said is kept: {printed}");
    assert!(!printed.contains(MADE_UP), "the key is in the trouble: {printed}");
}

#[test]
fn the_key_travels_in_a_header_and_never_in_an_address() {
    let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
    entry.key = Key::new(MADE_UP);
    for ask in [trial(&entry), probing(&entry, "a/b", 10, "m1"), listing(&entry)] {
        assert!(!ask.url.contains(MADE_UP), "the key is in the address: {}", ask.url);
        assert!(ask.headers.iter().all(|(_, value)| !value.contains(MADE_UP)), "the key is in a plain header");
        assert!(!format!("{ask:?}").contains(MADE_UP), "printing the request prints the key: {ask:?}");
    }
    let carried = trial(&entry).secret.expect("the trial really tries the key");
    assert_eq!(carried.header, "Authorization");
    assert_eq!(carried.key.expose(), MADE_UP, "and the key that goes out is the one that was pasted");
}

#[test]
fn a_local_server_carries_no_secret_at_all() {
    let entry = ollama();
    for ask in [trial(&entry), probing(&entry, "q", 10, "m1"), listing(&entry)] {
        assert!(ask.secret.is_none(), "an ollama server is reached with nothing secret: {}", ask.line());
    }
}

#[test]
fn trying_the_connection_says_what_answered() {
    let (web, seen) = canned(vec![("/api/version", r#"{"version":"0.34.2"}"#)]);
    assert_eq!(ask::try_connection(&web, &ollama()), Ok(Reached::Version("0.34.2".to_owned())));
    assert_eq!(seen.lock().expect("the list")[0].line(), "GET http://192.168.122.1:11434/api/version");

    let (web, _) = canned(vec![("/api/v1/key", r#"{"data":{"label":"a key"}}"#)]);
    let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
    entry.key = Key::new(MADE_UP);
    assert_eq!(ask::try_connection(&web, &entry), Ok(Reached::KeyAccepted));
}

/// A server whose window is `window` tokens: it reads a prompt as one token a word and throws
/// away everything past the window, exactly as the compatibility endpoints were measured doing.
fn server_with_window(window: u64) -> (Web, Arc<Mutex<Vec<usize>>>) {
    let sizes: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&sizes);
    let web = Web::new(move |ask: &Ask| {
        let body = ask.body.as_deref().unwrap_or_default();
        let sent: serde_json::Value = serde_json::from_str(body).expect("a probe is JSON");
        let prompt = sent["messages"][0]["content"].as_str().expect("a prompt");
        let words = prompt.split_whitespace().count();
        kept.lock().expect("the list").push(words);
        let read = u64::try_from(words).expect("a prompt this size fits").min(window);
        Ok(Answer { status: 200, body: format!("{{\"usage\":{{\"input_tokens\":{read}}}}}") })
    });
    (web, sizes)
}

#[test]
fn a_server_that_throws_the_front_of_a_prompt_away_is_caught_at_the_size_it_stops_reading() {
    // 3 012 is what the owner's own server was measured giving for a model claiming 262 144.
    let (web, sizes) = server_with_window(3_012);
    let measured = ask::measure(&web, &ollama(), "qwen3.8:latest").expect("the server answered");
    assert_eq!(measured, Measured::About(3_012), "the edge is where the prompt was cut, not what the model claims");
    assert!(measured.is_cramped(), "a window this size loses a coding agent's own instructions");
    let sent = sizes.lock().expect("the list");
    assert!(sent[0] < sent[1] && sent[1] < sent[2], "the probes grow: {sent:?}");
}

#[test]
fn probing_stops_at_the_first_prompt_that_was_cut_rather_than_sending_one_nothing_can_be_learnt_from() {
    // A window between the first two probes cuts every probe after them to the same place, so
    // the third one would cost a large request and tell nobody anything.
    let (web, sizes) = server_with_window(1_500);
    assert_eq!(ask::measure(&web, &ollama(), "q"), Ok(Measured::About(1_500)));
    assert_eq!(sizes.lock().expect("the list").len(), 2, "the third probe was never sent");
}

#[test]
fn a_server_that_takes_every_probe_whole_is_only_ever_said_to_be_at_least_that_big() {
    let (web, sizes) = server_with_window(1_000_000);
    let measured = ask::measure(&web, &ollama(), "q").expect("the server answered");
    let largest = u64::try_from(*sizes.lock().expect("the list").iter().max().expect("a probe")).expect("it fits");
    assert_eq!(measured, Measured::AtLeast(largest), "where the window ends was not found, so no edge is claimed");
    assert!(!measured.is_cramped());
    assert_eq!(sizes.lock().expect("the list").len(), 4, "every probe was sent because every one came back whole");
}

/// A server whose window is `window` tokens and which remembers the front of a prompt it has
/// already read, counting only what it had to read afresh — which is what the owner's own
/// server does, and what made probes that began alike report a window far under the real one.
fn server_that_remembers(window: u64) -> (Web, Arc<Mutex<Vec<usize>>>) {
    /// The smallest run of words such a server keeps.
    const BLOCK: usize = 64;
    let sizes: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&sizes);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let web = Web::new(move |ask: &Ask| {
        let body = ask.body.as_deref().unwrap_or_default();
        let sent: serde_json::Value = serde_json::from_str(body).expect("a probe is JSON");
        let prompt = sent["messages"][0]["content"].as_str().expect("a prompt");
        let words: Vec<&str> = prompt.split_whitespace().collect();
        kept.lock().expect("the list").push(words.len());
        let read = u64::try_from(words.len()).expect("a prompt this size fits").min(window);
        let read = usize::try_from(read).expect("the window fits a length");
        // How much of what it will read it has read before, as a prefix of an earlier prompt.
        let mut remembered = 0;
        for before in seen.lock().expect("the list").iter() {
            let earlier: Vec<&str> = before.split_whitespace().collect();
            let shared = words.iter().zip(earlier.iter()).take(read).take_while(|(a, b)| a == b).count();
            // A real cache keeps blocks, not single tokens, so a chance word in common is not
            // something the server could answer out of.
            if shared >= BLOCK {
                remembered = remembered.max(shared);
            }
        }
        seen.lock().expect("the list").push(prompt.to_owned());
        let counted = u64::try_from(read - remembered).expect("it fits");
        Ok(Answer { status: 200, body: format!("{{\"usage\":{{\"input_tokens\":{counted}}}}}") })
    });
    (web, sizes)
}

#[test]
fn a_server_that_remembers_what_it_read_before_is_still_measured_at_its_real_window() {
    // 16 384 is what the owner's own server gives through the endpoint a harness uses. Probes
    // that began alike were answered 9 512, and a second measurement 4, because only what was
    // new was counted. Nothing shares an opening, so nothing is remembered.
    let (web, sizes) = server_that_remembers(16_384);
    assert_eq!(ask::measure(&web, &ollama(), "qwen3.8-32k"), Ok(Measured::About(16_384)));
    assert!(sizes.lock().expect("the list").len() >= 2, "the edge was looked for, not guessed");

    // Asking again is a fresh measurement and must not be answered out of the first one.
    assert_eq!(ask::measure(&web, &ollama(), "qwen3.8-32k"), Ok(Measured::About(16_384)), "measured afresh");
}

#[test]
fn a_window_past_the_largest_probe_is_not_called_small_because_a_smaller_probe_fitted() {
    // The probes used to stop at twelve thousand words, so a server stopping at 16 384 was
    // reported as holding at least twelve thousand: true, and read as good news. The largest
    // probe now goes well past any window a harness could use, so the edge is found instead.
    let (web, _) = server_with_window(16_384);
    assert_eq!(ask::measure(&web, &ollama(), "qwen3.8-32k"), Ok(Measured::About(16_384)));
}

#[test]
fn a_probe_asks_for_one_token_back_because_the_question_is_what_the_server_read() {
    let entry = ollama();
    let ask = probing(&entry, "qwen3.8:latest", 50, "m1");
    assert_eq!(ask.line(), "POST http://192.168.122.1:11434/v1/messages");
    let sent: serde_json::Value = serde_json::from_str(ask.body.as_deref().expect("a body")).expect("JSON");
    assert_eq!(sent["max_tokens"], 1);
    assert_eq!(sent["model"], "qwen3.8:latest");
    let prompt = sent["messages"][0]["content"].as_str().expect("a prompt");
    assert_eq!(prompt.split_whitespace().count(), 52, "the fifty words, behind this probe's own opening");
    assert!(prompt.starts_with("m1 50 "), "the opening names the run and the probe's size: {prompt}");
    assert!(ask.headers.iter().any(|(name, _)| name == "anthropic-version"), "{:?}", ask.headers);
}

#[test]
fn an_openai_shaped_provider_is_probed_at_its_own_path_and_read_from_its_own_field() {
    let mut entry = ollama();
    entry.wire = Wire::OpenAi;
    assert_eq!(probing(&entry, "q", 10, "m1").line(), "POST http://192.168.122.1:11434/v1/chat/completions");
    let web = Web::new(|_| Ok(Answer { status: 200, body: r#"{"usage":{"prompt_tokens":4096}}"#.to_owned() }));
    assert_eq!(ask::measure(&web, &entry, "q"), Ok(Measured::About(4_096)), "the third probe was cut to the cap");
}

#[test]
fn an_answer_without_a_token_count_is_named_rather_than_guessed_at() {
    let web = Web::new(|_| Ok(Answer { status: 200, body: r#"{"content":[]}"#.to_owned() }));
    let trouble = ask::measure(&web, &ollama(), "q").expect_err("nothing can be measured from this");
    assert!(
        matches!(trouble, AskError::Unreadable { ref wanted, .. } if wanted == "usage.input_tokens"),
        "{trouble:?}"
    );
}

#[test]
fn an_address_that_cannot_be_reached_says_where_it_was_going() {
    let web =
        Web::new(|ask| Err(AskError::Unreachable { url: ask.url.clone(), reason: "connection refused".to_owned() }));
    let trouble = ask::list_models(&web, &ollama()).expect_err("there is no server");
    assert_eq!(trouble.url(), "http://192.168.122.1:11434/api/tags");
}

#[test]
fn a_model_carries_both_numbers_once_it_has_been_measured() {
    let mut model = Model::new("qwen3.8:latest");
    assert_eq!(model.claimed, None);
    model.claimed = Some(262_144);
    model.measured = Some(Measured::About(3_012));
    let mut providers = Providers::in_memory();
    let mut entry = ollama();
    entry.models = vec![model];
    providers.add(entry).expect("a new tag");
    let written = providers.to_toml();
    assert!(written.contains("claimed-context = 262144"), "{written}");
    assert!(written.contains("measured-context = 3012"), "{written}");
    assert!(written.contains("measured = \"about\""), "{written}");
    assert_eq!(read(&written).get("ev").expect("it").models[0].measured, Some(Measured::About(3_012)));
}

#[test]
fn a_quotation_mark_in_an_address_is_written_the_way_toml_writes_one() {
    let mut providers = Providers::in_memory();
    let mut entry = ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://h:1");
    entry.base = "http://h:1/\"odd\\path".to_owned();
    providers.add(entry).expect("a new tag");
    assert_eq!(read(&providers.to_toml()).get("ev").expect("it").base, "http://h:1/\"odd\\path");
}
