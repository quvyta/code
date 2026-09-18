//! The project screen: the rail of projects on the left, the tabs of one project on top, the
//! container's terminal in the middle and the widget panel on the right.
//!
//! The screen's whole reason for existing is one line of [`ContainerPlan::enter`]: a tab spawns
//! the engine binary with `exec`, so the program a tab shows always runs inside a container and
//! never on the machine QCode runs on. Everything else here is around that: which container a tab
//! enters, what a tab says when its container is not there, and what the panel shows.
//!
//! The screen speaks its own [`Msg`]: [`update`] answers with commands of it and [`view`] draws
//! for it, and the application maps both to its own message, so the screen is drawn and tested
//! without naming anything of the application.

mod files;
mod panel;
mod plan;
mod tab;

#[cfg(test)]
mod live;
#[cfg(test)]
mod tests;

pub use files::{FileEntry, FileTree};
pub use panel::{Panel, PanelWidget};
pub use plan::{ASSETS_DIR, ContainerPlan, HOME_DIR, KEEP_ALIVE, LaunchFailure, PROJECT_DIR, SHELL};
pub use tab::{Tab, TabKey, TabKind, TabState};

use std::ffi::OsString;

use qframe::date::Date;
use qframe::icons::Icons;
use qframe::keymap::KeyChord;
use qframe::prelude::*;
use qframe::widgets::{
    CollapsedMarker, EmptyState, Popover, RailTab, Side, SidePanel, Spinner, TabEdit, TabRail, TabWidth, Terminal,
    TerminalEvent, TerminalSession, Toast,
};

use crate::engine::{Container, Engine, EngineCommand, HostUser};
use crate::profile::Profile;
use crate::workspace::{ProjectFile, ProjectId, ProjectPaths, add_profile};

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

/// What a new tab opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewTab {
    /// A shell in the project's own container.
    Shell,
    /// The harness of the project's profile at this position.
    Profile(usize),
}

