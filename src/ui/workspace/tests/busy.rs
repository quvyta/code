//! Which tab is working: the strip marks a tab whose program is writing, turns the mark on the
//! screen's own clock while one is, and lets it go once the tab has been quiet for the bridge's
//! quiet time.
//!
//! The tabs run real programs on real pseudo-terminals, given to them the way a container that came
//! up gives a tab its session. The moment the marks are looked at is given where the test is about
//! the quiet time, so it says exactly which side of it it looks from; the mark's own turns are the
//! screen's timer, run on the harness clock.

use std::ffi::OsStr;
use std::time::Instant;

use super::*;

use crate::ui::workspace::bridge::QUIET;
use crate::ui::workspace::busy::TURN_EVERY;
use qframe::widgets::TerminalChange;

/// The workspace screen, with one more way in: a running program for a tab, as a container that
/// came up gives it.
struct Busy(WorkspaceScreen);

#[derive(Debug, Clone)]
enum Test {
    Screen(Msg),
    /// Gives the tab at this place a running program.
    Attach(usize, TerminalSession),
}

impl App for Busy {
    type Msg = Test;

    fn update(&mut self, message: Test) -> Command<Test> {
        match message {
            Test::Screen(message) => super::super::update(&mut self.0, message).map(Test::Screen),
            Test::Attach(index, session) => {
                let active = self.0.active;
                if let Some(workspace) = self.0.workspaces.get_mut(active) {
                    workspace.tabs[index].attached(session);
                }
                Command::none()
            }
        }
    }

    fn view(&self, ui: &mut View<'_, Test>) {
        super::super::view(&self.0, ui, Test::Screen, |_| {}, |_| {}, |_| {});
    }
}

/// A workspace with a Claude Code tab, a shell tab and a Codex tab, the first one open.
fn three_tabs(scratch: &Scratch) -> Harness<Busy> {
    let profiles = vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("codex-main", HarnessKind::Codex)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    open(&mut screen, Choice::Shell);
    open(&mut screen, Choice::NewChat("codex-main".to_owned()));
    let mut harness = Harness::with_env(Busy(screen), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Test::Screen(Msg::TogglePanel(false))).send(Test::Screen(Msg::OpenTab(0)));
    harness
}

/// Starts `script` as the program of the tab at `index`, and waits until it has written.
fn running(harness: &mut Harness<Busy>, scratch: &Scratch, index: usize, script: &str) -> TerminalSession {
    let session = TerminalSession::spawn(OsStr::new("/bin/sh"), &["-c", script], &scratch.0)
        .expect("a pseudo-terminal for the tab");
    let watch = session.watch();
    while watch.next_change_within(std::time::Duration::from_secs(20)).expect("the program writes within the wait")
        != TerminalChange::Output
    {}
    harness.send(Test::Attach(index, session.clone()));
    session
}

/// A program that writes once and then only reads.
const ONCE: &str = "stty -echo; printf 'ready'; exec cat > /dev/null";
/// A program that keeps writing, as a harness does while it works.
const WRITING: &str = "stty -echo; while :; do printf .; sleep 0.1; done";

/// Looks at the marks as of `now`, the moment the screen's own timer would reach.
fn look_at(harness: &mut Harness<Busy>, now: Instant) {
    harness.send(Test::Screen(Msg::Turn(now))).render();
}

/// The first line of the screen, where the strip is.
fn strip(harness: &Harness<Busy>) -> String {
    harness.screen().lines().next().unwrap_or_default().to_owned()
}

/// The mark in front of the tab labelled `label` on the strip, if it has one: a spinner's dots or
/// the still dot in the cell just before the space before its label.
fn mark(harness: &Harness<Busy>, label: &str) -> Option<char> {
    let line: Vec<char> = strip(harness).chars().collect();
    let label: Vec<char> = label.chars().collect();
    let at = line.windows(label.len()).position(|cells| cells == label.as_slice())?;
    let before = at.checked_sub(2)?;
    (line[at - 1] == ' ').then_some(line[before]).filter(|&cell| is_dots(cell) || cell == '•')
}

/// The column of the close mark of the tab labelled `label`.
fn close_at(harness: &Harness<Busy>, label: &str) -> usize {
    let line: Vec<char> = strip(harness).chars().collect();
    let label: Vec<char> = label.chars().collect();
    let at = line.windows(label.len()).position(|cells| cells == label.as_slice()).expect("the tab is on the strip");
    (at + label.len()..line.len()).find(|&x| line[x] == '×').expect("the tab has a close mark")
}

