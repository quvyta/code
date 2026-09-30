//! That no harness stops to ask the person before it acts, shown on the harness itself: in a real
//! container of a QCode template's image, started by the line a tab starts it with, in a real
//! terminal, the harness is given one task by a model that answers with a shell command and then
//! with a file to write, and both happen with not one key pressed after the task.
//!
//! The model is `fake_model.py`, a server on the container's own loopback at the relay's port, the
//! address a profile on a provider is pointed at: it speaks each harness's shape (Anthropic
//! messages, OpenAI chat completions and responses, Gemini's generateContent) and needs neither the
//! network nor an account. The relay itself is left out: it carries requests to the person's
//! provider, and here the provider stands where the relay would.
//!
//! The shell command also touches a file outside the workspace. That is a step opencode asks about
//! on its own (its `external_directory` permission), so a harness that asks about anything at all
//! is caught by the same step. What is drawn is read from the terminal widget the tab draws with,
//! and each screen is checked for the harness's own approval question as well: a harness that
//! asked and was then answered by nobody never writes the files, and one that asked in some way
//! the files do not show is caught by its words.
//!
//! Every test is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`:
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test permission_live -- --ignored --test-threads=1
//! ```
//!
//! `QCODE_CONTAINER_ENGINE=podman` keeps one engine and `QCODE_LIVE_HARNESS=<id>` one harness.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use qframe::runtime::Harness as Screen;
use qframe::widgets::TerminalSession;

use super::harness_live::{build, clear};
use super::identity::Home;
use super::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// The model every harness is pointed at, started in the container before the harness.
const FAKE_MODEL: &str = include_str!("fake_model.py");

/// What the model's shell command leaves in the workspace, and the file its second answer writes
/// there; both are named in `fake_model.py`.
const TOUCHED: &str = "asked-nothing";
const WRITTEN: &str = "edited-without-asking.txt";
const WRITTEN_TEXT: &str = "written without asking\n";

/// The task typed into the harness's prompt.
const TASK: &str = "Do the step the model asks for.";

/// The workspace the containers here belong to, which nobody has.
const WORKSPACE: &str = "izintest";

/// The words each harness's own approval question is drawn with, as each one drew it in a
/// container when it was started without what keeps it from asking. None of them may ever be on
/// the screen of a tab.
const QUESTIONS: [&str; 10] = [
    // Claude Code, before a command.
    "Do you want to proceed?",
    // opencode, before a step its permissions say to ask about.
    "Permission required",
    // Gemini CLI and Qwen Code, before a command and before an edit.
    "Allow execution",
    "Apply this change?",
    // Codex, before a command, and before it runs a hook nobody has reviewed yet (graphify's,
    // which QCode basic installs).
    "Would you like to run the following command?",
    "needs review before it can run",
    // Kimi Code CLI, before a command and before an edit.
    "Approve once",
    "Allow once",
    // Any question whose highlighted answer leaves: Claude Code's folder trust and its warning
    // about the mode QCode starts it in.
    "No, exit",
    "Bypass Permissions mode",
];

/// The engines installed on this machine, or nothing at all when the tests are switched off.
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

/// A workspace folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-izinlive-{name}-{stamp}"));
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

/// A profile of `harness` under `template` whose container has no network, on a provider of one's
/// own where the harness is offered one. Gemini CLI is offered none; it signs in with a key, and
/// the key and the address it asks are handed over in its own variables in [`start`].
fn profile(harness: HarnessKind, template: Template) -> Profile {
    let offered = harness.supports(AccountKind::Provider);
    Profile {
        name: SafeName::parse(&format!("izintest-{}-{}", harness.record().id, template.id()))
            .expect("the name is safe"),
        harness,
        template,
        account: if offered { AccountKind::Provider } else { AccountKind::ApiKey },
        provider: offered.then(|| ProviderChoice::model("izin", "m")),
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

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

/// Words the harness draws once its prompt is up and reading what is typed, on a provider.
fn prompt_of(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "shift+tab to cycle",
        HarnessKind::OpenCode => "Ask anything",
        HarnessKind::GeminiCli | HarnessKind::QwenCode => "Type your message",
        HarnessKind::Codex => "Ask Codex to do anything",
        HarnessKind::KimiCode => "Never Ask",
        HarnessKind::AntigravityIde => unreachable!("a window harness has no prompt in a tab"),
    }
}

/// The first of [`QUESTIONS`] on `drawn`, if one is.
fn question_on(drawn: &str) -> Option<&'static str> {
    QUESTIONS.into_iter().find(|words| drawn.contains(words))
}

