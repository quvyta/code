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
//! A taken message is typed into the receiving harness itself, as soon as that tab is quiet: the
//! person has no line started and not sent in it, and its program has stopped writing. Until then, and
//! whenever the harness is not running to be typed into, the message waits in the tab, where the person
//! sees who sent it and can read it. A message is never dropped on the way: the sending agent is
//! told whether its message went in or still waits, and a later list says so again, so an agent
//! cannot believe it handed work over when it did not.
//!
//! A window's tab has no prompt to type into. A message to it waits in the tab until the agent
//! inside the window asks for its inbox, which hands over every waiting message at once; the
//! sender is told that it waits for that, not that it went in.

use std::time::{Duration, Instant};

use qframe::prelude::*;
use qframe::runtime::{Confirm, Task};
use qframe::widgets::{ScrollView, TerminalSession, Toast};

use crate::bridge::config::Unregistered;
use crate::bridge::protocol::{Answer, Kind, Listed, MOST_TEXT, Outcome, Question, Received, Request, To, You};
use crate::bridge::rules::{Decision, End, Refusal};
use crate::bridge::socket::{Call, Inbox, Listener};
use crate::profile::{HarnessKind, NetworkMode, Profile};

use super::{Msg, OpenWorkspace, Tab, TabKey, TabKind, WorkspaceScreen};

/// How long a message waits for the person's answer before its sender is told that it waits.
/// Well inside the minute some harnesses give a tool before they give up on it.
pub(super) const ASK_WAIT: Duration = Duration::from_secs(45);

/// How long the harness's own output must have been quiet before a message is typed into its
/// tab. Generous on purpose: a shorter wait buys no correctness and types into a tool that is still
/// drawing on a loaded machine.
pub(super) const QUIET: Duration = Duration::from_secs(2);

/// Whether the program in `session` has written within [`QUIET`] of `now`. The bridge waits for it
/// to stop before typing a message in, and the tab strip marks the tab as working while it has not.
pub(super) fn speaking(session: &TerminalSession, now: Instant) -> bool {
    now.saturating_duration_since(session.last_output()) < QUIET
}

/// How long after a harness has taken a pasted message its Return is written.
///
/// Gemini CLI 0.61 reads a Return that comes within 40 ms of a paste as a new line inside the
/// pasted text, not as sending it (its prompt guards against a paste whose own line breaks would
/// send it half-written, unless the terminal speaks the kitty keyboard protocol), and any Return
/// within 30 ms of the last typed key the same way. A Return written straight after the paste
/// sent 3 messages of 20 into a real Gemini CLI; the rest stood in its prompt until somebody
/// pressed Return. The window starts when the harness handles the paste, which only the harness
/// knows, so this is counted from the moment it shows that it has: its first output after the
/// paste ([`PASTE_TAKEN`]). Two and a half times the longer window, so a loaded machine drawing
/// late still falls outside it; short enough that nobody waits on it.
pub(super) const AFTER_PASTE: Duration = Duration::from_millis(100);

/// How long a harness is given to show that it took a paste before the Return is written anyway.
/// A line editor draws the pasted text at once; a program that never echoes (a plain `cat`)
/// still gets its Return, [`AFTER_PASTE`] after this.
pub(super) const PASTE_TAKEN: Duration = Duration::from_millis(500);

/// A message written into a tab's prompt, and the Return that prompt is still owed.
///
/// Both waits are counted from `at`, the moment the paste went in, and `before` is what the harness
/// had drawn up to then: the first thing it draws afterwards is how it says it took the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pasted {
    /// When the paste was written into the prompt.
    at: Instant,
    /// The harness's last output before the paste.
    before: Instant,
    /// The message as it was written in, kept so that a harness which ends before its Return has
    /// the message put back in the tab rather than lost with its prompt.
    letter: Letter,
}

impl Pasted {
    /// Whether the Return for this paste is due at `now`, on the rule [`due`] gives.
    fn due(&self, session: &TerminalSession, now: Instant) -> bool {
        due(session, self.at, self.before, now)
    }

    /// The message as it was written into the prompt, for a tab that has to have it back.
    fn letter(&self) -> Letter {
        self.letter.clone()
    }
}

