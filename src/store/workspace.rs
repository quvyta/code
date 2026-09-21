//! `workspace.qcode`: what a workspace is called, when it was made, which profiles it carries, what
//! its backup leaves out and whether it takes the assets too.
//!
//! The file is a [`Document`](qframe::document::Document): the framework checks the shape and
//! reports every problem with its line and column, and what only qcode can judge — whether the
//! id is a workspace id, whether a date is a day the calendar has — is checked here. A broken
//! entry is skipped, and everything that is readable is still used.

use std::fmt::Write as _;

use qframe::date::Date;
use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, Table, ValueKind};

use super::{Loaded, WorkspaceId};
use crate::backup::{Place, PlaceProblem};

/// One profile a workspace carries, as `workspace.qcode` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceProfile {
    /// The profile's name, which is also the name of its folder below `Containers/Harness`.
    pub name: String,
    /// The day the profile was added, when the file records a readable one.
    pub added: Option<Date>,
}

/// The contents of a workspace's `workspace.qcode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFile {
    /// The workspace's identifier, which is also the name of its folder.
    pub id: WorkspaceId,
    /// The name the user gave it, in the user's own writing.
    pub name: String,
    /// The day the workspace was made, when the file records a readable one.
    pub created: Option<Date>,
    /// The profiles the workspace carries, in the order the file lists them.
    pub profiles: Vec<WorkspaceProfile>,
    /// The folders and files inside `Work/` its backup leaves out, each a path inside the
    /// workspace written with `/`, in the order the file lists them.
    pub backup_skip: Vec<String>,
    /// Whether its backup takes `Assets/` too. Off unless the file says so: assets are often
    /// large and kept elsewhere as well.
    pub backup_assets: bool,
}

impl WorkspaceFile {
    /// A workspace made today with no profiles yet.
    #[must_use]
    pub fn new(id: WorkspaceId, name: impl Into<String>, created: Date) -> Self {
        Self {
            id,
            name: name.into(),
            created: Some(created),
            profiles: Vec::new(),
            backup_skip: Vec::new(),
            backup_assets: false,
        }
    }

    /// Reads a `workspace.qcode`, reporting problems against `file`.
    ///
    /// The value is `None` only when the file names no workspace this program could open: the
    /// identifier is missing or is not one. Anything else — a missing name, an unreadable date,
    /// a profile without a name, a key nobody knows — is a diagnostic beside a usable workspace.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> Loaded<Option<Self>> {
        let document = Document::parse(file, text, &shape());
        let mut diagnostics = document.diagnostics().to_vec();
        let root = document.root();

        // A missing or wrongly typed `id` is already reported by the document; only an id that
        // is text and still no workspace id is qcode's own finding.
        let id = match root.text("id").map(|text| (text, WorkspaceId::parse(text))) {
            Some((_, Ok(id))) => Some(id),
            Some((_, Err(problem))) => {
                let at = root.value_location("id").cloned();
                diagnostics.push(Diagnostic::error(at, format!("`id` is not a workspace id: {problem:?}")));
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
                Some(WorkspaceProfile { name, added: date(entry, "added", &mut diagnostics) })
            })
            .collect();
        let backup_skip = skip_list(root, &mut diagnostics);
        let backup_assets = root.table("backup").and_then(|backup| backup.flag("assets")).unwrap_or(false);
        Loaded { value: Some(Self { id, name, created, profiles, backup_skip, backup_assets }), diagnostics }
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
        // The table's own key comes before its entries: TOML reads a key written after an
        // entry's header as the entry's.
        if self.backup_assets {
            out.push_str("\n[backup]\nassets = true\n");
        }
        for path in &self.backup_skip {
            let _ = write!(out, "\n[[backup.skip]]\npath = {}\n", quoted(path));
        }
        out
    }
}

/// What a `workspace.qcode` holds. Dates are declared as text: the document checks the type, and
/// the calendar is checked here, where a day that does not exist is a warning rather than a
/// reason to lose the workspace.
///
/// What the backup leaves out is a `[[backup.skip]]` entry per path rather than one list of
/// text: an entry has its own place in the file, so a path that is not one is reported at its
/// own line and the others are still used.
fn shape() -> Shape {
    let profile = Shape::new().required("name", ValueKind::text()).optional("added", ValueKind::text());
    let backup = Shape::new()
        .optional("assets", ValueKind::flag())
        .entries("skip", Shape::new().required("path", ValueKind::text()));
    Shape::new()
        .required("id", ValueKind::text())
        .optional("name", ValueKind::text())
        .optional("created", ValueKind::text())
        .entries("profile", profile)
        .table("backup", backup)
}

