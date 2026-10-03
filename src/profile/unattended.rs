//! What keeps a harness from stopping to ask, merged into the home of every tab as it starts.
//!
//! The container is the wall: a harness inside it reaches nothing of the person's machine whatever
//! it is told to do, so a question before each command buys no safety and costs a keystroke. Every
//! harness QCode starts is therefore started in its unattended mode, and the answers to the
//! questions each of them asks on the way are merged into the home before the harness starts.
//! Leaving them to the image alone would not do, because:
//!
//! - a harness passed an unattended argument still asks whether the folder is trusted, and two of
//!   them put the argument back and ask before every step while the folder is untrusted, so a
//!   home that never had the key has the question in every start;
//! - a home volume is filled from the image the first time and never again, so a key a later
//!   image gained reaches no workspace opened before it;
//! - a profile on a custom set of additions carries no settings at all, since it is the
//!   templates' files the image writes.
//!
//! So the keys are written into the home at every start rather than into the image, which also
//! means no image is rebuilt: the same home, read and written, is what makes a new workspace and
//! an old one behave alike. What each harness is given is the question-related part of what the
//! QCode templates write in [`Harness::settings`](super::Harness::settings) and
//! [`Harness::first_start`](super::Harness::first_start): the update checks and the telemetry
//! those files also turn off are the person's to decide, and are not part of this.
//!
//! Every file is read, the answers are put into it beside whatever the person keeps there, and it
//! is written back only when that changed something. A file that does not read as its format (a
//! comment in a JSON file, a broken TOML) is left as it is and said, rather than replaced by one
//! that can be read, and so is a value of the person's own standing where one of ours would go.
//! JSON is written with two-space indentation and the keys in the order they were read; a TOML
//! file is never rewritten, because the table is added at the end and the person's comments and
//! layout stay byte for byte as they were.
//!
//! A window's application keeps its approvals and its permission grants in its own database rather
//! than in a file of the home, and they are written where that database is prepared; see
//! [`crate::desktop::login`].

use serde_json::{Map, Value, json};
use toml::de::{DeTable, DeValue};

use crate::base::paths::CODE_DIR;
use crate::bridge::config::{self, read_in_home, write_in_home};
use crate::engine::Engine;
use crate::profile::{ConfigFile, HarnessKind};

/// Why a harness's settings could not be brought to what keeps it from asking. Its tab works as
/// before: a harness that asks is worse than one that does not, not one that cannot work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asks {
    /// Something of the person's own stands where one of our answers would go. The file, and what
    /// stands there.
    Theirs(String, String),
    /// The file cannot be read as its format. The file, and where.
    Unreadable(String, String),
    /// The engine could not read or write the file; its own words.
    Engine(String),
}

/// What merging a harness's answers into one of its files came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// The file already holds the answers; nothing is written.
    Unchanged,
    /// The file as it should be written, with the answers in it.
    Write(String),
    /// Something of the person's own stands where one of our answers would go; the file is left as
    /// it is. What stands there.
    Theirs(String),
    /// The file is not the format it should be; it is left as it is. Where it could not be read.
    Unreadable(String),
}

/// One file of a harness's home, and the merge that keeps a question out of it.
struct Answer {
    /// Where the file is, taken from the harness's own record so the two cannot drift apart. A
    /// record that names no such file leaves the harness as it is rather than guessing a path.
    path: &'static str,
    /// What is put into whatever the file already holds.
    merge: fn(Option<&str>) -> Merged,
}

/// The file of a record that `merge` belongs to, where the record names one.
fn answer(file: Option<ConfigFile>, merge: fn(Option<&str>) -> Merged) -> Option<Answer> {
    Some(Answer { path: file?.path, merge })
}

/// Every file of `harness`'s home that is merged into, in the order they are read.
fn answers(harness: HarnessKind) -> Vec<Answer> {
    let record = harness.record();
    let named: Vec<Option<Answer>> = match harness {
        HarnessKind::ClaudeCode => {
            vec![answer(record.settings, merge_claude_settings), answer(record.first_start, merge_claude_first_start)]
        }
        HarnessKind::OpenCode => vec![answer(record.settings, merge_opencode_permissions)],
        HarnessKind::GeminiCli | HarnessKind::QwenCode => vec![answer(record.settings, merge_folder_trust)],
        HarnessKind::Codex => vec![answer(record.settings, merge_codex_trust)],
        HarnessKind::KimiCode => vec![answer(record.first_start, merge_kimi_trust)],
        // A window's application keeps its approvals in its own database, which is written where
        // that home is prepared; see `crate::desktop::login`.
        HarnessKind::AntigravityIde => Vec::new(),
    };
    named.into_iter().flatten().collect()
}

