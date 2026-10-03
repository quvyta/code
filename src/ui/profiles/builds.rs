//! The profiles screen's side of its builds: when the wizard's image build is looked at, and what
//! the look says about it.
//!
//! The rule itself — how long a build may say nothing, how often it is looked at and what is said
//! then — is [`crate::ui::stalling`], which the workspace screen judges a tab's build by as well,
//! so a person watching one build in either place is told the same thing about it.
//!
//! What is here is this screen's own bookkeeping: a look is timed while the wizard is building an
//! image and never otherwise, and what came of one is a mark on the draft, so that the page reads
//! the answer rather than counting the silence itself while it draws.

use std::time::Instant;

use qframe::prelude::*;

use crate::ui::stalling;

use super::{Draft, Msg, Profiles};

/// Times the next look at the wizard's build, while it is building one and no look is timed
/// already: one look at a time, so a build is not watched twice over.
pub(super) fn follow(state: &mut Profiles) -> Command<Msg> {
    if !building(state) || state.watching {
        return Command::none();
    }
    state.watching = true;
    stalling::follow(state.stall, Msg::BuildLooked)
}

/// What came of a look at this moment: the draft is marked by whether its log has been silent for
/// as long as the rule asks, and the next look is timed while a build runs on.
///
/// A build that ended while the look was on its way is not marked, and nothing is timed after it:
/// a build the screen has heard the end of is not watched.
pub(super) fn looked(state: &mut Profiles, now: Instant) -> Command<Msg> {
    let quiet = state.stall.quiet;
    state.watching = false;
    if let Some(draft) = &mut state.draft {
        draft.build_looked(now, quiet);
    }
    follow(state)
}

/// Whether the wizard is building a profile's image.
fn building(state: &Profiles) -> bool {
    state.draft.as_ref().is_some_and(Draft::is_building)
}
