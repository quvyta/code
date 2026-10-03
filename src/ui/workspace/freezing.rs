//! The screen's side of freezing: when the workspace screen looks at what its profiles are doing,
//! and what it does with what it finds.
//!
//! [`freeze`] says when a profile's container may be frozen and what freezing one is. This is
//! everything around that question: the once-asked question of whether this machine can freeze at
//! all, the minute that brings the screen back to look, the readings a look is a difference from,
//! and the containers QCode has frozen and not woken yet.
//!
//! Nothing here decides on its own what a profile is doing. A look reads the machine's side of
//! [`freeze::Signals`] on a background thread and the screen builds the rest of them when the
//! answer comes, so what is judged is the screen as it is then rather than as it was when the
//! reading went out. A reading that cannot be had is a no rather than a maybe: a container whose
//! control group or whose `/proc` cannot be read is left exactly as it is, which is the whole
//! point of the rules in [`freeze`].
//!
//! The lengths the rules are judged by — [`freeze::QUIET`], [`freeze::CPU_WINDOW`] and the minute
//! between looks — and where the machine's control groups are read from are the screen's own
//! [`Where`], so that a test may shorten the first and point the last at a folder of its own. The
//! product uses the measured values; nothing else may.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use qframe::prelude::*;
use qframe::runtime::Task;

use crate::engine::{Engine, EngineCommand};
use crate::profile::Profile;

use super::freeze::{self, CPU_WINDOW, Lengths, QUIET, Signals};
use super::{Msg, OpenWorkspace, Tab, TabKey, TabKind, WorkspaceScreen, relay, shared};

/// How often the screen looks at what its profiles are doing.
///
/// A minute is short enough that a container which grows quiet is frozen soon after the last thing
/// in it finished, and long enough that a screen left open all day is not asking the engine and
/// the kernel about itself hundreds of times. The rules are measured in tens of minutes, so the
/// finest this reading has to be is whether a moment of work is behind it. It is shorter than
/// [`CPU_WINDOW`] on purpose: the CPU is then read from a reading kept across looks (see
/// `Freezing::spent`), so the window is judged every second look rather than never.
pub(super) const EVERY: Duration = Duration::from_secs(60);

/// What one look's reading of a container found, which is what [`Msg::Looked`] carries.
///
/// `None` for either field is the engine or the kernel refusing, and either way it is not a yes: a
/// container whose control group or whose `/proc` cannot be read is not judged on what it might
/// have been doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The microseconds of CPU everything in the container had used when the reading was taken.
    pub used: Option<u64>,
    /// The command line of every process in the container.
    pub processes: Option<Vec<String>>,
}

/// A container's CPU as of a moment: what it had used, and when that was read. Kept per container
/// between looks, since the reading is only a difference against the one before it.
#[derive(Debug, Clone, Copy)]
struct Taken {
    used: u64,
    at: Instant,
}

/// The lengths a look is judged by and the folder the machine's control groups are read from.
///
/// The product's are the measured [`freeze`] constants and the machine's own cgroup root; a test
/// shortens the first and points the last at a folder of its own, since a test cannot wait ten
/// minutes and cannot write to the real one.
#[derive(Debug, Clone)]
pub(super) struct Where {
    /// How long every tab of a profile must have written nothing.
    pub(super) quiet: Duration,
    /// The shortest window the container's CPU use is read over.
    pub(super) window: Duration,
    /// How often the screen looks.
    pub(super) every: Duration,
    /// Where this machine keeps every control group, which is the only place a frozen container's
    /// pages can be handed back from.
    pub(super) cgroups: PathBuf,
}

impl Default for Where {
    fn default() -> Self {
        Self { quiet: QUIET, window: CPU_WINDOW, every: EVERY, cgroups: PathBuf::from(freeze::CGROUP_ROOT) }
    }
}

impl From<&Where> for Lengths {
    fn from(where_: &Where) -> Self {
        Self { quiet: where_.quiet, window: where_.window }
    }
}

