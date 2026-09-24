//! The workspace screen, checked without a container runtime anywhere near it.
//!
//! The promise the screen exists for — that nothing a tab opens runs on this machine — is
//! checked at the level where it is decided: the command a tab spawns. The engine binary of
//! these tests is a path that cannot be run, so a message that reaches the engine layer fails at
//! once and no container is ever touched; the live tests in `live` are the ones that use a real
//! engine.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use qframe::date::Date;
use qframe::env::{AssetDirs, Env};
use qframe::event::{MouseButton, MouseKind};
use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::runtime::Harness;
use qframe::widgets::{TerminalEvent, TerminalSession};

use crate::engine::{Container, ContainerState, Engine, EngineCommand, EngineKind, HostUser};
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::store::{
    Session, SessionTab, SessionTabKind, SessionWorkspace, WorkspaceFile, WorkspaceId, WorkspacePaths, WorkspaceProfile,
};

use super::plan::{CODE_DIR, SHELL};

mod backups;
mod bridge;
mod closed_sign_in;
mod desktop;
mod engine_help;
mod files;
mod guidance;
mod history;
mod missing_image;
mod rebuilt_image;
mod registry;
mod shine;
mod sound;
mod viewers;
mod watch;
use super::{Choice, Msg, OpenWorkspace, PanelWidget, Tab, TabKey, TabKind, TabState, WorkspaceScreen};

/// A terminal wide enough for the rail, the tabs, the terminal and the panel.
const SIZE: (u16, u16) = (110, 32);

/// A binary that cannot be run, so every engine command fails before it starts anything.
const NO_ENGINE: &str = "/nonexistent/qcode-test-engine";

/// A folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-workspace-{name}-{stamp}"));
        fs::create_dir_all(path.join("Work").join("src")).expect("a workspace folder");
        fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        fs::write(path.join("Work").join("README.md"), "hello\n").expect("a file in the workspace");
        fs::write(path.join("Work").join("src").join("main.rs"), "fn main() {}\n").expect("a source file");
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
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The application under test: the workspace screen and nothing else.
struct Screen(WorkspaceScreen);

impl App for Screen {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        super::update(&mut self.0, message)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        super::view(&self.0, ui, std::convert::identity, |_| {}, |_| {}, |_| {});
    }
}

/// The workspace screen inside an application that hands it a header and a footer of its own,
/// the way QCode does.
struct Framed(WorkspaceScreen);

impl App for Framed {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        super::update(&mut self.0, message)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        super::view(
            &self.0,
            ui,
            std::convert::identity,
            |ui| {
                ui.add(Text::new("HEADER"));
            },
            |ui| {
                ui.add(Text::new("FOOTER"));
            },
            |ui| {
                ui.add(Text::new("UNDER"));
            },
        );
    }
}

/// QCode's own text and keys, and nothing of the person's.
fn env() -> Env {
    let dirs =
        AssetDirs { locale_sources: crate::locales(), keymap_source: Some(crate::keymap()), ..AssetDirs::default() };
    Env::load(&dirs).expect("the built-in files load")
}

fn profile(name: &str, harness: HarnessKind) -> Profile {
    Profile {
        name: SafeName::parse(name).expect("a usable profile name"),
        harness,
        template: Template::Recommended,
        account: AccountKind::Subscription,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// The file of a workspace `id` carrying the profiles named in `carried`.
fn file(id: &str, name: &str, carried: &[&str]) -> WorkspaceFile {
    let day = Date::new(2026, 9, 17).expect("a day the calendar has");
    let mut file = WorkspaceFile::new(WorkspaceId::parse(id).expect("a usable workspace id"), name, day);
    file.profiles =
        carried.iter().map(|name| WorkspaceProfile { name: (*name).to_owned(), added: Some(day) }).collect();
    file
}

/// A workspace that carries every one of `profiles`.
fn workspace(id: &str, name: &str, paths: WorkspacePaths, profiles: Vec<Profile>) -> OpenWorkspace {
    let carried: Vec<&str> = profiles.iter().map(|profile| profile.name.as_str()).collect();
    OpenWorkspace::new(&file(id, name, &carried), paths, profiles)
}

fn engine() -> Engine {
    Engine::new(EngineKind::Podman, NO_ENGINE)
}

/// A screen with one workspace carrying one profile, and an engine whose binary cannot be run.
fn one_workspace(scratch: &Scratch) -> WorkspaceScreen {
    let workspaces =
        vec![workspace("firefly", "Firefly", scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)])];
    WorkspaceScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces)
}

/// Applies a message without letting its background work run, which is how the states that only
/// exist while work is in flight are looked at.
fn apply(screen: &mut WorkspaceScreen, message: Msg) {
    drop(super::update(screen, message));
}

/// A harness of the screen, with the work the screen asks for on entry already done: the workspace
/// folder read, and the engine asked for the containers.
fn harness(screen: WorkspaceScreen, width: u16, height: u16) -> Harness<Screen> {
    let has_workspace = screen.workspace().is_some();
    let mut harness = Harness::with_env(Screen(screen), env(), width, height);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    if has_workspace {
        harness.send(Msg::OpenWorkspace(0));
    }
    harness.render();
    harness
}

/// Opens a blank tab and chooses `choice` on its page, the way the `+` and a row of the page do,
/// without letting the background work run.
fn open(screen: &mut WorkspaceScreen, choice: Choice) {
    apply(screen, Msg::NewTab);
    let tab = screen.workspace().and_then(OpenWorkspace::active_tab).map(Tab::key).expect("the blank tab is open");
    apply(screen, Msg::Choose(tab, choice));
}

/// The same through a harness, so the background work runs as it would in the application.
fn open_in(harness: &mut Harness<Screen>, choice: Choice) {
    harness.send(Msg::NewTab);
    let tab = harness.app().0.workspace().and_then(OpenWorkspace::active_tab).map(Tab::key);
    harness.send(Msg::Choose(tab.expect("the blank tab is open"), choice));
}

/// The profile of `one_workspace`, as a choice of a blank tab's page.
fn claude() -> Choice {
    Choice::NewChat("claude-sub".to_owned())
}

/// What the tabs of the open workspace are, in order.
fn kinds(screen: &WorkspaceScreen) -> Vec<TabKind> {
    screen.workspace().expect("a workspace is open").tabs().iter().map(|tab| tab.kind().clone()).collect()
}

/// The key of the tab at `index` of the open workspace.
fn key(screen: &WorkspaceScreen, index: usize) -> TabKey {
    screen.workspace().expect("a workspace is open").tabs()[index].key()
}

/// The words of `command` without the bridge's token: it is random, so a test that spells out
/// a command leaves it out, and `tests/bridge.rs` checks it is there.
fn spelled(command: &EngineCommand) -> Vec<String> {
    let mut words = Vec::new();
    let mut args = command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).peekable();
    while let Some(word) = args.next() {
        if word == "--env" && args.peek().is_some_and(|next| next.starts_with("QCODE_BRIDGE=")) {
            args.next();
            continue;
        }
        words.push(word);
    }
    words
}

#[test]
fn every_language_fits_the_workspace_screen_without_losing_a_label() {
    // The rail, the panel and the tab strip are all counted in cells, so a language with longer
    // words is where a heading or a hint would lose its end.
    let scratch = Scratch::new("languages");
    let mut catalog = qframe::i18n::I18n::builtin();
    for (file, text) in crate::locales() {
        catalog.add_source(&file, &text);
    }
    for code in ["en", "tr", "de", "es", "fr", "pt-BR", "ru", "zh-Hans", "ja"] {
        let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
        harness.set_locale(code).render();
        catalog.set_active(code);
        let screen = harness.screen();
        for key in [
            "workspace.widget.files",
            "workspace.widget.info",
            "workspace.widget.containers",
            "workspace.new-tab",
            "workspace.containers.stop",
            "workspace.containers.restart",
            "workspace.containers.refresh",
        ] {
            let label = catalog.translate(key, &[]);
            assert!(screen.contains(&label), "{code} loses `{label}`:\n{screen}");
        }
        // The only ellipsis a resting screen may show is one a text of its own ends with.
        let ours: Vec<String> = ["workspace.files.reading", "workspace.backup.size-reading", "workspace.starting"]
            .iter()
            .map(|key| catalog.translate(key, &[]))
            .collect();
        for line in screen.lines().filter(|line| line.contains('…')) {
            assert!(ours.iter().any(|text| line.contains(text.as_str())), "{code} cuts a line:\n{screen}");
        }

        // Dragged wide by its edge, as far as it goes, the panel has room for the three container
        // buttons side by side, and they still stand there whole.
        let edge = i32::from(SIZE.0 - super::PANEL_WIDTH);
        let row = i32::from(SIZE.1 / 2);
        harness.mouse(MouseKind::Down(MouseButton::Left), edge, row);
        harness.mouse(MouseKind::Drag(MouseButton::Left), 2, row);
        harness.mouse(MouseKind::Up(MouseButton::Left), 2, row);
        harness.render();
        let wide = harness.screen();
        let row: Vec<String> =
            ["workspace.containers.stop", "workspace.containers.restart", "workspace.containers.refresh"]
                .iter()
                .map(|key| catalog.translate(key, &[]))
                .collect();
        let line = wide
            .lines()
            .find(|line| row.iter().all(|label| line.contains(label.as_str())))
            .unwrap_or_else(|| panic!("{code} does not put the three on one line:\n{wide}"));
        assert!(!line.contains('…'), "{code} cuts the widened row:\n{wide}");
    }
}

#[test]
fn a_tab_opens_inside_a_container_and_never_on_this_machine() {
    let scratch = Scratch::new("inside");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    open(&mut screen, claude());

    let shell = screen.launch_command(key(&screen, 0)).expect("the shell tab has a command");
    let words: Vec<String> = shell.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert_eq!(shell.program, Path::new(NO_ENGINE), "a tab only ever starts the engine binary");
    assert_eq!(words, ["exec", "--interactive", "--tty", "qcode-firefly-base", SHELL[0], SHELL[1]]);

    let harness = screen.launch_command(key(&screen, 1)).expect("the profile tab has a command");
    let words = spelled(&harness);
    assert_eq!(harness.program, Path::new(NO_ENGINE));
    assert_eq!(
        words,
        ["exec", "--interactive", "--tty", "qcode-firefly-claude-sub", "claude", "--dangerously-skip-permissions"],
        "the harness is an argument of the engine, so it runs in the container"
    );
}

