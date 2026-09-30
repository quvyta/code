//! How much memory Claude Code tabs take in one workspace container, and in which process: the
//! `claude` program of each tab, every MCP server it starts (the bridge's `qcode-bridge.mjs`,
//! graphify's, a plugin's), every language server a plugin starts and whatever else runs.
//! Measured under QCode recommended and under base, and under QCode extra and Quvyta development
//! in a small Rust workspace with the network on, since two of their plugins start a program of
//! their own (accesslint's MCP server by `npx`, rust-analyzer by the rust-analyzer-lsp plugin);
//! with one, two and four tabs in one container, each started on the line a tab is started with,
//! and once more after each of the four tabs has done one small task. Each process is read as its
//! resident memory, its proportional share and the part only it holds, since the first counts the
//! program's pages once per process and the container holds them once.
//!
//! The tabs are on a provider whose model is `fake_model.py`, on the container's own loopback at
//! the relay's port, as in `permission_live`: no model on the network is asked and no account is
//! needed. The base image has no Python to run it with, so there Claude Code is given the same
//! address and answered by nobody; it still draws its prompt and starts its servers.
//!
//! What it prints is a measurement, not a verdict: the only things checked are that every tab came
//! up and that each count of tabs is running. `#[ignore]`d and doing nothing unless
//! `QCODE_CONTAINER_TESTS=1`:
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 QCODE_CONTAINER_ENGINE=podman \
//!   cargo test memory_live -- --ignored --nocapture --test-threads=1
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use qframe::runtime::Harness as Screen;
use qframe::widgets::TerminalSession;

use super::harness_live::{build, clear};
use super::identity::Home;
use super::{AccountKind, Extra, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template};
use crate::bridge::protocol::{Answer, You};
use crate::bridge::socket::Listener;
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// The model the tabs are pointed at, where the image can run it.
const FAKE_MODEL: &str = include_str!("fake_model.py");

/// Words Claude Code draws once its prompt is up.
const PROMPT: &str = "shift+tab to cycle";

/// The workspace the containers here belong to, which nobody has.
const WORKSPACE: &str = "bellektest";

/// How long the tabs are left alone before they are measured, so every server they start is up.
const SETTLE: Duration = Duration::from_secs(20);

/// What every tab is asked once the four are up: the fake model then runs a command, writes a
/// file and says it is done, which is a small task's worth of work.
const TASK: &str = "Do the task.";

/// How long the tabs are given for that task before they are measured again.
const WORKED: Duration = Duration::from_secs(60);

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
        let path = std::env::temp_dir().join(format!("qcode-bellek-{name}-{stamp}"));
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

/// A screen that draws nothing but one tab's terminal.
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

fn profile(template: Template, network: NetworkMode, without: Vec<Extra>) -> Profile {
    Profile {
        name: SafeName::parse(&format!("bellektest-claude-{}", template.id())).expect("the name is safe"),
        harness: HarnessKind::ClaudeCode,
        template,
        account: AccountKind::Provider,
        provider: Some(ProviderChoice::model("bellek", "m")),
        assets: MountAccess::ReadOnly,
        network,
        without,
        os: crate::base::Os::Debian,
    }
}

/// A small Rust workspace for a language server to index: a library of a few functions with a
/// test, and a program that calls it. It has no dependencies, so nothing is fetched to build it.
const RUST_FILES: [(&str, &str); 3] = [
    ("Cargo.toml", "[package]\nname = \"bellek\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    (
        "src/lib.rs",
        "//! Sums and words.\n\n/// The sum of `values`.\npub fn sum(values: &[i64]) -> i64 {\n    values.iter().sum()\n}\n\n\
         /// `text` split into words.\npub fn words(text: &str) -> Vec<&str> {\n    text.split_whitespace().collect()\n}\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn sums() {\n        assert_eq!(super::sum(&[1, 2, 3]), 6);\n    }\n}\n",
    ),
    ("src/main.rs", "fn main() {\n    println!(\"{} {:?}\", bellek::sum(&[1, 2]), bellek::words(\"a b\"));\n}\n"),
];

