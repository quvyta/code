//! A tab the engine refuses for a reason QCode recognises says what happened and what to do in the
//! person's words, with the engine's own words kept underneath; any other refusal keeps showing
//! the engine's words alone.
//!
//! The engine is a script that answers every call with one refusal, so the tab is opened the way
//! the person opens it and what is read back is the screen.

use super::*;

/// A screen whose engine of `kind` answers every call with `said` and fails.
fn refusing(scratch: &Scratch, kind: EngineKind, said: &str) -> WorkspaceScreen {
    let binary = scratch.0.join("engine");
    fs::write(&binary, format!("#!/bin/sh\nprintf '%s\\n' '{said}' >&2\nexit 1\n")).expect("the stand-in engine");
    fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
    let workspaces = vec![workspace("firefly", "Firefly", scratch.paths(), Vec::new())];
    WorkspaceScreen::new(Some(Engine::new(kind, &binary)), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces)
}

/// Opens a shell tab the way the person does: the `+`, then the shell's row on the page.
fn open_shell(harness: &mut Harness<Screen>) -> String {
    harness.click_text("New tab").render();
    harness.click_text("in the workspace's own container").render();
    harness.screen()
}

#[test]
fn a_socket_the_account_may_not_open_is_said_as_the_docker_group_with_the_line_that_adds_it() {
    let scratch = Scratch::new("known-permission");
    let said = "permission denied while trying to connect to the docker API at unix:///var/run/docker.sock";
    let mut harness = harness(refusing(&scratch, EngineKind::Docker, said), 140, 36);
    let screen = open_shell(&mut harness);
    assert!(screen.contains("Your account may not use Docker yet"), "{screen}");
    assert!(screen.contains("sudo usermod -aG docker $USER"), "{screen}");
    assert!(!screen.contains("The engine refused"), "the plain sentence stands in its place:\n{screen}");
    // The engine's own words stay, under the sentence, for whoever wants them.
    assert!(screen.contains("What the engine said"), "{screen}");
    assert!(screen.contains("permission denied while trying"), "{screen}");
    assert!(screen.contains("Start again"), "{screen}");
}

#[test]
fn an_image_the_engine_does_not_have_is_said_as_one_built_with_the_other_engine() {
    let scratch = Scratch::new("known-image");
    let said =
        "Error: short-name \"qcode/base\" did not resolve to an alias and no containers-registries.conf(5) was found";
    let mut harness = harness(refusing(&scratch, EngineKind::Podman, said), 140, 36);
    let screen = open_shell(&mut harness);
    assert!(screen.contains("Podman does not have the image this needs"), "{screen}");
    assert!(screen.contains("short-name"), "the engine's words are kept below:\n{screen}");
}

#[test]
fn a_refusal_qcode_does_not_recognise_keeps_the_engines_own_words_alone() {
    let scratch = Scratch::new("known-unknown");
    let mut harness = harness(refusing(&scratch, EngineKind::Podman, "Error: no space left on device"), 140, 36);
    let screen = open_shell(&mut harness);
    assert!(screen.contains("The engine refused"), "{screen}");
    assert!(screen.contains("no space left on device"), "{screen}");
    assert!(!screen.contains("What the engine said"), "{screen}");
}