/// What the screen knows about freezing: what this machine can do, what has been frozen, and where
/// each container's CPU was last read.
#[derive(Debug, Default)]
pub(super) struct Freezing {
    /// Whether this machine can freeze a container at all, asked once when the screen got its
    /// engine and never again: a session does not change its engine or its swap underneath it.
    /// `None` until the answer comes, and `None` means no timer runs.
    pub(super) supported: Option<bool>,
    /// The containers QCode has frozen and not woken since, by name. A container here is one the
    /// look leaves alone and the panel shows as frozen.
    pub(super) frozen: HashSet<String>,
    /// The last CPU reading of each container this screen has read, and when it was taken.
    readings: HashMap<String, Taken>,
    /// Whether the next look is timed, so that one is timed at a time.
    pub(super) timed: bool,
    /// The containers the engine has been asked to wake and has not answered for yet. A second
    /// wake of one of them is not asked: the engine refuses to unpause a container that is awake,
    /// and that refusal, coming after the first wake's success, would have the screen call an awake
    /// container frozen again — and hold every message for it for ever.
    waking: HashSet<String>,
}

impl Freezing {
    /// Whether `container` is one QCode has frozen and not woken since.
    #[must_use]
    pub(super) fn is_frozen(&self, container: &str) -> bool {
        self.frozen.contains(container)
    }

    /// Notes that `container` is frozen, or that it is not.
    ///
    /// A freeze that failed leaves nothing behind: no container noted, and nothing said to the
    /// person, so the next look reads the container again and tries once more. A thaw that failed
    /// leaves the container in the set, since it is still frozen and every later thing that needs
    /// it must ask the engine about it again.
    pub(super) fn note(&mut self, container: &str, frozen: bool) {
        if frozen {
            self.frozen.insert(container.to_owned());
        } else {
            self.frozen.remove(container);
        }
    }

    /// Forgets where the CPU of `container` was last read: a woken container's next reading is a
    /// difference from the moment it woke, not from the reading taken before it was put to sleep.
    pub(super) fn forgot(&mut self, container: &str) {
        self.readings.remove(container);
    }

    /// The CPU of `container` as a difference against its last reading, over the window between
    /// the two, and keeps this one. `None` for a container seen for the first time, which is not
    /// frozen on this look: there is nothing to take a difference from yet.
    /// Notes that the engine answered for the wake of `container`, whatever it said.
    pub(super) fn woke(&mut self, container: &str) {
        self.waking.remove(container);
    }

    /// Takes out of the frozen set every container the engine has just listed as anything but
    /// paused. The panel's own Stop and Restart, a crash of the engine or the person's own `podman
    /// unpause` all wake or end a container without the screen hearing of it, and a container the
    /// screen still called frozen would never be looked at again.
    pub(super) fn listed(&mut self, containers: &[crate::engine::Container]) {
        for container in containers {
            if !matches!(container.state, crate::engine::ContainerState::Paused) {
                self.frozen.remove(&container.name);
            }
        }
    }

    /// The CPU `container` has used since the reading this one is measured from, and over how long.
    ///
    /// That reading is kept until it is `window` old. The screen looks more often than the window
    /// is long, so measuring from the look before would give a window one look long on every look,
    /// shorter than the rule asks for, and no container would ever be frozen. Kept, the reading
    /// gives a window that grows look by look until it is long enough to be judged on, and only
    /// then does the new reading take its place.
    fn spent(&mut self, container: &str, used: u64, at: Instant, window: Duration) -> Option<(u64, Duration)> {
        let since = |before: &Taken| (used.saturating_sub(before.used), at.saturating_duration_since(before.at));
        match self.readings.get(container) {
            Some(before) if at.saturating_duration_since(before.at) < window => Some(since(before)),
            _ => self.readings.insert(container.to_owned(), Taken { used, at }).map(|before| since(&before)),
        }
    }
}

