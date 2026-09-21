//! QCode runs coding harnesses inside containers, never on the machine itself.
//!
//! The crate carries the application: its state, the messages that change it and the screens
//! that draw it. The `qcode` and `quvyta-code` commands are thin wrappers around [`QCode`].
//!
//! Every screen lives in its own module under [`ui`] and knows nothing about this one: each has
//! a state, an `update` and a `view`, and speaks a message type of its own that this module maps
//! into [`Msg`]. This module is where they become one application — which screen is open, what
//! each of them is given, and what happens when one of them asks for something only the
//! application can do.

pub mod backup;
pub mod base;
pub mod bridge;
pub mod desktop;
pub mod engine;
pub mod profile;
pub mod provider;
pub mod service;
pub mod store;
pub mod ui;

#[cfg(test)]
mod reopen_tests;

use std::io::{self, Write as _};
use std::path::PathBuf;

use qframe::keymap::{KeyChord, Scope};
use qframe::prelude::*;
use qframe::runtime::{Runtime, Task};
use qframe::widgets::{HelpLayer, IconButton, PageTransition, Toast};

use engine::run::capture;
use engine::{Engine, EngineKind, HostUser, detect};
use profile::identity::{RefreshError, Refreshed};
use profile::{Profile, SafeName};
use service::units::ServiceHost;
use store::{
    Config, HostDirs, Loaded, Platform, Registry, Session, SetupStep, Store, WorkspaceFile, WorkspaceId, WorkspacePaths,
};
use ui::home::{Entry, Home};
use ui::profiles::Profiles;
use ui::providers::Providers as ProvidersScreen;
use ui::settings::engine::{EngineState, Health, Trouble};
use ui::settings::{Request, ServiceRow, Settings as SettingsScreen};
use ui::setup::Setup;
use ui::setup::gates::{EngineCheck, Gates};
use ui::setup::install::{InstallHost, Installer};
use ui::workspace::{Farewell, OpenWorkspace, WorkspaceScreen};
use ui::workspaces::Workspaces;

/// What `qcode --version` prints: the package's name and version, the way cargo writes them.
#[must_use]
pub fn version_line() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

/// Starts the application: preferences come from the platform's config folder, the text is
/// compiled in, and the runtime drives the screens until the person leaves.
///
/// The gates of the design are asked here, before the first frame, because they decide what the
/// first frame is: a setup wizard or the home screen. They read the disk and start the container
/// engine, which is why they are asked once, on the way in, and never while drawing.
///
/// # Errors
///
/// Returns the terminal's error when the screen cannot be taken over or restored.
pub fn run() -> io::Result<()> {
    // Asked for in bug reports, so it answers before anything is read or held.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "--version" || arg == "-V") {
        return writeln!(io::stdout(), "{}", version_line());
    }
    let Loaded { value: config, diagnostics: left_behind } = Config::load();
    let places = service::Places::detect();
    let text = || std::sync::Arc::new(service::translator(config.settings().language().as_deref()));
    // The setting as it is on disk when the containers are about to be stopped, which may be long
    // after this QCode started and after another one changed it.
    let on_close = || Config::load().value.on_close();
    let engine_for = |kind: EngineKind| detect(kind).ok();
    // The background service starts the same binary with one word, and it has no screen: it
    // waits for the last QCode to close, answers in the language the person chose and exits.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == service::units::REAPER_ARG) {
        return qframe::i18n::scope(text(), || match &places {
            Some(places) => {
                let mut engines = service::Engines { engine_for: &engine_for, run: &mut capture };
                service::reaper(&mut io::stdout(), places, &on_close, &mut engines)
            }
            None => writeln!(io::stdout(), "{}", t!("reaper.nowhere")),
        });
    }
    // Held until this QCode is done, so the service and every other QCode know it is open.
    // Windows has no lock to hold, and there nothing is stopped on close.
    let open = places.and_then(|places| service::Open::hold(places).ok());
    let settings = config.settings().clone();
    let farewell = Farewell::default();
    let app = QCode::start(config.clone(), HostDirs::detect(), InstallHost::detect())
        .with_left_behind(left_behind)
        .with_farewell(farewell.clone());
    let (keys, bindings) = keymap();
    let mut runtime = Runtime::new(app).settings(&settings).keymap_source(keys, bindings);
    for (file, text) in locales() {
        runtime = runtime.locale_source(file, text);
    }
    let ran = runtime.run();
    // The workspaces are backed up before their containers may be stopped, and a backup that
    // failed is the one thing said about it: one that worked needs no word on the way out.
    let leaving = farewell.take();
    let say = |lines: Vec<String>| lines.iter().try_for_each(|line| writeln!(io::stdout(), "{line}"));
    let backed = leaving.as_ref().map_or(Ok(()), |leaving| qframe::i18n::scope(text(), || say(leaving.back_up())));
    // The screen is given back by now, so what the last QCode stopped is said on the terminal
    // the person started it from.
    let closed = open.map_or(Ok(()), |open| {
        let mut engines = service::Engines { engine_for: &engine_for, run: &mut capture };
        let reaped = open.close(&on_close, &mut engines)?;
        qframe::i18n::scope(text(), || match reaped {
            Some(reaped @ (service::stop::Reaped::Done(_) | service::stop::Reaped::Broken(_))) => {
                service::report(&reaped).iter().try_for_each(|line| writeln!(io::stdout(), "{line}"))
            }
            Some(service::stop::Reaped::Kept) | None => Ok(()),
        })
    });
    // A harness that keeps its conversations in a database is backed up only once its container
    // has stopped, which is now when this QCode stopped it.
    let stopped =
        leaving.as_ref().map_or(Ok(()), |leaving| qframe::i18n::scope(text(), || say(leaving.back_up_stopped())));
    ran.and(backed).and(closed).and(stopped)
}

/// The application's own locale files, compiled in so an installed program carries its text with
/// it instead of looking for files beside the binary.
#[must_use]
pub fn locales() -> Vec<(String, String)> {
    [
        ("en.toml", include_str!("../assets/locales/en.toml")),
        ("tr.toml", include_str!("../assets/locales/tr.toml")),
        ("de.toml", include_str!("../assets/locales/de.toml")),
        ("es.toml", include_str!("../assets/locales/es.toml")),
        ("fr.toml", include_str!("../assets/locales/fr.toml")),
        ("pt-BR.toml", include_str!("../assets/locales/pt-BR.toml")),
        ("ru.toml", include_str!("../assets/locales/ru.toml")),
        ("zh-Hans.toml", include_str!("../assets/locales/zh-Hans.toml")),
        ("ja.toml", include_str!("../assets/locales/ja.toml")),
    ]
    .into_iter()
    .map(|(file, text)| (file.to_owned(), text.to_owned()))
    .collect()
}

/// The application's own key bindings, compiled in for the same reason the text is: an
/// installed program carries its keys with it rather than looking for a file beside the binary.
#[must_use]
pub fn keymap() -> (String, String) {
    ("keymap.toml".to_owned(), include_str!("../assets/keymap.toml").to_owned())
}

/// The screens the application moves between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The setup wizard, which is the only screen until it has been through.
    Setup,
    /// The logo and the menu.
    Home,
    /// The workspaces of the store.
    Workspaces,
    /// The profiles of the store.
    Profiles,
    /// The model services of the person's own.
    Providers,
    /// One workspace: its tabs, its terminals and its panel.
    Workspace,
    /// What the person can change about QCode.
    Settings,
}

impl Page {
    /// The name the page keeps its focus and its scrolling under, and the key the transition
    /// between pages watches.
    fn key(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Home => "home",
            Self::Workspaces => "workspaces",
            Self::Profiles => "profiles",
            Self::Providers => "providers",
            Self::Workspace => "workspace",
            Self::Settings => "settings",
        }
    }
}

/// Everything one workspace needs before the workspace screen can show it.
///
/// Reading it touches the disk, so it is read on a task thread and travels back in a message.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceContents {
    /// The workspace's own file.
    pub file: WorkspaceFile,
    /// Where its folders are.
    pub paths: WorkspacePaths,
    /// Every profile of the store: a new tab offers them all, and one the workspace does not
    /// carry yet is added to it when it is opened.
    pub profiles: Vec<Profile>,
}

/// What the workspaces that were read are for.
#[derive(Debug, Clone)]
pub enum Opening {
    /// One workspace, picked from the list or opened last: it joins the workspace screen that is
    /// open, or opens one of its own.
    One(WorkspaceId),
    /// The workspaces and tabs of the last session, brought back as they were.
    Session(Session),
}

/// Everything that can happen in QCode.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Something happened on the home screen.
    Home(ui::home::Msg),
    /// A row of the home menu was opened.
    Open(Entry),
    /// Something happened in the setup wizard.
    Setup(ui::setup::Msg),
    /// Something happened on the workspaces screen.
    Workspaces(ui::workspaces::Msg),
    /// Something happened on the profiles screen.
    Profiles(ui::profiles::Msg),
    /// Something happened on the providers screen.
    Providers(Box<ui::providers::Msg>),
    /// Something happened on the workspace screen.
    Workspace(ui::workspace::Msg),
    /// Something happened on the settings screen.
    Settings(ui::settings::Msg),
    /// Leave the screen that is open and go back to the one before it.
    Back,
    /// Leave QCode, however that was asked for.
    Quit,
    /// The list of every key was opened or closed.
    Help(bool),
    /// The container engine was looked for again, and this is what was found.
    Looked(EngineKind, Result<Box<Engine>, Trouble>),
    /// The workspaces to open were read from the store.
    Read(Vec<WorkspaceContents>, Opening),
    /// The session file was written, or it was not.
    SessionSaved(Result<(), String>),
    /// A profile's login was deleted, or it was not.
    SignedOut(Result<(), String>),
    /// A profile's stored login was written into the workspaces that use it, or it was not.
    Refreshed(SafeName, Result<Refreshed, RefreshError>),
}

/// The application.
pub struct QCode {
    router: Router<Page>,
    config: Config,
    dirs: HostDirs,
    host: InstallHost,
    /// The language the machine itself is set to, kept so a single step the repair strip opens
    /// offers the same list the whole wizard would have.
    system: Option<String>,
    user: HostUser,
    engine: EngineState,
    found: Option<Engine>,
    setup: Option<Setup>,
    home: Home,
    workspaces: Option<Workspaces>,
    profiles: Option<Profiles>,
    providers: Option<ProvidersScreen>,
    workspace: Option<WorkspaceScreen>,
    settings: SettingsScreen,
    help: bool,
    /// Where the open workspaces are remembered between runs, or `None` where they are not.
    session_file: Option<PathBuf>,
    /// The session as the file holds it, or as it is being written: what "Continue" goes back to
    /// when no workspace screen is open, and what a changed screen is compared with.
    saved: Option<Session>,
    /// What was wrong with the session file when it was read, said when it is brought back.
    saved_problems: Vec<qframe::diagnostics::Diagnostic>,
    /// Whether the session file is being written, so writes never overtake one another.
    saving: bool,
    /// Whether the last write failed, so a disk that refuses is said once rather than at every
    /// change.
    save_failed: bool,
    /// Whether the workspace screen's file tree follows the disk; see [`WorkspaceScreen::watching`].
    live_files: bool,
    /// Where the containers QCode starts are noted, so they can be stopped once no QCode is
    /// open, or `None` where nothing is noted.
    registry: Option<PathBuf>,
    /// Where the background service's files go on this machine, or `None` where it gets none.
    service: Option<ServiceHost>,
    /// Where the workspaces open as QCode quits are left, to be backed up once the screen is
    /// given back.
    farewell: Farewell,
}

impl QCode {
    /// The application as this machine leaves it: the gates of the design are asked, and
    /// [`entry`](Self::entry) turns their answer into the step the wizard opens on, if any.
    #[must_use]
    pub fn start(config: Config, dirs: HostDirs, host: InstallHost) -> Self {
        let gates = Gates::probe(&config);
        // The gates only answer whether the engine works; the engine itself is what every screen
        // that reaches into a container needs, so it is looked up once more when it does work.
        let kind = config.engine_kind().and_then(EngineKind::from_name);
        let found = match (kind, gates.engine == EngineCheck::Working) {
            (Some(kind), true) => detect(kind).ok(),
            _ => None,
        };
        let entry = Self::entry(&config, &gates);
        Self::new(config, dirs, host, &gates, found, ui::setup::languages::system_here(), entry)
            .with_session(Session::file())
            .with_registry(Registry::file())
            .with_service(ServiceHost::detect(), Platform::host())
            .following_files()
    }

    /// The wizard step a machine whose settings are `config` opens on, or `None` when there is
    /// nothing left to ask.
    ///
    /// A setup that has been through is never asked again — that is the design's rule, and the
    /// one thing that overrides it is a settings file that has been through the wizard and still
    /// names no store, which leaves nothing to open.
    #[must_use]
    pub fn entry(config: &Config, gates: &Gates) -> Option<SetupStep> {
        let settled = config.setup_completed() && config.folder_path().is_some();
        if settled { None } else { gates.entry() }
    }

    /// The application with every answer already in hand, which is how a test builds one.
    ///
    /// `entry` is the wizard's step, or `None` for an application that opens on its home screen.
    /// `system` is the language the machine itself is set to, as
    /// [`languages::system`](ui::setup::languages::system) reads it. `None` describes a machine
    /// that names none, which is every test that has not said otherwise.
    #[must_use]
    pub fn new(
        config: Config,
        dirs: HostDirs,
        host: InstallHost,
        gates: &Gates,
        found: Option<Engine>,
        system: Option<String>,
        entry: Option<SetupStep>,
    ) -> Self {
        // Before anything reads the store: one written by an older QCode carries the names from
        // when a workspace was called a project, and every path below it is built from the names
        // of today. Opening the application is what gives them today's names, so it happens here
        // rather than on a screen, and what could not be renamed is said on the settings screen.
        let renamed = config.folder_path().map(|path| Store::new(path).adopt()).unwrap_or_default();
        let kind = config.engine_kind().and_then(EngineKind::from_name).unwrap_or(EngineKind::Podman);
        let engine = EngineState::new(kind, health(&gates.engine));
        let settings = SettingsScreen::new(&config, engine.clone()).with_left_behind(renamed);
        // Which step the wizard opens on is the gates' answer, never this one: `entry` only
        // says whether there is anything left to ask.
        let setup = entry
            .map(|_| Setup::new(config.clone(), &dirs, gates, host.clone(), Installer::shell(), system.as_deref()));
        Self {
            router: Router::new(if setup.is_some() { Page::Setup } else { Page::Home }),
            config,
            dirs,
            host,
            system,
            // The container's user mapping is this machine's, and a machine that cannot say who
            // it is runs the image's own user rather than stopping the application.
            user: HostUser::current().unwrap_or(HostUser::ImageDefault),
            engine,
            found,
            setup,
            home: Home::new(Vec::new()),
            workspaces: None,
            profiles: None,
            providers: None,
            workspace: None,
            settings,
            help: false,
            session_file: None,
            saved: None,
            saved_problems: Vec::new(),
            saving: false,
            save_failed: false,
            live_files: false,
            registry: None,
            service: None,
            farewell: Farewell::default(),
        }
        .refreshed_home()
    }

