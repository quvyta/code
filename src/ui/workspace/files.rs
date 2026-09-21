//! The file tree widget's state: what has been read of `Work/`, and what is open.
//!
//! Reading a directory is I/O, so it never happens while drawing. The screen asks for a folder
//! when it is opened and keeps the answer here; the tree is built from what is already known.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use qframe::prelude::*;
use qframe::runtime::Confirm;
use qframe::widgets::{Toast, TreeDrop};

use super::file_ops::{self, Change, FileError, NameProblem, is_within, name_of, parent_key};
use super::{Msg, OpenWorkspace, WorkspaceScreen};

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileEntry {
    /// Its name, without any folder before it.
    pub name: String,
    /// Whether it is a folder, and so can be opened.
    pub folder: bool,
}

/// What the file tree knows about the workspace's own folder.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileTree {
    root: PathBuf,
    children: BTreeMap<String, Vec<FileEntry>>,
    open: BTreeSet<String>,
    loading: BTreeSet<String>,
    selected: Option<String>,
    chosen: Vec<String>,
    error: Option<String>,
    cut: Vec<String>,
    naming: Option<Naming>,
}

/// What the name the dialog asks for is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameFor {
    /// A new, empty file.
    File,
    /// A new, empty folder.
    Folder,
    /// The entry of this key, which keeps its place and takes the new name.
    Rename(String),
}

/// The dialog that asks for a name, while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Naming {
    /// What the name is for.
    pub purpose: NameFor,
    /// The key of the folder the name is to be used in.
    pub folder: String,
    /// What has been typed.
    pub value: String,
    /// Whether the person tried to confirm; an empty name is only pointed out after that, so a
    /// dialog that has just opened does not start by scolding.
    pub tried: bool,
}

/// The key of the folder the tree is rooted at. Keys below it are paths relative to the root,
/// written with `/` whatever the platform, because they are identities rather than paths.
pub const ROOT: &str = "";

impl FileTree {
    /// A tree of the folder at `root`, with nothing read yet.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        // The workspace folder is the tree's top row, and it starts open: its entries are what the
        // tree is for.
        Self { root: root.into(), open: BTreeSet::from([ROOT.to_owned()]), ..Self::default() }
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

    /// The key of the entry the cursor is on: the row with the pillar, which the keys move from.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The keys of every selected entry, the cursor's among them unless it was taken out.
    #[must_use]
    pub fn chosen(&self) -> &[String] {
        &self.chosen
    }

    /// Why the folder could not be read, when it could not be.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Makes `key` the one selected entry, with the cursor on it.
    pub fn select(&mut self, key: &str) {
        self.selected = Some(key.to_owned());
        self.chosen = vec![key.to_owned()];
    }

    /// Moves the cursor to `key`, leaving what is selected to [`choose`](Self::choose): the tree
    /// reports the two apart, the selection right after the cursor.
    pub fn move_cursor(&mut self, key: &str) {
        self.selected = Some(key.to_owned());
    }

    /// Makes `keys` the whole selection.
    pub fn choose(&mut self, keys: Vec<String>) {
        self.chosen = keys;
    }

