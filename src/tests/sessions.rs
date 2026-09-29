//! The workspaces the rail holds and the session that brings them back: tabs opened, the list's
//! plus, Continue, and the session file read, written and broken.

use super::*;

/// The names of the rail of the open workspace screen, in order.
fn rail(harness: &qframe::runtime::Harness<crate::QCode>) -> Vec<String> {
    let screen = harness.app().workspace.as_ref().expect("a workspace screen");
    screen.workspaces().iter().map(|workspace| workspace.name().to_owned()).collect()
}

/// Opens a blank tab on the workspace screen and chooses a shell in it; the engine of these
/// tests is not there, so the tab fails at once and keeps the engine's words.
fn open_a_shell(harness: &mut qframe::runtime::Harness<crate::QCode>) {
    use crate::ui::workspace::{Choice, Msg as Workspace};
    harness.send(Msg::Workspace(Workspace::NewTab)).advance(MOMENT);
    let screen = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace);
    let key = screen.and_then(|workspace| workspace.active_tab()).map(crate::ui::workspace::Tab::key);
    let key = key.expect("the blank tab is open");
    harness.send(Msg::Workspace(Workspace::Choose(key, Choice::Shell))).advance(MOMENT);
}

/// What the tabs of the open workspace are, in order.
fn tab_kinds(harness: &qframe::runtime::Harness<crate::QCode>) -> Vec<crate::ui::workspace::TabKind> {
    let screen = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace);
    screen.map(|workspace| workspace.tabs().iter().map(|tab| tab.kind().clone()).collect()).unwrap_or_default()
}

