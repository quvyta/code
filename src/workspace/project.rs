//! `project.qcode`: what a project is called, when it was made and which profiles it carries.
//!
//! The file is a [`Document`](qframe::document::Document): the framework checks the shape and
//! reports every problem with its line and column, and what only qcode can judge — whether the
//! id is a project id, whether a date is a day the calendar has — is checked here. A broken
//! entry is skipped, and everything that is readable is still used.

use std::fmt::Write as _;

use qframe::date::Date;
use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, Table, ValueKind};

use super::{Loaded, ProjectId};

/// One profile a project carries, as `project.qcode` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectProfile {
    /// The profile's name, which is also the name of its folder below `Containers/Harness`.
    pub name: String,
    /// The day the profile was added, when the file records a readable one.
    pub added: Option<Date>,
}

/// The contents of a project's `project.qcode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// The project's identifier, which is also the name of its folder.
    pub id: ProjectId,
    /// The name the user gave it, in the user's own writing.
    pub name: String,
    /// The day the project was made, when the file records a readable one.
    pub created: Option<Date>,
    /// The profiles the project carries, in the order the file lists them.
    pub profiles: Vec<ProjectProfile>,
}

impl ProjectFile {
    /// A project made today with no profiles yet.
    #[must_use]
    pub fn new(id: ProjectId, name: impl Into<String>, created: Date) -> Self {
        Self { id, name: name.into(), created: Some(created), profiles: Vec::new() }
    }

    /// Reads a `project.qcode`, reporting problems against `file`.
    ///
    /// The value is `None` only when the file names no project this program could open: the
    /// identifier is missing or is not one. Anything else — a missing name, an unreadable date,
    /// a profile without a name, a key nobody knows — is a diagnostic beside a usable project.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> Loaded<Option<Self>> {
        let document = Document::parse(file, text, &shape());
        let mut diagnostics = document.diagnostics().to_vec();
        let root = document.root();

        // A missing or wrongly typed `id` is already reported by the document; only an id that
        // is text and still no project id is qcode's own finding.
        let id = match root.text("id").map(|text| (text, ProjectId::parse(text))) {
            Some((_, Ok(id))) => Some(id),
            Some((_, Err(problem))) => {
                let at = root.value_location("id").cloned();
                diagnostics.push(Diagnostic::error(at, format!("`id` is not a project id: {problem:?}")));
                None
            }
            None => None,
        };
        let Some(id) = id else {
            return Loaded { value: None, diagnostics };
        };
        let name = root.text("name").map_or_else(
            || {
                diagnostics.push(Diagnostic::warning(None, "no readable `name`; the id is shown instead"));
                id.as_str().to_owned()
            },
            str::to_owned,
        );
        let created = date(root, "created", &mut diagnostics);
        // An entry without a name was reported by the document as missing its required key;
        // skipping it here is what that report means.
        let profiles = root
            .entries("profile")
            .iter()
            .filter_map(|entry| {
                let name = entry.text("name")?.to_owned();
                Some(ProjectProfile { name, added: date(entry, "added", &mut diagnostics) })
            })
            .collect();
        Loaded { value: Some(Self { id, name, created, profiles }), diagnostics }
    }

    /// The file as it is written to disk.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "id = {}", quoted(self.id.as_str()));
        let _ = writeln!(out, "name = {}", quoted(&self.name));
        if let Some(created) = self.created {
            let _ = writeln!(out, "created = {}", quoted(&created.to_string()));
        }
        for profile in &self.profiles {
            let _ = write!(out, "\n[[profile]]\nname = {}\n", quoted(&profile.name));
            if let Some(added) = profile.added {
                let _ = writeln!(out, "added = {}", quoted(&added.to_string()));
            }
        }
        out
    }
}

/// What a `project.qcode` holds. Dates are declared as text: the document checks the type, and
/// the calendar is checked here, where a day that does not exist is a warning rather than a
/// reason to lose the project.
fn shape() -> Shape {
    let profile = Shape::new().required("name", ValueKind::text()).optional("added", ValueKind::text());
    Shape::new()
        .required("id", ValueKind::text())
        .optional("name", ValueKind::text())
        .optional("created", ValueKind::text())
        .entries("profile", profile)
}

