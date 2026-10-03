//! Which tab the strip shows as working: a tab whose program keeps writing shows the framework's
//! own spinner before its name and gives up no room for it, a tab whose program has gone quiet
//! shows nothing, and a tab holding a line the person has not sent shows nothing either.
//!
//! The tabs run real programs on real pseudo-terminals, given to them the way a container that came
//! up gives a tab its session. What is asserted is what the person reads off the strip row, and
//! every wait is bounded: a program writes in a loop that ends, and the strip is looked at until it
//! says what it will.

use std::ffi::OsStr;
use std::time::{Duration, Instant};

use super::*;

use crate::ui::workspace::busy::LOOK_EVERY;
use qframe::widgets::TerminalChange;

/// How long any one wait may last: generous enough for a loaded machine to start a shell, run a
/// program in it and be quiet again.
const WAIT: Duration = Duration::from_secs(10);

/// The pause between two looks at the strip: a program's writing is real time, so a step waits for
/// it.
const PAUSE: Duration = Duration::from_millis(25);

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

/// Starts `script` as the program of the tab at `index` and waits until it has written.
fn running(harness: &mut Harness<Busy>, scratch: &Scratch, index: usize, script: &str) -> TerminalSession {
    let session = TerminalSession::spawn(OsStr::new("/bin/sh"), &["-c", script], &scratch.0)
        .expect("a pseudo-terminal for the tab");
    let watch = session.watch();
    while watch.next_change_within(WAIT).expect("the program writes within the wait") != TerminalChange::Output {}
    harness.send(Test::Attach(index, session.clone()));
    // Opening the tab is what a person does with it, and what any message of the screen ends with:
    // the tabs are looked at again while one of them works, and that timer brings every look after
    // this one.
    harness.send(Test::Screen(Msg::OpenTab(index)));
    session
}

/// A program that writes once and then only reads, as a harness whose work is done.
const ONCE: &str = "stty -echo; printf 'ready'; exec cat > /dev/null";
/// A program that writes for a while and then only reads: the loop is bounded, so a tab that has to
/// go quiet can have one.
const WORKING: &str =
    "stty -echo; i=0; while [ $i -lt 25 ]; do printf .; sleep 0.1; i=$((i + 1)); done; exec cat > /dev/null";
/// A program that keeps writing for longer than any test of it waits, still in a bounded loop.
const WRITING: &str =
    "stty -echo; i=0; while [ $i -lt 600 ]; do printf .; sleep 0.1; i=$((i + 1)); done; exec cat > /dev/null";

/// Has the screen look at its tabs as of now: the message its own timer sends while a tab works,
/// and the moment the mark on the strip is decided at.
fn look(harness: &mut Harness<Busy>) {
    harness.send(Test::Screen(Msg::Turn(Instant::now()))).render();
}

/// Looks at the strip until `what` holds of it, for as long as that takes and no longer: each step is
/// the screen's own clock coming round, which is what brings the next look, with the program's
/// writing going on in between.
fn until(harness: &mut Harness<Busy>, what: impl Fn(&Harness<Busy>) -> bool) {
    let since = Instant::now();
    while !what(harness) {
        assert!(since.elapsed() < WAIT, "the strip never settled on it:\n{}", strip(harness));
        std::thread::sleep(PAUSE);
        harness.advance(LOOK_EVERY).render();
    }
}

/// The strip row, where the tabs are.
fn strip(harness: &Harness<Busy>) -> String {
    harness.screen().lines().next().unwrap_or_default().to_owned()
}

/// The column the tab labelled `label` starts at on the strip row.
fn at(harness: &Harness<Busy>, label: &str) -> usize {
    let line: Vec<char> = strip(harness).chars().collect();
    let letters: Vec<char> = label.chars().collect();
    line.windows(letters.len())
        .position(|cells| cells == letters.as_slice())
        .unwrap_or_else(|| panic!("{label} is on the strip:\n{}", strip(harness)))
}

