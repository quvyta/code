//! What the screens ask of the application and what comes of it: a login refreshed, quitting with
//! workspaces open, a setting that reaches the open workspaces, and the repair strip's one step.

use super::*;

#[test]
fn refreshing_an_identity_is_carried_out_and_answered_with_what_came_of_it() {
    // The request reaches the engine work and the answer is that work's own: with an engine
    // that is not there, the round stops at the engine and the toast carries its refusal
    // rather than a promise that the work will exist one day.
    let mut harness = harness(app_with_absent_engine(config(&scratch("refresh"), &[])), SIZE.0, SIZE.1);
    harness.click_text("Settings").advance(MOMENT);
    let profile = SafeName::parse("claude-sub").expect("the name is safe");
    harness.send(Msg::Settings(crate::ui::settings::Msg::Request(Request::RefreshIdentity(profile)))).advance(MOMENT);
    let screen = harness.screen();
    assert!(screen.contains("could not be refreshed"), "{screen}");
    assert!(!screen.contains("not part of QCode"), "{screen}");
}

#[test]
fn the_workspaces_open_as_qcode_quits_are_left_to_be_backed_up() {
    let root = scratch("farewell");
    store_of(&root, &["Alpha", "Beta"]);
    let farewell = crate::Farewell::default();
    let app = app_with_absent_engine(config(&root, &[])).with_farewell(farewell.clone());
    let mut harness = harness(app, SIZE.0, SIZE.1);
    open_from_the_list(&mut harness, "Alpha");
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::AddWorkspace)).advance(MOMENT);
    harness.click_text("Beta").advance(MOMENT);
    harness.press("ctrl+q");
    assert!(harness.quit_requested());
    let left = farewell.take().expect("the open workspaces were left behind");
    assert_eq!(left.names(), ["Alpha", "Beta"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn quitting_wakes_every_container_the_screen_had_frozen() {
    // A container left paused is one the next QCode finds asleep with every tab in it still holding
    // what it held, and one the reaper cannot stop, since a stop of a paused container is refused.
    // The quit is the last moment anything can be done about it: there is no later than this.
    use crate::engine::{Engine, EngineKind};

    let root = scratch("farewell-frozen");
    store_of(&root, &["Alpha"]);
    let folder = scratch("farewell-frozen-engine");
    std::fs::create_dir_all(&folder).expect("a folder for the stand-in engine");
    let (binary, calls) = (folder.join("engine"), folder.join("calls"));
    std::fs::write(&binary, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n", calls.display()))
        .expect("a stand-in engine");
    std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
    let engine = Engine::new(EngineKind::Podman, &binary);
    let mut harness = harness(app_with_engine(config(&root, &[]), engine), SIZE.0, SIZE.1);
    open_from_the_list(&mut harness, "Alpha");
    // The screen's own look is not what is under test here, so the message it would end with is
    // sent in its place: a container of this workspace, frozen.
    let container = "qcode-alpha-claude-sub";
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::Frozen(container.to_owned(), true)));
    harness.press("ctrl+q");
    assert!(harness.quit_requested());
    let asked = std::fs::read_to_string(&calls).unwrap_or_default();
    assert!(
        asked.lines().any(|call| call == format!("unpause {container}")),
        "the container is woken before QCode goes:\n{asked}"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn quitting_from_the_menu_leaves_them_too_and_nothing_when_none_is_open() {
    let farewell = crate::Farewell::default();
    let app = app_with_absent_engine(config(&scratch("farewell-none"), &[])).with_farewell(farewell.clone());
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.click_text("Quit");
    assert!(harness.quit_requested());
    assert_eq!(farewell.take(), None, "no workspace was open");
}

/// Chooses `option` on the settings row `row`, the way a person does it: the list is scrolled
/// with the wheel until the row is on screen, and a row whose choices all stand on it is
/// chosen by clicking the choice. A row that keeps them in a drop-down is opened by the arrow
/// that ends it first, and the option is then clicked in the list that falls open.
fn choose_setting(harness: &mut qframe::runtime::Harness<crate::QCode>, row: &str, option: &str) {
    use qframe::event::MouseKind;
    let middle = (i32::from(SIZE.0) / 2, i32::from(SIZE.1) / 2);
    for _ in 0..40 {
        if harness.find(row).is_some() {
            break;
        }
        harness.mouse(MouseKind::ScrollDown, middle.0, middle.1).render();
    }
    let at = harness.find(row).unwrap_or_else(|| panic!("`{row}` is on the settings screen:\n{}", harness.screen()));
    if let Some(spot) = harness.find(option)
        && spot.1 == at.1
    {
        harness.click(spot.0, spot.1).advance(MOMENT);
        return;
    }
    let line = harness.screen().lines().nth(usize::try_from(at.1).expect("on screen")).unwrap_or_default().to_owned();
    let arrow = i32::try_from(line.trim_end().chars().count() - 1).expect("on screen");
    harness.click(arrow, at.1).advance(MOMENT);
    let spot =
        harness.find(option).unwrap_or_else(|| panic!("`{option}` is offered under `{row}`:\n{}", harness.screen()));
    harness.click(spot.0, spot.1).advance(MOMENT);
}

#[test]
fn a_new_interval_reaches_the_open_workspaces_at_once() {
    use crate::backup::BackupEvery;

    let root = scratch("backup-every");
    store_of(&root, &["Alpha"]);
    let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
    open_from_the_list(&mut harness, "Alpha");
    let every = |harness: &qframe::runtime::Harness<crate::QCode>| {
        harness.app().workspace.as_ref().map(WorkspaceScreen::backup_every)
    };
    assert_eq!(every(&harness), Some(BackupEvery::Fifteen));
    rail_settings(&mut harness);
    choose_setting(&mut harness, "Back up open workspaces", "1 hour");
    click_back(&mut harness);
    harness.advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert_eq!(every(&harness), Some(BackupEvery::Hour));
    assert_eq!(harness.app().config.backup_every(), BackupEvery::Hour, "and it is stored");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_sound_choice_reaches_the_workspaces_opened_later_and_the_open_ones_at_once() {
    use crate::base::apps::Sound;

    let root = scratch("sound-choice");
    store_of(&root, &["Alpha"]);
    let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
    let sound =
        |harness: &qframe::runtime::Harness<crate::QCode>| harness.app().workspace.as_ref().map(WorkspaceScreen::sound);
    harness.click_text("Settings").advance(MOMENT);
    choose_setting(&mut harness, "Sounds", "Details only");
    assert!(harness.app().config.to_toml().contains("[apps]\nsound = \"details\""), "it is stored");
    click_back(&mut harness);
    harness.advance(MOMENT);
    open_from_the_list(&mut harness, "Alpha");
    assert_eq!(sound(&harness), Some(Sound::Details), "a screen made after the choice starts with it");
    rail_settings(&mut harness);
    choose_setting(&mut harness, "Sounds", "Play");
    click_back(&mut harness);
    harness.advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert_eq!(sound(&harness), Some(Sound::Play), "and the open screen takes the next one at once");
    assert!(!harness.app().config.to_toml().contains("[apps]"), "{}", harness.app().config.to_toml());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_last_row_leaves_the_application() {
    let mut harness = harness(app(config(&scratch("quit"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Quit");
    assert!(harness.quit_requested());
}

#[test]
fn the_repair_strip_runs_the_wizards_engine_step_on_its_own() {
    let mut harness = harness(app(config(&scratch("repair"), &[]), &without_engine(), None), SIZE.0, SIZE.1);
    harness.click_text("Repair").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Setup);
    let screen = harness.screen();
    assert!(screen.contains("Container engine"), "the engine step is the one that opened:\n{screen}");
    assert!(!screen.contains("Chinese (Simplified)"), "no step before it is asked again:\n{screen}");
    assert!(!screen.contains("Store"), "and none after it either:\n{screen}");
}

#[test]
fn the_settings_screen_can_move_the_store_through_the_same_one_step() {
    let mut harness = harness(app(config(&scratch("move"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Settings").advance(MOMENT);
    harness.click_text("Change").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
    let screen = harness.screen();
    assert!(screen.contains("live in one folder"), "the location step is the one that opened:\n{screen}");
}

#[test]
fn the_release_of_the_click_that_opened_the_location_step_does_not_finish_it() {
    use qframe::event::{MouseButton, MouseKind};

    // "Change" acts when the button goes down, and the page it opens puts Finish on the
    // screen while the button is still held. Wherever that release lands, it began on the
    // settings row and not on Finish, so the step must stay open for the person to answer.
    let mut harness = harness(app(config(&scratch("release"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Settings").advance(MOMENT);
    let (x, y) = harness.find("Change").expect("the settings screen offers to move the store");
    harness.mouse(MouseKind::Down(MouseButton::Left), x, y).advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
    let (x, y) = harness.find("Finish").expect("the location step can be finished");
    harness.mouse(MouseKind::Up(MouseButton::Left), x, y).advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Setup, "the step is still open:\n{}", harness.screen());
    // The same button, pressed and released on itself, does finish: it was reachable all along.
    harness.click_text("Finish").advance(MOMENT);
    assert_ne!(harness.app().page(), Page::Setup, "{}", harness.screen());
}
