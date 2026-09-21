//! The profiles screen: the profiles a store holds, and the wizard that makes one.
//!
//! A profile is what a workspace opens a harness with, so this screen answers two questions about
//! every profile — is its image built, and is it signed in — and never answers either of them
//! from memory. Both come from the engine, and until the engine has answered the screen says
//! that it does not know yet.
//!
//! The screen speaks its own [`Msg`], which the application maps to its own message on the way
//! out of [`update`] and [`view`]. Everything that touches the disk or
//! the engine happens in a [`Command`], so the state changes by message alone and every state
//! the screen has — empty, loading, broken, without an engine, mid-build, mid-login — is reached
//! in a test with no container runtime.

#[cfg(test)]
mod live;
pub(crate) mod recipe;
mod status;
mod wizard;
pub(crate) mod work;

use std::path::PathBuf;
use std::sync::Arc;

use qframe::diagnostics::Diagnostic;
use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::widgets::{
    Badge, EmptyState, Field, LogBuffer, LogLevel, LogLine, LogView, RadioGroup, SettingRow, SettingsList, Switch,
    Terminal, TerminalEvent, TerminalSession, TextInput, Toast, Wizard,
};

use crate::base::{Gap, Os, Refusal};
use crate::engine::Engine;
use crate::profile::{
    ASSUMED_CONTEXT_TOKENS, AccountKind, Extra, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template,
};
use crate::provider::{ProviderEntry, Providers};
use crate::store::Store;
use crate::ui::settings::engine::help;

pub use status::{Readiness, Row, Status};
pub use wizard::{Blocked, Build, Draft, Login, Stage, Unfinished};
pub use work::Problem;

/// Width of the wizard's pages: wide enough for every step to keep its name in the row of
/// steps, and still one column of text. A narrower terminal shrinks it, and the steps fall back
/// to their markers.
///
/// Measured against the seven English steps, whose row is 104 cells, with the frame around it;
/// German, French, Turkish, Russian, Portuguese and Chinese fit too. Spanish and Japanese name
/// their steps longer and show the markers with the current step's name, as a narrow terminal
/// does.
const PAGE_WIDTH: u16 = 110;

/// Rows the build log and the login terminal are given, so the buttons under them stay put.
const VIEWPORT_ROWS: u16 = 16;

/// Rows the tallest page of the wizard takes at [`PAGE_WIDTH`], with the steps above it and the
/// buttons under it: the image page of a machine without an engine. The wizard is placed as if
/// every page were this tall, so the steps stay on the same row from page to page.
const WIZARD_ROWS: u16 = 25;

/// Rows the application's own header and footer take around this screen, which the screen's
/// view cannot see but which share the terminal with it.
const CHROME_ROWS: u16 = 2;

/// What was read out of a store's `Profiles/` folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The profiles whose files could be read.
    pub profiles: Vec<Profile>,
    /// Everything that was wrong with the files, each with its place in one.
    pub diagnostics: Vec<Diagnostic>,
}

/// Everything that can happen on the profiles screen.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Read the store again. This is also how the screen is started.
    Reload,
    /// The store was read.
    Loaded(Listing),
    /// The providers file was read, on the same reload.
    ProvidersLoaded(Vec<ProviderEntry>),
    /// Read the providers file again, and only that: the Providers page was open over this
    /// screen, and what was added there is what the wizard's account page now offers.
    ReloadProviders,
    /// The engine answered about every profile.
    Probed(Vec<Status>),
    /// The selection moved.
    Select(usize),
    /// Signing the chosen profile in was asked for, which is how a login that was interrupted is
    /// picked up again.
    SignInAsked,
    /// Signing a profile out was asked for.
    SignOutAsked,
    /// The question was answered with yes.
    SignOutConfirmed,
    /// The login was removed, or could not be.
    SignedOut(Result<(), Problem>),
    /// Open the wizard.
    New,
    /// Open the wizard as soon as the store has been read, for a person who asked for a new
    /// profile from somewhere else and should not have to ask a second time here. It waits for
    /// the listing because the wizard's suggested name steps around the names already taken.
    NewWhenRead,
    /// Leave the wizard without keeping anything that has not been made yet.
    Cancel,
    /// Go back a page.
    Back,
    /// Go on a page.
    Next,
    /// Leave the wizard, keeping what was made.
    Finish,
    /// Go back to a finished page.
    Step(usize),
    /// The name was typed.
    Name(String),
    /// A harness was chosen.
    PickHarness(usize),
    /// An operating system was chosen, by its place in [`Os::ALL`].
    PickOs(usize),
    /// A template was chosen.
    PickTemplate(usize),
    /// A part of QCode high's additions was switched on (`true`) or off.
    SwitchExtra(Extra, bool),
    /// An account type was chosen.
    PickAccount(usize),
    /// A provider was chosen, on the account page's own list.
    PickProvider(usize),
    /// A model of the chosen provider was chosen.
    PickProviderModel(usize),
    /// The person asked to go to the Providers page, because the harness they chose can be
    /// pointed at one but they have not added any yet.
    ManageProviders,
    /// An access for `Assets/` was chosen.
    PickAssets(usize),
    /// A network mode was chosen.
    PickNetwork(usize),
    /// Build the image.
    BuildStart,
    /// The base image was not on the machine, so it is being built before the profile's image.
    BaseImageStarted,
    /// The build printed a line.
    BuildLine(String),
    /// The build ended.
    BuildEnded(Result<(), Problem>),
    /// Stop the build.
    BuildCancel,
    /// Open the container the login happens in.
    LoginStart,
    /// The container is up and the harness is running on a terminal.
    LoginOpened {
        /// The terminal the harness draws on.
        session: TerminalSession,
        /// The container it runs in.
        container: Arc<work::LoginContainer>,
    },
    /// Something happened on the login terminal.
    LoginEvent(TerminalEvent),
    /// The person says they have signed in; look for the login and store it.
    LoginDone,
    /// Stop the login and leave nothing behind.
    LoginCancel,
    /// The login was stored, or it was not.
    LoginStored(Result<usize, Problem>),
    /// The container could not be opened.
    LoginFailed(Problem),
    /// A container a login used has been cleared away.
    LoginDiscarded,
}

/// The profiles screen.
#[derive(Debug)]
pub struct Profiles {
    root: Option<PathBuf>,
    engine: Option<Engine>,
    loading: bool,
    rows: Vec<Row>,
    problems: Vec<Diagnostic>,
    selected: usize,
    draft: Option<Draft>,
    signing_out: bool,
    made: Option<SafeName>,
    /// The profile the wizard just finished, to be chosen in the list once the list has been read
    /// again with it in. Kept apart from `made`, which the application takes at once.
    choose_when_read: Option<SafeName>,
    /// The providers the person has added, as of the last reload, offered on the wizard's
    /// account page to a harness that can be pointed at one.
    providers: Vec<ProviderEntry>,
    /// Where they are read from: the file the Providers page writes, or nowhere on a machine
    /// without a data folder.
    providers_file: Option<PathBuf>,
    /// The wizard was asked for while the store was still being read; it opens with the listing.
    new_wanted: bool,
}

impl Profiles {
    /// A profiles screen for the store at `root`, using `engine`.
    ///
    /// Both are optional because both can be missing on a working machine: a store that has
    /// not been chosen yet, and an engine that was uninstalled after the setup. The screen opens
    /// either way and says what it cannot do.
    #[must_use]
    pub fn new(root: Option<PathBuf>, engine: Option<Engine>) -> Self {
        Self {
            loading: root.is_some(),
            root,
            engine,
            rows: Vec::new(),
            problems: Vec::new(),
            selected: 0,
            draft: None,
            signing_out: false,
            made: None,
            choose_when_read: None,
            providers: Vec::new(),
            providers_file: Providers::file(),
            new_wanted: false,
        }
    }

    /// The same screen reading the providers from `path` rather than from this machine's data
    /// folder, so that it offers exactly what the Providers page it sends the person to wrote.
    #[must_use]
    pub fn with_providers_file(mut self, path: Option<PathBuf>) -> Self {
        self.providers_file = path;
        self
    }

    /// The profile the wizard has just finished making, taken once.
    ///
    /// It is how this screen tells the application that the work it was opened for is done. Who
    /// opened it decides what that means: a workspace waiting behind this screen for a profile it
    /// can open a tab with gets it back, and nobody else is moved.
    pub fn just_made(&mut self) -> Option<SafeName> {
        self.made.take()
    }

    /// The profiles and what the engine said about them.
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The profile the selection is on.
    #[must_use]
    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// The wizard, while it is open.
    #[must_use]
    pub fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }

    /// Whether the store is still being read.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Everything that was wrong with the definition files.
    #[must_use]
    pub fn problems(&self) -> &[Diagnostic] {
        &self.problems
    }

    /// The store, when there is one.
    fn store(&self) -> Option<Store> {
        self.root.as_ref().map(Store::new)
    }
}

/// Applies a message to the screen. Everything the returned [`Command`] later delivers comes
/// back through the same `update`.
pub fn update(state: &mut Profiles, message: Msg) -> Command<Msg> {
    match message {
        Msg::Reload => reload(state),
        Msg::Loaded(listing) => {
            state.loading = false;
            state.problems = listing.diagnostics;
            state.rows = listing.profiles.iter().cloned().map(Row::new).collect();
            state.selected = state.selected.min(state.rows.len().saturating_sub(1));
            // A person who just made a profile is looking for it, not for whatever stood first.
            // A reading that was already under way when the wizard closed does not have it yet, so
            // the wish waits for the reading that does.
            let made = state.choose_when_read.as_ref();
            if let Some(index) = made.and_then(|made| state.rows.iter().position(|row| row.profile.name == *made)) {
                state.selected = index;
                state.choose_when_read = None;
            }
            let probing = probe(state, listing.profiles);
            if std::mem::take(&mut state.new_wanted) {
                return Command::batch([probing, update(state, Msg::New)]);
            }
            probing
        }
        Msg::ProvidersLoaded(providers) => {
            // A wizard that is open offers what the file has now, not what it had when it opened.
            if let Some(draft) = &mut state.draft {
                draft.offer_providers(providers.clone());
            }
            state.providers = providers;
            Command::none()
        }
        Msg::ReloadProviders => load_providers(state),
        Msg::Probed(answers) => {
            for row in &mut state.rows {
                for answer in &answers {
                    row.apply(answer);
                }
            }
            Command::none()
        }
        Msg::Select(index) => {
            if index < state.rows.len() {
                state.selected = index;
            }
            Command::none()
        }
        Msg::SignInAsked => {
            // A profile without an image has nothing to sign in from; the list says so already.
            let Some(row) = state.selected() else { return Command::none() };
            if state.engine.is_none() || row.image != Readiness::Present || !row.profile.account.needs_login() {
                return Command::none();
            }
            state.draft = Some(Draft::for_login(&row.profile));
            Command::focus("login-open")
        }
        Msg::SignOutAsked => ask_sign_out(state),
        Msg::SignOutConfirmed => sign_out(state),
        Msg::SignedOut(result) => {
            state.signing_out = false;
            match result {
                Ok(()) => Command::batch([Command::toast(Toast::success(t!("profiles.signed-out"))), reload(state)]),
                Err(problem) => Command::toast(failure_toast("profiles.sign-out-failed", &problem)),
            }
        }
        Msg::NewWhenRead if state.loading => {
            state.new_wanted = true;
            Command::none()
        }
        Msg::NewWhenRead | Msg::New => {
            state.choose_when_read = None;
            let taken = state.rows.iter().map(|row| row.profile.name.as_str().to_owned());
            state.draft = Some(Draft::new(taken, state.providers.clone()));
            Command::focus("profile-name")
        }
        Msg::Cancel => leave(state),
        // Finish stands where Next would on the last page. A profile with no sign-in ends on its
        // image page, where finishing before the image is built would leave with nothing made,
        // so it is answered the way Next is: with the way to the image.
        Msg::Finish => {
            if state.draft.as_ref().is_some_and(|draft| draft.blocked().is_some()) {
                return next(state);
            }
            // Finish is only offered once there is a profile; Cancel is the other way out and
            // leaves nothing behind, so only this one is a profile having been made.
            state.made = state.draft.as_ref().and_then(Draft::profile).map(|profile| profile.name);
            state.choose_when_read.clone_from(&state.made);
            leave(state)
        }
        Msg::Back => {
            if let Some(draft) = &mut state.draft {
                draft.back();
            }
            Command::none()
        }
        Msg::Step(index) => {
            if let Some(draft) = &mut state.draft {
                draft.go_to(index);
            }
            Command::none()
        }
        Msg::Next => next(state),
        Msg::Name(name) => {
            if let Some(draft) = &mut state.draft {
                draft.renamed = true;
                draft.name = name;
            }
            Command::none()
        }
        Msg::PickHarness(index) => {
            if let (Some(draft), Some(harness)) = (&mut state.draft, HarnessKind::ALL.get(index)) {
                draft.choose_harness(*harness);
            }
            Command::none()
        }
        Msg::PickOs(index) => {
            if let (Some(draft), Some(os)) = (&mut state.draft, Os::ALL.get(index)) {
                draft.os = *os;
            }
            Command::none()
        }
        Msg::PickTemplate(index) => {
            if let (Some(draft), Some(template)) = (&mut state.draft, Template::ALL.get(index)) {
                draft.template = *template;
            }
            Command::none()
        }
        Msg::SwitchExtra(extra, on) => {
            if let Some(draft) = &mut state.draft {
                draft.switch(extra, on);
            }
            Command::none()
        }
        Msg::PickAccount(index) => {
            if let Some(draft) = &mut state.draft
                && let Some(account) = draft.harness.record().accounts.get(index)
            {
                draft.account = *account;
                // Picking a provider is worth nothing on its own; the person still has to name a
                // model, so the first of whichever provider stands first is chosen along with it,
                // the same courtesy `choose_harness` pays the account page itself.
                let first_tag = draft.providers.first().map(|entry| entry.tag.to_string());
                if *account == AccountKind::Provider
                    && let Some(tag) = first_tag
                {
                    draft.choose_provider(&tag);
                }
            }
            Command::none()
        }
        Msg::PickProvider(index) => {
            if let Some(draft) = &mut state.draft {
                let tag = draft.providers.get(index).map(|entry| entry.tag.to_string());
                if let Some(tag) = tag {
                    draft.choose_provider(&tag);
                }
            }
            Command::none()
        }
        Msg::PickProviderModel(index) => {
            if let Some(draft) = &mut state.draft {
                let chosen = draft.provider_models().get(index).map(|model| model.id.clone());
                if let Some(id) = chosen {
                    draft.choose_provider_model(&id);
                }
            }
            Command::none()
        }
        Msg::ManageProviders => Command::none(),
        Msg::PickAssets(index) => {
            if let (Some(draft), Some(access)) = (&mut state.draft, MountAccess::ALL.get(index)) {
                draft.assets = *access;
            }
            Command::none()
        }
        Msg::PickNetwork(index) => {
            if let (Some(draft), Some(mode)) = (&mut state.draft, NetworkMode::ALL.get(index)) {
                draft.network = *mode;
            }
            Command::none()
        }
        Msg::BuildStart => start_build(state),
        Msg::BaseImageStarted => {
            if let Some(draft) = &mut state.draft {
                draft.log.push(LogLine::new(LogLevel::Info, t!("profiles.wizard.build-base")));
            }
            Command::none()
        }
        Msg::BuildLine(text) => {
            if let Some(draft) = &mut state.draft {
                draft.log.push(LogLine::new(LogLevel::Info, text));
            }
            Command::none()
        }
        Msg::BuildEnded(result) => {
            let Some(draft) = &mut state.draft else { return Command::none() };
            match result {
                Ok(()) => draft.build = Build::Done,
                Err(problem) => {
                    report(&mut draft.log, &problem);
                    draft.build = if problem == Problem::Cancelled { Build::Stopped } else { Build::Failed(problem) };
                }
            }
            Command::none()
        }
        Msg::BuildCancel => {
            let Some(draft) = &mut state.draft else { return Command::none() };
            let Build::Running(id) = draft.build else { return Command::none() };
            // The build removes the half-made image itself as soon as it notices the stop.
            draft.build = Build::Stopped;
            draft.log.push(LogLine::new(LogLevel::Warn, t!("profiles.wizard.build-stopping")));
            Command::cancel_task(id)
        }
        Msg::LoginStart => start_login(state),
        Msg::LoginOpened { session, container } => {
            let engine = state.engine.clone();
            let Some(draft) = &mut state.draft else {
                // The wizard was closed while the container was coming up; nothing keeps it.
                session.kill();
                return discard(engine, container);
            };
            let watch = session.watch();
            draft.login = Login::Running { session, container };
            Command::batch([Command::perform(move || Msg::LoginEvent(watch.next())), Command::focus("login-terminal")])
        }
        Msg::LoginEvent(event) => login_event(state, event),
        Msg::LoginDone => store_login(state),
        Msg::LoginCancel => cancel_login(state),
        Msg::LoginStored(result) => {
            let Some(draft) = &mut state.draft else { return Command::none() };
            draft.login = match result {
                Ok(stored) => Login::Stored(stored),
                Err(Problem::NoLogin) => Login::Unfinished(Unfinished::Missing),
                Err(problem) => Login::Failed(problem),
            };
            Command::none()
        }
        Msg::LoginFailed(problem) => {
            if let Some(draft) = &mut state.draft {
                draft.login = Login::Failed(problem);
            }
            Command::none()
        }
        Msg::LoginDiscarded => Command::none(),
    }
}

