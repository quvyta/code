//! That git in the base image, run the way [`super`] runs it on a real home volume, keeps a
//! profile's conversations and never its login, and brings them back without losing a newer
//! one.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`:
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test backup::conversations::live -- --ignored --test-threads=1
//! ```
//!
//! The volume and the container a test makes are removed when it ends, however it ends.

use std::fs;
use std::path::{Path, PathBuf};

use super::{Brought, Taken, history, restore, snapshot};
use crate::backup::{Reason, Restored, Snapshot};
use crate::base;
use crate::base::paths::{HOME_DIR, KEEP_ALIVE};
use crate::engine::names::{BASE_IMAGE, HOSTNAME};
use crate::engine::run::capture;
use crate::engine::{
    Access, ContainerCreate, Engine, EngineKind, HostUser, Mount, MountSource, Network, RunOnce, detect,
};
use crate::profile::identity::Home;
use crate::profile::{HarnessKind, SafeName};
use crate::workspace::{ProjectId, ProjectPaths};

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

/// A project folder and a home volume of this test's own, both removed by `Drop`, with the
/// profile's container too when the test made one.
struct Scratch {
    engine: Engine,
    root: PathBuf,
    home: Home,
}

impl Scratch {
    fn new(engine: &Engine) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let project = ProjectId::parse(&format!("chatlive{}", stamp % 1_000_000_000)).expect("a project id");
        let root = std::env::temp_dir().join(format!("qcode-live-conversations-{}-{stamp}", engine.kind().name()));
        fs::create_dir_all(&root).expect("a project folder");
        let home = Home::new(SafeName::parse("claude-live").expect("a name"), project);
        capture(&engine.create_volume(&home.volume())).expect("a home volume");
        Self { engine: engine.clone(), root, home }
    }

    fn paths(&self) -> ProjectPaths {
        ProjectPaths {
            file: self.root.join("project.qcode"),
            project: self.root.join("Project"),
            assets: self.root.join("Assets"),
            harness: self.root.join("Containers").join("Harness"),
            root: self.root.clone(),
        }
    }

    /// Runs `script` in the home, the way the harness would have written there.
    fn in_home(&self, user: HostUser, script: &str) -> String {
        let volume = self.home.volume();
        let mounts =
            [Mount { source: MountSource::Volume(&volume), target: Path::new(HOME_DIR), access: Access::ReadWrite }];
        let command = ["sh", "-c", script];
        let run = RunOnce {
            image: BASE_IMAGE,
            mounts: &mounts,
            network: Network::None,
            user,
            workdir: Some(Path::new(HOME_DIR)),
            command: &command,
        };
        capture(&self.engine.run_once(&run)).unwrap_or_else(|error| panic!("{script}: {error:?}"))
    }

    /// The files a snapshot holds, read straight from the profile's git folder by the machine's
    /// own git, so the scripts are not checked against themselves.
    fn files_in(&self, id: &str) -> Vec<String> {
        let git_dir = self.paths().backup().join("Conversations").join("claude-live.git");
        let listed = std::process::Command::new("git")
            .arg(format!("--git-dir={}", git_dir.display()))
            .args(["-c", "safe.directory=*", "ls-tree", "-r", "--name-only", id])
            .output()
            .expect("git on the machine");
        assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
        String::from_utf8_lossy(&listed.stdout).lines().map(str::to_owned).collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = capture(&self.engine.remove_container(&self.home.container()));
        let _ = capture(&self.engine.remove_volume(&self.home.volume()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn made(outcome: &Taken) -> String {
    match outcome {
        Taken::Done(Snapshot::Made { id, .. }) => id.as_str().to_owned(),
        other => panic!("no snapshot was made: {other:?}"),
    }
}

const TRANSCRIPTS: &str = ".claude/projects/-work-Project";

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn conversations_are_backed_up_without_the_login_and_brought_back_without_losing_a_newer_one() {
    for engine in engines() {
        let user = HostUser::current().expect("the current user");
        base::ensure(&engine, &|| false, &mut |_| {}).expect("the base image");
        let scratch = Scratch::new(&engine);
        let paths = scratch.paths();
        let home = &scratch.home;
        let harness = HarnessKind::ClaudeCode;
        scratch.in_home(
            user,
            &format!(
                "mkdir -p {TRANSCRIPTS} && echo first > {TRANSCRIPTS}/a.jsonl \
                 && echo secret > .claude/.credentials.json && echo '{{}}' > .claude/settings.json"
            ),
        );

        let first =
            made(&snapshot(&engine, &paths, home, harness, user).unwrap_or_else(|e| panic!("{engine:?}: {e:?}")));
        assert_eq!(scratch.files_in(&first), [format!("{TRANSCRIPTS}/a.jsonl")], "only the conversations are kept");

        let again = snapshot(&engine, &paths, home, harness, user).expect("a second round");
        assert_eq!(again, Taken::Done(Snapshot::Unchanged));

        scratch.in_home(user, &format!("echo second > {TRANSCRIPTS}/a.jsonl && echo newer > {TRANSCRIPTS}/b.jsonl"));
        let second = made(&snapshot(&engine, &paths, home, harness, user).expect("a third round"));
        assert_eq!(scratch.files_in(&second), [format!("{TRANSCRIPTS}/a.jsonl"), format!("{TRANSCRIPTS}/b.jsonl")]);

        let listed = history(&engine, &paths, home, user).expect("the list");
        assert_eq!(listed.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), [&second, &first]);
        assert_eq!(listed.iter().map(|entry| entry.changed).collect::<Vec<_>>(), [2, 1]);

        // A running container keeps its files as they are.
        let volume = home.volume();
        let mounts =
            [Mount { source: MountSource::Volume(&volume), target: Path::new(HOME_DIR), access: Access::ReadWrite }];
        let container = home.container();
        capture(&engine.create_container(&ContainerCreate {
            name: &container,
            hostname: HOSTNAME,
            labels: &[],
            image: BASE_IMAGE,
            mounts: &mounts,
            network: Network::None,
            user,
            workdir: None,
            command: KEEP_ALIVE,
        }))
        .expect("the profile's container");
        capture(&engine.start_container(&container)).expect("started");
        let first_id = listed[1].id.clone();
        assert_eq!(restore(&engine, &paths, home, harness, user, &first_id).expect("asked"), Brought::Running);
        capture(&engine.stop_container(&container)).expect("stopped");

        let brought = restore(&engine, &paths, home, harness, user, &first_id).expect("restored");
        assert_eq!(brought, Brought::Done(Restored::Done { before: Snapshot::Unchanged }));
        assert_eq!(scratch.in_home(user, &format!("cat {TRANSCRIPTS}/a.jsonl")), "first\n", "the file came back");
        assert_eq!(scratch.in_home(user, &format!("cat {TRANSCRIPTS}/b.jsonl")), "newer\n", "a newer one was lost");
        assert_eq!(scratch.in_home(user, "cat .claude/.credentials.json"), "secret\n");

        // The restore can be undone: the next round keeps what it brought back, as a change.
        let after = made(&snapshot(&engine, &paths, home, harness, user).expect("a round after"));
        let listed = history(&engine, &paths, home, user).expect("the list");
        assert_eq!(listed.first().map(|entry| entry.id.as_str()), Some(after.as_str()));
        assert!(listed.iter().all(|entry| entry.reason == Reason::Scheduled));
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn an_opencode_database_is_kept_whole_and_a_journal_never_outlives_its_database() {
    for engine in engines() {
        let user = HostUser::current().expect("the current user");
        base::ensure(&engine, &|| false, &mut |_| {}).expect("the base image");
        let scratch = Scratch::new(&engine);
        let paths = scratch.paths();
        let home = &scratch.home;
        let harness = HarnessKind::OpenCode;
        let dir = ".local/share/opencode";
        scratch
            .in_home(user, &format!("mkdir -p {dir} && echo db1 > {dir}/opencode.db && echo auth > {dir}/auth.json"));
        let first = made(&snapshot(&engine, &paths, home, harness, user).expect("a round"));
        assert_eq!(scratch.files_in(&first), [format!("{dir}/opencode.db")]);

        scratch.in_home(user, &format!("echo db2 > {dir}/opencode.db && echo wal2 > {dir}/opencode.db-wal"));
        let second = made(&snapshot(&engine, &paths, home, harness, user).expect("a round"));
        assert_eq!(scratch.files_in(&second), [format!("{dir}/opencode.db"), format!("{dir}/opencode.db-wal")]);

        let first_id = history(&engine, &paths, home, user).expect("the list")[1].id.clone();
        let brought = restore(&engine, &paths, home, harness, user, &first_id).expect("restored");
        assert!(matches!(brought, Brought::Done(Restored::Done { .. })), "{brought:?}");
        let listed = scratch.in_home(user, &format!("cat {dir}/opencode.db; ls {dir}"));
        assert_eq!(listed, "db1\nauth.json\nopencode.db\n", "the journal of the newer database stayed");

        // The journal that went is in the snapshot that restore can be undone with.
        let second_id = history(&engine, &paths, home, user).expect("the list")[0].id.clone();
        restore(&engine, &paths, home, harness, user, &second_id).expect("undone");
        let listed = scratch.in_home(user, &format!("cat {dir}/opencode.db {dir}/opencode.db-wal"));
        assert_eq!(listed, "db2\nwal2\n");
    }
}
