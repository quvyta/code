//! A profile's tab whose image the engine does not have asks before anything is made: it says
//! which image is missing where, offers to build it and opens once it is built, or closes if the
//! person would rather not. Nothing is created from an image that is not there.
//!
//! The engine is a script that writes down every call and has no image until something is built,
//! so the whole path is walked from the page's rows and buttons and read back from the screen and
//! from the calls.

use super::*;

use crate::ui::workspace::plan::LaunchFailure;

/// A stand-in podman that knows no container and no volume, has no image until a build has run,
/// and writes down every call it is given.
struct Imageless {
    scratch: Scratch,
    binary: PathBuf,
    calls: PathBuf,
}

impl Imageless {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let (binary, calls, built) = (scratch.0.join("engine"), scratch.0.join("calls"), scratch.0.join("built"));
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {calls}\n\
             for last; do :; done\n\
             case \"$1 $2\" in\n\
             'container inspect') echo \"Error: no such container $last\" >&2; exit 125 ;;\n\
             'image inspect') [ -e {built} ] && {{ echo sha256:0123; exit 0; }}; echo \"Error: $last: image not known\" >&2; exit 125 ;;\n\
             'volume inspect') echo \"Error: no such volume $last\" >&2; exit 125 ;;\n\
             esac\n\
             [ \"$1\" = build ] && {{ echo \"STEP 1/2: FROM qcode/base\"; echo \"COMMIT $3\"; touch {built}; exit 0; }}\n\
             exit 0\n",
            calls = calls.display(),
            built = built.display(),
        );
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        Self { scratch, binary, calls }
    }

    fn screen(&self) -> WorkspaceScreen {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        )];
        WorkspaceScreen::new(
            Some(Engine::new(EngineKind::Podman, &self.binary)),
            HostUser::Ids { uid: 1000, gid: 1000 },
            workspaces,
        )
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }
}

/// Opens a new chat of the workspace's profile the way the person does: the `+`, then its row.
fn open_chat(harness: &mut Harness<Screen>) {
    harness.click_text("New tab").render();
    harness.click_text("New chat").render();
}

#[test]
fn a_missing_image_is_offered_to_be_built_and_nothing_is_created_from_it() {
    let engine = Imageless::new("image-offered");
    let mut harness = harness(engine.screen(), 120, 36);
    open_chat(&mut harness);
    let screen = harness.screen();
    assert!(screen.contains("The image of claude-sub is not in Podman."), "{screen}");
    assert!(screen.contains("Build it now"), "{screen}");
    assert!(screen.contains("Cancel"), "{screen}");
    assert!(!screen.contains("The engine refused"), "{screen}");
    let calls = engine.calls();
    assert!(calls.iter().any(|call| call.starts_with("image inspect") && call.ends_with("qcode/profile/claude-sub")));
    assert!(!calls.iter().any(|call| call.starts_with("create")), "nothing is made from a missing image: {calls:#?}");
}

#[test]
fn building_it_now_builds_the_profile_and_then_opens_the_tab() {
    let engine = Imageless::new("image-built");
    let mut harness = harness(engine.screen(), 120, 36);
    open_chat(&mut harness);
    harness.click_text("Build it now").render();
    let calls = engine.calls();
    let built = calls.iter().position(|call| call.starts_with("build --tag qcode/profile/claude-sub"));
    let created = calls.iter().position(|call| call.starts_with("create --name qcode-firefly-claude-sub"));
    let (Some(built), Some(created)) = (built, created) else { panic!("built, then made: {calls:#?}") };
    assert!(built < created, "the image is built before the container is made from it: {calls:#?}");
    let state = harness.app().0.workspace().expect("a workspace").tabs()[0].state().clone();
    assert!(
        !matches!(state, TabState::NoImage | TabState::Building(_) | TabState::Failed(_)),
        "the tab went on to open: {state:?}\n{}",
        harness.screen()
    );
}

#[test]
fn cancelling_closes_the_tab_without_building_anything() {
    let engine = Imageless::new("image-cancelled");
    let mut harness = harness(engine.screen(), 120, 36);
    open_chat(&mut harness);
    harness.click_text("Cancel").render();
    assert!(harness.app().0.workspace().expect("a workspace").tabs().is_empty(), "{}", harness.screen());
    assert!(!engine.calls().iter().any(|call| call.starts_with("build")), "{:#?}", engine.calls());
}

