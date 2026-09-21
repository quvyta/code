//! The bridge between tabs on the workspace screen: each open workspace listens on its socket, the
//! calls of its tabs' agents are answered here by the rules, the person is asked about the first
//! message between two tabs, and a message that is taken waits in the tab it was sent to.
//!
//! A taken message is not yet typed into the receiving harness. Typing it in needs the terminal
//! session to paste in the harness's bracketed paste mode and to tell when the harness's output
//! has gone quiet, which the framework's session does not offer yet. Until it does, the message
//! waits in the tab, where the person sees who sent it and can read it, and the sending agent is
//! told exactly that. Delivering it is then one step: take the tab's oldest letter when the tab
//! is quiet and paste it.

use std::time::{Duration, Instant};

use qframe::prelude::*;
use qframe::runtime::{Confirm, Task};
use qframe::widgets::{ScrollView, Toast};

use crate::bridge::config::Unregistered;
use crate::bridge::protocol::{Answer, Listed, MOST_TEXT, Question, Request};
use crate::bridge::rules::{Decision, End, Refusal};
use crate::bridge::socket::{Call, Inbox, Listener};
use crate::profile::{HarnessKind, NetworkMode, Profile};

use super::{Msg, OpenWorkspace, Tab, TabKey, TabKind, WorkspaceScreen};

/// How long a message waits for the person's answer before its sender is told that it waits.
/// Well inside the minute some harnesses give a tool before they give up on it.
pub(super) const ASK_WAIT: Duration = Duration::from_secs(45);

/// How much of the first message the question to the person shows.
const PREVIEW_CHARS: usize = 600;

/// The most lines the letters of a tab take when they are shown; more scroll.
const LETTER_ROWS: u16 = 10;

/// Whether a workspace listens on its socket.
#[derive(Debug, Default)]
pub(super) enum Link {
    /// Not yet: the screen does not bridge, or has not come round to this workspace.
    #[default]
    Off,
    /// The socket could not be opened; the person was told why, once.
    Failed,
    /// Listening. `run` numbers the listener, so a call waited for on one that was closed is not
    /// taken for one of this one's.
    On {
        /// The socket.
        listener: Listener,
        /// Which listener this is.
        run: u64,
    },
}

/// A message that was taken and waits in the tab it was sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Letter {
    /// Who sent it: the harness and the title of the sending tab.
    pub from: String,
    /// The message.
    pub text: String,
}

/// A question to the person about one pair of tabs, and the messages waiting on the answer.
#[derive(Debug)]
pub(super) struct Asking {
    from: TabKey,
    to: TabKey,
    waiting: Vec<Waiting>,
}

/// A message waiting on the person's answer. Its call is answered and taken away when the wait
/// grows long, and the message still goes on or is dropped with the person's answer.
#[derive(Debug)]
struct Waiting {
    call: Option<Call>,
    text: String,
    hop: u32,
}

/// What can be done with the letters waiting in a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Letters {
    /// Show them, or hide them again.
    Show(bool),
    /// Throw them away.
    Discard,
}

/// Opens the socket of every open workspace that has none yet, when the screen bridges, and waits
/// for its first call. A workspace whose socket cannot be opened says why once and is not tried
/// again while it stays open.
pub(super) fn follow(screen: &mut WorkspaceScreen) -> Command<Msg> {
    if !screen.bridging {
        return Command::none();
    }
    let mut commands = Vec::new();
    for workspace in &mut screen.workspaces {
        if !matches!(workspace.link, Link::Off) {
            continue;
        }
        match Listener::open(&workspace.paths.mcp()) {
            Ok(listener) => {
                screen.last_link += 1;
                commands.push(wait(workspace.id.as_str().to_owned(), screen.last_link, listener.inbox()));
                workspace.link = Link::On { listener, run: screen.last_link };
            }
            Err(error) => {
                workspace.link = Link::Failed;
                let toast =
                    Toast::warning(t!("bridge.closed", workspace = workspace.name.as_str())).body(error.to_string());
                commands.push(Command::toast(toast));
            }
        }
    }
    Command::batch(commands)
}