/// A profile of [`AccountKind::Provider`], running on `tag`/`model`.
fn provider_profile(name: &str, tag: &str, model: &str) -> Profile {
    Profile {
        account: AccountKind::Provider,
        provider: Some(crate::profile::ProviderChoice { tag: tag.to_owned(), model: model.to_owned() }),
        ..profile(name, HarnessKind::ClaudeCode)
    }
}

/// A `providers.toml` at a scratch path of its own, carrying one provider tagged `tag` (an ollama
/// entry, which needs no key) when `present`, or nothing at all otherwise — the way the Providers
/// page would leave it after the person removed it.
fn providers_file(name: &str, tag: &str, present: bool) -> PathBuf {
    // A folder of its own: saving narrows the folder the file is in, and that must never be
    // the machine's shared temporary folder.
    let path = std::env::temp_dir().join(format!("qcode-workspace-providers-{name}")).join("providers.toml");
    let mut providers = crate::provider::Providers::in_memory();
    if present {
        let entry = crate::provider::ProviderEntry::new(
            crate::provider::Tag::parse(tag).expect("a tag"),
            crate::provider::ProviderKind::Ollama,
            "http://127.0.0.1:11434",
        );
        providers.add(entry).expect("the tag is free");
    }
    providers.at(&path).save().expect("the scratch file is written");
    path
}

/// A `providers.toml` carrying one ollama provider tagged `tag` whose model `model` was measured
/// to give `window` tokens, which is what a tab of it must be told.
fn measured_providers_file(name: &str, tag: &str, model: &str, window: u64) -> PathBuf {
    // A folder of its own: saving narrows the folder the file is in, and that must never be
    // the machine's shared temporary folder.
    let path = std::env::temp_dir().join(format!("qcode-workspace-providers-{name}")).join("providers.toml");
    let mut providers = crate::provider::Providers::in_memory();
    let mut entry = crate::provider::ProviderEntry::new(
        crate::provider::Tag::parse(tag).expect("a tag"),
        crate::provider::ProviderKind::Ollama,
        "http://127.0.0.1:11434",
    );
    let mut known = crate::provider::Model::new(model);
    known.claimed = Some(262_144);
    known.measured = Some(crate::provider::Measured::About(window));
    entry.models = vec![known];
    providers.add(entry).expect("the tag is free");
    providers.at(&path).save().expect("the scratch file is written");
    path
}

/// The `--env NAME=VALUE` pairs a command carries, by name.
fn envs(command: &EngineCommand) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut args = command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).peekable();
    while let Some(word) = args.next() {
        if word == "--env"
            && let Some(pair) = args.next()
            && let Some((name, value)) = pair.split_once('=')
        {
            map.insert(name.to_owned(), value.to_owned());
        }
    }
    map
}

#[test]
fn a_provider_tabs_environment_points_at_the_relay_and_never_carries_a_key() {
    let scratch = Scratch::new("provider-env");
    let profiles = vec![provider_profile("ev-tab", "ev1", "qwen3.8")];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("ev-tab".to_owned()));

    let command = screen.launch_command(key(&screen, 0)).expect("a chosen tab has a command");
    let env = envs(&command);
    assert_eq!(env.get("ANTHROPIC_BASE_URL").map(String::as_str), Some("http://127.0.0.1:41417"));
    assert_eq!(env.get("ANTHROPIC_MODEL").map(String::as_str), Some("qwen3.8"));
    assert!(env.contains_key("ANTHROPIC_AUTH_TOKEN"), "{env:?}");
    // The one thing that must never appear: a provider's real key. None was ever given to this
    // profile — providers.toml is never read to build this command — so the container's own
    // words are the proof: the key stays on this machine, never inside what a tab is started with.
    let spelled = format!("{command:?}");
    assert!(!spelled.to_lowercase().contains("apikey") && !spelled.contains("sk-"), "{spelled}");
}

#[test]
fn a_provider_tab_runs_the_relay_which_starts_the_harness_and_an_ordinary_tab_does_not() {
    let scratch = Scratch::new("provider-program");
    let profiles = vec![provider_profile("ev-tab", "ev1", "qwen3.8"), profile("plain", HarnessKind::ClaudeCode)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("ev-tab".to_owned()));

    let editor = screen.editor();
    let workspace = screen.workspace().expect("a workspace");
    let chosen = &workspace.tabs()[0];
    let program = workspace.program(chosen, editor).expect("a harness tab runs something");
    // The relay must already answer at the address the tab's environment names when the harness
    // makes its first request, and the only thing that can promise that is the relay starting the
    // harness itself.
    assert_eq!(program[0], "node", "{program:?}");
    assert!(program[1].ends_with("qcode-relay.mjs"), "{program:?}");
    assert_eq!(&program[2..], ["claude", "--dangerously-skip-permissions"], "{program:?}");

    // A profile that signs in the ordinary way runs its harness and nothing else: no relay is
    // started for a tab that has no provider to reach.
    open(&mut screen, Choice::NewChat("plain".to_owned()));
    let editor = screen.editor();
    let workspace = screen.workspace().expect("a workspace");
    let plain = &workspace.tabs()[1];
    let program = workspace.program(plain, editor).expect("a harness tab runs something");
    assert_eq!(program[0], "claude", "{program:?}");
}

