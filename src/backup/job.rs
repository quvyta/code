//! What each backup job runs, as data, and how its answer is read.
//!
//! Every job is one `sh -c` script in a one-off container of the base image, where git already
//! is. The script is a constant and everything that changes from job to job — the folders, the
//! exclude file, a commit, a path — travels as a positional argument, so nothing the person
//! named is ever read by a shell as part of a command.
//!
//! git is always told the same things: the backup's own git folder, the workspace as the tree it
//! looks at, and that it may trust both whoever owns them (the folders belong to the person,
//! the container may see another id); a name to sign commits with, because the image has none;
//! `LC_ALL=C` so the numbers in its answers are in the words this module reads; and no garbage
//! collection in the background, because the container is gone the moment the script ends.

use std::path::Path;

use crate::base::paths::{ASSETS_DIR, BACKUP_DIR, CODE_DIR};
use crate::engine::names::BASE_IMAGE;
use crate::engine::{Access, Engine, EngineCommand, HostUser, Mount, MountSource, Network, RunOnce};
use crate::store::WorkspacePaths;

use super::place::Place;
use super::{Entry, Reason, SnapshotId};

/// A folder of the workspace that is backed up, each into a git folder of its own inside
/// `Backup/`: they are backed up and brought back apart, and one's history says nothing of the
/// other's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tree {
    /// `Work/`, the workspace's own files.
    Code,
    /// `Assets/`, backed up only when the workspace asks for it.
    Assets,
}

/// The folder the snapshots of a workspace's code are kept in, inside its `Backup/`.
///
/// Named here and read by [`Store::adopt`](crate::store::Store::adopt), which has to give an
/// older workspace's folder this name: written out twice, the two would drift and a backup
/// history would quietly stop being found.
pub(crate) const CODE_SNAPSHOTS: &str = "Code.git";

impl Tree {
    /// The name of the git folder its snapshots are kept in, inside `Backup/`.
    pub(super) fn git(self) -> &'static str {
        match self {
            Self::Code => CODE_SNAPSHOTS,
            Self::Assets => "Assets.git",
        }
    }

    /// Where it is on the machine.
    pub(super) fn source(self, paths: &WorkspacePaths) -> &Path {
        match self {
            Self::Code => &paths.code,
            Self::Assets => &paths.assets,
        }
    }

    /// Where the job sees it, the same place the workspace's containers see it.
    fn target(self) -> &'static str {
        match self {
            Self::Code => CODE_DIR,
            Self::Assets => ASSETS_DIR,
        }
    }
}

/// The lines every script starts with: stop at the first failure, take the backup's git
/// folder (`$1`) and the workspace (`$2`) off the arguments, and make `g` git pointed at both.
/// What follows reads its own arguments from `$1` on.
macro_rules! preamble {
    () => {
        "set -e\nexport LC_ALL=C\nd=\"$1\"; w=\"$2\"; shift 2\n\
         g() { git --git-dir=\"$d\" --work-tree=\"$w\" --literal-pathspecs \
         -c safe.directory='*' -c user.name=QCode -c user.email=qcode@localhost \
         -c advice.addEmbeddedRepo=false -c gc.autoDetach=false \"$@\"; }\n"
    };
}

// The conversations backup runs git the same way, on another tree.
pub(super) use preamble;

/// Takes a snapshot. `$1` is the whole exclude file, `$2` the commit message.
///
/// A file that is backed up already and is left out now — a folder put on the skip list, a
/// line added to the workspace's own `.gitignore` — is taken out of the backup's index first, or
/// `add -A` would keep carrying it. A snapshot is only made when something changed, and the
/// last line says which: `made <id> <time>` or `unchanged`.
const SNAPSHOT: &str = concat!(
    preamble!(),
    "[ -d \"$d\" ] || git -c init.defaultBranch=backup init --quiet --bare \"$d\"\n",
    "mkdir -p \"$d/info\"\n",
    "printf '%s' \"$1\" > \"$d/info/exclude\"\n",
    "g ls-files -z --cached --ignored --exclude-standard > \"$d/dropped\"\n",
    "if [ -s \"$d/dropped\" ]; then\n",
    "  g rm -r --quiet --cached --ignore-unmatch --pathspec-from-file=\"$d/dropped\" --pathspec-file-nul\n",
    "fi\n",
    "rm -f \"$d/dropped\"\n",
    "g add -A\n",
    "if g diff --cached --quiet; then\n",
    "  echo unchanged\n",
    "else\n",
    "  g commit --quiet -m \"$2\"\n",
    "  g log -1 --format='made %H %ct'\n",
    "fi\n",
    "g gc --auto --quiet\n",
);

