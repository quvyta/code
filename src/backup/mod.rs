//! The project backup: frequent snapshots of `Project/` in a git folder of its own under
//! `Backup/`, and bringing the project or one file back from any of them.
//!
//! ```text
//! Projects/<project>/
//!   Backup/
//!     Project.git/     the snapshots, git's own folder with no working tree
//!     backup.lock      held while a snapshot or a restore runs
//! ```
//!
//! The backup sits beside the project, so it moves and is copied with it. It guards against a
//! wrong delete, a change that breaks things and a harness scattering files, not against a
//! failing disk.
//!
//! git does not run on the machine: QCode promises to run no program there, and the machine may
//! have no git. Every job runs in a one-off container of the base image that sees the project
//! and `Backup/` and removes itself when git is done (the `job` module has the commands). The person's
//! own repository inside `Project/`, if there is one, is never touched: the backup has a git
//! folder of its own, and git never takes a folder called `.git` into a snapshot.
//!
//! Everything public here runs engine commands and waits for them, so it belongs on a
//! background thread.

mod job;
#[cfg(test)]
mod live;
mod place;

pub use place::{BadPlace, Place, PlaceProblem};

use std::io;
use std::path::Path;
use std::time::Duration;

use qframe::storage::InstanceLock;

use crate::base;
use crate::engine::run::{EngineError, capture};
use crate::engine::{Engine, HostUser};
use crate::workspace::ProjectPaths;

/// The name of the lock file inside `Backup/`.
const LOCK: &str = "backup.lock";

/// How often an open project is backed up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackupEvery {
    /// Never on a timer.
    Off,
    /// Every five minutes.
    Five,
    /// Every fifteen minutes. A round with no change writes nothing, so the cost of a short
    /// interval is one container start; fifteen minutes keeps the work a session can lose small
    /// without waking a laptop on battery every minute.
    #[default]
    Fifteen,
    /// Every hour.
    Hour,
}

impl BackupEvery {
    /// Every choice, in the order the settings screen offers them.
    pub const ALL: [Self; 4] = [Self::Off, Self::Five, Self::Fifteen, Self::Hour];

    /// How the choice is written in the config file.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Five => "5m",
            Self::Fifteen => "15m",
            Self::Hour => "1h",
        }
    }

    /// The choice a config file names, if it names one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.key() == key)
    }

    /// The time between two backups, or `None` when there is no timer.
    #[must_use]
    pub fn interval(self) -> Option<Duration> {
        match self {
            Self::Off => None,
            Self::Five => Some(Duration::from_secs(5 * 60)),
            Self::Fifteen => Some(Duration::from_secs(15 * 60)),
            Self::Hour => Some(Duration::from_secs(60 * 60)),
        }
    }
}

/// Why a snapshot was taken, kept as its commit message so the list can say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// On the timer, or as the project closed.
    Scheduled,
    /// Just before a restore, so that the restore can be undone too.
    BeforeRestore,
}

impl Reason {
    /// The commit message. It is a key the list reads back, not text the person is shown.
    fn message(self) -> &'static str {
        match self {
            Self::Scheduled => "backup",
            Self::BeforeRestore => "before restore",
        }
    }

    /// The reason a commit message names; anything else was an ordinary backup.
    fn from_message(message: &str) -> Self {
        if message == Self::BeforeRestore.message() { Self::BeforeRestore } else { Self::Scheduled }
    }
}

/// The id of one snapshot: git's commit id, in full.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapshotId(String);

impl SnapshotId {
    /// `id` as a snapshot id, when it is a whole commit id: 40 lowercase hex digits, or 64 for
    /// a repository that uses SHA-256.
    ///
    /// Held to that shape because it goes on git's command line, where a word starting with `-`
    /// or naming a branch would be read as something else.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        let hex = id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        (hex && (id.len() == 40 || id.len() == 64)).then(|| Self(id.to_owned()))
    }

    /// The id as git writes it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What one round of [`snapshot`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Snapshot {
    /// A snapshot was made.
    Made {
        /// Its id.
        id: SnapshotId,
        /// When, in seconds since the Unix epoch.
        at: i64,
    },
    /// Nothing changed since the last one, so nothing was written.
    Unchanged,
    /// Another QCode with the same project open is backing it up right now; this round is
    /// skipped, because that one covers it.
    Busy,
}

/// What [`restore`] and [`restore_file`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restored {
    /// The files are back. `before` is the snapshot of how they were just before, which is
    /// how a restore is undone; [`Snapshot::Unchanged`] when the last snapshot already was
    /// that.
    Done {
        /// The snapshot taken first.
        before: Snapshot,
    },
    /// Another QCode is backing the project up right now; nothing was touched.
    Busy,
}

/// One snapshot in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its id, which [`restore`] takes.
    pub id: SnapshotId,
    /// When it was made, in seconds since the Unix epoch.
    pub at: i64,
    /// Why it was made.
    pub reason: Reason,
    /// How many files it changed from the one before, or of the one file asked after.
    pub changed: usize,
}

