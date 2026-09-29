//! The Claude Code plugins that start a program of their own, in a real image without the
//! network: accesslint's MCP server starts from the image, not from `npx`, and answers Claude
//! Code's own check; security-guidance's first start finds the agent SDK in the image and installs
//! nothing; rust-analyzer's plugin comes with the program it names under Quvyta development and not
//! at all under QCode extra, which has no Rust. Each image's size is printed.
//!
//! `#[ignore]`d and doing nothing unless `QCODE_CONTAINER_TESTS=1`; the image is built with the
//! network once:
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 QCODE_CONTAINER_ENGINE=podman \
//!   cargo test plugins_live -- --ignored --nocapture --test-threads=1
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use qframe::runtime::Harness as Screen;
use qframe::widgets::TerminalSession;

use super::harness_live::{build, clear};
use super::identity::Home;
use super::{AccountKind, Extra, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, ensure_running};

fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let only = std::env::var("QCODE_CONTAINER_ENGINE").ok();
    let found: Vec<Engine> = [EngineKind::Podman, EngineKind::Docker]
        .into_iter()
        .filter(|kind| only.as_deref().is_none_or(|only| format!("{kind:?}").eq_ignore_ascii_case(only)))
        .filter_map(|kind| detect(kind).ok())
        .collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-plugins-{name}-{stamp}"));
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

/// Builds `template`'s Claude Code image, starts a workspace container of it without the network,
/// and hands `check` a way to run a shell in it; everything is removed again afterwards.
fn in_a_container(template: Template, without: Vec<Extra>, check: impl Fn(EngineKind, &dyn Fn(&str) -> String)) {
    let profile = Profile {
        name: SafeName::parse(&format!("pluginstest-{}", template.id())).expect("a safe name"),
        harness: HarnessKind::ClaudeCode,
        template,
        account: AccountKind::Subscription,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without,
        os: crate::base::Os::Debian,
    };
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new(template.id());
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse("pluginstest").expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace);
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);
        let program = match kind {
            EngineKind::Podman => "podman",
            EngineKind::Docker => "docker",
        };
        let size = std::process::Command::new(program)
            .args(["image", "inspect", "--format", "{{.Size}}", &profile.image()])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        eprintln!("== {kind:?} {template:?}: image {size} bytes");
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
                .unwrap_or_else(|error| format!("{error:?}"))
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(kind, &run)));
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        clear(&engine, &profile, &plan.name);
        if let Err(failed) = result {
            std::panic::resume_unwind(failed);
        }
    }
}

/// Claude Code's own list of its servers, which starts each and says whether it answered.
fn servers(run: &dyn Fn(&str) -> String) -> String {
    run("cd /work && timeout 120 claude mcp list 2>&1; true")
}

/// The model the tab is pointed at, on the container's own loopback: no model on the network is
/// asked and no account is needed, and the tab's prompt comes up on it.
const FAKE_MODEL: &str = include_str!("fake_model.py");

/// Words Claude Code draws once its prompt is up and reading what is typed.
const PROMPT: &str = "shift+tab to cycle";

/// The workspace the container belongs to, which nobody has.
const WORKSPACE: &str = "guvenliktest";

/// A screen that draws nothing but one tab's terminal, the widget a tab draws with.
struct Watched(TerminalSession);

impl qframe::prelude::App for Watched {
    type Msg = ();

    fn update(&mut self, (): ()) -> qframe::prelude::Command<()> {
        qframe::prelude::Command::none()
    }

    fn view(&self, ui: &mut qframe::prelude::View<'_, ()>) {
        ui.add(qframe::widgets::Terminal::new(&self.0).read_only()).fill_width().fill_height();
    }
}

/// Draws the tab until its prompt is up, answering a first start's own questions with their
/// highlighted answer, and gives up after a generous while rather than waiting for ever.
fn until_prompt(session: &TerminalSession) -> String {
    let mut screen = Screen::new(Watched(session.clone()), 120, 36);
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut before = String::new();
    loop {
        screen.render();
        let drawn = screen.screen();
        if drawn.contains(PROMPT) || Instant::now() > deadline {
            return drawn;
        }
        if drawn == before {
            if drawn.contains("❯ No, exit") {
                let _ = session.write(b"\x1b[B");
                std::thread::sleep(Duration::from_millis(500));
            }
            let _ = session.write(b"\r");
        }
        before = drawn;
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// The profile QCode recommended makes for Claude Code: the set with the security-guidance plugin
/// in it, and the smallest one that carries it.
fn recommended() -> Profile {
    Profile {
        name: SafeName::parse("pluginstest-guvenlik").expect("a safe name"),
        harness: HarnessKind::ClaudeCode,
        template: Template::Recommended,
        account: AccountKind::Provider,
        provider: Some(ProviderChoice { tag: "guvenlik".to_owned(), model: "m".to_owned() }),
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// Opens one Claude Code tab in a container of `profile`'s image without the network, hands `check`
/// a way to run a shell in the container once the tab's prompt is up, and takes everything away
/// again afterwards.
fn in_a_tab(profile: &Profile, check: impl Fn(EngineKind, &dyn Fn(&str) -> String, &str)) {
    for engine in engines() {
        let kind = engine.kind();
        let harness = profile.harness;
        let scratch = Scratch::new("tab");
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, profile);
        let home = Home::new(profile.name.clone(), workspace);
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, profile);
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
                .unwrap_or_else(|error| format!("{error:?}"))
        };
        run(&format!("cat > /tmp/fake_model.py <<'PYTHON'\n{FAKE_MODEL}PYTHON"));
        let port = crate::provider::relay::PORT;
        let started =
            run(&format!("cd /tmp && (nohup python3 fake_model.py {port} > /tmp/fake-model.out 2>&1 &) && sleep 2"));
        assert!(!started.contains("not found"), "{kind:?}: the model did not start: {started}");

        let choice = profile.provider.clone().expect("the profile names a provider");
        let token = String::from("tok-guvenlik");
        crate::bridge::config::register(&engine, &plan.name, harness, &token)
            .unwrap_or_else(|trouble| panic!("{kind:?}: {trouble:?}"));
        let line = harness.command_line(None);
        let words: Vec<&str> = line.iter().map(String::as_str).collect();
        let mut envs = vec![(crate::bridge::TOKEN_VARIABLE.to_owned(), token.clone())];
        envs.extend(choice.environment(harness, &token, None));
        let pairs: Vec<(&str, &str)> = envs.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        let command = plan.enter_with(&engine, &words, &pairs);
        let session = TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0)
            .expect("a pseudo-terminal for the tab");
        let drawn = until_prompt(&session);
        assert!(drawn.contains(PROMPT), "{kind:?}: no prompt came up:\n{drawn}");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(kind, &run, &drawn)));
        session.kill();
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        clear(&engine, profile, &plan.name);
        if let Err(failed) = result {
            std::panic::resume_unwind(failed);
        }
    }
}

