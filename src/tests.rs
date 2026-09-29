//! Moving through the application: where it opens, how each screen is reached and left, and
//! what the repair strip runs. Nothing here needs a container runtime or a settings file.

use std::time::Duration;

use super::testing::{app, app_with_absent_engine, config, harness, scratch, settled};
use super::{Gates, Msg, Page, SetupStep, WorkspaceId, WorkspaceScreen};
use crate::profile::SafeName;
use crate::store::Config;
use crate::ui::settings::Request;
use crate::ui::setup::gates::{EngineCheck, EngineProblem, LocationCheck};
use crate::ui::workspace::{OpenWorkspace, Tab, TabState};

mod focus;
mod opening;
mod rail;
mod requests;
mod sessions;
mod update_notice;

/// A terminal with room for the logo, the menu and the widest screen behind them.
const SIZE: (u16, u16) = (96, 30);

/// How long a click is given to be answered, for the screens that answer in a command.
const MOMENT: Duration = Duration::from_millis(10);

/// Gates of a machine whose engine is gone and whose store is in place.
fn without_engine() -> Gates {
    Gates { language: true, engine: EngineCheck::Broken(EngineProblem::NotInstalled), location: LocationCheck::Usable }
}

/// Where the setup wizard sits on `screen`: the rows above it, the rows between it and the
/// application's own footer, and the columns left and right of it.
///
/// The wizard runs from its row of steps down to its row of buttons; the footer under it
/// belongs to the application and is not part of what is being centred.
fn placed(screen: &str) -> (usize, usize, usize, usize) {
    let rows: Vec<&str> = screen.lines().collect();
    let row_with =
        |text: &str| rows.iter().position(|line| line.contains(text)).unwrap_or_else(|| panic!("{text}:\n{screen}"));
    let top = rows.iter().position(|line| !line.trim().is_empty()).expect("something is drawn");
    let buttons = row_with("Next");
    let footer = rows.iter().rposition(|line| !line.trim().is_empty()).expect("something is drawn");
    let body = &rows[top..=buttons];
    let left = body.iter().filter(|line| !line.trim().is_empty());
    let right = left.clone();
    (
        top,
        footer.saturating_sub(buttons + 1),
        left.map(|line| line.len() - line.trim_start().len()).min().unwrap_or(0),
        right.map(|line| line.trim_end().chars().count()).max().unwrap_or(0),
    )
}

#[test]
fn the_wizard_stands_in_the_middle_of_a_terminal_with_room_to_spare() {
    // The whole screen this time, not one widget: what the person sees is the wizard in the
    // middle of the terminal, not pinned into its top left corner.
    let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
    let screen = harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), 120, 44);
    let screen = screen.screen();
    let (top, below, left, _) = placed(&screen);
    assert!(top.abs_diff(below) <= 1, "as much room above as below: {top} and {below}\n{screen}");
    assert!(top > 2, "the wizard is not at the top of a terminal this tall: {top}\n{screen}");
    assert!(left > 2, "the wizard is not at the left edge of a terminal this wide: {left}\n{screen}");
    // Twenty more columns and the wizard moves ten to the right: half of what is added goes
    // to each side, which is centring and nothing else, whatever the wizard's own width is.
    let wide = harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), 140, 44);
    let wide = wide.screen();
    assert_eq!(placed(&wide).2, left + 10, "{wide}");
}

#[test]
fn a_terminal_with_nothing_to_spare_keeps_the_wizard_at_the_top() {
    // Centring is room that is there; a short terminal has none, and losing the first rows
    // of the wizard to a gap would be worse than a wizard that starts at the top.
    let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
    let short = harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), 88, 20);
    let screen = short.screen();
    assert_eq!(placed(&screen).0, 0, "{screen}");
    assert!(screen.contains("Next"), "the way on is still on the screen:\n{screen}");
}

