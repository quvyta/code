//! Closing QCode and opening it again.
//!
//! A close and reopen here is the real thing rather than a message about one. A [`Machine`] is a
//! folder holding the three things QCode leaves on a disk — the settings file, the session file
//! and the store — and opening it builds an application over them. The application is then
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
use crate::store::{
    Config, HostDirs, Session, SessionTab, SessionTabKind, SessionWorkspace, Store, WorkspaceId, add_profile,
};
use crate::testing::{env, host};
use crate::ui::setup::gates::{EngineCheck, EngineProblem, Gates, LocationCheck, LocationProblem};
use crate::ui::workspace::WorkspaceScreen;
use crate::{Page, QCode};

/// A terminal with room for the settings list and the workspace screen alike.
const SIZE: (u16, u16) = (96, 40);

/// How long the work a click starts is given. Disk writes happen on a task thread, and the
/// answer comes back as a message; the wait only has to be finite, so it is generous.
const MOMENT: Duration = Duration::from_millis(250);

/// A machine whose gates all hold: it speaks a language, its engine works, its store folder
/// can be written.
fn settled() -> Gates {
    Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Usable }
}

/// One person's machine: the settings file, the session file and the store, each where QCode
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
    /// store, which is what a person who answered the wizard once leaves behind.
    fn set_up(what: &str) -> Self {
        let machine = Self::bare(what);
        let mut config = machine.config();
        config.set_setup_completed(true);
        config.set_folder_path(&machine.store());
        config.set_language("en");
        config.save().expect("the settings file is written");
        Store::new(machine.store()).prepare().expect("the store can be made");
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

    /// The store folder: the workspaces and the profiles.
    fn store(&self) -> PathBuf {
        self.root.join("Documents").join("Quvyta").join("Code")
    }

    /// The folders QCode works the machine's own places out from.
    fn dirs(&self) -> HostDirs {
        let documents = self.root.join("Documents");
        HostDirs { store: Some(self.store()), documents: Some(documents) }
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
        let app =
            QCode::new(config, self.dirs(), host(), gates, None, None, entry).with_session(Some(self.session_file()));
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

/// A profile as the profiles screen writes it into the store.
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
    for asked in ["Pick the language", "Container engine", "one store folder"] {
        assert!(!screen.contains(asked), "`{asked}` is asked again:\n{screen}");
    }
    let stored = machine.config();
    assert!(stored.setup_completed(), "the file says the wizard is through");
    assert_eq!(stored.engine_kind(), Some("podman"), "the engine it settled on");
    assert_eq!(stored.folder_path(), Some(machine.store()), "and where it put the store");
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
fn a_setup_that_was_through_but_left_no_store_asks_where_the_store_goes() {
    // The one thing that overrides the rule above: a settings file that has been through the
    // wizard and still names no store leaves nothing at all to open.
    let machine = Machine::bare("no-store");
    let mut config = machine.config();
    config.set_language("en");
    config.set_setup_completed(true);
    config.save().expect("the settings file is written");
    let gates =
        Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Broken(LocationProblem::Unset) };
    let harness = machine.open(&gates);
    assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
    let screen = harness.screen();
    assert!(screen.contains("live in one folder"), "and on the step that places it:\n{screen}");
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
fn the_editor_and_the_backup_interval_reach_a_workspace_opened_after_qcode_starts_again() {
    let machine = Machine::set_up("apps-reach-workspaces");
    let store = Store::new(machine.store());
    store.create_workspace("Firefly", Date::today_utc()).expect("the store takes a workspace");
    let mut harness = machine.reopen();
    to_settings(&mut harness);
    choose(&mut harness, "Editor", "vim");
    choose(&mut harness, "Back up open workspaces", "1 hour");
    drop(harness);

    let mut again = machine.reopen();
    again.click_text("Workspaces").advance(MOMENT);
    again.click_text("Firefly").advance(MOMENT);
    assert_eq!(again.app().page(), Page::Workspace, "{}", again.screen());
    let screen = again.app().workspace.as_ref().expect("a workspace screen");
    assert_eq!(screen.editor(), Editor::Vim, "a file of this workspace opens in the chosen editor");
    assert_eq!(screen.backup_every(), BackupEvery::Hour, "and it is backed up as often as was asked");
}

#[test]
fn the_profiles_of_the_store_come_back_whole() {
    // Making a profile builds a container image, which needs an engine; what the profiles screen
    // leaves behind is the definition file, and that is what has to survive a close.
    let machine = Machine::set_up("profiles");
    let store = Store::new(machine.store());
    let written = [
        profile("claude-sub", HarnessKind::ClaudeCode, AccountKind::Subscription),
        profile("gemini-key", HarnessKind::GeminiCli, AccountKind::ApiKey),
    ];
    for profile in &written {
        store.write_profile(profile).expect("the store takes a profile");
    }

    let mut harness = machine.reopen();
    harness.click_text("Profiles").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
    let screen = harness.screen();
    for profile in &written {
        assert!(screen.contains(profile.name.as_str()), "{} is listed:\n{screen}", profile.name.as_str());
    }
    let read = Store::new(machine.store()).profiles();
    assert!(read.is_clean(), "{:?}", read.diagnostics);
    assert_eq!(read.value, written, "every field of every profile is the one that was written");
}

#[test]
fn the_workspaces_their_order_and_the_one_that_was_open_come_back() {
    let machine = Machine::set_up("workspaces");
    let store = Store::new(machine.store());
    for name in ["Alpha", "Beta", "Gamma"] {
        store.create_workspace(name, Date::today_utc()).expect("the store takes a workspace");
    }
    // A workspace carries the profiles chosen for it, which the workspace file is the one record of.
    store.write_profile(&profile("claude-sub", HarnessKind::ClaudeCode, AccountKind::Subscription)).expect("write");
    let paths = store.workspace_paths(&WorkspaceId::from_display_name("Beta").expect("a usable id"));
    add_profile(&paths, "claude-sub", Date::today_utc()).expect("the workspace takes the profile");

    let mut harness = machine.reopen();
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Gamma").advance(MOMENT);
    harness.send(crate::Msg::Workspace(crate::ui::workspace::Msg::AddWorkspace)).advance(MOMENT);
    harness.click_text("Beta").advance(MOMENT);
    assert_eq!(rail(&harness), ["Gamma", "Beta"], "{}", harness.screen());
    drop(harness);

    let again = machine.reopen();
    assert_eq!(again.app().page(), Page::Home, "QCode opens where it always does");
    assert!(again.screen().contains("Continue"), "and offers the way back:\n{}", again.screen());
    let mut again = again;
    again.click_text("Continue").advance(MOMENT);
    assert_eq!(again.app().page(), Page::Workspace, "{}", again.screen());
    assert_eq!(rail(&again), ["Gamma", "Beta"], "the rail keeps its order:\n{}", again.screen());
    let open = again.app().workspace.as_ref().and_then(WorkspaceScreen::workspace);
    assert_eq!(open.map(crate::OpenWorkspace::name), Some("Beta"), "the workspace that was open is open again");
    assert!(
        open.is_some_and(|workspace| workspace.carries("claude-sub")),
        "the profile chosen for it is still its own"
    );
}

#[test]
fn the_tabs_of_a_workspace_come_back_the_way_they_were_left() {
    // Opening a tab reaches into a container, which no test has; what the person finds again is
    // written in the session file, so that file is what a second QCode is given.
    let machine = Machine::set_up("tabs");
    let store = Store::new(machine.store());
    store.create_workspace("Firefly", Date::today_utc()).expect("the store takes a workspace");
    store
        .write_profile(&profile("antigravity", HarnessKind::AntigravityIde, AccountKind::InApp))
        .expect("the store takes a profile");
    let id = WorkspaceId::from_display_name("Firefly").expect("a usable id");
    let paths = store.workspace_paths(&id);
    add_profile(&paths, "antigravity", Date::today_utc()).expect("the workspace takes the profile");

    let left = Session {
        active: Some(id.clone()),
        workspaces: vec![SessionWorkspace {
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
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    let screen = harness.screen();
    assert!(!screen.contains("Only part of the last session"), "nothing of it was lost:\n{screen}");
    let open = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).expect("the workspace is open");
    let kinds: Vec<crate::ui::workspace::TabKind> = open.tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(
        kinds,
        [
            crate::ui::workspace::TabKind::Shell,
            crate::ui::workspace::TabKind::Markdown("README.md".to_owned()),
            crate::ui::workspace::TabKind::Desktop("antigravity".to_owned()),
        ],
        "every tab is back, the window among them"
    );
    assert_eq!(open.active_tab().map(crate::ui::workspace::Tab::opened), Some(30), "the tab that was open is open");

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
    let store = Store::new(machine.store());
    for name in ["Alpha", "Beta"] {
        store.create_workspace(name, Date::today_utc()).expect("the store takes a workspace");
    }
    let mut harness = machine.reopen();
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Alpha").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    std::mem::drop(harness);

    let saved = machine.session().expect("what was open is on the disk already");
    let ids: Vec<String> = saved.workspaces.iter().map(|workspace| workspace.id.as_str().to_owned()).collect();
    assert_eq!(ids, ["alpha"], "the open workspace was written the moment it opened");
    assert_eq!(saved.active.map(|id| id.as_str().to_owned()), Some("alpha".to_owned()));
    let stored = machine.config();
    assert_eq!(
        stored.recent_workspaces().iter().map(|id| id.as_str().to_owned()).collect::<Vec<_>>(),
        ["alpha"],
        "and so was the workspace opened last"
    );
}

/// A machine as a QCode of before wrote it, in the names it used then: the settings file names
/// the store under `workspace.path` and the list of the recently opened under `projects.recent`,
/// the store keeps its workspaces in `Projects/`, each with a `project.qcode`, the person's code
/// in `Project/` and its backup history in `Backup/Project.git`, the profile's file says
/// `mounts.project`, and the session file's entries are `[[project]]`.
///
/// Written straight to the disk rather than through today's code, because today's code cannot
/// write a single one of these names any more.
fn machine_of_before(what: &str) -> Machine {
    let machine = Machine::bare(what);
    let store = machine.store();
    std::fs::create_dir_all(machine.config_dir()).expect("a folder for the settings");
    std::fs::write(
        machine.config_dir().join("code.conf"),
        format!(
            "language = \"en\"\n\n[setup]\ncompleted = true\n\n[workspace]\npath = \"{}\"\n\n\
             [projects]\nrecent = [\"firefly\"]\n",
            store.display()
        ),
    )
    .expect("the settings file is written");

    let workspace = store.join("Projects").join("firefly");
    for dir in [
        workspace.join("Project").join("src"),
        workspace.join("Assets"),
        workspace.join("Containers").join("Harness").join("claude-sub"),
        workspace.join("Backup").join("Project.git"),
        store.join("Profiles"),
    ] {
        std::fs::create_dir_all(&dir).expect("a folder of the store of before");
    }
    std::fs::write(workspace.join("Project").join("README.md"), "the person's own file\n").expect("a file of theirs");
    std::fs::write(workspace.join("Backup").join("Project.git").join("HEAD"), "ref: refs/heads/main\n")
        .expect("a backup history of theirs");
    std::fs::write(
        workspace.join("project.qcode"),
        "id = \"firefly\"\nname = \"Firefly\"\ncreated = \"2026-09-17\"\n\n\
         [[profile]]\nname = \"claude-sub\"\nadded = \"2026-09-17\"\n",
    )
    .expect("the workspace file is written");
    std::fs::write(
        store.join("Profiles").join("claude-sub.toml"),
        "name = \"claude-sub\"\nharness = \"claude-code\"\ntemplate = \"recommended\"\n\
         account = \"subscription\"\nimage = \"qcode/profile/claude-sub\"\n\n\
         [mounts]\nproject = \"rw\"\nassets = \"ro\"\n\n[network]\nmode = \"full\"\n",
    )
    .expect("the profile is written");

    std::fs::create_dir_all(machine.session_file().parent().expect("a folder")).expect("a folder for the session");
    std::fs::write(
        machine.session_file(),
        "active = \"firefly\"\n\n[[project]]\nid = \"firefly\"\nactive-tab = 0\n\n\
         [[project.tab]]\nkind = \"markdown\"\nfile = \"README.md\"\nopened = 10\n",
    )
    .expect("the session file is written");
    machine
}

/// Opening QCode over a machine of before loses nothing: the person finds their workspace, its
/// file, its profile and the tab they left open, and every name on the disk is the one of today.
#[test]
fn a_machine_written_before_workspaces_had_their_name_keeps_everything() {
    let machine = machine_of_before("before");
    let mut harness = machine.reopen();

    // What the person meets is their own workspace, not an empty one.
    assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert_eq!(rail(&harness), ["Firefly"], "{}", harness.screen());
    let open = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).expect("the workspace is open");
    assert!(open.carries("claude-sub"), "the profile it was given is still its own");
    let kinds: Vec<crate::ui::workspace::TabKind> = open.tabs().iter().map(|tab| tab.kind().clone()).collect();
    assert_eq!(kinds, [crate::ui::workspace::TabKind::Markdown("README.md".to_owned())], "the tab they left is back");
    let screen = harness.screen();
    assert!(screen.contains("README.md"), "their file is in the tree:\n{screen}");
    assert!(!screen.contains("Only part of the last session"), "nothing of the session was lost:\n{screen}");

    // And on the disk every name is today's, with the person's bytes inside it.
    let workspace = machine.store().join("Workspaces").join("firefly");
    assert!(workspace.join("workspace.qcode").is_file(), "the workspace file has its name of today");
    assert_eq!(
        std::fs::read_to_string(workspace.join("Work").join("README.md")).expect("their file moved with the folder"),
        "the person's own file\n"
    );
    assert!(workspace.join("Work").join("src").is_dir(), "and so did what was under it");
    assert!(workspace.join("Backup").join("Code.git").join("HEAD").is_file(), "the backup history moved too");
    assert!(!machine.store().join("Projects").exists(), "nothing of the old names is left");
    assert!(!workspace.join("project.qcode").exists());
    assert!(!workspace.join("Project").exists());
    assert!(!workspace.join("Code").exists(), "nor of the name the folder had in between");
    assert!(!workspace.join("Backup").join("Project.git").exists());

    // The settings keep the place they chose and the list they built, under the keys of today.
    let stored = machine.config();
    assert_eq!(stored.folder_path(), Some(machine.store()), "the store is still where they put it");
    assert_eq!(
        stored.recent_workspaces().iter().map(|id| id.as_str().to_owned()).collect::<Vec<_>>(),
        ["firefly"],
        "and the workspace they opened last is still on the list"
    );
    let text = std::fs::read_to_string(machine.config_dir().join("code.conf")).expect("the settings file is there");
    assert!(!text.contains("[workspace]") && !text.contains("[projects]"), "written back under today's keys:\n{text}");

    // A session brought back is not a session changed, so the file still reads as it was written;
    // the first thing the person changes writes it again, under today's name.
    harness.send(crate::Msg::Workspace(crate::ui::workspace::Msg::NewTab)).advance(MOMENT);
    std::mem::drop(harness);
    let text = std::fs::read_to_string(machine.session_file()).expect("the session file is there");
    assert!(text.contains("[[workspace]]") && !text.contains("[[project]]"), "{text}");
}

/// A machine of the QCode that had already named its workspaces but still kept the person's own
/// files in `Code/`, which is the folder that is called `Work/` today.
///
/// Written straight to the disk, for the same reason as [`machine_of_before`]: today's code
/// cannot make a workspace with that folder any more.
fn machine_of_the_code_folder(what: &str) -> Machine {
    let machine = Machine::bare(what);
    let store = machine.store();
    std::fs::create_dir_all(machine.config_dir()).expect("a folder for the settings");
    std::fs::write(
        machine.config_dir().join("code.conf"),
        format!(
            "language = \"en\"\n\n[setup]\ncompleted = true\n\n[folder]\npath = \"{}\"\n\n\
             [workspaces]\nrecent = [\"firefly\"]\n",
            store.display()
        ),
    )
    .expect("the settings file is written");

    let workspace = store.join("Workspaces").join("firefly");
    for dir in [
        workspace.join("Code").join("src"),
        workspace.join("Assets"),
        workspace.join("Containers").join("Harness").join("claude-sub"),
        store.join("Profiles"),
    ] {
        std::fs::create_dir_all(&dir).expect("a folder of the store of before");
    }
    std::fs::write(workspace.join("Code").join("README.md"), "the person's own file\n").expect("a file of theirs");
    std::fs::write(
        workspace.join("workspace.qcode"),
        "id = \"firefly\"\nname = \"Firefly\"\ncreated = \"2026-09-17\"\n\n\
         [[profile]]\nname = \"claude-sub\"\nadded = \"2026-09-17\"\n",
    )
    .expect("the workspace file is written");
    std::fs::write(
        store.join("Profiles").join("claude-sub.toml"),
        "name = \"claude-sub\"\nharness = \"claude-code\"\ntemplate = \"recommended\"\n\
         account = \"subscription\"\nimage = \"qcode/profile/claude-sub\"\n\n\
         [mounts]\ncode = \"rw\"\nassets = \"ro\"\n\n[network]\nmode = \"full\"\n",
    )
    .expect("the profile is written");

    std::fs::create_dir_all(machine.session_file().parent().expect("a folder")).expect("a folder for the session");
    std::fs::write(
        machine.session_file(),
        "active = \"firefly\"\n\n[[workspace]]\nid = \"firefly\"\nactive-tab = 0\n\n\
         [[workspace.tab]]\nkind = \"markdown\"\nfile = \"README.md\"\nopened = 10\n",
    )
    .expect("the session file is written");
    machine
}

/// A disk whose workspaces keep the person's files in `Code/` is carried the rest of the way:
/// the folder is `Work/` afterwards, with their bytes in it, and what they had open comes back.
///
/// The container mounts the folder on `/work`, and the person reads the same word at home; the
/// rename is only worth anything if nobody's work is left behind by it.
#[test]
fn a_machine_that_kept_the_code_folder_finds_it_under_its_name_of_today() {
    let machine = machine_of_the_code_folder("code-folder");
    let mut harness = machine.reopen();

    assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert_eq!(rail(&harness), ["Firefly"], "{}", harness.screen());
    let screen = harness.screen();
    assert!(screen.contains("README.md"), "their file is in the tree:\n{screen}");

    let workspace = machine.store().join("Workspaces").join("firefly");
    assert_eq!(
        std::fs::read_to_string(workspace.join("Work").join("README.md")).expect("their file moved with the folder"),
        "the person's own file\n"
    );
    assert!(workspace.join("Work").join("src").is_dir(), "and so did what was under it");
    assert!(!workspace.join("Code").exists(), "the folder of before is not left beside it");
}

/// The names of the rail of the open workspace screen, in order.
fn rail(harness: &Harness<QCode>) -> Vec<String> {
    let screen = harness.app().workspace.as_ref().expect("a workspace screen");
    screen.workspaces().iter().map(|workspace| workspace.name().to_owned()).collect()
}
