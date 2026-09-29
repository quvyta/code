//! Moving between the screens: a row of the home menu, the lists, the workspace screen that
//! reads what it opens first, and the session file that remembers what was open.

use qframe::prelude::*;
use qframe::widgets::Toast;

use crate::requests::identities;
use crate::store::{Session, SetupStep, WorkspaceFile, WorkspaceId};
use crate::ui::home::Entry;
use crate::ui::profiles::Profiles;
use crate::ui::providers::Providers as ProvidersScreen;
use crate::ui::workspace::{OpenWorkspace, WorkspaceScreen};
use crate::ui::workspaces::Workspaces;
use crate::{Msg, Opening, Page, QCode, WorkspaceContents, provider, ui};

impl QCode {
    /// Opens a row of the home menu.
    pub(super) fn open(&mut self, entry: Entry) -> Command<Msg> {
        match entry {
            Entry::Quit => self.ask_quit(),
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
    pub(super) fn show_workspaces(&mut self, new: bool) -> Command<Msg> {
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

    /// The workspaces open on the workspace screen right now.
    pub(super) fn open_workspaces(&self) -> Vec<WorkspaceId> {
        self.workspace.as_ref().map_or_else(Vec::new, |screen| {
            screen.workspaces().iter().filter_map(|workspace| WorkspaceId::parse(workspace.id()).ok()).collect()
        })
    }

    /// Opens the profiles screen.
    pub(super) fn show_profiles(&mut self) -> Command<Msg> {
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
    pub(super) fn show_providers(&mut self) -> Command<Msg> {
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
    pub(super) fn read_workspaces(&mut self, ids: Vec<WorkspaceId>, opening: Opening) -> Command<Msg> {
        let Some(store) = self.store() else { return self.repair(SetupStep::Location) };
        Command::perform(move || {
            let known = store.profiles().value;
            let mut files: Vec<WorkspaceFile> =
                store.workspaces().value.into_iter().filter_map(|entry| entry.file).collect();
            let contents = ids
                .iter()
                .filter_map(|id| {
                    let file = files.remove(files.iter().position(|file| file.id == *id)?);
                    let paths = store.paths_of(&file);
                    let folder_gone = file.folder.is_some() && !paths.code.is_dir();
                    Some(WorkspaceContents { file, paths, profiles: known.clone(), folder_gone })
                })
                .collect();
            Msg::Read(contents, opening)
        })
    }

    /// Opens what was read for `opening`.
    ///
    /// A workspace whose own folder of the person's is gone is said, by name and path, and left
    /// closed; nothing makes the folder again. It stays on the list and in the recent ones, since a
    /// disk plugged in again brings it back.
    pub(super) fn read(&mut self, contents: Vec<WorkspaceContents>, opening: Opening) -> Command<Msg> {
        let (gone, contents): (Vec<_>, Vec<_>) = contents.into_iter().partition(|one| one.folder_gone);
        let mut told: Vec<Command<Msg>> = gone
            .iter()
            .map(|one| {
                let toast = Toast::warning(t!("app.folder-gone", name = one.file.name.as_str()))
                    .body(t!("app.folder-gone-body", path = one.paths.code.display().to_string()));
                Command::toast(toast)
            })
            .collect();
        match opening {
            Opening::One(id) if gone.iter().any(|one| one.file.id == id) => told.push(self.show_workspaces(false)),
            Opening::One(id) => told.push(self.show_workspace(contents, &id)),
            Opening::Session(mut session) => {
                // Said once, above, in its own words: not among the ones the store no longer has.
                session.workspaces.retain(|record| !gone.iter().any(|one| one.file.id == record.id));
                told.push(self.restore(contents, &session));
            }
        }
        Command::batch(told)
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
                let mut screen = self.workspace_screen(vec![workspace]);
                let entered = ui::workspace::opened(&mut screen);
                self.workspace = Some(screen);
                entered
            }
        };
        self.enter_workspace();
        self.config.remember_workspace(id);
        Command::batch([entered.map(Msg::Workspace), self.take_focus(), self.save(), self.keep_session()])
    }

    /// A workspace screen of `workspaces`, set the way the settings file says. Opening one
    /// workspace and bringing a session back both make theirs here, so neither can miss a setting.
    fn workspace_screen(&self, workspaces: Vec<OpenWorkspace>) -> WorkspaceScreen {
        let mut screen = WorkspaceScreen::new(self.found.clone(), self.user, workspaces)
            .with_registry(self.registry.clone())
            .backing_up(self.config.backup_every())
            .watching(self.live_files)
            .bridging(self.live_files);
        screen.set_editor(self.config.editor());
        screen.set_sound(self.config.sound());
        screen.set_ask_first(self.config.ask_first());
        screen
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
        let mut screen = self.workspace_screen(workspaces);
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
    pub(super) fn enter_workspace(&mut self) {
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
    pub(super) fn keep_session(&mut self) -> Command<Msg> {
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
    pub(super) fn session_saved(&mut self, result: Result<(), String>) -> Command<Msg> {
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
}