/// Brings the home of the running container `container` to what keeps `harness` from asking, and
/// says what could not be done.
///
/// A harness that draws in a terminal is what this is for; a window's application keeps its own
/// answers elsewhere. Every key comes from the harness's own record, so a harness whose files are
/// named there cannot be missed, and the same merge runs under every template and in a home that
/// was made before the key existed.
///
/// Runs engine commands, so it belongs on a background thread.
///
/// # Errors
///
/// [`Asks`] when something of the person's own stands in the way, when a file cannot be read as
/// its format, or when the engine refuses.
pub fn ensure(engine: &Engine, container: &str, harness: HarnessKind) -> Result<(), Asks> {
    for Answer { path, merge } in answers(harness) {
        let existing = read_in_home(engine, container, path).map_err(|error| Asks::Engine(config::words(&error)))?;
        let text = match merge(existing.as_deref()) {
            Merged::Unchanged => continue,
            Merged::Write(text) => text,
            Merged::Theirs(says) => return Err(Asks::Theirs(path.to_owned(), says)),
            Merged::Unreadable(place) => return Err(Asks::Unreadable(path.to_owned(), place)),
        };
        write_in_home(engine, container, path, &text).map_err(|error| Asks::Engine(config::words(&error)))?;
    }
    Ok(())
}

/// Claude Code's settings file: the mode every tool runs in, and the warning about that mode,
/// which is a question QCode raises itself and whose highlighted answer leaves.
pub fn merge_claude_settings(existing: Option<&str>) -> Merged {
    json_keys(
        existing,
        &[
            (&["permissions", "defaultMode"], json!("bypassPermissions")),
            (&["skipDangerousModePermissionPrompt"], json!(true)),
        ],
    )
}

/// Claude Code's record of the questions its very first start asked: whether the workspace is
/// trusted, whether the text style is still to be chosen, and whether it has offered to trade the
/// mode for its auto mode. The bridge registers its server in this same file, so both merge into it.
///
/// The offer came with 2.1.285: a home whose user settings name a mode of their own, as the QCode
/// templates' do, is asked "Make auto mode your default permission mode?" with "Yes" highlighted,
/// and auto mode is one in which the agent checks each step and blocks what it judges risky.
/// `hasSeenAutoDefaultNudge` is the global record the program reads before it offers, and the one
/// it writes once the offer is answered either way.
pub fn merge_claude_first_start(existing: Option<&str>) -> Merged {
    json_keys(
        existing,
        &[
            (&["hasCompletedOnboarding"], json!(true)),
            (&["projects", CODE_DIR, "hasTrustDialogAccepted"], json!(true)),
            (&["hasSeenAutoDefaultNudge"], json!(true)),
        ],
    )
}

/// opencode's permissions: the wildcard allowed, and every other answer that would ask turned into
/// one that does not. A `deny` is not a question, so it is kept as the person wrote it.
pub fn merge_opencode_permissions(existing: Option<&str>) -> Merged {
    let mut root = match root_of(existing) {
        Ok(root) => root,
        Err(place) => return Merged::Unreadable(place),
    };
    let mut changed = false;
    {
        // Where the key already is in the file, so the person's own order survives.
        let entry = root.entry("permission".to_owned()).or_insert_with(|| Value::Object(Map::new()));
        // One word for every permission is not a list the wildcard can be added to. A refusal is
        // not a question and stands; anything else is a question, and becomes the list.
        if let Some(answer) = entry.as_str() {
            if answer == "deny" {
                return Merged::Unchanged;
            }
            *entry = Value::Object(Map::new());
            changed = true;
        }
        let Value::Object(answers) = entry else {
            return Merged::Theirs("permission is one answer, not a list of them".to_owned());
        };
        if answers.get("*").and_then(Value::as_str) != Some("allow") {
            answers.insert("*".to_owned(), json!("allow"));
            changed = true;
        }
        for (pattern, answer) in answers.iter_mut() {
            if pattern != "*" && answer.as_str() == Some("ask") {
                *answer = json!("allow");
                changed = true;
            }
        }
    }
    written(root, changed)
}

