//! The bridge between tabs on the workspace screen: each open workspace listens on its socket, the
//! calls of its tabs' agents are answered here by the rules, and a message that is taken waits in
//! the tab it was sent to.
//!
//! Tabs send each other messages without asking the person, unless the person turned asking on
//! in the settings; then the first message between two tabs asks. Either way the rules that do
//! not need the person hold: a tab without the network never sends to one with it, and an
//! exchange that goes on too long is ended. An ended exchange is not ended silently: both tabs
//! say so under their terminal, and the person is told once wherever they are looking.
//!
//! A taken message is typed into the receiving harness itself, as soon as that tab is quiet:
//! the person is not typing in it and its program has stopped writing. Until then, and whenever
//! the harness is not running to be typed into, the message waits in the tab, where the person
//! sees who sent it and can read it. A message is never dropped on the way: the sending agent is
//! told whether its message went in or still waits, and a later list says so again, so an agent
//! cannot believe it handed work over when it did not.

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

/// How long the person's typing and the harness's own output must both have been quiet before a
/// message is typed into a tab. Generous on purpose: a shorter wait buys no correctness and cuts
/// a person's half-written line in half on a loaded machine.
pub(super) const QUIET: Duration = Duration::from_secs(2);

/// How often a tab that still holds messages is looked at again. Nothing is timed while no tab
/// holds one.
pub(super) const RETRY_EVERY: Duration = Duration::from_secs(1);

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

/// Why the messages waiting in a tab have not been typed into its harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undelivered {
    /// There is no program to type into: the tab's harness has ended, its container stopped, or
    /// it has not started yet. The tab's own restart brings it back and delivery goes on by
    /// itself.
    NotRunning,
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
    /// The person read that the tab's exchange was ended; stop saying so.
    Seen,
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

/// The tab of `workspace` whose token the question carries.
///
/// A window's tab counts here as much as a terminal's: the agent inside the window starts the
/// same server and speaks for its own tab. What a window cannot be is a destination, which is
/// [`others`]'s to say.
fn sender(workspace: &OpenWorkspace, question: &Question) -> Option<TabKey> {
    workspace.tabs.iter().find(|tab| tab.kind().profile().is_some() && tab.token() == question.token).map(Tab::key)
}

/// The profile of the tab `key`, when it belongs to one the store still has.
fn profile_of(workspace: &OpenWorkspace, key: TabKey) -> Option<&Profile> {
    let tab = workspace.tabs.iter().find(|tab| tab.key() == key)?;
    let name = tab.kind().profile()?;
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
///
/// A window is not among them, and must not be: it draws no terminal, so there is no prompt a
/// message could be typed into, and an agent that could leave one there would believe it had
/// handed work over when nothing had been said to anyone.
fn others(workspace: &OpenWorkspace, sender: TabKey) -> Vec<(usize, &Tab, &Profile)> {
    workspace
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.key() != sender && matches!(tab.kind(), TabKind::Profile(_)))
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
            waiting: tab.letters().len(),
            trouble: tab.undelivered().filter(|_| !tab.letters().is_empty()).map(trouble),
        })
        .collect();
    if tabs.is_empty() {
        return Answer::listed(t!("bridge.answer.alone"), tabs);
    }
    let mut text = t!("bridge.answer.listed", n = tabs.len());
    for tab in &tabs {
        let network = if tab.network { t!("bridge.answer.online") } else { t!("bridge.answer.offline") };
        text.push('\n');
        // A tab with messages still in it says so in the same line: an agent that only reads the
        // words, and never the fields, still learns that its message has not arrived.
        let line = if tab.waiting == 0 {
            t!(
                "bridge.answer.tab",
                tab = tab.tab.as_str(),
                title = tab.title.as_str(),
                harness = tab.harness.as_str(),
                network = network
            )
        } else {
            let waiting = match &tab.trouble {
                Some(trouble) => t!("bridge.answer.waiting-stuck", n = tab.waiting, trouble = trouble.as_str()),
                None => t!("bridge.answer.waiting", n = tab.waiting),
            };
            t!(
                "bridge.answer.tab-waiting",
                tab = tab.tab.as_str(),
                title = tab.title.as_str(),
                harness = tab.harness.as_str(),
                network = network,
                waiting = waiting
            )
        };
        text.push_str(&line);
    }
    Answer::listed(text, tabs)
}

