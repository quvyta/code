//! The file operations of the file tree: making, renaming, moving and deleting entries of the
//! project folder, on the host, and never outside it.
//!
//! Every operation takes the project folder and tree keys, never a path, and turns keys into paths
//! itself, so nothing the tree hands over can reach past the folder: a key part that is empty,
//! `.` or `..` is refused, and so is a key that goes through a symbolic link, because a link can
//! point anywhere. A link is an entry like any other, so renaming, moving or deleting one acts on
//! the link and leaves what it points at alone.
//!
//! These touch the disk, so they run on a background thread.

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::ops::Range;
use std::path::{Path, PathBuf};

use qframe::t;

use super::files::{ROOT, child_key};

/// Why a name cannot be used, as the person types it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameProblem {
    /// Nothing, or only spaces, was typed.
    Empty,
    /// It holds a `/`, which would make it a path rather than a name.
    Slash,
    /// It holds a character no file name can hold.
    Nul,
    /// It is `.` or `..`, which already mean this folder and the one above it.
    Dots,
    /// An entry of the folder already has it.
    Taken,
}

impl NameProblem {
    /// What is wrong, in the person's language.
    #[must_use]
    pub fn message(&self) -> String {
        let key = match self {
            Self::Empty => "project.files.name-empty",
            Self::Slash => "project.files.name-slash",
            Self::Nul => "project.files.name-nul",
            Self::Dots => "project.files.name-dots",
            Self::Taken => "project.files.name-taken",
        };
        t!(key)
    }
}

/// Checks `name` as the name of a new entry among `siblings`.
///
/// `current` is the entry's own name when it is being renamed, which does not count as taken.
///
/// # Errors
///
/// The first thing wrong with the name.
pub fn check_name<'a>(
    name: &str,
    mut siblings: impl Iterator<Item = &'a str>,
    current: Option<&str>,
) -> Result<(), NameProblem> {
    if name.trim().is_empty() {
        return Err(NameProblem::Empty);
    }
    if name.contains('/') {
        return Err(NameProblem::Slash);
    }
    if name.contains('\0') {
        return Err(NameProblem::Nul);
    }
    if name == "." || name == ".." {
        return Err(NameProblem::Dots);
    }
    if current != Some(name) && siblings.any(|sibling| sibling == name) {
        return Err(NameProblem::Taken);
    }
    Ok(())
}

/// Why an operation was not done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    /// The name cannot be used.
    Name(NameProblem),
    /// The entry would be reached, or land, outside the project folder.
    Outside,
    /// A folder was to go into itself or into a folder below it.
    IntoItself,
    /// The target folder already has an entry of this name.
    Taken(String),
    /// Moving would cross to another file system, which a rename cannot do; files are not copied.
    CrossDevice,
    /// What the operating system said.
    System(String),
}

impl FileError {
    /// What went wrong, in the person's language.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Name(problem) => problem.message(),
            Self::Outside => t!("project.files.outside"),
            Self::IntoItself => t!("project.files.into-itself"),
            Self::Taken(name) => t!("project.files.taken", name = name.as_str()),
            Self::CrossDevice => t!("project.files.cross-device"),
            Self::System(said) => said.clone(),
        }
    }
}

impl From<std::io::Error> for FileError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == ErrorKind::CrossesDevices { Self::CrossDevice } else { Self::System(error.to_string()) }
    }
}

/// What an operation changed, by tree key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// An entry was made.
    Created(String),
    /// An entry moved from the first key to the second, by a rename or a move.
    Moved(String, String),
    /// An entry was deleted, with everything in it.
    Deleted(String),
}

/// The key of the folder an entry is in.
#[must_use]
pub fn parent_key(key: &str) -> &str {
    key.rsplit_once('/').map_or(ROOT, |(parent, _)| parent)
}

/// The name of the entry `key`, without the folders before it.
#[must_use]
pub fn name_of(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(_, name)| name)
}

