//! The conversations backup: each profile's conversations in a workspace, kept apart from the
//! workspace's own backup and brought back from any snapshot.
//!
//! ```text
//! Workspaces/<workspace>/
//!   Backup/
//!     Conversations/
//!       <profile>.git    the snapshots of one profile's conversations
//!       <profile>.lock   held while a snapshot or a restore of them runs
//! ```
//!
//! A harness keeps its conversations in the workspace's home volume for the profile, which the
//! machine cannot read by itself, so every job is a one-off container of the base image that
//! mounts the volume at the home directory and `Backup/` beside it. git runs there with the
//! settings the workspace's backup gives it, with the home as its tree, and takes only the paths
//! [`HarnessKind::conversation_paths`] names: the home also holds the login, and `Backup/` is a
//! plain folder on the machine.
//!
//! Each profile has a lock of its own rather than the workspace's `backup.lock`. The git folders
//! are separate, so nothing is gained by one waiting for another, and a round that found the
//! workspace's lock taken would be skipped as covered when nothing covers it.
//!
//! Everything public here runs engine commands and waits for them, so it belongs on a
//! background thread.

#[cfg(test)]
mod live;

use std::path::{Path, PathBuf};

use super::job::{preamble, read_history, read_snapshot};
use super::{BackupError, Entry, Held, Reason, Restored, Snapshot, SnapshotId, lock};
use crate::base;
use crate::base::paths::{BACKUP_DIR, HOME_DIR};
use crate::engine::names::BASE_IMAGE;
use crate::engine::run::capture;
use crate::engine::{Access, ContainerState, Engine, EngineCommand, HostUser, Mount, MountSource, Network, RunOnce};
use crate::profile::HarnessKind;
use crate::profile::identity::{self, Home};
use crate::store::WorkspacePaths;

/// The folder inside `Backup/` that holds every profile's git folder and lock.
const FOLDER: &str = "Conversations";

/// The preamble of every script here, and the one thing added to it: git reads no settings
/// from the home it looks at. A `.gitconfig` there was written by whatever ran in the profile's
/// container, and a setting such as `core.fsmonitor` names a program git would run, in a job
/// that can write to every backup of the workspace.
macro_rules! start {
    () => {
        concat!(preamble!(), "export HOME=/tmp XDG_CONFIG_HOME=/tmp GIT_CONFIG_GLOBAL=/dev/null\n")
    };
}

/// Takes a snapshot. `$1` is the commit message, the rest the paths to take.
///
/// A path is taken when it is there, or when the last snapshot had it, so that a file gone
/// since (a journal SQLite folded back into its database) leaves the snapshot too. A path that
/// is neither would make git stop at an unknown pathspec, so it is dropped first. The answer is
/// read as the workspace's is: `made <id> <time>` or `unchanged`.
const SNAPSHOT: &str = concat!(
    start!(),
    "m=\"$1\"; shift\n",
    "[ -d \"$d\" ] || git -c init.defaultBranch=backup init --quiet --bare \"$d\"\n",
    "n=$#\n",
    "for p in \"$@\"; do\n",
    "  if [ -e \"$p\" ] || [ -L \"$p\" ] || g ls-files --error-unmatch -- \"$p\" > /dev/null 2>&1; then\n",
    "    set -- \"$@\" \"$p\"\n",
    "  fi\n",
    "done\n",
    "shift \"$n\"\n",
    "[ \"$#\" -eq 0 ] || g add -A -- \"$@\"\n",
    "if g diff --cached --quiet; then\n",
    "  echo unchanged\n",
    "else\n",
    "  g commit --quiet -m \"$m\"\n",
    "  g log -1 --format='made %H %ct'\n",
    "fi\n",
    "g gc --auto --quiet\n",
);

/// Lists the snapshots, newest first, in the shape the workspace's list has.
const HISTORY: &str = concat!(
    start!(),
    "[ -d \"$d\" ] || exit 0\n",
    "g rev-parse --quiet --verify HEAD > /dev/null || exit 0\n",
    "g log --format='%x1e%H%x09%ct%x09%s' --shortstat\n",
);

