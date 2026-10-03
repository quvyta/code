//! That no harness stops to ask the person before it acts, shown on the harness itself: in a real
//! container of a profile's image under any template, started by the line a tab starts it with, in
//! a real terminal, the harness is given one task by a model that answers with a shell command and
//! then with a file to write, and both happen with not one key pressed after the task.
//!
//! The model is `fake_model.js`, a server on the container's own loopback at the relay's port, the
//! address a profile on a provider is pointed at: it speaks each harness's shape (Anthropic
//! messages, OpenAI chat completions and responses, Gemini's generateContent) and needs neither the
//! network nor an account. The relay itself is left out: it carries requests to the person's
//! provider, and here the provider stands where the relay would. It runs under Node, which every
//! image carries, rather than under the Python of `fake_model.py`, which the `base` image does not:
//! what is being shown here is what a harness does under `base`, where the image is one a person
//! would have built themselves.
//!
//! The container is brought up the way a tab brings it up, answers and all: the keys that keep a
//! harness from asking are merged into the home before the harness starts
//! ([`unattended`](super::unattended)), which is what makes these tests mean the same under every
//! template.
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
use super::unattended;
use super::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// The model every harness is pointed at, started in the container before the harness.
const FAKE_MODEL: &str = include_str!("fake_model.js");

/// What the model's shell command leaves in the workspace, and the file its second answer writes
/// there; both are named in `fake_model.js`.
const TOUCHED: &str = "asked-nothing";
const WRITTEN: &str = "edited-without-asking.txt";
const WRITTEN_TEXT: &str = "written without asking\n";

/// The task typed into the harness's prompt.
const TASK: &str = "Do the step the model asks for.";

/// What is written into Gemini CLI's own settings for the harness to start on its prompt with a key
/// and not open its sign-in dialog, which is what a person's first start leaves behind. Under Node,
/// since that is in every image.
const GEMINI_ON_A_KEY: &str = r#"node -e 'const fs = require("node:fs");
const file = process.env.HOME + "/.gemini/settings.json";
const own = JSON.parse(fs.readFileSync(file, "utf8"));
own.security = own.security || {};
own.security.auth = own.security.auth || {};
own.security.auth.selectedType = "gemini-api-key";
fs.writeFileSync(file, JSON.stringify(own, null, 2) + "\n");'"#;

/// The workspace the containers here belong to, which nobody has.
const WORKSPACE: &str = "izintest";

