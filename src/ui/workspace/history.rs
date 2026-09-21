//! What the page of a blank tab knows about the conversations each profile had in a workspace:
//! the last answer read out of the profile's container, whether a reading is under way, and
//! whether that reading has taken long enough to be shown.
//!
//! A reading is never drawn during its first [`SHOW_AFTER`], and once drawn it stays at least
//! [`SHOW_AT_LEAST`], so a quick answer never blinks an indicator and a slow one never flickers
//! one. A page shown again reads again, but keeps the answer it had on screen until the new one
//! arrives: the rows stay where they are rather than giving way to an indicator.

use std::time::Duration;

use qframe::date::DateTime;
use qframe::prelude::*;

use crate::engine::Engine;
use crate::profile::HarnessKind;
use crate::profile::history::{self, Conversation};

use super::plan::LaunchFailure;

/// How long a reading runs before it is shown: around here people start to notice waiting.
pub(super) const SHOW_AFTER: Duration = Duration::from_millis(300);

/// How long a reading, once shown, stays shown even when its answer comes sooner.
pub(super) const SHOW_AT_LEAST: Duration = Duration::from_millis(500);

/// How many of a profile's newest conversations a section shows before it is asked for all.
pub(super) const NEWEST: usize = 3;

/// The most characters of an engine's refusal a row carries: the row says why in a few words,
/// and the engine's whole answer would not fit on it.
const REASON_CHARS: usize = 60;

/// The conversations of one profile in one workspace, as far as the page knows them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HistoryKey {
    /// The workspace's id.
    pub workspace: String,
    /// The profile's name.
    pub profile: String,
}

/// What a section shows under its new chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shown<'a> {
    /// Nothing yet: the first reading has not taken long enough to be noticed.
    Nothing,
    /// A reading that has been under way long enough to say so.
    Reading,
    /// The reading failed, for this reason.
    Failed(&'a str),
    /// The conversations, newest first; empty when the profile has had none in the workspace.
    Conversations(&'a [Conversation]),
}

/// One profile's conversations in one workspace, with the reading that keeps them fresh.
#[derive(Debug, Default)]
pub(super) struct Shelf {
    /// The last answer, kept while the next reading runs.
    answer: Option<Result<Vec<Conversation>, String>>,
    /// Counts the readings, so the answer of one that was replaced is recognised and dropped.
    generation: u64,
    /// Whether a reading is under way.
    reading: bool,
    /// Whether the reading is drawn.
    showing: bool,
    /// Whether it is drawn because it has not yet been on screen for [`SHOW_AT_LEAST`].
    holding: bool,
}

impl Shelf {
    /// Starts a reading and returns its number.
    pub(super) fn start(&mut self) -> u64 {
        self.generation += 1;
        self.reading = true;
        self.generation
    }

    /// The reading `generation` has run for [`SHOW_AFTER`]. It is drawn when it is still the
    /// current one and there is no earlier answer to keep on screen; returns whether it now is,
    /// so its minimum time on screen can be counted.
    pub(super) fn slow(&mut self, generation: u64) -> bool {
        if generation != self.generation || !self.reading || self.answer.is_some() || self.showing {
            return false;
        }
        self.showing = true;
        self.holding = true;
        true
    }

    /// The drawn reading has been on screen for [`SHOW_AT_LEAST`]; it goes once its answer is in.
    pub(super) fn settled(&mut self) {
        self.holding = false;
        self.showing &= self.reading;
    }

    /// The reading `generation` answered. An answer of a reading that was replaced is dropped.
    pub(super) fn answered(&mut self, generation: u64, answer: Result<Vec<Conversation>, String>) {
        if generation != self.generation {
            return;
        }
        self.answer = Some(answer);
        self.reading = false;
        self.showing &= self.holding;
    }

    /// The number of the last reading started, which is the only one whose answer is taken.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// What the section shows now.
    pub(super) fn shown(&self) -> Shown<'_> {
        if self.showing {
            return Shown::Reading;
        }
        match &self.answer {
            None => Shown::Nothing,
            Some(Ok(found)) => Shown::Conversations(found),
            Some(Err(reason)) => Shown::Failed(reason),
        }
    }
}

