//! Noting the containers the screen starts, so that they can be stopped once no QCode is open.
//!
//! The engine of these tests cannot be run, so no container ever comes up here; noting is checked
//! where it is decided — what a started container's message becomes — and the screen is checked
//! for what it does with that message.

use super::*;

use crate::store::Registry;
use crate::ui::workspace::{Noting, noted};

/// Whether `message` is the plain message a start hands on, with nothing wrapped around it.
fn is_plain(message: &Msg) -> bool {
    matches!(message, Msg::RefreshContainers)
}

#[test]
fn a_started_container_is_noted_once_and_its_message_goes_on_unchanged() {
    let scratch = Scratch::new("noted");
    let list = scratch.0.join("data").join("containers.toml");
    for _ in 0..2 {
        let message = noted(Some(&list), EngineKind::Podman, "qcode-firefly-base", Msg::RefreshContainers);
        assert!(is_plain(&message), "{message:?}");
    }
    let read = Registry::load(&list);
    assert!(read.is_clean(), "{:?}", read.diagnostics);
    let names: Vec<&str> = read.value.containers.iter().map(|known| known.name.as_str()).collect();
    assert_eq!(names, ["qcode-firefly-base"]);
}

#[test]
fn a_screen_with_no_list_notes_nothing() {
    let message = noted(None, EngineKind::Podman, "qcode-firefly-base", Msg::RefreshContainers);
    assert!(is_plain(&message), "{message:?}");
}

#[test]
fn a_damaged_list_is_mended_and_said() {
    let scratch = Scratch::new("damaged");
    let list = scratch.0.join("containers.toml");
    fs::write(&list, "[[container]]\nname = \n").expect("a damaged list");
    let message = noted(Some(&list), EngineKind::Docker, "qcode-firefly-base", Msg::RefreshContainers);
    let Msg::Noted(Noting::Repaired(place), inner) = message else { panic!("the damage is carried along") };
    assert!(place.starts_with("containers.toml:2:"), "{place}");
    assert!(is_plain(&inner));
    assert!(Registry::load(&list).is_clean(), "the list is whole again");
}

#[test]
fn a_list_that_cannot_be_written_is_said_with_the_machines_words() {
    let scratch = Scratch::new("unwritable");
    // A file where the list's folder should be: nothing can be written below it.
    let blocker = scratch.0.join("data");
    fs::write(&blocker, "").expect("a file in the way");
    let list = blocker.join("containers.toml");
    let message = noted(Some(&list), EngineKind::Podman, "qcode-firefly-base", Msg::RefreshContainers);
    let Msg::Noted(Noting::Unwritten(reason), _) = message else { panic!("the failure is carried along") };
    assert!(reason.contains("containers.toml"), "{reason}");
}

#[test]
fn a_problem_with_the_list_is_said_once_and_the_start_still_lands() {
    let scratch = Scratch::new("told");
    let mut harness = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    let trouble = || Noting::Unwritten("No space left on device".to_owned());
    harness.send(Msg::Noted(trouble(), Box::new(Msg::TogglePanel(true))));
    harness.advance(Duration::from_millis(300));
    assert!(harness.screen().contains("could not note a container"), "{}", harness.screen());
    assert!(harness.app().0.panel().is_open(), "the message it came with is applied");
    assert!(harness.app().0.registry_told);
    harness.send(Msg::Noted(trouble(), Box::new(Msg::TogglePanel(false))));
    assert!(!harness.app().0.panel().is_open(), "later ones are applied without being said again");
}

#[test]
fn a_container_that_does_not_come_up_is_not_noted() {
    let scratch = Scratch::new("down");
    let list = scratch.0.join("data").join("containers.toml");
    let screen = one_workspace(&scratch).with_registry(Some(list.clone()));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    open_in(&mut harness, Choice::Shell);
    harness.render();
    assert!(!list.exists(), "the engine could not be run, so nothing started and nothing is listed");
}
