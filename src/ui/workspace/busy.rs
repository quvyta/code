//! Which tabs are working, as the tab strip marks them.
//!
//! A terminal tab works while its program writes: its terminal received output within the bridge's
//! quiet time, the same silence the bridge waits for before typing a message in, so the two can
//! never disagree about whether a tab is busy. What a tab writes while the person types into it is
//! mostly the echo of their own keys, so a tab holding a line the person has not sent is not
//! marked: the mark says the program is at work, not that the person is. A window has no terminal
//! to listen to, so its tab works while its state says the window is being brought up.
//!
//! The mark itself is the framework's: [`Tabs::busy`](qframe::widgets::Tabs::busy) turns a thin
//! spinner before a tab's name, in the accent colour and on the framework's own clock, in the cell
//! the resting name slides into, so it takes no room and no tab moves when it comes or goes. What is
//! left here is the two things the framework cannot know: which tab works, and when to look again to
//! notice that its program has stopped.
//!
//! The strip is looked at again every [`LOOK_EVERY`] only while a tab of the open workspace works; a
//! screen of quiet tabs times nothing and draws nothing on its own.

use std::time::{Duration, Instant};

use qframe::prelude::*;
use qframe::runtime::Task;

use super::{Msg, Tab, TabKind, TabState, WorkspaceScreen, bridge};

/// How often the tabs are looked at again while one of them works.
///
/// The framework turns the mark on its own clock, so a look is not what moves it: a look is what
/// notices that a program has gone quiet, and the mark goes away on the next one. A quarter of a
/// second late is not seen, and a look costs a pass over the tabs.
pub(super) const LOOK_EVERY: Duration = Duration::from_millis(250);

/// Whether `tab` is working as of `now`.
pub(super) fn working(tab: &Tab, now: Instant) -> bool {
    match tab.kind() {
        TabKind::Desktop(_) => matches!(tab.state(), TabState::Starting | TabState::Building(_)),
        _ => tab.session().is_some_and(|session| bridge::speaking(session, now) && !session.line_pending()),
    }
}

/// Times the next look at the tabs while a tab of the open workspace works and none is timed yet.
pub(super) fn follow(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let looked = screen.looked;
    let any = screen.workspace().is_some_and(|workspace| workspace.tabs.iter().any(|tab| working(tab, looked)));
    if !any || screen.turning {
        return Command::none();
    }
    screen.turning = true;
    Command::task(Task::new(t!("workspace.busy.timing"), |cx| {
        if cx.sleep(LOOK_EVERY) { Ok(Msg::Turn(Instant::now())) } else { Err(String::new()) }
    }))
}

/// Looks at the tabs as of `now`: a tab whose program has stopped writing shows no mark, and one
/// that is still writing is asked for its mark again.
pub(super) fn turn(screen: &mut WorkspaceScreen, now: Instant) {
    screen.turning = false;
    screen.looked = now;
}