/// Reads `harness`'s conversations out of the container `container`, in the words a row can
/// show when it cannot; `started` is called when the container had to be started for it.
///
/// This runs engine commands and waits for them, so it belongs on a background thread.
pub(super) fn read(
    engine: &Engine,
    container: &str,
    harness: HarnessKind,
    started: &mut dyn FnMut(),
) -> Result<Vec<Conversation>, String> {
    history::read_starting(engine, container, harness, started).map_err(|error| reason(&LaunchFailure::from(&error)))
}

/// The first line of what the engine said, short enough for the detail of a row; the command
/// itself when the engine said nothing.
fn reason(failure: &LaunchFailure) -> String {
    let said = [failure.output.as_str(), failure.command.as_str()]
        .into_iter()
        .find_map(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or_default();
    let mut cut: String = said.chars().take(REASON_CHARS).collect();
    if said.chars().count() > REASON_CHARS {
        cut.push('…');
    }
    cut
}

/// When a conversation was last used, as the page's time column writes it: the time of day for
/// today and yesterday, the date for anything older, all in the offset of `now`.
///
/// `now` is a parameter rather than read here, so the page and its tests agree on what "today"
/// is.
#[must_use]
pub(super) fn when(used_ms: i64, now: DateTime) -> String {
    let used = DateTime::from_unix(used_ms.div_euclid(1000), now.offset_minutes);
    let time = format!("{:02}:{:02}", used.time.hour, used.time.minute);
    let days = now.date.to_days() - used.date.to_days();
    match days {
        0 => t!("workspace.history.today", time = time),
        1 => t!("workspace.history.yesterday", time = time),
        _ => used.date.to_string(),
    }
}

/// The conversation a new-chat tab brought back from the last session was most likely showing:
/// the newest one used at or after the tab was opened (`opened`, in seconds) that no other tab
/// has already taken, given `found` newest first.
///
/// A new chat's id is not known when it starts, so this is the one guess the session makes; it
/// is safe because a conversation used before the tab existed can never be the tab's own, and a
/// conversation another tab shows is never shown twice.
#[must_use]
pub(super) fn claim<'a>(found: &'a [Conversation], opened: u64, taken: &[&str]) -> Option<&'a Conversation> {
    let since = i64::try_from(opened).unwrap_or(i64::MAX).saturating_mul(1000);
    found.iter().find(|conversation| conversation.used_ms >= since && !taken.contains(&conversation.id.as_str()))
}

#[cfg(test)]
mod tests {
    use qframe::date::{Date, TimeOfDay};

    use super::*;
    use crate::ui::settings::testing::translated;

    fn conversation(id: &str, used_ms: i64) -> Conversation {
        Conversation { id: id.to_owned(), title: None, used_ms }
    }

    /// 2026-09-18 15:00 in Istanbul (UTC+3).
    fn now() -> DateTime {
        DateTime { date: Date::new(2026, 9, 18).expect("a day"), time: TimeOfDay::new(15, 0, 0), offset_minutes: 180 }
    }

    fn at(date: (i32, u8, u8), time: (u8, u8)) -> i64 {
        let local = DateTime {
            date: Date::new(date.0, date.1, date.2).expect("a day"),
            time: TimeOfDay::new(time.0, time.1, 0),
            offset_minutes: 180,
        };
        local.to_unix() * 1000
    }

    #[test]
    fn today_yesterday_and_older_are_written_apart() {
        translated("en", || {
            assert_eq!(when(at((2026, 9, 18), (14, 32)), now()), "today 14:32");
            assert_eq!(when(at((2026, 9, 18), (0, 5)), now()), "today 00:05");
            assert_eq!(when(at((2026, 9, 17), (9, 10)), now()), "yesterday 09:10");
            assert_eq!(when(at((2026, 9, 16), (23, 59)), now()), "2026-09-16");
            assert_eq!(when(at((2025, 12, 31), (8, 0)), now()), "2025-12-31");
        });
    }

    #[test]
    fn the_day_is_the_local_one() {
        translated("en", || {
            // 01:30 on the 18th in Istanbul is still 22:30 on the 17th in UTC.
            let late = at((2026, 9, 18), (1, 30));
            assert_eq!(when(late, now()), "today 01:30");
            let utc = DateTime { offset_minutes: 0, ..now() };
            assert_eq!(when(late, utc), "yesterday 22:30", "the same moment, read in UTC");
        });
    }