/// Forgets where the CPU of every container of the workspace at `index` was last read, which is
/// leaving the rail: a container of a workspace that is not on the screen is not looked at again,
/// and one that comes back is read from this moment rather than from a reading taken before its
/// tabs were closed.
pub(super) fn left(screen: &mut WorkspaceScreen, index: usize) {
    let Some(workspace) = screen.workspaces.get(index) else { return };
    let containers: Vec<String> =
        workspace.profiles.iter().filter_map(|profile| container_of(workspace, profile)).collect();
    for container in containers {
        screen.freezing.forgot(&container);
    }
}

/// Asks once, on a background thread, whether this machine can freeze a container at all, and keeps
/// the answer: podman, running as the person who started it, and with swap. Nothing else about
/// freezing happens without it, and no timer is run at all when it says no.
///
/// Runs the engine, so it belongs on a background thread.
pub(super) fn ask(engine: Engine) -> Command<Msg> {
    Command::perform(move || Msg::Freezable(freeze::supported(&engine)))
}

/// The next look, timed once, while there is something to look at: the person wants quiet profiles
/// frozen, this machine can freeze one, and a workspace is open. One timer at a time, so a look
/// that is already timed is not timed again.
pub(super) fn again(screen: &mut WorkspaceScreen) -> Command<Msg> {
    if !may_look(screen) || screen.freezing.timed {
        return Command::none();
    }
    screen.freezing.timed = true;
    let every = screen.where_.every;
    Command::task(Task::new(t!("workspace.freezing.looking"), move |cx| {
        if cx.sleep(every) { Ok(Msg::Look) } else { Err(String::new()) }
    }))
}

/// Whether there is anything for a look to be about: the person turned freezing on, this machine can
/// do it, and there is an engine to ask.
#[must_use]
pub(super) fn may_look(screen: &WorkspaceScreen) -> bool {
    screen.freezes_idle() && screen.freezing.supported == Some(true) && screen.engine().is_some()
}

/// One look: for every open workspace, every profile of it that has a terminal tab and whose
/// container is not frozen already is read out of its container and its `/proc`.
///
/// The reading is what the engine and the kernel are asked, and it goes to a background thread of
/// its own for each profile, so a slow one holds up nothing but the profile it is about and no
/// part of the screen waits for it. What the profile's tabs are doing is not read here: it is built
/// when the answer comes, so what is judged is the screen as it is then.
///
/// A look that was timed before the person turned freezing off, or before this machine turned out
/// not to be able to freeze anything, reads nothing when it comes round: the person is not waiting
/// for a timer to finish before their choice takes hold, and no timer is timed again either.
pub(super) fn look(screen: &mut WorkspaceScreen) -> Command<Msg> {
    if !may_look(screen) {
        return Command::none();
    }
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let cgroups = screen.where_.cgroups.clone();
    let mut commands = Vec::new();
    for (index, workspace) in screen.workspaces.iter().enumerate() {
        for profile in &workspace.profiles {
            let Some(container) = container_of(workspace, profile) else { continue };
            if screen.freezing.is_frozen(&container) {
                continue;
            }
            let (id, name) = (workspace.id().to_owned(), profile.name.to_string());
            let (engine, container) = (engine.clone(), container.clone());
            let cgroups = cgroups.clone();
            commands.push(Command::perform(move || {
                Msg::Looked(index, id, name, read(&engine, &container, &cgroups), Instant::now())
            }));
        }
    }
    Command::batch(commands)
}

