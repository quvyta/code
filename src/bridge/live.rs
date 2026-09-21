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
//! Nobody signs in, so no model is asked anything but in one test. What the harness is asked is
//! what it answers without one: its list of servers, which for three of the four starts each
//! server and says whether it answered. The exception is the delivery into Claude Code, which
//! draws no prompt without a model to speak to; it is given one through the provider relay and
//! also needs `QCODE_PROVIDER_URL` and `QCODE_PROVIDER_MODEL`, as [`crate::provider::relay_live`]
//! does.

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
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, MCP_DIR, ensure_running};

/// The workspace the containers here belong to, which nobody has.
const WORKSPACE: &str = "bridgetest";

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

/// A workspace folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-bridgelive-{name}-{stamp}"));
        std::fs::create_dir_all(path.join("Work")).expect("a workspace folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    fn paths(&self) -> WorkspacePaths {
        WorkspacePaths {
            root: self.0.clone(),
            file: self.0.join("workspace.qcode"),
            code: self.0.join("Work"),
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
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
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
        // A harness that opens a window runs no agent of QCode's, so it is never registered and
        // never asked. `verify` is called for the four that are.
        HarnessKind::AntigravityIde => unreachable!("a window harness is not registered"),
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
                        waiting: 1,
                        trouble: Some("the coding tool in that tab is not running".to_owned()),
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

/// The token the tab whose settings are written would have. Nothing but a running QCode hands
/// one out, so a file holding it can only have been written here.
const TOKEN: &str = "5b3e0c7a91d4f26803ae5d17c94b6f20";

/// The whole check for one harness, on every engine.
fn verify(harness: HarnessKind) {
    let profile = profile(harness);
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new(harness.record().id);
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace.clone());
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);

        let listener = Listener::open(&paths.mcp()).expect("the workspace's socket opens");
        let (seen, questions) = mpsc::channel();
        let answering = answer_as_qcode(&listener, seen);
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));

        // The harness's settings, as the template left them, get the server and keep the rest.
        let mcp = harness.record().mcp.expect("a harness with an agent reads servers from a file");
        let settings = mcp.path;
        let before = run(&engine, &plan.name, &format!("cat \"$HOME/{settings}\" 2>/dev/null || true")).expect("read");
        config::register(&engine, &plan.name, harness, TOKEN).unwrap_or_else(|trouble| panic!("{kind:?}: {trouble:?}"));
        let after = run(&engine, &plan.name, &format!("cat \"$HOME/{settings}\"")).expect("the settings are there");
        assert!(after.contains("qcode-bridge.mjs"), "{kind:?} {harness:?}: {after}");
        // Claude Code's servers go into the file QCode's templates answer its first start in, so
        // the file is there before the bridge is, and every answer in it has to stay.
        if let Some(template) = Template::Recommended.files(harness).into_iter().find(|file| file.path == settings) {
            assert_eq!(before, template.contents, "{kind:?} {harness:?}: the template's file was there first");
            match mcp.shape {
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
        config::register(&engine, &plan.name, harness, TOKEN).expect("registering again is fine");
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

/// A screen that draws nothing but one tab's terminal, so that what a real harness has drawn can
/// be read back. The framework's session hands out no screen of its own: what the person sees is
/// what this widget draws, so this is where a delivery has to be looked for.
struct Watched(qframe::widgets::TerminalSession);

impl qframe::prelude::App for Watched {
    type Msg = ();

    fn update(&mut self, (): ()) -> qframe::prelude::Command<()> {
        qframe::prelude::Command::none()
    }

    fn view(&self, ui: &mut qframe::prelude::View<'_, ()>) {
        ui.add(qframe::widgets::Terminal::new(&self.0).read_only()).fill_width().fill_height();
    }
}

/// Draws the tab again and again until `wanted` is on it, giving a real harness a generous while
/// to get there: it starts a runtime, reads its settings and draws its first screen.
fn until_shown(screen: &mut qframe::runtime::Harness<Watched>, wanted: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
    loop {
        screen.render();
        let drawn = screen.screen();
        if drawn.contains(wanted) || std::time::Instant::now() > deadline {
            return drawn;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Waits for the harness's own prompt, answering with Return whatever it asks on its very first
/// start — Claude Code picks a text style before it shows a prompt, and a person answers that
/// question the same way. The questions Return answers wrongly are the ones whose first choice is
/// "No, exit" — Claude Code's folder trust, and its warning about the permission mode QCode starts
/// it in: a person moves down to the "Yes" before pressing it, and so does this. The answer is
/// typed into the harness, not sent past it: what is being checked is the screen a person would
/// be looking at.
fn until_prompt(
    screen: &mut qframe::runtime::Harness<Watched>,
    session: &qframe::widgets::TerminalSession,
    harness: HarnessKind,
) -> String {
    let wanted = prompt_of(harness);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
    let mut answered = 0;
    let mut before = String::new();
    loop {
        screen.render();
        let drawn = screen.screen();
        if drawn.contains(wanted) || std::time::Instant::now() > deadline {
            return drawn;
        }
        // Still on the same screen and no prompt: it is waiting for an answer, not working.
        if drawn == before && answered < 6 {
            answered += 1;
            if drawn.contains("❯ No, exit") {
                let _ = session.write(b"\x1b[B");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            let _ = session.write(b"\r");
        }
        before = drawn;
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

/// Words the harness itself draws once its own prompt is up and it is reading what is typed,
/// taken from the screen each one really drew in a container.
fn prompt_of(harness: HarnessKind) -> &'static str {
    match harness {
        // Claude Code draws its prompt only once it has a model to speak to: without the network
        // and without a provider it quits before drawing anything. Through the provider relay it
        // asks nobody to sign in; after the folder trust and permission mode questions it draws
        // an empty prompt with this line of the permission mode under it.
        HarnessKind::ClaudeCode => "shift+tab to cycle",
        HarnessKind::OpenCode => "Ask anything",
        HarnessKind::GeminiCli => "Type your message",
        HarnessKind::Codex => "To get started",
        HarnessKind::AntigravityIde => unreachable!("a window harness has no prompt in a tab"),
    }
}

/// A message really delivered into a real harness running in a container with no network of its
/// own: the same paste and Return the screen sends, and the harness's own prompt shows them.
fn verify_delivery(harness: HarnessKind) {
    use qframe::runtime::Harness as Screen;
    use qframe::widgets::TerminalSession;

    let profile = profile(harness);
    let sent = "Through QCode, from the Claude Code · claude-sub tab:";
    let task = "please write the test for the parser and run it";
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new(&format!("{}-delivery", harness.record().id));
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace.clone());
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));

        // Exactly the line a harness tab spawns, token and all.
        let line = harness.command_line(None);
        let parts: Vec<&str> = line.iter().map(String::as_str).collect();
        let command = plan.enter_with(&engine, &parts, &[(super::TOKEN_VARIABLE, "tok-delivery")]);
        let session = TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0)
            .expect("a pseudo-terminal for the tab");
        let mut screen = Screen::new(Watched(session.clone()), 120, 36);
        let drawn = until_prompt(&mut screen, &session, harness);
        assert!(drawn.contains(prompt_of(harness)), "{kind:?} {harness:?}: no prompt came up:\n{drawn}");

        // What delivery does, byte for byte: the sender's name in front, then Return.
        session.paste(&format!("{sent}\n\n{task}")).unwrap_or_else(|error| panic!("{kind:?}: the paste: {error}"));
        session.write(b"\r").unwrap_or_else(|error| panic!("{kind:?}: the Return: {error}"));
        let drawn = until_shown(&mut screen, task);
        assert!(drawn.contains(task), "{kind:?} {harness:?}: the message never reached the prompt:\n{drawn}");
        assert!(drawn.contains("QCode"), "{kind:?} {harness:?}: the sender is not named:\n{drawn}");

        session.kill();
        capture(&engine.remove_container(&plan.name)).expect("the container is removed");
        capture(&engine.remove_volume(&home.volume())).expect("the home is removed");
        clear(&engine, &profile, &plan.name);
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn a_message_delivered_into_opencode_reaches_its_own_prompt() {
    verify_delivery(HarnessKind::OpenCode);
}

/// The tag the provider of the delivery check is written down under.
const PROVIDER_TAG: &str = "teslim";

/// A Claude Code profile signed in with a provider of this machine, in a container with no
/// network — the same profile [`crate::provider::relay_live`] proves the road with.
fn provider_profile(model: &str) -> Profile {
    provider_profile_under(model, Template::Recommended)
}

/// [`provider_profile`] under `template`, named after it so two templates never share an image.
fn provider_profile_under(model: &str, template: Template) -> Profile {
    Profile {
        name: SafeName::parse(&format!("bridgetest-claude-code-provider-{}", template.id())).expect("the name is safe"),
        harness: HarnessKind::ClaudeCode,
        template,
        account: AccountKind::Provider,
        provider: Some(crate::profile::ProviderChoice { tag: PROVIDER_TAG.to_owned(), model: model.to_owned() }),
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
    }
}

/// A message delivered into a real Claude Code, the one harness that draws no prompt until it has
/// somebody to speak to. The provider relay gives it one: the container still has no network, the
/// key never enters it, and what it answers with comes from a model on this machine's own
/// network. Everything the tab is started with is the product's: the container of
/// [`ContainerPlan::profile`], the program of [`crate::provider::relay::wrapping`], the
/// environment of [`crate::profile::ProviderChoice::environment`].
#[test]
#[ignore = "needs a container engine, the network once for the image, and a model service; run with QCODE_CONTAINER_TESTS=1, QCODE_PROVIDER_URL and QCODE_PROVIDER_MODEL"]
fn a_message_delivered_into_claude_code_reaches_its_own_prompt() {
    use crate::provider::relay::{self, Listener as Relay, Upstream};
    use crate::provider::{ProviderEntry, ProviderKind, Tag};
    use qframe::runtime::Harness as Screen;
    use qframe::widgets::TerminalSession;

    let base = std::env::var("QCODE_PROVIDER_URL").unwrap_or_else(|_| "http://192.168.122.1:11434".to_owned());
    let model = std::env::var("QCODE_PROVIDER_MODEL").unwrap_or_else(|_| "qwen3.5-256k".to_owned());
    let profile = provider_profile(&model);
    let choice = profile.provider.clone().expect("the profile names a provider");
    let entry = ProviderEntry::new(Tag::parse(PROVIDER_TAG).expect("a tag"), ProviderKind::Ollama, &base);
    let harness = HarnessKind::ClaudeCode;
    let sent = "Through QCode, from the Codex · codex-main tab:";
    let task = "please write the test for the parser and run it";

    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new("claude-code-provider-delivery");
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace.clone());
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);

        // QCode's own side of the road, listening before the tab starts, so the harness finds it
        // the moment it looks.
        let token = super::token();
        let mine = token.clone();
        let carried = entry.clone();
        let relay = Relay::open(
            &paths.mcp(),
            move |asked| (asked == mine).then(|| carried.clone()),
            Upstream::network(),
            |_| (),
        )
        .expect("the workspace's relay socket opens");
        assert!(relay.socket().exists(), "{kind:?}: the socket is there for the container to reach");

        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));

        // Exactly the line a provider tab spawns: the relay in front of the harness, and the
        // variables that point the harness at it. Nothing was measured for the window here, and
        // a number invented for it would be the very thing the measurement exists to avoid.
        let line = harness.command_line(None);
        let parts = relay::wrapping(&line);
        let words: Vec<&str> = parts.iter().map(String::as_str).collect();
        let mut carried = vec![(super::TOKEN_VARIABLE.to_owned(), token.clone())];
        carried.extend(choice.environment(&token, None));
        let envs: Vec<(&str, &str)> = carried.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        let command = plan.enter_with(&engine, &words, &envs);
        let session = TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0)
            .expect("a pseudo-terminal for the tab");
        let mut screen = Screen::new(Watched(session.clone()), 120, 36);
        let drawn = until_prompt(&mut screen, &session, harness);
        assert!(drawn.contains(prompt_of(harness)), "{kind:?} {harness:?}: no prompt came up:\n{drawn}");

        // What delivery does, byte for byte: the sender's name in front, then Return.
        session.paste(&format!("{sent}\n\n{task}")).unwrap_or_else(|error| panic!("{kind:?}: the paste: {error}"));
        session.write(b"\r").unwrap_or_else(|error| panic!("{kind:?}: the Return: {error}"));
        let drawn = until_shown(&mut screen, task);
        assert!(drawn.contains(task), "{kind:?} {harness:?}: the message never reached the prompt:\n{drawn}");
        assert!(drawn.contains("QCode"), "{kind:?} {harness:?}: the sender is not named:\n{drawn}");

        session.kill();
        drop(relay);
        capture(&engine.remove_container(&plan.name)).expect("the container is removed");
        capture(&engine.remove_volume(&home.volume())).expect("the home is removed");
        clear(&engine, &profile, &plan.name);
    }
}