/// Draws the tab until its prompt is up, answering a first start's questions with their
/// highlighted answer the way `permission_live` does under base, for a generous while.
fn until_prompt(session: &TerminalSession) -> String {
    let mut screen = Screen::new(Watched(session.clone()), 120, 36);
    let deadline = Instant::now() + Duration::from_secs(240);
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

/// One process of the container, in KiB: its resident memory, its proportional share of it, and
/// the part only it holds (what the container would get back if the process were not there); and
/// what it runs.
struct Process {
    rss: u64,
    pss: u64,
    uss: u64,
    line: String,
}

/// Every process of `container`, read from `/proc`: the images promise no `ps`.
fn processes(engine: &Engine, container: &str) -> Vec<Process> {
    let script = "for p in /proc/[0-9]*; do \
                  r=$(sed -n 's/^VmRSS:[[:space:]]*\\([0-9]*\\).*/\\1/p' \"$p/status\" 2>/dev/null); \
                  [ -n \"$r\" ] || continue; \
                  s=$(sed -n 's/^Pss:[[:space:]]*\\([0-9]*\\).*/\\1/p' \"$p/smaps_rollup\" 2>/dev/null); \
                  u=$(sed -n 's/^Private_[CD][a-z]*:[[:space:]]*\\([0-9]*\\).*/\\1/p' \"$p/smaps_rollup\" 2>/dev/null \
                      | { t=0; while read n; do t=$((t + n)); done; echo $t; }); \
                  c=$(tr '\\000' ' ' < \"$p/cmdline\" 2>/dev/null | cut -c1-200); \
                  echo \"$r ${s:-0} ${u:-0} $c\"; done";
    let said =
        capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] })).unwrap_or_default();
    said.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, ' ');
            let rss = parts.next()?.parse().ok()?;
            let pss = parts.next()?.parse().ok()?;
            let uss = parts.next()?.parse().ok()?;
            let line = parts.next().unwrap_or_default().trim().to_owned();
            // The reading shell itself and its helpers are not the container's.
            (!line.starts_with("sh -c for p in") && !line.is_empty()).then_some(Process { rss, pss, uss, line })
        })
        .collect()
}

/// What a process is, by its command line.
fn what(line: &str) -> &'static str {
    if line.contains("qcode-bridge.mjs") {
        "bridge (qcode-bridge.mjs)"
    } else if line.contains("fake_model.py") {
        "fake model (not QCode's)"
    } else if line.contains("graphify") {
        "graphify"
    } else if line.contains("rust-analyzer") {
        "rust-analyzer"
    } else if line.contains("accesslint") {
        "accesslint"
    } else if line.starts_with("npm exec") || line.contains("/npx ") || line.starts_with("npx ") {
        "npx"
    } else if line.contains("context7") {
        "context7"
    } else if line.starts_with("claude") || line.contains("/claude ") || line.contains("claude-code") {
        "claude"
    } else if line.contains("qcode-relay") {
        "relay"
    } else {
        "other"
    }
}

