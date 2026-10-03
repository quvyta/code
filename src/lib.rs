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
pub mod switch;
pub mod ui;

mod application;
mod navigate;
mod requests;

#[cfg(test)]
mod keyboard_tests;
#[cfg(test)]
mod reopen_tests;

#[cfg(test)]
mod endurance_live;

use std::io::{self, Write as _};
use std::path::PathBuf;

use qframe::prelude::*;
use qframe::runtime::{Runtime, Update, UpdateCheck};
use qframe::storage::Family;

use engine::run::capture;
use engine::{Engine, EngineKind, HostUser, Unavailable, detect};
use profile::identity::{RefreshError, Refreshed};
use profile::{Profile, SafeName};
use requests::health;
use service::units::ServiceHost;
use store::{
    Config, HostDirs, Loaded, Platform, Registry, Session, SetupStep, Store, WorkspaceFile, WorkspaceId, WorkspacePaths,
};
use ui::home::{Entry, Home};
use ui::profiles::Profiles;
use ui::providers::Providers as ProvidersScreen;
use ui::settings::engine::{EngineState, Trouble};
use ui::settings::{ServiceRow, Settings as SettingsScreen};
use ui::setup::Setup;
use ui::setup::gates::{EngineCheck, Gates};
use ui::setup::install::{InstallHost, Installer};
use ui::workspace::{Farewell, WorkspaceScreen};
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
    // An engine found stuck is said so from background work, in the language chosen by then.
    engine::run::speak_with(|| {
        std::sync::Arc::new(service::translator(Config::load().value.settings().language().as_deref()))
    });
    // The setting as it is on disk when the containers are about to be stopped, which may be long
    // after this QCode started and after another one changed it.
    let settings_now = || Config::load().value;
    let on_close = || settings_now().on_close();
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
    let mut engines = service::Engines { engine_for: &engine_for, run: &mut capture };
    let left = leave(&mut io::stdout(), farewell.take().as_ref(), open, &settings_now, &mut engines, &text);
    ran.and(left)
}

/// What QCode does on the way out, once its screen is given back: the workspaces it leaves are
/// backed up, the containers are stopped or kept as `settings` say when this is the last QCode
/// open, and the harnesses that keep their conversations in a database are backed up once their
/// containers stopped. What is worth saying about it is written to `out`, in the words `text`
/// gives.
///
/// `settings` reads the settings file as it is at that moment, which may be long after this
/// QCode started and after another one changed it.
fn leave(
    out: &mut dyn io::Write,
    leaving: Option<&ui::workspace::Leaving>,
    open: Option<service::Open>,
    settings: &dyn Fn() -> Config,
    engines: &mut service::Engines<'_>,
    text: &dyn Fn() -> std::sync::Arc<qframe::i18n::I18n>,
) -> io::Result<()> {
    let on_close = || settings().on_close();
    let mut say = |lines: Vec<String>| lines.iter().try_for_each(|line| writeln!(out, "{line}"));
    // The workspaces are backed up before their containers may be stopped, and a backup that
    // failed is the one thing said about it: one that worked needs no word on the way out.
    let backed = leaving.map_or(Ok(()), |leaving| qframe::i18n::scope(text(), || say(leaving.back_up())));
    // The screen is given back by now, so what the last QCode stopped is said on the terminal
    // the person started it from.
    let closed = open.map_or(Ok(()), |open| {
        let reaped = open.close(&on_close, engines)?;
        qframe::i18n::scope(text(), || match reaped {
            Some(reaped @ (service::stop::Reaped::Done(_) | service::stop::Reaped::Broken(_))) => {
                say(service::report(&reaped))
            }
            Some(service::stop::Reaped::Kept) | None => Ok(()),
        })
    });
    // A harness that keeps its conversations in a database is backed up only once its container
    // has stopped, which is now when this QCode stopped it.
    let stopped = leaving.map_or(Ok(()), |leaving| qframe::i18n::scope(text(), || say(leaving.back_up_stopped())));
    backed.and(closed).and(stopped)
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
    /// What moving to the other container engine involves: images to build, homes to copy and
    /// what is left in the engine before.
    Switch,
}

