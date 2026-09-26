//! What a person added to a profile by hand, in the profile's shell: the commands they ran as the
//! administrator and the files of the home they changed.
//!
//! The profile's image already holds both, committed from the shell's container. They are kept
//! here as well, beside the definition, so that a rebuild — which starts from the recipe and
//! fetches everything again — can put them back: the commands run again as `RUN` steps, the files
//! come back from `own-home.tar`.
//!
//! ```text
//! {store}/Profiles/<profile>/own.toml        the commands and the home's paths
//! {store}/Profiles/<profile>/own-home.tar    the home's files, as the shell left them
//! ```
//!
//! They live apart from `<profile>.toml` so that a profile made before this existed, or copied
//! without its folder, still reads as it did; a folder that is not there means nothing was added.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, ValueKind};
use qframe::storage::atomic_write;

use crate::base::paths::{HOME_DIR, OPEN_HOME, USER};
use crate::profile::SafeName;

/// The file the commands and the home's paths are kept in.
const FILE: &str = "own.toml";

/// The archive of the home's files.
pub const HOME_ARCHIVE: &str = "own-home.tar";

/// Where a build finds the archive, inside its context and inside the image while it unpacks.
const STAGED: &str = "/tmp/qcode-own-home.tar";

/// One command, in the file.
const STEP: &str = "step";
/// One path of the home, in the file.
const HOME: &str = "home";
/// The key both carry their text under.
const RUN: &str = "run";
const PATH: &str = "path";

/// What a person added to one profile by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Own {
    /// The commands they ran as the administrator, in the order they ran them, as they reviewed
    /// them.
    pub steps: Vec<String>,
    /// The files and folders of the home they changed, relative to the home.
    pub home: Vec<String>,
}

impl Own {
    /// The folder a profile's additions are kept in, inside the store's `Profiles/`.
    #[must_use]
    pub fn folder(profiles: &Path, profile: &SafeName) -> PathBuf {
        profiles.join(profile.as_str())
    }

    /// The archive of the home's files of a profile.
    #[must_use]
    pub fn archive(profiles: &Path, profile: &SafeName) -> PathBuf {
        Self::folder(profiles, profile).join(HOME_ARCHIVE)
    }

    /// Whether nothing was added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.home.is_empty()
    }

    /// Reads a profile's additions. A folder or file that is not there is nothing added; a file
    /// that cannot be read or understood is reported with its place and read as far as it goes,
    /// never panicked over.
    #[must_use]
    pub fn load(profiles: &Path, profile: &SafeName) -> (Self, Vec<Diagnostic>) {
        let path = Self::folder(profiles, profile).join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&path.to_string_lossy(), &text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(error) => (Self::default(), vec![Diagnostic::error(None, format!("{}: {error}", path.display()))]),
        }
    }

    /// Reads the text of an `own.toml`.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> (Self, Vec<Diagnostic>) {
        let document = Document::parse(file, text, &shape());
        let root = document.root();
        let steps = root.entries(STEP).iter().filter_map(|entry| entry.text(RUN)).map(str::to_owned).collect();
        let home = root.entries(HOME).iter().filter_map(|entry| entry.text(PATH)).map(str::to_owned).collect();
        (Self { steps, home }, document.diagnostics().to_vec())
    }

    /// The text of an `own.toml`.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut text = String::new();
        for step in &self.steps {
            let _ = write!(text, "[[{STEP}]]\n{RUN} = {}\n\n", quoted(step));
        }
        for path in &self.home {
            let _ = write!(text, "[[{HOME}]]\n{PATH} = {}\n\n", quoted(path));
        }
        text
    }

    /// Writes a profile's additions, or removes the file when nothing is left of them.
    ///
    /// # Errors
    ///
    /// A diagnostic naming the folder or file that could not be written.
    pub fn save(&self, profiles: &Path, profile: &SafeName) -> Result<(), Diagnostic> {
        let folder = Self::folder(profiles, profile);
        let path = folder.join(FILE);
        let blocked = |error: &io::Error| Diagnostic::error(None, format!("{}: {error}", path.display()));
        if self.is_empty() {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(blocked(&error)),
            };
        }
        std::fs::create_dir_all(&folder).map_err(|error| blocked(&error))?;
        atomic_write(&path, self.to_toml().as_bytes()).map_err(|error| blocked(&error))
    }

    /// Adds what one session in the shell added: its commands after the ones already kept, its
    /// paths beside them, each path once.
    pub fn absorb(&mut self, steps: Vec<String>, home: Vec<String>) {
        self.steps.extend(steps);
        for path in home {
            if !self.home.contains(&path) {
                self.home.push(path);
            }
        }
    }

    /// What a build appends to a profile's recipe to put the additions back: the commands as
    /// root, one `RUN` each so a failing one is named by the engine's own log, then the home's
    /// files, and last the step that opens the home to whoever runs the container, as the recipe
    /// ends. `with_archive` says whether the archive is in the build context.
    #[must_use]
    pub fn containerfile_tail(&self, with_archive: bool) -> String {
        if self.steps.is_empty() && !with_archive {
            return String::new();
        }
        let mut lines = vec!["USER root".to_owned()];
        for step in &self.steps {
            lines.push(format!("RUN {step}"));
        }
        if with_archive {
            lines.push(format!("COPY {HOME_ARCHIVE} {STAGED}"));
            lines.push(format!("RUN tar -xf {STAGED} -C {HOME_DIR} && rm -f {STAGED}"));
        }
        // What an administrator's command left in the package lists is not the person's.
        lines.push("RUN rm -rf /var/lib/apt/lists/*".to_owned());
        lines.push(format!("RUN {OPEN_HOME}"));
        lines.push(format!("USER {USER}"));
        lines.push(String::new());
        lines.join("\n")
    }
}

