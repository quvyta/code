//! The file tree following the disk: what another program makes, removes or changes in the
//! workspace folder shows on screen without anyone asking, in the workspace that is open.
//!
//! The screen follows with each wait for the disk bounded, so the harness, which runs background
//! work in line, always gets its turn back; each test steps it until the screen shows the change,
//! for a generous but finite while. How the watch is kept (which folders, which batches) is the
//! framework's file manager's own and is tested there.

use std::fs;
use std::path::Path;

use qframe::event::{MouseButton, MouseKind};

use super::super::{Msg, WorkspaceScreen, escape};
use super::{Harness, HarnessKind, SIZE, Scratch, Screen, engine, harness, profile, workspace};

/// How long one wait for the disk may last.
const BOUND: std::time::Duration = std::time::Duration::from_millis(50);

/// One step of the harness while a change is waited for.
const STEP: std::time::Duration = std::time::Duration::from_millis(100);

/// How many steps a change may take to show: far more than it needs, and still an end.
const STEPS: usize = 150;

/// A screen that follows the disk, entered, with the workspace folder read.
fn live(scratch: &Scratch) -> Harness<Screen> {
    let workspaces =
        vec![workspace("firefly", "Firefly", scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)])];
    let screen =
        WorkspaceScreen::new(Some(engine()), crate::engine::HostUser::ImageDefault, workspaces).watching_within(BOUND);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    harness
}

/// Steps the harness until `shown` says the screen is as it should be, and answers whether it
/// came to that.
fn until(harness: &mut Harness<Screen>, shown: impl Fn(&str) -> bool) -> bool {
    for _ in 0..STEPS {
        if shown(&harness.screen()) {
            return true;
        }
        harness.advance(STEP);
    }
    shown(&harness.screen())
}

fn workspace_dir(scratch: &Scratch) -> std::path::PathBuf {
    scratch.0.join("Work")
}

/// Makes an empty file at `path`, the way another program would.
fn make(path: &Path) {
    fs::write(path, "").expect("a file made by another program");
}

#[test]
fn a_file_another_program_makes_shows_in_the_folder_it_was_made_in() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let scratch = Scratch::new("watch-made");
    let mut harness = live(&scratch);
    harness.click_text("src").advance(STEP);
    assert!(harness.screen().contains("main.rs"), "{}", harness.screen());

    make(&workspace_dir(&scratch).join("src/late.rs"));
    assert!(until(&mut harness, |screen| screen.contains("late.rs")), "it shows by itself:\n{}", harness.screen());
    make(&workspace_dir(&scratch).join("GUIDE.md"));
    assert!(until(&mut harness, |screen| screen.contains("GUIDE.md")), "at the top too:\n{}", harness.screen());
}

#[test]
fn what_changed_in_a_closed_folder_shows_when_it_is_opened_again() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let scratch = Scratch::new("watch-back");
    let mut harness = live(&scratch);
    harness.click_text("src").advance(STEP);
    harness.click_text("src").advance(STEP);
    assert!(!harness.screen().contains("main.rs"), "closed:\n{}", harness.screen());
    make(&workspace_dir(&scratch).join("src/shut.rs"));
    harness.advance(STEP);
    harness.click_text("src").advance(STEP);
    assert!(
        until(&mut harness, |screen| screen.contains("shut.rs")),
        "what happened while it was off screen is read:\n{}",
        harness.screen()
    );
}

#[test]
fn a_folder_another_program_removes_leaves_nothing_behind() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let scratch = Scratch::new("watch-gone");
    let mut harness = live(&scratch);
    harness.click_text("src").advance(STEP);
    let (x, y) = harness.find("main.rs").expect("the file's row");
    harness.mouse(MouseKind::Down(MouseButton::Right), x, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x, y);
    harness.advance(STEP);
    harness.click_text("Cut").advance(STEP);
    assert!(escape(&harness.app().0).is_some(), "something waits to be pasted");

    fs::remove_dir_all(workspace_dir(&scratch).join("src")).expect("removed by another program");
    assert!(until(&mut harness, |screen| !screen.contains("src")), "the folder goes:\n{}", harness.screen());
    assert!(escape(&harness.app().0).is_none(), "and nothing of it waits to be pasted");

    fs::create_dir_all(workspace_dir(&scratch).join("src")).expect("a new folder of the same name");
    make(&workspace_dir(&scratch).join("src/new.rs"));
    assert!(until(&mut harness, |screen| screen.contains("src")), "the new one comes:\n{}", harness.screen());
    harness.advance(STEP);
    assert!(!harness.screen().contains("new.rs"), "closed, as a new folder starts:\n{}", harness.screen());
}

#[test]
fn the_workspace_that_is_open_is_the_one_followed() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let first = Scratch::new("watch-first");
    let second = Scratch::new("watch-second");
    let workspaces = vec![
        workspace("firefly", "Firefly", first.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)]),
        workspace("serenity", "Serenity", second.paths(), Vec::new()),
    ];
    let screen =
        WorkspaceScreen::new(Some(engine()), crate::engine::HostUser::ImageDefault, workspaces).watching_within(BOUND);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();

    harness.send(Msg::OpenWorkspace(1));
    make(&workspace_dir(&second).join("SECOND.md"));
    assert!(until(&mut harness, |screen| screen.contains("SECOND.md")), "{}", harness.screen());

    make(&workspace_dir(&first).join("FIRST.md"));
    harness.send(Msg::OpenWorkspace(0));
    assert!(until(&mut harness, |screen| screen.contains("FIRST.md")), "{}", harness.screen());
    make(&workspace_dir(&first).join("AGAIN.md"));
    assert!(until(&mut harness, |screen| screen.contains("AGAIN.md")), "followed again:\n{}", harness.screen());
}

#[test]
fn a_screen_that_follows_the_disk_has_only_the_open_workspace_follow_it() {
    let first = Scratch::new("watch-live-first");
    let second = Scratch::new("watch-live-second");
    let workspaces = vec![
        workspace("firefly", "Firefly", first.paths(), Vec::new()),
        workspace("serenity", "Serenity", second.paths(), Vec::new()),
    ];
    // The work is dropped rather than run: a wait without a bound would never come back here.
    let mut screen =
        WorkspaceScreen::new(Some(engine()), crate::engine::HostUser::ImageDefault, workspaces).watching(true);
    drop(super::super::opened(&mut screen));
    let follows = |screen: &WorkspaceScreen| -> Vec<bool> {
        screen.workspaces.iter().map(|workspace| workspace.files().follows_changes()).collect()
    };
    assert_eq!(follows(&screen), [true, false]);
    super::apply(&mut screen, Msg::OpenWorkspace(1));
    assert_eq!(follows(&screen), [false, true], "the workspace left behind lets its watch go");

    let still = Scratch::new("watch-still");
    let mut screen = super::one_workspace(&still);
    drop(super::super::opened(&mut screen));
    assert_eq!(follows(&screen), [false], "a screen that does not follow the disk watches nothing");
}
