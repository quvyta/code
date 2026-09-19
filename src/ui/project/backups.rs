//! When the open projects are backed up, and what the screen says about it.
//!
//! The backups themselves are the `backup` module's work; this is when they are asked for. Each
//! project on the rail has a timer of its own, set one interval after it opens, and is backed up
//! once more as it is closed from the rail and as QCode quits. A backup runs engine commands, so
//! it always runs on a background thread and the screen only hears what came of it.
//!
//! A round takes the conversations of every profile whose tab was open since the round before
//! with it, each into a backup of its own. A harness that keeps them in a database is only
//! backed up while its container is stopped, so a round that finds it running passes it by
//! without a word; QCode takes it once more after it has stopped the containers as it quits.
//!
//! A backup is quiet. A round that found nothing new says nothing, a round another QCode is
//! already doing is skipped, and a failure is said once for the project rather than at every
//! round: a person told every fifteen minutes that the engine is gone learns nothing new the
//! second time.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use qframe::date::DateTime;
use qframe::prelude::*;
use qframe::runtime::Confirm;
use qframe::widgets::{Modal, Select, Spinner, Toast};

use crate::backup::conversations::{self, Brought};
use crate::backup::{self, BackupError, BackupEvery, Entry, Reason, Restored, Snapshot, SnapshotId};
use crate::base;
use crate::engine::run::EngineError;
use crate::engine::{Engine, HostUser};
use crate::profile::HarnessKind;
use crate::profile::identity::Home;
use crate::workspace::{ProjectPaths, set_backup_assets, set_backup_skip};

use super::{Msg, OpenProject, ProjectScreen, TabKind, TabState};

/// What the screen knows about the backups of one project.
#[derive(Debug, Default)]
pub(super) struct State {
    /// The number of the timer that backs the project up next, while one is set.
    timer: Option<u64>,
    /// Whether the newest backup has been asked after; it is asked once, as the project opens.
    asked: bool,
    /// When the newest backup was made, in seconds since the Unix epoch.
    last: Option<i64>,
    /// Whether a backup of the project is being made right now.
    running: bool,
    /// Whether a failure was said already.
    told: bool,
    /// The profiles whose harness tab was open at some time since the last round, by name: the
    /// next round takes their conversations.
    used: BTreeSet<String>,
    /// How much `Backup/` holds, in bytes, once it was read.
    size: Option<u64>,
    /// Whether the size was asked for since the project opened.
    sized: bool,
}

/// A part of a round besides the project's own snapshot, named when it could not be backed up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// `Assets/`, which the project asked to be backed up too.
    Assets,
    /// The conversations of the profile of this name.
    Conversations(String),
}

/// One profile's conversations in a project, as a round takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Chat {
    /// Where the harness keeps them.
    home: Home,
    /// The harness that wrote them, which says where in the home they are.
    harness: HarnessKind,
}

/// Why a backup was not made, carried back from the thread that tried.
///
/// Words are not chosen on that thread: the person's language is the screen's, so the reason
/// travels as what happened and is written out when it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupTrouble {
    /// A path the project file leaves out is not a path inside the project.
    Outside(String),
    /// The engine was stopped before it answered.
    Stopped,
    /// What the machine, the engine or git said.
    Said(String),
}

impl BackupTrouble {
    /// What went wrong with `error`.
    fn of(error: &BackupError) -> Self {
        let engine = |error: &EngineError| match error {
            EngineError::NotRunnable { error, .. } => Self::Said(error.to_string()),
            EngineError::Failed(failure) => Self::Said(failure.output.clone()),
            EngineError::Cancelled { .. } => Self::Stopped,
        };
        match error {
            BackupError::Place(bad) => Self::Outside(bad.path.clone()),
            BackupError::Host(error) | BackupError::Base(base::Failure::Host(error)) => Self::Said(error.to_string()),
            BackupError::Base(base::Failure::Engine(error)) | BackupError::Engine(error) => engine(error),
            BackupError::Answer(answer) => Self::Said(answer.clone()),
        }
    }

    /// The reason, in the person's language.
    #[must_use]
    pub fn words(&self) -> String {
        match self {
            Self::Outside(path) => t!("project.backup.outside", path = path.as_str()),
            Self::Stopped => t!("app.stopped"),
            Self::Said(words) => words.clone(),
        }
    }
}

/// Sets a timer for every project that has none, and asks after the newest backup of every
/// project that has not been asked after yet.
///
/// Without an engine nothing is set: a backup runs in a container, and a project opened without
/// one is backed up once the screen is made again with an engine.
pub(super) fn keep(screen: &mut ProjectScreen) -> Command<Msg> {
    let mut commands = Vec::new();
    for project in &mut screen.projects {
        note_used(project);
        // The size is read off the disk of this machine, so it is known with or without an engine.
        if !project.backup.sized {
            project.backup.sized = true;
            commands.push(read_size(project));
        }
    }
    let Some(engine) = screen.engine.clone() else { return Command::batch(commands) };
    let user = screen.user;
    let interval = screen.backup_every.interval();
    for project in &mut screen.projects {
        if !project.backup.asked {
            project.backup.asked = true;
            commands.push(read_newest(engine.clone(), user, project));
        }
        if let Some(interval) = interval
            && project.backup.timer.is_none()
        {
            screen.last_timer += 1;
            project.backup.timer = Some(screen.last_timer);
            commands.push(super::after(interval, Msg::BackupDue(project.id().to_owned(), screen.last_timer)));
        }
    }
    Command::batch(commands)
}

