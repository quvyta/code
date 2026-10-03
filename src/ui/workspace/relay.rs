//! The provider relay of each open workspace: opened beside the bridge's own socket, so a tab of
//! a provider profile can be pointed at it, and told which provider entry answers for which tab.
//!
//! This mirrors [`super::bridge::follow`] exactly, for the socket a harness's own request travels
//! over rather than the one its agent's calls travel over: opened once per open workspace, left
//! alone once it answers, and failing once and quietly rather than on every frame.
//!
//! Resolving a token is synchronous, unlike the bridge's calls: a harness waiting on an answer to
//! forward to the provider cannot wait on this screen's own message loop, so [`Listener::open`]
//! takes a `resolve` closure that answers for itself. It closes over [`Tokens`], the map from a
//! tab's token to the provider its profile named and the model it was asked for, which this
//! module keeps current as tabs open and close, and reads `providers.toml` fresh on every call:
//! a key rotated on the Providers page must reach the next request, not the request after QCode
//! is restarted.
//!
//! What the relay finds out reaches the screen the only way it can, and it is the way the bridge
//! hands over its own calls: a report is a closure on the relay's thread, which cannot touch this
//! screen or ask it for anything, so it leaves what it found in a channel a task of this screen
//! waits on, and that task's message is applied in an ordinary update. A fall back is the one
//! event said under the tab it happened to. A request that was carried through is what every turn
//! of a conversation looks like, and a provider that could not be reached is said by the harness
//! that was refused rather than by a second line of QCode's own.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qframe::prelude::*;
use qframe::runtime::Task;

use crate::profile::{Pick, ProviderChoice};
use crate::provider::{ProviderEntry, RelayEvent as Event, RelayListener, Route};

use super::{Msg, OpenWorkspace, Tab, WorkspaceScreen};

/// The token of a harness tab, mapped to the provider and the model its profile names.
pub(super) type Tokens = Arc<Mutex<HashMap<String, ProviderChoice>>>;

/// How long the line under a tab that its lineup fell back stays there.
///
/// Long enough to read a sentence naming two models and a status, short enough that a busy lineup
/// does not leave a person guessing which of the last few turns they are looking at. Nothing is
/// kept after it: a fall back is said once, under the tab it happened to.
pub(super) const TELL: Duration = Duration::from_secs(10);

/// What one fall back says, as the screen reads it at a draw.
///
/// [`run`](Self::run) says which fall back this is, so a timer set for an older one does not take
/// a newer line away with it: the newest fall back of a tab replaces the one before it, and its
/// own ten seconds are its own.
#[derive(Debug)]
pub(super) struct Notice {
    /// Which fall back this line is.
    run: u64,
    /// The lineup the tab runs on, or `None` when its profile named a single model.
    lineup: Option<String>,
    /// The provider's tag, which is what stands in for a lineup the tab does not have.
    tag: String,
    /// The model that was passed over.
    from: String,
    /// The model being asked instead.
    to: String,
    /// The status the passed-over model answered with, or `None` when it could not be reached.
    status: Option<u16>,
}

impl Notice {
    /// The sentence this fall back is said with, in the language the person reads the screen in.
    fn line(&self) -> String {
        // The lineup is the person's own name for the order their tab runs on, so it is what the
        // line is about; a tab whose profile named a single model has no lineup to speak for it,
        // and its provider's tag is the nearest thing it has.
        let what = self.lineup.as_deref().unwrap_or(&self.tag);
        match self.status {
            Some(status) => t!(
                "workspace.relay.fell-back",
                what = what,
                from = self.from.as_str(),
                status = status,
                to = self.to.as_str()
            ),
            None => t!(
                "workspace.relay.fell-back-unreachable",
                what = what,
                from = self.from.as_str(),
                to = self.to.as_str()
            ),
        }
    }
}

/// The lines a workspace's relay has said, the newest of each tab in place of the one before it.
#[derive(Debug, Default)]
pub(super) struct Notices {
    /// How many fall backs have been said, which numbers them.
    runs: u64,
    /// The line of each token, or of no token at all while none has fallen back.
    lines: HashMap<String, Notice>,
}

impl Notices {
    /// Leaves what `event` says under the tab it was about and answers the token it was about and
    /// which fall back that is.
    ///
    /// Every other event is dropped: a request that was carried through is what every turn of a
    /// conversation looks like, and a provider that could not be reached is said by the harness
    /// that was refused rather than by a second line of QCode's own.
    fn say(&mut self, event: Event) -> Option<(String, u64)> {
        let Event::FellBack { token, tag, lineup, from, to, status } = event else { return None };
        self.runs += 1;
        let run = self.runs;
        self.lines.insert(token.clone(), Notice { run, lineup, tag, from, to, status });
        Some((token, run))
    }

