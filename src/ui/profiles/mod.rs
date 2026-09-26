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
mod inbox_tests;
#[cfg(test)]
mod live;
pub(crate) mod recipe;
mod remove;
pub(crate) mod shell;
mod shell_page;
#[cfg(test)]
mod shell_tests;
mod status;
#[cfg(test)]
mod window_tests;
mod wizard;
pub(crate) mod work;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use qframe::diagnostics::{Diagnostic, Severity};
use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::widgets::{
    Column, ColumnWidth, EmptyState, Field, LogBuffer, LogLevel, LogLine, LogView, RadioGroup, SettingRow,
    SettingsList, ShimmerText, Switch, Table, TableCell, TableRow, Terminal, TerminalEvent, TerminalSession, TextInput,
    Toast, Wizard,
};

use crate::base::{Gap, Os, Refusal};
use crate::desktop::login::{self, Seen, SignIn};
use crate::desktop::{Display, NoDisplay, callback, signin};
use crate::engine::Engine;
use crate::profile::own::Own;
use crate::profile::{
    ASSUMED_CONTEXT_TOKENS, AccountKind, Extra, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template,
};
use crate::provider::{ProviderEntry, Providers};
use crate::store::Store;
use crate::ui::settings::engine::help;

pub use shell_page::{ShellMsg, ShellPage, Side as ShellSide, Stage as ShellStage};
pub use status::{Readiness, Revision, Row, Status};
pub use wizard::{Blocked, Build, Draft, Login, Page, Stage, Unfinished, WindowBack, WindowLogin};
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
const WIZARD_ROWS: u16 = 28;

/// Rows the application's own foot takes under this screen — the way back and the list of keys —
/// which the screen's view cannot see but which shares the terminal with it.
const CHROME_ROWS: u16 = 1;

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
    /// The selection moved to the profile at this place in the list.
    Select(usize),
    /// The selection moved to the row that makes a new profile.
    SelectNew,
    /// Signing the chosen profile in was asked for, which is how a login that was interrupted is
    /// picked up again.
    SignInAsked,
    /// Signing a profile out was asked for.
    SignOutAsked,
    /// The question was answered with yes.
    SignOutConfirmed,
    /// The login was removed, or could not be.
    SignedOut(Result<(), Problem>),
    /// Building the chosen profile's image again was asked for.
    RebuildAsked,
    /// The question was answered with yes: the image page opens on its own and the build starts.
    RebuildConfirmed,
    /// Changing the chosen profile was asked for: the wizard opens on it.
    EditAsked,
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
    /// A part of a QCode template's additions was switched on (`true`) or off.
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
    /// The window a login is made in is open on the person's screen; the number tells this opening
    /// apart from an earlier one.
    WindowOpened(u64, Arc<SignIn>),
    /// The look for the window's login found it, or found the window gone.
    WindowSeen(u64, Seen),
    /// The window asked for these web addresses to be opened, the newest last.
    WindowAsked(u64, Vec<String>),
    /// The page the window asked for was shown, or could not be.
    WindowPage(u64, String, Page),
    /// The listening for a window sign-in's way back ended; the second number is which listening.
    WindowBack(u64, u64, callback::Ending),
    /// Deleting the chosen profile was asked for: the engine is asked what it holds of it first.
    DeleteAsked,
    /// The engine answered what it holds of the profile to be deleted.
    DeleteSurveyed(Box<remove::Survey>),
    /// The question was answered with yes; `true` when the workspaces' homes of it go too.
    DeleteConfirmed(bool),
    /// The question was answered with no.
    DeleteKept,
    /// The deletion is over.
    Deleted(Box<remove::Survey>, remove::Outcome),
    /// Something about a profile's shell, or its own steps.
    Shell(ShellMsg),
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
    /// A profile is being looked at or deleted, so the button takes no second press.
    deleting: bool,
    /// What the question being asked is about.
    doomed: Option<Box<remove::Survey>>,
    /// Whether the selection is on the row that makes a new profile rather than on a profile.
    on_new: bool,
    /// What this machine offers a window, for the sign-in of a harness that opens one.
    display: Result<Display, NoDisplay>,
    /// A profile's shell, while it is open; it covers the list or the wizard it was opened from.
    shell: Option<ShellPage>,
    /// What each profile had added in its shell, by the profile's name, as last read.
    own: HashMap<String, Own>,
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
            deleting: false,
            doomed: None,
            on_new: false,
            display: crate::desktop::current(),
            shell: None,
            own: HashMap::new(),
        }
    }

    /// The same screen, opening a sign-in window on `display` rather than on the session this
    /// process was started in: a test's own stand-in, never the person's screen.
    #[must_use]
    pub fn showing_on(mut self, display: Result<Display, NoDisplay>) -> Self {
        self.display = display;
        self
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

    /// A profile's shell, while it is open.
    #[must_use]
    pub fn shell(&self) -> Option<&ShellPage> {
        self.shell.as_ref()
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
            state.own.clear();
            let own = shell_page::load(state.store(), &listing.profiles);
            let probing = Command::batch([probe(state, listing.profiles), own]);
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
                state.on_new = false;
            }
            Command::none()
        }
        Msg::SelectNew => {
            state.on_new = true;
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
        Msg::RebuildAsked => ask_rebuild(state),
        Msg::RebuildConfirmed => {
            let Some(row) = state.selected() else { return Command::none() };
            if state.engine.is_none() || row.image != Readiness::Present {
                return Command::none();
            }
            state.draft = Some(Draft::for_rebuild(&row.profile));
            start_build(state)
        }
        Msg::EditAsked => {
            let Some(row) = state.selected() else { return Command::none() };
            state.draft = Some(Draft::for_edit(&row.profile, state.providers.clone()));
            Command::focus("profile-harness")
        }
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
            // leaves nothing behind, so only this one is a profile having been made. A profile
            // that was only changed was not made: nobody waiting for a new one is handed it.
            let finished = state.draft.as_ref().and_then(Draft::profile).map(|profile| profile.name);
            if state.draft.as_ref().is_some_and(Draft::is_editing) {
                state.choose_when_read = finished;
            } else {
                state.made = finished;
                state.choose_when_read.clone_from(&state.made);
            }
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
            if let Some(draft) = &mut state.draft
                && let Some(template) = Template::offered(draft.harness).get(index)
            {
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
            let stopping =
                if draft.is_only_rebuild() { "profiles.rebuild.stopping" } else { "profiles.wizard.build-stopping" };
            draft.log.push(LogLine::new(LogLevel::Warn, t!(stopping)));
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
        Msg::WindowOpened(run, sign_in) => window_opened(state, run, sign_in),
        Msg::WindowSeen(run, seen) => {
            let current = state
                .draft
                .as_ref()
                .is_some_and(|draft| matches!(&draft.login, Login::Window(window) if window.run == run));
            // A window that is signed in, or gone, is done with: what its home holds is taken now.
            if current && seen != Seen::NotYet { store_login(state) } else { Command::none() }
        }
        Msg::WindowAsked(run, addresses) => window_asked(state, run, &addresses),
        Msg::WindowPage(run, address, page) => {
            if let Some(window) = current_window(state, run) {
                window.page = Some((address, page));
            }
            Command::none()
        }
        Msg::WindowBack(run, id, ending) => {
            if let Some(window) = current_window(state, run) {
                window.came_back(id, ending);
            }
            Command::none()
        }
        Msg::DeleteAsked => ask_delete(state),
        Msg::DeleteSurveyed(survey) => delete_question(state, survey),
        Msg::DeleteConfirmed(homes) => delete(state, homes),
        Msg::DeleteKept => {
            state.deleting = false;
            state.doomed = None;
            Command::none()
        }
        Msg::Deleted(survey, outcome) => deleted(state, &survey, outcome),
        Msg::Shell(message) => shell_page::update(state, message),
    }
}

/// Starts deleting the chosen profile by asking the engine what it holds of it, so the question
/// can name everything that goes.
fn ask_delete(state: &mut Profiles) -> Command<Msg> {
    let (Some(store), Some(row)) = (state.store(), state.selected()) else { return Command::none() };
    if state.deleting {
        return Command::none();
    }
    let name = row.profile.name.clone();
    let engine = state.engine.clone();
    state.deleting = true;
    Command::perform(move || Msg::DeleteSurveyed(Box::new(remove::survey(&store, engine.as_ref(), &name))))
}

/// Asks the question, once, naming what goes and what stays; or says why there is nothing to
/// ask, when a container of the profile is running.
///
/// The workspaces' homes of the profile are their own record of its work, so they are the one
/// thing that goes only when the person says so: the question then has a third answer.
fn delete_question(state: &mut Profiles, survey: Box<remove::Survey>) -> Command<Msg> {
    let name = survey.name.to_string();
    if !survey.running().is_empty() {
        state.deleting = false;
        return Command::toast(running_toast(&name, survey.running()));
    }
    let mut message = vec![t!("profiles.removal.definition")];
    message.push(match &survey.engine {
        remove::Reach::Absent => t!("profiles.removal.no-engine"),
        remove::Reach::Unreachable(said) => t!("profiles.removal.unreachable", said = said.as_str()),
        remove::Reach::Listed(held) => {
            let all: Vec<&str> =
                held.image.iter().chain(held.login.iter()).chain(held.containers.iter()).map(String::as_str).collect();
            if all.is_empty() {
                t!("profiles.removal.nothing-in-engine")
            } else {
                t!("profiles.removal.engine", names = all.join(", "))
            }
        }
    });
    if !survey.carried_by.is_empty() {
        let names: Vec<&str> = survey.carried_by.iter().map(crate::store::WorkspaceId::as_str).collect();
        message.push(t!("profiles.removal.carried", workspaces = names.join(", ")));
    }
    let homes = survey.homes();
    let title = t!("profiles.removal.title", name = name.as_str());
    let question = if homes.is_empty() {
        Confirm::new(title, Msg::DeleteConfirmed(false)).confirm_label(t!("profiles.removal.confirm"))
    } else {
        message.push(t!("profiles.removal.homes", homes = homes.join(", ")));
        Confirm::new(title, Msg::DeleteConfirmed(true))
            .alternative(t!("profiles.removal.keep-homes"), Msg::DeleteConfirmed(false))
            .confirm_label(t!("profiles.removal.with-homes"))
    };
    state.doomed = Some(survey);
    Command::confirm(question.message(message.join(" ")).on_cancel(Msg::DeleteKept).danger())
}

/// The toast that says a running container is what keeps a profile from being deleted.
fn running_toast(name: &str, running: &[String]) -> Toast<Msg> {
    Toast::warning(t!("profiles.removal.running", name = name))
        .body(t!("profiles.removal.running-body", containers = running.join(", ")))
}

/// Deletes what the question was about, on a background thread.
fn delete(state: &mut Profiles, homes: bool) -> Command<Msg> {
    let (Some(store), Some(survey)) = (state.store(), state.doomed.take()) else { return Command::none() };
    let engine = state.engine.clone();
    Command::perform(move || {
        let outcome = remove::remove(&store, engine.as_ref(), &survey, homes);
        Msg::Deleted(survey, outcome)
    })
}