/// The mark in front of the tab labelled `label`, if it shows one. The framework's mark stands in
/// the tab's own padding with a space between it and the name, so it is two cells back from where
/// the name begins; the spinner turns in braille, and with reduced motion one dot stands still
/// where it turns.
fn mark(harness: &Harness<Busy>, label: &str) -> Option<char> {
    let line: Vec<char> = strip(harness).chars().collect();
    let name = at(harness, label);
    let cell = *line.get(name.checked_sub(2)?)?;
    (line[name - 1] == ' ').then_some(cell).filter(|&cell| is_dots(cell) || cell == '●')
}

/// Whether `cell` is one of the braille dots the framework's spinner turns through.
fn is_dots(cell: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&cell)
}

/// Whether the tab labelled `label` shows the framework's spinner.
fn spinning(harness: &Harness<Busy>, label: &str) -> bool {
    mark(harness, label).is_some_and(is_dots)
}

#[test]
fn a_tab_whose_program_writes_shows_the_spinner_before_its_name_and_keeps_its_room() {
    let scratch = Scratch::new("busy-writing");
    let mut harness = three_tabs(&scratch);
    running(&mut harness, &scratch, 0, WORKING);

    until(&mut harness, |harness| spinning(harness, "claude-sub"));
    assert!(spinning(&harness, "claude-sub"), "the writing tab is marked: {}", strip(&harness));
    assert_eq!(mark(&harness, "codex-main"), None, "a tab with no program is not: {}", strip(&harness));
    // The mark takes no room of its own, so where the tab next along begins is read with the mark
    // showing, and read again once it is gone.
    let room = at(&harness, "Shell");

    // The program stops writing for good, so the mark goes away on the look that notices it.
    until(&mut harness, |harness| !spinning(harness, "claude-sub"));
    assert!(strip(&harness).contains("claude-sub"), "the tab itself stays: {}", strip(&harness));
    assert_eq!(at(&harness, "Shell"), room, "the mark took no room: {}", strip(&harness));
}

#[test]
fn a_tab_is_marked_while_its_program_writes_and_not_once_it_has_been_quiet() {
    let scratch = Scratch::new("busy-quiet");
    let mut harness = three_tabs(&scratch);
    running(&mut harness, &scratch, 0, ONCE);

    until(&mut harness, |harness| spinning(harness, "claude-sub"));
    assert!(spinning(&harness, "claude-sub"), "a program that has written is working: {}", strip(&harness));

    // Nothing is written after that, so the mark goes once the bridge's quiet time is over.
    until(&mut harness, |harness| !spinning(harness, "claude-sub"));
    assert!(strip(&harness).contains("claude-sub"), "the tab is still on the strip: {}", strip(&harness));
}

#[test]
fn the_tabs_are_looked_at_again_while_one_of_them_works_and_not_once_none_does() {
    let scratch = Scratch::new("busy-looked");
    let mut harness = three_tabs(&scratch);
    running(&mut harness, &scratch, 0, WORKING);
    until(&mut harness, |harness| spinning(harness, "claude-sub"));

    // A look is the only thing that moves the moment the tabs were looked at, so it is the screen's
    // own clock that brings the next one.
    let looked = harness.app().0.looked;
    harness.advance(LOOK_EVERY).render();
    assert!(harness.app().0.looked > looked, "the screen looked again while the tab worked");

    until(&mut harness, |harness| !spinning(harness, "claude-sub"));
    let looked = harness.app().0.looked;
    harness.advance(LOOK_EVERY * 10).render();
    assert_eq!(harness.app().0.looked, looked, "and nothing is timed once every tab is quiet");
}

#[test]
fn a_tab_holding_a_line_the_person_has_not_sent_is_not_marked_and_is_once_it_is_sent() {
    let scratch = Scratch::new("busy-typing");
    let mut harness = three_tabs(&scratch);
    // A program that keeps writing, so the only thing that changes below is the person's line; what
    // it writes while they type stands for the echo of their own keys.
    let session = running(&mut harness, &scratch, 0, WRITING);
    // `write` is the path every key the tab's terminal sends takes; which tab the keyboard is in is
    // the focus tests' concern.
    session.write(b"hello").expect("the keys reach the program");
    look(&mut harness);
    assert_eq!(mark(&harness, "claude-sub"), None, "typing is not the program working: {}", strip(&harness));

    session.write(b"\r").expect("the line is sent");
    // The mark comes on the next look, as it does for any program that starts writing.
    until(&mut harness, |harness| spinning(harness, "claude-sub"));
}
