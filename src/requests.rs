//! What the screens ask of the application: a step of the setup wizard run again, the engine
//! looked for and switched, the settings screen's requests, signing out and leaving.

use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::widgets::Toast;

use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, detect};
use crate::profile::identity::{RefreshError, Refreshed};
use crate::profile::{Profile, SafeName};
use crate::store::{SetupStep, WorkspaceId};
use crate::ui::settings::engine::{EngineState, Health, Trouble};
use crate::ui::settings::{Request, Settings as SettingsScreen};
use crate::ui::setup::Setup;
use crate::ui::setup::gates::{EngineCheck, Gates};
use crate::ui::setup::install::Installer;
use crate::{Msg, Page, QCode, engine, profile, service, ui};

impl QCode {
    /// Runs one step of the setup wizard on its own, which is what the repair strip and the
    /// settings screen ask for.
    pub(super) fn repair(&mut self, step: SetupStep) -> Command<Msg> {
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
    pub(super) fn wizard(&mut self, message: ui::setup::Msg) -> Command<Msg> {
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
        let update_notice = self.settings.update_notice();
        self.settings = SettingsScreen::new(&self.config, self.engine.clone())
            .with_service(service)
            .with_left_behind(left_behind)
            .with_update_notice(update_notice);
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
    pub(super) fn looked(&mut self, kind: EngineKind, found: Result<Box<Engine>, Trouble>) -> Command<Msg> {
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
        let command = command.map(Msg::Settings);
        // The engine chosen in the settings answers: the page that follows the change opens. One
        // that does not answer yet keeps it waiting, so it still opens once the engine is started.
        let from = match self.switching_from {
            Some(from) if from != kind && self.found.is_some() => self.switching_from.take(),
            _ => None,
        };
        match from {
            Some(from) => {
                self.open_switch((from, kind), false);
                Command::batch([command, self.survey(), self.take_focus()])
            }
            None => command,
        }
    }

    /// Asks both engines of the open switch page what the switch involves.
    pub(super) fn survey(&self) -> Command<Msg> {
        match (&self.switch, self.page()) {
            (Some(page), Page::Switch) => ui::switch::survey(page).map(Msg::Switch),
            _ => Command::none(),
        }
    }

    /// Does what the switch page asked for.
    pub(super) fn switched(&mut self, request: ui::switch::Request) -> Command<Msg> {
        match request {
            // The offer at start was taken: the engine that answers is the one from now on.
            ui::switch::Request::Use(kind) => {
                if let Some(page) = self.switch.as_mut() {
                    page.taken();
                }
                self.config.set_engine_kind(kind.name());
                Command::batch([self.save(), self.look()])
            }
            ui::switch::Request::Leave => {
                self.switch = None;
                if !self.router.back() {
                    self.router.replace(Page::Home);
                }
                self.take_focus()
            }
        }
    }

    /// Does what the settings screen asked for, past every question it needed.
    pub(super) fn asked(&mut self, request: Request) -> Command<Msg> {
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
                // Remembered until the new engine answers: the page that follows the change is
                // about an engine that works, and one that does not is the repair strip's.
                let before = self.config.engine_kind().and_then(EngineKind::from_name);
                self.switching_from = before.filter(|before| *before != kind);
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
            Request::AskFirst(ask) => {
                self.config.set_ask_first(ask);
                if let Some(screen) = self.workspace.as_mut() {
                    screen.set_ask_first(ask);
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
            Request::UpdateNotice(on) => self.store_update_notice(on),
            Request::InstallService => self.change_service(true),
            Request::RemoveService => self.change_service(false),
        }
    }

    /// Whether the screen that is open is one the person may leave, which is what both the way
    /// out on screen and the Esc key ask.
    pub(super) fn can_leave(&self) -> bool {
        self.router.can_go_back() && (self.page() != Page::Setup || self.setup.as_ref().is_some_and(Setup::is_alone))
    }

    /// Leaves the screen that is open, when it is one that may be left.
    ///
    /// The setup wizard is not: until the three questions are answered there is no application
    /// behind it to go back to. The one step the repair strip opens is another matter — it
    /// stands over a working application, and a person whose engine cannot be installed this
    /// minute must be able to put it down.
    pub(super) fn leave(&mut self) -> Command<Msg> {
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
    pub(super) fn reread_profiles(&self) -> Command<Msg> {
        let (Page::Workspace, Some(store)) = (self.page(), self.store()) else { return Command::none() };
        Command::perform(move || Msg::Workspace(ui::workspace::Msg::Profiles(store.profiles().value)))
    }

    /// The screen this one was opened from, which is where leaving it leads.
    pub(super) fn behind(&self) -> Option<Page> {
        let history = self.router.history();
        history.len().checked_sub(2).and_then(|index| history.get(index)).copied()
    }

    /// Whether the profiles screen is showing its list rather than its wizard.
    pub(super) fn draft_is_closed(&self) -> bool {
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
    pub(super) fn quit(&self) -> Command<Msg> {
        self.farewell.keep(self.workspace.as_ref().and_then(ui::workspace::leaving));
        Command::quit()
    }

    /// Leaves at once when no agent is at work, and otherwise asks once first.
    ///
    /// Quitting stops every container of the open workspaces, so an agent in the middle of a task
    /// is cut off there; its conversation is kept and goes on when the workspace is opened again.
    /// A stray key costs a question, never someone's running work. Asking to quit again while the
    /// question stands is the answer, so a person who meant it is never held by two questions.
    pub(super) fn ask_quit(&mut self) -> Command<Msg> {
        let working = self.workspace.as_ref().map_or(0, ui::workspace::agents_at_work);
        if working == 0 || self.asking_quit {
            return self.quit();
        }
        self.asking_quit = true;
        Command::confirm(
            qframe::runtime::Confirm::new(t!("app.quit-title"), Msg::Quit)
                .message(t!("app.quit-working", n = working))
                .confirm_label(t!("app.quit-anyway"))
                .cancel_label(t!("app.quit-stay"))
                .on_cancel(Msg::QuitKept)
                .danger(),
        )
    }

    /// Writes the settings file, off the render path, and tells the settings screen how it went.
    pub(super) fn save(&self) -> Command<Msg> {
        self.config.settings().save_command(|stored| Msg::Settings(ui::settings::Msg::Stored(stored)))
    }
}

/// Which profiles have a login stored, which is what the identity part of the settings screen
/// shows and acts on. A profile that signs in to nothing has no login to refresh or remove, so
/// it is not listed there.
///
/// Without an engine to ask, nothing is claimed either way and every profile is listed as signed
/// out, beside the strip that says why nothing here can be done.
pub(super) fn identities(
    engine: Option<&Engine>,
    profiles: &[Profile],
) -> Vec<ui::settings::identity::ProfileIdentity> {
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
pub(super) fn refreshed(profile: &SafeName, outcome: Result<Refreshed, RefreshError>) -> Toast<Msg> {
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
        engine::run::EngineError::TimedOut { command, after } => engine::run::timed_out(command, *after),
    }
}

/// The engine's health as the gates of the wizard left it.
pub(super) fn health(check: &EngineCheck) -> Health {
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
        // Both are refusals the settings screen reads again and says plainly.
        EngineProblem::Refused { output, .. }
        | EngineProblem::NoPermission { output, .. }
        | EngineProblem::NoIdRanges { output, .. } => Trouble::Refused(output.clone()),
        EngineProblem::NotRunnable { message, .. } => Trouble::NotRunnable(message.clone()),
    }
}