/// The shape of `own.toml`.
fn shape() -> Shape {
    Shape::new()
        .entries(STEP, Shape::new().required(RUN, ValueKind::text()))
        .entries(HOME, Shape::new().required(PATH, ValueKind::text()))
}

/// `text` as one TOML basic string.
fn quoted(text: &str) -> String {
    let mut spelled = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => spelled.push_str("\\\""),
            '\\' => spelled.push_str("\\\\"),
            '\n' => spelled.push_str("\\n"),
            '\t' => spelled.push_str("\\t"),
            '\r' => spelled.push_str("\\r"),
            other if other.is_control() => {
                let _ = write!(spelled, "\\u{:04X}", u32::from(other));
            }
            other => spelled.push(other),
        }
    }
    spelled.push('"');
    spelled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name() -> SafeName {
        SafeName::parse("own-test").expect("safe")
    }

    #[test]
    fn what_is_written_reads_back_as_itself_whatever_the_commands_hold() {
        let own = Own {
            steps: vec![
                "apt-get install -y jq".to_owned(),
                "printf '%s\\n' \"a \\\"quoted\\\" word\" > /etc/motd".to_owned(),
                "echo tab\there".to_owned(),
            ],
            home: vec!["notes.txt".to_owned(), ".config/tool/settings.json".to_owned()],
        };
        let (read, diagnostics) = Own::parse("own.toml", &own.to_toml());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(read, own);
    }

    #[test]
    fn a_broken_file_is_reported_at_its_place_and_not_panicked_over() {
        let (read, diagnostics) =
            Own::parse("own.toml", "[[step]]\nrun = \"apt-get install -y jq\"\n\n[[step]]\nrun = 3\n");
        assert_eq!(read.steps, ["apt-get install -y jq"]);
        let said = format!("{diagnostics:?}");
        assert!(!diagnostics.is_empty() && said.contains("own.toml"), "{said}");
    }

    #[test]
    fn a_profile_with_no_folder_has_nothing_added_and_saving_nothing_leaves_no_file() {
        let root = std::env::temp_dir().join(format!("qcode-own-{}", std::process::id()));
        let (own, diagnostics) = Own::load(&root, &name());
        assert!(own.is_empty() && diagnostics.is_empty());
        let mut own = Own::default();
        own.absorb(vec!["apt-get install -y jq".to_owned()], vec!["a".to_owned(), "a".to_owned()]);
        own.save(&root, &name()).expect("saved");
        assert_eq!(
            Own::load(&root, &name()).0,
            Own { steps: vec!["apt-get install -y jq".to_owned()], home: vec!["a".to_owned()] }
        );
        Own::default().save(&root, &name()).expect("emptied");
        assert!(!Own::folder(&root, &name()).join(FILE).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rebuild_runs_the_commands_as_root_then_unpacks_the_home_and_ends_as_the_images_user() {
        let own = Own {
            steps: vec!["apt-get update".to_owned(), "apt-get install -y jq".to_owned()],
            home: vec!["n".to_owned()],
        };
        let tail = own.containerfile_tail(true);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines[0], "USER root");
        assert_eq!(lines[1], "RUN apt-get update");
        assert_eq!(lines[2], "RUN apt-get install -y jq");
        assert_eq!(lines[3], "COPY own-home.tar /tmp/qcode-own-home.tar");
        assert!(lines[4].starts_with("RUN tar -xf /tmp/qcode-own-home.tar -C /home/qcode"), "{tail}");
        assert_eq!(lines.last(), Some(&"USER qcode"));
        assert!(tail.contains("RUN qcode-open-home"), "{tail}");
        assert_eq!(Own::default().containerfile_tail(false), "");
    }
}
