//! The workspace screen: the rail of open workspaces on the left, the tabs of one workspace on top,
//! the container's terminal in the middle and the widget panel on the right.
//!
//! The rail holds the workspaces the person opened and nothing else, the way a browser holds its
//! windows: a workspace joins it when it is opened from the list, leaves it when it is closed, and
//! the rail and every tab in it are what "Continue" on the home screen brings back.
//!
//! The screen's whole reason for existing is one line of [`ContainerPlan::enter`]: a tab spawns
//! the engine binary with `exec`, so the program a tab shows always runs inside a container and
//! never on the machine QCode runs on. Everything else here is around that: which container a tab
//! enters, what a tab says when its container is not there, and what the panel shows.
//!
//! The screen speaks its own [`Msg`]: [`update`] answers with commands of it and [`view`] draws
//! for it, and the application maps both to its own message, so the screen is drawn and tested
//! without naming anything of the application.

mod backups;
mod blank;
mod bridge;
mod desktop;
mod file_ops;
mod files;
mod history;
mod panel;
mod plan;
mod relay;
mod sound;
mod tab;
mod viewer;
mod watch;

#[cfg(test)]
mod live;
#[cfg(test)]
mod tests;

pub(crate) use backups::bytes;
pub use backups::{BackupOf, BackupTrouble, Farewell, Leaving, Part, leaving};
pub use blank::Choice;
pub use bridge::{Letter, Letters, Undelivered};
pub use desktop::Opening;
pub use file_ops::{Change, FileError, NameProblem};
pub use files::{DocumentTrouble, FileEntry, FileMsg, FileTree, NameFor, Naming};
pub use history::HistoryKey;
pub use panel::{Panel, PanelWidget};
pub use plan::{
    ASSETS_DIR, Bridge, CODE_DIR, ContainerPlan, HOME_DIR, KEEP_ALIVE, LaunchFailure, MCP_DIR, PLAN_LABEL, SHELL,
    ensure_running, open_page,
};
pub use tab::{Back, Pages, Shown, Tab, TabKey, TabKind, TabState};
pub use viewer::{MOST_TEXT, Taken};

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

use qframe::date::Date;
use qframe::icons::Icons;
use qframe::keymap::KeyChord;
use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::storage::FolderChange;
use qframe::widgets::{
    CollapsedMarker, EmptyState, LogView, Markdown, RailTab, ScrollView, Side, SidePanel, Spinner, TabEdit, TabRail,
    TabWidth, Terminal, TerminalEvent, TerminalSession, Toast,
};

use std::path::{Path, PathBuf};

use crate::backup::conversations::Brought;
use crate::backup::{BackupEvery, Entry, Snapshot, SnapshotId};
use crate::base::apps::{self, Editor, FileKind, Quiet, Sound};
use crate::bridge::config::{self as bridge_config, Unregistered};
use crate::bridge::rules::Rules;
use crate::bridge::socket::Call;
use crate::engine::{Container, Engine, EngineCommand, EngineKind, Exec, HostUser};
use crate::profile::guidance::Unguided;
use crate::profile::history::Conversation;
use crate::profile::{HarnessKind, Profile, ProviderChoice};
use crate::store::{
    Registry, Session, SessionTab, SessionTabKind, SessionWorkspace, WorkspaceFile, WorkspaceId, WorkspacePaths,
    add_profile,
};
use crate::ui::profiles::work::Problem;
use crate::ui::settings::engine::{Help, help, name as engine_name};

/// Width of the workspace rail: the framework's collapsed strip, which is the same four cells on
/// every screen. The rail never gives way to a narrow terminal, because losing it would mean
/// losing the only way between workspaces.
const RAIL_WIDTH: u16 = 4;

/// Height of one workspace block in the rail, in lines.
const RAIL_ROWS: u16 = 3;

/// Width of the widget panel when it is opened, and the range it can be dragged through. The
/// framework keeps the terminal a quarter of the screen whatever is asked for here.
const PANEL_WIDTH: u16 = 32;
/// Narrowest the widget panel can be dragged.
const PANEL_MIN: u16 = 18;
/// Widest the widget panel can be dragged.
const PANEL_MAX: u16 = 52;

/// Everything that can happen on the workspace screen.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A workspace of the rail was opened.
    OpenWorkspace(usize),
    /// A workspace was closed from the rail, with every tab in it.
    CloseWorkspace(usize),
    /// The list of workspaces was asked for, to open another one beside these. The screen itself
    /// cannot go there; the application that holds both answers this message.
    AddWorkspace,
    /// A tab was opened.
    OpenTab(usize),
    /// A tab was closed.
    CloseTab(usize),
    /// A tab was dragged to another place.
    MoveTab {
        /// Where it was.
        from: usize,
        /// Where it lands.
        to: usize,
    },
    /// A blank tab was asked for, to choose in it what it opens.
    NewTab,
    /// The keyboard was asked out of the harness, onto the tab strip.
    LeaveTerminal,
    /// The keyboard was asked back into the open tab's harness, from wherever it was.
    EnterTerminal,
    /// A row of a blank tab's page was chosen: the blank tab of this key turns into it.
    Choose(TabKey, Choice),
    /// The keyboard moved to another row of a blank tab's page.
    HighlightChoice(usize),
    /// Every conversation of a profile was asked for on a blank tab's page, not only the newest.
    ShowAll(HistoryKey),
    /// A reading of a profile's conversations answered: the reading's number, and the
    /// conversations newest first or why they could not be read.
    HistoryRead(HistoryKey, u64, Result<Vec<Conversation>, String>),
    /// A reading of a profile's conversations, by its number, has run long enough to be noticed.
    HistorySlow(HistoryKey, u64),
    /// The indicator of a reading has been on screen long enough to be read.
    HistorySettled(HistoryKey),
    /// A new profile was asked for, from the blank tab's "New profile" row: the application opens
    /// the profiles screen with its wizard already open. The screen itself cannot go there; the
    /// application that holds both answers this message.
    ManageProfiles,
    /// The profiles of the store were read again, after the person may have changed them.
    Profiles(Vec<Profile>),
    /// A profile was recorded in a workspace's `workspace.qcode`, or could not be.
    ProfileAdded(String, Result<WorkspaceFile, String>),
    /// The container of a tab is up, or the engine refused.
    Ready(TabKey, u64, Result<(), LaunchFailure>),
    /// Build the image of the profile of this tab, which the engine does not have.
    BuildImage(TabKey),
    /// A line the build of a tab's image said.
    ImageLine(TabKey, String),
    /// The build of a tab's image ended: done, or what went wrong in the engine's words, or
    /// `None` when the person stopped it.
    ImageBuilt(TabKey, Result<(), Option<String>>),
    /// Stop building a tab's image.
    StopBuild(TabKey),
    /// Do not open this tab after all: its image is missing and the person would rather not
    /// build it now.
    CancelOpen(TabKey),
    /// The container of a new-chat tab brought back from the last session is up, or the engine
    /// refused; with the profile's conversations when they could be read, to find the one the
    /// tab was showing.
    Woken(TabKey, u64, Result<(), LaunchFailure>, Option<Vec<Conversation>>),
    /// A tab's session printed something or ended.
    Output(TabKey, u64, TerminalEvent),
    /// Whether the container of a tab whose session ended is still running.
    Checked(TabKey, u64, bool),
    /// A tab was asked to start again.
    Restart(TabKey),
    /// The widget panel was opened or closed.
    TogglePanel(bool),
    /// The widget panel was dragged to another width.
    ResizePanel(u16),
    /// A widget of the panel was opened or closed.
    ToggleWidget(usize, bool),
    /// A widget of the panel was dragged to another place.
    MoveWidget {
        /// Where it was.
        from: usize,
        /// Where it lands.
        to: usize,
    },
    /// The chooser of widgets the panel does not carry was shown or dismissed.
    ShowWidgets(bool),
    /// The keyboard moved to another row of the chooser of widgets.
    HighlightWidget(usize),
    /// A widget was added to the panel.
    AddWidget(PanelWidget),
    /// A widget was taken off the panel.
    RemoveWidget(PanelWidget),
    /// The cursor of the file tree moved to an entry; what is selected comes as
    /// [`FileMsg::Choose`].
    SelectFile(String),
    /// A folder of the file tree was opened or closed.
    ExpandFile(String, bool),
    /// A folder of a workspace's file tree was read.
    FolderRead(String, String, Result<Vec<FileEntry>, String>),
    /// Something happened to the files of the file tree.
    Files(FileMsg),
    /// Other programs changed folders a workspace's tree shows: the workspace's id, the number of
    /// the watch that saw it, and what changed.
    FilesChanged(String, u64, Vec<FolderChange>),
    /// A file of the file tree was opened: Enter or a click on it.
    OpenFile(String),
    /// A file was asked for in the editor, from the Markdown tab that shows it.
    EditFile(String),
    /// The file the tab of this key opens is not in the workspace any more.
    Missing(TabKey, u64),
    /// The document a Markdown tab shows was read, or could not be.
    DocumentRead(TabKey, u64, Result<String, DocumentTrouble>),
    /// The text of the file a tab shows was taken out in the container, or could not be.
    TextRead(TabKey, u64, Result<Taken, LaunchFailure>),
    /// A page of the PDF the tab of this key shows was asked for as a picture, counted from 1,
    /// or with `None` its text again.
    Page(TabKey, Option<usize>),
    /// The sound of a tab can be played through the sound server's socket at this path, or the
    /// base image it plays in could not be made.
    Playable(TabKey, u64, Result<PathBuf, LaunchFailure>),
    /// The details of the sound of a tab were read, or could not be, and why they are shown
    /// rather than the sound played.
    Described(TabKey, u64, Quiet, Result<Taken, LaunchFailure>),
    /// The containers of sound tabs that were closed while playing were taken away.
    Silenced,
    /// The window of a tab is open, or it waits to be asked, or the engine refused.
    WindowOpened(TabKey, u64, Result<Opening, LaunchFailure>),
    /// The window of a tab closed, whoever closed it, with the exit code the engine reported.
    WindowEnded(TabKey, u64, Option<u32>),
    /// The window of this tab, in this run, asked for these web addresses to be opened.
    SignInWanted(TabKey, u64, Vec<String>),
    /// The opening of that address answered, saying where it was shown.
    SignInOpened(TabKey, u64, String, Shown),
    /// The listening for the way back of a sign-in, the one of that id, ended.
    SignInBack(TabKey, u64, u64, crate::desktop::callback::Ending),
    /// The person asked for the page of the sign-in in the window inside the container instead.
    SignInHere(TabKey),
    /// The open window of a tab was asked to show itself.
    RaiseWindow(TabKey),
    /// That asking finished; a refusal is worth saying, because nothing else would show it.
    WindowRaised(Result<(), LaunchFailure>),
    /// The window of a tab was asked to close.
    CloseWindow(TabKey),
    /// The containers of windows that were closed have been taken away, or one of them would not
    /// go and this is the engine's word on it.
    WindowsClosed(Option<LaunchFailure>),
    /// A row of the container widget was selected.
    SelectContainer(usize),
    /// The container widget was asked for the engine's current answer.
    RefreshContainers,
    /// The containers of a workspace came back from the engine.
    ContainersRead(String, Result<Vec<Container>, LaunchFailure>),
    /// A container was asked to stop.
    StopContainer(String),
    /// A container was asked to stop and start again.
    RestartContainer(String),
    /// An engine command on a container finished.
    ContainerActed(Result<(), LaunchFailure>),
    /// A container QCode started could not be noted cleanly in the list of the containers it
    /// started; the message that came with the start follows.
    Noted(Noting, Box<Msg>),
    /// It is time to back a workspace up: the workspace's id and the number of the timer that went
    /// off.
    BackupDue(String, u64),
    /// A backup of a workspace finished.
    BackedUp {
        /// The workspace's id.
        workspace: String,
        /// Its name, to say a failure by after the workspace has left the rail.
        name: String,
        /// What came of the workspace's own snapshot.
        made: Result<Snapshot, BackupTrouble>,
        /// The other parts of the round that could not be backed up, and why.
        missed: Vec<(Part, BackupTrouble)>,
    },
    /// The newest backup of a workspace was asked after: the workspace's id and when it was made, or
    /// `None` when there is none or the backup could not be asked.
    NewestBackup(String, Option<i64>),
    /// How often the open workspaces are backed up was changed.
    BackupEvery(BackupEvery),
    /// What a workspace's backup leaves out was written into its `workspace.qcode`: the workspace's
    /// id, and the list as the file now holds it or why it could not be written.
    SkipWritten(String, Result<Vec<String>, String>),
    /// How much a workspace's `Backup/` holds was read: the workspace's id and the bytes.
    BackupSize(String, u64),
    /// The backup of the open workspace was asked to take `Assets/` too, or no longer to.
    BackupAssets(bool),
    /// Whether a workspace's backup takes its assets was written into its `workspace.qcode`: the
    /// workspace's id, and the choice as the file now holds it or why it could not be written.
    AssetsWritten(String, Result<bool, String>),
    /// The list of the open workspace's backups was asked for, or of the earlier versions of the
    /// file of this key.
    ShowBackups(Option<String>),
    /// The list of backups was read: the number of the reading, and the backups newest first
    /// or why they could not be read.
    BackupsRead(u64, Result<Vec<Entry>, BackupTrouble>),
    /// A reading of the list of backups, by its number, has run long enough to be noticed.
    BackupsSlow(u64),
    /// The indicator of a reading of the list has been on screen long enough to be read.
    BackupsSettled(u64),
    /// The keyboard moved to another row of the list of backups.
    HighlightBackup(usize),
    /// The backup at this row of the list was chosen; the person is asked first.
    ChooseBackup(usize),
    /// The list of backups was made one of what it offers, at this position: the workspace's own
    /// files or a profile's conversations.
    BackupsOf(usize),
    /// The person said yes to bringing this backup back.
    RestoreBackup(SnapshotId),
    /// The person said yes to stopping the profile's container and then bringing this backup of
    /// its conversations back.
    StopAndRestore(SnapshotId),
    /// The person said no to bringing a backup back.
    KeepBackup,
    /// The list of backups was closed.
    CloseBackups,
    /// A call came to the bridge of the workspace of this id, on the listener of this number, or
    /// the listener was closed.
    Bridge(String, u64, Option<Call>),
    /// The person answered whether the agent of one tab may send messages to another's.
    Allow {
        /// The sending tab.
        from: TabKey,
        /// The receiving tab.
        to: TabKey,
        /// Whether it may.
        allow: bool,
    },
    /// The question about one tab sending to another has waited long enough that the senders
    /// are told their messages wait.
    StillAsking {
        /// The sending tab.
        from: TabKey,
        /// The receiving tab.
        to: TabKey,
    },
    /// Something was done with the messages waiting in a tab.
    Letters(TabKey, Letters),
    /// Time to look again at the tabs that still hold messages for their agents.
    Deliver,
    /// The bridge could not be registered in the settings of a harness whose container came up;
    /// the message that came with the start follows.
    Unbridged(HarnessKind, Unregistered, Box<Msg>),
    /// The instruction files of a QCode high profile whose container came up could not be brought
    /// up to date in the workspace; the message that came with the start follows.
    Unguided(HarnessKind, Unguided, Box<Msg>),
    /// Bringing a backup back finished.
    Restored {
        /// The workspace's id.
        workspace: String,
        /// Its name, to say what came back by.
        name: String,
        /// What was brought back.
        of: BackupOf,
        /// The file that was brought back, or `None` for all of it.
        file: Option<String>,
        /// What came of it.
        done: Result<Brought, BackupTrouble>,
    },
}