    /// The same application, reporting on its settings screen the settings files of an earlier
    /// version that stayed where they were when [`Config::load`] moved the settings.
    #[must_use]
    pub fn with_left_behind(mut self, files: Vec<qframe::diagnostics::Diagnostic>) -> Self {
        self.settings = self.settings.with_left_behind(files);
        self
    }

    /// The same application, remembering its open workspaces in the session file at `path`: the
    /// file is read now, on the way in like the gates, and written whenever what is open changes.
    /// `None` keeps nothing between runs.
    #[must_use]
    pub fn with_session(mut self, path: Option<PathBuf>) -> Self {
        let Loaded { value, diagnostics } = match &path {
            Some(path) => Session::load(path),
            None => Loaded { value: None, diagnostics: Vec::new() },
        };
        self.session_file = path;
        self.saved = value;
        self.saved_problems = diagnostics;
        self.refreshed_home()
    }

    /// The same application with the workspace screen's file tree following the disk and its
    /// workspaces listening for their tabs' agents. A test's application leaves both off, because
    /// its harness would wait for the disk and the socket forever.
    fn following_files(mut self) -> Self {
        self.live_files = true;
        self
    }

    /// The same application whose providers screen reads the file at `path` and asks its
    /// questions through `web`.
    ///
    /// Both are parameters for the same reason [`Installer`] is one: a test drives the whole
    /// screen — adding a provider, trying the connection, measuring a window — with a file of
    /// its own and a web that answers from a string, and nothing it does can reach the network.
    #[must_use]
    pub fn with_providers(mut self, path: Option<PathBuf>, web: provider::Web) -> Self {
        self.providers = Some(ProvidersScreen::new(path, web));
        self
    }

    /// The same application, noting every container it starts in the list at `path`, so that
    /// they can be stopped once no QCode is open. `None` notes nothing.
    #[must_use]
    pub fn with_registry(mut self, path: Option<PathBuf>) -> Self {
        self.registry = path;
        self
    }

    /// The same application, leaving in `farewell` the workspaces that are open as it quits, so
    /// that they can be backed up once the runtime is done with it.
    #[must_use]
    pub fn with_farewell(mut self, farewell: Farewell) -> Self {
        self.farewell = farewell;
        self
    }

    /// The same application, offering the background service whose files go where `host` says
    /// on a machine of `platform`. Whether it is installed is read from the disk now, on the way
    /// in; `None` offers no service.
    #[must_use]
    pub fn with_service(mut self, host: Option<ServiceHost>, platform: Platform) -> Self {
        let installed = host.as_ref().map(ServiceHost::is_installed);
        let row = ServiceRow::of(platform, installed);
        self.settings = self.settings.with_service(row);
        self.service = host;
        self
    }

    /// The same application on the workspace screen `screen`, the way a restored session opens
    /// it; for a screen built by hand, such as a demonstration's. The screen's first work, reading
    /// the open workspace's folder and asking after its containers, starts when
    /// [`ui::workspace::Msg::OpenWorkspace`] reaches it.
    #[must_use]
    pub fn with_workspace(mut self, screen: WorkspaceScreen) -> Self {
        self.workspace = Some(screen);
        self.enter_workspace();
        self.refreshed_home()
    }

    fn refreshed_home(mut self) -> Self {
        self.refresh_home();
        self
    }

    /// Tells the home screen what "Continue" goes back to: the workspace screen that is open, or
    /// else the last session, or else — for a session never written — the workspace opened last.
    fn refresh_home(&mut self) {
        let live = self.workspace.as_ref().map(WorkspaceScreen::workspaces).unwrap_or_default();
        let open: Vec<String> = if !live.is_empty() {
            live.iter().map(|workspace| workspace.name().to_owned()).collect()
        } else if let Some(saved) = &self.saved {
            saved.workspaces.iter().map(|workspace| workspace.id.as_str().to_owned()).collect()
        } else {
            self.config.recent_workspaces().first().map(|id| id.as_str().to_owned()).into_iter().collect()
        };
        self.home.set_open(open);
    }

    /// The workspace screen, once a workspace has been opened.
    #[must_use]
    pub fn workspace(&self) -> Option<&WorkspaceScreen> {
        self.workspace.as_ref()
    }

    /// Which screen is open.
    #[must_use]
    pub fn page(&self) -> Page {
        *self.router.current()
    }

    /// The store, once the setup has placed one.
    fn store(&self) -> Option<Store> {
        self.config.folder_path().map(Store::new)
    }

    /// The control the open screen hands the keyboard to, so the arrow keys work from the first
    /// key the person presses rather than after a Tab they were never told about.
    ///
    /// The wizard is not here: it focuses the control of the step it opens on, which only it
    /// knows. A screen whose list is still being read is not here either; it takes the keyboard
    /// when its answer arrives, because a control that is not on screen cannot be focused.
    fn take_focus(&self) -> Command<Msg> {
        let control = match self.page() {
            Page::Setup => None,
            Page::Home => Some(ui::home::entry()),
            Page::Workspaces => None,
            Page::Profiles => self.profiles.as_ref().and_then(ui::profiles::entry),
            Page::Providers => self.providers.as_ref().and_then(ui::providers::entry),
            Page::Workspace => self.workspace.as_ref().map(ui::workspace::entry),
            Page::Settings => Some(ui::settings::entry(&self.settings)),
        };
        control.map_or_else(Command::none, Command::focus)
    }

    /// Opens a row of the home menu.
    fn open(&mut self, entry: Entry) -> Command<Msg> {
        match entry {
            Entry::Quit => self.quit(),
            Entry::Workspaces => self.show_workspaces(false),
            Entry::NewWorkspace => self.show_workspaces(true),
            Entry::Profiles => self.show_profiles(),
            Entry::Providers => self.show_providers(),
            Entry::Settings => self.show_settings(),
            Entry::Continue => self.resume(),
        }
    }

    /// Goes back to where the person left off: the workspace screen as it is when one is open,
    /// else the last session as the file keeps it, else the workspace opened last on its own.
    fn resume(&mut self) -> Command<Msg> {
        if let Some(screen) = self.workspace.as_mut().filter(|screen| !screen.workspaces().is_empty()) {
            let entered = ui::workspace::opened(screen).map(Msg::Workspace);
            self.router.push(Page::Workspace);
            return Command::batch([entered, self.take_focus(), self.reread_profiles()]);
        }
        if let Some(saved) = self.saved.clone().filter(|saved| !saved.is_empty()) {
            let ids = saved.workspaces.iter().map(|workspace| workspace.id.clone()).collect();
            return self.read_workspaces(ids, Opening::Session(saved));
        }
        match self.config.recent_workspaces().first().cloned() {
            Some(id) => self.read_workspaces(vec![id.clone()], Opening::One(id)),
            // The row only stands there while there is something to go back to.
            None => self.show_workspaces(false),
        }
    }

    /// Opens the workspaces screen, with the new-workspace dialog over it when that is what was
    /// asked for.
    fn show_workspaces(&mut self, new: bool) -> Command<Msg> {
        let Some(store) = self.store() else { return self.repair(SetupStep::Location) };
        if self.workspaces.is_none() {
            let recent = self.config.recent_workspaces().iter().map(|id| id.as_str().to_owned()).collect();
            self.workspaces = Some(Workspaces::new(store, self.found.clone(), recent));
        }
        let Some(screen) = self.workspaces.as_mut() else { return Command::none() };
        self.router.push(Page::Workspaces);
        let (refresh, _) = ui::workspaces::update(screen, ui::workspaces::Msg::Refresh);
        if !new {
            return refresh.map(Msg::Workspaces);
        }
        let (start, _) = ui::workspaces::update(screen, ui::workspaces::Msg::Start);
        Command::batch([refresh, start]).map(Msg::Workspaces)
    }

    /// Opens the profiles screen.
    fn show_profiles(&mut self) -> Command<Msg> {
        let Some(store) = self.store() else { return self.repair(SetupStep::Location) };
        if self.profiles.is_none() {
            // The wizard offers what the Providers page writes, so both read the same file.
            let providers = match &self.providers {
                Some(screen) => screen.path().map(std::path::Path::to_path_buf),
                None => crate::provider::Providers::file(),
            };
            self.profiles = Some(
                Profiles::new(Some(store.root().to_path_buf()), self.found.clone()).with_providers_file(providers),
            );
        }
        let Some(screen) = self.profiles.as_mut() else { return Command::none() };
        self.router.push(Page::Profiles);
        ui::profiles::update(screen, ui::profiles::Msg::Reload).map(Msg::Profiles)
    }

    /// Opens the providers screen, which reads the providers file and nothing else: not one
    /// request leaves the machine because a page was opened.
    fn show_providers(&mut self) -> Command<Msg> {
        if self.providers.is_none() {
            self.providers = Some(ProvidersScreen::new(crate::provider::Providers::file(), provider::Web::network()));
        }
        let Some(screen) = self.providers.as_mut() else { return Command::none() };
        self.router.push(Page::Providers);
        ui::providers::update(screen, ui::providers::Msg::Reload).map(|message| Msg::Providers(Box::new(message)))
    }

    /// Opens the settings screen.
    fn show_settings(&mut self) -> Command<Msg> {
        self.router.push(Page::Settings);
        let profiles = self.store().map(|store| store.profiles().value).unwrap_or_default();
        let found = self.found.clone();
        let read =
            Command::perform(move || Msg::Settings(ui::settings::Msg::Profiles(identities(found.as_ref(), &profiles))));
        Command::batch([read, self.take_focus()])
    }

    /// Reads the workspaces `ids` of the store, in that order, for `opening`. A workspace that is
    /// not there any more is simply not in the answer.
    fn read_workspaces(&mut self, ids: Vec<WorkspaceId>, opening: Opening) -> Command<Msg> {
        let Some(store) = self.store() else { return self.repair(SetupStep::Location) };
        Command::perform(move || {
            let known = store.profiles().value;
            let mut files: Vec<WorkspaceFile> =
                store.workspaces().value.into_iter().filter_map(|entry| entry.file).collect();
            let contents = ids
                .iter()
                .filter_map(|id| {
                    let file = files.remove(files.iter().position(|file| file.id == *id)?);
                    let paths = store.workspace_paths(&file.id);
                    Some(WorkspaceContents { file, paths, profiles: known.clone() })
                })
                .collect();
            Msg::Read(contents, opening)
        })
    }

    /// Opens what was read for `opening`.
    fn read(&mut self, contents: Vec<WorkspaceContents>, opening: Opening) -> Command<Msg> {
        match opening {
            Opening::One(id) => self.show_workspace(contents, &id),
            Opening::Session(session) => self.restore(contents, &session),
        }
    }

    /// Opens the workspace `id`: into the workspace screen that is open, beside the workspaces it
    /// has and without closing anything, or on a screen of its own when none is open.
    fn show_workspace(&mut self, contents: Vec<WorkspaceContents>, id: &WorkspaceId) -> Command<Msg> {
        let Some(one) = contents.into_iter().find(|workspace| workspace.file.id == *id) else {
            // The workspace is gone from the store between the list and the opening of it; the
            // list is the honest answer, and the recent entry that led here goes.
            self.config.forget_workspace(id);
            self.refresh_home();
            return Command::batch([self.save(), self.show_workspaces(false)]);
        };
        let workspace = OpenWorkspace::new(&one.file, one.paths, one.profiles);
        let entered = match self.workspace.as_mut() {
            Some(screen) => ui::workspace::add(screen, workspace),
            None => {
                let mut screen = WorkspaceScreen::new(self.found.clone(), self.user, vec![workspace])
                    .with_registry(self.registry.clone())
                    .backing_up(self.config.backup_every())
                    .watching(self.live_files)
                    .bridging(self.live_files);
                screen.set_editor(self.config.editor());
                screen.set_sound(self.config.sound());
                let entered = ui::workspace::opened(&mut screen);
                self.workspace = Some(screen);
                entered
            }
        };
        self.enter_workspace();
        self.config.remember_workspace(id);
        Command::batch([entered.map(Msg::Workspace), self.take_focus(), self.save(), self.keep_session()])
    }

    /// Brings back the workspaces and tabs of `session` that the store still has, and says
    /// which it no longer has.
    fn restore(&mut self, contents: Vec<WorkspaceContents>, session: &Session) -> Command<Msg> {
        let gone: Vec<&str> = session
            .workspaces
            .iter()
            .filter(|record| !contents.iter().any(|one| one.file.id == record.id))
            .map(|record| record.id.as_str())
            .collect();
        let mut told = Vec::new();
        if !gone.is_empty() {
            let toast = Toast::warning(t!("app.session-gone", n = gone.len())).body(gone.join(", "));
            told.push(Command::toast(toast));
        }
        if let Some(problem) = self.saved_problems.first() {
            told.push(Command::toast(Toast::warning(t!("app.session-broken")).body(problem.to_string())));
            self.saved_problems.clear();
        }
        if contents.is_empty() {
            told.push(self.show_workspaces(false));
            return Command::batch(told);
        }
        let ids: Vec<WorkspaceId> = contents.iter().map(|one| one.file.id.clone()).collect();
        let workspaces: Vec<OpenWorkspace> =
            contents.into_iter().map(|one| OpenWorkspace::new(&one.file, one.paths, one.profiles)).collect();
        let mut screen = WorkspaceScreen::new(self.found.clone(), self.user, workspaces)
            .with_registry(self.registry.clone())
            .backing_up(self.config.backup_every())
            .watching(self.live_files)
            .bridging(self.live_files);
        screen.set_editor(self.config.editor());
        screen.set_sound(self.config.sound());
        for (index, id) in ids.iter().enumerate() {
            if let Some(record) = session.workspaces.iter().find(|record| record.id == *id) {
                screen.restore_tabs(index, record);
            }
        }
        let active = session.active.as_ref().and_then(|active| ids.iter().position(|id| id == active));
        screen.set_active(active.unwrap_or(0));
        told.push(ui::workspace::opened(&mut screen).map(Msg::Workspace));
        self.workspace = Some(screen);
        self.enter_workspace();
        told.extend([self.take_focus(), self.keep_session()]);
        Command::batch(told)
    }