/// Notes every profile of `project` whose harness tab is open now, for the next round to take
/// its conversations. A tab brought back from the last session and not shown yet has not
/// started its harness, so it does not count until it is shown.
fn note_used(project: &mut OpenProject) {
    for tab in &project.tabs {
        if let TabKind::Profile(name) = tab.kind()
            && tab.state() != &TabState::Waiting
        {
            project.backup.used.insert(name.clone());
        }
    }
}

/// The profiles whose conversations the backup of `project` made now takes: every one whose tab was
/// open since the last round, and every one open now.
pub(super) fn used(project: &OpenProject) -> BTreeSet<String> {
    let mut used = project.backup.used.clone();
    for tab in &project.tabs {
        if let TabKind::Profile(name) = tab.kind()
            && tab.state() != &TabState::Waiting
        {
            used.insert(name.clone());
        }
    }
    used
}

/// The conversations the backup of `project` takes, each profile's with the harness that wrote
/// them. A profile gone from the workspace is passed by: there is no harness to say where its
/// conversations are.
fn chats(project: &OpenProject, names: &BTreeSet<String>) -> Vec<Chat> {
    names
        .iter()
        .filter_map(|name| project.profiles.iter().find(|profile| profile.name.as_str() == name))
        .map(|profile| Chat { home: Home::new(profile.name.clone(), project.id.clone()), harness: profile.harness })
        .collect()
}

/// The conversations the round of `project` starting now takes, which it takes off the list: the
/// list starts again with the tabs still open, since they go on writing.
fn take_chats(project: &mut OpenProject) -> Vec<Chat> {
    let chats = chats(project, &used(project));
    project.backup.used.clear();
    note_used(project);
    chats
}

/// Takes `every` as how often the open projects are backed up from now on. Their timers start
/// again from now, so a shorter interval is not held up by a long one set before it.
pub(super) fn set_every(screen: &mut ProjectScreen, every: BackupEvery) {
    screen.backup_every = every;
    for project in &mut screen.projects {
        project.backup.timer = None;
    }
}

/// Asks the backup of the project `id` for the newest snapshot, on a background thread. A
/// project that was never backed up answers at once, without a container.
fn read_newest(engine: Engine, user: HostUser, project: &OpenProject) -> Command<Msg> {
    let id = project.id().to_owned();
    let paths = project.paths().clone();
    Command::perform(move || {
        let newest = backup::history(&engine, &paths, user, None).ok().and_then(|found| found.first().map(|e| e.at));
        Msg::NewestBackup(id, newest)
    })
}

/// Reads how much the backup of `project` holds, on a background thread.
fn read_size(project: &OpenProject) -> Command<Msg> {
    let id = project.id().to_owned();
    let paths = project.paths().clone();
    Command::perform(move || Msg::BackupSize(id, backup::size(&paths)))
}

/// Takes how much the backup of the project `id` holds.
pub(super) fn sized(screen: &mut ProjectScreen, id: &str, bytes: u64) {
    if let Some(project) = screen.project_mut(id) {
        project.backup.size = Some(bytes);
    }
}

/// What the panel says the backup of `project` holds: its size, once it was read.
pub(super) fn size_text(project: &OpenProject) -> String {
    project.backup.size.map_or_else(|| t!("project.backup.size-reading"), bytes)
}

/// `count` bytes in the person's words: the unit that keeps the number below a thousand, with one
/// decimal below ten so a small backup still shows it grow.
#[must_use]
pub(super) fn bytes(count: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut unit = 0;
    let mut tenths = u128::from(count) * 10;
    while tenths >= 10_000 && unit + 1 < UNITS.len() {
        tenths /= 1000;
        unit += 1;
    }
    let (whole, tenth) = (tenths / 10, tenths % 10);
    let number = if unit == 0 || whole >= 10 {
        whole.to_string()
    } else {
        format!("{whole}{}{tenth}", t!("project.backup.decimal"))
    };
    t!("project.backup.size", n = number, unit = UNITS[unit])
}

/// Takes the newest backup of the project `id`, unless a backup made since says something newer.
pub(super) fn newest(screen: &mut ProjectScreen, id: &str, at: Option<i64>) {
    if let Some(project) = screen.project_mut(id) {
        project.backup.last = project.backup.last.max(at);
    }
}

/// The timer `run` of the project `id` went off: the project is backed up, and [`keep`] sets the
/// next timer. A timer that was replaced meanwhile, or whose project was closed, does nothing.
pub(super) fn due(screen: &mut ProjectScreen, id: &str, run: u64) -> Command<Msg> {
    let (engine, user) = (screen.engine.clone(), screen.user);
    let Some(project) = screen.project_mut(id) else { return Command::none() };
    if project.backup.timer != Some(run) {
        return Command::none();
    }
    project.backup.timer = None;
    match engine {
        Some(engine) => back_up(&engine, user, project),
        None => Command::none(),
    }
}

/// Backs the project at `index` up as it leaves the rail, when backups are on.
pub(super) fn closing(screen: &mut ProjectScreen, index: usize) -> Command<Msg> {
    if screen.backup_every == BackupEvery::Off {
        return Command::none();
    }
    let (Some(engine), user) = (screen.engine.clone(), screen.user) else { return Command::none() };
    match screen.projects.get_mut(index) {
        Some(project) => back_up(&engine, user, project),
        None => Command::none(),
    }
}