/// What came of a look's reading: the signals the screen builds now for the profile, the CPU it
/// spent since the reading before it, and whether the container may be frozen.
///
/// A profile whose workspace has closed, or which the store no longer carries, is left alone: there
/// is no container of it on the screen to do anything with. A reading that could not be had carries
/// no `cpu` at all, so a container whose control group would not say is never frozen on a reading
/// that says nothing.
pub(super) fn looked(
    screen: &mut WorkspaceScreen,
    index: usize,
    id: &str,
    name: &str,
    read: Read,
    at: Instant,
) -> Command<Msg> {
    let Some(workspace) = screen.workspaces.get(index).filter(|workspace| workspace.id() == id) else {
        return Command::none();
    };
    let Some(profile) = workspace.profiles.iter().find(|profile| profile.name.as_str() == name) else {
        return Command::none();
    };
    let Some(container) = container_of(workspace, profile) else { return Command::none() };
    let mut signals = signals(screen, index, workspace, profile);
    let window = screen.where_.window;
    signals.cpu = read.used.and_then(|used| screen.freezing.spent(&container, used, at, window));
    signals.processes = read.processes;
    if !freeze::may_freeze(&signals, profile.harness, Lengths::from(&screen.where_)) {
        return Command::none();
    }
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let cgroups = screen.where_.cgroups.clone();
    Command::perform(move || {
        // Paused is frozen, whatever came of the reclaim after it: a container the engine paused and
        // the kernel took nothing back from is still one whose tabs answer nothing until it is woken,
        // and the screen has to know so to wake it.
        let paused = !matches!(freeze::freeze(&engine, &container, &cgroups), Err(freeze::FreezeTrouble::Engine(_)));
        Msg::Frozen(container, paused)
    })
}

/// Wakes the container of the tab the person has just brought into view, when QCode froze it: a
/// paused container answers nothing to the harness in it and refuses every command the screen runs
/// there, and the person has come back to this tab.
///
/// Every place the screen changes which tab is shown comes through here, so there is one answer to
/// "the shown tab is a profile's, and that profile's container is frozen" rather than one for each
/// way of showing a tab. A tab that is still starting is left to the engine bringing it up, which
/// wakes a frozen container itself: asking twice would have one of the two refused, and a refusal
/// is what a tab shows as its own failure.
#[must_use]
pub(super) fn shown(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let Some(tab) = screen.workspace().and_then(OpenWorkspace::active_tab) else { return Command::none() };
    if tab.state().is_starting() {
        return Command::none();
    }
    let Some(workspace) = screen.workspace() else { return Command::none() };
    let Some(container) = container_of_tab(workspace, tab) else { return Command::none() };
    thawing(screen, container)
}

/// The container of the tab `key` being closed, when QCode froze it, and `None` for every other
/// tab: what the command that ends that tab's work runs in has to be awake for, since a paused
/// container refuses it and a harness nobody is left to read goes on working.
#[must_use]
pub(super) fn closing(screen: &WorkspaceScreen, key: TabKey) -> Option<String> {
    let workspace = screen.owner(key)?;
    let tab = workspace.tabs.iter().find(|tab| tab.key() == key)?;
    let container = container_of_tab(workspace, tab)?;
    screen.freezing.is_frozen(&container).then_some(container)
}

/// The container of `tab`, when QCode froze it and a message is waiting in the tab to be typed into
/// its harness, and `None` for every other case: a paused container would take the paste into a
/// program that is not running, and a message is better a while in the tab where the person can
/// read it. A tab with no message waiting is not a reason to wake anything.
#[must_use]
pub(super) fn waiting_in(screen: &WorkspaceScreen, workspace: &OpenWorkspace, tab: &Tab) -> Option<String> {
    if tab.letters().is_empty() {
        return None;
    }
    let container = container_of_tab(workspace, tab)?;
    screen.freezing.is_frozen(&container).then_some(container)
}

/// Notes that the container of the tab `key` is up, which is what the engine's own bringing a tab
/// up does to a container QCode froze: the screen stops calling it frozen, and the CPU it had used
/// before it slept says nothing about what it has done since.
pub(super) fn up(screen: &mut WorkspaceScreen, key: TabKey) {
    let container = screen.owner(key).and_then(|workspace| {
        let tab = workspace.tabs.iter().find(|tab| tab.key() == key)?;
        container_of_tab(workspace, tab)
    });
    if let Some(container) = container {
        screen.freezing.note(&container, false);
        screen.freezing.forgot(&container);
    }
}

