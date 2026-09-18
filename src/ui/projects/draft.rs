//! The new-project dialog's own state: the name, where the content comes from, and everything
//! that can stop a project being made.
//!
//! Nothing here touches the disk. Every problem is a value the view turns into a sentence from
//! the language files, so the same check reads the same way in every language and can be tested
//! without a screen.

use std::path::{Path, PathBuf};

use qframe::t;
use qframe::widgets::{FileBrowser, PickMode};

use crate::workspace::{NewProjectError, ProjectId, ProjectIdError};

/// Where a new project's content comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Nothing but the folder tree of a project.
    Empty,
    /// A folder of the person's own, copied in. Their folder is never moved.
    Folder,
    /// A git address, cloned inside a container.
    Git,
}

impl Source {
    /// The sources in the order the dialog offers them.
    pub const ALL: [Self; 3] = [Self::Empty, Self::Folder, Self::Git];

    /// The source at `index` of [`Source::ALL`]; anything else is the plain empty project, which
    /// is the one source that can never fail.
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or(Self::Empty)
    }

    /// Its place in [`Source::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|source| *source == self).unwrap_or(0)
    }

    /// The translated label of its segment.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Empty => t!("projects.source-empty"),
            Self::Folder => t!("projects.source-folder"),
            Self::Git => t!("projects.source-git"),
        }
    }
}

/// What stopped a project being made, kept apart from its wording.
///
/// A name that is already taken is never quietly turned into `name-2`: it becomes
/// [`Problem::Taken`], which the dialog shows beside the name field and asks the person to
/// change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The name yields no identifier, and why.
    Name(ProjectIdError),
    /// A project of this identifier is already in the workspace.
    Taken(String),
    /// The workspace could not be written; the diagnostic says which path and why.
    Blocked(String),
    /// No folder was chosen to copy.
    NoFolder,
    /// The chosen folder holds the workspace, so copying it would copy the copy.
    FolderHoldsWorkspace,
    /// No address was written to clone.
    NoUrl,
    /// An address that would reach git as an option rather than a repository.
    UrlIsOption,
    /// Cloning was asked for while no container engine is there to clone in.
    NoEngine,
    /// The copy or the clone ran and did not finish; the text is what it said.
    Failed(String),
}

impl Problem {
    /// The field the problem belongs beside.
    #[must_use]
    pub fn field(&self) -> &'static str {
        match self {
            Self::Name(_) | Self::Taken(_) => NAME_FIELD,
            Self::NoFolder | Self::FolderHoldsWorkspace => FOLDER_FIELD,
            Self::NoUrl | Self::UrlIsOption | Self::NoEngine => URL_FIELD,
            // The workspace refusing to be written, and work that ran and stopped, are about
            // nothing the person typed; marking a field would blame the wrong thing, so they
            // are only told above the form.
            Self::Blocked(_) | Self::Failed(_) => WORK_FIELD,
        }
    }

    /// The sentence the person reads.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Name(problem) => name_message(*problem),
            Self::Taken(id) => t!("projects.name-taken", id = id.as_str()),
            Self::Blocked(detail) => format!("{} {detail}", t!("projects.blocked")),
            Self::NoFolder => t!("projects.folder-missing"),
            Self::FolderHoldsWorkspace => t!("projects.folder-inside"),
            Self::NoUrl => t!("projects.url-missing"),
            Self::UrlIsOption => t!("projects.url-flag"),
            Self::NoEngine => t!("projects.engine-missing"),
            Self::Failed(detail) => format!("{} {detail}", t!("projects.failed")),
        }
    }
}

impl From<NewProjectError> for Problem {
    fn from(error: NewProjectError) -> Self {
        match error {
            NewProjectError::Name(problem) => Self::Name(problem),
            NewProjectError::Taken(id) => Self::Taken(id.as_str().to_owned()),
            NewProjectError::Blocked(diagnostic) => Self::Blocked(diagnostic.to_string()),
        }
    }
}

