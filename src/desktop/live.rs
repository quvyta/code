//! The part of a desktop harness no record can answer for: that the image really builds from the
//! maker's archive, that the container really starts on both engines with the options QCode gives
//! it, that the application's own sandbox really comes up inside it, and that stopping it really
//! closes the window in a few seconds with nothing left behind.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. The image build downloads about 230 MiB
//! from the maker once per engine, and the built image is about 1.4 GiB, so these are slow and
//! large by nature.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```
//!
//! [`a_window_really_opens_on_this_screen`] is the one test that puts a window on the person's own
//! screen. It needs a Wayland session and it closes what it opened, but it is still something a
//! test suite should not do behind someone's back, so it asks for `QCODE_WINDOW_TESTS=1` as well.
//!
//! Everything a test makes is named `qcode/profile/desktoptest` or `qcode-desktoptest…` and is
//! removed again, image, container and volume alike. The machine's own `qcode/base` is built when
//! it is missing and left, as [`crate::base::ensure`] means it to be.

use std::path::Path;
use std::time::{Duration, Instant};

use super::{Display, RUNTIME_DIR, SHM_SIZE, WINDOW_GRACE, seccomp};
use crate::base::paths::{HOME_DIR, PROJECT_DIR};
use crate::engine::run::{build_image, capture};
use crate::engine::{
    Access, ContainerState, Engine, EngineKind, Exec, HostUser, ImageBuild, Mount, MountSource, Network, RunOnce,
    RunWindow, Tmpfs, detect,
};
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::ui::profiles::recipe;
use crate::ui::project::ContainerPlan;
use crate::workspace::{ProjectId, ProjectPaths};

/// The image and container these tests make.
const IMAGE: &str = "qcode/profile/desktoptest";
/// The project these tests open a window for.
const PROJECT: &str = "desktoptest";
/// The container a window is opened in, named the way the application names it.
const CONTAINER: &str = "qcode-desktoptest-desktoptest.desk";
/// The volume that stands in for a project's home volume.
const VOLUME: &str = "qcode-home-desktoptest-desktoptest";

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

/// The profile the window is opened for: the `recommended` template, so the settings the template
/// writes are in the image and can be read back.
fn profile() -> Profile {
    Profile {
        name: SafeName::parse("desktoptest").expect("the name is safe"),
        harness: HarnessKind::AntigravityIde,
        template: Template::Recommended,
        account: AccountKind::InApp,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
    }
}

/// Builds the profile image of the desktop harness on `engine`, from the maker's archive, and says
/// how long it took.
fn build(engine: &Engine) -> Duration {
    crate::base::ensure(engine, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("the base image does not build on {:?}: {error:?}", engine.kind()));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-desktoptest-{stamp}"));
    std::fs::create_dir_all(&folder).expect("a folder in the temporary folder");
    let recipe = recipe::image(&profile());
    std::fs::write(folder.join("Containerfile"), &recipe.containerfile).expect("a Containerfile");
    for (path, contents) in &recipe.files {
        let target = folder.join(path);
        std::fs::create_dir_all(target.parent().expect("the file has a folder")).expect("a folder");
        std::fs::write(&target, contents).expect("a template file");
    }
    let mut said = String::new();
    let started = Instant::now();
    let built = build_image(
        engine,
        &ImageBuild { image: IMAGE, containerfile: &folder.join("Containerfile"), context: &folder },
        &|| false,
        &mut |line| {
            said.push_str(line);
            said.push('\n');
        },
    );
    let took = started.elapsed();
    let _ = std::fs::remove_dir_all(&folder);
    assert!(built.is_ok(), "the desktop image does not build on {:?}:\n{said}", engine.kind());
    took
}

/// Takes away everything a run makes, except the machine's own base image.
fn clear(engine: &Engine) {
    let _ = capture(&engine.remove_container(CONTAINER));
    let _ = capture(&engine.remove_volume(VOLUME));
    let _ = capture(&engine.remove_image(IMAGE));
}

/// Runs `script` with a shell in the running container and answers what it printed.
fn run(engine: &Engine, script: &str) -> Result<String, String> {
    let command = ["sh", "-c", script];
    capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: &command }))
        .map_err(|error| format!("{error:?}"))
}

