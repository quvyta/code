//! A home volume really crosses from one engine to the other, readable and writable by the person
//! on the far side, and stays where it was.
//!
//! `#[ignore]`d and switched on with `QCODE_CONTAINER_TESTS=1`; it needs both engines and QCode's
//! base image in each (it is built when missing). It only ever touches a volume named after a
//! workspace and a profile nobody has, which it removes on both engines when it ends.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test switch::live -- --ignored --test-threads=1
//! ```

use std::path::Path;

use crate::base::paths::HOME_DIR;
use crate::engine::names::{BASE_IMAGE, home_volume};
use crate::engine::run::capture;
use crate::engine::{Access, Engine, EngineKind, HostUser, Mount, MountSource, Network, RunOnce, detect};

/// Runs `script` in the base image with the volume at the home folder as its working folder, as
/// `user`, and returns what it printed, or the refusal in its place so the volumes are still
/// cleared before the test fails.
fn inside(engine: &Engine, volume: &str, user: HostUser, script: &str) -> String {
    let mounts =
        [Mount { source: MountSource::Volume(volume), target: Path::new(HOME_DIR), access: Access::ReadWrite }];
    let command = ["sh", "-c", script];
    let once = RunOnce {
        image: BASE_IMAGE,
        mounts: &mounts,
        network: Network::None,
        user,
        workdir: Some(Path::new(HOME_DIR)),
        command: &command,
    };
    capture(&engine.run_once(&once)).unwrap_or_else(|error| format!("{:?} refused: {error:?}", engine.kind()))
}

#[test]
#[ignore = "copies a volume between the two real engines; run with QCODE_CONTAINER_TESTS=1"]
fn a_home_is_copied_to_the_other_engine_and_kept_on_the_first() {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return;
    }
    let (Ok(podman), Ok(docker)) = (detect(EngineKind::Podman), detect(EngineKind::Docker)) else {
        panic!("both engines are needed for a switch")
    };
    let user = HostUser::current().expect("the person's ids");
    let volume = home_volume("motortest", "motorprof");
    let clear = || {
        let _ = capture(&podman.remove_volume(&volume));
        let _ = capture(&docker.remove_volume(&volume));
    };
    clear();
    for (from, to) in [(&podman, &docker), (&docker, &podman)] {
        capture(&from.create_volume(&volume)).expect("the volume is made");
        let marker = format!("marker from {:?}", from.kind());
        let wrote = inside(
            from,
            &volume,
            user,
            &format!("mkdir -p .claude && printf '%s' '{marker}' > .claude/marker && chmod 600 .claude/marker"),
        );
        let copied = super::copy(from, to, &volume, user, false);
        let there = inside(to, &volume, user, "cat .claude/marker; touch .claude/written && echo ' writable'");
        let kept = inside(from, &volume, user, "cat .claude/marker");
        clear();
        assert!(!wrote.contains("refused"), "{wrote}");
        copied.unwrap_or_else(|said| panic!("{:?} to {:?}: {said}", from.kind(), to.kind()));
        assert_eq!(there.trim(), format!("{marker} writable"), "{:?} to {:?}", from.kind(), to.kind());
        assert_eq!(kept, marker, "the volume is still on {:?}, as it was", from.kind());
    }
}