/// What went wrong noting a container QCode started, so that it can be stopped once no QCode is
/// open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Noting {
    /// The list could not be written: the container is not in it, and may keep running after
    /// QCode closes. The machine's words.
    Unwritten(String),
    /// The list was damaged; what could be read of it was kept along with the new name. Where
    /// it was damaged.
    Repaired(String),
}

/// One workspace the rail can switch to, with its own tabs and its own panel content.
#[derive(Debug)]
pub struct OpenWorkspace {
    id: WorkspaceId,
    name: String,
    paths: WorkspacePaths,
    profiles: Vec<Profile>,
    carried: Vec<String>,
    tabs: Vec<Tab>,
    active_tab: usize,
    /// The conversations of each profile, by the profile's name, as the blank tab's page last
    /// read them.
    history: HashMap<String, history::Shelf>,
    files: FileTree,
    /// Whether the folders the tree shows are watched for changes other programs make.
    live: watch::Live,
    containers: Vec<Container>,
    container_row: usize,
    container_error: Option<LaunchFailure>,
    busy: bool,
    /// When the workspace was backed up last, and when it is backed up next.
    backup: backups::State,
    /// The folders and files inside the workspace its backup leaves out, as its file lists them.
    skip: Vec<String>,
    /// Whether its backup takes `Assets/` too, as its file says.
    assets: bool,
    /// Whether the workspace listens for its tabs' agents.
    link: bridge::Link,
    /// Whether the workspace's provider relay is listening, for a tab of a profile that runs on
    /// a provider of the person's own.
    relay: relay::Link,
}

impl OpenWorkspace {
    /// A workspace as its own file describes it, living at `paths`, offering `profiles`.
    ///
    /// The profiles are every profile of the store, already read: this screen shows
    /// profiles and enters their containers, it does not go looking for their definitions. The
    /// ones the workspace's `workspace.qcode` names come first; any other one is added to the workspace
    /// the first time a tab of it is opened, so a profile made after the workspace needs no hand
    /// edit to be used in it.
    #[must_use]
    pub fn new(file: &WorkspaceFile, paths: WorkspacePaths, profiles: Vec<Profile>) -> Self {
        let files = FileTree::new(paths.code.clone());
        let carried = file.profiles.iter().map(|profile| profile.name.clone()).collect();
        let mut workspace = Self {
            id: file.id.clone(),
            name: file.name.clone(),
            paths,
            profiles: Vec::new(),
            carried,
            tabs: Vec::new(),
            active_tab: 0,
            history: HashMap::new(),
            files,
            live: watch::Live::Off,
            containers: Vec::new(),
            container_row: 0,
            container_error: None,
            busy: false,
            backup: backups::State::default(),
            skip: file.backup_skip.clone(),
            assets: file.backup_assets,
            link: bridge::Link::default(),
            relay: relay::Link::default(),
        };
        workspace.set_profiles(profiles);
        workspace
    }

    /// Takes `profiles` as the store's profiles, the ones the workspace carries first in the
    /// order its file lists them, then the others in the order they came.
    fn set_profiles(&mut self, mut profiles: Vec<Profile>) {
        profiles.sort_by_key(|profile| {
            self.carried.iter().position(|name| name == profile.name.as_str()).unwrap_or(usize::MAX)
        });
        self.profiles = profiles;
    }

    /// The workspace's identifier, which is also what its containers are named after.
    #[must_use]
    pub fn id(&self) -> &str {
        self.id.as_str()
    }

    /// The name the person gave the workspace.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the workspace's folders are.
    #[must_use]
    pub fn paths(&self) -> &WorkspacePaths {
        &self.paths
    }

    /// The profiles a new tab can open: every profile of the store, the ones the workspace
    /// carries first.
    #[must_use]
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// Whether the workspace's `workspace.qcode` names the profile `name`.
    #[must_use]
    pub fn carries(&self, name: &str) -> bool {
        self.carried.iter().any(|carried| carried == name)
    }

    /// The tabs open in this workspace, in the order they are shown.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The open tab, when there is one.
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    /// What the file tree knows about the workspace's own folder.
    #[must_use]
    pub fn files(&self) -> &FileTree {
        &self.files
    }

    /// The folders and files inside the workspace its backup leaves out.
    #[must_use]
    pub fn backup_skip(&self) -> &[String] {
        &self.skip
    }

    /// Whether the workspace's backup takes `Assets/` too.
    #[must_use]
    pub fn backs_up_assets(&self) -> bool {
        self.assets
    }

    /// The containers of this workspace, as the engine last listed them.
    #[must_use]
    pub fn containers(&self) -> &[Container] {
        &self.containers
    }

    /// The container a tab of `kind` enters. A picture, a file in the editor, a PDF and an
    /// office document are opened in the workspace's own container, the one its shell runs in; a
    /// Markdown document needs none, and a sound plays in a container made for it each time it
    /// plays, which no tab enters.
    #[must_use]
    pub fn plan(&self, kind: &TabKind) -> Option<ContainerPlan> {
        match kind {
            TabKind::Shell | TabKind::Image(_) | TabKind::Editor(_) | TabKind::Pdf(_) | TabKind::Office(_) => {
                Some(ContainerPlan::base(self.id.as_str(), &self.paths))
            }
            TabKind::Markdown(_) | TabKind::Sound(_) => None,
            TabKind::Profile(name) => self
                .profiles
                .iter()
                .find(|profile| profile.name.as_str() == name)
                .map(|profile| ContainerPlan::profile(&self.id, &self.paths, profile)),
            // A window has a container of its own, beside the one the same profile's harness tabs
            // enter, so that the container's end and the window's are one thing.
            TabKind::Desktop(name) => self
                .profiles
                .iter()
                .find(|profile| profile.name.as_str() == name)
                .and_then(|profile| ContainerPlan::window(&self.id, &self.paths, profile)),
            TabKind::New => None,
        }
    }

    /// What `tab` runs inside its container: a login shell, the harness of the profile with the
    /// arguments that let it work without asking, opening the tab's conversation when it shows
    /// one and a new conversation otherwise, chafa drawing a picture or a page of a PDF, or
    /// `editor` on a file. A PDF whose text is shown runs nothing in a terminal.
    #[must_use]
    pub fn program(&self, tab: &Tab, editor: Editor) -> Option<Vec<String>> {
        match tab.kind() {
            TabKind::Shell => Some(plan::SHELL.iter().map(|part| (*part).to_owned()).collect()),
            TabKind::Profile(name) => {
                let profile = self.profiles.iter().find(|profile| profile.name.as_str() == name)?;
                let command = profile.harness.command_line(tab.conversation());
                // A profile that runs on a provider of the person's own is pointed at the relay's
                // address, so the relay has to be listening inside the container before the
                // harness says anything: it is what starts the harness.
                Some(if profile.provider.is_some() { crate::provider::relay::wrapping(&command) } else { command })
            }
            TabKind::Image(file) => Some(apps::picture(&inside_container(file))),
            TabKind::Editor(file) => Some(editor.command(&inside_container(file))),
            TabKind::Pdf(file) if tab.pages().drawn => Some(apps::pdf_page(&inside_container(file), tab.pages().page)),
            // A window is not run in a terminal, so it has no program a tab spawns: it is started
            // by its own container, which `desktop` does.
            TabKind::New
            | TabKind::Markdown(_)
            | TabKind::Pdf(_)
            | TabKind::Office(_)
            | TabKind::Sound(_)
            | TabKind::Desktop(_) => None,
        }
    }

    /// The label of the tab at `index`: the profile's name, `Shell` numbered among the shell tabs
    /// so several of them can be told apart, or `New tab` while it is blank.
    fn tab_label(&self, index: usize) -> String {
        let Some(tab) = self.tabs.get(index) else { return String::new() };
        match tab.kind() {
            TabKind::Profile(name) => name.clone(),
            // The window's tab is labelled by its profile like a harness tab, with the mark that
            // says it is a window rather than a terminal.
            TabKind::Desktop(name) => t!("workspace.tab.window", profile = name.clone()),
            TabKind::New => t!("workspace.new-tab"),
            TabKind::Image(file)
            | TabKind::Markdown(file)
            | TabKind::Editor(file)
            | TabKind::Pdf(file)
            | TabKind::Office(file)
            | TabKind::Sound(file) => files::name(file).to_owned(),
            TabKind::Shell => {
                let shell = t!("workspace.tab.shell");
                let position = self.tabs[..index].iter().filter(|tab| tab.kind() == &TabKind::Shell).count() + 1;
                let total = self.tabs.iter().filter(|tab| tab.kind() == &TabKind::Shell).count();
                if total > 1 { format!("{shell} {position}") } else { shell }
            }
        }
    }

    /// Whether the workspace listens for its tabs' agents.
    #[must_use]
    pub fn is_bridged(&self) -> bool {
        matches!(self.link, bridge::Link::On { .. })
    }

    /// The tab of `key` and where it sits.
    fn find(&mut self, key: TabKey) -> Option<(usize, &mut Tab)> {
        self.tabs.iter_mut().enumerate().find(|(_, tab)| tab.key() == key)
    }
}

/// Where the workspace's file `key` is inside a container: under [`CODE_DIR`], which is the
/// workspace's own folder mounted.
fn inside_container(key: &str) -> String {
    format!("{CODE_DIR}/{key}")
}

/// The workspace screen.
#[derive(Debug)]
pub struct WorkspaceScreen {
    engine: Option<Engine>,
    user: HostUser,
    workspaces: Vec<OpenWorkspace>,
    active: usize,
    panel: Panel,
    /// The row the keyboard rests on in a blank tab's page.
    blank_row: usize,
    /// The profiles whose every conversation a blank tab's page shows, not only the newest.
    expanded: HashSet<HistoryKey>,
    /// The blank tab whose page was shown last, so showing a page again is told apart from
    /// drawing the same one again.
    shown_blank: Option<TabKey>,
    /// The Markdown tab that was shown last, so a document is read again when it comes back into
    /// view and not on every frame it stays there.
    shown_document: Option<TabKey>,
    /// The editor a file opens in.
    editor: Editor,
    /// What opening a sound does.
    sound: Sound,
    /// This machine's runtime folder, where the sound server's socket is looked for.
    runtime: Option<PathBuf>,
    /// What this machine offers a window, or why it offers none. Read once when the screen is
    /// made: a session does not change its compositor underneath QCode.
    display: Result<crate::desktop::Display, crate::desktop::NoDisplay>,
    next_key: u64,
    /// Whether the open workspace's folders are watched, so the tree follows the disk.
    live: bool,
    /// The number the last folder watch was given.
    last_watch: u64,
    /// Where the containers this screen starts are noted, so they can be stopped once no QCode
    /// is open; `None` notes nothing.
    registry: Option<PathBuf>,
    /// Where `providers.toml` is read from before a tab of a provider profile is started, so its
    /// tag can be checked against the providers that exist right now rather than a stale copy
    /// the profile once carried; `None` means this machine has no data folder to hold one, so no
    /// provider profile can ever start.
    providers_path: Option<PathBuf>,
    /// Whether a problem with that list was said already: it is said once, not at every tab.
    registry_told: bool,
    /// How often the open workspaces are backed up.
    backup_every: BackupEvery,
    /// The number the last backup timer was given.
    last_timer: u64,
    /// The list of backups, while it is open.
    listing: Option<backups::Listing>,
    /// The number the last reading of a list of backups was given.
    last_listing: u64,
    /// Whether each open workspace listens for its tabs' agents.
    bridging: bool,
    /// The number the last listener of a workspace was given.
    last_link: u64,
    /// What the bridge remembers to apply its rules.
    rules: Rules,
    /// The questions to the person about one tab sending to another that are not answered yet.
    asking: Vec<bridge::Asking>,
    /// Whether a look at the tabs that hold messages is already timed, so that one is timed at a
    /// time and none at all while no tab holds a message.
    delivering: bool,
    /// Whether the person is asked before one tab first sends to another.
    ask_first: bool,
}

