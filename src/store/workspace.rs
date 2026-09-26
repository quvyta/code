//! `workspace.qcode`: what a workspace is called, when it was made, which folder it works in when
//! that is not its own `Work/`, which profiles it carries, what its backup leaves out and whether it
//! takes the assets too.
//!
//! The file is a [`Document`](qframe::document::Document): the framework checks the shape and
//! reports every problem with its line and column, and what only qcode can judge — whether the
//! id is a workspace id, whether a date is a day the calendar has — is checked here. A broken
//! entry is skipped, and everything that is readable is still used.

use std::fmt::Write as _;
use std::path::PathBuf;

use qframe::date::Date;
use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, Table, ValueKind};
use qframe::t;

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
    /// The person's own folder the workspace works in where it stands, when it was made that way:
    /// an absolute path on this machine, mounted as the containers' work folder in place of the
    /// workspace's own `Work/`. `None` is a workspace with a `Work/` of its own, which every file
    /// written before the choice existed is.
    pub folder: Option<PathBuf>,
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
            folder: None,
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
                diagnostics.push(Diagnostic::error(at, t!("workspaces.file.not-an-id", problem = problem.said())));
                None
            }
            None => None,
        };
        let Some(id) = id else {
            return Loaded { value: None, diagnostics };
        };
        let name = root.text("name").map_or_else(
            || {
                diagnostics.push(Diagnostic::warning(None, t!("workspaces.file.no-name")));
                id.as_str().to_owned()
            },
            str::to_owned,
        );
        let created = date(root, "created", &mut diagnostics);
        let folder = folder(root, &mut diagnostics);
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
        Loaded { value: Some(Self { id, name, created, folder, profiles, backup_skip, backup_assets }), diagnostics }
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
        // Written only for a workspace that has one, so a file of a workspace with its own `Work/`
        // reads the same byte for byte as it did before the choice existed.
        if let Some(folder) = &self.folder {
            let _ = writeln!(out, "folder = {}", quoted(&folder.to_string_lossy()));
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
        .optional("folder", ValueKind::text())
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
                let at = entry.value_location("path").cloned();
                let message = t!("workspaces.file.skip", path = quoted(path), why = outside(bad.problem));
                diagnostics.push(Diagnostic::warning(at, message));
            }
        }
    }
    skip
}

/// The folder the workspace works in where it stands, when the file names one. A path that is not
/// absolute is reported and not used: relative to what, would be a guess, and a guess here decides
/// which of the person's folders the agents change.
fn folder(root: &Table, diagnostics: &mut Vec<Diagnostic>) -> Option<PathBuf> {
    let text = root.text("folder")?;
    let path = PathBuf::from(text);
    if path.is_absolute() {
        return Some(path);
    }
    let at = root.value_location("folder").cloned();
    let message = t!("workspaces.file.folder", path = quoted(text));
    diagnostics.push(Diagnostic::warning(at, message));
    None
}

