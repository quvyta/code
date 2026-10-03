//! What no stand-in engine can answer for: that the images QCode takes away for itself are the
//! untagged ones a rebuild of a profile left behind, and that a tagged image, an image the person
//! made and an image a container is made from are all still there afterwards.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They pull `alpine` the first time, so
//! the first run needs the network.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::engine::names::{HOSTNAME, PROFILE_LABEL};
use crate::engine::run::{build_image, capture};
use crate::engine::{ContainerCreate, Engine, EngineCommand, EngineKind, HostUser, ImageBuild, Network, detect};

use super::work;

/// The image the stand-in images are built on, named in full: podman refuses a short name without
/// a terminal to ask at. `qcode/base` would answer the same rule and is a whole profile's
/// ingredient, which a test has no business building.
const ALPINE: &str = "docker.io/library/alpine:3";

/// The profile the images under test pretend to be, kept away from any real profile on the
/// machine.
const PROFILE: &str = "cleanuptest";

/// The image of the profile under test, and the one of a second profile whose first image a
/// container still runs from.
const TAGGED: &str = "qcode/profile/cleanuptest";
const HELD: &str = "qcode/profile/cleanuptest-held";

/// The container made from the image a rebuild replaced, and the image it is made from.
const CONTAINER: &str = "qcode-cleanuptest-keep";

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