/// Two stand-in engines that answer everything and hold nothing, found by the switch page in
/// place of the machine's own: the page is about the application moving between them, and
/// the machine running the test may have neither.
fn stand_in_engines(name: &str) -> crate::ui::switch::Finder {
    let folder = scratch(&format!("switch-{name}"));
    std::fs::create_dir_all(&folder).expect("a folder");
    for engine in ["podman", "docker"] {
        let path = folder.join(engine);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("a stand-in engine");
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
    }
    crate::ui::switch::Finder::new(move |kind| Some(crate::engine::Engine::new(kind, folder.join(kind.name()))))
}

#[test]
fn choosing_the_other_engine_in_the_settings_opens_what_the_move_takes_once_it_answers() {
    let store = scratch("switch-settings-store");
    let app = app(config(&store, &[]), &settled(), None).with_finder(stand_in_engines("settings"));
    let mut harness = harness(app, 110, 44);
    harness.click_text("Settings").render();
    assert_eq!(harness.app().page(), Page::Settings, "{}", harness.screen());
    // The engine's own choice, the way the person makes it: the list, then the other engine.
    harness.click_text("Podman").render();
    harness.click_text("Docker").render();
    assert_eq!(harness.app().config.engine_kind(), Some("docker"), "{}", harness.screen());
    // What looking for Docker answers on a machine that has it; the machine running this test
    // may not, so the answer is handed over rather than asked of it.
    let docker = crate::engine::Engine::new(crate::engine::EngineKind::Docker, "/stand-in/docker");
    harness.send(Msg::Looked(crate::engine::EngineKind::Docker, Ok(Box::new(docker)))).render();
    assert_eq!(harness.app().page(), Page::Switch, "{}", harness.screen());
    assert!(harness.screen().contains("Moving from Podman to Docker"), "{}", harness.screen());
    harness.click_text("Done").render();
    assert_eq!(harness.app().page(), Page::Settings, "Done goes back where the change was made");
}

#[test]
fn a_saved_engine_that_does_not_answer_beside_one_that_does_opens_on_the_same_offer() {
    let store = scratch("switch-start-store");
    let app = app(config(&store, &[]), &without_engine(), None)
        .with_finder(stand_in_engines("start"))
        .with_switch_offer(Some((crate::engine::EngineKind::Podman, crate::engine::EngineKind::Docker)));
    let mut harness = harness(app, 110, 44);
    let screen = harness.screen();
    assert_eq!(harness.app().page(), Page::Switch, "{screen}");
    assert!(screen.contains("Podman does not answer, and Docker does."), "{screen}");
    assert_eq!(harness.app().config.engine_kind(), Some("podman"), "nothing is changed by itself");
    harness.click_text("Use Docker").render();
    assert_eq!(harness.app().config.engine_kind(), Some("docker"));
    assert!(!harness.screen().contains("Use Docker"), "the offer was taken:\n{}", harness.screen());
    assert!(harness.screen().contains("Moving from Podman to Docker"), "the rest of the page stays");
}

/// The mark the chosen row of a list of choices carries, read off `screen` as the one
/// character that stands before `name` and before nothing else in `names`.
///
/// Read rather than named, so the check holds whatever glyph the theme draws the mark with.
fn marked(screen: &str, names: &[&str]) -> Vec<String> {
    let mark = |name: &str| {
        let line = screen.lines().find(|line| line.contains(name)).unwrap_or_else(|| panic!("{name}:\n{screen}"));
        line.trim_start().chars().next().unwrap_or(' ').to_string()
    };
    let marks: Vec<String> = names.iter().map(|name| mark(name)).collect();
    let chosen = |index: usize| marks.iter().enumerate().all(|(other, m)| (other == index) == (*m == marks[index]));
    (0..marks.len()).filter(|index| chosen(*index)).map(|index| names[index].to_owned()).collect()
}