/// Lists the snapshots, newest first, each as a line `\x1e<id>\t<time>\t<message>` followed by
/// git's count of the files it changed. A path in `$1` narrows it to that path; a backup that
/// was never made, or holds nothing yet, lists nothing.
const HISTORY: &str = concat!(
    preamble!(),
    "[ -d \"$d\" ] || exit 0\n",
    "g rev-parse --quiet --verify HEAD > /dev/null || exit 0\n",
    "g log --format='%x1e%H%x09%ct%x09%s' --shortstat -- \"$@\"\n",
);

/// Brings `$2` back as it was in the snapshot `$1`, `.` for the whole workspace.
///
/// `--overlay` is git's default and is spelled out because it is the promise: a file the
/// snapshot does not have is left where it is, never deleted.
const RESTORE: &str = concat!(preamble!(), "g checkout --quiet --overlay \"$1\" -- \"$2\"\n");

/// The command that takes a snapshot of `tree`, leaving out every line of `exclude`.
pub(super) fn snapshot(
    engine: &Engine,
    paths: &WorkspacePaths,
    tree: Tree,
    user: HostUser,
    exclude: &str,
    reason: Reason,
) -> EngineCommand {
    one_off(engine, paths, tree, user, Access::ReadOnly, SNAPSHOT, &[exclude, reason.message()])
}

/// The command that lists the snapshots of `tree`, or the ones that changed `file`.
pub(super) fn history(
    engine: &Engine,
    paths: &WorkspacePaths,
    tree: Tree,
    user: HostUser,
    file: Option<&Place>,
) -> EngineCommand {
    let file: Vec<&str> = file.map(Place::as_str).into_iter().collect();
    one_off(engine, paths, tree, user, Access::ReadOnly, HISTORY, &file)
}

/// The command that brings `what` of `tree` back from the snapshot `id`, `None` for all of it.
pub(super) fn restore(
    engine: &Engine,
    paths: &WorkspacePaths,
    tree: Tree,
    user: HostUser,
    id: &SnapshotId,
    what: Option<&Place>,
) -> EngineCommand {
    let what = what.map_or(".", Place::as_str);
    one_off(engine, paths, tree, user, Access::ReadWrite, RESTORE, &[id.as_str(), what])
}

/// A one-off container of the base image with `tree` and the backup, and no network: nothing a
/// backup does needs one.
///
/// It starts in the tree, because git reads the paths it is given as relative to where it is,
/// and a path relative to anywhere else would be outside the tree it looks at.
fn one_off(
    engine: &Engine,
    paths: &WorkspacePaths,
    tree: Tree,
    user: HostUser,
    access: Access,
    script: &str,
    args: &[&str],
) -> EngineCommand {
    let backup = paths.backup();
    let mounts = [
        Mount { source: MountSource::Path(tree.source(paths)), target: Path::new(tree.target()), access },
        Mount { source: MountSource::Path(&backup), target: Path::new(BACKUP_DIR), access: Access::ReadWrite },
    ];
    let git_dir = format!("{BACKUP_DIR}/{}", tree.git());
    let mut command = vec!["sh", "-c", script, "sh", &git_dir, tree.target()];
    command.extend_from_slice(args);
    engine.run_once(&RunOnce {
        image: BASE_IMAGE,
        mounts: &mounts,
        network: Network::None,
        user,
        workdir: Some(Path::new(tree.target())),
        command: &command,
    })
}

/// What [`SNAPSHOT`] said it did: `Some` with the snapshot it made, `None` when nothing had
/// changed, and an error with the whole answer when it said neither.
pub(super) fn read_snapshot(output: &str) -> Result<Option<(SnapshotId, i64)>, String> {
    let last = output.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
    if last == "unchanged" {
        return Ok(None);
    }
    let mut words = last.split(' ');
    if let (Some("made"), Some(id), Some(at), None) = (words.next(), words.next(), words.next(), words.next())
        && let (Some(id), Ok(at)) = (SnapshotId::parse(id), at.parse())
    {
        return Ok(Some((id, at)));
    }
    Err(output.to_owned())
}