/// The words that tell an agent why the messages waiting in a tab have not been typed in.
fn trouble(why: Undelivered) -> String {
    match why {
        Undelivered::NotRunning => t!("bridge.answer.not-running"),
    }
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
            if refusal == Refusal::Chain {
                return ended(screen, (from, sender_name), (to, receiver_name));
            }
            return Command::none();
        }
    };
    // Without asking, a pair nobody was asked about is taken as allowed. What the person did
    // answer while asking was on still holds: a pair they denied stays denied until QCode closes.
    let decision = match screen.rules.decision(from.0, to.0) {
        None if !screen.ask_first => Some(Decision::Allowed),
        decision => decision,
    };
    match decision {
        Some(Decision::Denied) => {
            call.answer(Answer::refused(refused(Refusal::Denied, &receiver_name)));
            Command::none()
        }
        Some(Decision::Allowed) => {
            screen.rules.sent(from.0, now);
            queue(screen, to, sender_name, text, hop, now);
            // Typed in before the sender is answered, so the answer says what really happened
            // rather than what is about to be tried.
            let delivering = deliver(screen, now);
            call.answer(taken(screen, to, &receiver_name));
            delivering
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

/// Says that the exchange between `from` and `to`, each with the name the person knows it by,
/// was ended by the loop limit: under both tabs, until the person has read it, and once in a
/// notice wherever the person is looking, because neither tab may be the one on screen.
///
/// An agent that keeps trying is refused each time; the notice is not given again for that.
fn ended(screen: &mut WorkspaceScreen, from: (TabKey, String), to: (TabKey, String)) -> Command<Msg> {
    let ((from, from_name), (to, to_name)) = (from, to);
    let mut new = false;
    for (key, other) in [(from, &to_name), (to, &from_name)] {
        if let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) {
            new |= tab.stopped() != Some(other.as_str());
            tab.stop(other.clone());
        }
    }
    if !new {
        return Command::none();
    }
    let most = crate::bridge::rules::MOST_HOPS;
    let toast = Toast::warning(t!("bridge.stopped.title", from = from_name.as_str(), to = to_name.as_str()))
        .body(t!("bridge.stopped.why", most = most));
    Command::toast(toast)
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
pub(super) fn allowed(screen: &mut WorkspaceScreen, from: TabKey, to: TabKey, allow: bool) -> Command<Msg> {
    screen.rules.decide(from.0, to.0, if allow { Decision::Allowed } else { Decision::Denied });
    let Some(position) = screen.asking.iter().position(|asking| asking.from == from && asking.to == to) else {
        return Command::none();
    };
    let asking = screen.asking.remove(position);
    let Some(workspace) = screen.owner(to) else {
        for waiting in asking.waiting {
            if let Some(call) = waiting.call {
                call.answer(Answer::refused(t!("bridge.answer.gone")));
            }
        }
        return Command::none();
    };
    let (sender_name, receiver_name) = (named(workspace, from), named(workspace, to));
    let now = Instant::now();
    let mut commands = Vec::new();
    for waiting in asking.waiting {
        if allow {
            queue(screen, to, sender_name.clone(), waiting.text, waiting.hop, now);
            commands.push(deliver(screen, now));
        }
        if let Some(call) = waiting.call {
            call.answer(if allow {
                taken(screen, to, &receiver_name)
            } else {
                Answer::refused(refused(Refusal::Denied, &receiver_name))
            });
        }
    }
    Command::batch(commands)
}

/// The answer a sender is given once its message has been taken: typed into `to` already, or
/// still waiting there, with what is stopping it when something is.
fn taken(screen: &WorkspaceScreen, to: TabKey, name: &str) -> Answer {
    let Some(tab) = screen.owner(to).and_then(|workspace| workspace.tabs.iter().find(|tab| tab.key() == to)) else {
        return Answer::refused(t!("bridge.answer.gone"));
    };
    if tab.letters().is_empty() {
        return Answer::done(t!("bridge.answer.delivered", tab = name));
    }
    match tab.undelivered() {
        Some(why) => Answer::done(t!("bridge.answer.queued-stuck", tab = name, trouble = trouble(why))),
        None => Answer::done(t!("bridge.answer.queued", tab = name)),
    }
}

/// Types the oldest message waiting in every tab into its harness, as far as the tabs allow it
/// now, and asks to be called again while any message is still waiting. Nothing is timed while
/// no tab holds a message.
pub(super) fn deliver(screen: &mut WorkspaceScreen, now: Instant) -> Command<Msg> {
    let mut waiting = false;
    for workspace in &mut screen.workspaces {
        for tab in &mut workspace.tabs {
            hand_over(tab, now);
            waiting |= !tab.letters().is_empty();
        }
    }
    if !waiting || screen.delivering {
        return Command::none();
    }
    screen.delivering = true;
    Command::task(Task::new(t!("bridge.delivering"), move |cx| {
        if cx.sleep(RETRY_EVERY) { Ok(Msg::Deliver) } else { Err(String::new()) }
    }))
}

/// Types the oldest message waiting in `tab` into its harness, when the tab has a session, the
/// person has not been typing in it and the program has not been writing. A message that cannot
/// go in stays where it is and the tab keeps why, because dropping it would leave the agent that
/// sent it believing its work had been handed over.
fn hand_over(tab: &mut Tab, now: Instant) {
    let Some(letter) = tab.letters().first().cloned() else { return };
    let Some(session) = tab.session().cloned() else {
        tab.not_delivered(Undelivered::NotRunning);
        return;
    };
    if now.saturating_duration_since(session.last_input()) < QUIET
        || now.saturating_duration_since(session.last_output()) < QUIET
    {
        return;
    }
    let text = t!("bridge.handed", from = letter.from.as_str(), text = letter.text.as_str());
    // Enter is written rather than pasted: inside a paste it would be a line break in the text
    // instead of the person's own Return, and nothing would be sent off.
    match session.paste(&text).and_then(|()| session.write(b"\r")) {
        Ok(()) => tab.delivered(),
        Err(_) => tab.not_delivered(Undelivered::NotRunning),
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
            Letters::Seen => tab.unstop(),
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

/// What the bridge shows under a tab: that the loop limit ended its exchange with another tab,
/// and which messages wait in it.
pub(super) fn view(tab: &Tab, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        stopped(tab, ui);
        waiting(tab, ui);
    })
    .gap(1)
    .fill_width();
}

/// The line under a tab whose exchange with another the loop limit ended, with the way to say it
/// was read.
fn stopped(tab: &Tab, ui: &mut View<'_, Msg>) {
    let Some(other) = tab.stopped() else { return };
    let most = crate::bridge::rules::MOST_HOPS;
    ui.row(|ui| {
        let line = t!("bridge.stopped.line", tab = other, most = most);
        ui.add(Text::new(line).role("secondary")).fill_width();
        ui.add(Button::new(t!("bridge.stopped.seen")).on_press(Msg::Letters(tab.key(), Letters::Seen)))
            .id("workspace-stopped-seen");
    })
    .gap(2)
    .padding(Padding { left: 2, right: 1, ..Padding::default() })
    .fill_width()
    .id("workspace-stopped");
}

/// The line under a tab that says which messages wait in it, with the way to read them and the
/// way to throw them away; with the letters themselves above it when they are shown.
fn waiting(tab: &Tab, ui: &mut View<'_, Msg>) {
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
            let line = match tab.undelivered() {
                Some(Undelivered::NotRunning) => {
                    t!("bridge.waiting-stuck", n = letters.len(), from = last.from.as_str())
                }
                None => t!("bridge.waiting", n = letters.len(), from = last.from.as_str()),
            };
            ui.add(Text::new(line).role("secondary").no_wrap()).fill_width();
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