#[test]
fn a_tab_is_told_the_window_this_server_was_measured_to_give_rather_than_leaving_it_assumed() {
    let scratch = Scratch::new("provider-window");
    let path = measured_providers_file("window", "ev1", "qwen3.8", 31_512);
    let profiles = vec![provider_profile("ev-tab", "ev1", "qwen3.8")];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .with_providers_path(Some(path.clone()));
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("ev-tab".to_owned()));

    // Not a default: a harness told nothing assumes its own models' room, which here would be
    // six times what this server really gives.
    let env = envs(&screen.launch_command(key(&screen, 0)).expect("a chosen tab has a command"));
    assert_eq!(env.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS").map(String::as_str), Some("31512"), "{env:?}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_tab_on_a_ready_made_service_is_told_the_window_the_service_gives_without_measuring_it() {
    let scratch = Scratch::new("provider-ready-made");
    // What the Providers page writes when the person picks Xiaomi's Token Plan and pastes a key:
    // the maker's models with the window it publishes, nothing measured.
    let path = std::env::temp_dir().join("qcode-workspace-providers-ready-made").join("providers.toml");
    let mut providers = crate::provider::Providers::in_memory();
    let kind = crate::provider::ProviderKind::MimoTokenPlan;
    let entry = crate::provider::ProviderEntry::new(
        crate::provider::Tag::parse("mimo").expect("a tag"),
        kind,
        kind.suggested_base(),
    );
    providers.add(entry).expect("the tag is free");
    // Beside it an ollama server whose model claims a window nobody measured: its claim is not
    // what it gives, so it is still not handed over.
    let mut ollama = crate::provider::ProviderEntry::new(
        crate::provider::Tag::parse("ev1").expect("a tag"),
        crate::provider::ProviderKind::Ollama,
        "http://127.0.0.1:11434",
    );
    let mut claimed = crate::provider::Model::new("qwen3.8");
    claimed.claimed = Some(262_144);
    ollama.models = vec![claimed];
    providers.add(ollama).expect("the tag is free");
    providers.at(&path).save().expect("the scratch file is written");

    let profiles =
        vec![provider_profile("mimo-tab", "mimo", "mimo-v2.6-flash"), provider_profile("ev-tab", "ev1", "qwen3.8")];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .with_providers_path(Some(path.clone()));
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("mimo-tab".to_owned()));
    open(&mut screen, Choice::NewChat("ev-tab".to_owned()));

    let env = envs(&screen.launch_command(key(&screen, 0)).expect("a chosen tab has a command"));
    assert_eq!(env.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS").map(String::as_str), Some("1048576"), "{env:?}");
    assert_eq!(
        env.get("ANTHROPIC_BASE_URL").map(String::as_str),
        Some("http://127.0.0.1:41417"),
        "the relay, not Xiaomi"
    );
    let env = envs(&screen.launch_command(key(&screen, 1)).expect("a chosen tab has a command"));
    assert_eq!(env.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"), None, "a claim of a server of one's own: {env:?}");
    let _ = std::fs::remove_dir_all(path.parent().expect("its folder"));
}

#[test]
fn an_opencode_tab_on_a_provider_is_told_of_the_relay_in_its_own_configuration_with_the_measured_window() {
    let scratch = Scratch::new("provider-opencode");
    let path = measured_providers_file("opencode", "ev1", "qwen3-coder:30b", 31_512);
    let profiles =
        vec![Profile { harness: HarnessKind::OpenCode, ..provider_profile("oc-tab", "ev1", "qwen3-coder:30b") }];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .with_providers_path(Some(path.clone()));
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("oc-tab".to_owned()));

    let editor = screen.editor();
    let workspace = screen.workspace().expect("a workspace");
    let program = workspace.program(&workspace.tabs()[0], editor).expect("a harness tab runs something");
    assert!(program[1].ends_with("qcode-relay.mjs"), "the relay starts opencode too: {program:?}");
    assert_eq!(&program[2..], ["opencode", "--auto"], "{program:?}");

    let env = envs(&screen.launch_command(key(&screen, 0)).expect("a chosen tab has a command"));
    assert!(!env.contains_key("ANTHROPIC_BASE_URL"), "opencode is not Claude Code: {env:?}");
    let config: serde_json::Value =
        serde_json::from_str(env.get("OPENCODE_CONFIG_CONTENT").expect("opencode is told of the provider"))
            .expect("JSON");
    let provider = &config["provider"]["ev1"];
    assert_eq!(provider["options"]["baseURL"], "http://127.0.0.1:41417/v1");
    assert_eq!(config["model"], "ev1/qwen3-coder:30b");
    // A value only the measurement on the Providers page could have put there.
    assert_eq!(provider["models"]["qwen3-coder:30b"]["limit"]["context"], 31_512);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_codex_tab_on_a_provider_names_the_relay_on_its_command_line_before_a_conversation_it_resumes() {
    let scratch = Scratch::new("provider-codex");
    let profiles =
        vec![Profile { harness: HarnessKind::Codex, ..provider_profile("cx-tab", "ev1", "kimi-for-coding") }];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("cx-tab".to_owned()));
    open(&mut screen, Choice::Resume("cx-tab".to_owned(), "01a0cf22-23dc-7fd0-8c28-6a9fef67a234".to_owned()));

    let editor = screen.editor();
    let workspace = screen.workspace().expect("a workspace");
    let program = workspace.program(&workspace.tabs()[0], editor).expect("a harness tab runs something");
    assert!(program[1].ends_with("qcode-relay.mjs"), "the relay starts Codex too: {program:?}");
    assert_eq!(program[2], "codex");
    assert_eq!(program[3], "-c", "the provider comes right after the program: {program:?}");
    assert!(program.contains(&"model=\"kimi-for-coding\"".to_owned()), "{program:?}");
    assert_eq!(program.last().map(String::as_str), Some("--dangerously-bypass-approvals-and-sandbox"));
    // Resuming, the subcommand still follows the provider, and the unattended argument still
    // follows the subcommand.
    let resumed = workspace.program(&workspace.tabs()[1], editor).expect("a harness tab runs something");
    let at = resumed.iter().position(|word| word == "resume").expect("the subcommand is there");
    assert_eq!(
        &resumed[at..],
        ["resume", "--dangerously-bypass-approvals-and-sandbox", "01a0cf22-23dc-7fd0-8c28-6a9fef67a234"]
    );
    assert!(resumed[3..at].iter().any(|word| word.starts_with("model_providers.qcode=")), "{resumed:?}");

    let env = envs(&screen.launch_command(key(&screen, 0)).expect("a chosen tab has a command"));
    assert!(env.contains_key("QCODE_BRIDGE"), "the variable Codex sends as its key is set: {env:?}");
}

#[test]
fn kimi_code_and_qwen_code_tabs_on_a_provider_are_told_of_the_relay_in_their_own_variables() {
    let scratch = Scratch::new("provider-kimi-qwen");
    let path = measured_providers_file("kimi-qwen", "ev1", "qwen3-coder:30b", 31_512);
    let profiles = vec![
        Profile { harness: HarnessKind::KimiCode, ..provider_profile("kimi-tab", "ev1", "qwen3-coder:30b") },
        Profile { harness: HarnessKind::QwenCode, ..provider_profile("qwen-tab", "ev1", "qwen3-coder:30b") },
    ];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .with_providers_path(Some(path.clone()));
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("kimi-tab".to_owned()));
    open(&mut screen, Choice::NewChat("qwen-tab".to_owned()));

    let editor = screen.editor();
    let workspace = screen.workspace().expect("a workspace");
    let kimi = workspace.program(&workspace.tabs()[0], editor).expect("a harness tab runs something");
    assert_eq!(&kimi[2..], ["kimi", "--auto"], "{kimi:?}");
    let qwen = workspace.program(&workspace.tabs()[1], editor).expect("a harness tab runs something");
    assert_eq!(&qwen[2..], ["qwen", "--approval-mode=yolo"], "{qwen:?}");

    let env = envs(&screen.launch_command(key(&screen, 0)).expect("a chosen tab has a command"));
    assert_eq!(env.get("KIMI_MODEL_BASE_URL").map(String::as_str), Some("http://127.0.0.1:41417/v1"), "{env:?}");
    assert_eq!(env.get("KIMI_MODEL_NAME").map(String::as_str), Some("qwen3-coder:30b"));
    // A value only the measurement on the Providers page could have put there.
    assert_eq!(env.get("KIMI_MODEL_MAX_CONTEXT_SIZE").map(String::as_str), Some("31512"));
    let env = envs(&screen.launch_command(key(&screen, 1)).expect("a chosen tab has a command"));
    assert_eq!(env.get("OPENAI_BASE_URL").map(String::as_str), Some("http://127.0.0.1:41417/v1"), "{env:?}");
    assert_eq!(env.get("OPENAI_MODEL").map(String::as_str), Some("qwen3-coder:30b"));
    assert_eq!(env.get("OPENAI_API_KEY"), env.get("QCODE_BRIDGE"), "the tab's own token, never a provider's key");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_profile_whose_provider_tag_was_deleted_says_so_instead_of_starting() {
    let scratch = Scratch::new("provider-missing");
    let path = providers_file("missing", "ev1", false);
    let profiles = vec![provider_profile("ev-tab", "ev1", "qwen3.8")];
    let screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .with_providers_path(Some(path));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    open_in(&mut harness, Choice::NewChat("ev-tab".to_owned()));

    let state = harness.app().0.workspace().expect("a workspace").tabs()[0].state().clone();
    let TabState::Failed(failure) = state else { panic!("a deleted tag cannot start a tab: {state:?}") };
    assert!(failure.output.contains("ev1"), "the tag it could not find is named: {failure:?}");
    assert_eq!(failure.command, "", "nothing was even tried against the engine");
}

#[test]
fn the_container_a_tab_enters_carries_the_workspace_and_the_profiles_permissions() {
    let scratch = Scratch::new("create");
    let screen = one_workspace(&scratch);
    let open = screen.workspace().expect("a workspace is open");
    let engine = screen.engine().expect("an engine");
    let user = HostUser::Ids { uid: 1000, gid: 1000 };

    let base = open.plan(&TabKind::Shell).expect("the base container is planned");
    let words: Vec<String> =
        base.create(engine, user).args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert!(words.contains(&"qcode-firefly-base".to_owned()), "{words:?}");
    assert!(words.contains(&"qcode/base".to_owned()), "{words:?}");
    assert!(words.iter().any(|word| word.contains(&format!("Work:{CODE_DIR}:rw"))), "{words:?}");

    let profile = open.plan(&TabKind::Profile("claude-sub".to_owned())).expect("the profile container is planned");
    let words: Vec<String> =
        profile.create(engine, user).args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert!(words.contains(&"qcode/profile/claude-sub".to_owned()), "{words:?}");
    assert!(words.contains(&"--userns=keep-id".to_owned()), "podman maps the user its own way: {words:?}");
    assert!(words.iter().any(|word| word.contains("/Assets:/assets:ro")), "read-only assets: {words:?}");
    assert!(words.iter().any(|word| word.starts_with("qcode-home-firefly-claude-sub:")), "{words:?}");
    let bridge = format!("{}:/run/qcode-mcp:ro,z", scratch.0.join("Containers").join("MCP").display());
    assert!(words.contains(&bridge), "the profile container sees the bridge, read-only: {words:?}");
    let label = format!("qcode.plan={}", profile.digest(engine, user));
    assert!(words.contains(&label), "the container carries the plan it was made from: {words:?}");

    let base_words: Vec<String> =
        base.create(engine, user).args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert!(!base_words.iter().any(|word| word.contains("qcode-mcp")), "no harness, no bridge: {base_words:?}");
}

/// A file opened from the tree runs its built-in app in the workspace's own container, which is
/// always Debian's, whatever system the workspace's profiles were built on. That is why a program
/// another system's image does not carry (docx2txt on Alpine, sox on Arch) never leaves a file
/// without its opener: the opener never runs there.
#[test]
fn every_file_opens_in_the_debian_container_whatever_system_the_profiles_run_on() {
    let scratch = Scratch::new("opener");
    let screen = one_workspace(&scratch);
    let open = screen.workspace().expect("a workspace is open");
    for kind in [
        TabKind::Image("a.png".to_owned()),
        TabKind::Editor("a.rs".to_owned()),
        TabKind::Pdf("a.pdf".to_owned()),
        TabKind::Office("a.docx".to_owned()),
        TabKind::Shell,
    ] {
        let plan = open.plan(&kind).expect("the file is opened in a container");
        assert_eq!(plan.image, crate::base::Os::Debian.image(), "{kind:?}");
    }
    assert_eq!(crate::base::Os::Debian.gaps(), [], "the Debian image carries every opener's program");
}

#[test]
fn a_plan_that_mounts_or_reaches_anything_else_is_another_plan() {
    let scratch = Scratch::new("digest");
    let screen = one_workspace(&scratch);
    let open = screen.workspace().expect("a workspace is open");
    let engine = screen.engine().expect("an engine");
    let user = HostUser::Ids { uid: 1000, gid: 1000 };
    let plan = open.plan(&TabKind::Profile("claude-sub".to_owned())).expect("the profile container is planned");
    assert_eq!(plan.digest(engine, user), plan.digest(engine, user), "the same plan, the same digest");
    let without_bridge = super::ContainerPlan { bridge: None, ..plan.clone() };
    let without_network = super::ContainerPlan { network: crate::engine::Network::None, ..plan.clone() };
    for other in [without_bridge.digest(engine, user), without_network.digest(engine, user)] {
        assert_ne!(other, plan.digest(engine, user));
    }
    // Docker names the user in the command; podman maps whoever runs it, so there it is not
    // part of the plan.
    let docker = Engine::new(EngineKind::Docker, NO_ENGINE);
    let someone_else = HostUser::Ids { uid: 1001, gid: 1001 };
    assert_ne!(plan.digest(&docker, user), plan.digest(&docker, someone_else));
}

#[test]
fn a_screen_with_no_tab_says_what_a_tab_is_and_offers_one() {
    let scratch = Scratch::new("empty");
    let harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    let screen = harness.screen();
    assert!(screen.contains("No tab is open"), "{screen}");
    assert!(screen.contains("New tab"), "{screen}");
    assert!(screen.contains("Panel"), "the widget panel stands beside it:\n{screen}");
    assert!(screen.contains('F'), "the collapsed rail marks the workspace by its initial:\n{screen}");
}

/// The line of the tab strip, which is the first line of the screen.
fn strip(harness: &Harness<Screen>) -> String {
    harness.screen().lines().next().unwrap_or_default().to_owned()
}

/// The column of the `+` that opens a blank tab, on the line of the tab strip.
fn plus_column(harness: &Harness<Screen>) -> usize {
    let line = strip(harness);
    line.chars().position(|cell| cell == '+').unwrap_or_else(|| panic!("the strip carries a plus:\n{line}"))
}

#[test]
fn the_plus_stands_right_after_the_last_tab() {
    let scratch = Scratch::new("plus-after");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    open(&mut screen, claude());
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    for mode in [GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        let line: Vec<char> = strip(&harness).chars().collect();
        let label = "claude-sub".chars().count();
        let last = line.windows(label).rposition(|cells| cells.iter().collect::<String>() == "claude-sub");
        let end = last.expect("the last tab is on the strip") + label;
        let plus = plus_column(&harness);
        let between: String = line[end..plus].iter().collect();
        // Only the tab's own close mark and the air of the tab and of the button stand between.
        assert!(between.chars().all(|cell| cell == ' ' || cell == '×' || cell == 'x'), "{mode:?}: {between:?}");
        assert!(between.chars().count() <= 8, "the plus follows the tab closely in {mode:?}:\n{}", strip(&harness));
    }
}

#[test]
fn with_no_tab_the_plus_stands_at_the_top_left() {
    let scratch = Scratch::new("plus-left");
    let harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    let plus = plus_column(&harness);
    let rail = usize::from(super::RAIL_WIDTH);
    assert!((rail..rail + 3).contains(&plus), "{}", harness.screen());
    let (_, empty_y) = harness.find("No tab is open").expect("the empty state");
    assert!(empty_y > 0, "above the empty state, at its left edge:\n{}", harness.screen());
}

#[test]
fn with_many_tabs_the_plus_keeps_its_place_at_the_right_end() {
    let scratch = Scratch::new("plus-many");
    let mut screen = one_workspace(&scratch);
    for _ in 0..20 {
        open(&mut screen, Choice::Shell);
    }
    let harness = harness(screen, SIZE.0, SIZE.1);
    let plus = plus_column(&harness);
    let (panel, _) = harness.find("Panel").expect("the panel");
    let panel = usize::try_from(panel).expect("a column");
    assert!(plus + 12 > panel && plus < panel, "the plus ends the middle column:\n{}", harness.screen());
    assert!(strip(&harness).contains("Shell 20"), "the open tab, the last one, is in view:\n{}", strip(&harness));
}

#[test]
fn the_plus_the_empty_state_and_the_message_open_a_blank_tab_at_once() {
    let scratch = Scratch::new("blank");
    let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    let plus = i32::try_from(plus_column(&harness)).expect("a column");
    harness.click(plus, 0).advance(Duration::from_millis(300));
    assert_eq!(kinds(&harness.app().0), [TabKind::New], "the plus opens a blank tab:\n{}", harness.screen());
    assert!(harness.is_focused("workspace-choices"), "and its page has the keyboard");

    harness.send(Msg::CloseTab(0));
    assert!(harness.screen().contains("No tab is open"), "{}", harness.screen());
    harness.click_text("New tab").advance(Duration::from_millis(300));
    assert_eq!(kinds(&harness.app().0), [TabKind::New], "so does the empty state's button");

    harness.send(Msg::NewTab);
    let open = harness.app().0.workspace().expect("a workspace");
    assert_eq!(kinds(&harness.app().0), [TabKind::New, TabKind::New], "several blank tabs are fine");
    assert_eq!(open.active_tab().map(Tab::key), Some(open.tabs()[1].key()), "the new one is open");
    for tab in open.tabs() {
        assert_eq!(tab.state(), &TabState::Waiting, "nothing started");
        assert!(harness.app().0.launch_command(tab.key()).is_none(), "and nothing would be spawned");
    }
    assert_eq!(strip(&harness).matches("New tab").count(), 2, "{}", strip(&harness));
}

#[test]
fn the_keyboard_reaches_the_plus_from_the_tabs_it_ends() {
    let scratch = Scratch::new("plus-keys");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    for _ in 0..8 {
        if harness.is_focused("workspace-tabs") {
            break;
        }
        harness.press("tab");
    }
    assert!(harness.is_focused("workspace-tabs"), "the strip takes the keyboard:\n{}", harness.screen());
    // The `+` is the strip's second stop, so Tab from the tabs lands on it and Enter presses it.
    harness.press("tab").press("enter").advance(Duration::from_millis(300));
    assert_eq!(kinds(&harness.app().0), [TabKind::Shell, TabKind::New], "{}", harness.screen());
    assert!(harness.is_focused("workspace-choices"), "the new tab's page has the keyboard");
}

#[test]
fn the_page_lists_the_shell_then_a_section_for_every_profile() {
    let scratch = Scratch::new("page");
    let written = file("firefly", "Firefly", &["claude-sub"]);
    let profiles = vec![profile("opencode", HarnessKind::OpenCode), profile("claude-sub", HarnessKind::ClaudeCode)];
    let screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::ImageDefault,
        vec![OpenWorkspace::new(&written, scratch.paths(), profiles)],
    );
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.send(Msg::NewTab);
    let text = harness.screen();
    let row = |needle: &str| harness.find(needle).unwrap_or_else(|| panic!("`{needle}` is on the page:\n{text}")).1;
    assert!(row("What should this tab open?") < row("in the workspace's own container"), "{text}");
    let shell = row("in the workspace's own container");
    let claude = row("claude-sub • Claude Code");
    let opencode = row("opencode • opencode • adds to workspace");
    assert!(shell < claude && claude < opencode, "the shell, then the carried profile, then the other:\n{text}");
    let chats: Vec<usize> =
        text.lines().enumerate().filter(|(_, line)| line.contains("New chat")).map(|(y, _)| y).collect();
    let (claude, opencode) = (usize::try_from(claude).expect("a row"), usize::try_from(opencode).expect("a row"));
    assert_eq!(chats, [claude + 1, opencode + 1], "each section starts with a new chat:\n{text}");
    let (title_x, _) = harness.find("What should this tab open?").expect("the question");
    assert!(title_x < i32::from(SIZE.0) / 8, "the page reads from the left, not the middle:\n{text}");
}

