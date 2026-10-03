//! The workspace screen's side of its builds: when a tab whose image is being built is looked at,
//! and what the look says about it.
//!
//! The rule itself — how long a build may say nothing, how often it is looked at and what is said
//! then — is [`crate::ui::stalling`], which the wizard's image page judges by as well, so that a
//! person who watches one build in either place is told the same thing about it.
//!
//! What is here is this screen's own bookkeeping: a look is timed while a tab is building an
//! image and never otherwise, and what came of one is a mark on the tab the build belongs to, so
//! that the page reads the answer rather than counting the silence itself while it draws.

use std::time::Instant;

use qframe::prelude::*;

use crate::ui::stalling;

use super::{Msg, Tab, WorkspaceScreen};

/// Times the next look at the tab whose image is being built, while there is one and no look is
/// timed already: one look at a time, so a build is not watched twice over.
pub(super) fn follow(screen: &mut WorkspaceScreen) -> Command<Msg> {
    if !building(screen) || screen.watching {
        return Command::none();
    }
    screen.watching = true;
    stalling::follow(screen.stall, Msg::BuildLooked)
}

/// What came of a look at this moment: every tab building an image is marked by whether its log
/// has been silent for as long as the rule asks, and the next look is timed while a build runs on.
///
/// A tab whose build ended while the look was on its way is not marked, and nothing is timed after
/// it: a build the screen has heard the end of is not watched.
pub(super) fn looked(screen: &mut WorkspaceScreen, now: Instant) -> Command<Msg> {
    let quiet = screen.stall.quiet;
    screen.watching = false;
    for workspace in &mut screen.workspaces {
        for tab in &mut workspace.tabs {
            tab.build_looked(now, quiet);
        }
    }
    follow(screen)
}

/// Whether a tab of any open workspace is building its profile's image.
fn building(screen: &WorkspaceScreen) -> bool {
    screen.workspaces.iter().flat_map(|workspace| workspace.tabs.iter()).any(Tab::is_building)
}