impl Page {
    /// The name the page keeps its focus and its scrolling under, and the key the transition
    /// between pages watches.
    ///
    /// Never the name of a control on it: a screen asking for the keyboard to go to its list
    /// while the list is not drawn yet would find the page of the same name instead, and the
    /// keyboard would land on whatever the page shows first, such as a button about to go.
    fn key(self) -> &'static str {
        match self {
            Self::Setup => "page-setup",
            Self::Home => "page-home",
            Self::Workspaces => "page-workspaces",
            Self::Profiles => "page-profiles",
            Self::Providers => "page-providers",
            Self::Workspace => "page-workspace",
            Self::Settings => "page-settings",
            Self::Switch => "page-switch",
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
    /// The workspace works in the person's own folder where it stands, and that folder was not
    /// there when it was read. Such a workspace is not opened: its containers would be given a
    /// folder that is not there, and an engine may make an empty one in its place.
    pub folder_gone: bool,
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
    /// Something happened on the page that follows a change of engine.
    Switch(ui::switch::Msg),
    /// Leave the screen that is open and go back to the one before it.
    Back,
    /// Leave QCode, however that was asked for.
    Quit,
    /// The person asked to leave, with a key or the menu: leave, or first ask when an agent is
    /// still at work.
    AskQuit,
    /// The person answered the question before quitting by staying.
    QuitKept,
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
    /// The images QCode's own rebuilds left behind were taken away, and this is how many of them
    /// there were. Nothing is said about it: nobody asked for it, and a disk that gave its
    /// gigabytes back is not a thing a person is shown.
    Cleared(usize),
    /// A newer version of QCode is out.
    NewVersion(Update),
}

/// Where the family's update notice is kept and where QCode remembers when it last asked for a
/// newer version of itself.
///
/// The switch is the family's, one for every Quvyta application, so it is read from the family's
/// folder rather than from QCode's settings. A test gives folders of its own, so nothing it does
/// reads or turns off the person's own switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFolders {
    /// The family's configuration folder, whose shared file holds the switch.
    pub config: PathBuf,
    /// QCode's state folder, which remembers when the question was last asked.
    pub state: PathBuf,
}

impl UpdateFolders {
    /// This machine's folders, or `None` without a home folder, where nothing could remember the
    /// switch or the last question and so nothing is asked.
    #[must_use]
    pub fn here() -> Option<Self> {
        let family = Family::QUVYTA;
        family.config_dir().zip(family.state_dir(store::APP)).map(|(config, state)| Self { config, state })
    }
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
    /// Whether the question before quitting is on screen, so that asking to quit again answers
    /// it rather than asking a second time.
    asking_quit: bool,
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
    /// The page that follows a change of engine, while it is open.
    switch: Option<ui::switch::Switch>,
    /// The engine the settings were just changed away from, until the new one has answered and
    /// the page that follows the change has been opened.
    switching_from: Option<EngineKind>,
    /// How the page that follows a change of engine finds the two engines.
    finder: ui::switch::Finder,
    /// Where the family's update notice is kept, or `None` where QCode asks for no newer version.
    updates: Option<UpdateFolders>,
}

impl QCode {
    /// The application as this machine leaves it: the gates of the design are asked, and
    /// [`entry`](Self::entry) turns their answer into the step the wizard opens on, if any.
    #[must_use]
    pub fn start(config: Config, dirs: HostDirs, host: InstallHost) -> Self {
        Self::start_with(config, dirs, host, &detect)
    }