    #[test]
    fn turkish_writes_bugun_and_dun() {
        translated("tr", || {
            assert_eq!(when(at((2026, 9, 18), (14, 32)), now()), "bugün 14:32");
            assert_eq!(when(at((2026, 9, 17), (9, 10)), now()), "dün 09:10");
        });
    }

    #[test]
    fn a_restored_new_chat_takes_the_newest_conversation_used_after_it_opened_and_not_taken() {
        let found = [conversation("c", 5_000_000), conversation("b", 4_000_000), conversation("a", 1_000_000)];
        assert_eq!(claim(&found, 2_000, &[]).map(|c| c.id.as_str()), Some("c"), "the newest after it opened");
        assert_eq!(claim(&found, 2_000, &["c"]).map(|c| c.id.as_str()), Some("b"), "one another tab has is skipped");
        assert_eq!(claim(&found, 2_000, &["c", "b"]), None, "one used before it opened is never its own");
        assert_eq!(claim(&found, 4_000, &["c"]).map(|c| c.id.as_str()), Some("b"), "used the very second it opened");
        assert_eq!(claim(&[], 0, &[]), None, "nothing read, nothing claimed");
    }

    #[test]
    fn a_quick_answer_is_never_shown_as_reading() {
        let mut shelf = Shelf::default();
        let first = shelf.start();
        assert_eq!(shelf.shown(), Shown::Nothing);
        shelf.answered(first, Ok(vec![conversation("a", 1)]));
        assert!(!shelf.slow(first), "its timer comes after the answer and finds nothing to show");
        assert!(matches!(shelf.shown(), Shown::Conversations(found) if found.len() == 1));
    }

    #[test]
    fn a_slow_reading_shows_and_stays_its_minimum() {
        let mut shelf = Shelf::default();
        let first = shelf.start();
        assert!(shelf.slow(first));
        assert_eq!(shelf.shown(), Shown::Reading);
        shelf.answered(first, Ok(Vec::new()));
        assert_eq!(shelf.shown(), Shown::Reading, "an answer inside the minimum waits for it");
        shelf.settled();
        assert_eq!(shelf.shown(), Shown::Conversations(&[]));

        let mut shelf = Shelf::default();
        let first = shelf.start();
        shelf.slow(first);
        shelf.settled();
        assert_eq!(shelf.shown(), Shown::Reading, "a reading past its minimum shows until it answers");
        shelf.answered(first, Err("refused".to_owned()));
        assert_eq!(shelf.shown(), Shown::Failed("refused"));
    }

    #[test]
    fn reading_again_keeps_the_last_answer_on_screen() {
        let mut shelf = Shelf::default();
        let first = shelf.start();
        shelf.answered(first, Ok(vec![conversation("a", 1)]));
        let second = shelf.start();
        assert!(!shelf.slow(second), "the rows stay; no indicator takes their place");
        assert!(matches!(shelf.shown(), Shown::Conversations(found) if found.len() == 1));
        shelf.answered(first, Ok(Vec::new()));
        assert!(matches!(shelf.shown(), Shown::Conversations(found) if found.len() == 1), "a stale answer is dropped");
        shelf.answered(second, Ok(vec![conversation("a", 1), conversation("b", 2)]));
        assert!(matches!(shelf.shown(), Shown::Conversations(found) if found.len() == 2));
    }

    #[test]
    fn a_refusal_is_cut_to_its_first_line() {
        let failure = LaunchFailure {
            command: "podman exec x".to_owned(),
            output: "\nError: no such container\nmore".to_owned(),
            image_missing: false,
        };
        assert_eq!(reason(&failure), "Error: no such container");
        let long = LaunchFailure { command: String::new(), output: "x".repeat(100), image_missing: false };
        assert_eq!(reason(&long).chars().count(), REASON_CHARS + 1);
        let silent = LaunchFailure { command: "podman exec x".to_owned(), output: String::new(), image_missing: false };
        assert_eq!(reason(&silent), "podman exec x");
    }
}
