//! The project screen, checked without a container runtime anywhere near it.
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
use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::runtime::Harness;
use qframe::widgets::TerminalEvent;

use crate::engine::{Container, ContainerState, Engine, EngineKind, HostUser};
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::workspace::{ProjectFile, ProjectId, ProjectPaths};

use super::plan::{PROJECT_DIR, SHELL};
use super::{Msg, NewTab, OpenProject, PanelWidget, ProjectScreen, Tab, TabKey, TabKind, TabState};

/// A terminal wide enough for the rail, the tabs, the terminal and the panel.
const SIZE: (u16, u16) = (110, 32);

/// A binary that cannot be run, so every engine command fails before it starts anything.
const NO_ENGINE: &str = "/nonexistent/qcode-test-engine";

/// A folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-project-{name}-{stamp}"));
        fs::create_dir_all(path.join("Project").join("src")).expect("a project folder");
        fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        fs::write(path.join("Project").join("README.md"), "hello\n").expect("a file in the project");
        fs::write(path.join("Project").join("src").join("main.rs"), "fn main() {}\n").expect("a source file");
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
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The application under test: the project screen and nothing else.
struct Screen(ProjectScreen);

impl App for Screen {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        super::update(&mut self.0, message)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        super::view(&self.0, ui);
    }
}

fn env() -> Env {
    Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() }).expect("the built-in files load")
}

fn profile(name: &str, harness: HarnessKind) -> Profile {
    Profile {
        name: SafeName::parse(name).expect("a usable profile name"),
        harness,
        template: Template::Recommended,
        account: AccountKind::Subscription,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
    }
}

fn project(id: &str, name: &str, paths: ProjectPaths, profiles: Vec<Profile>) -> OpenProject {
    let file = ProjectFile::new(
        ProjectId::parse(id).expect("a usable project id"),
        name,
        Date::new(2026, 9, 17).expect("a day the calendar has"),
    );
    OpenProject::new(&file, paths, profiles)
}

fn engine() -> Engine {
    Engine::new(EngineKind::Podman, NO_ENGINE)
}

/// A screen with one project carrying one profile, and an engine whose binary cannot be run.
fn one_project(scratch: &Scratch) -> ProjectScreen {
    let projects =
        vec![project("firefly", "Firefly", scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)])];
    ProjectScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, projects)
}

/// Applies a message without letting its background work run, which is how the states that only
/// exist while work is in flight are looked at.
fn apply(screen: &mut ProjectScreen, message: Msg) {
    drop(super::update(screen, message));
}

/// A harness of the screen, with the work the screen asks for on entry already done: the project
/// folder read, and the engine asked for the containers.
fn harness(screen: ProjectScreen, width: u16, height: u16) -> Harness<Screen> {
    let has_project = screen.project().is_some();
    let mut harness = Harness::with_env(Screen(screen), env(), width, height);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    if has_project {
        harness.send(Msg::OpenProject(0));
    }
    harness.render();
    harness
}

/// The key of the tab at `index` of the open project.
fn key(screen: &ProjectScreen, index: usize) -> TabKey {
    screen.project().expect("a project is open").tabs()[index].key()
}

#[test]
fn a_tab_opens_inside_a_container_and_never_on_this_machine() {
    let scratch = Scratch::new("inside");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    apply(&mut screen, Msg::NewTab(NewTab::Profile(0)));

    let shell = screen.launch_command(key(&screen, 0)).expect("the shell tab has a command");
    let words: Vec<String> = shell.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert_eq!(shell.program, Path::new(NO_ENGINE), "a tab only ever starts the engine binary");
    assert_eq!(words, ["exec", "--interactive", "--tty", "qcode-firefly-base", SHELL[0], SHELL[1]]);

    let harness = screen.launch_command(key(&screen, 1)).expect("the profile tab has a command");
    let words: Vec<String> = harness.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert_eq!(harness.program, Path::new(NO_ENGINE));
    assert_eq!(
        words,
        ["exec", "--interactive", "--tty", "qcode-firefly-claude-sub", "claude", "--dangerously-skip-permissions"],
        "the harness is an argument of the engine, so it runs in the container"
    );
}