#[test]
fn the_build_is_shown_as_it_speaks_and_can_be_stopped() {
    let scratch = Scratch::new("image-log");
    let mut screen = one_workspace(&scratch);
    open(&mut screen, claude());
    let tab = key(&screen, 0);
    let missing = LaunchFailure { command: String::new(), output: String::new(), image_missing: true };
    apply(&mut screen, Msg::Ready(tab, 0, Err(missing)));
    apply(&mut screen, Msg::BuildImage(tab));
    apply(&mut screen, Msg::ImageLine(tab, "STEP 4/9: RUN npm install -g @anthropic-ai/claude-code".to_owned()));
    let mut harness = harness(screen, 120, 36);
    let screen = harness.screen();
    assert!(screen.contains("Building the image of claude-sub"), "{screen}");
    assert!(screen.contains("STEP 4/9"), "the build's own lines are in the tab:\n{screen}");
    harness.click_text("Stop").render();
    assert_eq!(harness.app().0.workspace().expect("a workspace").tabs()[0].state(), &TabState::NoImage);
    assert!(harness.screen().contains("Build it now"), "{}", harness.screen());
}

/// The profile of the open workspace, as the tab would build it.
fn workspace_profile(harness: &Harness<Screen>) -> Profile {
    harness.app().0.workspace().expect("a workspace").profiles[0].clone()
}

/// The engines of this machine when `QCODE_CONTAINER_TESTS=1`, and none otherwise.
fn live_engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    [EngineKind::Podman, EngineKind::Docker].into_iter().filter_map(|kind| crate::engine::detect(kind).ok()).collect()
}

/// On every engine of the machine: a profile nobody has, whose image is not there, opened from the
/// page of a workspace nobody has, offers the build; the build is the product's own, and the tab
/// comes up in a container made from what it built. Everything it made is removed afterwards,
/// image included, so the next run meets a missing image again.
///
/// ```text
/// QCODE_CONTAINER_TESTS=1 cargo test missing_image_live -- --ignored --test-threads=1
/// ```
#[test]
#[ignore = "builds a real profile image; run with QCODE_CONTAINER_TESTS=1"]
fn missing_image_live_is_built_from_the_tab_and_the_tab_opens() {
    use crate::engine::names;
    use crate::engine::run::capture;
    for engine in live_engines() {
        let scratch = Scratch::new("motorlive");
        let mut chosen = profile("motortest", HarnessKind::ClaudeCode);
        chosen.template = Template::Base;
        let image = chosen.image();
        let container = names::profile_container("motorlive", "motortest");
        let volume = names::home_volume("motorlive", "motortest");
        let clear = || {
            let _ = capture(&engine.remove_container(&container));
            let _ = capture(&engine.remove_volume(&volume));
            let _ = capture(&engine.remove_image(&image));
        };
        clear();
        let workspaces = vec![workspace("motorlive", "Motor", scratch.paths(), vec![chosen])];
        let screen = WorkspaceScreen::new(Some(engine.clone()), HostUser::current().expect("ids"), workspaces);
        let mut harness = harness(screen, 120, 36);
        open_chat(&mut harness);
        let asked = harness.screen();
        // The harness gives a background task ten seconds before it calls it stuck, and a real
        // build takes minutes, so the button's own build runs here, in the test's thread: the
        // same function, the same profile, the same engine. That the button starts it, and that a
        // finished build starts the tab again, is what the stand-in engine's tests above show.
        let tab = harness.app().0.workspace().expect("a workspace").tabs()[0].key();
        let built = crate::ui::profiles::work::build_whole(
            &engine,
            &workspace_profile(&harness),
            &|| false,
            &mut || {},
            &mut |_| {},
        );
        assert!(built.is_ok(), "{:?}: {built:?}", engine.kind());
        // What a finished build does next: the tab starts again, from its container upwards.
        harness.send(Msg::Restart(tab)).render();
        let state = harness.app().0.workspace().expect("a workspace").tabs()[0].state().clone();
        let running = capture(&engine.container_state(&container)).unwrap_or_default();
        let made_from = capture(&engine.container_label(&container, "qcode.plan")).is_ok();
        clear();
        assert!(asked.contains("The image of motortest is not in"), "{:?}:\n{asked}", engine.kind());
        assert!(
            matches!(state, TabState::Running | TabState::Ended { .. }),
            "{:?}: the tab opened: {state:?}\n{}",
            engine.kind(),
            harness.screen()
        );
        assert_eq!(running.trim(), "running", "{:?}: its container is up", engine.kind());
        assert!(made_from, "{:?}: the container is QCode's own", engine.kind());
    }
}