/// Why a backup job did not do what was asked.
#[derive(Debug)]
pub enum BackupError {
    /// A skip-list entry or a file to restore is not a path inside the project.
    Place(BadPlace),
    /// `Backup/` or its lock could not be made.
    Host(io::Error),
    /// The base image, which the job runs in, is missing and could not be built.
    Base(base::Failure),
    /// The engine or git refused; the engine's and git's own words are in it.
    Engine(EngineError),
    /// git answered, but not in a way the snapshot understands; its whole answer.
    Answer(String),
}

impl From<EngineError> for BackupError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

/// Backs the project up, leaving out every folder or file of `skip` as well as what the
/// project's own `.gitignore` leaves out.
///
/// Makes `Backup/` the first time. When another QCode is backing the same project up at this
/// moment the round is skipped: the two would only race for git's own lock.
///
/// # Errors
///
/// An entry of `skip` that is not a path inside the project, before anything runs; a
/// `Backup/` that cannot be made; and the engine's or git's own words when the job fails.
pub fn snapshot(
    engine: &Engine,
    paths: &ProjectPaths,
    user: HostUser,
    skip: &[String],
) -> Result<Snapshot, BackupError> {
    let exclude = place::exclude_file(&place::places(skip).map_err(BackupError::Place)?);
    let Some(_lock) = hold(paths)? else { return Ok(Snapshot::Busy) };
    take(engine, paths, user, &exclude, Reason::Scheduled)
}

/// The snapshots of the project, newest first; those that changed `file` when one is named.
/// A project that has never been backed up has none, and no container is started to say so.
///
/// # Errors
///
/// A `file` that is not a path inside the project, and the engine's or git's own words when the
/// listing fails.
pub fn history(
    engine: &Engine,
    paths: &ProjectPaths,
    user: HostUser,
    file: Option<&str>,
) -> Result<Vec<Entry>, BackupError> {
    let file = file.map(Place::new).transpose().map_err(BackupError::Place)?;
    if !paths.backup().join(job::PROJECT_GIT).is_dir() {
        return Ok(Vec::new());
    }
    let output = capture(&job::history(engine, paths, user, file.as_ref()))?;
    Ok(job::read_history(&output))
}

/// Brings every file of the snapshot `id` back into the project, after a snapshot of how the
/// project is now.
///
/// Nothing is deleted: a file made after `id` stays where it is. The person removes it if they
/// want it gone; losing something by accident is worse than one file too many.
///
/// # Errors
///
/// As [`snapshot`], and the engine's or git's own words when the restore fails — an `id` the
/// backup does not have among them.
pub fn restore(
    engine: &Engine,
    paths: &ProjectPaths,
    user: HostUser,
    skip: &[String],
    id: &SnapshotId,
) -> Result<Restored, BackupError> {
    bring_back(engine, paths, user, skip, id, None)
}

/// Brings `file` back as it was in the snapshot `id`, after a snapshot of how the project is
/// now. The rest of the project is left as it is.
///
/// # Errors
///
/// A `file` that is not a path inside the project, and everything [`restore`] fails for.
pub fn restore_file(
    engine: &Engine,
    paths: &ProjectPaths,
    user: HostUser,
    skip: &[String],
    id: &SnapshotId,
    file: &str,
) -> Result<Restored, BackupError> {
    let file = Place::new(file).map_err(BackupError::Place)?;
    bring_back(engine, paths, user, skip, id, Some(&file))
}

/// The restore both [`restore`] and [`restore_file`] are, under one hold of the lock so that
/// no other backup runs between the snapshot before and the restore itself.
fn bring_back(
    engine: &Engine,
    paths: &ProjectPaths,
    user: HostUser,
    skip: &[String],
    id: &SnapshotId,
    what: Option<&Place>,
) -> Result<Restored, BackupError> {
    let exclude = place::exclude_file(&place::places(skip).map_err(BackupError::Place)?);
    let Some(_lock) = hold(paths)? else { return Ok(Restored::Busy) };
    let before = take(engine, paths, user, &exclude, Reason::BeforeRestore)?;
    capture(&job::restore(engine, paths, user, id, what))?;
    Ok(Restored::Done { before })
}

/// Runs a snapshot job, with the lock already held.
fn take(
    engine: &Engine,
    paths: &ProjectPaths,
    user: HostUser,
    exclude: &str,
    reason: Reason,
) -> Result<Snapshot, BackupError> {
    base::ensure(engine, &|| false, &mut |_| {}).map_err(BackupError::Base)?;
    let output = capture(&job::snapshot(engine, paths, user, exclude, reason))?;
    Ok(match job::read_snapshot(&output).map_err(BackupError::Answer)? {
        Some((id, at)) => Snapshot::Made { id, at },
        None => Snapshot::Unchanged,
    })
}