/// Reads the store again, and asks the engine about what it finds.
fn reload(state: &mut Profiles) -> Command<Msg> {
    let providers = load_providers(state);
    let Some(store) = state.store() else {
        state.loading = false;
        return providers;
    };
    state.loading = true;
    Command::batch([
        Command::perform(move || {
            let loaded = store.profiles();
            Msg::Loaded(Listing { profiles: loaded.value, diagnostics: loaded.diagnostics })
        }),
        providers,
    ])
}

/// Reads `providers.toml` again, on a background thread; a file that is not there yet, or that
/// this machine has no data folder for, is simply an empty list.
fn load_providers(state: &Profiles) -> Command<Msg> {
    let path = state.providers_file.clone();
    Command::perform(move || {
        let providers = path.map(|path| Providers::open(&path).value).unwrap_or_else(Providers::in_memory);
        Msg::ProvidersLoaded(providers.entries().to_vec())
    })
}

/// Asks the engine whether each profile has an image and a login.
fn probe(state: &Profiles, profiles: Vec<Profile>) -> Command<Msg> {
    let (Some(engine), false) = (state.engine.clone(), profiles.is_empty()) else { return Command::none() };
    Command::task(Task::new(t!("profiles.probing"), move |_| Ok(Msg::Probed(work::probe(&engine, &profiles)))))
}

/// Asks before removing a login, saying plainly what goes and what stays.
fn ask_sign_out(state: &Profiles) -> Command<Msg> {
    let Some(row) = state.selected() else { return Command::none() };
    if state.engine.is_none() || row.identity != Readiness::Present {
        return Command::none();
    }
    let name = row.profile.name.to_string();
    Command::confirm(
        Confirm::new(t!("profiles.sign-out-title", name = name.as_str()), Msg::SignOutConfirmed)
            .message(t!("profiles.sign-out-message"))
            .confirm_label(t!("profiles.sign-out"))
            .danger(),
    )
}

/// Removes a profile's login.
fn sign_out(state: &mut Profiles) -> Command<Msg> {
    let (Some(engine), Some(row)) = (state.engine.clone(), state.selected()) else { return Command::none() };
    let name = row.profile.name.clone();
    state.signing_out = true;
    Command::task(Task::new(t!("profiles.sign-out"), move |_| Ok(Msg::SignedOut(work::sign_out(&engine, &name)))))
}

/// Goes on a page, or says why it cannot.
fn next(state: &mut Profiles) -> Command<Msg> {
    let Some(draft) = &mut state.draft else { return Command::none() };
    match draft.advance() {
        Some(Blocked::NameEmpty | Blocked::NameTaken) => Command::focus("profile-name"),
        Some(Blocked::NoImage) => Command::focus("build-start"),
        Some(Blocked::NoProvider) => Command::focus("profile-provider"),
        Some(Blocked::Unsupported(_)) => Command::focus("profile-system"),
        None if draft.stage == Stage::Image && draft.build == Build::Waiting => start_build(state),
        None => Command::none(),
    }
}

/// Leaves the wizard. Whatever the engine has already made — the image, the login — stays; what
/// is still running is stopped and cleared away.
fn leave(state: &mut Profiles) -> Command<Msg> {
    let mut commands = Vec::new();
    let engine = state.engine.clone();
    if let Some(draft) = &state.draft {
        if let Build::Running(id) = draft.build {
            commands.push(Command::cancel_task(id));
        }
        if let Login::Running { session, container } = &draft.login {
            session.kill();
            commands.push(discard(engine, Arc::clone(container)));
        }
    }
    state.draft = None;
    // Whatever was made is on disk and in the engine now, so the list is read again rather than
    // guessed at.
    commands.push(reload(state));
    Command::batch(commands)
}

/// Starts the image build.
fn start_build(state: &mut Profiles) -> Command<Msg> {
    let (Some(engine), Some(store)) = (state.engine.clone(), state.store()) else {
        return Command::none();
    };
    let Some(draft) = &mut state.draft else { return Command::none() };
    let Some(profile) = draft.profile() else { return Command::none() };
    if matches!(draft.build, Build::Running(_)) {
        return Command::none();
    }
    draft.log.clear();
    let task = Task::new(t!("profiles.wizard.step-image"), move |cx| {
        let cancel = || cx.is_cancelled();
        let mut line = |text: &str| cx.send(Msg::BuildLine(text.to_owned()));
        // The base image of the profile's own system comes first: a profile on Arch is built on
        // Arch's, and the log is told when that one starts building.
        let built = work::build_whole(&engine, &profile, &cancel, &mut || cx.send(Msg::BaseImageStarted), &mut line);
        // The definition file is written only once there is an image behind it, so a store
        // never holds a profile that cannot be opened.
        let result =
            built.and_then(|()| store.write_profile(&profile).map_err(|problem| Problem::Machine(problem.message)));
        Ok(Msg::BuildEnded(result))
    });
    draft.build = Build::Running(task.id());
    Command::task(task)
}

/// Opens the container the login happens in and starts the harness on a terminal.
fn start_login(state: &mut Profiles) -> Command<Msg> {
    let Some(engine) = state.engine.clone() else { return Command::none() };
    let Some(draft) = &mut state.draft else { return Command::none() };
    let Some(profile) = draft.profile() else { return Command::none() };
    if draft.login.is_busy() {
        return Command::none();
    }
    draft.login = Login::Opening;
    Command::task(Task::new(t!("profiles.wizard.step-login"), move |_| {
        let container = match work::open_login(&engine, &profile) {
            Ok(container) => container,
            Err(problem) => return Ok(Msg::LoginFailed(problem)),
        };
        match work::start_harness(&engine, &profile, &container.name) {
            Ok(session) => Ok(Msg::LoginOpened { session, container: Arc::new(container) }),
            Err(problem) => {
                work::close_login(&engine, &container);
                Ok(Msg::LoginFailed(problem))
            }
        }
    }))
}

/// Keeps watching the login terminal. A harness that closes by itself is not a login: the
/// container is asked whether a login file is there, and only that answers the question.
fn login_event(state: &mut Profiles, event: TerminalEvent) -> Command<Msg> {
    let Some(draft) = &state.draft else { return Command::none() };
    let Login::Running { session, .. } = &draft.login else { return Command::none() };
    match event {
        TerminalEvent::Output => {
            let watch = session.watch();
            Command::perform(move || Msg::LoginEvent(watch.next()))
        }
        TerminalEvent::Exited(_) => store_login(state),
    }
}

/// Looks for the login the person just made and puts it in the profile's volume.
fn store_login(state: &mut Profiles) -> Command<Msg> {
    let Some(engine) = state.engine.clone() else { return Command::none() };
    let Some(draft) = &mut state.draft else { return Command::none() };
    let Login::Running { session, container } = &draft.login else { return Command::none() };
    let Some(profile) = draft.profile() else { return Command::none() };
    session.kill();
    let container = Arc::clone(container);
    draft.login = Login::Storing;
    Command::task(Task::new(t!("profiles.wizard.storing"), move |_| {
        let stored = work::store_login(&engine, &profile, &container);
        work::close_login(&engine, &container);
        Ok(Msg::LoginStored(stored))
    }))
}

/// Stops a login the person no longer wants, leaving no container and no volume behind.
fn cancel_login(state: &mut Profiles) -> Command<Msg> {
    let engine = state.engine.clone();
    let Some(draft) = &mut state.draft else { return Command::none() };
    let Login::Running { session, container } = &draft.login else { return Command::none() };
    session.kill();
    let command = discard(engine, Arc::clone(container));
    draft.login = Login::Unfinished(Unfinished::Stopped);
    command
}

/// Clears away the container a login used.
fn discard(engine: Option<Engine>, container: Arc<work::LoginContainer>) -> Command<Msg> {
    let Some(engine) = engine else { return Command::none() };
    Command::perform(move || {
        work::close_login(&engine, &container);
        Msg::LoginDiscarded
    })
}

/// Puts a failure's own words into the build log, so the engine is quoted rather than summarised.
fn report(log: &mut LogBuffer, problem: &Problem) {
    if let Some(output) = problem.output() {
        for line in output.lines() {
            log.push(LogLine::new(LogLevel::Error, line));
        }
    }
}

/// A toast that says what failed and shows the first line the engine said about it.
fn failure_toast(key: &str, problem: &Problem) -> Toast<Msg> {
    let toast = Toast::danger(t!(key));
    match problem.output().and_then(|output| output.lines().next()) {
        Some(line) => toast.body(line.to_owned()),
        None => toast,
    }
}

/// Says plainly what an engine refusal QCode recognises means and what puts it right, above the
/// engine's own words, which the log or the line below keeps as they were.
fn recognised(state: &Profiles, problem: &Problem, ui: &mut View<'_, Msg>) {
    let kind = state.engine.as_ref().map(Engine::kind);
    if let Some(help) = kind.zip(problem.output()).and_then(|(kind, output)| help(kind, output)) {
        help.show(ui);
    }
}

/// Draws the screen: the list of profiles, or the wizard while one is being made.
pub fn view(state: &Profiles, ui: &mut View<'_, Msg>) {
    match state.draft() {
        Some(draft) => draw_wizard(state, draft, ui),
        None => draw_list(state, ui),
    }
}

/// The control that takes the keyboard when the screen opens, once there is one: the list of
/// profiles. An empty screen has a single button and nothing to walk, so nothing is focused and
/// the button is one Tab away.
#[must_use]
pub fn entry(state: &Profiles) -> Option<&'static str> {
    (!state.rows().is_empty()).then_some("profiles")
}

