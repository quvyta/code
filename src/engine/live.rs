//! The part of the engine layer no argument list can answer for: that the commands are the ones
//! the engines really accept, on a machine where they are really installed.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They pull `alpine` the first time, so
//! the first run needs the network.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```

use super::names::{HOSTNAME, base_container, credential_volume, home_volume, profile_container, profile_image};
use super::run::{EngineError, build_image, capture};
use super::{
    Access, ContainerCreate, ContainerState, Engine, EngineKind, Exec, HostUser, ImageBuild, Mount, MountSource,
    Network, detect,
};
use qframe::widgets::{TerminalEvent, TerminalSession};
use std::path::{Path, PathBuf};

/// The image the test images are built on, named in full: podman refuses a short name without a
/// terminal to ask at.
const ALPINE: &str = "docker.io/library/alpine:3";

/// The project these tests pretend to be, kept away from any real QCode project on the machine.
const PROJECT: &str = "enginelivetest";

/// The profile these tests pretend to be.
const PROFILE: &str = "live";

/// The engines installed on this machine, or nothing at all when the tests are switched off.
///
/// Every test runs against each engine it finds, because the whole point of them is the
/// difference between the two.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let found: Vec<Engine> =
        [EngineKind::Podman, EngineKind::Docker].into_iter().filter_map(|kind| detect(kind).ok()).collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// A folder of this test's own, removed by `Scratch`'s `Drop`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-live-{name}-{stamp}"));
        std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let file = self.0.join(name);
        std::fs::write(&file, text).expect("a file in the temporary folder");
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Takes away whatever an earlier run left behind. Failures are the point of the call being
/// here: there is usually nothing to remove.
fn clear(engine: &Engine, containers: &[&str], volumes: &[&str], images: &[&str]) {
    for container in containers {
        let _ = capture(&engine.remove_container(container));
    }
    for volume in volumes {
        let _ = capture(&engine.remove_volume(volume));
    }
    for image in images {
        let _ = capture(&engine.remove_image(image));
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn an_installed_engine_is_found_and_answers() {
    for engine in engines() {
        assert!(engine.bin().is_file(), "{:?} was found somewhere real", engine.kind());
        capture(&engine.info()).expect("a working engine describes itself");
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn builds_an_image_then_lives_a_container_through_its_whole_life() {
    for engine in engines() {
        let image = profile_image(PROFILE);
        let container = profile_container(PROJECT, PROFILE);
        let home = home_volume(PROJECT, PROFILE);
        clear(&engine, &[&container], &[&home], &[&image]);

        let scratch = Scratch::new("build");
        let containerfile =
            scratch.write("Containerfile", &format!("FROM {ALPINE}\nRUN echo built-by-qcode > /qcode-marker\n"));
        let mut lines = Vec::new();
        build_image(
            &engine,
            &ImageBuild { image: &image, containerfile: &containerfile, context: scratch.path() },
            &|| false,
            &mut |line| lines.push(line.to_owned()),
        )
        .expect("the image builds");
        assert!(!lines.is_empty(), "a build says what it is doing");
        capture(&engine.image_exists(&image)).expect("the image it just built is there");

        capture(&engine.create_volume(&home)).expect("a volume is made");
        let project = scratch.path().join("Project");
        std::fs::create_dir_all(&project).expect("a project folder");
        let mounts = [
            Mount {
                source: MountSource::Path(&project),
                target: Path::new("/work/Project"),
                access: Access::ReadWrite,
            },
            Mount { source: MountSource::Volume(&home), target: Path::new("/home/qcode"), access: Access::ReadWrite },
        ];
        capture(&engine.create_container(&ContainerCreate {
            name: &container,
            hostname: HOSTNAME,
            image: &image,
            mounts: &mounts,
            network: Network::Full,
            user: HostUser::current().expect("the current user"),
            workdir: Some(Path::new("/work/Project")),
            command: &["sleep", "600"],
        }))
        .expect("the container is made");
        assert_eq!(state(&engine, &container), ContainerState::Created);

        capture(&engine.start_container(&container)).expect("the container starts");
        assert_eq!(state(&engine, &container), ContainerState::Running);
        let listed = super::Container::parse_list(&capture(&engine.list_containers()).expect("a listing"));
        let found = listed.iter().find(|entry| entry.name == container).expect("the container is in the listing");
        assert_eq!(found.state, ContainerState::Running);

        // The engines only take `exec --tty` from a real terminal, which is how QCode uses it,
        // so this one goes through a pseudo-terminal like a tab does.
        let exec = engine
            .exec(&Exec { container: &container, command: &["sh", "-c", "cp /qcode-marker /work/Project/marker"] });
        assert_eq!(in_a_terminal(&exec), Some(0), "exec ran inside the container");
        let copied = std::fs::read_to_string(project.join("marker")).expect("the mount reaches the host");
        assert_eq!(copied.trim(), "built-by-qcode");

        capture(&engine.stop_container(&container)).expect("the container stops");
        assert_eq!(state(&engine, &container), ContainerState::Exited);
        capture(&engine.remove_container(&container)).expect("the container is removed");
        capture(&engine.container_state(&container)).expect_err("it is gone");
        capture(&engine.remove_volume(&home)).expect("the volume is removed");
        capture(&engine.remove_image(&image)).expect("the image is removed");
        capture(&engine.image_exists(&image)).expect_err("the image is gone");
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn carries_a_credential_into_a_volume_and_back_out() {
    for engine in engines() {
        let volume = credential_volume(PROFILE);
        let container = base_container(PROJECT);
        clear(&engine, &[&container], &[&volume], &[]);

        let scratch = Scratch::new("volume");
        let sent = scratch.path().join("sent");
        std::fs::create_dir_all(&sent).expect("a folder to send");
        std::fs::write(sent.join("credential.json"), "{\"token\":\"qcode\"}").expect("a credential to send");

        capture(&engine.create_volume(&volume)).expect("the volume is made");
        let mounts =
            [Mount { source: MountSource::Volume(&volume), target: Path::new("/mount"), access: Access::ReadWrite }];
        capture(&engine.create_container(&ContainerCreate {
            name: &container,
            hostname: HOSTNAME,
            image: ALPINE,
            mounts: &mounts,
            network: Network::None,
            user: HostUser::ImageDefault,
            workdir: None,
            command: &["sleep", "600"],
        }))
        .expect("a container to reach the volume through");
        capture(&engine.start_container(&container)).expect("it starts");

        capture(&engine.copy_in(&super::CopyIn {
            host: &sent.join("."),
            container: &container,
            target: Path::new("/mount"),
        }))
        .expect("the credential goes in");
        let back = scratch.path().join("back");
        std::fs::create_dir_all(&back).expect("a folder to receive");
        capture(&engine.copy_out(&super::CopyOut {
            container: &container,
            source: Path::new("/mount/."),
            host: &back,
        }))
        .expect("the credential comes out");
        assert_eq!(
            std::fs::read_to_string(back.join("credential.json")).expect("it came back"),
            "{\"token\":\"qcode\"}"
        );

        let volumes = capture(&engine.list_volumes()).expect("a listing");
        assert!(volumes.lines().any(|name| name == volume), "the volume is listed");
        clear(&engine, &[&container], &[&volume], &[]);
        let volumes = capture(&engine.list_volumes()).expect("a listing");
        assert!(!volumes.lines().any(|name| name == volume), "the volume is gone");
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_cancelled_build_leaves_no_image_behind() {
    for engine in engines() {
        let image = profile_image("cancelled");
        clear(&engine, &[], &[], &[&image]);
        let scratch = Scratch::new("cancel");
        let containerfile = scratch.write("Containerfile", &format!("FROM {ALPINE}\nRUN sleep 600\n"));

        let seen = std::cell::Cell::new(0_usize);
        let error = build_image(
            &engine,
            &ImageBuild { image: &image, containerfile: &containerfile, context: scratch.path() },
            &|| seen.get() > 0,
            &mut |_| seen.set(seen.get() + 1),
        )
        .expect_err("a cancelled build does not finish");
        assert!(matches!(error, EngineError::Cancelled { .. }), "{error:?}");
        capture(&engine.image_exists(&image)).expect_err("nothing half-built is left under the name");
    }
}

/// The state of one container, as the engine reports it.
fn state(engine: &Engine, container: &str) -> ContainerState {
    ContainerState::parse(&capture(&engine.container_state(container)).expect("a state"))
}

/// Runs an interactive command in a pseudo-terminal, the way a tab does, and waits for it to
/// end. Answers with its exit code.
fn in_a_terminal(command: &super::EngineCommand) -> Option<u32> {
    let session =
        TerminalSession::spawn(command.program.as_os_str(), &command.args, Path::new(".")).expect("a pseudo-terminal");
    let watch = session.watch();
    loop {
        if let TerminalEvent::Exited(code) = watch.next() {
            return code;
        }
    }
}