#[test]
fn the_keyboard_walks_the_page_and_enter_chooses() {
    let scratch = Scratch::new("page-keys");
    let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    harness.send(Msg::NewTab).render();
    let tab = key(&harness.app().0, 0);
    harness.press("down");
    assert_eq!(harness.app().0.blank_row, 3, "the heading and the gap are stepped over");
    harness.press("enter");
    assert_eq!(kinds(&harness.app().0), [TabKind::Profile("claude-sub".to_owned())], "{}", harness.screen());
    assert_eq!(key(&harness.app().0, 0), tab, "the same tab turned into it");
}

#[test]
fn choosing_turns_the_same_tab_into_a_shell_or_a_chat_inside_a_container() {
    let scratch = Scratch::new("choose");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    apply(&mut screen, Msg::NewTab);
    apply(&mut screen, Msg::NewTab);
    apply(&mut screen, Msg::OpenTab(1));
    let (first, second) = (key(&screen, 1), key(&screen, 2));
    assert_eq!(super::entry(&screen), "workspace-choices", "a blank tab's page takes the keyboard");

    apply(&mut screen, Msg::Choose(first, claude()));
    apply(&mut screen, Msg::Choose(second, Choice::Shell));
    assert_eq!(kinds(&screen), [TabKind::Shell, TabKind::Profile("claude-sub".to_owned()), TabKind::Shell]);
    assert_eq!((key(&screen, 1), key(&screen, 2)), (first, second), "each in its own place");
    let chosen = &screen.workspace().expect("a workspace").tabs()[1];
    assert!(chosen.state().is_starting(), "its container is being started");
    assert_eq!(chosen.conversation(), None, "a new chat has no conversation yet");
    assert!(chosen.opened() > 0, "and was opened now");
    assert_eq!(super::entry(&screen), "workspace-terminal");

    let words = |key: TabKey| -> Vec<String> {
        let command = screen.launch_command(key).expect("a chosen tab has a command");
        assert_eq!(command.program, Path::new(NO_ENGINE), "only the engine binary is ever started");
        spelled(&command)
    };
    assert_eq!(words(first)[..4], ["exec", "--interactive", "--tty", "qcode-firefly-claude-sub"]);
    assert_eq!(words(second)[..4], ["exec", "--interactive", "--tty", "qcode-firefly-base"]);

    apply(&mut screen, Msg::Choose(first, Choice::Shell));
    assert_eq!(kinds(&screen)[1], TabKind::Profile("claude-sub".to_owned()), "a chosen tab is not chosen again");
}

#[test]
fn a_profile_the_workspace_does_not_carry_is_offered_and_joins_it_when_opened() {
    // The profile was made after the workspace, so the workspace's file does not name it. It is
    // offered all the same, says that choosing it adds it, and choosing it writes the file.
    let scratch = Scratch::new("joins");
    let paths = scratch.paths();
    let written = file("firefly", "Firefly", &["claude-sub"]);
    fs::write(&paths.file, written.to_toml()).expect("the workspace's file");
    let profiles = vec![profile("opencode", HarnessKind::OpenCode), profile("claude-sub", HarnessKind::ClaudeCode)];
    let open = OpenWorkspace::new(&written, paths.clone(), profiles);
    let order: Vec<&str> = open.profiles().iter().map(|profile| profile.name.as_str()).collect();
    assert_eq!(order, ["claude-sub", "opencode"], "the carried profile comes first");
    let screen = WorkspaceScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, vec![open]);

    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.click_text("New tab").advance(Duration::from_millis(300));
    let text = harness.screen();
    assert!(text.contains("opencode"), "a profile of the store is offered:\n{text}");
    assert!(text.contains("adds to workspace"), "and says what choosing it does:\n{text}");

    // Its new chat is the row under its heading.
    let (x, y) = harness.find("opencode • opencode").expect("the heading of its section");
    harness.click(x, y + 1).advance(Duration::from_millis(300));
    let open = harness.app().0.workspace().expect("a workspace");
    let kinds: Vec<TabKind> = open.tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [TabKind::Profile("opencode".to_owned())], "the tab opened");
    assert!(open.carries("opencode"), "the screen knows the workspace carries it now");
    let on_disk = WorkspaceFile::parse("workspace.qcode", &fs::read_to_string(&paths.file).expect("the file is there"));
    let names: Vec<String> =
        on_disk.value.expect("still a workspace").profiles.into_iter().map(|profile| profile.name).collect();
    assert_eq!(names, ["claude-sub", "opencode"], "the file records it, after what it named before");
    assert!(paths.harness_profile("opencode").is_dir(), "with the profile's folder in the workspace");
}

#[test]
fn a_profile_that_could_not_be_recorded_still_opens_and_says_so() {
    // No workspace.qcode at the workspace's place: the container does not need it, so the tab opens,
    // and the person learns the record is missing.
    let scratch = Scratch::new("unrecorded");
    let open = OpenWorkspace::new(
        &file("firefly", "Firefly", &[]),
        scratch.paths(),
        vec![profile("claude-sub", HarnessKind::ClaudeCode)],
    );
    let screen = WorkspaceScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, vec![open]);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    open_in(&mut harness, claude());
    harness.advance(Duration::from_millis(300));
    let open = harness.app().0.workspace().expect("a workspace");
    assert_eq!(open.tabs().len(), 1, "the tab opened");
    assert!(!open.carries("claude-sub"));
    let text = harness.screen();
    assert!(text.contains("could not be recorded"), "{text}");
}