/// Gemini CLI's and Qwen Code's folder trust, which is what puts their unattended argument back:
/// each of them returns the mode to its default in a folder nobody has trusted, and the workspace
/// directory of a fresh container is a folder nobody has.
pub fn merge_folder_trust(existing: Option<&str>) -> Merged {
    json_keys(existing, &[(&["security", "folderTrust", "enabled"], json!(false))])
}

/// Codex's answer to whether the workspace is trusted, in the one file it keeps every answer in.
///
/// The table is added at the end rather than written into the file, so the person's comments and
/// layout stay as they were. A table that says something else is the person's own answer: Codex
/// asks about the folder only where it has none, and a workspace it does not trust may be one the
/// person means to be asked about.
pub fn merge_codex_trust(existing: Option<&str>) -> Merged {
    let table = format!("[projects.\"{CODE_DIR}\"]\ntrust_level = \"trusted\"\n");
    let Some(text) = existing.filter(|text| !text.trim().is_empty()) else { return Merged::Write(table) };
    let root = match DeTable::parse(text) {
        Ok(root) => root.into_inner(),
        Err(error) => return Merged::Unreadable(config::place(text, error.span())),
    };
    if let Some(projects) = root.get("projects") {
        let DeValue::Table(projects) = projects.get_ref() else {
            return Merged::Theirs("projects is not a table of folders".to_owned());
        };
        if let Some(found) = projects.get(CODE_DIR) {
            return if trusted(found.get_ref()) { Merged::Unchanged } else { Merged::Theirs(WORKS.to_owned()) };
        }
    }
    let mut written = text.to_owned();
    if !written.ends_with('\n') {
        written.push('\n');
    }
    written.push('\n');
    written.push_str(&table);
    // An inline `projects = { … }` cannot be added to by a table further down, and a file that
    // would stop reading is not one to write into.
    match DeTable::parse(&written) {
        Ok(_) => Merged::Write(written),
        Err(_) => Merged::Theirs("projects is written in a way the table cannot be added to".to_owned()),
    }
}

/// What is said of a workspace Codex already has an answer about that is not trust.
const WORKS: &str = "the workspace is trusted under another level";

/// Whether Codex's entry for the workspace folder says it is trusted.
fn trusted(found: &DeValue<'_>) -> bool {
    let DeValue::Table(entry) = found else { return false };
    matches!(entry.get("trust_level").map(toml::Spanned::get_ref), Some(DeValue::String(level)) if level == "trusted")
}

/// Kimi Code CLI's record of the folder it trusts, a file it reads whole. There is nothing to merge
/// into it, so it is written as the record holds it when it is not there, and an answer that is
/// there is one the harness or the person wrote, either of which counts.
pub fn merge_kimi_trust(existing: Option<&str>) -> Merged {
    if existing.is_some_and(|text| !text.trim().is_empty()) {
        return Merged::Unchanged;
    }
    let Some(answer) = HarnessKind::KimiCode.record().first_start else { return Merged::Unchanged };
    Merged::Write(answer.contents.to_owned())
}

/// Puts each of `keys` into the JSON `existing`, or makes the file when there is none.
fn json_keys(existing: Option<&str>, keys: &[(&[&str], Value)]) -> Merged {
    let mut root = match root_of(existing) {
        Ok(root) => root,
        Err(place) => return Merged::Unreadable(place),
    };
    let mut changed = false;
    for (path, value) in keys {
        match put(&mut root, path, value.clone()) {
            Ok(one) => changed |= one,
            Err(stands) => return Merged::Theirs(stands),
        }
    }
    written(root, changed)
}

/// The object a JSON file holds, or a new one when there is no file; where the reading of it went
/// wrong when what is there is not one.
fn root_of(existing: Option<&str>) -> Result<Map<String, Value>, String> {
    let Some(text) = existing.filter(|text| !text.trim().is_empty()) else { return Ok(Map::new()) };
    match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) => match value {
            Value::Object(object) => Ok(object),
            _ => Err("1:1".to_owned()),
        },
        Ok(_) => Err("1:1".to_owned()),
        Err(error) => Err(format!("{}:{}", error.line(), error.column())),
    }
}