impl WorkspaceScreen {
    /// A screen showing `workspaces`, entering their containers through `engine`.
    ///
    /// Without an engine the screen still opens: the design asks that a missing engine leave the
    /// application usable, so every action that would touch a container is drawn faint and says
    /// why instead of failing when it is pressed.
    #[must_use]
    pub fn new(engine: Option<Engine>, user: HostUser, workspaces: Vec<OpenWorkspace>) -> Self {
        Self {
            engine,
            user,
            workspaces,
            active: 0,
            panel: Panel::default(),
            blank_row: 0,
            expanded: HashSet::new(),
            shown_blank: None,
            shown_document: None,
            editor: Editor::default(),
            sound: Sound::default(),
            runtime: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            display: desktop::display(),
            next_key: 0,
            live: false,
            last_watch: 0,
            registry: None,
            providers_path: crate::provider::Providers::file(),
            registry_told: false,
            backup_every: BackupEvery::default(),
            last_timer: 0,
            listing: None,
            last_listing: 0,
            bridging: false,
            last_link: 0,
            rules: Rules::default(),
            asking: Vec::new(),
            delivering: false,
            ask_first: false,
        }
    }

    /// The same screen following the disk when `live` is true: the folders the open workspace's
    /// tree shows are watched, and what other programs change in them shows at once.
    ///
    /// It is off unless asked for because a watch waits for the disk on a background thread, and
    /// a test harness runs background work in line, where that wait would never end. Tests of
    /// the watch take its batches by hand instead.
    #[must_use]
    pub fn watching(mut self, live: bool) -> Self {
        self.live = live;
        self
    }

    /// The same screen, listening for the agents of each open workspace's tabs when `bridging` is
    /// true, so they can send each other messages.
    ///
    /// It is off unless asked for for the same reason as [`WorkspaceScreen::watching`]: a listener
    /// waits on a background thread, which a test harness would run in line. Tests hand the
    /// screen its calls by hand instead.
    #[must_use]
    pub fn bridging(mut self, bridging: bool) -> Self {
        self.bridging = bridging;
        self
    }

    /// The same screen, noting every container it starts in the list at `path`, which is what
    /// lets them be stopped once no QCode is open. `None` notes nothing.
    #[must_use]
    pub fn with_registry(mut self, path: Option<PathBuf>) -> Self {
        self.registry = path;
        self
    }

    /// The same screen, reading `providers.toml` at `path` rather than this machine's own data
    /// folder before a provider tab starts, so a test can say what it holds without ever
    /// touching a real one.
    #[must_use]
    pub fn with_providers_path(mut self, path: Option<PathBuf>) -> Self {
        self.providers_path = path;
        self
    }

    /// The same screen, backing its workspaces up as often as `every` says.
    #[must_use]
    pub fn backing_up(mut self, every: BackupEvery) -> Self {
        self.backup_every = every;
        self
    }

    /// How often the open workspaces are backed up.
    #[must_use]
    pub fn backup_every(&self) -> BackupEvery {
        self.backup_every
    }

    /// The workspace the rail has open.
    #[must_use]
    pub fn workspace(&self) -> Option<&OpenWorkspace> {
        self.workspaces.get(self.active)
    }

    /// Every workspace of the rail, in its order.
    #[must_use]
    pub fn workspaces(&self) -> &[OpenWorkspace] {
        &self.workspaces
    }

    /// Makes `editor` the one a file opens in from now on. Tabs already open keep the editor
    /// they started with until they are opened again.
    pub fn set_editor(&mut self, editor: Editor) {
        self.editor = editor;
    }

    /// The editor a file opens in.
    #[must_use]
    pub fn editor(&self) -> Editor {
        self.editor
    }

    /// Makes `sound` what opening a sound does from now on. A sound tab already open keeps what
    /// it does until it is started again.
    pub fn set_sound(&mut self, sound: Sound) {
        self.sound = sound;
    }

    /// What opening a sound does.
    #[must_use]
    pub fn sound(&self) -> Sound {
        self.sound
    }

    /// Makes the first message from one tab to another ask the person when `ask` is true, and
    /// go without asking otherwise. What the person already answered about a pair still holds.
    pub fn set_ask_first(&mut self, ask: bool) {
        self.ask_first = ask;
    }

    /// Whether the first message from one tab to another asks the person.
    #[must_use]
    pub fn ask_first(&self) -> bool {
        self.ask_first
    }

    /// The same screen, looking for the sound server's socket in `runtime` rather than in the
    /// runtime folder this process was given.
    #[must_use]
    pub fn hearing_in(mut self, runtime: Option<PathBuf>) -> Self {
        self.runtime = runtime;
        self
    }

    /// The same screen, opening windows on `display` rather than on the session this process was
    /// started in.
    ///
    /// The application never calls this: the screen reads the session itself. It exists so that
    /// what a window's tab does can be checked on a machine with no compositor, and what it says
    /// when there is none on a machine that has one.
    #[must_use]
    pub fn showing_on(mut self, display: Result<crate::desktop::Display, crate::desktop::NoDisplay>) -> Self {
        self.display = display;
        self
    }

    /// Opens the workspace at `index` of the rail, when there is one there.
    pub fn set_active(&mut self, index: usize) {
        if index < self.workspaces.len() {
            self.active = index;
        }
    }

    /// Gives the workspace at `index` the tabs `record` lists, each waiting to be shown before it
    /// starts, with the tab the record had open open again.
    ///
    /// A profile tab whose profile is gone from the store is left out: there is no container
    /// it could enter, and a tab that can only ever fail is not worth bringing back. A blank tab
    /// comes back blank; it never had a container. A tab of one of the workspace's files comes back
    /// whether or not the file is still there, and says so when it is shown; one whose path
    /// climbs out of the workspace is left out, because nothing QCode wrote would name one.
    pub fn restore_tabs(&mut self, index: usize, record: &SessionWorkspace) {
        let Some(workspace) = self.workspaces.get_mut(index) else { return };
        let mut active = 0;
        for (position, saved) in record.tabs.iter().enumerate() {
            let kind = match &saved.kind {
                SessionTabKind::Shell => TabKind::Shell,
                SessionTabKind::Profile(name) => TabKind::Profile(name.clone()),
                SessionTabKind::New => TabKind::New,
                SessionTabKind::Image(file) => TabKind::Image(file.clone()),
                SessionTabKind::Markdown(file) => TabKind::Markdown(file.clone()),
                SessionTabKind::Editor(file) => TabKind::Editor(file.clone()),
                SessionTabKind::Pdf(file) => TabKind::Pdf(file.clone()),
                SessionTabKind::Office(file) => TabKind::Office(file.clone()),
                SessionTabKind::Sound(file) => TabKind::Sound(file.clone()),
                SessionTabKind::Desktop(name) => TabKind::Desktop(name.clone()),
            };
            let usable = match &kind {
                TabKind::New => true,
                TabKind::Image(file)
                | TabKind::Markdown(file)
                | TabKind::Editor(file)
                | TabKind::Pdf(file)
                | TabKind::Office(file)
                | TabKind::Sound(file) => files::is_inside(file),
                TabKind::Shell | TabKind::Profile(_) | TabKind::Desktop(_) => workspace.plan(&kind).is_some(),
            };
            if !usable {
                continue;
            }
            if position <= record.active_tab {
                active = workspace.tabs.len();
            }
            let key = TabKey(self.next_key);
            self.next_key += 1;
            let mut tab = Tab::restored(key, kind, saved.opened, saved.conversation.clone());
            // Opening QCode again is not asking to hear a sound again, and no more is it asking for
            // a window on the screen: both come back saying so, with the way to ask for them.
            tab.hold(matches!(tab.kind(), TabKind::Sound(_) | TabKind::Desktop(_)));
            workspace.tabs.push(tab);
        }
        workspace.active_tab = active;
    }

    /// What is open, in the shape the session file keeps it: the workspaces in rail order, the one
    /// that is open, and the tabs of each.
    #[must_use]
    pub fn session(&self) -> Session {
        let workspaces = self
            .workspaces
            .iter()
            .map(|workspace| SessionWorkspace {
                id: workspace.id.clone(),
                active_tab: workspace.active_tab,
                tabs: workspace
                    .tabs
                    .iter()
                    .map(|tab| SessionTab {
                        kind: match tab.kind() {
                            TabKind::Shell => SessionTabKind::Shell,
                            TabKind::Profile(name) => SessionTabKind::Profile(name.clone()),
                            TabKind::New => SessionTabKind::New,
                            TabKind::Image(file) => SessionTabKind::Image(file.clone()),
                            TabKind::Markdown(file) => SessionTabKind::Markdown(file.clone()),
                            TabKind::Editor(file) => SessionTabKind::Editor(file.clone()),
                            TabKind::Pdf(file) => SessionTabKind::Pdf(file.clone()),
                            TabKind::Office(file) => SessionTabKind::Office(file.clone()),
                            TabKind::Sound(file) => SessionTabKind::Sound(file.clone()),
                            TabKind::Desktop(name) => SessionTabKind::Desktop(name.clone()),
                        },
                        conversation: tab.conversation().map(str::to_owned),
                        opened: tab.opened(),
                    })
                    .collect(),
            })
            .collect();
        Session { active: self.workspace().map(|workspace| workspace.id.clone()), workspaces }
    }

    /// The engine the screen enters containers through.
    #[must_use]
    pub fn engine(&self) -> Option<&Engine> {
        self.engine.as_ref()
    }

    /// The widget panel's state.
    #[must_use]
    pub fn panel(&self) -> &Panel {
        &self.panel
    }

    /// The command the tab `key` spawns, which is always the engine entering a container.
    ///
    /// This is the one place a tab's program is decided, so a test can read it back and see for
    /// itself that no tab runs anything on the machine QCode runs on.
    #[must_use]
    pub fn launch_command(&self, key: TabKey) -> Option<EngineCommand> {
        let engine = self.engine.as_ref()?;
        let workspace = self.workspaces.iter().find(|workspace| workspace.tabs.iter().any(|tab| tab.key() == key))?;
        let tab = workspace.tabs.iter().find(|tab| tab.key() == key)?;
        if let TabKind::Sound(_) = tab.kind() {
            return sound::play_command(engine, self.user, workspace, tab, tab.socket()?);
        }
        let plan = workspace.plan(tab.kind())?;
        let program = workspace.program(tab, self.editor)?;
        let parts: Vec<&str> = program.iter().map(String::as_str).collect();
        // A harness tab carries the token its agent's bridge server hands back, which is how
        // QCode knows which tab a message comes from; a tab of a provider profile carries the
        // same token again, because the relay reuses the bridge's rather than minting a second
        // one, and the provider's own address and model besides.
        match tab.kind() {
            TabKind::Profile(_) => {
                let mut env = vec![(crate::bridge::TOKEN_VARIABLE.to_owned(), tab.token().to_owned())];
                let name = tab.kind().profile();
                let profile = workspace.profiles.iter().find(|profile| Some(profile.name.as_str()) == name);
                if let Some(profile) = profile
                    && let Some(provider) = &profile.provider
                {
                    // Each harness is pointed at the relay its own way, so the environment
                    // follows the profile's harness as well as its provider.
                    env.extend(provider.environment(profile.harness, tab.token(), self.measured_window(provider)));
                }
                let env: Vec<(&str, &str)> = env.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
                Some(plan.enter_with(engine, &parts, &env))
            }
            _ => Some(plan.enter(engine, &parts)),
        }
    }

    /// The command that takes the text out of the file the tab `key` shows, which is always the
    /// engine running a program in the workspace's own container, without a terminal; `None` for a
    /// tab that shows no such text.
    #[must_use]
    pub fn read_command(&self, key: TabKey) -> Option<EngineCommand> {
        let engine = self.engine.as_ref()?;
        let workspace = self.workspaces.iter().find(|workspace| workspace.tabs.iter().any(|tab| tab.key() == key))?;
        let tab = workspace.tabs.iter().find(|tab| tab.key() == key)?;
        let plan = workspace.plan(tab.kind())?;
        let program = viewer::text_command(tab.kind())?;
        let parts: Vec<&str> = program.iter().map(String::as_str).collect();
        Some(engine.exec_without_terminal(&Exec { container: &plan.name, command: &parts }))
    }

    /// The workspace of `id`, when the screen has it open.
    fn workspace_mut(&mut self, id: &str) -> Option<&mut OpenWorkspace> {
        self.workspaces.iter_mut().find(|workspace| workspace.id.as_str() == id)
    }

    /// What a blank tab's page knows of the conversations of `key`, when its workspace is open.
    fn shelf(&mut self, key: &HistoryKey) -> Option<&mut history::Shelf> {
        Some(self.workspace_mut(&key.workspace)?.history.entry(key.profile.clone()).or_default())
    }

    /// The workspace holding the tab `key`, when one does.
    fn owner(&self, key: TabKey) -> Option<&OpenWorkspace> {
        self.workspaces.iter().find(|workspace| workspace.tabs.iter().any(|tab| tab.key() == key))
    }

    /// The workspace holding the tab `key`, when one does.
    fn owner_mut(&mut self, key: TabKey) -> Option<&mut OpenWorkspace> {
        self.workspaces.iter_mut().find(|workspace| workspace.tabs.iter().any(|tab| tab.key() == key))
    }
}

impl WorkspaceScreen {
    /// Attaches `session` to the tab `key` and marks the tab running, which is what the screen
    /// does itself once a tab's container is up. A program started some other way, such as the
    /// shell of a demonstration, is shown in the tab the same way.
    pub fn attach(&mut self, key: TabKey, session: TerminalSession) {
        if let Some((_, tab)) = self.owner_mut(key).and_then(|workspace| workspace.find(key)) {
            tab.attached(session);
        }
    }

    /// The number of the reading of `profile`'s conversations last started in the open workspace.
    /// An answer, [`Msg::HistoryRead`], is only taken when it carries this number.
    #[must_use]
    pub fn reading(&self, profile: &str) -> u64 {
        self.workspace().and_then(|workspace| workspace.history.get(profile)).map_or(0, history::Shelf::generation)
    }

    /// The number of the reading of the list of backups last started. An answer,
    /// [`Msg::BackupsRead`], is only taken when it carries this number.
    #[must_use]
    pub fn backups_reading(&self) -> u64 {
        self.listing.as_ref().map_or(0, |_| self.last_listing)
    }
}