/// The snapshots [`HISTORY`] listed, in its order, newest first.
///
/// An entry git gave no count for changed no file, which only a snapshot limited to a path can
/// be. A line that is not what the script writes is left out rather than guessed at.
pub(super) fn read_history(output: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    // The entry a count belongs to; none after a head that was left out, so its count is too.
    let mut open = false;
    for line in output.lines() {
        if let Some(head) = line.strip_prefix('\u{1e}') {
            let mut fields = head.splitn(3, '\t');
            open = false;
            if let (Some(id), Some(at), Some(message)) = (fields.next(), fields.next(), fields.next())
                && let (Some(id), Ok(at)) = (SnapshotId::parse(id), at.parse())
            {
                entries.push(Entry { id, at, reason: Reason::from_message(message), changed: 0 });
                open = true;
            }
        } else if open
            && line.contains(" changed")
            && let Some(entry) = entries.last_mut()
            && let Some(Ok(count)) = line.split_whitespace().next().map(str::parse)
        {
            entry.changed = count;
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::{HISTORY, RESTORE, SNAPSHOT, Tree, history, read_history, read_snapshot, restore, snapshot};
    use crate::backup::place::Place;
    use crate::backup::{Entry, Reason, SnapshotId};
    use crate::engine::{Engine, EngineCommand, EngineKind, HostUser};
    use crate::store::WorkspacePaths;
    use std::path::{Path, PathBuf};

    const ID: &str = "0123456789abcdef0123456789abcdef01234567";
    const OLDER: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn paths() -> WorkspacePaths {
        let root = PathBuf::from("/home/me/QCode/Workspaces/p");
        WorkspacePaths {
            file: root.join("workspace.qcode"),
            code: root.join("Work"),
            assets: root.join("Assets"),
            harness: root.join("Containers").join("Harness"),
            root,
        }
    }

    fn podman() -> Engine {
        Engine::new(EngineKind::Podman, "/usr/bin/podman")
    }

    fn args(command: &EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    /// The words every job starts with, up to and including the script.
    fn container(workspace: &str, script: &str) -> Vec<String> {
        [
            "run",
            "--rm",
            "--userns=keep-id",
            "--network=none",
            "--volume",
            &format!("/home/me/QCode/Workspaces/p/Work:/work:{workspace},z"),
            "--volume",
            "/home/me/QCode/Workspaces/p/Backup:/backup:rw,z",
            "--workdir",
            "/work",
            "qcode/base",
            "sh",
            "-c",
            script,
            "sh",
            "/backup/Code.git",
            "/work",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    const ME: HostUser = HostUser::Ids { uid: 1000, gid: 1000 };

    #[test]
    fn a_snapshot_reads_the_workspace_and_writes_only_the_backup() {
        let command = snapshot(&podman(), &paths(), Tree::Code, ME, "/data\n", Reason::Scheduled);
        assert_eq!(command.program, Path::new("/usr/bin/podman"));
        let mut expected = container("ro", SNAPSHOT);
        expected.extend(["/data\n".to_owned(), "backup".to_owned()]);
        assert_eq!(args(&command), expected);
        let before = snapshot(&podman(), &paths(), Tree::Code, ME, "", Reason::BeforeRestore);
        assert_eq!(args(&before).last().map(String::as_str), Some("before restore"));
    }

    #[test]
    fn the_history_names_a_path_only_when_it_is_asked_for_one() {
        assert_eq!(args(&history(&podman(), &paths(), Tree::Code, ME, None)), container("ro", HISTORY));
        let file = Place::new("src/main.rs").expect("a path");
        let mut expected = container("ro", HISTORY);
        expected.push("src/main.rs".to_owned());
        assert_eq!(args(&history(&podman(), &paths(), Tree::Code, ME, Some(&file))), expected);
    }

    #[test]
    fn a_restore_is_the_one_job_that_writes_to_the_workspace() {
        let id = SnapshotId::parse(ID).expect("an id");
        let mut expected = container("rw", RESTORE);
        expected.extend([ID.to_owned(), ".".to_owned()]);
        assert_eq!(args(&restore(&podman(), &paths(), Tree::Code, ME, &id, None)), expected);
        let file = Place::new("src/main.rs").expect("a path");
        let mut expected = container("rw", RESTORE);
        expected.extend([ID.to_owned(), "src/main.rs".to_owned()]);
        assert_eq!(args(&restore(&podman(), &paths(), Tree::Code, ME, &id, Some(&file))), expected);
    }

    #[test]
    fn the_assets_are_read_only_to_a_snapshot_and_written_only_by_a_restore() {
        let mounted = |command: &EngineCommand| {
            let words = args(command);
            let at = words.iter().position(|word| word == "--volume").expect("a mount");
            (words[at + 1].clone(), words[at + 5].clone(), words[words.len() - 3..].to_vec())
        };
        let command = snapshot(&podman(), &paths(), Tree::Assets, ME, "", Reason::Scheduled);
        let (volume, workdir, _) = mounted(&command);
        assert_eq!(volume, "/home/me/QCode/Workspaces/p/Assets:/assets:ro,z");
        assert_eq!(workdir, "/assets");
        assert!(args(&command).contains(&"/backup/Assets.git".to_owned()), "{:?}", args(&command));

        let id = SnapshotId::parse(ID).expect("an id");
        let (volume, _, tail) = mounted(&restore(&podman(), &paths(), Tree::Assets, ME, &id, None));
        assert_eq!(volume, "/home/me/QCode/Workspaces/p/Assets:/assets:rw,z");
        assert_eq!(tail, ["/assets", ID, "."]);
        let (volume, ..) = mounted(&history(&podman(), &paths(), Tree::Assets, ME, None));
        assert!(volume.ends_with(":ro,z"), "{volume}");
    }

    #[test]
    fn every_script_keeps_git_away_from_the_persons_own_repository_and_deletes_nothing() {
        for script in [SNAPSHOT, HISTORY, RESTORE] {
            assert!(script.contains("--git-dir=\"$d\""), "{script}");
            assert!(script.contains("--literal-pathspecs"), "{script}");
            assert!(!script.contains(".git/"), "{script}");
        }
        assert!(SNAPSHOT.contains("gc.autoDetach=false"), "a detached gc dies with the container");
        assert!(SNAPSHOT.contains("rm -r --quiet --cached"), "only the index loses anything");
        assert!(RESTORE.contains("--overlay"), "{RESTORE}");
    }

    #[test]
    fn the_snapshot_answer_says_whether_one_was_made() {
        assert_eq!(read_snapshot("unchanged\n"), Ok(None));
        let made = read_snapshot(&format!("made {ID} 1789700000\n")).expect("understood");
        assert_eq!(made, Some((SnapshotId::parse(ID).expect("an id"), 1_789_700_000)));
        assert!(read_snapshot("").is_err());
        assert!(read_snapshot("made nothing 12").is_err());
        assert!(read_snapshot(&format!("made {ID} 12 extra")).is_err());
    }

    #[test]
    fn the_history_is_read_newest_first_with_the_count_of_changed_files() {
        let output = format!(
            "\u{1e}{ID}\t1789700600\tbefore restore\n\n 2 files changed, 3 insertions(+), 1 deletion(-)\n\
             \u{1e}{OLDER}\t1789700000\tbackup\n\n 1 file changed, 0 insertions(+), 0 deletions(-)\n"
        );
        assert_eq!(
            read_history(&output),
            [
                Entry {
                    id: SnapshotId::parse(ID).expect("an id"),
                    at: 1_789_700_600,
                    reason: Reason::BeforeRestore,
                    changed: 2
                },
                Entry {
                    id: SnapshotId::parse(OLDER).expect("an id"),
                    at: 1_789_700_000,
                    reason: Reason::Scheduled,
                    changed: 1
                },
            ]
        );
        assert_eq!(read_history(""), []);
        let odd = format!("\u{1e}{ID}\t1\tbackup\n\u{1e}not-an-id\t2\tbackup\n\n 5 files changed\n");
        assert_eq!(read_history(&odd).iter().map(|entry| entry.changed).collect::<Vec<_>>(), [0]);
    }
}
