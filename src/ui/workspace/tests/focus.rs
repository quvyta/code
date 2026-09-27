//! The keyboard follows the tab: whichever way the person switches, what they type next goes to
//! the program of the tab they switched to, with no click in it first. Walking the strip with its
//! arrows is the one switch that leaves the keyboard on the strip, and Enter or Down steps in.
//!
//! Each tab runs a program on this machine standing in for the one in a container; it shows, in
//! hex, the first three bytes it receives, so what reached which tab is read from its screen and
//! from the session's own record of the last key written to it.

use std::time::Instant;

use super::*;

/// What each tab's program runs: raw input, a line that says it is ready, then the first three
/// bytes it receives in hex.
fn echoing(name: &str) -> String {
    format!("stty raw -echo; printf 'ready {name} '; head -c 3 | od -An -tx1; sleep 30")
}

/// A workspace with two running shell tabs, "Shell 1" and "Shell 2", the first one open, in a
/// harness with the first one's terminal focused.
fn two_shells(scratch: &Scratch) -> (Harness<Keyed>, TerminalSession, TerminalSession) {
    let mut screen = one_workspace(scratch);
    open(&mut screen, Choice::Shell);
    open(&mut screen, Choice::Shell);
    assert_eq!(kinds(&screen), [TabKind::Shell, TabKind::Shell]);
    let spawn = |name: &str| {
        TerminalSession::spawn("/bin/sh".as_ref(), &["-c", &echoing(name)], Path::new("/"))
            .expect("a terminal for the shell")
    };
    let (first, second) = (spawn("one"), spawn("two"));
    screen.attach(key(&screen, 0), first.clone());
    screen.attach(key(&screen, 1), second.clone());
    apply(&mut screen, Msg::OpenTab(0));
    let harness = keyed(screen);
    (harness, first, second)
}

/// Types into whatever has the keyboard and waits for the second tab's program to show it, then
/// checks that nothing reached the first one after `since`.
fn typed_into_the_second(harness: &mut Harness<Keyed>, first: &TerminalSession, since: Instant) {
    harness.type_text("xyz");
    wait_for(harness, "78 79 7a");
    assert!(harness.screen().contains("ready two"), "the second tab's program showed it:\n{}", harness.screen());
    assert!(first.last_input() <= since, "the first tab's program was sent nothing");
}

#[test]
fn ctrl_pgdn_inside_a_harness_switches_and_the_next_keys_go_to_the_new_tab() {
    let scratch = Scratch::new("focus-chord");
    let (mut harness, first, second) = two_shells(&scratch);
    let since = Instant::now();

    harness.press("ctrl+pgdn");
    assert_eq!(harness.app().screen.workspace().map(|workspace| workspace.active_tab), Some(1));
    typed_into_the_second(&mut harness, &first, since);
    first.kill();
    second.kill();
}

#[test]
fn alt_with_a_digit_goes_to_that_tab_and_ctrl_pgup_goes_round_to_the_last() {
    let scratch = Scratch::new("focus-digit");
    let (mut harness, first, second) = two_shells(&scratch);
    let since = Instant::now();

    // Before the first tab is the last one.
    harness.press("ctrl+pgup");
    assert_eq!(harness.app().screen.workspace().map(|workspace| workspace.active_tab), Some(1));
    harness.press("alt+1");
    assert_eq!(harness.app().screen.workspace().map(|workspace| workspace.active_tab), Some(0));
    harness.press("alt+2");
    typed_into_the_second(&mut harness, &first, since);
    first.kill();
    second.kill();
}

#[test]
fn a_click_on_another_tab_puts_the_keyboard_in_its_program() {
    let scratch = Scratch::new("focus-click");
    let (mut harness, first, second) = two_shells(&scratch);
    let since = Instant::now();

    harness.click_text("Shell 2");
    typed_into_the_second(&mut harness, &first, since);
    first.kill();
    second.kill();
}

#[test]
fn the_strips_arrows_walk_the_tabs_and_enter_steps_into_the_one_shown() {
    let scratch = Scratch::new("focus-walk");
    let (mut harness, first, second) = two_shells(&scratch);
    let since = Instant::now();

    harness.press("ctrl+alt+space");
    assert!(harness.is_focused("workspace-tabs"));
    harness.press("right");
    assert_eq!(harness.app().screen.workspace().map(|workspace| workspace.active_tab), Some(1));
    assert!(harness.is_focused("workspace-tabs"), "the arrow switched and the strip kept the keyboard");
    harness.press("left").press("right");
    assert!(harness.is_focused("workspace-tabs"), "however far it walks");

    harness.press("enter");
    assert!(harness.is_focused("workspace-terminal"), "Enter steps into the tab:\n{}", harness.screen());
    typed_into_the_second(&mut harness, &first, since);
    first.kill();
    second.kill();
}

#[test]
fn down_on_the_strip_steps_into_the_tab_too() {
    let scratch = Scratch::new("focus-down");
    let (mut harness, first, second) = two_shells(&scratch);

    harness.press("ctrl+alt+space").press("right").press("down");
    assert!(harness.is_focused("workspace-terminal"), "{}", harness.screen());
    let since = Instant::now();
    typed_into_the_second(&mut harness, &first, since);
    first.kill();
    second.kill();
}