/// Whether the container is running right now.
fn running(engine: &Engine) -> bool {
    capture(&engine.container_state(CONTAINER)).is_ok_and(|word| ContainerState::parse(&word).is_running())
}

/// The command that opens the window of `display` in `engine`, for a project folder and a home
/// volume of this test's own.
///
/// It is the application's own command, built by [`ContainerPlan::open_window`] rather than
/// written again here: a copy written for the test would be the thing tested, and the first copy
/// of it silently left out the variables that tell the application where the compositor is, so
/// the window it opened was one no person would ever get.
fn open_command(engine: &Engine, project: &Path, display: &Display) -> crate::engine::EngineCommand {
    let paths = ProjectPaths {
        root: project.parent().expect("the project has a folder").to_path_buf(),
        file: project.with_file_name("project.qcode"),
        project: project.to_path_buf(),
        assets: project.with_file_name("Assets"),
        harness: project.with_file_name("Harness"),
    };
    let id = ProjectId::parse(PROJECT).expect("a usable project id");
    let plan = ContainerPlan::window(&id, &paths, &profile()).expect("this profile opens a window");
    assert_eq!(plan.name, CONTAINER, "the container these tests clean up is the one the window uses");
    assert_eq!(plan.image, IMAGE, "the image these tests build is the one the window uses");
    let seccomp = engine.needs_sandbox_profile().then(|| seccomp::file().expect("the profile is written"));
    plan.open_window(engine, HostUser::current().expect("the current user"), display, seccomp.as_deref())
        .expect("this plan opens a window")
}

