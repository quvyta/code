//! The project screen: the rail of open projects on the left, the tabs of one project on top,
//! the container's terminal in the middle and the widget panel on the right.
//!
//! The rail holds the projects the person opened and nothing else, the way a browser holds its
//! windows: a project joins it when it is opened from the list, leaves it when it is closed, and
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

mod blank;
mod file_ops;
mod files;
mod history;
mod panel;
mod plan;
mod tab;
mod watch;

#[cfg(test)]
mod live;
#[cfg(test)]
mod tests;

pub use blank::Choice;
pub use file_ops::{Change, FileError, NameProblem};
pub use files::{DocumentTrouble, FileEntry, FileMsg, FileTree, NameFor, Naming};
pub use history::HistoryKey;
pub use panel::{Panel, PanelWidget};
pub use plan::{ASSETS_DIR, ContainerPlan, HOME_DIR, KEEP_ALIVE, LaunchFailure, PROJECT_DIR, SHELL};
pub use tab::{Tab, TabKey, TabKind, TabState};

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

use qframe::date::Date;
use qframe::icons::Icons;
use qframe::keymap::KeyChord;
use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::storage::FolderChange;
use qframe::widgets::{
    CollapsedMarker, EmptyState, Markdown, RailTab, ScrollView, Side, SidePanel, Spinner, TabEdit, TabRail, TabWidth,
    Terminal, TerminalEvent, TerminalSession, Toast,
};

use std::path::{Path, PathBuf};

use crate::base::apps::{self, Editor, FileKind};
use crate::engine::{Container, Engine, EngineCommand, EngineKind, HostUser};
use crate::profile::Profile;
use crate::profile::history::Conversation;
use crate::workspace::{
    ProjectFile, ProjectId, ProjectPaths, Registry, Session, SessionProject, SessionTab, SessionTabKind, add_profile,
};

/// Width of the project rail: the framework's collapsed strip, which is the same four cells on
/// every screen. The rail never gives way to a narrow terminal, because losing it would mean
/// losing the only way between projects.
const RAIL_WIDTH: u16 = 4;

/// Height of one project block in the rail, in lines.
const RAIL_ROWS: u16 = 3;

/// Width of the widget panel when it is opened, and the range it can be dragged through. The
/// framework keeps the terminal a quarter of the screen whatever is asked for here.
const PANEL_WIDTH: u16 = 32;
/// Narrowest the widget panel can be dragged.
const PANEL_MIN: u16 = 18;
/// Widest the widget panel can be dragged.
const PANEL_MAX: u16 = 52;

/// Everything that can happen on the project screen.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A project of the rail was opened.
    OpenProject(usize),
    /// A project was closed from the rail, with every tab in it.
    CloseProject(usize),
    /// The list of projects was asked for, to open another one beside these. The screen itself
    /// cannot go there; the application that holds both answers this message.
    AddProject,
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
    /// The profiles screen was asked for, to make the first profile. The screen itself cannot go
    /// there; the application that holds both answers this message.
    ManageProfiles,
    /// The profiles of the workspace were read again, after the person may have changed them.
    Profiles(Vec<Profile>),
    /// A profile was recorded in a project's `project.qcode`, or could not be.
    ProfileAdded(String, Result<ProjectFile, String>),
    /// The container of a tab is up, or the engine refused.
    Ready(TabKey, u64, Result<(), LaunchFailure>),
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
    /// A folder of a project's file tree was read.
    FolderRead(String, String, Result<Vec<FileEntry>, String>),
    /// Something happened to the files of the file tree.
    Files(FileMsg),
    /// Other programs changed folders a project's tree shows: the project's id, the number of
    /// the watch that saw it, and what changed.
    FilesChanged(String, u64, Vec<FolderChange>),
    /// A file of the file tree was opened: Enter or a click on it.
    OpenFile(String),
    /// A file was asked for in the editor, from the Markdown tab that shows it.
    EditFile(String),
    /// The file the tab of this key opens is not in the project any more.
    Missing(TabKey, u64),
    /// The document a Markdown tab shows was read, or could not be.
    DocumentRead(TabKey, u64, Result<String, DocumentTrouble>),
    /// A row of the container widget was selected.
    SelectContainer(usize),
    /// The container widget was asked for the engine's current answer.
    RefreshContainers,
    /// The containers of a project came back from the engine.
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

/// One project the rail can switch to, with its own tabs and its own panel content.
#[derive(Debug)]
pub struct OpenProject {
    id: ProjectId,
    name: String,
    paths: ProjectPaths,
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
}

impl OpenProject {
    /// A project as its own file describes it, living at `paths`, offering `profiles`.
    ///
    /// The profiles are every profile of the workspace, already read: this screen shows
    /// profiles and enters their containers, it does not go looking for their definitions. The
    /// ones the project's `project.qcode` names come first; any other one is added to the project
    /// the first time a tab of it is opened, so a profile made after the project needs no hand
    /// edit to be used in it.
    #[must_use]
    pub fn new(file: &ProjectFile, paths: ProjectPaths, profiles: Vec<Profile>) -> Self {
        let files = FileTree::new(paths.project.clone());
        let carried = file.profiles.iter().map(|profile| profile.name.clone()).collect();
        let mut project = Self {
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
        };
        project.set_profiles(profiles);
        project
    }

    /// Takes `profiles` as the workspace's profiles, the ones the project carries first in the
    /// order its file lists them, then the others in the order they came.
    fn set_profiles(&mut self, mut profiles: Vec<Profile>) {
        profiles.sort_by_key(|profile| {
            self.carried.iter().position(|name| name == profile.name.as_str()).unwrap_or(usize::MAX)
        });
        self.profiles = profiles;
    }

    /// The project's identifier, which is also what its containers are named after.
    #[must_use]
    pub fn id(&self) -> &str {
        self.id.as_str()
    }

    /// The name the person gave the project.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the project's folders are.
    #[must_use]
    pub fn paths(&self) -> &ProjectPaths {
        &self.paths
    }