/// Backs `project` up on a background thread, unless a backup of it is running already.
fn back_up(engine: &Engine, user: HostUser, project: &mut OpenProject) -> Command<Msg> {
    if project.backup.running {
        return Command::none();
    }
    project.backup.running = true;
    let engine = engine.clone();
    let (id, name, paths) = (project.id().to_owned(), project.name().to_owned(), project.paths().clone());
    let (skip, assets) = (project.backup_skip().to_vec(), project.assets);
    let chats = take_chats(project);
    Command::perform(move || {
        let made = backup::snapshot(&engine, &paths, user, &skip).map_err(|error| BackupTrouble::of(&error));
        let mut missed = back_up_assets(&engine, &paths, user, assets);
        missed.extend(back_up_chats(&engine, &paths, user, &chats));
        Msg::BackedUp { project: id, name, made, missed }
    })
}

/// Backs `Assets/` up when the project asks for it, and answers the part when it could not be.
fn back_up_assets(engine: &Engine, paths: &ProjectPaths, user: HostUser, assets: bool) -> Vec<(Part, BackupTrouble)> {
    if !assets {
        return Vec::new();
    }
    backup::snapshot_assets(engine, paths, user)
        .err()
        .map(|error| (Part::Assets, BackupTrouble::of(&error)))
        .into_iter()
        .collect()
}

/// Backs up the conversations of `chats`, and answers each part that could not be. A harness
/// running on a database is passed by while its container is up: its round is when the container
/// has stopped.
fn back_up_chats(engine: &Engine, paths: &ProjectPaths, user: HostUser, chats: &[Chat]) -> Vec<(Part, BackupTrouble)> {
    chats
        .iter()
        .filter_map(|chat| {
            let error = conversations::snapshot(engine, paths, &chat.home, chat.harness, user).err()?;
            Some((Part::Conversations(chat.home.profile().as_str().to_owned()), BackupTrouble::of(&error)))
        })
        .collect()
}

/// What the person is told when `part` of a project could not be backed up.
fn part_title(part: &Part) -> String {
    match part {
        Part::Assets => t!("project.backup.assets-failed"),
        Part::Conversations(profile) => t!("project.backup.conversations-failed", profile = profile.as_str()),
    }
}

/// Takes what a backup of the project `id`, called `name`, came to: its own snapshot `made`, and
/// the parts of the round that were `missed`. A failure is said once for the project, and said
/// again only after a round worked in between; one of a project closed meanwhile is said, since
/// it was that project's last backup.
pub(super) fn backed_up(
    screen: &mut ProjectScreen,
    id: &str,
    name: &str,
    made: &Result<Snapshot, BackupTrouble>,
    missed: &[(Part, BackupTrouble)],
) -> Command<Msg> {
    let failure = match made {
        Err(trouble) => Some((t!("project.backup.failed", name = name), trouble)),
        Ok(_) => missed.first().map(|(part, trouble)| (part_title(part), trouble)),
    };
    let told = failure.map(|(title, trouble)| Command::toast(Toast::warning(title).body(trouble.words())));
    let Some(project) = screen.project_mut(id) else { return told.unwrap_or_else(Command::none) };
    project.backup.running = false;
    if let Ok(Snapshot::Made { at, .. }) = made {
        project.backup.last = project.backup.last.max(Some(*at));
    }
    // Whatever the round wrote, and backing up assets or conversations may write when the
    // project's own snapshot did not, the size is read again.
    let size = read_size(project);
    let said = match told {
        // Another QCode has the same project open and is backing it up this minute, so this round
        // says nothing either way.
        None if made == &Ok(Snapshot::Busy) => Command::none(),
        None => {
            project.backup.told = false;
            Command::none()
        }
        Some(_) if project.backup.told => Command::none(),
        Some(told) => {
            project.backup.told = true;
            told
        }
    };
    Command::batch([said, size])
}

/// Writes into the project's `project.qcode` whether its backup takes `Assets/` too, on a
/// background thread. The next round follows the file.
pub(super) fn set_assets(project: &OpenProject, assets: bool) -> Command<Msg> {
    let id = project.id().to_owned();
    let paths = project.paths().clone();
    Command::perform(move || {
        let written =
            set_backup_assets(&paths, assets).map(|file| file.backup_assets).map_err(|problem| problem.to_string());
        Msg::AssetsWritten(id, written)
    })
}

/// Takes whether the project `id`'s file now backs its assets up, or says why it could not be
/// written; the switch shows the choice as it stood before.
pub(super) fn assets_written(screen: &mut ProjectScreen, id: &str, written: Result<bool, String>) -> Command<Msg> {
    match written {
        Ok(assets) => {
            if let Some(project) = screen.project_mut(id) {
                project.assets = assets;
            }
            Command::none()
        }
        Err(reason) => Command::toast(Toast::warning(t!("project.backup.skip-failed")).body(reason)),
    }
}

/// Writes into the project's `project.qcode` that its backup leaves `keys` out, or takes them in
/// again when `out` is false, on a background thread. The next backup follows the file.
pub(super) fn leave_out(project: &OpenProject, keys: Vec<String>, out: bool) -> Command<Msg> {
    let keys: Vec<String> = keys.into_iter().filter(|key| super::files::is_inside(key)).collect();
    if keys.is_empty() {
        return Command::none();
    }
    let id = project.id().to_owned();
    let paths = project.paths().clone();
    Command::perform(move || {
        let written =
            set_backup_skip(&paths, &keys, out).map(|file| file.backup_skip).map_err(|problem| problem.to_string());
        Msg::SkipWritten(id, written)
    })
}