/// Everything that can happen on the project screen.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A project of the rail was opened.
    OpenProject(usize),
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
    /// The chooser of what a new tab opens was shown or dismissed.
    ShowPicker(bool),
    /// A new tab was asked for.
    NewTab(NewTab),
    /// The profiles screen was asked for, to make the first profile. The screen itself cannot go
    /// there; the application that holds both answers this message.
    ManageProfiles,
    /// The profiles of the workspace were read again, after the person may have changed them.
    Profiles(Vec<Profile>),
    /// A profile was recorded in a project's `project.qcode`, or could not be.
    ProfileAdded(String, Result<ProjectFile, String>),
    /// The container of a tab is up, or the engine refused.
    Ready(TabKey, u64, Result<(), LaunchFailure>),
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
    /// An entry of the file tree was selected.
    SelectFile(String),
    /// A folder of the file tree was opened or closed.
    ExpandFile(String, bool),
    /// A folder of a project's file tree was read.
    FolderRead(String, String, Result<Vec<FileEntry>, String>),
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
    files: FileTree,
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
            files,
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

    /// The container a tab of `kind` enters.
    #[must_use]
    pub fn plan(&self, kind: &TabKind) -> Option<ContainerPlan> {
        match kind {
            TabKind::Shell => Some(ContainerPlan::base(self.id.as_str(), &self.paths)),
            TabKind::Profile(name) => self
                .profiles
                .iter()
                .find(|profile| profile.name.as_str() == name)
                .map(|profile| ContainerPlan::profile(&self.id, &self.paths, profile)),
        }
    }

    /// What a tab of `kind` runs inside its container: a login shell, or the harness of the
    /// profile with the arguments that let it work without asking.
    #[must_use]
    pub fn program(&self, kind: &TabKind) -> Option<Vec<String>> {
        match kind {
            TabKind::Shell => Some(plan::SHELL.iter().map(|part| (*part).to_owned()).collect()),
            TabKind::Profile(name) => {
                let profile = self.profiles.iter().find(|profile| profile.name.as_str() == name)?;
                let harness = profile.harness.record();
                let mut command = vec![harness.command.to_owned()];
                command.extend(harness.auto_run.iter().map(|arg| (*arg).to_owned()));
                Some(command)
            }
        }
    }

    /// The label of the tab at `index`: the profile's name, or `Shell` numbered among the shell
    /// tabs so several of them can be told apart.
    fn tab_label(&self, index: usize) -> String {
        let Some(tab) = self.tabs.get(index) else { return String::new() };
        match tab.kind() {
            TabKind::Profile(name) => name.clone(),
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

/// The project screen.
#[derive(Debug)]
pub struct ProjectScreen {
    engine: Option<Engine>,
    user: HostUser,
    projects: Vec<OpenProject>,
    active: usize,
    panel: Panel,
    picker: bool,
    next_key: u64,
}

impl ProjectScreen {
    /// A screen showing `projects`, entering their containers through `engine`.
    ///
    /// Without an engine the screen still opens: the design asks that a missing engine leave the
    /// application usable, so every action that would touch a container is drawn faint and says
    /// why instead of failing when it is pressed.
    #[must_use]
    pub fn new(engine: Option<Engine>, user: HostUser, projects: Vec<OpenProject>) -> Self {
        Self { engine, user, projects, active: 0, panel: Panel::default(), picker: false, next_key: 0 }
    }

    /// The project the rail has open.
    #[must_use]
    pub fn project(&self) -> Option<&OpenProject> {
        self.projects.get(self.active)
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
        let program = project.program(tab.kind())?;
        let parts: Vec<&str> = program.iter().map(String::as_str).collect();
        Some(plan.enter(engine, &parts))
    }

    /// The project of `id`, when the screen has it open.
    fn project_mut(&mut self, id: &str) -> Option<&mut OpenProject> {
        self.projects.iter_mut().find(|project| project.id.as_str() == id)
    }

    /// The project holding the tab `key`, when one does.
    fn owner_mut(&mut self, key: TabKey) -> Option<&mut OpenProject> {
        self.projects.iter_mut().find(|project| project.tabs.iter().any(|tab| tab.key() == key))
    }
}

/// The work the screen needs the moment it is shown: reading the open project's folder and
/// asking the engine which of its containers are up.
///
/// The screen never touches the disk or the engine while drawing, so this is the caller's part
/// of the bargain: run it when the screen is entered, and again when it is returned to.
pub fn opened(screen: &mut ProjectScreen) -> Command<Msg> {
    Command::batch([load_root(screen), list_containers(screen)])
}

/// Applies a message to the screen.
pub fn update(screen: &mut ProjectScreen, message: Msg) -> Command<Msg> {
    match message {
        Msg::OpenProject(index) => {
            if index < screen.projects.len() {
                screen.active = index;
            }
            Command::batch([load_root(screen), list_containers(screen)])
        }
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
        Msg::ShowPicker(show) => {
            screen.picker = show;
            Command::none()
        }
        Msg::NewTab(what) => open_tab(screen, what),
        Msg::ManageProfiles => {
            screen.picker = false;
            Command::none()
        }
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
                project.files.select(&key);
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

/// Opens a tab and starts the work that brings its container up.
fn open_tab(screen: &mut ProjectScreen, what: NewTab) -> Command<Msg> {
    screen.picker = false;
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let user = screen.user;
    let key = TabKey(screen.next_key);
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    let kind = match what {
        NewTab::Shell => TabKind::Shell,
        NewTab::Profile(index) => match project.profiles.get(index) {
            Some(profile) => TabKind::Profile(profile.name.as_str().to_owned()),
            None => return Command::none(),
        },
    };
    let Some(plan) = project.plan(&kind) else { return Command::none() };
    let record = match &kind {
        TabKind::Profile(name) if !project.carries(name) => record_profile(project, name),
        _ => Command::none(),
    };
    screen.next_key += 1;
    project.tabs.push(Tab::new(key, kind));
    project.active_tab = project.tabs.len() - 1;
    Command::batch([
        Command::perform(move || Msg::Ready(key, 0, plan::ensure_running(&engine, &plan, user))),
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

/// Starts the tab `key` again, from its container upwards.
fn restart_tab(screen: &mut ProjectScreen, key: TabKey) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let user = screen.user;
    let Some(project) = screen.owner_mut(key) else { return Command::none() };
    let Some((_, tab)) = project.find(key) else { return Command::none() };
    tab.restarting();
    let run = tab.run();
    let kind = tab.kind().clone();
    let Some(plan) = project.plan(&kind) else { return Command::none() };
    Command::perform(move || Msg::Ready(key, run, plan::ensure_running(&engine, &plan, user)))
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

/// Reads the root of the open project's file tree, when it has not been read yet.
fn load_root(screen: &mut ProjectScreen) -> Command<Msg> {
    let Some(project) = screen.projects.get_mut(screen.active) else { return Command::none() };
    if project.files.children(files::ROOT).is_some() || !project.files.start_reading(files::ROOT) {
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
    let name = name.to_owned();
    Command::perform(move || {
        let result = if again { plan::restart(&engine, &name) } else { plan::stop(&engine, &name) };
        Msg::ContainerActed(result)
    })
}

/// The name the terminal in the middle is focused by.
const TERMINAL_ID: &str = "project-terminal";

/// The name the control that opens a tab is focused by.
const NEW_TAB_ID: &str = "project-new-tab";

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

/// The rail of projects down the left.
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
            .on_select(Msg::OpenProject),
    )
    .fill()
    .id("project-rail");
}

/// The tab strip and the control that opens a new tab.
fn header(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let Some(project) = screen.project() else { return };
    let labels: Vec<String> = (0..project.tabs.len()).map(|index| project.tab_label(index)).collect();
    ui.row(|ui| {
        ui.add(
            Tabs::new(labels)
                .active(project.active_tab)
                .tab_width(TabWidth::Fit)
                .closable(Msg::CloseTab)
                .reorderable(move |from, to| Msg::MoveTab { from, to })
                .on_select(Msg::OpenTab),
        )
        .fill_width()
        .id("project-tabs");
        new_tab(screen, project, ui);
    })
    .fill_width();
}

/// The `+` control and the chooser of what a new tab opens.
fn new_tab(screen: &ProjectScreen, project: &OpenProject, ui: &mut View<'_, Msg>) {
    let ready = screen.engine.is_some();
    Popover::new(screen.picker && ready)
        .on_dismiss(Msg::ShowPicker(false))
        .anchor(|ui| {
            ui.add(
                Button::new(t!("project.new-tab"))
                    .icon("add")
                    .disabled(!ready)
                    .on_press(Msg::ShowPicker(!screen.picker)),
            )
            .id(NEW_TAB_ID);
        })
        .content(|ui| {
            let mut items = vec![ListItem::new(t!("project.tab.shell")).icon("prompt", None)];
            items.extend(project.profiles.iter().map(|profile| {
                let harness = profile.harness.record().display_name;
                // A profile the project does not carry yet is offered all the same; the row says
                // that choosing it adds it, so the file changing is no surprise.
                let detail = if project.carries(profile.name.as_str()) {
                    harness.to_owned()
                } else {
                    t!("project.tab.adds", harness = harness)
                };
                ListItem::new(profile.name.as_str().to_owned()).icon("bullet", None).detail(detail)
            }));
            let none = project.profiles.is_empty();
            // With no profile at all, the list says where one is made instead of ending at the
            // shell, and the row takes the person there.
            if none {
                items.push(
                    ListItem::new(t!("project.make-profile")).icon("add", None).detail(t!("project.no-profiles")),
                );
            }
            ui.add(List::new(items).on_activate(move |index| match index {
                0 => Msg::NewTab(NewTab::Shell),
                _ if none => Msg::ManageProfiles,
                _ => Msg::NewTab(NewTab::Profile(index - 1)),
            }))
            .id("project-new-tab-list")
            .width(Length::Cells(44));
        })
        .show(ui);
}

/// What the middle shows: the open tab, or why there is nothing to show.
fn center(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let Some(project) = screen.project() else {
        ui.add(EmptyState::new(t!("project.none.title")).icon("inbox").message(t!("project.none.message"))).fill();
        return;
    };
    let Some(tab) = project.active_tab() else {
        let mut empty = EmptyState::new(t!("project.empty.title")).icon("inbox");
        empty = if screen.engine.is_some() {
            empty
                .message(t!("project.empty.message"))
                .action(Button::new(t!("project.new-tab")).variant("primary").on_press(Msg::ShowPicker(true)))
        } else {
            empty.message(t!("project.no-engine"))
        };
        ui.add(empty).fill().id("project-empty");
        return;
    };
    match tab.state() {
        TabState::Starting => {
            ui.column(|ui| {
                ui.add(Spinner::new().label(t!("project.starting")));
            })
            .fill()
            .align(Align::Center)
            .justify(Align::Center)
            .id("project-starting");
        }
        TabState::Running => match tab.session() {
            Some(session) => {
                ui.add(Terminal::new(session)).fill().id(TERMINAL_ID);
            }
            None => {
                ui.add(Spinner::new().label(t!("project.starting"))).fill();
            }
        },
        TabState::Ended { code } => stopped(ui, tab, &ended_text(*code), None),
        TabState::Stopped => stopped(ui, tab, &t!("project.stopped"), None),
        TabState::Failed(failure) => {
            // A failure of the machine itself has no engine command to show above its words.
            let detail = if failure.command.is_empty() {
                failure.output.clone()
            } else {
                format!("{}\n{}", failure.command, failure.output)
            };
            stopped(ui, tab, &t!("project.failed"), Some(&detail));
        }
    }
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
fn stopped(ui: &mut View<'_, Msg>, tab: &Tab, message: &str, detail: Option<&str>) {
    let key = tab.key();
    ui.column(|ui| {
        if let Some(session) = tab.session() {
            ui.add(Terminal::new(session)).fill().id(TERMINAL_ID);
        }
        ui.column(|ui| {
            ui.add(Text::new(message.to_owned()).role("secondary"));
            if let Some(detail) = detail {
                ui.add(Text::new(detail.to_owned()).role("faint")).selectable(true);
            }
            ui.add(Button::new(t!("project.restart")).variant("primary").on_press(Msg::Restart(key)))
                .id("project-restart");
        })
        .gap(1)
        .padding(Padding::symmetric(1, 2))
        .fill_width();
    })
    .fill()
    .id("project-stopped");
}

/// The control that takes the keyboard when the screen opens: the terminal of the open tab, or
/// the control that opens the first tab while there is none.
#[must_use]
pub fn entry(screen: &ProjectScreen) -> &'static str {
    match screen.project().and_then(OpenProject::active_tab) {
        Some(_) => TERMINAL_ID,
        None => NEW_TAB_ID,
    }
}

/// The keys of the project screen that are not in the keymap, for the key list.
///
/// A focused terminal hands every key to the program in it but two: `shift+tab`, which leaves
/// it, and `ctrl+q`. So the list says how to get out of the terminal, after which the rest of
/// these keys and the key list itself work again.
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