    /// The profiles a new tab can open: every profile of the workspace, the ones the project
    /// carries first.
    #[must_use]
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// Whether the project's `project.qcode` names the profile `name`.
    #[must_use]
    pub fn carries(&self, name: &str) -> bool {
        self.carried.iter().any(|carried| carried == name)
    }

    /// The tabs open in this project, in the order they are shown.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The open tab, when there is one.
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    /// What the file tree knows about the project's own folder.
    #[must_use]
    pub fn files(&self) -> &FileTree {
        &self.files
    }

    /// The containers of this project, as the engine last listed them.
    #[must_use]
    pub fn containers(&self) -> &[Container] {
        &self.containers
    }

    /// The container a tab of `kind` enters. A picture and a file in the editor are opened in
    /// the project's own container, the one its shell runs in; a Markdown document needs none.
    #[must_use]
    pub fn plan(&self, kind: &TabKind) -> Option<ContainerPlan> {
        match kind {
            TabKind::Shell | TabKind::Image(_) | TabKind::Editor(_) => {
                Some(ContainerPlan::base(self.id.as_str(), &self.paths))
            }
            TabKind::Markdown(_) => None,
            TabKind::Profile(name) => self
                .profiles
                .iter()
                .find(|profile| profile.name.as_str() == name)
                .map(|profile| ContainerPlan::profile(&self.id, &self.paths, profile)),
            TabKind::New => None,
        }
    }

    /// What a tab of `kind` runs inside its container: a login shell, the harness of the
    /// profile with the arguments that let it work without asking, opening `conversation` when
    /// the tab shows one and a new conversation otherwise, chafa drawing a picture, or `editor`
    /// on a file.
    #[must_use]
    pub fn program(&self, kind: &TabKind, conversation: Option<&str>, editor: Editor) -> Option<Vec<String>> {
        match kind {
            TabKind::Shell => Some(plan::SHELL.iter().map(|part| (*part).to_owned()).collect()),
            TabKind::Profile(name) => {
                let profile = self.profiles.iter().find(|profile| profile.name.as_str() == name)?;
                Some(profile.harness.command_line(conversation))
            }
            TabKind::Image(file) => Some(apps::picture(&inside_container(file))),
            TabKind::Editor(file) => Some(editor.command(&inside_container(file))),
            TabKind::New | TabKind::Markdown(_) => None,
        }
    }

    /// The label of the tab at `index`: the profile's name, `Shell` numbered among the shell tabs
    /// so several of them can be told apart, or `New tab` while it is blank.
    fn tab_label(&self, index: usize) -> String {
        let Some(tab) = self.tabs.get(index) else { return String::new() };
        match tab.kind() {
            TabKind::Profile(name) => name.clone(),
            TabKind::New => t!("project.new-tab"),
            TabKind::Image(file) | TabKind::Markdown(file) | TabKind::Editor(file) => files::name(file).to_owned(),
            TabKind::Shell => {
                let shell = t!("project.tab.shell");
                let position = self.tabs[..index].iter().filter(|tab| tab.kind() == &TabKind::Shell).count() + 1;
                let total = self.tabs.iter().filter(|tab| tab.kind() == &TabKind::Shell).count();
                if total > 1 { format!("{shell} {position}") } else { shell }
            }
        }
    }

    /// The tab of `key` and where it sits.
    fn find(&mut self, key: TabKey) -> Option<(usize, &mut Tab)> {
        self.tabs.iter_mut().enumerate().find(|(_, tab)| tab.key() == key)
    }
}

/// Where the project's file `key` is inside a container: under [`PROJECT_DIR`], which is the
/// project's own folder mounted.
fn inside_container(key: &str) -> String {
    format!("{PROJECT_DIR}/{key}")
}

/// The project screen.
#[derive(Debug)]
pub struct ProjectScreen {
    engine: Option<Engine>,
    user: HostUser,
    projects: Vec<OpenProject>,
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
    next_key: u64,
    /// Whether the open project's folders are watched, so the tree follows the disk.
    live: bool,
    /// The number the last folder watch was given.
    last_watch: u64,
    /// Where the containers this screen starts are noted, so they can be stopped once no QCode
    /// is open; `None` notes nothing.
    registry: Option<PathBuf>,
    /// Whether a problem with that list was said already: it is said once, not at every tab.
    registry_told: bool,
}

impl ProjectScreen {
    /// A screen showing `projects`, entering their containers through `engine`.
    ///
    /// Without an engine the screen still opens: the design asks that a missing engine leave the
    /// application usable, so every action that would touch a container is drawn faint and says
    /// why instead of failing when it is pressed.
    #[must_use]
    pub fn new(engine: Option<Engine>, user: HostUser, projects: Vec<OpenProject>) -> Self {
        Self {
            engine,
            user,
            projects,
            active: 0,
            panel: Panel::default(),
            blank_row: 0,
            expanded: HashSet::new(),
            shown_blank: None,
            shown_document: None,
            editor: Editor::default(),
            next_key: 0,
            live: false,
            last_watch: 0,
            registry: None,
            registry_told: false,
        }
    }

    /// The same screen following the disk when `live` is true: the folders the open project's
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

    /// The same screen, noting every container it starts in the list at `path`, which is what
    /// lets them be stopped once no QCode is open. `None` notes nothing.
    #[must_use]
    pub fn with_registry(mut self, path: Option<PathBuf>) -> Self {
        self.registry = path;
        self
    }

    /// The project the rail has open.
    #[must_use]
    pub fn project(&self) -> Option<&OpenProject> {
        self.projects.get(self.active)
    }