/// Takes what the project `id`'s file now leaves out of its backup, or says why it could not be
/// written; the tree shows the list as it stood before.
pub(super) fn skip_written(screen: &mut ProjectScreen, id: &str, written: Result<Vec<String>, String>) -> Command<Msg> {
    match written {
        Ok(skip) => {
            if let Some(project) = screen.project_mut(id) {
                project.skip = skip;
            }
            Command::none()
        }
        Err(reason) => Command::toast(Toast::warning(t!("project.backup.skip-failed")).body(reason)),
    }
}

/// Whether the backup of a project that leaves `skip` out leaves out the entry `key`: it is
/// named, or it is inside a folder that is.
#[must_use]
pub(super) fn is_left_out(skip: &[String], key: &str) -> bool {
    skip.iter().any(|out| super::file_ops::is_within(key, out))
}

/// What the panel says the backup of `project` leaves out: the names, or that it takes
/// everything in.
pub(super) fn left_out_text(project: &OpenProject) -> String {
    if project.skip.is_empty() { t!("project.backup.nothing-left-out") } else { project.skip.join(", ") }
}

/// What the panel says about the last backup of `project`: when it was made, in the offset of
/// `now`, or why there is none.
pub(super) fn last_text(screen: &ProjectScreen, project: &OpenProject, now: DateTime) -> String {
    if screen.backup_every == BackupEvery::Off {
        return t!("project.backup.off");
    }
    if screen.engine.is_none() {
        return t!("project.backup.no-engine");
    }
    match project.backup.last {
        Some(at) => stamp(at, now),
        None if project.backup.running => t!("project.backup.running"),
        None => t!("project.backup.never"),
    }
}

/// The moment `at` as a clock on the wall shows it: the time of day when it is today, and the
/// date before it otherwise.
#[must_use]
pub(super) fn stamp(at: i64, now: DateTime) -> String {
    let then = DateTime::from_unix(at, now.offset_minutes);
    let time = format!("{:02}:{:02}", then.time.hour, then.time.minute);
    if then.date == now.date { time } else { format!("{} {time}", then.date) }
}

/// The projects to back up once QCode has closed.
///
/// The application hands this to the runtime and keeps a copy: the runtime takes the
/// application with it when it ends, so the copy is how the list is still in hand after the
/// screen is given back, when the backups are made.
#[derive(Debug, Clone, Default)]
pub struct Farewell(Arc<Mutex<Option<Leaving>>>);

impl Farewell {
    /// Keeps `leaving` as what is backed up once QCode has closed, in place of anything kept
    /// before.
    pub fn keep(&self, leaving: Option<Leaving>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = leaving;
    }

    /// What was kept, which is then no longer kept.
    #[must_use]
    pub fn take(&self) -> Option<Leaving> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The projects open as QCode closes, with the engine that backs them up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaving {
    engine: Engine,
    user: HostUser,
    projects: Vec<Left>,
}

/// One project open as QCode closes: what its last round takes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Left {
    name: String,
    paths: ProjectPaths,
    skip: Vec<String>,
    assets: bool,
    chats: Vec<Chat>,
}

impl Leaving {
    /// The names of the projects, in rail order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.projects.iter().map(|left| left.name.as_str()).collect()
    }

    /// Backs every project up, one after the other, with the conversations of the profiles used
    /// in it, and answers a line for each part that could not be, in the person's language. A
    /// project another QCode is backing up this moment is left to it. The conversations of a
    /// harness running on a database wait for [`back_up_stopped`](Self::back_up_stopped). This
    /// waits for the engine, so it is run once the screen is given back.
    #[must_use]
    pub fn back_up(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for left in &self.projects {
            if let Err(error) = backup::snapshot(&self.engine, &left.paths, self.user, &left.skip) {
                let reason = BackupTrouble::of(&error).words();
                lines.push(t!("project.backup.quit-failed", name = left.name.as_str(), reason = reason));
            }
            let mut missed = back_up_assets(&self.engine, &left.paths, self.user, left.assets);
            let chats: Vec<Chat> =
                left.chats.iter().filter(|chat| chat.harness.conversation_database().is_none()).cloned().collect();
            missed.extend(back_up_chats(&self.engine, &left.paths, self.user, &chats));
            lines.extend(missed.into_iter().map(|missed| line(left, missed)));
        }
        lines
    }

    /// Backs up the conversations of the profiles used whose harness keeps them in a database,
    /// and answers a line for each that could not be. Run after QCode has stopped the containers
    /// it started, when it stops them: one still running is passed by without a word.
    #[must_use]
    pub fn back_up_stopped(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for left in &self.projects {
            let chats: Vec<Chat> =
                left.chats.iter().filter(|chat| chat.harness.conversation_database().is_some()).cloned().collect();
            let missed = back_up_chats(&self.engine, &left.paths, self.user, &chats);
            lines.extend(missed.into_iter().map(|missed| line(left, missed)));
        }
        lines
    }
}

/// The line said on the way out for a part of the project `left` that could not be backed up.
fn line(left: &Left, (part, trouble): (Part, BackupTrouble)) -> String {
    let what = part_title(&part);
    t!("project.backup.quit-part-failed", what = what, name = left.name.as_str(), reason = trouble.words())
}

