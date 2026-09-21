//! The part of the backup no argument list can answer for: that git in the base image, run the
//! way [`super::job`] runs it, really keeps the snapshots, leaves out what it is told to, and
//! brings a workspace back without deleting anything.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test backup::live -- --ignored --test-threads=1
//! ```
//!
//! The containers remove themselves. The machine's own `qcode/base` is built when it is missing
//! and left, as [`crate::base::ensure`] means it to be.

use std::fs;
use std::path::{Path, PathBuf};

use super::{
    Reason, Restored, Snapshot, assets_history, history, restore, restore_assets, restore_file, snapshot,
    snapshot_assets,
};
use crate::engine::{Engine, EngineKind, HostUser, detect};
use crate::store::WorkspacePaths;

/// The engines installed on this machine, or nothing at all when the tests are switched off.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let found: Vec<Engine> =
        [EngineKind::Podman, EngineKind::Docker].into_iter().filter_map(|kind| detect(kind).ok()).collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// A workspace folder of this test's own, removed by `Drop`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(engine: &Engine) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-live-backup-{}-{stamp}", engine.kind().name()));
        fs::create_dir_all(path.join("Work")).expect("a workspace folder");
        Self(path)
    }

    fn paths(&self) -> WorkspacePaths {
        WorkspacePaths {
            root: self.0.clone(),
            file: self.0.join("workspace.qcode"),
            code: self.0.join("Work"),
            assets: self.0.join("Assets"),
            harness: self.0.join("Containers").join("Harness"),
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join("Work").join(name)
    }

    fn write(&self, name: &str, text: &str) {
        let file = self.file(name);
        fs::create_dir_all(file.parent().expect("inside the workspace")).expect("a folder");
        fs::write(file, text).expect("a file in the workspace");
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.file(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The files a snapshot holds, read straight from the backup's git folder.
fn files_in(engine: &Engine, paths: &WorkspacePaths, id: &str) -> Vec<String> {
    let git_dir = paths.backup().join("Code.git");
    // The container wrote the backup as the person, so the person's git — when the machine
    // running the test has one — reads it without the container. Asking the container again
    // would only check the scripts against themselves.
    let listed = std::process::Command::new("git")
        .arg(format!("--git-dir={}", git_dir.display()))
        .args(["-c", "safe.directory=*", "ls-tree", "-r", "--name-only", id])
        .output()
        .unwrap_or_else(|error| panic!("git on {:?}: {error}", engine.kind()));
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    String::from_utf8_lossy(&listed.stdout).lines().map(str::to_owned).collect()
}

fn made(outcome: &Snapshot) -> &str {
    match outcome {
        Snapshot::Made { id, .. } => id.as_str(),
        other => panic!("no snapshot was made: {other:?}"),
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_workspace_is_backed_up_and_brought_back_without_losing_a_newer_file() {
    for engine in engines() {
        let user = HostUser::current().expect("the current user");
        let scratch = Scratch::new(&engine);
        let paths = scratch.paths();
        let skip = ["data".to_owned()];
        scratch.write("README.md", "first\n");
        scratch.write("src/main.rs", "fn main() {}\n");
        scratch.write("data/big.bin", "left out\n");
        scratch.write("node_modules/dep.js", "ignored by the workspace\n");
        scratch.write(".gitignore", "node_modules/\n");
        // The person's own repository: the backup must neither read its history nor write it.
        fs::create_dir_all(scratch.file(".git")).expect("a folder");
        scratch.write(".git/HEAD", "ref: refs/heads/mine\n");

        let first = snapshot(&engine, &paths, user, &skip).unwrap_or_else(|error| panic!("{engine:?}: {error:?}"));
        let first_id = made(&first).to_owned();
        assert_eq!(files_in(&engine, &paths, &first_id), [".gitignore", "README.md", "src/main.rs"]);
        assert_eq!(scratch.read(".git/HEAD"), "ref: refs/heads/mine\n", "the person's repository was touched");

        let again = snapshot(&engine, &paths, user, &skip).expect("a second round");
        assert_eq!(again, Snapshot::Unchanged, "nothing changed, so nothing is written");

        scratch.write("README.md", "second\n");
        let second = snapshot(&engine, &paths, user, &skip).expect("a third round");
        let second_id = made(&second).to_owned();

        let listed = history(&engine, &paths, user, None).expect("the list");
        assert_eq!(listed.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), [&second_id, &first_id]);
        assert_eq!(listed.iter().map(|entry| entry.changed).collect::<Vec<_>>(), [1, 3]);
        assert!(listed.iter().all(|entry| entry.reason == Reason::Scheduled));
        let of_main = history(&engine, &paths, user, Some("src/main.rs")).expect("one file's list");
        assert_eq!(of_main.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), [&first_id]);

        // Leaving `src` out now takes it out of the next snapshot too.
        scratch.write("README.md", "third\n");
        scratch.write("notes.txt", "made after the first snapshot\n");
        let skip = ["data".to_owned(), "src".to_owned()];
        let first = listed.last().expect("the first snapshot").id.clone();
        let restored = restore(&engine, &paths, user, &skip, &first).expect("restored");
        let Restored::Done { before } = restored else { panic!("the lock was free: {restored:?}") };
        let before_id = made(&before).to_owned();
        assert_eq!(files_in(&engine, &paths, &before_id), [".gitignore", "README.md", "notes.txt"]);
        assert_eq!(scratch.read("README.md"), "first\n", "the file came back");
        assert_eq!(scratch.read("notes.txt"), "made after the first snapshot\n", "a newer file was deleted");
        assert_eq!(scratch.read("data/big.bin"), "left out\n");

        let listed = history(&engine, &paths, user, None).expect("the list");
        assert_eq!(listed.first().map(|entry| entry.reason), Some(Reason::BeforeRestore));

        // One file, from the snapshot the restore took first.
        let before = listed.first().expect("the snapshot before the restore").id.clone();
        let one = restore_file(&engine, &paths, user, &skip, &before, "README.md").expect("one file");
        assert!(matches!(one, Restored::Done { .. }), "{one:?}");
        assert_eq!(scratch.read("README.md"), "third\n");
        assert!(Path::new(&scratch.file("src/main.rs")).is_file());
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_assets_are_backed_up_apart_and_brought_back_without_losing_a_newer_file() {
    for engine in engines() {
        let user = HostUser::current().expect("the current user");
        let scratch = Scratch::new(&engine);
        let paths = scratch.paths();
        let asset = |name: &str| paths.assets.join(name);
        fs::create_dir_all(asset("icons")).expect("an assets folder");
        fs::write(asset("icons/sun.svg"), "<svg>sun</svg>\n").expect("an asset");
        fs::write(asset("photo.jpg"), "first\n").expect("an asset");

        let first = snapshot_assets(&engine, &paths, user).unwrap_or_else(|error| panic!("{engine:?}: {error:?}"));
        let first_id = made(&first).to_owned();
        let git_dir = paths.backup().join("Assets.git");
        assert!(git_dir.is_dir(), "the assets have a git folder of their own");
        assert!(!paths.backup().join("Code.git").exists(), "the workspace's backup is not touched");
        assert_eq!(snapshot_assets(&engine, &paths, user).expect("a second round"), Snapshot::Unchanged);

        fs::write(asset("photo.jpg"), "second\n").expect("a changed asset");
        fs::write(asset("new.png"), "made after the first snapshot\n").expect("a new asset");
        let listed = assets_history(&engine, &paths, user).expect("the list");
        assert_eq!(listed.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), [&first_id]);

        let first = listed[0].id.clone();
        let restored = restore_assets(&engine, &paths, user, &first).expect("restored");
        let Restored::Done { before } = restored else { panic!("the lock was free: {restored:?}") };
        made(&before);
        assert_eq!(fs::read_to_string(asset("photo.jpg")).expect("the asset"), "first\n", "the asset came back");
        assert_eq!(
            fs::read_to_string(asset("new.png")).expect("the newer asset"),
            "made after the first snapshot\n",
            "a newer file was deleted"
        );
        let listed = assets_history(&engine, &paths, user).expect("the list");
        assert_eq!(
            listed.iter().map(|entry| entry.reason).collect::<Vec<_>>(),
            [Reason::BeforeRestore, Reason::Scheduled]
        );
    }
}