/// Whether the Return for a paste written at `at` into `session`, which had drawn nothing since
/// `before`, is due at `now`: once the harness has shown that it took the paste and [`AFTER_PASTE`]
/// has passed, or [`PASTE_TAKEN`] and [`AFTER_PASTE`] together when it shows nothing at all, as a
/// plain `cat` does not.
///
/// This is the whole rule, the screen's and [`type_in`]'s alike, so that neither can drift from
/// the other.
fn due(session: &TerminalSession, at: Instant, before: Instant, now: Instant) -> bool {
    let waited = now.saturating_duration_since(at);
    if session.last_output() > before { waited >= AFTER_PASTE } else { waited >= PASTE_TAKEN + AFTER_PASTE }
}

/// Writes the message `letter` into `session` as the one paste a handover is made in, and says when
/// it went in and what the harness had drawn before it: the two things [`due`] reads.
fn paste_in(session: &TerminalSession, letter: Letter) -> std::io::Result<Pasted> {
    let before = session.last_output();
    session.paste(&handed(&letter))?;
    Ok(Pasted { at: Instant::now(), before, letter })
}

/// Types `text` into `session` the way a message is handed to a harness: as one paste, then a
/// Return once the harness has taken the paste ([`AFTER_PASTE`]).
///
/// The Return is written rather than pasted: inside a paste it would be a line break in the text
/// instead of the person's own Return, and nothing would be sent off. Waits at most
/// [`PASTE_TAKEN`] and [`AFTER_PASTE`] together, on the calling thread, on the rule [`due`] gives,
/// which is the same one the screen keeps its own waits to.
///
/// The screen does not call this: it writes that Return from its own timer instead, and leaves
/// the wait to the tab ([`Pasted`]). This is the way the live tests hand a message to a real
/// harness, whose thread is nothing but the wait.
///
/// # Errors
///
/// The session's own error when the program no longer reads its input.
#[cfg(test)]
pub(crate) fn type_in(session: &TerminalSession, text: &str) -> std::io::Result<()> {
    let before = session.last_output();
    session.paste(text)?;
    let at = Instant::now();
    while !due(session, at, before, Instant::now()) {
        std::thread::sleep(RETURN_EVERY);
    }
    session.write(b"\r")
}

/// How often a tab that owes a Return is looked at again. Its wait ends within [`PASTE_TAKEN`] and
/// [`AFTER_PASTE`] of the paste, so a look has to be finer than that for the Return to go as soon
/// as the rule allows; a look is one message and nothing else, and the screen is free between them.
pub(super) const RETURN_EVERY: Duration = Duration::from_millis(50);

/// Looks again at the Return the tab `key` is owed, once [`RETURN_EVERY`] has passed. Every tab's
/// wait is its own, so a tab that is owed a Return is looked at by its own timer and no other.
fn owed_again(key: TabKey) -> Command<Msg> {
    Command::task(Task::new(t!("bridge.delivering"), move |cx| {
        if cx.sleep(RETURN_EVERY) { Ok(Msg::Return(key)) } else { Err(String::new()) }
    }))
}

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
    /// Who sent it, as the person reads it under the tab: the harness and the title of the
    /// sending tab.
    pub from: String,
    /// The id of the sending tab inside the workspace, the one to answer to.
    pub tab: u32,
    /// The title of the sending tab.
    pub title: String,
    /// The sending tab's harness, as it names itself.
    pub harness: String,
    /// What the sender asks of the receiver.
    pub kind: Kind,
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
    /// The tab is a window, with no prompt to type into: the messages wait until its agent asks
    /// for its inbox.
    Inbox,
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
    letter: Letter,
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
        Request::Send { to, text, kind } => send(screen, id, sender, &to, &text, kind, call, now),
        Request::Inbox => {
            call.answer(inbox(screen, sender));
            Command::none()
        }
        Request::Peek => {
            call.answer(peek(workspace, sender));
            Command::none()
        }
    }
}

/// The messages waiting in the tab `key`, left where they are: a window's watcher asks this to
/// know when to tell its agent to take them.
fn peek(workspace: &OpenWorkspace, key: TabKey) -> Answer {
    let Some(tab) = workspace.tabs.iter().find(|tab| tab.key() == key) else {
        return Answer::refused(t!("bridge.answer.stranger"));
    };
    let messages: Vec<Received> = tab.letters().iter().map(received).collect();
    if messages.is_empty() {
        return Answer::received(t!("bridge.answer.inbox-empty"), messages);
    }
    Answer::received(t!("bridge.answer.peek", n = messages.len()), messages)
}

