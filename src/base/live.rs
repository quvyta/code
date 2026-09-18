//! The part of the base image no text can answer for: that the image really builds on both
//! engines, that a container of it really stays up, and that the person the container runs as
//! can really write where the contract says they can.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They reach the network the first time:
//! the Node image is pulled and one harness is installed from npm.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```
//!
//! Everything a test makes is named `qcode-basetest-…` or `qcode/basetest-…` and is removed
//! again, so nothing on the machine that QCode itself made is touched. The one exception is
//! [`super::ensure`], whose whole subject is the real `qcode/base`: it builds that image and
//! leaves it, because it is the image the machine is meant to have and taking it away again
//! would cost the next profile build a quarter of an hour.

use std::path::{Path, PathBuf};

use super::paths::{ASSETS_DIR, HOME_DIR, KEEP_ALIVE, OPEN_HOME, PROJECT_DIR};
use super::{Outcome, Presence, containerfile, ensure, presence};
use crate::engine::names::HOSTNAME;
use crate::engine::run::{build_image, capture};
use crate::engine::{
    Access, ContainerCreate, ContainerState, Engine, EngineKind, Exec, HostUser, ImageBuild, Mount, MountSource,
    Network, detect,
};
use crate::profile::HarnessKind;

/// The image these tests build for themselves, so the machine's own `qcode/base` is neither
/// read nor written by the tests that do not mean to.
const TEST_BASE: &str = "qcode/basetest-base";

/// The image a harness is installed into, standing in for a profile image.
const TEST_PROFILE: &str = "qcode/basetest-profile";

/// The container these tests live in.
const CONTAINER: &str = "qcode-basetest-container";

/// The volume standing in for a project's home.
const HOME_VOLUME: &str = "qcode-basetest-home";

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

/// A folder of this test's own, removed by `Drop`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-basetest-{name}-{stamp}"));
        std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
        Self(path)
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
        path
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let file = self.0.join(name);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("a folder in the temporary folder");
        }
        std::fs::write(&file, text).expect("a file in the temporary folder");
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Takes away whatever an earlier run left behind. The failures are the point of it being here:
/// there is usually nothing to remove.
fn clear(engine: &Engine) {
    let _ = capture(&engine.remove_container(CONTAINER));
    let _ = capture(&engine.remove_volume(HOME_VOLUME));
    let _ = capture(&engine.remove_image(TEST_PROFILE));
    let _ = capture(&engine.remove_image(TEST_BASE));
}

/// Builds `image` from `text` and says nothing unless it fails, in which case it says everything
/// the engine said.
fn build(engine: &Engine, image: &str, text: &str, scratch: &Scratch) {
    let containerfile = scratch.file(&format!("{}/Containerfile", image.replace('/', "-")), text);
    let context = containerfile.parent().expect("the file is in a folder").to_path_buf();
    let mut said = String::new();
    let built = build_image(
        engine,
        &ImageBuild { image, containerfile: &containerfile, context: &context },
        &|| false,
        &mut |line| {
            said.push_str(line);
            said.push('\n');
        },
    );
    assert!(built.is_ok(), "{image} on {:?} did not build:\n{said}", engine.kind());
    assert!(!said.is_empty(), "a build says what it is doing");
}

/// Runs `script` in the container with a shell, without a terminal, and answers with what it
/// printed. Fails with the shell's own words when it refuses.
fn run_in(engine: &Engine, script: &str) -> String {
    let command = ["sh", "-c", script];
    let asked = capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: &command }));
    match asked {
        Ok(output) => output,
        Err(error) => {
            let kind = engine.kind();
            panic!("`{script}` on {kind:?} failed: {error:?}")
        }
    }
}