/// Brings the snapshot `$1` back. `$2` is the harness's database, empty when it has none, and
/// the rest are its journals.
///
/// `--overlay`, as for the workspace: a conversation the snapshot does not have stays. The one
/// thing taken away is a journal the snapshot did not have, and only when the snapshot has the
/// database: a journal belongs to the database it was written with and would be replayed into
/// an older one. The snapshot taken just before holds it.
const RESTORE: &str = concat!(
    start!(),
    "id=\"$1\"; db=\"$2\"; shift 2\n",
    "if [ -n \"$db\" ] && g cat-file -e \"$id:$db\" 2> /dev/null; then\n",
    "  for p in \"$@\"; do g cat-file -e \"$id:$p\" 2> /dev/null || rm -f -- \"$p\"; done\n",
    "fi\n",
    "[ -z \"$(g ls-tree \"$id\")\" ] || g checkout --quiet --overlay \"$id\" -- .\n",
);

/// What one round of [`snapshot`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taken {
    /// The round ran, or was skipped for another QCode running it, as the workspace's does.
    Done(Snapshot),
    /// The harness keeps its conversations in a database and its container is running, so a
    /// copy could be torn. Nothing was touched; the round belongs to when the container stops,
    /// as the workspace closes or QCode quits.
    Running,
}

/// What [`restore`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Brought {
    /// The restore ran, or was skipped for another QCode backing the same profile up.
    Done(Restored),
    /// The profile's container is running, and the files of a running harness are not changed
    /// under it. Nothing was touched; the container is stopped first.
    Running,
    /// The workspace has no home for the profile any more, so there is nowhere to bring the
    /// conversations back to. Nothing was made: a home that comes into being here would never
    /// be given the profile's login.
    NoHome,
}

/// Backs up the conversations `harness` had in `home`.
///
/// A home that is not there yet has had no conversations, and nothing is made to find that
/// out: an engine makes a volume a container names, and a home made that way would never be
/// given the profile's login.
///
/// # Errors
///
/// A `Backup/` that cannot be made, and the engine's or git's own words when the job fails.
pub fn snapshot(
    engine: &Engine,
    paths: &WorkspacePaths,
    home: &Home,
    harness: HarnessKind,
    user: HostUser,
) -> Result<Taken, BackupError> {
    let Some(_lock) = hold(paths, home)? else { return Ok(Taken::Done(Snapshot::Busy)) };
    if harness.conversation_database().is_some() && running(engine, home) {
        return Ok(Taken::Running);
    }
    if !identity::home_exists(engine, home)? {
        return Ok(Taken::Done(Snapshot::Unchanged));
    }
    take(engine, paths, home, harness, user, Reason::Scheduled).map(Taken::Done)
}

/// The snapshots of the profile's conversations, newest first. A profile never backed up has
/// none, and no container is started to say so.
///
/// # Errors
///
/// The engine's or git's own words when the listing fails.
pub fn history(
    engine: &Engine,
    paths: &WorkspacePaths,
    home: &Home,
    user: HostUser,
) -> Result<Vec<Entry>, BackupError> {
    if !git_dir(paths, home).is_dir() {
        return Ok(Vec::new());
    }
    Ok(read_history(&capture(&history_job(engine, paths, home, user))?))
}