/// What is backed up when QCode closes with `screen` as it is: every project of the rail, or
/// nothing when backups are off or there is no engine to make them.
#[must_use]
pub fn leaving(screen: &ProjectScreen) -> Option<Leaving> {
    let engine = screen.engine.clone().filter(|_| screen.backup_every != BackupEvery::Off)?;
    let projects: Vec<Left> = screen
        .projects
        .iter()
        .map(|project| Left {
            name: project.name().to_owned(),
            paths: project.paths().clone(),
            skip: project.backup_skip().to_vec(),
            assets: project.assets,
            chats: chats(project, &used(project)),
        })
        .collect();
    (!projects.is_empty()).then_some(Leaving { engine, user: screen.user, projects })
}

/// What a list of backups is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupOf {
    /// The project's own files.
    Project,
    /// `Assets/`.
    Assets,
    /// The conversations of the profile of this name.
    Conversations(String),
}

impl BackupOf {
    /// How the choice of what the list shows names it.
    fn label(&self) -> String {
        match self {
            Self::Project => t!("project.backup.of-project"),
            Self::Assets => t!("project.backup.of-assets"),
            Self::Conversations(profile) => t!("project.backup.of-conversations", profile = profile.as_str()),
        }
    }
}

/// Where a list of backups is read from and brought back into, found out of the project open.
enum Target {
    /// The project's own files, or the one file of this key.
    Project(Option<String>),
    /// `Assets/`.
    Assets,
    /// A profile's conversations: the home they are in and the harness that wrote them.
    Conversations(Home, HarnessKind),
}

impl Target {
    /// The target of the list of `of` in `project`, and of `file` in it when one is named. A
    /// profile gone from the workspace has none: there is no harness to say where its
    /// conversations are.
    fn of(project: &OpenProject, of: &BackupOf, file: Option<&str>) -> Option<Self> {
        match of {
            BackupOf::Project => Some(Self::Project(file.map(str::to_owned))),
            BackupOf::Assets => Some(Self::Assets),
            BackupOf::Conversations(name) => {
                let profile = project.profiles.iter().find(|profile| profile.name.as_str() == name)?;
                Some(Self::Conversations(Home::new(profile.name.clone(), project.id.clone()), profile.harness))
            }
        }
    }

    /// The backups of the target, newest first. Waits for the engine.
    fn history(&self, engine: &Engine, paths: &ProjectPaths, user: HostUser) -> Result<Vec<Entry>, BackupError> {
        match self {
            Self::Project(file) => backup::history(engine, paths, user, file.as_deref()),
            Self::Assets => backup::assets_history(engine, paths, user),
            Self::Conversations(home, _) => conversations::history(engine, paths, home, user),
        }
    }

    /// Brings the backup `id` of the target back. Waits for the engine.
    fn restore(
        &self,
        engine: &Engine,
        paths: &ProjectPaths,
        user: HostUser,
        skip: &[String],
        id: &SnapshotId,
    ) -> Result<Brought, BackupError> {
        match self {
            Self::Project(None) => backup::restore(engine, paths, user, skip, id).map(Brought::Done),
            Self::Project(Some(file)) => backup::restore_file(engine, paths, user, skip, id, file).map(Brought::Done),
            Self::Assets => backup::restore_assets(engine, paths, user, id).map(Brought::Done),
            Self::Conversations(home, harness) => conversations::restore(engine, paths, home, *harness, user, id),
        }
    }
}

/// The list of backups the person opened: of the whole project, of the earlier versions of one
/// of its files, or of a profile's conversations in it.
#[derive(Debug)]
pub(super) struct Listing {
    /// The id of the project whose backups these are.
    project: String,
    /// What the list is of now.
    of: BackupOf,
    /// What else it can be of, in the order the choice offers them; empty for one file's list,
    /// which is of that file only.
    offered: Vec<BackupOf>,
    /// The file whose versions these are, or `None` for the whole project.
    file: Option<String>,
    /// The number of the reading, so an answer for a list closed meanwhile is dropped.
    reading: u64,
    /// The backups newest first, once they are read, or why they could not be.
    entries: Option<Result<Vec<Entry>, BackupTrouble>>,
    /// Whether the reading has run long enough for its indicator to be shown.
    slow: bool,
    /// Whether the indicator, once shown, has been there long enough to be read.
    settled: bool,
    /// The row the keyboard rests on.
    row: usize,
    /// The backup the person chose, while they are asked about it or it is being brought back,
    /// so the question can be asked again when the answer calls for it.
    chosen: Option<Entry>,
    /// Whether the person is being asked about a backup of the list, so a second press while the
    /// question is up does not ask again.
    asking: bool,
    /// Whether a backup of the list is being brought back.
    restoring: bool,
}

/// The name the list of backups is focused by.
pub(super) const LIST_ID: &str = "project-backups";

/// The name the choice of what the list shows is focused by.
pub(super) const OF_ID: &str = "project-backups-of";

/// Width of the dialog of backups, in cells: a date, a time and the longest note beside them,
/// a backup before a restore with its count of files, in either language.
const LIST_WIDTH: u16 = 64;

/// The most rows the list shows at once before it scrolls.
const LIST_ROWS: usize = 10;