/// Reads the day under `key`, reporting a value that is not one and answering `None`.
fn date(table: &Table, key: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<Date> {
    match Date::parse(table.text(key)?) {
        Ok(date) => Some(date),
        Err(problem) => {
            let at = table.value_location(key).cloned();
            diagnostics.push(Diagnostic::warning(at, format!("`{key}` is not a day: {problem}; it is ignored")));
            None
        }
    }
}

/// Writes `text` as a TOML basic string.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "project.qcode";

    fn located(diagnostic: &Diagnostic) -> String {
        diagnostic.location.as_ref().map_or_else(|| "nowhere".to_owned(), ToString::to_string)
    }

    #[test]
    fn a_written_file_reads_back_the_same() {
        let mut file = ProjectFile::new(
            ProjectId::from_display_name("Görünen ad").expect("usable"),
            "Görünen ad",
            Date::new(2026, 9, 17).expect("a real date"),
        );
        file.profiles.push(ProjectProfile { name: "claude-sub".to_owned(), added: Date::new(2026, 9, 17) });
        let text = file.to_toml();
        assert_eq!(
            text,
            "id = \"gorunen-ad\"\nname = \"Görünen ad\"\ncreated = \"2026-09-17\"\n\n[[profile]]\nname = \"claude-sub\"\nadded = \"2026-09-17\"\n"
        );
        let read = ProjectFile::parse(FILE, &text);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(file));
    }

    #[test]
    fn a_broken_profile_entry_is_skipped_and_the_rest_is_used() {
        let text = "id = \"proje\"\nname = \"Proje\"\ncreated = \"2026-09-17\"\n\n\
                    [[profile]]\nadded = \"2026-09-17\"\n\n\
                    [[profile]]\nname = \"opencode\"\nadded = \"2026-09-18\"\n";
        let read = ProjectFile::parse(FILE, text);
        let file = read.value.expect("the project is still usable");
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.profiles[0].name, "opencode");
        assert_eq!(read.diagnostics.len(), 1);
        assert_eq!(located(&read.diagnostics[0]), "project.qcode:5:1");
    }

    #[test]
    fn a_missing_or_unusable_id_makes_the_project_unreadable() {
        let read = ProjectFile::parse(FILE, "name = \"Proje\"\n");
        assert_eq!(read.value, None);
        assert_eq!(read.diagnostics.len(), 1);

        // An id that is text but no project id is qcode's own finding, reported at the key.
        let read = ProjectFile::parse(FILE, "id = \"Proje\"\nname = \"Proje\"\n");
        assert_eq!(read.value, None);
        assert_eq!(read.diagnostics.len(), 1);
        assert_eq!(located(&read.diagnostics[0]), "project.qcode:1:6");
    }

    #[test]
    fn a_missing_name_falls_back_to_the_id_with_a_warning() {
        let read = ProjectFile::parse(FILE, "id = \"proje\"\n");
        let file = read.value.expect("usable");
        assert_eq!(file.name, "proje");
        assert_eq!(file.created, None);
        assert_eq!(read.diagnostics.len(), 1, "only the name is reported; a missing date is normal");
    }

    #[test]
    fn a_broken_date_is_a_warning_and_leaves_the_project_usable() {
        let text = "id = \"proje\"\nname = \"Proje\"\ncreated = \"2026-13-40\"\n";
        let read = ProjectFile::parse(FILE, text);
        assert_eq!(read.value.expect("usable").created, None);
        assert_eq!(read.diagnostics.len(), 1);
        assert_eq!(located(&read.diagnostics[0]), "project.qcode:3:11", "the key that holds the bad day");
        assert!(read.diagnostics[0].message.ends_with("; it is ignored"), "{}", read.diagnostics[0].message);
    }

    #[test]
    fn a_syntax_error_points_at_the_line_and_column() {
        let read = ProjectFile::parse(FILE, "id = \"proje\"\nname = \n");
        assert!(read.diagnostics.iter().any(|d| located(d).starts_with("project.qcode:2:")), "{:?}", read.diagnostics);
        // The line that parses is still used; only the broken one is lost.
        assert_eq!(read.value.expect("usable").id.as_str(), "proje");

        let read = ProjectFile::parse(FILE, "name = \"Proje\"\nid\n");
        assert!(read.diagnostics.iter().any(|d| located(d).starts_with("project.qcode:2:")), "{:?}", read.diagnostics);
        assert_eq!(read.value, None, "without a readable id there is no project");
    }

    #[test]
    fn a_wrongly_typed_value_is_reported_and_not_guessed() {
        let read = ProjectFile::parse(FILE, "id = 7\nname = \"Proje\"\n");
        assert_eq!(read.value, None);
        assert_eq!(located(&read.diagnostics[0]), "project.qcode:1:6");

        let read = ProjectFile::parse(FILE, "id = \"proje\"\nname = \"Proje\"\nprofile = 3\n");
        assert!(read.value.expect("usable").profiles.is_empty());
        assert_eq!(located(&read.diagnostics[0]), "project.qcode:3:11");
    }

    #[test]
    fn an_unknown_key_is_reported_and_left_alone() {
        let read = ProjectFile::parse(FILE, "id = \"proje\"\nname = \"Proje\"\ncolour = \"red\"\n");
        assert!(read.value.is_some());
        assert_eq!(located(&read.diagnostics[0]), "project.qcode:3:1");
    }
}
