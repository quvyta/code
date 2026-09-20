//! The tab of a desktop harness: a window open on the person's own screen, and the little QCode
//! can say and do about it from a terminal.
//!
//! There is no terminal on this tab, because the harness does not draw in one. What there is
//! instead is one line saying where the window stands and, while it is open, the two things that
//! can be done to it: ask it to come forward, and close it. Everything else the person does, they
//! do in the window.
//!
//! How the tab knows where the window stands, without asking a compositor anything: the window has
//! a container to itself whose only program is the application, so the container ends exactly when
//! the window closes, and the engine will say so if it is asked to wait. That wait is the tab's one
//! background thread, and it is what turns "the person closed the window" into a message.
//!
//! What the two actions really are is in [`plan::raise_window`] and [`plan::close_window`]; that
//! the raise may only mark the window as asking for attention, rather than bringing it forward, is
//! Wayland's rule and is said on the tab rather than hidden.

use qframe::prelude::*;

use crate::desktop::{self, Display, NoDisplay};
use crate::engine::Network;

use super::plan::{self, ContainerPlan, LaunchFailure};
use super::{Msg, OpenProject, ProjectScreen, Tab, TabKey, TabKind, TabState};

/// The name the tab's one action is focused by, whichever action the tab's state offers.
pub(super) const WINDOW_ID: &str = "project-window";

/// What came of asking to open a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// The window is up: a container was started for it, or one that was already running was taken
    /// over rather than a second window opened.
    Up,
    /// Nothing was opened, because the tab is waiting to be asked.
    Waiting,
}

/// The failure a tab shows when this machine has no screen to open a window on.
///
/// There is no engine command behind it, so only the reason is shown; it is put into a failure
/// rather than a state of its own because that is what every other way of not opening is.
fn no_display(reason: &NoDisplay) -> LaunchFailure {
    let output = match reason {
        NoDisplay::NoWayland => t!("project.window.no-wayland"),
        NoDisplay::NoSocket(path) => t!("project.window.no-socket", socket = path.display().to_string()),
    };
    LaunchFailure { command: String::new(), output }
}

/// The plan of the window the tab `key` opens, when that is what the tab opens.
fn plan_of(screen: &ProjectScreen, key: TabKey) -> Option<(ContainerPlan, bool)> {
    let project = screen.projects.iter().find(|project| project.tabs.iter().any(|tab| tab.key() == key))?;
    let tab = project.tabs.iter().find(|tab| tab.key() == key)?;
    let TabKind::Desktop(_) = tab.kind() else { return None };
    let plan = project.plan(tab.kind())?;
    let offline = plan.network == Network::None;
    Some((plan, offline))
}

/// Opens the window of the tab `key` in its run `run`, on a background thread, and then waits for
/// it: the message that comes back says it is open, and a second one comes when it closes.
///
/// A tab brought back from the last session waits to be asked instead. Opening QCode again is not
/// asking for a window on the screen, and a window is not something that can be missed on a tab
/// that is not in view; so the tab comes back saying the window is closed, with the way to open it.
pub(super) fn open(screen: &ProjectScreen, key: TabKey, run: u64) -> Command<Msg> {
    let Some((plan, _)) = plan_of(screen, key) else { return Command::none() };
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let held = screen
        .projects
        .iter()
        .find_map(|project| project.tabs.iter().find(|tab| tab.key() == key))
        .is_some_and(Tab::is_held);
    if held {
        return Command::perform(move || Msg::WindowOpened(key, run, Ok(Opening::Waiting)));
    }
    let (user, registry) = (screen.user, screen.registry.clone());
    let display = match &screen.display {
        Ok(display) => display.clone(),
        Err(reason) => {
            let failure = no_display(reason);
            return Command::perform(move || Msg::WindowOpened(key, run, Err(failure)));
        }
    };
    Command::perform(move || {
        let opened = plan::open_window(&engine, &plan, user, &display).map(|_| Opening::Up);
        // Noted like every container QCode starts, so a QCode that is closed while a window is
        // open stops it rather than leaving it on the screen with nothing behind it.
        let up = opened.is_ok();
        let message = Msg::WindowOpened(key, run, opened);
        if up { super::noted(registry.as_deref(), engine.kind(), &plan.name, message) } else { message }
    })
}