    /// What an action asked for on the row `key` acts on: the whole selection when the row is
    /// one of several selected, and the row alone otherwise, the way the tree's menu treats a
    /// right click. An entry inside a folder that is also taken is left out, because it goes
    /// wherever the folder goes; the workspace folder itself is never taken.
    #[must_use]
    pub fn targets(&self, key: &str) -> Vec<String> {
        targets(&self.chosen, key)
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
        // A folder closed or gone while it was being read keeps nothing of the answer, so opening
        // it again, or a new folder of its name, reads afresh.
        if key != ROOT && !self.open.contains(key) {
            return;
        }
        match entries {
            Ok(entries) => {
                if key == ROOT {
                    self.error = None;
                }
                self.children.insert(key.to_owned(), entries);
                self.prune(key);
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

    /// The keys of the entries that were cut and wait to be pasted.
    #[must_use]
    pub fn cut(&self) -> &[String] {
        &self.cut
    }

    /// Whether the entry `key` was cut, or is inside a folder that was.
    #[must_use]
    pub fn is_cut(&self, key: &str) -> bool {
        self.cut.iter().any(|cut| is_within(key, cut))
    }

    /// The dialog asking for a name, while it is open.
    #[must_use]
    pub fn naming(&self) -> Option<&Naming> {
        self.naming.as_ref()
    }

    /// Whether the entry `key` is a folder, as far as the tree has read.
    #[must_use]
    pub fn is_folder(&self, key: &str) -> bool {
        let name = name_of(key);
        self.children(parent_key(key)).is_some_and(|entries| entries.iter().any(|e| e.folder && e.name == name))
    }

    /// The keys of every folder the tree has read so far, the root excepted.
    #[must_use]
    pub fn folder_keys(&self) -> BTreeSet<String> {
        self.children
            .iter()
            .flat_map(|(parent, entries)| {
                entries.iter().filter(|entry| entry.folder).map(move |entry| child_key(parent, &entry.name))
            })
            .collect()
    }

    /// What is wrong with the name typed so far, if anything; an empty name only once the person
    /// has tried to confirm it.
    #[must_use]
    pub fn naming_problem(&self) -> Option<NameProblem> {
        let naming = self.naming.as_ref()?;
        let siblings = self.children(&naming.folder).unwrap_or_default().iter().map(|entry| entry.name.as_str());
        let current = match &naming.purpose {
            NameFor::Rename(key) => Some(name_of(key)),
            NameFor::File | NameFor::Folder => None,
        };
        let problem = file_ops::check_name(&naming.value, siblings, current).err()?;
        (problem != NameProblem::Empty || naming.tried).then_some(problem)
    }

    /// The folders whose entries are shown: the root and every open folder that has been read.
    fn shown_folders(&self) -> Vec<String> {
        std::iter::once(ROOT.to_owned())
            .chain(self.open.iter().filter(|key| *key != ROOT && self.children.contains_key(*key)).cloned())
            .collect()
    }

    /// The folders whose rows are on screen: every open folder whose folders above it are all
    /// open too, the workspace folder first when it is. A folder left open inside a closed one is
    /// remembered, not shown.
    #[must_use]
    pub fn visible_folders(&self) -> Vec<String> {
        self.open
            .iter()
            .filter(|key| {
                let mut above = key.as_str();
                while above != ROOT {
                    above = parent_key(above);
                    if !self.open.contains(above) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect()
    }

    /// Forgets what the tree held below the folder `key` that its entries, just read, no longer
    /// have: an entry removed or moved away by another program leaves no open folder, cut or
    /// selection behind that would act on a name that is not there.
    fn prune(&mut self, key: &str) {
        let Some(entries) = self.children.get(key) else { return };
        let names: BTreeSet<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        let held = self
            .open
            .iter()
            .chain(self.children.keys())
            .chain(&self.loading)
            .chain(&self.cut)
            .chain(&self.chosen)
            .chain(&self.selected);
        let gone: BTreeSet<String> = held
            .filter(|held| *held != key && (key == ROOT || is_within(held, key)))
            .map(|held| {
                let below = if key == ROOT { held.as_str() } else { &held[key.len() + 1..] };
                below.split('/').next().unwrap_or(below)
            })
            .filter(|name| !names.contains(name))
            .map(|name| child_key(key, name))
            .collect();
        for child in gone {
            self.forget(&child);
        }
    }

    /// Gives every key at or below `from` the place `to` instead, after a rename or a move, so an
    /// open folder stays open and the selection stays on what moved.
    fn rekey(&mut self, from: &str, to: &str) {
        let moved = |key: &str| is_within(key, from).then(|| format!("{to}{}", &key[from.len()..]));
        self.open = self.open.iter().map(|key| moved(key).unwrap_or_else(|| key.clone())).collect();
        self.loading.retain(|key| !is_within(key, from));
        self.children = std::mem::take(&mut self.children)
            .into_iter()
            .map(|(key, entries)| (moved(&key).unwrap_or(key), entries))
            .collect();
        for key in self.selected.iter_mut().chain(&mut self.cut).chain(&mut self.chosen) {
            if let Some(new) = moved(key) {
                *key = new;
            }
        }
    }

    /// Forgets everything at or below `key`, after it was deleted. The selection goes to the
    /// folder it was in, the nearest thing still there.
    fn forget(&mut self, key: &str) {
        self.open.retain(|open| !is_within(open, key));
        self.loading.retain(|loading| !is_within(loading, key));
        self.children.retain(|folder, _| !is_within(folder, key));
        self.cut.retain(|cut| !is_within(cut, key));
        self.chosen.retain(|chosen| !is_within(chosen, key));
        if self.selected.as_deref().is_some_and(|selected| is_within(selected, key)) {
            let parent = parent_key(key);
            self.selected = (parent != ROOT).then(|| parent.to_owned());
        }
    }

    /// The path on disk of the entry `key`.
    #[must_use]
    pub fn path(&self, key: &str) -> PathBuf {
        key.split('/').filter(|part| !part.is_empty()).fold(self.root.clone(), |path, part| path.join(part))
    }
}

/// What an action on the row `key` acts on when `chosen` is selected; see [`FileTree::targets`].
#[must_use]
pub fn targets(chosen: &[String], key: &str) -> Vec<String> {
    let keys = if chosen.len() > 1 && chosen.iter().any(|chosen| chosen == key) {
        chosen.to_vec()
    } else {
        vec![key.to_owned()]
    };
    outermost(keys)
}

/// `keys` without any key inside a folder that is also among them, and without the workspace
/// folder, in their order.
fn outermost(keys: Vec<String>) -> Vec<String> {
    let all = keys.clone();
    keys.into_iter()
        .filter(|key| key != ROOT && !all.iter().any(|other| other != key && is_within(key, other)))
        .collect()
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

/// What can happen to the files of the file tree.
#[derive(Debug, Clone)]
pub enum FileMsg {
    /// A new file was asked for in the folder of this key.
    NewFile(String),
    /// A new folder was asked for in the folder of this key.
    NewFolder(String),
    /// The entry of this key was asked to take another name.
    Rename(String),
    /// The entries of these keys became the selection.
    Choose(Vec<String>),
    /// The entry of this key was cut, with the rest of the selection when it is part of it, to
    /// be pasted into another folder.
    Cut(String),
    /// What was cut was asked to go into the folder of this key.
    Paste(String),
    /// What was cut is to stay where it is after all.
    DropCut,
    /// Entries were dragged onto a folder, or onto the free space that stands for the workspace
    /// folder.
    Drop(TreeDrop),
    /// The entry of this key was asked to be deleted, with the rest of the selection when it is
    /// part of it; the person is asked first.
    Delete(String),
    /// The person said yes to deleting the entries of these keys.
    DeleteConfirmed(Vec<String>),
    /// The open folders were asked to be read again.
    Refresh,
    /// The entry of this key, with the rest of the selection when it is part of it, was asked to
    /// be left out of the workspace's backup, or taken into it again when `false`.
    LeaveOut(String, bool),
    /// The name in the dialog changed.
    Name(String),
    /// The name in the dialog was confirmed.
    Submit,
    /// The dialog was closed without a name.
    CloseNaming,
    /// Operations on a workspace's files finished or were refused: the workspace's id, and for each
    /// entry its key and what came of it.
    Done(String, Vec<(String, Result<Change, FileError>)>),
}

/// Applies a message of the file tree to the open workspace.
pub(super) fn update(screen: &mut WorkspaceScreen, message: FileMsg) -> Command<Msg> {
    if let FileMsg::Done(id, results) = message {
        let Some(workspace) = screen.workspace_mut(&id) else { return Command::none() };
        return done(workspace, results);
    }
    let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
    match message {
        FileMsg::NewFile(folder) => ask_name(workspace, NameFor::File, folder, String::new()),
        FileMsg::NewFolder(folder) => ask_name(workspace, NameFor::Folder, folder, String::new()),
        FileMsg::Rename(key) => {
            let (folder, name) = (parent_key(&key).to_owned(), name_of(&key).to_owned());
            ask_name(workspace, NameFor::Rename(key), folder, name)
        }
        FileMsg::Choose(keys) => {
            workspace.files.choose(keys);
            Command::none()
        }
        FileMsg::Cut(key) => {
            workspace.files.cut = workspace.files.targets(&key);
            Command::none()
        }
        FileMsg::DropCut => {
            workspace.files.cut.clear();
            Command::none()
        }
        FileMsg::Paste(into) => {
            let keys = workspace.files.cut.clone();
            move_all(workspace, keys, into)
        }
        FileMsg::Drop(TreeDrop { keys, into }) => move_all(workspace, outermost(keys), into.unwrap_or_default()),
        FileMsg::Delete(key) => ask_delete(&workspace.files, workspace.files.targets(&key)),
        FileMsg::DeleteConfirmed(keys) => {
            run_each(workspace, keys, |root, key| (key.to_owned(), file_ops::delete(root, key)))
        }
        FileMsg::Refresh => refresh(workspace),
        FileMsg::LeaveOut(key, out) => {
            let keys = workspace.files.targets(&key);
            super::backups::leave_out(workspace, keys, out)
        }
        FileMsg::Name(value) => {
            if let Some(naming) = &mut workspace.files.naming {
                naming.value = value;
            }
            Command::none()
        }
        FileMsg::Submit => submit(workspace),
        FileMsg::CloseNaming => {
            workspace.files.naming = None;
            Command::none()
        }
        FileMsg::Done(..) => Command::none(),
    }
}

/// Opens the dialog asking for a name in `folder`, opening the folder too: the new entry will
/// be shown there, and the names already in it are what the name is checked against.
fn ask_name(workspace: &mut OpenWorkspace, purpose: NameFor, folder: String, value: String) -> Command<Msg> {
    let read = folder != ROOT && workspace.files.expand(&folder, true);
    let command = if read { super::read_folder(workspace, &folder) } else { Command::none() };
    workspace.files.naming = Some(Naming { purpose, folder, value, tried: false });
    command
}

/// Takes the name in the dialog, or points out what is wrong with it.
fn submit(workspace: &mut OpenWorkspace) -> Command<Msg> {
    let Some(naming) = workspace.files.naming.as_mut() else { return Command::none() };
    naming.tried = true;
    if workspace.files.naming_problem().is_some() {
        return Command::none();
    }
    let Some(Naming { purpose, folder, value: name, .. }) = workspace.files.naming.take() else {
        return Command::none();
    };
    match purpose {
        NameFor::File => run_each(workspace, vec![folder], move |root, folder| {
            (child_key(folder, &name), file_ops::create_file(root, folder, &name))
        }),
        NameFor::Folder => run_each(workspace, vec![folder], move |root, folder| {
            (child_key(folder, &name), file_ops::create_folder(root, folder, &name))
        }),
        // The same name again is nothing to do, not a clash with itself.
        NameFor::Rename(key) if name_of(&key) == name => Command::none(),
        NameFor::Rename(key) => {
            run_each(workspace, vec![key], move |root, key| (key.to_owned(), file_ops::rename(root, key, &name)))
        }
    }
}

/// Moves the entries `keys` into the folder `into`, each on its own: one that cannot go does not
/// keep the others from going, and the answer says which stayed and why.
fn move_all(workspace: &OpenWorkspace, keys: Vec<String>, into: String) -> Command<Msg> {
    if keys.is_empty() {
        return Command::none();
    }
    run_each(workspace, keys, move |root, key| (key.to_owned(), file_ops::move_into(root, key, &into)))
}

/// How many names a question lists before it only counts the rest.
const NAMES_SHOWN: usize = 5;

/// Asks before deleting, which cannot be undone; a folder says it takes everything in it along.
fn ask_delete(files: &FileTree, keys: Vec<String>) -> Command<Msg> {
    let folders = keys.iter().any(|key| files.is_folder(key));
    let (title, message) = match keys.as_slice() {
        [] => return Command::none(),
        [key] => {
            let name = name_of(key);
            let text = if folders { "workspace.files.delete-folder-text" } else { "workspace.files.delete-file-text" };
            (t!("workspace.files.delete-title", name = name), t!(text, name = name))
        }
        many => {
            let mut names = many.iter().take(NAMES_SHOWN).map(|key| name_of(key)).collect::<Vec<_>>().join(", ");
            if many.len() > NAMES_SHOWN {
                names = format!("{names} {}", t!("workspace.files.and-more", n = many.len() - NAMES_SHOWN));
            }
            let text =
                if folders { "workspace.files.delete-many-folders-text" } else { "workspace.files.delete-many-text" };
            (t!("workspace.files.delete-many-title", n = many.len()), t!(text, names = names.as_str()))
        }
    };
    let label =
        if keys.len() > 1 { t!("workspace.files.delete-many", n = keys.len()) } else { t!("workspace.files.delete") };
    Command::confirm(
        Confirm::new(title, Msg::Files(FileMsg::DeleteConfirmed(keys))).message(message).confirm_label(label).danger(),
    )
}

/// Runs a file operation for each of `items` on a background thread, on the host, inside the
/// workspace folder, one after the other: a move of several entries is done in the order they
/// were given, so a clash between two of them is decided the same way every time.
fn run_each(
    workspace: &OpenWorkspace,
    items: Vec<String>,
    work: impl Fn(&Path, &str) -> (String, Result<Change, FileError>) + Send + 'static,
) -> Command<Msg> {
    let id = workspace.id.as_str().to_owned();
    let root = workspace.files.root().to_path_buf();
    Command::perform(move || Msg::Files(FileMsg::Done(id, items.iter().map(|item| work(&root, item)).collect())))
}

/// Takes what the operations changed: the tree follows it, the folders they touched are read
/// again and the selection goes to what was made or moved. What was refused is said, entry by
/// entry when there were several.
fn done(workspace: &mut OpenWorkspace, results: Vec<(String, Result<Change, FileError>)>) -> Command<Msg> {
    let files = &mut workspace.files;
    let total = results.len();
    let mut touched = Vec::new();
    let mut arrived = Vec::new();
    let mut refused = Vec::new();
    for (key, result) in results {
        match result {
            Err(error) => refused.push((key, error)),
            Ok(Change::Created(key)) => {
                touched.push(parent_key(&key).to_owned());
                arrived.push(key);
            }
            Ok(Change::Moved(from, to)) => {
                // A cut entry that went somewhere is used up; one that was refused stays cut, to
                // be tried elsewhere.
                files.cut.retain(|cut| *cut != from);
                if from == to {
                    continue;
                }
                files.rekey(&from, &to);
                let parent = parent_key(&to).to_owned();
                if parent != ROOT {
                    files.open.insert(parent.clone());
                }
                touched.extend([parent_key(&from).to_owned(), parent]);
                arrived.push(to);
            }
            Ok(Change::Deleted(key)) => {
                files.forget(&key);
                touched.push(parent_key(&key).to_owned());
            }
        }
    }
    if let Some(first) = arrived.first() {
        files.selected = Some(first.clone());
        files.chosen = arrived;
    }
    touched.sort();
    touched.dedup();
    Command::batch([refusal(total, &refused), reread(workspace, touched)])
}

/// Says what was refused: the reason alone when there was one entry, and which entries stayed
/// with each one's reason when there were several.
fn refusal(total: usize, refused: &[(String, FileError)]) -> Command<Msg> {
    match refused {
        [] => Command::none(),
        [(_, error)] if total == 1 => Command::toast(Toast::danger(t!("workspace.files.failed")).body(error.message())),
        _ => {
            let lines: Vec<String> =
                refused.iter().map(|(key, error)| format!("{}: {}", name_of(key), error.message())).collect();
            let title = t!("workspace.files.failed-some", failed = refused.len(), total = total);
            Command::toast(Toast::danger(title).body(lines.join("\n")))
        }
    }
}

/// Reads the folders `keys` again, each while it is still shown.
pub(super) fn reread(workspace: &mut OpenWorkspace, keys: Vec<String>) -> Command<Msg> {
    let keys: Vec<String> = keys.into_iter().filter(|key| key == ROOT || workspace.files.is_open(key)).collect();
    let commands: Vec<Command<Msg>> = keys
        .into_iter()
        .map(|key| {
            workspace.files.loading.insert(key.clone());
            super::read_folder(workspace, &key)
        })
        .collect();
    Command::batch(commands)
}

/// Reads again every folder the tree shows: when the person asks, when the screen is returned to,
/// and when a watch says anything may have changed. Where folders cannot be watched this is how
/// the tree catches up, since reading at intervals is ruled out.
pub(super) fn refresh(workspace: &mut OpenWorkspace) -> Command<Msg> {
    let shown = workspace.files.shown_folders();
    reread(workspace, shown)
}

/// Whether `key` names an entry inside the tree's root: a path of plain names, none of them empty,
/// `.` or `..`, and not starting at `/`.
///
/// Keys come from the tree itself, which only ever joins the names a folder listed, but they also
/// come back from the session file, which anyone can edit. A key that climbs out of the workspace
/// would open a file of the host in a tab, or hand the container a path outside the one folder it
/// was given, so it is refused wherever a key becomes a path.
#[must_use]
pub fn is_inside(key: &str) -> bool {
    !key.is_empty() && !key.contains('\0') && key.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The name of the entry `key`, without the folders before it.
#[must_use]
pub fn name(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// Why a document of the workspace could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentTrouble {
    /// There is no file there any more.
    Missing,
    /// The file is a link that leads out of the workspace folder.
    Outside,
    /// What the operating system said.
    Unreadable(String),
}

/// Reads the text of the file at `path`, which must lie inside `root` once every link on the way
/// is followed.
///
/// The key was checked already; what is checked here is the one thing a key cannot show: a link
/// in the workspace that points somewhere else on this machine. Text that is not UTF-8 is shown
/// with its broken bytes replaced rather than not at all.
///
/// This touches the disk, so it belongs on a background thread.
///
/// # Errors
///
/// When the file is gone, leads out of `root`, or cannot be read.
pub fn read_document(path: &Path, root: &Path) -> Result<String, DocumentTrouble> {
    let trouble = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => DocumentTrouble::Missing,
        _ => DocumentTrouble::Unreadable(error.to_string()),
    };
    let real = path.canonicalize().map_err(trouble)?;
    let root = root.canonicalize().map_err(trouble)?;
    if !real.starts_with(&root) {
        return Err(DocumentTrouble::Outside);
    }
    let bytes = std::fs::read(&real).map_err(trouble)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