/// Hands the tab `key` every message waiting in it, taking them out of the tab: the answer to its
/// agent asking for its inbox.
fn inbox(screen: &mut WorkspaceScreen, key: TabKey) -> Answer {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) else {
        return Answer::refused(t!("bridge.answer.stranger"));
    };
    let taken = tab.take_letters();
    if taken.is_empty() {
        return Answer::received(t!("bridge.answer.inbox-empty"), Vec::new());
    }
    let mut text = t!("bridge.answer.inbox", n = taken.len());
    for letter in &taken {
        text.push_str("\n\n");
        text.push_str(&handed(letter));
    }
    Answer::received(text, taken.iter().map(received).collect())
}

/// `letter` as an inbox answers it.
fn received(letter: &Letter) -> Received {
    Received { from: letter.from.clone(), tab: letter.tab.to_string(), kind: letter.kind, text: letter.text.clone() }
}

/// `letter` as its receiving agent reads it: two lines that say who sent it, what kind of message
/// it is and how to answer, then the message.
pub(super) fn handed(letter: &Letter) -> String {
    let tab = letter.tab.to_string();
    let kind = match letter.kind {
        Kind::Info => t!("bridge.kind.info"),
        Kind::Question => t!("bridge.kind.question"),
        Kind::Report => t!("bridge.kind.report"),
    };
    let reply = match letter.kind {
        Kind::Info => t!("bridge.reply.info", tab = tab.as_str()),
        Kind::Question => t!("bridge.reply.question", tab = tab.as_str()),
        Kind::Report => t!("bridge.reply.report", tab = tab.as_str()),
    };
    let header = t!(
        "bridge.header",
        tab = tab.as_str(),
        title = letter.title.as_str(),
        harness = letter.harness.as_str(),
        kind = kind
    );
    t!("bridge.handed", header = header, reply = reply, text = letter.text.as_str())
}

/// The message `text` of the kind `kind`, sent by the tab `from` of `workspace`.
fn letter(workspace: &OpenWorkspace, from: TabKey, kind: Kind, text: String) -> Letter {
    let index = workspace.tabs.iter().position(|tab| tab.key() == from);
    Letter {
        from: named(workspace, from),
        tab: index.map_or(0, |index| workspace.tabs[index].number()),
        title: index.map(|index| workspace.tab_label(index)).unwrap_or_default(),
        harness: profile_of(workspace, from)
            .map(|profile| profile.harness.record().display_name.to_owned())
            .unwrap_or_default(),
        kind,
        text,
    }
}

/// The tab of `workspace` whose token the question carries.
///
/// A window's tab counts here as much as a terminal's: the agent inside the window starts the
/// same server and speaks for its own tab. What a window cannot be is a destination, which is
/// [`others`]'s to say.
///
/// opencode's shared server asks for every tab of its profile with the server's own token, and
/// names the conversation the question came from: the sender is the tab of that profile showing
/// that conversation. A conversation alone names nobody, and neither does the token of another
/// profile's server, so an agent cannot speak for a tab by knowing what it shows.
fn sender(workspace: &OpenWorkspace, question: &Question) -> Option<TabKey> {
    if let Some(tab) = workspace.tabs.iter().find(|tab| tab.kind().profile().is_some() && tab.token() == question.token)
    {
        return Some(tab.key());
    }
    let session = question.session.as_deref()?;
    let profile = workspace.profiles.iter().find(|profile| {
        super::shared::shares(profile) && workspace.servers.token(profile.name.as_str()) == question.token
    })?;
    workspace
        .tabs
        .iter()
        .find(|tab| {
            matches!(tab.kind(), TabKind::Profile(name) if name == profile.name.as_str())
                && tab.conversation() == Some(session)
        })
        .map(Tab::key)
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

/// The other harness tabs of `workspace`, the ones `sender` can send to: the terminals, which
/// take a message into their prompt, and the windows, whose agent takes it from its inbox.
fn others(workspace: &OpenWorkspace, sender: TabKey) -> Vec<(usize, &Tab, &Profile)> {
    workspace
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.key() != sender && matches!(tab.kind(), TabKind::Profile(_) | TabKind::Desktop(_)))
        .filter_map(|(index, tab)| profile_of(workspace, tab.key()).map(|profile| (index, tab, profile)))
        .collect()
}