/// The work the screen needs the moment it is shown: reading the open workspace's folder and
/// asking the engine which of its containers are up.
///
/// The screen never touches the disk or the engine while drawing, so this is the caller's part
/// of the bargain: run it when the screen is entered, and again when it is returned to.
///
/// It also starts the open tab when it was brought back from the last session and has been
/// waiting to be shown, and reads the conversations a blank tab's page offers when the open tab
/// is one.
pub fn opened(screen: &mut WorkspaceScreen) -> Command<Msg> {
    relay::follow(screen);
    let command = Command::batch([load_root(screen), list_containers(screen), wake(screen), show_page(screen, true)]);
    Command::batch([command, follow_disk(screen), backups::keep(screen), bridge::follow(screen)])
}

/// Adds `workspace` to the rail and opens it, or only opens it when the rail has it already; the
/// tabs of every workspace stay as they are either way.
pub fn add(screen: &mut WorkspaceScreen, workspace: OpenWorkspace) -> Command<Msg> {
    match screen.workspaces.iter().position(|open| open.id == workspace.id) {
        Some(index) => screen.active = index,
        None => {
            screen.workspaces.push(workspace);
            screen.active = screen.workspaces.len() - 1;
        }
    }
    opened(screen)
}

/// Applies a message to the screen.
///
/// Whatever the message did, a tab that is now in view and was brought back from the last session
/// starts here: switching to it, closing the tab before it and opening its workspace all show it.
/// The same goes for a blank tab's page that comes into view, which reads its conversations.
pub fn update(screen: &mut WorkspaceScreen, message: Msg) -> Command<Msg> {
    // Profiles read again may carry a provider a workspace's did not before, so this runs before
    // anything else has the chance to start a tab that needs the relay already open.
    relay::follow(screen);
    // Profiles read again may be new ones, whose conversations the page shown has not read.
    let profiles = matches!(message, Msg::Profiles(_));
    let command = apply(screen, message);
    let kept = Command::batch([reread(screen), follow_disk(screen), backups::keep(screen), bridge::follow(screen)]);
    Command::batch([command, wake(screen), show_page(screen, profiles), kept])
}

/// Keeps the open workspace's watch on the folders its tree shows, whatever the message changed
/// about them, and lets the watches of the other workspaces go: their trees are not on screen, and
/// they are read again when they are opened.
fn follow_disk(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let mut commands = Vec::new();
    for (index, workspace) in screen.workspaces.iter_mut().enumerate() {
        if screen.live && index == screen.active {
            commands.push(watch::follow(workspace, &mut screen.last_watch));
        } else {
            watch::stop(workspace);
        }
    }
    Command::batch(commands)
}

/// Applies a message to the screen, leaving the start of a waiting tab to [`update`].
fn apply(screen: &mut WorkspaceScreen, message: Msg) -> Command<Msg> {
    match message {
        Msg::OpenWorkspace(index) => {
            screen.set_active(index);
            Command::batch([load_root(screen), list_containers(screen)])
        }
        Msg::CloseWorkspace(index) => {
            let backed = backups::closing(screen, index);
            let (silenced, shut) = match screen.workspaces.get(index) {
                Some(workspace) => {
                    let tabs: Vec<&Tab> = workspace.tabs.iter().collect();
                    (sound::silence(screen, workspace, &tabs), desktop::close_all(screen, workspace, &tabs))
                }
                None => (Command::none(), Command::none()),
            };
            let silenced = Command::batch([silenced, shut]);
            let Some(workspace) = screen.workspaces.get_mut(index) else { return Command::none() };
            for tab in &mut workspace.tabs {
                tab.close_session();
            }
            let keys: Vec<TabKey> = workspace.tabs.iter().map(Tab::key).collect();
            let tokens: Vec<String> = workspace.tabs.iter().map(|tab| tab.token().to_owned()).collect();
            relay::closed(workspace, &tokens);
            bridge::closed(screen, &keys);
            TabEdit::Close(index).apply(&mut screen.workspaces, &mut screen.active);
            Command::batch([backed, silenced, load_root(screen), list_containers(screen)])
        }
        // The application opens the list; nothing on this screen changes until a workspace comes
        // back from it.
        Msg::AddWorkspace => Command::none(),
        Msg::OpenTab(index) => {
            if let Some(workspace) = screen.workspaces.get_mut(screen.active)
                && index < workspace.tabs.len()
            {
                workspace.active_tab = index;
            }
            Command::none()
        }
        Msg::CloseTab(index) => {
            let silenced = match screen.workspace() {
                Some(workspace) => {
                    let tabs: Vec<&Tab> = workspace.tabs.get(index).into_iter().collect();
                    Command::batch([
                        sound::silence(screen, workspace, &tabs),
                        desktop::close_all(screen, workspace, &tabs),
                    ])
                }
                None => Command::none(),
            };
            let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
            let mut gone = Vec::new();
            let mut token = None;
            if let Some(tab) = workspace.tabs.get_mut(index) {
                tab.close_session();
                gone.push(tab.key());
                token = Some(tab.token().to_owned());
            }
            if let Some(token) = token {
                relay::closed(workspace, &[token]);
            }
            TabEdit::Close(index).apply(&mut workspace.tabs, &mut workspace.active_tab);
            bridge::closed(screen, &gone);
            silenced
        }
        Msg::MoveTab { from, to } => {
            if let Some(workspace) = screen.workspaces.get_mut(screen.active) {
                TabEdit::Move { from, to }.apply(&mut workspace.tabs, &mut workspace.active_tab);
            }
            Command::none()
        }
        Msg::NewTab => open_blank(screen),
        // The strip, because from there the arrows switch tabs and Tab reaches the rest of the
        // screen, the terminal included.
        Msg::LeaveTerminal => Command::focus(TABS_ID),
        // Only a tab with a terminal on it has somewhere to go back into; on a blank or a
        // Markdown tab the key does nothing rather than send the keyboard nowhere.
        Msg::EnterTerminal => {
            let terminal = screen.workspace().and_then(OpenWorkspace::active_tab).and_then(Tab::session).is_some();
            if terminal { Command::focus(TERMINAL_ID) } else { Command::none() }
        }
        Msg::Choose(key, choice) => choose(screen, key, choice),
        Msg::HighlightChoice(row) => {
            screen.blank_row = row;
            Command::none()
        }
        Msg::ShowAll(key) => {
            screen.expanded.insert(key);
            Command::none()
        }
        Msg::HistoryRead(key, generation, answer) => {
            blank::keep_row(screen, |screen| {
                if let Some(shelf) = screen.shelf(&key) {
                    shelf.answered(generation, answer);
                }
            });
            Command::none()
        }
        Msg::HistorySlow(key, generation) => {
            let mut shown = false;
            blank::keep_row(screen, |screen| {
                shown = screen.shelf(&key).is_some_and(|shelf| shelf.slow(generation));
            });
            if shown { after(history::SHOW_AT_LEAST, Msg::HistorySettled(key)) } else { Command::none() }
        }
        Msg::HistorySettled(key) => {
            blank::keep_row(screen, |screen| {
                if let Some(shelf) = screen.shelf(&key) {
                    shelf.settled();
                }
            });
            Command::none()
        }
        // The application opens the profiles screen; this one stays as it is.
        Msg::ManageProfiles => Command::none(),
        Msg::Profiles(profiles) => {
            for workspace in &mut screen.workspaces {
                workspace.set_profiles(profiles.clone());
            }
            Command::none()
        }
        Msg::ProfileAdded(id, result) => match result {
            Ok(file) => {
                if let Some(workspace) = screen.workspace_mut(&id) {
                    workspace.carried = file.profiles.into_iter().map(|profile| profile.name).collect();
                }
                Command::none()
            }
            // The tab is open whatever the file says; only the record of it is missing, and the
            // person is told so rather than finding the profile gone from the file later.
            Err(reason) => Command::toast(Toast::warning(t!("workspace.add-failed")).body(reason)),
        },
        Msg::Ready(key, run, result) => ready(screen, key, run, &result),
        Msg::BuildImage(key) => build_image(screen, key),
        Msg::ImageLine(key, text) => {
            if let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) {
                tab.build_line(&text, false);
            }
            Command::none()
        }
        Msg::ImageBuilt(key, result) => image_built(screen, key, result),
        Msg::StopBuild(key) => {
            let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) else {
                return Command::none();
            };
            let TabState::Building(task) = *tab.state() else { return Command::none() };
            // The build takes its half-made image away itself once it notices; the tab goes back
            // to offering it.
            tab.settled(TabState::NoImage);
            Command::cancel_task(task)
        }
        Msg::CancelOpen(key) => {
            let index = screen.workspace().and_then(|workspace| workspace.tabs.iter().position(|tab| tab.key() == key));
            match index {
                Some(index) => update(screen, Msg::CloseTab(index)),
                None => Command::none(),
            }
        }
        Msg::Woken(key, run, result, found) => {
            if let Some(found) = found {
                resume_newest(screen, key, run, &found);
            }
            ready(screen, key, run, &result)
        }
        Msg::Output(key, run, event) => output(screen, key, run, event),
        Msg::Checked(key, run, running) => {
            if let Some(workspace) = screen.owner_mut(key)
                && let Some((_, tab)) = workspace.find(key)
                && tab.run() == run
                && let TabState::Ended { .. } = tab.state()
                && !running
            {
                tab.settled(TabState::Stopped);
                // The container went away under the tab, so the panel's list no longer holds.
                let id = workspace.id.as_str().to_owned();
                return containers_of(screen, &id);
            }
            Command::none()
        }
        Msg::Restart(key) => restart_tab(screen, key),
        Msg::TogglePanel(open) => {
            screen.panel.set_open(open);
            Command::none()
        }
        Msg::ResizePanel(width) => {
            screen.panel.set_width(width);
            Command::none()
        }
        Msg::ToggleWidget(index, open) => {
            screen.panel.toggle(index, open);
            Command::none()
        }
        Msg::MoveWidget { from, to } => {
            screen.panel.move_widget(from, to);
            Command::none()
        }
        Msg::ShowWidgets(show) => {
            screen.panel.show_chooser(show);
            // The list takes the keyboard as it opens, so a widget is chosen with the arrows and
            // Enter as readily as with the pointer.
            if show { Command::focus(panel::CHOOSER_ID) } else { Command::none() }
        }
        Msg::HighlightWidget(row) => {
            screen.panel.highlight(row);
            Command::none()
        }
        Msg::AddWidget(widget) => {
            screen.panel.add(widget);
            Command::none()
        }
        Msg::RemoveWidget(widget) => {
            screen.panel.remove(widget);
            Command::none()
        }
        Msg::SelectFile(key) => {
            if let Some(workspace) = screen.workspaces.get_mut(screen.active) {
                workspace.files.move_cursor(&key);
            }
            Command::none()
        }
        Msg::ExpandFile(key, open) => {
            let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
            if !workspace.files.expand(&key, open) {
                return Command::none();
            }
            read_folder(workspace, &key)
        }
        Msg::FolderRead(workspace, key, entries) => {
            if let Some(workspace) = screen.workspace_mut(&workspace) {
                workspace.files.read(&key, entries);
            }
            Command::none()
        }
        Msg::Files(message) => files::update(screen, message),
        Msg::FilesChanged(id, run, batch) => match screen.workspace_mut(&id) {
            Some(workspace) => watch::changed(workspace, run, batch),
            None => Command::none(),
        },
        Msg::OpenFile(key) => open_file(screen, &key),
        Msg::EditFile(key) => {
            if !files::is_inside(&key) {
                return Command::none();
            }
            open_tab(screen, TabKind::Editor(key))
        }
        Msg::Missing(key, run) => {
            if let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key))
                && tab.run() == run
            {
                tab.settled(TabState::Missing);
            }
            Command::none()
        }
        Msg::DocumentRead(key, run, answer) => {
            document_read(screen, key, run, answer);
            Command::none()
        }
        Msg::TextRead(key, run, answer) => viewer::text_read(screen, key, run, answer),
        Msg::Page(key, page) => viewer::show_page(screen, key, page),
        Msg::Playable(key, run, answer) => sound::playable(screen, key, run, answer),
        Msg::Described(key, run, why, answer) => sound::described(screen, key, run, why, answer),
        Msg::Silenced => Command::none(),
        Msg::WindowOpened(key, run, answer) => desktop::opened(screen, key, run, answer),
        Msg::WindowEnded(key, run, code) => desktop::ended(screen, key, run, code),
        Msg::SignInWanted(key, run, addresses) => desktop::sign_in_wanted(screen, key, run, &addresses),
        Msg::SignInOpened(key, run, address, opened) => desktop::sign_in_opened(screen, key, run, &address, opened),
        Msg::SignInBack(key, run, id, ending) => desktop::sign_in_back(screen, key, run, id, ending),
        Msg::SignInHere(key) => desktop::sign_in_here(screen, key),
        Msg::RaiseWindow(key) => desktop::raise(screen, key),
        // Nothing is said when it worked: the window came forward, or the compositor marked it,
        // and either way the person is looking at their screen rather than at this tab.
        Msg::WindowRaised(result) => match result {
            Ok(()) => Command::none(),
            Err(failure) => Command::toast(Toast::warning(t!("workspace.window.raise-failed")).body(failure.output)),
        },
        Msg::CloseWindow(key) => desktop::close(screen, key),
        Msg::WindowsClosed(failure) => match failure {
            None => Command::none(),
            Some(failure) => Command::toast(Toast::danger(t!("workspace.window.close-failed")).body(failure.output)),
        },
        Msg::SelectContainer(index) => {
            if let Some(workspace) = screen.workspaces.get_mut(screen.active) {
                workspace.container_row = index;
            }
            Command::none()
        }
        Msg::RefreshContainers => list_containers(screen),
        Msg::ContainersRead(id, result) => {
            if let Some(workspace) = screen.workspace_mut(&id) {
                workspace.busy = false;
                match result {
                    Ok(containers) => {
                        workspace.container_row = workspace.container_row.min(containers.len().saturating_sub(1));
                        workspace.containers = containers;
                        workspace.container_error = None;
                    }
                    Err(failure) => workspace.container_error = Some(failure),
                }
            }
            Command::none()
        }
        Msg::StopContainer(name) => container_action(screen, &name, false),
        Msg::RestartContainer(name) => container_action(screen, &name, true),
        Msg::Noted(noting, message) => {
            let told = if screen.registry_told {
                Command::none()
            } else {
                screen.registry_told = true;
                let toast = match noting {
                    Noting::Unwritten(reason) => Toast::warning(t!("workspace.registry-unwritten")).body(reason),
                    Noting::Repaired(place) => Toast::warning(t!("workspace.registry-repaired")).body(place),
                };
                Command::toast(toast)
            };
            Command::batch([told, update(screen, *message)])
        }
        Msg::BackupDue(id, run) => backups::due(screen, &id, run),
        Msg::BackedUp { workspace, name, made, missed } => {
            backups::backed_up(screen, &workspace, &name, &made, &missed)
        }
        Msg::NewestBackup(id, at) => {
            backups::newest(screen, &id, at);
            Command::none()
        }
        Msg::SkipWritten(id, written) => backups::skip_written(screen, &id, written),
        Msg::BackupAssets(on) => match screen.workspaces.get(screen.active) {
            Some(workspace) => backups::set_assets(workspace, on),
            None => Command::none(),
        },
        Msg::AssetsWritten(id, written) => backups::assets_written(screen, &id, written),
        Msg::BackupSize(id, bytes) => {
            backups::sized(screen, &id, bytes);
            Command::none()
        }
        Msg::ShowBackups(file) => backups::show(screen, file),
        Msg::BackupsRead(reading, found) => backups::read(screen, reading, found),
        Msg::BackupsSlow(reading) => backups::slow(screen, reading),
        Msg::BackupsSettled(reading) => {
            backups::settled(screen, reading);
            Command::none()
        }
        Msg::HighlightBackup(row) => {
            backups::highlight(screen, row);
            Command::none()
        }
        Msg::ChooseBackup(row) => backups::choose(screen, row),
        Msg::BackupsOf(index) => backups::switch(screen, index),
        Msg::RestoreBackup(id) => backups::restore(screen, id, false),
        Msg::StopAndRestore(id) => backups::restore(screen, id, true),
        Msg::KeepBackup => {
            backups::declined(screen);
            Command::none()
        }
        Msg::CloseBackups => {
            backups::close(screen);
            Command::none()
        }
        Msg::Restored { workspace, name, of, file, done } => {
            let told = backups::restored(screen, &workspace, &name, &of, file.as_deref(), &done);
            // Bringing conversations back may have stopped their container first.
            Command::batch([told, containers_of(screen, &workspace)])
        }
        Msg::BackupEvery(every) => {
            backups::set_every(screen, every);
            Command::none()
        }
        Msg::Bridge(id, run, call) => bridge::called(screen, &id, run, call),
        Msg::Allow { from, to, allow } => bridge::allowed(screen, from, to, allow),
        Msg::StillAsking { from, to } => {
            bridge::still_asking(screen, from, to);
            Command::none()
        }
        Msg::Letters(key, action) => {
            bridge::letters(screen, key, action);
            Command::none()
        }
        Msg::Deliver => {
            screen.delivering = false;
            bridge::deliver(screen, std::time::Instant::now())
        }
        Msg::Unbridged(harness, trouble, message) => {
            Command::batch([bridge::unregistered(harness, &trouble), update(screen, *message)])
        }
        Msg::Unguided(harness, trouble, message) => {
            Command::batch([unguided(harness, &trouble), update(screen, *message)])
        }
        Msg::ContainerActed(result) => {
            let refresh = list_containers(screen);
            match result {
                Ok(()) => refresh,
                Err(failure) => Command::batch([
                    Command::toast(Toast::danger(t!("workspace.containers.failed")).body(failure.output)),
                    refresh,
                ]),
            }
        }
    }
}

