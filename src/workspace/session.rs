//! `session.toml`: which projects were open, in what order, and the tabs of each, so the home
//! screen's "Continue" can put the person back where they left off, the way a browser brings
//! back its windows and their tabs.
//!
//! The file is state, not a setting: it lives in the application's data folder, beside nothing
//! the person edits, and it is rewritten whenever what is open changes. It is still read the way
//! every file of the family is read — as a [`Document`], so a broken file is reported with its
//! line and column, what can be read is used and nothing ever panics.
//!
//! ```toml
//! active = "firefly"
//!
//! [[project]]
//! id = "firefly"
//! active-tab = 1
//!
//! [[project.tab]]
//! kind = "shell"
//! opened = 1758196800
//!
//! [[project.tab]]
//! kind = "profile"
//! profile = "claude-sub"
//! conversation = "4f1c…"
//! opened = 1758197000
//!
//! [[project.tab]]
//! kind = "new"
//! opened = 1758197300
//!
//! [[project.tab]]
//! kind = "markdown"
//! file = "guide/harbour.md"
//! opened = 1758197400
//! ```
//!
//! A tab that opens one of the project's files — `image`, `markdown`, `editor`, `pdf`, `office` or
//! `sound` — names the file
//! by its path inside `Project/`, written with `/`.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, Table, ValueKind};
use qframe::storage::{atomic_write, data_dir};

use super::project::quoted;
use super::{Loaded, ProjectId};

/// The folder of the application's own data, named the way its settings folder is.
const APP: &str = "quvyta/code";

/// The name of the session file in that folder.
const FILE: &str = "session.toml";

/// What a tab of a saved session opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTabKind {
    /// A shell in the project's own container.
    Shell,
    /// The harness of the profile of this name.
    Profile(String),
    /// A blank tab, still asking what it should open.
    New,
    /// A picture of the project, drawn in the project's own container, by its path inside
    /// `Project/`.
    Image(String),
    /// A Markdown document of the project, shown by QCode itself.
    Markdown(String),
    /// A file of the project, open in the chosen editor in the project's own container.
    Editor(String),
    /// A PDF of the project, its text shown by QCode and its pages drawn in the project's own
    /// container.
    Pdf(String),
    /// A word processor's document of the project, its text shown by QCode.
    Office(String),
    /// A sound of the project, played in a container of its own or described.
    Sound(String),
    /// The window of the desktop harness of the profile of this name.
    Desktop(String),
}

/// One tab of a saved session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTab {
    /// What the tab opens.
    pub kind: SessionTabKind,
    /// The harness conversation the tab was showing, when it is known.
    pub conversation: Option<String>,
    /// When the tab was opened, in seconds since the Unix epoch.
    pub opened: u64,
}

/// One project of a saved session, with its tabs in the order they were shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProject {
    /// The project.
    pub id: ProjectId,
    /// The position of the tab that was open.
    pub active_tab: usize,
    /// The tabs, in order.
    pub tabs: Vec<SessionTab>,
}

/// The projects that were open, in the order the rail showed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    /// The project the rail had open, when there was one.
    pub active: Option<ProjectId>,
    /// The projects, in rail order.
    pub projects: Vec<SessionProject>,
}

impl Session {
    /// Where the session of this user is kept: `session.toml` in QCode's data folder, or `None`
    /// on a machine that names no home, where the session is not kept at all.
    #[must_use]
    pub fn file() -> Option<PathBuf> {
        data_dir(APP).map(|dir| dir.join(FILE))
    }

    /// Whether the session has no project to go back to.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }

    /// The position of the project that was open, which is the first one when the file names
    /// none or names one it does not list.
    #[must_use]
    pub fn active_index(&self) -> usize {
        self.active.as_ref().and_then(|id| self.projects.iter().position(|project| project.id == *id)).unwrap_or(0)
    }