/// The words each harness's own approval question is drawn with, as each one drew it in a
/// container when it was started without what keeps it from asking. None of them may ever be on
/// the screen of a tab.
const QUESTIONS: [&str; 11] = [
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
    // Claude Code 2.1.285's offer to trade the mode for its auto mode, "Yes" highlighted.
    "Make auto mode your default permission mode?",
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
    // Where the product puts them, and before the harness starts: without them a harness under
    // `base` is asked before every step, and these tests would be measuring the asking.
    unattended::ensure(&engine, &plan.name, harness).unwrap_or_else(|trouble| panic!("{kind:?}: {trouble:?}"));
    let port = crate::provider::relay::PORT;
    if model {
        run(&format!("cat > /tmp/fake_model.js <<'SCRIPT'\n{FAKE_MODEL}SCRIPT"));
        run(&format!("cd /tmp && (nohup node fake_model.js {port} > /tmp/fake-model.out 2>&1 &) && sleep 2"));
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
            // first start would have. Node rather than Python, so that it also happens on the base
            // image, which carries no Python.
            run(GEMINI_ON_A_KEY);
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

/// The model under Node, on this machine, answering each shape a harness asks in. The images
/// under test are not needed for this, and the base image has no Python: what these tests ask of
/// the model is that it speaks, and this is where that is shown.
#[test]
fn the_model_under_node_answers_each_harness_in_the_shape_it_asks_in() {
    use std::io::Read as _;

    let Some(node) = crate::testing::node("answering for the harnesses") else { return };
    let folder = crate::engine::scratch::Scratch::new("fake-model").expect("a folder");
    // Written to a file and run as one, the way a container runs it, so the port is where the
    // script looks for it.
    let script = folder.path().join("fake_model.js");
    std::fs::write(&script, FAKE_MODEL).expect("the model is written");
    let port = 41_000 + u16::try_from(std::process::id() % 2_000).expect("a port");
    let mut server = std::process::Command::new(&node)
        .arg(&script)
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the model starts");
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build(),
    );
    let address = format!("http://127.0.0.1:{port}");
    let mut listening = Err("not asked yet".to_owned());
    for _ in 0..40 {
        listening = agent
            .get(format!("{address}/v1/models"))
            .force_send_body()
            .send_empty()
            .map(|_| ())
            .map_err(|error| error.to_string());
        if listening.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if let Err(refused) = listening {
        let _ = server.kill();
        let out = server.wait_with_output().expect("the model ends");
        panic!("the model is listening on {address}: {refused}\n{}", String::from_utf8_lossy(&out.stderr));
    }
    let ask = |path: &str, body: &str| {
        let answer = agent.post(format!("{address}{path}")).send(body).expect("the model answers");
        let mut said = String::new();
        answer.into_body().into_reader().read_to_string(&mut said).expect("the answer is text");
        said
    };
    let schema = r#"{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}"#;

    // Anthropic messages, as Claude Code asks them, then the same again with the command's result.
    let first = ask(
        "/v1/messages",
        &format!(
            r#"{{"model":"m","tools":[{{"name":"Bash","input_schema":{schema}}}],"messages":[{{"role":"user","content":"go"}}]}}"#
        ),
    );
    assert!(first.contains(r#""stop_reason":"tool_use""#), "{first}");
    assert!(first.contains("touch /tmp/outside-the-workspace /work/asked-nothing"), "{first}");
    let second = ask(
        "/v1/messages",
        &format!(
            r#"{{"model":"m","tools":[{{"name":"Bash","input_schema":{schema}}}],
                "messages":[{{"role":"user","content":"go"}},
                {{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t","content":"done"}}]}}]}}"#
        ),
    );
    assert!(second.contains("edited-without-asking.txt"), "{second}");

    // OpenAI chat completions, as opencode and Kimi Code CLI ask them.
    let chat = ask(
        "/v1/chat/completions",
        &format!(
            r#"{{"model":"m","tools":[{{"type":"function","function":{{"name":"shell","parameters":{schema}}}}}],"messages":[{{"role":"user","content":"go"}}]}}"#
        ),
    );
    assert!(chat.contains(r#""finish_reason":"tool_calls""#), "{chat}");
    assert!(chat.contains("asked-nothing"), "{chat}");

    // OpenAI responses, as Codex asks them.
    let responses = ask(
        "/v1/responses",
        &format!(r#"{{"model":"m","tools":[{{"type":"function","name":"shell","parameters":{schema}}}],"input":[]}}"#),
    );
    assert!(responses.contains(r#""type":"function_call""#), "{responses}");
    assert!(responses.contains("asked-nothing"), "{responses}");

    // Gemini's generateContent, streamed and not, as Gemini CLI and Qwen Code ask them.
    let gemini = ask(
        "/v1beta/models/m:streamGenerateContent",
        &format!(
            r#"{{"contents":[{{"parts":[{{"text":"go"}}]}}],"tools":[{{"functionDeclarations":[{{"name":"run_shell_command","parameters":{schema}}}]}}]}}"#
        ),
    );
    assert!(gemini.contains(r#""functionCall""#), "{gemini}");
    assert!(gemini.contains("asked-nothing"), "{gemini}");
    // The side question of its own, which it asks with a schema and is answered in that schema.
    let side = ask(
        "/v1beta/models/m:generateContent",
        r#"{"contents":[{"parts":[{"text":"how hard"}]}],"generationConfig":{"responseMimeType":"application/json","responseSchema":{"type":"object","properties":{"level":{"type":"string","enum":["easy","hard"]}},"required":["level"]}}}"#,
    );
    assert!(side.contains(r#"{\"level\":\"easy\"}"#), "{side}");

    let _ = server.kill();
    let _ = server.wait();
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

/// Under `base` the image holds none of QCode's files, so every question a harness asks on the way
/// is QCode's to answer, and it is answered in the home before the harness starts: the folder's
/// trust, the permission mode, the first start itself. So the same check as under a template is
/// made here, on the same task and with not one key pressed.
#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_runs_and_writes_without_asking_under_base() {
    acts_without_asking(HarnessKind::ClaudeCode, Template::Base);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_runs_and_writes_without_asking_under_base() {
    acts_without_asking(HarnessKind::OpenCode, Template::Base);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn gemini_cli_runs_and_writes_without_asking_under_base() {
    acts_without_asking(HarnessKind::GeminiCli, Template::Base);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn codex_runs_and_writes_without_asking_under_base() {
    acts_without_asking(HarnessKind::Codex, Template::Base);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn kimi_code_runs_and_writes_without_asking_under_base() {
    acts_without_asking(HarnessKind::KimiCode, Template::Base);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn qwen_code_runs_and_writes_without_asking_under_base() {
    acts_without_asking(HarnessKind::QwenCode, Template::Base);
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
