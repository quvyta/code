//! Following the workspace folder while other programs change it: a harness in a container writes
//! files through the workspace mount, and the tree shows them without being asked.
//!
//! The system tells when an entry of a watched folder appears, goes or is renamed, so nothing is
//! polled and an idle tree costs nothing. Only the folders whose rows are on screen are watched,
//! and only in the workspace that is open; a change rereads the one folder it happened in.
//!
//! Where the system has no watch to give (a platform other than Linux, or its limit on watches
//! reached), the tree is read again after its own operations, when the screen is returned to and
//! on Refresh, as it would be without any of this.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use qframe::prelude::*;
use qframe::storage::{FolderChange, FolderChangeKind, FolderChanges, FolderWatch};

use super::files;
use super::{Msg, OpenWorkspace};

/// Whether a workspace's folders are watched.
#[derive(Debug, Default)]
pub(super) enum Live {
    /// Not watched: the screen does not follow the disk, or the workspace is not the open one.
    #[default]
    Off,
    /// The system has no watch to give; asking again for every message would only fail again.
    Unavailable,
    /// Watched.
    On(Watching),
}

/// A watch on the folders a workspace's tree shows.
#[derive(Debug)]
pub(super) struct Watching {
    watch: FolderWatch,
    /// Which watch this is, so changes a dropped watch sent before it noticed are not taken for
    /// one of this watch's.
    run: u64,
    /// The folders it was asked to watch, by tree key, with the path each was watched under; a
    /// folder the system refused has no path and is not asked for again while it stays on screen.
    folders: BTreeMap<String, Option<PathBuf>>,
}

impl Watching {
    /// The key of the folder a change was reported for.
    fn key_of(&self, folder: &Path) -> Option<String> {
        self.folders.iter().find(|(_, path)| path.as_deref() == Some(folder)).map(|(key, _)| key.clone())
    }

    /// The keys of the folders being watched.
    #[cfg(test)]
    pub(super) fn watched(&self) -> Vec<&str> {
        self.folders.iter().filter(|(_, path)| path.is_some()).map(|(key, _)| key.as_str()).collect()
    }

    /// Which watch this is.
    #[cfg(test)]
    pub(super) fn run(&self) -> u64 {
        self.run
    }

    /// The waiting side of the watch, for a test to take a batch by hand.
    #[cfg(test)]
    pub(super) fn changes(&self) -> FolderChanges {
        self.watch.changes()
    }
}

/// Keeps the watch of `workspace` on exactly the folders its tree shows: a watch is made the first
/// time, folders that came on screen are watched and folders that left it are let go.
///
/// A folder that comes back on screen with entries already known is read again, since whatever
/// happened in it while it was not watched went unseen. `run` numbers a new watch.
pub(super) fn follow(workspace: &mut OpenWorkspace, run: &mut u64) -> Command<Msg> {
    let mut commands = Vec::new();
    if matches!(workspace.live, Live::Off) {
        match FolderWatch::new() {
            Ok(watch) => {
                *run += 1;
                commands.push(wait(workspace.id().to_owned(), *run, watch.changes()));
                workspace.live = Live::On(Watching { watch, run: *run, folders: BTreeMap::new() });
            }
            Err(_) => workspace.live = Live::Unavailable,
        }
    }
    let Live::On(watching) = &mut workspace.live else { return Command::batch(commands) };
    let visible = workspace.files.visible_folders();
    let left: Vec<String> = watching.folders.keys().filter(|key| !visible.contains(key)).cloned().collect();
    for key in left {
        if let Some(Some(path)) = watching.folders.remove(&key) {
            watching.watch.unwatch(&path);
        }
    }
    let mut again = Vec::new();
    for key in visible {
        if watching.folders.contains_key(&key) {
            continue;
        }
        let path = workspace.files.path(&key);
        let watched = watching.watch.watch(&path).is_ok();
        if watched && workspace.files.children(&key).is_some() && !workspace.files.is_loading(&key) {
            again.push(key.clone());
        }
        watching.folders.insert(key, watched.then_some(path));
    }
    commands.push(files::reread(workspace, again));
    Command::batch(commands)
}

/// Lets the watch of `workspace` go, which also ends its waiting thread.
pub(super) fn stop(workspace: &mut OpenWorkspace) {
    if matches!(workspace.live, Live::On(_)) {
        workspace.live = Live::Off;
    }
}

/// Takes a batch of changes of the watch `run`: the folders they happened in are read again, and
/// the next batch is waited for.
///
/// A change of an entry's content leaves the tree as it is, since the tree shows names and not
/// what is in the files; a harness saving a file over and over reads nothing. A watched folder
/// that went away, or changes that came faster than the system could keep, mean anything may
/// have changed, so every folder on screen is read again.
pub(super) fn changed(workspace: &mut OpenWorkspace, run: u64, batch: Vec<FolderChange>) -> Command<Msg> {
    let Live::On(watching) = &mut workspace.live else { return Command::none() };
    // An empty batch only comes from a watch that was dropped, which this one is not; waiting
    // again on it would spin.
    if watching.run != run || batch.is_empty() {
        return Command::none();
    }
    let next = wait(workspace.id.as_str().to_owned(), run, watching.watch.changes());
    let mut again = Vec::new();
    let mut everything = false;
    for change in batch {
        let Some(key) = watching.key_of(&change.folder) else { continue };
        match change.kind {
            FolderChangeKind::Modified => {}
            FolderChangeKind::Created | FolderChangeKind::Removed | FolderChangeKind::Renamed { .. } => {
                again.push(key);
            }
            FolderChangeKind::Gone => {
                // The system no longer watches it; forgetting it lets it be watched again should
                // it come back on screen.
                watching.folders.remove(&key);
                everything = true;
            }
            FolderChangeKind::Overflow => everything = true,
        }
    }
    let reread = if everything {
        files::refresh(workspace)
    } else {
        again.sort();
        again.dedup();
        files::reread(workspace, again)
    };
    Command::batch([reread, next])
}

/// Waits on a background thread for the next batch of the watch `run` of the workspace `id`.
fn wait(id: String, run: u64, changes: FolderChanges) -> Command<Msg> {
    Command::perform(move || Msg::FilesChanged(id, run, changes.next()))
}