/// Creates and starts the test container from `image`, with the mounts of the path contract.
fn start(engine: &Engine, image: &str, project: &Path, assets: &Path) {
    let mounts = [
        Mount { source: MountSource::Path(project), target: Path::new(PROJECT_DIR), access: Access::ReadWrite },
        Mount { source: MountSource::Path(assets), target: Path::new(ASSETS_DIR), access: Access::ReadOnly },
        Mount { source: MountSource::Volume(HOME_VOLUME), target: Path::new(HOME_DIR), access: Access::ReadWrite },
    ];
    capture(&engine.create_container(&ContainerCreate {
        name: CONTAINER,
        hostname: HOSTNAME,
        image,
        mounts: &mounts,
        network: Network::Full,
        // The whole point: the container runs as the person, and the image was built by someone
        // else entirely.
        user: HostUser::current().expect("the current user"),
        workdir: Some(Path::new(PROJECT_DIR)),
        command: KEEP_ALIVE,
    }))
    .expect("the container is made");
    capture(&engine.start_container(CONTAINER)).expect("the container starts");
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_image_builds_and_keeps_a_container_that_writes_where_the_contract_says() {
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("contract");
        build(&engine, TEST_BASE, &containerfile(), &scratch);

        let project = scratch.dir("Project");
        let assets = scratch.dir("Assets");
        start(&engine, TEST_BASE, &project, &assets);
        assert_eq!(
            ContainerState::parse(&capture(&engine.container_state(CONTAINER)).expect("a state")),
            ContainerState::Running,
            "{:?}: the keep-alive command holds the container up",
            engine.kind()
        );

        // The project reaches the host, and what it leaves there belongs to the person.
        run_in(&engine, &format!("echo from-the-container > {PROJECT_DIR}/marker"));
        let marker = project.join("marker");
        assert_eq!(
            std::fs::read_to_string(&marker).expect("the host sees the file").trim(),
            "from-the-container",
            "{:?}",
            engine.kind()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let HostUser::Ids { uid, .. } = HostUser::current().expect("the current user") else {
                panic!("a unix machine has ids")
            };
            assert_eq!(
                std::fs::metadata(&marker).expect("the file is there").uid(),
                uid,
                "{:?}: what the container writes belongs to the person, not to root",
                engine.kind()
            );
        }

        // The home directory is the one place a harness keeps anything, so the person must be
        // able to write into it and into a directory they make inside it.
        run_in(&engine, &format!("test -w {HOME_DIR}"));
        run_in(&engine, &format!("mkdir -p {HOME_DIR}/.qcode-probe && echo kept > {HOME_DIR}/.qcode-probe/file"));
        assert_eq!(run_in(&engine, &format!("cat {HOME_DIR}/.qcode-probe/file")).trim(), "kept");
        assert_eq!(run_in(&engine, "echo $HOME").trim(), HOME_DIR, "{:?}: the image names the home", engine.kind());

        // Assets are mounted read-only here, and a read-only mount that can be written to is a
        // permission the profile never gave.
        let output = capture(&engine.exec_without_terminal(&Exec {
            container: CONTAINER,
            command: &["sh", "-c", &format!("echo no > {ASSETS_DIR}/no")],
        }));
        assert!(output.is_err(), "{:?}: a read-only mount was written to", engine.kind());

        // Stopping is asked of the keep-alive command itself; one that ignores the request is
        // killed by the engine only after its ten-second timeout, which every stop would pay.
        let asked = std::time::Instant::now();
        capture(&engine.stop_container(CONTAINER)).expect("the container stops");
        let took = asked.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "{:?}: stopping took {took:?}", engine.kind());
        assert_eq!(
            ContainerState::parse(&capture(&engine.container_state(CONTAINER)).expect("a state")),
            ContainerState::Exited,
            "{:?}",
            engine.kind()
        );

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_repository_is_cloned_inside_the_container() {
    // A project can be made from a git address without the person having git, which only holds
    // if the image has it. The repository is made inside the container too, so the test needs
    // neither the network nor git on this machine.
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("clone");
        build(&engine, TEST_BASE, &containerfile(), &scratch);
        start(&engine, TEST_BASE, &scratch.dir("Project"), &scratch.dir("Assets"));

        run_in(
            &engine,
            &format!(
                "set -e\n\
                 mkdir -p $HOME/seed && cd $HOME/seed\n\
                 git init --quiet --initial-branch=main .\n\
                 echo merhaba > hello.txt\n\
                 git add hello.txt\n\
                 git -c user.email=qcode@example.invalid -c user.name=qcode commit --quiet -m seed\n\
                 cd {PROJECT_DIR}\n\
                 git clone --progress -- $HOME/seed cloned"
            ),
        );
        assert_eq!(run_in(&engine, &format!("cat {PROJECT_DIR}/cloned/hello.txt")).trim(), "merhaba");

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_harness_installs_on_top_of_the_image_and_answers() {
    // The base image exists to be built on. This builds what a profile image is — the harness
    // installed with its own command, the configuration of the `recommended` template copied
    // into the home directory — and then checks the two things that make it usable: the harness
    // runs, and the person the container runs as can write beside the settings the build wrote,
    // which is where the login goes.
    let harness = HarnessKind::ClaudeCode;
    let record = harness.record();
    let settings = record.settings.expect("Claude Code has a settings file");
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("harness");
        build(&engine, TEST_BASE, &containerfile(), &scratch);

        let mut lines = vec![format!("FROM {TEST_BASE}")];
        for step in record.install {
            lines.push(format!("RUN {step}"));
        }
        // The same shape `ui::profiles::recipe` writes: the file is staged and moved into the
        // home directory by the image's own shell, so the recipe never names that directory.
        lines.push(format!(
            "RUN mkdir -p \"$HOME/$(dirname '{path}')\" && printf '%s' '{contents}' > \"$HOME/{path}\"",
            path = settings.path,
            contents = settings.contents.replace('\n', " ")
        ));
        lines.push(format!("RUN {OPEN_HOME}"));
        lines.push(String::new());
        build(&engine, TEST_PROFILE, &lines.join("\n"), &scratch);

        start(&engine, TEST_PROFILE, &scratch.dir("Project"), &scratch.dir("Assets"));
        let version = capture(
            &engine.exec_without_terminal(&Exec { container: CONTAINER, command: &[record.command, "--version"] }),
        )
        .unwrap_or_else(|error| panic!("{} does not answer on {:?}: {error:?}", record.command, engine.kind()));
        assert!(version.trim().starts_with(|c: char| c.is_ascii_digit()), "{:?}: `{version}`", engine.kind());

        // The login goes beside the settings the build wrote, in a directory the build made.
        let login = record.identity.first().expect("the harness says where its login lives");
        run_in(&engine, &format!("printf '%s' '{{}}' > \"$HOME/{login}\""));
        assert_eq!(run_in(&engine, &format!("cat \"$HOME/{login}\"")).trim(), "{}");
        // And the settings themselves can still be changed by whoever runs.
        run_in(&engine, &format!("test -w \"$HOME/{}\"", settings.path));

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_image_is_built_once_and_then_left_alone() {
    // This one works on the machine's real `qcode/base`, because that is what `ensure` is for,
    // and leaves it behind on purpose: it is the image the machine is meant to have.
    for engine in engines() {
        let mut lines = 0_usize;
        let first = ensure(&engine, &|| false, &mut |_| lines += 1)
            .unwrap_or_else(|error| panic!("the base image does not build on {:?}: {error:?}", engine.kind()));
        if first == Outcome::Built {
            assert!(lines > 0, "{:?}: a build says what it is doing", engine.kind());
        }
        assert_eq!(presence(&engine), Presence::Current, "{:?}", engine.kind());

        let mut again = 0_usize;
        let second = ensure(&engine, &|| false, &mut |_| again += 1).expect("the image is already there");
        assert_eq!(second, Outcome::AlreadyThere, "{:?}: an image that is current is not built again", engine.kind());
        assert_eq!(again, 0, "{:?}: and nothing runs to find that out", engine.kind());
    }
}
