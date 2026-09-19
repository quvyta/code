//! The folder tree of the workspace: creating it, and reading it back.
//!
//! ```text
//! {workspace}/
//!   Profiles/
//!   Projects/
//!     <project>/
//!       project.qcode
//!       Project/
//!       Assets/
//!       Containers/Harness/<profile>/
//! ```
//!
//! `Backup/` and `Containers/MCP/` are not created here; they are born with the features that
//! need them. Nothing in this module panics: a workspace that cannot be read or written is a
//! [`Diagnostic`], and a project folder that is broken or half-made is listed as broken beside
//! the ones that are fine.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use qframe::date::Date;
use qframe::diagnostics::Diagnostic;
use qframe::storage::atomic_write;

use crate::profile::Profile;

use super::{Loaded, ProjectFile, ProjectId, ProjectIdError, ProjectProfile};

/// The name of a project's own file.
const PROJECT_FILE: &str = "project.qcode";

/// The folder the profile definitions live in.
const PROFILES: &str = "Profiles";

/// The folder the projects live in.
const PROJECTS: &str = "Projects";

/// Why a new project could not be made.
#[derive(Debug)]
pub enum NewProjectError {
    /// The display name yields no identifier.
    Name(ProjectIdError),
    /// A project of that identifier is already there. This layer never quietly adds a number:
    /// the caller tells the user and asks for another name.
    Taken(ProjectId),
    /// The workspace could not be written.
    Blocked(Diagnostic),
}

/// Where everything of one project is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPaths {
    /// The project's own folder.
    pub root: PathBuf,
    /// The project's `project.qcode`.
    pub file: PathBuf,
    /// The folder the user's code lives in.
    pub project: PathBuf,
    /// The folder the user's other material lives in.
    pub assets: PathBuf,
    /// The folder that holds one folder per profile the project carries.
    pub harness: PathBuf,
}

impl ProjectPaths {
    /// The folder of one profile of this project.
    #[must_use]
    pub fn harness_profile(&self, profile: &str) -> PathBuf {
        self.harness.join(profile)
    }

    /// The folder the project's backups are kept in.
    ///
    /// Not made with the project: the first backup makes it, so a project that has never been
    /// backed up has no folder claiming otherwise.
    #[must_use]
    pub fn backup(&self) -> PathBuf {
        self.root.join("Backup")
    }
}

/// One project folder as it was found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectEntry {
    /// The project's folder.
    pub dir: PathBuf,
    /// What its `project.qcode` says, when the folder names a project at all.
    pub file: Option<ProjectFile>,
    /// Everything wrong with this project. Empty means it is whole.
    pub problems: Vec<Diagnostic>,
}

impl ProjectEntry {
    /// Whether anything about this project is broken or missing. A broken project is still
    /// listed, with its problems, so the user can see it and repair it.
    #[must_use]
    pub fn is_broken(&self) -> bool {
        !self.problems.is_empty()
    }
}