/// The answer to a list: the asking tab itself, then every other harness tab of the workspace.
fn list(workspace: &OpenWorkspace, sender: TabKey) -> Answer {
    let index = workspace.tabs.iter().position(|tab| tab.key() == sender);
    let profile = profile_of(workspace, sender);
    let you = You {
        tab: index.map(|index| workspace.tabs[index].number().to_string()).unwrap_or_default(),
        title: index.map(|index| workspace.tab_label(index)).unwrap_or_default(),
        harness: profile.map(|profile| profile.harness.record().display_name.to_owned()).unwrap_or_default(),
        profile: profile.map(|profile| profile.name.as_str().to_owned()).unwrap_or_default(),
        workspace: workspace.name.clone(),
    };
    let mut text =
        t!("bridge.answer.you", tab = you.tab.as_str(), title = you.title.as_str(), workspace = you.workspace.as_str());
    text.push('\n');
    let tabs: Vec<Listed> = others(workspace, sender)
        .into_iter()
        .map(|(index, tab, profile)| Listed {
            tab: tab.number().to_string(),
            title: workspace.tab_label(index),
            harness: profile.harness.record().display_name.to_owned(),
            profile: profile.name.as_str().to_owned(),
            network: profile.network == NetworkMode::Full,
            inbox: by_inbox(tab),
            waiting: tab.letters().len(),
            trouble: tab.undelivered().filter(|_| !tab.letters().is_empty()).map(trouble),
        })
        .collect();
    if tabs.is_empty() {
        text.push_str(&t!("bridge.answer.alone"));
        return Answer::listed(text, you, tabs);
    }
    text.push_str(&t!("bridge.answer.listed", n = tabs.len()));
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
        // An agent that only reads the words still learns that this one reads no prompt.
        if tab.inbox {
            text.push_str(&t!("bridge.answer.by-inbox"));
        }
    }
    Answer::listed(text, you, tabs)
}

/// Whether `tab` takes its messages only through its agent's inbox: a window, which has no prompt.
fn by_inbox(tab: &Tab) -> bool {
    matches!(tab.kind(), TabKind::Desktop(_))
}

/// The words that tell an agent why the messages waiting in a tab have not been typed in.
fn trouble(why: Undelivered) -> String {
    match why {
        Undelivered::NotRunning => t!("bridge.answer.not-running"),
        Undelivered::Inbox => t!("bridge.answer.not-read"),
    }
}

