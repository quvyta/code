//! The bridge in real containers: for each harness, on each engine, the server starts in the
//! profile container, answers the protocol, reaches QCode's socket through the read-only mount
//! of a container without the network, and the harness itself finds it in its settings.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`. Each one
//! builds a throwaway profile image, which needs the network once per harness; the containers
//! the checks run in have none, the way a profile without the network runs.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test bridge::live -- --ignored --test-threads=1
//! ```
//!
//! Nobody signs in, so no model is ever asked anything. What the harness is asked is what it
//! answers without one: its list of servers, which for three of the four starts each server and
//! says whether it answered.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde_json::Value;

use super::config;
use super::protocol::{Answer, Listed, Question, Request};
use super::socket::Listener;
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::profile::harness_live::{build, clear};
use crate::profile::identity::Home;
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::ui::project::{ContainerPlan, MCP_DIR, ensure_running};
use crate::workspace::{ProjectId, ProjectPaths};

/// The project the containers here belong to, which nobody has.
const PROJECT: &str = "bridgetest";

/// The engines installed on this machine, or nothing at all when the tests are switched off.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let found: Vec<Engine> =
        [EngineKind::Podman, EngineKind::Docker].into_iter().filter_map(|kind| detect(kind).ok()).collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// A project folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-bridgelive-{name}-{stamp}"));
        std::fs::create_dir_all(path.join("Project")).expect("a project folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    fn paths(&self) -> ProjectPaths {
        ProjectPaths {
            root: self.0.clone(),
            file: self.0.join("project.qcode"),
            project: self.0.join("Project"),
            assets: self.0.join("Assets"),
            harness: self.0.join("Containers").join("Harness"),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A profile of `harness` whose container has no network.
fn profile(harness: HarnessKind) -> Profile {
    Profile {
        name: SafeName::parse(&format!("bridgetest-{}", harness.record().id)).expect("the name is safe"),
        harness,
        template: Template::Recommended,
        account: AccountKind::ApiKey,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
    }
}

/// Runs `script` with a shell in `container` and answers what it printed.
fn run(engine: &Engine, container: &str, script: &str) -> Result<String, String> {
    capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] }))
        .map_err(|error| format!("{error:?}"))
}

/// What the harness itself says about the server once it is registered, and the words that
/// show it found it: started and answering where the harness checks, read back where it only
/// lists.
fn harness_says(harness: HarnessKind) -> (&'static str, &'static [&'static str]) {
    match harness {
        HarnessKind::ClaudeCode => ("claude mcp list", &["qcode", "Connected"]),
        HarnessKind::OpenCode => ("opencode mcp list", &["qcode", "connected"]),
        HarnessKind::GeminiCli => ("gemini mcp list", &["qcode", "Connected"]),
        HarnessKind::Codex => ("codex mcp get qcode", &["enabled: true", "command: node", "qcode-bridge.mjs"]),
    }
}

/// The questions the server is sent, one JSON-RPC message a line: the handshake of the older
/// revisions, both tools, and the same asked the way the 2026-07-28 revision asks.
fn questions() -> Vec<String> {
    let modern = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}"#;
    vec![
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_tabs","arguments":{}}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"send_message","arguments":{"tab":"7","text":"run the tests"}}}"#.to_owned(),
        format!(r#"{{"jsonrpc":"2.0","id":5,"method":"server/discover","params":{{{modern}}}}}"#),
        format!(r#"{{"jsonrpc":"2.0","id":6,"method":"tools/list","params":{{{modern}}}}}"#),
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"1900-01-01"}}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"delete_everything","arguments":{}}}"#.to_owned(),
    ]
}

/// Answers every call QCode's socket gets the way the screen would for one other tab, and hands
/// every call to `seen`; ends when the listener closes.
fn answer_as_qcode(listener: &Listener, seen: mpsc::Sender<Question>) -> std::thread::JoinHandle<()> {
    let inbox = listener.inbox();
    std::thread::spawn(move || {
        while let Some(call) = inbox.next() {
            let Ok(question) = call.question.clone() else {
                call.answer(Answer::refused("malformed".to_owned()));
                continue;
            };
            let answer = match &question.request {
                Request::List => Answer::listed(
                    "one tab".to_owned(),
                    vec![Listed {
                        tab: "7".to_owned(),
                        title: "codex-main".to_owned(),
                        harness: "Codex".to_owned(),
                        profile: "codex-main".to_owned(),
                        network: false,
                    }],
                ),
                Request::Send { tab, text } => Answer::done(format!("queued for {tab}: {text}")),
            };
            call.answer(answer);
            let _ = seen.send(question);
        }
    })
}

/// Sends the questions to the server in `container` under the shell script `wrap`, which is
/// given the server's command as `$1` and the questions in its input, and reads the answers by
/// their ids.
fn converse(engine: &Engine, container: &str, wrap: &str) -> std::collections::HashMap<u64, Value> {
    let questions = questions().join("\n");
    let script = format!("printf '%s\\n' '{}' | {wrap}", questions.replace('\'', "'\\''"),);
    let server = format!("node {MCP_DIR}/{}", super::SCRIPT_NAME);
    let printed = run(engine, container, &script.replace("$1", &server))
        .unwrap_or_else(|said| panic!("{:?}: the server did not run: {said}", engine.kind()));
    let mut answers = std::collections::HashMap::new();
    for line in printed.lines() {
        let value: Value = serde_json::from_str(line).unwrap_or_else(|_| panic!("not one message a line: {line}"));
        if let Some(id) = value["id"].as_u64() {
            answers.insert(id, value);
        }
    }
    answers
}

