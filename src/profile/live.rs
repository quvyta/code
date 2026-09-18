//! The part of the identity copy no argument list can answer for: that a login stored in a
//! profile's volume really reaches the home volume of every project that uses the profile, that
//! a project whose harness is running is really left alone, and that a profile without a login
//! really gives nothing and makes nothing.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They pull `alpine` the first time, so
//! the first run needs the network.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```
//!
//! Everything a test makes carries `identitytest` in its name and is removed again, so nothing
//! on the machine that QCode itself made is touched.

use std::path::Path;

use super::SafeName;
use super::identity::{self, Home, RefreshError, Refreshed, STORE_DIR};
use crate::base::paths::{HOME_DIR, KEEP_ALIVE};
use crate::engine::run::{build_image, capture};
use crate::engine::{
    Access, ContainerCreate, CopyIn, Engine, EngineKind, Exec, HostUser, ImageBuild, Mount, MountSource, Network,
    detect, names,
};
use crate::workspace::ProjectId;

/// The image the stand-in profile image is built on, named in full: podman refuses a short name
/// without a terminal to ask at.
const ALPINE: &str = "docker.io/library/alpine:3";

/// The profile these tests pretend to be, kept away from any real profile on the machine.
const PROFILE: &str = "identitytest";

/// The projects that use it.
const PROJECTS: [&str; 2] = ["identitytest-a", "identitytest-b"];

/// The file the stand-in login is, where a Claude Code login would be.
const LOGIN: &str = ".claude/.credentials.json";

/// The container that puts the stand-in login into the credentials volume.
const SEED: &str = "qcode-identitytest-seed";

/// The container that reads a home volume back.
const READER: &str = "qcode-identitytest-reader";

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

fn profile() -> SafeName {
    SafeName::parse(PROFILE).expect("the name is safe")
}

fn projects() -> Vec<ProjectId> {
    PROJECTS.iter().map(|id| ProjectId::parse(id).expect("the id is legal")).collect()
}

fn homes() -> Vec<Home> {
    projects().into_iter().map(|project| Home::new(profile(), project)).collect()
}

fn user() -> HostUser {
    HostUser::current().expect("the current user is readable")
}

/// Builds the image that stands in for the profile's: alpine with the home directory the
/// contract names, so the courier's copy lands where a harness container would read it.
fn stand_in_image(engine: &Engine) -> String {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-identitytest-{stamp}"));
    std::fs::create_dir_all(&folder).expect("a folder in the temporary folder");
    let containerfile = folder.join("Containerfile");
    std::fs::write(&containerfile, format!("FROM {ALPINE}\nRUN mkdir -p {HOME_DIR} && chmod 0777 {HOME_DIR}\n"))
        .expect("a Containerfile");
    let image = names::profile_image(PROFILE);
    let _ = capture(&engine.remove_image(&image));
    build_image(
        engine,
        &ImageBuild { image: &image, containerfile: &containerfile, context: &folder },
        &|| false,
        &mut |_| {},
    )
    .expect("the stand-in profile image builds");
    let _ = std::fs::remove_dir_all(&folder);
    image
}

/// Runs `script` in a fresh container of the stand-in image with `volume` mounted at `at`, after
/// `copy` when there is one, and answers what it printed, or the engine's words.
fn with_volume(
    engine: &Engine,
    container: &str,
    volume: &str,
    at: &str,
    script: &str,
    copy: Option<&CopyIn<'_>>,
) -> Result<String, String> {
    let _ = capture(&engine.remove_container(container));
    let mounts = [Mount { source: MountSource::Volume(volume), target: Path::new(at), access: Access::ReadWrite }];
    let request = ContainerCreate {
        name: container,
        hostname: names::HOSTNAME,
        image: &names::profile_image(PROFILE),
        mounts: &mounts,
        network: Network::None,
        user: user(),
        workdir: None,
        command: KEEP_ALIVE,
    };
    let answer = capture(&engine.create_container(&request))
        .and_then(|_| capture(&engine.start_container(container)))
        .and_then(|_| copy.map_or(Ok(String::new()), |copy| capture(&engine.copy_in(copy))))
        .and_then(|_| capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] })))
        .map_err(|error| format!("{error:?}"));
    let _ = capture(&engine.remove_container(container));
    answer
}

/// Puts a stand-in login into the profile's credentials volume the way a capture does: copied in
/// from a folder of this machine through a container that holds the volume open. Written from
/// inside as the person's user it could not be: on docker a volume mounted where the image has
/// no directory belongs to root, and the copy from the host is what gets past that.
fn seed_login(engine: &Engine, token: &str) {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-identitytest-login-{stamp}"));
    let file = folder.join(LOGIN);
    std::fs::create_dir_all(file.parent().expect("the login has a folder")).expect("a folder in the temporary folder");
    std::fs::write(&file, token).expect("a file in the temporary folder");
    let store = names::credential_volume(PROFILE);
    let copy = CopyIn { host: &folder.join("."), container: SEED, target: Path::new(STORE_DIR) };
    let script = format!("test -f {STORE_DIR}/{LOGIN}");
    with_volume(engine, SEED, &store, STORE_DIR, &script, Some(&copy)).expect("the stand-in login is stored");
    let _ = std::fs::remove_dir_all(&folder);
}