/// A folder of this test's own, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-desktoplive-{stamp}"));
        std::fs::create_dir_all(path.join("Project")).expect("a project folder");
        // A window is opened for a whole project, and the application's container is given the
        // project's other folders as well; they have to be there before it starts.
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        std::fs::create_dir_all(path.join("Harness")).expect("a harness folder");
        std::fs::write(path.join("Project").join("README.md"), "hello\n").expect("a file in the project");
        Self(path)
    }

    fn project(&self) -> std::path::PathBuf {
        self.0.join("Project")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_image_builds_from_the_makers_archive_and_carries_the_application_and_its_settings() {
    let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
    for engine in engines() {
        clear(&engine);
        let took = build(&engine);
        // Nothing is asserted about how long it takes; it is printed so a change is visible.
        println!("{:?}: the desktop image built in {took:?}", engine.kind());

        // The size the record promises is the size that was built. A record that drifts is a
        // number shown to the person before they start a download of gigabytes, so it is checked
        // here rather than trusted: one tenth of slack for what an engine counts differently.
        let bytes: u64 = capture(&engine.image_size(IMAGE))
            .expect("the engine says how large the image is")
            .trim()
            .parse()
            .expect("a size in bytes");
        let mib = bytes / (1024 * 1024);
        println!("{:?}: the desktop image is {mib} MiB, the record says {}", engine.kind(), desktop.image_mib);
        let slack = desktop.image_mib / 10;
        assert!(
            mib.abs_diff(desktop.image_mib) <= slack,
            "{:?}: the record says {} MiB and the image is {mib} MiB; re-measure it",
            engine.kind(),
            desktop.image_mib
        );

        // A one-off container is enough to read the image: no window, no display, no sandbox.
        let read = |script: &str| {
            let command = ["sh", "-c", script];
            capture(&engine.run_once(&RunOnce {
                image: IMAGE,
                mounts: &[],
                network: Network::None,
                user: HostUser::current().expect("the current user"),
                workdir: None,
                command: &command,
            }))
            .unwrap_or_else(|error| panic!("{:?}: `{script}` failed: {error:?}", engine.kind()))
        };
        // The program the record names is there and can be run.
        assert_eq!(read(&format!("test -x '{}' && echo yes", desktop.command())).trim(), "yes");
        // The version the record describes is the one that was unpacked. It is `ideVersion` in
        // the application's own `product.json`; `package.json` beside it carries the version of
        // the editor this application is built on, which is a different number entirely.
        let product = read(&format!("cat '{}/resources/app/product.json'", desktop.install_dir));
        assert!(
            product.contains(&format!("\"ideVersion\": \"{}\"", desktop.version)),
            "{:?}: the archive is not {}",
            engine.kind(),
            desktop.version
        );
        // The set-user-id helper the sandbox falls back to kept its bit, because root unpacked it.
        let sandbox = read(&format!("ls -l '{}/chrome-sandbox'", desktop.install_dir));
        assert!(sandbox.contains("rws") || sandbox.contains("rwsr"), "{:?}: {sandbox}", engine.kind());
        // The image is its own user again, never root.
        assert_eq!(read("id -un").trim(), crate::base::paths::USER);
        // The settings the template writes are in the image's home, ready to be copied into a
        // project's home volume the first time one is filled.
        let settings = read(&format!("cat '{HOME_DIR}/.config/Antigravity IDE/User/settings.json'"));
        assert!(settings.contains("\"telemetry.telemetryLevel\": \"off\""), "{:?}: {settings}", engine.kind());
        assert!(settings.contains("\"update.mode\": \"none\""), "{:?}: {settings}", engine.kind());
        assert!(!settings.contains("workspace.trust"), "the trust question is the person's: {settings}");
        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_container_starts_stops_and_leaves_nothing_behind_on_both_engines() {
    // The lifecycle without a window: the container runs the application's own `--version`, which
    // needs none of the display, so this runs on a machine with no compositor at all.
    let scratch = Scratch::new();
    let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
    for engine in engines() {
        clear(&engine);
        build(&engine);
        let mounts = [Mount {
            source: MountSource::Path(&scratch.project()),
            target: Path::new(PROJECT_DIR),
            access: Access::ReadWrite,
        }];
        let seccomp = engine.needs_sandbox_profile().then(|| seccomp::file().expect("the profile is written"));
        let tmpfs = [Tmpfs { target: Path::new(RUNTIME_DIR), mode: super::RUNTIME_MODE }];
        // `sleep` rather than the application: the point here is the container's own life, and a
        // window would need a screen. The options are the ones a window is given.
        let command = engine.run_window(&RunWindow {
            name: CONTAINER,
            once: RunOnce {
                image: IMAGE,
                mounts: &mounts,
                network: Network::None,
                user: HostUser::current().expect("the current user"),
                workdir: Some(Path::new(PROJECT_DIR)),
                command: &["sh", "-c", "trap 'exit 0' TERM; while :; do sleep 1 & wait $!; done"],
            },
            env: &[],
            sockets: &[],
            tmpfs: &tmpfs,
            devices: &[],
            shm: SHM_SIZE,
            seccomp: seccomp.as_deref(),
        });
        capture(&command).unwrap_or_else(|error| panic!("{:?}: the container does not run: {error:?}", engine.kind()));
        assert!(running(&engine), "{:?}", engine.kind());

        // What the container was given: its own runtime folder, unreadable to anyone else, and the
        // shared memory the application needs.
        let mode = run(&engine, &format!("stat -c %a '{RUNTIME_DIR}'")).expect("the folder is there");
        assert_eq!(mode.trim(), "700", "{:?}", engine.kind());
        let owned = run(&engine, &format!("test -w '{RUNTIME_DIR}' && echo yes")).expect("the folder answers");
        assert_eq!(owned.trim(), "yes", "{:?}: the runtime folder is not the container's own", engine.kind());
        let shm = run(&engine, "df -m /dev/shm | tail -1").expect("shared memory is there");
        let megabytes: u64 =
            shm.split_whitespace().nth(1).and_then(|word| word.parse().ok()).expect("a size in mebibytes");
        assert!(megabytes >= 1_000, "{:?}: /dev/shm is only {megabytes} MiB", engine.kind());
        // The program the window would run is there, and the project is where the record says.
        assert_eq!(run(&engine, &format!("test -x '{}' && echo yes", desktop.command())).expect("it is").trim(), "yes");
        assert_eq!(run(&engine, &format!("cat '{PROJECT_DIR}/README.md'")).expect("the project").trim(), "hello");

        // The stop closes it in its own time rather than being killed at the end of the grace.
        let started = Instant::now();
        capture(&engine.stop_container_within(CONTAINER, WINDOW_GRACE))
            .unwrap_or_else(|error| panic!("{:?}: the container will not stop: {error:?}", engine.kind()));
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(9),
            "{:?}: the stop took {took:?}, so the signal was not answered",
            engine.kind()
        );
        assert!(!running(&engine), "{:?}", engine.kind());
        // And the wait answers the code, which is how a closed window reaches QCode.
        assert_eq!(
            capture(&engine.wait_container(CONTAINER)).expect("the wait answers").trim().lines().last(),
            Some("0"),
            "{:?}",
            engine.kind()
        );
        clear(&engine);
        assert!(!running(&engine), "{:?}: something is left running", engine.kind());
    }
}

#[test]
#[ignore = "opens a real window on this screen; run with QCODE_CONTAINER_TESTS=1 QCODE_WINDOW_TESTS=1"]
fn a_window_really_opens_on_this_screen() {
    if std::env::var("QCODE_WINDOW_TESTS").as_deref() != Ok("1") {
        return;
    }
    let display = super::current().expect("this session has a Wayland compositor");
    let scratch = Scratch::new();
    let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
    // One window at a time, and each one closed before the next engine is tried.
    for engine in engines() {
        clear(&engine);
        build(&engine);
        let started = Instant::now();
        capture(&open_command(&engine, &scratch.project(), &display))
            .unwrap_or_else(|error| panic!("{:?}: the window does not open: {error:?}", engine.kind()));

        // The window is on its way up. What says the application's own sandbox came up is a
        // process carrying a second seccomp filter: the container's filter plus Chromium's own.
        // The process names cannot say it, because a renderer is forked from a zygote and keeps
        // the zygote's name in the process list until it re-executes, so there is often no
        // process called `--type=renderer` at all while the window is perfectly up.
        let mut filters = String::new();
        for _ in 0..80 {
            std::thread::sleep(Duration::from_millis(500));
            assert!(
                running(&engine),
                "{:?}: the container ended while opening, saying: {}",
                engine.kind(),
                capture(&engine.container_logs(CONTAINER)).unwrap_or_else(|error| format!("{error:?}"))
            );
            filters = run(&engine, "grep -h Seccomp_filters /proc/*/status 2>/dev/null | sort -u").unwrap_or_default();
            if filters.lines().any(|line| line.split_whitespace().nth(1) == Some("2")) {
                break;
            }
        }
        println!("{:?}: the window came up in {:?}", engine.kind(), started.elapsed());
        assert!(
            filters.lines().any(|line| line.split_whitespace().nth(1) == Some("2")),
            "{:?}: no process has a filter of its own on top of the container's: {filters}",
            engine.kind()
        );
        // Never turned off: the application must not be running with its sandbox disabled.
        let started_with = run(&engine, "ps -e -o args=").unwrap_or_default();
        assert!(started_with.contains("--ozone-platform=wayland"), "{:?}: {started_with}", engine.kind());
        assert!(!started_with.contains("--no-sandbox"), "{:?}: the sandbox is off: {started_with}", engine.kind());

        // Asking it to come forward starts the application a second time, which tells the one
        // already running and exits. Whether the window is raised or only marked is the
        // compositor's; that it does not fail is what can be checked here.
        let raise = ["sh", "-c", &format!("'{}' {PROJECT_DIR}", desktop.command())];
        let _ = capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: &raise }));
        assert!(running(&engine), "{:?}: asking it to come forward ended it", engine.kind());

        // And closing it: the stop is what closes the window, and nothing is left afterwards.
        let stopped = Instant::now();
        capture(&engine.stop_container_within(CONTAINER, WINDOW_GRACE))
            .unwrap_or_else(|error| panic!("{:?}: the window will not close: {error:?}", engine.kind()));
        println!("{:?}: the window closed in {:?}", engine.kind(), stopped.elapsed());
        assert!(!running(&engine), "{:?}", engine.kind());
        clear(&engine);
    }
}