/// Opens the list of the open project's backups, or of the earlier versions of its `file`, and
/// reads it on a background thread. Without an engine there is nothing to read them with.
///
/// The list of the whole project can be switched to its assets, when they are backed up, and to
/// the conversations of each profile the project carries; a file's list is of that file only.
pub(super) fn show(screen: &mut ProjectScreen, file: Option<String>) -> Command<Msg> {
    if screen.engine.is_none() {
        return Command::none();
    }
    if file.as_deref().is_some_and(|file| !super::files::is_inside(file)) {
        return Command::none();
    }
    let Some(project) = screen.projects.get(screen.active) else { return Command::none() };
    let offered = if file.is_some() {
        Vec::new()
    } else {
        let carried = project.profiles.iter().filter(|profile| project.carries(profile.name.as_str()));
        std::iter::once(BackupOf::Project)
            .chain(project.assets.then_some(BackupOf::Assets))
            .chain(carried.map(|profile| BackupOf::Conversations(profile.name.as_str().to_owned())))
            .collect()
    };
    screen.listing = Some(Listing {
        project: project.id().to_owned(),
        of: BackupOf::Project,
        offered,
        file,
        reading: 0,
        entries: None,
        slow: false,
        settled: false,
        row: 0,
        chosen: None,
        asking: false,
        restoring: false,
    });
    start_reading(screen)
}

/// Makes the open list one of what it offers at `index`, and reads it. Nothing changes while a
/// backup of it is asked about or being brought back.
pub(super) fn switch(screen: &mut ProjectScreen, index: usize) -> Command<Msg> {
    let Some(listing) = screen.listing.as_mut().filter(|listing| !listing.restoring && !listing.asking) else {
        return Command::none();
    };
    let Some(of) = listing.offered.get(index).cloned() else { return Command::none() };
    if of == listing.of {
        return Command::none();
    }
    listing.of = of;
    start_reading(screen)
}

/// Reads the open list afresh, on a background thread, under a new number: an answer for what it
/// showed before is dropped.
fn start_reading(screen: &mut ProjectScreen) -> Command<Msg> {
    let (Some(engine), user) = (screen.engine.clone(), screen.user) else { return Command::none() };
    screen.last_listing += 1;
    let reading = screen.last_listing;
    let Some(listing) = screen.listing.as_mut() else { return Command::none() };
    listing.reading = reading;
    listing.entries = None;
    listing.slow = false;
    listing.settled = false;
    listing.row = 0;
    let Some(project) = screen.projects.iter().find(|project| project.id() == listing.project) else {
        return Command::none();
    };
    let paths = project.paths().clone();
    let target = Target::of(project, &listing.of, listing.file.as_deref());
    Command::batch([
        Command::perform(move || {
            let found = match target {
                Some(target) => target.history(&engine, &paths, user).map_err(|error| BackupTrouble::of(&error)),
                // Only a profile that left the workspace while the list was open has no target;
                // its conversations cannot be read without its harness, and there are none to show.
                None => Ok(Vec::new()),
            };
            Msg::BackupsRead(reading, found)
        }),
        super::after(super::history::SHOW_AFTER, Msg::BackupsSlow(reading)),
    ])
}

/// The list open now, when it is the one of the reading `reading`.
fn listing(screen: &mut ProjectScreen, reading: u64) -> Option<&mut Listing> {
    screen.listing.as_mut().filter(|listing| listing.reading == reading)
}

/// Takes the backups the reading `reading` found, and hands the list the keyboard.
pub(super) fn read(screen: &mut ProjectScreen, reading: u64, found: Result<Vec<Entry>, BackupTrouble>) -> Command<Msg> {
    let Some(listing) = listing(screen, reading) else { return Command::none() };
    listing.entries = Some(found);
    listing.row = 0;
    Command::focus(LIST_ID)
}

/// The reading `reading` has run long enough for its indicator to be shown; it then stays long
/// enough to be read, so a quick answer never blinks one and a slow one never flickers.
pub(super) fn slow(screen: &mut ProjectScreen, reading: u64) -> Command<Msg> {
    let Some(listing) = listing(screen, reading) else { return Command::none() };
    if listing.entries.is_some() {
        return Command::none();
    }
    listing.slow = true;
    super::after(super::history::SHOW_AT_LEAST, Msg::BackupsSettled(reading))
}

/// The indicator of the reading `reading` has been on screen long enough.
pub(super) fn settled(screen: &mut ProjectScreen, reading: u64) {
    if let Some(listing) = listing(screen, reading) {
        listing.settled = true;
    }
}

/// Puts the keyboard of the list on `row`.
pub(super) fn highlight(screen: &mut ProjectScreen, row: usize) {
    if let Some(listing) = screen.listing.as_mut() {
        listing.row = row;
    }
}

/// Closes the list, unless a backup of it is being brought back: then the list stays until it
/// is done, so the answer has somewhere to land.
pub(super) fn close(screen: &mut ProjectScreen) {
    if screen.listing.as_ref().is_some_and(|listing| !listing.restoring) {
        screen.listing = None;
    }
}

/// Asks before bringing back the backup at `row` of the list: what comes back, that nothing is
/// deleted, and that how things are now is backed up first.
pub(super) fn choose(screen: &mut ProjectScreen, row: usize) -> Command<Msg> {
    let Some(listing) = screen.listing.as_mut().filter(|listing| !listing.restoring && !listing.asking) else {
        return Command::none();
    };
    let Some(Ok(entries)) = &listing.entries else { return Command::none() };
    let Some(entry) = entries.get(row).cloned() else { return Command::none() };
    listing.chosen = Some(entry);
    ask(screen, false)
}

