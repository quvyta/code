//! What a screen says when a profile's image build has said nothing for long enough to look hung.
//!
//! A build has no time limit of its own: a big image takes twenty minutes, and that is normal.
//! What the person cannot read off a log that has stopped moving is whether to keep waiting, so
//! both screens that show a build — the workspace tab that is building a missing image and the
//! wizard's image page — judge the same silence the same way and say the same one line beside it.
//!
//! Nothing here stops a build, and nothing asks the engine whether it is stuck: the engine does not
//! say, and a build QCode gave up on is a half-made image and a person who wanted their profile.
//! The person's own Stop is the only way a build ends, and this rule only tells them it is time to
//! consider it.
//!
//! The silence is judged by [`Lengths`], which a screen holds and a test shortens: the product
//! uses the measured [`QUIET`] and [`LOOK`], and the number in the sentence is the first of them
//! in whole minutes rather than a measured silence, which a person cannot be shown.

use std::time::{Duration, Instant};

use qframe::prelude::*;
use qframe::runtime::Task;

/// How long a build may say nothing before it is said to be quiet.
///
/// Five minutes is well inside the longest silence of a build that is going well — a big layer
/// being pulled, a package being compiled — and well inside how long a person waits before
/// wondering whether to stop it.
pub const QUIET: Duration = Duration::from_secs(5 * 60);

/// How often a build that is running is looked at.
///
/// Short enough that the warning is up within a look of the silence being long enough, and long
/// enough that a build of twenty minutes is asked about some eighty times rather than once per
/// line it prints.
pub const LOOK: Duration = Duration::from_secs(15);

/// The two lengths a build's silence is judged by. The product's are [`QUIET`] and [`LOOK`]; a
/// test shortens both, since a test cannot wait five minutes for a warning and cannot ask the
/// engine for a build that really takes that long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lengths {
    /// How long a build may say nothing before it is said to be stuck.
    pub quiet: Duration,
    /// How often a build that is running is looked at.
    pub look: Duration,
}

impl Default for Lengths {
    fn default() -> Self {
        Self { quiet: QUIET, look: LOOK }
    }
}

impl Lengths {
    /// The whole minutes of [`Self::quiet`], which is what the sentence counts: the rule is one
    /// length for every person, and what is said is that length rather than a silence nobody
    /// measured to the second.
    #[must_use]
    pub fn minutes(&self) -> i64 {
        i64::try_from(self.quiet.as_secs() / 60).unwrap_or(i64::MAX)
    }
}

/// Whether a build that last said something at `said` has been quiet for longer than `quiet` as
/// of `now`, which is what the warning beside it is raised and taken down by.
#[must_use]
pub fn quiet_for(said: Instant, now: Instant, quiet: Duration) -> bool {
    now.saturating_duration_since(said) >= quiet
}

/// The next look at the builds running on a screen, timed once: a task that sleeps
/// [`Lengths::look`] and hands `woke` the moment it woke at, so what is judged is the screen as it
/// is then and not as it was when the look was asked for.
///
/// The caller decides whether a look is wanted and builds the message out of the moment, since
/// what a look says is the message each screen speaks.
pub fn follow<M: Send + 'static>(lengths: Lengths, woke: impl Fn(Instant) -> M + Send + 'static) -> Command<M> {
    let look = lengths.look;
    Command::task(Task::new(t!("build.looking"), move |cx| {
        if cx.sleep(look) { Ok(woke(Instant::now())) } else { Err(String::new()) }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lengths a test judges by: a silence a moment of real time reaches and a look the
    /// harness's clock reaches at once.
    const BRIEF: Lengths = Lengths { quiet: Duration::from_millis(50), look: Duration::from_millis(10) };

    /// The two moments one look is taken between: a build whose last line was `ago` before the
    /// look, and the look itself.
    fn quiet_since(ago: Duration) -> (Instant, Instant) {
        let last = Instant::now();
        (last, last + ago)
    }

    #[test]
    fn a_build_is_quiet_when_nothing_has_been_written_for_as_long_as_the_rule_asks() {
        let (last, now) = quiet_since(BRIEF.quiet - Duration::from_millis(1));
        assert!(!quiet_for(last, now, BRIEF.quiet), "the silence has not reached the rule yet");
        let (last, now) = quiet_since(BRIEF.quiet);
        assert!(quiet_for(last, now, BRIEF.quiet), "and reaching it is what raises the warning");
        // A line arriving starts the silence again, so the same rule says nothing afterwards.
        let (last, _) = quiet_since(BRIEF.quiet);
        assert!(!quiet_for(last, last + BRIEF.quiet / 2, BRIEF.quiet));
    }

    #[test]
    fn the_number_in_the_sentence_is_the_rules_own_length_in_whole_minutes() {
        assert_eq!(Lengths::default().minutes(), 5, "the product's five minutes, said as five");
        assert_eq!(BRIEF.minutes(), 0, "a test's rule of no minutes at all says zero rather than a fraction");
    }
}