/// The plugin's own SessionStart hook, run the way its `hooks.json` runs it, and what it answered.
fn the_hooks_own_words(run: &dyn Fn(&str) -> String) -> String {
    run(
        "cd /work && timeout 180 bash \"$HOME\"/.claude/plugins/cache/claude-plugins-official/security-guidance/*/hooks/sg-python.sh \
         \"$HOME\"/.claude/plugins/cache/claude-plugins-official/security-guidance/*/hooks/ensure_agent_sdk.py 2>&1; true",
    )
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn accesslint_starts_from_the_image_without_the_network_under_qcode_extra() {
    in_a_container(Template::High, Vec::new(), |kind, run| {
        let said = servers(run);
        eprintln!("== {kind:?}: claude mcp list\n{said}");
        let line = said.lines().find(|line| line.contains("accesslint")).unwrap_or_default();
        assert!(line.contains("/usr/local/npm/bin/accesslint-mcp"), "{kind:?}: {said}");
        assert!(line.contains("Connected"), "{kind:?}: the server answers without the network: {said}");
        assert!(!said.contains("npx"), "{kind:?}: {said}");
        let plugins = run("claude plugin list 2>&1; true");
        assert!(!plugins.contains("rust-analyzer-lsp"), "{kind:?}: no Rust, no rust-analyzer plugin: {plugins}");
        assert!(run("command -v rust-analyzer || echo none").trim().ends_with("none"), "{kind:?}");
    });
}

/// A tab's first start, with no network: the plugin's own hook finds the agent SDK in the image
/// and installs nothing, so nothing of the tab's own start is a pip run.
#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn security_guidances_first_start_finds_its_agent_sdk_without_the_network() {
    in_a_tab(&recommended(), |kind, run, drawn| {
        assert!(!drawn.to_lowercase().contains("pip install"), "{kind:?}: the tab's start said:\n{drawn}");
        // The environment the image made is the one the tab used, and its python still answers for
        // the package: the hook found it and built nothing of its own.
        let imports =
            run("\"$HOME\"/.claude/security/agent-sdk-venv/bin/python -c 'import claude_agent_sdk' 2>&1; echo \"$?\"");
        assert_eq!(imports.trim(), "0", "{kind:?}: {imports}");
        let said = the_hooks_own_words(run);
        eprintln!("== {kind:?}: the hook at a second start\n{said}");
        // Its own outcomes: 0 is the package already importable by the interpreter and 1 is the
        // environment beside the plugin's own state, both no-ops; 2 is what it built and 3 what it
        // could not.
        let outcome: serde_json::Value =
            said.lines().filter_map(|line| serde_json::from_str(line.trim()).ok()).next_back().expect("an answer");
        let built = outcome["metrics"]["sdk_bootstrap"].as_u64().expect("what it did");
        assert!(built < 2, "{kind:?}: it built one itself: {said}");
        // The plugin is in the tab's own list, so the hook above is the one the tab ran.
        let plugins = run("claude plugin list 2>&1; true");
        assert!(plugins.contains("security-guidance"), "{kind:?}: {plugins}");
    });
}

/// Chromium is left out: it has nothing to do with the plugins, and it is the largest download.
#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn rust_analyzers_plugin_finds_its_program_without_the_network_under_quvyta_development() {
    in_a_container(Template::QuvytaDev, vec![Extra::Chromium], |kind, run| {
        let plugins = run("claude plugin list 2>&1; true");
        assert!(plugins.contains("rust-analyzer-lsp"), "{kind:?}: {plugins}");
        // The program the plugin names, on the path every shell of the container has.
        let version = run("cd /work && rust-analyzer --version 2>&1");
        eprintln!("== {kind:?}: {version}");
        assert!(version.starts_with("rust-analyzer "), "{kind:?}: {version}");
        let servers = servers(run);
        let line = servers.lines().find(|line| line.contains("accesslint")).unwrap_or_default();
        assert!(line.contains("Connected"), "{kind:?}: {servers}");
    });
}