/// Opens a blank tab after the last one and hands its page the keyboard. Nothing starts: the
/// tab only asks what it should open.
fn open_blank(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let key = TabKey(screen.next_key);
    let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
    screen.next_key += 1;
    workspace.tabs.push(Tab::blank(key));
    workspace.active_tab = workspace.tabs.len() - 1;
    screen.blank_row = 0;
    screen.expanded.clear();
    Command::focus(blank::CHOICES_ID)
}

/// Turns the blank tab `key` into what was chosen on its page, in its own place in the strip,
/// and starts the work that brings its container up.
fn choose(screen: &mut WorkspaceScreen, key: TabKey, choice: Choice) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let user = screen.user;
    let registry = screen.registry.clone();
    let providers_path = screen.providers_path.clone();
    let Some(workspace) = screen.owner_mut(key) else { return Command::none() };
    let (kind, conversation) = match choice {
        Choice::Shell => (TabKind::Shell, None),
        Choice::NewChat(name) => (TabKind::Profile(name), None),
        Choice::Resume(name, id) => (TabKind::Profile(name), Some(id)),
        Choice::Window(name) => (TabKind::Desktop(name), None),
    };
    let Some(plan) = workspace.plan(&kind) else { return Command::none() };
    // A tab that was chosen already is not chosen again: two quick presses open one container.
    if workspace.find(key).is_none_or(|(_, tab)| tab.kind() != &TabKind::New) {
        return Command::none();
    }
    let record = match kind.profile() {
        Some(name) if !workspace.carries(name) => record_profile(workspace, name),
        _ => Command::none(),
    };
    let window = plan.window.is_some();
    let provider = provider_choice(workspace, &kind);
    // A profile whose sign-in its harness's makers closed still opens, since it goes on working
    // for some; the person is told why it may not, at the moment they open it.
    let closed = kind
        .profile()
        .and_then(|name| workspace.profiles.iter().find(|profile| profile.name.as_str() == name))
        .and_then(|profile| {
            let words = crate::ui::profiles::closed_sign_in(profile)?;
            let title = t!("profiles.gemini.closed-title", name = profile.name.as_str());
            Some(Command::toast(Toast::warning(title).body(words)))
        })
        .unwrap_or_else(Command::none);
    let Some((_, tab)) = workspace.find(key) else { return Command::none() };
    tab.choose(kind, conversation);
    let run = tab.run();
    let token = tab.token().to_owned();
    if let Some(provider) = &provider {
        relay::entered(&*workspace, &token, &provider.tag);
    }
    // A window is not entered with a terminal, so it takes the window's own path and the keyboard
    // goes to the tab's one action rather than to a terminal that is not there.
    if window {
        return Command::batch([desktop::open(screen, key, run), Command::focus(desktop::WINDOW_ID), record, closed]);
    }
    let launch = Launch::new(key, run, token, provider, providers_path);
    Command::batch([
        Command::perform(move || {
            let Launch { key, run, gate, token } = launch;
            start(registry.as_deref(), &engine, &plan, user, gate.as_ref(), &token, |result| {
                Msg::Ready(key, run, result)
            })
        }),
        Command::focus(TERMINAL_ID),
        record,
        closed,
    ])
}

impl WorkspaceScreen {
    /// The window the model `provider` names is given on its server: what this machine measured,
    /// or for a ready-made service what it says it gives ([`crate::provider::Model::window`]);
    /// `None` when neither is known, or the provider is gone.
    ///
    /// Read from `providers.toml` as a tab starts rather than carried on the profile: a window
    /// measured again on the Providers page must reach the next tab, not the tab after QCode is
    /// started again.
    fn measured_window(&self, provider: &ProviderChoice) -> Option<u64> {
        let path = self.providers_path.as_ref()?;
        let providers = crate::provider::Providers::open(path).value;
        let entry = providers.get(&provider.tag)?;
        entry.model(&provider.model)?.window(entry.kind)
    }
}

/// The provider and model a profile of `kind` runs on, when `kind` is a harness tab of a profile
/// that names one of its own.
fn provider_choice(workspace: &OpenWorkspace, kind: &TabKind) -> Option<ProviderChoice> {
    let name = kind.profile()?;
    workspace.profiles.iter().find(|profile| profile.name.as_str() == name)?.provider.clone()
}

/// Writes into the workspace's `workspace.qcode` that it carries the profile `name`, on a background
/// thread.
///
/// The file is the one record of which workspace carries which profile: refreshing a login from its
/// profile reaches exactly the workspaces it names. The container itself does not wait for it, so
/// the tab opens either way.
fn record_profile(workspace: &OpenWorkspace, name: &str) -> Command<Msg> {
    let id = workspace.id.as_str().to_owned();
    let paths = workspace.paths.clone();
    let name = name.to_owned();
    Command::perform(move || {
        let result = add_profile(&paths, &name, Date::today_utc()).map_err(|problem| problem.to_string());
        Msg::ProfileAdded(id, result)
    })
}

/// Starts the open tab of the open workspace when it was brought back from the last session and
/// is being shown for the first time. Without an engine it keeps waiting, and the middle says why.
/// A blank tab has no container to start, so it keeps asking what it should open.
///
/// A harness tab that was a new chat when the session was recorded has no conversation id: a new
/// conversation's id is the harness's to make, and it is not known when the tab starts. So once
/// its container is up the profile's conversations are read, and [`resume_newest`] gives the
/// tab the one it was most likely showing before its session is spawned.
fn wake(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
    let Some(tab) = workspace.tabs.get_mut(workspace.active_tab) else { return Command::none() };
    if tab.state() != &TabState::Waiting {
        return Command::none();
    }
    // A Markdown document is read from the disk by QCode itself, so it opens with or without an
    // engine.
    if let TabKind::Markdown(file) = tab.kind() {
        let file = file.clone();
        tab.wake();
        let (key, run) = (tab.key(), tab.run());
        return read_document(workspace, key, run, &file);
    }
    if screen.engine.is_none() {
        return Command::none();
    }
    if let TabKind::Sound(_) = tab.kind() {
        tab.wake();
        let (key, run) = (tab.key(), tab.run());
        return sound::hear(screen, key, run);
    }
    // A window is opened by a container of its own rather than entered, so it never goes through
    // the terminal path below.
    if let TabKind::Desktop(_) = tab.kind() {
        tab.wake();
        let (key, run) = (tab.key(), tab.run());
        return desktop::open(screen, key, run);
    }
    // A PDF shows its text first, which is read rather than run in a terminal.
    if viewer::text_command(tab.kind()).is_some() && !viewer::draws(tab) {
        tab.wake();
        let (key, run) = (tab.key(), tab.run());
        return viewer::take_text(screen, key, run);
    }
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let user = screen.user;
    let registry = screen.registry.clone();
    let providers_path = screen.providers_path.clone();
    let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
    let Some(tab) = workspace.tabs.get(workspace.active_tab) else { return Command::none() };
    let Some(plan) = workspace.plan(tab.kind()) else { return Command::none() };
    let provider = provider_choice(workspace, tab.kind());
    let file = tab.kind().file().map(|file| workspace.files.path(file));
    let harness = match tab.kind() {
        TabKind::Profile(name) if tab.conversation().is_none() => {
            workspace.profiles.iter().find(|profile| profile.name.as_str() == name).map(|profile| profile.harness)
        }
        _ => None,
    };
    let Some(tab) = workspace.tabs.get_mut(workspace.active_tab) else { return Command::none() };
    tab.wake();
    let (key, run) = (tab.key(), tab.run());
    let token = tab.token().to_owned();
    if let Some(provider) = &provider {
        relay::entered(&*workspace, &token, &provider.tag);
    }
    match harness {
        None => bring_up(engine, plan, Launch::new(key, run, token, provider, providers_path), user, file, registry),
        Some(harness) => {
            let launch = Launch::new(key, run, token, provider, providers_path);
            Command::perform(move || {
                let Launch { key, run, gate, token } = launch;
                start(registry.as_deref(), &engine, &plan, user, gate.as_ref(), &token, |result| {
                    // Not being able to read only means the tab cannot be matched to its
                    // conversation; it then starts a new one, the way it started last time. The
                    // container is up by now, so reading it starts nothing.
                    let found =
                        result.is_ok().then(|| history::read(&engine, &plan.name, harness, &mut || {}).ok()).flatten();
                    Msg::Woken(key, run, result, found)
                })
            })
        }
    }
}

/// Gives the new-chat tab `key`, woken as `run`, the conversation of `found` it was most likely
/// showing ([`history::claim`]), so it opens that one and the session file keeps its id from now
/// on. A tab that was closed, restarted or given a conversation meanwhile is left as it is.
fn resume_newest(screen: &mut WorkspaceScreen, key: TabKey, run: u64, found: &[Conversation]) {
    let Some(workspace) = screen.owner_mut(key) else { return };
    let Some((_, tab)) = workspace.find(key) else { return };
    if tab.run() != run || !tab.state().is_starting() || tab.conversation().is_some() {
        return;
    }
    let (kind, opened) = (tab.kind().clone(), tab.opened());
    let taken: Vec<String> = workspace
        .tabs
        .iter()
        .filter(|other| other.key() != key && other.kind() == &kind)
        .filter_map(|other| other.conversation().map(str::to_owned))
        .collect();
    let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
    let Some(chosen) = history::claim(found, opened, &taken).map(|conversation| conversation.id.clone()) else {
        return;
    };
    if let Some((_, tab)) = workspace.find(key) {
        tab.show_conversation(chosen);
    }
}

