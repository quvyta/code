//! The file tree following the disk: what another program changes in a folder on screen is read
//! again without a refresh, only there, and folders that leave the screen are let go.
//!
//! A harness runs background work in line, where the watch's wait for the disk would never end,
//! so these tests drive the screen's messages themselves and take each batch of changes by hand,
//! the way the waiting thread would hand it over.

use std::fs;

use qframe::storage::{FolderChange, FolderChangeKind};

use super::super::files::{self, ROOT};
use super::super::watch::{Live, Watching};
use super::super::{FileMsg, Msg, ProjectScreen};
use super::{HarnessKind, Scratch, apply, one_project, profile, project};

/// The id of the project `one_project` opens.
const ID: &str = "firefly";

/// A screen that follows the disk, entered, with the project folder read.
fn live_screen(scratch: &Scratch) -> ProjectScreen {
    let mut screen = one_project(scratch).watching(true);
    drop(super::super::opened(&mut screen));
    read(&mut screen, ROOT);
    screen
}

/// Answers the reading of the folder `key` the way its background work would.
fn read(screen: &mut ProjectScreen, key: &str) {
    let path = screen.project().expect("a project").files().path(key);
    apply(screen, Msg::FolderRead(ID.to_owned(), key.to_owned(), files::read_folder(&path)));
}

/// The open project's watch.
fn watching(screen: &ProjectScreen) -> &Watching {
    match &screen.projects[screen.active].live {
        Live::On(watching) => watching,
        other => panic!("the open project is watched, not {other:?}"),
    }
}

/// Waits for changes until a batch has one that `wanted` picks, handing every batch to the
/// screen; the system may split one burst over two batches.
fn changes_until(screen: &mut ProjectScreen, wanted: impl Fn(&FolderChange) -> bool) {
    for _ in 0..5 {
        let (run, changes) = (watching(screen).run(), watching(screen).changes());
        let batch = changes.next();
        let done = batch.iter().any(&wanted);
        apply(screen, Msg::FilesChanged(ID.to_owned(), run, batch));
        if done {
            return;
        }
    }
    panic!("the change never came");
}

fn loading(screen: &ProjectScreen, key: &str) -> bool {
    screen.project().expect("a project").files().is_loading(key)
}

fn project_dir(scratch: &Scratch) -> std::path::PathBuf {
    scratch.0.join("Project")
}

#[test]
fn a_file_made_by_another_program_is_read_in_its_own_folder_only() {
    let scratch = Scratch::new("watch-made");
    let mut screen = live_screen(&scratch);
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), true));
    read(&mut screen, "src");

    fs::write(project_dir(&scratch).join("src/late.rs"), "").expect("a file made elsewhere");
    changes_until(&mut screen, |change| change.kind == FolderChangeKind::Created);
    assert!(loading(&screen, "src"), "the folder it appeared in is read again");
    assert!(!loading(&screen, ROOT), "and nothing else");

    read(&mut screen, "src");
    let entries = screen.project().expect("a project").files().children("src").expect("read").to_vec();
    assert!(entries.iter().any(|entry| entry.name == "late.rs"), "{entries:?}");
}

#[test]
fn only_the_folders_on_screen_are_watched() {
    let scratch = Scratch::new("watch-open");
    let mut screen = live_screen(&scratch);
    assert_eq!(watching(&screen).watched(), [ROOT]);
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), true));
    assert_eq!(watching(&screen).watched(), [ROOT, "src"], "an opened folder is watched");
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), false));
    assert_eq!(watching(&screen).watched(), [ROOT], "a closed one is let go");

    apply(&mut screen, Msg::ExpandFile("src".to_owned(), true));
    apply(&mut screen, Msg::ExpandFile(ROOT.to_owned(), false));
    assert!(watching(&screen).watched().is_empty(), "a folder inside a closed one is off screen too");
}