/// The container's memory as the engine's own stats say it.
fn stats(engine: &Engine, container: &str) -> String {
    let program = match engine.kind() {
        EngineKind::Podman => "podman",
        EngineKind::Docker => "docker",
    };
    std::process::Command::new(program)
        .args(["stats", "--no-stream", "--format", "{{.MemUsage}}", container])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Opens tabs in one container under `template` until there are four, measuring after the first,
/// the second and the fourth. `rust` puts [`RUST_FILES`] into the workspace first.
fn measure(template: Template, network: NetworkMode, without: Vec<Extra>, rust: bool) {
    let profile = profile(template, network, without);
    let harness = profile.harness;
    let choice = profile.provider.clone().expect("the profile names a provider");
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new(template.id());
        let paths = scratch.paths();
        if rust {
            for (path, text) in RUST_FILES {
                let file = paths.code.join(path);
                std::fs::create_dir_all(file.parent().expect("a folder")).expect("the folder");
                std::fs::write(file, text).expect("a file of the workspace");
            }
        }
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace);
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);

        // QCode's end of the bridge, which also writes the bridge's server where containers see it.
        let listener = Listener::open(&paths.mcp()).expect("the bridge's socket");
        let inbox = listener.inbox();
        let answering = std::thread::spawn(move || {
            while let Some(call) = inbox.next() {
                call.answer(Answer::listed("nobody else".to_owned(), You::default(), Vec::new()));
            }
        });

        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
        };
        let port = crate::provider::relay::PORT;
        if run("command -v python3").is_ok() {
            run(&format!("cat > /tmp/fake_model.py <<'PYTHON'\n{FAKE_MODEL}PYTHON")).expect("the model is written");
            // In a Rust workspace the task writes a source file, which is what a language server
            // of the harness starts for.
            let file = if rust { "/work/src/written.rs" } else { "" };
            run(&format!(
                "cd /tmp && (nohup python3 fake_model.py {port} {file} > /tmp/fake-model.out 2>&1 &) && sleep 2"
            ))
            .expect("the model starts");
        }

        let mut sessions = Vec::new();
        for count in 1..=4 {
            let token = format!("tok-bellek-{count}");
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
            assert!(drawn.contains(PROMPT), "{kind:?} {template:?} tab {count}: no prompt:\n{drawn}");
            sessions.push(session);
            if count == 3 {
                continue;
            }
            std::thread::sleep(SETTLE);
            report(&engine, &plan.name, template, &format!("{count} tab(s), idle"), count);
        }

        // What the tabs were given to start: the servers in the user settings and in every
        // installed plugin, and, once each tab has done one task, the tools the model was offered.
        let said = run("sed -n '/\"mcpServers\"/,/^  }/p' \"$HOME/.claude.json\"; \
                        find / \\( -path /proc -o -path /sys -o -path /work \\) -prune -o -name .mcp.json -path '*/plugins/cache/*' -print 2>/dev/null \
                        | while read -r file; do echo \"$file\"; cat \"$file\"; done; true")
            .unwrap_or_else(|error| format!("{error:?}"));
        eprintln!("== servers configured:\n{said}");

        for session in &sessions {
            let _ = session.paste(TASK);
            std::thread::sleep(Duration::from_millis(500));
            let _ = session.write(b"\r");
        }
        std::thread::sleep(WORKED);
        report(&engine, &plan.name, template, "4 tab(s), after one task each", 4);
        let tools = run("sed -n 's/.*\\(tools=\\[[^]]*\\]\\).*/\\1/p' /tmp/fake-model.log | sort | uniq -c; \
                         grep -c . /tmp/fake-model.log; true")
        .unwrap_or_else(|error| format!("{error:?}"));
        eprintln!("== tools offered to the model:\n{tools}");

        for session in &sessions {
            session.kill();
        }
        drop(listener);
        let _ = answering.join();
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        clear(&engine, &profile, &plan.name);
    }
}

/// Prints what `container` holds with `count` tabs, `when` saying at which point: the engine's figure, then every kind of process
/// with how many there are and their memory, then every process.
fn report(engine: &Engine, container: &str, template: Template, when: &str, count: usize) {
    let all = processes(engine, container);
    let claude = all.iter().filter(|process| what(&process.line) == "claude").count();
    assert!(claude >= count, "{template:?}: {count} tabs and {claude} claude processes");
    let mut kinds: BTreeMap<&str, (usize, u64, u64, u64)> = BTreeMap::new();
    for process in &all {
        let entry = kinds.entry(what(&process.line)).or_default();
        entry.0 += 1;
        entry.1 += process.rss;
        entry.2 += process.pss;
        entry.3 += process.uss;
    }
    eprintln!("== {:?} {template:?}, {when}: engine says {}", engine.kind(), stats(engine, container));
    for (name, (number, rss, pss, uss)) in &kinds {
        eprintln!(
            "   {name:28} x{number:<3} RSS {:>6} MiB  PSS {:>6} MiB  USS {:>6} MiB",
            rss / 1024,
            pss / 1024,
            uss / 1024
        );
    }
    for process in &all {
        eprintln!(
            "     {:>7} KiB rss {:>7} KiB pss {:>7} KiB uss  {}",
            process.rss, process.pss, process.uss, process.line
        );
    }
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_tabs_memory_under_qcode_recommended() {
    measure(Template::Recommended, NetworkMode::None, Vec::new(), false);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_tabs_memory_under_base() {
    measure(Template::Base, NetworkMode::None, Vec::new(), false);
}

/// With the network on, because accesslint's server is fetched by `npx` when a tab starts.
#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_tabs_memory_under_qcode_extra_in_a_rust_workspace() {
    measure(Template::High, NetworkMode::Full, Vec::new(), true);
}

/// Chromium is left out: it starts no process of its own in a tab, and it is the largest download.
#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_tabs_memory_under_quvyta_development_in_a_rust_workspace() {
    measure(Template::QuvytaDev, NetworkMode::Full, vec![Extra::Chromium], true);
}