/// A workspace directory: the projects and profiles of one installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// The workspace at `root`. Nothing is read or written until it is asked for.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The workspace directory itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder the profile definitions live in.
    #[must_use]
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join(PROFILES)
    }

    /// The folder the projects live in.
    #[must_use]
    pub fn projects_dir(&self) -> PathBuf {
        self.root.join(PROJECTS)
    }

    /// Creates the folders the workspace needs, and proves they can be written.
    ///
    /// Safe to call at every start: a workspace that is already there is left alone.
    ///
    /// # Errors
    ///
    /// A diagnostic naming the directory and the reason, when it cannot be created or written.
    pub fn prepare(&self) -> Result<(), Diagnostic> {
        for dir in [self.profiles_dir(), self.projects_dir()] {
            fs::create_dir_all(&dir).map_err(|error| blocked(&dir, &error))?;
        }
        Ok(())
    }

    /// Where everything of the project `id` is, whether or not it exists yet.
    #[must_use]
    pub fn project_paths(&self, id: &ProjectId) -> ProjectPaths {
        let root = self.projects_dir().join(id.as_str());
        ProjectPaths {
            file: root.join(PROJECT_FILE),
            project: root.join("Project"),
            assets: root.join("Assets"),
            harness: root.join("Containers").join("Harness"),
            root,
        }
    }

    /// Makes a project the user called `name` and gives it the tree of the design.
    ///
    /// # Errors
    ///
    /// [`NewProjectError::Name`] when the name yields no identifier, [`NewProjectError::Taken`]
    /// when a folder of that identifier is already there — including one that only differs in
    /// case, which a file system that ignores case would treat as the same folder — and
    /// [`NewProjectError::Blocked`] when the workspace cannot be written.
    pub fn create_project(&self, name: &str, created: Date) -> Result<ProjectFile, NewProjectError> {
        let id = ProjectId::from_display_name(name).map_err(NewProjectError::Name)?;
        self.prepare().map_err(NewProjectError::Blocked)?;
        for taken in self.folder_names().map_err(NewProjectError::Blocked)? {
            if id.clashes_with(&taken) {
                return Err(NewProjectError::Taken(id));
            }
        }
        let file = ProjectFile::new(id, name, created);
        self.write_project(&file).map_err(NewProjectError::Blocked)?;
        Ok(file)
    }

    /// Writes a project's `project.qcode` atomically and makes sure its folders — including one
    /// per profile it names — are there.
    ///
    /// # Errors
    ///
    /// A diagnostic naming the directory or file that could not be written.
    pub fn write_project(&self, file: &ProjectFile) -> Result<(), Diagnostic> {
        write_project_at(&self.project_paths(&file.id), file)
    }

    /// Reads one project's `project.qcode`.
    #[must_use]
    pub fn read_project(&self, id: &ProjectId) -> Loaded<Option<ProjectFile>> {
        let path = self.project_paths(id).file;
        match fs::read_to_string(&path) {
            Ok(text) => ProjectFile::parse(PROJECT_FILE, &text),
            Err(error) => Loaded { value: None, diagnostics: vec![blocked(&path, &error)] },
        }
    }

    /// Every project folder of the workspace, sorted by folder name.
    ///
    /// A folder that is broken or half-made is listed with its problems rather than left out:
    /// the user cannot repair what the list refuses to show. A workspace that cannot be read at
    /// all yields an empty list and one diagnostic.
    #[must_use]
    pub fn projects(&self) -> Loaded<Vec<ProjectEntry>> {
        let names = match self.folder_names() {
            Ok(names) => names,
            Err(problem) => return Loaded { value: Vec::new(), diagnostics: vec![problem] },
        };
        let mut projects = Vec::with_capacity(names.len());
        let mut diagnostics = Vec::new();
        for name in names {
            let entry = self.read_entry(&name);
            diagnostics.extend(entry.problems.iter().cloned());
            projects.push(entry);
        }
        Loaded { value: projects, diagnostics }
    }

    /// Every profile definition of the workspace, sorted by file name.
    ///
    /// A `Profiles/` folder that is not there yet is not a problem: it means no profiles. A file
    /// that cannot be read or cannot be understood is reported and the rest are still returned,
    /// so one broken definition never hides the profiles that work.
    #[must_use]
    pub fn profiles(&self) -> Loaded<Vec<Profile>> {
        let folder = self.profiles_dir();
        let mut value = Vec::new();
        let mut diagnostics = Vec::new();
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Loaded { value, diagnostics },
            Err(error) => return Loaded { value, diagnostics: vec![blocked(&folder, &error)] },
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
            .collect();
        files.sort();
        for path in files {
            match fs::read_to_string(&path) {
                Ok(text) => {
                    let loaded = Profile::parse(&path.to_string_lossy(), &text);
                    diagnostics.extend(loaded.diagnostics);
                    value.extend(loaded.profile);
                }
                Err(error) => diagnostics.push(blocked(&path, &error)),
            }
        }
        Loaded { value, diagnostics }
    }

    /// Writes a profile's definition file atomically, making the workspace's folders first.
    ///
    /// # Errors
    ///
    /// A diagnostic naming the folder or file that could not be written.
    pub fn write_profile(&self, profile: &Profile) -> Result<(), Diagnostic> {
        self.prepare()?;
        let path = self.profiles_dir().join(format!("{}.toml", profile.name));
        atomic_write(&path, profile.to_toml().as_bytes()).map_err(|error| blocked(&path, &error))
    }

    /// Reads one project folder, collecting everything that is wrong with it.
    fn read_entry(&self, name: &str) -> ProjectEntry {
        let dir = self.projects_dir().join(name);
        let mut problems = Vec::new();
        let id = match ProjectId::parse(name) {
            Ok(id) => id,
            Err(problem) => {
                problems.push(Diagnostic::error(
                    None,
                    format!("{}: this folder name is no project id: {problem:?}", dir.display()),
                ));
                return ProjectEntry { dir, file: None, problems };
            }
        };
        let read = self.read_project(&id);
        problems.extend(read.diagnostics);
        let file = read.value.filter(|file| {
            let matches = file.id == id;
            if !matches {
                problems.push(Diagnostic::error(
                    None,
                    format!("{}: the project file calls this project `{}`", dir.display(), file.id),
                ));
            }
            matches
        });
        let paths = self.project_paths(&id);
        for missing in [&paths.project, &paths.assets, &paths.harness].into_iter().filter(|path| !path.is_dir()) {
            problems
                .push(Diagnostic::error(None, format!("{}: this folder of the project is missing", missing.display())));
        }
        ProjectEntry { dir, file, problems }
    }

    /// The names of the folders below `Projects`, sorted.
    fn folder_names(&self) -> Result<Vec<String>, Diagnostic> {
        let dir = self.projects_dir();
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|error| blocked(&dir, &error))? {
            let entry = entry.map_err(|error| blocked(&dir, &error))?;
            if entry.file_type().map_err(|error| blocked(&dir, &error))?.is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }
}

