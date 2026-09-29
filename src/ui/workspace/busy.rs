//! Which tabs are working, as the tab strip marks them.
//!
//! A terminal tab works while its program writes: its terminal received output within the bridge's
//! quiet time, the same silence the bridge waits for before typing a message in, so the two can
//! never disagree about whether a tab is busy. What a tab writes while the person types into it is
//! mostly the echo of their own keys, so a tab holding a line the person has not sent is not
//! marked: the mark says the program is at work, not that the person is. A window has no terminal
//! to listen to, so its tab works while its state says the window is being brought up.
//!
//! A working tab's label starts with a turning mark, one frame of the framework's dots spinner, and
//! a still dot when motion is reduced. The mark takes its cell only while the tab works: that is how
//! the framework's own mark on a tab is asked to behave, so the strip keeps its look when the mark
//! moves there, and every frame is one cell, so a tab never changes its width while it turns.
//!
//! The strip is looked at again every [`TURN_EVERY`] only while a tab of the open workspace works; a
//! screen of quiet tabs times nothing and draws nothing on its own.

use std::time::{Duration, Instant};

use qframe::env::Env;
use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::widgets::SpinnerStyle;

use super::{Msg, OpenWorkspace, Tab, TabKind, TabState, WorkspaceScreen, bridge};

/// How often the mark turns while a tab works.
pub(super) const TURN_EVERY: Duration = Duration::from_millis(100);

/// Whether `tab` is working as of `now`.
pub(super) fn working(tab: &Tab, now: Instant) -> bool {
    match tab.kind() {
        TabKind::Desktop(_) => matches!(tab.state(), TabState::Starting | TabState::Building(_)),
        _ => tab.session().is_some_and(|session| bridge::speaking(session, now) && !session.line_pending()),
    }
}

/// Times the next turn of the mark while a tab of the open workspace works and none is timed yet.
pub(super) fn follow(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let looked = screen.looked;
    let any = screen.workspace().is_some_and(|workspace| workspace.tabs.iter().any(|tab| working(tab, looked)));
    if !any || screen.turning {
        return Command::none();
    }
    screen.turning = true;
    Command::task(Task::new(t!("workspace.busy.timing"), |cx| {
        if cx.sleep(TURN_EVERY) { Ok(Msg::Turn(Instant::now())) } else { Err(String::new()) }
    }))
}

/// Takes a turn of the mark: the tabs are looked at as of `now`, and a working one shows the next
/// frame.
pub(super) fn turn(screen: &mut WorkspaceScreen, now: Instant) {
    screen.turning = false;
    screen.looked = now;
    screen.turn = screen.turn.wrapping_add(1);
}

/// The label of the tab at `index` of `workspace` on the strip: its mark first while it works.
pub(super) fn label(screen: &WorkspaceScreen, workspace: &OpenWorkspace, index: usize, env: &Env) -> String {
    let label = workspace.tab_label(index);
    match workspace.tabs.get(index) {
        Some(tab) if working(tab, screen.looked) => format!("{} {label}", mark(screen.turn, env)),
        _ => label,
    }
}

/// The mark of a working tab at turn `turn`: a frame of the framework's dots spinner, or a still
/// dot when motion is reduced.
fn mark(turn: usize, env: &Env) -> String {
    let icons = env.icons();
    if env.reduced_motion() {
        return icons.glyph("bullet").into_owned();
    }
    match icons.animation(SpinnerStyle::Dots.animation()) {
        Some(frames) if !frames.frames().is_empty() => {
            frames.glyph(turn % frames.frames().len(), icons.mode()).to_owned()
        }
        _ => icons.glyph("bullet").into_owned(),
    }
}