    /// Every project of the rail, in its order.
    #[must_use]
    pub fn projects(&self) -> &[OpenProject] {
        &self.projects
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

    /// Opens the project at `index` of the rail, when there is one there.
    pub fn set_active(&mut self, index: usize) {
        if index < self.projects.len() {
            self.active = index;
        }
    }

    /// Gives the project at `index` the tabs `record` lists, each waiting to be shown before it
    /// starts, with the tab the record had open open again.
    ///
    /// A profile tab whose profile is gone from the workspace is left out: there is no container
    /// it could enter, and a tab that can only ever fail is not worth bringing back. A blank tab
    /// comes back blank; it never had a container. A tab of one of the project's files comes back
    /// whether or not the file is still there, and says so when it is shown; one whose path
    /// climbs out of the project is left out, because nothing QCode wrote would name one.
    pub fn restore_tabs(&mut self, index: usize, record: &SessionProject) {
        let Some(project) = self.projects.get_mut(index) else { return };
        let mut active = 0;
        for (position, saved) in record.tabs.iter().enumerate() {
            let kind = match &saved.kind {
                SessionTabKind::Shell => TabKind::Shell,
                SessionTabKind::Profile(name) => TabKind::Profile(name.clone()),
                SessionTabKind::New => TabKind::New,
                SessionTabKind::Image(file) => TabKind::Image(file.clone()),
                SessionTabKind::Markdown(file) => TabKind::Markdown(file.clone()),
                SessionTabKind::Editor(file) => TabKind::Editor(file.clone()),
            };
            let usable = match &kind {
                TabKind::New => true,
                TabKind::Image(file) | TabKind::Markdown(file) | TabKind::Editor(file) => files::is_inside(file),
                TabKind::Shell | TabKind::Profile(_) => project.plan(&kind).is_some(),
            };
            if !usable {
                continue;
            }
            if position <= record.active_tab {
                active = project.tabs.len();
            }
            let key = TabKey(self.next_key);
            self.next_key += 1;
            project.tabs.push(Tab::restored(key, kind, saved.opened, saved.conversation.clone()));
        }
        project.active_tab = active;
    }

    /// What is open, in the shape the session file keeps it: the projects in rail order, the one
    /// that is open, and the tabs of each.
    #[must_use]
    pub fn session(&self) -> Session {
        let projects = self
            .projects
            .iter()
            .map(|project| SessionProject {
                id: project.id.clone(),
                active_tab: project.active_tab,
                tabs: project
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
                        },
                        conversation: tab.conversation().map(str::to_owned),
                        opened: tab.opened(),
                    })
                    .collect(),
            })
            .collect();
        Session { active: self.project().map(|project| project.id.clone()), projects }
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
        let project = self.projects.iter().find(|project| project.tabs.iter().any(|tab| tab.key() == key))?;
        let tab = project.tabs.iter().find(|tab| tab.key() == key)?;
        let plan = project.plan(tab.kind())?;
        let program = project.program(tab.kind(), tab.conversation(), self.editor)?;
        let parts: Vec<&str> = program.iter().map(String::as_str).collect();
        Some(plan.enter(engine, &parts))
    }

    /// The project of `id`, when the screen has it open.
    fn project_mut(&mut self, id: &str) -> Option<&mut OpenProject> {
        self.projects.iter_mut().find(|project| project.id.as_str() == id)
    }

    /// What a blank tab's page knows of the conversations of `key`, when its project is open.
    fn shelf(&mut self, key: &HistoryKey) -> Option<&mut history::Shelf> {
        Some(self.project_mut(&key.project)?.history.entry(key.profile.clone()).or_default())
    }

    /// The project holding the tab `key`, when one does.
    fn owner_mut(&mut self, key: TabKey) -> Option<&mut OpenProject> {
        self.projects.iter_mut().find(|project| project.tabs.iter().any(|tab| tab.key() == key))
    }
}

impl ProjectScreen {
    /// Attaches `session` to the tab `key` and marks the tab running, which is what the screen
    /// does itself once a tab's container is up. A program started some other way, such as the
    /// shell of a demonstration, is shown in the tab the same way.
    pub fn attach(&mut self, key: TabKey, session: TerminalSession) {
        if let Some((_, tab)) = self.owner_mut(key).and_then(|project| project.find(key)) {
            tab.attached(session);
        }
    }

    /// The number of the reading of `profile`'s conversations last started in the open project.
    /// An answer, [`Msg::HistoryRead`], is only taken when it carries this number.
    #[must_use]
    pub fn reading(&self, profile: &str) -> u64 {
        self.project().and_then(|project| project.history.get(profile)).map_or(0, history::Shelf::generation)
    }
}

/// The work the screen needs the moment it is shown: reading the open project's folder and
/// asking the engine which of its containers are up.
///
/// The screen never touches the disk or the engine while drawing, so this is the caller's part
/// of the bargain: run it when the screen is entered, and again when it is returned to.
///
/// It also starts the open tab when it was brought back from the last session and has been
/// waiting to be shown, and reads the conversations a blank tab's page offers when the open tab
/// is one.
pub fn opened(screen: &mut ProjectScreen) -> Command<Msg> {
    let command = Command::batch([load_root(screen), list_containers(screen), wake(screen), show_page(screen, true)]);
    Command::batch([command, follow_disk(screen)])
}

/// Adds `project` to the rail and opens it, or only opens it when the rail has it already; the
/// tabs of every project stay as they are either way.
pub fn add(screen: &mut ProjectScreen, project: OpenProject) -> Command<Msg> {
    match screen.projects.iter().position(|open| open.id == project.id) {
        Some(index) => screen.active = index,
        None => {
            screen.projects.push(project);
            screen.active = screen.projects.len() - 1;
        }
    }
    opened(screen)
}

/// Applies a message to the screen.
///
/// Whatever the message did, a tab that is now in view and was brought back from the last session
/// starts here: switching to it, closing the tab before it and opening its project all show it.
/// The same goes for a blank tab's page that comes into view, which reads its conversations.
pub fn update(screen: &mut ProjectScreen, message: Msg) -> Command<Msg> {
    // Profiles read again may be new ones, whose conversations the page shown has not read.
    let profiles = matches!(message, Msg::Profiles(_));
    let command = apply(screen, message);
    Command::batch([command, wake(screen), show_page(screen, profiles), reread(screen), follow_disk(screen)])
}