/// Asks about the backup chosen in the open list. The conversations of a profile whose container
/// runs, as the panel last heard, or as the restore itself found when `running` says so, are
/// brought back only after it is stopped, and the question says so and offers that.
fn ask(screen: &mut ProjectScreen, running: bool) -> Command<Msg> {
    let Some(listing) = screen.listing.as_ref() else { return Command::none() };
    let Some(entry) = listing.chosen.clone() else { return Command::none() };
    let time = stamp(entry.at, DateTime::now_local());
    let restore = Msg::RestoreBackup(entry.id.clone());
    let confirm = match (&listing.of, &listing.file) {
        (BackupOf::Project, None) => Confirm::new(t!("project.backup.restore-title", time = time.as_str()), restore)
            .message(t!("project.backup.restore-text"))
            .confirm_label(t!("project.backup.restore")),
        (BackupOf::Project, Some(file)) => {
            let name = super::files::name(file);
            Confirm::new(t!("project.backup.restore-file-title", name = name, time = time.as_str()), restore)
                .message(t!("project.backup.restore-file-text", name = name))
                .confirm_label(t!("project.backup.restore"))
        }
        (BackupOf::Assets, _) => Confirm::new(t!("project.backup.restore-assets-title", time = time.as_str()), restore)
            .message(t!("project.backup.restore-assets-text"))
            .confirm_label(t!("project.backup.restore")),
        (BackupOf::Conversations(profile), _) => {
            let project = screen.projects.iter().find(|project| project.id() == listing.project);
            let container = crate::engine::names::profile_container(&listing.project, profile);
            let up = running
                || project.is_some_and(|project| {
                    project.containers.iter().any(|known| known.name == container && known.state.is_running())
                });
            let title =
                t!("project.backup.restore-conversations-title", profile = profile.as_str(), time = time.as_str());
            let text = t!("project.backup.restore-conversations-text");
            if up {
                let stop = t!("project.backup.restore-conversations-stop", profile = profile.as_str());
                Confirm::new(title, Msg::StopAndRestore(entry.id.clone()))
                    .message(format!("{stop} {text}"))
                    .confirm_label(t!("project.backup.stop-and-restore"))
            } else {
                Confirm::new(title, restore).message(text).confirm_label(t!("project.backup.restore"))
            }
        }
    };
    if let Some(listing) = screen.listing.as_mut() {
        listing.asking = true;
    }
    Command::confirm(confirm.on_cancel(Msg::KeepBackup))
}

/// The person said no to the question about a backup, so a backup can be chosen again.
pub(super) fn declined(screen: &mut ProjectScreen) {
    if let Some(listing) = screen.listing.as_mut() {
        listing.asking = false;
        listing.chosen = None;
    }
}

/// Brings the backup `id` of the list back, on a background thread, the snapshot of how things
/// are now first. With `stop` the profile's container is stopped before, the way the panel's
/// button stops it.
pub(super) fn restore(screen: &mut ProjectScreen, id: SnapshotId, stop: bool) -> Command<Msg> {
    let (Some(engine), user) = (screen.engine.clone(), screen.user) else { return Command::none() };
    let Some(listing) = screen.listing.as_ref().filter(|listing| !listing.restoring) else { return Command::none() };
    let (project_id, of, file) = (listing.project.clone(), listing.of.clone(), listing.file.clone());
    let Some(project) = screen.project_mut(&project_id) else { return Command::none() };
    let Some(target) = Target::of(project, &of, file.as_deref()) else { return Command::none() };
    let (name, paths, skip) = (project.name().to_owned(), project.paths().clone(), project.backup_skip().to_vec());
    let container = match (&target, stop) {
        (Target::Conversations(home, _), true) => Some(home.container()),
        _ => None,
    };
    if let Some(listing) = screen.listing.as_mut() {
        listing.asking = false;
        listing.restoring = true;
    }
    Command::perform(move || {
        let stopped = container.map_or(Ok(()), |container| {
            super::plan::stop(&engine, &container).map_err(|failure| BackupTrouble::Said(failure.output))
        });
        let done = stopped.and_then(|()| {
            target.restore(&engine, &paths, user, &skip, &id).map_err(|error| BackupTrouble::of(&error))
        });
        Msg::Restored { project: project_id, name, of, file, done }
    })
}

/// Takes what bringing a backup back came to: the list closes and the tree reads the folders it
/// shows again when it worked; the list stays, to try again, when it did not. Conversations whose
/// container turned out to be running are asked about again, offering to stop it first.
pub(super) fn restored(
    screen: &mut ProjectScreen,
    id: &str,
    name: &str,
    of: &BackupOf,
    file: Option<&str>,
    done: &Result<Brought, BackupTrouble>,
) -> Command<Msg> {
    let keep = !matches!(done, Ok(Brought::Done(Restored::Done { .. })));
    let mut again = false;
    if let Some(listing) = screen.listing.as_mut().filter(|listing| listing.project == id) {
        listing.restoring = false;
        again = matches!(done, Ok(Brought::Running)) && listing.chosen.is_some();
        if !keep {
            screen.listing = None;
        }
    }
    if again {
        return ask(screen, true);
    }
    let told = match (done, of) {
        (Ok(Brought::Done(Restored::Done { .. })), BackupOf::Conversations(profile)) => {
            Toast::success(t!("project.backup.restored-conversations", profile = profile.as_str()))
                .body(t!("project.backup.restored-text"))
        }
        (Ok(Brought::Done(Restored::Done { .. })), BackupOf::Project) => {
            let title = match file {
                None => t!("project.backup.restored", name = name),
                Some(file) => t!("project.backup.restored-file", name = super::files::name(file)),
            };
            Toast::success(title).body(t!("project.backup.restored-text"))
        }
        (Ok(Brought::Done(Restored::Busy)), _) => Toast::info(t!("project.backup.busy")),
        // The container was found running again after it was asked about once; the list stays.
        (Ok(Brought::Running), _) => Toast::warning(t!("project.backup.still-running")),
        (Ok(Brought::NoHome), BackupOf::Conversations(profile)) => {
            Toast::warning(t!("project.backup.no-home", profile = profile.as_str()))
                .body(t!("project.backup.no-home-text"))
        }
        (Ok(Brought::Done(Restored::Done { .. })), BackupOf::Assets) => {
            Toast::success(t!("project.backup.restored-assets")).body(t!("project.backup.restored-text"))
        }
        (Ok(Brought::NoHome), BackupOf::Project | BackupOf::Assets) => {
            Toast::warning(t!("project.backup.restore-failed"))
        }
        (Err(trouble), _) => Toast::danger(t!("project.backup.restore-failed")).body(trouble.words()),
    };
    let Some(project) = screen.project_mut(id) else { return Command::toast(told) };
    // The snapshot taken before a restore is written into the backup.
    let told = Command::batch([Command::toast(told), read_size(project)]);
    match (done, of) {
        (Ok(Brought::Done(Restored::Done { before: Snapshot::Made { at, .. } })), BackupOf::Project) => {
            project.backup.last = project.backup.last.max(Some(*at));
            Command::batch([told, super::files::refresh(project)])
        }
        (_, BackupOf::Project) => Command::batch([told, super::files::refresh(project)]),
        (_, BackupOf::Assets) => told,
        // A container may have been stopped on the way, which the panel shows.
        (_, BackupOf::Conversations(_)) => Command::batch([told, super::list_containers(screen)]),
    }
}