/// Waits on a background thread for the next call to the workspace `id`'s listener `run`.
fn wait(id: String, run: u64, inbox: Inbox) -> Command<Msg> {
    Command::perform(move || Msg::Bridge(id, run, inbox.next()))
}

/// Takes a call to the workspace `id`'s listener `run`, answers it, and waits for the next one.
/// `None` is a listener that was closed, which ends its waiting.
pub(super) fn called(screen: &mut WorkspaceScreen, id: &str, run: u64, call: Option<Call>) -> Command<Msg> {
    let Some(call) = call else { return Command::none() };
    let Some(workspace) = screen.workspaces.iter().find(|workspace| workspace.id.as_str() == id) else {
        return Command::none();
    };
    let Link::On { listener, run: current } = &workspace.link else { return Command::none() };
    if *current != run {
        return Command::none();
    }
    let next = wait(id.to_owned(), run, listener.inbox());
    let asked = answer(screen, id, call, Instant::now());
    Command::batch([next, asked])
}

/// Answers `call`, made to the workspace `id` at `now`.
pub(super) fn answer(screen: &mut WorkspaceScreen, id: &str, call: Call, now: Instant) -> Command<Msg> {
    let question = match &call.question {
        Ok(question) => question.clone(),
        Err(_) => {
            call.answer(Answer::refused(t!("bridge.answer.malformed")));
            return Command::none();
        }
    };
    let Some(workspace) = screen.workspaces.iter().find(|workspace| workspace.id.as_str() == id) else {
        return Command::none();
    };
    let Some(sender) = sender(workspace, &question) else {
        call.answer(Answer::refused(t!("bridge.answer.stranger")));
        return Command::none();
    };
    match question.request {
        Request::List => {
            call.answer(list(workspace, sender));
            Command::none()
        }
        Request::Send { tab, text } => send(screen, id, sender, &tab, text, call, now),
    }
}

/// The harness tab of `workspace` whose token the question carries.
fn sender(workspace: &OpenWorkspace, question: &Question) -> Option<TabKey> {
    workspace
        .tabs
        .iter()
        .find(|tab| matches!(tab.kind(), TabKind::Profile(_)) && tab.token() == question.token)
        .map(Tab::key)
}

/// The profile of the harness tab `key`, when it has one the store still has.
fn profile_of(workspace: &OpenWorkspace, key: TabKey) -> Option<&Profile> {
    let tab = workspace.tabs.iter().find(|tab| tab.key() == key)?;
    let TabKind::Profile(name) = tab.kind() else { return None };
    workspace.profiles.iter().find(|profile| profile.name.as_str() == name)
}

/// How a tab is named to the person and to the agents: its harness and its title.
fn named(workspace: &OpenWorkspace, key: TabKey) -> String {
    let title = workspace.tabs.iter().position(|tab| tab.key() == key).map(|index| workspace.tab_label(index));
    let harness = profile_of(workspace, key).map(|profile| profile.harness.record().display_name);
    match (harness, title) {
        (Some(harness), Some(title)) => format!("{harness} · {title}"),
        (None, Some(title)) => title,
        _ => String::new(),
    }
}

/// The other harness tabs of `workspace`, the ones `sender` can send to.
fn others(workspace: &OpenWorkspace, sender: TabKey) -> Vec<(usize, &Tab, &Profile)> {
    workspace
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.key() != sender)
        .filter_map(|(index, tab)| profile_of(workspace, tab.key()).map(|profile| (index, tab, profile)))
        .collect()
}

