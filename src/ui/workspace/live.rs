//! The one thing about the workspace screen no argument list can prove: that what a tab opens
//! really is inside a container.
//!
//! The tests here are `#[ignore]`d and do nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They pull `alpine` the first time, so
//! the first run needs the network. They never touch a real QCode image, workspace or profile:
//! the container they make is named after a workspace nobody has.
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

/// The container this test lives in, named after a workspace nobody has.
const CONTAINER: &str = "qcode-uiworkspacelive-base";

/// A second container of the same kind, for the check that two of them share one machine name.
const TWIN: &str = "qcode-uiworkspacelive-twin";

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

/// A workspace folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-uilive-{stamp}"));
        std::fs::create_dir_all(path.join("Work")).expect("a workspace folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    fn plan(&self, name: &str) -> ContainerPlan {
        ContainerPlan {
            name: name.to_owned(),
            image: ALPINE.to_owned(),
            home: None,
            browser: None,
            code: self.0.join("Work"),
            assets: self.0.join("Assets"),
            assets_access: Access::ReadWrite,
            network: Network::Full,
            window: None,
            bridge: None,
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
        session.write(b"hostname > /work/where\nexit\n").expect("the shell reads its input");
        let watch = session.watch();
        while !matches!(watch.next(), TerminalEvent::Exited(_)) {}

        let written = std::fs::read_to_string(scratch.0.join("Work").join("where"))
            .expect("the shell wrote through the workspace mount");
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

/// The container and home of the check that a container made from an older plan is made again.
const REMADE: &str = "qcode-uiworkspacelive-remade";

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_stopped_container_of_an_older_plan_is_made_again_with_the_bridge_and_keeps_its_home() {
    use crate::base::paths::{HOME_DIR, MCP_DIR};
    use crate::engine::names::BASE_IMAGE;
    use crate::profile::identity::Home;
    use crate::profile::{HarnessKind, SafeName};
    use crate::store::WorkspaceId;

    use super::plan::{Bridge, PLAN_LABEL, ensure_running};

    for engine in engines() {
        crate::base::ensure(&engine, &|| false, &mut |_| {}).expect("the base image is there");
        let scratch = Scratch::new();
        let user = HostUser::current().expect("the current user");
        let home = Home::new(
            SafeName::parse("remade").expect("a safe name"),
            WorkspaceId::parse("uiworkspacelive").expect("a workspace id"),
        );
        let _ = capture(&engine.remove_container(REMADE));
        let _ = capture(&engine.remove_volume(&home.volume()));
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: REMADE, command: &["sh", "-c", script] }))
        };

        // A container as QCode made it before the bridge: no mount of Containers/MCP.
        let older = ContainerPlan { image: BASE_IMAGE.to_owned(), home: Some(home.clone()), ..scratch.plan(REMADE) };
        ensure_running(&engine, &older, user).expect("the older container comes up");
        run(&format!("echo kept > {HOME_DIR}/marker")).expect("the home is written");
        capture(&engine.stop_container(REMADE)).expect("the container stops");

        let folder = scratch.0.join("Containers").join("MCP");
        let newer = ContainerPlan {
            bridge: Some(Bridge { folder: folder.clone(), harness: HarnessKind::ClaudeCode }),
            ..older.clone()
        };
        assert_ne!(older.digest(&engine, user), newer.digest(&engine, user), "the bridge changes the plan");
        ensure_running(&engine, &newer, user).expect("the newer container comes up");
        let label = capture(&engine.container_label(REMADE, PLAN_LABEL)).expect("the label is read");
        assert_eq!(label.trim(), newer.digest(&engine, user), "{:?}: the container was made again", engine.kind());
        assert_eq!(run(&format!("cat {HOME_DIR}/marker")).expect("the home is read").trim(), "kept");
        std::fs::write(folder.join("probe"), "from the host").expect("the folder was made as the person");
        assert_eq!(run(&format!("cat {MCP_DIR}/probe")).expect("the bridge is mounted").trim(), "from the host");
        assert!(run(&format!("touch {MCP_DIR}/written")).is_err(), "{:?}: the bridge is read-only", engine.kind());

        // Running, a container is never made again: tabs work in it.
        ensure_running(&engine, &older, user).expect("the running container is left as it is");
        let label = capture(&engine.container_label(REMADE, PLAN_LABEL)).expect("the label is read");
        assert_eq!(label.trim(), newer.digest(&engine, user));

        // Stopped and asked for with the same plan, it is only started: what was written into the
        // container itself is still there.
        run("echo same > /tmp/layer").expect("the container's own files are written");
        capture(&engine.stop_container(REMADE)).expect("the container stops");
        ensure_running(&engine, &newer, user).expect("the container starts again");
        assert_eq!(run("cat /tmp/layer").expect("the same container").trim(), "same");

        capture(&engine.remove_container(REMADE)).expect("the container is removed");
        capture(&engine.remove_volume(&home.volume())).expect("the home is removed");
    }
}

/// The name of the machine QCode is running on, read without asking a shell for it.
fn this_machine() -> String {
    std::fs::read_to_string(Path::new("/etc/hostname")).unwrap_or_default().trim().to_owned()
}
