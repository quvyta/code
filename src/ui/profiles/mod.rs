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

mod actions;
mod builds;
#[cfg(test)]
mod cleanup_live;
#[cfg(test)]
mod inbox_tests;
mod list;
#[cfg(test)]
mod live;
// Public for the README's recording (tests/readme_gif.rs), which builds a profile's image with
// QCode's own recipe; it is not part of the library's interface, so it is left out of its documentation.
#[doc(hidden)]
pub mod recipe;
mod remove;
pub(crate) mod shell;
mod shell_page;
#[cfg(test)]
mod shell_tests;
mod status;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod window_tests;
mod wizard;
mod wizard_view;
pub(crate) mod work;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use qframe::diagnostics::Diagnostic;
use qframe::prelude::*;
use qframe::widgets::{LogLevel, LogLine, TerminalEvent, TerminalSession, Toast};

use crate::base::Os;
use crate::desktop::login::{Seen, SignIn};
use crate::desktop::{Display, NoDisplay, callback};
use crate::engine::Engine;
use crate::profile::own::Own;
use crate::profile::{AccountKind, Extra, MountAccess, Profile, SafeName, Template};
use crate::provider::{ProviderEntry, Providers};
use crate::store::Store;
use crate::ui::stalling;

pub use list::{closed_sign_in, entry, hints, view};
pub use shell_page::{ShellMsg, ShellPage, Side as ShellSide, Stage as ShellStage};
pub use status::{Readiness, Revision, Row, Status};
pub use wizard::{Blocked, Build, Draft, Login, Page, PickRow, Stage, Unfinished, WindowBack, WindowLogin};
pub use work::Problem;

use actions::{
    arrive, ask_delete, ask_rebuild, ask_sign_out, cancel_login, current_window, delete, delete_question, deleted,
    discard, failure_toast, leave, load_providers, login_event, next, probe, reload, report, sign_out, start_build,
    start_login, store_login, window_asked, window_opened,
};

/// Width of the wizard's pages: wide enough for every step to keep its name in the row of
/// steps, and still one column of text. A narrower terminal shrinks it, and the steps fall back
/// to their markers.
///
/// Measured against the seven English steps, whose row is 104 cells, with the frame around it;
/// German, French, Turkish, Russian, Portuguese and Chinese fit too. Spanish and Japanese name
/// their steps longer and show the markers with the current step's name, as a narrow terminal
/// does.
const PAGE_WIDTH: u16 = 110;

/// Width of the list of profiles: a page of its own, since it is a table of five columns of words
/// and the words do not fit the page the settings screen keeps. The longest row the list can make
/// — a profile with a long name, the longest harness name, "signed in inside its window" for its
/// account and both the longest answers of the engine — is 98 cells, so nothing in the last column
/// is cut.
const LIST_WIDTH: u16 = 100;

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
    /// A model of the chosen provider, or one of its lineups, was chosen by its place in the list
    /// the filter leaves.
    PickProviderModel(usize),
    /// The filter of the model list was typed.
    ModelFilter(String),
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
    /// The build was looked at, which is what says whether its log has gone quiet for long enough
    /// to be stuck.
    BuildLooked(std::time::Instant),
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
    /// The lengths a build's silence is judged by. The product's are the measured ones; a test
    /// shortens them, since a test cannot wait five minutes for a warning.
    stall: stalling::Lengths,
    /// Whether the next look at a build is timed, so that one is timed at a time.
    watching: bool,
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
            stall: stalling::Lengths::default(),
            watching: false,
        }
    }

    /// The same screen, opening a sign-in window on `display` rather than on the session this
    /// process was started in: a test's own stand-in, never the person's screen.
    #[must_use]
    pub fn showing_on(mut self, display: Result<Display, NoDisplay>) -> Self {
        self.display = display;
        self
    }

    /// The same screen, saying a build is stuck after `lengths.quiet` of silence and looking every
    /// `lengths.look`, which the product uses as [`stalling::QUIET`] and [`stalling::LOOK`].
    ///
    /// Only for a test: the product uses those two measured lengths, since a test cannot wait five
    /// minutes to hear a warning and cannot ask the engine for a build that really takes that long.
    #[cfg(test)]
    #[must_use]
    pub fn stalling(mut self, lengths: stalling::Lengths) -> Self {
        self.stall = lengths;
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
            // The page's first question, the harness, above the name that follows from it.
            Command::focus("profile-harness")
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
            let Some(draft) = &mut state.draft else { return Command::none() };
            draft.back();
            arrive(draft)
        }
        Msg::Step(index) => {
            let Some(draft) = &mut state.draft else { return Command::none() };
            draft.go_to(index);
            arrive(draft)
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
            if let Some(draft) = &mut state.draft
                && let Some(harness) = draft.harnesses().get(index).copied()
            {
                draft.choose_harness(harness);
            }
            Command::none()
        }
        Msg::PickOs(index) => {
            if let (Some(draft), Some(os)) = (&mut state.draft, Os::ALL.get(index)) {
                draft.choose_os(*os);
            }
            Command::none()
        }
        Msg::PickTemplate(index) => {
            if let Some(draft) = &mut state.draft
                && let Some(template) = Template::offered(draft.harness).get(index).copied()
            {
                draft.choose(template);
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
                draft.choose_pick_row(index);
            }
            Command::none()
        }
        Msg::ModelFilter(query) => {
            if let Some(draft) = &mut state.draft {
                draft.model_filter = query;
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
            if let Some(draft) = &mut state.draft
                && let Some(mode) = draft.networks().get(index).copied()
            {
                draft.network = mode;
            }
            Command::none()
        }
        Msg::BuildStart => start_build(state),
        Msg::BaseImageStarted => {
            if let Some(draft) = &mut state.draft {
                draft.log.push(LogLine::new(LogLevel::Info, t!("profiles.wizard.build-base")));
                // A line of the build's own is what ends the silence the page warns about.
                draft.build_said();
            }
            Command::none()
        }
        Msg::BuildLine(text) => {
            if let Some(draft) = &mut state.draft {
                draft.log.push(LogLine::new(LogLevel::Info, text));
                draft.build_said();
            }
            Command::none()
        }
        // The look came round: the draft is marked by how long its log has been quiet, and the
        // next look is timed while a build is still running.
        Msg::BuildLooked(now) => builds::looked(state, now),
        Msg::BuildEnded(result) => {
            let Some(draft) = &mut state.draft else { return Command::none() };
            match result {
                Ok(()) => draft.build = Build::Done,
                Err(problem) => {
                    report(&mut draft.log, &problem);
                    draft.build = if problem == Problem::Cancelled { Build::Stopped } else { Build::Failed(problem) };
                }
            }
            draft.build_ended();
            Command::none()
        }
        Msg::BuildCancel => {
            let Some(draft) = &mut state.draft else { return Command::none() };
            let Build::Running(id) = draft.build else { return Command::none() };
            // The build removes the half-made image itself as soon as it notices the stop.
            draft.build = Build::Stopped;
            draft.build_ended();
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