/// Whether `cell` is one of the braille dots the framework's dots spinner turns through.
fn is_dots(cell: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&cell)
}

#[test]
fn a_tab_whose_program_writes_is_marked_until_it_has_been_quiet_for_the_quiet_time() {
    let scratch = Scratch::new("busy-quiet");
    let mut harness = three_tabs(&scratch);
    let session = running(&mut harness, &scratch, 0, ONCE);

    look_at(&mut harness, session.last_output() + QUIET / 2);
    let shown = mark(&harness, "claude-sub");
    assert!(shown.is_some_and(is_dots), "the writing tab is marked: {}", strip(&harness));
    assert_eq!(mark(&harness, "codex-main"), None, "a tab with no program is not: {}", strip(&harness));

    look_at(&mut harness, session.last_output() + QUIET);
    assert_eq!(mark(&harness, "claude-sub"), None, "a quiet tab shows nothing: {}", strip(&harness));
    assert!(strip(&harness).contains("claude-sub"), "{}", strip(&harness));
}

#[test]
fn the_open_tab_and_a_tab_behind_it_are_both_marked_and_the_mark_turns_in_its_own_cell() {
    let scratch = Scratch::new("busy-both");
    let mut harness = three_tabs(&scratch);
    running(&mut harness, &scratch, 0, WRITING);
    running(&mut harness, &scratch, 2, WRITING);
    // The first look starts the screen's timer; from there on the turns are its own.
    look_at(&mut harness, Instant::now());

    let first = (mark(&harness, "claude-sub"), mark(&harness, "codex-main"));
    assert!(first.0.is_some_and(is_dots), "the open tab is marked: {}", strip(&harness));
    assert!(first.1.is_some_and(is_dots), "so is the one behind it: {}", strip(&harness));
    let close = (close_at(&harness, "claude-sub"), close_at(&harness, "codex-main"));

    harness.advance(TURN_EVERY);
    let second = (mark(&harness, "claude-sub"), mark(&harness, "codex-main"));
    assert!(second.0.is_some_and(is_dots) && second.1.is_some_and(is_dots), "{}", strip(&harness));
    assert_ne!(first.0, second.0, "the mark turned on the screen's clock: {}", strip(&harness));
    assert_eq!(
        (close_at(&harness, "claude-sub"), close_at(&harness, "codex-main")),
        close,
        "no tab changed its width while its mark turned: {}",
        strip(&harness)
    );
}

#[test]
fn nothing_is_timed_while_every_tab_is_quiet() {
    let scratch = Scratch::new("busy-still");
    let mut harness = three_tabs(&scratch);
    let session = running(&mut harness, &scratch, 0, ONCE);
    look_at(&mut harness, session.last_output() + QUIET);
    assert_eq!(mark(&harness, "claude-sub"), None, "{}", strip(&harness));

    let turned = harness.app().0.turn;
    harness.advance(TURN_EVERY * 10);
    assert!(!harness.app().0.turning, "no turn is timed");
    assert_eq!(harness.app().0.turn, turned, "and none was taken");
}

#[test]
fn with_motion_reduced_the_mark_is_a_still_dot() {
    let scratch = Scratch::new("busy-reduced");
    let mut harness = three_tabs(&scratch);
    harness.set_reduced_motion(true);
    running(&mut harness, &scratch, 0, WRITING);
    look_at(&mut harness, Instant::now());
    assert_eq!(mark(&harness, "claude-sub"), Some('•'), "{}", strip(&harness));
    harness.advance(TURN_EVERY * 3);
    assert_eq!(mark(&harness, "claude-sub"), Some('•'), "it does not move: {}", strip(&harness));
}

#[test]
fn a_tab_is_not_marked_while_the_person_has_a_line_typed_into_it_and_is_once_it_is_sent() {
    let scratch = Scratch::new("busy-typing");
    let mut harness = three_tabs(&scratch);
    // A program that keeps writing, so the only thing that changes below is the person's line;
    // what it writes while they type stands for the echo of their keys.
    let session = running(&mut harness, &scratch, 0, WRITING);
    // `write` is the path every key the tab's terminal sends takes; which tab the keyboard is in is
    // the focus tests' concern.
    session.write(b"hello").expect("the keys reach the program");
    look_at(&mut harness, Instant::now());
    assert_eq!(mark(&harness, "claude-sub"), None, "typing is not the program working: {}", strip(&harness));

    session.write(b"\r").expect("the line is sent");
    look_at(&mut harness, Instant::now());
    assert!(mark(&harness, "claude-sub").is_some_and(is_dots), "once sent, it is: {}", strip(&harness));
}
