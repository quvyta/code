//! QCode as a framework application: the messages it takes, the actions its keys name, and how
//! each screen is drawn with the rail and the foot around it.

use qframe::keymap::{KeyChord, Scope};
use qframe::prelude::*;
use qframe::widgets::{HelpLayer, IconButton, PageTransition, Toast};

use crate::requests::refreshed;
use crate::ui::home::Entry;
use crate::{Msg, Opening, Page, QCode, ui};

/// What the list of keys is reached by at the foot of the rail: the icon set's question mark.
const KEYS_ICON: &str = "help";

/// The ways out of the workspace screen, at the foot of its rail: an empty row, the list of keys,
/// the settings, another empty row and the way back on the last row.
///
/// They stand under the rail rather than in a header and a footer of their own because the
/// middle of that screen is a terminal, and a terminal is worth more rows than three controls
/// that are each one cell wide. The way back is nearest the bottom edge, which is where the
/// hand that reaches for it already is. The empty rows are the owner's: one keeps the foot apart
/// from the last workspace, one keeps the way back apart from the settings, so a hand going for
/// one does not land on the other.
fn under_rail(ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.spacer().height(Length::Cells(1));
        ui.add(IconButton::new(KEYS_ICON).tooltip(t!("app.keys")).on_press(Msg::Help(true))).id("keys");
        ui.add(IconButton::new("settings").tooltip(t!("home.settings")).on_press(Msg::Open(Entry::Settings)))
            .id("rail-settings");
        ui.spacer().height(Length::Cells(1));
        ui.add(IconButton::new("arrow-left").tooltip(t!("app.back")).on_press(Msg::Back)).id("back");
    });
}

/// The foot every screen but the workspace screen carries: the way back at the bottom left, when
/// the screen can be left, and at the bottom right the key that opens the list of every key. Both
/// are drawn the same way — the key, then what it does — and both are buttons for the pointer too.
///
/// The way back stands at the foot rather than over the screen because the top row is where a
/// pointer on its way to the screen's own first controls passes, and a Back there was pressed by
/// accident. The workspace screen keeps the same order at the foot of its rail. The hint bars the
/// screens once had are the list of keys now, so the screens keep their room and nothing they
/// listed is lost.
fn foot(can_leave: bool, ui: &mut View<'_, Msg>) {
    let keymap = ui.env().keymap();
    let back = keymap.chords_for(Scope::App, "back").first().map(KeyChord::label);
    let help = keymap.chords_for(Scope::Global, "help").first().map(KeyChord::label);
    ui.row(|ui| {
        if can_leave {
            let mut button = Button::new(t!("app.back")).on_press(Msg::Back);
            if let Some(key) = back {
                button = button.shortcut(key);
            }
            ui.add(button).id("back");
        }
        ui.spacer();
        let mut button = Button::new(t!("app.keys")).on_press(Msg::Help(true));
        if let Some(key) = help {
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
        Page::Switch => ui::switch::hints(icons),
    }
}

impl App for QCode {
    type Msg = Msg;

    /// Puts the keyboard on the first screen before the first key is read: the wizard's open
    /// step, or the menu of the home screen. Without this the first arrow key went nowhere.
    fn init(&mut self) -> Command<Msg> {
        match self.setup.as_mut() {
            Some(setup) => ui::setup::opened(setup).map(Msg::Setup),
            // Not while the wizard is open: a first start has enough to say, and the next start
            // asks.
            None => Command::batch([self.take_focus(), self.survey(), self.ask_for_update()]),
        }
    }

    /// Every way out passes through the same quit as the menu's, so that the workspaces open then
    /// are backed up however QCode was left.
    fn before_quit(&self) -> Option<Msg> {
        Some(Msg::AskQuit)
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
                // What is open is read at every message rather than when the screen opened: the
                // workspace screen stays alive behind this one and opens workspaces of its own.
                let open = self.open_workspaces();
                let Some(screen) = self.workspaces.as_mut() else { return Command::none() };
                screen.set_open(open);
                let (command, opened) = ui::workspaces::update(screen, message);
                let command = command.map(Msg::Workspaces);
                // A deleted workspace is not offered again as the one to continue with.
                if let Some(gone) = screen.just_deleted() {
                    self.config.forget_workspace(&gone);
                    self.refresh_home();
                    return Command::batch([command, self.save()]);
                }
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
            Msg::Switch(message) => {
                let Some(page) = self.switch.as_mut() else { return Command::none() };
                let (command, request) = ui::switch::update(page, message);
                let command = command.map(Msg::Switch);
                match request {
                    Some(request) => Command::batch([command, self.switched(request)]),
                    None => command,
                }
            }
            Msg::Back => self.leave(),
            Msg::Quit => self.quit(),
            Msg::AskQuit => self.ask_quit(),
            Msg::QuitKept => {
                self.asking_quit = false;
                Command::none()
            }
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
            Msg::NewVersion(update) => Command::toast(update.toast()),
        }
    }

    /// Turns a keymap action into a message. `back` is Esc, which reaches here only when
    /// nothing nearer has used it: a focused widget first, so the terminal of a harness tab
    /// keeps its own Esc, and then an open dialog, which closes instead of letting the screen go.
    /// On the workspace screen an entry of the file tree that waits to be pasted is let go first.
    /// `help` is the framework's own action for the list of keys; the application opens it.
    /// `new-tab` opens a blank tab, `leave-terminal` takes the keyboard back into the open tab's
    /// harness, `rename-tab` names the open tab, and the switching keys go to another tab with the
    /// keyboard in it; all of them only mean something while a workspace is open.
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
            // From inside a harness too: the terminal lets these keys through.
            _ => ui::workspace::action(name).filter(|_| workspace_open).map(Msg::Workspace),
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
        let can_leave = self.can_leave();
        AppShell::new()
            .header(|ui| self.header(page, ui))
            .body(|ui| self.body(page, ui))
            .footer(move |ui| foot(can_leave, ui))
            .show(ui);
    }

    /// What stands over every screen: the strip while the engine is gone.
    ///
    /// Whatever Esc does, the way out is also on screen, at the foot (see [`foot`]): a screen that
    /// can be left says so. The setup itself cannot be — it is left by finishing it — while the
    /// single step the repair strip opens can, because it stands over an application that already
    /// works.
    fn header(&self, page: Page, ui: &mut View<'_, Msg>) {
        // The wizard is where a missing engine is put right, so the strip that leads there does
        // not stand over it.
        if page != Page::Setup {
            ui.map(Msg::Settings, |ui| ui::settings::bar::view(&self.engine, ui)).fill_width();
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
            Page::Switch => {
                if let Some(page) = &self.switch {
                    ui.map(Msg::Switch, |ui| ui::switch::view(page, ui)).fill();
                }
            }
            // Drawn whole by `draw` once its screen exists; before that there is nothing to show.
            Page::Workspace => {}
        }
    }
}