/// The answer to a list: every other harness tab of the workspace.
fn list(workspace: &OpenWorkspace, sender: TabKey) -> Answer {
    let tabs: Vec<Listed> = others(workspace, sender)
        .into_iter()
        .map(|(index, tab, profile)| Listed {
            tab: tab.key().0.to_string(),
            title: workspace.tab_label(index),
            harness: profile.harness.record().display_name.to_owned(),
            profile: profile.name.as_str().to_owned(),
            network: profile.network == NetworkMode::Full,
        })
        .collect();
    if tabs.is_empty() {
        return Answer::listed(t!("bridge.answer.alone"), tabs);
    }
    let mut text = t!("bridge.answer.listed", n = tabs.len());
    for tab in &tabs {
        let network = if tab.network { t!("bridge.answer.online") } else { t!("bridge.answer.offline") };
        text.push('\n');
        text.push_str(&t!(
            "bridge.answer.tab",
            tab = tab.tab.as_str(),
            title = tab.title.as_str(),
            harness = tab.harness.as_str(),
            network = network
        ));
    }
    Answer::listed(text, tabs)
}

/// The tab `wanted` names among the other harness tabs of `workspace`: by its id, or by its title
/// when exactly one tab has it.
fn target(workspace: &OpenWorkspace, sender: TabKey, wanted: &str) -> Option<TabKey> {
    let others = others(workspace, sender);
    let wanted = wanted.trim();
    if let Some((_, tab, _)) = others.iter().find(|(_, tab, _)| tab.key().0.to_string() == wanted) {
        return Some(tab.key());
    }
    let titled: Vec<TabKey> = others
        .iter()
        .filter(|(index, _, _)| workspace.tab_label(*index) == wanted)
        .map(|(_, tab, _)| tab.key())
        .collect();
    match titled.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// Handles a message from `from` to the tab `wanted` of the workspace `id`.
fn send(
    screen: &mut WorkspaceScreen,
    id: &str,
    from: TabKey,
    wanted: &str,
    text: String,
    call: Call,
    now: Instant,
) -> Command<Msg> {
    let Some(workspace) = screen.workspaces.iter().find(|workspace| workspace.id.as_str() == id) else {
        return Command::none();
    };
    let Some(to) = target(workspace, from, wanted) else {
        call.answer(Answer::refused(t!("bridge.answer.no-tab", tab = wanted)));
        return Command::none();
    };
    if text.trim().is_empty() {
        call.answer(Answer::refused(t!("bridge.answer.empty")));
        return Command::none();
    }
    if text.chars().count() > MOST_TEXT {
        call.answer(Answer::refused(t!("bridge.answer.long", most = MOST_TEXT)));
        return Command::none();
    }
    let (Some(from_profile), Some(to_profile)) = (profile_of(workspace, from), profile_of(workspace, to)) else {
        call.answer(Answer::refused(t!("bridge.answer.no-tab", tab = wanted)));
        return Command::none();
    };
    let ends = (
        End { tab: from.0, network: from_profile.network == NetworkMode::Full },
        End { tab: to.0, network: to_profile.network == NetworkMode::Full },
    );
    let (sender_name, receiver_name) = (named(workspace, from), named(workspace, to));
    let hop = match screen.rules.check(ends.0, ends.1, now) {
        Ok(hop) => hop,
        Err(refusal) => {
            call.answer(Answer::refused(refused(refusal, &receiver_name)));
            return Command::none();
        }
    };
    match screen.rules.decision(from.0, to.0) {
        Some(Decision::Denied) => {
            call.answer(Answer::refused(refused(Refusal::Denied, &receiver_name)));
            Command::none()
        }
        Some(Decision::Allowed) => {
            screen.rules.sent(from.0, now);
            queue(screen, to, sender_name, text, hop, now);
            call.answer(Answer::done(t!("bridge.answer.queued", tab = receiver_name.as_str())));
            Command::none()
        }
        None => {
            screen.rules.sent(from.0, now);
            let waiting = Waiting { call: Some(call), text: text.clone(), hop };
            if let Some(asking) = screen.asking.iter_mut().find(|asking| asking.from == from && asking.to == to) {
                asking.waiting.push(waiting);
                return Command::none();
            }
            screen.asking.push(Asking { from, to, waiting: vec![waiting] });
            Command::batch([ask(&sender_name, &receiver_name, &text, from, to), remind(from, to)])
        }
    }
}

/// The words that tell the sending agent why its message to `to` was refused.
fn refused(refusal: Refusal, to: &str) -> String {
    match refusal {
        Refusal::Network => t!("bridge.answer.network", tab = to),
        Refusal::Chain => {
            let most = crate::bridge::rules::MOST_HOPS;
            t!("bridge.answer.chain", next = most + 1, most = most)
        }
        Refusal::Pace => t!(
            "bridge.answer.pace",
            most = crate::bridge::rules::MOST_PER_WINDOW,
            seconds = i64::try_from(crate::bridge::rules::RATE_WINDOW.as_secs()).unwrap_or(i64::MAX)
        ),
        Refusal::Denied => t!("bridge.answer.denied", tab = to),
    }
}

/// Asks the person whether `from` may send to `to`, showing the first message.
fn ask(from_name: &str, to_name: &str, text: &str, from: TabKey, to: TabKey) -> Command<Msg> {
    let mut preview: String = text.chars().take(PREVIEW_CHARS).collect();
    if text.chars().count() > PREVIEW_CHARS {
        preview.push('…');
    }
    Command::confirm(
        Confirm::new(t!("bridge.ask.title", from = from_name, to = to_name), Msg::Allow { from, to, allow: true })
            .message(t!("bridge.ask.message", text = preview))
            .confirm_label(t!("bridge.ask.allow"))
            .cancel_label(t!("bridge.ask.deny"))
            .on_cancel(Msg::Allow { from, to, allow: false }),
    )
}

/// Says after [`ASK_WAIT`] that the question about `from` sending to `to` has waited long.
fn remind(from: TabKey, to: TabKey) -> Command<Msg> {
    Command::task(Task::new(t!("bridge.asking"), move |cx| {
        if cx.sleep(ASK_WAIT) { Ok(Msg::StillAsking { from, to }) } else { Err(String::new()) }
    }))
}

/// Takes the person's answer about `from` sending to `to`: it holds for the pair until QCode
/// closes, and every message waiting on it goes on or is refused.
pub(super) fn allowed(screen: &mut WorkspaceScreen, from: TabKey, to: TabKey, allow: bool) {
    screen.rules.decide(from.0, to.0, if allow { Decision::Allowed } else { Decision::Denied });
    let Some(position) = screen.asking.iter().position(|asking| asking.from == from && asking.to == to) else {
        return;
    };
    let asking = screen.asking.remove(position);
    let Some(workspace) = screen.owner(to) else {
        for waiting in asking.waiting {
            if let Some(call) = waiting.call {
                call.answer(Answer::refused(t!("bridge.answer.gone")));
            }
        }
        return;
    };
    let (sender_name, receiver_name) = (named(workspace, from), named(workspace, to));
    let now = Instant::now();
    for waiting in asking.waiting {
        if allow {
            queue(screen, to, sender_name.clone(), waiting.text, waiting.hop, now);
        }
        if let Some(call) = waiting.call {
            call.answer(if allow {
                Answer::done(t!("bridge.answer.queued", tab = receiver_name.as_str()))
            } else {
                Answer::refused(refused(Refusal::Denied, &receiver_name))
            });
        }
    }
}

/// Tells the senders of messages that have waited [`ASK_WAIT`] on the person's answer about
/// `from` sending to `to` that they still wait. The messages themselves keep waiting.
pub(super) fn still_asking(screen: &mut WorkspaceScreen, from: TabKey, to: TabKey) {
    let Some(asking) = screen.asking.iter_mut().find(|asking| asking.from == from && asking.to == to) else {
        return;
    };
    for waiting in &mut asking.waiting {
        if let Some(call) = waiting.call.take() {
            call.answer(Answer::done(t!("bridge.answer.asking")));
        }
    }
}

/// Leaves `text` from `from` waiting in the tab `to`, as step `hop` of its chain.
fn queue(screen: &mut WorkspaceScreen, to: TabKey, from: String, text: String, hop: u32, now: Instant) {
    screen.rules.received(to.0, hop, now);
    if let Some((_, tab)) = screen.owner_mut(to).and_then(|workspace| workspace.find(to)) {
        tab.receive(Letter { from, text });
    }
}

/// Forgets the tabs of `keys`, which were closed: the rules drop them, and a message waiting on
/// the person's answer to or from one of them is refused.
pub(super) fn closed(screen: &mut WorkspaceScreen, keys: &[TabKey]) {
    for key in keys {
        screen.rules.forget(key.0);
    }
    let (gone, kept): (Vec<Asking>, Vec<Asking>) = std::mem::take(&mut screen.asking)
        .into_iter()
        .partition(|asking| keys.contains(&asking.from) || keys.contains(&asking.to));
    screen.asking = kept;
    for asking in gone {
        for waiting in asking.waiting {
            if let Some(call) = waiting.call {
                call.answer(Answer::refused(t!("bridge.answer.gone")));
            }
        }
    }
}

/// Shows, hides or throws away the letters waiting in the tab `key`.
pub(super) fn letters(screen: &mut WorkspaceScreen, key: TabKey, action: Letters) {
    if let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) {
        match action {
            Letters::Show(open) => tab.show_letters(open),
            Letters::Discard => tab.discard_letters(),
        }
    }
}