#[test]
fn the_container_a_tab_enters_carries_the_project_and_the_profiles_permissions() {
    let scratch = Scratch::new("create");
    let screen = one_project(&scratch);
    let open = screen.project().expect("a project is open");
    let engine = screen.engine().expect("an engine");
    let user = HostUser::Ids { uid: 1000, gid: 1000 };

    let base = open.plan(&TabKind::Shell).expect("the base container is planned");
    let words: Vec<String> =
        base.create(engine, user).args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert!(words.contains(&"qcode-firefly-base".to_owned()), "{words:?}");
    assert!(words.contains(&"qcode/base".to_owned()), "{words:?}");
    assert!(words.iter().any(|word| word.contains(&format!("Project:{PROJECT_DIR}:rw"))), "{words:?}");

    let profile = open.plan(&TabKind::Profile("claude-sub".to_owned())).expect("the profile container is planned");
    let words: Vec<String> =
        profile.create(engine, user).args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert!(words.contains(&"qcode/profile/claude-sub".to_owned()), "{words:?}");
    assert!(words.contains(&"--userns=keep-id".to_owned()), "podman maps the user its own way: {words:?}");
    assert!(words.iter().any(|word| word.contains("/Assets:/work/Assets:ro")), "read-only assets: {words:?}");
    assert!(words.iter().any(|word| word.starts_with("qcode-home-firefly-claude-sub:")), "{words:?}");
}

#[test]
fn a_screen_with_no_tab_says_what_a_tab_is_and_offers_one() {
    let scratch = Scratch::new("empty");
    let harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    let screen = harness.screen();
    assert!(screen.contains("No tab is open"), "{screen}");
    assert!(screen.contains("New tab"), "{screen}");
    assert!(screen.contains("Panel"), "the widget panel stands beside it:\n{screen}");
    assert!(screen.contains('F'), "the collapsed rail marks the project by its initial:\n{screen}");
}

#[test]
fn the_new_tab_chooser_offers_a_shell_and_every_profile() {
    let scratch = Scratch::new("picker");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.click_text("New tab").advance(Duration::from_millis(300));
    let text = harness.screen();
    assert!(text.contains("Shell"), "an empty terminal is the first choice:\n{text}");
    assert!(text.contains("claude-sub"), "and every profile follows:\n{text}");
    assert!(text.contains("Claude Code"), "each profile says which harness it runs:\n{text}");

    harness.click_text("claude-sub");
    let kinds: Vec<TabKind> =
        harness.app().0.project().expect("a project").tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [TabKind::Profile("claude-sub".to_owned())], "the chosen profile is what opened");
}

#[test]
fn a_hovered_project_names_itself_beside_the_collapsed_rail() {
    let first = Scratch::new("hover-a");
    let second = Scratch::new("hover-b");
    let projects = vec![
        project("firefly", "Firefly", first.paths(), Vec::new()),
        project("moth", "Moth", second.paths(), Vec::new()),
    ];
    let screen = ProjectScreen::new(Some(engine()), HostUser::ImageDefault, projects);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    assert!(!harness.screen().contains("Moth"), "a four-cell rail has no room for names");
    let (x, y) = harness.find("M").expect("the second project's initial is on screen");
    harness.hover(x, y);
    assert!(harness.screen().contains("Moth"), "the pointer brings the name card:\n{}", harness.screen());
}

#[test]
fn a_tab_waiting_for_its_container_says_so() {
    let scratch = Scratch::new("starting");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    assert!(screen.project().expect("a project").tabs()[0].state().is_starting());
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("Starting the container"), "{text}");
    assert!(text.contains("Shell"), "the tab is in the strip:\n{text}");
}