/// Writes `file` as the `project.qcode` at `paths` atomically, and makes sure the project's
/// folders — including one per profile it names — are there.
fn write_project_at(paths: &ProjectPaths, file: &ProjectFile) -> Result<(), Diagnostic> {
    let mut dirs = vec![paths.project.clone(), paths.assets.clone(), paths.harness.clone()];
    dirs.extend(file.profiles.iter().map(|profile| paths.harness_profile(&profile.name)));
    for dir in dirs {
        fs::create_dir_all(&dir).map_err(|error| blocked(&dir, &error))?;
    }
    atomic_write(&paths.file, file.to_toml().as_bytes()).map_err(|error| blocked(&paths.file, &error))
}

/// Records in the project at `paths` that it carries the profile `name`, added on `added`, and
/// answers with the file as it now stands.
///
/// The file is read again first rather than written from what a screen remembers, so whatever
/// was changed in it since the screen read it — by hand, or by another window — is kept. A
/// profile the file already names is left as it is, with its own date.
///
/// Runs file work, so it belongs on a background thread.
///
/// # Errors
///
/// A diagnostic when the file cannot be read, names no usable project, or cannot be written.
pub fn add_profile(paths: &ProjectPaths, name: &str, added: Date) -> Result<ProjectFile, Diagnostic> {
    let text = fs::read_to_string(&paths.file).map_err(|error| blocked(&paths.file, &error))?;
    let read = ProjectFile::parse(PROJECT_FILE, &text);
    let Some(mut file) = read.value else {
        let reason = read.diagnostics.first().map_or_else(|| "no usable project".to_owned(), ToString::to_string);
        return Err(Diagnostic::error(None, format!("{}: {reason}", paths.file.display())));
    };
    if !file.profiles.iter().any(|carried| carried.name == name) {
        file.profiles.push(ProjectProfile { name: name.to_owned(), added: Some(added) });
        write_project_at(paths, &file)?;
    }
    Ok(file)
}