/// Checks the answers of [`converse`] against the protocol, both revisions.
fn check_answers(kind: EngineKind, answers: &std::collections::HashMap<u64, Value>) {
    assert_eq!(answers.len(), 8, "{kind:?}: every request answered, the notification not: {answers:?}");
    assert_eq!(answers[&1]["result"]["protocolVersion"], "2025-06-18", "{kind:?}: the version asked for is spoken");
    assert!(answers[&1]["result"]["capabilities"]["tools"].is_object());
    assert_eq!(answers[&1]["result"]["serverInfo"]["name"], "qcode");
    let names: Vec<&str> = answers[&2]["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, ["list_tabs", "send_message"]);
    assert_eq!(answers[&3]["result"]["isError"], false);
    assert_eq!(answers[&3]["result"]["structuredContent"]["tabs"][0]["tab"], "7", "{kind:?}: {}", answers[&3]);
    assert_eq!(answers[&4]["result"]["content"][0]["text"], "queued for 7: run the tests", "{kind:?}");
    assert_eq!(answers[&5]["result"]["supportedVersions"][0], "2026-07-28");
    assert_eq!(answers[&5]["result"]["resultType"], "complete");
    assert_eq!(answers[&6]["result"]["resultType"], "complete");
    assert_eq!(answers[&7]["error"]["code"], -32022, "{kind:?}: an unknown revision is refused with the modern error");
    assert_eq!(answers[&8]["error"]["code"], -32602, "{kind:?}: an unknown tool is a protocol error");
}

/// The whole check for one harness, on every engine.
fn verify(harness: HarnessKind) {
    let profile = profile(harness);
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new(harness.record().id);
        let paths = scratch.paths();
        let project = ProjectId::parse(PROJECT).expect("a project id");
        let plan = ContainerPlan::profile(&project, &paths, &profile);
        let home = Home::new(profile.name.clone(), project.clone());
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);

        let listener = Listener::open(&paths.mcp()).expect("the project's socket opens");
        let (seen, questions) = mpsc::channel();
        let answering = answer_as_qcode(&listener, seen);
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));

        // The harness's settings, as the template left them, get the server and keep the rest.
        let settings = harness.record().mcp.path;
        let before = run(&engine, &plan.name, &format!("cat \"$HOME/{settings}\" 2>/dev/null || true")).expect("read");
        config::register(&engine, &plan.name, harness).unwrap_or_else(|trouble| panic!("{kind:?}: {trouble:?}"));
        let after = run(&engine, &plan.name, &format!("cat \"$HOME/{settings}\"")).expect("the settings are there");
        assert!(after.contains("qcode-bridge.mjs"), "{kind:?} {harness:?}: {after}");
        if let Some(template) = harness.record().settings.filter(|template| template.path == settings) {
            assert_eq!(before, template.contents, "{kind:?} {harness:?}: the template's file was there first");
            match harness.record().mcp.shape {
                crate::profile::McpShape::Codex => assert!(after.starts_with(&before), "{after}"),
                _ => {
                    let was: Value = serde_json::from_str(&before).expect("the template is JSON");
                    let now: Value = serde_json::from_str(&after).expect("the settings are JSON");
                    for (key, value) in was.as_object().expect("an object") {
                        assert_eq!(&now[key], value, "{kind:?} {harness:?}: `{key}` of the template is kept");
                    }
                }
            }
        }
        config::register(&engine, &plan.name, harness).expect("registering again is fine");
        let again = run(&engine, &plan.name, &format!("cat \"$HOME/{settings}\"")).expect("read again");
        assert_eq!(again, after, "{kind:?} {harness:?}: registered once, written once");

        // The harness finds it.
        let (command, words) = harness_says(harness);
        let said = run(&engine, &plan.name, &format!("{command} 2>&1"))
            .unwrap_or_else(|said| panic!("{kind:?}: `{command}` failed: {said}"));
        for word in words {
            assert!(said.contains(word), "{kind:?} {harness:?}: `{command}` does not say `{word}`:\n{said}");
        }

        // The server speaks the protocol and reaches the socket, with the token in its own
        // environment, the way a harness that passes its variables on starts it.
        let answers = converse(&engine, &plan.name, "QCODE_BRIDGE=tok-direct $1");
        check_answers(kind, &answers);
        // And with the token only in its parent's, the way Codex starts it.
        let answers = converse(&engine, &plan.name, "QCODE_BRIDGE=tok-parent sh -c 'env -u QCODE_BRIDGE $0' \"$1\"");
        check_answers(kind, &answers);

        drop(listener);
        answering.join().expect("the answering thread ends");
        let tokens: Vec<String> = questions.try_iter().map(|question| question.token).collect();
        assert_eq!(tokens, ["tok-direct", "tok-direct", "tok-parent", "tok-parent"], "{kind:?} {harness:?}");

        capture(&engine.remove_container(&plan.name)).expect("the container is removed");
        capture(&engine.remove_volume(&home.volume())).expect("the home is removed");
        clear(&engine, &profile, &plan.name);
        assert!(!Path::new(&paths.mcp().join(super::SOCKET_NAME)).exists(), "the socket went with the listener");
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_starts_the_bridge_and_reaches_qcode() {
    verify(HarnessKind::ClaudeCode);
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_starts_the_bridge_and_reaches_qcode() {
    verify(HarnessKind::OpenCode);
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn gemini_cli_starts_the_bridge_and_reaches_qcode() {
    verify(HarnessKind::GeminiCli);
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn codex_starts_the_bridge_and_reaches_qcode() {
    verify(HarnessKind::Codex);
}