/// The waking of `container`, which the screen has frozen, on a background thread. A container
/// whose wake is already on its way is not asked for again.
#[must_use]
pub(super) fn thawing(screen: &mut WorkspaceScreen, container: String) -> Command<Msg> {
    if !screen.freezing.is_frozen(&container) || screen.freezing.waking.contains(&container) {
        return Command::none();
    }
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    screen.freezing.waking.insert(container.clone());
    Command::perform(move || Msg::Thawed(container.clone(), awake(&engine, &container)))
}

/// Wakes `container` and answers whether it is awake now. A wake the engine refused is asked about
/// once more, since the refusal may only mean that something else woke it first — the engine
/// bringing a tab up, the panel's own Stop, the person's own `podman unpause` — and an awake
/// container the screen still called frozen would hold every message for it.
///
/// Runs engine commands, so it belongs on a background thread.
fn awake(engine: &Engine, container: &str) -> bool {
    freeze::thaw(engine, container).is_ok()
        || crate::engine::run::capture(&engine.container_state(container)).is_ok_and(|word| {
            !matches!(crate::engine::ContainerState::parse(&word), crate::engine::ContainerState::Paused)
        })
}

/// The waking of `container` on the screen's own account followed by the work that has to run in it,
/// both on one background thread and in that order: a command run in a paused container is refused,
/// and a second command would not wait for the first.
///
/// A container the engine would not wake refuses the second command as well, which costs nothing:
/// there was nothing running in it to end, and the next thing that needs it asks again.
pub(super) fn thawing_then(screen: &mut WorkspaceScreen, container: String, then: EngineCommand) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    screen.freezing.waking.insert(container.clone());
    Command::perform(move || {
        let awake = awake(&engine, &container);
        let _ = crate::engine::run::capture(&then);
        Msg::Thawed(container, awake)
    })
}

/// The engine commands that wake every container the screen has frozen, one name each and in an
/// order of their own, for the moment QCode is going.
///
/// A container left paused is one the next QCode finds asleep with every tab in it still holding
/// what it held, and one the reaper cannot stop, since a stop of a paused container is refused.
/// QCode waits for the engine over these, since there is no later to run them in.
#[must_use]
pub fn waking_on_the_way_out(screen: &WorkspaceScreen) -> Vec<EngineCommand> {
    let Some(engine) = screen.engine.as_ref() else { return Vec::new() };
    let mut names: Vec<&str> = screen.freezing.frozen.iter().map(String::as_str).collect();
    names.sort_unstable();
    names.into_iter().map(|name| engine.unpause_container(name)).collect()
}

/// Builds what the screen knows about `profile` of `workspace`, which is every rule but the two that
/// are a difference over time and the two the machine is asked about.
///
/// A profile that opens a window is never frozen: its window is a container of its own and the
/// person is looking at it. A profile with no terminal tab is not judged either, since
/// [`Signals::quiet_for`] is `None` without one, which is the rules' own answer.
fn signals(screen: &WorkspaceScreen, index: usize, workspace: &OpenWorkspace, profile: &Profile) -> Signals {
    let now = Instant::now();
    let tabs: Vec<&Tab> = workspace.tabs.iter().filter(|tab| owns(tab, profile)).collect();
    let quiet_for = tabs
        .iter()
        .filter_map(|tab| tab.session())
        .map(|session| now.saturating_duration_since(session.last_output()))
        .min();
    let (relay_busy, relay_quiet_for) = relay_of(workspace, profile, now);
    Signals {
        // Only the tab the person is looking at is on screen, and only the open workspace has one:
        // a tab of a workspace behind another is not on the screen, and a profile of the same name in
        // another workspace is a container of its own that nothing of this one can be on the screen
        // of.
        on_screen: index == screen.active
            && screen.workspace().and_then(OpenWorkspace::active_tab).is_some_and(|tab| owns(tab, profile)),
        // A line the person has not sent, a message being pasted, or a message waiting to be
        // delivered: each is work in a tab of this profile whatever its container is doing.
        typing: tabs.iter().any(|tab| tab.person_typed() || tab.owed().is_some() || !tab.letters().is_empty()),
        quiet_for,
        relay_busy,
        relay_quiet_for,
        cpu: None,
        processes: None,
        desktop: profile.harness.desktop().is_some(),
    }
}