    /// The same application, with every engine question put to `detect` instead of to the
    /// machine, so that a test can count what a start asks of the engines.
    ///
    /// The gates are asked once and the engine that answered is the engine the application
    /// keeps: `podman info` takes about 0.4 s before the first frame, so asking it a second time
    /// for the engine itself would be half the wait from launch to the home screen.
    #[must_use]
    pub fn start_with(
        config: Config,
        dirs: HostDirs,
        host: InstallHost,
        detect: &dyn Fn(EngineKind) -> Result<Engine, Unavailable>,
    ) -> Self {
        let (gates, found) = Gates::probe_with(&config, detect);
        let kind = config.engine_kind().and_then(EngineKind::from_name);
        let entry = Self::entry(&config, &gates);
        // The saved engine does not answer and the other one does: the same page a change in the
        // settings opens is offered, and nothing is changed until the person takes it. Only
        // asked of a setup that is through, since the wizard is where an engine is first chosen.
        let offer = match (kind, entry) {
            (Some(saved), None) if gates.engine != EngineCheck::Working => {
                let other = switch::other(saved);
                switch::offer_at_start(saved, false, detect(other).is_ok()).map(|to| (saved, to))
            }
            _ => None,
        };
        Self::new(config, dirs, host, &gates, found, ui::setup::languages::system_here(), entry)
            .with_switch_offer(offer)
            .with_session(Session::file())
            .with_registry(Registry::file())
            .with_service(ServiceHost::detect(), Platform::host())
            .with_update_notice(UpdateFolders::here())
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
            asking_quit: false,
            session_file: None,
            saved: None,
            saved_problems: Vec::new(),
            saving: false,
            save_failed: false,
            live_files: false,
            registry: None,
            service: None,
            farewell: Farewell::default(),
            switch: None,
            switching_from: None,
            finder: ui::switch::Finder::detecting(),
            updates: None,
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

    /// The same application opening on the offer to move from the first engine to the second,
    /// which QCode makes at start when the saved engine does not answer and the other one does;
    /// `None` opens as it would have.
    #[must_use]
    pub fn with_switch_offer(mut self, offer: Option<(EngineKind, EngineKind)>) -> Self {
        if let Some(engines) = offer {
            self.open_switch(engines, true);
        }
        self
    }

    /// The same application finding the engines of the page that follows a change of engine
    /// with `finder`, which is how a test hands it engines of its own.
    #[must_use]
    pub fn with_finder(mut self, finder: ui::switch::Finder) -> Self {
        self.finder = finder;
        self
    }

    /// Opens the page that follows a change of engine from the first to the second. It looks
    /// at both engines as soon as it is shown.
    fn open_switch(&mut self, engines: (EngineKind, EngineKind), offered: bool) {
        let profiles = self.store().map(|store| store.profiles().value).unwrap_or_default();
        self.switch = Some(ui::switch::Switch::new(engines, offered, self.user, profiles, self.finder.clone()));
        if self.page() != Page::Switch {
            self.router.push(Page::Switch);
        }
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

    /// The same application, asking at start whether a newer version is out while the family's
    /// update notice in `folders` is on, and showing that switch in the settings. `None` asks
    /// nothing and shows no switch, which is every test that has not said otherwise.
    #[must_use]
    pub fn with_update_notice(mut self, folders: Option<UpdateFolders>) -> Self {
        let on = folders.as_ref().map(|folders| Family::QUVYTA.update_notice_in(&folders.config));
        self.settings = self.settings.with_update_notice(on);
        self.updates = folders;
        self
    }

    /// The question for a newer version of QCode, when the family's update notice is on.
    ///
    /// The switch is read here, not only where the question is sent: a family that turned it off
    /// asks nothing at all, whoever runs the question.
    fn ask_for_update(&self) -> Command<Msg> {
        let Some(folders) = &self.updates else { return Command::none() };
        if !Family::QUVYTA.update_notice_in(&folders.config) {
            return Command::none();
        }
        let check = UpdateCheck::new(
            Family::QUVYTA,
            store::APP,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            Msg::NewVersion,
        )
        .in_folders(folders.config.clone(), folders.state.clone());
        Command::check_for_update(check)
    }

    /// Takes away the images of ours that a rebuild left without a name, once at start and off the
    /// render path.
    ///
    /// This is the moment for it: an image a rebuild replaced is found by the engine and by nothing
    /// else, and an engine asked at start would hold the first frame while it answers, so it is
    /// asked on a thread of its own and the person is told nothing. Without an engine there is
    /// nothing to ask, and a machine still in the wizard has nothing on it to take away yet.
    fn clear_leftovers(&self) -> Command<Msg> {
        let Some(engine) = self.found.clone() else { return Command::none() };
        Command::perform(move || Msg::Cleared(ui::profiles::work::clear_leftovers(&engine)))
    }

    /// Turns the family's update notice on or off in its shared file, off the render path.
    fn store_update_notice(&self, on: bool) -> Command<Msg> {
        let Some(folders) = &self.updates else { return Command::none() };
        let folder = folders.config.clone();
        Command::perform(move || {
            let stored = Family::QUVYTA.set_update_notice_in(&folder, on).map_err(|error| error.to_string());
            Msg::Settings(ui::settings::Msg::Stored(stored))
        })
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
            Page::Switch => self.switch.as_ref().map(ui::switch::entry),
        };
        control.map_or_else(Command::none, Command::focus)
    }
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod version {
    #[test]
    fn the_version_line_names_the_package_and_its_version() {
        assert_eq!(super::version_line(), format!("quvyta-code {}", env!("CARGO_PKG_VERSION")));
    }
}