/// Brings the conversations of the snapshot `id` back into `home`, after a snapshot of how they
/// are now, while the profile's container is stopped.
///
/// A conversation made after `id` stays, as a file made after a workspace snapshot does.
///
/// # Errors
///
/// As [`snapshot`], and the engine's or git's own words when the restore fails — an `id` the
/// backup does not have among them.
pub fn restore(
    engine: &Engine,
    paths: &WorkspacePaths,
    home: &Home,
    harness: HarnessKind,
    user: HostUser,
    id: &SnapshotId,
) -> Result<Brought, BackupError> {
    let Some(_lock) = hold(paths, home)? else { return Ok(Brought::Done(Restored::Busy)) };
    if running(engine, home) {
        return Ok(Brought::Running);
    }
    if !identity::home_exists(engine, home)? {
        return Ok(Brought::NoHome);
    }
    let before = take(engine, paths, home, harness, user, Reason::BeforeRestore)?;
    capture(&restore_job(engine, paths, home, harness, user, id))?;
    Ok(Brought::Done(Restored::Done { before }))
}

/// Runs a snapshot job, with the lock already held.
fn take(
    engine: &Engine,
    paths: &WorkspacePaths,
    home: &Home,
    harness: HarnessKind,
    user: HostUser,
    reason: Reason,
) -> Result<Snapshot, BackupError> {
    base::ensure(engine, &|| false, &mut |_| {}).map_err(BackupError::Base)?;
    let output = capture(&snapshot_job(engine, paths, home, harness, user, reason))?;
    Ok(match read_snapshot(&output).map_err(BackupError::Answer)? {
        Some((id, at)) => Snapshot::Made { id, at },
        None => Snapshot::Unchanged,
    })
}

/// Whether the profile's container is up, or anywhere it could be writing.
///
/// Only a container that is not there, was never started or has stopped counts as stopped; a
/// paused one is frozen in the middle of whatever it was writing. An engine that cannot answer
/// counts as stopped, because the job it would guard cannot run either. The container could
/// start between this question and the job; QCode only starts it for a tab, and a tab is not
/// opened on a profile while its restore runs.
fn running(engine: &Engine, home: &Home) -> bool {
    let state = capture(&engine.container_state(&home.container())).ok().map(|word| ContainerState::parse(&word));
    !matches!(state, None | Some(ContainerState::Created | ContainerState::Exited | ContainerState::Dead))
}

/// Makes `Backup/Conversations/` and takes the profile's lock there, as the workspace's
/// [`hold`](super::hold) does.
fn hold(paths: &WorkspacePaths, home: &Home) -> Result<Option<Held>, BackupError> {
    let folder = paths.backup().join(FOLDER);
    std::fs::create_dir_all(&folder).map_err(BackupError::Host)?;
    lock(&folder.join(format!("{}.lock", home.profile())))
}

/// The profile's git folder on the machine.
fn git_dir(paths: &WorkspacePaths, home: &Home) -> PathBuf {
    paths.backup().join(FOLDER).join(format!("{}.git", home.profile()))
}

/// The command that takes a snapshot of `harness`'s conversations in `home`.
fn snapshot_job(
    engine: &Engine,
    paths: &WorkspacePaths,
    home: &Home,
    harness: HarnessKind,
    user: HostUser,
    reason: Reason,
) -> EngineCommand {
    let mut args = vec![reason.message()];
    args.extend_from_slice(harness.conversation_paths());
    one_off(engine, paths, home, Some(Access::ReadOnly), user, SNAPSHOT, &args)
}

/// The command that lists the snapshots. It has no use for the home and does not mount it, so
/// a home that is gone is not made again by asking.
fn history_job(engine: &Engine, paths: &WorkspacePaths, home: &Home, user: HostUser) -> EngineCommand {
    one_off(engine, paths, home, None, user, HISTORY, &[])
}

/// The command that brings the snapshot `id` back into `home`.
fn restore_job(
    engine: &Engine,
    paths: &WorkspacePaths,
    home: &Home,
    harness: HarnessKind,
    user: HostUser,
    id: &SnapshotId,
) -> EngineCommand {
    let database = harness.conversation_database().unwrap_or_default();
    let journals: Vec<String> = harness
        .conversation_database()
        .map_or_else(Vec::new, |database| ["-wal", "-shm"].map(|journal| format!("{database}{journal}")).to_vec());
    let mut args = vec![id.as_str(), database];
    args.extend(journals.iter().map(String::as_str));
    one_off(engine, paths, home, Some(Access::ReadWrite), user, RESTORE, &args)
}

