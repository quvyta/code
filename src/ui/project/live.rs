//! The one thing about the project screen no argument list can prove: that what a tab opens
//! really is inside a container.
//!
//! The tests here are `#[ignore]`d and do nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They pull `alpine` the first time, so
//! the first run needs the network. They never touch a real QCode image, project or profile:
//! the container they make is named after a project nobody has.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```

use std::path::{Path, PathBuf};

use qframe::widgets::{TerminalEvent, TerminalSession};

use crate::engine::names::HOSTNAME;
use crate::engine::run::capture;
use crate::engine::{Access, Engine, EngineKind, Exec, HostUser, Network, detect};

use super::plan::{ContainerPlan, SHELL};

/// The image the test container is made from, named in full: podman refuses a short name without
/// a terminal to ask at.
const ALPINE: &str = "docker.io/library/alpine:3";

/// The container this test lives in, named after a project nobody has.
const CONTAINER: &str = "qcode-uiprojectlive-base";

/// A second container of the same kind, for the check that two of them share one machine name.
const TWIN: &str = "qcode-uiprojectlive-twin";

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

/// A project folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-uilive-{stamp}"));
        std::fs::create_dir_all(path.join("Project")).expect("a project folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    fn plan(&self, name: &str) -> ContainerPlan {
        ContainerPlan {
            name: name.to_owned(),
            image: ALPINE.to_owned(),
            home: None,
            project: self.0.join("Project"),
            assets: self.0.join("Assets"),
            assets_access: Access::ReadWrite,
            network: Network::Full,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn what_a_tab_opens_runs_in_the_container_and_not_on_this_machine() {
    for engine in engines() {
        let scratch = Scratch::new();
        let plan = scratch.plan(CONTAINER);
        let _ = capture(&engine.remove_container(CONTAINER));

        let user = HostUser::current().expect("the current user");
        capture(&plan.create(&engine, user)).expect("the container is made");
        capture(&engine.start_container(CONTAINER)).expect("the container starts");

        // Exactly the command a tab spawns, in exactly the way a tab spawns it.
        let command = plan.enter(&engine, SHELL);
        let session =
            TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0).expect("a pseudo-terminal");
        session.write(b"hostname > /work/Project/where\nexit\n").expect("the shell reads its input");
        let watch = session.watch();
        while !matches!(watch.next(), TerminalEvent::Exited(_)) {}

        let written = std::fs::read_to_string(scratch.0.join("Project").join("where"))
            .expect("the shell wrote through the project mount");
        let inside = written.trim().to_owned();
        assert!(!inside.is_empty(), "the shell answered from somewhere");
        assert_ne!(inside, this_machine(), "the shell was not this machine's: {inside}");

        capture(&engine.remove_container(CONTAINER)).expect("the container is removed");
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn every_container_of_the_plan_answers_to_the_same_machine_name() {
    for engine in engines() {
        let scratch = Scratch::new();
        let user = HostUser::current().expect("the current user");
        let mut answered = Vec::new();
        for name in [CONTAINER, TWIN] {
            let _ = capture(&engine.remove_container(name));
            capture(&scratch.plan(name).create(&engine, user)).expect("the container is made");
            capture(&engine.start_container(name)).expect("the container starts");
            let inside = capture(&engine.exec_without_terminal(&Exec { container: name, command: &["hostname"] }))
                .expect("the container says its machine name");
            answered.push(inside.trim().to_owned());
            capture(&engine.remove_container(name)).expect("the container is removed");
        }
        // Not the engine's random name, and not this machine's either: the one QCode fixed, so a
        // login keyed to the machine name decrypts in whichever container it is copied into.
        assert_eq!(answered, [HOSTNAME, HOSTNAME], "{:?}", engine.kind());
    }
}

/// The name of the machine QCode is running on, read without asking a shell for it.
fn this_machine() -> String {
    std::fs::read_to_string(Path::new("/etc/hostname")).unwrap_or_default().trim().to_owned()
}