/// What the login file in `home` says, or nothing when there is none.
fn login_in(engine: &Engine, home: &Home) -> Option<String> {
    with_volume(engine, READER, &home.volume(), HOME_DIR, &format!("cat {HOME_DIR}/{LOGIN}"), None).ok()
}

/// Takes away everything a run of these tests makes.
fn clear(engine: &Engine) {
    for home in homes() {
        let _ = capture(&engine.remove_container(&home.container()));
        let _ = capture(&engine.remove_container(&format!("qcode-refresh-{}-{}", home.project(), home.profile())));
        let _ = capture(&engine.remove_volume(&home.volume()));
    }
    for container in [SEED, READER] {
        let _ = capture(&engine.remove_container(container));
    }
    let _ = capture(&engine.remove_volume(&names::credential_volume(PROFILE)));
    let _ = capture(&engine.remove_image(&names::profile_image(PROFILE)));
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_stored_login_reaches_the_home_of_every_project_that_uses_the_profile() {
    for engine in engines() {
        clear(&engine);
        stand_in_image(&engine);
        seed_login(&engine, "first");

        let done = identity::refresh_all(&engine, &profile(), &projects(), user()).expect("the copies are made");
        assert_eq!(done, Refreshed { written: projects(), running: Vec::new() }, "{:?}", engine.kind());
        for home in homes() {
            assert_eq!(login_in(&engine, &home).as_deref(), Some("first"), "{:?}: {}", engine.kind(), home.volume());
        }

        // A second login replaces the first in every project: that is what refreshing means.
        seed_login(&engine, "second");
        identity::refresh_all(&engine, &profile(), &projects(), user()).expect("the copies are made again");
        for home in homes() {
            assert_eq!(login_in(&engine, &home).as_deref(), Some("second"), "{:?}: {}", engine.kind(), home.volume());
        }
        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_project_whose_harness_is_running_is_left_alone_and_named() {
    for engine in engines() {
        clear(&engine);
        stand_in_image(&engine);
        seed_login(&engine, "first");
        identity::refresh_all(&engine, &profile(), &projects(), user()).expect("the copies are made");

        // The first project's container is up, standing in for a harness someone is using.
        let [busy, idle] = homes().try_into().expect("two homes");
        let mounts = [Mount {
            source: MountSource::Volume(&busy.volume()),
            target: Path::new(HOME_DIR),
            access: Access::ReadWrite,
        }];
        let request = ContainerCreate {
            name: &busy.container(),
            hostname: names::HOSTNAME,
            image: &names::profile_image(PROFILE),
            mounts: &mounts,
            network: Network::None,
            user: user(),
            workdir: None,
            command: KEEP_ALIVE,
        };
        capture(&engine.create_container(&request)).expect("the busy container is made");
        capture(&engine.start_container(&busy.container())).expect("the busy container starts");

        seed_login(&engine, "second");
        let done = identity::refresh_all(&engine, &profile(), &projects(), user()).expect("the round completes");
        assert_eq!(
            done,
            Refreshed { written: vec![idle.project().clone()], running: vec![busy.project().clone()] },
            "{:?}",
            engine.kind()
        );
        capture(&engine.remove_container(&busy.container())).expect("the busy container is removed");
        assert_eq!(login_in(&engine, &busy).as_deref(), Some("first"), "{:?}: left alone", engine.kind());
        assert_eq!(login_in(&engine, &idle).as_deref(), Some("second"), "{:?}: rewritten", engine.kind());
        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_profile_without_a_login_gives_nothing_and_makes_no_volume() {
    for engine in engines() {
        clear(&engine);
        stand_in_image(&engine);
        let store = names::credential_volume(PROFILE);

        let refused = identity::refresh_all(&engine, &profile(), &projects(), user()).expect_err("nothing to give");
        assert_eq!(refused, RefreshError::NotStored, "{:?}", engine.kind());

        let [home, _] = homes().try_into().expect("two homes");
        assert!(!identity::home_exists(&engine, &home).expect("the volumes are listed"), "{:?}", engine.kind());
        identity::first_fill(&engine, &home, user()).expect("a profile without a login is not a failure");

        let listing = capture(&engine.list_volumes()).expect("a listing");
        let made = |volume: &str| listing.lines().any(|line| line.trim() == volume);
        assert!(!made(&store), "{:?}: no credentials volume came into being: {listing}", engine.kind());
        assert!(!made(&home.volume()), "{:?}: and no home either: {listing}", engine.kind());
        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_home_made_for_a_signed_in_profile_starts_with_its_login() {
    for engine in engines() {
        clear(&engine);
        stand_in_image(&engine);
        seed_login(&engine, "first");
        let [home, _] = homes().try_into().expect("two homes");

        assert!(!identity::home_exists(&engine, &home).expect("the volumes are listed"), "{:?}", engine.kind());
        identity::first_fill(&engine, &home, user()).expect("the login is copied in");
        assert!(identity::home_exists(&engine, &home).expect("the volumes are listed"), "{:?}", engine.kind());
        assert_eq!(login_in(&engine, &home).as_deref(), Some("first"), "{:?}", engine.kind());
        clear(&engine);
    }
}