    /// Reads the session file at `path`.
    ///
    /// A file that is not there is no session and no problem: nothing was saved yet. A file that
    /// cannot be read is no session either, with a diagnostic that says why.
    #[must_use]
    pub fn load(path: &Path) -> Loaded<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or(FILE);
                let read = Self::parse(name, &text);
                Loaded { value: Some(read.value), diagnostics: read.diagnostics }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Loaded { value: None, diagnostics: Vec::new() },
            Err(error) => Loaded {
                value: None,
                diagnostics: vec![Diagnostic::error(None, format!("{} could not be read: {error}", path.display()))],
            },
        }
    }

    /// Reads a session from `text`, reporting problems against `file`.
    ///
    /// A project whose id is unusable and a tab that does not say what it opens are skipped with
    /// a diagnostic; everything else of the file is kept.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> Loaded<Self> {
        let document = Document::parse(file, text, &shape());
        let mut diagnostics = document.diagnostics().to_vec();
        let root = document.root();
        let active = root.text("active").and_then(|text| id(root, "active", text, &mut diagnostics));
        let projects = root.entries("project").iter().filter_map(|entry| project(entry, &mut diagnostics)).collect();
        Loaded { value: Self { active, projects }, diagnostics }
    }

    /// The file as it is written to disk.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        if let Some(active) = &self.active {
            let _ = writeln!(out, "active = {}", quoted(active.as_str()));
        }
        for project in &self.projects {
            let _ = write!(
                out,
                "\n[[project]]\nid = {}\nactive-tab = {}\n",
                quoted(project.id.as_str()),
                project.active_tab
            );
            for tab in &project.tabs {
                out.push_str("\n[[project.tab]]\n");
                match &tab.kind {
                    SessionTabKind::Shell => out.push_str("kind = \"shell\"\n"),
                    SessionTabKind::New => out.push_str("kind = \"new\"\n"),
                    SessionTabKind::Profile(name) => {
                        let _ = write!(out, "kind = \"profile\"\nprofile = {}\n", quoted(name));
                    }
                    SessionTabKind::Image(file) => {
                        let _ = write!(out, "kind = \"image\"\nfile = {}\n", quoted(file));
                    }
                    SessionTabKind::Markdown(file) => {
                        let _ = write!(out, "kind = \"markdown\"\nfile = {}\n", quoted(file));
                    }
                    SessionTabKind::Editor(file) => {
                        let _ = write!(out, "kind = \"editor\"\nfile = {}\n", quoted(file));
                    }
                    SessionTabKind::Pdf(file) => {
                        let _ = write!(out, "kind = \"pdf\"\nfile = {}\n", quoted(file));
                    }
                    SessionTabKind::Office(file) => {
                        let _ = write!(out, "kind = \"office\"\nfile = {}\n", quoted(file));
                    }
                    SessionTabKind::Sound(file) => {
                        let _ = write!(out, "kind = \"sound\"\nfile = {}\n", quoted(file));
                    }
                    SessionTabKind::Desktop(name) => {
                        let _ = write!(out, "kind = \"desktop\"\nprofile = {}\n", quoted(name));
                    }
                }
                if let Some(conversation) = &tab.conversation {
                    let _ = writeln!(out, "conversation = {}", quoted(conversation));
                }
                let _ = writeln!(out, "opened = {}", tab.opened);
            }
        }
        out
    }

    /// Writes the session to `path` atomically, making its folder first when it is not there.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the folder or the file cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        atomic_write(path, self.to_toml().as_bytes())
    }
}

/// What a `session.toml` holds.
fn shape() -> Shape {
    let tab = Shape::new()
        .required(
            "kind",
            ValueKind::choice(["shell", "profile", "new", "image", "markdown", "editor", "pdf", "office", "sound"]),
        )
        .optional("profile", ValueKind::text())
        .optional("file", ValueKind::text())
        .optional("conversation", ValueKind::text())
        .required("opened", ValueKind::integer());
    let project =
        Shape::new().required("id", ValueKind::text()).optional("active-tab", ValueKind::integer()).entries("tab", tab);
    Shape::new().optional("active", ValueKind::text()).entries("project", project)
}

