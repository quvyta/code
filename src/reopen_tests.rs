//! Closing QCode and opening it again.
//!
//! A close and reopen here is the real thing rather than a message about one. A [`Machine`] is a
//! folder holding the three things QCode leaves on a disk — the settings file, the session file
//! and the workspace — and opening it builds an application over them. The application is then
//! dropped where it stands, with no shutdown of any kind, the way a machine that loses power
//! gives none, and a second application is built over the same folder. What the person finds is
//! what these tests read.
//!
//! Every choice is made from the control the person touches, so that a setting which is stored
//! but never offered, or offered but never stored, is caught here rather than by them.

use std::path::PathBuf;
use std::time::Duration;

use qframe::date::Date;
use qframe::icons::{GlyphMode, IconMode};
use qframe::runtime::Harness;
use qframe::storage::Settings;

use crate::backup::BackupEvery;
use crate::base::apps::Editor;
use crate::engine::EngineKind;
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::testing::{env, host};
use crate::ui::project::ProjectScreen;
use crate::ui::setup::gates::{EngineCheck, EngineProblem, Gates, LocationCheck, LocationProblem};
use crate::workspace::{
    Config, HostDirs, ProjectId, Session, SessionProject, SessionTab, SessionTabKind, Workspace, add_profile,
};
use crate::{Page, QCode};

/// A terminal with room for the settings list and the project screen alike.
const SIZE: (u16, u16) = (96, 40);

/// How long the work a click starts is given. Disk writes happen on a task thread, and the
/// answer comes back as a message; the wait only has to be finite, so it is generous.
const MOMENT: Duration = Duration::from_millis(250);

/// A machine whose gates all hold: it speaks a language, its engine works, its workspace folder
/// can be written.
fn settled() -> Gates {
    Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Usable }
}

/// One person's machine: the settings file, the session file and the workspace, each where QCode
/// puts it, all under a folder of this test's own.
struct Machine {
    root: PathBuf,
}