/// The characters of `name` a rename starts with selected: the name before its extension, so
/// typing replaces `report.final` and keeps `.md`.
///
/// A folder has no extension, and neither has a name whose only dot starts it (`.gitignore`):
/// both are selected whole. The range counts characters, the way the field counts them.
#[must_use]
pub fn stem(name: &str, folder: bool) -> Range<usize> {
    let length = name.chars().count();
    if folder {
        return 0..length;
    }
    match name.chars().rev().position(|c| c == '.').map(|from_end| length - 1 - from_end) {
        Some(dot) if dot > 0 => 0..dot,
        _ => 0..length,
    }
}

/// Whether `key` is `folder` or somewhere below it.
#[must_use]
pub fn is_within(key: &str, folder: &str) -> bool {
    key == folder || key.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// The path of the entry `key`, which is not followed if it is a link.
///
/// Every folder on the way must be a real folder of the project, not a link, so the path cannot
/// lead out of it.
fn entry_path(root: &Path, key: &str) -> Result<PathBuf, FileError> {
    let parts: Vec<&str> = key.split('/').collect();
    let mut path = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || *part == "." || *part == ".." || part.contains('\0') {
            return Err(FileError::Outside);
        }
        path.push(part);
        if index + 1 < parts.len() {
            real_folder(&path)?;
        }
    }
    Ok(path)
}

/// The path of the folder `key`, which must be a real folder inside the project.
fn folder_path(root: &Path, key: &str) -> Result<PathBuf, FileError> {
    if key == ROOT {
        return Ok(root.to_path_buf());
    }
    let path = entry_path(root, key)?;
    real_folder(&path)?;
    Ok(path)
}

/// Refuses a path that is a link or not a folder.
fn real_folder(path: &Path) -> Result<(), FileError> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(FileError::Outside);
    }
    Ok(())
}