/// A real Claude Code tab, started exactly as the product starts one, opens on its own prompt with
/// not one key pressed: the image of a QCode template has answered the questions of its first
/// start already. Every one of them would otherwise stand in the way with "No, exit" highlighted
/// under the cursor, where a person pressing Return leaves. It is watched for the prompt only; a
/// question on the screen means the prompt never comes and the test shows what was drawn instead.
fn claude_code_opens_on_its_prompt(template: Template) {
    use crate::provider::relay::{self, Listener as Relay, Upstream};
    use crate::provider::{ProviderEntry, ProviderKind, Tag};
    use qframe::runtime::Harness as Screen;
    use qframe::widgets::TerminalSession;

    let base = std::env::var("QCODE_PROVIDER_URL").unwrap_or_else(|_| "http://192.168.122.1:11434".to_owned());
    let model = std::env::var("QCODE_PROVIDER_MODEL").unwrap_or_else(|_| "qwen3.5-256k".to_owned());
    let profile = provider_profile_under(&model, template);
    let choice = profile.provider.clone().expect("the profile names a provider");
    let entry = ProviderEntry::new(Tag::parse(PROVIDER_TAG).expect("a tag"), ProviderKind::Ollama, &base);
    let harness = HarnessKind::ClaudeCode;

    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new(&format!("claude-code-first-start-{}", template.id()));
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace.clone());
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);

        let token = super::token();
        let mine = token.clone();
        let carried = entry.clone();
        let relay = Relay::open(
            &paths.mcp(),
            move |asked| (asked == mine).then(|| carried.clone()),
            Upstream::network(),
            |_| (),
        )
        .expect("the workspace's relay socket opens");
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        // The bridge registers its server into the very file the answers are in, before the tab
        // starts, the way a workspace does.
        config::register(&engine, &plan.name, harness, &token)
            .unwrap_or_else(|trouble| panic!("{kind:?}: {trouble:?}"));

        let line = harness.command_line(None);
        let parts = relay::wrapping(&line);
        let words: Vec<&str> = parts.iter().map(String::as_str).collect();
        let mut carried = vec![(super::TOKEN_VARIABLE.to_owned(), token.clone())];
        carried.extend(choice.environment(&token, None));
        let envs: Vec<(&str, &str)> = carried.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        let command = plan.enter_with(&engine, &words, &envs);
        let session = TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0)
            .expect("a pseudo-terminal for the tab");
        let mut screen = Screen::new(Watched(session.clone()), 120, 36);
        let started = std::time::Instant::now();
        let drawn = until_shown(&mut screen, prompt_of(harness));
        assert!(
            drawn.contains(prompt_of(harness)),
            "{kind:?} {template:?}: no prompt without a key being pressed:\n{drawn}"
        );
        assert!(!drawn.contains("No, exit"), "{kind:?} {template:?}:\n{drawn}");
        eprintln!(
            "{kind:?} {template:?}: Claude Code drew its prompt {} s after it started",
            started.elapsed().as_secs()
        );

        session.kill();
        drop(relay);
        capture(&engine.remove_container(&plan.name)).expect("the container is removed");
        capture(&engine.remove_volume(&home.volume())).expect("the home is removed");
        clear(&engine, &profile, &plan.name);
    }
}

#[test]
#[ignore = "needs a container engine, the network once for the image, and a model service; run with QCODE_CONTAINER_TESTS=1, QCODE_PROVIDER_URL and QCODE_PROVIDER_MODEL"]
fn claude_code_opens_on_its_prompt_under_qcode_basic_without_a_key_pressed() {
    claude_code_opens_on_its_prompt(Template::Recommended);
}

#[test]
#[ignore = "needs a container engine, the network once for the image, and a model service; run with QCODE_CONTAINER_TESTS=1, QCODE_PROVIDER_URL and QCODE_PROVIDER_MODEL"]
fn claude_code_opens_on_its_prompt_under_qcode_high_without_a_key_pressed() {
    claude_code_opens_on_its_prompt(Template::High);
}