/// Reads the conversations the page of the open tab offers, when the open tab is blank and its
/// page has just come into view, or always when `again` asks for it.
///
/// Every profile of the workspace is read at once, each on its own thread, and each keeps the rows
/// it had on screen until its answer is in.
fn show_page(screen: &mut WorkspaceScreen, again: bool) -> Command<Msg> {
    let showing = screen.workspace().and_then(OpenWorkspace::active_tab).filter(|tab| tab.kind() == &TabKind::New);
    let showing = showing.map(Tab::key);
    let new = showing != screen.shown_blank;
    screen.shown_blank = showing;
    if showing.is_none() || !(new || again) {
        return Command::none();
    }
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let registry = screen.registry.clone();
    let user = screen.user;
    let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
    let mut commands = Vec::new();
    for profile in &workspace.profiles {
        let key = HistoryKey { workspace: workspace.id.as_str().to_owned(), profile: profile.name.as_str().to_owned() };
        let generation = workspace.history.entry(key.profile.clone()).or_default().start();
        let plan = ContainerPlan::profile(&workspace.id, &workspace.paths, profile);
        let (engine, harness, answer, registry) = (engine.clone(), profile.harness, key.clone(), registry.clone());
        commands.push(Command::perform(move || {
            // A container made from an image that has been built again since is made anew before
            // it is read, since reading starts it and the tab would then take it as it is.
            let renewed = plan::renew_rebuilt(&engine, &plan, user);
            // Reading a stopped container starts it and leaves it running for the tab that opens
            // a conversation from it, so it is noted like any container QCode starts.
            let mut started = renewed;
            let container = plan.name;
            let read = history::read(&engine, &container, harness, &mut || started = true);
            let message = Msg::HistoryRead(answer, generation, read);
            if started { noted(registry.as_deref(), engine.kind(), &container, message) } else { message }
        }));
        commands.push(after(history::SHOW_AFTER, Msg::HistorySlow(key, generation)));
    }
    Command::batch(commands)
}

/// Delivers `message` once `delay` has passed, without holding up anything meanwhile.
fn after(delay: std::time::Duration, message: Msg) -> Command<Msg> {
    Command::task(Task::new(t!("workspace.history.reading"), move |cx| {
        if cx.sleep(delay) { Ok(message) } else { Err(String::new()) }
    }))
}

/// Starts the tab `key` again, from its container upwards; a Markdown tab reads its file again.
fn restart_tab(screen: &mut WorkspaceScreen, key: TabKey) -> Command<Msg> {
    let engine = screen.engine.clone();
    let user = screen.user;
    let registry = screen.registry.clone();
    let providers_path = screen.providers_path.clone();
    let Some(workspace) = screen.owner_mut(key) else { return Command::none() };
    let Some((_, tab)) = workspace.find(key) else { return Command::none() };
    let kind = tab.kind().clone();
    if let TabKind::Markdown(file) = &kind {
        tab.restarting();
        let run = tab.run();
        return read_document(workspace, key, run, file);
    }
    let Some(engine) = engine else { return Command::none() };
    // A sound is met again from the start: the settings or the machine may have changed, and a
    // tab brought back from the last session has now been asked to play.
    if let TabKind::Sound(_) = &kind {
        tab.restarting();
        tab.hold(false);
        let run = tab.run();
        return sound::hear(screen, key, run);
    }
    // Asking for the window is what lifts the hold a tab brought back from the last session has.
    if let TabKind::Desktop(_) = &kind {
        tab.restarting();
        tab.hold(false);
        let run = tab.run();
        return desktop::open(screen, key, run);
    }
    // A PDF whose text could not be read reads it again; one showing a page draws it again.
    let reads = viewer::text_command(&kind).is_some() && !viewer::draws(tab);
    tab.restarting();
    let run = tab.run();
    let token = tab.token().to_owned();
    if reads {
        return viewer::take_text(screen, key, run);
    }
    let Some(plan) = workspace.plan(&kind) else { return Command::none() };
    let provider = provider_choice(workspace, &kind);
    if let Some(provider) = &provider {
        let token = workspace.tabs.iter().find(|tab| tab.key() == key).map(|tab| tab.token().to_owned());
        if let Some(token) = token {
            relay::entered(&*workspace, &token, &provider.tag);
        }
    }
    let file = kind.file().map(|file| workspace.files.path(file));
    bring_up(engine, plan, Launch::new(key, run, token, provider, providers_path), user, file, registry)
}

/// What has to be true of a provider profile's provider before its tab is trusted to speak for
/// it, and the words to say when it is not: the provider it names, where its providers are read
/// from, and, made here rather than on the background thread [`start`] runs on, what to say if
/// it is gone. Translating needs the thread the person's own language is loaded on; `start` only
/// reads the file and decides whether the words are needed. `None` for a tab of no provider.
struct ProviderGate {
    /// The provider and model the tab's profile runs on.
    provider: ProviderChoice,
    /// Where `providers.toml` is read from to check that provider still exists.
    path: Option<PathBuf>,
    /// What to say if it is gone, made in the person's language before this ever reaches a
    /// background thread.
    missing: String,
}

impl ProviderGate {
    /// The gate a tab of `provider` is checked against, when it has one.
    fn of(provider: Option<ProviderChoice>, path: Option<PathBuf>) -> Option<Self> {
        let provider = provider?;
        let missing = t!("workspace.provider-missing", tag = provider.tag.as_str());
        Some(Self { provider, path, missing })
    }
}

/// What ties a container's launch to one tab: its key, the run it is starting as, and, for a
/// provider profile, what its tab is checked against before it starts. Bundled so the functions
/// that carry it stay under the argument count a reader can hold in their head at once.
struct Launch {
    /// The tab this launch is for.
    key: TabKey,
    /// The run it is starting as; an answer that names another run has moved on.
    run: u64,
    /// What this tab's provider is checked against, when it has one.
    gate: Option<ProviderGate>,
    /// The token by which the bridge knows this tab's agent.
    token: String,
}

impl Launch {
    /// A launch for the tab `key` in its run `run`, known to the bridge as `token`, naming the
    /// provider `provider` runs on when it has one and where `providers.toml` is read from to
    /// check it still exists.
    fn new(
        key: TabKey,
        run: u64,
        token: String,
        provider: Option<ProviderChoice>,
        providers_path: Option<PathBuf>,
    ) -> Self {
        Self { key, run, gate: ProviderGate::of(provider, providers_path), token }
    }
}

/// Brings the container of `plan` up for `launch`'s tab, on a background thread.
///
/// A tab that opens one of the workspace's files first looks for the file at `file` on this
/// machine: an editor started on a file that is gone would open an empty one of that name and
/// save it back, which is not what the tab promised, so the tab says the file is gone instead.
fn bring_up(
    engine: Engine,
    plan: ContainerPlan,
    launch: Launch,
    user: HostUser,
    file: Option<PathBuf>,
    registry: Option<PathBuf>,
) -> Command<Msg> {
    Command::perform(move || {
        let Launch { key, run, gate, token } = launch;
        if file.is_some_and(|file| !file.exists()) {
            return Msg::Missing(key, run);
        }
        start(registry.as_deref(), &engine, &plan, user, gate.as_ref(), &token, |result| Msg::Ready(key, run, result))
    })
}

/// Brings the container of `plan` up and notes it in `registry` when it is up, then hands the
/// outcome to `answer` for the message it becomes.
///
/// A container that was running already is noted too: it is one of QCode's, and a list that
/// lost its name — to another QCode writing at the same moment, or to a damaged file — gets it
/// back the next time a tab opens in it. Runs engine commands and writes the disk, so it belongs
/// on a background thread.
fn start(
    registry: Option<&Path>,
    engine: &Engine,
    plan: &ContainerPlan,
    user: HostUser,
    gate: Option<&ProviderGate>,
    token: &str,
    answer: impl FnOnce(Result<(), LaunchFailure>) -> Msg,
) -> Msg {
    // A profile whose provider tag was removed from the Providers page since it was chosen here
    // cannot be pointed anywhere: the relay would refuse every request with an unknown tab, which
    // reads to the person as the harness itself being broken. Checked before anything is started,
    // against the file as it is right now rather than a stale copy this profile once carried; the
    // words for it were made before this thread was, because a background thread has no language
    // of its own to translate them into.
    if let Some(gate) = gate {
        let providers = match &gate.path {
            Some(path) => crate::provider::Providers::open(path).value,
            None => crate::provider::Providers::in_memory(),
        };
        if providers.get(&gate.provider.tag).is_none() {
            let message = gate.missing.clone();
            return answer(Err(LaunchFailure { command: String::new(), output: message, image_missing: false }));
        }
    }
    let result = plan::ensure_running(engine, plan, user);
    let up = result.is_ok();
    // Registered before the harness starts, so it finds the bridge's server at its first start.
    let unregistered = match &plan.bridge {
        Some(bridge) if up => bridge_config::register(engine, &plan.name, bridge.harness, token)
            .err()
            .map(|trouble| (bridge.harness, trouble)),
        _ => None,
    };
    // After the bridge and before the harness starts, so the agent reads the files as they are
    // meant to be from its first turn.
    let unguided = match plan.guidance {
        Some(harness) if up => {
            let trouble = plan::guide(engine, &plan.name, &plan.code, harness, plan.graphify).err();
            // After graphify's installer, which is what points the agent at the map; started and
            // not waited for, so the harness starts at once.
            if plan.graphify {
                plan::build_map(engine, &plan.name, &plan.code);
            }
            trouble.map(|trouble| (harness, trouble))
        }
        _ => None,
    };
    let mut message = answer(result);
    if let Some((harness, trouble)) = unregistered {
        message = Msg::Unbridged(harness, trouble, Box::new(message));
    }
    if let Some((harness, trouble)) = unguided {
        message = Msg::Unguided(harness, trouble, Box::new(message));
    }
    if up { noted(registry, engine.kind(), &plan.name, message) } else { message }
}

/// The words for a QCode high profile whose instruction files in the workspace could not be
/// brought up to date. Its tab starts all the same: the agent works, it only has not been told
/// everything it would have been.
pub(super) fn unguided(harness: HarnessKind, trouble: &Unguided) -> Command<Msg> {
    let body = match trouble {
        Unguided::Graphify(words) => t!("workspace.unguided.graphify", words = words.as_str()),
        Unguided::Unwritten(file, words) => {
            t!("workspace.unguided.unwritten", file = file.as_str(), words = words.as_str())
        }
        Unguided::Broken(file) => t!("workspace.unguided.broken", file = file.as_str()),
    };
    let title = t!("workspace.unguided.title", harness = harness.record().display_name);
    Command::toast(Toast::warning(title).body(body))
}

/// Notes in the list at `registry` that QCode started the container `name` in `engine`, and
/// hands `message` on — wrapped with what went wrong, when something did.
pub(super) fn noted(registry: Option<&Path>, engine: EngineKind, name: &str, message: Msg) -> Msg {
    let Some(path) = registry else { return message };
    let noting = match Registry::record(path, name, engine) {
        Ok(problems) => match problems.first() {
            None => return message,
            Some(problem) => Noting::Repaired(problem.to_string()),
        },
        Err(error) => Noting::Unwritten(format!("{}: {error}", path.display())),
    };
    Msg::Noted(noting, Box::new(message))
}

/// Reads the document of the Markdown tab `key`, in its run `run`, on a background thread.
fn read_document(workspace: &OpenWorkspace, key: TabKey, run: u64, file: &str) -> Command<Msg> {
    let path = workspace.files.path(file);
    let root = workspace.files.root().to_path_buf();
    Command::perform(move || Msg::DocumentRead(key, run, files::read_document(&path, &root)))
}

/// Takes what reading a Markdown tab's document answered. A tab that was restarted or closed
/// meanwhile is left alone.
fn document_read(screen: &mut WorkspaceScreen, key: TabKey, run: u64, answer: Result<String, DocumentTrouble>) {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) else { return };
    if tab.run() != run {
        return;
    }
    match answer {
        Ok(text) => tab.read(text, false),
        Err(DocumentTrouble::Missing) => tab.settled(TabState::Missing),
        Err(DocumentTrouble::Outside) => {
            let file = tab.kind().file().unwrap_or_default().to_owned();
            tab.settled(TabState::Unreadable(t!("workspace.file.outside", file = file)));
        }
        Err(DocumentTrouble::Unreadable(reason)) => tab.settled(TabState::Unreadable(reason)),
    }
}

/// Reads the open Markdown tab's document again when the tab comes back into view, so a change
/// made in the editor meanwhile is what it shows. The text it had stays on screen until the new
/// one is in.
fn reread(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let Some(workspace) = screen.workspaces.get(screen.active) else {
        screen.shown_document = None;
        return Command::none();
    };
    let showing = workspace.active_tab().filter(|tab| matches!(tab.kind(), TabKind::Markdown(_)));
    let key = showing.map(Tab::key);
    let again = key.is_some() && key != screen.shown_document;
    screen.shown_document = key;
    match showing {
        Some(tab) if again && tab.state() == &TabState::Running => {
            let file = tab.kind().file().unwrap_or_default().to_owned();
            read_document(workspace, tab.key(), tab.run(), &file)
        }
        _ => Command::none(),
    }
}

/// Opens the file `key` of the open workspace's tree in the built-in app its name calls for, or
/// says that none opens it yet.
fn open_file(screen: &mut WorkspaceScreen, key: &str) -> Command<Msg> {
    if !files::is_inside(key) {
        return Command::none();
    }
    let kind = match apps::classify(files::name(key)) {
        FileKind::Image => TabKind::Image(key.to_owned()),
        FileKind::Markdown => TabKind::Markdown(key.to_owned()),
        FileKind::Text => TabKind::Editor(key.to_owned()),
        FileKind::Pdf => TabKind::Pdf(key.to_owned()),
        FileKind::Office => TabKind::Office(key.to_owned()),
        FileKind::Sound => TabKind::Sound(key.to_owned()),
        FileKind::Unknown => {
            return Command::toast(Toast::info(t!("workspace.file.no-app")).body(files::name(key).to_owned()));
        }
    };
    open_tab(screen, kind)
}

/// Shows the tab of `kind` in the open workspace: the one already open when there is one, since a
/// click on a file in the tree opens it and a second click should not open it twice, or a new
/// tab after the last one. The new tab waits to be started, which [`update`] does at once.
fn open_tab(screen: &mut WorkspaceScreen, kind: TabKind) -> Command<Msg> {
    let key = TabKey(screen.next_key);
    let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
    let focus = if matches!(kind, TabKind::Markdown(_) | TabKind::Pdf(_) | TabKind::Office(_)) {
        DOCUMENT_ID
    } else {
        TERMINAL_ID
    };
    if let Some(index) = workspace.tabs.iter().position(|tab| tab.kind() == &kind) {
        workspace.active_tab = index;
        return Command::focus(focus);
    }
    screen.next_key += 1;
    workspace.tabs.push(Tab::waiting(key, kind));
    workspace.active_tab = workspace.tabs.len() - 1;
    Command::focus(focus)
}