    /// Shows the workspace screen. Coming from the list the workspace screen itself opened, that is
    /// a step back to it rather than a second workspace screen on top of the first.
    fn enter_workspace(&mut self) {
        let history = self.router.history();
        let under = history.len().checked_sub(2).and_then(|place| history.get(place));
        if self.page() != Page::Workspace && under == Some(&Page::Workspace) {
            self.router.back();
        } else {
            self.router.push(Page::Workspace);
        }
    }

    /// Writes the session file when what is open no longer matches what it holds, off the render
    /// path, and never while an earlier write is still going: that one's answer asks again.
    fn keep_session(&mut self) -> Command<Msg> {
        let Some(screen) = &self.workspace else { return Command::none() };
        let snapshot = screen.session();
        self.refresh_home();
        if self.saving || self.saved.as_ref() == Some(&snapshot) {
            return Command::none();
        }
        self.saved = Some(snapshot.clone());
        let Some(path) = self.session_file.clone() else { return Command::none() };
        self.saving = true;
        Command::perform(move || Msg::SessionSaved(snapshot.save(&path).map_err(|error| error.to_string())))
    }

    /// Takes in how a write of the session file went, and writes again when what is open changed
    /// while it was going.
    fn session_saved(&mut self, result: Result<(), String>) -> Command<Msg> {
        self.saving = false;
        let told = match result {
            Err(reason) if !self.save_failed => {
                self.save_failed = true;
                Command::toast(Toast::warning(t!("app.session-unsaved")).body(reason))
            }
            Err(_) => Command::none(),
            Ok(()) => {
                self.save_failed = false;
                Command::none()
            }
        };
        Command::batch([told, self.keep_session()])
    }

    /// Runs one step of the setup wizard on its own, which is what the repair strip and the
    /// settings screen ask for.
    fn repair(&mut self, step: SetupStep) -> Command<Msg> {
        let gates = Gates { language: true, engine: self.check(), location: Default::default() };
        let mut setup = Setup::for_step(
            self.config.clone(),
            &self.dirs,
            &gates,
            self.host.clone(),
            Installer::shell(),
            self.system.as_deref(),
            step,
        );
        let asked = ui::setup::opened(&mut setup).map(Msg::Setup);
        self.setup = Some(setup);
        self.router.push(Page::Setup);
        asked
    }

    /// What the last look at the engine says, in the shape the wizard's gates are written in.
    fn check(&self) -> EngineCheck {
        match self.engine.health() {
            Health::Working => EngineCheck::Working,
            Health::Checking => EngineCheck::Running,
            Health::Missing(_) => EngineCheck::Unknown,
        }
    }

    /// Applies a message of the setup wizard, and leaves the wizard when it is through.
    fn wizard(&mut self, message: ui::setup::Msg) -> Command<Msg> {
        let Some(setup) = self.setup.as_mut() else { return Command::none() };
        let command = ui::setup::update(setup, message).map(Msg::Setup);
        if !setup.is_finished() {
            return command;
        }
        let Some(setup) = self.setup.take() else { return command };
        // The wizard applies the language to the running application and leaves the storing to
        // whoever owns the settings file, which is here: the language belongs to the whole
        // application and not to that one screen.
        let language = setup.language();
        self.config = setup.into_config();
        self.config.set_language(language);
        // Everything a screen was given came from the settings that have just changed, so the
        // screens are made again from the new ones when they are next opened.
        self.workspaces = None;
        self.profiles = None;
        self.workspace = None;
        self.refresh_home();
        let service = self.settings.service();
        let left_behind = self.settings.left_behind().to_vec();
        self.settings =
            SettingsScreen::new(&self.config, self.engine.clone()).with_service(service).with_left_behind(left_behind);
        if !self.router.back() {
            self.router.replace(Page::Home);
        }
        Command::batch([command, self.take_focus(), self.save(), self.look()])
    }

    /// Looks for the chosen engine again, off the render path.
    fn look(&mut self) -> Command<Msg> {
        let Some(kind) = self.config.engine_kind().and_then(EngineKind::from_name) else { return Command::none() };
        self.engine = EngineState::new(kind, Health::Checking);
        Command::perform(move || {
            let found = detect(kind).map(Box::new).map_err(|missing| Trouble::of(&missing));
            Msg::Looked(kind, found)
        })
    }

    /// Takes in what the engine answered: the strip over every screen, the settings screen and
    /// every screen that reaches into a container all follow from this one answer.
    fn looked(&mut self, kind: EngineKind, found: Result<Box<Engine>, Trouble>) -> Command<Msg> {
        let health = match &found {
            Ok(_) => Health::Working,
            Err(trouble) => Health::Missing(trouble.clone()),
        };
        self.engine = EngineState::new(kind, health.clone());
        self.found = found.ok().map(|engine| *engine);
        self.workspaces = None;
        self.profiles = None;
        self.workspace = None;
        self.refresh_home();
        let (command, _) = ui::settings::update(&mut self.settings, ui::settings::Msg::Checked(health));
        command.map(Msg::Settings)
    }

    /// Does what the settings screen asked for, past every question it needed.
    fn asked(&mut self, request: Request) -> Command<Msg> {
        match request {
            Request::Language(code) => {
                self.config.set_language(&code);
                self.save()
            }
            Request::Theme(id) => {
                self.config.set_theme(&id);
                self.save()
            }
            Request::Icons(mode) => {
                self.config.set_icons(mode);
                self.save()
            }
            Request::ReducedMotion(reduced) => {
                self.config.set_reduced_motion(reduced);
                self.save()
            }
            Request::Engine(kind) => {
                self.config.set_engine_kind(kind.name());
                Command::batch([self.save(), self.look()])
            }
            Request::Editor(editor) => {
                self.config.set_editor(editor);
                if let Some(screen) = self.workspace.as_mut() {
                    screen.set_editor(editor);
                }
                self.save()
            }
            Request::Sound(sound) => {
                self.config.set_sound(sound);
                if let Some(screen) = self.workspace.as_mut() {
                    screen.set_sound(sound);
                }
                self.save()
            }
            Request::OpenEngineStep => self.repair(SetupStep::Engine),
            Request::OpenLocationStep => self.repair(SetupStep::Location),
            Request::SignOut(profile) => self.sign_out(&profile),
            Request::RefreshIdentity(profile) => self.refresh_identity(&profile),
            Request::OnClose(choice) => {
                self.config.set_on_close(choice);
                self.save()
            }
            Request::BackupEvery(choice) => {
                self.config.set_backup_every(choice);
                let applied = match self.workspace.as_mut() {
                    Some(screen) => {
                        ui::workspace::update(screen, ui::workspace::Msg::BackupEvery(choice)).map(Msg::Workspace)
                    }
                    None => Command::none(),
                };
                Command::batch([applied, self.save()])
            }
            Request::InstallService => self.change_service(true),
            Request::RemoveService => self.change_service(false),
        }
    }

    /// Whether the screen that is open is one the person may leave, which is what both the way
    /// out on screen and the Esc key ask.
    fn can_leave(&self) -> bool {
        self.router.can_go_back() && (self.page() != Page::Setup || self.setup.as_ref().is_some_and(Setup::is_alone))
    }

    /// Leaves the screen that is open, when it is one that may be left.
    ///
    /// The setup wizard is not: until the three questions are answered there is no application
    /// behind it to go back to. The one step the repair strip opens is another matter — it
    /// stands over a working application, and a person whose engine cannot be installed this
    /// minute must be able to put it down.
    fn leave(&mut self) -> Command<Msg> {
        if !self.can_leave() {
            return Command::none();
        }
        self.router.back();
        Command::batch([self.take_focus(), self.reread_profiles(), self.reread_providers()])
    }

    /// Reads the providers again for the profiles screen when it is returned to: its wizard sends
    /// the person to the Providers page to add one, and the one added there is what they came
    /// back to choose.
    fn reread_providers(&mut self) -> Command<Msg> {
        let (Page::Profiles, Some(screen)) = (self.page(), self.profiles.as_mut()) else { return Command::none() };
        ui::profiles::update(screen, ui::profiles::Msg::ReloadProviders).map(Msg::Profiles)
    }

    /// Reads the store's profiles again for the workspace screen when it is returned to, so a
    /// profile made on the profiles screen in between is offered by the next new tab.
    fn reread_profiles(&self) -> Command<Msg> {
        let (Page::Workspace, Some(store)) = (self.page(), self.store()) else { return Command::none() };
        Command::perform(move || Msg::Workspace(ui::workspace::Msg::Profiles(store.profiles().value)))
    }

    /// The screen this one was opened from, which is where leaving it leads.
    fn behind(&self) -> Option<Page> {
        let history = self.router.history();
        history.len().checked_sub(2).and_then(|index| history.get(index)).copied()
    }

    /// Whether the profiles screen is showing its list rather than its wizard.
    fn draft_is_closed(&self) -> bool {
        self.profiles.as_ref().is_some_and(|screen| screen.draft().is_none())
    }

    /// Deletes the volume that holds a profile's login, which is what signing out means.
    fn sign_out(&mut self, profile: &SafeName) -> Command<Msg> {
        let Some(engine) = self.found.clone() else { return Command::none() };
        let volume = profile::identity::sign_out(profile);
        Command::perform(move || {
            let gone = capture(&engine.remove_volume(&volume)).map(|_| ()).map_err(|error| said(&error));
            Msg::SignedOut(gone)
        })
    }

    /// Writes a profile's stored login into the home of every workspace that carries the profile,
    /// on a task thread, and answers with what came of it.
    ///
    /// Which workspaces those are is read from the store on the same thread: the workspace files
    /// are the one record of which workspace carries which profile, and reading them is disk work
    /// like the rest.
    fn refresh_identity(&self, profile: &SafeName) -> Command<Msg> {
        let (Some(engine), Some(store)) = (self.found.clone(), self.store()) else { return Command::none() };
        let user = self.user;
        let profile = profile.clone();
        Command::task(Task::new(t!("settings.refreshing", profile = profile.as_str()), move |_| {
            let workspaces: Vec<WorkspaceId> = store
                .workspaces()
                .value
                .into_iter()
                .filter_map(|entry| entry.file)
                .filter(|file| file.profiles.iter().any(|carried| carried.name == profile.as_str()))
                .map(|file| file.id)
                .collect();
            let done = profile::identity::refresh_all(&engine, &profile, &workspaces, user);
            Ok(Msg::Refreshed(profile, done))
        }))
    }

    /// Installs or removes the background service on a task thread, and tells the settings screen
    /// how it went. This is the one place QCode changes the person's service manager, and it is
    /// reached only from the button that says so.
    fn change_service(&self, installing: bool) -> Command<Msg> {
        let Some(host) = self.service.clone() else { return Command::none() };
        Command::perform(move || {
            let steps = if installing { host.install() } else { host.uninstall() };
            let result = service::units::perform(&steps, &mut service::units::run_host);
            let installed = host.is_installed();
            Msg::Settings(ui::settings::Msg::ServiceDone { installing, installed, result })
        })
    }

    /// Leaves the workspaces that are open where [`run`] finds them once the screen is given back,
    /// and quits.
    fn quit(&self) -> Command<Msg> {
        self.farewell.keep(self.workspace.as_ref().and_then(ui::workspace::leaving));
        Command::quit()
    }

    /// Writes the settings file, off the render path, and tells the settings screen how it went.
    fn save(&self) -> Command<Msg> {
        self.config.settings().save_command(|stored| Msg::Settings(ui::settings::Msg::Stored(stored)))
    }
}

/// Which profiles have a login stored, which is what the identity part of the settings screen
/// shows and acts on. A profile that signs in to nothing has no login to refresh or remove, so
/// it is not listed there.
///
/// Without an engine to ask, nothing is claimed either way and every profile is listed as signed
/// out, beside the strip that says why nothing here can be done.
fn identities(engine: Option<&Engine>, profiles: &[Profile]) -> Vec<ui::settings::identity::ProfileIdentity> {
    let volumes = engine.and_then(|engine| capture(&engine.list_volumes()).ok()).unwrap_or_default();
    profiles
        .iter()
        .filter(|profile| profile.account.needs_login())
        .map(|profile| {
            let stored = profile::identity::is_stored(&volumes, &profile.name);
            ui::settings::identity::ProfileIdentity::new(profile.name.clone(), stored)
        })
        .collect()
}

/// What the person is told when a refresh of `profile`'s login is over.
///
/// A round that wrote nowhere because no workspace carries the profile is said as such: a success
/// that counts zero would read as if something had been done.
fn refreshed(profile: &SafeName, outcome: Result<Refreshed, RefreshError>) -> Toast<Msg> {
    match outcome {
        Ok(done) if done.written.is_empty() && done.running.is_empty() => {
            Toast::warning(t!("settings.refresh-unused", profile = profile.as_str()))
        }
        Ok(done) if done.running.is_empty() => Toast::success(t!("settings.refreshed", n = done.written.len())),
        Ok(done) => {
            let names: Vec<&str> = done.running.iter().map(WorkspaceId::as_str).collect();
            Toast::warning(t!("settings.refreshed", n = done.written.len()))
                .body(t!("settings.refresh-running", workspaces = names.join(", ")))
        }
        Err(RefreshError::NotStored) => Toast::warning(t!("settings.no-identity", profile = profile.as_str())),
        Err(RefreshError::Engine(said)) => Toast::danger(t!("settings.refresh-failed")).body(said),
    }
}

/// What an engine command said when it would not do what was asked.
fn said(error: &engine::run::EngineError) -> String {
    match error {
        engine::run::EngineError::NotRunnable { error, .. } => error.to_string(),
        engine::run::EngineError::Failed(failure) => failure.output.clone(),
        engine::run::EngineError::Cancelled { .. } => t!("app.stopped"),
    }
}