/// A held lock, or none on a platform without one.
type Held = Option<InstanceLock>;

/// Makes `Backup/` and takes its lock: `Some` to go on, `None` when another QCode holds it.
///
/// Where the platform has no advisory lock (Windows) the job goes on without one; there git's
/// own `index.lock` stops a second one, which then fails that round instead of skipping it.
fn hold(paths: &ProjectPaths) -> Result<Option<Held>, BackupError> {
    let backup = paths.backup();
    std::fs::create_dir_all(&backup).map_err(BackupError::Host)?;
    lock(&backup.join(LOCK))
}

/// Takes the lock at `path`, as [`hold`] describes.
fn lock(path: &Path) -> Result<Option<Held>, BackupError> {
    match InstanceLock::try_exclusive(path) {
        Ok(Some(lock)) => Ok(Some(Some(lock))),
        Ok(None) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => Ok(Some(None)),
        Err(error) => Err(BackupError::Host(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::{BackupError, BackupEvery, Reason, Snapshot, SnapshotId, history, hold, snapshot};
    use crate::engine::{Engine, EngineKind, HostUser};
    use crate::workspace::ProjectPaths;
    use std::path::PathBuf;
    use std::time::Duration;

    /// A project folder of this test's own, removed again when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let stamp =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path = std::env::temp_dir().join(format!("qcode-backup-{name}-{stamp}"));
            std::fs::create_dir_all(path.join("Project")).expect("a project folder");
            Self(path)
        }

        fn paths(&self) -> ProjectPaths {
            ProjectPaths {
                root: self.0.clone(),
                file: self.0.join("project.qcode"),
                project: self.0.join("Project"),
                assets: self.0.join("Assets"),
                harness: self.0.join("Containers").join("Harness"),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An engine that is never there: a test that reaches it has run a command it should not.
    fn missing() -> Engine {
        Engine::new(EngineKind::Podman, "/qcode/no/such/engine")
    }

    const ME: HostUser = HostUser::Ids { uid: 1000, gid: 1000 };

    #[test]
    fn the_backup_is_every_fifteen_minutes_until_the_person_chooses() {
        assert_eq!(BackupEvery::default(), BackupEvery::Fifteen);
        assert_eq!(BackupEvery::Fifteen.interval(), Some(Duration::from_secs(900)));
        assert_eq!(BackupEvery::Off.interval(), None);
        for choice in BackupEvery::ALL {
            assert_eq!(BackupEvery::from_key(choice.key()), Some(choice));
        }
        assert_eq!(BackupEvery::from_key("1m"), None);
    }

    #[test]
    fn a_snapshot_id_is_a_whole_commit_id_and_nothing_git_could_read_as_more() {
        assert!(SnapshotId::parse(&"a".repeat(40)).is_some());
        assert!(SnapshotId::parse(&"0".repeat(64)).is_some());
        assert!(SnapshotId::parse("abc123").is_none(), "an abbreviation can become ambiguous");
        assert!(SnapshotId::parse(&"A".repeat(40)).is_none());
        assert!(SnapshotId::parse(&format!("-{}", "a".repeat(39))).is_none());
        assert!(SnapshotId::parse("HEAD").is_none());
    }

    #[test]
    fn the_reason_survives_the_commit_message() {
        for reason in [Reason::Scheduled, Reason::BeforeRestore] {
            assert_eq!(Reason::from_message(reason.message()), reason);
        }
    }

    #[test]
    fn a_bad_skip_entry_stops_the_snapshot_before_anything_is_made() {
        let scratch = Scratch::new("skip");
        let error = snapshot(&missing(), &scratch.paths(), ME, &["../elsewhere".to_owned()]).expect_err("refused");
        assert!(matches!(error, BackupError::Place(_)), "{error:?}");
        assert!(!scratch.paths().backup().exists(), "Backup/ was made for nothing");
    }

    #[test]
    fn a_project_never_backed_up_has_no_history_and_starts_no_container() {
        let scratch = Scratch::new("history");
        assert_eq!(history(&missing(), &scratch.paths(), ME, None).expect("nothing to ask"), []);
        assert!(matches!(history(&missing(), &scratch.paths(), ME, Some("/etc")), Err(BackupError::Place(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_second_backup_of_the_same_project_skips_its_round() {
        let scratch = Scratch::new("lock");
        let first = hold(&scratch.paths()).expect("Backup/ is made").expect("nobody holds it");
        assert!(scratch.paths().backup().join("backup.lock").is_file());
        let second = snapshot(&missing(), &scratch.paths(), ME, &[]).expect("skipped, not failed");
        assert_eq!(second, Snapshot::Busy);
        drop(first);
        // A program another test thread is starting holds a copy of the lock between `fork` and
        // `exec`, so the lock is free once that copy closes too, a moment later.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while hold(&scratch.paths()).expect("asked").is_none() {
            assert!(std::time::Instant::now() < deadline, "the lock stayed with the first holder");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