/// The profile whose container `tab` runs in, when it runs in one: a harness in a terminal, or the
/// administrator's shell, which is a terminal in the same container. A window's tab is not one — it
/// has a container of its own, which is never frozen — and neither is a file, a shell or a blank
/// tab.
#[must_use]
fn of(tab: &Tab) -> Option<&str> {
    match tab.kind() {
        TabKind::Profile(name) | TabKind::Admin(name) => Some(name.as_str()),
        _ => None,
    }
}

/// Whether `tab` is a terminal tab of `profile`, which is what a profile's container is entered
/// with.
#[must_use]
fn owns(tab: &Tab, profile: &Profile) -> bool {
    of(tab).is_some_and(|name| name == profile.name.as_str())
}

/// The container `profile`'s terminal tabs of `workspace` run in, and `None` when it has none: a
/// profile nobody opened a terminal tab of has no container to read, and a window's own container is
/// a thing of its own that is never frozen.
#[must_use]
fn container_of(workspace: &OpenWorkspace, profile: &Profile) -> Option<String> {
    workspace
        .tabs
        .iter()
        .any(|tab| owns(tab, profile))
        .then(|| crate::engine::names::profile_container(workspace.id(), profile.name.as_str()))
}

/// The container `tab` runs in, and `None` for a tab that runs in none or in a window's own.
#[must_use]
fn container_of_tab(workspace: &OpenWorkspace, tab: &Tab) -> Option<String> {
    let name = of(tab)?;
    let profile = workspace.profiles.iter().find(|profile| profile.name.as_str() == name)?;
    container_of(workspace, profile)
}

/// What the relay is doing for `profile`'s tokens, and how long ago the last request of one of them
/// finished.
///
/// The tokens are each tab's own, and, for opencode's shared server, the server's own token as
/// well: a request of a shared server is carried by the server's token, not by the tab that asked
/// for it. A profile that does not run on a provider carries `None` for both, which skips the
/// relay's rules rather than failing them, and so does one whose relay is not listening, since
/// nothing of it can be on its way through a relay that is not there.
fn relay_of(workspace: &OpenWorkspace, profile: &Profile, now: Instant) -> (Option<bool>, Option<Duration>) {
    if profile.provider.is_none() {
        return (None, None);
    }
    let relay::Link::On { activity, .. } = &workspace.relay else { return (None, None) };
    let mut tokens: Vec<String> =
        workspace.tabs.iter().filter(|tab| owns(tab, profile)).map(|tab| tab.token().to_owned()).collect();
    if shared::shares(profile) {
        tokens.push(workspace.servers.token(profile.name.as_str()));
    }
    let busy = tokens.iter().any(|token| activity.busy(token));
    let quiet_for = tokens
        .iter()
        .filter_map(|token| activity.last_finished(token))
        .map(|at| now.saturating_duration_since(at))
        .min();
    (Some(busy), quiet_for)
}

/// The microseconds of CPU everything in `container` has used, read out of the control group the
/// engine says it has under `cgroups`, and the command line of every process in it. Either may be
/// missing when the engine or the kernel would not say, and either way it is not a yes.
fn read(engine: &Engine, container: &str, cgroups: &Path) -> Read {
    Read {
        used: freeze::cgroup(engine, container, cgroups).as_deref().and_then(freeze::cpu_used),
        processes: freeze::processes(engine, container),
    }
}