/// The keys of the profiles screen that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("hints.move")), (icons.glyph("enter").into_owned(), t!("hints.open"))]
}

/// The list of profiles, what the engine says about the chosen one, and the way to a new one.
fn draw_list(state: &Profiles, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    // While a login is being removed the button that asked for it shows the work and takes no
    // second press.
    let busy = state.signing_out;
    let rows = state.rows();
    let selected = state.selected().cloned();
    let problems = state.problems().len();
    let loading = state.is_loading();
    let store = state.root.is_some();

    ui.column(|ui| {
        // Only the title stands above the list: what a profile is for belongs on the empty
        // screen, where it is the answer to a question, and a full list needs its rows instead.
        ui.add(Text::new(t!("profiles.title")).bold());
        if engineless {
            ui.add(Text::new(t!("profiles.no-engine")).color("warning")).fill_width();
        }
        if !store {
            ui.add(Text::new(t!("profiles.no-folder")).color("warning")).fill_width();
            return;
        }
        if problems > 0 {
            ui.add(Text::new(t!("profiles.broken", n = i64::try_from(problems).unwrap_or(i64::MAX))).color("warning"));
        }
        if loading {
            ui.add(Text::new(t!("profiles.reading")).role("secondary"));
            return;
        }
        if rows.is_empty() {
            ui.add(
                EmptyState::new(t!("profiles.empty-title"))
                    .icon("inbox")
                    .message(t!("profiles.empty-message"))
                    .action(Button::new(t!("profiles.new")).variant("primary").on_press(Msg::New)),
            )
            .id("profiles-empty")
            .fill();
            return;
        }

        // The row carries the name and the harness only; everything else about the chosen
        // profile is spelled out under the list, where a narrow screen still has room for it.
        let items = rows
            .iter()
            .map(|row| ListItem::new(row.profile.name.to_string()).detail(row.profile.harness.record().display_name));
        ui.add(List::new(items).selected(Some(state.selected)).on_select(Msg::Select)).id("profiles").fill();

        if let Some(row) = selected {
            let needs_login = row.profile.account.needs_login();
            ui.row(|ui| {
                ui.add(badge(row.image, "profiles.image"));
                if needs_login {
                    ui.add(badge(row.identity, "profiles.identity"));
                } else {
                    // Nothing to ask the engine: a profile that signs in to nothing is as ready
                    // as its image, and the badge says so rather than "not signed in".
                    ui.add(Badge::new(t!("profiles.identity-free")).variant("success"));
                }
            })
            .gap(1);
            ui.add(Text::new(summary(&row.profile)).role("secondary")).fill_width();
            ui.add(Text::new(mounts(&row.profile)).role("secondary")).fill_width();
            let signed_in = row.is_signed_in();
            ui.row(|ui| {
                // Signing in and out mean nothing to a profile without an account, so it is not
                // offered two buttons that could never do anything.
                if needs_login {
                    let mut again = Button::new(t!("profiles.sign-in"));
                    if !engineless && row.image == Readiness::Present {
                        again = again.on_press(Msg::SignInAsked);
                    }
                    ui.add(again.disabled(engineless || row.image != Readiness::Present)).id("profile-sign-in");
                    let mut out = Button::new(t!("profiles.sign-out")).loading(busy);
                    if !engineless && signed_in && !busy {
                        out = out.on_press(Msg::SignOutAsked);
                    }
                    ui.add(out.variant("danger").disabled(engineless || !signed_in)).id("profile-sign-out");
                }
                ui.spacer();
                ui.add(Button::new(t!("profiles.new")).variant("primary").on_press(Msg::New)).id("profile-new");
            })
            .gap(2)
            .fill_width();
        }
    })
    .fill()
    .gap(1);
}

/// The badge of one answer, which says plainly when there is no answer yet.
fn badge(readiness: Readiness, key: &str) -> Badge {
    let (word, variant) = match readiness {
        Readiness::Present => ("ready", "success"),
        Readiness::Missing => ("missing", "warning"),
        Readiness::Unknown => ("unknown", ""),
    };
    let badge = Badge::new(t!(&format!("{key}-{word}")));
    if variant.is_empty() { badge } else { badge.variant(variant) }
}

/// What a profile runs, in one line. The template is named without the recommendation the
/// wizard gives it: that is advice for choosing, and this profile has chosen already.
fn summary(profile: &Profile) -> String {
    let template = t!(match profile.template {
        Template::Base => "profiles.summary-base",
        Template::Recommended => "profiles.summary-recommended",
        Template::High => "profiles.summary-high",
    });
    let account = match &profile.provider {
        Some(provider) => {
            t!("profiles.account-provider-named", tag = provider.tag.as_str(), model = provider.model.as_str())
        }
        None => account_word(profile.account),
    };
    let summary = t!(
        "profiles.summary",
        harness = profile.harness.record().display_name,
        template = template.as_str(),
        account = account.as_str()
    );
    // Debian goes unsaid, as it did before there was a choice; any other system is named, since
    // it is the one thing about the profile the rest of the line does not show.
    if profile.os == Os::Debian {
        summary
    } else {
        t!("profiles.summary-system", summary = summary.as_str(), system = profile.os.display_name())
    }
}

/// What a profile's containers may see and reach, in one line.
fn mounts(profile: &Profile) -> String {
    t!(
        "profiles.mounts",
        assets = access_word(profile.assets).as_str(),
        network = network_word(profile.network).as_str()
    )
}

/// The word for a template.
fn template_word(template: Template) -> String {
    t!(match template {
        Template::Base => "profiles.template-base",
        Template::Recommended => "profiles.template-recommended",
        Template::High => "profiles.template-high",
    })
}

/// The word for an account type.
fn account_word(account: AccountKind) -> String {
    t!(match account {
        AccountKind::Free => "profiles.account-free",
        AccountKind::Subscription => "profiles.account-subscription",
        AccountKind::ApiKey => "profiles.account-api-key",
        AccountKind::InApp => "profiles.account-in-app",
        AccountKind::Provider => "profiles.account-provider",
    })
}

/// Why a profile has no sign-in step, for the two accounts that have none: the harness needs no
/// account at all, or the person signs in inside its own window and QCode never sees it.
fn no_login_detail(account: AccountKind, harness: &str) -> Option<String> {
    match account {
        AccountKind::Free => Some(t!("profiles.wizard.account-free-detail", harness = harness)),
        AccountKind::InApp => Some(t!("profiles.wizard.account-in-app-detail", harness = harness)),
        AccountKind::Subscription | AccountKind::ApiKey | AccountKind::Provider => None,
    }
}

/// The word for a mount access.
fn access_word(access: MountAccess) -> String {
    t!(match access {
        MountAccess::ReadWrite => "profiles.access-rw",
        MountAccess::ReadOnly => "profiles.access-ro",
    })
}

/// The word for a network mode.
fn network_word(mode: NetworkMode) -> String {
    t!(match mode {
        NetworkMode::Full => "profiles.network-full",
        NetworkMode::None => "profiles.network-none",
    })
}

/// The wizard, or the login page on its own when that is all the draft is for, in the middle of
/// the screen.
///
/// The column is centred across, and pushed down by half of what the terminal has beyond the
/// tallest page rather than centred on the page being shown: pages differ in height, and a wizard
/// centred on each one would move its steps up and down on every Next. A terminal too short for
/// the tallest page gets the wizard at the top, where it was, so nothing is pushed off the screen.
fn draw_wizard(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let top = ui.size().height.saturating_sub(WIZARD_ROWS + CHROME_ROWS) / 2;
    ui.column(|ui| {
        ui.spacer().height(Length::Cells(top));
        if draft.is_only_login() {
            draw_sign_in(state, draft, ui);
        } else {
            draw_steps(state, draft, ui);
        }
    })
    .fill()
    .align(Align::Center);
}

/// The login page on its own, for a profile that exists already.
fn draw_sign_in(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.add(Text::new(t!("profiles.wizard.sign-in-title", name = draft.name.as_str())).bold());
        draw_login(state, draft, ui);
        ui.row(|ui| {
            ui.spacer();
            ui.add(Button::new(t!("profiles.wizard.close")).variant("primary").on_press(Msg::Finish)).id("login-close");
        })
        .fill_width();
    })
    .gap(1)
    .width(Length::Cells(PAGE_WIDTH));
}

/// The whole wizard, one page at a time.
fn draw_steps(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let labels = draft.stages().iter().map(|stage| step_word(*stage));
    let wizard = Wizard::new(labels)
        .current(draft.stage.index())
        .busy(draft.is_busy())
        .on_next(Msg::Next)
        .on_back(Msg::Back)
        .on_cancel(Msg::Cancel)
        .on_finish(Msg::Finish)
        .on_step(Msg::Step);
    wizard
        .show(ui, |ui| match draft.stage {
            Stage::Harness => draw_harness(draft, ui),
            Stage::System => draw_system(draft, ui),
            Stage::Template => draw_template(draft, ui),
            Stage::Account => draw_account(draft, ui),
            Stage::Permissions => draw_permissions(draft, ui),
            Stage::Image => draw_image(state, draft, ui),
            Stage::Login => draw_login(state, draft, ui),
        })
        .width(Length::Cells(PAGE_WIDTH));
}

/// The name of one step.
fn step_word(stage: Stage) -> String {
    t!(match stage {
        Stage::Harness => "profiles.wizard.step-harness",
        Stage::System => "profiles.wizard.step-system",
        Stage::Template => "profiles.wizard.step-template",
        Stage::Account => "profiles.wizard.step-account",
        Stage::Permissions => "profiles.wizard.step-permissions",
        Stage::Image => "profiles.wizard.step-image",
        Stage::Login => "profiles.wizard.step-login",
    })
}

/// Which harness, and what the profile is called.
fn draw_harness(draft: &Draft, ui: &mut View<'_, Msg>) {
    let error = match draft.blocked() {
        Some(Blocked::NameEmpty) => Some(t!("profiles.wizard.name-empty")),
        Some(Blocked::NameTaken) => Some(t!("profiles.wizard.name-taken")),
        _ => None,
    };
    let chosen = HarnessKind::ALL.iter().position(|harness| *harness == draft.harness);
    ui.add(Text::new(t!("profiles.wizard.harness-lead")).role("secondary")).fill_width();
    ui.add(
        RadioGroup::new(HarnessKind::ALL.map(|harness| harness.record().display_name.to_owned()))
            .selected(chosen)
            .on_select(Msg::PickHarness),
    )
    .id("profile-harness");
    // A person who went back from the system page to change the harness learns at once that the
    // system they chose there cannot run this one, not only when they reach it again.
    if let Some(refusal) = draft.os.refuses(draft.harness) {
        ui.add(Text::new(refusal_line(refusal, draft)).color("warning")).fill_width();
    }
    ui.add_with(
        Field::new(t!("profiles.wizard.name"))
            .hint(t!("profiles.wizard.name-hint"))
            .error(error.clone())
            .required(true),
        |ui| {
            ui.add(
                TextInput::new(draft.name.clone())
                    .invalid(error.is_some())
                    .max_length(SafeName::MAX_LENGTH)
                    .on_change(Msg::Name),
            )
            .id("profile-name")
            .fill_width();
        },
    )
    .fill_width();
    if let Some(safe) = draft.safe_name()
        && safe.as_str() != draft.name
    {
        ui.add(Text::new(t!("profiles.wizard.name-folded", name = safe.as_str())).role("secondary"));
    }
}

/// Which operating system the image is built on: each with the size its base image was measured
/// at, Alpine with the words that it is not recommended, and under the choice what that system
/// means, what its image does not carry, and — when it cannot run the chosen harness — why, in the
/// colour of a problem, with the page held until something else is chosen.
fn draw_system(draft: &Draft, ui: &mut View<'_, Msg>) {
    let chosen = Os::ALL.iter().position(|os| *os == draft.os);
    ui.add(Text::new(t!("profiles.wizard.system-lead")).role("secondary")).fill_width();
    ui.add(RadioGroup::new(Os::ALL.map(system_option)).selected(chosen).on_select(Msg::PickOs)).id("profile-system");
    let detail = t!(match draft.os {
        Os::Debian => "profiles.wizard.system-debian-detail",
        Os::Arch => "profiles.wizard.system-arch-detail",
        Os::Ubuntu => "profiles.wizard.system-ubuntu-detail",
        Os::Alpine => "profiles.wizard.system-alpine-detail",
    });
    ui.add(Text::new(detail).role("secondary")).fill_width();
    for gap in draft.os.gaps() {
        let said = t!(match gap {
            Gap::Docx2txt => "profiles.wizard.system-gap-docx2txt",
            Gap::Sox => "profiles.wizard.system-gap-sox",
            Gap::SoxOpus => "profiles.wizard.system-gap-sox-opus",
        });
        ui.add(Text::new(said).color("warning")).fill_width();
    }
    if let Some(refusal) = draft.os.refuses(draft.harness) {
        ui.add(Text::new(refusal_line(refusal, draft)).color("danger")).fill_width();
    }
}

/// One system as the wizard offers it: its name, how large its image is, and for Alpine that it is
/// not recommended.
fn system_option(os: Os) -> String {
    let key = if os.recommended() {
        "profiles.wizard.system-option"
    } else {
        "profiles.wizard.system-option-not-recommended"
    };
    t!(key, name = os.display_name(), mb = os.image_mb().to_string())
}

/// Why the chosen system does not run the chosen harness, in words.
fn refusal_line(refusal: Refusal, draft: &Draft) -> String {
    let key = match refusal {
        Refusal::TerminalLibrary => "profiles.wizard.system-refused-terminal",
        Refusal::GlibcProgram => "profiles.wizard.system-refused-glibc",
        Refusal::WindowOnDebianOnly => "profiles.wizard.system-refused-window",
    };
    t!(key, harness = draft.harness.record().display_name, system = draft.os.display_name())
}