/// Keeps the open project's watch on the folders its tree shows, whatever the message changed
/// about them, and lets the watches of the other projects go: their trees are not on screen, and
/// they are read again when they are opened.
fn follow_disk(screen: &mut ProjectScreen) -> Command<Msg> {
    let mut commands = Vec::new();
    for (index, project) in screen.projects.iter_mut().enumerate() {
        if screen.live && index == screen.active {
            commands.push(watch::follow(project, &mut screen.last_watch));
        } else {
            watch::stop(project);
        }
    }
    Command::batch(commands)
}

/// Applies a message to the screen, leaving the start of a waiting tab to [`update`].
fn apply(screen: &mut ProjectScreen, message: Msg) -> Command<Msg> {
    match message {
        Msg::OpenProject(index) => {
            screen.set_active(index);
            Command::batch([load_root(screen), list_containers(screen)])
        }
        Msg::CloseProject(index) => {
            let Some(project) = screen.projects.get_mut(index) else { return Command::none() };
            for tab in &mut project.tabs {
                tab.close_session();
            }
            TabEdit::Close(index).apply(&mut screen.projects, &mut screen.active);
            Command::batch([load_root(screen), list_containers(screen)])
        }
        // The application opens the list; nothing on this screen changes until a project comes
        // back from it.
        Msg::AddProject => Command::none(),
        Msg::OpenTab(index) => {
            if let Some(project) = screen.projects.get_mut(screen.active)
                && index < project.tabs.len()
            {
                project.active_tab = index;
            }
            Command::none()
        }
        Msg::CloseTab(index) => {
            let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
            if let Some(tab) = project.tabs.get_mut(index) {
                tab.close_session();
            }
            TabEdit::Close(index).apply(&mut project.tabs, &mut project.active_tab);
            Command::none()
        }
        Msg::MoveTab { from, to } => {
            if let Some(project) = screen.projects.get_mut(screen.active) {
                TabEdit::Move { from, to }.apply(&mut project.tabs, &mut project.active_tab);
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
            let terminal = screen.project().and_then(OpenProject::active_tab).and_then(Tab::session).is_some();
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
            for project in &mut screen.projects {
                project.set_profiles(profiles.clone());
            }
            Command::none()
        }
        Msg::ProfileAdded(id, result) => match result {
            Ok(file) => {
                if let Some(project) = screen.project_mut(&id) {
                    project.carried = file.profiles.into_iter().map(|profile| profile.name).collect();
                }
                Command::none()
            }
            // The tab is open whatever the file says; only the record of it is missing, and the
            // person is told so rather than finding the profile gone from the file later.
            Err(reason) => Command::toast(Toast::warning(t!("project.add-failed")).body(reason)),
        },
        Msg::Ready(key, run, result) => ready(screen, key, run, &result),
        Msg::Woken(key, run, result, found) => {
            if let Some(found) = found {
                resume_newest(screen, key, run, &found);
            }
            ready(screen, key, run, &result)
        }
        Msg::Output(key, run, event) => output(screen, key, run, event),
        Msg::Checked(key, run, running) => {
            if let Some(project) = screen.owner_mut(key)
                && let Some((_, tab)) = project.find(key)
                && tab.run() == run
                && let TabState::Ended { .. } = tab.state()
                && !running
            {
                tab.settled(TabState::Stopped);
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
            if let Some(project) = screen.projects.get_mut(screen.active) {
                project.files.move_cursor(&key);
            }
            Command::none()
        }
        Msg::ExpandFile(key, open) => {
            let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
            if !project.files.expand(&key, open) {
                return Command::none();
            }
            read_folder(project, &key)
        }
        Msg::FolderRead(project, key, entries) => {
            if let Some(project) = screen.project_mut(&project) {
                project.files.read(&key, entries);
            }
            Command::none()
        }
        Msg::Files(message) => files::update(screen, message),
        Msg::FilesChanged(id, run, batch) => match screen.project_mut(&id) {
            Some(project) => watch::changed(project, run, batch),
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
            if let Some((_, tab)) = screen.owner_mut(key).and_then(|project| project.find(key))
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
        Msg::SelectContainer(index) => {
            if let Some(project) = screen.projects.get_mut(screen.active) {
                project.container_row = index;
            }
            Command::none()
        }
        Msg::RefreshContainers => list_containers(screen),
        Msg::ContainersRead(id, result) => {
            if let Some(project) = screen.project_mut(&id) {
                project.busy = false;
                match result {
                    Ok(containers) => {
                        project.container_row = project.container_row.min(containers.len().saturating_sub(1));
                        project.containers = containers;
                        project.container_error = None;
                    }
                    Err(failure) => project.container_error = Some(failure),
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
                    Noting::Unwritten(reason) => Toast::warning(t!("project.registry-unwritten")).body(reason),
                    Noting::Repaired(place) => Toast::warning(t!("project.registry-repaired")).body(place),
                };
                Command::toast(toast)
            };
            Command::batch([told, update(screen, *message)])
        }
        Msg::ContainerActed(result) => {
            let refresh = list_containers(screen);
            match result {
                Ok(()) => refresh,
                Err(failure) => Command::batch([
                    Command::toast(Toast::danger(t!("project.containers.failed")).body(failure.output)),
                    refresh,
                ]),
            }
        }
    }
}

/// Opens a blank tab after the last one and hands its page the keyboard. Nothing starts: the
/// tab only asks what it should open.
fn open_blank(screen: &mut ProjectScreen) -> Command<Msg> {
    let key = TabKey(screen.next_key);
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    screen.next_key += 1;
    project.tabs.push(Tab::blank(key));
    project.active_tab = project.tabs.len() - 1;
    screen.blank_row = 0;
    screen.expanded.clear();
    Command::focus(blank::CHOICES_ID)
}

/// Turns the blank tab `key` into what was chosen on its page, in its own place in the strip,
/// and starts the work that brings its container up.
fn choose(screen: &mut ProjectScreen, key: TabKey, choice: Choice) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let user = screen.user;
    let registry = screen.registry.clone();
    let Some(project) = screen.owner_mut(key) else { return Command::none() };
    let (kind, conversation) = match choice {
        Choice::Shell => (TabKind::Shell, None),
        Choice::NewChat(name) => (TabKind::Profile(name), None),
        Choice::Resume(name, id) => (TabKind::Profile(name), Some(id)),
    };
    let Some(plan) = project.plan(&kind) else { return Command::none() };
    // A tab that was chosen already is not chosen again: two quick presses open one container.
    if project.find(key).is_none_or(|(_, tab)| tab.kind() != &TabKind::New) {
        return Command::none();
    }
    let record = match &kind {
        TabKind::Profile(name) if !project.carries(name) => record_profile(project, name),
        _ => Command::none(),
    };
    let Some((_, tab)) = project.find(key) else { return Command::none() };
    tab.choose(kind, conversation);
    let run = tab.run();
    Command::batch([
        Command::perform(move || {
            start(registry.as_deref(), &engine, &plan, user, |result| Msg::Ready(key, run, result))
        }),
        Command::focus(TERMINAL_ID),
        record,
    ])
}

/// Writes into the project's `project.qcode` that it carries the profile `name`, on a background
/// thread.
///
/// The file is the one record of which project carries which profile: refreshing a login from its
/// profile reaches exactly the projects it names. The container itself does not wait for it, so
/// the tab opens either way.
fn record_profile(project: &OpenProject, name: &str) -> Command<Msg> {
    let id = project.id.as_str().to_owned();
    let paths = project.paths.clone();
    let name = name.to_owned();
    Command::perform(move || {
        let result = add_profile(&paths, &name, Date::today_utc()).map_err(|problem| problem.to_string());
        Msg::ProfileAdded(id, result)
    })
}

/// Starts the open tab of the open project when it was brought back from the last session and
/// is being shown for the first time. Without an engine it keeps waiting, and the middle says why.
/// A blank tab has no container to start, so it keeps asking what it should open.
///
/// A harness tab that was a new chat when the session was recorded has no conversation id: a new
/// conversation's id is the harness's to make, and it is not known when the tab starts. So once
/// its container is up the profile's conversations are read, and [`resume_newest`] gives the
/// tab the one it was most likely showing before its session is spawned.
fn wake(screen: &mut ProjectScreen) -> Command<Msg> {
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    let Some(tab) = project.tabs.get_mut(project.active_tab) else { return Command::none() };
    if tab.state() != &TabState::Waiting {
        return Command::none();
    }
    // A Markdown document is read from the disk by QCode itself, so it opens with or without an
    // engine.
    if let TabKind::Markdown(file) = tab.kind() {
        let file = file.clone();
        tab.wake();
        let (key, run) = (tab.key(), tab.run());
        return read_document(project, key, run, &file);
    }
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let user = screen.user;
    let registry = screen.registry.clone();
    let Some(tab) = project.tabs.get(project.active_tab) else { return Command::none() };
    let Some(plan) = project.plan(tab.kind()) else { return Command::none() };
    let file = tab.kind().file().map(|file| project.files.path(file));
    let harness = match tab.kind() {
        TabKind::Profile(name) if tab.conversation().is_none() => {
            project.profiles.iter().find(|profile| profile.name.as_str() == name).map(|profile| profile.harness)
        }
        _ => None,
    };
    let Some(tab) = project.tabs.get_mut(project.active_tab) else { return Command::none() };
    tab.wake();
    let (key, run) = (tab.key(), tab.run());
    match harness {
        None => bring_up(engine, plan, user, key, run, file, registry),
        Some(harness) => Command::perform(move || {
            start(registry.as_deref(), &engine, &plan, user, |result| {
                // Not being able to read only means the tab cannot be matched to its
                // conversation; it then starts a new one, the way it started last time. The
                // container is up by now, so reading it starts nothing.
                let found =
                    result.is_ok().then(|| history::read(&engine, &plan.name, harness, &mut || {}).ok()).flatten();
                Msg::Woken(key, run, result, found)
            })
        }),
    }
}

/// Gives the new-chat tab `key`, woken as `run`, the conversation of `found` it was most likely
/// showing ([`history::claim`]), so it opens that one and the session file keeps its id from now
/// on. A tab that was closed, restarted or given a conversation meanwhile is left as it is.
fn resume_newest(screen: &mut ProjectScreen, key: TabKey, run: u64, found: &[Conversation]) {
    let Some(project) = screen.owner_mut(key) else { return };
    let Some((_, tab)) = project.find(key) else { return };
    if tab.run() != run || !tab.state().is_starting() || tab.conversation().is_some() {
        return;
    }
    let (kind, opened) = (tab.kind().clone(), tab.opened());
    let taken: Vec<String> = project
        .tabs
        .iter()
        .filter(|other| other.key() != key && other.kind() == &kind)
        .filter_map(|other| other.conversation().map(str::to_owned))
        .collect();
    let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
    let Some(chosen) = history::claim(found, opened, &taken).map(|conversation| conversation.id.clone()) else {
        return;
    };
    if let Some((_, tab)) = project.find(key) {
        tab.show_conversation(chosen);
    }
}

/// Reads the conversations the page of the open tab offers, when the open tab is blank and its
/// page has just come into view, or always when `again` asks for it.
///
/// Every profile of the project is read at once, each on its own thread, and each keeps the rows
/// it had on screen until its answer is in.
fn show_page(screen: &mut ProjectScreen, again: bool) -> Command<Msg> {
    let showing = screen.project().and_then(OpenProject::active_tab).filter(|tab| tab.kind() == &TabKind::New);
    let showing = showing.map(Tab::key);
    let new = showing != screen.shown_blank;
    screen.shown_blank = showing;
    if showing.is_none() || !(new || again) {
        return Command::none();
    }
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let registry = screen.registry.clone();
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    let mut commands = Vec::new();
    for profile in &project.profiles {
        let key = HistoryKey { project: project.id.as_str().to_owned(), profile: profile.name.as_str().to_owned() };
        let generation = project.history.entry(key.profile.clone()).or_default().start();
        let container = ContainerPlan::profile(&project.id, &project.paths, profile).name;
        let (engine, harness, answer, registry) = (engine.clone(), profile.harness, key.clone(), registry.clone());
        commands.push(Command::perform(move || {
            // Reading a stopped container starts it and leaves it running for the tab that opens
            // a conversation from it, so it is noted like any container QCode starts.
            let mut started = false;
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
    Command::task(Task::new(t!("project.history.reading"), move |cx| {
        if cx.sleep(delay) { Ok(message) } else { Err(String::new()) }
    }))
}

/// Starts the tab `key` again, from its container upwards; a Markdown tab reads its file again.
fn restart_tab(screen: &mut ProjectScreen, key: TabKey) -> Command<Msg> {
    let engine = screen.engine.clone();
    let user = screen.user;
    let registry = screen.registry.clone();
    let Some(project) = screen.owner_mut(key) else { return Command::none() };
    let Some((_, tab)) = project.find(key) else { return Command::none() };
    let kind = tab.kind().clone();
    if let TabKind::Markdown(file) = &kind {
        tab.restarting();
        let run = tab.run();
        return read_document(project, key, run, file);
    }
    let Some(engine) = engine else { return Command::none() };
    tab.restarting();
    let run = tab.run();
    let Some(plan) = project.plan(&kind) else { return Command::none() };
    let file = kind.file().map(|file| project.files.path(file));
    bring_up(engine, plan, user, key, run, file, registry)
}

/// Brings the container of `plan` up for the tab `key` in its run `run`, on a background thread.
///
/// A tab that opens one of the project's files first looks for the file at `file` on this
/// machine: an editor started on a file that is gone would open an empty one of that name and
/// save it back, which is not what the tab promised, so the tab says the file is gone instead.
fn bring_up(
    engine: Engine,
    plan: ContainerPlan,
    user: HostUser,
    key: TabKey,
    run: u64,
    file: Option<PathBuf>,
    registry: Option<PathBuf>,
) -> Command<Msg> {
    Command::perform(move || {
        if file.is_some_and(|file| !file.exists()) {
            return Msg::Missing(key, run);
        }
        start(registry.as_deref(), &engine, &plan, user, |result| Msg::Ready(key, run, result))
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
    answer: impl FnOnce(Result<(), LaunchFailure>) -> Msg,
) -> Msg {
    let result = plan::ensure_running(engine, plan, user);
    let up = result.is_ok();
    let message = answer(result);
    if up { noted(registry, engine.kind(), &plan.name, message) } else { message }
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
fn read_document(project: &OpenProject, key: TabKey, run: u64, file: &str) -> Command<Msg> {
    let path = project.files.path(file);
    let root = project.files.root().to_path_buf();
    Command::perform(move || Msg::DocumentRead(key, run, files::read_document(&path, &root)))
}

/// Takes what reading a Markdown tab's document answered. A tab that was restarted or closed
/// meanwhile is left alone.
fn document_read(screen: &mut ProjectScreen, key: TabKey, run: u64, answer: Result<String, DocumentTrouble>) {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|project| project.find(key)) else { return };
    if tab.run() != run {
        return;
    }
    match answer {
        Ok(text) => tab.read(text),
        Err(DocumentTrouble::Missing) => tab.settled(TabState::Missing),
        Err(DocumentTrouble::Outside) => {
            let file = tab.kind().file().unwrap_or_default().to_owned();
            tab.settled(TabState::Unreadable(t!("project.file.outside", file = file)));
        }
        Err(DocumentTrouble::Unreadable(reason)) => tab.settled(TabState::Unreadable(reason)),
    }
}

/// Reads the open Markdown tab's document again when the tab comes back into view, so a change
/// made in the editor meanwhile is what it shows. The text it had stays on screen until the new
/// one is in.
fn reread(screen: &mut ProjectScreen) -> Command<Msg> {
    let Some(project) = screen.projects.get(screen.active) else {
        screen.shown_document = None;
        return Command::none();
    };
    let showing = project.active_tab().filter(|tab| matches!(tab.kind(), TabKind::Markdown(_)));
    let key = showing.map(Tab::key);
    let again = key.is_some() && key != screen.shown_document;
    screen.shown_document = key;
    match showing {
        Some(tab) if again && tab.state() == &TabState::Running => {
            let file = tab.kind().file().unwrap_or_default().to_owned();
            read_document(project, tab.key(), tab.run(), &file)
        }
        _ => Command::none(),
    }
}

/// Opens the file `key` of the open project's tree in the built-in app its name calls for, or
/// says that none opens it yet.
fn open_file(screen: &mut ProjectScreen, key: &str) -> Command<Msg> {
    if !files::is_inside(key) {
        return Command::none();
    }
    let kind = match apps::classify(files::name(key)) {
        FileKind::Image => TabKind::Image(key.to_owned()),
        FileKind::Markdown => TabKind::Markdown(key.to_owned()),
        FileKind::Text => TabKind::Editor(key.to_owned()),
        FileKind::Unknown => {
            return Command::toast(Toast::info(t!("project.file.no-app")).body(files::name(key).to_owned()));
        }
    };
    open_tab(screen, kind)
}

/// Shows the tab of `kind` in the open project: the one already open when there is one, since a
/// click on a file in the tree opens it and a second click should not open it twice, or a new
/// tab after the last one. The new tab waits to be started, which [`update`] does at once.
fn open_tab(screen: &mut ProjectScreen, kind: TabKind) -> Command<Msg> {
    let key = TabKey(screen.next_key);
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    let focus = if matches!(kind, TabKind::Markdown(_)) { DOCUMENT_ID } else { TERMINAL_ID };
    if let Some(index) = project.tabs.iter().position(|tab| tab.kind() == &kind) {
        project.active_tab = index;
        return Command::focus(focus);
    }
    screen.next_key += 1;
    project.tabs.push(Tab::waiting(key, kind));
    project.active_tab = project.tabs.len() - 1;
    Command::focus(focus)
}

/// Attaches a session to a tab whose container came up, or tells it why it did not.
fn ready(screen: &mut ProjectScreen, key: TabKey, run: u64, result: &Result<(), LaunchFailure>) -> Command<Msg> {
    let Some(command) = screen.launch_command(key) else { return Command::none() };
    let Some(project) = screen.owner_mut(key) else { return Command::none() };
    let folder = project.paths.root.clone();
    let Some((_, tab)) = project.find(key) else { return Command::none() };
    if tab.run() != run || !tab.state().is_starting() {
        return Command::none();
    }
    if let Err(failure) = result {
        tab.settled(TabState::Failed(failure.clone()));
        return Command::none();
    }
    let args: Vec<OsString> = command.args.clone();
    match TerminalSession::spawn(command.program.as_os_str(), &args, &folder) {
        Ok(session) => {
            let watch = session.watch();
            tab.attached(session);
            Command::perform(move || Msg::Output(key, run, watch.next()))
        }
        Err(error) => {
            tab.settled(TabState::Failed(LaunchFailure::spawn(&command, &error)));
            Command::none()
        }
    }
}

/// Keeps a tab's session watched, and asks after its container when the session ends.
fn output(screen: &mut ProjectScreen, key: TabKey, run: u64, event: TerminalEvent) -> Command<Msg> {
    let engine = screen.engine.clone();
    let Some(project) = screen.owner_mut(key) else { return Command::none() };
    let kind = project.find(key).map(|(_, tab)| tab.kind().clone());
    let plan = kind.and_then(|kind| project.plan(&kind));
    let Some((_, tab)) = project.find(key) else { return Command::none() };
    if tab.run() != run {
        return Command::none();
    }
    match event {
        TerminalEvent::Output => match tab.session() {
            Some(session) => {
                let watch = session.watch();
                Command::perform(move || Msg::Output(key, run, watch.next()))
            }
            None => Command::none(),
        },
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

/// Reads the root of the open project's file tree the first time, and every folder it shows
/// again after that: the files may have changed while the screen was away.
fn load_root(screen: &mut ProjectScreen) -> Command<Msg> {
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    if project.files.children(files::ROOT).is_some() {
        return files::refresh(project);
    }
    if !project.files.start_reading(files::ROOT) {
        return Command::none();
    }
    read_folder(project, files::ROOT)
}

/// Reads one folder of a project's file tree on a background thread.
fn read_folder(project: &OpenProject, key: &str) -> Command<Msg> {
    let id = project.id.as_str().to_owned();
    let key = key.to_owned();
    let path = project.files.path(&key);
    Command::perform(move || Msg::FolderRead(id, key, files::read_folder(&path)))
}

/// Asks the engine which containers the open project has.
fn list_containers(screen: &mut ProjectScreen) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    project.busy = true;
    let id = project.id.as_str().to_owned();
    Command::perform(move || {
        let containers = plan::project_containers(&engine, &id);
        Msg::ContainersRead(id, containers)
    })
}

/// Stops a container, or stops and starts it again.
fn container_action(screen: &ProjectScreen, name: &str, again: bool) -> Command<Msg> {
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
const TERMINAL_ID: &str = "project-terminal";

/// The name the document of a Markdown tab is focused by.
const DOCUMENT_ID: &str = "project-document";

/// The name the tab strip is focused by; with no tab its `+` is the whole strip, and the focus.
const TABS_ID: &str = "project-tabs";

/// Draws the project screen for an application whose messages are `P`: `wrap` turns the screen's
/// own messages into the application's, `above` draws what the application puts over the tabs
/// (its header) and `below` what it puts under the terminal (its footer).
///
/// The rail runs down the whole left edge and the widget panel down the whole right edge, from
/// the very first row to the very last; the application's header and footer belong to the middle
/// column only, with the tabs and the terminal. That is why the application hands its parts in
/// here instead of drawing them around this screen.
pub fn view<P: 'static>(
    screen: &ProjectScreen,
    ui: &mut View<'_, P>,
    wrap: fn(Msg) -> P,
    above: impl FnOnce(&mut View<'_, P>),
    below: impl FnOnce(&mut View<'_, P>),
) {
    AppShell::new()
        .sidebar_width(RAIL_WIDTH)
        // The rail is the only way between projects, so it never collapses away; four cells fit
        // on any terminal QCode can draw on.
        .collapse_below(0)
        .sidebar(|ui| {
            ui.map(wrap, |ui| rail(screen, ui)).fill();
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
                .id("project-body");
        })
        .show(ui);
}

/// The rail of open projects down the left, ending in the `+` that opens another one.
fn rail(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let tabs = screen.projects.iter().map(|project| {
        let running = project.containers.iter().filter(|container| container.state.is_running()).count();
        let tab = RailTab::new(project.name.clone()).icon("folder");
        if running > 0 { tab.status("success").badge(running.to_string()) } else { tab }
    });
    ui.add(
        TabRail::new(tabs)
            .active(screen.active)
            .collapsed(true)
            .collapsed_marker(CollapsedMarker::Initial)
            .row_height(RAIL_ROWS)
            .closable(Msg::CloseProject)
            .on_add(|| Msg::AddProject)
            .on_select(Msg::OpenProject),
    )
    .fill()
    .id("project-rail");
}

/// The tab strip, ending in the `+` that opens a blank tab right after its last tab, the way a
/// browser's strip ends; with no tab at all the `+` stands at the top left.
///
/// The strip places the `+` itself: it keeps the button's room while laying out the tabs, so the
/// `+` follows the last tab while they fit and keeps its place at the right end once they scroll.
fn header(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let Some(project) = screen.project() else { return };
    let labels: Vec<String> = (0..project.tabs.len()).map(|index| project.tab_label(index)).collect();
    ui.add(
        Tabs::new(labels)
            .active(project.active_tab)
            .tab_width(TabWidth::Fit)
            .closable(Msg::CloseTab)
            .reorderable(move |from, to| Msg::MoveTab { from, to })
            .on_select(Msg::OpenTab)
            .on_add(|| Msg::NewTab),
    )
    .fill_width()
    .id(TABS_ID);
}

/// What the middle shows: the open tab, or why there is nothing to show.
fn center(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let Some(project) = screen.project() else {
        let open = Button::new(t!("project.none.open")).icon("project").variant("primary").on_press(Msg::AddProject);
        ui.add(
            EmptyState::new(t!("project.none.title")).icon("inbox").message(t!("project.none.message")).action(open),
        )
        .fill()
        .id("project-none");
        return;
    };
    let Some(tab) = project.active_tab() else {
        // Without an engine a blank tab can still be opened: its page shows what there would be
        // to choose and says why nothing can be.
        let message = if screen.engine.is_some() { t!("project.empty.message") } else { t!("project.no-engine") };
        let open = Button::new(t!("project.new-tab")).variant("primary").on_press(Msg::NewTab);
        ui.add(EmptyState::new(t!("project.empty.title")).icon("inbox").message(message).action(open))
            .fill()
            .id("project-empty");
        return;
    };
    if tab.kind() == &TabKind::New {
        blank::view(screen, project, tab.key(), ui);
        return;
    }
    if let TabKind::Markdown(file) = tab.kind() {
        document(tab, file, ui);
        return;
    }
    let image = matches!(tab.kind(), TabKind::Image(_));
    let restart = t!("project.restart");
    match tab.state() {
        // With an engine a waiting tab is started before the frame is drawn, so only a screen
        // without one ever shows it, and then the reason is what there is to say.
        TabState::Waiting if screen.engine.is_none() => {
            ui.add(EmptyState::new(t!("project.waiting")).icon("inbox").message(t!("project.no-engine")))
                .fill()
                .id("project-waiting");
        }
        TabState::Waiting | TabState::Starting => {
            ui.column(|ui| {
                ui.add(Spinner::new().label(t!("project.starting")));
            })
            .fill()
            .align(Align::Center)
            .justify(Align::Center)
            .id("project-starting");
        }
        TabState::Running | TabState::Ended { .. } if image => picture(tab, ui),
        TabState::Running => match tab.session() {
            Some(session) => {
                terminal(ui, session);
            }
            None => {
                ui.add(Spinner::new().label(t!("project.starting"))).fill();
            }
        },
        // Leaving the editor is how a file is closed, so that is what the tab says, and the way
        // back opens the same file again.
        TabState::Ended { .. } if matches!(tab.kind(), TabKind::Editor(_)) => {
            stopped(ui, tab, &t!("project.file.closed"), None, t!("project.file.reopen"));
        }
        TabState::Ended { code } => stopped(ui, tab, &ended_text(*code), None, restart),
        TabState::Stopped => stopped(ui, tab, &t!("project.stopped"), None, restart),
        TabState::Failed(failure) => {
            // A failure of the machine itself has no engine command to show above its words.
            let detail = if failure.command.is_empty() {
                failure.output.clone()
            } else {
                format!("{}\n{}", failure.command, failure.output)
            };
            stopped(ui, tab, &t!("project.failed"), Some(&detail), restart);
        }
        TabState::Missing | TabState::Unreadable(_) => file_trouble(tab, ui),
    }
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
                ui.add(Spinner::new().label(t!("project.starting"))).fill();
            }
            None => {
                ui.spacer();
            }
        }
        ui.row(|ui| {
            if failed {
                ui.add(Text::new(t!("project.file.not-drawn")).role("secondary"));
            }
            ui.spacer();
            ui.add(Button::new(t!("project.file.redraw")).on_press(Msg::Restart(key))).id("project-redraw");
        })
        .gap(2)
        .padding(Padding { left: 1, right: 1, ..Padding::default() })
        .fill_width();
    })
    .fill()
    .id("project-picture");
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
                    ui.add(Button::new(t!("project.file.edit")).on_press(edit)).id("project-edit");
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
            .id("project-document-page");
        }
        (_, None) => {
            ui.column(|ui| {
                ui.add(Spinner::new().label(t!("project.file.reading")));
            })
            .fill()
            .align(Align::Center)
            .justify(Align::Center)
            .id("project-reading");
        }
    }
}

/// A tab whose file is gone or cannot be read, with the way to look for it again.
fn file_trouble(tab: &Tab, ui: &mut View<'_, Msg>) {
    let file = tab.kind().file().unwrap_or_default().to_owned();
    match tab.state() {
        TabState::Unreadable(reason) => {
            stopped(ui, tab, &t!("project.file.unreadable"), Some(reason), t!("project.file.try-again"));
        }
        _ => stopped(ui, tab, &t!("project.file.missing", file = file), None, t!("project.file.try-again")),
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
        Some(0) | None => t!("project.ended"),
        Some(code) => t!("project.ended-code", code = code),
    }
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
            ui.add(Button::new(again).variant("primary").on_press(Msg::Restart(key))).id("project-restart");
        })
        .gap(1)
        .padding(Padding::symmetric(1, 2))
        .fill_width();
    })
    .fill()
    .id("project-stopped");
}

/// The control that takes the keyboard when the screen opens: the terminal of the open tab, the
/// page of a blank one, or the strip's `+` that opens the first tab while there is none.
#[must_use]
pub fn entry(screen: &ProjectScreen) -> &'static str {
    match screen.project().and_then(OpenProject::active_tab).map(Tab::kind) {
        Some(TabKind::New) => blank::CHOICES_ID,
        Some(TabKind::Markdown(_)) => DOCUMENT_ID,
        Some(_) => TERMINAL_ID,
        None => TABS_ID,
    }
}

/// What Esc means on the project screen before it means leaving it: while an entry of the file
/// tree is cut, Esc lets it stay where it is. The application asks this when Esc reaches it,
/// which is only after every nearer widget and dialog passed it on.
#[must_use]
pub fn escape(screen: &ProjectScreen) -> Option<Msg> {
    screen.project().filter(|project| !project.files.cut().is_empty()).map(|_| Msg::Files(FileMsg::DropCut))
}

/// The keys of the project screen that are not in the keymap, for the key list.
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
        (tab_keys, t!("project.hints.tabs")),
        (label("ctrl+w"), t!("project.hints.close")),
        (label("shift+tab"), t!("project.hints.leave")),
    ]
}
