//! The rules a message between two tabs has to pass, and what QCode remembers to apply them.
//!
//! In the order they are asked:
//!
//! 1. **The network's direction.** A tab whose profile has no network never sends to a tab whose
//!    profile has it: the second tab could carry the first one's files out, which is exactly what
//!    taking the network away was meant to prevent. The other way round, and between two tabs of
//!    the same kind, nothing leaves that could not leave already.
//! 2. **The chain.** A message sent by a tab that was itself sent one a short while ago continues
//!    that exchange, one step further; a chain longer than [`MOST_HOPS`] is cut, so two agents
//!    answering each other stop on their own.
//! 3. **The pace.** One tab sends at most [`MOST_PER_WINDOW`] messages in [`RATE_WINDOW`], so an
//!    agent that sends in a loop is stopped even when every message starts a new chain.
//! 4. **The person.** The first message from one tab to another asks the person, who allows or
//!    denies it; the answer holds for that pair, that direction, until QCode closes. It is kept
//!    in memory only: a pair of tabs does not outlive QCode, and a new session starts asking
//!    again.
//!
//! Nothing here reads a clock: every question is given the moment it is asked, which is how the
//! windows are tested without waiting.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// The longest chain of messages. Asking and answering is two steps; a task that goes through
/// a third tab and comes back with its answer is four. Six leaves room for that and one more
/// question, and stops two agents that answer each other after three rounds.
pub const MOST_HOPS: u32 = 6;

/// How long a tab that was sent a message is taken to be answering it. Ten minutes is longer
/// than an agent takes to work on a message and reply, and short enough that a new task the
/// person sets later starts a new chain.
pub const CHAIN_WINDOW: Duration = Duration::from_secs(10 * 60);

/// The most messages one tab sends in [`RATE_WINDOW`]. An agent handing out work writes to a
/// few tabs at once; five in a minute covers that, and a loop reaches it within seconds.
pub const MOST_PER_WINDOW: usize = 5;

/// The window [`MOST_PER_WINDOW`] is counted in.
pub const RATE_WINDOW: Duration = Duration::from_secs(60);

/// One side of a message: the tab, by its key, and whether its container reaches the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct End {
    /// The tab's key.
    pub tab: u64,
    /// Whether its profile has the network.
    pub network: bool,
}

/// Why a message was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A tab without the network may not send to one with it.
    Network,
    /// The chain is longer than [`MOST_HOPS`].
    Chain,
    /// The sender sent [`MOST_PER_WINDOW`] messages in the last [`RATE_WINDOW`] already.
    Pace,
    /// The person denied messages from this tab to that one.
    Denied,
}

/// What the person said about one pair, in one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Messages from the first tab to the second are taken.
    Allowed,
    /// They are refused.
    Denied,
}

/// What QCode remembers for the rules, for as long as it runs.
#[derive(Debug, Default)]
pub struct Rules {
    /// The person's answers, by sender and receiver.
    decisions: HashMap<(u64, u64), Decision>,
    /// When each tab sent its recent messages, oldest first.
    sent: HashMap<u64, VecDeque<Instant>>,
    /// The step of the last message each tab was sent, and when.
    received: HashMap<u64, (u32, Instant)>,
}

impl Rules {
    /// Asks the rules that do not need the person about a message from `from` to `to` at `now`,
    /// and answers the step of the chain it would be.
    ///
    /// Nothing is recorded: a message that then waits for the person or is denied by them has
    /// not been sent.
    ///
    /// # Errors
    ///
    /// The first rule the message breaks.
    pub fn check(&self, from: End, to: End, now: Instant) -> Result<u32, Refusal> {
        if !from.network && to.network {
            return Err(Refusal::Network);
        }
        let hop = self.hop(from.tab, now);
        if hop > MOST_HOPS {
            return Err(Refusal::Chain);
        }
        let recent = self
            .sent
            .get(&from.tab)
            .map_or(0, |times| times.iter().filter(|&&at| now.saturating_duration_since(at) < RATE_WINDOW).count());
        if recent >= MOST_PER_WINDOW {
            return Err(Refusal::Pace);
        }
        Ok(hop)
    }

    /// The step a message from `tab` at `now` would be: one more than the last message it was
    /// sent, when that came within [`CHAIN_WINDOW`], and the first step otherwise.
    #[must_use]
    pub fn hop(&self, tab: u64, now: Instant) -> u32 {
        match self.received.get(&tab) {
            Some(&(hop, at)) if now.saturating_duration_since(at) < CHAIN_WINDOW => hop + 1,
            _ => 1,
        }
    }

    /// What the person said about messages from `from` to `to`, when they were asked.
    #[must_use]
    pub fn decision(&self, from: u64, to: u64) -> Option<Decision> {
        self.decisions.get(&(from, to)).copied()
    }

    /// Records what the person said about messages from `from` to `to`.
    pub fn decide(&mut self, from: u64, to: u64, decision: Decision) {
        self.decisions.insert((from, to), decision);
    }

    /// Counts a message `from` sent at `now` against its pace, whether it is taken at once or
    /// waits for the person.
    pub fn sent(&mut self, from: u64, now: Instant) {
        let times = self.sent.entry(from).or_default();
        while times.front().is_some_and(|&at| now.saturating_duration_since(at) >= RATE_WINDOW) {
            times.pop_front();
        }
        times.push_back(now);
    }

    /// Records that `to` was handed a message that was step `hop` of its chain, at `now`; what
    /// `to` sends next continues the chain.
    pub fn received(&mut self, to: u64, hop: u32, now: Instant) {
        self.received.insert(to, (hop, now));
    }