/// The workspace screen, remembering whether it asked for the profiles screen, which is the
/// application's to open.
struct Watched(WorkspaceScreen, bool);

impl App for Watched {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        self.1 |= matches!(message, Msg::ManageProfiles);
        super::update(&mut self.0, message)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        super::view(&self.0, ui, std::convert::identity, |_| {}, |_| {}, |_| {});
    }
}

#[test]
fn with_no_profile_at_all_the_page_leads_to_the_profiles_screen() {
    let scratch = Scratch::new("no-profiles");
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), Vec::new())],
    );
    apply(&mut screen, Msg::NewTab);
    let mut harness = Harness::with_env(Watched(screen, false), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).render();
    let text = harness.screen();
    assert!(text.contains("Shell"), "the shell is still there:\n{text}");
    assert!(text.contains("New profile"), "and the way to a profile after it:\n{text}");
    assert!(text.contains("No profile yet"), "{text}");

    harness.click_text("New profile").advance(Duration::from_millis(300));
    assert!(harness.app().1, "the row asks for the profiles screen");
    assert_eq!(kinds(&harness.app().0), [TabKind::New], "and the tab stays blank for when the person is back");
}

#[test]
fn profiles_read_again_reach_every_open_workspace() {
    let scratch = Scratch::new("reread");
    let mut screen = one_workspace(&scratch);
    apply(
        &mut screen,
        Msg::Profiles(vec![profile("opencode", HarnessKind::OpenCode), profile("claude-sub", HarnessKind::ClaudeCode)]),
    );
    let names: Vec<&str> =
        screen.workspace().expect("a workspace").profiles().iter().map(|profile| profile.name.as_str()).collect();
    assert_eq!(names, ["claude-sub", "opencode"], "the new one is offered, after the carried one");
}

#[test]
fn a_hovered_workspace_names_itself_beside_the_collapsed_rail() {
    let first = Scratch::new("hover-a");
    let second = Scratch::new("hover-b");
    let workspaces = vec![
        workspace("firefly", "Firefly", first.paths(), Vec::new()),
        workspace("moth", "Moth", second.paths(), Vec::new()),
    ];
    let screen = WorkspaceScreen::new(Some(engine()), HostUser::ImageDefault, workspaces);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    assert!(!harness.screen().contains("Moth"), "a four-cell rail has no room for names");
    let (x, y) = harness.find("M").expect("the second workspace's initial is on screen");
    harness.hover(x, y);
    assert!(harness.screen().contains("Moth"), "the pointer brings the name card:\n{}", harness.screen());
}

#[test]
fn a_tab_waiting_for_its_container_says_so() {
    let scratch = Scratch::new("starting");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    assert!(screen.workspace().expect("a workspace").tabs()[0].state().is_starting());
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("Starting the container"), "{text}");
    assert!(text.contains("Shell"), "the tab is in the strip:\n{text}");
}

#[test]
fn a_container_that_stops_is_named_and_offered_a_restart() {
    let scratch = Scratch::new("stopped");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    let tab = key(&screen, 0);
    // The session ends, and the engine answers that the container under it is gone: that pair is
    // the case the design asks to be named, rather than left as "the shell exited".
    apply(&mut screen, Msg::Output(tab, 0, TerminalEvent::Exited(Some(0))));
    apply(&mut screen, Msg::Checked(tab, 0, false));
    assert_eq!(screen.workspace().expect("a workspace").tabs()[0].state(), &TabState::Stopped);

    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("The container stopped"), "{text}");
    assert!(text.contains("Start again"), "{text}");
    harness.click_text("Start again");
    // The engine of these tests cannot be run, so the restart gets as far as the engine and no
    // further; that it got there at all is what the button promises. A container that is gone
    // is made again from the base image, which is made sure of first, so the engine is met at
    // the base image's build.
    let state = harness.app().0.workspace().expect("a workspace").tabs()[0].state().clone();
    let TabState::Failed(failure) = state else { panic!("pressing it goes back to the engine: {state:?}") };
    assert!(failure.command.contains("build"), "it starts from the base image: {failure:?}");
    assert!(failure.command.contains("qcode/base"), "{failure:?}");
}

#[test]
fn an_engine_that_refuses_shows_its_own_words() {
    let scratch = Scratch::new("refused");
    let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    // The engine binary of these tests cannot be run, so opening a tab really does fail here.
    open_in(&mut harness, Choice::Shell);
    let text = harness.screen();
    assert!(text.contains("The engine refused"), "{text}");
    assert!(text.contains("Start again"), "{text}");
    let state = harness.app().0.workspace().expect("a workspace").tabs()[0].state().clone();
    let TabState::Failed(failure) = state else { panic!("the tab carries the engine's refusal") };
    assert!(failure.command.contains(NO_ENGINE), "the command it tried is kept: {failure:?}");
    assert!(!failure.output.is_empty(), "so is what the system said: {failure:?}");
}

#[test]
fn without_an_engine_the_page_is_faint_and_nothing_starts() {
    let scratch = Scratch::new("no-engine");
    let shown = |engine: Option<Engine>| {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            scratch.paths(),
            vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        )];
        let mut harness = harness(WorkspaceScreen::new(engine, HostUser::ImageDefault, workspaces), SIZE.0, SIZE.1);
        harness.click_text("New tab").advance(Duration::from_millis(300));
        harness
    };
    let working = shown(Some(engine()));
    let mut harness = shown(None);
    let text = harness.screen();
    assert_eq!(kinds(&harness.app().0), [TabKind::New], "the page opens all the same:\n{text}");
    assert!(text.contains("What should this tab open?"), "{text}");
    assert!(text.contains("No container engine was found"), "and says why nothing can be chosen:\n{text}");
    assert!(!working.screen().contains("No container engine was found"), "{}", working.screen());
    let fg = |harness: &Harness<Screen>| {
        let (x, y) = harness.find("New chat").expect("the row of a new chat");
        harness.fg(u16::try_from(x).expect("a column"), u16::try_from(y).expect("a row"))
    };
    assert_ne!(fg(&harness), fg(&working), "its rows are faint");

    harness.click_text("Shell").advance(Duration::from_millis(300));
    harness.click_text("New chat").advance(Duration::from_millis(300));
    let tab = &harness.app().0.workspace().expect("a workspace").tabs()[0];
    assert_eq!((tab.kind(), tab.state()), (&TabKind::New, &TabState::Waiting), "choosing does nothing");
}

#[test]
fn tabs_close_and_move() {
    let scratch = Scratch::new("tabs");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    open(&mut screen, claude());
    apply(&mut screen, Msg::MoveTab { from: 1, to: 0 });
    let kinds: Vec<TabKind> =
        screen.workspace().expect("a workspace").tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [TabKind::Profile("claude-sub".to_owned()), TabKind::Shell]);

    apply(&mut screen, Msg::CloseTab(0));
    let kinds: Vec<TabKind> =
        screen.workspace().expect("a workspace").tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [TabKind::Shell], "closing a tab leaves the others where they were");
}

#[test]
fn several_shells_are_told_apart() {
    let scratch = Scratch::new("shells");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    open(&mut screen, Choice::Shell);
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("Shell 1"), "{text}");
    assert!(text.contains("Shell 2"), "{text}");
}

#[test]
fn the_rail_switches_between_workspaces() {
    let first = Scratch::new("rail-a");
    let second = Scratch::new("rail-b");
    let workspaces = vec![
        workspace("firefly", "Firefly", first.paths(), Vec::new()),
        workspace("moth", "Moth", second.paths(), Vec::new()),
    ];
    let mut screen = WorkspaceScreen::new(Some(engine()), HostUser::ImageDefault, workspaces);
    apply(&mut screen, Msg::OpenWorkspace(1));
    assert_eq!(screen.workspace().expect("a workspace").name(), "Moth");
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains('M'), "the collapsed rail marks a workspace by its initial:\n{text}");
}

#[test]
fn the_file_tree_reads_the_workspace_folder_and_opens_a_folder() {
    let scratch = Scratch::new("files");
    let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    harness.send(Msg::OpenWorkspace(0));
    let text = harness.screen();
    assert!(text.contains("README.md"), "{text}");
    assert!(text.contains("src"), "{text}");
    assert!(!text.contains("main.rs"), "a closed folder shows nothing of itself:\n{text}");

    harness.send(Msg::ExpandFile("src".to_owned(), true));
    let text = harness.screen();
    assert!(text.contains("main.rs"), "opening the folder reads it:\n{text}");
}

#[test]
fn an_unreadable_workspace_folder_says_why() {
    let scratch = Scratch::new("unreadable");
    let mut paths = scratch.paths();
    paths.code = paths.code.join("gone");
    let workspaces = vec![workspace("firefly", "Firefly", paths, Vec::new())];
    let screen = WorkspaceScreen::new(Some(engine()), HostUser::ImageDefault, workspaces);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.send(Msg::OpenWorkspace(0));
    let text = harness.screen();
    assert!(text.contains("The workspace folder"), "the panel says which folder it is about:\n{text}");
    let reason = harness.app().0.workspace().expect("a workspace").files().error().unwrap_or_default().to_owned();
    assert!(!reason.is_empty(), "and keeps what the system said");
    assert!(text.contains("No such file"), "which is on screen too:\n{text}");
}

#[test]
fn widgets_are_added_taken_off_and_moved() {
    let scratch = Scratch::new("widgets");
    let mut screen = one_workspace(&scratch);
    assert_eq!(screen.panel().order(), PanelWidget::ALL);

    apply(&mut screen, Msg::RemoveWidget(PanelWidget::Files));
    assert!(!screen.panel().carries(PanelWidget::Files), "the widget is off the panel");
    assert_eq!(screen.panel().missing(), [PanelWidget::Files]);
    apply(&mut screen, Msg::ShowWidgets(true));
    apply(&mut screen, Msg::AddWidget(PanelWidget::Files));
    assert_eq!(screen.panel().order().last(), Some(&PanelWidget::Files), "it comes back at the end");
    assert!(screen.panel().is_unfolded(2), "unfolded, since it was added to be looked at");
    assert!(!screen.panel().chooser_open(), "adding closes the chooser");

    apply(&mut screen, Msg::MoveWidget { from: 2, to: 0 });
    assert_eq!(screen.panel().order()[0], PanelWidget::Files);

    apply(&mut screen, Msg::ToggleWidget(0, false));
    assert!(!screen.panel().is_unfolded(0), "a folded widget stays folded");
}

