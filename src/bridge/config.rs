//! Registering the bridge's server in a harness's own settings, beside whatever the person keeps
//! there.
//!
//! Each harness reads the MCP servers it starts from a settings file in its home directory, which
//! in QCode is the project's home volume for the profile. The file belongs to the person and to
//! the harness: either may have written servers, options and comments into it. So the file is
//! never written from scratch. It is read, the one entry named [`SERVER_NAME`] is added when it is
//! missing, and the file is written back only when that changed it:
//!
//! - an entry of that name that already starts this server is left as it is, and nothing is
//!   written;
//! - an entry of that name that starts something else is the person's own, and is left alone;
//!   the bridge then does not reach that harness, and QCode says so;
//! - a file that cannot be read as its format (a comment in a JSON file, a broken TOML) is left
//!   alone the same way, rather than replaced by one that can.
//!
//! JSON files are written back with two-space indentation and the keys in the order they were
//! read. A TOML file is never rewritten: the entry is added as a table of its own at the end, so
//! the person's comments and layout stay exactly as they were.

use serde_json::{Map, Value, json};
use toml::de::{DeTable, DeValue};

use super::{SERVER_NAME, script_in_container};
use crate::engine::run::{EngineError, capture, feed};
use crate::engine::{Engine, Exec};
use crate::profile::{HarnessKind, McpShape};

/// Why the server could not be registered for a harness, which then does not reach the other
/// tabs; its tab works as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unregistered {
    /// The settings file has a server of the same name that starts something else. The file,
    /// relative to the home directory.
    Taken(String),
    /// The settings file cannot be read as its format. The file, and where.
    Unreadable(String, String),
    /// The engine could not read or write the file; its own words.
    Engine(String),
}

/// The exit code of the reading script for a file that is not there.
const ABSENT: i32 = 3;

/// Registers the server in the settings of `harness` inside the running container `container`,
/// whose home is the project's home volume for the profile.
///
/// The file is read and written by a shell in the container, because a volume is only reached
/// through a container, and the new text goes through the shell's input, because a harness's
/// settings can be longer than one argument may be. It is written beside the old file with the
/// old file's mode and moved over it, so a harness reading at that moment finds the old file or
/// the new one, never half of one. Runs engine commands, so it belongs on a background thread.
///
/// # Errors
///
/// [`Unregistered`] when the file holds a server of the same name, cannot be read as its format,
/// or the engine refuses.
pub fn register(engine: &Engine, container: &str, harness: HarnessKind) -> Result<(), Unregistered> {
    let settings = harness.record().mcp;
    let read = ["sh", "-c", "[ -e \"$HOME/$1\" ] || exit 3; cat -- \"$HOME/$1\"", "sh", settings.path];
    let existing = match capture(&engine.exec_without_terminal(&Exec { container, command: &read })) {
        Ok(text) => Some(text),
        Err(EngineError::Failed(failure)) if failure.code == Some(ABSENT) => None,
        Err(error) => return Err(Unregistered::Engine(words(&error))),
    };
    let text = match merge(settings.shape, existing.as_deref()) {
        Merged::Unchanged => return Ok(()),
        Merged::Write(text) => text,
        Merged::Taken => return Err(Unregistered::Taken(settings.path.to_owned())),
        Merged::Unreadable(place) => return Err(Unregistered::Unreadable(settings.path.to_owned(), place)),
    };
    let write = [
        "sh",
        "-c",
        "set -e; file=\"$HOME/$1\"; mkdir -p -- \"$(dirname -- \"$file\")\"; \
         cp -p -- \"$file\" \"$file.qcode-new\" 2>/dev/null || true; \
         cat > \"$file.qcode-new\"; mv -f -- \"$file.qcode-new\" \"$file\"",
        "sh",
        settings.path,
    ];
    feed(&engine.exec_reading(&Exec { container, command: &write }), text.as_bytes())
        .map(|_| ())
        .map_err(|error| Unregistered::Engine(words(&error)))
}

/// What an engine said when it refused, for the person to read.
fn words(error: &EngineError) -> String {
    match error {
        EngineError::NotRunnable { error, .. } => error.to_string(),
        EngineError::Failed(failure) => failure.output.clone(),
        EngineError::Cancelled { .. } => String::new(),
    }
}