impl Machine {
    /// A machine with nothing on it yet, named after `what` and this process so that two tests
    /// never meet and neither touches the person running them.
    fn bare(what: &str) -> Self {
        let root = std::env::temp_dir().join(format!("qcode-reopen-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a folder for the machine");
        Self { root }
    }

    /// The same machine with the setup already through: the settings file says so and names the
    /// workspace, which is what a person who answered the wizard once leaves behind.
    fn set_up(what: &str) -> Self {
        let machine = Self::bare(what);
        let mut config = machine.config();
        config.set_setup_completed(true);
        config.set_workspace_path(&machine.workspace());
        config.set_language("en");
        config.save().expect("the settings file is written");
        Workspace::new(machine.workspace()).prepare().expect("the workspace can be made");
        machine
    }

    /// Where the family keeps its settings files on this machine.
    fn config_dir(&self) -> PathBuf {
        self.root.join("config").join("quvyta")
    }

    /// Where an earlier version would have left its settings, which is empty on every machine
    /// these tests describe.
    fn legacy_dir(&self) -> PathBuf {
        self.root.join("config").join("quvyta").join("code")
    }

    /// Where QCode notes what is open.
    fn session_file(&self) -> PathBuf {
        self.root.join("data").join("session.toml")
    }

    /// The workspace folder: the projects and the profiles.
    fn workspace(&self) -> PathBuf {
        self.root.join("Documents").join("Quvyta").join("Code")
    }

    /// The folders QCode works the machine's own places out from.
    fn dirs(&self) -> HostDirs {
        let documents = self.root.join("Documents");
        HostDirs { workspace: Some(self.workspace()), documents: Some(documents) }
    }

    /// The settings file as it stands on the disk right now.
    fn config(&self) -> Config {
        Config::load_in(&self.config_dir(), &self.legacy_dir()).value
    }

    /// The session file as it stands on the disk right now, or `None` where none was written.
    fn session(&self) -> Option<Session> {
        Session::load(&self.session_file()).value
    }

    /// Opens QCode on this machine, with `gates` as what the machine answers.
    ///
    /// This is the way the program itself opens: the settings file is read from the disk,
    /// [`QCode::entry`] turns the gates into the step the wizard opens on — or into no wizard at
    /// all — and the drawing wears what the file keeps.
    fn open(&self, gates: &Gates) -> Harness<QCode> {
        let config = self.config();
        let worn = config.settings().clone();
        let entry = QCode::entry(&config, gates);
        let app = QCode::new(config, self.dirs(), host(), gates, None, entry).with_session(Some(self.session_file()));
        let mut harness = Harness::with_env(app, env(), SIZE.0, SIZE.1);
        wear(&mut harness, &worn);
        harness.render();
        harness
    }

    /// Opens QCode on a machine whose gates all hold.
    fn reopen(&self) -> Harness<QCode> {
        self.open(&settled())
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Dresses `harness` in the theme, language, glyphs and motion `settings` keeps, which is what
/// the runtime does with the settings file before the first frame is drawn.
fn wear(harness: &mut Harness<QCode>, settings: &Settings) {
    // A terminal in a test says nothing about where it stands, so a machine that has not been
    // asked yet is read in English rather than in whatever the machine running the test speaks.
    harness.set_locale(&settings.language().unwrap_or_else(|| "en".to_owned()));
    if let Some(id) = settings.get::<String>(Settings::THEME) {
        harness.set_theme(&id);
    }
    let icons = settings.get::<String>(Settings::ICONS).and_then(|name| {
        IconMode::ALL.into_iter().find(|mode| mode.name() == name).and_then(|mode| match mode {
            IconMode::Nerd => Some(GlyphMode::Nerd),
            IconMode::Unicode => Some(GlyphMode::Unicode),
            IconMode::Ascii => Some(GlyphMode::Ascii),
            // Which glyphs "auto" means is the terminal's answer, not the file's.
            IconMode::Auto => None,
        })
    });
    harness.set_glyph_mode(icons.unwrap_or(GlyphMode::Unicode));
    harness.set_reduced_motion(settings.get::<bool>(Settings::REDUCED_MOTION).unwrap_or(false));
}

/// Opens the settings screen from the home menu.
fn to_settings(harness: &mut Harness<QCode>) {
    harness.click_text("Settings").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Settings, "{}", harness.screen());
}

/// Chooses `option` on the settings row `row`, the way a person does it.
///
/// A row whose choices all stand on it is chosen by clicking the choice. A row that keeps its
/// choices in a drop-down is opened by its own control first, and the option is then clicked in
/// the list that falls open.
fn choose(harness: &mut Harness<QCode>, row: &str, option: &str) {
    let at = harness.find(row).unwrap_or_else(|| panic!("`{row}` is on the settings screen:\n{}", harness.screen()));
    if let Some(spot) = harness.find(option)
        && spot.1 == at.1
    {
        harness.click(spot.0, spot.1).advance(MOMENT);
        return;
    }
    let arrow = arrow_on(harness, at.1).unwrap_or_else(|| panic!("`{row}` carries a drop-down:\n{}", harness.screen()));
    harness.click(arrow, at.1).advance(MOMENT);
    let spot =
        harness.find(option).unwrap_or_else(|| panic!("`{option}` is offered under `{row}`:\n{}", harness.screen()));
    harness.click(spot.0, spot.1).advance(MOMENT);
}

/// The column of the drop-down's arrow on the screen row `y`, which is the last cell the row
/// draws: the control sits at the right of its label, and the arrow ends it. The arrow itself is
/// a different glyph in each of the three glyph modes, so it is found by where it is rather than
/// by what it looks like.
fn arrow_on(harness: &Harness<QCode>, y: i32) -> Option<i32> {
    let line = harness.screen().lines().nth(usize::try_from(y).ok()?)?.to_owned();
    let column = line.trim_end().chars().count().checked_sub(1)?;
    i32::try_from(column).ok()
}

/// A profile as the profiles screen writes it into the workspace.
fn profile(name: &str, harness: HarnessKind, account: AccountKind) -> Profile {
    Profile {
        name: SafeName::parse(name).expect("the name is safe"),
        harness,
        template: Template::Recommended,
        account,
        assets: MountAccess::ReadWrite,
        network: NetworkMode::None,
    }
}

#[test]
fn the_setup_wizard_is_asked_once_and_never_again() {
    let machine = Machine::bare("wizard-once");
    let fresh = Gates { language: false, engine: EngineCheck::Working, location: LocationCheck::Usable };
    let mut harness = machine.open(&fresh);
    assert_eq!(harness.app().page(), Page::Setup, "a machine that was never set up asks:\n{}", harness.screen());
    assert!(harness.screen().contains("Pick the language"), "{}", harness.screen());

    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Finish").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Home, "the wizard is through:\n{}", harness.screen());
    drop(harness);

    let again = machine.reopen();
    assert_eq!(again.app().page(), Page::Home, "nothing is asked a second time:\n{}", again.screen());
    let screen = again.screen();
    for asked in ["Pick the language", "Container engine", "one workspace folder"] {
        assert!(!screen.contains(asked), "`{asked}` is asked again:\n{screen}");
    }
    let stored = machine.config();
    assert!(stored.setup_completed(), "the file says the wizard is through");
    assert_eq!(stored.engine_kind(), Some("podman"), "the engine it settled on");
    assert_eq!(stored.workspace_path(), Some(machine.workspace()), "and where it put the workspace");
}

#[test]
fn a_gate_that_falls_after_the_setup_does_not_drag_the_person_through_it_again() {
    // The engine stopped working between two starts. That is worth repairing and worth saying,
    // but the three questions were answered once and answering them again would change nothing.
    let machine = Machine::set_up("engine-gone");
    let broken = Gates {
        language: true,
        engine: EngineCheck::Broken(EngineProblem::NotInstalled),
        location: LocationCheck::Usable,
    };
    let harness = machine.open(&broken);
    assert_eq!(harness.app().page(), Page::Home, "the wizard is not asked again:\n{}", harness.screen());
    let screen = harness.screen();
    assert!(!screen.contains("Pick the language"), "{screen}");
    assert!(screen.contains("Repair"), "the one step that is needed is offered instead:\n{screen}");
}

#[test]
fn a_setup_that_was_through_but_left_no_workspace_asks_where_the_workspace_goes() {
    // The one thing that overrides the rule above: a settings file that has been through the
    // wizard and still names no workspace leaves nothing at all to open.
    let machine = Machine::bare("no-workspace");
    let mut config = machine.config();
    config.set_language("en");
    config.set_setup_completed(true);
    config.save().expect("the settings file is written");
    let gates =
        Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Broken(LocationProblem::Unset) };
    let harness = machine.open(&gates);
    assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
    let screen = harness.screen();
    assert!(screen.contains("one workspace folder"), "and on the step that places it:\n{screen}");
    assert!(!screen.contains("Pick the language"), "no step before it is asked again:\n{screen}");
}

#[test]
fn every_language_the_wizard_offers_is_the_one_qcode_speaks_when_it_opens_again() {
    // The wizard offers a language for every file QCode carries. One that the settings file will
    // not hold is chosen, written and gone by the next start: the person picks Japanese and comes
    // back to a machine speaking English.
    for code in Config::LANGUAGES {
        let machine = Machine::bare(&format!("wizard-language-{code}"));
        let fresh = Gates { language: false, engine: EngineCheck::Working, location: LocationCheck::Usable };
        let mut harness = machine.open(&fresh);
        let name = harness.env().i18n().translate(&format!("setup.language-{code}"), &[]);
        harness.click_text(&name).advance(MOMENT);
        // The screen takes the choice at once, so the way on is now worded in it.
        let next = harness.env().i18n().translate("quvyta.wizard.next", &[]);
        let finish = harness.env().i18n().translate("quvyta.wizard.finish", &[]);
        harness.click_text(&next).advance(MOMENT);
        harness.click_text(&next).advance(MOMENT);
        harness.click_text(&finish).advance(MOMENT);
        drop(harness);

        let again = machine.reopen();
        assert_eq!(
            again.env().i18n().active(),
            code,
            "the wizard was answered in {code} and QCode opened in {}",
            again.env().i18n().active()
        );
    }
}

#[test]
fn the_appearance_chosen_on_the_settings_screen_is_what_qcode_opens_in() {
    let machine = Machine::set_up("appearance");
    let mut harness = machine.reopen();
    to_settings(&mut harness);
    let worn = harness.env().theme().id().to_owned();
    let other = harness
        .env()
        .themes()
        .into_iter()
        .find(|(id, _)| *id != worn)
        .unwrap_or_else(|| panic!("QCode carries more than one theme"));

    choose(&mut harness, "Theme", &other.1);
    choose(&mut harness, "Icons", "ASCII");
    let motion = harness.find("Reduce motion").expect("the switch is on screen");
    harness.click(i32::from(SIZE.0) - 12, motion.1).advance(MOMENT);
    choose(&mut harness, "Language", "Türkçe");
    assert!(harness.env().reduced_motion(), "the switch moved:\n{}", harness.screen());
    drop(harness);

    let again = machine.reopen();
    assert_eq!(again.env().theme().id(), other.0, "the theme is the chosen one");
    assert!(again.env().reduced_motion(), "motion is still reduced");
    assert_eq!(again.env().i18n().active(), "tr", "and QCode speaks Turkish");
    // The glyph mode reaches the drawing rather than a field to read, so the screen is the
    // answer: the menu's icons are the ASCII column rather than the Unicode one.
    let screen = again.screen();
    assert!(screen.contains("x Çıkış"), "the menu draws its ASCII glyphs, in Turkish:\n{screen}");
    assert!(screen.contains("* Ayarlar"), "{screen}");
    let stored = machine.config();
    assert_eq!(stored.settings().icon_mode(), Some(IconMode::Ascii), "and the file keeps the mode itself");
}

#[test]
fn the_container_engine_chosen_is_the_one_qcode_holds_when_it_opens_again() {
    let machine = Machine::set_up("engine");
    let mut harness = machine.reopen();
    assert_eq!(harness.app().engine.kind(), EngineKind::Podman, "podman until another is chosen");
    to_settings(&mut harness);
    choose(&mut harness, "Container engine", "Docker");
    drop(harness);

    let again = machine.reopen();
    assert_eq!(again.app().engine.kind(), EngineKind::Docker, "the application reaches for Docker now");
    let mut again = again;
    to_settings(&mut again);
    assert!(again.screen().contains("Docker"), "and the screen says so:\n{}", again.screen());
}

#[test]
fn the_editor_and_the_backup_interval_reach_a_project_opened_after_qcode_starts_again() {
    let machine = Machine::set_up("apps-reach-projects");
    let workspace = Workspace::new(machine.workspace());
    workspace.create_project("Firefly", Date::today_utc()).expect("the workspace takes a project");
    let mut harness = machine.reopen();
    to_settings(&mut harness);
    choose(&mut harness, "Editor", "vim");
    choose(&mut harness, "Back up open projects", "1 hour");
    drop(harness);

    let mut again = machine.reopen();
    again.click_text("Projects").advance(MOMENT);
    again.click_text("Firefly").advance(MOMENT);
    assert_eq!(again.app().page(), Page::Project, "{}", again.screen());
    let screen = again.app().project.as_ref().expect("a project screen");
    assert_eq!(screen.editor(), Editor::Vim, "a file of this project opens in the chosen editor");
    assert_eq!(screen.backup_every(), BackupEvery::Hour, "and it is backed up as often as was asked");
}

#[test]
fn the_profiles_of_the_workspace_come_back_whole() {
    // Making a profile builds a container image, which needs an engine; what the profiles screen
    // leaves behind is the definition file, and that is what has to survive a close.
    let machine = Machine::set_up("profiles");
    let workspace = Workspace::new(machine.workspace());
    let written = [
        profile("claude-sub", HarnessKind::ClaudeCode, AccountKind::Subscription),
        profile("gemini-key", HarnessKind::GeminiCli, AccountKind::ApiKey),
    ];
    for profile in &written {
        workspace.write_profile(profile).expect("the workspace takes a profile");
    }

    let mut harness = machine.reopen();
    harness.click_text("Profiles").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
    let screen = harness.screen();
    for profile in &written {
        assert!(screen.contains(profile.name.as_str()), "{} is listed:\n{screen}", profile.name.as_str());
    }
    let read = Workspace::new(machine.workspace()).profiles();
    assert!(read.is_clean(), "{:?}", read.diagnostics);
    assert_eq!(read.value, written, "every field of every profile is the one that was written");
}

#[test]
fn the_projects_their_order_and_the_one_that_was_open_come_back() {
    let machine = Machine::set_up("projects");
    let workspace = Workspace::new(machine.workspace());
    for name in ["Alpha", "Beta", "Gamma"] {
        workspace.create_project(name, Date::today_utc()).expect("the workspace takes a project");
    }
    // A project carries the profiles chosen for it, which the project file is the one record of.
    workspace.write_profile(&profile("claude-sub", HarnessKind::ClaudeCode, AccountKind::Subscription)).expect("write");
    let paths = workspace.project_paths(&ProjectId::from_display_name("Beta").expect("a usable id"));
    add_profile(&paths, "claude-sub", Date::today_utc()).expect("the project takes the profile");

    let mut harness = machine.reopen();
    harness.click_text("Projects").advance(MOMENT);
    harness.click_text("Gamma").advance(MOMENT);
    harness.send(crate::Msg::Project(crate::ui::project::Msg::AddProject)).advance(MOMENT);
    harness.click_text("Beta").advance(MOMENT);
    assert_eq!(rail(&harness), ["Gamma", "Beta"], "{}", harness.screen());
    drop(harness);

    let again = machine.reopen();
    assert_eq!(again.app().page(), Page::Home, "QCode opens where it always does");
    assert!(again.screen().contains("Continue"), "and offers the way back:\n{}", again.screen());
    let mut again = again;
    again.click_text("Continue").advance(MOMENT);
    assert_eq!(again.app().page(), Page::Project, "{}", again.screen());
    assert_eq!(rail(&again), ["Gamma", "Beta"], "the rail keeps its order:\n{}", again.screen());
    let open = again.app().project.as_ref().and_then(ProjectScreen::project);
    assert_eq!(open.map(crate::OpenProject::name), Some("Beta"), "the project that was open is open again");
    assert!(open.is_some_and(|project| project.carries("claude-sub")), "the profile chosen for it is still its own");
}

#[test]
fn the_tabs_of_a_project_come_back_the_way_they_were_left() {
    // Opening a tab reaches into a container, which no test has; what the person finds again is
    // written in the session file, so that file is what a second QCode is given.
    let machine = Machine::set_up("tabs");
    let workspace = Workspace::new(machine.workspace());
    workspace.create_project("Firefly", Date::today_utc()).expect("the workspace takes a project");
    workspace
        .write_profile(&profile("antigravity", HarnessKind::AntigravityIde, AccountKind::InApp))
        .expect("the workspace takes a profile");
    let id = ProjectId::from_display_name("Firefly").expect("a usable id");
    let paths = workspace.project_paths(&id);
    add_profile(&paths, "antigravity", Date::today_utc()).expect("the project takes the profile");

    let left = Session {
        active: Some(id.clone()),
        projects: vec![SessionProject {
            id: id.clone(),
            active_tab: 2,
            tabs: vec![
                SessionTab { kind: SessionTabKind::Shell, conversation: None, opened: 10 },
                SessionTab { kind: SessionTabKind::Markdown("README.md".to_owned()), conversation: None, opened: 20 },
                SessionTab { kind: SessionTabKind::Desktop("antigravity".to_owned()), conversation: None, opened: 30 },
            ],
        }],
    };
    left.save(&machine.session_file()).expect("the session file is written");

    let mut harness = machine.reopen();
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Project, "{}", harness.screen());
    let screen = harness.screen();
    assert!(!screen.contains("Only part of the last session"), "nothing of it was lost:\n{screen}");
    let open = harness.app().project.as_ref().and_then(ProjectScreen::project).expect("the project is open");
    let kinds: Vec<crate::ui::project::TabKind> = open.tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(
        kinds,
        [
            crate::ui::project::TabKind::Shell,
            crate::ui::project::TabKind::Markdown("README.md".to_owned()),
            crate::ui::project::TabKind::Desktop("antigravity".to_owned()),
        ],
        "every tab is back, the window among them"
    );
    assert_eq!(open.active_tab().map(crate::ui::project::Tab::opened), Some(30), "the tab that was open is open");