/// The engine's health as the gates of the wizard left it.
fn health(check: &EngineCheck) -> Health {
    match check {
        EngineCheck::Working => Health::Working,
        EngineCheck::Unknown | EngineCheck::Running => Health::Checking,
        EngineCheck::Broken(problem) => Health::Missing(trouble(problem)),
    }
}

/// The same reason, in the shape the settings screen and the repair strip keep it.
fn trouble(problem: &ui::setup::gates::EngineProblem) -> Trouble {
    use ui::setup::gates::EngineProblem;
    match problem {
        EngineProblem::NotInstalled => Trouble::NotInstalled,
        EngineProblem::DaemonStopped { .. } => Trouble::DaemonStopped,
        EngineProblem::MachineStopped { .. } => Trouble::MachineStopped,
        EngineProblem::Refused { output, .. } => Trouble::Refused(output.clone()),
        EngineProblem::NotRunnable { message, .. } => Trouble::NotRunnable(message.clone()),
    }
}

/// The one key hint every screen carries, at the bottom right: the key that opens the list of
/// every key, which is also a button for the pointer. The hint bars the screens once had are that
/// list now, so the screens keep their room and nothing they listed is lost.
/// What the list of keys is reached by at the foot of the rail: a question mark, which QCode
/// adds to the icon set itself because the framework's has no glyph for this yet.
const KEYS_ICON: &str = "help";

/// The ways out of the workspace screen, at the foot of its rail: the list of keys, the settings
/// and the way back, in that order down to the last row.
///
/// They stand under the rail rather than in a header and a footer of their own because the
/// middle of that screen is a terminal, and a terminal is worth more rows than three controls
/// that are each one cell wide. The way back is nearest the bottom edge, which is where the
/// hand that reaches for it already is.
fn under_rail(ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.add(IconButton::new(KEYS_ICON).tooltip(t!("app.keys")).on_press(Msg::Help(true))).id("keys");
        ui.add(IconButton::new("settings").tooltip(t!("home.settings")).on_press(Msg::Open(Entry::Settings)))
            .id("rail-settings");
        ui.add(IconButton::new("arrow-left").tooltip(t!("app.back")).on_press(Msg::Back)).id("back");
    });
}

fn keys_hint(ui: &mut View<'_, Msg>) {
    let key = ui.env().keymap().chords_for(Scope::Global, "help").first().map(KeyChord::label);
    ui.row(|ui| {
        ui.spacer();
        let mut button = Button::new(t!("app.keys")).on_press(Msg::Help(true));
        if let Some(key) = key {
            button = button.shortcut(key);
        }
        ui.add(button).id("keys");
    })
    .fill_width();
}

/// The keys of `page` that are not in the keymap, which the list of keys shows first.
fn screen_hints(page: Page, icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    match page {
        Page::Setup => ui::setup::hints(icons),
        Page::Home => ui::home::hints(icons),
        Page::Workspaces => ui::workspaces::hints(icons),
        Page::Profiles => ui::profiles::hints(icons),
        Page::Providers => ui::providers::hints(icons),
        Page::Workspace => ui::workspace::hints(icons),
        Page::Settings => ui::settings::hints(icons),
    }
}

impl App for QCode {
    type Msg = Msg;

    /// Puts the keyboard on the first screen before the first key is read: the wizard's open
    /// step, or the menu of the home screen. Without this the first arrow key went nowhere.
    fn init(&mut self) -> Command<Msg> {
        match self.setup.as_mut() {
            Some(setup) => ui::setup::opened(setup).map(Msg::Setup),
            None => self.take_focus(),
        }
    }

    /// Every way out passes through the same quit as the menu's, so that the workspaces open then
    /// are backed up however QCode was left.
    fn before_quit(&self) -> Option<Msg> {
        Some(Msg::Quit)
    }

    /// Asks nothing, so a hangup, whose terminal is gone, is answered the same way.
    fn terminating(&self, _cause: qframe::runtime::Termination) -> Option<Msg> {
        Some(Msg::Quit)
    }

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        match msg {
            Msg::Home(message) => ui::home::update(&mut self.home, message),
            Msg::Open(entry) => self.open(entry),
            Msg::Setup(message) => self.wizard(message),
            Msg::Workspaces(message) => {
                let Some(screen) = self.workspaces.as_mut() else { return Command::none() };
                let (command, opened) = ui::workspaces::update(screen, message);
                let command = command.map(Msg::Workspaces);
                match opened {
                    Some(id) => Command::batch([command, self.read_workspaces(vec![id.clone()], Opening::One(id))]),
                    None => command,
                }
            }
            Msg::Providers(message) => {
                let Some(screen) = self.providers.as_mut() else { return Command::none() };
                ui::providers::update(screen, *message).map(|message| Msg::Providers(Box::new(message)))
            }
            Msg::Profiles(message) => {
                let Some(screen) = self.profiles.as_mut() else { return Command::none() };
                // The listing is what puts the list on screen, and the wizard is the person's
                // own doing, so the keyboard is only moved when there is no wizard over it.
                let landed = matches!(message, ui::profiles::Msg::Loaded(_));
                // The wizard's own account page cannot open a screen of its own; the row that
                // says a provider is needed asks the application for it, the way the workspace
                // screen already asks for the profiles screen below.
                let wants_providers = matches!(message, ui::profiles::Msg::ManageProviders);
                let command = ui::profiles::update(screen, message).map(Msg::Profiles);
                if wants_providers {
                    return Command::batch([command, self.show_providers()]);
                }
                // A workspace waiting behind this screen was waiting for exactly one thing, and it
                // now has it: the wizard closing is the way back. Reached from anywhere else the
                // screen stays open, because there the list of profiles is the thing wanted.
                let made = screen.just_made().is_some();
                if made && self.behind() == Some(Page::Workspace) {
                    return Command::batch([command, self.leave()]);
                }
                match landed && self.page() == Page::Profiles && self.draft_is_closed() {
                    true => Command::batch([command, self.take_focus()]),
                    false => command,
                }
            }
            Msg::Workspace(message) => {
                // Two things the screen asks for are screens of their own, which only the
                // application can open.
                let asked = match message {
                    ui::workspace::Msg::ManageProfiles => Some(Page::Profiles),
                    ui::workspace::Msg::AddWorkspace => Some(Page::Workspaces),
                    _ => None,
                };
                let command = match self.workspace.as_mut() {
                    Some(screen) => ui::workspace::update(screen, message).map(Msg::Workspace),
                    None => Command::none(),
                };
                let shown = match asked {
                    // The row the person pressed says "New profile", so the wizard is what opens,
                    // not the list with a second button to press for the same wish.
                    Some(Page::Profiles) => {
                        let shown = self.show_profiles();
                        let arrived = self.page() == Page::Profiles;
                        let wizard = match self.profiles.as_mut().filter(|_| arrived) {
                            Some(screen) => {
                                ui::profiles::update(screen, ui::profiles::Msg::NewWhenRead).map(Msg::Profiles)
                            }
                            None => Command::none(),
                        };
                        Command::batch([shown, wizard])
                    }
                    Some(Page::Workspaces) => self.show_workspaces(false),
                    _ => Command::none(),
                };
                Command::batch([command, shown, self.keep_session()])
            }
            Msg::Settings(message) => {
                let (command, request) = ui::settings::update(&mut self.settings, message);
                let command = command.map(Msg::Settings);
                match request {
                    Some(request) => Command::batch([command, self.asked(request)]),
                    None => command,
                }
            }
            Msg::Back => self.leave(),
            Msg::Quit => self.quit(),
            Msg::Help(open) => {
                self.help = open;
                Command::none()
            }
            Msg::Looked(kind, found) => self.looked(kind, found),
            Msg::Read(contents, opening) => self.read(contents, opening),
            Msg::SessionSaved(result) => self.session_saved(result),
            Msg::SignedOut(Ok(())) => Command::toast(Toast::success(t!("settings.signed-out"))),
            Msg::SignedOut(Err(reason)) => Command::toast(Toast::danger(t!("settings.sign-out-failed")).body(reason)),
            Msg::Refreshed(profile, outcome) => Command::toast(refreshed(&profile, outcome)),
        }
    }

    /// Turns a keymap action into a message. `back` is Esc, which reaches here only when
    /// nothing nearer has used it: a focused widget first, so the terminal of a harness tab
    /// keeps its own Esc, and then an open dialog, which closes instead of letting the screen go.
    /// On the workspace screen an entry of the file tree that waits to be pasted is let go first.
    /// `help` is the framework's own action for the list of keys; the application opens it.
    /// `new-tab` opens a blank tab and `leave-terminal` takes the keyboard back into the open
    /// tab's harness; both only mean something while a workspace is open.
    fn action(&self, name: &str) -> Option<Msg> {
        let workspace_open =
            self.page() == Page::Workspace && self.workspace.as_ref().is_some_and(|s| s.workspace().is_some());
        match name {
            "back" => Some(
                self.workspace
                    .as_ref()
                    .filter(|_| self.page() == Page::Workspace)
                    .and_then(ui::workspace::escape)
                    .map_or(Msg::Back, Msg::Workspace),
            ),
            "help" => Some(Msg::Help(true)),
            "new-tab" => workspace_open.then_some(Msg::Workspace(ui::workspace::Msg::NewTab)),
            // Inside a terminal the terminal's own node answers this key and leaves; here it
            // arrives only from outside one, so it goes back in.
            "leave-terminal" => workspace_open.then_some(Msg::Workspace(ui::workspace::Msg::EnterTerminal)),
            _ => None,
        }
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        let page = self.page();
        ui.add_with(PageTransition::new(page.key()).slide(true).direction(self.router.direction()), |ui| {
            ui.page(page.key(), |ui| self.draw(page, ui));
        })
        .fill();
        if self.help {
            // The keys of the screen that is open come first, then the keymap's own, so the list
            // holds everything the hint bars of the screens used to show.
            let hints = screen_hints(page, ui.env().icons());
            let layer =
                hints.into_iter().fold(HelpLayer::new(Msg::Help(false)), |layer, (key, label)| layer.hint(key, label));
            ui.add(layer);
        }
    }
}

impl QCode {
    /// Draws the screen that is open.
    ///
    /// The workspace screen brings a shell of its own — a rail down the left and a panel down the
    /// right, both from the top row to the bottom one — so it is drawn whole, with the
    /// application's header and footer handed in for its middle column; every other screen is a
    /// body in the application's own shell.
    fn draw(&self, page: Page, ui: &mut View<'_, Msg>) {
        if page == Page::Workspace
            && let Some(screen) = &self.workspace
        {
            // The workspace screen carries its own ways out at the foot of the rail, so the row
            // that would hold Back and the row that would hold Keys go back to the terminal.
            ui::workspace::view(screen, ui, Msg::Workspace, |ui| self.header(page, ui), |_| (), under_rail);
            return;
        }
        AppShell::new().header(|ui| self.header(page, ui)).body(|ui| self.body(page, ui)).footer(keys_hint).show(ui);
    }

    /// What stands over every screen: the strip while the engine is gone, and the way back.
    fn header(&self, page: Page, ui: &mut View<'_, Msg>) {
        // The wizard is where a missing engine is put right, so the strip that leads there does
        // not stand over it.
        if page != Page::Setup {
            ui.map(Msg::Settings, |ui| ui::settings::bar::view(&self.engine, ui)).fill_width();
        }
        // Whatever Esc does, the way out is also on screen: a screen that can be left says so.
        // The setup itself cannot be — it is left by finishing it — while the single step the
        // repair strip opens can, because it stands over an application that already works.
        // The workspace screen keeps its own at the foot of the rail, where it costs no row.
        if self.can_leave() && page != Page::Workspace {
            ui.row(|ui| {
                ui.add(Button::new(t!("app.back")).icon("arrow-left").on_press(Msg::Back)).id("back");
                ui.spacer();
            })
            .fill_width();
        }
    }

    /// The screen itself.
    fn body(&self, page: Page, ui: &mut View<'_, Msg>) {
        match page {
            Page::Setup => {
                if let Some(setup) = &self.setup {
                    ui.map(Msg::Setup, |ui| ui::setup::view(setup, ui)).fill();
                }
            }
            Page::Home => ui::home::view(&self.home, ui),
            Page::Workspaces => {
                if let Some(screen) = &self.workspaces {
                    ui.map(Msg::Workspaces, |ui| ui::workspaces::view(screen, ui)).fill();
                }
            }
            Page::Profiles => {
                if let Some(screen) = &self.profiles {
                    ui.map(Msg::Profiles, |ui| ui::profiles::view(screen, ui)).fill();
                }
            }
            Page::Providers => {
                if let Some(screen) = &self.providers {
                    ui.map(|message| Msg::Providers(Box::new(message)), |ui| ui::providers::view(screen, ui)).fill();
                }
            }
            Page::Settings => {
                ui.map(Msg::Settings, |ui| ui::settings::view(&self.settings, ui)).fill();
            }
            // Drawn whole by `draw` once its screen exists; before that there is nothing to show.
            Page::Workspace => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! What the application's own tests build an application out of, so that none of them needs
    //! the person's settings file, a container engine or a store of their own.

    use std::path::{Path, PathBuf};

    use qframe::env::{AssetDirs, Env};
    use qframe::icons::GlyphMode;
    use qframe::runtime::Harness;

    use super::{Config, Engine, EngineKind, Gates, HostDirs, InstallHost, QCode, SetupStep};
    use crate::store::Platform;
    use crate::ui::setup::gates::{EngineCheck, LocationCheck};

    /// A folder of this machine's temporary directory, named after `what` and this process.
    pub fn scratch(what: &str) -> PathBuf {
        std::env::temp_dir().join(format!("qcode-app-{what}-{}", std::process::id()))
    }

    /// A machine whose Documents folder is in a temporary home folder.
    pub fn dirs() -> HostDirs {
        let documents = scratch("home").join("Documents");
        HostDirs { store: Some(documents.join("Quvyta").join("Code")), documents: Some(documents) }
    }

    /// An Arch machine with `paru`, so every install command a test sees is the same one.
    pub fn host() -> InstallHost {
        InstallHost::read(Platform::Linux, Some("ID=arch\n"), |tool| tool == "paru")
    }

    /// Gates that all hold, which is a machine with everything in place.
    pub fn settled() -> Gates {
        Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Usable }
    }

    /// A settings file of a finished setup, with `recent` as the workspaces opened before.
    pub fn config(store: &Path, recent: &[&str]) -> Config {
        let list = recent.iter().map(|id| format!("\"{id}\"")).collect::<Vec<_>>().join(", ");
        let text = format!(
            "language = \"en\"\n\n[setup]\ncompleted = true\nstep = \"location\"\n\n[engine]\nkind = \"podman\"\n\n\
             [folder]\npath = \"{}\"\n\n[workspaces]\nrecent = [{list}]\n",
            store.display()
        );
        Config::parse_str("code.conf", &text)
    }

    /// An application over `config`, opening on the wizard's `entry` step or on the home screen.
    ///
    /// Its providers screen reads a file of this test's own and asks through a web that reaches
    /// nothing, so no test can read the person's data folder or leave the machine.
    pub fn app(config: Config, gates: &Gates, entry: Option<SetupStep>) -> QCode {
        QCode::new(config, dirs(), host(), gates, None, None, entry).with_providers(Some(providers_file()), no_web())
    }

    /// A providers file of this process's own, never the person's.
    pub fn providers_file() -> PathBuf {
        scratch("providers").join("providers.toml")
    }

    /// A web that reaches nothing at all: a test that sends a request by accident is told so
    /// rather than quietly going out over the network.
    pub fn no_web() -> crate::provider::Web {
        crate::provider::Web::new(|ask| {
            Err(crate::provider::AskError::Unreachable {
                url: ask.url.clone(),
                reason: "no test reaches the network".to_owned(),
            })
        })
    }

    /// An application on its home screen holding an engine whose binary is not there: every
    /// piece of engine work is really run and really fails, with the machine's own words, so a
    /// test sees that the work was reached without a container runtime on the machine.
    pub fn app_with_absent_engine(config: Config) -> QCode {
        let engine = Engine::new(EngineKind::Podman, "/qcode/no/such/engine");
        QCode::new(config, dirs(), host(), &settled(), Some(engine), None, None)
            .with_providers(Some(providers_file()), no_web())
    }

    /// Runs the test `name` of this crate again in a process of its own, with none of the
    /// person's XDG folders and with `vars` set, and answers what it printed.
    ///
    /// The framework reads where things live from the environment, and a test process shares
    /// one environment between all its threads; a child with its own is how a test sees where
    /// QCode puts things on a machine it describes, without touching the person's folders.
    pub fn in_child(name: &str, vars: &[(&str, &std::ffi::OsStr)]) -> String {
        let output = child_command(name, vars).output().expect("the test binary runs");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "{printed}{}", String::from_utf8_lossy(&output.stderr));
        printed
    }