/// A running container of the profile image and the tab started in it.
struct Tab {
    engine: Engine,
    plan: ContainerPlan,
    home: Home,
    profile: Profile,
    session: TerminalSession,
    screen: Screen<Watched>,
    scratch: Scratch,
}

impl Tab {
    fn run(&self, script: &str) -> Result<String, String> {
        capture(
            &self.engine.exec_without_terminal(&Exec { container: &self.plan.name, command: &["sh", "-c", script] }),
        )
        .map_err(|error| format!("{error:?}"))
    }

    /// Draws the tab again and again until `wanted` is on it or a generous while has passed,
    /// failing as soon as an approval question is drawn.
    fn until_shown(&mut self, wanted: &str, within: Duration) -> String {
        let deadline = Instant::now() + within;
        loop {
            self.screen.render();
            let drawn = self.screen.screen();
            self.no_question(&drawn);
            if drawn.contains(wanted) || Instant::now() > deadline {
                return drawn;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn no_question(&self, drawn: &str) {
        if let Some(words) = question_on(drawn) {
            panic!(
                "{:?} {:?} {:?}: the harness asked (`{words}`):\n{drawn}",
                self.engine.kind(),
                self.profile.harness,
                self.profile.template
            );
        }
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.session.kill();
        let _ = capture(&self.engine.remove_container(&self.plan.name));
        let _ = capture(&self.engine.remove_volume(&self.home.volume()));
        clear(&self.engine, &self.profile, &self.plan.name);
    }
}

/// Builds the image of `profile`, starts its container the way a workspace does, starts the model
/// when `model` is asked for, and opens a tab of the harness in it on the line a tab is started
/// with: the program and its unattended-mode arguments from [`HarnessKind::command_line`], the
/// provider's arguments right after the program and its variables in the environment.
fn start(engine: Engine, profile: Profile, model: bool) -> Tab {
    let kind = engine.kind();
    let harness = profile.harness;
    let scratch = Scratch::new(&format!("{}-{}", harness.record().id, profile.template.id()));
    let paths = scratch.paths();
    let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
    let plan = ContainerPlan::profile(&workspace, &paths, &profile);
    let home = Home::new(profile.name.clone(), workspace);
    let _ = capture(&engine.remove_container(&plan.name));
    let _ = capture(&engine.remove_volume(&home.volume()));
    build(&engine, &profile);
    let user = HostUser::current().expect("the current user");
    ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));

    let run = |script: &str| {
        capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
            .unwrap_or_else(|error| panic!("{kind:?}: `{script}`: {error:?}"))
    };
    let port = crate::provider::relay::PORT;
    if model {
        run(&format!("cat > /tmp/fake_model.py <<'PYTHON'\n{FAKE_MODEL}PYTHON"));
        run(&format!("cd /tmp && (nohup python3 fake_model.py {port} > /tmp/fake-model.out 2>&1 &) && sleep 2"));
    }

    let mut line = harness.command_line(None);
    let mut envs = vec![(crate::bridge::TOKEN_VARIABLE.to_owned(), "tok-izin".to_owned())];
    match &profile.provider {
        Some(provider) => {
            line.splice(1..1, provider.arguments(harness));
            envs.extend(provider.environment(harness, "tok-izin", None));
        }
        None => {
            // Gemini CLI on a key: the key is taken from its variable and the address it asks from
            // its own, and the answer its sign-in dialog writes is written for it, as a person's
            // first start would have.
            run("python3 -c \"import json, os; p = os.path.expanduser('~/.gemini/settings.json'); \
                 d = json.load(open(p)); d.setdefault('security', {}).setdefault('auth', {})['selectedType'] = \
                 'gemini-api-key'; json.dump(d, open(p, 'w'))\"");
            envs.push(("GEMINI_API_KEY".to_owned(), "x".to_owned()));
            envs.push(("GOOGLE_GEMINI_BASE_URL".to_owned(), format!("http://127.0.0.1:{port}")));
        }
    }
    let words: Vec<&str> = line.iter().map(String::as_str).collect();
    let pairs: Vec<(&str, &str)> = envs.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
    let command = plan.enter_with(&engine, &words, &pairs);
    let session = TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0)
        .expect("a pseudo-terminal for the tab");
    let screen = Screen::new(Watched(session.clone()), 120, 36);
    Tab { engine, plan, home, profile, session, screen, scratch }
}

