//! The folder tree of the store: creating it, and reading it back.
//!
//! ```text
//! {store}/
//!   Profiles/
//!   Workspaces/
//!     <workspace>/
//!       workspace.qcode
//!       Work/
//!       Assets/
//!       Containers/Harness/<profile>/
//! ```
//!
//! `Backup/` and `Containers/MCP/` are not created here; they are born with the features that
//! need them. Nothing in this module panics: a store that cannot be read or written is a
//! [`Diagnostic`], and a workspace folder that is broken or half-made is listed as broken beside
//! the ones that are fine.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use qframe::date::Date;
use qframe::diagnostics::Diagnostic;
use qframe::storage::atomic_write;

use crate::backup::Place;
use crate::profile::Profile;

use super::{Loaded, WorkspaceFile, WorkspaceId, WorkspaceIdError, WorkspaceProfile};

/// The name of a workspace's own file.
const WORKSPACE_FILE: &str = "workspace.qcode";

/// The folder the profile definitions live in.
const PROFILES: &str = "Profiles";

/// The folder the workspaces live in.
const WORKSPACES: &str = "Workspaces";

/// The folder the user's code lives in, inside a workspace. It carries the name the container
/// mounts it on ([`crate::base::paths::CODE_DIR`]), so the person reads one word in both places.
const CODE: &str = "Work";

/// The names a store written by an older QCode carries, in the order they were left behind:
/// first the one from before workspaces had their name, then the one from when the person's own
/// folder was called `Code/`. [`Store::adopt`] gives each one the name it has today, and a store
/// that stopped at either of them is carried the rest of the way.
const LEGACY: [(&str, &str); 3] = [("project.qcode", WORKSPACE_FILE), ("Project", CODE), ("Code", CODE)];

/// The same for the two that are not beside each other: the folder of all workspaces, and the
/// snapshots of one workspace's code inside its `Backup/`.
const LEGACY_WORKSPACES: &str = "Projects";

/// Why a new workspace could not be made.
#[derive(Debug)]
pub enum NewWorkspaceError {
    /// The display name yields no identifier.
    Name(WorkspaceIdError),
    /// A workspace of that identifier is already there. This layer never quietly adds a number:
    /// the caller tells the user and asks for another name.
    Taken(WorkspaceId),
    /// The store could not be written.
    Blocked(Diagnostic),
}

/// Where everything of one workspace is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaths {
    /// The workspace's own folder.
    pub root: PathBuf,
    /// The workspace's `workspace.qcode`.
    pub file: PathBuf,
    /// The folder the user's code lives in.
    pub code: PathBuf,
    /// The folder the user's other material lives in.
    pub assets: PathBuf,
    /// The folder that holds one folder per profile the workspace carries.
    pub harness: PathBuf,
}

impl WorkspacePaths {
    /// The folder of one profile of this workspace.
    #[must_use]
    pub fn harness_profile(&self, profile: &str) -> PathBuf {
        self.harness.join(profile)
    }

    /// The folder the bridge between the workspace's tabs lives in: the socket QCode answers on
    /// while the workspace is open, and the server the harnesses start. Every profile container
    /// of the workspace sees it read-only.
    ///
    /// Not made with the workspace: opening it makes the folder, so a workspace never opened since
    /// the bridge came has none.
    #[must_use]
    pub fn mcp(&self) -> PathBuf {
        self.root.join("Containers").join("MCP")
    }

    /// The folder a window's container leaves web addresses in, for QCode to open in the
    /// person's own browser.
    ///
    /// Not made with the workspace: opening a window makes it, so a workspace that has never opened
    /// one has none.
    #[must_use]
    pub fn browser(&self) -> PathBuf {
        self.root.join("Containers").join("Browser")
    }

    /// The folder the workspace's backups are kept in.
    ///
    /// Not made with the workspace: the first backup makes it, so a workspace that has never been
    /// backed up has no folder claiming otherwise.
    #[must_use]
    pub fn backup(&self) -> PathBuf {
        self.root.join("Backup")
    }
}