/// The tab `wanted` names among the other harness tabs of `workspace`: by its id, or by its title
/// when exactly one tab has it.
fn target(workspace: &OpenWorkspace, sender: TabKey, wanted: &str) -> Option<TabKey> {
    let others = others(workspace, sender);
    let wanted = wanted.trim();
    if let Some((_, tab, _)) = others.iter().find(|(_, tab, _)| tab.number().to_string() == wanted) {
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

/// Handles a message of the kind `kind` from `from` to `to` in the workspace `id`: to one tab, or
/// to every other agent tab, each of which it reaches by the same rules as a message to it alone.
#[expect(clippy::too_many_arguments, reason = "a call carries all of these, and each is used once")]
fn send(
    screen: &mut WorkspaceScreen,
    id: &str,
    from: TabKey,
    to: &To,
    text: &str,
    kind: Kind,
    call: Call,
    now: Instant,
) -> Command<Msg> {
    let Some(workspace) = screen.workspaces.iter().find(|workspace| workspace.id.as_str() == id) else {
        return Command::none();
    };
    let targets: Vec<TabKey> = match to {
        To::Tab(wanted) => {
            let Some(target) = target(workspace, from, wanted) else {
                call.answer(Answer::refused(t!("bridge.answer.no-tab", tab = wanted.as_str())));
                return Command::none();
            };
            vec![target]
        }
        To::All => others(workspace, from).into_iter().map(|(_, tab, _)| tab.key()).collect(),
    };
    if targets.is_empty() {
        call.answer(Answer::refused(t!("bridge.answer.alone")));
        return Command::none();
    }
    if text.trim().is_empty() {
        call.answer(Answer::refused(t!("bridge.answer.empty")));
        return Command::none();
    }
    if text.chars().count() > MOST_TEXT {
        call.answer(Answer::refused(t!("bridge.answer.long", most = MOST_TEXT)));
        return Command::none();
    }
    // The person gave the sending tab something since it was last handed a message: what it sends
    // now is their task, not its answer to another agent, and starts an exchange of its own. Without
    // this, a tab the person keeps giving work runs into the loop limit of answers it sent long ago.
    if workspace.tabs.iter().any(|tab| tab.key() == from && tab.person_typed()) {
        screen.rules.new_chain(from.0);
    }
    // How each tab is known to the agent, taken before anything is handed over.
    let named: Vec<(TabKey, String, String)> = targets
        .iter()
        .filter_map(|&key| {
            let index = workspace.tabs.iter().position(|tab| tab.key() == key)?;
            Some((key, workspace.tabs[index].number().to_string(), workspace.tab_label(index)))
        })
        .collect();
    let mut call = Some(call);
    let single = matches!(to, To::Tab(_));
    let mut commands = Vec::new();
    let mut counted = false;
    let mut sent = Vec::new();
    for (key, number, title) in named {
        // A message to every tab is answered at once, tab by tab: a tab that waits for the
        // person's answer is said to wait rather than holding the answer about all the others.
        let mut parked = None;
        let offered = offer(screen, from, key, text, kind, now, if single { &mut call } else { &mut parked });
        commands.push(offered.command);
        counted |= offered.counts;
        if let Some(answer) = offered.answer {
            sent.push(Outcome { tab: number, title, ok: answer.ok, text: answer.text });
        }
    }
    // However many tabs a message went to, the sender sent one message.
    if counted {
        screen.rules.sent(from.0, now);
    }
    if let Some(call) = call {
        let answer = if single {
            sent.pop().map_or_else(
                || Answer::refused(t!("bridge.answer.gone")),
                |outcome| {
                    if outcome.ok { Answer::done(outcome.text) } else { Answer::refused(outcome.text) }
                },
            )
        } else {
            let mut text = t!("bridge.answer.all", n = sent.len());
            for outcome in &sent {
                text.push('\n');
                text.push_str(&t!(
                    "bridge.answer.all-tab",
                    tab = outcome.tab.as_str(),
                    title = outcome.title.as_str(),
                    what = outcome.text.as_str()
                ));
            }
            Answer::broadcast(text, sent)
        };
        call.answer(answer);
    }
    Command::batch(commands)
}

/// What became of a message offered to one tab.
struct Offered {
    /// The answer about that tab; `None` when the call was kept to be answered once the person
    /// has answered.
    answer: Option<Answer>,
    /// Whether the message counts against the sender's pace: it was taken, or waits for the
    /// person.
    counts: bool,
    /// What has to run for it.
    command: Command<Msg>,
}

/// Offers the message `text` of the kind `kind` from `from` to the tab `to`, by the rules. When
/// it has to wait for the person's answer, `call` is kept with it to be answered then, when there
/// is one; without one the answer says it waits.
fn offer(
    screen: &mut WorkspaceScreen,
    from: TabKey,
    to: TabKey,
    text: &str,
    kind: Kind,
    now: Instant,
    call: &mut Option<Call>,
) -> Offered {
    let answered = |answer: Answer| Offered { answer: Some(answer), counts: false, command: Command::none() };
    let Some(workspace) = screen.owner(to) else {
        return answered(Answer::refused(t!("bridge.answer.gone")));
    };
    let (Some(from_profile), Some(to_profile)) = (profile_of(workspace, from), profile_of(workspace, to)) else {
        return answered(Answer::refused(t!("bridge.answer.no-tab", tab = named(workspace, to))));
    };
    let ends = (
        End { tab: from.0, network: from_profile.network == NetworkMode::Full },
        End { tab: to.0, network: to_profile.network == NetworkMode::Full },
    );
    let (sender_name, receiver_name) = (named(workspace, from), named(workspace, to));
    let letter = letter(workspace, from, kind, text.to_owned());
    let hop = match screen.rules.check(ends.0, ends.1, now) {
        Ok(hop) => hop,
        Err(refusal) => {
            let answer = Answer::refused(refused(refusal, &receiver_name));
            if refusal == Refusal::Chain {
                let command = ended(screen, (from, sender_name), (to, receiver_name));
                return Offered { answer: Some(answer), counts: false, command };
            }
            return answered(answer);
        }
    };
    // Without asking, a pair nobody was asked about is taken as allowed. What the person did
    // answer while asking was on still holds: a pair they denied stays denied until QCode closes.
    let decision = match screen.rules.decision(from.0, to.0) {
        None if !screen.ask_first => Some(Decision::Allowed),
        decision => decision,
    };
    match decision {
        Some(Decision::Denied) => answered(Answer::refused(refused(Refusal::Denied, &receiver_name))),
        Some(Decision::Allowed) => {
            queue(screen, to, letter, hop, now);
            // Typed in before the sender is answered, so the answer says what really happened
            // rather than what is about to be tried.
            let command = deliver(screen, now);
            Offered { answer: Some(taken(screen, to, &receiver_name)), counts: true, command }
        }
        None => {
            let waiting = Waiting { call: call.take(), letter, hop };
            let answer = waiting.call.is_none().then(|| Answer::done(t!("bridge.answer.asking")));
            if let Some(asking) = screen.asking.iter_mut().find(|asking| asking.from == from && asking.to == to) {
                asking.waiting.push(waiting);
                return Offered { answer, counts: true, command: Command::none() };
            }
            let command = Command::batch([ask(&sender_name, &receiver_name, text, from, to), remind(from, to)]);
            screen.asking.push(Asking { from, to, waiting: vec![waiting] });
            Offered { answer, counts: true, command }
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
    let receiver_name = named(workspace, to);
    let now = Instant::now();
    let mut commands = Vec::new();
    for waiting in asking.waiting {
        if allow {
            queue(screen, to, waiting.letter, waiting.hop, now);
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
        Some(Undelivered::Inbox) => Answer::done(t!("bridge.answer.queued-inbox", tab = name)),
        Some(why) => Answer::done(t!("bridge.answer.queued-stuck", tab = name, trouble = trouble(why))),
        None => Answer::done(t!("bridge.answer.queued", tab = name)),
    }
}

/// Types the oldest message waiting in every tab into its harness, as far as the tabs allow it
/// now, and asks to be called again while any message is still waiting. Nothing is timed while
/// no tab holds a message.
pub(super) fn deliver(screen: &mut WorkspaceScreen, now: Instant) -> Command<Msg> {
    let mut commands = Vec::new();
    let mut waiting = false;
    for workspace in &mut screen.workspaces {
        for tab in &mut workspace.tabs {
            commands.push(hand_over(tab, now));
            // A window's messages wait for its agent, not for a moment of quiet, so nothing is
            // timed for them.
            waiting |= !tab.letters().is_empty() && !by_inbox(tab);
        }
    }
    if !waiting || screen.delivering {
        // What a paste is owed is timed by the tab itself, whether or not another look at a
        // waiting message is already set going, so those commands go out either way.
        return Command::batch(commands);
    }
    screen.delivering = true;
    commands.push(Command::task(Task::new(t!("bridge.delivering"), move |cx| {
        if cx.sleep(RETRY_EVERY) { Ok(Msg::Deliver) } else { Err(String::new()) }
    })));
    Command::batch(commands)
}

/// Types the oldest message waiting in `tab` into its harness, when the tab has a session, the
/// person has not been typing in it and the program has not been writing. A message that cannot
/// go in stays where it is and the tab keeps why, because dropping it would leave the agent that
/// sent it believing its work had been handed over.
///
/// The paste goes in at once and the tab is left owing its Return, which [`returned`] writes from
/// the screen's own timer. Waiting here for the harness to take the paste would hold the whole
/// screen still for as long as the wait lasts, and a message to every tab would hold it once per
/// tab: a person cannot type, and no tab can be closed, while a message is on its way.
fn hand_over(tab: &mut Tab, now: Instant) -> Command<Msg> {
    let Some(letter) = tab.letters().first().cloned() else { return Command::none() };
    if by_inbox(tab) {
        tab.not_delivered(Undelivered::Inbox);
        return Command::none();
    }
    // A message whose Return has not gone yet is still in that prompt: a second one would join it
    // in a single line, and the harness would read both as one message.
    if tab.owed().is_some() {
        return Command::none();
    }
    let Some(session) = tab.session().cloned() else {
        tab.not_delivered(Undelivered::NotRunning);
        return Command::none();
    };
    // A line the person started and has not sent holds every message back, however long they
    // pause over it: a message typed in then joins their line and goes out with their Return
    // (seen in the endurance trial, 2026-09-24). Once they send or clear it, the usual quiet is
    // enough.
    if session.line_pending() || now.saturating_duration_since(session.last_input()) < QUIET || speaking(&session, now)
    {
        return Command::none();
    }
    match paste_in(&session, letter) {
        Ok(pasted) => {
            tab.pasted(pasted);
            tab.delivered();
            owed_again(tab.key())
        }
        Err(_) => {
            tab.not_delivered(Undelivered::NotRunning);
            Command::none()
        }
    }
}

/// Writes the Return the tab `key` is owed, when the rule in [`due`] says it is time, and looks
/// again in [`RETURN_EVERY`] when it is not yet.
///
/// A harness that no longer reads its input cannot be sent the message: the paste went in and the
/// program went away, so the message goes back into the tab, where the person can read it and the
/// tab's own restart hands it over again, rather than being lost with a prompt that is gone.
pub(super) fn returned(screen: &mut WorkspaceScreen, key: TabKey, now: Instant) -> Command<Msg> {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) else {
        return Command::none();
    };
    let Some(pasted) = tab.owed().cloned() else { return Command::none() };
    let Some(session) = tab.session().cloned() else {
        tab.returned();
        return Command::none();
    };
    if !pasted.due(&session, now) {
        return owed_again(key);
    }
    tab.returned();
    if session.write(b"\r").is_err() {
        tab.put_back(pasted.letter());
    }
    Command::none()
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

/// Leaves `letter` waiting in the tab `to`, as step `hop` of its chain.
fn queue(screen: &mut WorkspaceScreen, to: TabKey, letter: Letter, hop: u32, now: Instant) {
    screen.rules.received(to.0, hop, now);
    if let Some((_, tab)) = screen.owner_mut(to).and_then(|workspace| workspace.find(to)) {
        tab.receive(letter);
    }
}

/// Asks the person before the tab `key`, named `name` on the strip, is closed with `waiting`
/// messages from other tabs still in it, or with `owed` telling whether a message it has already
/// taken still stands in its prompt without its Return.
///
/// Either way the messages would go with the tab, and the agents that sent them were told their
/// work was taken, so nothing about it would reach anyone again. A message waiting its Return is
/// a moment from being sent, so keeping the tab a moment is enough; the words say so.
pub(super) fn ask_close(key: TabKey, name: &str, waiting: usize, owed: bool) -> Command<Msg> {
    let mut said = Vec::new();
    if waiting > 0 {
        said.push(t!("bridge.close.message", n = waiting));
    }
    if owed {
        said.push(t!("bridge.close.owed"));
    }
    Command::confirm(
        Confirm::new(t!("bridge.close.title", tab = name), Msg::CloseTabAnyway(key))
            .message(said.join(" "))
            .confirm_label(t!("bridge.close.confirm"))
            .cancel_label(t!("bridge.close.cancel")),
    )
}

/// Writes the Return the tab `key` owes as the tab is closed, before its program is ended.
///
/// A message that was pasted into that prompt was taken out of the tab, and its sender was told
/// the tool there had it. Waiting for the rule in [`due`] would lose it: the program is going, and
/// the person has answered that this tab goes. So the Return is written now, which is the last
/// thing the tab can do for that message, and the words of the question say what it amounts to.
pub(super) fn returning_on_close(screen: &mut WorkspaceScreen, key: TabKey) {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) else { return };
    if tab.owed().is_none() {
        return;
    }
    let Some(session) = tab.session().cloned() else { return };
    tab.returned();
    // A program that no longer reads its input takes nothing anyway, and the tab is going with it:
    // there is nowhere left to put the message back, so what the person was asked about is what
    // happens.
    let _ = session.write(b"\r");
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
                Some(Undelivered::Inbox) => t!("bridge.waiting-inbox", n = letters.len(), from = last.from.as_str()),
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
