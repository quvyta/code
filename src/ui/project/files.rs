//! The file tree widget's state: what has been read of `Project/`, and what is open.
//!
//! Reading a directory is I/O, so it never happens while drawing. The screen asks for a folder
//! when it is opened and keeps the answer here; the tree is built from what is already known.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileEntry {
    /// Its name, without any folder before it.
    pub name: String,
    /// Whether it is a folder, and so can be opened.
    pub folder: bool,
}

/// What the file tree knows about the project's own folder.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileTree {
    root: PathBuf,
    children: BTreeMap<String, Vec<FileEntry>>,
    open: BTreeSet<String>,
    loading: BTreeSet<String>,
    selected: Option<String>,
    error: Option<String>,
}

/// The key of the folder the tree is rooted at. Keys below it are paths relative to the root,
/// written with `/` whatever the platform, because they are identities rather than paths.
pub const ROOT: &str = "";

impl FileTree {
    /// A tree of the folder at `root`, with nothing read yet.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), ..Self::default() }
    }

    /// The folder the tree shows.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The entries of the folder `key`, when they have been read.
    #[must_use]
    pub fn children(&self, key: &str) -> Option<&[FileEntry]> {
        self.children.get(key).map(Vec::as_slice)
    }

    /// Whether the folder `key` is open.
    #[must_use]
    pub fn is_open(&self, key: &str) -> bool {
        self.open.contains(key)
    }

    /// Whether the folder `key` is being read right now.
    #[must_use]
    pub fn is_loading(&self, key: &str) -> bool {
        self.loading.contains(key)
    }

    /// The key of the selected entry.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Why the folder could not be read, when it could not be.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Moves the selection to `key`.
    pub fn select(&mut self, key: &str) {
        self.selected = Some(key.to_owned());
    }

    /// Opens or closes the folder `key`, and answers whether its entries still have to be read.
    ///
    /// A folder is read once; opening it again shows what is already known and reads nothing, so
    /// clicking a chevron twice does not go to the disk twice.
    pub fn expand(&mut self, key: &str, open: bool) -> bool {
        if !open {
            self.open.remove(key);
            return false;
        }
        self.open.insert(key.to_owned());
        if self.children.contains_key(key) || self.loading.contains(key) {
            return false;
        }
        self.loading.insert(key.to_owned());
        true
    }

    /// Marks the folder `key` as being read, and answers whether it was not already.
    pub fn start_reading(&mut self, key: &str) -> bool {
        if self.loading.contains(key) {
            return false;
        }
        self.loading.insert(key.to_owned());
        true
    }

    /// Takes the answer for the folder `key`.
    ///
    /// A folder that could not be read keeps its place in the tree and says what happened, so
    /// the person sees the folder and the reason rather than a gap.
    pub fn read(&mut self, key: &str, entries: Result<Vec<FileEntry>, String>) {
        self.loading.remove(key);
        match entries {
            Ok(entries) => {
                if key == ROOT {
                    self.error = None;
                }
                self.children.insert(key.to_owned(), entries);
            }
            Err(problem) => {
                if key == ROOT {
                    self.error = Some(problem);
                } else {
                    self.children.insert(key.to_owned(), Vec::new());
                }
            }
        }
    }

    /// The path on disk of the entry `key`.
    #[must_use]
    pub fn path(&self, key: &str) -> PathBuf {
        key.split('/').filter(|part| !part.is_empty()).fold(self.root.clone(), |path, part| path.join(part))
    }
}

/// The key of the entry `name` inside the folder `parent`.
#[must_use]
pub fn child_key(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") }
}

/// Reads one folder, folders first and then files, each group in name order.
///
/// This touches the disk, so it belongs on a background thread.
///
/// # Errors
///
/// What the operating system said, when the folder cannot be read.
pub fn read_folder(path: &Path) -> Result<Vec<FileEntry>, String> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        // A name the platform does not spell as text is still an entry; showing it lossily is
        // better than pretending the folder holds less than it does.
        let name = entry.file_name().to_string_lossy().into_owned();
        let folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        entries.push(FileEntry { name, folder });
    }
    entries.sort_by(|a, b| b.folder.cmp(&a.folder).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}