#[test]
fn a_container_that_stops_is_named_and_offered_a_restart() {
    let scratch = Scratch::new("stopped");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    let tab = key(&screen, 0);
    // The session ends, and the engine answers that the container under it is gone: that pair is
    // the case the design asks to be named, rather than left as "the shell exited".
    apply(&mut screen, Msg::Output(tab, 0, TerminalEvent::Exited(Some(0))));
    apply(&mut screen, Msg::Checked(tab, 0, false));
    assert_eq!(screen.project().expect("a project").tabs()[0].state(), &TabState::Stopped);

    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("The container stopped"), "{text}");
    assert!(text.contains("Start again"), "{text}");
    harness.click_text("Start again");
    // The engine of these tests cannot be run, so the restart gets as far as the engine and no
    // further; that it got there at all is what the button promises. A container that is gone
    // is made again from the base image, which is made sure of first, so the engine is met at
    // the base image's build.
    let state = harness.app().0.project().expect("a project").tabs()[0].state().clone();
    let TabState::Failed(failure) = state else { panic!("pressing it goes back to the engine: {state:?}") };
    assert!(failure.command.contains("build"), "it starts from the base image: {failure:?}");
    assert!(failure.command.contains("qcode/base"), "{failure:?}");
}

#[test]
fn an_engine_that_refuses_shows_its_own_words() {
    let scratch = Scratch::new("refused");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    // The engine binary of these tests cannot be run, so opening a tab really does fail here.
    harness.send(Msg::NewTab(NewTab::Shell));
    let text = harness.screen();
    assert!(text.contains("The engine refused"), "{text}");
    assert!(text.contains("Start again"), "{text}");
    let state = harness.app().0.project().expect("a project").tabs()[0].state().clone();
    let TabState::Failed(failure) = state else { panic!("the tab carries the engine's refusal") };
    assert!(failure.command.contains(NO_ENGINE), "the command it tried is kept: {failure:?}");
    assert!(!failure.output.is_empty(), "so is what the system said: {failure:?}");
}

#[test]
fn without_an_engine_nothing_pretends_to_open() {
    let scratch = Scratch::new("no-engine");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    let screen = ProjectScreen::new(None, HostUser::ImageDefault, projects);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("No container engine was found"), "{text}");
    harness.click_text("New tab");
    assert!(harness.app().0.project().expect("a project").tabs().is_empty(), "a disabled control opens nothing");
}

#[test]
fn tabs_close_and_move() {
    let scratch = Scratch::new("tabs");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    apply(&mut screen, Msg::NewTab(NewTab::Profile(0)));
    apply(&mut screen, Msg::MoveTab { from: 1, to: 0 });
    let kinds: Vec<TabKind> =
        screen.project().expect("a project").tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [TabKind::Profile("claude-sub".to_owned()), TabKind::Shell]);

    apply(&mut screen, Msg::CloseTab(0));
    let kinds: Vec<TabKind> =
        screen.project().expect("a project").tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [TabKind::Shell], "closing a tab leaves the others where they were");
}

#[test]
fn several_shells_are_told_apart() {
    let scratch = Scratch::new("shells");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("Shell 1"), "{text}");
    assert!(text.contains("Shell 2"), "{text}");
}

#[test]
fn the_rail_switches_between_projects() {
    let first = Scratch::new("rail-a");
    let second = Scratch::new("rail-b");
    let projects = vec![
        project("firefly", "Firefly", first.paths(), Vec::new()),
        project("moth", "Moth", second.paths(), Vec::new()),
    ];
    let mut screen = ProjectScreen::new(Some(engine()), HostUser::ImageDefault, projects);
    apply(&mut screen, Msg::OpenProject(1));
    assert_eq!(screen.project().expect("a project").name(), "Moth");
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains('M'), "the collapsed rail marks a project by its initial:\n{text}");
}

#[test]
fn the_file_tree_reads_the_project_folder_and_opens_a_folder() {
    let scratch = Scratch::new("files");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.send(Msg::OpenProject(0));
    let text = harness.screen();
    assert!(text.contains("README.md"), "{text}");
    assert!(text.contains("src"), "{text}");
    assert!(!text.contains("main.rs"), "a closed folder shows nothing of itself:\n{text}");

    harness.send(Msg::ExpandFile("src".to_owned(), true));
    let text = harness.screen();
    assert!(text.contains("main.rs"), "opening the folder reads it:\n{text}");
}

#[test]
fn an_unreadable_project_folder_says_why() {
    let scratch = Scratch::new("unreadable");
    let mut paths = scratch.paths();
    paths.project = paths.project.join("gone");
    let projects = vec![project("firefly", "Firefly", paths, Vec::new())];
    let screen = ProjectScreen::new(Some(engine()), HostUser::ImageDefault, projects);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.send(Msg::OpenProject(0));
    let text = harness.screen();
    assert!(text.contains("The project folder"), "the panel says which folder it is about:\n{text}");
    let reason = harness.app().0.project().expect("a project").files().error().unwrap_or_default().to_owned();
    assert!(!reason.is_empty(), "and keeps what the system said");
    assert!(text.contains("No such file"), "which is on screen too:\n{text}");
}