/// The line of the panel's title, and the column the title starts at.
fn panel_title_row(harness: &Harness<Screen>) -> (String, usize) {
    let (x, y) = harness.find("Panel").expect("the panel's title is on screen");
    let line =
        harness.screen().lines().nth(usize::try_from(y).expect("a row on screen")).unwrap_or_default().to_owned();
    (line, usize::try_from(x).expect("a column on screen"))
}

#[test]
fn with_every_widget_on_the_panel_there_is_nothing_to_add_and_no_button() {
    let scratch = Scratch::new("all-widgets");
    let harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    let (line, x) = panel_title_row(&harness);
    let after: String = line.chars().skip(x + "Panel".len()).collect();
    assert!(after.trim().is_empty(), "nothing stands beside the title:\n{}", harness.screen());
    assert!(!harness.screen().contains("Widgets"), "the old label is gone:\n{}", harness.screen());
}

#[test]
fn the_add_button_is_a_bare_plus_that_lists_only_the_missing_widgets() {
    let scratch = Scratch::new("add-widget");
    let mut screen = one_workspace(&scratch);
    apply(&mut screen, Msg::RemoveWidget(PanelWidget::Info));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    let (line, x) = panel_title_row(&harness);
    let after: String = line.chars().skip(x + "Panel".len()).collect();
    assert_eq!(after.trim(), "+", "an icon and no label:\n{}", harness.screen());
    assert!(!harness.screen().contains("Widgets"), "{}", harness.screen());

    let (plus_x, plus_y) = {
        let (_, y) = harness.find("Panel").expect("the title");
        let column = line.chars().collect::<Vec<_>>().iter().rposition(|c| *c == '+').expect("the plus");
        (i32::try_from(column).expect("a column"), y)
    };
    harness.click(plus_x, plus_y);
    assert!(harness.app().0.panel().chooser_open(), "{}", harness.screen());
    let text = harness.screen();
    assert!(text.contains("Workspace"), "the missing widget is offered:\n{text}");
    // The carried ones are on the panel as titles (the popover may cover one), but they are not
    // offered a second time.
    assert!(text.matches("Files").count() <= 1, "a carried widget is not offered:\n{text}");
    assert_eq!(text.matches("Containers").count(), 1, "{text}");
    for mark in ['✓', '✔', '☐', '☑'] {
        assert!(!text.contains(mark), "a plain list, no checkboxes:\n{text}");
    }

    harness.press("enter");
    let panel = harness.app().0.panel();
    assert!(panel.carries(PanelWidget::Info), "choosing adds it:\n{}", harness.screen());
    assert!(!panel.chooser_open(), "and closes the popover");
    let (line, x) = panel_title_row(&harness);
    let after: String = line.chars().skip(x + "Panel".len()).collect();
    assert!(after.trim().is_empty(), "with everything carried the button goes:\n{}", harness.screen());
}

#[test]
fn a_right_click_on_the_panel_takes_a_widget_off() {
    use qframe::event::{MouseButton, MouseKind};

    let scratch = Scratch::new("remove-widget");
    let mut screen = one_workspace(&scratch);
    // Folded, the widget shows only its title, and it can still be taken off.
    apply(&mut screen, Msg::ToggleWidget(1, false));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    let (x, y) = harness.find("Workspace").expect("the folded widget's title");
    harness.mouse(MouseKind::Down(MouseButton::Right), x, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x, y);
    let text = harness.screen();
    assert!(text.contains("Remove Workspace from the panel"), "{text}");
    harness.click_text("Remove Workspace from the panel");
    let panel = harness.app().0.panel();
    assert!(!panel.carries(PanelWidget::Info), "{}", harness.screen());
    assert!(panel.carries(PanelWidget::Files) && panel.carries(PanelWidget::Containers));
    assert!(harness.screen().contains('+'), "and it can be added again:\n{}", harness.screen());
}

#[test]
fn the_panel_runs_from_the_top_row_to_the_bottom_one() {
    let scratch = Scratch::new("full-height");
    let screen = one_workspace(&scratch);
    let mut harness = Harness::with_env(Framed(screen), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Msg::OpenWorkspace(0)).render();
    let text = harness.screen();
    let (panel_x, _) = harness.find("Panel").expect("the panel");
    let (header_x, header_y) = harness.find("HEADER").expect("the application's header");
    let (footer_x, footer_y) = harness.find("FOOTER").expect("the application's footer");
    assert_eq!(header_y, 0, "the header is on the first row:\n{text}");
    assert_eq!(footer_y, i32::from(SIZE.1) - 1, "the footer on the last:\n{text}");
    assert!(header_x < panel_x && footer_x < panel_x, "both in the middle column:\n{text}");
    let right = SIZE.0 - 1;
    let panel_bg = harness.bg(right, 5);
    assert_eq!(harness.bg(right, 0), panel_bg, "the panel's own surface on the first row:\n{text}");
    assert_eq!(harness.bg(right, SIZE.1 - 1), panel_bg, "and on the last:\n{text}");
    // The rail takes the first columns of the first row, so the header starts beside it, and
    // its own surface reaches the last row.
    assert!(header_x >= i32::from(super::RAIL_WIDTH), "the rail runs the whole height too:\n{text}");
    assert_eq!(harness.bg(0, SIZE.1 - 1), harness.bg(0, SIZE.1 / 2), "{text}");

    // Folding and widening still reach the screen through the application's messages.
    for _ in 0..8 {
        if harness.is_focused("workspace-tabs") {
            break;
        }
        harness.press("tab");
    }
    harness.press("alt+b");
    assert!(!harness.app().0.panel().is_open(), "alt+b folds the panel:\n{}", harness.screen());
    harness.press("alt+b");
    assert!(harness.app().0.panel().is_open(), "and opens it again");
    harness.set_reduced_motion(true).render();
    let edge = i32::from(SIZE.0) - i32::from(harness.app().0.panel().width());
    harness.drag((edge, 10), (edge - 6, 10));
    assert!(harness.app().0.panel().width() > super::PANEL_WIDTH, "dragging the edge widens it");
}

#[test]
fn the_container_widget_lists_this_workspaces_containers_and_acts_on_one() {
    let scratch = Scratch::new("containers");
    let mut screen = one_workspace(&scratch);
    let listed = vec![
        Container { name: "qcode-firefly-base".to_owned(), state: ContainerState::Running },
        Container { name: "qcode-firefly-claude-sub".to_owned(), state: ContainerState::Exited },
    ];
    apply(&mut screen, Msg::ContainersRead("firefly".to_owned(), Ok(listed)));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("base"), "the part of the name that differs is what the panel shows:\n{text}");
    assert!(text.contains("claude-sub"), "{text}");
    assert!(text.contains("running"), "the state is written, never only coloured:\n{text}");
    assert!(text.contains("stopped"), "{text}");

    harness.send(Msg::SelectContainer(1));
    let text = harness.screen();
    assert!(text.contains("Stop"), "{text}");
    assert!(text.contains("Restart"), "a stopped container can still be restarted:\n{text}");
}

#[test]
fn a_workspace_without_a_container_says_so_in_a_whole_sentence() {
    // The panel is a third of a 120-column screen at most, and the sentence is longer than that.
    let scratch = Scratch::new("no-container");
    let mut harness = harness(one_workspace(&scratch), 120, 36);
    harness.send(Msg::ContainersRead("firefly".to_owned(), Ok(Vec::new())));
    harness.render();
    let screen = harness.screen();
    let panel = panel_lines(&harness);
    let read = panel.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(read.contains("This workspace has no container yet"), "the sentence is read to its end:\n{screen}");
    assert!(!panel.iter().any(|line| line.contains('…')), "and nothing in the panel is cut:\n{screen}");
}

/// The panel's column of each line of the harness's screen, trimmed.
fn panel_lines(harness: &Harness<Screen>) -> Vec<String> {
    let width = usize::from(harness.app().0.panel().width());
    harness
        .screen()
        .lines()
        .map(|line| {
            let cells: Vec<char> = line.chars().collect();
            cells[cells.len().saturating_sub(width)..].iter().collect::<String>().trim().to_owned()
        })
        .collect()
}

#[test]
fn a_container_a_tab_brings_up_appears_in_the_panel_without_a_refresh() {
    // An engine that knows no container until one is started, and lists the started ones after.
    // Once started it says the container is running, so the tab's session ending at once is not
    // taken for the container going away: the list can only be read again because it came up.
    let scratch = Scratch::new("brought-up");
    let (binary, listed) = (scratch.0.join("engine"), scratch.0.join("listed"));
    let script = format!(
        "#!/bin/sh\n\
         [ \"$1\" = start ] && printf '%s\\trunning\\n' \"$2\" >> {listed}\n\
         [ \"$1\" = ps ] && {{ cat {listed} 2>/dev/null; exit 0; }}\n\
         [ \"$2\" = inspect ] && [ -e {listed} ] && {{ echo running; exit 0; }}\n\
         for word in \"$@\"; do case \"$word\" in *'qcode-new'*) cat > /dev/null ;; esac; done\n\
         exit 0\n",
        listed = listed.display(),
    );
    fs::write(&binary, script).expect("the stand-in engine is written");
    fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
    let workspaces =
        vec![workspace("firefly", "Firefly", scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)])];
    let screen = WorkspaceScreen::new(
        Some(Engine::new(EngineKind::Podman, &binary)),
        HostUser::Ids { uid: 1000, gid: 1000 },
        workspaces,
    );
    let mut harness = harness(screen, 120, 36);
    harness.set_reduced_motion(true).render();
    let before = panel_lines(&harness).join(" ");
    assert!(before.contains("no container"), "nothing is up yet:\n{}", harness.screen());

    harness.click_text("New tab").render();
    harness.click_text("New chat").advance(Duration::from_millis(400)).render();
    assert!(listed.exists(), "the tab started its container:\n{}", harness.screen());
    let after = panel_lines(&harness).join(" ");
    assert!(after.contains("claude-sub") && after.contains("running"), "the panel lists it:\n{}", harness.screen());
    assert!(!after.contains("no container"), "and no longer says there is none:\n{}", harness.screen());
}

