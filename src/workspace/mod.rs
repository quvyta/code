//! The workspace: where QCode's settings, projects and profiles live on disk.
//!
//! Five things live here, and each of them is readable without ever panicking:
//!
//! - [`HostDirs`] holds the default workspace location, which the framework works out.
//! - [`Config`] is the application's `code.conf`, checked and repaired by a [`Schema`].
//! - [`ProjectId`] turns a display name into a name every file system accepts.
//! - [`Workspace`] creates and reads the folder tree of the workspace and its projects.
//! - [`Session`] records which projects and tabs were open, so they can be opened again.
//!
//! Every loader answers with a [`Loaded`]: the part that could be used, plus a
//! [`Diagnostic`] for each problem, pointing at the file, line and column where it is.
//!
//! [`Schema`]: qframe::storage::Schema

mod config;
mod identity;
mod layout;
mod paths;
mod project;
mod session;

pub use config::{Config, SetupStep};
pub use identity::{ProjectId, ProjectIdError};
pub use layout::{NewProjectError, ProjectEntry, ProjectPaths, Workspace, add_profile};
pub use paths::{APP_TITLE, HostDirs, Platform};
pub use project::{ProjectFile, ProjectProfile};
pub use session::{Session, SessionProject, SessionTab, SessionTabKind};

use qframe::diagnostics::Diagnostic;

/// What could be read, together with everything that was wrong with it.
///
/// A broken entry is skipped rather than fatal, so `value` is still the usable part of the
/// file: a project list keeps its readable projects, a project file keeps its readable
/// profiles. `diagnostics` is empty exactly when nothing was wrong.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded<T> {
    /// The part of the input that could be used.
    pub value: T,
    /// Every problem found while reading, in the order they were found.
    pub diagnostics: Vec<Diagnostic>,
}

impl<T> Loaded<T> {
    /// Whether nothing was wrong with the input.
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }
}
