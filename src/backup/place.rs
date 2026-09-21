//! The paths a person names inside `Work/`: a folder to leave out of the backup, a file to
//! bring back.
//!
//! They come from the workspace file and from the file tree, and they end up in two places that
//! read them in their own way: git's `info/exclude`, where `*`, `[` or a leading `!` would mean
//! something, and a git command line. So each one is held to what it is meant to be — a path
//! inside the workspace — before it goes anywhere, and is written out so git reads it as the path
//! it is and nothing more.

use std::fmt;

/// A path inside `Work/`, checked, with `/` between its parts and no `/` at either end.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place(String);

/// Why a path is not a place inside the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceProblem {
    /// Nothing is named.
    Empty,
    /// It starts at the root of a drive or of the machine rather than inside the workspace.
    Absolute,
    /// A `.` or `..` part: the one could name the whole workspace, the other leaves it.
    Outside,
    /// A line break or a NUL, which no exclude file and no command line would read as one path.
    LineBreak,
}

/// A path that was refused, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadPlace {
    /// The path as it was given.
    pub path: String,
    /// What is wrong with it.
    pub problem: PlaceProblem,
}

impl Place {
    /// `path` as a place inside the workspace, when it is one.
    ///
    /// A `/` at the end is dropped, because the file tree names a folder either way; `\` is not
    /// read as a separator, because on the machines whose file names may hold one it is a
    /// letter.
    ///
    /// # Errors
    ///
    /// The path and why it is not a place inside the workspace.
    pub fn new(path: &str) -> Result<Self, BadPlace> {
        let refuse = |problem| Err(BadPlace { path: path.to_owned(), problem });
        if path.contains(['\n', '\r', '\0']) {
            return refuse(PlaceProblem::LineBreak);
        }
        if path.starts_with(['/', '\\']) || is_drive(path) {
            return refuse(PlaceProblem::Absolute);
        }
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            return refuse(PlaceProblem::Empty);
        }
        let mut parts = Vec::new();
        for part in trimmed.split('/') {
            match part {
                // `a//b` is `a/b` to every file system QCode runs on.
                "" => {}
                "." | ".." => return refuse(PlaceProblem::Outside),
                part => parts.push(part),
            }
        }
        Ok(Self(parts.join("/")))
    }

    /// The path, as git and the file tree name it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The line of an exclude file that leaves this place out and nothing else.
    ///
    /// The leading `/` ties it to the top of the workspace, so `data` leaves out `data/` and not
    /// every folder called `data` further down; every character git would read as a pattern is
    /// escaped, and so is every space, because git drops the ones at the end of a line.
    fn exclude_line(&self) -> String {
        let mut line = String::from("/");
        for char in self.0.chars() {
            if matches!(char, '\\' | '*' | '?' | '[' | ']' | ' ') {
                line.push('\\');
            }
            line.push(char);
        }
        line
    }
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `path` starts with a Windows drive, `C:`.
fn is_drive(path: &str) -> bool {
    let mut chars = path.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic()) && chars.next() == Some(':')
}

/// Every entry of a skip list as a place inside the workspace.
///
/// # Errors
///
/// The first entry that is not one. A skip list with a bad entry is not written at all: a list
/// that was half applied would back up a folder the person asked to leave out and say nothing.
pub fn places(skip: &[String]) -> Result<Vec<Place>, BadPlace> {
    skip.iter().map(|path| Place::new(path)).collect()
}

/// The whole of `info/exclude` for `skip`: one line per place, each ending in a line break.
#[must_use]
pub fn exclude_file(skip: &[Place]) -> String {
    skip.iter().map(|place| place.exclude_line() + "\n").collect()
}

#[cfg(test)]
mod tests {
    use super::{Place, PlaceProblem, exclude_file, places};

    fn problem(path: &str) -> PlaceProblem {
        Place::new(path).expect_err("refused").problem
    }

    #[test]
    fn a_path_inside_the_workspace_is_kept_as_it_is_named() {
        assert_eq!(Place::new("data").expect("a folder").as_str(), "data");
        assert_eq!(Place::new("out/big/").expect("a folder").as_str(), "out/big");
        assert_eq!(Place::new("out//big").expect("a folder").as_str(), "out/big");
        assert_eq!(Place::new(".cache").expect("a hidden folder").as_str(), ".cache");
        assert_eq!(Place::new("a..b").expect("dots inside a name").as_str(), "a..b");
    }

    #[test]
    fn a_path_that_leaves_the_workspace_is_refused() {
        assert_eq!(problem(""), PlaceProblem::Empty);
        assert_eq!(problem("/"), PlaceProblem::Absolute);
        assert_eq!(problem("/etc"), PlaceProblem::Absolute);
        assert_eq!(problem("\\share"), PlaceProblem::Absolute);
        assert_eq!(problem("C:"), PlaceProblem::Absolute);
        assert_eq!(problem(".."), PlaceProblem::Outside);
        assert_eq!(problem("data/../.."), PlaceProblem::Outside);
        assert_eq!(problem("."), PlaceProblem::Outside);
        assert_eq!(problem("data\n/etc"), PlaceProblem::LineBreak);
        assert_eq!(problem("data\r"), PlaceProblem::LineBreak);
        assert_eq!(problem("data\0"), PlaceProblem::LineBreak);
    }

    #[test]
    fn one_bad_entry_refuses_the_whole_list() {
        let skip = ["data".to_owned(), "../home".to_owned()];
        let bad = places(&skip).expect_err("one entry leaves the workspace");
        assert_eq!(bad.path, "../home");
        assert_eq!(bad.problem, PlaceProblem::Outside);
    }

    #[test]
    fn the_exclude_file_ties_each_place_to_the_top_and_reads_no_pattern_into_it() {
        let skip = places(&["data".to_owned(), "out/big".to_owned(), "!odd [1]*? \\ ".to_owned()]).expect("valid");
        assert_eq!(exclude_file(&skip), "/data\n/out/big\n/!odd\\ \\[1\\]\\*\\?\\ \\\\\\ \n");
        assert_eq!(exclude_file(&[]), "");
    }
}
