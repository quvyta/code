//! The part of the login flow no state machine can answer for: that a login really leaves the
//! container it was made in, that a login that never happened really leaves nothing behind, and
//! that a template's configuration really lands in the image's home directory.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They pull `alpine` the first time, so
//! the first run needs the network.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```
//!
//! The harnesses themselves are not signed in to here: that needs an account. What is checked is
//! everything around the sign-in, with a shell standing in for the harness and writing exactly
//! the file the harness record says it writes.

use std::path::Path;

use crate::base::paths::OPEN_HOME;
use crate::engine::names;
use crate::engine::run::{build_image, capture};
use crate::engine::{Engine, EngineKind, Exec, ImageBuild, detect};
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};

use super::recipe;
use super::status::Readiness;
use super::work::{self, Problem};

/// The image the stand-in profile image is built on, named in full: podman refuses a short name
/// without a terminal to ask at.
const ALPINE: &str = "docker.io/library/alpine:3";

/// The profile these tests pretend to be, kept away from any real profile on the machine.
const PROFILE: &str = "uilivetest";

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

/// The profile under test: Claude Code, whose login is one file under the home directory.
fn profile() -> Profile {
    Profile {
        name: SafeName::parse(PROFILE).expect("the name is safe"),
        harness: HarnessKind::ClaudeCode,
        template: Template::Recommended,
        account: AccountKind::Subscription,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// Builds the image the tests use in place of a profile image.
///
/// It is the profile's own recipe with the base image swapped for alpine, and a home directory
/// of its own, because `qcode/base` is what decides where `$HOME` is and this test cannot build
/// it. Everything the recipe does with `$HOME` is therefore really exercised.
fn stand_in_image(engine: &Engine, folder: &Path) -> String {
    let profile = profile();
    let recipe = recipe::image(&profile);
    let home = "/tmp/qcode-home";
    let containerfile = recipe
        .containerfile
        .replace(
            &format!("FROM {}", crate::engine::names::BASE_IMAGE),
            &format!("FROM {ALPINE}\nENV HOME={home}\nRUN mkdir -p {home} && chmod 0777 {home}"),
        )
        // The harness itself is not installed: npm is not in alpine, and what is under test is
        // everything the recipe does around the install.
        .replace("RUN npm install -g @anthropic-ai/claude-code", "RUN true")
        // The step that opens the home directory is a program of the base image; alpine has
        // only the shell command it stands for.
        .replace(&format!("RUN {OPEN_HOME}"), &format!("RUN chmod -R a+rwX {home}"));
    std::fs::write(folder.join("Containerfile"), &containerfile).expect("a Containerfile");
    for (path, contents) in &recipe.files {
        let target = folder.join(path);
        std::fs::create_dir_all(target.parent().expect("the file has a folder")).expect("a folder");
        std::fs::write(&target, contents).expect("a template file");
    }
    let image = profile.image();
    let _ = capture(&engine.remove_image(&image));
    build_image(
        engine,
        &ImageBuild { image: &image, containerfile: &folder.join("Containerfile"), context: folder },
        &|| false,
        &mut |_| {},
    )
    .expect("the stand-in profile image builds");
    image
}

/// Runs a shell script in a container without a terminal and answers whether it succeeded.
fn in_container(engine: &Engine, container: &str, script: &str) -> bool {
    capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] })).is_ok()
}

/// A folder of this test's own.
fn scratch(name: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let path = std::env::temp_dir().join(format!("qcode-ui-live-{name}-{stamp}"));
    std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
    path
}