/// What registering the server in a settings file came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// The server was registered already; nothing is written.
    Unchanged,
    /// The file as it should be written, with the server registered.
    Write(String),
    /// An entry of the server's name starts something else; the file is left alone.
    Taken,
    /// The file is not the format it should be; it is left alone. Where it could not be read.
    Unreadable(String),
}

/// Registers the server in the settings `existing` holds, in the shape of `shape`; `None` is a
/// file that is not there.
#[must_use]
pub fn merge(shape: McpShape, existing: Option<&str>) -> Merged {
    let existing = existing.filter(|text| !text.trim().is_empty());
    match shape {
        McpShape::Claude => json_merge(existing, &["mcpServers"], &claude_entry(), &Map::new()),
        McpShape::OpenCode => {
            let mut fresh = Map::new();
            fresh.insert("$schema".to_owned(), Value::String("https://opencode.ai/config.json".to_owned()));
            json_merge(existing, &["mcp"], &opencode_entry(), &fresh)
        }
        McpShape::Gemini => json_merge(existing, &["mcpServers"], &gemini_entry(), &Map::new()),
        McpShape::Codex => toml_merge(existing),
    }
}

/// Claude Code's entry: a stdio server by its program and arguments, as `claude mcp add
/// --scope user` writes it into `~/.claude.json`.
fn claude_entry() -> Value {
    json!({ "type": "stdio", "command": "node", "args": [script_in_container()] })
}

/// opencode's entry: a local server by one command line, turned on.
fn opencode_entry() -> Value {
    json!({ "type": "local", "command": ["node", script_in_container()], "enabled": true })
}

/// Gemini CLI's entry. `trust` lets its tools run without asking, like every tool of a harness
/// QCode starts unattended; without it Gemini CLI asks before each message.
fn gemini_entry() -> Value {
    json!({ "command": "node", "args": [script_in_container()], "trust": true })
}

/// Whether the entry `found` starts the same program as `ours`. Only the fields that say what
/// is started are compared, so an entry the harness itself added options to is still ours.
fn same_server(found: &Value, ours: &Value) -> bool {
    ["command", "args"].iter().all(|key| found.get(key) == ours.get(key))
}

/// Adds `entry` under `path` then [`SERVER_NAME`] in the JSON `existing`, or in `fresh` when there
/// is no file.
fn json_merge(existing: Option<&str>, path: &[&str], entry: &Value, fresh: &Map<String, Value>) -> Merged {
    let mut root = match existing {
        None => Value::Object(fresh.clone()),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(value @ Value::Object(_)) => value,
            Ok(_) => return Merged::Unreadable("1:1".to_owned()),
            Err(error) => return Merged::Unreadable(format!("{}:{}", error.line(), error.column())),
        },
    };
    let mut place = &mut root;
    for key in path {
        let Value::Object(object) = place else { return Merged::Taken };
        place = object.entry((*key).to_owned()).or_insert_with(|| Value::Object(Map::new()));
    }
    let Value::Object(servers) = place else { return Merged::Taken };
    match servers.get(SERVER_NAME) {
        Some(found) if same_server(found, entry) => Merged::Unchanged,
        Some(_) => Merged::Taken,
        None => {
            servers.insert(SERVER_NAME.to_owned(), entry.clone());
            let mut text = serde_json::to_string_pretty(&root).unwrap_or_default();
            text.push('\n');
            Merged::Write(text)
        }
    }
}

/// Codex's entry, as the table `codex mcp add` writes into `~/.codex/config.toml`.
fn codex_table() -> String {
    format!("[mcp_servers.{SERVER_NAME}]\ncommand = \"node\"\nargs = [\"{}\"]\n", script_in_container())
}

/// Adds the Codex table to the TOML `existing`, by appending it.
fn toml_merge(existing: Option<&str>) -> Merged {
    let Some(text) = existing else { return Merged::Write(codex_table()) };
    let root = match DeTable::parse(text) {
        Ok(root) => root.into_inner(),
        Err(error) => return Merged::Unreadable(place(text, error.span())),
    };
    if let Some(servers) = root.get("mcp_servers") {
        let DeValue::Table(servers) = servers.get_ref() else { return Merged::Taken };
        if let Some(found) = servers.get(SERVER_NAME) {
            return if codex_is_ours(found.get_ref()) { Merged::Unchanged } else { Merged::Taken };
        }
    }
    let mut written = text.to_owned();
    if !written.ends_with('\n') {
        written.push('\n');
    }
    written.push('\n');
    written.push_str(&codex_table());
    // An inline `mcp_servers = { … }` cannot be added to by a table further down; the result is
    // read again rather than trusted, and a file that would break is left as it is.
    match DeTable::parse(&written) {
        Ok(_) => Merged::Write(written),
        Err(_) => Merged::Taken,
    }
}