/// Waits for the window of the tab `key` in its run `run` to close, on a background thread of its
/// own. The wait lasts as long as the window is open, which is the point of it.
pub(super) fn watch(screen: &ProjectScreen, key: TabKey, run: u64) -> Command<Msg> {
    let Some((plan, _)) = plan_of(screen, key) else { return Command::none() };
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    Command::perform(move || Msg::WindowEnded(key, run, plan::await_window(&engine, &plan.name)))
}

/// Takes the answer that the window of the tab `key` is open, or why it is not.
pub(super) fn opened(
    screen: &mut ProjectScreen,
    key: TabKey,
    run: u64,
    answer: Result<Opening, LaunchFailure>,
) -> Command<Msg> {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|project| project.find(key)) else {
        return Command::none();
    };
    if tab.run() != run || !tab.state().is_starting() {
        return Command::none();
    }
    match answer {
        // A tab that waits to be asked shows the window as closed, with the way to open it.
        Ok(Opening::Waiting) => {
            tab.settled(TabState::Ended { code: None });
            Command::none()
        }
        Ok(Opening::Up) => {
            tab.settled(TabState::Running);
            watch(screen, key, run)
        }
        Err(failure) => {
            tab.settled(TabState::Failed(failure));
            Command::none()
        }
    }
}

/// Takes the news that the window of the tab `key` closed, whoever closed it, and leaves nothing
/// of its container behind.
pub(super) fn ended(screen: &mut ProjectScreen, key: TabKey, run: u64, code: Option<u32>) -> Command<Msg> {
    let Some((plan, _)) = plan_of(screen, key) else { return Command::none() };
    let engine = screen.engine.clone();
    let Some((_, tab)) = screen.owner_mut(key).and_then(|project| project.find(key)) else {
        return Command::none();
    };
    if tab.run() != run {
        return Command::none();
    }
    tab.settled(TabState::Ended { code });
    // The container that ended has nothing left in it — everything the person did is in the home
    // volume — and a stopped container of this name in the way is what the next open would have to
    // clear. So it goes now, while QCode is the one watching.
    match engine {
        Some(engine) => Command::perform(move || Msg::WindowsClosed(plan::close_window(&engine, &plan.name).err())),
        None => Command::none(),
    }
}

/// Asks the open window of the tab `key` to show itself, on a background thread.
pub(super) fn raise(screen: &ProjectScreen, key: TabKey) -> Command<Msg> {
    let Some((plan, _)) = plan_of(screen, key) else { return Command::none() };
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let open = screen
        .projects
        .iter()
        .find_map(|project| project.tabs.iter().find(|tab| tab.key() == key))
        .is_some_and(|tab| tab.state() == &TabState::Running);
    if !open {
        return Command::none();
    }
    Command::perform(move || Msg::WindowRaised(plan::raise_window(&engine, &plan)))
}

/// Closes the window of the tab `key`, on a background thread. The wait that is watching the
/// container answers by itself once it is gone, which is what moves the tab.
pub(super) fn close(screen: &ProjectScreen, key: TabKey) -> Command<Msg> {
    let Some((plan, _)) = plan_of(screen, key) else { return Command::none() };
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    Command::perform(move || Msg::WindowsClosed(plan::close_window(&engine, &plan.name).err()))
}