/// A one-off container of the base image with `Backup/` and, when `access` says how, the
/// profile's home, reaching no network.
///
/// It starts in the home, because git reads the paths it is given as relative to where it is.
fn one_off(
    engine: &Engine,
    paths: &WorkspacePaths,
    home: &Home,
    access: Option<Access>,
    user: HostUser,
    script: &str,
    args: &[&str],
) -> EngineCommand {
    let backup = paths.backup();
    let volume = home.volume();
    let mut mounts = Vec::new();
    if let Some(access) = access {
        mounts.push(Mount { source: MountSource::Volume(&volume), target: Path::new(HOME_DIR), access });
    }
    mounts.push(Mount { source: MountSource::Path(&backup), target: Path::new(BACKUP_DIR), access: Access::ReadWrite });
    let git_dir = format!("{BACKUP_DIR}/{FOLDER}/{}.git", home.profile());
    let mut command = vec!["sh", "-c", script, "sh", &git_dir, HOME_DIR];
    command.extend_from_slice(args);
    engine.run_once(&RunOnce {
        image: BASE_IMAGE,
        mounts: &mounts,
        network: Network::None,
        user,
        workdir: Some(Path::new(HOME_DIR)),
        command: &command,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Brought, HISTORY, RESTORE, SNAPSHOT, Taken, history, history_job, restore, restore_job, snapshot, snapshot_job,
    };
    use crate::backup::{BackupError, Reason, Restored, Snapshot, SnapshotId};
    use crate::engine::{Engine, EngineCommand, EngineKind, HostUser};
    use crate::profile::identity::Home;
    use crate::profile::{HarnessKind, SafeName};
    use crate::store::{WorkspaceId, WorkspacePaths};
    use std::path::PathBuf;

    const ID: &str = "0123456789abcdef0123456789abcdef01234567";
    const ME: HostUser = HostUser::Ids { uid: 1000, gid: 1000 };

    fn paths_at(root: PathBuf) -> WorkspacePaths {
        WorkspacePaths {
            file: root.join("workspace.qcode"),
            code: root.join("Work"),
            assets: root.join("Assets"),
            harness: root.join("Containers").join("Harness"),
            root,
        }
    }

    fn paths() -> WorkspacePaths {
        paths_at(PathBuf::from("/home/me/QCode/Workspaces/p"))
    }

    fn home() -> Home {
        Home::new(SafeName::parse("chat").expect("a name"), WorkspaceId::parse("p").expect("an id"))
    }

    fn podman() -> Engine {
        Engine::new(EngineKind::Podman, "/usr/bin/podman")
    }

    fn args(command: &EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    /// The words every job starts with, up to and including the script; `home` is how the home
    /// is mounted, when it is.
    fn container(home: Option<&str>, script: &str) -> Vec<String> {
        let mut words = vec!["run", "--rm", "--userns=keep-id", "--network=none"];
        let volume = home.map(|access| format!("qcode-home-p-chat:/home/qcode:{access},z"));
        if let Some(volume) = &volume {
            words.extend(["--volume", volume]);
        }
        words.extend([
            "--volume",
            "/home/me/QCode/Workspaces/p/Backup:/backup:rw,z",
            "--workdir",
            "/home/qcode",
            "--pull=never",
            "qcode/base",
            "sh",
            "-c",
            script,
            "sh",
            "/backup/Conversations/chat.git",
            "/home/qcode",
        ]);
        words.into_iter().map(str::to_owned).collect()
    }

    #[test]
    fn a_snapshot_reads_the_home_and_names_only_the_conversations() {
        let command = snapshot_job(&podman(), &paths(), &home(), HarnessKind::ClaudeCode, ME, Reason::Scheduled);
        let mut expected = container(Some("ro"), SNAPSHOT);
        expected.extend(["backup", ".claude/projects"].map(str::to_owned));
        assert_eq!(args(&command), expected);

        let command = snapshot_job(&podman(), &paths(), &home(), HarnessKind::OpenCode, ME, Reason::BeforeRestore);
        let mut expected = container(Some("ro"), SNAPSHOT);
        expected.push("before restore".to_owned());
        expected.extend(HarnessKind::OpenCode.conversation_paths().iter().map(|path| (*path).to_owned()));
        assert_eq!(args(&command), expected);
    }

    #[test]
    fn every_harness_is_backed_up_from_where_it_writes() {
        let taken = |harness| args(&snapshot_job(&podman(), &paths(), &home(), harness, ME, Reason::Scheduled));
        let tail = |harness| taken(harness).split_off(container(Some("ro"), SNAPSHOT).len() + 1);
        assert_eq!(tail(HarnessKind::ClaudeCode), [".claude/projects"]);
        assert_eq!(
            tail(HarnessKind::OpenCode),
            [
                ".local/share/opencode/opencode.db",
                ".local/share/opencode/opencode.db-wal",
                ".local/share/opencode/opencode.db-shm"
            ]
        );
        assert_eq!(tail(HarnessKind::GeminiCli), [".gemini/tmp", ".gemini/projects.json"]);
        assert_eq!(tail(HarnessKind::Codex), [".codex/sessions", ".codex/session_index.jsonl"]);
        for harness in HarnessKind::ALL {
            let words = taken(harness);
            for login in harness.record().identity {
                assert!(!words.iter().any(|word| word == login), "{harness:?} takes its login {login}");
            }
        }
    }

    #[test]
    fn the_list_never_mounts_the_home() {
        assert_eq!(args(&history_job(&podman(), &paths(), &home(), ME)), container(None, HISTORY));
    }

    #[test]
    fn a_restore_writes_the_home_and_names_a_database_and_its_journals() {
        let id = SnapshotId::parse(ID).expect("an id");
        let mut expected = container(Some("rw"), RESTORE);
        expected.extend([ID, ""].map(str::to_owned));
        assert_eq!(args(&restore_job(&podman(), &paths(), &home(), HarnessKind::Codex, ME, &id)), expected);

        let mut expected = container(Some("rw"), RESTORE);
        expected.extend(
            [
                ID,
                ".local/share/opencode/opencode.db",
                ".local/share/opencode/opencode.db-wal",
                ".local/share/opencode/opencode.db-shm",
            ]
            .map(str::to_owned),
        );
        assert_eq!(args(&restore_job(&podman(), &paths(), &home(), HarnessKind::OpenCode, ME, &id)), expected);
    }

    #[test]
    fn every_script_reads_no_settings_from_the_home_and_deletes_no_conversation() {
        for script in [SNAPSHOT, HISTORY, RESTORE] {
            assert!(script.contains("--git-dir=\"$d\""), "{script}");
            assert!(script.contains("--literal-pathspecs"), "{script}");
            assert!(script.contains("GIT_CONFIG_GLOBAL=/dev/null"), "{script}");
            assert!(script.contains("HOME=/tmp XDG_CONFIG_HOME=/tmp"), "{script}");
        }
        assert!(SNAPSHOT.contains("g add -A -- \"$@\""), "only the named paths are added");
        assert!(!SNAPSHOT.contains("g add -A\n"), "the whole home would be added");
        assert!(RESTORE.contains("--overlay"), "{RESTORE}");
        assert_eq!(RESTORE.matches("rm ").count(), 1, "only a journal is ever removed");
        assert!(RESTORE.contains("g cat-file -e \"$id:$db\""), "a journal is only removed beside its database");
    }

    /// A workspace folder of this test's own, removed again when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let stamp =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path = std::env::temp_dir().join(format!("qcode-conversations-{name}-{stamp}"));
            std::fs::create_dir_all(&path).expect("a workspace folder");
            Self(path)
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

    /// A stand-in engine that says every container runs and writes down what it was asked,
    /// so the refusal is checked against the questions that were really put.
    #[cfg(unix)]
    fn always_running(scratch: &Scratch) -> (Engine, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let asked = scratch.0.join("asked");
        let bin = scratch.0.join("engine");
        std::fs::write(&bin, format!("#!/bin/sh\necho \"$*\" >> '{}'\necho running\n", asked.display()))
            .expect("the stand-in");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        (Engine::new(EngineKind::Podman, bin), asked)
    }

    /// What `round` comes to once the lock the round before it took is free again: a program
    /// another test thread is starting holds a copy of it between `fork` and `exec`.
    fn settled<T: PartialEq + std::fmt::Debug>(mut round: impl FnMut() -> Result<T, BackupError>, busy: &T) -> T {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let outcome = round().expect("asked");
            if outcome != *busy {
                return outcome;
            }
            assert!(std::time::Instant::now() < deadline, "the lock stayed with the round before");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_backed_up_or_brought_back_under_a_running_harness_that_could_tear_it() {
        let scratch = Scratch::new("running");
        let paths = paths_at(scratch.0.clone());
        let (engine, asked) = always_running(&scratch);
        let id = SnapshotId::parse(ID).expect("an id");

        let taken =
            settled(|| snapshot(&engine, &paths, &home(), HarnessKind::OpenCode, ME), &Taken::Done(Snapshot::Busy));
        assert_eq!(taken, Taken::Running);
        let busy = Brought::Done(Restored::Busy);
        let brought = settled(|| restore(&engine, &paths, &home(), HarnessKind::ClaudeCode, ME, &id), &busy);
        assert_eq!(brought, Brought::Running);
        let brought = settled(|| restore(&engine, &paths, &home(), HarnessKind::OpenCode, ME, &id), &busy);
        assert_eq!(brought, Brought::Running);

        let asked = std::fs::read_to_string(asked).expect("the questions");
        assert_eq!(asked.lines().collect::<Vec<_>>(), ["container inspect --format {{.State.Status}} qcode-p-chat"; 3]);
        assert!(!paths.backup().join("Conversations").join("chat.git").exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_workspaces_own_backup_does_not_hold_up_the_conversations_but_the_same_profile_does() {
        let scratch = Scratch::new("lock");
        let paths = paths_at(scratch.0.clone());
        let (engine, _) = always_running(&scratch);
        let workspace = super::super::hold(&paths).expect("Backup/ is made").expect("nobody holds it");
        let taken = snapshot(&engine, &paths, &home(), HarnessKind::OpenCode, ME).expect("asked");
        assert_eq!(taken, Taken::Running, "the workspace's lock kept the round from even asking");
        drop(workspace);

        // A program another test thread is starting holds a copy of the lock the round above
        // took between `fork` and `exec`, so it is free once that copy closes too. The wait is
        // long because a loaded machine takes its time over that, and a short one would report a
        // working lock as broken: this failed once at five seconds with the whole suite running.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let _profile = loop {
            if let Some(held) = super::hold(&paths, &home()).expect("made") {
                break held;
            }
            assert!(std::time::Instant::now() < deadline, "the lock stayed with the round above");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(paths.backup().join("Conversations").join("chat.lock").is_file());
        let taken = snapshot(&missing(), &paths, &home(), HarnessKind::ClaudeCode, ME).expect("skipped");
        assert_eq!(taken, Taken::Done(Snapshot::Busy));
        let id = SnapshotId::parse(ID).expect("an id");
        let brought = restore(&missing(), &paths, &home(), HarnessKind::ClaudeCode, ME, &id).expect("skipped");
        assert_eq!(brought, Brought::Done(Restored::Busy));
    }

    #[test]
    fn a_profile_never_backed_up_has_no_history_and_starts_no_container() {
        let scratch = Scratch::new("history");
        assert_eq!(history(&missing(), &paths_at(scratch.0.clone()), &home(), ME).expect("nothing to ask"), []);
    }
}