    /// Takes away the line under the tab of `token`, when it is the fall back `run`: the ten
    /// seconds of a line that a newer fall back has already replaced take nothing with them.
    fn hush(&mut self, token: &str, run: u64) {
        if self.lines.get(token).is_some_and(|notice| notice.run == run) {
            self.lines.remove(token);
        }
    }
}

/// Where the screen waits for what its workspace's relay has to say, the way the bridge's own
/// [`Inbox`](crate::bridge::socket::Inbox) is: a receiver behind a lock, so the task that waits
/// is given a handle of its own and holds it for as long as the relay reports.
#[derive(Debug)]
struct Falls(Arc<Mutex<std::sync::mpsc::Receiver<Event>>>);

impl Falls {
    /// A channel the relay reports into and the screen waits on.
    fn open() -> (Sender<Event>, Self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        (sender, Self(Arc::new(Mutex::new(receiver))))
    }
}

/// Whether a workspace's provider relay is listening.
#[derive(Debug, Default)]
pub(super) enum Link {
    /// Not yet: the screen has not come round to this workspace, or it carries no profile that
    /// could ever need the relay.
    #[default]
    Off,
    /// The socket could not be opened.
    Failed,
    /// Listening, with the map a tab of a provider profile is entered into.
    On {
        /// The socket. Never read again once opened: unlike the bridge, a relay answers a
        /// harness's own request on a thread of its own rather than through this screen's
        /// message loop, so this is held for nothing but its `Drop`, which closes the socket
        /// when the workspace does.
        _listener: RelayListener,
        /// What the relay is doing for each of the workspace's tokens, which is the only side of a
        /// request that knows whether one is on its way to a model: the container's script hands it
        /// over and gets the answer back, so nothing between the harness and the provider is left
        /// holding a moment in time. Read by the freeze rules, which ask whether a profile is
        /// waiting on a model before they judge it idle.
        activity: crate::provider::Activity,
        /// The tokens the relay's `resolve` closure was given at open, and this screen still
        /// writes into as tabs open and close.
        tokens: Tokens,
        /// What has been said under the tabs so far, which this screen draws and times.
        lines: Notices,
    },
}

/// Opens the relay of every open workspace that carries a provider profile and has none yet, and
/// starts each one's wait for what its relay has to say.
///
/// Unlike the bridge, this is not gated on the person asking for it: a provider profile is
/// useless without it, so it is opened the moment such a profile is in a workspace, the way the
/// container it points at is made without being asked for by name either.
pub(super) fn follow(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let upstream = screen.upstream.clone();
    let mut commands = Vec::new();
    for workspace in &mut screen.workspaces {
        if !matches!(workspace.relay, Link::Off) {
            continue;
        }
        if !workspace.profiles.iter().any(|profile| profile.provider.is_some()) {
            continue;
        }
        let providers_path = screen.providers_path.clone();
        let tokens: Tokens = Arc::new(Mutex::new(HashMap::new()));
        let entered = Arc::clone(&tokens);
        // The listener is given the map and the file rather than the workspace: it answers on a
        // thread of its own, where this screen's own workspaces are not to be read.
        let resolve_provider = move |token: &str| resolve(&entered, providers_path.as_deref(), token);
        let (found, falls) = Falls::open();
        let telling = move |event| {
            let _ = found.send(event);
        };
        match RelayListener::open(&workspace.paths.mcp(), resolve_provider, upstream.clone(), telling) {
            Ok(listener) => {
                workspace.relay =
                    Link::On { activity: listener.activity(), _listener: listener, tokens, lines: Notices::default() };
                commands.push(listening(falls));
            }
            Err(_) => workspace.relay = Link::Failed,
        }
    }
    Command::batch(commands)
}

/// A task that waits for what the relay has to say and hands each of it to the screen, which is
/// where the bridge's own wait hands a call over: a message into the loop, applied like any other,
/// rather than something drawn behind the screen's back.
///
/// The wait lasts as long as the relay does. It is not armed again from the message that ends it,
/// since a message says nothing about the next one and the relay's thread goes on reporting.
fn listening(falls: Falls) -> Command<Msg> {
    Command::task(Task::new(t!("workspace.relay.listening"), move |cx| {
        let Ok(receiver) = falls.0.lock() else { return Err(String::new()) };
        while let Some(event) = cx.recv(&receiver) {
            cx.send(Msg::Reported(event));
        }
        // The relay is gone and there is nothing left to wait for.
        Err(String::new())
    }))
}

/// A task that takes the line under the tab of `token` away once the fall back `run` has been on
/// screen for [`TELL`], unless a newer fall back has put a newer line there in the meantime.
fn keep_timed(token: String, run: u64) -> Command<Msg> {
    Command::task(Task::new(t!("workspace.relay.telling"), move |cx| {
        if cx.sleep(TELL) { Ok(Msg::Silence(token, run)) } else { Err(String::new()) }
    }))
}