/// The harness as it comes, or set up the way QCode runs it.
fn draw_template(draft: &Draft, ui: &mut View<'_, Msg>) {
    let chosen = Template::ALL.iter().position(|template| *template == draft.template);
    ui.add(Text::new(t!("profiles.wizard.template-lead")).role("secondary")).fill_width();
    ui.add(RadioGroup::new(Template::ALL.map(template_word)).selected(chosen).on_select(Msg::PickTemplate))
        .id("profile-template");
    let detail = match draft.template {
        Template::Base => t!("profiles.wizard.template-base-detail"),
        Template::Recommended => t!("profiles.wizard.template-recommended-detail"),
        Template::High => t!("profiles.wizard.template-high-detail"),
    };
    ui.add(Text::new(detail).role("secondary")).fill_width();
    let extras = draft.template.extras(draft.harness);
    if extras.is_empty() {
        ui.add(Text::new(t!("profiles.wizard.unattended")).role("secondary")).fill_width();
        return;
    }
    // What QCode high installs is said where it is chosen, item by item, in the page's own text
    // rather than in a footnote under it: the person is agreeing to a download of hundreds of
    // megabytes and to tools of other makers in their image. Each is on unless switched off here,
    // as the owner approved them: visible, and each one the person's to refuse.
    ui.add(Text::new(t!("profiles.wizard.high-adds")).bold());
    ui.add(Text::new(t!("profiles.wizard.high-choose")).role("secondary")).fill_width();
    SettingsList::show(ui, |list| {
        let mut headed = false;
        for extra in extras {
            // The plugins stand together under one heading, which also gives their size.
            if matches!(extra, Extra::Plugin(_)) && !headed {
                list.heading(t!("profiles.wizard.high-claude-plugins"));
                headed = true;
            }
            let row = match extra {
                Extra::Graphify => SettingRow::new(extra.id()).description(t!("profiles.wizard.high-graphify")),
                Extra::OhMyOpenAgent => SettingRow::new(extra.id()).description(t!("profiles.wizard.high-omo")),
                Extra::Plugin(_) => SettingRow::new(extra.id()),
            };
            list.row(row, |ui| {
                ui.add(Switch::new(draft.has(extra)).on_toggle(move |on| Msg::SwitchExtra(extra, on)));
            });
        }
    })
    .id("profile-extras")
    .fill_width();
    draw_download(draft, ui);
}

/// What the build downloads and what the network means, in the colour of a warning; or, when every
/// part of QCode high is switched off, that nothing more is downloaded.
fn draw_download(draft: &Draft, ui: &mut View<'_, Msg>) {
    if !draft.adds_anything() {
        ui.add(Text::new(t!("profiles.wizard.high-none")).role("secondary")).fill_width();
        return;
    }
    // The system's own repositories are named, since the packages come from there.
    let download = t!("profiles.wizard.high-download", system = draft.os.display_name());
    ui.add(Text::new(download).color("warning")).fill_width();
    if let Some(offline) = offline_line(draft) {
        ui.add(Text::new(offline).color("warning")).fill_width();
    }
}

/// What will not work at run time in a QCode high profile whose containers have no network, when
/// that is the profile being made; `None` otherwise.
///
/// Measured in containers without one: context7 of the Claude Code plugins is a server on
/// `mcp.context7.com`; oh-my-openagent brings three such servers (web search, context7, grep.app),
/// and opencode lists all three as failed. graphify, the other plugins and oh-my-openagent's own
/// agents are in the image and work.
fn offline_line(draft: &Draft) -> Option<String> {
    if draft.network != NetworkMode::None {
        return None;
    }
    // Each line names what stops answering; one whose part was switched off has nothing to say,
    // and the plain line speaks of graphify, so it goes when graphify does.
    let context7 = Extra::parse("context7").is_some_and(|context7| draft.has(context7));
    match draft.harness {
        HarnessKind::ClaudeCode if context7 => Some(t!("profiles.wizard.high-offline-claude")),
        HarnessKind::OpenCode if draft.has(Extra::OhMyOpenAgent) => Some(t!("profiles.wizard.high-offline-opencode")),
        _ if draft.has(Extra::Graphify) => Some(t!("profiles.wizard.high-offline")),
        _ => None,
    }
}

/// What the profile signs in with.
fn draw_account(draft: &Draft, ui: &mut View<'_, Msg>) {
    let offered = draft.harness.record().accounts;
    let chosen = offered.iter().position(|account| *account == draft.account);
    ui.add(Text::new(t!("profiles.wizard.account-lead")).role("secondary")).fill_width();
    ui.add(
        RadioGroup::new(offered.iter().map(|account| account_word(*account)))
            .selected(chosen)
            .on_select(Msg::PickAccount),
    )
    .id("profile-account");
    if let Some(detail) = no_login_detail(draft.account, draft.harness.record().display_name) {
        ui.add(Text::new(detail).role("secondary")).fill_width();
    }
    if draft.account == AccountKind::Provider {
        draw_provider_choice(draft, ui);
    }
}

/// The provider and model a profile of [`AccountKind::Provider`] runs on: the person's own
/// providers first, then the models of whichever one is chosen. Offering nothing but their own
/// providers and that provider's own models is the point — a list borrowed from nowhere else.
fn draw_provider_choice(draft: &Draft, ui: &mut View<'_, Msg>) {
    if draft.providers.is_empty() {
        ui.add(Text::new(t!("profiles.wizard.provider-none")).role("secondary")).fill_width();
        ui.add(Button::new(t!("profiles.wizard.provider-manage")).on_press(Msg::ManageProviders))
            .id("profile-provider-manage");
        return;
    }
    let chosen_tag = draft.providers.iter().position(|entry| Some(entry.tag.as_str()) == draft.provider_tag.as_deref());
    ui.add(Text::new(t!("profiles.wizard.provider-lead")).role("secondary")).fill_width();
    ui.add(
        RadioGroup::new(draft.providers.iter().map(|entry| entry.tag.to_string()))
            .selected(chosen_tag)
            .on_select(Msg::PickProvider),
    )
    .id("profile-provider");
    let models = draft.provider_models();
    if models.is_empty() {
        ui.add(Text::new(t!("profiles.wizard.provider-model-none")).role("secondary")).fill_width();
        ui.add(Button::new(t!("profiles.wizard.provider-manage")).on_press(Msg::ManageProviders))
            .id("profile-provider-model-manage");
        return;
    }
    let chosen_model = models.iter().position(|model| Some(model.id.as_str()) == draft.provider_model.as_deref());
    ui.add(Text::new(t!("profiles.wizard.provider-model-lead")).role("secondary")).fill_width();
    ui.add(
        RadioGroup::new(models.iter().map(|model| model.id.clone()))
            .selected(chosen_model)
            .on_select(Msg::PickProviderModel),
    )
    .id("profile-provider-model");
    // Only a window QCode measured is handed to Claude Code; without one it works to the room of
    // its own models and prints a warning at every start. Measuring is left to the person on the
    // Providers page, because it loads the model on their server.
    let unmeasured = models.iter().find(|model| Some(model.id.as_str()) == draft.provider_model.as_deref());
    if draft.harness == HarnessKind::ClaudeCode
        && let Some(model) = unmeasured.filter(|model| model.measured.is_none())
    {
        let said = t!(
            "profiles.wizard.provider-model-unmeasured",
            model = model.id.as_str(),
            tokens = ASSUMED_CONTEXT_TOKENS.to_string()
        );
        ui.add(Text::new(said).role("secondary")).fill_width();
    }
}

/// What the container may see and reach.
fn draw_permissions(draft: &Draft, ui: &mut View<'_, Msg>) {
    ui.add(Text::new(t!("profiles.wizard.permissions-lead")).role("secondary")).fill_width();
    ui.add(Text::new(t!("profiles.wizard.permissions-code")).bold());
    ui.add(Text::new(t!("profiles.access-rw")).role("secondary"));
    ui.add(Text::new(t!("profiles.wizard.code-why")).role("secondary")).fill_width();
    ui.add(Text::new(t!("profiles.wizard.permissions-assets")).bold());
    ui.add(
        RadioGroup::new(MountAccess::ALL.map(access_word))
            .selected(MountAccess::ALL.iter().position(|access| *access == draft.assets))
            .horizontal(true)
            .on_select(Msg::PickAssets),
    )
    .id("profile-assets");
    ui.add(Text::new(t!("profiles.wizard.permissions-network")).bold());
    ui.add(
        RadioGroup::new(NetworkMode::ALL.map(network_word))
            .selected(NetworkMode::ALL.iter().position(|mode| *mode == draft.network))
            .horizontal(true)
            .on_select(Msg::PickNetwork),
    )
    .id("profile-network");
    // Chosen here, after the template: this is where the person turns the network off, so this
    // is where they learn what of QCode high that costs.
    if let Some(offline) = offline_line(draft) {
        ui.add(Text::new(offline).color("warning")).fill_width();
    }
}

/// Building the image, with everything the engine says as it says it.
fn draw_image(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    let lead = match &draft.build {
        Build::Waiting => t!("profiles.wizard.build-waiting"),
        Build::Running(_) => t!("profiles.wizard.build-running"),
        Build::Done => t!("profiles.wizard.build-done"),
        Build::Stopped => t!("profiles.wizard.build-stopped"),
        Build::Failed(_) if draft.high_download_failed() => t!("profiles.wizard.high-build-failed"),
        Build::Failed(_) => t!("profiles.wizard.build-failed"),
    };
    ui.add(Text::new(lead).role(if matches!(draft.build, Build::Failed(_)) { "danger" } else { "secondary" }))
        .fill_width();
    if let Build::Failed(problem) = &draft.build {
        recognised(state, problem, ui);
    }
    if draft.build == Build::Done && !draft.account.needs_login() {
        let harness = draft.harness.record().display_name;
        let ready = match draft.account {
            AccountKind::InApp => t!("profiles.wizard.in-app-ready", harness = harness),
            _ => t!("profiles.wizard.free-ready", harness = harness),
        };
        ui.add(Text::new(ready).color("success")).fill_width();
    }
    // A window's image carries a whole desktop application, which is an order of magnitude more
    // than a command-line harness; the person is told before the build rather than after.
    if let Some(desktop) = draft.harness.desktop()
        && matches!(draft.build, Build::Waiting | Build::Running(_))
    {
        let size = t!(
            "profiles.wizard.desktop-size",
            harness = draft.harness.record().display_name,
            version = desktop.version,
            mib = desktop.image_mib.to_string()
        );
        ui.add(Text::new(size).role("secondary")).fill_width();
    }
    // Said again right before the build starts, which is when the download happens.
    if draft.adds_anything() && matches!(draft.build, Build::Waiting | Build::Running(_)) {
        draw_download(draft, ui);
    }
    if engineless {
        ui.add(Text::new(t!("profiles.no-engine-detail")).color("warning")).fill_width();
    }
    ui.add(LogView::new(&draft.log).empty_text(t!("profiles.wizard.build-empty")))
        .id("build-log")
        .fill_width()
        .height(Length::Cells(VIEWPORT_ROWS));
    ui.row(|ui| {
        match draft.build {
            Build::Running(_) => {
                ui.add(Button::new(t!("profiles.wizard.build-cancel")).on_press(Msg::BuildCancel)).id("build-cancel");
            }
            Build::Done => {}
            _ => {
                let label = if draft.build == Build::Waiting {
                    t!("profiles.wizard.build-start")
                } else {
                    t!("profiles.wizard.build-again")
                };
                let mut button = Button::new(label).variant("primary").disabled(engineless);
                if !engineless {
                    button = button.on_press(Msg::BuildStart);
                }
                ui.add(button).id("build-start");
            }
        }
        ui.spacer();
    })
    .gap(2)
    .fill_width();
}

