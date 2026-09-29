//! The keyboard as the screens open and close: the screen that opens takes it, and Esc closes
//! what is open over a screen before it leaves the screen.

use super::*;

#[test]
fn a_screen_that_opens_takes_the_keyboard_so_the_arrow_keys_work_at_once() {
    // The hint under every screen promises the arrow keys. A screen that opens without the
    // keyboard on its list breaks that promise until a Tab nobody mentioned is pressed.
    let root = scratch("focus-list");
    let _ = std::fs::remove_dir_all(&root);
    let store = crate::store::Store::new(&root);
    for name in ["Alpha", "Beta"] {
        store.create_workspace(name, qframe::date::Date::today_utc()).expect("the store takes a workspace");
    }
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    assert!(harness.is_focused("workspaces"), "the list has the keyboard:\n{}", harness.screen());
    harness.press("down").press("enter").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "down and enter alone opened the second workspace");
    let open = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(OpenWorkspace::name);
    assert_eq!(open, Some("Beta"), "the row under the first one is the one that opened");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_very_first_key_works_on_the_home_screen() {
    // No click, no Tab: the application has just opened, and the menu already has the
    // keyboard. Down from the first row is Workspaces.
    let mut harness = harness(app(config(&scratch("focus-first"), &[]), &settled(), None), SIZE.0, SIZE.1);
    assert!(harness.is_focused("menu"), "{}", harness.screen());
    harness.press("down").press("enter").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
}

#[test]
fn the_wizard_opens_with_the_keyboard_on_its_first_question() {
    let harness =
        harness(app(config(&scratch("focus-wizard"), &[]), &settled(), Some(SetupStep::Language)), SIZE.0, SIZE.1);
    // The gates are all settled, so the wizard opens on its last step; the keyboard is on
    // that step's question, not on the first step's.
    assert_eq!(harness.app().page(), Page::Setup);
    assert!(harness.is_focused("setup-location"), "{}", harness.screen());
}

#[test]
fn the_settings_screen_takes_the_keyboard_as_it_opens() {
    let mut harness = harness(app(config(&scratch("focus-settings"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Settings").advance(MOMENT);
    assert!(harness.is_focused("settings"), "{}", harness.screen());
}

#[test]
fn coming_back_puts_the_keyboard_on_the_menu_again() {
    let mut harness = harness(app(config(&scratch("focus-home"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Settings").advance(MOMENT);
    click_back(&mut harness);
    harness.advance(MOMENT);
    assert!(harness.is_focused("menu"), "{}", harness.screen());
    // The click left the selection on Settings, so one press up is the row above it, and
    // that press is the first key after coming back.
    harness.press("up").press("enter").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Providers, "{}", harness.screen());
}

#[test]
fn escape_leaves_the_screen_that_is_open() {
    let mut harness = harness(app(config(&scratch("escape"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Settings").advance(MOMENT);
    harness.press("esc").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
}

#[test]
fn escape_closes_what_is_open_over_a_screen_before_it_leaves_the_screen() {
    let root = scratch("escape-dialog");
    let _ = std::fs::remove_dir_all(&root);
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("New workspace").advance(MOMENT);
    assert!(harness.screen().contains("Start from"), "the dialog is open:\n{}", harness.screen());
    harness.press("esc").advance(MOMENT);
    assert!(!harness.screen().contains("Start from"), "the dialog closed:\n{}", harness.screen());
    assert_eq!(harness.app().page(), Page::Workspaces, "and the screen under it stayed");
    harness.press("esc").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Home, "a second press leaves the screen");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn escape_does_nothing_in_the_setup_wizard() {
    // There is no application behind the wizard to go back to, so the keys that leave a
    // screen do not leave this one; it is left by finishing it.
    let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
    let app = app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language));
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.press("esc").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
    assert!(harness.screen().contains("Chinese (Simplified)"), "{}", harness.screen());
    assert!(!harness.screen().contains("Back"), "and no way out is offered:\n{}", harness.screen());
}

#[test]
fn the_one_step_the_repair_strip_opens_can_be_put_down_again() {
    // A person whose engine cannot be installed this minute must be able to leave the step
    // and go on using everything that does not need a container.
    let mut harness = harness(app(config(&scratch("repair-esc"), &[]), &without_engine(), None), SIZE.0, SIZE.1);
    harness.click_text("Repair").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Setup);
    assert!(harness.screen().contains("Back"), "the way out is on screen too:\n{}", harness.screen());
    harness.press("esc").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
}

#[test]
fn escape_leaves_the_workspace_screen_while_no_tab_holds_the_keyboard() {
    // With a tab open the terminal has the keyboard and every key, Esc included, belongs to
    // the harness inside the container; the way out is then the one on screen. That half
    // needs a container runtime, so it is not tested here. This is the other half.
    let root = scratch("escape-workspace");
    let _ = std::fs::remove_dir_all(&root);
    let store = crate::store::Store::new(&root);
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Firefly").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace);
    let arrow = harness.env().icons().glyph("arrow-left").into_owned();
    assert!(harness.screen().contains(&arrow), "the way out is on screen:\n{}", harness.screen());
    harness.press("esc").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn escape_first_lets_a_cut_file_stay_and_only_then_leaves_the_workspace_screen() {
    let root = scratch("escape-cut");
    let _ = std::fs::remove_dir_all(&root);
    let store = crate::store::Store::new(&root);
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Firefly").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace);
    let id = harness.app().workspace.as_ref().and_then(|screen| screen.workspace()).map(|p| p.id().to_owned());
    let cut = qframe::widgets::FileManagerMsg::Cut("notes.txt".to_owned());
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::Files(id.expect("a workspace is open"), cut)));
    harness.press("esc").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "the first Esc only lets the cut go");
    let cut = harness.app().workspace.as_ref().and_then(|screen| screen.workspace()).map(|p| p.files().cut().len());
    assert_eq!(cut, Some(0));
    harness.press("esc").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}
