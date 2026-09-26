//! Shift+Enter in a harness writes a new line of the message instead of sending it.
//!
//! The framework's terminal sends the program a key of its own for it: `CSI 13;2u` when the
//! program turned on the kitty keyboard protocol, and `ESC CR`, the bytes of Alt+Enter, when it
//! did not. Harnesses such as Claude Code read either as a line break and a plain `CR` as "send".
//! The test presses the keys where the person's hand is and reads back the bytes a program
//! standing in for the harness received.

use super::*;

/// A screen whose one tab is a running harness tab, attached to `script` run by `sh` on this
/// machine in place of the harness's container.
fn harness_running(scratch: &Scratch, script: &str) -> (WorkspaceScreen, TerminalSession) {
    let mut screen = one_workspace(scratch);
    open(&mut screen, claude());
    assert_eq!(kinds(&screen), [TabKind::Profile("claude-sub".to_owned())]);
    let key = key(&screen, 0);
    let session =
        TerminalSession::spawn("/bin/sh".as_ref(), &["-c", script], Path::new("/")).expect("a terminal for the shell");
    screen.attach(key, session.clone());
    (screen, session)
}

#[test]
fn shift_enter_in_a_harness_is_a_new_line_and_enter_still_sends() {
    let scratch = Scratch::new("new-line");
    // The program shows, in hex, the first five bytes it receives.
    let (screen, session) = harness_running(&scratch, "stty raw -echo; printf 'ready '; head -c 5 | od -An -tx1");
    let mut harness = keyed(screen);

    harness.press("shift+enter").press("ctrl+enter").press("enter");
    wait_for(&mut harness, "1b 0d 1b 0d 0d");
    session.kill();
}

#[test]
fn shift_enter_in_a_harness_that_asks_for_the_kitty_keyboard_is_its_own_key() {
    let scratch = Scratch::new("new-line-kitty");
    // The program turns on the kitty keyboard protocol's first level, the way Claude Code does,
    // and shows, in hex, the first eight bytes it receives.
    let script = "stty raw -echo; printf '\\033[>1uready '; head -c 8 | od -An -tx1";
    let (screen, session) = harness_running(&scratch, script);
    let mut harness = keyed(screen);

    harness.press("shift+enter").press("enter");
    wait_for(&mut harness, "1b 5b 31 33 3b 32 75 0d");
    session.kill();
}