/// The words for a harness whose settings the bridge could not be registered in.
pub(super) fn unregistered(harness: HarnessKind, trouble: &Unregistered) -> Command<Msg> {
    let body = match trouble {
        Unregistered::Taken(file) => t!("bridge.unregistered.taken", file = file.as_str()),
        Unregistered::Unreadable(file, place) => {
            t!("bridge.unregistered.unreadable", file = file.as_str(), place = place.as_str())
        }
        Unregistered::Engine(words) => words.clone(),
    };
    let title = t!("bridge.unregistered.title", harness = harness.record().display_name);
    Command::toast(Toast::warning(title).body(body))
}

/// The line under a tab that says which messages wait in it, with the way to read them and the
/// way to throw them away; with the letters themselves above it when they are shown.
pub(super) fn view(tab: &Tab, ui: &mut View<'_, Msg>) {
    let letters = tab.letters();
    let Some(last) = letters.last() else { return };
    let key = tab.key();
    ui.column(|ui| {
        if tab.letters_shown() {
            let height = u16::try_from(letters.iter().map(|letter| letter.text.lines().count() + 2).sum::<usize>())
                .unwrap_or(LETTER_ROWS)
                .min(LETTER_ROWS);
            ui.add_with(ScrollView::new(), |ui| {
                ui.column(|ui| {
                    for letter in letters {
                        ui.add(Text::new(t!("bridge.letter.from", from = letter.from.as_str())).role("faint"));
                        ui.add(Text::new(letter.text.clone())).selectable(true).fill_width();
                    }
                })
                .gap(1)
                .padding(Padding::symmetric(0, 2))
                .fill_width();
            })
            .height(Length::Cells(height))
            .fill_width()
            .id("workspace-letters");
        }
        ui.row(|ui| {
            // The words give way before the buttons do: a narrow tab cuts the sentence short and
            // keeps both ways out whole.
            ui.add(
                Text::new(t!("bridge.waiting", n = letters.len(), from = last.from.as_str()))
                    .role("secondary")
                    .no_wrap(),
            )
            .fill_width();
            let (label, open) = if tab.letters_shown() {
                (t!("bridge.letters.hide"), false)
            } else {
                (t!("bridge.letters.show"), true)
            };
            ui.add(Button::new(label).on_press(Msg::Letters(key, Letters::Show(open)))).id("workspace-letters-show");
            ui.add(Button::new(t!("bridge.letters.discard")).on_press(Msg::Letters(key, Letters::Discard)))
                .id("workspace-letters-discard");
        })
        .gap(2)
        .padding(Padding { left: 2, right: 1, ..Padding::default() })
        .fill_width()
        .id("workspace-waiting-letters");
    })
    .gap(1)
    .fill_width();
}