/// Whether the Codex entry `found` starts this server.
fn codex_is_ours(found: &DeValue<'_>) -> bool {
    let DeValue::Table(entry) = found else { return false };
    let command = entry.get("command").map(toml::Spanned::get_ref);
    let args = entry.get("args").map(toml::Spanned::get_ref);
    let script = script_in_container();
    let command_is_node = matches!(command, Some(DeValue::String(text)) if text == "node");
    let args_are_ours = match args {
        Some(DeValue::Array(items)) => {
            items.len() == 1 && matches!(items[0].get_ref(), DeValue::String(text) if *text == script)
        }
        _ => false,
    };
    command_is_node && args_are_ours
}

/// `line:column` of the byte where `span` starts in `text`, counted from 1.
fn place(text: &str, span: Option<std::ops::Range<usize>>) -> String {
    let start = span.map_or(0, |span| span.start).min(text.len());
    let before = &text[..text.floor_char_boundary(start)];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |last| last.chars().count()) + 1;
    format!("{line}:{column}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "/run/qcode-mcp/qcode-bridge.mjs";

    fn written(merged: Merged) -> String {
        match merged {
            Merged::Write(text) => text,
            other => panic!("expected a file to write, got {other:?}"),
        }
    }

    fn json(text: &str) -> Value {
        serde_json::from_str(text).expect("the result is JSON")
    }

    #[test]
    fn claude_gets_a_user_server_beside_everything_it_keeps() {
        let existing = r#"{
  "numStartups": 4,
  "mcpServers": { "github": { "type": "stdio", "command": "gh-mcp", "args": [] } },
  "projects": { "/work/Project": { "allowedTools": [] } }
}"#;
        let result = json(&written(merge(McpShape::Claude, Some(existing))));
        assert_eq!(result["numStartups"], 4);
        assert_eq!(result["mcpServers"]["github"]["command"], "gh-mcp", "the person's server stays");
        assert_eq!(result["mcpServers"]["qcode"]["type"], "stdio");
        assert_eq!(result["mcpServers"]["qcode"]["command"], "node");
        assert_eq!(result["mcpServers"]["qcode"]["args"][0], SCRIPT);
        assert!(result["projects"]["/work/Project"].is_object());
        let order: Vec<&String> = result.as_object().expect("an object").keys().collect();
        assert_eq!(order, ["numStartups", "mcpServers", "projects"], "keys stay where they were");
    }

    #[test]
    fn a_missing_or_empty_file_is_made_with_only_the_server() {
        for existing in [None, Some(""), Some("  \n")] {
            let result = json(&written(merge(McpShape::Claude, existing)));
            assert_eq!(
                result,
                json(&format!(
                    r#"{{"mcpServers":{{"qcode":{{"type":"stdio","command":"node","args":["{SCRIPT}"]}}}}}}"#
                ))
            );
        }
        let result = json(&written(merge(McpShape::OpenCode, None)));
        assert_eq!(result["$schema"], "https://opencode.ai/config.json");
        assert_eq!(result["mcp"]["qcode"]["command"], json(&format!(r#"["node","{SCRIPT}"]"#)));
    }

    #[test]
    fn opencode_gets_a_local_server_and_keeps_its_permission() {
        let existing = "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"permission\": {\n    \"*\": \"allow\"\n  },\n  \"mcp\": {\n    \"docs\": { \"type\": \"remote\", \"url\": \"https://example.org/mcp\" }\n  }\n}\n";
        let result = json(&written(merge(McpShape::OpenCode, Some(existing))));
        assert_eq!(result["permission"]["*"], "allow");
        assert_eq!(result["mcp"]["docs"]["type"], "remote");
        assert_eq!(result["mcp"]["qcode"]["type"], "local");
        assert_eq!(result["mcp"]["qcode"]["enabled"], true);
        assert_eq!(result["mcp"]["qcode"]["command"][1], SCRIPT);
    }

    #[test]
    fn gemini_gets_a_trusted_server_and_keeps_its_folder_trust() {
        let existing = "{\n  \"security\": {\n    \"folderTrust\": {\n      \"enabled\": false\n    }\n  }\n}\n";
        let result = json(&written(merge(McpShape::Gemini, Some(existing))));
        assert_eq!(result["security"]["folderTrust"]["enabled"], false);
        assert_eq!(result["mcpServers"]["qcode"]["trust"], true);
        assert_eq!(result["mcpServers"]["qcode"]["args"][0], SCRIPT);
    }

    #[test]
    fn a_json_file_that_has_the_server_already_is_not_written_again() {
        for shape in [McpShape::Claude, McpShape::OpenCode, McpShape::Gemini] {
            let once = written(merge(shape, None));
            assert_eq!(merge(shape, Some(&once)), Merged::Unchanged, "{shape:?}");
        }
        // The harness may add options of its own to the entry; it still starts this server.
        let grown = format!(
            r#"{{"mcpServers":{{"qcode":{{"command":"node","args":["{SCRIPT}"],"trust":true,"timeout":600000}}}}}}"#
        );
        assert_eq!(merge(McpShape::Gemini, Some(&grown)), Merged::Unchanged);
    }

    #[test]
    fn a_server_of_the_same_name_that_starts_something_else_is_the_persons() {
        let theirs = r#"{"mcpServers":{"qcode":{"command":"python3","args":["my-own.py"]}}}"#;
        assert_eq!(merge(McpShape::Claude, Some(theirs)), Merged::Taken);
        assert_eq!(merge(McpShape::Gemini, Some(theirs)), Merged::Taken);
        let theirs = r#"{"mcp":{"qcode":{"type":"local","command":["deno","run","x.ts"]}}}"#;
        assert_eq!(merge(McpShape::OpenCode, Some(theirs)), Merged::Taken);
        // A list of servers that is not an object has no place for an entry.
        assert_eq!(merge(McpShape::Claude, Some(r#"{"mcpServers":[]}"#)), Merged::Taken);
    }

    #[test]
    fn a_json_file_that_does_not_read_as_json_is_left_alone_and_says_where() {
        let commented = "{\n  // the person's note\n  \"theme\": \"dark\"\n}\n";
        assert_eq!(merge(McpShape::Gemini, Some(commented)), Merged::Unreadable("2:3".to_owned()));
        assert_eq!(merge(McpShape::Claude, Some("[1, 2]")), Merged::Unreadable("1:1".to_owned()));
    }

    #[test]
    fn codex_gets_a_table_of_its_own_at_the_end_and_the_rest_stays_byte_for_byte() {
        let existing = "# my settings\napproval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n\n[mcp_servers.github]\ncommand = \"gh-mcp\" # mine\n";
        let result = written(merge(McpShape::Codex, Some(existing)));
        assert!(result.starts_with(existing), "{result}");
        assert!(
            result.ends_with(&format!("\n[mcp_servers.qcode]\ncommand = \"node\"\nargs = [\"{SCRIPT}\"]\n")),
            "{result}"
        );
        assert_eq!(merge(McpShape::Codex, Some(&result)), Merged::Unchanged, "added once");
        let without_end = "model = \"o3\"";
        assert!(
            written(merge(McpShape::Codex, Some(without_end))).starts_with("model = \"o3\"\n\n[mcp_servers.qcode]")
        );
        assert_eq!(written(merge(McpShape::Codex, None)), codex_table());
    }

    #[test]
    fn codex_keeps_a_table_of_the_same_name_the_person_wrote() {
        let theirs = "[mcp_servers.qcode]\ncommand = \"python3\"\nargs = [\"my-own.py\"]\n";
        assert_eq!(merge(McpShape::Codex, Some(theirs)), Merged::Taken);
        let dotted = format!("mcp_servers.qcode.command = \"node\"\nmcp_servers.qcode.args = [\"{SCRIPT}\"]\n");
        assert_eq!(merge(McpShape::Codex, Some(&dotted)), Merged::Unchanged, "the same entry written with dotted keys");
    }

    #[test]
    fn codex_settings_that_would_break_or_do_not_read_are_left_alone() {
        let inline = "mcp_servers = { github = { command = \"gh-mcp\" } }\n";
        assert_eq!(merge(McpShape::Codex, Some(inline)), Merged::Taken);
        assert_eq!(merge(McpShape::Codex, Some("mcp_servers = 3\n")), Merged::Taken);
        assert_eq!(merge(McpShape::Codex, Some("model = \"o3\"\nbroken = \n")), Merged::Unreadable("2:10".to_owned()));
    }
}