/// Closes the windows the desktop tabs of `tabs` still have open, on a background thread.
///
/// Closing a tab or a project must leave nothing on the screen: a window whose tab is gone could
/// not be closed from QCode any more, and its container would keep running.
pub(super) fn close_all(screen: &ProjectScreen, project: &OpenProject, tabs: &[&Tab]) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let names: Vec<String> = tabs
        .iter()
        .filter(|tab| matches!(tab.state(), TabState::Running | TabState::Starting))
        .filter_map(|tab| tab.kind().profile().filter(|_| matches!(tab.kind(), TabKind::Desktop(_))))
        .map(|profile| crate::engine::names::desktop_container(project.id(), profile))
        .collect();
    if names.is_empty() {
        return Command::none();
    }
    Command::perform(move || {
        let failure = names.iter().filter_map(|name| plan::close_window(&engine, name).err()).next();
        Msg::WindowsClosed(failure)
    })
}

/// The desktop tab, in whatever state it is: one line of status, and the actions that state has.
///
/// The shape is colour and space, never a box: a heading-sized line for where the window stands, a
/// quieter line under it for what that means, and the actions below that.
pub(super) fn view(screen: &ProjectScreen, tab: &Tab, profile: &str, ui: &mut View<'_, Msg>) {
    let key = tab.key();
    let offline = plan_of(screen, key).is_some_and(|(_, offline)| offline);
    let (colour, headline, detail) = words(tab, profile, offline);
    ui.column(|ui| {
        ui.column(|ui| {
            ui.add(Text::new(headline).role("title").color(colour));
            if let Some(detail) = detail {
                ui.add(Text::new(detail).role("secondary")).selectable(true).fill_width();
            }
            ui.row(|ui| match tab.state() {
                TabState::Running => {
                    ui.add(Button::new(t!("project.window.raise")).variant("primary").on_press(Msg::RaiseWindow(key)))
                        .id(WINDOW_ID);
                    ui.add(Button::new(t!("project.window.close")).on_press(Msg::CloseWindow(key)))
                        .id("project-window-close");
                }
                TabState::Waiting | TabState::Starting => {}
                _ => {
                    let again = t!("project.window.open");
                    ui.add(Button::new(again).variant("primary").on_press(Msg::Restart(key))).id(WINDOW_ID);
                }
            })
            .gap(2)
            .fill_width();
        })
        .gap(1)
        .width(Length::Cells(READABLE_WIDTH))
        .fill_width();
    })
    .padding(Padding::symmetric(1, 2))
    .fill()
    .id("project-desktop");
}

/// The widest the tab's words grow, so a line of explanation stays readable on a wide terminal.
const READABLE_WIDTH: u16 = 72;

/// What the tab says: the colour of its first line, that line, and the quieter one under it.
fn words(tab: &Tab, profile: &str, offline: bool) -> (&'static str, String, Option<String>) {
    match tab.state() {
        TabState::Waiting | TabState::Starting => {
            ("accent", t!("project.window.opening"), Some(t!("project.window.opening-detail")))
        }
        TabState::Running => {
            let mut detail = t!("project.window.open-detail", profile = profile);
            // The application has nothing to work with until the person signs in, and that sign-in
            // is not something QCode can do for them yet; saying so on the tab is the least it owes
            // them, because the window itself is the only place it can be done.
            detail.push(' ');
            detail.push_str(&t!("project.window.sign-in"));
            if offline {
                detail.push(' ');
                detail.push_str(&t!("project.window.offline"));
            }
            ("success", t!("project.window.open-now"), Some(detail))
        }
        TabState::Failed(failure) => {
            let detail = if failure.command.is_empty() {
                failure.output.clone()
            } else {
                format!("{}\n{}", failure.command, failure.output)
            };
            ("danger", t!("project.window.failed"), Some(detail))
        }
        TabState::Ended { code: Some(code) } if *code != 0 => {
            ("warning", t!("project.window.closed"), Some(t!("project.window.closed-code", code = code.to_string())))
        }
        _ => ("secondary", t!("project.window.closed"), Some(t!("project.window.closed-detail"))),
    }
}

/// What this machine offers a window, read once when the screen is made: a session does not change
/// its compositor underneath QCode, and when there is none the reason is what the tab shows.
pub(super) fn display() -> Result<Display, NoDisplay> {
    desktop::current()
}