/// The dialog of backups, while one is open. It waits for an answer rather than sitting beside
/// the screen: choosing a backup is all there is to do until it is chosen or closed.
pub(super) fn view(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let Some(listing) = &screen.listing else { return };
    let title = match &listing.file {
        None => t!("project.backup.list-title"),
        Some(file) => t!("project.backup.versions-title", name = super::files::name(file)),
    };
    let entries = match &listing.entries {
        Some(Ok(entries)) if !listing.slow || listing.settled => Some(entries.as_slice()),
        _ => None,
    };
    let row = listing.row.min(entries.map_or(0, |entries| entries.len().saturating_sub(1)));
    let bring = Button::new(t!("project.backup.restore"))
        .variant("primary")
        .loading(listing.restoring)
        .disabled(entries.is_none_or(<[Entry]>::is_empty) || listing.restoring)
        .on_press(Msg::ChooseBackup(row));
    let dialog = Modal::new()
        .title(title)
        .width(LIST_WIDTH)
        .on_close(Msg::CloseBackups)
        .action(Button::new(t!("project.backup.close")).on_press(Msg::CloseBackups))
        .action(bring);
    ui.add_with(dialog, |ui| {
        // A project with no profile has only its own files to offer, and no choice to make.
        if listing.offered.len() > 1 {
            let labels = listing.offered.iter().map(BackupOf::label);
            let chosen = listing.offered.iter().position(|of| *of == listing.of);
            ui.add(Select::new(labels).selected(chosen).disabled(listing.restoring).on_select(Msg::BackupsOf))
                .id(OF_ID)
                .fill_width();
            // A line apart, so the choice does not read as the first row of the list.
            ui.spacer().height(Length::Cells(1));
        }
        contents(listing, entries, row, ui);
    });
}

/// What the dialog of backups shows below its choice: the list, why it could not be read, or
/// that it is being read.
fn contents(listing: &Listing, entries: Option<&[Entry]>, row: usize, ui: &mut View<'_, Msg>) {
    match (&listing.entries, entries) {
        (Some(Err(trouble)), _) if !listing.slow || listing.settled => {
            ui.add(Text::new(t!("project.backup.unread")).role("secondary"));
            ui.add(Text::new(trouble.words()).role("faint")).selectable(true);
        }
        (_, Some(entries)) => {
            let now = DateTime::now_local();
            let whole = listing.file.is_none();
            let items = entries.iter().map(|entry| ListItem::new(stamp(entry.at, now)).detail(detail(entry, whole)));
            let empty = match (&listing.of, whole) {
                (BackupOf::Conversations(_), _) => t!("project.backup.no-conversations"),
                (BackupOf::Assets, _) => t!("project.backup.no-assets"),
                (BackupOf::Project, true) => t!("project.backup.none"),
                (BackupOf::Project, false) => t!("project.backup.no-versions"),
            };
            let height = u16::try_from(entries.len().clamp(1, LIST_ROWS)).unwrap_or(1);
            ui.add(
                List::new(items)
                    .selected(Some(row))
                    .empty_text(empty)
                    .on_select(Msg::HighlightBackup)
                    .on_activate(Msg::ChooseBackup),
            )
            .id(LIST_ID)
            .fill_width()
            .height(Length::Cells(height));
        }
        _ if listing.slow => {
            ui.add(Spinner::new().label(t!("project.backup.reading")));
        }
        // Not drawn during its first moment, so a quick answer never blinks an indicator.
        _ => {
            ui.spacer().height(Length::Cells(1));
        }
    }
}

/// What a row of the list says beside its time: how many files the backup changed, for the
/// whole project, and whether it was the one taken just before a restore.
fn detail(entry: &Entry, whole: bool) -> String {
    match (entry.reason, whole) {
        (Reason::BeforeRestore, true) => t!("project.backup.before-restore-changed", n = entry.changed),
        (Reason::BeforeRestore, false) => t!("project.backup.before-restore"),
        (Reason::Scheduled, true) => t!("project.backup.changed", n = entry.changed),
        (Reason::Scheduled, false) => String::new(),
    }
}