/// The language step as the person sees it on a machine set to `LANG`, printed by a child
/// process, since the language of the whole application is read from the environment it was
/// started in and a test process shares one environment between its threads.
#[cfg(unix)]
#[test]
fn the_language_that_looks_chosen_is_the_one_the_wizard_is_speaking() {
    const PRINT: &str = "QCODE_TEST_PRINT_LANGUAGE_STEP";
    if std::env::var_os(PRINT).is_some() {
        let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
        let app = super::QCode::new(
            Config::parse_str("code.conf", ""),
            super::testing::dirs(),
            super::testing::host(),
            &gates,
            None,
            crate::ui::setup::languages::system_here(),
            Some(SetupStep::Language),
        );
        let mut harness = qframe::runtime::Harness::with_env(app, super::testing::env(), SIZE.0, SIZE.1);
        // No locale is set here on purpose: the wizard has to agree with the language the
        // machine itself hands the application.
        harness.set_glyph_mode(qframe::icons::GlyphMode::Unicode).set_reduced_motion(true);
        harness.render();
        println!("{}", harness.screen());
        return;
    }
    let turkish = "tr_TR.UTF-8".as_ref();
    let printed = super::testing::in_child(
        "tests::the_language_that_looks_chosen_is_the_one_the_wizard_is_speaking",
        &[(PRINT, "1".as_ref()), ("LANG", turkish), ("LC_ALL", turkish), ("LC_MESSAGES", turkish)],
    );
    assert!(printed.contains("Çince (Basitleştirilmiş)"), "the screen is drawn in Turkish:\n{printed}");
    let names =
        ["Türkçe", "Almanca", "Çince", "Fransızca", "İngilizce", "İspanyolca", "Japonca", "Portekizce", "Rusça"];
    assert_eq!(
        marked(&printed, &names),
        ["Türkçe"],
        "the row that carries the mark is the language on screen:\n{printed}"
    );
    let rows: Vec<&str> =
        printed.lines().filter_map(|line| names.iter().find(|name| line.contains(**name)).copied()).collect();
    assert_eq!(rows, names, "the machine's own language first, the rest alphabetical:\n{printed}");
}

/// The last row of the screen.
fn last_row(harness: &qframe::runtime::Harness<super::QCode>) -> String {
    harness.screen().lines().last().unwrap_or_default().to_owned()
}

/// Clicks the way back where it stands, at the foot of the screen: the lowest Back on screen,
/// since a screen's own rows may say the word too ("Back up open workspaces").
fn click_back(harness: &mut qframe::runtime::Harness<super::QCode>) {
    let screen = harness.screen();
    let (y, x) = screen
        .lines()
        .enumerate()
        .filter_map(|(y, row)| row.find("Back").map(|at| (y, row[..at].chars().count())))
        .last()
        .unwrap_or_else(|| panic!("no way back on screen:\n{screen}"));
    harness.click(i32::try_from(x).expect("on screen"), i32::try_from(y).expect("on screen"));
}

/// Presses one of the ways out at the foot of the workspace rail: the way back on the last
/// row, the settings above it and the list of keys above that.
///
/// By where the person's finger goes, not by the word on it: these are one cell wide and the
/// whole point of them standing there is that they cost the middle of the screen no row.
fn rail_foot(harness: &mut qframe::runtime::Harness<super::QCode>, up: i32) {
    harness.click(1, i32::from(SIZE.1) - 1 - up).advance(MOMENT);
}

/// The way back, at the very bottom of the workspace rail.
fn rail_back(harness: &mut qframe::runtime::Harness<super::QCode>) {
    rail_foot(harness, 0);
}

/// The settings, above the empty row over the way back.
fn rail_settings(harness: &mut qframe::runtime::Harness<super::QCode>) {
    rail_foot(harness, 2);
}

/// The list of keys, right above the settings.
fn rail_keys(harness: &mut qframe::runtime::Harness<super::QCode>) {
    rail_foot(harness, 3);
}