    /// Forgets everything about the tab `tab`, which was closed: its keys are never used again,
    /// and nothing of it should be held for as long as QCode runs.
    pub fn forget(&mut self, tab: u64) {
        self.decisions.retain(|&(from, to), _| from != tab && to != tab);
        self.sent.remove(&tab);
        self.received.remove(&tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONLINE: bool = true;
    const OFFLINE: bool = false;

    fn end(tab: u64, network: bool) -> End {
        End { tab, network }
    }

    #[test]
    fn a_tab_without_the_network_never_sends_to_one_with_it() {
        let rules = Rules::default();
        let now = Instant::now();
        assert_eq!(rules.check(end(1, OFFLINE), end(2, ONLINE), now), Err(Refusal::Network));
        assert_eq!(rules.check(end(1, ONLINE), end(2, OFFLINE), now), Ok(1), "the other way leaks nothing");
        assert_eq!(rules.check(end(1, OFFLINE), end(2, OFFLINE), now), Ok(1));
        assert_eq!(rules.check(end(1, ONLINE), end(2, ONLINE), now), Ok(1));
    }

    #[test]
    fn the_person_saying_yes_does_not_open_the_network_rule() {
        let mut rules = Rules::default();
        rules.decide(1, 2, Decision::Allowed);
        assert_eq!(rules.check(end(1, OFFLINE), end(2, ONLINE), Instant::now()), Err(Refusal::Network));
    }

    #[test]
    fn a_decision_holds_for_one_pair_in_one_direction() {
        let mut rules = Rules::default();
        assert_eq!(rules.decision(1, 2), None, "nobody was asked yet");
        rules.decide(1, 2, Decision::Allowed);
        rules.decide(3, 2, Decision::Denied);
        assert_eq!(rules.decision(1, 2), Some(Decision::Allowed));
        assert_eq!(rules.decision(2, 1), None, "answering is asked about on its own");
        assert_eq!(rules.decision(3, 2), Some(Decision::Denied));
        assert_eq!(rules.decision(1, 3), None);
    }

    #[test]
    fn a_closed_tab_is_forgotten_with_every_decision_about_it() {
        let mut rules = Rules::default();
        rules.decide(1, 2, Decision::Allowed);
        rules.decide(2, 3, Decision::Allowed);
        rules.decide(3, 4, Decision::Denied);
        rules.forget(2);
        assert_eq!(rules.decision(1, 2), None);
        assert_eq!(rules.decision(2, 3), None);
        assert_eq!(rules.decision(3, 4), Some(Decision::Denied));
    }

    #[test]
    fn two_tabs_answering_each_other_are_stopped_after_the_longest_chain() {
        let mut rules = Rules::default();
        let start = Instant::now();
        let (a, b) = (end(1, ONLINE), end(2, ONLINE));
        let mut steps = Vec::new();
        for turn in 0..10u64 {
            // A minute apart, so the pace never stands in the way: only the chain does.
            let now = start + Duration::from_secs(60 * turn);
            let (from, to) = if turn % 2 == 0 { (a, b) } else { (b, a) };
            match rules.check(from, to, now) {
                Ok(hop) => {
                    rules.sent(from.tab, now);
                    rules.received(to.tab, hop, now);
                    steps.push(hop);
                }
                Err(refusal) => {
                    assert_eq!(refusal, Refusal::Chain);
                    break;
                }
            }
        }
        assert_eq!(steps, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn a_chain_ends_when_the_tab_was_not_sent_anything_for_a_while() {
        let mut rules = Rules::default();
        let now = Instant::now();
        rules.received(1, MOST_HOPS, now);
        assert_eq!(rules.check(end(1, ONLINE), end(2, ONLINE), now), Err(Refusal::Chain));
        assert_eq!(rules.hop(1, now + CHAIN_WINDOW - Duration::from_secs(1)), MOST_HOPS + 1);
        assert_eq!(rules.check(end(1, ONLINE), end(2, ONLINE), now + CHAIN_WINDOW), Ok(1), "a new task, a new chain");
    }

    #[test]
    fn a_tab_sending_in_a_loop_is_stopped_by_its_pace_and_let_go_a_minute_later() {
        let mut rules = Rules::default();
        let start = Instant::now();
        let (from, to) = (end(1, ONLINE), end(2, ONLINE));
        for second in 0..MOST_PER_WINDOW as u64 {
            let now = start + Duration::from_secs(second);
            assert_eq!(rules.check(from, to, now), Ok(1));
            rules.sent(from.tab, now);
        }
        let later = start + Duration::from_secs(10);
        assert_eq!(rules.check(from, to, later), Err(Refusal::Pace));
        assert_eq!(rules.check(end(3, ONLINE), to, later), Ok(1), "another sender has its own pace");
        assert_eq!(rules.check(from, to, start + RATE_WINDOW), Ok(1), "the oldest message left the window");
    }

    #[test]
    fn the_rules_are_asked_in_order_network_chain_then_pace() {
        let mut rules = Rules::default();
        let now = Instant::now();
        rules.received(1, MOST_HOPS, now);
        for _ in 0..MOST_PER_WINDOW {
            rules.sent(1, now);
        }
        assert_eq!(rules.check(end(1, OFFLINE), end(2, ONLINE), now), Err(Refusal::Network));
        assert_eq!(rules.check(end(1, ONLINE), end(2, ONLINE), now), Err(Refusal::Chain));
        rules.received(1, 1, now);
        assert_eq!(rules.check(end(1, ONLINE), end(2, ONLINE), now), Err(Refusal::Pace));
    }
}