/// Whether anything, a dangling link included, is at `path`.
fn occupied(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Checks a name the operations are handed; the dialog checked it already, so this only guards
/// against a key or a name that did not come through it.
fn usable(name: &str) -> Result<(), FileError> {
    check_name(name, std::iter::empty(), None).map_err(FileError::Name)
}

/// Makes an empty file `name` in the folder `folder`.
///
/// # Errors
///
/// A name that cannot be used, a folder outside the project, a name already there, or what the
/// system said.
pub fn create_file(root: &Path, folder: &str, name: &str) -> Result<Change, FileError> {
    usable(name)?;
    let path = folder_path(root, folder)?.join(name);
    // `create_new` refuses an existing entry in the same step that makes the file, so nothing
    // already there is ever truncated.
    OpenOptions::new().write(true).create_new(true).open(&path).map_err(|error| taken_or(error, name))?;
    Ok(Change::Created(child_key(folder, name)))
}

/// Makes an empty folder `name` in the folder `folder`.
///
/// # Errors
///
/// As [`create_file`].
pub fn create_folder(root: &Path, folder: &str, name: &str) -> Result<Change, FileError> {
    usable(name)?;
    let path = folder_path(root, folder)?.join(name);
    fs::create_dir(&path).map_err(|error| taken_or(error, name))?;
    Ok(Change::Created(child_key(folder, name)))
}

/// Gives the entry `key` the name `name`, in the same folder.
///
/// # Errors
///
/// A name that cannot be used, an entry outside the project, a name already there, or what the
/// system said.
pub fn rename(root: &Path, key: &str, name: &str) -> Result<Change, FileError> {
    usable(name)?;
    let from = entry_path(root, key)?;
    let folder = parent_key(key);
    if name_of(key) == name {
        return Ok(Change::Moved(key.to_owned(), key.to_owned()));
    }
    let to = folder_path(root, folder)?.join(name);
    if occupied(&to) {
        return Err(FileError::Taken(name.to_owned()));
    }
    fs::rename(&from, &to)?;
    Ok(Change::Moved(key.to_owned(), child_key(folder, name)))
}

/// Moves the entry `key` into the folder `into`, keeping its name.
///
/// A move is a rename on the same file system; files are never copied, so a move to another
/// file system is refused and said so.
///
/// # Errors
///
/// An entry or folder outside the project, a folder into itself, a name already there, another
/// file system, or what the system said.
pub fn move_into(root: &Path, key: &str, into: &str) -> Result<Change, FileError> {
    if is_within(into, key) {
        return Err(FileError::IntoItself);
    }
    let from = entry_path(root, key)?;
    let name = name_of(key);
    if parent_key(key) == into {
        return Ok(Change::Moved(key.to_owned(), key.to_owned()));
    }
    let to = folder_path(root, into)?.join(name);
    if occupied(&to) {
        return Err(FileError::Taken(name.to_owned()));
    }
    fs::rename(&from, &to)?;
    Ok(Change::Moved(key.to_owned(), child_key(into, name)))
}

/// Deletes the entry `key`, a folder with everything in it. A link is removed as a link; what it
/// points at stays.
///
/// # Errors
///
/// An entry outside the project, or what the system said.
pub fn delete(root: &Path, key: &str) -> Result<Change, FileError> {
    let path = entry_path(root, key)?;
    let meta = fs::symlink_metadata(&path)?;
    // The standard library's `remove_dir_all` does not follow links inside the folder either, so
    // a link in there is removed without touching where it leads.
    if meta.is_dir() {
        fs::remove_dir_all(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    Ok(Change::Deleted(key.to_owned()))
}

/// An error of making `name`, where an entry already there is said by name.
fn taken_or(error: std::io::Error, name: &str) -> FileError {
    if error.kind() == ErrorKind::AlreadyExists { FileError::Taken(name.to_owned()) } else { error.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test's own, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let stamp =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path = std::env::temp_dir().join(format!("qcode-file-ops-{name}-{stamp}"));
            fs::create_dir_all(path.join("Project").join("src")).expect("a project folder");
            fs::write(path.join("Project").join("README.md"), "hello\n").expect("a file");
            fs::write(path.join("outside.txt"), "keep\n").expect("a file outside the project");
            Self(path)
        }

        fn root(&self) -> PathBuf {
            self.0.join("Project")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_are_checked_the_way_the_dialog_shows_them() {
        let siblings = ["src", "README.md"];
        let check = |name: &str, current| check_name(name, siblings.iter().copied(), current);
        assert_eq!(check("", None), Err(NameProblem::Empty));
        assert_eq!(check("   ", None), Err(NameProblem::Empty));
        assert_eq!(check("a/b", None), Err(NameProblem::Slash));
        assert_eq!(check("a\0b", None), Err(NameProblem::Nul));
        assert_eq!(check(".", None), Err(NameProblem::Dots));
        assert_eq!(check("..", None), Err(NameProblem::Dots));
        assert_eq!(check("src", None), Err(NameProblem::Taken));
        assert_eq!(check("src", Some("src")), Ok(()), "an entry's own name is not taken from it");
        assert_eq!(check("lib.rs", None), Ok(()));
        assert_eq!(check(".hidden", None), Ok(()), "a name may start with a dot");
    }

    #[test]
    fn the_name_before_the_extension_is_what_a_rename_selects() {
        assert_eq!(stem("report.final.md", false), 0..12);
        assert_eq!(stem("main.rs", false), 0..4);
        assert_eq!(stem("şğü.txt", false), 0..3, "characters, not bytes");
        assert_eq!(stem("Makefile", false), 0..8);
        assert_eq!(stem(".gitignore", false), 0..10, "a dotfile is all name");
        assert_eq!(stem(".env.local", false), 0..4);
        assert_eq!(stem("v1.2", true), 0..4, "a folder has no extension");
    }

    #[test]
    fn keys_that_reach_outside_the_project_are_refused() {
        let scratch = Scratch::new("outside");
        let root = scratch.root();
        for key in ["..", "../outside.txt", "src/../../outside.txt", "./README.md", "src//x", "/etc"] {
            assert_eq!(delete(&root, key), Err(FileError::Outside), "{key}");
            assert_eq!(rename(&root, key, "x"), Err(FileError::Outside), "{key}");
            assert_eq!(move_into(&root, key, "src"), Err(FileError::Outside), "{key}");
        }
        assert_eq!(create_file(&root, "..", "x"), Err(FileError::Outside));
        assert_eq!(move_into(&root, "README.md", ".."), Err(FileError::Outside));
        assert_eq!(rename(&root, "README.md", "../x"), Err(FileError::Name(NameProblem::Slash)));
        assert!(scratch.0.join("outside.txt").exists(), "nothing outside was touched");
        assert!(!scratch.0.join("x").exists());
    }

    #[test]
    fn a_link_out_of_the_project_is_never_followed() {
        let scratch = Scratch::new("link");
        let root = scratch.root();
        let away = scratch.0.join("away");
        fs::create_dir_all(&away).expect("a folder outside");
        fs::write(away.join("secret.txt"), "keep\n").expect("a file outside");
        std::os::unix::fs::symlink(&away, root.join("door")).expect("a link out");

        assert_eq!(create_file(&root, "door", "x"), Err(FileError::Outside), "nothing is made through it");
        assert_eq!(delete(&root, "door/secret.txt"), Err(FileError::Outside), "nothing is deleted through it");
        assert_eq!(move_into(&root, "README.md", "door"), Err(FileError::Outside), "nothing moves into it");
        assert!(!away.join("x").exists());

        assert_eq!(delete(&root, "door"), Ok(Change::Deleted("door".to_owned())), "the link itself goes");
        assert!(away.join("secret.txt").exists(), "and what it pointed at stays");
    }

    #[test]
    fn entries_are_made_renamed_moved_and_deleted() {
        let scratch = Scratch::new("ops");
        let root = scratch.root();
        assert_eq!(create_file(&root, "src", "lib.rs"), Ok(Change::Created("src/lib.rs".to_owned())));
        assert!(root.join("src/lib.rs").is_file());
        assert_eq!(create_folder(&root, ROOT, "docs"), Ok(Change::Created("docs".to_owned())));
        assert!(root.join("docs").is_dir());
        assert_eq!(rename(&root, "README.md", "GUIDE.md"), Ok(Change::Moved("README.md".into(), "GUIDE.md".into())));
        assert!(root.join("GUIDE.md").is_file() && !root.join("README.md").exists());
        assert_eq!(move_into(&root, "GUIDE.md", "docs"), Ok(Change::Moved("GUIDE.md".into(), "docs/GUIDE.md".into())));
        assert_eq!(fs::read_to_string(root.join("docs/GUIDE.md")).ok().as_deref(), Some("hello\n"));
        assert_eq!(move_into(&root, "docs", "src"), Ok(Change::Moved("docs".into(), "src/docs".into())));
        assert_eq!(delete(&root, "src"), Ok(Change::Deleted("src".to_owned())));
        assert!(!root.join("src").exists(), "a folder goes with everything in it");
    }

    #[test]
    fn a_name_already_there_is_refused_and_nothing_is_overwritten() {
        let scratch = Scratch::new("clash");
        let root = scratch.root();
        fs::write(root.join("src/README.md"), "other\n").expect("a clashing file");
        assert_eq!(create_file(&root, ROOT, "README.md"), Err(FileError::Taken("README.md".to_owned())));
        assert_eq!(create_folder(&root, ROOT, "src"), Err(FileError::Taken("src".to_owned())));
        assert_eq!(rename(&root, "README.md", "src"), Err(FileError::Taken("src".to_owned())));
        assert_eq!(move_into(&root, "README.md", "src"), Err(FileError::Taken("README.md".to_owned())));
        assert_eq!(fs::read_to_string(root.join("README.md")).ok().as_deref(), Some("hello\n"));
        assert_eq!(fs::read_to_string(root.join("src/README.md")).ok().as_deref(), Some("other\n"));
    }

    #[test]
    fn a_folder_cannot_go_into_itself_or_below_itself() {
        let scratch = Scratch::new("itself");
        let root = scratch.root();
        fs::create_dir_all(root.join("src/deep")).expect("a folder below");
        assert_eq!(move_into(&root, "src", "src"), Err(FileError::IntoItself));
        assert_eq!(move_into(&root, "src", "src/deep"), Err(FileError::IntoItself));
        assert!(root.join("src/deep").is_dir());
        assert!(!is_within("srcs", "src"), "a folder whose name only starts the same is another folder");
    }
}