/// Says what the relay reported under the tab it happened to, and sets the timer that takes the
/// line away again [`TELL`] later.
///
/// The ten seconds are counted from here and not from the moment the relay found out: a report
/// travels from the relay's own thread, and a line is on screen for as long as it can be read,
/// whether the screen was busy at the time or not.
pub(super) fn fell(screen: &mut WorkspaceScreen, event: Event) -> Command<Msg> {
    let Event::FellBack { token, .. } = &event else { return Command::none() };
    // A token belongs to one tab of one workspace, and a workspace with no relay listening has
    // nothing to say: a report that arrives after the tab is gone has nowhere to be written.
    let index = screen.workspaces.iter().position(|workspace| workspace.tabs.iter().any(|tab| tab.token() == token));
    let Some(workspace) = index.map(|index| &mut screen.workspaces[index]) else { return Command::none() };
    let Link::On { lines, .. } = &mut workspace.relay else { return Command::none() };
    let Some((token, run)) = lines.say(event) else { return Command::none() };
    keep_timed(token, run)
}

/// Stops saying the line under the tab of `token`, when it is the fall back `run` that has been
/// said for [`TELL`]: a fall back that came after this one is on its own ten seconds.
pub(super) fn silence(screen: &mut WorkspaceScreen, token: &str, run: u64) {
    for workspace in &mut screen.workspaces {
        if let Link::On { lines, .. } = &mut workspace.relay {
            lines.hush(token, run);
        }
    }
}

/// The route `token` names among the tabs entered into `tokens`, read fresh from `providers_path`
/// every time: a key rotated on the Providers page must reach the very next request the relay
/// carries, and a lineup changed there must reach the turn after it.
///
/// A profile that chose a model is carried to that one model, which is the single step a request
/// has. A profile that chose a lineup is carried to the lineup's steps in the order the file holds
/// them, with the lineup's name beside them for the relay's own reports. A lineup that is gone from
/// the file, or that names no step, is no route at all: there is nowhere to send the request, and
/// a relay that guessed a step would send it somewhere the person never chose.
pub(super) fn resolve(tokens: &Tokens, providers_path: Option<&Path>, token: &str) -> Option<Route> {
    let choice = tokens.lock().ok()?.get(token).cloned()?;
    let entry: ProviderEntry = entry_of(providers_path, &choice.tag)?;
    let (models, lineup) = match &choice.pick {
        Pick::Model(model) => (vec![model.clone()], None),
        Pick::Lineup(name) => {
            let steps = entry.lineup(name)?.models.clone();
            (!steps.is_empty()).then(|| (steps, Some(name.clone())))?
        }
    };
    Some(Route { entry, models, lineup })
}

/// The entry `tag` names, from `providers.toml` when the workspace knows where it is, and from
/// what is in memory when it does not.
fn entry_of(providers_path: Option<&Path>, tag: &str) -> Option<ProviderEntry> {
    let providers = match providers_path {
        Some(path) => crate::provider::Providers::open(path).value,
        None => crate::provider::Providers::in_memory(),
    };
    providers.get(tag).cloned()
}

/// Enters `token` into `workspace`'s relay map under the provider and model `choice` names, when
/// the relay is open: from here on, a request carrying this token is carried to that provider,
/// asking for the model the person chose.
pub(super) fn entered(workspace: &OpenWorkspace, token: &str, choice: &ProviderChoice) {
    if let Link::On { tokens, .. } = &workspace.relay
        && let Ok(mut tokens) = tokens.lock()
    {
        tokens.insert(token.to_owned(), choice.clone());
    }
}

/// Forgets the closed tabs' tokens `closed`, so a token nobody carries any more resolves to
/// nothing rather than lingering in the map, and takes away whatever their relay said, since a
/// closed tab has no terminal to say it under.
pub(super) fn closed(workspace: &mut OpenWorkspace, closed: &[String]) {
    let Link::On { tokens, lines, .. } = &mut workspace.relay else { return };
    if let Ok(mut tokens) = tokens.lock() {
        for token in closed {
            tokens.remove(token);
        }
    }
    for token in closed {
        lines.lines.remove(token);
    }
}

/// The line under a tab that its lineup fell back, in the same quiet words as the bridge's own
/// notices and cut to the width rather than wrapped: a harness's terminal is the one thing in the
/// middle that must not be rebuilt to make room for a sentence.
pub(super) fn view(workspace: &OpenWorkspace, tab: &Tab, ui: &mut View<'_, Msg>) {
    let Link::On { lines, .. } = &workspace.relay else { return };
    let Some(notice) = lines.lines.get(tab.token()) else { return };
    ui.row(|ui| {
        ui.add(Text::new(notice.line()).role("secondary").no_wrap());
    })
    .padding(Padding { left: 2, right: 1, ..Padding::default() })
    .fill_width()
    .id("workspace-relay-line");
}