/// Attaches a session to a tab whose container came up, or tells it why it did not.
fn ready(screen: &mut WorkspaceScreen, key: TabKey, run: u64, result: &Result<(), LaunchFailure>) -> Command<Msg> {
    let Some(command) = screen.launch_command(key) else { return Command::none() };
    let Some(workspace) = screen.owner_mut(key) else { return Command::none() };
    let folder = workspace.paths.root.clone();
    // Bringing a tab up makes, remakes or starts a container, and even one that failed halfway may
    // have left one behind; the panel's list is what the engine said before, so it is asked again.
    let id = workspace.id.as_str().to_owned();
    let Some((_, tab)) = workspace.find(key) else { return Command::none() };
    if tab.run() != run || !tab.state().is_starting() {
        return Command::none();
    }
    if let Err(failure) = result {
        // Only a profile's tab is ever told its image is missing, and that one can build it.
        let state = if failure.image_missing && matches!(tab.kind(), TabKind::Profile(_)) {
            TabState::NoImage
        } else {
            TabState::Failed(failure.clone())
        };
        tab.settled(state);
        return containers_of(screen, &id);
    }
    let args: Vec<OsString> = command.args.clone();
    let attached = match TerminalSession::spawn(command.program.as_os_str(), &args, &folder) {
        Ok(session) => {
            let watch = session.watch();
            tab.attached(session);
            let next = Command::perform(move || Msg::Output(key, run, watch.next()));
            // A harness that has just come back takes whatever waited for it while it was gone.
            Command::batch([next, bridge::deliver(screen, std::time::Instant::now())])
        }
        Err(error) => {
            tab.settled(TabState::Failed(LaunchFailure::spawn(&command, &error)));
            Command::none()
        }
    };
    Command::batch([attached, containers_of(screen, &id)])
}

/// Builds the image of the profile of the tab `key`, the same whole build the profile wizard
/// runs, with every line of it in the tab's log.
fn build_image(screen: &mut WorkspaceScreen, key: TabKey) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let Some(workspace) = screen.owner_mut(key) else { return Command::none() };
    let Some((_, tab)) = workspace.find(key) else { return Command::none() };
    let (TabKind::Profile(name), TabState::NoImage) = (tab.kind(), tab.state()) else { return Command::none() };
    let name = name.clone();
    let Some(profile) = workspace.profiles.iter().find(|profile| profile.name.as_str() == name).cloned() else {
        return Command::none();
    };
    let task = Task::new(t!("workspace.image.building", profile = name), move |cx| {
        let cancel = || cx.is_cancelled();
        let mut line = |text: &str| cx.send(Msg::ImageLine(key, text.to_owned()));
        let built = crate::ui::profiles::work::build_whole(&engine, &profile, &cancel, &mut || {}, &mut line);
        Ok(Msg::ImageBuilt(
            key,
            built.map_err(|problem| match problem {
                Problem::Cancelled => None,
                other => Some(other.output().unwrap_or_default().to_owned()),
            }),
        ))
    });
    if let Some((_, tab)) = workspace.find(key) {
        tab.building(task.id());
    }
    Command::task(task)
}

/// Takes the end of a tab's image build: a built image opens the tab, which is what the person
/// asked for when they chose it; a failed build is the tab's failure, in the engine's words.
fn image_built(screen: &mut WorkspaceScreen, key: TabKey, result: Result<(), Option<String>>) -> Command<Msg> {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) else {
        return Command::none();
    };
    if !matches!(tab.state(), TabState::Building(_)) {
        return Command::none();
    }
    match result {
        Ok(()) => restart_tab(screen, key),
        Err(None) => {
            tab.settled(TabState::NoImage);
            Command::none()
        }
        Err(Some(output)) => {
            tab.settled(TabState::Failed(LaunchFailure { command: String::new(), output, image_missing: false }));
            Command::none()
        }
    }
}

/// Keeps a tab's session watched, and asks after its container when the session ends.
fn output(screen: &mut WorkspaceScreen, key: TabKey, run: u64, event: TerminalEvent) -> Command<Msg> {
    let engine = screen.engine.clone();
    let Some(workspace) = screen.owner_mut(key) else { return Command::none() };
    let kind = workspace.find(key).map(|(_, tab)| tab.kind().clone());
    let plan = kind.and_then(|kind| workspace.plan(&kind));
    let Some((_, tab)) = workspace.find(key) else { return Command::none() };
    if tab.run() != run {
        return Command::none();
    }
    match event {
        TerminalEvent::Output => {
            let Some(session) = tab.session() else { return Command::none() };
            let watch = session.watch();
            let next = Command::perform(move || Msg::Output(key, run, watch.next()));
            // The tab has just spoken, so the wait for its quiet starts again from here, and a
            // message held for it is looked at once that wait is over.
            Command::batch([next, bridge::deliver(screen, std::time::Instant::now())])
        }
        TerminalEvent::Exited(code) => {
            tab.settled(TabState::Ended { code });
            // A session can end because the person left the shell, or because the container
            // under it went away. Only the engine knows which, and the answer changes what the
            // tab says, so it is asked rather than guessed.
            match (engine, plan) {
                (Some(engine), Some(plan)) => {
                    Command::perform(move || Msg::Checked(key, run, plan::is_running(&engine, &plan)))
                }
                _ => Command::none(),
            }
        }
    }
}

/// Reads the root of the open workspace's file tree the first time, and every folder it shows
/// again after that: the files may have changed while the screen was away.
fn load_root(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let Some(workspace) = screen.workspaces.get_mut(screen.active) else { return Command::none() };
    if workspace.files.children(files::ROOT).is_some() {
        return files::refresh(workspace);
    }
    if !workspace.files.start_reading(files::ROOT) {
        return Command::none();
    }
    read_folder(workspace, files::ROOT)
}

/// Reads one folder of a workspace's file tree on a background thread.
fn read_folder(workspace: &OpenWorkspace, key: &str) -> Command<Msg> {
    let id = workspace.id.as_str().to_owned();
    let key = key.to_owned();
    let path = workspace.files.path(&key);
    Command::perform(move || Msg::FolderRead(id, key, files::read_folder(&path)))
}

/// Asks the engine which containers the open workspace has.
fn list_containers(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let Some(id) = screen.workspaces.get(screen.active).map(|workspace| workspace.id.as_str().to_owned()) else {
        return Command::none();
    };
    containers_of(screen, &id)
}

/// Asks the engine which containers the workspace `id` has, open or waiting behind another.
fn containers_of(screen: &mut WorkspaceScreen, id: &str) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let Some(workspace) = screen.workspace_mut(id) else { return Command::none() };
    workspace.busy = true;
    let id = workspace.id.as_str().to_owned();
    Command::perform(move || {
        let containers = plan::workspace_containers(&engine, &id);
        Msg::ContainersRead(id, containers)
    })
}

/// Stops a container, or stops and starts it again.
fn container_action(screen: &WorkspaceScreen, name: &str, again: bool) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let registry = screen.registry.clone();
    let name = name.to_owned();
    Command::perform(move || {
        if !again {
            return Msg::ContainerActed(plan::stop(&engine, &name));
        }
        let result = plan::restart(&engine, &name);
        let up = result.is_ok();
        let message = Msg::ContainerActed(result);
        if up { noted(registry.as_deref(), engine.kind(), &name, message) } else { message }
    })
}

/// The name the terminal in the middle is focused by.
const TERMINAL_ID: &str = "workspace-terminal";

/// The name the document of a Markdown tab, or the text of a PDF, is focused by.
const DOCUMENT_ID: &str = "workspace-document";

/// The name the tab strip is focused by; with no tab its `+` is the whole strip, and the focus.
const TABS_ID: &str = "workspace-tabs";

/// Draws the workspace screen for an application whose messages are `P`: `wrap` turns the screen's
/// own messages into the application's, `above` draws what the application puts over the tabs
/// (its header) and `below` what it puts under the terminal (its footer).
///
/// The rail runs down the whole left edge and the widget panel down the whole right edge, from
/// the very first row to the very last; the application's header and footer belong to the middle
/// column only, with the tabs and the terminal. That is why the application hands its parts in
/// here instead of drawing them around this screen. `under` is what the application puts at the
/// foot of the rail, under the workspaces: the ways out of this screen, which stand there rather
/// than over and under the terminal so that every row of the middle belongs to the work.
pub fn view<P: 'static>(
    screen: &WorkspaceScreen,
    ui: &mut View<'_, P>,
    wrap: fn(Msg) -> P,
    above: impl FnOnce(&mut View<'_, P>),
    below: impl FnOnce(&mut View<'_, P>),
    under: impl FnOnce(&mut View<'_, P>),
) {
    AppShell::new()
        .sidebar_width(RAIL_WIDTH)
        // The rail is the only way between workspaces, so it never collapses away; four cells fit
        // on any terminal QCode can draw on.
        .collapse_below(0)
        .sidebar(|ui| {
            ui.column(|ui| {
                ui.map(wrap, |ui| rail(screen, ui)).fill();
                under(ui);
            })
            .fill();
        })
        .body(|ui| {
            SidePanel::new(screen.panel.width())
                .side(Side::Right)
                .open(screen.panel.is_open())
                .limits(PANEL_MIN, PANEL_MAX)
                .on_toggle(move |open| wrap(Msg::TogglePanel(open)))
                .on_resize(move |width| wrap(Msg::ResizePanel(width)))
                .panel(|ui| {
                    ui.map(wrap, |ui| panel::view(screen, ui)).fill();
                })
                .body(|ui| {
                    AppShell::new()
                        .header(|ui| {
                            above(ui);
                            ui.map(wrap, |ui| header(screen, ui)).fill_width();
                        })
                        .body(|ui| {
                            ui.map(wrap, |ui| center(screen, ui)).fill();
                        })
                        .footer(below)
                        .show(ui);
                })
                .show(ui)
                .id("workspace-body");
        })
        .show(ui);
}

/// The rail of open workspaces down the left, ending in the `+` that opens another one.
fn rail(screen: &WorkspaceScreen, ui: &mut View<'_, Msg>) {
    let tabs = screen.workspaces.iter().map(|workspace| {
        let running = workspace.containers.iter().filter(|container| container.state.is_running()).count();
        let tab = RailTab::new(workspace.name.clone()).icon("folder");
        if running > 0 { tab.status("success").badge(running.to_string()) } else { tab }
    });
    ui.add(
        TabRail::new(tabs)
            .active(screen.active)
            .collapsed(true)
            .collapsed_marker(CollapsedMarker::Initial)
            .row_height(RAIL_ROWS)
            .closable(Msg::CloseWorkspace)
            .on_add(|| Msg::AddWorkspace)
            .on_select(Msg::OpenWorkspace),
    )
    .fill()
    .id("workspace-rail");
}

/// The tab strip, ending in the `+` that opens a blank tab right after its last tab, the way a
/// browser's strip ends; with no tab at all the `+` stands at the top left.
///
/// The strip places the `+` itself: it keeps the button's room while laying out the tabs, so the
/// `+` follows the last tab while they fit and keeps its place at the right end once they scroll.
fn header(screen: &WorkspaceScreen, ui: &mut View<'_, Msg>) {
    let Some(workspace) = screen.workspace() else { return };
    let labels: Vec<String> = (0..workspace.tabs.len()).map(|index| workspace.tab_label(index)).collect();
    ui.add(
        Tabs::new(labels)
            .active(workspace.active_tab)
            .tab_width(TabWidth::Fit)
            .closable(Msg::CloseTab)
            .reorderable(move |from, to| Msg::MoveTab { from, to })
            .on_select(Msg::OpenTab)
            .on_add(|| Msg::NewTab),
    )
    .fill_width()
    .id(TABS_ID);
}

/// What the middle shows: the open tab, or why there is nothing to show, and under a tab the
/// messages other tabs' agents left waiting in it and whether the loop limit ended its exchange.
fn center(screen: &WorkspaceScreen, ui: &mut View<'_, Msg>) {
    let waiting = screen
        .workspace()
        .and_then(OpenWorkspace::active_tab)
        .filter(|tab| !tab.letters().is_empty() || tab.stopped().is_some());
    match waiting {
        Some(tab) => {
            ui.column(|ui| {
                ui.column(|ui| tab_body(screen, ui)).fill();
                bridge::view(tab, ui);
            })
            .fill();
        }
        None => tab_body(screen, ui),
    }
}

/// The open tab, or why there is nothing to show.
fn tab_body(screen: &WorkspaceScreen, ui: &mut View<'_, Msg>) {
    let Some(workspace) = screen.workspace() else {
        let open =
            Button::new(t!("workspace.none.open")).icon("workspace").variant("primary").on_press(Msg::AddWorkspace);
        ui.add(
            EmptyState::new(t!("workspace.none.title"))
                .icon("inbox")
                .message(t!("workspace.none.message"))
                .action(open),
        )
        .fill()
        .id("workspace-none");
        return;
    };
    let Some(tab) = workspace.active_tab() else {
        // Without an engine a blank tab can still be opened: its page shows what there would be
        // to choose and says why nothing can be.
        let message = if screen.engine.is_some() { t!("workspace.empty.message") } else { t!("workspace.no-engine") };
        let open = Button::new(t!("workspace.new-tab")).variant("primary").on_press(Msg::NewTab);
        ui.add(EmptyState::new(t!("workspace.empty.title")).icon("inbox").message(message).action(open))
            .fill()
            .id("workspace-empty");
        return;
    };
    if tab.kind() == &TabKind::New {
        blank::view(screen, workspace, tab.key(), ui);
        return;
    }
    if let TabKind::Markdown(file) = tab.kind() {
        document(tab, file, ui);
        return;
    }
    if viewer::shows(tab) {
        viewer::view(tab, ui);
        return;
    }
    if let TabKind::Sound(file) = tab.kind()
        && sound::shows(tab)
    {
        sound::view(tab, file, ui);
        return;
    }
    // A window's tab draws its own status in every state it can be in, failure included: there is
    // no terminal on it and no last screen to keep, so the general shape below does not fit.
    if let TabKind::Desktop(profile) = tab.kind() {
        desktop::view(screen, tab, profile, ui);
        return;
    }
    let image = matches!(tab.kind(), TabKind::Image(_));
    let restart = t!("workspace.restart");
    match tab.state() {
        // With an engine a waiting tab is started before the frame is drawn, so only a screen
        // without one ever shows it, and then the reason is what there is to say.
        TabState::Waiting if screen.engine.is_none() => {
            ui.add(EmptyState::new(t!("workspace.waiting")).icon("inbox").message(t!("workspace.no-engine")))
                .fill()
                .id("workspace-waiting");
        }
        TabState::Waiting | TabState::Starting => {
            ui.column(|ui| {
                ui.add(Spinner::new().label(t!("workspace.starting")));
            })
            .fill()
            .align(Align::Center)
            .justify(Align::Center)
            .id("workspace-starting");
        }
        TabState::Running | TabState::Ended { .. } if image => picture(tab, ui),
        TabState::Running => match tab.session() {
            Some(session) => {
                terminal(ui, session);
            }
            None => {
                ui.add(Spinner::new().label(t!("workspace.starting"))).fill();
            }
        },
        // Leaving the editor is how a file is closed, so that is what the tab says, and the way
        // back opens the same file again.
        TabState::Ended { .. } if matches!(tab.kind(), TabKind::Editor(_)) => {
            stopped(ui, tab, &t!("workspace.file.closed"), None, t!("workspace.file.reopen"));
        }
        TabState::Ended { code } => stopped(ui, tab, &ended_text(*code), None, restart),
        TabState::Stopped => stopped(ui, tab, &t!("workspace.stopped"), None, restart),
        TabState::Failed(failure) => {
            // A failure of the machine itself has no engine command to show above its words.
            let detail = if failure.command.is_empty() {
                failure.output.clone()
            } else {
                format!("{}\n{}", failure.command, failure.output)
            };
            // A refusal QCode recognises is said plainly first, with the line that puts it right;
            // the engine's words stay underneath for whoever wants to read them.
            match screen.engine.as_ref().and_then(|engine| help(engine.kind(), &failure.output)) {
                Some(help) => refused(ui, tab, &help, &detail, restart),
                None => stopped(ui, tab, &t!("workspace.failed"), Some(&detail), restart),
            }
        }
        TabState::NoImage => no_image(screen, tab, ui),
        TabState::Building(_) => building(tab, ui),
        TabState::Missing | TabState::Unreadable(_) => file_trouble(tab, ui),
    }
}

/// A profile's tab whose image the engine does not have: what is missing, and the offer to build
/// it now, the tab opening once it is built, or not to open the tab after all.
fn no_image(screen: &WorkspaceScreen, tab: &Tab, ui: &mut View<'_, Msg>) {
    let key = tab.key();
    let profile = match tab.kind() {
        TabKind::Profile(name) => name.clone(),
        _ => String::new(),
    };
    let engine = screen.engine.as_ref().map(|engine| engine_name(engine.kind())).unwrap_or_default();
    ui.column(|ui| {
        ui.add(Text::new(t!("workspace.image.missing", profile = profile, engine = engine)).bold()).fill_width();
        ui.add(Text::new(t!("workspace.image.missing-why")).role("secondary")).fill_width();
        ui.row(|ui| {
            ui.add(Button::new(t!("workspace.image.build")).variant("primary").on_press(Msg::BuildImage(key)))
                .id("workspace-build-image");
            ui.add(Button::new(t!("workspace.image.cancel")).on_press(Msg::CancelOpen(key)))
                .id("workspace-cancel-open");
        })
        .gap(2);
    })
    .gap(1)
    .padding(Padding::symmetric(1, 2))
    .fill()
    .id("workspace-no-image");
}

/// A profile's tab whose image is being built: the build as it speaks, and the way to stop it.
fn building(tab: &Tab, ui: &mut View<'_, Msg>) {
    let key = tab.key();
    let profile = match tab.kind() {
        TabKind::Profile(name) => name.clone(),
        _ => String::new(),
    };
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Spinner::new().label(t!("workspace.image.building", profile = profile)));
            ui.spacer();
            ui.add(Button::new(t!("workspace.image.stop")).on_press(Msg::StopBuild(key))).id("workspace-stop-build");
        })
        .fill_width();
        ui.add(LogView::new(tab.build_log())).fill().id("workspace-build-log");
    })
    .gap(1)
    .padding(Padding::symmetric(1, 2))
    .fill()
    .id("workspace-building");
}