#[test]
fn a_narrow_screen_keeps_the_rail_the_terminal_and_the_panel() {
    let scratch = Scratch::new("narrow");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    let mut harness = harness(screen, 46, 18);
    let text = harness.screen();
    // The framework keeps the terminal a quarter of the width whatever the panel asks for, so
    // the middle is narrow here and its label is cut rather than lost.
    assert!(text.contains("Startin"), "the middle keeps its place:\n{text}");
    assert!(text.contains("Panel"), "so does the panel:\n{text}");
    assert!(text.contains('F'), "the rail is still there:\n{text}");
    for line in text.lines() {
        assert!(line.chars().count() <= 46, "nothing runs off the screen:\n{text}");
    }
    // The panel runs the whole height beside the tabs, so on a screen this narrow the tab strip
    // has its room back once the panel is folded away.
    harness.set_reduced_motion(true).send(Msg::TogglePanel(false)).render();
    let text = harness.screen();
    assert!(text.contains("Shell"), "the tab strip is there with the panel away:\n{text}");
}

#[test]
fn a_closed_panel_gives_its_width_to_the_terminal() {
    let scratch = Scratch::new("panel-closed");
    let mut screen = one_workspace(&scratch);
    apply(&mut screen, Msg::TogglePanel(false));
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(!text.contains("Containers"), "the panel is away:\n{text}");
    assert!(text.contains("No tab is open"), "{text}");
}

#[test]
fn nothing_is_bracketed_lined_or_framed() {
    let scratch = Scratch::new("aesthetic");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, claude());
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        let text = harness.screen();
        for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│'] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {mode:?}:\n{text}");
        }
        assert!(!text.contains("=="), "{text}");
        assert!(!text.contains("->"), "{text}");
    }
}

#[test]
fn ascii_mode_draws_nothing_but_ascii() {
    let scratch = Scratch::new("ascii");
    let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    harness.set_glyph_mode(GlyphMode::Ascii).render();
    let text = harness.screen();
    assert!(text.is_ascii(), "{text}");
    assert!(text.contains("No tab is open"), "{text}");
}

#[test]
fn the_keyboard_reaches_the_rail_the_tabs_and_the_panel() {
    let scratch = Scratch::new("keys");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    open(&mut screen, claude());
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let (mut rail, mut tabs) = (false, false);
    for _ in 0..12 {
        harness.press("tab");
        rail |= harness.is_focused("workspace-rail");
        tabs |= harness.is_focused("workspace-tabs");
    }
    assert!(rail, "the keyboard reaches the rail:\n{}", harness.screen());
    assert!(tabs, "and the tabs:\n{}", harness.screen());
    for _ in 0..12 {
        if harness.is_focused("workspace-tabs") {
            break;
        }
        harness.press("tab");
    }
    harness.press("left");
    assert_eq!(harness.app().0.workspace().expect("a workspace").active_tab().map(Tab::key), Some(TabKey(0)));
}

#[test]
fn reduced_motion_keeps_the_screen_working() {
    let scratch = Scratch::new("motion");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    let text = harness.screen();
    assert!(text.contains("Starting the container"), "{text}");
}

#[test]
fn turkish_reads_as_turkish() {
    let scratch = Scratch::new("turkish");
    let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    harness.set_locale("tr").render();
    let text = harness.screen();
    for label in ["Açık sekme yok", "Yeni sekme", "Kapsayıcılar", "Dosyalar"] {
        assert!(text.contains(label), "`{label}` is missing:\n{text}");
    }
}

/// A screen of the workspaces `names`, each in a folder of its own, with an engine whose binary
/// cannot be run. The folders go when the returned scratches are dropped.
fn several(names: &[(&str, &str)]) -> (WorkspaceScreen, Vec<Scratch>) {
    let scratches: Vec<Scratch> = names.iter().map(|(id, _)| Scratch::new(id)).collect();
    let workspaces = names
        .iter()
        .zip(&scratches)
        .map(|((id, name), scratch)| {
            workspace(id, name, scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)])
        })
        .collect();
    (WorkspaceScreen::new(Some(engine()), HostUser::ImageDefault, workspaces), scratches)
}

/// A session of `firefly` with a shell and a profile tab, the profile one open, and `moth` with
/// one shell; `moth` is the open workspace.
fn recorded() -> Session {
    let id = |text: &str| WorkspaceId::parse(text).expect("a usable workspace id");
    Session {
        active: Some(id("moth")),
        workspaces: vec![
            SessionWorkspace {
                id: id("firefly"),
                active_tab: 1,
                tabs: vec![
                    SessionTab { kind: SessionTabKind::Shell, conversation: None, opened: 100 },
                    SessionTab {
                        kind: SessionTabKind::Profile("claude-sub".to_owned()),
                        conversation: Some("c-42".to_owned()),
                        opened: 200,
                    },
                ],
            },
            SessionWorkspace {
                id: id("moth"),
                active_tab: 0,
                tabs: vec![SessionTab { kind: SessionTabKind::Shell, conversation: None, opened: 300 }],
            },
        ],
    }
}

/// The workspaces of `session` brought back into a screen of those workspaces, the way the
/// application does it, without the work the screen asks for being run.
fn restored(session: &Session) -> (WorkspaceScreen, Vec<Scratch>) {
    let (mut screen, scratches) = several(&[("firefly", "Firefly"), ("moth", "Moth")]);
    for (index, record) in session.workspaces.iter().enumerate() {
        screen.restore_tabs(index, record);
    }
    screen.set_active(session.active_index());
    drop(super::opened(&mut screen));
    (screen, scratches)
}

fn state(screen: &WorkspaceScreen, workspace: usize, tab: usize) -> TabState {
    screen.workspaces()[workspace].tabs()[tab].state().clone()
}

#[test]
fn a_restored_screen_is_the_session_it_came_from() {
    let session = recorded();
    let (screen, _scratches) = restored(&session);
    assert_eq!(screen.session(), session, "order, open workspace, tabs, open tab, conversation and time");
    assert_eq!(screen.workspace().map(OpenWorkspace::name), Some("Moth"));
    let firefly = &screen.workspaces()[0];
    assert_eq!(firefly.active_tab().map(Tab::conversation), Some(Some("c-42")));
    assert_eq!(firefly.tabs()[0].opened(), 100);
}

#[test]
fn restored_tabs_wait_until_they_are_shown() {
    let (mut screen, _scratches) = restored(&recorded());
    assert_eq!(state(&screen, 1, 0), TabState::Starting, "the tab in view starts at once");
    assert_eq!(state(&screen, 0, 0), TabState::Waiting, "the ones out of view do not");
    assert_eq!(state(&screen, 0, 1), TabState::Waiting);

    apply(&mut screen, Msg::OpenWorkspace(0));
    assert_eq!(state(&screen, 0, 1), TabState::Starting, "opening the workspace shows its open tab");
    assert_eq!(state(&screen, 0, 0), TabState::Waiting, "but not the tab beside it");

    apply(&mut screen, Msg::OpenTab(0));
    assert_eq!(state(&screen, 0, 0), TabState::Starting, "switching to it starts it");
}

#[test]
fn closing_the_open_tab_starts_the_waiting_one_that_takes_its_place() {
    let (mut screen, _scratches) = restored(&recorded());
    apply(&mut screen, Msg::OpenWorkspace(0));
    apply(&mut screen, Msg::CloseTab(1));
    assert_eq!(state(&screen, 0, 0), TabState::Starting);
}

#[test]
fn a_restored_tab_whose_profile_is_gone_is_left_out() {
    let mut session = recorded();
    session.workspaces[0].tabs[1].kind = SessionTabKind::Profile("gone".to_owned());
    let (screen, _scratches) = restored(&session);
    let firefly = &screen.workspaces()[0];
    assert_eq!(firefly.tabs().len(), 1, "only the shell comes back");
    assert_eq!(firefly.active_tab().map(Tab::opened), Some(100), "and the tab before the lost one is open");
}

#[test]
fn without_an_engine_a_restored_tab_waits_and_says_why() {
    let scratch = Scratch::new("restored-no-engine");
    let workspaces = vec![workspace("firefly", "Firefly", scratch.paths(), Vec::new())];
    let mut screen = WorkspaceScreen::new(None, HostUser::ImageDefault, workspaces);
    let mut session = recorded();
    session.workspaces.truncate(1);
    session.workspaces[0].tabs.truncate(1);
    session.workspaces[0].active_tab = 0;
    screen.restore_tabs(0, &session.workspaces[0]);
    let harness = harness(screen, SIZE.0, SIZE.1);
    assert_eq!(state(&harness.app().0, 0, 0), TabState::Waiting);
    let text = harness.screen();
    assert!(text.contains("not started yet"), "{text}");
    assert!(text.contains("No container engine was found"), "{text}");
}

#[test]
fn a_workspace_joins_the_rail_once_and_is_opened_when_it_is_there_already() {
    let (mut screen, scratches) = several(&[("firefly", "Firefly")]);
    open(&mut screen, Choice::Shell);
    let tab = key(&screen, 0);
    let moth = Scratch::new("moth");
    drop(super::add(&mut screen, workspace("moth", "Moth", moth.paths(), Vec::new())));
    let names: Vec<&str> = screen.workspaces().iter().map(OpenWorkspace::name).collect();
    assert_eq!(names, ["Firefly", "Moth"], "it joins at the end of the rail");
    assert_eq!(screen.workspace().map(OpenWorkspace::name), Some("Moth"), "and is the one open");

    drop(super::add(&mut screen, workspace("firefly", "Firefly", scratches[0].paths(), Vec::new())));
    assert_eq!(screen.workspaces().len(), 2, "a workspace already there is not added twice");
    assert_eq!(screen.workspace().map(OpenWorkspace::name), Some("Firefly"));
    assert_eq!(key(&screen, 0), tab, "and its tabs are the ones it had");
}

#[test]
fn closing_a_workspace_ends_its_sessions_and_opens_the_next() {
    let (mut screen, _scratches) = several(&[("firefly", "Firefly"), ("moth", "Moth"), ("lantern", "Lantern")]);
    screen.set_active(1);
    open(&mut screen, Choice::Shell);
    // A real session on the host, standing in for the one a tab holds in its container, so its
    // end can be watched for.
    let session =
        qframe::widgets::TerminalSession::spawn(std::ffi::OsStr::new("sleep"), &["30"], &std::env::temp_dir())
            .expect("sleep runs");
    let watch = session.watch();
    screen.workspaces[1].tabs[0].attached(session);

    apply(&mut screen, Msg::CloseWorkspace(1));
    let names: Vec<&str> = screen.workspaces().iter().map(OpenWorkspace::name).collect();
    assert_eq!(names, ["Firefly", "Lantern"]);
    assert_eq!(screen.workspace().map(OpenWorkspace::name), Some("Lantern"), "the next workspace is open");
    let ended = std::thread::spawn(move || {
        loop {
            if let TerminalEvent::Exited(_) = watch.next() {
                return true;
            }
        }
    });
    assert!(ended.join().unwrap_or(false), "the tab's session was ended");

    apply(&mut screen, Msg::CloseWorkspace(1));
    apply(&mut screen, Msg::CloseWorkspace(0));
    assert!(screen.workspace().is_none(), "the last one can go too");
    assert_eq!(screen.session(), Session::default());
}