/// The whole check for one harness under one QCode template, on every engine.
fn acts_without_asking(harness: HarnessKind, template: Template) {
    let only = std::env::var("QCODE_LIVE_HARNESS").ok();
    if only.as_deref().is_some_and(|id| id != harness.record().id) {
        return;
    }
    for engine in engines() {
        let kind = engine.kind();
        let started = Instant::now();
        let mut tab = start(engine, profile(harness, template), true);

        // The prompt comes up with no key pressed: nothing is asked before it either.
        let prompt = prompt_of(harness);
        let drawn = tab.until_shown(prompt, Duration::from_secs(240));
        assert!(drawn.contains(prompt), "{kind:?} {harness:?}: no prompt came up:\n{drawn}");
        // A harness may draw its prompt and a question over it a moment later.
        std::thread::sleep(Duration::from_secs(5));
        tab.until_shown(prompt, Duration::from_secs(1));

        tab.session.paste(TASK).unwrap_or_else(|error| panic!("{kind:?}: the paste: {error}"));
        std::thread::sleep(Duration::from_millis(500));
        tab.session.write(b"\r").unwrap_or_else(|error| panic!("{kind:?}: the Return: {error}"));

        // Then nothing more is typed. Both files appear within a generous while, and no screen on
        // the way shows a question.
        let code = tab.scratch.paths().code;
        let deadline = Instant::now() + Duration::from_secs(240);
        let mut drawn = String::new();
        while !(code.join(TOUCHED).exists() && code.join(WRITTEN).exists()) && Instant::now() < deadline {
            drawn = tab.until_shown("\u{0}", Duration::from_millis(500));
        }
        let said = tab.run("cat /tmp/fake-model.log 2>/dev/null; tail -c 1500 /tmp/fake-model.out").unwrap_or_default();
        assert!(code.join(TOUCHED).exists(), "{kind:?} {harness:?}: the command never ran:\n{drawn}\n{said}");
        assert!(code.join(WRITTEN).exists(), "{kind:?} {harness:?}: the file was never written:\n{drawn}\n{said}");
        let text = std::fs::read_to_string(code.join(WRITTEN)).unwrap_or_default();
        assert_eq!(text, WRITTEN_TEXT, "{kind:?} {harness:?}: the file holds what the model wrote");
        // The command's other file is outside the workspace, where opencode asks by default.
        tab.run("test -e /tmp/outside-the-workspace")
            .unwrap_or_else(|said| panic!("{kind:?} {harness:?}: only half the command ran: {said}"));
        // A last look, after the harness has had a moment to draw what came of it.
        std::thread::sleep(Duration::from_secs(3));
        tab.until_shown("\u{0}", Duration::from_secs(1));
        eprintln!("{kind:?} {harness:?} {template:?}: acted without asking, {} s", started.elapsed().as_secs());
    }
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_runs_and_writes_without_asking_under_qcode_basic() {
    acts_without_asking(HarnessKind::ClaudeCode, Template::Recommended);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_runs_and_writes_without_asking_under_qcode_basic() {
    acts_without_asking(HarnessKind::OpenCode, Template::Recommended);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn gemini_cli_runs_and_writes_without_asking_under_qcode_basic() {
    acts_without_asking(HarnessKind::GeminiCli, Template::Recommended);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn codex_runs_and_writes_without_asking_under_qcode_basic() {
    acts_without_asking(HarnessKind::Codex, Template::Recommended);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn kimi_code_runs_and_writes_without_asking_under_qcode_basic() {
    acts_without_asking(HarnessKind::KimiCode, Template::Recommended);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn qwen_code_runs_and_writes_without_asking_under_qcode_basic() {
    acts_without_asking(HarnessKind::QwenCode, Template::Recommended);
}

/// Under `base` the image holds none of QCode's files, so Claude Code meets its first start as it
/// comes: the text style, its security notes and whether the folder is trusted, which are its own
/// questions and are answered here as a person would, moving to "Yes" where "No, exit" is
/// highlighted. What must never come is the warning about the permission mode, whose highlighted
/// answer leaves: the mode is QCode's choice, made by the argument every template passes, so the
/// same argument has to carry the answer too.
#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_under_base_is_never_warned_about_the_mode_qcode_starts_it_in() {
    let harness = HarnessKind::ClaudeCode;
    for engine in engines() {
        let kind = engine.kind();
        // No model: Claude Code draws its prompt as soon as it has an address to speak to, and the
        // base image has no Python to run one with.
        let mut tab = start(engine, profile(harness, Template::Base), false);
        let wanted = prompt_of(harness);
        let deadline = Instant::now() + Duration::from_secs(240);
        let mut before = String::new();
        let mut seen = Vec::new();
        let drawn = loop {
            tab.screen.render();
            let drawn = tab.screen.screen();
            assert!(!drawn.contains("Bypass Permissions mode"), "{kind:?}: the mode warning came up:\n{drawn}");
            if drawn.contains(wanted) || Instant::now() > deadline {
                break drawn;
            }
            // Still on the same screen and no prompt: it is waiting for an answer.
            if drawn == before && seen.len() < 6 {
                seen.push(drawn.lines().find(|line| line.contains('❯')).unwrap_or_default().trim().to_owned());
                if drawn.contains("❯ No, exit") {
                    let _ = tab.session.write(b"\x1b[B");
                    std::thread::sleep(Duration::from_millis(500));
                }
                let _ = tab.session.write(b"\r");
            }
            before = drawn;
            std::thread::sleep(Duration::from_secs(3));
        };
        assert!(drawn.contains(wanted), "{kind:?}: no prompt came up:\n{drawn}");
        eprintln!("{kind:?}: under base, Claude Code asked on its first start: {seen:?}");
    }
}

/// The checker of [`antigravity_reads_every_setting_the_templates_write`].
const DECLARED_SETTINGS: &str = include_str!("declared_settings.js");

/// Every key the QCode templates write into Antigravity IDE's settings is one the application in
/// the image declares, with a value its declaration takes, and reads: a key renamed or dropped in
/// a later version, or a value it stopped taking, fails here instead of being ignored in silence.
/// The image is built from the product's own recipe, the archive from the maker's address.
#[test]
#[ignore = "needs a container engine and the network for the application's archive; run with QCODE_CONTAINER_TESTS=1"]
fn antigravity_reads_every_setting_the_templates_write() {
    use crate::base::paths::{CODE_DIR, KEEP_ALIVE};
    use crate::engine::names::HOSTNAME;
    use crate::engine::{ContainerCreate, Network};

    let harness = HarnessKind::AntigravityIde;
    let settings = harness.record().settings.expect("the templates write Antigravity IDE's settings");
    let desktop = harness.desktop().expect("it opens a window");
    let profile = Profile {
        name: SafeName::parse("izintest-antigravity-ide").expect("the name is safe"),
        harness,
        template: Template::Recommended,
        account: AccountKind::InApp,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    };
    for engine in engines() {
        let kind = engine.kind();
        let container = "qcode-izintest-antigravity-ide";
        let _ = capture(&engine.remove_container(container));
        build(&engine, &profile);
        let made = capture(&engine.create_container(&ContainerCreate {
            name: container,
            hostname: HOSTNAME,
            labels: &[],
            image: &profile.image(),
            mounts: &[],
            network: Network::None,
            user: HostUser::current().expect("the current user"),
            workdir: Some(std::path::Path::new(CODE_DIR)),
            command: KEEP_ALIVE,
        }))
        .and_then(|_| capture(&engine.start_container(container)));
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] }))
                .map_err(|error| format!("{error:?}"))
        };
        let checked = made.map_err(|error| format!("{error:?}")).and_then(|_| {
            run(&format!("cat > /tmp/declared_settings.js <<'SCRIPT'\n{DECLARED_SETTINGS}SCRIPT"))?;
            // The file in the image is the template's, key for key.
            let written = run(&format!("cat \"$HOME/{}\"", settings.path))?;
            assert_eq!(written, settings.contents, "{kind:?}: the template's file reached the image");
            run(&format!("node /tmp/declared_settings.js '{}' \"$HOME/{}\"", desktop.install_dir, settings.path))
        });
        let _ = capture(&engine.remove_container(container));
        clear(&engine, &profile, container);
        let said = checked.unwrap_or_else(|said| panic!("{kind:?}: a setting the application does not read:\n{said}"));
        eprintln!("{kind:?}:\n{said}");
    }
}
