//! The whole road, in a real container: a harness of a provider profile asks a model service on
//! this machine's network through the relay, and answers — while its container has no network at
//! all and the provider's key never leaves this process.
//!
//! This is the one thing no test with a stand-in can say. [`super::relay`]'s own tests drive the
//! socket and the framing; what they cannot prove is that Claude Code, started the way QCode
//! starts it, really speaks to the relay and really gets a model's answer back. That is what is
//! proven here, with the product's own builders: the container is [`ContainerPlan::profile`]'s,
//! the program is [`relay::wrapping`] over the harness's own command line, and the environment is
//! [`ProviderChoice::environment`]. Nothing here writes its own idea of any of them, because a
//! copy that drifts from the product proves nothing about the product.
//!
//! The test is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`. It needs a
//! container engine, the network once to build the image, and a model service reachable from
//! this machine:
//!
//! ```text
//! QCODE_PROVIDER_URL=http://192.168.122.1:11434 QCODE_PROVIDER_MODEL=qwen3.5-256k \
//!   QCODE_CONTAINER_TESTS=1 cargo test provider::relay_live -- --ignored --test-threads=1
//! ```
//!
//! A local model answers slowly: a prompt of a few thousand tokens took half a minute on the
//! machine this was written on, and a larger model four times that. The waits here are therefore
//! generous rather than tight — a short limit would report a working provider as broken.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::relay::{self, Listener, Upstream};
use super::{ProviderEntry, ProviderKind, Tag};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::profile::harness_live::{build, clear};
use crate::profile::identity::Home;
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// The workspace these containers belong to, which nobody has.
const WORKSPACE: &str = "relaytest";

/// The tag the provider is written down under here.
const TAG: &str = "olcum";

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

/// The provider this run asks, and the model it asks for.
fn provider() -> (String, String) {
    let base = std::env::var("QCODE_PROVIDER_URL").unwrap_or_else(|_| "http://192.168.122.1:11434".to_owned());
    let model = std::env::var("QCODE_PROVIDER_MODEL").unwrap_or_else(|_| "qwen3.5-256k".to_owned());
    (base, model)
}

/// A workspace folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-relaylive-{name}-{stamp}"));
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

/// The profile: Claude Code, signed in with a provider of the person's own, in a container with
/// no network. The network is off on purpose — everything the harness reaches, it reaches
/// through the relay, so a container that could have gone around it could not have.
fn profile(model: &str) -> Profile {
    Profile {
        name: SafeName::parse("relaytest-claude-code").expect("the name is safe"),
        harness: HarnessKind::ClaudeCode,
        template: Template::Recommended,
        account: AccountKind::Provider,
        provider: Some(ProviderChoice { tag: TAG.to_owned(), model: model.to_owned() }),
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
    }
}

/// Runs `script` with a shell in `container` and answers what it printed, or everything the
/// shell said when it failed.
fn run(engine: &Engine, container: &str, script: &str) -> Result<String, String> {
    capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] }))
        .map_err(|error| format!("{error:?}"))
}

/// The one shell word for `text`, so a prompt with quotes in it is still one argument.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Asks the harness `prompt` inside `container`, started exactly as QCode starts it: the tab's
/// environment, then the relay with the harness after it.
fn ask(engine: &Engine, container: &str, choice: &ProviderChoice, token: &str, prompt: &str) -> Result<String, String> {
    let mut words = vec!["env".to_owned(), format!("{}={token}", crate::bridge::TOKEN_VARIABLE)];
    // Nothing was measured here: this run is about the road, not the window, and a number
    // invented for it would be the very thing the measurement exists to avoid.
    for (name, value) in choice.environment(token, None) {
        words.push(format!("{name}={}", quoted(&value)));
    }
    words.extend(relay::wrapping(&HarnessKind::ClaudeCode.command_line(None)).into_iter().map(|word| quoted(&word)));
    words.push("-p".to_owned());
    words.push(quoted(prompt));
    run(engine, container, &words.join(" "))
}

#[test]
#[ignore = "needs a container engine, the network once for the image, and a model service; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_answers_and_uses_a_tool_through_a_provider_of_this_machine() {
    let (base, model) = provider();
    let profile = profile(&model);
    let choice = profile.provider.clone().expect("the profile names a provider");
    let entry = ProviderEntry::new(Tag::parse(TAG).expect("a tag"), ProviderKind::Ollama, &base);
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new("claude-code");
        let paths = scratch.paths();
        std::fs::write(paths.code.join("parola.txt"), "kirlangic\n").expect("a file for the harness to read");
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace.clone());
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);

        // QCode's own side: the socket the container reaches, the entry the token resolves to,
        // and the key added on the way out — of which there is none for an ollama server, which
        // is why this one can be run by anybody with a model on their own network.
        let token = crate::bridge::token();
        let mine = token.clone();
        let carried = entry.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let listener = Listener::open(
            &paths.mcp(),
            move |asked| (asked == mine).then(|| carried.clone()),
            Upstream::network(),
            move |event| seen.lock().expect("the events are not poisoned").push(event),
        )
        .expect("the workspace's relay socket opens");
        assert!(listener.socket().exists(), "{kind:?}: the socket is there for the container to reach");

        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));

        // What a person types first: a question with one right answer no container could have
        // made up on its own.
        let said = ask(&engine, &plan.name, &choice, &token, "Reply with just the digits: what is 2+2?")
            .unwrap_or_else(|trouble| panic!("{kind:?}: the harness said nothing: {trouble}"));
        assert!(said.contains('4'), "{kind:?}: the model answered through the relay: {said}");

        // And the thing a coding agent is for: reading a file of the workspace and saying what is
        // in it. Only a model that can call a tool and read its result can answer this.
        let said = ask(
            &engine,
            &plan.name,
            &choice,
            &token,
            "Read the file parola.txt in this folder and reply with only the word inside it.",
        )
        .unwrap_or_else(|trouble| panic!("{kind:?}: the harness said nothing: {trouble}"));
        assert!(said.contains("kirlangic"), "{kind:?}: the agent read the file through its tools: {said}");

        let events = events.lock().expect("the events are not poisoned");
        assert!(
            events.iter().any(|event| matches!(event, super::RelayEvent::Forwarded { .. })),
            "{kind:?}: QCode carried the requests itself: {events:?}"
        );
        assert!(
            !events.iter().any(|event| matches!(event, super::RelayEvent::UnknownTab | super::RelayEvent::Malformed)),
            "{kind:?}: nothing knocked that QCode did not know: {events:?}"
        );
        drop(events);
        drop(listener);
        clear(&engine, &profile, &plan.name);
        let _ = capture(&engine.remove_volume(&home.volume()));
    }
}