/// Opens the workspace `Firefly` of a fresh store at `root`.
fn open_workspace(root: &std::path::Path) -> qframe::runtime::Harness<super::QCode> {
    let _ = std::fs::remove_dir_all(root);
    let store = crate::store::Store::new(root);
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
    let mut harness = harness(app(config(root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Firefly").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    harness
}

/// A stand-in engine at `dir` that writes every call it is given into `dir/calls` and answers
/// each with success and nothing else.
#[cfg(unix)]
fn recording_engine(dir: &std::path::Path) -> (crate::engine::Engine, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let (binary, calls) = (dir.join("engine"), dir.join("calls"));
    std::fs::write(&binary, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n", calls.display()))
        .expect("the stand-in is written");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("it can be run");
    (crate::engine::Engine::new(crate::engine::EngineKind::Podman, binary), calls)
}

#[cfg(unix)]
#[test]
fn a_workspace_used_in_place_opens_on_the_real_folder_and_its_containers_mount_it_as_work() {
    let root = scratch("in-place-open");
    let _ = std::fs::remove_dir_all(&root);
    let own = root.join("my project");
    std::fs::create_dir_all(&own).expect("the person's folder");
    std::fs::write(own.join("notes.md"), "mine").expect("their notes");
    let store = crate::store::Store::new(root.join("store"));
    store
        .create_workspace_in("Firefly", qframe::date::Date::today_utc(), Some(own.clone()))
        .expect("the store takes a workspace");
    let (engine, calls) = recording_engine(&root);
    let app = super::QCode::new(
        config(store.root(), &[]),
        super::testing::dirs(),
        super::testing::host(),
        &settled(),
        Some(engine),
        None,
        None,
    )
    .with_providers(Some(super::testing::providers_file()), super::testing::no_web());
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Firefly").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    let screen = harness.app().workspace.as_ref().expect("the workspace screen is open");
    assert_eq!(screen.workspaces()[0].paths().code, own, "the file tree, the shell and the backup see the real folder");

    // The shell is the first container a person reaches; its mounts are the ones every
    // container of the workspace gets for its work folder.
    harness.click_text("New tab").render();
    harness.click_text("Shell").advance(Duration::from_millis(400)).render();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mounted = format!("{}:{}:rw", own.display(), crate::base::paths::CODE_DIR);
    let created = || {
        std::fs::read_to_string(&calls)
            .unwrap_or_default()
            .lines()
            .find(|call| call.starts_with("create "))
            .map(str::to_owned)
    };
    while created().is_none() && std::time::Instant::now() < deadline {
        harness.advance(Duration::from_millis(20)).render();
    }
    let create = created().unwrap_or_else(|| panic!("a container was made:\n{}", harness.screen()));
    assert!(create.contains(&mounted), "{create}");
    assert!(!create.contains("/Work:"), "no Work of the workspace's own is mounted: {create}");
    assert_eq!(std::fs::read_to_string(own.join("notes.md")).ok().as_deref(), Some("mine"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_workspace_whose_own_folder_is_gone_says_so_and_is_neither_opened_nor_given_a_new_folder() {
    let root = scratch("in-place-gone");
    let _ = std::fs::remove_dir_all(&root);
    let own = root.join("unplugged disk").join("project");
    std::fs::create_dir_all(&own).expect("the person's folder");
    let store = crate::store::Store::new(root.join("store"));
    store
        .create_workspace_in("Firefly", qframe::date::Date::today_utc(), Some(own.clone()))
        .expect("the store takes a workspace");
    std::fs::remove_dir_all(root.join("unplugged disk")).expect("the disk is gone");

    let mut harness = harness(app(config(store.root(), &["firefly"]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Continue").advance(MOMENT);
    let screen = harness.screen();
    assert_ne!(harness.app().page(), Page::Workspace, "{screen}");
    assert!(screen.contains("Firefly works in a folder that is not there"), "{screen}");
    assert!(!own.exists(), "nothing made the folder again");
    assert!(!root.join("unplugged disk").exists());
    assert_eq!(harness.app().config.recent_workspaces().first().map(WorkspaceId::as_str), Some("firefly"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_workspace_left_open_behind_the_list_is_not_deleted_from_under_its_tabs() {
    let root = scratch("delete-open");
    let mut harness = open_workspace(&root);
    // Back to the list the way the rail leads there, with the workspace still open behind it.
    rail_back(&mut harness);
    harness.advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
    harness.click_text("Delete a workspace").advance(MOMENT);
    let screen = harness.screen();
    let rows: Vec<&str> = screen.lines().collect();
    let start = rows.iter().position(|row| row.contains("Choose the workspace")).expect("the choice opened");
    let (y, row) = rows.iter().enumerate().skip(start + 1).find(|(_, row)| row.contains("Firefly")).expect("listed");
    let x = row[..row.find("Firefly").unwrap_or_default()].chars().count();
    harness.click(i32::try_from(x).expect("on screen"), i32::try_from(y).expect("on screen")).advance(MOMENT);
    let screen = harness.screen();
    assert!(screen.contains("Firefly is open"), "{screen}");
    assert!(!screen.contains("Delete Firefly?"), "nothing is asked:\n{screen}");
    assert!(root.join("Workspaces").join("firefly").is_dir(), "and nothing is deleted");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_switch_that_asks_before_messages_between_tabs_reaches_the_open_workspace_and_the_file() {
    let root = scratch("ask-first");
    let mut harness = open_workspace(&root);
    let asks = |harness: &qframe::runtime::Harness<super::QCode>| {
        let screen = harness.app().workspace.as_ref().expect("the workspace screen is open");
        (screen.ask_first(), harness.app().config.ask_first())
    };
    assert_eq!(asks(&harness), (false, false), "QCode starts without asking");

    rail_settings(&mut harness);
    assert_eq!(harness.app().page(), Page::Settings, "{}", harness.screen());
    harness.resize(SIZE.0, 70).render();
    // A switch is drawn in colour alone; it stands at the column's right edge, which the
    // language drop-down's arrow marks.
    let (_, row) = harness.find("Ask before the first message").expect("the row is on screen");
    let (edge, _) = harness.find("▾").expect("the language drop-down");
    harness.click(edge - 1, row).advance(MOMENT);
    assert_eq!(asks(&harness), (true, true), "{}", harness.screen());

    harness.click(edge - 1, row).advance(MOMENT);
    assert_eq!(asks(&harness), (false, false));
    let file = harness.app().config.to_toml();
    assert!(!file.contains("[bridge]"), "not asking is not written down:\n{file}");
    let _ = std::fs::remove_dir_all(&root);

    // A settings file that turned it on opens its workspaces asking.
    let root = scratch("ask-first-stored");
    let _ = std::fs::remove_dir_all(&root);
    crate::store::Store::new(&root)
        .create_workspace("Firefly", qframe::date::Date::today_utc())
        .expect("the store takes a workspace");
    let mut stored = config(&root, &[]);
    stored.set_ask_first(true);
    let mut opened = super::testing::harness(app(stored, &settled(), None), SIZE.0, SIZE.1);
    opened.click_text("Workspaces").advance(MOMENT);
    opened.click_text("Firefly").advance(MOMENT);
    assert_eq!(asks(&opened), (true, true), "{}", opened.screen());
    let _ = std::fs::remove_dir_all(&root);
}

/// A store at `root` holding the workspaces `names`, made afresh.
fn store_of(root: &std::path::Path, names: &[&str]) -> crate::store::Store {
    let _ = std::fs::remove_dir_all(root);
    let store = crate::store::Store::new(root);
    for name in names {
        store.create_workspace(name, qframe::date::Date::today_utc()).expect("the store takes a workspace");
    }
    store
}

/// Opens `name` from the list of workspaces, reached from the home screen.
fn open_from_the_list(harness: &mut qframe::runtime::Harness<crate::QCode>, name: &str) {
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text(name).advance(MOMENT);
}