#[test]
fn switching_to_a_blank_tab_puts_the_keyboard_on_its_page() {
    let scratch = Scratch::new("focus-blank");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    let session = TerminalSession::spawn("/bin/sh".as_ref(), &["-c", &echoing("one")], Path::new("/"))
        .expect("a terminal for the shell");
    screen.attach(key(&screen, 0), session.clone());
    apply(&mut screen, Msg::NewTab);
    apply(&mut screen, Msg::OpenTab(0));
    let mut harness = keyed(screen);

    harness.press("alt+2");
    assert!(harness.is_focused("workspace-choices"), "the blank tab's page has the keyboard:\n{}", harness.screen());
    harness.press("alt+1");
    assert!(harness.is_focused("workspace-terminal"), "back in the shell:\n{}", harness.screen());
    harness.press("ctrl+pgdn");
    assert!(harness.is_focused("workspace-choices"), "the next tab's page, the same way:\n{}", harness.screen());
    session.kill();
}

#[test]
fn closing_the_open_tab_with_its_close_mark_puts_the_keyboard_in_the_one_shown_next() {
    let scratch = Scratch::new("focus-close");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    open(&mut screen, Choice::Shell);
    let first = TerminalSession::spawn("/bin/sh".as_ref(), &["-c", &echoing("two")], Path::new("/"))
        .expect("a terminal for the shell");
    let second = TerminalSession::spawn("/bin/sh".as_ref(), &["-c", &echoing("one")], Path::new("/"))
        .expect("a terminal for the shell");
    screen.attach(key(&screen, 0), first.clone());
    screen.attach(key(&screen, 1), second.clone());
    let mut harness = keyed(screen);
    let since = Instant::now();

    // The open tab is the second; its close mark is the last one on the strip.
    let line: Vec<char> = harness.screen().lines().next().unwrap_or_default().chars().collect();
    let mark = line.iter().rposition(|&cell| cell == '×').expect("the open tab has a close mark");
    harness.click(i32::try_from(mark).expect("a column"), 0);
    assert_eq!(kinds(&harness.app().screen), [TabKind::Shell], "{}", harness.screen());
    // The tab left is the first, whose program says "ready two".
    typed_into_the_second(&mut harness, &second, since);
    first.kill();
    second.kill();
}

#[test]
fn a_workspace_chosen_on_the_rail_puts_the_keyboard_in_its_open_tab() {
    let scratch = Scratch::new("focus-rail");
    let other = Scratch::new("focus-rail-other");
    let workspaces = vec![
        workspace("firefly", "Firefly", scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)]),
        workspace("serenity", "Serenity", other.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)]),
    ];
    let mut screen = WorkspaceScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces);
    apply(&mut screen, Msg::OpenWorkspace(1));
    open(&mut screen, Choice::Shell);
    let session = TerminalSession::spawn("/bin/sh".as_ref(), &["-c", &echoing("two")], Path::new("/"))
        .expect("a terminal for the shell");
    screen.attach(key(&screen, 0), session.clone());
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::Shell);
    let first = TerminalSession::spawn("/bin/sh".as_ref(), &["-c", &echoing("one")], Path::new("/"))
        .expect("a terminal for the shell");
    screen.attach(key(&screen, 0), first.clone());
    let mut harness = keyed(screen);
    let since = Instant::now();

    // The rail is collapsed to each workspace's initial, down the left edge.
    let text = harness.screen();
    let (x, y) = text
        .lines()
        .enumerate()
        .find_map(|(y, line)| line.chars().take(4).position(|cell| cell == 'S').map(|x| (x, y)))
        .unwrap_or_else(|| panic!("the rail shows Serenity's initial:\n{text}"));
    harness.click(i32::try_from(x).expect("a column"), i32::try_from(y).expect("a row"));
    assert_eq!(harness.app().screen.workspace().map(OpenWorkspace::name), Some("Serenity"));
    typed_into_the_second(&mut harness, &first, since);
    first.kill();
    session.kill();
}

#[test]
fn the_page_of_a_new_tab_goes_round_over_its_headings() {
    let scratch = Scratch::new("page-round");
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
    // The `+` after the tabs opens the page.
    harness.press("tab").press("enter").advance(Duration::from_millis(300));
    assert!(harness.is_focused("workspace-choices"), "the new tab's page has the keyboard:\n{}", harness.screen());
    // A heading and a gap stand between the rows; going round steps over them as the arrows do.
    let rows = ["in the workspace's own container", "+ New chat", "No chats yet"];
    assert_eq!(crate::testing::highlighted(&harness, &rows), Some(rows[0]), "{}", harness.screen());
    harness.press("up");
    assert_eq!(
        crate::testing::highlighted(&harness, &rows),
        Some(rows[2]),
        "Up on the first row:\n{}",
        harness.screen()
    );
    harness.press("down");
    assert_eq!(
        crate::testing::highlighted(&harness, &rows),
        Some(rows[0]),
        "Down on the last row:\n{}",
        harness.screen()
    );
}
