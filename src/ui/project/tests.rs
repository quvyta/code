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
use crate::workspace::{ProjectFile, ProjectId, ProjectPaths, ProjectProfile};

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
        super::view(&self.0, ui, std::convert::identity, |_| {}, |_| {});
    }
}

/// The project screen inside an application that hands it a header and a footer of its own,
/// the way QCode does.
struct Framed(ProjectScreen);

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
        );
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

/// The file of a project `id` carrying the profiles named in `carried`.
fn file(id: &str, name: &str, carried: &[&str]) -> ProjectFile {
    let day = Date::new(2026, 9, 17).expect("a day the calendar has");
    let mut file = ProjectFile::new(ProjectId::parse(id).expect("a usable project id"), name, day);
    file.profiles = carried.iter().map(|name| ProjectProfile { name: (*name).to_owned(), added: Some(day) }).collect();
    file
}

/// A project that carries every one of `profiles`.
fn project(id: &str, name: &str, paths: ProjectPaths, profiles: Vec<Profile>) -> OpenProject {
    let carried: Vec<&str> = profiles.iter().map(|profile| profile.name.as_str()).collect();
    OpenProject::new(&file(id, name, &carried), paths, profiles)
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
fn a_profile_the_project_does_not_carry_is_offered_and_joins_it_when_opened() {
    // The profile was made after the project, so the project's file does not name it. It is
    // offered all the same, says that choosing it adds it, and choosing it writes the file.
    let scratch = Scratch::new("joins");
    let paths = scratch.paths();
    let written = file("firefly", "Firefly", &["claude-sub"]);
    fs::write(&paths.file, written.to_toml()).expect("the project's file");
    let profiles = vec![profile("opencode", HarnessKind::OpenCode), profile("claude-sub", HarnessKind::ClaudeCode)];
    let open = OpenProject::new(&written, paths.clone(), profiles);
    let order: Vec<&str> = open.profiles().iter().map(|profile| profile.name.as_str()).collect();
    assert_eq!(order, ["claude-sub", "opencode"], "the carried profile comes first");
    let screen = ProjectScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, vec![open]);

    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.click_text("New tab").advance(Duration::from_millis(300));
    let text = harness.screen();
    assert!(text.contains("opencode"), "a profile of the workspace is offered:\n{text}");
    assert!(text.contains("adds to project"), "and says what choosing it does:\n{text}");

    harness.click_text("opencode").advance(Duration::from_millis(300));
    let open = harness.app().0.project().expect("a project");
    let kinds: Vec<TabKind> = open.tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [TabKind::Profile("opencode".to_owned())], "the tab opened");
    assert!(open.carries("opencode"), "the screen knows the project carries it now");
    let on_disk = ProjectFile::parse("project.qcode", &fs::read_to_string(&paths.file).expect("the file is there"));
    let names: Vec<String> =
        on_disk.value.expect("still a project").profiles.into_iter().map(|profile| profile.name).collect();
    assert_eq!(names, ["claude-sub", "opencode"], "the file records it, after what it named before");
    assert!(paths.harness_profile("opencode").is_dir(), "with the profile's folder in the project");
}

#[test]
fn a_profile_that_could_not_be_recorded_still_opens_and_says_so() {
    // No project.qcode at the project's place: the container does not need it, so the tab opens,
    // and the person learns the record is missing.
    let scratch = Scratch::new("unrecorded");
    let open = OpenProject::new(
        &file("firefly", "Firefly", &[]),
        scratch.paths(),
        vec![profile("claude-sub", HarnessKind::ClaudeCode)],
    );
    let screen = ProjectScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, vec![open]);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.send(Msg::NewTab(NewTab::Profile(0))).advance(Duration::from_millis(300));
    let open = harness.app().0.project().expect("a project");
    assert_eq!(open.tabs().len(), 1, "the tab opened");
    assert!(!open.carries("claude-sub"));
    let text = harness.screen();
    assert!(text.contains("could not be recorded"), "{text}");
}

#[test]
fn with_no_profile_at_all_the_chooser_leads_to_the_profiles_screen() {
    let scratch = Scratch::new("no-profiles");
    let screen = ProjectScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![project("firefly", "Firefly", scratch.paths(), Vec::new())],
    );
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.click_text("New tab").advance(Duration::from_millis(300));
    let text = harness.screen();
    assert!(text.contains("Shell"), "the shell is still there:\n{text}");
    assert!(text.contains("New profile"), "and the way to a profile beside it:\n{text}");

    harness.click_text("New profile").advance(Duration::from_millis(300));
    assert!(harness.app().0.project().expect("a project").tabs().is_empty(), "no tab opened");
    assert!(!harness.app().0.picker, "the chooser closed for the profiles screen");
}

#[test]
fn profiles_read_again_reach_every_open_project() {
    let scratch = Scratch::new("reread");
    let mut screen = one_project(&scratch);
    apply(
        &mut screen,
        Msg::Profiles(vec![profile("opencode", HarnessKind::OpenCode), profile("claude-sub", HarnessKind::ClaudeCode)]),
    );
    let names: Vec<&str> =
        screen.project().expect("a project").profiles().iter().map(|profile| profile.name.as_str()).collect();
    assert_eq!(names, ["claude-sub", "opencode"], "the new one is offered, after the carried one");
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
    let harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    let (line, x) = panel_title_row(&harness);
    let after: String = line.chars().skip(x + "Panel".len()).collect();
    assert!(after.trim().is_empty(), "nothing stands beside the title:\n{}", harness.screen());
    assert!(!harness.screen().contains("Widgets"), "the old label is gone:\n{}", harness.screen());
}

#[test]
fn the_add_button_is_a_bare_plus_that_lists_only_the_missing_widgets() {
    let scratch = Scratch::new("add-widget");
    let mut screen = one_project(&scratch);
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
    assert!(text.contains("Project"), "the missing widget is offered:\n{text}");
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
    let mut screen = one_project(&scratch);
    // Folded, the widget shows only its title, and it can still be taken off.
    apply(&mut screen, Msg::ToggleWidget(1, false));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    let (x, y) = harness.find("Project").expect("the folded widget's title");
    harness.mouse(MouseKind::Down(MouseButton::Right), x, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x, y);
    let text = harness.screen();
    assert!(text.contains("Remove Project from the panel"), "{text}");
    harness.click_text("Remove Project from the panel");
    let panel = harness.app().0.panel();
    assert!(!panel.carries(PanelWidget::Info), "{}", harness.screen());
    assert!(panel.carries(PanelWidget::Files) && panel.carries(PanelWidget::Containers));
    assert!(harness.screen().contains('+'), "and it can be added again:\n{}", harness.screen());
}

#[test]
fn the_panel_runs_from_the_top_row_to_the_bottom_one() {
    let scratch = Scratch::new("full-height");
    let screen = one_project(&scratch);
    let mut harness = Harness::with_env(Framed(screen), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Msg::OpenProject(0)).render();
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
        if harness.is_focused("project-new-tab") {
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
fn a_narrow_screen_keeps_the_rail_the_terminal_and_the_panel() {
    let scratch = Scratch::new("narrow");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::NewTab(NewTab::Shell));
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