/// The name of the name field, which the error summary and the focus use.
pub const NAME_FIELD: &str = "name";

/// The name of the folder field.
pub const FOLDER_FIELD: &str = "folder";

/// The name of the address field.
pub const URL_FIELD: &str = "url";

/// The name of the problem that belongs to no field: it is shown above the form and nowhere else.
pub const WORK_FIELD: &str = "work";

/// The sentence for one reason a name cannot become an identifier. The position is counted from
/// one, because that is how a person counts the letters of the name they just typed.
fn name_message(problem: ProjectIdError) -> String {
    match problem {
        ProjectIdError::Empty => t!("projects.name-empty"),
        ProjectIdError::Illegal { position, character } => t!(
            "projects.name-illegal",
            character = character.to_string(),
            position = i64::try_from(position.saturating_add(1)).unwrap_or(i64::MAX),
        ),
        ProjectIdError::TooLong { length } => t!(
            "projects.name-too-long",
            length = i64::try_from(length).unwrap_or(i64::MAX),
            max = i64::try_from(ProjectId::MAX_LEN).unwrap_or(i64::MAX),
        ),
        ProjectIdError::Reserved => t!("projects.name-reserved"),
    }
}

/// Everything the new-project dialog holds while it is open.
#[derive(Debug)]
pub struct Draft {
    /// The name as it is being typed.
    pub name: String,
    /// Which of the three sources is chosen.
    pub source: Source,
    /// The git address, while one is being written.
    pub url: String,
    /// The folder browser of the folder source.
    pub browser: FileBrowser,
    /// The folder chosen to copy.
    pub folder: Option<PathBuf>,
    /// What stopped the last attempt, until something is changed.
    pub problem: Option<Problem>,
    /// Whether the long work is running, which turns the dialog into its own progress.
    pub busy: bool,
}

impl Draft {
    /// An empty draft whose folder browser starts at `start`.
    #[must_use]
    pub fn new(start: PathBuf) -> Self {
        Self {
            name: String::new(),
            source: Source::Empty,
            url: String::new(),
            browser: FileBrowser::new(start, PickMode::Folders),
            folder: None,
            problem: None,
            busy: false,
        }
    }

    /// The identifier the name would get on disk, while it yields one.
    #[must_use]
    pub fn preview(&self) -> Option<String> {
        ProjectId::from_display_name(&self.name).ok().map(|id| id.as_str().to_owned())
    }

    /// The problem of the name as it is being typed, or `None` while it is still empty: an
    /// untouched field is not a mistake, it is a field nobody has reached yet.
    #[must_use]
    pub fn typing_problem(&self) -> Option<Problem> {
        if self.name.trim().is_empty() {
            return None;
        }
        ProjectId::from_display_name(&self.name).err().map(Problem::Name)
    }

    /// Everything that can be answered without touching the disk, checked in the order the
    /// dialog reads: first the name, then whatever the chosen source needs.
    ///
    /// `workspace` is the workspace folder, which a folder to copy may not contain, and `engine`
    /// says whether there is a container engine to clone in.
    #[must_use]
    pub fn check(&self, workspace: &Path, engine: bool) -> Option<Problem> {
        if let Err(problem) = ProjectId::from_display_name(&self.name) {
            return Some(Problem::Name(problem));
        }
        match self.source {
            Source::Empty => None,
            Source::Folder => match &self.folder {
                None => Some(Problem::NoFolder),
                // A folder holding the workspace would be copied into its own copy, which never
                // ends; the workspace is inside it exactly when its path starts with the folder.
                Some(folder) if workspace.starts_with(folder) => Some(Problem::FolderHoldsWorkspace),
                Some(_) => None,
            },
            Source::Git => match self.url.trim() {
                "" => Some(Problem::NoUrl),
                url if url.starts_with('-') => Some(Problem::UrlIsOption),
                _ if !engine => Some(Problem::NoEngine),
                _ => None,
            },
        }
    }
}