/// The paths the backup leaves out, each once, as the backup names them. One that is not a path
/// inside the workspace is reported and skipped: handed to the backup it would stop every round.
fn skip_list(root: &Table, diagnostics: &mut Vec<Diagnostic>) -> Vec<String> {
    let mut skip: Vec<String> = Vec::new();
    let entries = root.table("backup").map(|backup| backup.entries("skip")).unwrap_or_default();
    // An entry without a path was reported by the document as missing its required key.
    for (entry, path) in entries.iter().filter_map(|entry| Some((entry, entry.text("path")?))) {
        match Place::new(path) {
            Ok(place) if skip.iter().any(|known| known == place.as_str()) => {}
            Ok(place) => skip.push(place.as_str().to_owned()),
            Err(bad) => {
                let why = match bad.problem {
                    PlaceProblem::Empty => "it names nothing",
                    PlaceProblem::Absolute => "it starts outside the workspace",
                    PlaceProblem::Outside => "a `.` or `..` part leaves the workspace",
                    PlaceProblem::LineBreak => "it holds a line break",
                };
                let at = entry.value_location("path").cloned();
                let message =
                    format!("`backup.skip` path {} is not inside the workspace: {why}; it is ignored", quoted(path));
                diagnostics.push(Diagnostic::warning(at, message));
            }
        }
    }
    skip
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
pub(super) fn quoted(text: &str) -> String {
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

    const FILE: &str = "workspace.qcode";

    fn located(diagnostic: &Diagnostic) -> String {
        diagnostic.location.as_ref().map_or_else(|| "nowhere".to_owned(), ToString::to_string)
    }

    #[test]
    fn a_written_file_reads_back_the_same() {
        let mut file = WorkspaceFile::new(
            WorkspaceId::from_display_name("Görünen ad").expect("usable"),
            "Görünen ad",
            Date::new(2026, 9, 17).expect("a real date"),
        );
        file.profiles.push(WorkspaceProfile { name: "claude-sub".to_owned(), added: Date::new(2026, 9, 17) });
        let text = file.to_toml();
        assert_eq!(
            text,
            "id = \"gorunen-ad\"\nname = \"Görünen ad\"\ncreated = \"2026-09-17\"\n\n[[profile]]\nname = \"claude-sub\"\nadded = \"2026-09-17\"\n"
        );
        let read = WorkspaceFile::parse(FILE, &text);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(file));
    }

    #[test]
    fn a_broken_profile_entry_is_skipped_and_the_rest_is_used() {
        let text = "id = \"proje\"\nname = \"Proje\"\ncreated = \"2026-09-17\"\n\n\
                    [[profile]]\nadded = \"2026-09-17\"\n\n\
                    [[profile]]\nname = \"opencode\"\nadded = \"2026-09-18\"\n";
        let read = WorkspaceFile::parse(FILE, text);
        let file = read.value.expect("the workspace is still usable");
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.profiles[0].name, "opencode");
        assert_eq!(read.diagnostics.len(), 1);
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:5:1");
    }

    #[test]
    fn a_missing_or_unusable_id_makes_the_workspace_unreadable() {
        let read = WorkspaceFile::parse(FILE, "name = \"Proje\"\n");
        assert_eq!(read.value, None);
        assert_eq!(read.diagnostics.len(), 1);

        // An id that is text but no workspace id is qcode's own finding, reported at the key.
        let read = WorkspaceFile::parse(FILE, "id = \"Proje\"\nname = \"Proje\"\n");
        assert_eq!(read.value, None);
        assert_eq!(read.diagnostics.len(), 1);
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:1:6");
    }

    #[test]
    fn a_missing_name_falls_back_to_the_id_with_a_warning() {
        let read = WorkspaceFile::parse(FILE, "id = \"proje\"\n");
        let file = read.value.expect("usable");
        assert_eq!(file.name, "proje");
        assert_eq!(file.created, None);
        assert_eq!(read.diagnostics.len(), 1, "only the name is reported; a missing date is normal");
    }

    #[test]
    fn a_broken_date_is_a_warning_and_leaves_the_workspace_usable() {
        let text = "id = \"proje\"\nname = \"Proje\"\ncreated = \"2026-13-40\"\n";
        let read = WorkspaceFile::parse(FILE, text);
        assert_eq!(read.value.expect("usable").created, None);
        assert_eq!(read.diagnostics.len(), 1);
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:3:11", "the key that holds the bad day");
        assert!(read.diagnostics[0].message.ends_with("; it is ignored"), "{}", read.diagnostics[0].message);
    }

    #[test]
    fn a_syntax_error_points_at_the_line_and_column() {
        let read = WorkspaceFile::parse(FILE, "id = \"proje\"\nname = \n");
        assert!(
            read.diagnostics.iter().any(|d| located(d).starts_with("workspace.qcode:2:")),
            "{:?}",
            read.diagnostics
        );
        // The line that parses is still used; only the broken one is lost.
        assert_eq!(read.value.expect("usable").id.as_str(), "proje");

        let read = WorkspaceFile::parse(FILE, "name = \"Proje\"\nid\n");
        assert!(
            read.diagnostics.iter().any(|d| located(d).starts_with("workspace.qcode:2:")),
            "{:?}",
            read.diagnostics
        );
        assert_eq!(read.value, None, "without a readable id there is no workspace");
    }

    #[test]
    fn a_wrongly_typed_value_is_reported_and_not_guessed() {
        let read = WorkspaceFile::parse(FILE, "id = 7\nname = \"Proje\"\n");
        assert_eq!(read.value, None);
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:1:6");

        let read = WorkspaceFile::parse(FILE, "id = \"proje\"\nname = \"Proje\"\nprofile = 3\n");
        assert!(read.value.expect("usable").profiles.is_empty());
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:3:11");
    }

    #[test]
    fn what_the_backup_leaves_out_reads_back_the_same_and_is_not_written_when_empty() {
        let mut file = WorkspaceFile::new(
            WorkspaceId::parse("proje").expect("an id"),
            "Proje",
            Date::new(2026, 9, 19).expect("a day"),
        );
        assert!(!file.to_toml().contains("backup"), "nothing left out, nothing written");
        file.backup_skip = vec!["data".to_owned(), "out/big file".to_owned()];
        let text = file.to_toml();
        assert!(
            text.ends_with("\n[[backup.skip]]\npath = \"data\"\n\n[[backup.skip]]\npath = \"out/big file\"\n"),
            "{text}"
        );
        let read = WorkspaceFile::parse(FILE, &text);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(file));
    }

    #[test]
    fn the_assets_switch_is_written_only_when_on_and_reads_back_beside_the_skip_list() {
        let mut file = WorkspaceFile::new(
            WorkspaceId::parse("proje").expect("an id"),
            "Proje",
            Date::new(2026, 9, 19).expect("a day"),
        );
        file.backup_skip = vec!["data".to_owned()];
        assert!(!file.to_toml().contains("assets"), "off is the default and is not written");
        file.backup_assets = true;
        let text = file.to_toml();
        assert!(text.contains("\n[backup]\nassets = true\n\n[[backup.skip]]\npath = \"data\"\n"), "{text}");
        let read = WorkspaceFile::parse(FILE, &text);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(file));

        let read = WorkspaceFile::parse(FILE, "id = \"proje\"\n\n[backup]\nassets = \"yes\"\n");
        assert!(!read.value.expect("usable").backup_assets, "a value that is not a flag is not guessed at");
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:4:10");
    }

    #[test]
    fn a_left_out_path_that_is_not_inside_the_workspace_is_reported_and_the_rest_is_used() {
        let text = "id = \"proje\"\nname = \"Proje\"\n\n\
                    [[backup.skip]]\npath = \"../home\"\n\n\
                    [[backup.skip]]\npath = \"data/\"\n\n\
                    [[backup.skip]]\npath = \"data\"\n\n\
                    [[backup.skip]]\npath = \"/etc\"\n\n\
                    [[backup.skip]]\n";
        let read = WorkspaceFile::parse(FILE, text);
        assert_eq!(read.value.expect("usable").backup_skip, ["data"], "written as the backup names it, once");
        let mut places: Vec<String> = read.diagnostics.iter().map(located).collect();
        places.sort();
        assert_eq!(
            places,
            ["workspace.qcode:14:8", "workspace.qcode:16:1", "workspace.qcode:5:8"],
            "{:?}",
            read.diagnostics
        );
        let climbing = read.diagnostics.iter().find(|d| located(d) == "workspace.qcode:5:8").expect("reported");
        assert!(climbing.message.contains("leaves the workspace"), "{}", climbing.message);
    }

    #[test]
    fn an_unknown_key_is_reported_and_left_alone() {
        let read = WorkspaceFile::parse(FILE, "id = \"proje\"\nname = \"Proje\"\ncolour = \"red\"\n");
        assert!(read.value.is_some());
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:3:1");
    }
}