/// Signing in: why a terminal opens, what to do in it, and how QCode decides it worked.
fn draw_login(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    let harness = draft.harness.record().display_name;
    match &draft.login {
        Login::Waiting => {
            ui.add(Text::new(t!("profiles.wizard.login-why", harness = harness)).role("secondary")).fill_width();
            ui.add(Text::new(t!("profiles.wizard.login-what")).role("secondary")).fill_width();
            ui.add(Text::new(t!("profiles.wizard.login-proof")).role("secondary")).fill_width();
            if engineless {
                ui.add(Text::new(t!("profiles.no-engine-detail")).color("warning")).fill_width();
            }
            let mut button = Button::new(t!("profiles.wizard.login-open")).variant("primary").disabled(engineless);
            if !engineless {
                button = button.on_press(Msg::LoginStart);
            }
            ui.add(button).id("login-open");
        }
        Login::Opening => {
            ui.add(Text::new(t!("profiles.wizard.login-opening")).role("secondary"));
        }
        Login::Running { session, .. } => {
            ui.add(Text::new(t!("profiles.wizard.login-running", harness = harness)).role("secondary")).fill_width();
            ui.add(Terminal::new(session)).id("login-terminal").fill_width().height(Length::Cells(VIEWPORT_ROWS));
            ui.row(|ui| {
                ui.add(Button::new(t!("profiles.wizard.login-done")).variant("primary").on_press(Msg::LoginDone))
                    .id("login-done");
                ui.add(Button::new(t!("profiles.wizard.login-stop")).on_press(Msg::LoginCancel)).id("login-stop");
                ui.spacer();
            })
            .gap(2)
            .fill_width();
        }
        Login::Storing => {
            ui.add(Text::new(t!("profiles.wizard.login-storing")).role("secondary"));
        }
        Login::Stored(files) => {
            ui.add(
                Text::new(t!("profiles.wizard.login-stored", n = i64::try_from(*files).unwrap_or(i64::MAX)))
                    .color("success"),
            )
            .fill_width();
            ui.add(Text::new(t!("profiles.wizard.login-stored-detail")).role("secondary")).fill_width();
        }
        Login::Unfinished(why) => {
            let text = match why {
                Unfinished::Stopped => t!("profiles.wizard.login-interrupted"),
                Unfinished::Missing => t!("profiles.wizard.login-not-found"),
            };
            ui.add(Text::new(text).color("warning")).fill_width();
            ui.add(Text::new(t!("profiles.wizard.login-later")).role("secondary")).fill_width();
            ui.add(Button::new(t!("profiles.wizard.login-again")).on_press(Msg::LoginStart)).id("login-again");
        }
        Login::Failed(problem) => {
            ui.add(Text::new(t!("profiles.wizard.login-failed")).color("danger")).fill_width();
            recognised(state, problem, ui);
            if let Some(output) = problem.output() {
                ui.add(Text::new(output.to_owned()).role("secondary")).fill_width();
            }
            ui.add(Text::new(t!("profiles.wizard.login-later")).role("secondary")).fill_width();
            ui.add(Button::new(t!("profiles.wizard.login-again")).on_press(Msg::LoginStart)).id("login-again");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qframe::env::{AssetDirs, Env};
    use qframe::icons::GlyphMode;
    use qframe::runtime::Harness;

    /// A terminal wide enough for the list beside its detail and for a wizard page.
    const SIZE: (u16, u16) = (96, 34);

    /// The screen on its own, so that a test drives exactly this module and nothing else. The
    /// application's message is this module's message, which is what wiring the screen into
    /// QCode does with one more layer around it.
    struct Host {
        state: Profiles,
    }

    impl App for Host {
        type Msg = Msg;

        fn update(&mut self, message: Msg) -> Command<Msg> {
            update(&mut self.state, message)
        }

        fn view(&self, ui: &mut View<'_, Msg>) {
            AppShell::new().body(|ui| view(&self.state, ui)).show(ui);
        }
    }

    fn env() -> Env {
        Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() })
            .expect("the built-in files load")
    }

    fn profile(name: &str, harness: HarnessKind) -> Profile {
        Profile {
            name: SafeName::parse(name).expect("the name is safe"),
            harness,
            template: Template::Recommended,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        }
    }

    /// A store path that is never read: the tests deliver what the disk would have given,
    /// so nothing here touches a file system or an engine.
    fn root() -> PathBuf {
        std::env::temp_dir().join("qcode-profiles-screen")
    }

    /// A screen with a store and no engine, on a terminal of `width` by `height`.
    fn screen(width: u16, height: u16) -> Harness<Host> {
        let state = Profiles::new(Some(root()), None).with_providers_file(None);
        let mut harness = Harness::with_env(Host { state }, env(), width, height);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        harness.render();
        harness
    }

    /// A screen with a store and an engine that is never actually run: every answer the
    /// engine would give is delivered as a message instead.
    fn with_engine() -> Harness<Host> {
        // A binary that is not there: an engine QCode holds but never gets an answer out of, so
        // the test says what the answers are instead of a machine happening to have podman.
        let engine = Engine::new(crate::engine::EngineKind::Podman, "/qcode/no/such/engine");
        let state = Profiles::new(Some(root()), Some(engine)).with_providers_file(None);
        let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, SIZE.1);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        harness.render();
        harness
    }

    /// A screen holding `profiles`, delivered the way the store would deliver them.
    fn loaded(profiles: Vec<Profile>) -> Harness<Host> {
        let mut harness = screen(SIZE.0, SIZE.1);
        harness.send(Msg::Loaded(Listing { profiles, diagnostics: Vec::new() })).render();
        harness
    }

    /// The engine's answer about every profile on screen.
    fn answer(harness: &mut Harness<Host>, image: Readiness, identity: Readiness) {
        let answers: Vec<Status> = harness
            .app()
            .state
            .rows()
            .iter()
            .map(|row| Status { name: row.profile.name.clone(), image, identity })
            .collect();
        harness.send(Msg::Probed(answers)).render();
    }

    #[test]
    fn a_store_without_profiles_starts_empty() {
        let state = Profiles::new(None, None);
        assert!(state.rows().is_empty());
        assert!(!state.is_loading(), "without a store there is nothing to wait for");
    }

    #[test]
    fn an_empty_store_says_that_nothing_runs_without_a_profile() {
        let harness = loaded(Vec::new());
        let screen = harness.screen();
        assert!(screen.contains("No profiles yet"), "{screen}");
        assert!(screen.contains("harness through a profile"), "{screen}");
        assert!(screen.contains("New profile"), "the way out is offered:\n{screen}");
    }

    #[test]
    fn a_store_being_read_says_so_instead_of_looking_empty() {
        let root = std::env::temp_dir().join("qcode-profiles-loading");
        let state = Profiles::new(Some(root), None);
        assert!(state.is_loading());
        let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, SIZE.1);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).render();
        let screen = harness.screen();
        assert!(screen.contains("Reading the profiles"), "{screen}");
        assert!(!screen.contains("No profiles yet"), "a store being read is not an empty one:\n{screen}");
    }

    #[test]
    fn a_broken_definition_file_is_counted_on_the_screen() {
        let mut harness = screen(SIZE.0, SIZE.1);
        let diagnostics = vec![qframe::diagnostics::Diagnostic::error(
            Some(qframe::diagnostics::Location { file: "half.toml".to_owned(), line: 1, column: 1 }),
            "`name` is missing",
        )];
        harness
            .send(Msg::Loaded(Listing { profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)], diagnostics }));
        harness.render();
        let screen = harness.screen();
        assert!(screen.contains("1 profile file could not be read"), "{screen}");
        assert!(screen.contains("claude-sub"), "the readable profile is still listed:\n{screen}");
    }

    #[test]
    fn before_the_engine_answers_nothing_claims_to_be_ready() {
        let harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        let screen = harness.screen();
        assert!(screen.contains("Image not checked"), "{screen}");
        assert!(screen.contains("Login not checked"), "{screen}");
        assert!(!screen.contains("Signed in"), "nothing is signed in until the engine says so:\n{screen}");
    }

    #[test]
    fn the_engines_answer_becomes_the_two_badges() {
        let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        answer(&mut harness, Readiness::Present, Readiness::Missing);
        let screen = harness.screen();
        assert!(screen.contains("Image ready"), "{screen}");
        assert!(screen.contains("Not signed in"), "{screen}");
        answer(&mut harness, Readiness::Present, Readiness::Present);
        assert!(harness.screen().contains("Signed in"), "{}", harness.screen());
    }

    #[test]
    fn a_profile_on_another_system_names_it_in_the_list_and_one_on_debian_does_not() {
        let arch = Profile { os: Os::Arch, ..profile("claude-arch", HarnessKind::ClaudeCode) };
        let harness = loaded(vec![arch, profile("claude-sub", HarnessKind::ClaudeCode)]);
        let screen = harness.screen();
        assert!(screen.contains("Claude Code, QCode basic, subscription, on Arch Linux"), "{screen}");
        assert!(!screen.contains("Debian"), "a profile on Debian reads as it did before:\n{screen}");
    }

    #[test]
    fn a_profile_summary_says_what_it_runs_and_what_it_may_reach() {
        let harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        let screen = harness.screen();
        assert!(screen.contains("Claude Code"), "{screen}");
        assert!(screen.contains("Claude Code, QCode basic, subscription"), "{screen}");
        assert!(screen.contains("subscription"), "{screen}");
        assert!(screen.contains("assets read-only"), "{screen}");
        assert!(screen.contains("network full"), "{screen}");
    }

    #[test]
    fn without_an_engine_the_screen_says_what_it_cannot_do_and_the_buttons_are_dead() {
        let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        answer(&mut harness, Readiness::Present, Readiness::Present);
        let screen = harness.screen();
        assert!(screen.contains("No container engine was found"), "{screen}");
        assert!(screen.contains("signed in or signed out"), "the reason stands beside the dead buttons:\n{screen}");
        harness.click_text("Sign out").render();
        assert!(!harness.screen().contains("Sign claude-sub out?"), "a dead button asks nothing:\n{screen}");
    }

    #[test]
    fn signing_out_is_asked_first_and_says_exactly_what_goes() {
        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }));
        answer(&mut harness, Readiness::Present, Readiness::Present);
        harness.send(Msg::SignOutAsked).render();
        let screen = harness.screen();
        assert!(screen.contains("Sign claude-sub out?"), "{screen}");
        assert!(screen.contains("Workspaces that already have a copy keep working"), "{screen}");
        assert!(screen.contains("new workspaces get no login"), "{screen}");
    }

    #[test]
    fn a_profile_that_is_not_signed_in_cannot_be_signed_out() {
        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }));
        answer(&mut harness, Readiness::Present, Readiness::Missing);
        harness.send(Msg::SignOutAsked).render();
        assert!(!harness.screen().contains("Sign claude-sub out?"), "{}", harness.screen());
    }

    #[test]
    fn the_wizard_walks_through_its_seven_pages_in_order() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New).render();
        let screen = harness.screen();
        assert!(screen.contains("Harness"), "the first step names itself:\n{screen}");
        assert!(screen.contains("Claude Code") && screen.contains("Codex"), "{screen}");
        assert!(screen.contains("Name"), "{screen}");
        for expected in [
            "The operating system this profile",
            "How much of the harness",
            "What this profile signs in with",
            "may see and reach",
            "Build the image",
        ] {
            harness.send(Msg::Next).render();
            assert!(harness.screen().contains(expected), "`{expected}` is missing:\n{}", harness.screen());
        }
        harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next).render();
        assert!(harness.screen().contains("Open the sign-in terminal"), "{}", harness.screen());
        assert_eq!(harness.app().state.draft().expect("the wizard is open").stage, Stage::Login);
    }

    /// A provider entry with `models` already known, the way the Providers page leaves it once
    /// someone has listed them.
    fn provider_entry(tag: &str, models: &[&str]) -> ProviderEntry {
        let mut entry = ProviderEntry::new(
            crate::provider::Tag::parse(tag).expect("a tag"),
            crate::provider::ProviderKind::Ollama,
            "http://127.0.0.1:11434",
        );
        entry.models = models.iter().map(|id| crate::provider::Model::new(*id)).collect();
        entry
    }

    #[test]
    fn pressing_the_providers_own_row_and_its_models_row_makes_a_profile_that_runs_on_it() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::ProvidersLoaded(vec![provider_entry("ev1", &["qwen3.8", "qwen3.8-32k"])]));
        harness.send(Msg::New).render();
        // Claude Code is the harness already offered first; System, Template and Account are next.
        harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
        assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
        // Pressed exactly where the person would: the row that names the account, then the rows
        // that name the provider and its model, never the message sent by hand.
        harness.click_text("a provider of your own").render();
        assert!(harness.screen().contains("ev1"), "the person's own provider is offered:\n{}", harness.screen());
        harness.click_text("ev1").render();
        assert!(harness.screen().contains("qwen3.8-32k"), "its models are offered:\n{}", harness.screen());
        harness.click_text("qwen3.8-32k").render();

        let draft = harness.app().state.draft().expect("the wizard is open");
        assert_eq!(draft.provider_tag.as_deref(), Some("ev1"));
        assert_eq!(draft.provider_model.as_deref(), Some("qwen3.8-32k"));
        let profile = draft.profile().expect("a provider and a model are both chosen");
        let provider = profile.provider.expect("a provider profile carries one");
        assert_eq!(provider.tag, "ev1");
        assert_eq!(provider.model, "qwen3.8-32k");
    }

    #[test]
    fn a_model_whose_window_was_never_measured_says_what_claude_code_will_assume() {
        let mut entry = provider_entry("ev1", &["qwen3.8", "qwen3.8-32k"]);
        entry.models[1].measured = Some(crate::provider::Measured::About(31_512));
        let mut harness = loaded(Vec::new());
        // What the providers file holds, delivered the way the screen reads it.
        harness.send(Msg::ProvidersLoaded(vec![entry])).render();
        harness.click_text("New profile").render();
        next(&mut harness);
        next(&mut harness);
        next(&mut harness);
        harness.click_text("a provider of your own").render();
        harness.click_text("ev1").render();
        // Read across the rows the sentence wraps onto, the way a person reads it.
        let read = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
        let said = "Claude Code will assume qwen3.8 has room for 200000 tokens until its window is measured on the \
                    Providers page.";
        assert!(read(&harness).contains(said), "the first model was never measured:\n{}", harness.screen());
        harness.click_text("qwen3.8-32k").render();
        let screen = harness.screen();
        assert!(!screen.contains("will assume"), "a measured window is handed over, so nothing is said:\n{screen}");
        harness.click_text("qwen3.8").render();
        harness.set_locale("tr").render();
        assert!(read(&harness).contains("Claude Code, qwen3.8 için 200000 belirteçlik"), "{}", harness.screen());
    }

    #[test]
    fn with_no_provider_added_the_account_page_says_so_and_offers_no_empty_list() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New).render();
        harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
        harness.click_text("a provider of your own").render();
        let screen = harness.screen();
        assert!(screen.contains("You have not added a provider yet"), "{screen}");
        assert!(!screen.contains("qwen"), "nothing borrowed from nowhere is offered:\n{screen}");
        harness.click_text("Go to Providers").render();
        // The application, not this screen, opens the Providers page; this screen only asked.
        assert!(harness.app().state.draft().is_some(), "the wizard itself is not closed by asking");
    }

    #[test]
    fn opencode_offers_a_provider_of_ones_own_from_the_rows_a_person_presses() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::ProvidersLoaded(vec![provider_entry("ev1", &["qwen3.8", "qwen3-coder:30b"])])).render();
        harness.click_text("New profile").render();
        harness.click_text("opencode").render();
        assert_eq!(harness.app().state.draft().expect("open").harness, HarnessKind::OpenCode);
        next(&mut harness);
        next(&mut harness);
        next(&mut harness);
        assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
        harness.click_text("a provider of your own").render();
        harness.click_text("ev1").render();
        harness.click_text("qwen3-coder:30b").render();
        // What Claude Code would assume is Claude Code's alone and is not said of opencode.
        assert!(!harness.screen().contains("will assume"), "{}", harness.screen());
        let profile = harness.app().state.draft().expect("open").profile().expect("a provider and a model");
        assert_eq!(profile.harness, HarnessKind::OpenCode);
        assert_eq!(profile.account, AccountKind::Provider);
        let provider = profile.provider.expect("a provider profile carries one");
        assert_eq!((provider.tag.as_str(), provider.model.as_str()), ("ev1", "qwen3-coder:30b"));
    }

    #[test]
    fn codex_and_gemini_cli_offer_no_provider_of_ones_own() {
        for name in ["Codex", "Gemini CLI"] {
            let mut harness = loaded(Vec::new());
            harness.click_text("New profile").render();
            harness.click_text(name).render();
            next(&mut harness);
            next(&mut harness);
            next(&mut harness);
            assert!(harness.screen().contains("What this profile signs in with"), "{name}: {}", harness.screen());
            assert!(!harness.screen().contains("a provider of your own"), "{name}: {}", harness.screen());
        }
    }

    #[test]
    fn a_name_that_is_taken_stops_the_wizard_on_the_first_page() {
        let mut harness = loaded(vec![profile("claude-code", HarnessKind::ClaudeCode)]);
        harness.send(Msg::New).send(Msg::Next).render();
        let screen = harness.screen();
        assert!(screen.contains("There is already a profile with this name"), "{screen}");
        assert!(harness.is_focused("profile-name"), "the focus goes to the problem:\n{screen}");
        harness.send(Msg::Name("Günlük İş".to_owned())).render();
        assert!(harness.screen().contains("It is written as gunluk-is"), "{}", harness.screen());
        harness.send(Msg::Next).render();
        assert!(harness.screen().contains("The operating system this profile"), "{}", harness.screen());
    }

    #[test]
    fn the_permissions_page_says_the_workspace_is_always_writable() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..4 {
            harness.send(Msg::Next);
        }
        harness.render();
        let screen = harness.screen();
        assert!(screen.contains("always writable"), "{screen}");
        assert!(screen.contains("read-only") && screen.contains("writable"), "{screen}");
        assert!(screen.contains("full") && screen.contains("none"), "{screen}");
    }

    /// Clicks the wizard's `Next` button, the way a person moves on a page.
    fn next(harness: &mut Harness<Host>) {
        harness.click_text("Next").render();
    }

    #[test]
    fn the_workspace_folder_is_called_work_in_the_list_and_on_the_permissions_page() {
        // The folder is `Work/` on disk and `/work` in the container; a screen that still calls
        // it Code sends the person looking for a folder that is not there.
        let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        let screen = harness.screen();
        assert!(screen.contains("Work writable"), "the list names the folder:\n{screen}");
        assert!(!screen.contains("Code writable"), "{screen}");
        harness.click_text("New profile").render();
        for _ in 0..4 {
            next(&mut harness);
        }
        let screen = harness.screen();
        assert!(screen.contains("may see and reach"), "the permissions page is open:\n{screen}");
        assert!(screen.contains("The Work folder, /work in the container"), "{screen}");
        assert!(!screen.contains("code directory"), "{screen}");
        harness.set_locale("tr").render();
        let screen = harness.screen();
        assert!(screen.contains("Work klasörü, kapsayıcıda /work"), "{screen}");
        assert!(!screen.contains("kod klasörü"), "{screen}");
    }

    #[test]
    fn the_image_page_cannot_be_left_until_there_is_an_image() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        harness.render();
        assert!(harness.screen().contains("Build the image"), "{}", harness.screen());
        harness.send(Msg::Next).render();
        assert!(harness.screen().contains("Build the image"), "a page without an image keeps its place");
        harness.send(Msg::BuildEnded(Ok(()))).render();
        assert!(harness.screen().contains("written to the QCode folder"), "{}", harness.screen());
        harness.send(Msg::Next).render();
        assert!(harness.screen().contains("signs in with its own flow"), "{}", harness.screen());
    }

    #[test]
    fn a_build_refused_for_a_reason_qcode_recognises_says_it_plainly_above_the_engines_words() {
        let mut harness = with_engine();
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        let said = "Error: cannot find UID/GID for user ada: no subuid ranges found for user \"ada\" in /etc/subuid";
        harness.send(Msg::BuildEnded(Err(Problem::Refused(said.to_owned())))).render();
        let screen = harness.screen();
        assert!(screen.contains("The build failed"), "{screen}");
        assert!(screen.contains("no user id ranges for your account"), "{screen}");
        assert!(screen.contains("--add-subuids"), "the line that adds them is there to copy:\n{screen}");
        assert!(screen.contains("no subuid ranges found"), "the engine is still quoted:\n{screen}");
    }

    #[test]
    fn a_failed_build_shows_the_engines_own_words_and_leaves_no_image() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        harness.send(Msg::BuildLine("STEP 1/3: FROM qcode/base".to_owned()));
        harness.send(Msg::BuildEnded(Err(Problem::Refused("Error: qcode/base: image not known".to_owned()))));
        harness.render();
        let screen = harness.screen();
        assert!(screen.contains("The build failed"), "{screen}");
        assert!(screen.contains("image not known"), "the engine is quoted, not summarised:\n{screen}");
        assert!(screen.contains("nothing was left behind"), "{screen}");
        assert!(screen.contains("Build again"), "{screen}");
        harness.send(Msg::Next).render();
        assert!(harness.screen().contains("Build again"), "a failed build is not an image");
    }

    #[test]
    fn a_stopped_build_says_that_nothing_was_written() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        harness.send(Msg::BuildEnded(Err(Problem::Cancelled))).render();
        let screen = harness.screen();
        assert!(screen.contains("The build was stopped"), "{screen}");
        assert!(screen.contains("Nothing was written"), "{screen}");
    }

    #[test]
    fn the_login_page_says_why_a_terminal_opens_what_to_do_and_how_it_is_checked() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next).render();
        let screen = harness.screen();
        assert!(screen.contains("Claude Code signs in with its own flow"), "why:\n{screen}");
        assert!(screen.contains("Sign in in the terminal"), "what to do:\n{screen}");
        assert!(screen.contains("does not take your word for it"), "how it is checked:\n{screen}");
        assert!(screen.contains("Open the sign-in terminal"), "{screen}");
    }

    #[test]
    fn a_harness_that_closed_without_a_login_is_never_counted_as_signed_in() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next);
        harness.send(Msg::LoginStored(Err(Problem::NoLogin))).render();
        let screen = harness.screen();
        assert!(screen.contains("closed without leaving a login"), "{screen}");
        assert!(screen.contains("nothing was stored"), "{screen}");
        assert!(screen.contains("Sign in from the list whenever you like"), "the way back is offered:\n{screen}");
        let draft = harness.app().state.draft().expect("the wizard is open");
        assert!(!draft.login.is_stored());
    }

    #[test]
    fn an_interrupted_login_says_that_nothing_was_kept() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next);
        // The page reached by stopping a sign-in that was under way.
        harness.send(Msg::LoginFailed(Problem::Refused("Error: no such container".to_owned()))).render();
        let screen = harness.screen();
        assert!(screen.contains("The sign-in could not be opened"), "{screen}");
        assert!(screen.contains("no such container"), "{screen}");
        assert!(screen.contains("Try again"), "{screen}");
    }

    #[test]
    fn a_stored_login_says_how_much_was_kept_and_what_happens_to_it() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New);
        for _ in 0..5 {
            harness.send(Msg::Next);
        }
        harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next);
        harness.send(Msg::LoginStored(Ok(1))).render();
        let screen = harness.screen();
        assert!(screen.contains("Signed in. 1 file was stored"), "{screen}");
        assert!(screen.contains("gets its own copy"), "{screen}");
    }

    #[test]
    fn a_profile_with_an_image_can_be_signed_in_again_from_the_list() {
        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }));
        answer(&mut harness, Readiness::Present, Readiness::Missing);
        harness.send(Msg::SignInAsked).render();
        let screen = harness.screen();
        assert!(screen.contains("Sign claude-sub in"), "{screen}");
        assert!(screen.contains("Open the sign-in terminal"), "{screen}");
        assert!(!screen.contains("Permissions"), "signing in again is not the whole wizard:\n{screen}");
    }

    #[test]
    fn a_profile_without_an_image_cannot_be_signed_in() {
        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }));
        answer(&mut harness, Readiness::Missing, Readiness::Missing);
        harness.send(Msg::SignInAsked).render();
        assert!(!harness.screen().contains("Sign claude-sub in"), "{}", harness.screen());
    }

    #[test]
    fn the_keyboard_reaches_the_list_and_moves_the_selection() {
        let mut harness =
            loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("codex-key", HarnessKind::Codex)]);
        harness.press("tab");
        assert!(harness.is_focused("profiles"), "the list takes the first focus:\n{}", harness.screen());
        harness.press("down").render();
        let selected = harness.app().state.selected().expect("a profile is chosen");
        assert_eq!(selected.profile.name.as_str(), "codex-key");
        assert!(harness.screen().contains("Codex"), "{}", harness.screen());
    }

    #[test]
    fn the_pointer_chooses_a_profile_and_hovering_changes_nothing_else() {
        let mut harness =
            loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("codex-key", HarnessKind::Codex)]);
        let (x, y) = harness.find("codex-key").expect("the second profile is on screen");
        harness.hover(x, y).render();
        assert_eq!(
            harness.app().state.selected().expect("a profile is chosen").profile.name.as_str(),
            "claude-sub",
            "hovering shows, it does not choose"
        );
        harness.click(x, y).render();
        assert_eq!(harness.app().state.selected().expect("a profile is chosen").profile.name.as_str(), "codex-key");
    }

    #[test]
    fn a_narrow_screen_keeps_the_profiles_and_their_state() {
        let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        harness.resize(34, 18).render();
        let screen = harness.screen();
        assert!(screen.contains("claude-sub"), "{screen}");
        assert!(screen.contains("Sign out"), "{screen}");
    }

    #[test]
    fn the_wizard_still_works_in_a_narrow_terminal() {
        let mut harness = loaded(Vec::new());
        harness.send(Msg::New).render();
        harness.resize(40, 20).render();
        let screen = harness.screen();
        assert!(screen.contains("Claude Code"), "{screen}");
        harness.send(Msg::Next).render();
        assert!(harness.screen().contains("Debian 13"), "{}", harness.screen());
        harness.send(Msg::Next).render();
        assert!(harness.screen().contains("recommended"), "{}", harness.screen());
    }

    #[test]
    fn ascii_mode_draws_nothing_but_ascii() {
        let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        harness.set_glyph_mode(GlyphMode::Ascii).render();
        let screen = harness.screen();
        assert!(screen.is_ascii(), "{screen}");
        assert!(screen.contains("claude-sub"), "{screen}");
    }

    #[test]
    fn nothing_is_bracketed_lined_or_framed() {
        let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
            harness.set_glyph_mode(mode).render();
            let screen = harness.screen();
            for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│', '+'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` in {mode:?}:\n{screen}");
            }
            assert!(!screen.contains("=="), "{screen}");
            assert!(!screen.contains("->"), "{screen}");
        }
    }

    #[test]
    fn reduced_motion_keeps_the_wizard_working() {
        let mut harness = loaded(Vec::new());
        harness.set_reduced_motion(true).send(Msg::New).render();
        harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
        assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
    }

    #[test]
    fn turkish_reads_as_turkish() {
        let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        harness.set_locale("tr").render();
        let screen = harness.screen();
        for text in ["Profiller", "QCode basic", "Giriş yap", "Çıkış yap", "abonelik"] {
            assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
        }
        harness.send(Msg::New).render();
        let screen = harness.screen();
        assert!(screen.contains("Düzenek"), "{screen}");
        assert!(screen.contains("kodlama ajanı"), "{screen}");
    }

    /// The column of `label` on the row where it follows the small square of a radio group, and
    /// that row, so a test can look at the mark two cells before it.
    fn radio_mark(harness: &Harness<Host>, label: &str) -> (u16, u16) {
        let square = harness.env().icons().glyph("radio-mark-small").into_owned();
        let screen = harness.screen();
        let (y, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(&format!("{square}  {label}")))
            .unwrap_or_else(|| panic!("`{label}` is offered with the small square:\n{screen}"));
        let at = line.find(&format!("{square}  {label}")).expect("the option is on the row");
        let x = line[..at].chars().count();
        (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"))
    }

    /// A wizard opened on a new profile and walked `pages` pages on.
    fn wizard_on(pages: usize) -> Harness<Host> {
        let mut harness = loaded(Vec::new());
        harness.set_reduced_motion(true).send(Msg::New);
        for _ in 0..pages {
            harness.send(Msg::Next);
        }
        harness.render();
        harness
    }

    #[test]
    fn the_wizard_stands_in_the_middle_of_a_wide_screen() {
        let mut harness = wizard_on(0);
        harness.resize(160, 50).render();
        let (x, y) = harness.find("Harness").expect("the first step is named");
        assert!(x >= 25, "the wizard is centred across, not glued to the left edge:\n{}", harness.screen());
        assert!(y >= 8, "the wizard is pushed down from the top:\n{}", harness.screen());
        let first = (x, y);
        harness.send(Msg::Next).render();
        assert_eq!(harness.find("Harness"), Some(first), "the steps stay where they were from page to page");

        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }));
        answer(&mut harness, Readiness::Present, Readiness::Missing);
        harness.send(Msg::SignInAsked).resize(160, 50).render();
        let (x, y) = harness.find("Sign claude-sub in").expect("the sign-in page is open");
        assert!(x >= 25 && y >= 8, "the sign-in page is centred too:\n{}", harness.screen());
    }

    #[test]
    fn a_short_terminal_keeps_the_wizard_at_the_top() {
        let mut harness = wizard_on(5);
        harness.resize(SIZE.0, 24).render();
        // Seven steps do not keep their names in a terminal this narrow, so the row of steps shows
        // their markers and the current step's name.
        let (_, y) = harness.find("Image").expect("the current step is named");
        assert!(y <= 1, "no room to spare means no rows given away:\n{}", harness.screen());
    }

    #[test]
    fn the_tallest_page_fits_the_rows_the_wizard_is_placed_by() {
        // The image page without an engine is the tallest; if it grows past the constant the
        // wizard would be placed too low and its buttons pushed towards the bottom edge.
        let mut harness = wizard_on(5);
        harness.resize(PAGE_WIDTH, 60).render();
        let (_, top) = harness.find("Harness").expect("the steps are drawn");
        let (_, bottom) = harness.find("Cancel").expect("the buttons are drawn");
        let rows = u16::try_from(bottom - top + 1).expect("the buttons are under the steps");
        assert!(rows <= WIZARD_ROWS, "{rows} rows:\n{}", harness.screen());
    }

    #[test]
    fn every_choice_in_the_wizard_is_marked_with_the_small_square() {
        let harness = wizard_on(0);
        let square = harness.env().icons().glyph("radio-mark-small").into_owned();
        // The square marks every option, the chosen one included; the style that grows the
        // chosen one into a full box would leave one short.
        assert_eq!(harness.screen().matches(square.as_str()).count(), HarnessKind::ALL.len(), "{}", harness.screen());
        let chosen = radio_mark(&harness, "Claude Code");
        let other = radio_mark(&harness, "Codex");
        assert_ne!(harness.fg(chosen.0, chosen.1), harness.fg(other.0, other.1), "{}", harness.screen());
        let harness = wizard_on(1);
        assert_eq!(harness.screen().matches(square.as_str()).count(), Os::ALL.len(), "{}", harness.screen());
        let harness = wizard_on(2);
        assert_eq!(harness.screen().matches(square.as_str()).count(), Template::ALL.len(), "{}", harness.screen());
        let harness = wizard_on(4);
        let options = MountAccess::ALL.len() + NetworkMode::ALL.len();
        assert_eq!(harness.screen().matches(square.as_str()).count(), options, "{}", harness.screen());
    }

    #[test]
    fn the_assets_come_writable_and_chosen_from_the_first_frame() {
        let harness = wizard_on(4);
        assert_eq!(harness.app().state.draft().expect("the wizard is open").assets, MountAccess::ReadWrite);
        let writable = radio_mark(&harness, "writable");
        let read_only = radio_mark(&harness, "read-only");
        let full = radio_mark(&harness, "full");
        let tone = |(x, y): (u16, u16)| harness.fg(x, y);
        assert_eq!(tone(writable), tone(full), "writable wears the chosen tone:\n{}", harness.screen());
        assert_ne!(tone(writable), tone(read_only), "{}", harness.screen());
    }

    #[test]
    fn the_qcode_templates_carry_the_names_the_owner_chose() {
        let harness = wizard_on(2);
        assert!(harness.screen().contains("QCode basic (recommended)"), "{}", harness.screen());
        assert!(harness.screen().contains("QCode high"), "{}", harness.screen());
        assert_eq!(harness.app().state.draft().expect("open").template, Template::Recommended, "basic is chosen");
        let mut harness = wizard_on(2);
        harness.set_locale("tr").render();
        assert!(harness.screen().contains("QCode basic (önerilen)"), "{}", harness.screen());
        assert!(harness.screen().contains("QCode high"), "{}", harness.screen());
    }

    #[test]
    fn opencode_is_offered_free_first_and_finishes_without_a_sign_in() {
        let mut harness = wizard_on(0);
        let opencode = HarnessKind::ALL.iter().position(|harness| *harness == HarnessKind::OpenCode);
        harness.send(Msg::PickHarness(opencode.expect("opencode is offered")));
        harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
        let screen = harness.screen();
        assert!(screen.contains("free, no account"), "{screen}");
        assert!(screen.contains("nothing to sign in to"), "{screen}");
        assert!(!screen.contains("Sign in"), "the steps leave the sign-in out:\n{screen}");
        let (free, subscription) = (screen.find("free, no account"), screen.find("subscription"));
        assert!(free < subscription, "free use is listed first:\n{screen}");
        assert_eq!(harness.app().state.draft().expect("open").account, AccountKind::Free);

        harness.send(Msg::Next).send(Msg::Next).render();
        assert!(harness.screen().contains("Build the image"), "{}", harness.screen());
        harness.send(Msg::Finish).render();
        assert!(harness.app().state.draft().is_some(), "finishing without an image leaves nothing behind");
        harness.send(Msg::BuildEnded(Ok(()))).render();
        let screen = harness.screen();
        assert!(screen.contains("Finish"), "the image page is the last one:\n{screen}");
        assert!(screen.contains("opencode needs no sign-in"), "{screen}");
        harness.send(Msg::Next).render();
        assert_eq!(harness.app().state.draft().expect("open").stage, Stage::Image, "there is no page after it");
        harness.send(Msg::Finish).render();
        assert!(harness.app().state.draft().is_none(), "the wizard closes with the profile made");
    }

    #[test]
    fn a_profile_finished_from_the_list_is_the_one_chosen_when_the_list_comes_back() {
        // No engine: the build's answer is given below instead of a missing binary failing it.
        let before = vec![profile("aaa-first", HarnessKind::ClaudeCode), profile("bbb-second", HarnessKind::Codex)];
        let mut harness = loaded(before.clone());
        harness.click_text("New profile").render();
        harness.click_text("opencode").render();
        let name = harness.app().state.draft().expect("the wizard is open").name.clone();
        for _ in 0..5 {
            next(&mut harness);
        }
        assert!(harness.screen().contains("Build the image"), "{}", harness.screen());
        // The engine's answer: the image is built. Nothing is built for real in a test.
        harness.send(Msg::BuildEnded(Ok(()))).render();
        harness.click_text("Finish").render();
        assert!(harness.app().state.draft().is_none(), "the wizard closed:\n{}", harness.screen());
        // What the store holds now, read back the way the reload reads it: the new profile last.
        // The test's store is empty, so its own reading comes back first without the profile,
        // which is the race a real reading can lose too.
        let mut after = before;
        let mut made = profile(&name, HarnessKind::OpenCode);
        made.account = AccountKind::Free;
        after.push(made);
        harness.send(Msg::Loaded(Listing { profiles: after, diagnostics: Vec::new() })).render();
        let chosen = harness.app().state.selected().expect("a profile is chosen");
        assert_eq!(chosen.profile.name.as_str(), name, "the new profile is the chosen one:\n{}", harness.screen());
        // And a later reading, with nothing just made, leaves the person's own choice alone.
        harness.click_text("aaa-first").render();
        let listing = harness.app().state.rows().iter().map(|row| row.profile.clone()).collect();
        harness.send(Msg::Loaded(Listing { profiles: listing, diagnostics: Vec::new() })).render();
        assert_eq!(harness.app().state.selected().expect("chosen").profile.name.as_str(), "aaa-first");
    }

    #[test]
    fn a_free_profile_reads_as_ready_and_offers_no_sign_in() {
        let mut free = profile("oc-free", HarnessKind::OpenCode);
        free.account = AccountKind::Free;
        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing { profiles: vec![free], diagnostics: Vec::new() }));
        answer(&mut harness, Readiness::Present, Readiness::Missing);
        let screen = harness.screen();
        assert!(screen.contains("No sign-in needed"), "{screen}");
        assert!(!screen.contains("Not signed in"), "{screen}");
        assert!(!screen.contains("Sign in") && !screen.contains("Sign out"), "{screen}");
        assert!(screen.contains("opencode, QCode basic, free, no account"), "{screen}");
        assert!(harness.app().state.selected().expect("chosen").is_runnable());
        harness.send(Msg::SignInAsked).render();
        assert!(harness.app().state.draft().is_none(), "there is nothing to sign in to");
        harness.set_locale("tr").render();
        assert!(harness.screen().contains("Giriş gerekmez"), "{}", harness.screen());
        assert!(harness.screen().contains("ücretsiz, hesapsız"), "{}", harness.screen());
    }

    #[test]
    fn choosing_qcode_high_says_what_it_installs_and_what_the_network_means_and_saves_it() {
        // Every step is taken where the person takes it: the button that opens the wizard, the
        // button that moves on, the row that names the template and the one that names the
        // network, never the message sent by hand.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profiles-high-{stamp}"));
        let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
        harness.click_text("New profile").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        assert!(harness.screen().contains("How much of the harness"), "{}", harness.screen());
        assert!(!harness.screen().contains("graphify"), "basic installs nothing more:\n{}", harness.screen());

        harness.click_text("QCode high").render();
        let screen = harness.screen();
        assert_eq!(harness.app().state.draft().expect("open").template, Template::High, "{screen}");
        for installed in ["graphify", "superpowers", "context7", "code-review", "security-guidance", "block-no-verify"]
        {
            assert!(screen.contains(installed), "`{installed}` is not listed:\n{screen}");
        }
        assert!(!screen.contains("oh-my-openagent"), "that is opencode's, not Claude Code's:\n{screen}");
        assert!(screen.contains("so the build needs the network"), "the download is said:\n{screen}");
        let at = |text: &str| {
            let (x, y) = harness.find(text).unwrap_or_else(|| panic!("`{text}` is drawn"));
            harness.fg(u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"))
        };
        assert_ne!(at("needs the network"), at("superpowers"), "the network line stands out:\n{screen}");
        assert!(!screen.contains("context7 cannot fetch"), "the network is still on:\n{screen}");

        harness.click_text("Next").render();
        harness.click_text("Next").render();
        assert!(harness.screen().contains("may see and reach"), "{}", harness.screen());
        let (x, y) = radio_mark(&harness, "none");
        harness.click(i32::from(x), i32::from(y)).render();
        assert_eq!(harness.app().state.draft().expect("open").network, NetworkMode::None, "{}", harness.screen());
        assert!(harness.screen().contains("context7 cannot fetch documentation"), "{}", harness.screen());

        harness.click_text("Next").render();
        let screen = harness.screen();
        assert!(screen.contains("Build the image"), "{screen}");
        assert!(screen.contains("so the build needs the network"), "said again where the build starts:\n{screen}");
        assert!(screen.contains("context7 cannot fetch documentation"), "{screen}");

        // What the build task writes once the image is there, from the profile the wizard hands it.
        let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
        harness.app().state.store().expect("a store").write_profile(&profile).expect("the file is written");
        let text =
            std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the file is there");
        assert!(text.contains("template = \"high\""), "{text}");
        let read = crate::profile::Profile::parse("claude-code.toml", &text);
        assert_eq!(read.profile.map(|profile| profile.template), Some(Template::High), "{:?}", read.diagnostics);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Where the switch of the row labelled `label` is drawn: the rightmost cell of that row whose
    /// ground is not the row's own, a cell inside the switch's track.
    fn switch_of(harness: &Harness<Host>, label: &str) -> (i32, i32) {
        let screen = harness.screen();
        let (y, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.split_whitespace().any(|word| word == label))
            .unwrap_or_else(|| panic!("`{label}` has a row:\n{screen}"));
        let y = u16::try_from(y).expect("on screen");
        let start = line.find(label).map(|at| line[..at].chars().count()).expect("the label is on the row");
        let after = u16::try_from(start + label.chars().count() + 1).expect("on screen");
        let ground = harness.bg(after, y);
        // The whole width, not the line's: a switch is drawn in coloured blanks, which the text of
        // the screen trims away.
        let width = harness.buffer().area.width;
        let cell = (after..width)
            .rev()
            .find(|x| harness.bg(*x, y) != ground)
            .unwrap_or_else(|| panic!("`{label}` has a switch on its row:\n{screen}"));
        (i32::from(cell) - 1, i32::from(y))
    }

    /// A profiles screen on a store in a scratch folder of its own, with an engine that is a script
    /// keeping every Containerfile it is asked to build, under the image's tag with the slashes
    /// turned into dashes, so what reaches an image is read from there. Nothing is built for real.
    fn recording(name: &str) -> (PathBuf, Harness<Host>) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profiles-{name}-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a scratch folder");
        // Each image's Containerfile is kept under its tag, with the slashes turned into dashes.
        let script = format!(
            "#!/bin/sh\n\
             [ \"$1\" = build ] || exit 0\n\
             while [ \"$#\" -gt 0 ]; do\n\
             case \"$1\" in --tag) tag=\"$2\" ;; --file) file=\"$2\" ;; esac\n\
             shift\n\
             done\n\
             cp \"$file\" \"{folder}/built-$(echo \"$tag\" | tr / -)\"\n",
            folder = folder.display()
        );
        let binary = folder.join("engine");
        std::fs::write(&binary, script).expect("the stand-in engine is written");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        let engine = Engine::new(crate::engine::EngineKind::Podman, &binary);
        let mut harness =
            Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), Some(engine)) }, env(), 96, 60);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
        (folder, harness)
    }

    #[test]
    fn choosing_arch_where_the_hand_is_saves_it_and_builds_the_image_on_arch_with_pacman() {
        let (folder, mut harness) = recording("arch");
        harness.click_text("New profile").render();
        harness.click_text("Next").render();
        let screen = harness.screen();
        assert!(screen.contains("Debian 13, 517 MB"), "every system is offered with its size:\n{screen}");
        assert!(screen.contains("Alpine 3.24, 312 MB, not recommended"), "{screen}");
        assert_eq!(harness.app().state.draft().expect("open").os, Os::Debian, "Debian is chosen until changed");
        harness.click_text("Arch Linux, 809 MB").render();
        assert_eq!(harness.app().state.draft().expect("open").os, Os::Arch, "{}", harness.screen());
        assert!(
            harness.screen().contains("This image has no sox"),
            "what Arch leaves out is said:\n{}",
            harness.screen()
        );
        harness.click_text("Next").render();
        harness.click_text("QCode high").render();
        assert!(harness.screen().contains("downloaded from Arch Linux, PyPI"), "{}", harness.screen());
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        // Arriving on the image page is what starts the build.
        harness.click_text("Next").render();
        let build = &harness.app().state.draft().expect("open").build;
        assert!(matches!(build, Build::Done), "the build ended well: {build:?}\n{}", harness.screen());

        let text =
            std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the profile is saved");
        assert!(text.contains("\nos = \"arch\"\n"), "{text}");
        let saved = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
        assert_eq!(saved.os, Os::Arch);

        let base = std::fs::read_to_string(folder.join("built-qcode-base-arch")).expect("Arch's base image was built");
        assert!(base.starts_with(Os::Arch.containerfile()), "from Arch's own description");
        assert!(!folder.join("built-qcode-base").exists(), "Debian's base is not what this profile needed");
        let image = std::fs::read_to_string(folder.join("built-qcode-profile-claude-code"))
            .expect("the profile's image was built from a Containerfile");
        assert!(image.starts_with("FROM qcode/base-arch\n"), "{image}");
        assert!(image.contains("pacman -Syu --noconfirm --needed python python-pipx"), "{image}");
        assert!(!image.contains("apt-get"), "{image}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn on_alpine_a_harness_that_does_not_run_there_is_explained_and_never_built() {
        let (folder, mut harness) = recording("alpine");
        harness.click_text("New profile").render();
        harness.click_text("Gemini CLI").render();
        harness.click_text("Next").render();
        harness.click_text("Alpine 3.24, 312 MB, not recommended").render();
        let read = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            read(&harness).contains("Gemini CLI does not run on Alpine 3.24: the terminal library"),
            "the reason is said where the system is chosen:\n{}",
            harness.screen()
        );
        assert!(read(&harness).contains("Kept for other work; not recommended."), "{}", harness.screen());
        harness.click_text("Next").render();
        assert_eq!(harness.app().state.draft().expect("open").stage, Stage::System, "it cannot be built there");
        assert!(harness.is_focused("profile-system"), "the focus goes to the choice:\n{}", harness.screen());

        // Back on the first page, the harness that cannot run there says so right under the list.
        harness.click_text("Back").render();
        assert!(read(&harness).contains("Gemini CLI does not run on Alpine 3.24"), "{}", harness.screen());
        harness.click_text("Antigravity IDE").render();
        assert!(read(&harness).contains("its program is built for glibc"), "{}", harness.screen());
        harness.click_text("Codex").render();
        assert!(!read(&harness).contains("does not run on"), "Codex runs on Alpine:\n{}", harness.screen());
        harness.click_text("Next").render();
        assert!(
            read(&harness).contains("Alpine packages no docx2txt"),
            "what Alpine lacks is said:\n{}",
            harness.screen()
        );
        harness.click_text("Next").render();
        assert_eq!(harness.app().state.draft().expect("open").stage, Stage::Template, "Codex goes on");
        assert!(!folder.join("Profiles").exists(), "nothing was saved on the way");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_plugin_switched_off_in_the_wizard_is_saved_with_the_profile_and_left_out_of_its_image() {
        // Driven from where the person's hand is: the buttons, the template's row, the switch
        // itself, the key on it, and the build button.
        let (folder, mut harness) = recording("switch");
        harness.click_text("New profile").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        harness.click_text("QCode high").render();

        let context7 = Extra::parse("context7").expect("a part of QCode high");
        let has = |harness: &Harness<Host>, extra| harness.app().state.draft().expect("open").has(extra);
        for extra in Template::High.extras(HarnessKind::ClaudeCode) {
            assert!(has(&harness, extra), "{extra:?} is on until switched off:\n{}", harness.screen());
        }
        let (x, y) = switch_of(&harness, "context7");
        harness.click(x, y).render();
        assert!(!has(&harness, context7), "a click switches it off:\n{}", harness.screen());
        // The keys reach the same switch: the list has the row the click was on.
        harness.press("space").render();
        assert!(has(&harness, context7), "space switches it back on:\n{}", harness.screen());
        harness.press("space").render();
        assert!(!has(&harness, context7), "and off again:\n{}", harness.screen());
        for extra in Template::High.extras(HarnessKind::ClaudeCode).into_iter().filter(|extra| *extra != context7) {
            assert!(has(&harness, extra), "{extra:?} stays on");
        }

        harness.click_text("Next").render();
        harness.click_text("Next").render();
        // Arriving on the image page is what starts the build.
        harness.click_text("Next").render();
        let build = &harness.app().state.draft().expect("open").build;
        assert!(matches!(build, Build::Done), "the build ended well: {build:?}\n{}", harness.screen());

        let text =
            std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the profile is saved");
        assert!(text.contains("[additions]\ncontext7 = false\n"), "{text}");
        let saved = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
        assert_eq!(saved.without, [context7], "{text}");

        let image = std::fs::read_to_string(folder.join("built-qcode-profile-claude-code"))
            .expect("the profile's image was built from a Containerfile");
        assert!(!image.contains("context7"), "{image}");
        for plugin in crate::profile::CLAUDE_PLUGINS.iter().filter(|plugin| !plugin.starts_with("context7@")) {
            assert!(image.contains(&format!("claude plugin install {plugin}")), "{plugin}: {image}");
        }
        assert!(image.contains("pipx install --global graphifyy"), "graphify stays: {image}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn opencode_under_qcode_high_lists_oh_my_openagent_and_not_the_plugins() {
        let mut harness = wizard_on(0);
        harness.click_text("opencode").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        harness.click_text("QCode high").render();
        let screen = harness.screen();
        assert!(screen.contains("oh-my-openagent"), "{screen}");
        assert!(screen.contains("graphify"), "{screen}");
        assert!(!screen.contains("superpowers"), "{screen}");
    }

    #[test]
    fn a_build_of_qcode_high_that_could_not_download_says_so() {
        let mut harness = wizard_on(2);
        harness.click_text("QCode high").render();
        for _ in 0..3 {
            harness.send(Msg::Next);
        }
        let said = format!(
            "{} could not install graphify. What QCode high adds is downloaded while the image is built, so the build needs the network.",
            recipe::HIGH_FAILED
        );
        harness
            .send(Msg::BuildLine(said))
            .send(Msg::BuildEnded(Err(Problem::Refused("exit status 1".to_owned()))))
            .render();
        let screen = harness.screen();
        assert!(screen.contains("could not download what it adds"), "{screen}");
        // Any other failure keeps the plain words.
        let mut harness = wizard_on(5);
        harness.send(Msg::BuildEnded(Err(Problem::Refused("no space left".to_owned())))).render();
        assert!(!harness.screen().contains("could not download what it adds"), "{}", harness.screen());
    }

    #[test]
    fn the_lines_of_the_system_page_are_really_in_every_language() {
        // As for QCode high's lines: every key being there says nothing about the language it is
        // in. With the names of the systems and the tools taken out, most of what each language
        // says must be words of its own.
        let keys = [
            "profiles.wizard.step-system",
            "profiles.wizard.system-lead",
            "profiles.wizard.system-option-not-recommended",
            "profiles.wizard.system-debian-detail",
            "profiles.wizard.system-arch-detail",
            "profiles.wizard.system-ubuntu-detail",
            "profiles.wizard.system-alpine-detail",
            "profiles.wizard.system-gap-docx2txt",
            "profiles.wizard.system-gap-sox",
            "profiles.wizard.system-gap-sox-opus",
            "profiles.wizard.system-refused-terminal",
            "profiles.wizard.system-refused-glibc",
            "profiles.wizard.system-refused-window",
            "profiles.summary-system",
        ];
        let names = [
            "Debian",
            "Arch",
            "Ubuntu",
            "Alpine",
            "Linux",
            "glibc",
            "musl",
            "docx2txt",
            "sox",
            "ffmpeg",
            "opus",
            "Word",
            "Node",
            "Node-Projekt",
            "Gemini",
            "CLI",
            "Antigravity",
            "IDE",
            "MB",
            "Mo",
            "МБ",
            "System",
            "{harness}",
            "{system}",
            "{name}",
            "{mb}",
            "{summary}",
            "24.04",
            "24",
        ];
        let words = |text: &str| -> Vec<String> {
            text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.' && c != '{' && c != '}')
                .map(|word| word.trim_matches('.').to_lowercase())
                .filter(|word| !word.is_empty() && word.chars().any(char::is_alphabetic))
                .filter(|word| !names.iter().any(|name| name.to_lowercase() == *word))
                .collect()
        };
        let mut catalog = qframe::i18n::I18n::builtin();
        for (file, text) in crate::locales() {
            catalog.add_source(&file, &text);
        }
        catalog.set_active("en");
        let english: Vec<(String, Vec<String>)> =
            keys.iter().map(|key| (catalog.translate(key, &[]), words(&catalog.translate(key, &[])))).collect();
        for code in crate::store::Config::LANGUAGES.into_iter().filter(|code| *code != "en") {
            catalog.set_active(code);
            for (key, (english_text, english_words)) in keys.iter().zip(&english) {
                let text = catalog.translate(key, &[]);
                assert!(!text.starts_with('⟦') && !text.is_empty(), "{code} has no words for `{key}`");
                // The step's name is one word, and German calls it what English does.
                if *key == "profiles.wizard.step-system" {
                    continue;
                }
                assert_ne!(&text, english_text, "{code}: `{key}` is the English line");
                let own = words(&text);
                let borrowed = own.iter().filter(|word| english_words.contains(word)).count();
                assert!(borrowed * 2 < own.len().max(1), "{code}: `{key}` reads as English: {text}");
            }
        }
    }

    #[test]
    fn the_lines_of_qcode_high_are_really_in_every_language() {
        // A file whose values are English still has every key. What each language is checked for
        // is its own words: with the names of the tools taken out, most of what is left must not
        // be words of the English line.
        let keys = [
            "profiles.wizard.template-high-detail",
            "profiles.wizard.high-adds",
            "profiles.wizard.high-choose",
            "profiles.wizard.high-none",
            "profiles.wizard.high-graphify",
            "profiles.wizard.high-claude-plugins",
            "profiles.wizard.high-omo",
            "profiles.wizard.high-download",
            "profiles.wizard.high-offline-claude",
            "profiles.wizard.high-offline-opencode",
            "profiles.wizard.high-offline",
            "profiles.wizard.high-build-failed",
            "profiles.template-recommended",
        ];
        let names = [
            "QCode",
            "basic",
            "high",
            "graphify",
            "Claude",
            "Code",
            "opencode",
            "oh-my-openagent",
            "context7",
            "grep.app",
            "Python",
            "Debian",
            "PyPI",
            "npm",
            "GitHub",
            "MB",
            "{plugins}",
            "{system}",
        ];
        let words = |text: &str| -> Vec<String> {
            text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.' && c != '{' && c != '}')
                .map(|word| word.trim_matches('.').to_lowercase())
                .filter(|word| !word.is_empty() && word.chars().any(char::is_alphabetic))
                .filter(|word| !names.iter().any(|name| name.to_lowercase() == *word))
                .collect()
        };
        let mut catalog = qframe::i18n::I18n::builtin();
        for (file, text) in crate::locales() {
            catalog.add_source(&file, &text);
        }
        catalog.set_active("en");
        let english: Vec<(String, Vec<String>)> =
            keys.iter().map(|key| (catalog.translate(key, &[]), words(&catalog.translate(key, &[])))).collect();
        for code in crate::store::Config::LANGUAGES.into_iter().filter(|code| *code != "en") {
            catalog.set_active(code);
            for (key, (english_text, english_words)) in keys.iter().zip(&english) {
                let text = catalog.translate(key, &[]);
                assert_ne!(&text, english_text, "{code}: `{key}` is the English line");
                let own = words(&text);
                let borrowed = own.iter().filter(|word| english_words.contains(word)).count();
                assert!(borrowed * 2 < own.len().max(1), "{code}: `{key}` reads as English: {text}");
            }
        }
    }

    #[test]
    fn every_english_key_this_screen_uses_has_a_turkish_one() {
        let missing = env().i18n().missing_keys("tr", "en");
        assert_eq!(missing, Vec::<String>::new());
    }
}