/// A picture drawn in the tab: what chafa drew, which stays when chafa is done, and under it a
/// quiet way to draw it again once the tab has changed size. A picture that could not be drawn
/// says so beside it; one that was drawn says nothing, because a picture that is there needs no
/// note that the program drawing it has finished.
fn picture(tab: &Tab, ui: &mut View<'_, Msg>) {
    let key = tab.key();
    let failed = matches!(tab.state(), TabState::Ended { code: Some(code) } if *code != 0);
    ui.column(|ui| {
        match tab.session() {
            Some(session) => {
                terminal(ui, session);
            }
            None if tab.state() == &TabState::Running => {
                ui.add(Spinner::new().label(t!("workspace.starting"))).fill();
            }
            None => {
                ui.spacer();
            }
        }
        ui.row(|ui| {
            if failed {
                ui.add(Text::new(t!("workspace.file.not-drawn")).role("secondary"));
            }
            ui.spacer();
            ui.add(Button::new(t!("workspace.file.redraw")).on_press(Msg::Restart(key))).id("workspace-redraw");
        })
        .gap(2)
        .padding(Padding { left: 1, right: 1, ..Padding::default() })
        .fill_width();
    })
    .fill()
    .id("workspace-picture");
}

/// A Markdown tab: the document, with the way to change it in the editor above it.
fn document(tab: &Tab, file: &str, ui: &mut View<'_, Msg>) {
    match (tab.state(), tab.document()) {
        (TabState::Missing | TabState::Unreadable(_), _) => file_trouble(tab, ui),
        // Read again while it is shown, the text it had stays until the new one is in.
        (_, Some(text)) => {
            let edit = Msg::EditFile(file.to_owned());
            ui.column(|ui| {
                ui.row(|ui| {
                    ui.add(Text::new(file.to_owned()).role("faint").no_wrap());
                    ui.spacer();
                    ui.add(Button::new(t!("workspace.file.edit")).on_press(edit)).id("workspace-edit");
                })
                .gap(2)
                .padding(Padding { left: 2, right: 1, ..Padding::default() })
                .fill_width();
                ui.add_with(ScrollView::new(), |ui| {
                    ui.column(|ui| {
                        ui.add(Markdown::new(text)).fill_width();
                    })
                    .padding(Padding::symmetric(0, 2))
                    .fill_width();
                })
                .fill()
                .id(DOCUMENT_ID);
            })
            .gap(1)
            .fill()
            .id("workspace-document-page");
        }
        (_, None) => {
            ui.column(|ui| {
                ui.add(Spinner::new().label(t!("workspace.file.reading")));
            })
            .fill()
            .align(Align::Center)
            .justify(Align::Center)
            .id("workspace-reading");
        }
    }
}

/// A tab whose file is gone or cannot be read, with the way to look for it again.
fn file_trouble(tab: &Tab, ui: &mut View<'_, Msg>) {
    let file = tab.kind().file().unwrap_or_default().to_owned();
    match tab.state() {
        TabState::Unreadable(reason) => {
            stopped(ui, tab, &t!("workspace.file.unreadable"), Some(reason), t!("workspace.file.try-again"));
        }
        _ => stopped(ui, tab, &t!("workspace.file.missing", file = file), None, t!("workspace.file.try-again")),
    }
}

/// The terminal of a harness or shell tab.
///
/// It hands the program every key but the few that must stay QCode's while a harness has the
/// keyboard: the key list, the side panel's key and the way out to the tabs. Only keys with
/// `ctrl` or `alt` and keys that type nothing, such as `f1`, can pass; `?` still types into the
/// program, because the terminal is where the person writes.
fn tab_terminal(session: &TerminalSession) -> Terminal {
    Terminal::new(session)
        .pass_through(Scope::Global, "help")
        .pass_through(Scope::Global, "toggle-panel")
        .pass_through(Scope::App, "leave-terminal")
}

/// Puts a tab's terminal on screen.
///
/// The key that leaves it is answered here, by the terminal's own node: while the keyboard is in
/// the terminal the key goes to the tab strip, and anywhere else it reaches the application,
/// which sends it back in. The runtime looks at where the keyboard is when the key arrives, so a
/// click or Tab that moved it is never a press lost.
fn terminal(ui: &mut View<'_, Msg>, session: &TerminalSession) {
    ui.add(tab_terminal(session)).fill().id(TERMINAL_ID).on_action(Scope::App, "leave-terminal", Msg::LeaveTerminal);
}

/// The words for a session that ended by itself.
fn ended_text(code: Option<u32>) -> String {
    match code {
        Some(0) | None => t!("workspace.ended"),
        Some(code) => t!("workspace.ended-code", code = code),
    }
}

/// How many tabs of the open workspaces have an agent in them that is running or starting: a
/// harness in a terminal, or a harness's window. A shell, a file or a tab that has ended is left
/// out, since quitting takes nothing from it that opening the workspace again does not give back.
#[must_use]
pub fn agents_at_work(screen: &WorkspaceScreen) -> usize {
    screen
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.tabs.iter())
        .filter(|tab| matches!(tab.kind(), TabKind::Profile(_) | TabKind::Desktop(_)))
        .filter(|tab| matches!(tab.state(), TabState::Running | TabState::Starting))
        .count()
}

/// A tab that is not running: what happened, the engine's own words when there are any, and the
/// way back. The last screen of the session stays above it while there is one.
fn stopped(ui: &mut View<'_, Msg>, tab: &Tab, message: &str, detail: Option<&str>, again: String) {
    let key = tab.key();
    ui.column(|ui| {
        if let Some(session) = tab.session() {
            terminal(ui, session);
        }
        ui.column(|ui| {
            ui.add(Text::new(message.to_owned()).role("secondary"));
            if let Some(detail) = detail {
                ui.add(Text::new(detail.to_owned()).role("faint")).selectable(true);
            }
            ui.add(Button::new(again).variant("primary").on_press(Msg::Restart(key))).id("workspace-restart");
        })
        .gap(1)
        .padding(Padding::symmetric(1, 2))
        .fill_width();
    })
    .fill()
    .id("workspace-stopped");
}

/// A tab the engine refused for a reason QCode recognises: the plain sentence, the line to run
/// when there is one, the engine's own words below them, and the way to try again.
fn refused(ui: &mut View<'_, Msg>, tab: &Tab, help: &Help, detail: &str, again: String) {
    let key = tab.key();
    ui.column(|ui| {
        ui.column(|ui| {
            help.show(ui);
            ui.add(Button::new(again).variant("primary").on_press(Msg::Restart(key))).id("workspace-restart");
            ui.add(Text::new(t!("known.said")).role("faint"));
            ui.add(Text::new(detail.to_owned()).role("faint")).selectable(true);
        })
        .gap(1)
        .padding(Padding::symmetric(1, 2))
        .fill_width();
    })
    .fill()
    .id("workspace-stopped");
}

/// The control that takes the keyboard when the screen opens: the terminal of the open tab, the
/// page of a blank one, or the strip's `+` that opens the first tab while there is none.
#[must_use]
pub fn entry(screen: &WorkspaceScreen) -> &'static str {
    match screen.workspace().and_then(OpenWorkspace::active_tab).map(Tab::kind) {
        Some(TabKind::New) => blank::CHOICES_ID,
        Some(TabKind::Markdown(_)) => DOCUMENT_ID,
        Some(TabKind::Pdf(_) | TabKind::Office(_))
            if !screen.workspace().and_then(OpenWorkspace::active_tab).is_some_and(viewer::draws) =>
        {
            DOCUMENT_ID
        }
        Some(TabKind::Sound(_))
            if screen.workspace().and_then(OpenWorkspace::active_tab).is_some_and(|tab| tab.quiet().is_some()) =>
        {
            DOCUMENT_ID
        }
        Some(TabKind::Desktop(_)) => desktop::WINDOW_ID,
        Some(_) => TERMINAL_ID,
        None => TABS_ID,
    }
}

/// What Esc means on the workspace screen before it means leaving it: while an entry of the file
/// tree is cut, Esc lets it stay where it is. The application asks this when Esc reaches it,
/// which is only after every nearer widget and dialog passed it on.
#[must_use]
pub fn escape(screen: &WorkspaceScreen) -> Option<Msg> {
    screen.workspace().filter(|workspace| !workspace.files.cut().is_empty()).map(|_| Msg::Files(FileMsg::DropCut))
}

/// The keys of the workspace screen that are not in the keymap, for the key list.
///
/// A focused terminal hands every key to the program in it but a few: `shift+tab`, which leaves
/// it, `ctrl+q`, and the key list and the side panel's key. So the list says how to get out of
/// the terminal, after which the rest of these keys work again.
#[must_use]
pub fn hints(icons: &Icons) -> Vec<(String, String)> {
    let tab_keys = format!("{}{}", icons.glyph("arrow-left"), icons.glyph("arrow-right"));
    // Written the way the keymap's own keys are, so the list reads as one.
    let label = |chord: &str| chord.parse::<KeyChord>().map_or_else(|_| chord.to_owned(), |chord| chord.label());
    vec![
        (tab_keys, t!("workspace.hints.tabs")),
        (label("ctrl+w"), t!("workspace.hints.close")),
        (label("shift+tab"), t!("workspace.hints.leave")),
    ]
}