/// A folder of this test's own, removed by [`Scratch`]'s `Drop`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-cleanup-live-{name}-{stamp}"));
        std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
        Self(path)
    }

    /// A Containerfile that adds `step` bytes of its own to `ALPINE`, carrying the profile's label
    /// when `labelled` says so. A second step makes a second image, which is what a rebuild is:
    /// the one before it keeps its layers and loses its name.
    fn step(&self, step: &str, labelled: bool) -> PathBuf {
        let label = if labelled { format!("LABEL {PROFILE_LABEL}=\"{PROFILE}\"\n") } else { String::new() };
        let file = self.0.join(format!("{step}.Containerfile"));
        std::fs::write(&file, format!("FROM {ALPINE}\n{label}RUN echo {step} > /qcode-cleanuptest\n"))
            .expect("a Containerfile");
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds `containerfile` as `image` and answers the identity the image then has.
fn build_tagged(engine: &Engine, image: &str, containerfile: &Path) -> String {
    build_image(
        engine,
        &ImageBuild { image, containerfile, context: containerfile.parent().unwrap_or(Path::new(".")) },
        &|| false,
        &mut |_| {},
    )
    .unwrap_or_else(|error| panic!("{:?}: {image} builds: {error:?}", engine.kind()));
    let id = capture(&engine.image_exists(image))
        .unwrap_or_else(|error| panic!("{:?}: {image} is there: {error:?}", engine.kind()));
    id.trim().to_owned()
}

/// Builds `containerfile` under no name at all, and answers the identity the engine gave it.
///
/// [`ImageBuild`] cannot spell this, and that is the whole case under test: an image nobody made
/// a name for is what every rebuild leaves behind, and one of the person's own is one of those
/// too. `--iidfile` is where the engine says which one it made, and both engines take it.
fn build_untagged(engine: &Engine, containerfile: &Path) -> String {
    let context = containerfile.parent().unwrap_or(Path::new("."));
    let iid = context.join(format!("{}.iid", containerfile.display()));
    let command = EngineCommand {
        program: engine.bin().to_owned(),
        args: [
            OsString::from("build"),
            OsString::from("--iidfile"),
            iid.as_os_str().to_owned(),
            OsString::from("--file"),
            containerfile.as_os_str().to_owned(),
            context.as_os_str().to_owned(),
        ]
        .into(),
        deadline: None,
    };
    let said =
        capture(&command).unwrap_or_else(|error| panic!("{:?}: an untagged image builds: {error:?}", engine.kind()));
    assert!(!said.is_empty(), "{:?}: the build said what it did", engine.kind());
    std::fs::read_to_string(&iid)
        .unwrap_or_else(|error| panic!("{:?}: the engine wrote the identity: {error}", engine.kind()))
        .trim()
        .to_owned()
}

/// Whether the engine still has the image `id`.
fn there(engine: &Engine, id: &str) -> bool {
    capture(&engine.image_exists(id)).is_ok()
}

/// Everything a run of this test makes on one engine, taken away again when it ends, whether the
/// test passed or gave up halfway: a container and four images, three of which are under no name
/// and so cannot be removed by the name they were built with.
struct Made {
    engine: Engine,
    ids: Vec<String>,
}

impl Made {
    fn new(engine: &Engine) -> Self {
        for name in [CONTAINER, TAGGED, HELD] {
            let _ = capture(&engine.remove_container(name));
            let _ = capture(&engine.remove_image(name));
        }
        Self { engine: engine.clone(), ids: Vec::new() }
    }

    /// An image this run made, to be taken away again whatever became of it.
    fn made(&mut self, id: &str) -> String {
        self.ids.push(id.to_owned());
        id.to_owned()
    }
}

impl Drop for Made {
    fn drop(&mut self) {
        for name in [CONTAINER, TAGGED, HELD] {
            let _ = capture(&self.engine.remove_container(name));
            let _ = capture(&self.engine.remove_image(name));
        }
        for id in &self.ids {
            let _ = capture(&self.engine.remove_image(id));
        }
        // Whatever untagged image of ours is still under no name is this test's — it is the only
        // thing on a machine that has been through this test that carries the label and no name —
        // and a QCode that started would take it away then too.
        let _ = work::clear_leftovers(&self.engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn an_image_of_ours_left_without_a_name_goes_and_nothing_else_does() {
    for engine in engines() {
        let scratch = Scratch::new("leftovers");
        let mut made = Made::new(&engine);

        // A profile's image, built and then built again: the first one is left on the disk under
        // no name at all, which is the whole of what there is to clean up.
        let first = made.made(&build_tagged(&engine, TAGGED, &scratch.step("first", true)));
        let second = build_tagged(&engine, TAGGED, &scratch.step("second", true));
        assert_ne!(first, second, "{:?}: a second build is a new image", engine.kind());

        // An image the person made: under no name, like a leftover, and carrying nothing of ours.
        let own = made.made(&build_untagged(&engine, &scratch.step("own", false)));

        // A second profile's image, whose first one a container is still made from. The engine
        // refuses that one and says why, and the container goes on as it was.
        let held = made.made(&build_tagged(&engine, HELD, &scratch.step("held", true)));
        let now = build_tagged(&engine, HELD, &scratch.step("now", true));
        assert_ne!(held, now, "{:?}: a second build is a new image", engine.kind());
        let request = ContainerCreate {
            name: CONTAINER,
            hostname: HOSTNAME,
            labels: &[],
            image: &held,
            mounts: &[],
            network: Network::None,
            user: HostUser::current().expect("the current user"),
            workdir: None,
            command: &["sleep", "600"],
        };
        capture(&engine.create_container(&request)).expect("the container is made");
        capture(&engine.start_container(CONTAINER)).expect("the container runs");
        capture(&engine.stop_container(CONTAINER)).expect("the container stops");

        // At least the one this test made, and exactly one of the two it made untagged with the
        // label: the other is a container's, and a machine that has been rebuilt a few times over
        // brings its own.
        assert!(work::clear_leftovers(&engine) >= 1, "{:?}: something of ours went", engine.kind());

        // Ours, untagged, made from by nothing: gone.
        assert!(!there(&engine, &first), "{:?}: the image the rebuild replaced is gone", engine.kind());
        // The person's own, untagged: still there.
        assert!(there(&engine, &own), "{:?}: an image without our label stays", engine.kind());
        // Ours, untagged, made from by a container: still there.
        assert!(there(&engine, &held), "{:?}: an image a container is made from stays", engine.kind());
        // Ours, with its name: still there.
        assert!(there(&engine, TAGGED), "{:?}: the image with a name stays", engine.kind());
        assert!(there(&engine, HELD), "{:?}: the other image with a name stays", engine.kind());
        // And the container kept the image it was made from.
        let held_now = capture(&engine.container_image(CONTAINER)).expect("the container's image");
        assert_eq!(held_now.trim(), held, "{:?}: the container still runs from its own image", engine.kind());
    }
}