/// The diagnostic for a path the file system would not let this program use.
fn blocked(path: &Path, error: &io::Error) -> Diagnostic {
    Diagnostic::error(None, format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, SafeName, Template};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A directory of this test's own, removed first so a crashed run cannot poison the next.
    fn scratch(name: &str) -> PathBuf {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let unique = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qcode-workspace-{name}-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("the directory is there")
            .map(|entry| entry.expect("a readable entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn today() -> Date {
        Date::new(2026, 9, 17).expect("a real date")
    }

    fn profile(name: &str) -> Profile {
        Profile {
            name: SafeName::parse(name).expect("the name is safe"),
            harness: HarnessKind::ClaudeCode,
            template: Template::Recommended,
            account: AccountKind::Subscription,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
        }
    }

    #[test]
    fn a_workspace_without_a_profiles_folder_simply_has_no_profiles() {
        let loaded = Workspace::new(scratch("no-profiles")).profiles();
        assert!(loaded.value.is_empty());
        assert!(loaded.is_clean(), "a folder that was never made is not a problem: {:?}", loaded.diagnostics);
    }

    #[test]
    fn a_written_profile_reads_back_as_the_same_profile() {
        let workspace = Workspace::new(scratch("profile-roundtrip"));
        let written = profile("claude-sub");
        workspace.write_profile(&written).expect("the workspace can be written");
        let loaded = workspace.profiles();
        assert!(loaded.is_clean(), "{:?}", loaded.diagnostics);
        assert_eq!(loaded.value, vec![written]);
        let _ = fs::remove_dir_all(workspace.root());
    }

    #[test]
    fn a_broken_definition_is_reported_and_the_readable_ones_are_kept() {
        let workspace = Workspace::new(scratch("profile-broken"));
        workspace.write_profile(&profile("claude-sub")).expect("the workspace can be written");
        fs::write(workspace.profiles_dir().join("half.toml"), "harness = \"claude-code\"\n")
            .expect("the folder exists");
        let loaded = workspace.profiles();
        assert_eq!(loaded.value.len(), 1, "the readable profile is kept: {:?}", loaded.value);
        assert!(!loaded.is_clean(), "the broken one is reported");
        let _ = fs::remove_dir_all(workspace.root());
    }

    #[test]
    fn preparing_a_workspace_creates_the_two_folders_of_the_design_and_no_more() {
        let root = scratch("prepare");
        let workspace = Workspace::new(&root);
        workspace.prepare().expect("a writable temporary directory");
        assert_eq!(entries(&root), ["Profiles", "Projects"]);
        workspace.prepare().expect("preparing again changes nothing");
        assert_eq!(entries(&root), ["Profiles", "Projects"]);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_new_project_gets_the_tree_of_the_design_without_backup_or_mcp() {
        let root = scratch("create");
        let workspace = Workspace::new(&root);
        workspace.prepare().expect("writable");
        let file = workspace.create_project("Görünen ad", today()).expect("a free name");
        assert_eq!(file.id.as_str(), "gorunen-ad");

        let paths = workspace.project_paths(&file.id);
        assert_eq!(entries(&paths.root), ["Assets", "Containers", "Project", "project.qcode"]);
        assert_eq!(entries(&paths.root.join("Containers")), ["Harness"]);
        assert!(paths.harness.join("").is_dir());
        assert_eq!(entries(&paths.harness), [] as [String; 0]);
        assert_eq!(paths.backup(), paths.root.join("Backup"), "named, but left for the first backup to make");

        let read = workspace.read_project(&file.id);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(file));
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_name_that_is_already_taken_is_reported_and_nothing_is_overwritten() {
        let root = scratch("clash");
        let workspace = Workspace::new(&root);
        workspace.prepare().expect("writable");
        let first = workspace.create_project("Proje", today()).expect("a free name");
        // Different display name, same identifier, and on a case-insensitive file system the
        // same folder: the caller decides what to do, this layer never invents `proje-2`.
        match workspace.create_project("PROJE", today()) {
            Err(NewProjectError::Taken(id)) => assert_eq!(id, first.id),
            other => panic!("expected a clash, got {other:?}"),
        }
        assert_eq!(entries(&workspace.projects_dir()), ["proje"]);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_name_with_nothing_usable_in_it_never_reaches_the_disk() {
        let root = scratch("empty-name");
        let workspace = Workspace::new(&root);
        workspace.prepare().expect("writable");
        assert!(matches!(workspace.create_project("...", today()), Err(NewProjectError::Name(ProjectIdError::Empty))));
        assert_eq!(entries(&workspace.projects_dir()), [] as [String; 0]);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_broken_project_appears_in_the_list_with_its_diagnostic() {
        let root = scratch("list");
        let workspace = Workspace::new(&root);
        workspace.prepare().expect("writable");
        workspace.create_project("Saglam", today()).expect("a free name");

        // Half a project: a folder, no project file and none of the subfolders.
        fs::create_dir_all(workspace.projects_dir().join("yarim")).expect("writable");
        // A project file that cannot be read at all.
        let broken = workspace.projects_dir().join("bozuk");
        fs::create_dir_all(&broken).expect("writable");
        fs::write(broken.join("project.qcode"), "id = \n").expect("writable");
        // A folder whose name no project could ever have.
        fs::create_dir_all(workspace.projects_dir().join("Büyük Harf")).expect("writable");

        let listed = workspace.projects();
        let names: Vec<String> = listed
            .value
            .iter()
            .map(|entry| entry.dir.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Büyük Harf", "bozuk", "saglam", "yarim"]);
        assert_eq!(listed.value.iter().filter(|entry| entry.is_broken()).count(), 3);
        assert_eq!(listed.value[2].file.as_ref().map(|file| file.name.clone()), Some("Saglam".to_owned()));
        assert!(listed.value[0].file.is_none(), "an illegal folder name cannot name a project");
        assert!(!listed.diagnostics.is_empty());
        assert!(listed.value.iter().filter(|entry| entry.is_broken()).all(|entry| !entry.problems.is_empty()));
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_project_whose_file_names_another_project_is_broken() {
        let root = scratch("mismatch");
        let workspace = Workspace::new(&root);
        workspace.prepare().expect("writable");
        let file = workspace.create_project("Proje", today()).expect("a free name");
        let paths = workspace.project_paths(&file.id);
        fs::write(&paths.file, "id = \"baska\"\nname = \"Proje\"\n").expect("writable");
        let listed = workspace.projects();
        assert!(listed.value[0].is_broken());
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn listing_an_empty_or_missing_workspace_is_not_an_error() {
        let root = scratch("missing");
        let workspace = Workspace::new(&root);
        let listed = workspace.projects();
        assert!(listed.value.is_empty());
        assert_eq!(listed.diagnostics.len(), 1, "a missing workspace is worth saying out loud");

        workspace.prepare().expect("writable");
        let listed = workspace.projects();
        assert!(listed.value.is_empty() && listed.is_clean());
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn writing_a_project_creates_the_folder_of_every_profile_it_names() {
        let root = scratch("profiles");
        let workspace = Workspace::new(&root);
        workspace.prepare().expect("writable");
        let mut file = workspace.create_project("Proje", today()).expect("a free name");
        file.profiles.push(ProjectProfile { name: "claude-sub".to_owned(), added: Some(today()) });
        workspace.write_project(&file).expect("writable");
        assert_eq!(entries(&workspace.project_paths(&file.id).harness), ["claude-sub"]);
        assert_eq!(workspace.read_project(&file.id).value, Some(file));
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn adding_a_profile_records_it_once_and_keeps_what_the_file_already_says() {
        let root = scratch("add-profile");
        let workspace = Workspace::new(&root);
        let mut file = workspace.create_project("Proje", today()).expect("a free name");
        file.profiles.push(ProjectProfile { name: "opencode".to_owned(), added: Some(today()) });
        workspace.write_project(&file).expect("writable");
        let paths = workspace.project_paths(&file.id);
        let later = Date::new(2026, 9, 18).expect("a real date");

        let added = add_profile(&paths, "claude-sub", later).expect("writable");
        let names: Vec<&str> = added.profiles.iter().map(|profile| profile.name.as_str()).collect();
        assert_eq!(names, ["opencode", "claude-sub"], "the profile the file named by hand is kept");
        assert_eq!(workspace.read_project(&file.id).value, Some(added.clone()), "and it is on disk");
        assert_eq!(entries(&paths.harness), ["claude-sub", "opencode"], "with a folder of its own");

        // Adding it again changes nothing, not even the day it was first added.
        let again = add_profile(&paths, "claude-sub", Date::new(2026, 9, 19).expect("a real date")).expect("fine");
        assert_eq!(again, added);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn adding_a_profile_to_a_project_that_is_not_there_is_a_diagnostic() {
        let root = scratch("add-missing");
        let workspace = Workspace::new(&root);
        let paths = workspace.project_paths(&ProjectId::parse("yok").expect("an id"));
        let error = add_profile(&paths, "claude-sub", today()).expect_err("nothing to add to");
        assert!(error.message.contains("project.qcode"), "{}", error.message);
        assert!(!paths.file.exists(), "no file is made up for a project that is not there");
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_that_cannot_be_written_is_a_diagnostic_and_not_a_panic() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("read-only");
        fs::create_dir_all(&root).expect("writable");
        let inner = root.join("QCode");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).expect("a mode this user may set");

        let workspace = Workspace::new(&inner);
        let blocked = workspace.prepare().expect_err("the parent forbids it");
        assert!(blocked.to_string().contains(&inner.display().to_string()), "{blocked}");

        let listed = workspace.projects();
        assert!(listed.value.is_empty() && !listed.is_clean());

        assert!(matches!(workspace.create_project("Proje", today()), Err(NewProjectError::Blocked(_))));

        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("a mode this user may set");
        fs::remove_dir_all(&root).expect("cleaned up");
    }
}