/// Reads the day under `key`, reporting a value that is not one and answering `None`.
fn date(table: &Table, key: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<Date> {
    match Date::parse(table.text(key)?) {
        Ok(date) => Some(date),
        Err(problem) => {
            let at = table.value_location(key).cloned();
            let message = t!("workspaces.file.day", key = key, problem = problem.to_string());
            diagnostics.push(Diagnostic::warning(at, message));
            None
        }
    }
}

/// Why a path is not one inside the workspace, in the active language.
pub(super) fn outside(problem: PlaceProblem) -> String {
    match problem {
        PlaceProblem::Empty => t!("workspaces.file.names-nothing"),
        PlaceProblem::Absolute => t!("workspaces.file.absolute"),
        PlaceProblem::Outside => t!("workspaces.file.climbs"),
        PlaceProblem::LineBreak => t!("workspaces.file.line-break"),
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
        let read = crate::ui::in_language("en", || WorkspaceFile::parse(FILE, text));
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
        let read = crate::ui::in_language("en", || WorkspaceFile::parse(FILE, text));
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
    fn a_folder_used_in_place_reads_back_the_same_and_a_file_without_one_reads_as_before() {
        let mut file = WorkspaceFile::new(
            WorkspaceId::parse("proje").expect("an id"),
            "Proje",
            Date::new(2026, 9, 25).expect("a day"),
        );
        let before = file.to_toml();
        assert_eq!(before, "id = \"proje\"\nname = \"Proje\"\ncreated = \"2026-09-25\"\n");
        let read = WorkspaceFile::parse(FILE, &before);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value.as_ref().and_then(|file| file.folder.clone()), None, "an older file is a copy");
        assert_eq!(read.value.expect("usable").to_toml(), before, "and is written back byte for byte");

        file.folder = Some(PathBuf::from("/home/kişi/kod \"proje\""));
        file.profiles.push(WorkspaceProfile { name: "claude".to_owned(), added: None });
        let text = file.to_toml();
        assert!(text.contains("\nfolder = \"/home/kişi/kod \\\"proje\\\"\"\n\n[[profile]]"), "{text}");
        let read = WorkspaceFile::parse(FILE, &text);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(file));
    }

    #[test]
    fn a_folder_that_is_not_absolute_is_reported_and_not_guessed_at() {
        let read =
            crate::ui::in_language("en", || WorkspaceFile::parse(FILE, "id = \"proje\"\nfolder = \"kod/proje\"\n"));
        assert_eq!(read.value.expect("usable").folder, None);
        let said = read.diagnostics.iter().find(|d| d.message.contains("`folder`")).expect("the folder is reported");
        assert_eq!(located(said), "workspace.qcode:2:10");
    }

    #[test]
    fn what_is_wrong_with_a_workspace_file_is_said_in_the_active_language() {
        let text = "id = \"proje\"\nfolder = \"kod/proje\"\ncreated = \"2026-13-40\"\n\n\
                    [[backup.skip]]\npath = \"\"\n\n\
                    [[backup.skip]]\npath = \"/etc\"\n\n\
                    [[backup.skip]]\npath = \"../ev\"\n\n\
                    [[backup.skip]]\npath = \"a\\nb\"\n";
        let read = crate::ui::in_language("tr", || WorkspaceFile::parse(FILE, text));
        let said: Vec<&str> = read.diagnostics.iter().map(|problem| problem.message.as_str()).collect();
        for expected in [
            "okunabilen bir `name` yok; onun yerine kimlik gösteriliyor",
            "`folder` \"kod/proje\" mutlak bir yol değil; yok sayılıyor",
            "`backup.skip` yolu \"\" çalışma alanının içinde değil: hiçbir şeyi adlandırmıyor; yok sayılıyor",
            "`backup.skip` yolu \"/etc\" çalışma alanının içinde değil: çalışma alanının dışında başlıyor; yok sayılıyor",
            "`backup.skip` yolu \"../ev\" çalışma alanının içinde değil: bir `.` ya da `..` parçası çalışma alanının \
             dışına çıkıyor; yok sayılıyor",
            "`backup.skip` yolu \"a\\nb\" çalışma alanının içinde değil: içinde bir satır sonu var; yok sayılıyor",
        ] {
            assert!(said.contains(&expected), "`{expected}` in {said:#?}");
        }
        assert!(
            said.iter().any(|message| message.starts_with("`created` bir gün değil: ") && message.ends_with("; yok sayılıyor")),
            "{said:#?}"
        );

        let read = crate::ui::in_language("tr", || WorkspaceFile::parse(FILE, "id = \"con\"\n"));
        let said: Vec<&str> = read.diagnostics.iter().map(|problem| problem.message.as_str()).collect();
        assert_eq!(
            said,
            ["`id` bir çalışma alanı kimliği değil: Bu ad Windows'ta bir aygıtın adı. Yanına bir kelime ekle."]
        );
    }

    #[test]
    fn an_unknown_key_is_reported_and_left_alone() {
        let read = WorkspaceFile::parse(FILE, "id = \"proje\"\nname = \"Proje\"\ncolour = \"red\"\n");
        assert!(read.value.is_some());
        assert_eq!(located(&read.diagnostics[0]), "workspace.qcode:3:1");
    }
}