#[test]
fn the_rail_ends_in_a_plus_that_asks_for_the_list_of_workspaces() {
    let (screen, _scratches) = several(&[("firefly", "Firefly")]);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    let plus = text.lines().position(|line| line.chars().take(4).any(|cell| cell == '+'));
    let Some(row) = plus else { panic!("the rail carries a plus:\n{text}") };
    let row = i32::try_from(row).expect("a row of the screen");
    harness.hover(1, row);
    assert!(harness.screen().contains("Open a workspace"), "its card says what it does:\n{}", harness.screen());
    harness.set_locale("tr").render();
    assert!(harness.screen().contains("Çalışma alanı aç"), "{}", harness.screen());
}

#[test]
fn with_no_workspace_left_the_middle_offers_to_open_one() {
    let (mut screen, _scratches) = several(&[("firefly", "Firefly")]);
    apply(&mut screen, Msg::CloseWorkspace(0));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("No workspace is open"), "{text}");
    assert!(text.contains("Open a workspace"), "{text}");
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        let text = harness.screen();
        for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│'] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {mode:?}:\n{text}");
        }
    }
    harness.set_glyph_mode(GlyphMode::Ascii).render();
    assert!(harness.screen().is_ascii(), "{}", harness.screen());
    harness.set_locale("tr").render();
    assert!(harness.screen().contains("Çalışma alanı aç"), "{}", harness.screen());
}

#[test]
fn a_blank_tab_comes_back_blank_and_never_starts() {
    let mut session = recorded();
    session.workspaces[1].tabs.push(SessionTab { kind: SessionTabKind::New, conversation: None, opened: 400 });
    session.workspaces[1].active_tab = 1;
    let (mut screen, _scratches) = restored(&session);
    assert_eq!(screen.session(), session, "a blank tab is kept in the session like any other");
    assert_eq!(state(&screen, 1, 1), TabState::Waiting, "shown, it still has nothing to start");
    assert_eq!(super::entry(&screen), "workspace-choices", "its page takes the keyboard");
    apply(&mut screen, Msg::OpenTab(0));
    apply(&mut screen, Msg::OpenTab(1));
    assert_eq!(state(&screen, 1, 1), TabState::Waiting, "coming back to it starts nothing either");
    assert!(screen.launch_command(key(&screen, 1)).is_none());
}

#[test]
fn the_page_has_no_brackets_or_frames_in_any_glyph_mode_and_reads_as_turkish() {
    let scratch = Scratch::new("page-look");
    let open = OpenWorkspace::new(
        &file("firefly", "Firefly", &["claude-sub"]),
        scratch.paths(),
        vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("opencode", HarnessKind::OpenCode)],
    );
    let mut screen = WorkspaceScreen::new(Some(engine()), HostUser::ImageDefault, vec![open]);
    apply(&mut screen, Msg::NewTab);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        let text = harness.screen();
        for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│'] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {mode:?}:\n{text}");
        }
        assert!(text.contains("New chat"), "{text}");
    }
    let text = harness.screen();
    assert!(text.is_ascii(), "ASCII draws nothing but ASCII:\n{text}");
    assert!(text.contains("claude-sub - Claude Code"), "the heading keeps its separator in ASCII:\n{text}");

    harness.set_glyph_mode(GlyphMode::Unicode).set_locale("tr").render();
    let text = harness.screen();
    for label in [
        "Yeni sekmede ne açılsın?",
        "Kabuk",
        "çalışma alanının kendi kapsayıcısında",
        "Yeni sohbet",
        "çalışma alanına eklenir",
    ] {
        assert!(text.contains(label), "`{label}` is missing:\n{text}");
    }
    assert!(text.contains("Yeni sekme"), "the tab's own label:\n{text}");
}

/// The workspace screen inside an application that turns the key list's action into a message of
/// its own, the way QCode does, and remembers that it came.
struct Keyed {
    screen: WorkspaceScreen,
    help: usize,
}

/// What [`Keyed`] hears: the screen's own messages, or the key list being asked for.
#[derive(Debug, Clone)]
enum KeyedMsg {
    Screen(Msg),
    Help,
}

impl App for Keyed {
    type Msg = KeyedMsg;

    fn update(&mut self, message: KeyedMsg) -> Command<KeyedMsg> {
        match message {
            KeyedMsg::Screen(message) => super::update(&mut self.screen, message).map(KeyedMsg::Screen),
            KeyedMsg::Help => {
                self.help += 1;
                Command::none()
            }
        }
    }

    fn view(&self, ui: &mut View<'_, KeyedMsg>) {
        super::view(&self.screen, ui, KeyedMsg::Screen, |_| {}, |_| {}, |_| {});
    }

    fn action(&self, name: &str) -> Option<KeyedMsg> {
        match name {
            "help" => Some(KeyedMsg::Help),
            "leave-terminal" => Some(KeyedMsg::Screen(Msg::EnterTerminal)),
            _ => None,
        }
    }
}

/// A screen whose one tab is a running shell tab, attached to `script` run by `sh` on this
/// machine in place of a container: what the terminal does with keys and the mouse is the same
/// whatever runs in it.
fn running(scratch: &Scratch, script: &str) -> (WorkspaceScreen, TerminalSession) {
    let mut screen = one_workspace(scratch);
    open(&mut screen, Choice::Shell);
    let key = key(&screen, 0);
    let session =
        TerminalSession::spawn("/bin/sh".as_ref(), &["-c", script], Path::new("/")).expect("a terminal for the shell");
    let (_, tab) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)).expect("the tab is open");
    tab.attached(session.clone());
    (screen, session)
}

/// Waits until the program's screen, as the harness draws it, shows `text`.
fn wait_for(harness: &mut Harness<Keyed>, text: &str) {
    let started = std::time::Instant::now();
    while !harness.render().screen().contains(text) {
        assert!(started.elapsed() < Duration::from_secs(10), "{}", harness.screen());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A harness of [`Keyed`] with the terminal of the running tab focused.
fn keyed(screen: WorkspaceScreen) -> Harness<Keyed> {
    let mut harness = Harness::with_env(Keyed { screen, help: 0 }, env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    wait_for(&mut harness, "ready");
    for _ in 0..16 {
        if harness.is_focused("workspace-terminal") {
            break;
        }
        harness.press("tab");
    }
    assert!(harness.is_focused("workspace-terminal"), "{}", harness.screen());
    harness
}

#[test]
fn a_focused_harness_lets_the_key_list_and_the_panel_key_through_but_types_a_question_mark() {
    let scratch = Scratch::new("pass-through");
    // The program shows, in hex, the first four bytes it receives.
    let (screen, session) = running(&scratch, "stty raw -echo; printf 'ready '; head -c 4 | od -An -tx1");
    let mut harness = keyed(screen);
    assert!(harness.app().screen.panel().is_open(), "the panel starts open");

    harness.press("f1");
    assert_eq!(harness.app().help, 1, "f1 opens the key list from inside the harness");
    harness.press("alt+b");
    assert!(!harness.app().screen.panel().is_open(), "the panel's key closes it from inside the harness");
    assert!(harness.is_focused("workspace-terminal"), "and the harness keeps the keyboard");

    harness.press("?").type_text("abc");
    wait_for(&mut harness, "3f 61 62 63");
    assert_eq!(harness.app().help, 1, "`?` is typed into the program, not taken for the key list");
    session.kill();
}

#[test]
fn a_click_on_a_harness_that_asks_for_the_mouse_reaches_it() {
    let scratch = Scratch::new("mouse");
    // The program turns on mouse reporting in the SGR encoding and shows, in hex, the first
    // three bytes it receives: a report starts with ESC [ <.
    let script = "stty raw -echo; printf '\\033[?1000h\\033[?1006hready '; head -c 3 | od -An -tx1";
    let (screen, session) = running(&scratch, script);
    let mut harness = keyed(screen);
    let (x, y) = harness.find("ready").expect("the program's screen is drawn");
    harness.click(x + 1, y);
    wait_for(&mut harness, "1b 5b 3c");
    session.kill();
}

#[test]
fn ctrl_alt_space_takes_the_keyboard_between_the_harness_and_the_tabs() {
    let scratch = Scratch::new("leave-terminal");
    // The program shows, in hex, the first four bytes it receives: the chord must not be one.
    let (screen, session) = running(&scratch, "stty raw -echo; printf 'ready '; head -c 4 | od -An -tx1");
    let mut harness = keyed(screen);

    harness.press("ctrl+alt+space");
    assert!(harness.is_focused("workspace-tabs"), "the tab strip has the keyboard:\n{}", harness.screen());
    harness.press("ctrl+alt+space");
    assert!(harness.is_focused("workspace-terminal"), "the same key goes back into the harness");

    // However the keyboard left the harness, by Tab here, the key finds its way back.
    harness.press("shift+tab");
    assert!(!harness.is_focused("workspace-terminal"), "shift+tab leaves the harness");
    harness.press("ctrl+alt+space");
    assert!(harness.is_focused("workspace-terminal"), "back from wherever the keyboard was");

    harness.type_text("abcd");
    wait_for(&mut harness, "61 62 63 64");
    session.kill();
}

#[test]
fn a_running_harness_counts_as_an_agent_at_work_and_a_running_shell_does_not() {
    let scratch = Scratch::new("at-work");
    let mut screen = one_workspace(&scratch);
    let alive = || {
        TerminalSession::spawn("/bin/sh".as_ref(), &["-c", "sleep 30"], Path::new("/"))
            .expect("a terminal for the shell")
    };
    open(&mut screen, Choice::Shell);
    screen.attach(key(&screen, 0), alive());
    assert_eq!(super::agents_at_work(&screen), 0, "a shell is nobody's work");
    open(&mut screen, claude());
    screen.attach(key(&screen, 1), alive());
    assert_eq!(super::agents_at_work(&screen), 1, "the harness is at work");
}