    // Nothing on this machine is going to open that window: there is no container engine. A tab
    // that keeps saying it is opening one promises something nobody is doing and offers the
    // person nothing to do about it, so it says what every other tab says in the same place.
    let screen = harness.screen();
    assert!(!screen.contains("Opening the window"), "the window tab claims nothing:\n{screen}");
    assert!(screen.contains("This tab has not started yet"), "it says where it stands:\n{screen}");
}

#[test]
fn what_is_open_is_written_as_it_changes_so_a_drop_with_no_warning_loses_nothing() {
    // QCode is given no chance to save on the way out here: the application is dropped where it
    // stands, as it would be if the machine lost power. Anything written only on the way out
    // would be gone.
    let machine = Machine::set_up("hard-drop");
    let workspace = Workspace::new(machine.workspace());
    for name in ["Alpha", "Beta"] {
        workspace.create_project(name, Date::today_utc()).expect("the workspace takes a project");
    }
    let mut harness = machine.reopen();
    harness.click_text("Projects").advance(MOMENT);
    harness.click_text("Alpha").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Project, "{}", harness.screen());
    std::mem::drop(harness);

    let saved = machine.session().expect("what was open is on the disk already");
    let ids: Vec<String> = saved.projects.iter().map(|project| project.id.as_str().to_owned()).collect();
    assert_eq!(ids, ["alpha"], "the open project was written the moment it opened");
    assert_eq!(saved.active.map(|id| id.as_str().to_owned()), Some("alpha".to_owned()));
    let stored = machine.config();
    assert_eq!(
        stored.recent_projects().iter().map(|id| id.as_str().to_owned()).collect::<Vec<_>>(),
        ["alpha"],
        "and so was the project opened last"
    );
}

/// The names of the rail of the open project screen, in order.
fn rail(harness: &Harness<QCode>) -> Vec<String> {
    let screen = harness.app().project.as_ref().expect("a project screen");
    screen.projects().iter().map(|project| project.name().to_owned()).collect()
}