/// One workspace folder as it was found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceEntry {
    /// The workspace's folder.
    pub dir: PathBuf,
    /// What its `workspace.qcode` says, when the folder names a workspace at all.
    pub file: Option<WorkspaceFile>,
    /// Everything wrong with this workspace. Empty means it is whole.
    pub problems: Vec<Diagnostic>,
}

impl WorkspaceEntry {
    /// Whether anything about this workspace is broken or missing. A broken workspace is still
    /// listed, with its problems, so the user can see it and repair it.
    #[must_use]
    pub fn is_broken(&self) -> bool {
        !self.problems.is_empty()
    }
}

/// Moves `from` to `to`, when `from` is there and `to` is not.
fn adopt_name(from: &Path, to: &Path, said: &mut Vec<Diagnostic>) {
    if fs::symlink_metadata(from).is_err() {
        return;
    }
    if fs::symlink_metadata(to).is_ok() {
        let message = format!("{} is there as well; {} is left as it is", to.display(), from.display());
        said.push(Diagnostic::warning(None, message));
        return;
    }
    if let Err(error) = fs::rename(from, to) {
        let message =
            format!("{} could not be renamed to {} ({error}); it stays where it is", from.display(), to.display());
        said.push(Diagnostic::error(None, message));
    }
}

/// A store directory: the workspaces and profiles of one installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store at `root`. Nothing is read or written until it is asked for.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store directory itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder the profile definitions live in.
    #[must_use]
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join(PROFILES)
    }

    /// The folder the workspaces live in.
    #[must_use]
    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join(WORKSPACES)
    }

    /// Creates the folders the store needs, and proves they can be written.
    ///
    /// Safe to call at every start: a store that is already there is left alone.
    ///
    /// # Errors
    ///
    /// A diagnostic naming the directory and the reason, when it cannot be created or written.
    pub fn prepare(&self) -> Result<(), Diagnostic> {
        for dir in [self.profiles_dir(), self.workspaces_dir()] {
            fs::create_dir_all(&dir).map_err(|error| blocked(&dir, &error))?;
        }
        Ok(())
    }

    /// Gives every name this store carries from an older QCode the name it has today: `Projects/`
    /// becomes `Workspaces/`, and inside each workspace `project.qcode` becomes `workspace.qcode`,
    /// `Project/` and then `Code/` become `Work/`, and `Backup/Project.git` becomes
    /// `Backup/Code.git`.
    ///
    /// Each move is a single rename, so an interruption leaves the old name whole rather than two
    /// halves, and nothing is renamed onto something that is there already: a store holding both
    /// names is left as it is and said. Safe to call at every start — a store with none of the old
    /// names only looks.
    pub fn adopt(&self) -> Vec<Diagnostic> {
        let mut said = Vec::new();
        adopt_name(&self.root.join(LEGACY_WORKSPACES), &self.workspaces_dir(), &mut said);
        let Ok(entries) = fs::read_dir(self.workspaces_dir()) else {
            return said;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            for (old, new) in LEGACY {
                adopt_name(&dir.join(old), &dir.join(new), &mut said);
            }
            let backup = dir.join("Backup");
            adopt_name(&backup.join("Project.git"), &backup.join(crate::backup::CODE_SNAPSHOTS), &mut said);
        }
        said
    }

    /// Where everything of the workspace `id` is, whether or not it exists yet.
    #[must_use]
    pub fn workspace_paths(&self, id: &WorkspaceId) -> WorkspacePaths {
        let root = self.workspaces_dir().join(id.as_str());
        WorkspacePaths {
            file: root.join(WORKSPACE_FILE),
            code: root.join(CODE),
            assets: root.join("Assets"),
            harness: root.join("Containers").join("Harness"),
            root,
        }
    }

    /// Makes a workspace the user called `name` and gives it the tree of the design.
    ///
    /// # Errors
    ///
    /// [`NewWorkspaceError::Name`] when the name yields no identifier, [`NewWorkspaceError::Taken`]
    /// when a folder of that identifier is already there — including one that only differs in
    /// case, which a file system that ignores case would treat as the same folder — and
    /// [`NewWorkspaceError::Blocked`] when the store cannot be written.
    pub fn create_workspace(&self, name: &str, created: Date) -> Result<WorkspaceFile, NewWorkspaceError> {
        let id = WorkspaceId::from_display_name(name).map_err(NewWorkspaceError::Name)?;
        self.prepare().map_err(NewWorkspaceError::Blocked)?;
        for taken in self.folder_names().map_err(NewWorkspaceError::Blocked)? {
            if id.clashes_with(&taken) {
                return Err(NewWorkspaceError::Taken(id));
            }
        }
        let file = WorkspaceFile::new(id, name, created);
        self.write_workspace(&file).map_err(NewWorkspaceError::Blocked)?;
        Ok(file)
    }

    /// Writes a workspace's `workspace.qcode` atomically and makes sure its folders — including one
    /// per profile it names — are there.
    ///
    /// # Errors
    ///
    /// A diagnostic naming the directory or file that could not be written.
    pub fn write_workspace(&self, file: &WorkspaceFile) -> Result<(), Diagnostic> {
        write_workspace_at(&self.workspace_paths(&file.id), file)
    }

    /// Reads one workspace's `workspace.qcode`.
    #[must_use]
    pub fn read_workspace(&self, id: &WorkspaceId) -> Loaded<Option<WorkspaceFile>> {
        let path = self.workspace_paths(id).file;
        match fs::read_to_string(&path) {
            Ok(text) => WorkspaceFile::parse(WORKSPACE_FILE, &text),
            Err(error) => Loaded { value: None, diagnostics: vec![blocked(&path, &error)] },
        }
    }

    /// Every workspace folder of the store, sorted by folder name.
    ///
    /// A folder that is broken or half-made is listed with its problems rather than left out:
    /// the user cannot repair what the list refuses to show. A store that cannot be read at
    /// all yields an empty list and one diagnostic.
    #[must_use]
    pub fn workspaces(&self) -> Loaded<Vec<WorkspaceEntry>> {
        let names = match self.folder_names() {
            Ok(names) => names,
            Err(problem) => return Loaded { value: Vec::new(), diagnostics: vec![problem] },
        };
        let mut workspaces = Vec::with_capacity(names.len());
        let mut diagnostics = Vec::new();
        for name in names {
            let entry = self.read_entry(&name);
            diagnostics.extend(entry.problems.iter().cloned());
            workspaces.push(entry);
        }
        Loaded { value: workspaces, diagnostics }
    }

    /// Every profile definition of the store, sorted by file name.
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

    /// Writes a profile's definition file atomically, making the store's folders first.
    ///
    /// # Errors
    ///
    /// A diagnostic naming the folder or file that could not be written.
    pub fn write_profile(&self, profile: &Profile) -> Result<(), Diagnostic> {
        self.prepare()?;
        let path = self.profiles_dir().join(format!("{}.toml", profile.name));
        atomic_write(&path, profile.to_toml().as_bytes()).map_err(|error| blocked(&path, &error))
    }

    /// Reads one workspace folder, collecting everything that is wrong with it.
    fn read_entry(&self, name: &str) -> WorkspaceEntry {
        let dir = self.workspaces_dir().join(name);
        let mut problems = Vec::new();
        let id = match WorkspaceId::parse(name) {
            Ok(id) => id,
            Err(problem) => {
                problems.push(Diagnostic::error(
                    None,
                    format!("{}: this folder name is no workspace id: {problem:?}", dir.display()),
                ));
                return WorkspaceEntry { dir, file: None, problems };
            }
        };
        let read = self.read_workspace(&id);
        problems.extend(read.diagnostics);
        let file = read.value.filter(|file| {
            let matches = file.id == id;
            if !matches {
                problems.push(Diagnostic::error(
                    None,
                    format!("{}: the workspace file calls this workspace `{}`", dir.display(), file.id),
                ));
            }
            matches
        });
        let paths = self.workspace_paths(&id);
        for missing in [&paths.code, &paths.assets, &paths.harness].into_iter().filter(|path| !path.is_dir()) {
            problems.push(Diagnostic::error(
                None,
                format!("{}: this folder of the workspace is missing", missing.display()),
            ));
        }
        WorkspaceEntry { dir, file, problems }
    }

    /// The names of the folders below `Workspaces`, sorted.
    fn folder_names(&self) -> Result<Vec<String>, Diagnostic> {
        let dir = self.workspaces_dir();
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

/// Writes `file` as the `workspace.qcode` at `paths` atomically, and makes sure the workspace's
/// folders — including one per profile it names — are there.
fn write_workspace_at(paths: &WorkspacePaths, file: &WorkspaceFile) -> Result<(), Diagnostic> {
    let mut dirs = vec![paths.code.clone(), paths.assets.clone(), paths.harness.clone()];
    dirs.extend(file.profiles.iter().map(|profile| paths.harness_profile(&profile.name)));
    for dir in dirs {
        fs::create_dir_all(&dir).map_err(|error| blocked(&dir, &error))?;
    }
    atomic_write(&paths.file, file.to_toml().as_bytes()).map_err(|error| blocked(&paths.file, &error))
}

/// Records in the workspace at `paths` that it carries the profile `name`, added on `added`, and
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
/// A diagnostic when the file cannot be read, names no usable workspace, or cannot be written.
pub fn add_profile(paths: &WorkspacePaths, name: &str, added: Date) -> Result<WorkspaceFile, Diagnostic> {
    let mut file = read_workspace_at(paths)?;
    if !file.profiles.iter().any(|carried| carried.name == name) {
        file.profiles.push(WorkspaceProfile { name: name.to_owned(), added: Some(added) });
        write_workspace_at(paths, &file)?;
    }
    Ok(file)
}

/// Records in the workspace at `paths` that its backup leaves `keys` out, or takes them in again
/// when `skip` is false, and answers with the file as it now stands.
///
/// Like [`add_profile`], the file is read again first, so a change made to it meanwhile is kept.
/// A path already inside a folder that is left out is not added again, and leaving a folder out
/// drops the entries inside it, which it covers from then on. Taking a path in again takes in
/// only that path: a file inside a folder that stays left out stays out with it. The file is
/// written only when the list changed.
///
/// # Errors
///
/// A key that is not a path inside the workspace, and a `workspace.qcode` that cannot be read, names
/// no workspace or cannot be written.
pub fn set_backup_skip(paths: &WorkspacePaths, keys: &[String], skip: bool) -> Result<WorkspaceFile, Diagnostic> {
    let mut places = Vec::new();
    for key in keys {
        let place = Place::new(key).map_err(|bad| {
            Diagnostic::error(None, format!("`{}` is not a path inside the workspace: {:?}", bad.path, bad.problem))
        })?;
        places.push(place.as_str().to_owned());
    }
    let mut file = read_workspace_at(paths)?;
    let before = file.backup_skip.clone();
    let inside = |path: &str, folder: &str| path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'));
    for place in places {
        if !skip {
            file.backup_skip.retain(|known| *known != place);
        } else if !file.backup_skip.iter().any(|known| *known == place || inside(&place, known)) {
            file.backup_skip.retain(|known| !inside(known, &place));
            file.backup_skip.push(place);
        }
    }
    if file.backup_skip != before {
        write_workspace_at(paths, &file)?;
    }
    Ok(file)
}

/// Records in the workspace at `paths` whether its backup takes `Assets/` too, and answers with the
/// file as it now stands. The file is read again first, as for [`set_backup_skip`], and written
/// only when the choice changed.
///
/// # Errors
///
/// A `workspace.qcode` that cannot be read, names no workspace or cannot be written.
pub fn set_backup_assets(paths: &WorkspacePaths, assets: bool) -> Result<WorkspaceFile, Diagnostic> {
    let mut file = read_workspace_at(paths)?;
    if file.backup_assets != assets {
        file.backup_assets = assets;
        write_workspace_at(paths, &file)?;
    }
    Ok(file)
}

/// The workspace file at `paths` as it is on disk now.
fn read_workspace_at(paths: &WorkspacePaths) -> Result<WorkspaceFile, Diagnostic> {
    let text = fs::read_to_string(&paths.file).map_err(|error| blocked(&paths.file, &error))?;
    let read = WorkspaceFile::parse(WORKSPACE_FILE, &text);
    read.value.ok_or_else(|| {
        let reason = read.diagnostics.first().map_or_else(|| "no usable workspace".to_owned(), ToString::to_string);
        Diagnostic::error(None, format!("{}: {reason}", paths.file.display()))
    })
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
        let dir = std::env::temp_dir().join(format!("qcode-store-{name}-{}-{unique}", std::process::id()));
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
    fn a_store_without_a_profiles_folder_simply_has_no_profiles() {
        let loaded = Store::new(scratch("no-profiles")).profiles();
        assert!(loaded.value.is_empty());
        assert!(loaded.is_clean(), "a folder that was never made is not a problem: {:?}", loaded.diagnostics);
    }

    #[test]
    fn a_written_profile_reads_back_as_the_same_profile() {
        let store = Store::new(scratch("profile-roundtrip"));
        let written = profile("claude-sub");
        store.write_profile(&written).expect("the store can be written");
        let loaded = store.profiles();
        assert!(loaded.is_clean(), "{:?}", loaded.diagnostics);
        assert_eq!(loaded.value, vec![written]);
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn a_broken_definition_is_reported_and_the_readable_ones_are_kept() {
        let store = Store::new(scratch("profile-broken"));
        store.write_profile(&profile("claude-sub")).expect("the store can be written");
        fs::write(store.profiles_dir().join("half.toml"), "harness = \"claude-code\"\n").expect("the folder exists");
        let loaded = store.profiles();
        assert_eq!(loaded.value.len(), 1, "the readable profile is kept: {:?}", loaded.value);
        assert!(!loaded.is_clean(), "the broken one is reported");
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn preparing_a_store_creates_the_two_folders_of_the_design_and_no_more() {
        let root = scratch("prepare");
        let store = Store::new(&root);
        store.prepare().expect("a writable temporary directory");
        assert_eq!(entries(&root), ["Profiles", "Workspaces"]);
        store.prepare().expect("preparing again changes nothing");
        assert_eq!(entries(&root), ["Profiles", "Workspaces"]);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_new_workspace_gets_the_tree_of_the_design_without_backup_or_mcp() {
        let root = scratch("create");
        let store = Store::new(&root);
        store.prepare().expect("writable");
        let file = store.create_workspace("Görünen ad", today()).expect("a free name");
        assert_eq!(file.id.as_str(), "gorunen-ad");

        let paths = store.workspace_paths(&file.id);
        assert_eq!(entries(&paths.root), ["Assets", "Containers", "Work", "workspace.qcode"]);
        assert_eq!(entries(&paths.root.join("Containers")), ["Harness"]);
        assert!(paths.harness.join("").is_dir());
        assert_eq!(entries(&paths.harness), [] as [String; 0]);
        assert_eq!(paths.backup(), paths.root.join("Backup"), "named, but left for the first backup to make");

        let read = store.read_workspace(&file.id);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        assert_eq!(read.value, Some(file));
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_name_that_is_already_taken_is_reported_and_nothing_is_overwritten() {
        let root = scratch("clash");
        let store = Store::new(&root);
        store.prepare().expect("writable");
        let first = store.create_workspace("Proje", today()).expect("a free name");
        // Different display name, same identifier, and on a case-insensitive file system the
        // same folder: the caller decides what to do, this layer never invents `proje-2`.
        match store.create_workspace("PROJE", today()) {
            Err(NewWorkspaceError::Taken(id)) => assert_eq!(id, first.id),
            other => panic!("expected a clash, got {other:?}"),
        }
        assert_eq!(entries(&store.workspaces_dir()), ["proje"]);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_name_with_nothing_usable_in_it_never_reaches_the_disk() {
        let root = scratch("empty-name");
        let store = Store::new(&root);
        store.prepare().expect("writable");
        assert!(matches!(
            store.create_workspace("...", today()),
            Err(NewWorkspaceError::Name(WorkspaceIdError::Empty))
        ));
        assert_eq!(entries(&store.workspaces_dir()), [] as [String; 0]);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_broken_workspace_appears_in_the_list_with_its_diagnostic() {
        let root = scratch("list");
        let store = Store::new(&root);
        store.prepare().expect("writable");
        store.create_workspace("Saglam", today()).expect("a free name");

        // Half a workspace: a folder, no workspace file and none of the subfolders.
        fs::create_dir_all(store.workspaces_dir().join("yarim")).expect("writable");
        // A workspace file that cannot be read at all.
        let broken = store.workspaces_dir().join("bozuk");
        fs::create_dir_all(&broken).expect("writable");
        fs::write(broken.join("workspace.qcode"), "id = \n").expect("writable");
        // A folder whose name no workspace could ever have.
        fs::create_dir_all(store.workspaces_dir().join("Büyük Harf")).expect("writable");

        let listed = store.workspaces();
        let names: Vec<String> = listed
            .value
            .iter()
            .map(|entry| entry.dir.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Büyük Harf", "bozuk", "saglam", "yarim"]);
        assert_eq!(listed.value.iter().filter(|entry| entry.is_broken()).count(), 3);
        assert_eq!(listed.value[2].file.as_ref().map(|file| file.name.clone()), Some("Saglam".to_owned()));
        assert!(listed.value[0].file.is_none(), "an illegal folder name cannot name a workspace");
        assert!(!listed.diagnostics.is_empty());
        assert!(listed.value.iter().filter(|entry| entry.is_broken()).all(|entry| !entry.problems.is_empty()));
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn a_workspace_whose_file_names_another_workspace_is_broken() {
        let root = scratch("mismatch");
        let store = Store::new(&root);
        store.prepare().expect("writable");
        let file = store.create_workspace("Proje", today()).expect("a free name");
        let paths = store.workspace_paths(&file.id);
        fs::write(&paths.file, "id = \"baska\"\nname = \"Proje\"\n").expect("writable");
        let listed = store.workspaces();
        assert!(listed.value[0].is_broken());
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn listing_an_empty_or_missing_store_is_not_an_error() {
        let root = scratch("missing");
        let store = Store::new(&root);
        let listed = store.workspaces();
        assert!(listed.value.is_empty());
        assert_eq!(listed.diagnostics.len(), 1, "a missing store is worth saying out loud");

        store.prepare().expect("writable");
        let listed = store.workspaces();
        assert!(listed.value.is_empty() && listed.is_clean());
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn writing_a_workspace_creates_the_folder_of_every_profile_it_names() {
        let root = scratch("profiles");
        let store = Store::new(&root);
        store.prepare().expect("writable");
        let mut file = store.create_workspace("Proje", today()).expect("a free name");
        file.profiles.push(WorkspaceProfile { name: "claude-sub".to_owned(), added: Some(today()) });
        store.write_workspace(&file).expect("writable");
        assert_eq!(entries(&store.workspace_paths(&file.id).harness), ["claude-sub"]);
        assert_eq!(store.read_workspace(&file.id).value, Some(file));
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn adding_a_profile_records_it_once_and_keeps_what_the_file_already_says() {
        let root = scratch("add-profile");
        let store = Store::new(&root);
        let mut file = store.create_workspace("Proje", today()).expect("a free name");
        file.profiles.push(WorkspaceProfile { name: "opencode".to_owned(), added: Some(today()) });
        store.write_workspace(&file).expect("writable");
        let paths = store.workspace_paths(&file.id);
        let later = Date::new(2026, 9, 18).expect("a real date");

        let added = add_profile(&paths, "claude-sub", later).expect("writable");
        let names: Vec<&str> = added.profiles.iter().map(|profile| profile.name.as_str()).collect();
        assert_eq!(names, ["opencode", "claude-sub"], "the profile the file named by hand is kept");
        assert_eq!(store.read_workspace(&file.id).value, Some(added.clone()), "and it is on disk");
        assert_eq!(entries(&paths.harness), ["claude-sub", "opencode"], "with a folder of its own");

        // Adding it again changes nothing, not even the day it was first added.
        let again = add_profile(&paths, "claude-sub", Date::new(2026, 9, 19).expect("a real date")).expect("fine");
        assert_eq!(again, added);
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn what_the_backup_leaves_out_is_written_into_the_file_as_it_stands_now() {
        let root = scratch("skip");
        let store = Store::new(&root);
        let file = store.create_workspace("Proje", today()).expect("writable");
        let paths = store.workspace_paths(&file.id);
        let keys = |keys: &[&str]| keys.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>();

        let out = set_backup_skip(&paths, &keys(&["data/raw", "out"]), true).expect("writable");
        assert_eq!(out.backup_skip, ["data/raw", "out"]);
        // Changed by hand meanwhile: the hand's change is kept.
        add_profile(&paths, "claude-sub", today()).expect("writable");
        let out = set_backup_skip(&paths, &keys(&["data", "out/big"]), true).expect("writable");
        assert_eq!(out.backup_skip, ["out", "data"], "the folder covers what was inside it, and what `out` covers");
        assert_eq!(out.profiles.len(), 1, "the profile added meanwhile is still there");
        assert_eq!(store.read_workspace(&file.id).value, Some(out.clone()), "and it is on disk");

        let back = set_backup_skip(&paths, &keys(&["out", "data/raw"]), false).expect("writable");
        assert_eq!(back.backup_skip, ["data"], "a path inside a folder still left out stays out with it");
        assert!(set_backup_skip(&paths, &keys(&["../elsewhere"]), true).is_err(), "only paths inside the workspace");
        assert_eq!(store.read_workspace(&file.id).value, Some(back));
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn the_assets_switch_is_written_into_the_file_as_it_stands_now() {
        let root = scratch("assets");
        let store = Store::new(&root);
        let file = store.create_workspace("Proje", today()).expect("writable");
        let paths = store.workspace_paths(&file.id);
        let on = set_backup_assets(&paths, true).expect("writable");
        assert!(on.backup_assets);
        // Changed by hand meanwhile: the hand's change is kept.
        set_backup_skip(&paths, &["data".to_owned()], true).expect("writable");
        let off = set_backup_assets(&paths, false).expect("writable");
        assert!(!off.backup_assets);
        assert_eq!(off.backup_skip, ["data"]);
        assert_eq!(store.read_workspace(&file.id).value, Some(off));
        assert!(!fs::read_to_string(&paths.file).expect("the file").contains("assets"), "off is not written");
        let missing = store.workspace_paths(&WorkspaceId::parse("yok").expect("an id"));
        assert!(set_backup_assets(&missing, true).is_err());
        fs::remove_dir_all(&root).expect("cleaned up");
    }

    #[test]
    fn adding_a_profile_to_a_workspace_that_is_not_there_is_a_diagnostic() {
        let root = scratch("add-missing");
        let store = Store::new(&root);
        let paths = store.workspace_paths(&WorkspaceId::parse("yok").expect("an id"));
        let error = add_profile(&paths, "claude-sub", today()).expect_err("nothing to add to");
        assert!(error.message.contains("workspace.qcode"), "{}", error.message);
        assert!(!paths.file.exists(), "no file is made up for a workspace that is not there");
    }

    #[cfg(unix)]
    #[test]
    fn a_store_that_cannot_be_written_is_a_diagnostic_and_not_a_panic() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("read-only");
        fs::create_dir_all(&root).expect("writable");
        let inner = root.join("QCode");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).expect("a mode this user may set");

        let store = Store::new(&inner);
        let blocked = store.prepare().expect_err("the parent forbids it");
        assert!(blocked.to_string().contains(&inner.display().to_string()), "{blocked}");

        let listed = store.workspaces();
        assert!(listed.value.is_empty() && !listed.is_clean());

        assert!(matches!(store.create_workspace("Proje", today()), Err(NewWorkspaceError::Blocked(_))));

        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("a mode this user may set");
        fs::remove_dir_all(&root).expect("cleaned up");
    }
}