/// Says how the deletion went and reads the list again, which is the honest account of what is
/// left.
fn deleted(state: &mut Profiles, survey: &remove::Survey, outcome: remove::Outcome) -> Command<Msg> {
    state.deleting = false;
    let name = survey.name.to_string();
    let told = match outcome {
        remove::Outcome::Deleted => Toast::success(t!("profiles.removal.done", name = name.as_str())),
        remove::Outcome::Running(running) => running_toast(&name, &running),
        remove::Outcome::Partly(left) => {
            Toast::danger(t!("profiles.removal.partly", name = name.as_str())).body(left.join("\n"))
        }
    };
    Command::batch([Command::toast(told), reload(state)])
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

/// Asks before building a profile's image again, saying what is built, what is kept and what
/// the tabs already open go on running.
fn ask_rebuild(state: &Profiles) -> Command<Msg> {
    let Some(row) = state.selected() else { return Command::none() };
    if state.engine.is_none() || row.image != Readiness::Present {
        return Command::none();
    }
    let name = row.profile.name.to_string();
    let mut message = t!("profiles.rebuild.message");
    // What the person added in the profile's shell is put back, and they are told how much.
    let steps = state.own.get(name.as_str()).map_or(0, |own| own.steps.len());
    if steps > 0 {
        message.push(' ');
        message.push_str(&t!("profiles.own.rebuild", n = i64::try_from(steps).unwrap_or(i64::MAX)));
    }
    Command::confirm(
        Confirm::new(t!("profiles.rebuild.title", name = name.as_str()), Msg::RebuildConfirmed)
            .message(message)
            .confirm_label(t!("profiles.rebuild.confirm")),
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
        Some(Blocked::NoChromium) => Command::focus("profile-extras"),
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
            commands.push(discard(engine.clone(), Arc::clone(container)));
        }
        if let Login::Window(window) = &draft.login {
            if let Some(id) = window.looking {
                commands.push(Command::cancel_task(id));
            }
            commands.push(close_window(engine, Arc::clone(&window.sign_in)));
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
    if state.draft.as_ref().is_some_and(Draft::is_editing) {
        return save_edit(state);
    }
    let (Some(engine), Some(store)) = (state.engine.clone(), state.store()) else {
        return Command::none();
    };
    let Some(draft) = &mut state.draft else { return Command::none() };
    let Some(profile) = draft.profile() else { return Command::none() };
    if matches!(draft.build, Build::Running(_)) {
        return Command::none();
    }
    draft.log.clear();
    let profiles = store.profiles_dir();
    if draft.is_only_rebuild() {
        let task = Task::new(t!("profiles.rebuild.button"), move |cx| {
            let cancel = || cx.is_cancelled();
            let mut line = |text: &str| cx.send(Msg::BuildLine(text.to_owned()));
            // The definition file is already there and nothing of it changes, so only the image
            // is made, with what the person added in the profile's shell after the recipe.
            let started = &mut || cx.send(Msg::BaseImageStarted);
            let built = work::rebuild(&engine, &profile, &profiles, &cancel, started, &mut line);
            Ok(Msg::BuildEnded(built))
        });
        draft.build = Build::Running(task.id());
        return Command::task(task);
    }
    let task = Task::new(t!("profiles.wizard.step-image"), move |cx| {
        let cancel = || cx.is_cancelled();
        let mut line = |text: &str| cx.send(Msg::BuildLine(text.to_owned()));
        // The base image of the profile's own system comes first: a profile on Arch is built on
        // Arch's, and the log is told when that one starts building.
        let started = &mut || cx.send(Msg::BaseImageStarted);
        let built = work::build_whole_in(&engine, &profile, &profiles, &cancel, started, &mut line);
        // The definition file is written only once there is an image behind it, so a store
        // never holds a profile that cannot be opened.
        let result =
            built.and_then(|()| store.write_profile(&profile).map_err(|problem| Problem::Machine(problem.message)));
        Ok(Msg::BuildEnded(result))
    });
    draft.build = Build::Running(task.id());
    Command::task(task)
}

/// Saves the changes to a profile: its definition file, after building its image again when the
/// changes reach the image.
///
/// The rebuild is the one the list's button starts, so the image that was there stays until the
/// new one is built, and a rebuild that fails or is stopped saves nothing: the definition would
/// otherwise describe an image that was never made. Changes that do not reach the image are
/// saved at once and need no engine; the containers of the workspaces take them the next time
/// each is started, because a container made from another plan is made again then
/// (`ensure_running`).
fn save_edit(state: &mut Profiles) -> Command<Msg> {
    let Some(store) = state.store() else { return Command::none() };
    let engine = state.engine.clone();
    let Some(draft) = &mut state.draft else { return Command::none() };
    let Some(profile) = draft.profile() else { return Command::none() };
    if matches!(draft.build, Build::Running(_)) {
        return Command::none();
    }
    let rebuild = draft.rebuilds();
    if rebuild && engine.is_none() {
        return Command::none();
    }
    draft.log.clear();
    let task = Task::new(t!("profiles.edit.saving"), move |cx| {
        let built = match (&engine, rebuild) {
            (Some(engine), true) => {
                let cancel = || cx.is_cancelled();
                let mut line = |text: &str| cx.send(Msg::BuildLine(text.to_owned()));
                let started = &mut || cx.send(Msg::BaseImageStarted);
                work::rebuild(engine, &profile, &store.profiles_dir(), &cancel, started, &mut line)
            }
            _ => Ok(()),
        };
        let saved =
            built.and_then(|()| store.write_profile(&profile).map_err(|problem| Problem::Machine(problem.message)));
        Ok(Msg::BuildEnded(saved))
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
    if profile.harness.desktop().is_some() {
        return start_window_login(state, engine, profile);
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
    if let Login::Window(window) = &draft.login {
        let Some(profile) = draft.profile() else { return Command::none() };
        let stop = window.looking.map_or_else(Command::none, Command::cancel_task);
        let sign_in = Arc::clone(&window.sign_in);
        // Dropping the window's state gives up any port still listened on for its way back.
        draft.login = Login::Storing;
        let task = Task::new(t!("profiles.wizard.storing"), move |_| {
            let stored = login::take(&engine, &profile, &sign_in);
            login::close(&engine, &sign_in);
            Ok(Msg::LoginStored(stored))
        });
        return Command::batch([stop, Command::task(task)]);
    }
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
    if let Login::Window(window) = &draft.login {
        let stop = window.looking.map_or_else(Command::none, Command::cancel_task);
        let close = close_window(engine, Arc::clone(&window.sign_in));
        draft.login = Login::Unfinished(Unfinished::Stopped);
        return Command::batch([stop, close]);
    }
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

/// Tells one opening of a sign-in window from the next.
static WINDOW_RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Opens the window a harness that draws in one is signed in from, on this machine's screen.
///
/// A machine with no screen to open it on is said to have none, in the words the workspace uses
/// for the same thing, rather than failing at the engine.
fn start_window_login(state: &mut Profiles, engine: Engine, profile: Profile) -> Command<Msg> {
    let display = match &state.display {
        Ok(display) => display.clone(),
        Err(reason) => {
            let words = match reason {
                NoDisplay::NoWayland => t!("workspace.window.no-wayland"),
                NoDisplay::NoSocket(path) => t!("workspace.window.no-socket", socket = path.display().to_string()),
            };
            if let Some(draft) = &mut state.draft {
                draft.login = Login::Failed(Problem::Machine(words));
            }
            return Command::none();
        }
    };
    let Some(draft) = &mut state.draft else { return Command::none() };
    draft.login = Login::Opening;
    let run = WINDOW_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Command::task(Task::new(t!("profiles.wizard.step-login"), move |_| {
        Ok(match login::open(&engine, &profile, &display) {
            Ok(sign_in) => Msg::WindowOpened(run, Arc::new(sign_in)),
            Err(problem) => Msg::LoginFailed(problem),
        })
    }))
}

/// Takes the window that is open for a login. From then on one task stays with it for as long as
/// it is open: it passes on every page the window asks to have opened, the moment it is asked for,
/// and looks inside every few seconds for the login.
///
/// One task rather than a watch of the folder beside it: both only wait, so the waiting is counted
/// in the task's own pauses, which a test's clock decides; and the task ends with the window.
fn window_opened(state: &mut Profiles, run: u64, sign_in: Arc<SignIn>) -> Command<Msg> {
    let engine = state.engine.clone();
    let Some(draft) = state.draft.as_mut().filter(|draft| matches!(draft.login, Login::Opening)) else {
        // The wizard was closed, or the sign-in stopped, while the window was coming up.
        return close_window(engine, sign_in);
    };
    let Some(engine) = engine else { return Command::none() };
    let looked = Arc::clone(&sign_in);
    let task = Task::new(t!("profiles.window.looking"), move |cx| {
        let every = login::LOOK_EVERY.as_millis() / login::ASK_EVERY.as_millis().max(1);
        let mut pauses: u128 = 0;
        loop {
            let asked = signin::taken(&looked.browser);
            if !asked.is_empty() {
                cx.send(Msg::WindowAsked(run, asked));
            }
            if pauses.is_multiple_of(every.max(1)) {
                match login::seen(&engine, &looked) {
                    Seen::NotYet => {}
                    seen => return Ok(Msg::WindowSeen(run, seen)),
                }
            }
            pauses += 1;
            if !cx.sleep(login::ASK_EVERY) {
                return Ok(Msg::LoginDiscarded);
            }
        }
    });
    let mut window = WindowLogin::new(run, Arc::clone(&sign_in));
    window.looking = Some(task.id());
    draft.login = Login::Window(window);
    Command::task(task)
}

/// The window being signed in from, when `run` is its opening.
fn current_window(state: &mut Profiles, run: u64) -> Option<&mut WindowLogin> {
    match &mut state.draft.as_mut()?.login {
        Login::Window(window) if window.run == run => Some(window),
        _ => None,
    }
}

/// Takes the addresses the sign-in window asked to have opened: the newest is opened in the
/// person's own browser, after its way back to the application's `localhost` is listened for on
/// this machine and carried into the window's container, exactly as a workspace's window does
/// ([`callback`]). A page whose way back cannot be listened for is not opened: its sign-in would go
/// to whoever holds the port.
fn window_asked(state: &mut Profiles, run: u64, addresses: &[String]) -> Command<Msg> {
    let engine = state.engine.clone();
    let Some(window) = current_window(state, run) else { return Command::none() };
    let Some(address) = addresses.last().cloned() else { return Command::none() };
    if !signin::is_web(&address) {
        window.page = Some((address, Page::Nowhere));
        return Command::none();
    }
    let listening = match (callback::return_to(&address), engine) {
        (Some(back), _) if window.listens_on(back.port) => Command::none(),
        (Some(back), Some(engine)) => match callback::Listener::bind(&back) {
            Ok(listener) => {
                let carrier = callback::carrier(&engine, &window.sign_in.window, &back);
                let (stop, stopped) = callback::Stop::new();
                let id = stop.id();
                window.back = Some((back.port, WindowBack::Listening(stop)));
                Command::task(Task::new(t!("workspace.window.signin-task"), move |cx| {
                    let ending = listener
                        .serve(&carrier, |pause| cx.sleep(pause) && !stopped.load(std::sync::atomic::Ordering::SeqCst));
                    Ok(Msg::WindowBack(run, id, ending))
                }))
            }
            Err(refusal) => {
                let why = match refusal {
                    callback::NotListening::Taken => WindowBack::Taken,
                    callback::NotListening::Refused(words) => WindowBack::Refused(words),
                };
                window.back = Some((back.port, why));
                window.page = Some((address, Page::Held));
                return Command::none();
            }
        },
        _ => Command::none(),
    };
    window.page = Some((address.clone(), Page::Asked));
    let answer = address.clone();
    let open = Command::open_with(qframe::runtime::Open::new(address).answer(move |outcome| {
        let page = if outcome == qframe::runtime::OpenOutcome::Opened { Page::InBrowser } else { Page::Nowhere };
        Msg::WindowPage(run, answer, page)
    }));
    Command::batch([listening, open])
}

/// Clears away the window a login was made in, and everything made for it.
fn close_window(engine: Option<Engine>, sign_in: Arc<SignIn>) -> Command<Msg> {
    let Some(engine) = engine else { return Command::none() };
    Command::perform(move || {
        login::close(&engine, &sign_in);
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
    if let Some(page) = &state.shell {
        let top = ui.size().height.saturating_sub(WIZARD_ROWS + CHROME_ROWS) / 2;
        ui.column(|ui| {
            ui.spacer().height(Length::Cells(top));
            shell_page::draw(page, ui);
        })
        .fill()
        .align(Align::Center);
        return;
    }
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
    // Only a file that could not be read is counted as one: a warning is about a profile that
    // loaded, and what it says is spelled out beside that profile instead.
    let problems = state.problems().iter().filter(|problem| problem.severity == Severity::Error).count();
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

        // Every profile on a row of its own, what the engine said about it in columns that line
        // up, so the state of all of them is read at a glance. The way to a new profile is the
        // first row, the way it is on the workspaces screen, so the keyboard reaches it like any
        // profile; moving onto it only chooses it, and Enter or a click opens the wizard.
        let new = TableRow::new([TableCell::new(t!("profiles.new")).icon("add", Some("accent"))]);
        let table_rows: Vec<TableRow> = std::iter::once(new).chain(rows.iter().map(table_row)).collect();
        let height = u16::try_from(table_rows.len() + 1).unwrap_or(u16::MAX);
        let columns = [
            Column::new(t!("profiles.column.name")).width(ColumnWidth::Fit),
            Column::new(t!("profiles.column.harness")).width(ColumnWidth::Fit),
            Column::new(t!("profiles.column.account")).width(ColumnWidth::Fit),
            Column::new(t!("profiles.column.image")).width(ColumnWidth::Fit),
            Column::new(t!("profiles.column.sign-in")).width(ColumnWidth::Fill(1)),
        ];
        let chosen = if state.on_new { 0 } else { state.selected + 1 };
        let table = Table::new(columns, table_rows)
            .selected(Some(chosen))
            .on_select(|row| row.checked_sub(1).map_or(Msg::SelectNew, Msg::Select))
            .on_activate(|row| row.checked_sub(1).map_or(Msg::New, Msg::Select));
        ui.add(table).id("profiles").height(Length::Cells(height)).fill_width();

        // What can be done to the chosen profile stands once, right under the rows, and nothing
        // stands there while the new-profile row is chosen: the rows themselves stay plain.
        if let Some(row) = selected.filter(|_| !state.on_new) {
            let needs_login = row.profile.account.needs_login();
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
                // Everything but the name can be changed, whatever the engine says.
                ui.add(Button::new(t!("profiles.edit.button")).on_press(Msg::EditAsked)).id("profile-edit");

                // Only an image that is there can be built again; one that is not is built from
                // the tab that needs it, with the words for that.
                let rebuildable = !engineless && row.image == Readiness::Present;
                let mut rebuild = Button::new(t!("profiles.rebuild.button"));
                if rebuildable {
                    rebuild = rebuild.on_press(Msg::RebuildAsked);
                }
                ui.add(rebuild.disabled(!rebuildable)).id("profile-rebuild");
                let mut delete = Button::new(t!("profiles.removal.button")).variant("danger").loading(state.deleting);
                if !state.deleting {
                    delete = delete.on_press(Msg::DeleteAsked);
                }
                ui.add(delete).id("profile-delete");
                ui.spacer();
            })
            .gap(2)
            .fill_width();
            ui.add(Text::new(summary(&row.profile)).role("secondary")).fill_width();
            ui.add(Text::new(mounts(&row.profile)).role("secondary")).fill_width();
            if let Some(closed) = closed_sign_in(&row.profile) {
                ui.add(Text::new(closed).color("warning")).fill_width();
            }
            if row.image == Readiness::Present && row.revision == Revision::Earlier {
                ui.add(Text::new(t!("profiles.rebuild.earlier")).color("warning")).fill_width();
            }
            // On a row of its own beside what was added in it, so the actions above keep their
            // room on a narrow screen. Only an image that is there has a container to open.
            let shell = !engineless && row.image == Readiness::Present;
            ui.row(|ui| {
                let mut open = Button::new(t!("profiles.shell.button"));
                if shell {
                    open = open.on_press(Msg::Shell(ShellMsg::Asked));
                }
                ui.add(open.disabled(!shell)).id("profile-shell");
                ui.spacer();
            })
            .fill_width();
            if let Some(own) = state.own.get(row.profile.name.as_str()) {
                shell_page::draw_own(own, ui);
            }
        }
    })
    .fill()
    .gap(1);
}

/// One profile's row: its name, its harness, what it signs in with, and what the engine said
/// about its image and its login.
fn table_row(row: &Row) -> TableRow {
    let sign_in = if row.profile.account.needs_login() {
        state_cell(row.identity, "profiles.identity")
    } else {
        // Nothing to ask the engine: a profile that signs in to nothing is as ready as its image,
        // and its cell says so, quietly, rather than "not signed in".
        TableCell::new(t!("profiles.identity-free")).icon("dot-outline", None)
    };
    let account = match &row.profile.provider {
        Some(_) => account_word(AccountKind::Provider),
        None => account_word(row.profile.account),
    };
    TableRow::new([
        TableCell::new(row.profile.name.to_string()),
        TableCell::new(row.profile.harness.record().display_name),
        TableCell::new(account),
        state_cell(row.image, "profiles.image"),
        sign_in,
    ])
}

/// The cell of one answer of the engine, which says plainly when there is no answer yet: a dot in
/// the answer's colour, and its words.
fn state_cell(readiness: Readiness, key: &str) -> TableCell {
    let (word, colour) = match readiness {
        Readiness::Present => ("ready", Some("success")),
        Readiness::Missing => ("missing", Some("warning")),
        Readiness::Unknown => ("unknown", None),
    };
    TableCell::new(t!(&format!("{key}-{word}"))).icon("dot", colour)
}

/// What a profile runs, in one line. The template is named without the recommendation the
/// wizard gives it: that is advice for choosing, and this profile has chosen already.
fn summary(profile: &Profile) -> String {
    let template = t!(match profile.template {
        Template::Base => "profiles.template.summary-base",
        Template::Recommended => "profiles.template.name-recommended",
        Template::Slim => "profiles.template.name-slim",
        Template::High => "profiles.template.name-extra",
        Template::QuvytaDev => "profiles.summary-quvyta-dev",
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

/// What the person is told about a profile whose account its harness's makers closed, in plain
/// words: who closed it, what the harness now answers, for whom it still works, and the ways on.
/// `None` for every profile whose sign-in still stands.
#[must_use]
pub fn closed_sign_in(profile: &Profile) -> Option<String> {
    if !profile.harness.withdrawn(profile.account) {
        return None;
    }
    match profile.harness {
        HarnessKind::GeminiCli => Some(t!("profiles.gemini.closed")),
        HarnessKind::ClaudeCode
        | HarnessKind::OpenCode
        | HarnessKind::Codex
        | HarnessKind::KimiCode
        | HarnessKind::QwenCode
        | HarnessKind::AntigravityIde => None,
    }
}

/// The word for a template.
fn template_word(template: Template) -> String {
    t!(match template {
        Template::Base => "profiles.template.name-base",
        Template::Recommended => "profiles.template.name-recommended",
        Template::Slim => "profiles.template.name-slim",
        Template::High => "profiles.template.name-extra",
        Template::QuvytaDev => "profiles.template-quvyta-dev",
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

/// Why a profile has no sign-in step, for the one account that has none: the harness needs no
/// account at all.
fn no_login_detail(account: AccountKind, harness: &str) -> Option<String> {
    match account {
        AccountKind::Free => Some(t!("profiles.wizard.account-free-detail", harness = harness)),
        AccountKind::Subscription | AccountKind::ApiKey | AccountKind::InApp | AccountKind::Provider => None,
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
        } else if draft.is_only_rebuild() {
            draw_rebuild(state, draft, ui);
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

/// The image page on its own, building an existing profile's image again. Closing it while the
/// build runs stops the build, which leaves the image that was there.
fn draw_rebuild(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.add(Text::new(t!("profiles.rebuild.heading", name = draft.name.as_str())).bold());
        draw_image(state, draft, ui);
        ui.row(|ui| {
            ui.spacer();
            ui.add(Button::new(t!("profiles.wizard.close")).variant("primary").on_press(Msg::Cancel))
                .id("rebuild-close");
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
        .show(ui, |ui| {
            match draft.stage {
                Stage::Harness => draw_harness(draft, ui),
                Stage::System => draw_system(draft, ui),
                Stage::Template => draw_template(draft, ui),
                Stage::Account => draw_account(draft, ui),
                Stage::Permissions => draw_permissions(draft, ui),
                Stage::Image => draw_image(state, draft, ui),
                Stage::Login => draw_login(state, draft, ui),
            }
            // On the last page, once there is an image: the profile is made, and this is where
            // the person may still change it by hand before any workspace copies it.
            let last = draft.stages().last() == Some(&draft.stage);
            let signing = draft.login.is_busy() || matches!(draft.login, Login::Running { .. } | Login::Window(_));
            if last && draft.build == Build::Done && !signing {
                shell_page::offer(state, ui);
            }
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
    // A profile being changed keeps its name, and the page says why in one quiet line rather than
    // offering a field that would not take.
    if draft.is_editing() {
        ui.add(Text::new(t!("profiles.wizard.name")).bold());
        ui.add(Text::new(draft.name.clone()));
        ui.add(Text::new(t!("profiles.edit.name-fixed")).role("secondary")).fill_width();
        return;
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
    let offered = Template::offered(draft.harness);
    let chosen = offered.iter().position(|template| *template == draft.template);
    ui.add(Text::new(t!("profiles.wizard.template-lead")).role("secondary")).fill_width();
    ui.add(
        RadioGroup::new(offered.into_iter().map(template_word).collect::<Vec<_>>())
            .selected(chosen)
            .on_select(Msg::PickTemplate),
    )
    .id("profile-template");
    let detail = match draft.template {
        Template::Base => t!("profiles.wizard.template-base-detail"),
        Template::Recommended => t!("profiles.template.recommended-detail"),
        Template::Slim => t!("profiles.template.slim-detail"),
        Template::High => t!("profiles.template.extra-detail"),
        Template::QuvytaDev => t!("profiles.template.dev-detail"),
    };
    ui.add(Text::new(detail).role("secondary")).fill_width();
    let extras = draft.template.extras(draft.harness);
    if extras.is_empty() {
        ui.add(Text::new(t!("profiles.template.unattended")).role("secondary")).fill_width();
        return;
    }
    // What a QCode template installs is said where it is chosen, item by item, in the page's own
    // text rather than in a footnote under it: the person is agreeing to a download of hundreds of
    // megabytes and to tools of other makers in their image. Each is on unless switched off here,
    // as the owner approved them: visible, and each one the person's to refuse.
    ui.add(Text::new(t!("profiles.template.adds")).bold());
    ui.add(Text::new(t!("profiles.template.adds-choose")).role("secondary")).fill_width();
    // The parts that are not plugins, each with a line of what it is and how large.
    let (plugins, parts): (Vec<Extra>, Vec<Extra>) =
        extras.into_iter().partition(|extra| matches!(extra, Extra::Plugin(_)));
    extra_switches(draft, &parts, ui).id("profile-extras").fill_width();
    if plugins.is_empty() {
        draw_blocked_and_download(draft, ui);
        return;
    }
    // The plugins stand together under one heading, which also gives their size: QCode
    // recommended's five in one column, the sixteen of QCode extra in columns side by side, so the
    // page still fits the wizard's height and every one keeps a switch of its own.
    let heading = if draft.template.carries_high() {
        t!("profiles.template.extra-plugins")
    } else {
        t!("profiles.wizard.high-claude-plugins")
    };
    ui.add(Text::new(heading).bold());
    // As many columns as fit the page with the longest name whole, at most three, each measured
    // and painted at the same width. The page is as wide as the screen lets it be, up to its own.
    let page = ui.size().width.min(PAGE_WIDTH);
    let longest = plugins.iter().map(|plugin| plugin.id().chars().count()).max().unwrap_or_default();
    let needed = u16::try_from(longest).unwrap_or(u16::MAX).saturating_add(PLUGIN_ROW_ROOM);
    let fit = usize::from((page + PLUGIN_GAP) / (needed + PLUGIN_GAP)).clamp(1, PLUGIN_COLUMNS);
    let columns = if plugins.len() > PLUGINS_IN_ONE_COLUMN { fit } else { 1 };
    let per_column = plugins.len().div_ceil(columns);
    let spread = u16::try_from(columns).unwrap_or(1);
    let width = page.saturating_sub(PLUGIN_GAP * (spread - 1)) / spread;
    ui.row(|ui| {
        for (index, column) in plugins.chunks(per_column).enumerate() {
            let list = extra_switches(draft, column, ui).id(format!("profile-plugins-{index}"));
            if columns == 1 {
                list.fill_width();
            } else {
                list.width(Length::Cells(width));
            }
        }
    })
    .gap(PLUGIN_GAP)
    .fill_width();
    draw_blocked_and_download(draft, ui);
}

/// Plugins up to this many stand in one column; more are spread over [`PLUGIN_COLUMNS`].
const PLUGINS_IN_ONE_COLUMN: usize = 6;

/// How many columns the plugins of QCode extra are spread over.
const PLUGIN_COLUMNS: usize = 3;

/// Cells between two columns of plugins.
const PLUGIN_GAP: u16 = 2;

/// Cells a plugin's row needs beside its name: the pillar before it, the gap, the switch with its
/// word in any language, and the gap after it.
const PLUGIN_ROW_ROOM: u16 = 18;

/// One list of switches, one row for each of `extras`: graphify, oh-my-openagent and Chromium
/// with a line saying what they are, a plugin by its name alone.
fn extra_switches<'v>(draft: &Draft, extras: &[Extra], ui: &'v mut View<'_, Msg>) -> qframe::widget::NodeMut<'v, Msg> {
    SettingsList::show(ui, |list| {
        for extra in extras.iter().copied() {
            let row = match extra {
                Extra::Graphify => SettingRow::new(extra.id()).description(t!("profiles.wizard.high-graphify")),
                Extra::OhMyOpenAgent => SettingRow::new(extra.id()).description(t!("profiles.wizard.high-omo")),
                Extra::OhMyOpenCodeSlim => SettingRow::new(extra.id()).description(t!("profiles.template.slim-omo")),
                Extra::Chromium => SettingRow::new(extra.id()).description(t!("profiles.template.dev-chromium")),
                Extra::Plugin(_) => SettingRow::new(extra.id()),
            };
            list.row(row, |ui| {
                ui.add(Switch::new(draft.has(extra)).on_toggle(move |on| Msg::SwitchExtra(extra, on)));
            });
        }
    })
}

/// Under the parts: why the chosen system cannot have Chromium, when it cannot, and what the build
/// downloads.
fn draw_blocked_and_download(draft: &Draft, ui: &mut View<'_, Msg>) {
    if draft.blocked() == Some(Blocked::NoChromium) {
        ui.add(Text::new(t!("profiles.template.dev-no-chromium", system = draft.os.display_name())).color("danger"))
            .fill_width();
    }
    draw_download(draft, ui);
}

/// What the build downloads and what the network means, in the colour of a warning; or, when every
/// part of a QCode template is switched off, that nothing more is downloaded.
fn draw_download(draft: &Draft, ui: &mut View<'_, Msg>) {
    if !draft.adds_anything() {
        ui.add(Text::new(t!("profiles.template.adds-none")).role("secondary")).fill_width();
        return;
    }
    // The system's own repositories are named, since the packages come from there. graphify alone
    // comes from them and PyPI; the plugins add npm and GitHub, and Quvyta development its own.
    let graphify_alone =
        draft.template.extras(draft.harness).into_iter().all(|extra| extra == Extra::Graphify || !draft.has(extra));
    let system = draft.os.display_name();
    let download = match draft.template {
        Template::QuvytaDev => t!("profiles.template.dev-download", system = system),
        _ if graphify_alone => t!("profiles.template.download-graphify", system = system),
        _ => t!("profiles.template.download", template = template_word(draft.template).as_str(), system = system),
    };
    ui.add(Text::new(download).color("warning")).fill_width();
    if let Some(offline) = offline_line(draft) {
        ui.add(Text::new(offline).color("warning")).fill_width();
    }
}

/// What will not work at run time in a profile of a QCode template whose containers have no network, when
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
        HarnessKind::OpenCode if draft.template == Template::Slim && draft.has(Extra::OhMyOpenCodeSlim) => {
            Some(t!("profiles.template.slim-offline"))
        }
        HarnessKind::OpenCode
            if draft.template.extras(draft.harness).contains(&Extra::OhMyOpenAgent)
                && draft.has(Extra::OhMyOpenAgent) =>
        {
            Some(t!("profiles.wizard.high-offline-opencode"))
        }
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
    // Said where the account is chosen, so the sign-in at the end of the wizard is expected, and
    // so is what it is for: one login for the profile, not one for every workspace.
    if draft.account.needs_login() {
        let harness = draft.harness.record().display_name;
        ui.add(Text::new(t!("profiles.signing.ahead", harness = harness)).role("secondary")).fill_width();
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
    // Only a window QCode measured, or one a ready-made service gives for its own models, is
    // handed to Claude Code; without one it works to the room of its own models and prints a
    // warning at every start. Measuring is left to the person on the Providers page, because it
    // loads the model on their server.
    let kind = draft.provider_tag.as_deref().and_then(|tag| draft.provider_entry(tag)).map(|entry| entry.kind);
    let unmeasured = models.iter().find(|model| Some(model.id.as_str()) == draft.provider_model.as_deref());
    if draft.harness == HarnessKind::ClaudeCode
        && let Some(model) = unmeasured.filter(|model| kind.is_none_or(|kind| model.window(kind).is_none()))
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
    // is where they learn what of a QCode template that costs.
    if let Some(offline) = offline_line(draft) {
        ui.add(Text::new(offline).color("warning")).fill_width();
    }
}

/// Building the image, with everything the engine says as it says it.
fn draw_image(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    let rebuild = draft.is_only_rebuild() || (draft.is_editing() && draft.rebuilds());
    let kept = draft.is_editing() && !draft.rebuilds();
    let lead = match &draft.build {
        // A change that does not reach the image saves the file and builds nothing.
        Build::Waiting | Build::Running(_) if kept => t!("profiles.edit.saving"),
        Build::Done if kept => t!("profiles.edit.kept"),
        Build::Done if draft.is_editing() => t!("profiles.edit.rebuilt", name = draft.name.as_str()),
        Build::Failed(_) if kept => t!("profiles.edit.unsaved"),
        // A rebuild keeps the image that was there until the new one is finished, so its words
        // for a build under way, stopped or failed are not the wizard's.
        Build::Running(_) if rebuild => t!("profiles.rebuild.running"),
        Build::Done if rebuild => t!("profiles.rebuild.done", name = draft.name.as_str()),
        Build::Stopped if rebuild => t!("profiles.rebuild.stopped"),
        Build::Failed(_) if rebuild => t!("profiles.rebuild.failed"),
        Build::Waiting => t!("profiles.wizard.build-waiting"),
        Build::Running(_) => t!("profiles.wizard.build-running"),
        Build::Done => t!("profiles.wizard.build-done"),
        Build::Stopped => t!("profiles.wizard.build-stopped"),
        Build::Failed(_) if draft.dev_download_failed() => t!("profiles.template.dev-build-failed"),
        Build::Failed(_) if draft.extra_download_failed() => {
            t!("profiles.template.build-failed", template = template_word(Template::High).as_str())
        }
        Build::Failed(_) if draft.slim_download_failed() => {
            t!("profiles.template.build-failed", template = template_word(Template::Slim).as_str())
        }
        Build::Failed(_) if draft.recommended_download_failed() => {
            t!("profiles.template.build-failed", template = template_word(Template::Recommended).as_str())
        }
        Build::Failed(_) => t!("profiles.wizard.build-failed"),
    };
    // While the build runs its words shine, the one thing on the page that moves besides the log:
    // the page is waiting, and says so without a spinner of its own.
    // A shimmer is one line that is never wrapped, so it carries the short word and the sentence
    // under it the rest.
    if matches!(draft.build, Build::Running(_)) {
        ui.add(ShimmerText::new(t!("profiles.working.build"))).id("build-working");
    }
    ui.add(Text::new(lead).role(if matches!(draft.build, Build::Failed(_)) { "danger" } else { "secondary" }))
        .fill_width();
    if let Build::Failed(problem) = &draft.build {
        recognised(state, problem, ui);
    }
    if draft.is_editing() {
        match draft.build {
            // What happens to the containers already made from the profile.
            Build::Done => {
                ui.add(Text::new(t!("profiles.edit.runtime")).role("secondary")).fill_width();
            }
            Build::Failed(_) | Build::Stopped if rebuild => {
                ui.add(Text::new(t!("profiles.edit.unsaved")).color("warning")).fill_width();
            }
            _ => {}
        }
    }
    if draft.build == Build::Done && !draft.account.needs_login() && !rebuild && !draft.is_editing() {
        let harness = draft.harness.record().display_name;
        ui.add(Text::new(t!("profiles.wizard.free-ready", harness = harness)).color("success")).fill_width();
    }
    // A window's image carries a whole desktop application, which is an order of magnitude more
    // than a command-line harness; the person is told before the build rather than after.
    if let Some(desktop) = draft.harness.desktop()
        && matches!(draft.build, Build::Waiting | Build::Running(_))
        && !kept
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
    if draft.adds_anything() && matches!(draft.build, Build::Waiting | Build::Running(_)) && !kept {
        draw_download(draft, ui);
    }
    // Saving a change the image does not see needs no engine.
    let engineless = engineless && !kept;
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
            // Gemini CLI's own dialog is kept to the key in the image, so the page says where the
            // key comes from and what the dialog will ask, rather than speak of a sign-in flow.
            // A window is signed in to in a window, so its page says so, and what QCode reads to be
            // sure of it is the application's own record rather than a file.
            let window = draft.harness.desktop().is_some();
            let (why, what, proof) = if window {
                ("profiles.window.why", "profiles.window.what", "profiles.window.proof")
            } else if draft.harness == HarnessKind::GeminiCli && draft.account == AccountKind::ApiKey {
                ("profiles.gemini.login-why", "profiles.gemini.login-what", "profiles.wizard.login-proof")
            } else {
                ("profiles.wizard.login-why", "profiles.wizard.login-what", "profiles.wizard.login-proof")
            };
            ui.add(Text::new(t!("profiles.signing.kept")).bold()).fill_width();
            ui.add(Text::new(t!(why, harness = harness)).role("secondary")).fill_width();
            ui.add(Text::new(t!(what, harness = harness)).role("secondary")).fill_width();
            ui.add(Text::new(t!(proof)).role("secondary")).fill_width();
            // A profile being made can be finished from here without signing in; what that costs
            // is said before the person decides, not after.
            if !draft.is_only_login() {
                ui.add(Text::new(t!("profiles.signing.skip", harness = harness)).role("secondary")).fill_width();
            }
            if engineless {
                ui.add(Text::new(t!("profiles.no-engine-detail")).color("warning")).fill_width();
            }
            let open = if window { t!("profiles.window.open") } else { t!("profiles.wizard.login-open") };
            let mut button = Button::new(open).variant("primary").disabled(engineless);
            if !engineless {
                button = button.on_press(Msg::LoginStart);
            }
            ui.add(button).id("login-open");
        }
        Login::Opening => {
            ui.add(ShimmerText::new(t!("profiles.wizard.login-opening"))).id("login-working");
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
        Login::Window(window) => draw_window(window, harness, ui),
        Login::Storing => {
            ui.add(ShimmerText::new(t!("profiles.wizard.login-storing"))).id("login-working");
        }
        Login::Stored(_) if draft.harness.desktop().is_some() => {
            ui.add(Text::new(t!("profiles.window.stored", harness = harness)).color("success")).fill_width();
            ui.add(Text::new(t!("profiles.signing.stored")).role("secondary")).fill_width();
        }
        Login::Stored(files) => {
            ui.add(
                Text::new(t!("profiles.wizard.login-stored", n = i64::try_from(*files).unwrap_or(i64::MAX)))
                    .color("success"),
            )
            .fill_width();
            ui.add(Text::new(t!("profiles.signing.stored")).role("secondary")).fill_width();
        }
        Login::Unfinished(why) => {
            let text = match why {
                Unfinished::Stopped => t!("profiles.wizard.login-interrupted"),
                Unfinished::Missing if draft.harness.desktop().is_some() => {
                    t!("profiles.window.not-found", harness = harness)
                }
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

/// The sign-in window's page: where the window is, what to do in it, where the sign-in page went,
/// and the two things that can be done from here.
fn draw_window(window: &WindowLogin, harness: &str, ui: &mut View<'_, Msg>) {
    ui.add(Text::new(t!("profiles.window.running", harness = harness))).fill_width();
    ui.add(Text::new(t!("profiles.window.noticed", harness = harness)).role("secondary")).fill_width();
    let minutes = (callback::WAIT.as_secs() / 60).to_string();
    match &window.page {
        None => {}
        Some((address, Page::Asked | Page::InBrowser)) => {
            ui.add(Text::new(t!("workspace.window.signin-in-browser")).role("secondary")).fill_width();
            ui.add(Text::new(address.clone()).role("secondary")).fill_width();
        }
        Some((address, Page::Nowhere | Page::Held)) => {
            ui.add(Text::new(t!("workspace.window.signin-open-yourself")).color("warning")).fill_width();
            ui.add(Text::new(address.clone())).fill_width();
        }
    }
    match &window.back {
        None => {}
        Some((port, WindowBack::Listening(_))) => {
            let text = t!("workspace.window.signin-listening", port = port.to_string(), minutes = minutes.clone());
            ui.add(Text::new(text).role("secondary")).fill_width();
        }
        Some((port, WindowBack::Returned)) => {
            let text = t!("workspace.window.signin-returned", port = port.to_string());
            ui.add(Text::new(text).color("success")).fill_width();
        }
        Some((port, WindowBack::TimedOut)) => {
            let text = t!("workspace.window.signin-timed-out", port = port.to_string(), minutes = minutes);
            ui.add(Text::new(text).color("warning")).fill_width();
        }
        Some((port, WindowBack::Taken)) => {
            ui.add(Text::new(t!("profiles.window.port-taken", port = port.to_string())).color("warning")).fill_width();
        }
        Some((port, WindowBack::Refused(reason))) => {
            let text = t!("profiles.window.no-listen", port = port.to_string(), reason = reason.clone());
            ui.add(Text::new(text).color("warning")).fill_width();
        }
    }
    ui.row(|ui| {
        ui.add(Button::new(t!("profiles.wizard.login-done")).variant("primary").on_press(Msg::LoginDone))
            .id("login-done");
        ui.add(Button::new(t!("profiles.wizard.login-stop")).on_press(Msg::LoginCancel)).id("login-stop");
        ui.spacer();
    })
    .gap(2)
    .fill_width();
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
        std::env::temp_dir().join(format!("qcode-profiles-screen-{}", std::process::id()))
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
            .map(|row| Status { name: row.profile.name.clone(), image, revision: Revision::Unknown, identity })
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
        let root = std::env::temp_dir().join(format!("qcode-profiles-loading-{}", std::process::id()));
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
        assert!(screen.contains("Claude Code, QCode recommended, subscription, on Arch Linux"), "{screen}");
        assert!(!screen.contains("Debian"), "a profile on Debian reads as it did before:\n{screen}");
    }

    #[test]
    fn a_profile_summary_says_what_it_runs_and_what_it_may_reach() {
        let harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
        let screen = harness.screen();
        assert!(screen.contains("Claude Code"), "{screen}");
        assert!(screen.contains("Claude Code, QCode recommended, subscription"), "{screen}");
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
    fn the_account_page_offers_each_harness_its_own_accounts_and_a_provider_only_where_one_was_proven() {
        let offered = [
            ("Gemini CLI", &["API key"][..], false),
            ("Codex", &["subscription", "API key"][..], true),
            ("Kimi Code CLI", &["subscription"][..], true),
            ("Qwen Code", &[][..], true),
        ];
        for (name, accounts, provider) in offered {
            let mut harness = loaded(Vec::new());
            harness.click_text("New profile").render();
            harness.click_text(name).render();
            next(&mut harness);
            next(&mut harness);
            next(&mut harness);
            let screen = harness.screen();
            assert!(screen.contains("What this profile signs in with"), "{name}:\n{screen}");
            assert_eq!(screen.contains("a provider of your own"), provider, "{name}:\n{screen}");
            for account in accounts {
                assert!(screen.contains(account), "{name} offers {account}:\n{screen}");
            }
            // A key is only offered where the harness keeps it in a file of its own.
            if !accounts.contains(&"API key") {
                assert!(!screen.contains("API key"), "{name}:\n{screen}");
            }
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
        assert!(screen.contains("Kept with this profile"), "{screen}");
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
    fn a_gemini_cli_profile_with_a_key_is_told_where_the_key_comes_from_and_what_the_dialog_asks() {
        let mut harness = with_engine();
        let keyed = Profile { account: AccountKind::ApiKey, ..profile("gemini-key", HarnessKind::GeminiCli) };
        harness.send(Msg::Loaded(Listing { profiles: vec![keyed], diagnostics: Vec::new() }));
        answer(&mut harness, Readiness::Present, Readiness::Missing);
        harness.click_text("Sign in").render();
        let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(screen.contains("Gemini CLI signs in with an API key from Google AI Studio"), "{screen}");
        assert!(screen.contains("aistudio.google.com/app/apikey"), "{screen}");
        assert!(screen.contains("Choose Use Gemini API Key, paste the key"), "{screen}");
        assert!(!screen.contains("signs in with its own flow"), "{screen}");
    }

    #[test]
    fn a_gemini_cli_profile_made_with_the_closed_sign_in_says_so_where_it_is_listed_and_still_loads() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profiles-closed-{stamp}"));
        let signed_in = profile("gemini-sub", HarnessKind::GeminiCli);
        Store::new(&folder).write_profile(&signed_in).expect("the definition is written");
        let state = Profiles::new(Some(folder.clone()), None).with_providers_file(None);
        let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, SIZE.1);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        // The store is read the way the screen always reads it, from the file on the disk.
        harness.send(Msg::Reload).render();
        let _ = std::fs::remove_dir_all(&folder);
        let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(harness.app().state.rows().len(), 1, "the profile still loads:\n{screen}");
        assert!(
            screen.contains("Google closed Gemini CLI's sign-in with a Google account for personal accounts"),
            "{screen}"
        );
        assert!(screen.contains("no longer supported for Gemini Code Assist for individuals"), "{screen}");
        assert!(screen.contains("Standard and Enterprise"), "{screen}");
        assert!(screen.contains("Antigravity IDE"), "{screen}");
        assert!(screen.contains("API key from Google AI Studio"), "{screen}");
        assert!(!screen.contains("could not be read"), "a profile that loaded is not an unreadable file:\n{screen}");
    }

    #[test]
    fn a_gemini_cli_profile_with_a_key_says_nothing_of_a_closed_sign_in() {
        let keyed = Profile { account: AccountKind::ApiKey, ..profile("gemini-key", HarnessKind::GeminiCli) };
        let harness = loaded(vec![keyed]);
        assert!(!harness.screen().contains("Google closed"), "{}", harness.screen());
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
            // The row that makes a new profile carries the icon set's own mark for adding, which
            // is a plus in two of the modes; it is an icon, not a drawn corner.
            let icons = harness.env().icons();
            let add = icons.glyph("add").into_owned();
            let screen = harness.screen().replace(&format!("{add} New profile"), "New profile");
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
        for text in ["Profiller", "QCode önerilen", "Giriş yap", "Çıkış yap", "abonelik"] {
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
        let offered = Template::offered(HarnessKind::ClaudeCode).len();
        assert_eq!(harness.screen().matches(square.as_str()).count(), offered, "{}", harness.screen());
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
    fn the_templates_carry_the_names_the_owner_chose_in_each_language() {
        let harness = wizard_on(2);
        let screen = harness.screen();
        for name in ["As it comes", "QCode recommended", "QCode extra", "Quvyta development"] {
            assert!(screen.contains(name), "`{name}`:\n{screen}");
        }
        assert_eq!(harness.app().state.draft().expect("open").template, Template::Recommended, "recommended is chosen");
        let mut harness = wizard_on(2);
        harness.set_locale("tr").render();
        let screen = harness.screen();
        for name in ["Olduğu gibi", "QCode önerilen", "QCode ekstra", "Quvyta geliştirme"] {
            assert!(screen.contains(name), "`{name}`:\n{screen}");
        }
        // The old names are gone from every screen, in every template's page.
        let names = [
            ("en", ["As it comes", "QCode recommended", "QCode extra", "Quvyta development"]),
            ("tr", ["Olduğu gibi", "QCode önerilen", "QCode ekstra", "Quvyta geliştirme"]),
        ];
        for (language, rows) in names {
            for row in rows {
                let mut harness = wizard_on(2);
                harness.set_locale(language).render();
                harness.click_text(row).render();
                let screen = harness.screen();
                for word in ["basic", "high", "yüksek"] {
                    let found =
                        screen.split(|c: char| !c.is_alphanumeric()).any(|each| each.eq_ignore_ascii_case(word));
                    assert!(!found, "{language} {row}: `{word}`:\n{screen}");
                }
            }
        }
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
        assert!(screen.contains("opencode, QCode recommended, free, no account"), "{screen}");
        assert!(harness.app().state.selected().expect("chosen").is_runnable());
        harness.send(Msg::SignInAsked).render();
        assert!(harness.app().state.draft().is_none(), "there is nothing to sign in to");
        harness.set_locale("tr").render();
        assert!(harness.screen().contains("Giriş gerekmez"), "{}", harness.screen());
        assert!(harness.screen().contains("ücretsiz, hesapsız"), "{}", harness.screen());
    }

    #[test]
    fn choosing_qcode_extra_says_what_it_installs_and_what_the_network_means_and_saves_it() {
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
        let screen = harness.screen();
        assert!(screen.contains("How much of the harness"), "{screen}");
        // QCode recommended installs graphify and Claude Code's starter plugins, and says where
        // they come from.
        for installed in ["graphify", "superpowers", "context7", "code-review", "security-guidance", "block-no-verify"]
        {
            assert!(screen.contains(installed), "recommended lists `{installed}`:\n{screen}");
        }
        assert!(!screen.contains("hookify"), "the rest are extra's:\n{screen}");
        assert!(screen.contains("about 30 MB together"), "{screen}");
        assert!(
            screen.contains("What QCode recommended adds is downloaded from Debian 13, PyPI, npm and GitHub"),
            "recommended's download is said:\n{screen}"
        );

        harness.click_text("QCode extra").render();
        let screen = harness.screen();
        assert_eq!(harness.app().state.draft().expect("open").template, Template::High, "{screen}");
        for extra in Template::High.extras(HarnessKind::ClaudeCode) {
            assert!(screen.contains(extra.id()), "`{}` is not listed:\n{screen}", extra.id());
        }
        assert!(screen.contains("about 60 MB together"), "{screen}");
        assert!(screen.contains("What QCode extra adds is downloaded"), "{screen}");
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

    #[test]
    fn opencode_offers_oh_my_opencode_slim_whose_image_carries_graphify_and_the_slim_team_and_not_oh_my_openagent() {
        // Chosen where the person chooses it: the button that opens the wizard, the harness's
        // row, the button that moves on and the template's own row.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profiles-slim-{stamp}"));
        let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
        harness.click_text("New profile").render();
        harness.click_text("opencode").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        assert!(harness.screen().contains("How much of the harness"), "{}", harness.screen());
        harness.click_text("oh my opencode slim").render();
        let screen = harness.screen();
        assert_eq!(harness.app().state.draft().expect("open").template, Template::Slim, "{screen}");
        assert!(screen.contains("oh-my-opencode-slim"), "what it installs is named:\n{screen}");
        // Never both in one profile: oh-my-openagent is not among what the page installs.
        assert!(!screen.contains("About 470 MB"), "{screen}");
        let extras = Template::Slim.extras(HarnessKind::OpenCode);
        assert!(extras.contains(&Extra::OhMyOpenCodeSlim) && !extras.contains(&Extra::OhMyOpenAgent), "{extras:?}");

        let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
        let files = profile.files();
        let settings = files
            .iter()
            .find(|file| file.path == ".config/opencode/opencode.json")
            .expect("opencode's settings are written");
        assert!(
            settings.contents.contains("file:///usr/local/npm/lib/node_modules/oh-my-opencode-slim"),
            "opencode loads the slim plugin from the image: {}",
            settings.contents
        );
        assert!(!settings.contents.contains("oh-my-openagent"), "{}", settings.contents);
        let own = files
            .iter()
            .find(|file| file.path == ".config/opencode/oh-my-opencode-slim.json")
            .expect("the plugin's own settings are written");
        assert!(own.contents.contains("\"autoUpdate\": false"), "{}", own.contents);
        let recipe = super::recipe::image(&profile);
        assert!(
            recipe.containerfile.contains("npm install -g --cache /tmp/qcode-npm-cache oh-my-opencode-slim"),
            "{}",
            recipe.containerfile
        );
        assert!(!recipe.containerfile.contains("oh-my-openagent"), "{}", recipe.containerfile);
        assert!(recipe.containerfile.contains("graphifyy"), "graphify is installed too: {}", recipe.containerfile);
        assert!(screen.contains("graphify"), "and listed where the template is chosen:\n{screen}");

        // Written and read back as the slim template.
        harness.app().state.store().expect("a store").write_profile(&profile).expect("the file is written");
        let name = format!("{}.toml", profile.name);
        let text = std::fs::read_to_string(folder.join("Profiles").join(&name)).expect("the file is there");
        assert!(text.contains("template = \"slim\""), "{text}");
        let read = crate::profile::Profile::parse(&name, &text);
        assert_eq!(read.profile.map(|profile| profile.template), Some(Template::Slim), "{:?}", read.diagnostics);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn oh_my_opencode_slim_is_offered_to_opencode_alone() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profiles-slim-claude-{stamp}"));
        let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
        harness.click_text("New profile").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        let screen = harness.screen();
        assert!(screen.contains("How much of the harness"), "{screen}");
        assert!(screen.contains("QCode extra"), "{screen}");
        assert!(!screen.contains("oh my opencode slim"), "slim is opencode's:\n{screen}");
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
        // the screen trims away. The first such cell after the label is its own switch, also when
        // rows stand in columns side by side; one cell further in is inside the track.
        let width = harness.buffer().area.width;
        let cell = (after..width)
            .find(|x| harness.bg(*x, y) != ground)
            .unwrap_or_else(|| panic!("`{label}` has a switch on its row:\n{screen}"));
        (i32::from(cell) + 1, i32::from(y))
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

    /// A stand-in podman for building an image that is already there again. It writes down every
    /// call; it has the profile's image under the identity `sha-old` with no revision label, the
    /// way an image an earlier QCode built has, until a build finishes, and then `sha-new` with
    /// the label the built Containerfile carried. The base image is always the current one. A build
    /// fails when `fail` exists in the folder.
    struct Rebuilt {
        folder: PathBuf,
        engine: Engine,
    }

    impl Rebuilt {
        fn new(name: &str) -> Self {
            let stamp =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let folder = std::env::temp_dir().join(format!("qcode-profiles-{name}-{stamp}"));
            std::fs::create_dir_all(&folder).expect("a scratch folder");
            let script = r#"#!/bin/sh
printf '%s\n' "$*" >> FOLDER/calls
case "$1 $2 $4" in
'image inspect {{.Id}}') [ -e FOLDER/label ] && echo sha-new || echo sha-old; exit 0 ;;
esac
[ "$1 $2 $5" = "image inspect qcode/base" ] && { echo BASE; exit 0; }
[ "$1 $2" = "image inspect" ] && { cat FOLDER/label 2>/dev/null; echo; exit 0; }
[ "$1" = build ] || exit 0
echo 'STEP 1/9: FROM qcode/base'
[ -e FOLDER/fail ] && { echo 'Error: building at STEP 4/9: exit status 1' >&2; exit 1; }
while [ "$#" -gt 0 ]; do [ "$1" = --file ] && file="$2"; shift; done
cp "$file" FOLDER/built
sed -n 's/^LABEL qcode.profile.revision="\(.*\)"$/\1/p' "$file" > FOLDER/label
echo 'COMMIT qcode/profile/claude-sub'
"#
            .replace("FOLDER", &folder.display().to_string())
            .replace("BASE", &crate::base::revision());
            let binary = folder.join("engine");
            std::fs::write(&binary, script).expect("the stand-in engine is written");
            std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
            let engine = Engine::new(crate::engine::EngineKind::Podman, &binary);
            Self { folder, engine }
        }

        /// The profiles screen on this engine, holding `profile` and with the engine's answers
        /// about it in, as the screen asks for them itself.
        fn screen(&self, profile: Profile) -> Harness<Host> {
            let state = Profiles::new(Some(self.folder.clone()), Some(self.engine.clone())).with_providers_file(None);
            let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, 40);
            harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
            harness.send(Msg::Loaded(Listing { profiles: vec![profile], diagnostics: Vec::new() })).render();
            harness
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.folder.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
        }

        fn builds(&self) -> Vec<String> {
            self.calls().into_iter().filter(|call| call.starts_with("build")).collect()
        }
    }

    impl Drop for Rebuilt {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }

    #[test]
    fn an_image_an_earlier_qcode_built_says_so_and_rebuilding_it_is_asked_first() {
        let engine = Rebuilt::new("rebuild-asked");
        let mut harness = engine.screen(profile("claude-sub", HarnessKind::ClaudeCode));
        let screen = harness.screen();
        assert!(screen.contains("Image ready"), "{screen}");
        assert!(screen.contains("This image was built by an earlier QCode."), "{screen}");
        harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
        // The question wraps inside its frame, so it is read as one line of words.
        let screen = harness.screen().split_whitespace().filter(|word| *word != "▌").collect::<Vec<_>>().join(" ");
        assert!(screen.contains("Rebuild the image of claude-sub?"), "{screen}");
        assert!(screen.contains("the harness is downloaded again"), "{screen}");
        assert!(screen.contains("the login and the conversations are kept"), "{screen}");
        assert!(screen.contains("keep running on the old image until they are opened again"), "{screen}");
        assert!(screen.contains("the old image stays"), "{screen}");
        assert!(engine.builds().is_empty(), "nothing is built before the answer: {:#?}", engine.calls());
        harness.press("esc").advance(std::time::Duration::from_millis(400));
        assert!(engine.builds().is_empty(), "a question put away builds nothing: {:#?}", engine.calls());
        assert!(harness.app().state.draft().is_none(), "{}", harness.screen());
    }

    #[test]
    fn rebuilding_builds_the_image_from_its_first_step_with_todays_recipe_and_lets_the_old_one_go() {
        let engine = Rebuilt::new("rebuild-done");
        let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
        let mut harness = engine.screen(chosen.clone());
        harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
        harness.click_text("Build it again").render();
        let builds = engine.builds();
        assert_eq!(builds.len(), 1, "{:#?}", engine.calls());
        assert!(builds[0].starts_with("build --no-cache --tag qcode/profile/claude-sub --file "), "{builds:#?}");
        let built = std::fs::read_to_string(engine.folder.join("built")).expect("a Containerfile was built");
        let recipe = recipe::image(&chosen);
        assert_eq!(built, recipe.labelled(), "today's recipe, labelled with its revision");
        let screen = harness.screen();
        assert!(screen.contains("Rebuilding the image of claude-sub"), "{screen}");
        assert!(screen.contains("STEP 1/9: FROM qcode/base"), "the build's own lines are shown:\n{screen}");
        assert!(screen.contains("The image of claude-sub is built again."), "{screen}");
        let calls = engine.calls();
        assert!(calls.contains(&"image rm sha-old".to_owned()), "the image it replaced is let go: {calls:#?}");
        assert!(!calls.iter().any(|call| call.contains("--force")), "never by force: {calls:#?}");
        assert!(
            !std::fs::read_dir(&engine.folder)
                .expect("the folder")
                .flatten()
                .any(|entry| entry.file_name() == "Profiles"),
            "the definition is not written again"
        );
        harness.click_text("Close").render();
        let screen = harness.screen();
        assert!(harness.app().state.draft().is_none(), "{screen}");
    }

    #[test]
    fn a_rebuilt_image_no_longer_says_an_earlier_qcode_built_it() {
        let engine = Rebuilt::new("rebuild-current");
        let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
        std::fs::write(engine.folder.join("label"), recipe::image(&chosen).revision()).expect("the label is set");
        let harness = engine.screen(chosen);
        let screen = harness.screen();
        assert!(screen.contains("Image ready"), "{screen}");
        assert!(!screen.contains("built by an earlier QCode"), "{screen}");
    }

    /// The definition file of `name` in the store of `engine`, as the disk has it.
    fn definition(engine: &Rebuilt, name: &str) -> String {
        std::fs::read_to_string(engine.folder.join("Profiles").join(format!("{name}.toml"))).unwrap_or_default()
    }

    /// Opens the wizard on the chosen profile the way the person does, from its button under the
    /// rows, and walks `pages` pages on with the wizard's own Next.
    fn edit(harness: &mut Harness<Host>, pages: usize) {
        harness.click_text("Edit").render();
        assert!(harness.app().state.draft().is_some_and(Draft::is_editing), "{}", harness.screen());
        for _ in 0..pages {
            harness.click_text("Next").render();
        }
    }

    #[test]
    fn editing_a_profile_opens_the_wizard_on_it_with_its_name_kept() {
        let engine = Rebuilt::new("edit-open");
        let mut chosen = profile("claude-sub", HarnessKind::ClaudeCode);
        chosen.network = NetworkMode::None;
        let mut harness = engine.screen(chosen);
        edit(&mut harness, 0);
        let screen = harness.screen();
        assert!(screen.contains("The name stays as it is"), "{screen}");
        assert!(screen.contains("claude-sub"), "{screen}");
        assert!(!screen.contains("The name of the definition file"), "no field that would not take: {screen}");
        // Where the name stands, a click and typing change nothing: there is no field to take it.
        let (x, y) = harness.find("claude-sub").expect("the name is shown");
        harness.click(x + 2, y).render();
        harness.type_text("-new").render();
        assert_eq!(harness.app().state.draft().expect("open").name, "claude-sub", "{}", harness.screen());
        let draft = harness.app().state.draft().expect("open");
        assert_eq!(draft.network, NetworkMode::None, "everything else comes as the profile has it");
        // Leaving before the image page writes nothing.
        harness.click_text("Cancel").render();
        assert_eq!(definition(&engine, "claude-sub"), "", "nothing was saved");
        assert!(engine.builds().is_empty(), "{:#?}", engine.calls());
    }

    #[test]
    fn a_change_the_image_is_built_from_is_saved_through_a_rebuild() {
        let engine = Rebuilt::new("edit-image");
        let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
        let mut harness = engine.screen(chosen.clone());
        edit(&mut harness, 2);
        harness.click_text("QCode extra").render();
        // Account, permissions, then the image page, which saves by building again.
        edit_on(&mut harness, 3);
        let builds = engine.builds();
        assert_eq!(builds.len(), 1, "one rebuild: {:#?}", engine.calls());
        assert!(builds[0].starts_with("build --no-cache --tag qcode/profile/claude-sub "), "{builds:#?}");
        let changed = Profile { template: Template::High, ..chosen };
        let built = std::fs::read_to_string(engine.folder.join("built")).expect("a Containerfile was built");
        assert_eq!(built, recipe::image(&changed).labelled(), "the changed profile's recipe");
        let text = definition(&engine, "claude-sub");
        assert!(text.contains("template = \"high\""), "{text}");
        let read = Profile::parse("claude-sub.toml", &text).profile.expect("the file reads");
        assert_eq!(read, changed);
        let screen = harness.screen();
        assert!(screen.contains("is built again with the changes"), "{screen}");
        // The login it has stays: the account and the harness did not change.
        assert_eq!(harness.app().state.draft().expect("open").stages(), Stage::WITHOUT_LOGIN);
        harness.click_text("Finish").render();
        assert!(harness.app().state.draft().is_none(), "{}", harness.screen());
        assert_eq!(harness.app().state.made, None, "a changed profile is not a new one");
    }

    #[test]
    fn a_change_of_account_brings_the_sign_in_back_and_builds_nothing() {
        let engine = Rebuilt::new("edit-account");
        let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
        let mut harness = engine.screen(chosen.clone());
        edit(&mut harness, 3);
        let (x, y) = radio_mark(&harness, "API key");
        harness.click(i32::from(x) + 1, i32::from(y)).render();
        assert_eq!(harness.app().state.draft().expect("open").account, AccountKind::ApiKey, "{}", harness.screen());
        edit_on(&mut harness, 2);
        assert!(engine.builds().is_empty(), "nothing the image is built from changed: {:#?}", engine.calls());
        let read = Profile::parse("claude-sub.toml", &definition(&engine, "claude-sub")).profile.expect("saved");
        assert_eq!(read, Profile { account: AccountKind::ApiKey, ..chosen });
        // The login it has was made for a subscription; the sign-in page is the next one.
        harness.click_text("Next").render();
        let draft = harness.app().state.draft().expect("open");
        assert_eq!(draft.stage, Stage::Login, "{}", harness.screen());
        assert!(harness.screen().contains("Open the sign-in"), "{}", harness.screen());
    }

    #[test]
    fn a_change_of_the_network_is_saved_without_a_rebuild_and_remakes_a_stopped_container() {
        let engine = Rebuilt::new("edit-network");
        let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
        let mut harness = engine.screen(chosen.clone());
        edit(&mut harness, 4);
        let (x, y) = radio_mark(&harness, "none");
        harness.click(i32::from(x) + 1, i32::from(y)).render();
        edit_on(&mut harness, 1);
        assert!(engine.builds().is_empty(), "the network is the containers', not the image's: {:#?}", engine.calls());
        let screen = harness.screen();
        assert!(screen.contains("the image is kept as it is"), "{screen}");
        assert!(screen.contains("made again with the changes the next time it starts"), "{screen}");
        let read = Profile::parse("claude-sub.toml", &definition(&engine, "claude-sub")).profile.expect("saved");
        assert_eq!(read, Profile { network: NetworkMode::None, ..chosen.clone() });

        // A workspace's stopped container of the profile, made from the plan as it was, is made
        // again from the one the saved profile gives, with the same home volume.
        let workspace = crate::store::WorkspaceId::parse("firefly").expect("an id");
        let paths = crate::store::WorkspacePaths {
            root: engine.folder.join("w"),
            file: engine.folder.join("w").join("workspace.qcode"),
            code: engine.folder.join("w").join("Work"),
            assets: engine.folder.join("w").join("Assets"),
            harness: engine.folder.join("w").join("Containers").join("Harness"),
        };
        let user = crate::engine::HostUser::Ids { uid: 1000, gid: 1000 };
        let stand_in = engine.folder.join("containers");
        let before = crate::ui::workspace::ContainerPlan::profile(&workspace, &paths, &chosen);
        let after = crate::ui::workspace::ContainerPlan::profile(&workspace, &paths, &read);
        let podman = Engine::new(crate::engine::EngineKind::Podman, &stand_in);
        assert_ne!(before.digest(&podman, user), after.digest(&podman, user));
        let script = r#"#!/bin/sh
printf '%s\n' "$*" >> FOLDER/container-calls
case "$1 $2 $4" in
'container inspect {{.State.Status}}') echo exited; exit 0 ;;
'container inspect {{.Image}}') echo sha-same; exit 0 ;;
'container inspect '*) echo DIGEST; exit 0 ;;
'image inspect {{.Id}}') case "$5" in qcode/workspace/*) exit 1 ;; esac; echo sha-same; exit 0 ;;
esac
exit 0
"#
        .replace("FOLDER", &engine.folder.display().to_string())
        .replace("DIGEST", &before.digest(&podman, user));
        std::fs::write(&stand_in, script).expect("the stand-in engine");
        std::fs::set_permissions(&stand_in, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        crate::ui::workspace::ensure_running(&podman, &after, user).expect("it comes up");
        let calls = std::fs::read_to_string(engine.folder.join("container-calls")).expect("calls");
        let calls: Vec<&str> = calls.lines().collect();
        let removed =
            calls.iter().position(|call| call.starts_with("rm ") && call.contains("qcode-firefly-claude-sub"));
        let created = calls.iter().position(|call| call.starts_with("create ") && call.contains("--network=none"));
        assert!(removed.is_some() && created.is_some() && removed < created, "made again: {calls:#?}");
        let label = format!("qcode.plan={}", after.digest(&podman, user));
        assert!(calls.iter().any(|call| call.contains(&label)), "with the new plan's label: {calls:#?}");
        assert!(!calls.iter().any(|call| call.starts_with("volume rm")), "the home stays: {calls:#?}");
    }

    /// Walks `pages` more pages on with the wizard's own Next.
    fn edit_on(harness: &mut Harness<Host>, pages: usize) {
        for _ in 0..pages {
            harness.click_text("Next").render();
        }
    }

    #[test]
    fn a_rebuild_that_fails_says_the_old_image_is_kept_and_removes_nothing() {
        let engine = Rebuilt::new("rebuild-failed");
        std::fs::write(engine.folder.join("fail"), "").expect("the build will fail");
        let mut harness = engine.screen(profile("claude-sub", HarnessKind::ClaudeCode));
        harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
        harness.click_text("Build it again").render();
        let screen = harness.screen();
        assert!(screen.contains("The rebuild failed."), "{screen}");
        assert!(screen.contains("the image that was there before is kept"), "{screen}");
        assert!(screen.contains("exit status 1"), "the engine's own words:\n{screen}");
        let calls = engine.calls();
        assert!(!calls.iter().any(|call| call.starts_with("image rm")), "the old image is kept: {calls:#?}");
    }

    #[test]
    fn a_profile_without_an_image_offers_no_rebuild() {
        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }));
        answer(&mut harness, Readiness::Missing, Readiness::Missing);
        harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
        assert!(!harness.screen().contains("Rebuild the image of"), "{}", harness.screen());
    }

    #[test]
    fn the_wizard_says_from_the_account_on_that_it_signs_in_once_for_every_workspace_and_what_skipping_costs() {
        let (folder, mut harness) = recording("sign-in-step");
        harness.click_text("New profile").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        let words = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
        let account = words(&harness);
        assert!(account.contains("The last step of this wizard signs Claude Code in, once."), "{account}");
        assert!(account.contains("never asks for the account again"), "{account}");
        harness.click_text("Next").render();
        // Arriving on the image page is what starts the build; the stand-in engine finishes it.
        harness.click_text("Next").render();
        assert!(matches!(harness.app().state.draft().expect("open").build, Build::Done), "{}", harness.screen());
        harness.click_text("Next").render();
        let page = words(&harness);
        assert!(page.contains("Sign in"), "the step is named:\n{page}");
        assert!(page.contains("the login is kept with this profile"), "{page}");
        assert!(page.contains("You can finish without signing in"), "{page}");
        assert!(page.contains("each workspace then asks for the account"), "{page}");

        // The engine's answer that the login was found and stored, which only a container gives.
        harness.send(Msg::LoginStored(Ok(3))).render();
        let page = words(&harness);
        assert!(page.contains("Signed in. 3 files were stored"), "{page}");
        assert!(page.contains("Kept with this profile. Every workspace that opens it from now on"), "{page}");
        assert!(!page.contains("You can finish without signing in"), "said only before a login:\n{page}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn signing_in_again_from_the_list_does_not_speak_of_finishing_without_it() {
        let mut harness = with_engine();
        harness.send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }));
        answer(&mut harness, Readiness::Present, Readiness::Missing);
        harness.click_text("claude-sub").render();
        harness.click_text("Sign in").render();
        let page = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(page.contains("the login is kept with this profile"), "{page}");
        assert!(!page.contains("You can finish without signing in"), "{page}");
    }

    /// The wizard on its image page with the build under way. A build in the harness would run to
    /// its end before the first frame, so the state it stands in while it runs is set here, the way
    /// the task leaves it the moment it starts.
    fn building(reduced: bool) -> Harness<Host> {
        let mut state = Profiles::new(Some(root()), None).with_providers_file(None);
        drop(update(&mut state, Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })));
        drop(update(&mut state, Msg::New));
        let task = Task::new("build", |_| Ok(Msg::LoginDiscarded));
        if let Some(draft) = state.draft.as_mut() {
            draft.stage = Stage::Image;
            draft.build = Build::Running(task.id());
        }
        let mut harness = Harness::with_env(Host { state }, env(), 96, 60);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(reduced).render();
        harness
    }

    /// Whether the letters of `text` change colour while time passes.
    fn shines(harness: &mut Harness<Host>, text: &str) -> bool {
        let colours = |harness: &Harness<Host>| {
            let (x, y) = harness.find(text).unwrap_or_else(|| panic!("`{text}`:\n{}", harness.screen()));
            let (x, y) = (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"));
            let width = u16::try_from(text.chars().count()).expect("short");
            (x..x + width).map(|column| harness.fg(column, y)).collect::<Vec<_>>()
        };
        let mut seen = vec![colours(harness)];
        for _ in 0..8 {
            harness.advance(std::time::Duration::from_millis(120)).render();
            seen.push(colours(harness));
        }
        seen.windows(2).any(|pair| pair[0] != pair[1])
    }

    /// The wizard on its sign-in page with the login at `login`, a state the work leaves it in
    /// only while it runs.
    fn signing_in(login: Login) -> Harness<Host> {
        let mut state = Profiles::new(Some(root()), None).with_providers_file(None);
        drop(update(&mut state, Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })));
        drop(update(&mut state, Msg::New));
        if let Some(draft) = state.draft.as_mut() {
            draft.stage = Stage::Login;
            draft.build = Build::Done;
            draft.login = login;
        }
        let mut harness = Harness::with_env(Host { state }, env(), 96, 60);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).render();
        harness
    }

    #[test]
    fn the_state_of_every_profile_stands_in_columns_and_the_actions_once_under_the_rows() {
        let free = Profile { account: AccountKind::Free, ..profile("open-free", HarnessKind::OpenCode) };
        let mut harness = loaded(vec![
            profile("claude-main", HarnessKind::ClaudeCode),
            free,
            profile("codex-work", HarnessKind::Codex),
        ]);
        let answers = vec![
            Status {
                name: SafeName::parse("claude-main").expect("safe"),
                image: Readiness::Present,
                revision: Revision::Current,
                identity: Readiness::Present,
            },
            Status {
                name: SafeName::parse("open-free").expect("safe"),
                image: Readiness::Present,
                revision: Revision::Current,
                identity: Readiness::Missing,
            },
            Status {
                name: SafeName::parse("codex-work").expect("safe"),
                image: Readiness::Missing,
                revision: Revision::Unknown,
                identity: Readiness::Missing,
            },
        ];
        harness.send(Msg::Probed(answers)).render();
        harness.click_text("claude-main").render();
        let screen = harness.screen();
        let lines: Vec<&str> = screen.lines().collect();
        let row = |name: &str| lines.iter().position(|line| line.contains(name)).expect("the profile's row");
        let column = |line: usize, words: &str| {
            let at = lines[line].find(words).unwrap_or_else(|| panic!("`{words}`:\n{screen}"));
            lines[line][..at].chars().count()
        };
        let (main, free, codex) = (row("claude-main"), row("open-free"), row("codex-work"));
        // Each answer on its profile's own row, and the answers of one kind under each other.
        assert_eq!(column(main, "Image ready"), column(codex, "No image"), "{screen}");
        assert_eq!(column(main, "Image ready"), column(free, "Image ready"), "{screen}");
        assert_eq!(column(main, "Signed in"), column(free, "No sign-in needed"), "{screen}");
        assert_eq!(column(main, "Signed in"), column(codex, "Not signed in"), "{screen}");
        // The actions stand once, one empty row under the last profile.
        assert_eq!(screen.matches("Rebuild image").count(), 1, "{screen}");
        assert_eq!(row("Rebuild image"), codex + 2, "{screen}");
        assert!(lines[row("Rebuild image")].contains("Delete"), "{screen}");
    }

    #[test]
    fn a_login_being_opened_or_stored_shines() {
        let mut opening = signing_in(Login::Opening);
        assert!(shines(&mut opening, "Opening the container"), "{}", opening.screen());
        let mut storing = signing_in(Login::Storing);
        assert!(shines(&mut storing, "Looking for the login"), "{}", storing.screen());
    }

    #[test]
    fn a_build_under_way_shines_and_stands_still_under_reduced_motion() {
        let mut harness = building(false);
        assert!(shines(&mut harness, "Building the image"), "{}", harness.screen());
        let mut still = building(true);
        assert!(!shines(&mut still, "Building the image"), "{}", still.screen());
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
        harness.click_text("QCode extra").render();
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
        harness.click_text("QCode extra").render();

        // A plugin QCode extra alone installs.
        let hookify = Extra::parse("hookify").expect("a part of QCode extra");
        let has = |harness: &Harness<Host>, extra| harness.app().state.draft().expect("open").has(extra);
        for extra in Template::High.extras(HarnessKind::ClaudeCode) {
            assert!(has(&harness, extra), "{extra:?} is on until switched off:\n{}", harness.screen());
        }
        let (x, y) = switch_of(&harness, "hookify");
        harness.click(x, y).render();
        assert!(!has(&harness, hookify), "a click switches it off:\n{}", harness.screen());
        // The keys reach the same switch: the list has the row the click was on.
        harness.press("space").render();
        assert!(has(&harness, hookify), "space switches it back on:\n{}", harness.screen());
        harness.press("space").render();
        assert!(!has(&harness, hookify), "and off again:\n{}", harness.screen());
        for extra in Template::High.extras(HarnessKind::ClaudeCode).into_iter().filter(|extra| *extra != hookify) {
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
        assert!(text.contains("template = \"high\""), "{text}");
        assert!(text.contains("[additions]\nhookify = false\n"), "{text}");
        let saved = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
        assert_eq!(saved.without, [hookify], "{text}");

        let image = std::fs::read_to_string(folder.join("built-qcode-profile-claude-code"))
            .expect("the profile's image was built from a Containerfile");
        assert!(!image.contains("hookify"), "{image}");
        for plugin in crate::profile::CLAUDE_PLUGINS.iter().filter(|plugin| !plugin.starts_with("hookify@")) {
            assert!(image.contains(&format!("claude plugin install {plugin}")), "{plugin}: {image}");
        }
        assert!(image.contains("pipx install --global graphifyy"), "graphify stays: {image}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn quvyta_development_is_offered_and_saved_with_chromium_switched_off_and_built_with_rust() {
        // From where the person's hand is: the buttons, the template's row, the switch.
        let (folder, mut harness) = recording("quvyta-dev");
        harness.click_text("New profile").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        harness.click_text("Quvyta development").render();
        let screen = harness.screen();
        assert_eq!(harness.app().state.draft().expect("open").template, Template::QuvytaDev, "{screen}");
        assert!(screen.contains("Rust's stable toolchain"), "what it adds is said:\n{screen}");
        assert!(screen.contains("GitHub, rustup.rs and"), "and where it comes from:\n{screen}");
        for part in ["graphify", "superpowers", "chromium"] {
            assert!(screen.contains(part), "`{part}` is listed:\n{screen}");
        }
        let has = |harness: &Harness<Host>| harness.app().state.draft().expect("open").has(Extra::Chromium);
        assert!(has(&harness), "on until switched off");
        let (x, y) = switch_of(&harness, "chromium");
        harness.click(x, y).render();
        assert!(!has(&harness), "a click switches it off:\n{}", harness.screen());

        harness.click_text("Next").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        let build = &harness.app().state.draft().expect("open").build;
        assert!(matches!(build, Build::Done), "the build ended well: {build:?}\n{}", harness.screen());

        let text =
            std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the profile is saved");
        assert!(text.contains("template = \"quvyta-dev\""), "{text}");
        assert!(text.contains("[additions]\nchromium = false\n"), "{text}");
        let saved = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
        assert_eq!((saved.template, saved.without), (Template::QuvytaDev, vec![Extra::Chromium]), "{text}");
        let image = std::fs::read_to_string(folder.join("built-qcode-profile-claude-code"))
            .expect("the profile's image was built from a Containerfile");
        assert!(image.contains("https://sh.rustup.rs"), "{image}");
        assert!(!image.contains("chromium"), "{image}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn on_ubuntu_quvyta_development_waits_on_its_page_until_chromium_is_switched_off() {
        let (folder, mut harness) = recording("quvyta-dev-ubuntu");
        harness.click_text("New profile").render();
        harness.click_text("Next").render();
        harness.click_text("Ubuntu 24.04 LTS").render();
        harness.click_text("Next").render();
        harness.click_text("Quvyta development").render();
        harness.click_text("Next").render();
        let stage = |harness: &Harness<Host>| harness.app().state.draft().expect("open").stage;
        assert_eq!(stage(&harness), Stage::Template, "{}", harness.screen());
        let screen = harness.screen();
        assert!(screen.contains("Ubuntu 24.04 LTS packages Chromium only as a snap"), "{screen}");
        let (x, y) = switch_of(&harness, "chromium");
        harness.click(x, y).render();
        assert!(!harness.screen().contains("only as a snap"), "{}", harness.screen());
        harness.click_text("Next").render();
        assert_eq!(stage(&harness), Stage::Account, "{}", harness.screen());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_sixteen_plugins_of_qcode_extra_fit_a_screen_of_forty_rows_each_with_its_switch() {
        let mut harness = wizard_on(2);
        harness.resize(100, 40).render();
        harness.click_text("QCode extra").render();
        let screen = harness.screen();
        for extra in Template::High.extras(HarnessKind::ClaudeCode) {
            assert!(screen.contains(extra.id()), "`{}` is not listed:\n{screen}", extra.id());
            if matches!(extra, Extra::Plugin(_)) {
                // Every plugin keeps a switch of its own, reached by a click on it.
                let (x, y) = switch_of(&harness, extra.id());
                harness.click(x, y).render();
                assert!(
                    !harness.app().state.draft().expect("open").has(extra),
                    "{}:\n{}",
                    extra.id(),
                    harness.screen()
                );
                harness.click(x, y).render();
                assert!(harness.app().state.draft().expect("open").has(extra), "{}", extra.id());
            }
        }
        assert!(screen.contains("needs the network"), "{screen}");
        assert!(screen.contains("Next"), "the buttons are still on screen:\n{screen}");
    }

    #[test]
    fn opencode_under_every_qcode_template_lists_oh_my_openagent_and_not_the_plugins() {
        for row in ["QCode recommended", "QCode extra"] {
            let mut harness = wizard_on(0);
            harness.click_text("opencode").render();
            harness.click_text("Next").render();
            harness.click_text("Next").render();
            harness.click_text(row).render();
            let screen = harness.screen();
            assert!(screen.contains("oh-my-openagent"), "{row}: {screen}");
            assert!(screen.contains("graphify"), "{row}: {screen}");
            assert!(!screen.contains("superpowers"), "{row}: {screen}");
            assert!(screen.contains("npm and GitHub"), "{row}: the download is said: {screen}");
        }
    }

    #[test]
    fn a_build_of_a_qcode_template_that_could_not_download_says_whose_it_was() {
        for (row, failed, name) in [
            ("QCode extra", recipe::EXTRA_FAILED, "QCode extra"),
            ("QCode recommended", recipe::RECOMMENDED_FAILED, "QCode recommended"),
        ] {
            let mut harness = wizard_on(2);
            harness.click_text(row).render();
            for _ in 0..3 {
                harness.send(Msg::Next);
            }
            let said = format!(
                "{failed} could not install graphify. What {name} adds is downloaded while the image is built, so the build needs the network."
            );
            harness
                .send(Msg::BuildLine(said))
                .send(Msg::BuildEnded(Err(Problem::Refused("exit status 1".to_owned()))))
                .render();
            let screen = harness.screen();
            assert!(screen.contains(&format!("{name} could not download what it adds")), "{screen}");
        }
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
    fn the_lines_of_the_template_parts_are_really_in_every_language() {
        // A file whose values are English still has every key. What each language is checked for
        // is its own words: with the names of the tools taken out, most of what is left must not
        // be words of the English line.
        let keys = [
            "profiles.wizard.high-graphify",
            "profiles.wizard.high-claude-plugins",
            "profiles.wizard.high-omo",
            "profiles.wizard.high-offline-claude",
            "profiles.wizard.high-offline-opencode",
            "profiles.wizard.high-offline",
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