#[test]
fn a_folder_back_on_screen_is_read_again() {
    let scratch = Scratch::new("watch-back");
    let mut screen = live_screen(&scratch);
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), true));
    read(&mut screen, "src");
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), false));
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), true));
    assert!(loading(&screen, "src"), "what changed while it was not watched went unseen, so it is read");
}

#[test]
fn a_change_of_content_reads_nothing() {
    let scratch = Scratch::new("watch-content");
    let mut screen = live_screen(&scratch);
    fs::write(project_dir(&scratch).join("README.md"), "changed\n").expect("a file written again");
    changes_until(&mut screen, |change| change.kind == FolderChangeKind::Modified);
    assert!(!loading(&screen, ROOT), "the tree shows names, not what is in the files");
}

#[test]
fn a_watched_folder_that_goes_away_reads_everything_on_screen_again() {
    let scratch = Scratch::new("watch-gone");
    let mut screen = live_screen(&scratch);
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), true));
    read(&mut screen, "src");
    apply(&mut screen, Msg::Files(FileMsg::Cut("src/main.rs".to_owned())));

    fs::remove_dir_all(project_dir(&scratch).join("src")).expect("removed by another program");
    changes_until(&mut screen, |change| change.kind == FolderChangeKind::Gone);
    assert!(loading(&screen, ROOT), "everything on screen is read again");
    read(&mut screen, ROOT);
    let files = screen.project().expect("a project").files();
    assert!(!files.is_open("src") && files.cut().is_empty(), "nothing is left of the folder that went");
    assert_eq!(watching(&screen).watched(), [ROOT]);
}

#[test]
fn an_overflow_reads_every_folder_on_screen_again() {
    let scratch = Scratch::new("watch-overflow");
    let mut screen = live_screen(&scratch);
    apply(&mut screen, Msg::ExpandFile("src".to_owned(), true));
    read(&mut screen, "src");
    let overflow =
        FolderChange { folder: project_dir(&scratch).join("src"), name: None, kind: FolderChangeKind::Overflow };
    let run = watching(&screen).run();
    apply(&mut screen, Msg::FilesChanged(ID.to_owned(), run, vec![overflow]));
    assert!(loading(&screen, ROOT) && loading(&screen, "src"));
}

#[test]
fn a_batch_of_a_watch_that_was_let_go_is_ignored() {
    let scratch = Scratch::new("watch-stale");
    let mut screen = live_screen(&scratch);
    let created =
        FolderChange { folder: project_dir(&scratch), name: Some("late.rs".into()), kind: FolderChangeKind::Created };
    let run = watching(&screen).run();
    apply(&mut screen, Msg::FilesChanged(ID.to_owned(), run + 1, vec![created]));
    assert!(!loading(&screen, ROOT));
}

#[test]
fn only_the_open_project_is_watched() {
    let scratch = Scratch::new("watch-first");
    let other = Scratch::new("watch-second");
    let projects = vec![
        project(ID, "Firefly", scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)]),
        project("serenity", "Serenity", other.paths(), Vec::new()),
    ];
    let engine = super::engine();
    let mut screen = ProjectScreen::new(Some(engine), crate::engine::HostUser::ImageDefault, projects).watching(true);
    drop(super::super::opened(&mut screen));
    assert!(matches!(screen.projects[0].live, Live::On(_)));
    assert!(matches!(screen.projects[1].live, Live::Off));
    apply(&mut screen, Msg::OpenProject(1));
    assert!(matches!(screen.projects[0].live, Live::Off), "the project left behind lets its watch go");
    assert!(matches!(screen.projects[1].live, Live::On(_)));
}

#[test]
fn a_screen_that_does_not_follow_the_disk_watches_nothing() {
    let scratch = Scratch::new("watch-off");
    let mut screen = one_project(&scratch);
    drop(super::super::opened(&mut screen));
    assert!(matches!(screen.projects[0].live, Live::Off));
}