    /// [`in_child`] started and left running, for a test that acts on the child while it lives:
    /// a lock it holds is released only when it exits.
    pub fn spawn_child(name: &str, vars: &[(&str, &std::ffi::OsStr)]) -> std::process::Child {
        child_command(name, vars)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the test binary runs")
    }

    /// The test binary, set to run only the test `name`, as [`in_child`] describes.
    fn child_command(name: &str, vars: &[(&str, &std::ffi::OsStr)]) -> std::process::Command {
        let exe = std::env::current_exe().expect("the test binary");
        let mut command = std::process::Command::new(exe);
        command.args([name, "--exact", "--nocapture", "--test-threads=1"]);
        for var in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_DOCUMENTS_DIR"] {
            command.env_remove(var);
        }
        command.envs(vars.iter().copied());
        command
    }

    /// QCode's own text and keys, and nothing of the person's.
    pub fn env() -> Env {
        let dirs = AssetDirs {
            locale_sources: crate::locales(),
            keymap_source: Some(crate::keymap()),
            ..AssetDirs::default()
        };
        Env::load(&dirs).expect("the built-in files load")
    }

    /// A harness over `app`, in English, with Unicode glyphs and no motion.
    pub fn harness(app: QCode, width: u16, height: u16) -> Harness<QCode> {
        let mut harness = Harness::with_env(app, env(), width, height);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.render();
        harness
    }
}

#[cfg(test)]
mod tests {
    //! Moving through the application: where it opens, how each screen is reached and left, and
    //! what the repair strip runs. Nothing here needs a container runtime or a settings file.

    use std::time::Duration;

    use super::testing::{app, app_with_absent_engine, config, harness, scratch, settled};
    use super::{Gates, Msg, OpenWorkspace, Page, Request, SetupStep, WorkspaceId, WorkspaceScreen};
    use crate::profile::SafeName;
    use crate::store::Config;
    use crate::ui::setup::gates::{EngineCheck, EngineProblem, LocationCheck};
    use crate::ui::workspace::{Tab, TabState};

    /// A terminal with room for the logo, the menu and the widest screen behind them.
    const SIZE: (u16, u16) = (96, 30);

    /// How long a click is given to be answered, for the screens that answer in a command.
    const MOMENT: Duration = Duration::from_millis(10);

    /// Gates of a machine whose engine is gone and whose store is in place.
    fn without_engine() -> Gates {
        Gates {
            language: true,
            engine: EngineCheck::Broken(EngineProblem::NotInstalled),
            location: LocationCheck::Usable,
        }
    }

    /// Where the setup wizard sits on `screen`: the rows above it, the rows between it and the
    /// application's own footer, and the columns left and right of it.
    ///
    /// The wizard runs from its row of steps down to its row of buttons; the footer under it
    /// belongs to the application and is not part of what is being centred.
    fn placed(screen: &str) -> (usize, usize, usize, usize) {
        let rows: Vec<&str> = screen.lines().collect();
        let row_with = |text: &str| {
            rows.iter().position(|line| line.contains(text)).unwrap_or_else(|| panic!("{text}:\n{screen}"))
        };
        let top = rows.iter().position(|line| !line.trim().is_empty()).expect("something is drawn");
        let buttons = row_with("Next");
        let footer = rows.iter().rposition(|line| !line.trim().is_empty()).expect("something is drawn");
        let body = &rows[top..=buttons];
        let left = body.iter().filter(|line| !line.trim().is_empty());
        let right = left.clone();
        (
            top,
            footer.saturating_sub(buttons + 1),
            left.map(|line| line.len() - line.trim_start().len()).min().unwrap_or(0),
            right.map(|line| line.trim_end().chars().count()).max().unwrap_or(0),
        )
    }