#[test]
fn widgets_are_added_taken_off_and_moved() {
    let scratch = Scratch::new("widgets");
    let mut screen = one_project(&scratch);
    assert_eq!(screen.panel().order(), PanelWidget::ALL);

    apply(&mut screen, Msg::ToggleShown(0));
    assert!(!screen.panel().carries(PanelWidget::Files), "the widget is off the panel");
    apply(&mut screen, Msg::ToggleShown(0));
    assert_eq!(screen.panel().order().last(), Some(&PanelWidget::Files), "it comes back at the end");

    apply(&mut screen, Msg::MoveWidget { from: 2, to: 0 });
    assert_eq!(screen.panel().order()[0], PanelWidget::Files);

    apply(&mut screen, Msg::ToggleWidget(0, false));
    assert!(!screen.panel().is_unfolded(0), "a folded widget stays folded");
}

#[test]
fn the_container_widget_lists_this_projects_containers_and_acts_on_one() {
    let scratch = Scratch::new("containers");
    let mut screen = one_project(&scratch);
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
fn a_narrow_screen_keeps_the_rail_the_tabs_and_the_terminal() {
    let scratch = Scratch::new("narrow");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    let harness = harness(screen, 46, 18);
    let text = harness.screen();
    // The framework keeps the terminal a quarter of the width whatever the panel asks for, so
    // the middle is narrow here and its label is cut rather than lost.
    assert!(text.contains("Startin"), "the middle keeps its place:\n{text}");
    assert!(text.contains("Shell"), "the tab strip keeps its place:\n{text}");
    assert!(text.contains("Panel"), "so does the panel:\n{text}");
    assert!(text.contains('F'), "the rail is still there:\n{text}");
    for line in text.lines() {
        assert!(line.chars().count() <= 46, "nothing runs off the screen:\n{text}");
    }
}

#[test]
fn a_closed_panel_gives_its_width_to_the_terminal() {
    let scratch = Scratch::new("panel-closed");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::TogglePanel(false));
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(!text.contains("Containers"), "the panel is away:\n{text}");
    assert!(text.contains("No tab is open"), "{text}");
}

#[test]
fn nothing_is_bracketed_lined_or_framed() {
    let scratch = Scratch::new("aesthetic");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Profile(0)));
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
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.set_glyph_mode(GlyphMode::Ascii).render();
    let text = harness.screen();
    assert!(text.is_ascii(), "{text}");
    assert!(text.contains("No tab is open"), "{text}");
}

#[test]
fn the_keyboard_reaches_the_rail_the_tabs_and_the_panel() {
    let scratch = Scratch::new("keys");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    apply(&mut screen, Msg::NewTab(NewTab::Profile(0)));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let (mut rail, mut tabs) = (false, false);
    for _ in 0..12 {
        harness.press("tab");
        rail |= harness.is_focused("project-rail");
        tabs |= harness.is_focused("project-tabs");
    }
    assert!(rail, "the keyboard reaches the rail:\n{}", harness.screen());
    assert!(tabs, "and the tabs:\n{}", harness.screen());
    for _ in 0..12 {
        if harness.is_focused("project-tabs") {
            break;
        }
        harness.press("tab");
    }
    harness.press("left");
    assert_eq!(harness.app().0.project().expect("a project").active_tab().map(Tab::key), Some(TabKey(0)));
}

#[test]
fn reduced_motion_keeps_the_screen_working() {
    let scratch = Scratch::new("motion");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    let text = harness.screen();
    assert!(text.contains("Starting the container"), "{text}");
}

#[test]
fn turkish_reads_as_turkish() {
    let scratch = Scratch::new("turkish");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.set_locale("tr").render();
    let text = harness.screen();
    for label in ["Açık sekme yok", "Yeni sekme", "Kapsayıcılar", "Dosyalar"] {
        assert!(text.contains(label), "`{label}` is missing:\n{text}");
    }
}