/// Sets `value` at `path`, making the objects in between, and answers whether that changed
/// anything.
///
/// The name of what stands in the way, when the last name on the path is not a list of answers
/// but something of the person's own: writing ours there would lose theirs.
fn put(root: &mut Map<String, Value>, path: &[&str], value: Value) -> Result<bool, String> {
    let Some((last, above)) = path.split_last() else { return Ok(false) };
    let mut place = root;
    for key in above {
        let entry = place.entry((*key).to_owned()).or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(object) = entry else { return Err(format!("{key} is one answer, not a list of them")) };
        place = object;
    }
    if place.get(*last) == Some(&value) {
        return Ok(false);
    }
    place.insert((*last).to_owned(), value);
    Ok(true)
}

/// The file as it should be written, or that it is already as it should be.
fn written(root: Map<String, Value>, changed: bool) -> Merged {
    if !changed {
        return Merged::Unchanged;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
    text.push('\n');
    Merged::Write(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The merge of one file of one harness's home.
    struct File {
        harness: HarnessKind,
        path: &'static str,
        merge: fn(Option<&str>) -> Merged,
    }

    /// Every file of every harness, as the harness's own record names it.
    fn files() -> Vec<File> {
        HarnessKind::ALL
            .into_iter()
            .flat_map(|harness| {
                answers(harness).into_iter().map(move |answer| File { harness, path: answer.path, merge: answer.merge })
            })
            .collect()
    }

    fn written(merged: Merged) -> String {
        match merged {
            Merged::Write(text) => text,
            other => panic!("expected a file to write, got {other:?}"),
        }
    }

    fn json(text: &str) -> Value {
        serde_json::from_str(text).expect("the merged file is JSON")
    }

    /// What Codex is given: the one table it keeps its answers in.
    fn table() -> String {
        format!("[projects.\"{CODE_DIR}\"]\ntrust_level = \"trusted\"\n")
    }

    /// What the file holds once every key is in it, which is what the answer for every JSON
    /// harness is: the mode, the folder trusted, and the question about the mode closed.
    fn answered_here(root: &Value) -> bool {
        let mode = root["permissions"]["defaultMode"] == json!("bypassPermissions")
            || root["permission"]["*"] == json!("allow")
            || root["security"]["folderTrust"]["enabled"] == json!(false);
        let onboarding = root["hasCompletedOnboarding"] == json!(true)
            && root["projects"][CODE_DIR]["hasTrustDialogAccepted"] == json!(true)
            && root["hasSeenAutoDefaultNudge"] == json!(true);
        mode || onboarding
    }

    #[test]
    fn every_harness_that_draws_in_a_terminal_has_a_file_of_its_own_to_answer_in() {
        // Under `base` a workspace's home is filled from an image that carries none of this, so a
        // harness whose record names no file to merge into would be asked in every start.
        let named: Vec<&'static str> = HarnessKind::TERMINAL
            .into_iter()
            .flat_map(|harness| answers(harness).into_iter().map(|a| a.path))
            .collect();
        assert_eq!(
            named,
            [
                ".claude/settings.json",
                ".claude.json",
                ".config/opencode/opencode.json",
                ".gemini/settings.json",
                ".codex/config.toml",
                ".kimi-code/workspace-trust/wd_work_0c9a453fad61",
                ".qwen/settings.json",
            ]
        );
        for file in files() {
            let record = file.harness.record();
            let known =
                [record.settings, record.first_start].into_iter().flatten().any(|known| known.path == file.path);
            assert!(known, "{:?}: {} is not in the record", file.harness, file.path);
        }
        // A window's application keeps its own answers in its database, and nothing is read for it.
        assert!(answers(HarnessKind::AntigravityIde).is_empty());
        assert_eq!(
            ensure(&Engine::new(crate::engine::EngineKind::Podman, "/no/engine"), "c", HarnessKind::AntigravityIde),
            Ok(())
        );
    }

    #[test]
    fn a_home_with_nothing_in_it_is_given_the_answers() {
        for file in files() {
            for existing in [None, Some(""), Some("  \n")] {
                let merged = (file.merge)(existing);
                let text = written(merged);
                assert!(!text.trim().is_empty(), "{:?} {}: nothing was written", file.harness, file.path);
                let again = (file.merge)(Some(&text));
                assert_eq!(again, Merged::Unchanged, "{:?} {}: written again", file.harness, file.path);
                if file.path.ends_with(".json") {
                    assert!(answered_here(&json(&text)), "{:?}: {text}", file.harness);
                }
            }
        }
    }

    #[test]
    fn the_answers_a_harness_is_given_are_the_ones_its_own_record_holds() {
        // What the templates write into the image is the same set of keys, so a home filled from
        // an image already answers every question: nothing is written into it again at a start.
        for harness in HarnessKind::TERMINAL {
            for answer in answers(harness) {
                let record = harness.record();
                let file = [record.settings, record.first_start].into_iter().flatten().find(|f| f.path == answer.path);
                let file = file.expect("the record names the file");
                let merged = (answer.merge)(Some(file.contents));
                assert_eq!(
                    merged,
                    Merged::Unchanged,
                    "{harness:?} {}: the image's own file is written again",
                    file.path
                );
            }
        }
    }

    #[test]
    fn what_the_person_keeps_in_a_file_stays_where_it_was_and_its_keys_keep_their_order() {
        let existing = r#"{
  "numStartups": 4,
  "mcpServers": { "github": { "command": "gh-mcp" } },
  "projects": { "/work": { "allowedTools": [] } }
}"#;
        let result = json(&written(merge_claude_first_start(Some(existing))));
        assert_eq!(result["numStartups"], 4);
        assert_eq!(result["mcpServers"]["github"]["command"], "gh-mcp", "the person's server stays");
        assert_eq!(result["projects"]["/work"]["allowedTools"], json!([]));
        assert_eq!(result["projects"]["/work"]["hasTrustDialogAccepted"], true);
        let order: Vec<&String> = result.as_object().expect("an object").keys().collect();
        assert_eq!(
            order,
            ["numStartups", "mcpServers", "projects", "hasCompletedOnboarding", "hasSeenAutoDefaultNudge"],
            "keys stay where they were"
        );

        let settings = r#"{
  "theme": "dark",
  "permissions": { "allow": ["Bash(ls:*)"] },
  "env": { "http_proxy": "localhost:3128" }
}"#;
        let result = json(&written(merge_claude_settings(Some(settings))));
        assert_eq!(result["theme"], "dark");
        assert_eq!(result["permissions"]["allow"], json!(["Bash(ls:*)"]), "their own list is kept");
        assert_eq!(result["permissions"]["defaultMode"], "bypassPermissions");
        assert_eq!(result["skipDangerousModePermissionPrompt"], true);
        assert_eq!(result["env"]["http_proxy"], "localhost:3128");
    }

    #[test]
    fn opencode_is_told_to_allow_the_wildcard_and_every_answer_that_would_ask() {
        let existing = r#"{
  "$schema": "https://opencode.ai/config.json",
  "permission": { "edit": "ask", "bash": "deny", "webfetch": "allow" },
  "mcp": { "docs": { "type": "remote" } }
}"#;
        let result = json(&written(merge_opencode_permissions(Some(existing))));
        assert_eq!(result["$schema"], "https://opencode.ai/config.json");
        assert_eq!(result["mcp"]["docs"]["type"], "remote", "the person's server stays");
        assert_eq!(result["permission"]["*"], "allow");
        assert_eq!(result["permission"]["edit"], "allow", "an ask is a question, and stops being one");
        assert_eq!(result["permission"]["webfetch"], "allow", "an answer of their own stays");
        assert_eq!(result["permission"]["bash"], "deny", "a refusal is not a question");
        let order: Vec<&String> = result.as_object().expect("an object").keys().collect();
        assert_eq!(order, ["$schema", "permission", "mcp"], "keys stay where they were");
        // A file of its own is not written again, whatever the person left in it.
        for text in [
            "{\"permission\":{\"*\":\"allow\"}}\n",
            "{\"permission\":{\"*\":\"allow\",\"edit\":\"allow\",\"bash\":\"deny\"}}\n",
        ] {
            assert_eq!(merge_opencode_permissions(Some(text)), Merged::Unchanged, "{text}");
        }
    }

    #[test]
    fn opencode_one_answer_for_the_whole_file_becomes_the_list_the_wildcard_belongs_in() {
        // The whole file may be answered with one word, which is not a list to add a key to; a
        // refusal stands, and anything else would be a question before every step.
        assert_eq!(
            json(&written(merge_opencode_permissions(Some("{\"permission\":\"ask\"}"))))["permission"]["*"],
            "allow"
        );
        assert_eq!(
            merge_opencode_permissions(Some("{\"permission\":\"deny\"}")),
            Merged::Unchanged,
            "a whole-file refusal is left as it is"
        );
        let other = merge_opencode_permissions(Some("{\"permission\":[\"ask\"]}"));
        assert!(matches!(other, Merged::Theirs(_)), "{other:?}");
    }

    #[test]
    fn codex_gets_its_table_at_the_end_and_the_rest_of_the_file_stays_byte_for_byte() {
        let existing =
            "# my settings\napproval_policy = \"never\"\n\n[mcp_servers.github]\ncommand = \"gh-mcp\" # mine\n";
        let result = written(merge_codex_trust(Some(existing)));
        assert!(result.starts_with(existing), "{result}");
        assert!(result.ends_with("[projects.\"/work\"]\ntrust_level = \"trusted\"\n"), "{result}");
        assert_eq!(merge_codex_trust(Some(&result)), Merged::Unchanged, "added once");
        assert_eq!(
            merge_codex_trust(None),
            Merged::Write(table()),
            "a file that is not there holds nothing but the table"
        );
        // A file without a last newline still gets one before the table.
        assert_eq!(
            written(merge_codex_trust(Some("model = \"o3\""))),
            "model = \"o3\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n"
        );
        // A dotted key is the same table, so a workspace trusted already is nothing to write.
        assert_eq!(merge_codex_trust(Some("projects.\"/work\".trust_level = \"trusted\"\n")), Merged::Unchanged);
    }

    #[test]
    fn codex_an_answer_of_the_persons_own_about_the_workspace_is_left_and_said() {
        // A table of the person's own that says something else is left as it is: Codex asks about
        // the folder only where it has no answer, and a workspace it does not trust may be one the
        // person means to be asked about.
        assert!(matches!(
            merge_codex_trust(Some("[projects.\"/work\"]\ntrust_level = \"untrusted\"\n")),
            Merged::Theirs(_)
        ));
        assert_eq!(
            merge_codex_trust(Some("projects = { \"/work\" = { trust_level = \"trusted\" } }\n")),
            Merged::Unchanged
        );
        assert!(matches!(merge_codex_trust(Some("projects = 3\n")), Merged::Theirs(_)), "not a table at all");
        // A table of the person's own that is empty is not an answer, and the table QCode adds
        // would be a second one of the same name: the file is left alone and said.
        let theirs = merge_codex_trust(Some("[projects.\"/work\"]\n"));
        assert!(matches!(theirs, Merged::Theirs(_)), "{theirs:?}");
    }

    #[test]
    fn a_file_that_does_not_read_as_its_format_is_left_alone_and_says_where() {
        let commented = "{\n  // the person's note\n  \"theme\": \"dark\"\n}\n";
        assert_eq!(merge_folder_trust(Some(commented)), Merged::Unreadable("2:3".to_owned()));
        assert_eq!(merge_claude_settings(Some("[1, 2]")), Merged::Unreadable("1:1".to_owned()));
        assert_eq!(merge_codex_trust(Some("model = \"o3\"\nbroken = \n")), Merged::Unreadable("2:10".to_owned()));
        // Kimi Code CLI's file is read whole and is never a question about its shape.
        assert_eq!(merge_kimi_trust(Some("half a record")), Merged::Unchanged);
    }

    #[test]
    fn a_value_of_the_persons_own_where_our_answer_would_go_is_left_alone_and_said() {
        let theirs = merge_claude_first_start(Some("{\"projects\": \"/work\"}"));
        assert!(matches!(theirs, Merged::Theirs(_)), "{theirs:?}");
        let security = merge_folder_trust(Some("{\"security\": \"off\"}"));
        assert!(matches!(security, Merged::Theirs(_)), "{security:?}");
        // The value itself is read before anything is written, so nothing half-done is left.
        let settings = merge_claude_settings(Some("{\"permissions\": 3}"));
        assert!(matches!(settings, Merged::Theirs(_)), "{settings:?}");
    }

    #[test]
    fn kimi_codes_record_is_written_when_it_is_missing_and_left_when_it_is_there() {
        let wanted = HarnessKind::KimiCode.record().first_start.expect("the trust question is answered");
        let once = written(merge_kimi_trust(None));
        assert_eq!(once, wanted.contents);
        assert_eq!(merge_kimi_trust(Some(&once)), Merged::Unchanged);
        let theirs = "{\"root\":\"/elsewhere\",\"trustedAt\":1757000000000}\n";
        assert_eq!(merge_kimi_trust(Some(theirs)), Merged::Unchanged, "their own answer stands");
    }
}