    #[test]
    fn the_wizard_stands_in_the_middle_of_a_terminal_with_room_to_spare() {
        // The whole screen this time, not one widget: what the person sees is the wizard in the
        // middle of the terminal, not pinned into its top left corner.
        let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
        let screen = harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), 120, 44);
        let screen = screen.screen();
        let (top, below, left, _) = placed(&screen);
        assert!(top.abs_diff(below) <= 1, "as much room above as below: {top} and {below}\n{screen}");
        assert!(top > 2, "the wizard is not at the top of a terminal this tall: {top}\n{screen}");
        assert!(left > 2, "the wizard is not at the left edge of a terminal this wide: {left}\n{screen}");
        // Twenty more columns and the wizard moves ten to the right: half of what is added goes
        // to each side, which is centring and nothing else, whatever the wizard's own width is.
        let wide = harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), 140, 44);
        let wide = wide.screen();
        assert_eq!(placed(&wide).2, left + 10, "{wide}");
    }

    #[test]
    fn a_terminal_with_nothing_to_spare_keeps_the_wizard_at_the_top() {
        // Centring is room that is there; a short terminal has none, and losing the first rows
        // of the wizard to a gap would be worse than a wizard that starts at the top.
        let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
        let short = harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), 88, 20);
        let screen = short.screen();
        assert_eq!(placed(&screen).0, 0, "{screen}");
        assert!(screen.contains("Next"), "the way on is still on the screen:\n{screen}");
    }

    /// The mark the chosen row of a list of choices carries, read off `screen` as the one
    /// character that stands before `name` and before nothing else in `names`.
    ///
    /// Read rather than named, so the check holds whatever glyph the theme draws the mark with.
    fn marked(screen: &str, names: &[&str]) -> Vec<String> {
        let mark = |name: &str| {
            let line = screen.lines().find(|line| line.contains(name)).unwrap_or_else(|| panic!("{name}:\n{screen}"));
            line.trim_start().chars().next().unwrap_or(' ').to_string()
        };
        let marks: Vec<String> = names.iter().map(|name| mark(name)).collect();
        let chosen = |index: usize| marks.iter().enumerate().all(|(other, m)| (other == index) == (*m == marks[index]));
        (0..marks.len()).filter(|index| chosen(*index)).map(|index| names[index].to_owned()).collect()
    }

    /// The language step as the person sees it on a machine set to `LANG`, printed by a child
    /// process, since the language of the whole application is read from the environment it was
    /// started in and a test process shares one environment between its threads.
    #[cfg(unix)]
    #[test]
    fn the_language_that_looks_chosen_is_the_one_the_wizard_is_speaking() {
        const PRINT: &str = "QCODE_TEST_PRINT_LANGUAGE_STEP";
        if std::env::var_os(PRINT).is_some() {
            let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
            let app = super::QCode::new(
                Config::parse_str("code.conf", ""),
                super::testing::dirs(),
                super::testing::host(),
                &gates,
                None,
                crate::ui::setup::languages::system_here(),
                Some(SetupStep::Language),
            );
            let mut harness = qframe::runtime::Harness::with_env(app, super::testing::env(), SIZE.0, SIZE.1);
            // No locale is set here on purpose: the wizard has to agree with the language the
            // machine itself hands the application.
            harness.set_glyph_mode(qframe::icons::GlyphMode::Unicode).set_reduced_motion(true);
            harness.render();
            println!("{}", harness.screen());
            return;
        }
        let turkish = "tr_TR.UTF-8".as_ref();
        let printed = super::testing::in_child(
            "tests::the_language_that_looks_chosen_is_the_one_the_wizard_is_speaking",
            &[(PRINT, "1".as_ref()), ("LANG", turkish), ("LC_ALL", turkish), ("LC_MESSAGES", turkish)],
        );
        assert!(printed.contains("QCode'un konuşacağı dili seç"), "the screen is drawn in Turkish:\n{printed}");
        let names =
            ["Türkçe", "Almanca", "Çince", "Fransızca", "İngilizce", "İspanyolca", "Japonca", "Portekizce", "Rusça"];
        assert_eq!(
            marked(&printed, &names),
            ["Türkçe"],
            "the row that carries the mark is the language on screen:\n{printed}"
        );
        let rows: Vec<&str> =
            printed.lines().filter_map(|line| names.iter().find(|name| line.contains(**name)).copied()).collect();
        assert_eq!(rows, names, "the machine's own language first, the rest alphabetical:\n{printed}");
    }

    /// The last row of the screen.
    fn last_row(harness: &qframe::runtime::Harness<super::QCode>) -> String {
        harness.screen().lines().last().unwrap_or_default().to_owned()
    }

    /// Presses one of the ways out at the foot of the workspace rail: the way back on the last
    /// row, the settings above it and the list of keys above that.
    ///
    /// By where the person's finger goes, not by the word on it: these are one cell wide and the
    /// whole point of them standing there is that they cost the middle of the screen no row.
    fn rail_foot(harness: &mut qframe::runtime::Harness<super::QCode>, up: i32) {
        harness.click(1, i32::from(SIZE.1) - 1 - up).advance(MOMENT);
    }

    /// The way back, at the very bottom of the workspace rail.
    fn rail_back(harness: &mut qframe::runtime::Harness<super::QCode>) {
        rail_foot(harness, 0);
    }

    /// The list of keys, two rows above it.
    fn rail_keys(harness: &mut qframe::runtime::Harness<super::QCode>) {
        rail_foot(harness, 2);
    }

    /// Opens the workspace `Firefly` of a fresh store at `root`.
    fn open_workspace(root: &std::path::Path) -> qframe::runtime::Harness<super::QCode> {
        let _ = std::fs::remove_dir_all(root);
        let store = crate::store::Store::new(root);
        store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
        let mut harness = harness(app(config(root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text("Firefly").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
        harness
    }

    #[test]
    fn the_foot_of_the_rail_carries_the_keys_the_settings_and_the_way_back_one_row_each() {
        let root = scratch("rail-foot");
        let mut harness = open_workspace(&root);
        let screen = harness.screen();
        let rows: Vec<&str> = screen.lines().collect();
        let icons = harness.env().icons();
        let (settings, arrow) = (icons.glyph("settings").into_owned(), icons.glyph("arrow-left").into_owned());
        // Three rows, not three each: a workspace tab is three rows tall and these are not tabs.
        assert_eq!(rows[rows.len() - 3].trim(), "?", "the keys:\n{screen}");
        assert_eq!(rows[rows.len() - 2].trim(), settings, "the settings above the way back:\n{screen}");
        assert_eq!(rows[rows.len() - 1].trim(), arrow, "and the way back on the very last row:\n{screen}");

        // Each one does what it stands for, pressed where it stands.
        rail_keys(&mut harness);
        assert!(harness.screen().contains("close tab"), "the keys opened:\n{}", harness.screen());
        harness.press("esc").advance(MOMENT);
        rail_foot(&mut harness, 1);
        assert_eq!(harness.app().page(), Page::Settings, "the settings opened:\n{}", harness.screen());
        harness.click_text("Back").advance(MOMENT);
        rail_back(&mut harness);
        assert_eq!(harness.app().page(), Page::Workspaces, "the way back led back:\n{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_screen_carries_one_key_hint_and_no_hint_bar() {
        let wizard = harness(
            app(
                Config::parse_str("code.conf", ""),
                &Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown },
                Some(SetupStep::Language),
            ),
            SIZE.0,
            SIZE.1,
        );
        let mut home = harness(app(config(&scratch("hint-bar"), &[]), &settled(), None), SIZE.0, SIZE.1);
        let mut screens = vec![(Page::Setup, last_row(&wizard), wizard.screen())];
        screens.push((Page::Home, last_row(&home), home.screen()));
        for row in ["Workspaces", "Profiles", "Settings"] {
            home.click_text(row).advance(MOMENT);
            screens.push((home.app().page(), last_row(&home), home.screen()));
            home.click_text("Back").advance(MOMENT);
        }
        let root = scratch("hint-bar-workspace");
        let workspace = open_workspace(&root);
        screens.push((Page::Workspace, last_row(&workspace), workspace.screen()));
        let _ = std::fs::remove_dir_all(&root);

        for (page, last, screen) in screens {
            // The workspace screen keeps its way to the keys at the foot of its rail, where it
            // costs the terminal no row; every other screen has it on the last row.
            match page {
                Page::Workspace => {
                    let rows: Vec<&str> = screen.lines().collect();
                    let keys = rows[rows.len() - 3].trim();
                    assert_eq!(keys, "?", "the way to the keys is in the rail:\n{screen}");
                    assert!(!screen.contains("Keys"), "and nowhere else:\n{screen}");
                }
                _ => assert_eq!(last.split_whitespace().collect::<Vec<_>>(), ["?", "Keys"], "{page:?}:\n{screen}"),
            }
            // The words of the old bars are gone: moving, choosing, quitting and the tab keys.
            for word in ["move", "choose", "quit", "close tab"] {
                assert!(!screen.contains(word), "{page:?} still says `{word}`:\n{screen}");
            }
        }
    }

    #[test]
    fn the_key_hint_sits_in_the_bottom_right_corner() {
        let harness = harness(app(config(&scratch("hint-corner"), &[]), &settled(), None), SIZE.0, SIZE.1);
        let (x, y) = harness.find("Keys").expect("the hint is on screen");
        assert_eq!(y, i32::from(SIZE.1) - 1, "{}", harness.screen());
        assert!(x + 4 >= i32::from(SIZE.0) - 3, "at the right edge:\n{}", harness.screen());
    }

    #[test]
    fn the_help_key_opens_the_list_with_the_screens_own_keys_and_esc_closes_it() {
        let mut harness = harness(app(config(&scratch("help-key"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Settings").advance(MOMENT);
        harness.press("?");
        let screen = harness.screen();
        assert!(screen.contains("This screen"), "the key list is open:\n{screen}");
        for word in ["move", "open", "leave this screen"] {
            assert!(screen.contains(word), "`{word}` is in the list:\n{screen}");
        }
        harness.press("esc").advance(MOMENT);
        assert!(!harness.screen().contains("This screen"), "Esc closes it:\n{}", harness.screen());
        assert_eq!(harness.app().page(), Page::Settings, "and only it; the screen stays");

        harness.press("f1");
        assert!(harness.screen().contains("This screen"), "F1 opens it too:\n{}", harness.screen());
    }

    #[test]
    fn a_click_on_the_hint_opens_the_list() {
        let mut harness = harness(app(config(&scratch("help-click"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Keys");
        let screen = harness.screen();
        assert!(screen.contains("This screen"), "{screen}");
        assert!(screen.contains("move"), "the home screen's keys are listed:\n{screen}");
    }

    #[test]
    fn the_workspace_screens_key_list_says_how_to_leave_the_terminal() {
        let root = scratch("help-workspace");
        let mut harness = open_workspace(&root);
        rail_keys(&mut harness);
        let screen = harness.screen();
        for word in ["tabs", "ctrl w", "close tab", "shift tab", "leave the terminal"] {
            assert!(screen.contains(word), "`{word}` is in the list:\n{screen}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_key_list_names_the_way_from_the_harness_to_the_tabs_in_both_languages() {
        let root = scratch("help-leave");
        let mut harness = open_workspace(&root);
        rail_keys(&mut harness);
        let screen = harness.screen();
        assert!(screen.contains("ctrl alt space"), "{screen}");
        assert!(screen.contains("between the harness and the tabs"), "{screen}");
        harness.set_locale("tr").render();
        assert!(harness.screen().contains("düzenek ile sekmeler arasında geç"), "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn on_the_workspace_screen_the_panel_runs_from_the_first_row_to_the_last() {
        let root = scratch("full-height-panel");
        let harness = open_workspace(&root);
        let screen = harness.screen();
        let (panel_x, _) = harness.find("Panel").expect("the panel is open");
        let arrow = harness.env().icons().glyph("arrow-left").into_owned();
        let (back_x, back_y) = harness.find(&arrow).expect("the way back");
        let (keys_x, keys_y) = harness.find("?").expect("the way to the keys");
        assert_eq!(back_y, i32::from(SIZE.1) - 1, "the way back is on the last row:\n{screen}");
        assert_eq!(keys_y, i32::from(SIZE.1) - 3, "and the keys two rows above it:\n{screen}");
        assert!(back_x < panel_x && keys_x < panel_x, "both in the rail, not over the panel:\n{screen}");
        // The rail is where they are, so the tabs have the first row and the terminal the last.
        assert!(screen.lines().next().is_some_and(|row| row.contains('+')), "the tabs are first:\n{screen}");
        let right = SIZE.0 - 1;
        let panel = harness.bg(right, SIZE.1 / 2);
        assert_eq!(harness.bg(right, 0), panel, "the panel's surface on the first row:\n{screen}");
        assert_eq!(harness.bg(right, SIZE.1 - 1), panel, "and on the last:\n{screen}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_machine_that_was_never_set_up_opens_the_wizard_on_the_gate_that_fell() {
        let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
        let harness =
            harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), SIZE.0, SIZE.1);
        assert_eq!(harness.app().page(), Page::Setup);
        assert!(harness.screen().contains("Pick the language"), "{}", harness.screen());
    }

    #[test]
    fn a_finished_setup_opens_the_home_screen() {
        let harness = harness(app(config(&scratch("open"), &[]), &settled(), None), SIZE.0, SIZE.1);
        assert_eq!(harness.app().page(), Page::Home);
        assert!(harness.screen().contains("New workspace"), "{}", harness.screen());
    }

    #[test]
    fn settings_moved_from_the_old_folder_open_the_home_screen_they_describe() {
        let folder = scratch("moved-config");
        let _ = std::fs::remove_dir_all(&folder);
        let legacy = folder.join("code");
        std::fs::create_dir_all(&legacy).expect("folder");
        std::fs::write(legacy.join("settings.toml"), config(&scratch("moved"), &[]).to_toml()).expect("file");

        let loaded = Config::load_in(&folder, &legacy);

        assert!(loaded.is_clean(), "{:?}", loaded.diagnostics);
        let harness = harness(app(loaded.value, &settled(), None), SIZE.0, SIZE.1);
        assert_eq!(harness.app().page(), Page::Home, "the finished setup came along");
        assert!(harness.screen().contains("New workspace"), "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn an_old_settings_file_that_stayed_behind_is_said_on_the_settings_screen() {
        let folder = scratch("left-config");
        let _ = std::fs::remove_dir_all(&folder);
        let legacy = folder.join("code");
        std::fs::create_dir_all(&legacy).expect("folder");
        std::fs::write(legacy.join("settings.toml"), "[engine]\nkind = \"docker\"\n").expect("file");
        std::fs::write(folder.join("code.conf"), config(&scratch("left"), &[]).to_toml()).expect("file");

        let loaded = Config::load_in(&folder, &legacy);
        let app = app(loaded.value, &settled(), None).with_left_behind(loaded.diagnostics);
        // Tall enough for the report and every row of the page under it, so its title is not
        // scrolled away to keep the settings list in view.
        let mut harness = harness(app, SIZE.0, 40);
        assert_eq!(harness.app().page(), Page::Home);
        harness.click_text("Settings");

        let screen = harness.screen();
        assert!(screen.contains("Some old settings files stayed where they were"), "{screen}");
        assert!(screen.contains("settings.toml"), "{screen}");
        // The report has the keyboard, so the list below it does not scroll it out of sight.
        assert!(harness.is_focused("left-behind-read"), "{screen}");

        harness.press("enter").advance(MOMENT);
        let screen = harness.screen();
        assert!(!screen.contains("Some old settings files stayed where they were"), "{screen}");
        assert!(harness.is_focused("settings"), "the list takes the keyboard once the report is read");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_gate_that_falls_after_the_setup_leaves_the_wizard_shut_and_puts_the_strip_up() {
        // The design's rule: once the wizard has been through it never opens by itself again.
        // A missing engine is said in the strip over the screen, not asked for all over again.
        let harness = harness(app(config(&scratch("gone"), &[]), &without_engine(), None), SIZE.0, SIZE.1);
        assert_eq!(harness.app().page(), Page::Home);
        let screen = harness.screen();
        assert!(screen.contains("No container engine"), "{screen}");
        assert!(screen.contains("Repair"), "{screen}");
    }

    #[test]
    fn every_row_of_the_menu_opens_its_screen_and_the_way_back_leads_home() {
        let mut harness = harness(app(config(&scratch("menu"), &[]), &settled(), None), SIZE.0, SIZE.1);
        for (row, page, word) in [
            ("Workspaces", Page::Workspaces, "New workspace"),
            ("Profiles", Page::Profiles, "Profiles"),
            ("Providers", Page::Providers, "plain text"),
            ("Settings", Page::Settings, "Language"),
        ] {
            harness.click_text(row).advance(MOMENT);
            assert_eq!(harness.app().page(), page, "`{row}` opens its screen:\n{}", harness.screen());
            assert!(harness.screen().contains(word), "`{word}` is on the screen `{row}` opened:\n{}", harness.screen());
            harness.click_text("Back").advance(MOMENT);
            assert_eq!(harness.app().page(), Page::Home, "the way back leads home:\n{}", harness.screen());
        }
    }

    /// Walks the whole wizard, choosing the language whose name on the first step is `language`,
    /// and answers what the settings file says afterwards when it is read back from its own text.
    fn language_after_the_wizard(name: &str, language: &str, next: &str, finish: &str) -> Option<String> {
        let gates = Gates { language: false, engine: EngineCheck::Working, location: LocationCheck::Usable };
        let text = format!("[folder]\npath = \"{}\"\n", scratch(name).display());
        let app = app(Config::parse_str("code.conf", &text), &gates, Some(SetupStep::Language));
        let mut harness = harness(app, SIZE.0, SIZE.1);
        harness.click_text(language).advance(MOMENT);
        for _ in 0..2 {
            harness.click_text(next).advance(MOMENT);
        }
        harness.click_text(finish).advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
        // Read back from the text that would be written, not from the settings in memory: a
        // choice that is not in the file is a choice the next start does not have.
        Config::parse_str("code.conf", &harness.app().config.to_toml()).settings().language()
    }

    #[test]
    fn a_wizard_finished_in_turkish_opens_in_turkish_next_time() {
        assert_eq!(language_after_the_wizard("language-tr", "Turkish", "İleri", "Bitir").as_deref(), Some("tr"));
    }

    #[test]
    fn a_wizard_finished_in_english_opens_in_english_next_time() {
        // English is the answer that used to be the schema's default, and a default is the one
        // answer a settings file can lose: without it written down, a machine whose own locale
        // is Turkish would open in Turkish however plainly the person said English.
        assert_eq!(language_after_the_wizard("language-en", "English", "Next", "Finish").as_deref(), Some("en"));
    }

    #[test]
    fn a_workspace_of_the_list_opens_its_own_screen_and_the_way_back_leads_to_the_list() {
        let root = scratch("open-workspace");
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::store::Store::new(&root);
        store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text("Firefly").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
        assert!(harness.screen().contains("No tab is open"), "{}", harness.screen());
        rail_back(&mut harness);
        assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_way_to_a_profile_from_a_workspace_comes_back_with_the_new_profile_offered() {
        let root = scratch("workspace-profiles");
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::store::Store::new(&root);
        store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text("Firefly").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace);

        harness.send(Msg::Workspace(crate::ui::workspace::Msg::ManageProfiles)).advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Profiles, "the row of the chooser leads to the profiles screen");

        // A profile made there, while the workspace screen waits behind it.
        let profile = crate::profile::Profile {
            name: SafeName::parse("claude-sub").expect("the name is safe"),
            harness: crate::profile::HarnessKind::ClaudeCode,
            template: crate::profile::Template::Recommended,
            account: crate::profile::AccountKind::Subscription,
            provider: None,
            assets: crate::profile::MountAccess::ReadOnly,
            network: crate::profile::NetworkMode::Full,
            without: Vec::new(),
        };
        store.write_profile(&profile).expect("the store takes a profile");
        harness.click_text("Back").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
        let offered: Vec<String> = harness
            .app()
            .workspace
            .as_ref()
            .and_then(WorkspaceScreen::workspace)
            .map(|workspace| workspace.profiles().iter().map(|profile| profile.name.as_str().to_owned()).collect())
            .unwrap_or_default();
        assert_eq!(offered, ["claude-sub"], "the next new tab offers it without opening the workspace again");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_profile_made_for_a_workspace_takes_the_person_straight_back_to_it() {
        use crate::ui::profiles::Msg as Profiles;
        let root = scratch("workspace-profile-back");
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::store::Store::new(&root);
        store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text("Firefly").advance(MOMENT);
        // The row a workspace's new tab offers when it has no profile to open a tab with. One
        // press for one wish: it lands in the wizard, not on a list with another such button.
        harness.click_text("New tab").advance(MOMENT);
        harness.click_text("New profile").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
        assert!(
            harness.app().profiles.as_ref().and_then(|screen| screen.draft()).is_some(),
            "the wizard is open:\n{}",
            harness.screen()
        );
        assert!(!harness.screen().contains("No profiles yet"), "{}", harness.screen());

        harness.type_text("scout");
        // A harness with no sign-in, so the image page is the wizard's last one.
        harness.click_text("opencode").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("free, no account").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        // The engine's answer, which is the one thing here that a machine has to give.
        harness.send(Msg::Profiles(Profiles::BuildEnded(Ok(())))).advance(MOMENT);

        assert_eq!(harness.app().page(), Page::Profiles, "still on the wizard until Finish is pressed");
        harness.click_text("Finish").advance(MOMENT);
        assert_eq!(
            harness.app().page(),
            Page::Workspace,
            "the profile was made for the workspace, so the workspace is where it goes:\n{}",
            harness.screen()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_provider_added_on_the_way_out_of_the_wizard_is_offered_when_the_person_comes_back() {
        let root = scratch("wizard-provider");
        let _ = std::fs::remove_dir_all(&root);
        let _store = crate::store::Store::new(&root);
        let providers = scratch("wizard-provider-file").join("providers.toml");
        let _ = std::fs::remove_dir_all(providers.parent().expect("its folder"));
        let app =
            app(config(&root, &[]), &settled(), None).with_providers(Some(providers.clone()), crate::testing::no_web());
        let mut harness = harness(app, SIZE.0, SIZE.1);
        harness.click_text("Profiles").advance(MOMENT);
        harness.click_text("New profile").advance(MOMENT);
        // Claude Code is the harness offered first; the template and then the account follow.
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("a provider of your own").advance(MOMENT);
        assert!(harness.screen().contains("You have not added a provider yet"), "{}", harness.screen());

        // Exactly what the page says to do: go to Providers, add one, come back.
        harness.click_text("Go to Providers").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Providers, "{}", harness.screen());
        harness.click_text("Add a provider").advance(MOMENT);
        harness.click_text("ollama").advance(MOMENT);
        harness.press("tab");
        harness.type_text("ev");
        harness.press("tab");
        for _ in 0..80 {
            harness.press("backspace");
        }
        harness.type_text("http://192.168.122.1:11434");
        harness.click_text("Add").advance(MOMENT);
        assert!(std::fs::read_to_string(&providers).is_ok_and(|text| text.contains("\"ev\"")), "it was added");
        harness.click_text("Back").advance(MOMENT);

        assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
        let screen = harness.screen();
        assert!(!screen.contains("You have not added a provider yet"), "the wizard no longer says so:\n{screen}");
        assert!(screen.contains("ev"), "the provider just added is offered:\n{screen}");
        harness.click_text("ev").advance(MOMENT);
        let draft = harness.app().profiles.as_ref().and_then(|screen| screen.draft()).expect("the wizard is open");
        assert_eq!(draft.provider_tag.as_deref(), Some("ev"), "and it can be chosen");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(providers.parent().expect("its folder"));
    }

    #[test]
    fn a_profile_made_from_the_home_screen_leaves_the_list_of_profiles_open() {
        use crate::ui::profiles::Msg as Profiles;
        // Nobody was waiting for this one. Somebody who came to the profiles to make profiles is
        // most likely making another, so the list they came for stays in front of them.
        let root = scratch("home-profile-stays");
        let _ = std::fs::remove_dir_all(&root);
        let _store = crate::store::Store::new(&root);
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Profiles").advance(MOMENT);
        harness.click_text("New profile").advance(MOMENT);
        harness.type_text("scout");
        harness.click_text("opencode").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("free, no account").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.click_text("Next").advance(MOMENT);
        harness.send(Msg::Profiles(Profiles::BuildEnded(Ok(())))).advance(MOMENT);
        harness.click_text("Finish").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_screen_that_opens_takes_the_keyboard_so_the_arrow_keys_work_at_once() {
        // The hint under every screen promises the arrow keys. A screen that opens without the
        // keyboard on its list breaks that promise until a Tab nobody mentioned is pressed.
        let root = scratch("focus-list");
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::store::Store::new(&root);
        for name in ["Alpha", "Beta"] {
            store.create_workspace(name, qframe::date::Date::today_utc()).expect("the store takes a workspace");
        }
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        assert!(harness.is_focused("workspaces"), "the list has the keyboard:\n{}", harness.screen());
        harness.press("down").press("enter").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "down and enter alone opened the second workspace");
        let open = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(OpenWorkspace::name);
        assert_eq!(open, Some("Beta"), "the row under the first one is the one that opened");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_very_first_key_works_on_the_home_screen() {
        // No click, no Tab: the application has just opened, and the menu already has the
        // keyboard. Down from the first row is Workspaces.
        let mut harness = harness(app(config(&scratch("focus-first"), &[]), &settled(), None), SIZE.0, SIZE.1);
        assert!(harness.is_focused("menu"), "{}", harness.screen());
        harness.press("down").press("enter").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
    }

    #[test]
    fn the_wizard_opens_with_the_keyboard_on_its_first_question() {
        let harness =
            harness(app(config(&scratch("focus-wizard"), &[]), &settled(), Some(SetupStep::Language)), SIZE.0, SIZE.1);
        // The gates are all settled, so the wizard opens on its last step; the keyboard is on
        // that step's question, not on the first step's.
        assert_eq!(harness.app().page(), Page::Setup);
        assert!(harness.is_focused("setup-location"), "{}", harness.screen());
    }

    #[test]
    fn the_settings_screen_takes_the_keyboard_as_it_opens() {
        let mut harness = harness(app(config(&scratch("focus-settings"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Settings").advance(MOMENT);
        assert!(harness.is_focused("settings"), "{}", harness.screen());
    }

    #[test]
    fn coming_back_puts_the_keyboard_on_the_menu_again() {
        let mut harness = harness(app(config(&scratch("focus-home"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Settings").advance(MOMENT);
        harness.click_text("Back").advance(MOMENT);
        assert!(harness.is_focused("menu"), "{}", harness.screen());
        // The click left the selection on Settings, so one press up is the row above it, and
        // that press is the first key after coming back.
        harness.press("up").press("enter").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Providers, "{}", harness.screen());
    }

    #[test]
    fn escape_leaves_the_screen_that_is_open() {
        let mut harness = harness(app(config(&scratch("escape"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Settings").advance(MOMENT);
        harness.press("esc").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
    }

    #[test]
    fn escape_closes_what_is_open_over_a_screen_before_it_leaves_the_screen() {
        let root = scratch("escape-dialog");
        let _ = std::fs::remove_dir_all(&root);
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text("New workspace").advance(MOMENT);
        assert!(harness.screen().contains("Start from"), "the dialog is open:\n{}", harness.screen());
        harness.press("esc").advance(MOMENT);
        assert!(!harness.screen().contains("Start from"), "the dialog closed:\n{}", harness.screen());
        assert_eq!(harness.app().page(), Page::Workspaces, "and the screen under it stayed");
        harness.press("esc").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Home, "a second press leaves the screen");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn escape_does_nothing_in_the_setup_wizard() {
        // There is no application behind the wizard to go back to, so the keys that leave a
        // screen do not leave this one; it is left by finishing it.
        let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
        let app = app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language));
        let mut harness = harness(app, SIZE.0, SIZE.1);
        harness.press("esc").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
        assert!(harness.screen().contains("Pick the language"), "{}", harness.screen());
        assert!(!harness.screen().contains("Back"), "and no way out is offered:\n{}", harness.screen());
    }

    #[test]
    fn the_one_step_the_repair_strip_opens_can_be_put_down_again() {
        // A person whose engine cannot be installed this minute must be able to leave the step
        // and go on using everything that does not need a container.
        let mut harness = harness(app(config(&scratch("repair-esc"), &[]), &without_engine(), None), SIZE.0, SIZE.1);
        harness.click_text("Repair").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Setup);
        assert!(harness.screen().contains("Back"), "the way out is on screen too:\n{}", harness.screen());
        harness.press("esc").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
    }

    #[test]
    fn escape_leaves_the_workspace_screen_while_no_tab_holds_the_keyboard() {
        // With a tab open the terminal has the keyboard and every key, Esc included, belongs to
        // the harness inside the container; the way out is then the one on screen. That half
        // needs a container runtime, so it is not tested here. This is the other half.
        let root = scratch("escape-workspace");
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::store::Store::new(&root);
        store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text("Firefly").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace);
        let arrow = harness.env().icons().glyph("arrow-left").into_owned();
        assert!(harness.screen().contains(&arrow), "the way out is on screen:\n{}", harness.screen());
        harness.press("esc").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn escape_first_lets_a_cut_file_stay_and_only_then_leaves_the_workspace_screen() {
        let root = scratch("escape-cut");
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::store::Store::new(&root);
        store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text("Firefly").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace);
        harness.send(Msg::Workspace(crate::ui::workspace::Msg::Files(crate::ui::workspace::FileMsg::Cut(
            "notes.txt".to_owned(),
        ))));
        harness.press("esc").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "the first Esc only lets the cut go");
        let cut = harness.app().workspace.as_ref().and_then(|screen| screen.workspace()).map(|p| p.files().cut().len());
        assert_eq!(cut, Some(0));
        harness.press("esc").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refreshing_an_identity_is_carried_out_and_answered_with_what_came_of_it() {
        // The request reaches the engine work and the answer is that work's own: with an engine
        // that is not there, the round stops at the engine and the toast carries its refusal
        // rather than a promise that the work will exist one day.
        let mut harness = harness(app_with_absent_engine(config(&scratch("refresh"), &[])), SIZE.0, SIZE.1);
        harness.click_text("Settings").advance(MOMENT);
        let profile = SafeName::parse("claude-sub").expect("the name is safe");
        harness
            .send(Msg::Settings(crate::ui::settings::Msg::Request(Request::RefreshIdentity(profile))))
            .advance(MOMENT);
        let screen = harness.screen();
        assert!(screen.contains("could not be refreshed"), "{screen}");
        assert!(!screen.contains("not part of QCode"), "{screen}");
    }

    #[test]
    fn the_workspaces_open_as_qcode_quits_are_left_to_be_backed_up() {
        let root = scratch("farewell");
        store_of(&root, &["Alpha", "Beta"]);
        let farewell = super::Farewell::default();
        let app = app_with_absent_engine(config(&root, &[])).with_farewell(farewell.clone());
        let mut harness = harness(app, SIZE.0, SIZE.1);
        open_from_the_list(&mut harness, "Alpha");
        harness.send(Msg::Workspace(crate::ui::workspace::Msg::AddWorkspace)).advance(MOMENT);
        harness.click_text("Beta").advance(MOMENT);
        harness.press("ctrl+q");
        assert!(harness.quit_requested());
        let left = farewell.take().expect("the open workspaces were left behind");
        assert_eq!(left.names(), ["Alpha", "Beta"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quitting_from_the_menu_leaves_them_too_and_nothing_when_none_is_open() {
        let farewell = super::Farewell::default();
        let app = app_with_absent_engine(config(&scratch("farewell-none"), &[])).with_farewell(farewell.clone());
        let mut harness = harness(app, SIZE.0, SIZE.1);
        harness.click_text("Quit");
        assert!(harness.quit_requested());
        assert_eq!(farewell.take(), None, "no workspace was open");
    }

    #[test]
    fn a_new_interval_reaches_the_open_workspaces_at_once() {
        use crate::backup::BackupEvery;

        let root = scratch("backup-every");
        store_of(&root, &["Alpha"]);
        let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
        open_from_the_list(&mut harness, "Alpha");
        let every = |harness: &qframe::runtime::Harness<super::QCode>| {
            harness.app().workspace.as_ref().map(WorkspaceScreen::backup_every)
        };
        assert_eq!(every(&harness), Some(BackupEvery::Fifteen));
        harness.send(Msg::Settings(crate::ui::settings::Msg::BackupEvery(BackupEvery::Hour))).advance(MOMENT);
        assert_eq!(every(&harness), Some(BackupEvery::Hour));
        assert_eq!(harness.app().config.backup_every(), BackupEvery::Hour, "and it is stored");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_sound_choice_reaches_the_workspaces_opened_later_and_the_open_ones_at_once() {
        use crate::base::apps::Sound;

        let root = scratch("sound-choice");
        store_of(&root, &["Alpha"]);
        let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
        let settings = |sound| Msg::Settings(crate::ui::settings::Msg::Sound(sound));
        let sound = |harness: &qframe::runtime::Harness<super::QCode>| {
            harness.app().workspace.as_ref().map(WorkspaceScreen::sound)
        };
        harness.send(settings(Sound::Details)).advance(MOMENT);
        assert!(harness.app().config.to_toml().contains("[apps]\nsound = \"details\""), "it is stored");
        open_from_the_list(&mut harness, "Alpha");
        assert_eq!(sound(&harness), Some(Sound::Details), "a screen made after the choice starts with it");
        harness.send(settings(Sound::Play)).advance(MOMENT);
        assert_eq!(sound(&harness), Some(Sound::Play), "and the open screen takes the next one at once");
        assert!(!harness.app().config.to_toml().contains("[apps]"), "{}", harness.app().config.to_toml());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_last_row_leaves_the_application() {
        let mut harness = harness(app(config(&scratch("quit"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Quit");
        assert!(harness.quit_requested());
    }

    #[test]
    fn the_repair_strip_runs_the_wizards_engine_step_on_its_own() {
        let mut harness = harness(app(config(&scratch("repair"), &[]), &without_engine(), None), SIZE.0, SIZE.1);
        harness.click_text("Repair").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Setup);
        let screen = harness.screen();
        assert!(screen.contains("Container engine"), "the engine step is the one that opened:\n{screen}");
        assert!(!screen.contains("Pick the language"), "no step before it is asked again:\n{screen}");
        assert!(!screen.contains("Store"), "and none after it either:\n{screen}");
    }

    #[test]
    fn the_settings_screen_can_move_the_store_through_the_same_one_step() {
        let mut harness = harness(app(config(&scratch("move"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Settings").advance(MOMENT);
        harness.click_text("Change").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
        let screen = harness.screen();
        assert!(screen.contains("live in one folder"), "the location step is the one that opened:\n{screen}");
    }

    #[test]
    fn the_release_of_the_click_that_opened_the_location_step_does_not_finish_it() {
        use qframe::event::{MouseButton, MouseKind};

        // "Change" acts when the button goes down, and the page it opens puts Finish on the
        // screen while the button is still held. Wherever that release lands, it began on the
        // settings row and not on Finish, so the step must stay open for the person to answer.
        let mut harness = harness(app(config(&scratch("release"), &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.click_text("Settings").advance(MOMENT);
        let (x, y) = harness.find("Change").expect("the settings screen offers to move the store");
        harness.mouse(MouseKind::Down(MouseButton::Left), x, y).advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Setup, "{}", harness.screen());
        let (x, y) = harness.find("Finish").expect("the location step can be finished");
        harness.mouse(MouseKind::Up(MouseButton::Left), x, y).advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Setup, "the step is still open:\n{}", harness.screen());
        // The same button, pressed and released on itself, does finish: it was reachable all along.
        harness.click_text("Finish").advance(MOMENT);
        assert_ne!(harness.app().page(), Page::Setup, "{}", harness.screen());
    }

    /// A store at `root` holding the workspaces `names`, made afresh.
    fn store_of(root: &std::path::Path, names: &[&str]) -> crate::store::Store {
        let _ = std::fs::remove_dir_all(root);
        let store = crate::store::Store::new(root);
        for name in names {
            store.create_workspace(name, qframe::date::Date::today_utc()).expect("the store takes a workspace");
        }
        store
    }

    /// The names of the rail of the open workspace screen, in order.
    fn rail(harness: &qframe::runtime::Harness<super::QCode>) -> Vec<String> {
        let screen = harness.app().workspace.as_ref().expect("a workspace screen");
        screen.workspaces().iter().map(|workspace| workspace.name().to_owned()).collect()
    }

    /// Opens `name` from the list of workspaces, reached from the home screen.
    fn open_from_the_list(harness: &mut qframe::runtime::Harness<super::QCode>, name: &str) {
        harness.click_text("Workspaces").advance(MOMENT);
        harness.click_text(name).advance(MOMENT);
    }

    /// Opens a blank tab on the workspace screen and chooses a shell in it; the engine of these
    /// tests is not there, so the tab fails at once and keeps the engine's words.
    fn open_a_shell(harness: &mut qframe::runtime::Harness<super::QCode>) {
        use crate::ui::workspace::{Choice, Msg as Workspace};
        harness.send(Msg::Workspace(Workspace::NewTab)).advance(MOMENT);
        let screen = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace);
        let key = screen.and_then(|workspace| workspace.active_tab()).map(crate::ui::workspace::Tab::key);
        let key = key.expect("the blank tab is open");
        harness.send(Msg::Workspace(Workspace::Choose(key, Choice::Shell))).advance(MOMENT);
    }

    /// What the tabs of the open workspace are, in order.
    fn tab_kinds(harness: &qframe::runtime::Harness<super::QCode>) -> Vec<crate::ui::workspace::TabKind> {
        let screen = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace);
        screen.map(|workspace| workspace.tabs().iter().map(|tab| tab.kind().clone()).collect()).unwrap_or_default()
    }

    #[test]
    fn ctrl_t_opens_a_blank_tab_and_the_key_list_says_so() {
        use crate::ui::workspace::TabKind;

        let root = scratch("ctrl-t");
        store_of(&root, &["Alpha"]);
        let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
        harness.press("ctrl+t").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Home, "away from a workspace the key means nothing");
        open_from_the_list(&mut harness, "Alpha");
        harness.press("ctrl+t").advance(MOMENT);
        assert_eq!(tab_kinds(&harness), [TabKind::New], "{}", harness.screen());
        assert!(harness.is_focused("workspace-choices"), "the blank tab's page has the keyboard");
        harness.press("ctrl+t").advance(MOMENT);
        assert_eq!(tab_kinds(&harness), [TabKind::New, TabKind::New], "the page lets the key through");

        harness.press("?");
        let screen = harness.screen();
        assert!(screen.contains("open a new tab"), "the key list names it:\n{screen}");
        assert!(screen.contains("ctrl t"), "{screen}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_workspace_opened_from_the_list_is_the_only_one_in_the_rail() {
        let root = scratch("only-one");
        store_of(&root, &["Alpha", "Beta", "Gamma"]);
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        open_from_the_list(&mut harness, "Beta");
        assert_eq!(rail(&harness), ["Beta"], "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_rails_plus_opens_the_list_and_the_pick_joins_the_same_screen() {
        let root = scratch("rail-add");
        store_of(&root, &["Alpha", "Beta"]);
        let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
        open_from_the_list(&mut harness, "Alpha");
        open_a_shell(&mut harness);
        let tab = harness
            .app()
            .workspace
            .as_ref()
            .and_then(WorkspaceScreen::workspace)
            .map(|workspace| workspace.tabs()[0].key());

        let text = harness.screen();
        let row = text.lines().position(|line| line.chars().take(4).any(|cell| cell == '+'));
        let row = i32::try_from(row.expect("the rail carries a plus")).expect("a row");
        harness.click(1, row).advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspaces, "the plus opens the list:\n{}", harness.screen());
        harness.click_text("Beta").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
        assert_eq!(rail(&harness), ["Alpha", "Beta"], "the pick joins the screen that was open");
        harness.send(Msg::Workspace(crate::ui::workspace::Msg::OpenWorkspace(0))).advance(MOMENT);
        let kept = harness
            .app()
            .workspace
            .as_ref()
            .and_then(WorkspaceScreen::workspace)
            .map(|workspace| workspace.tabs()[0].key());
        assert_eq!(kept, tab, "and nothing of the first workspace was closed");

        rail_back(&mut harness);
        assert_eq!(harness.app().page(), Page::Workspaces, "the list is not stacked twice:\n{}", harness.screen());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn continue_goes_back_to_the_live_screen_as_it_was() {
        let root = scratch("continue-live");
        store_of(&root, &["Alpha", "Beta"]);
        let mut harness = harness(app_with_absent_engine(config(&root, &[])), SIZE.0, SIZE.1);
        open_from_the_list(&mut harness, "Alpha");
        open_a_shell(&mut harness);
        let before = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(|workspace| {
            let tab = &workspace.tabs()[0];
            (tab.key(), tab.state().clone())
        });
        rail_back(&mut harness);
        harness.click_text("Back").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Home);
        assert!(harness.screen().contains("Continue"), "{}", harness.screen());
        assert!(harness.screen().contains("Alpha"), "the row names the open workspace:\n{}", harness.screen());

        // The list opens another workspace into the screen that is still live behind the home
        // screen, rather than a screen of its own.
        open_from_the_list(&mut harness, "Beta");
        assert_eq!(rail(&harness), ["Alpha", "Beta"]);
        harness.press("esc").advance(MOMENT);
        harness.press("esc").advance(MOMENT);
        assert!(harness.screen().contains("Alpha +1"), "{}", harness.screen());
        harness.click_text("Continue").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
        assert_eq!(rail(&harness), ["Alpha", "Beta"]);
        let open = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(OpenWorkspace::name);
        assert_eq!(open, Some("Beta"), "the workspace that was open is open again");
        harness.send(Msg::Workspace(crate::ui::workspace::Msg::OpenWorkspace(0))).advance(MOMENT);
        let after = harness.app().workspace.as_ref().and_then(WorkspaceScreen::workspace).map(|workspace| {
            let tab = &workspace.tabs()[0];
            (tab.key(), tab.state().clone())
        });
        assert_eq!(after, before, "the tab is the same tab, in the state it was left in");
        rail_back(&mut harness);
        assert_eq!(harness.app().page(), Page::Home, "continue came straight from home");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn continue_brings_back_the_saved_session_and_starts_only_the_tab_in_view() {
        let root = scratch("continue-saved");
        store_of(&root, &["Alpha", "Beta"]);
        let file = scratch("continue-saved-session").join("session.toml");
        let _ = std::fs::remove_file(&file);
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
        let text = "active = \"alpha\"\n\n\
                    [[workspace]]\nid = \"beta\"\nactive-tab = 0\n\n\
                    [[workspace]]\nid = \"alpha\"\nactive-tab = 1\n\n\
                    [[workspace.tab]]\nkind = \"shell\"\nopened = 10\n\n\
                    [[workspace.tab]]\nkind = \"shell\"\nconversation = \"c-1\"\nopened = 20\n";
        std::fs::write(&file, text).expect("a session file");
        let app = app_with_absent_engine(config(&root, &["beta"])).with_session(Some(file.clone()));
        let mut harness = harness(app, SIZE.0, SIZE.1);
        assert!(harness.screen().contains("beta +1"), "{}", harness.screen());
        harness.click_text("Continue").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
        assert_eq!(rail(&harness), ["Beta", "Alpha"], "the rail's order is the session's");
        let screen = harness.app().workspace.as_ref().expect("a workspace screen");
        let alpha = screen.workspace().expect("a workspace is open");
        assert_eq!(alpha.name(), "Alpha", "the open workspace is the session's");
        let tabs: Vec<(u64, Option<&str>)> =
            alpha.tabs().iter().map(|tab| (tab.opened(), tab.conversation())).collect();
        assert_eq!(tabs, [(10, None), (20, Some("c-1"))]);
        assert_eq!(alpha.active_tab().map(Tab::opened), Some(20), "the open tab is the session's");
        assert_eq!(alpha.tabs()[0].state(), &TabState::Waiting, "the tab out of view has not started");
        assert!(
            matches!(alpha.tabs()[1].state(), TabState::Failed(_)),
            "the tab in view went to the engine at once: {:?}",
            alpha.tabs()[1].state()
        );
        // Nothing changed, so nothing was written back over the file.
        assert_eq!(std::fs::read_to_string(&file).ok().as_deref(), Some(text));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn workspaces_of_the_session_that_are_gone_are_skipped_and_named() {
        let root = scratch("continue-gone");
        store_of(&root, &["Alpha"]);
        let file = scratch("continue-gone-session").join("session.toml");
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
        let text = "[[workspace]]\nid = \"ghost\"\n\n[[workspace]]\nid = \"alpha\"\n\n[[workspace]]\nid = \"wraith\"\n";
        std::fs::write(&file, text).expect("a session file");
        let app = app(config(&root, &[]), &settled(), None).with_session(Some(file.clone()));
        let mut harness = harness(app, SIZE.0, SIZE.1);
        harness.click_text("Continue").advance(MOMENT);
        assert_eq!(rail(&harness), ["Alpha"]);
        let screen = harness.screen();
        assert!(screen.contains("2 workspaces of the last session are gone"), "{screen}");
        assert!(screen.contains("ghost, wraith"), "{screen}");
        let saved = crate::store::Session::load(&file).value.expect("the file is rewritten");
        assert_eq!(saved.workspaces.len(), 1, "and it now holds what is open");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_broken_session_file_is_read_in_part_and_says_where() {
        let root = scratch("continue-broken");
        store_of(&root, &["Alpha"]);
        let file = scratch("continue-broken-session").join("session.toml");
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
        std::fs::write(&file, "[[workspace]]\nid = \"alpha\"\nactive-tab = \"x\"\n").expect("a session file");
        let app = app(config(&root, &[]), &settled(), None).with_session(Some(file.clone()));
        let mut harness = harness(app, SIZE.0, SIZE.1);
        harness.click_text("Continue").advance(MOMENT);
        assert_eq!(rail(&harness), ["Alpha"]);
        let screen = harness.screen();
        assert!(screen.contains("Only part of the last session"), "{screen}");
        assert!(screen.contains("session.toml:3:"), "the toast says where:\n{screen}");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_session_file_that_cannot_be_read_falls_back_to_the_last_workspace() {
        let root = scratch("continue-unreadable");
        store_of(&root, &["Alpha", "Beta"]);
        // A folder where the file should be.
        let file = scratch("continue-unreadable-session");
        std::fs::create_dir_all(&file).expect("a folder");
        let app = app(config(&root, &["beta"]), &settled(), None).with_session(Some(file.clone()));
        let mut harness = harness(app, SIZE.0, SIZE.1);
        harness.click_text("Continue").advance(MOMENT);
        assert_eq!(rail(&harness), ["Beta"], "the workspace opened last, alone");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&file);
    }

    #[test]
    fn the_session_is_written_when_what_is_open_changes_and_only_then() {
        let root = scratch("session-writes");
        store_of(&root, &["Alpha", "Beta"]);
        let file = scratch("session-writes-file").join("deeper").join("session.toml");
        let _ = std::fs::remove_dir_all(file.parent().expect("a folder"));
        let app = app(config(&root, &[]), &settled(), None).with_session(Some(file.clone()));
        let mut harness = harness(app, SIZE.0, SIZE.1);
        open_from_the_list(&mut harness, "Alpha");
        let saved = crate::store::Session::load(&file).value.expect("opening a workspace writes the session");
        assert_eq!(saved.workspaces.iter().map(|workspace| workspace.id.as_str()).collect::<Vec<_>>(), ["alpha"]);

        std::fs::remove_file(&file).expect("the file is there");
        harness.send(Msg::Workspace(crate::ui::workspace::Msg::TogglePanel(false))).advance(MOMENT);
        harness.send(Msg::Workspace(crate::ui::workspace::Msg::OpenWorkspace(0))).advance(MOMENT);
        assert!(!file.exists(), "a change that leaves the session as it was writes nothing");

        harness.send(Msg::Workspace(crate::ui::workspace::Msg::AddWorkspace)).advance(MOMENT);
        harness.click_text("Beta").advance(MOMENT);
        let saved = crate::store::Session::load(&file).value.expect("a new workspace writes it again");
        assert_eq!(saved.active.as_ref().map(WorkspaceId::as_str), Some("beta"));
        harness.send(Msg::Workspace(crate::ui::workspace::Msg::CloseWorkspace(1))).advance(MOMENT);
        let saved = crate::store::Session::load(&file).value.expect("closing one writes it again");
        assert_eq!(saved.workspaces.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(file.parent().expect("a folder"));
    }

    #[test]
    fn the_on_close_choice_is_stored_and_the_default_is_not_written() {
        use crate::store::OnClose;
        let root = scratch("on-close");
        let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
        harness.send(Msg::Settings(crate::ui::settings::Msg::OnClose(OnClose::Keep)));
        assert_eq!(harness.app().config.on_close(), OnClose::Keep);
        assert!(harness.app().config.to_toml().contains("[containers]\non-close = \"keep\""));
        harness.send(Msg::Settings(crate::ui::settings::Msg::OnClose(OnClose::Stop)));
        assert_eq!(harness.app().config.on_close(), OnClose::Stop);
        assert!(!harness.app().config.to_toml().contains("containers"), "{}", harness.app().config.to_toml());
    }

    #[test]
    fn the_service_row_follows_the_machine_and_what_is_installed_on_it() {
        use crate::service::units::ServiceHost;
        use crate::store::Platform;
        use crate::ui::settings::ServiceRow;
        let root = scratch("service-row");
        let _ = std::fs::remove_dir_all(&root);
        let host = ServiceHost {
            platform: Platform::Linux,
            units: root.join("systemd"),
            registry: root.join("containers.toml"),
            program: root.join("qcode"),
            uid: Some(1000),
        };
        let linux = app(config(&root, &[]), &settled(), None).with_service(Some(host.clone()), Platform::Linux);
        assert_eq!(linux.settings.service(), Some(ServiceRow::Ready { installed: false, busy: false }));
        // The files alone say it is installed: they are read from the disk on the way in. They
        // are written here by hand; pressing the button would run the machine's service manager.
        for (path, text) in host.files() {
            std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
            std::fs::write(path, text).expect("written");
        }
        let installed = app(config(&root, &[]), &settled(), None).with_service(Some(host), Platform::Linux);
        assert_eq!(installed.settings.service(), Some(ServiceRow::Ready { installed: true, busy: false }));
        let windows = app(config(&root, &[]), &settled(), None).with_service(None, Platform::Windows);
        assert_eq!(windows.settings.service(), Some(ServiceRow::Unsupported));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod version {
    #[test]
    fn the_version_line_names_the_package_and_its_version() {
        assert_eq!(super::version_line(), format!("quvyta-code {}", env!("CARGO_PKG_VERSION")));
    }
}