/// Reads the project id under `key`, reporting text that is not one.
fn id(table: &Table, key: &str, text: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<ProjectId> {
    match ProjectId::parse(text) {
        Ok(id) => Some(id),
        Err(problem) => {
            let at = table.value_location(key).cloned();
            diagnostics
                .push(Diagnostic::warning(at, format!("`{key}` is not a project id: {problem:?}; it is skipped")));
            None
        }
    }
}

/// One `[[project]]` entry, or `None` when it names no project.
fn project(entry: &Table, diagnostics: &mut Vec<Diagnostic>) -> Option<SessionProject> {
    // A missing id was reported by the document as a required key; only a bad one is ours.
    let id = id(entry, "id", entry.text("id")?, diagnostics)?;
    let tabs: Vec<SessionTab> = entry.entries("tab").iter().filter_map(|tab| session_tab(tab, diagnostics)).collect();
    let active_tab = match entry.integer("active-tab").map(usize::try_from) {
        Some(Ok(index)) if index < tabs.len() || tabs.is_empty() && index == 0 => index,
        Some(_) => {
            let at = entry.value_location("active-tab").cloned();
            diagnostics.push(Diagnostic::warning(at, "`active-tab` names no tab; the first one is open"));
            0
        }
        None => 0,
    };
    Some(SessionProject { id, active_tab, tabs })
}

/// One `[[project.tab]]` entry, or `None` when it does not say what it opens.
fn session_tab(entry: &Table, diagnostics: &mut Vec<Diagnostic>) -> Option<SessionTab> {
    let kind = match entry.text("kind")? {
        "shell" => SessionTabKind::Shell,
        "new" => SessionTabKind::New,
        kind @ ("image" | "markdown" | "editor" | "pdf" | "office" | "sound") => {
            let Some(file) = entry.text("file") else {
                let at = entry.value_location("kind").cloned();
                diagnostics.push(Diagnostic::warning(at, format!("an {kind} tab names no `file`; it is skipped")));
                return None;
            };
            let file = file.to_owned();
            match kind {
                "image" => SessionTabKind::Image(file),
                "markdown" => SessionTabKind::Markdown(file),
                "pdf" => SessionTabKind::Pdf(file),
                "office" => SessionTabKind::Office(file),
                "sound" => SessionTabKind::Sound(file),
                _ => SessionTabKind::Editor(file),
            }
        }
        // A window tab and a harness tab are both a profile's; an unknown kind that names a
        // profile is read as a harness tab, as it was before windows existed.
        kind => match entry.text("profile") {
            Some(name) if kind == "desktop" => SessionTabKind::Desktop(name.to_owned()),
            Some(name) => SessionTabKind::Profile(name.to_owned()),
            None => {
                let at = entry.value_location("kind").cloned();
                diagnostics.push(Diagnostic::warning(at, "a profile tab names no `profile`; it is skipped"));
                return None;
            }
        },
    };
    // A missing time was reported by the document; the tab is still worth bringing back.
    let opened = match entry.integer("opened").map(u64::try_from) {
        Some(Ok(seconds)) => seconds,
        Some(Err(_)) => {
            let at = entry.value_location("opened").cloned();
            diagnostics.push(Diagnostic::warning(at, "`opened` is before 1970; it is read as unknown"));
            0
        }
        None => 0,
    };
    Some(SessionTab { kind, conversation: entry.text("conversation").map(str::to_owned), opened })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "session.toml";

    fn located(diagnostic: &Diagnostic) -> String {
        diagnostic.location.as_ref().map_or_else(|| "nowhere".to_owned(), ToString::to_string)
    }

    fn id(text: &str) -> ProjectId {
        ProjectId::parse(text).expect("a usable project id")
    }

    fn sample() -> Session {
        Session {
            active: Some(id("moth")),
            projects: vec![
                SessionProject {
                    id: id("firefly"),
                    active_tab: 1,
                    tabs: vec![
                        SessionTab { kind: SessionTabKind::Shell, conversation: None, opened: 1_758_196_800 },
                        SessionTab {
                            kind: SessionTabKind::Profile("claude-sub".to_owned()),
                            conversation: Some("4f1c \"quoted\"".to_owned()),
                            opened: 1_758_197_000,
                        },
                        SessionTab { kind: SessionTabKind::New, conversation: None, opened: 1_758_197_300 },
                        SessionTab {
                            kind: SessionTabKind::Image("art/logo \"one\".png".to_owned()),
                            conversation: None,
                            opened: 1_758_197_400,
                        },
                        SessionTab {
                            kind: SessionTabKind::Markdown("guide/harbour.md".to_owned()),
                            conversation: None,
                            opened: 1_758_197_500,
                        },
                        SessionTab {
                            kind: SessionTabKind::Editor("src/main.rs".to_owned()),
                            conversation: None,
                            opened: 1_758_197_600,
                        },
                        SessionTab {
                            kind: SessionTabKind::Pdf("papers/tide tables.pdf".to_owned()),
                            conversation: None,
                            opened: 1_758_197_700,
                        },
                        SessionTab {
                            kind: SessionTabKind::Office("letters/to the harbour master.docx".to_owned()),
                            conversation: None,
                            opened: 1_758_197_800,
                        },
                        SessionTab {
                            kind: SessionTabKind::Sound("sounds/foghorn.ogg".to_owned()),
                            conversation: None,
                            opened: 1_758_197_900,
                        },
                    ],
                },
                SessionProject { id: id("moth"), active_tab: 0, tabs: Vec::new() },
            ],
        }
    }

    #[test]
    fn a_written_session_reads_back_the_same() {
        let session = sample();
        let text = session.to_toml();
        let read = Session::parse(NAME, &text);
        assert!(read.is_clean(), "{:?}\n{text}", read.diagnostics);
        assert_eq!(read.value, session);
        assert_eq!(read.value.active_index(), 1);
    }

    #[test]
    fn saving_makes_the_folder_and_loading_reads_it_back() {
        let dir = std::env::temp_dir().join(format!("qcode-session-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("deeper").join(NAME);
        assert_eq!(Session::load(&path), Loaded { value: None, diagnostics: Vec::new() }, "no file is no session");
        sample().save(&path).expect("the session is written");
        let read = Session::load(&path);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(sample()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_file_is_no_session_and_says_why() {
        let dir = std::env::temp_dir().join(format!("qcode-session-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        // A folder where the file should be cannot be read as text.
        let read = Session::load(&dir);
        assert_eq!(read.value, None);
        assert_eq!(read.diagnostics.len(), 1, "{:?}", read.diagnostics);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_syntax_error_points_at_its_line_and_the_rest_is_kept() {
        let text = "active = \"firefly\"\n\n[[project]]\nid = \"firefly\"\nactive-tab = \n\n\
                    [[project.tab]]\nkind = \"shell\"\nopened = 5\n";
        let read = Session::parse(NAME, text);
        assert!(read.diagnostics.iter().any(|d| located(d).starts_with("session.toml:5:")), "{:?}", read.diagnostics);
        assert_eq!(read.value.projects.len(), 1);
        assert_eq!(read.value.projects[0].tabs.len(), 1);
    }

    #[test]
    fn unknown_and_broken_entries_are_skipped_with_their_place() {
        let text = "colour = \"red\"\n\n\
                    [[project]]\nid = \"Not An Id\"\n\n\
                    [[project]]\nid = \"moth\"\nactive-tab = 9\n\n\
                    [[project.tab]]\nkind = \"browser\"\nopened = 1\n\n\
                    [[project.tab]]\nkind = \"profile\"\nopened = 2\n\n\
                    [[project.tab]]\nkind = \"profile\"\nprofile = \"opencode\"\nopened = -4\n";
        let read = Session::parse(NAME, text);
        let places: Vec<String> = read.diagnostics.iter().map(located).collect();
        for place in ["session.toml:1:1", "session.toml:4:6", "session.toml:8:14", "session.toml:11:8"] {
            assert!(places.iter().any(|at| at == place), "{place} in {places:?}");
        }
        let session = read.value;
        assert_eq!(session.projects.len(), 1, "the project with a bad id is skipped");
        let moth = &session.projects[0];
        assert_eq!(moth.tabs.len(), 1, "an unknown kind and a profile tab without a profile are skipped");
        assert_eq!(moth.tabs[0].kind, SessionTabKind::Profile("opencode".to_owned()));
        assert_eq!(moth.tabs[0].opened, 0, "a time before the epoch is unknown");
        assert_eq!(moth.active_tab, 0, "an active tab past the end falls back to the first");
    }

    #[test]
    fn a_file_tab_without_its_file_is_skipped_with_its_place() {
        let text = "[[project]]\nid = \"moth\"\n\n\
                    [[project.tab]]\nkind = \"markdown\"\nopened = 1\n\n\
                    [[project.tab]]\nkind = \"editor\"\nfile = \"notes.txt\"\nopened = 2\n";
        let read = Session::parse(NAME, text);
        let places: Vec<String> = read.diagnostics.iter().map(located).collect();
        assert_eq!(places, ["session.toml:5:8"], "{:?}", read.diagnostics);
        let tabs = &read.value.projects[0].tabs;
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].kind, SessionTabKind::Editor("notes.txt".to_owned()));
    }

    #[test]
    fn garbage_never_panics() {
        for text in
            ["", "\u{0}", "[[project]]\n[[project.tab]]\n", "project = 3\n", "[[project]]]]\nid=\n", "active = 7"]
        {
            let read = Session::parse(NAME, text);
            assert!(read.value.projects.iter().all(|project| project.active_tab <= project.tabs.len()));
        }
    }

    #[test]
    fn an_active_project_that_is_not_listed_opens_the_first() {
        let mut session = sample();
        session.active = Some(id("gone"));
        assert_eq!(session.active_index(), 0);
        session.active = None;
        assert_eq!(session.active_index(), 0);
    }
}