#[test]
fn ctrl_t_opens_a_blank_tab_and_the_key_list_says_so() {
    use crate::ui::workspace::TabKind;

    let root = scratch("ctrl-t");
    store_of(&root, &["Alpha"]);
    let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
    harness.press("ctrl+t").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Home, "away from a workspace the key means nothing");
    open_from_the_list(&mut harness, "Alpha");
    harness.press("ctrl+t").advance(MOMENT);
    assert_eq!(tab_kinds(&harness), [TabKind::New], "{}", harness.screen());
    assert!(harness.is_focused("workspace-choices"), "the blank tab's page has the keyboard");
    harness.press("ctrl+t").advance(MOMENT);
    assert_eq!(tab_kinds(&harness), [TabKind::New, TabKind::New], "the page lets the key through");

    harness.press("?");
    let screen = harness.screen();
    assert!(screen.contains("open a new tab"), "the key list names it:\n{screen}");
    assert!(screen.contains("ctrl t"), "{screen}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_workspace_opened_from_the_list_is_the_only_one_in_the_rail() {
    let root = scratch("only-one");
    store_of(&root, &["Alpha", "Beta", "Gamma"]);
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    open_from_the_list(&mut harness, "Beta");
    assert_eq!(rail(&harness), ["Beta"], "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_rails_plus_opens_the_list_and_the_pick_joins_the_same_screen() {
    let root = scratch("rail-add");
    store_of(&root, &["Alpha", "Beta"]);
    let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
    open_from_the_list(&mut harness, "Alpha");
    open_a_shell(&mut harness);
    let tab = harness
        .app()
        .workspace
        .as_ref()
        .and_then(WorkspaceScreen::workspace)
        .map(|workspace| workspace.tabs()[0].key());

    let text = harness.screen();
    let row = text.lines().position(|line| line.chars().take(4).any(|cell| cell == '+'));
    let row = i32::try_from(row.expect("the rail carries a plus")).expect("a row");
    harness.click(1, row).advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspaces, "the plus opens the list:\n{}", harness.screen());
    harness.click_text("Beta").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert_eq!(rail(&harness), ["Alpha", "Beta"], "the pick joins the screen that was open");
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::OpenWorkspace(0))).advance(MOMENT);
    let kept = harness
        .app()
        .workspace
        .as_ref()
        .and_then(WorkspaceScreen::workspace)
        .map(|workspace| workspace.tabs()[0].key());
    assert_eq!(kept, tab, "and nothing of the first workspace was closed");

    rail_back(&mut harness);
    assert_eq!(harness.app().page(), Page::Workspaces, "the list is not stacked twice:\n{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn continue_goes_back_to_the_live_screen_as_it_was() {
    let root = scratch("continue-live");
    store_of(&root, &["Alpha", "Beta"]);
    let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
    open_from_the_list(&mut harness, "Alpha");
    open_a_shell(&mut harness);
    let before = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(|workspace| {
        let tab = &workspace.tabs()[0];
        (tab.key(), tab.state().clone())
    });
    rail_back(&mut harness);
    click_back(&mut harness);
    harness.advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Home);
    assert!(harness.screen().contains("Continue"), "{}", harness.screen());
    assert!(harness.screen().contains("Alpha"), "the row names the open workspace:\n{}", harness.screen());

    // The list opens another workspace into the screen that is still live behind the home
    // screen, rather than a screen of its own.
    open_from_the_list(&mut harness, "Beta");
    assert_eq!(rail(&harness), ["Alpha", "Beta"]);
    harness.press("esc").advance(MOMENT);
    harness.press("esc").advance(MOMENT);
    assert!(harness.screen().contains("Alpha +1"), "{}", harness.screen());
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert_eq!(rail(&harness), ["Alpha", "Beta"]);
    let open = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(OpenWorkspace::name);
    assert_eq!(open, Some("Beta"), "the workspace that was open is open again");
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::OpenWorkspace(0))).advance(MOMENT);
    let after = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(|workspace| {
        let tab = &workspace.tabs()[0];
        (tab.key(), tab.state().clone())
    });
    assert_eq!(after, before, "the tab is the same tab, in the state it was left in");
    rail_back(&mut harness);
    assert_eq!(harness.app().page(), Page::Home, "continue came straight from home");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn continue_brings_back_the_saved_session_and_starts_only_the_tab_in_view() {
    let root = scratch("continue-saved");
    store_of(&root, &["Alpha", "Beta"]);
    let file = scratch("continue-saved-session").join("session.toml");
    let _ = std::fs::remove_file(&file);
    std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
    let text = "active = \"alpha\"\n\n\
                [[workspace]]\nid = \"beta\"\nactive-tab = 0\n\n\
                [[workspace]]\nid = \"alpha\"\nactive-tab = 1\n\n\
                [[workspace.tab]]\nkind = \"shell\"\nnumber = 1\nopened = 10\n\n\
                [[workspace.tab]]\nkind = \"shell\"\nconversation = \"c-1\"\nnumber = 2\nopened = 20\n";
    std::fs::write(&file, text).expect("a session file");
    let app = app_with_absent_engine(config(&root, &["beta"])).with_session(Some(file.clone()));
    let mut harness = harness(app, SIZE.0, SIZE.1);
    assert!(harness.screen().contains("beta +1"), "{}", harness.screen());
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert_eq!(rail(&harness), ["Beta", "Alpha"], "the rail's order is the session's");
    let screen = harness.app().workspace.as_ref().expect("a workspace screen");
    let alpha = screen.workspace().expect("a workspace is open");
    assert_eq!(alpha.name(), "Alpha", "the open workspace is the session's");
    let tabs: Vec<(u64, Option<&str>)> = alpha.tabs().iter().map(|tab| (tab.opened(), tab.conversation())).collect();
    assert_eq!(tabs, [(10, None), (20, Some("c-1"))]);
    assert_eq!(alpha.active_tab().map(Tab::opened), Some(20), "the open tab is the session's");
    assert_eq!(alpha.tabs()[0].state(), &TabState::Waiting, "the tab out of view has not started");
    assert!(
        matches!(alpha.tabs()[1].state(), TabState::Failed(_)),
        "the tab in view went to the engine at once: {:?}",
        alpha.tabs()[1].state()
    );
    // Nothing changed, so nothing was written back over the file.
    assert_eq!(std::fs::read_to_string(&file).ok().as_deref(), Some(text));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&file);
}

