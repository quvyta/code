//! The workspace's files: the file manager of the side panel, the framework's own, bound to a
//! workspace, and reading a document of the workspace for a tab that shows it.
//!
//! The manager's state lives in each open workspace. Its messages carry the workspace's id, so an
//! answer from the background (a folder read, an operation done, a change seen on disk) reaches
//! the workspace it was for, even after another one was opened.

use std::path::Path;

use qframe::prelude::*;
use qframe::widgets::FileManagerMsg;

use super::{Msg, OpenWorkspace, WorkspaceScreen};

/// Turns the file manager's messages into the screen's own, for the workspace `id`.
pub(super) fn wrap(id: &str) -> impl Fn(FileManagerMsg) -> Msg + Send + Sync + Clone + 'static {
    let id = id.to_owned();
    move |message| Msg::Files(id.clone(), message)
}

/// Hands a message of the file manager to the workspace `id`; one for a workspace that has been
/// closed meanwhile changes nothing.
pub(super) fn update(screen: &mut WorkspaceScreen, id: &str, message: FileManagerMsg) -> Command<Msg> {
    let Some(workspace) = screen.workspace_mut(id) else { return Command::none() };
    workspace.files.update(message, wrap(id))
}

/// Reads the workspace folder the first time, and every folder the tree shows again after that:
/// the files may have changed while the screen was away.
pub(super) fn load(workspace: &mut OpenWorkspace) -> Command<Msg> {
    let wrap = wrap(workspace.id());
    workspace.files.load(wrap)
}

/// Reads again every folder the tree shows, after something outside the manager changed the
/// workspace folder, such as a restore from a backup.
pub(super) fn refresh(workspace: &mut OpenWorkspace) -> Command<Msg> {
    let wrap = wrap(workspace.id());
    workspace.files.update(FileManagerMsg::Refresh, wrap)
}

/// The key of the entry at `path` under the workspace folder `root`, the way the tabs and the
/// session name a file: the folders below the root joined by `/`.
///
/// A path that is not under the root has no key, and gives an empty one, which
/// [`is_inside`] refuses wherever a key becomes a path.
#[must_use]
pub(super) fn key_of(root: &Path, path: &Path) -> String {
    let Ok(below) = path.strip_prefix(root) else { return String::new() };
    below.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// Whether `key` names an entry inside the tree's root: a path of plain names, none of them empty,
/// `.` or `..`, and not starting at `/`.
///
/// Keys come from the tree itself, which only ever joins the names a folder listed, but they also
/// come back from the session file, which anyone can edit. A key that climbs out of the workspace
/// would open a file of the host in a tab, or hand the container a path outside the one folder it
/// was given, so it is refused wherever a key becomes a path.
#[must_use]
pub fn is_inside(key: &str) -> bool {
    !key.is_empty() && !key.contains('\0') && key.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The name of the entry `key`, without the folders before it.
#[must_use]
pub fn name(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// Why a document of the workspace could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentTrouble {
    /// There is no file there any more.
    Missing,
    /// The file is a link that leads out of the workspace folder.
    Outside,
    /// What the operating system said.
    Unreadable(String),
}

/// Reads the text of the file at `path`, which must lie inside `root` once every link on the way
/// is followed.
///
/// The key was checked already; what is checked here is the one thing a key cannot show: a link
/// in the workspace that points somewhere else on this machine. Text that is not UTF-8 is shown
/// with its broken bytes replaced rather than not at all.
///
/// This touches the disk, so it belongs on a background thread.
///
/// # Errors
///
/// When the file is gone, leads out of `root`, or cannot be read.
pub fn read_document(path: &Path, root: &Path) -> Result<String, DocumentTrouble> {
    let trouble = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => DocumentTrouble::Missing,
        _ => DocumentTrouble::Unreadable(error.to_string()),
    };
    let real = path.canonicalize().map_err(trouble)?;
    let root = root.canonicalize().map_err(trouble)?;
    if !real.starts_with(&root) {
        return Err(DocumentTrouble::Outside);
    }
    let bytes = std::fs::read(&real).map_err(trouble)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