/// Takes away everything a run of these tests makes.
fn clear(engine: &Engine, image: &str) {
    let name = SafeName::parse(PROFILE).expect("the name is safe");
    let _ = capture(&engine.remove_volume(&names::credential_volume(name.as_str())));
    let _ = capture(&engine.remove_image(image));
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_login_that_really_happened_leaves_the_container_and_reaches_the_volume() {
    for engine in engines() {
        let folder = scratch("stored");
        let image = stand_in_image(&engine, &folder);
        let profile = profile();
        let volume = names::credential_volume(profile.name.as_str());
        let _ = capture(&engine.remove_volume(&volume));

        let container = work::open_login(&engine, &profile).expect("the login container opens");
        // A shell stands in for the harness and writes exactly the file the record names.
        let login = profile.harness.record().identity[0];
        let wrote = in_container(
            &engine,
            &container.name,
            &format!("mkdir -p \"$HOME/$(dirname '{login}')\" && printf '{{\"token\":\"live\"}}' > \"$HOME/{login}\""),
        );
        assert!(wrote, "{:?}: the stand-in harness wrote its login", engine.kind());

        let stored = work::store_login(&engine, &profile, &container).expect("the login is found and stored");
        assert_eq!(stored, 1, "{:?}", engine.kind());
        work::close_login(&engine, &container);

        let listing = capture(&engine.list_volumes()).expect("a listing");
        assert!(listing.lines().any(|line| line.trim() == volume), "{:?}: {listing}", engine.kind());
        let answers = work::probe(&engine, std::slice::from_ref(&profile));
        assert_eq!(answers[0].image, Readiness::Present, "{:?}", engine.kind());
        assert_eq!(answers[0].identity, Readiness::Present, "{:?}", engine.kind());

        work::sign_out(&engine, &profile.name).expect("signing out removes the volume");
        let answers = work::probe(&engine, std::slice::from_ref(&profile));
        assert_eq!(answers[0].identity, Readiness::Missing, "{:?}", engine.kind());

        clear(&engine, &image);
        let _ = std::fs::remove_dir_all(&folder);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_login_that_never_happened_stores_nothing_and_makes_no_volume() {
    for engine in engines() {
        let folder = scratch("missing");
        let image = stand_in_image(&engine, &folder);
        let profile = profile();
        let volume = names::credential_volume(profile.name.as_str());
        let _ = capture(&engine.remove_volume(&volume));

        let container = work::open_login(&engine, &profile).expect("the login container opens");
        // The harness stands in and does not sign in; nothing may be taken on trust.
        let problem = work::store_login(&engine, &profile, &container).expect_err("there is no login to store");
        assert_eq!(problem, Problem::NoLogin, "{:?}", engine.kind());
        work::close_login(&engine, &container);

        let listing = capture(&engine.list_volumes()).expect("a listing");
        assert!(!listing.lines().any(|line| line.trim() == volume), "{:?}: {listing}", engine.kind());
        let answers = work::probe(&engine, std::slice::from_ref(&profile));
        assert_eq!(answers[0].identity, Readiness::Missing, "{:?}", engine.kind());

        clear(&engine, &image);
        let _ = std::fs::remove_dir_all(&folder);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_template_lands_in_the_home_directory_of_the_image_it_is_built_into() {
    for engine in engines() {
        let folder = scratch("template");
        let image = stand_in_image(&engine, &folder);
        let profile = profile();
        let container = work::open_login(&engine, &profile).expect("the login container opens");
        let file = Template::Recommended.files(profile.harness).into_iter().next().expect("claude-code has settings");
        let found =
            in_container(&engine, &container.name, &format!("grep -q bypassPermissions \"$HOME/{}\"", file.path));
        assert!(found, "{:?}: the template reached the home directory", engine.kind());
        work::close_login(&engine, &container);
        clear(&engine, &image);
        let _ = std::fs::remove_dir_all(&folder);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_login_container_is_removed_even_when_the_login_is_interrupted() {
    for engine in engines() {
        let folder = scratch("interrupted");
        let image = stand_in_image(&engine, &folder);
        let profile = profile();
        let container = work::open_login(&engine, &profile).expect("the login container opens");
        let name = container.name.clone();
        let capture_dir = container.capture.clone();
        work::close_login(&engine, &container);
        capture(&engine.container_state(&name)).expect_err("the container is gone");
        assert!(!capture_dir.exists(), "{:?}: the directory it used is gone", engine.kind());
        clear(&engine, &image);
        let _ = std::fs::remove_dir_all(&folder);
    }
}

/// The whole promise of Rebuild image, on every engine: building again from a recipe whose text
/// did not change still runs every step (a harness installed by a step that reads the same would
/// otherwise never move on), a changed recipe reaches the image, the label says which recipe it
/// was, the image it replaced goes when nothing is made from it, and a rebuild that fails leaves
/// the image that was there. The recipe is a small one of the product's own shape, built by the
/// product's own function, so the test takes seconds rather than a harness download.
#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_rebuild_runs_every_step_again_takes_a_changed_recipe_and_keeps_the_image_when_it_fails() {
    const IMAGE: &str = "qcode/profile/rebuildlive";
    let recipe = |marker: &str| recipe::Recipe {
        containerfile: format!(
            "FROM {ALPINE}\nRUN echo {marker} > /marker\nRUN cat /proc/sys/kernel/random/uuid > /stamp\n"
        ),
        files: Vec::new(),
    };
    for engine in engines() {
        let kind = engine.kind();
        let _ = capture(&engine.remove_image(IMAGE));
        let read = |path: &str| {
            let command = engine.run_once(&crate::engine::RunOnce {
                image: IMAGE,
                mounts: &[],
                network: crate::engine::Network::None,
                user: crate::engine::HostUser::current().expect("the current user"),
                workdir: None,
                command: &["cat", path],
            });
            capture(&command).unwrap_or_else(|error| panic!("{kind:?}: {path} is read: {error:?}")).trim().to_owned()
        };
        let id = || capture(&engine.image_exists(IMAGE)).expect("the image is there").trim().to_owned();
        let label = || capture(&engine.image_label(IMAGE, recipe::REVISION_LABEL)).expect("asked").trim().to_owned();

        let one = recipe("one");
        work::rebuild_from(&engine, IMAGE, &one, &|| false, &mut |_| {}).expect("the first build");
        let (first, stamp) = (id(), read("/stamp"));
        assert_eq!(label(), one.revision(), "{kind:?}: the recipe is written on the image");

        work::rebuild_from(&engine, IMAGE, &one, &|| false, &mut |_| {}).expect("the same recipe again");
        assert_ne!(read("/stamp"), stamp, "{kind:?}: every step ran again, none was taken from before");
        let second = id();
        assert_ne!(second, first, "{kind:?}");
        assert!(capture(&engine.image_exists(&first)).is_err(), "{kind:?}: the replaced image is gone");

        let two = recipe("two");
        work::rebuild_from(&engine, IMAGE, &two, &|| false, &mut |_| {}).expect("a changed recipe");
        assert_eq!(read("/marker"), "two", "{kind:?}: the change is in the image");
        assert_eq!(label(), two.revision(), "{kind:?}");
        let third = id();

        let broken = recipe::Recipe { containerfile: format!("FROM {ALPINE}\nRUN false\n"), files: Vec::new() };
        let failed = work::rebuild_from(&engine, IMAGE, &broken, &|| false, &mut |_| {});
        assert!(matches!(failed, Err(Problem::Refused(_))), "{kind:?}: {failed:?}");
        let kept = capture(&engine.image_exists(IMAGE)).map(|id| id.trim().to_owned());
        let marker = kept.is_ok().then(|| read("/marker"));
        let _ = capture(&engine.remove_image(IMAGE));
        assert_eq!(kept.ok().as_deref(), Some(third.as_str()), "{kind:?}: the image that was there is kept");
        assert_eq!(marker.as_deref(), Some("two"), "{kind:?}");
    }
}