#[test]
fn workspaces_of_the_session_that_are_gone_are_skipped_and_named() {
    let root = scratch("continue-gone");
    store_of(&root, &["Alpha"]);
    let file = scratch("continue-gone-session").join("session.toml");
    std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
    let text = "[[workspace]]\nid = \"ghost\"\n\n[[workspace]]\nid = \"alpha\"\n\n[[workspace]]\nid = \"wraith\"\n";
    std::fs::write(&file, text).expect("a session file");
    let app = app(config(&root, &[]), &settled(), None).with_session(Some(file.clone()));
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(rail(&harness), ["Alpha"]);
    let screen = harness.screen();
    assert!(screen.contains("2 workspaces of the last session are gone"), "{screen}");
    assert!(screen.contains("ghost, wraith"), "{screen}");
    let saved = crate::store::Session::load(&file).value.expect("the file is rewritten");
    assert_eq!(saved.workspaces.len(), 1, "and it now holds what is open");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_broken_session_file_is_read_in_part_and_says_where() {
    let root = scratch("continue-broken");
    store_of(&root, &["Alpha"]);
    let file = scratch("continue-broken-session").join("session.toml");
    std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
    std::fs::write(&file, "[[workspace]]\nid = \"alpha\"\nactive-tab = \"x\"\n").expect("a session file");
    let app = app(config(&root, &[]), &settled(), None).with_session(Some(file.clone()));
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(rail(&harness), ["Alpha"]);
    let screen = harness.screen();
    assert!(screen.contains("Only part of the last session"), "{screen}");
    assert!(screen.contains("session.toml:3:"), "the toast says where:\n{screen}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_session_file_that_cannot_be_read_falls_back_to_the_last_workspace() {
    let root = scratch("continue-unreadable");
    store_of(&root, &["Alpha", "Beta"]);
    // A folder where the file should be.
    let file = scratch("continue-unreadable-session");
    std::fs::create_dir_all(&file).expect("a folder");
    let app = app(config(&root, &["beta"]), &settled(), None).with_session(Some(file.clone()));
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.click_text("Continue").advance(MOMENT);
    assert_eq!(rail(&harness), ["Beta"], "the workspace opened last, alone");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&file);
}

#[test]
fn the_session_is_written_when_what_is_open_changes_and_only_then() {
    let root = scratch("session-writes");
    store_of(&root, &["Alpha", "Beta"]);
    let file = scratch("session-writes-file").join("deeper").join("session.toml");
    let _ = std::fs::remove_dir_all(file.parent().expect("a folder"));
    let app = app(config(&root, &[]), &settled(), None).with_session(Some(file.clone()));
    let mut harness = harness(app, SIZE.0, SIZE.1);
    open_from_the_list(&mut harness, "Alpha");
    let saved = crate::store::Session::load(&file).value.expect("opening a workspace writes the session");
    assert_eq!(saved.workspaces.iter().map(|workspace| workspace.id.as_str()).collect::<Vec<_>>(), ["alpha"]);

    std::fs::remove_file(&file).expect("the file is there");
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::TogglePanel(false))).advance(MOMENT);
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::OpenWorkspace(0))).advance(MOMENT);
    assert!(!file.exists(), "a change that leaves the session as it was writes nothing");

    harness.send(Msg::Workspace(crate::ui::workspace::Msg::AddWorkspace)).advance(MOMENT);
    harness.click_text("Beta").advance(MOMENT);
    let saved = crate::store::Session::load(&file).value.expect("a new workspace writes it again");
    assert_eq!(saved.active.as_ref().map(WorkspaceId::as_str), Some("beta"));
    harness.send(Msg::Workspace(crate::ui::workspace::Msg::CloseWorkspace(1))).advance(MOMENT);
    let saved = crate::store::Session::load(&file).value.expect("closing one writes it again");
    assert_eq!(saved.workspaces.len(), 1);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(file.parent().expect("a folder"));
}

#[test]
fn the_on_close_choice_is_stored_and_the_default_is_not_written() {
    use crate::store::OnClose;
    let root = scratch("on-close");
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.send(Msg::Settings(crate::ui::settings::Msg::OnClose(OnClose::Keep)));
    assert_eq!(harness.app().config.on_close(), OnClose::Keep);
    assert!(harness.app().config.to_toml().contains("[containers]\non-close = \"keep\""));
    harness.send(Msg::Settings(crate::ui::settings::Msg::OnClose(OnClose::Stop)));
    assert_eq!(harness.app().config.on_close(), OnClose::Stop);
    assert!(!harness.app().config.to_toml().contains("containers"), "{}", harness.app().config.to_toml());
}

#[test]
fn the_service_row_follows_the_machine_and_what_is_installed_on_it() {
    use crate::service::units::ServiceHost;
    use crate::store::Platform;
    use crate::ui::settings::ServiceRow;
    let root = scratch("service-row");
    let _ = std::fs::remove_dir_all(&root);
    let host = ServiceHost {
        platform: Platform::Linux,
        units: root.join("systemd"),
        registry: root.join("containers.toml"),
        program: root.join("qcode"),
        uid: Some(1000),
    };
    let linux = app(config(&root, &[]), &settled(), None).with_service(Some(host.clone()), Platform::Linux);
    assert_eq!(linux.settings.service(), Some(ServiceRow::Ready { installed: false, busy: false }));
    // The files alone say it is installed: they are read from the disk on the way in. They
    // are written here by hand; pressing the button would run the machine's service manager.
    for (path, text) in host.files() {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        std::fs::write(path, text).expect("written");
    }
    let installed = app(config(&root, &[]), &settled(), None).with_service(Some(host), Platform::Linux);
    assert_eq!(installed.settings.service(), Some(ServiceRow::Ready { installed: true, busy: false }));
    let windows = app(config(&root, &[]), &settled(), None).with_service(None, Platform::Windows);
    assert_eq!(windows.settings.service(), Some(ServiceRow::Unsupported));
    let _ = std::fs::remove_dir_all(&root);
}
