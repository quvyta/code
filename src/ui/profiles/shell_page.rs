//! The page of a profile's shell, and the profile's own steps in the list.
//!
//! The shell is opened from the wizard's last page and from the list, and covers whichever it was
//! opened from until it is closed; the wizard is still there under it. Everything that touches the
//! engine runs in a task and comes back as a [`ShellMsg`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::widgets::{ShimmerText, Terminal, TerminalEvent, TerminalSession, Toast};

use crate::engine::HostUser;
use crate::profile::Profile;
use crate::profile::own::Own;
use crate::store::Store;

use super::shell::{self, Session};
use super::work::Problem;
use super::{Msg, Profiles, VIEWPORT_ROWS, failure_toast, reload};

/// Which of the shell's two terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// `bash` as the person.
    You,
    /// `bash` as root.
    Admin,
}

/// Everything that happens to a profile's shell and to the profile's own steps.
#[derive(Debug, Clone)]
pub enum ShellMsg {
    /// The button that opens the shell was pressed, on the wizard's last page or in the list.
    Asked,
    /// The container is up and the person's terminal is running in it, or it could not be.
    Opened(Result<(Arc<str>, TerminalSession), Problem>),
    /// A shell that was never closed left its container with work in it; the person is asked.
    Leftover(Arc<str>, Session),
    /// Carry on in the shell that was left open.
    Continue,
    /// Add what the shell that was left open holds to the profile.
    AddLeftover,
    /// A terminal drew or ended.
    Event(Side, TerminalEvent),
    /// "As administrator" was pressed.
    Admin,
    /// The administrator's terminal is running, or could not be started.
    AdminOpened(Result<TerminalSession, Problem>),
    /// The person chose which terminal to look at.
    Show(Side),
    /// The shell is closed: what changed is read.
    Close,
    /// What the shell changed and what its administrator ran.
    Surveyed(Result<Session, Problem>),
    /// Add what the shell holds to the profile.
    Add,
    /// Throw away what the shell holds.
    Discard,
    /// Adding is over.
    Added(Result<Own, Problem>),
    /// The shell's container is gone.
    Discarded,
    /// The own step at this place of the chosen profile is taken off its list.
    Trim(usize),
    /// A step could not be taken off, because the store could not be written.
    TrimFailed(Problem),
    /// Every profile's own steps and home files, as the store holds them.
    OwnRead(HashMap<String, Own>),
}

/// Where the shell is.
#[derive(Debug, Clone)]
pub enum Stage {
    /// The container is being made.
    Opening,
    /// A shell that was never closed left this behind, and the person says what becomes of it.
    Leftover(Session),
    /// The person is working in it.
    Open,
    /// It was closed and what changed is being read.
    Surveying,
    /// What changed is known and the question is asked.
    Deciding(Session),
    /// What it holds is being made the profile's.
    Adding,
    /// It could not be opened or its work could not be added.
    Failed(Problem),
}

/// A profile's shell, while it is open.
#[derive(Debug)]
pub struct ShellPage {
    profile: Profile,
    container: Option<Arc<str>>,
    you: Option<TerminalSession>,
    admin: Option<TerminalSession>,
    showing: Side,
    stage: Stage,
}

impl ShellPage {
    /// Where the shell is.
    #[must_use]
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The name of the container the shell runs in, once it is up.
    #[must_use]
    pub fn container(&self) -> Option<&str> {
        self.container.as_deref()
    }

    fn kill(&self) {
        for session in [&self.you, &self.admin].into_iter().flatten() {
            session.kill();
        }
    }
}

/// Applies a message about the shell.
pub(super) fn update(state: &mut Profiles, message: ShellMsg) -> Command<Msg> {
    match message {
        ShellMsg::Asked => open(state),
        ShellMsg::Opened(Ok((container, session))) => {
            let Some(page) = &mut state.shell else {
                // The screen was left while the container came up; nothing keeps it.
                session.kill();
                return discard_container(state.engine.clone(), &container);
            };
            let watch = session.watch();
            page.container = Some(container);
            page.you = Some(session);
            page.stage = Stage::Open;
            Command::batch([
                Command::perform(move || Msg::Shell(ShellMsg::Event(Side::You, watch.next()))),
                Command::focus("shell-terminal"),
            ])
        }
        ShellMsg::Opened(Err(problem)) => {
            if let Some(page) = &mut state.shell {
                // Nothing of this opening stands, and a container left open before it is not
                // this page's to throw away when it is closed.
                page.container = None;
                page.stage = Stage::Failed(problem);
            }
            Command::none()
        }
        ShellMsg::Leftover(container, session) => {
            let Some(page) = &mut state.shell else { return Command::none() };
            page.container = Some(container);
            page.stage = Stage::Leftover(session);
            Command::none()
        }
        ShellMsg::Continue => carry_on(state),
        ShellMsg::AddLeftover => {
            let Some(page) = &mut state.shell else { return Command::none() };
            let Stage::Leftover(session) = &page.stage else { return Command::none() };
            page.stage = Stage::Deciding(session.clone());
            add(state)
        }
        ShellMsg::AdminOpened(Err(problem)) => {
            if let Some(page) = &mut state.shell {
                page.kill();
                page.stage = Stage::Failed(problem);
            }
            Command::none()
        }
        ShellMsg::Event(side, event) => watch(state, side, event),
        ShellMsg::Admin => open_admin(state),
        ShellMsg::AdminOpened(Ok(session)) => {
            let Some(page) = &mut state.shell else {
                session.kill();
                return Command::none();
            };
            let watch = session.watch();
            page.admin = Some(session);
            page.showing = Side::Admin;
            Command::batch([
                Command::perform(move || Msg::Shell(ShellMsg::Event(Side::Admin, watch.next()))),
                Command::focus("shell-terminal"),
            ])
        }
        ShellMsg::Show(side) => {
            if let Some(page) = &mut state.shell {
                page.showing = side;
            }
            Command::focus("shell-terminal")
        }
        ShellMsg::Close => close(state),
        ShellMsg::Surveyed(result) => surveyed(state, result),
        ShellMsg::Add => add(state),
        ShellMsg::Discard => {
            let Some(page) = &state.shell else { return Command::none() };
            let (Some(engine), name) = (state.engine.clone(), page.profile.name.clone()) else {
                state.shell = None;
                return Command::none();
            };
            Command::perform(move || {
                shell::discard(&engine, &name);
                Msg::Shell(ShellMsg::Discarded)
            })
        }
        ShellMsg::Discarded => {
            state.shell = None;
            Command::toast(Toast::info(t!("profiles.shell.discarded")))
        }
        ShellMsg::Added(Ok(own)) => {
            let name = state.shell.take().map(|page| page.profile.name.to_string()).unwrap_or_default();
            let steps = i64::try_from(own.steps.len()).unwrap_or(i64::MAX);
            let told = Toast::success(t!("profiles.shell.added", name = name.as_str()))
                .body(t!("profiles.shell.added-body", n = steps));
            Command::batch([Command::toast(told), reload(state)])
        }
        ShellMsg::Added(Err(problem)) => {
            if let Some(page) = &mut state.shell {
                page.stage = Stage::Failed(problem);
            }
            Command::none()
        }
        ShellMsg::Trim(index) => trim(state, index),
        ShellMsg::TrimFailed(problem) => Command::toast(failure_toast("profiles.own.trim-failed", &problem)),
        ShellMsg::OwnRead(own) => {
            state.own.extend(own);
            Command::none()
        }
    }
}

/// The profile the shell is asked for: the wizard's, when the wizard is open, or the list's
/// chosen one.
fn asked_for(state: &Profiles) -> Option<Profile> {
    match &state.draft {
        Some(draft) => draft.profile(),
        None => state.selected().map(|row| row.profile.clone()),
    }
}

/// Opens the shell's container and the person's terminal in it.
fn open(state: &mut Profiles) -> Command<Msg> {
    let Some(engine) = state.engine.clone() else { return Command::none() };
    if state.shell.is_some() {
        return Command::none();
    }
    let Some(profile) = asked_for(state) else { return Command::none() };
    state.shell = Some(ShellPage {
        profile: profile.clone(),
        container: None,
        you: None,
        admin: None,
        showing: Side::You,
        stage: Stage::Opening,
    });
    Command::task(Task::new(t!("profiles.shell.opening"), move |_| {
        let opened = HostUser::current()
            .map_err(|error| Problem::Machine(error.to_string()))
            .and_then(|user| shell::open(&engine, &profile, user));
        let container = match opened {
            Ok(shell::Opened::Fresh(container)) => container,
            Ok(shell::Opened::Leftover(container, session)) => {
                return Ok(Msg::Shell(ShellMsg::Leftover(Arc::from(container.as_str()), session)));
            }
            Err(problem) => return Ok(Msg::Shell(ShellMsg::Opened(Err(problem)))),
        };
        let entered = spawn(&shell::enter(&engine, &container, false));
        if entered.is_err() {
            // Made just now and nothing done in it yet.
            shell::discard(&engine, &profile.name);
        }
        Ok(Msg::Shell(ShellMsg::Opened(entered.map(|session| (Arc::from(container.as_str()), session)))))
    }))
}

/// Opens the person's terminal in the shell that was left open, which goes on as if it had never
/// been left: closing it asks about everything done in it, before and now.
fn carry_on(state: &mut Profiles) -> Command<Msg> {
    let Some(engine) = state.engine.clone() else { return Command::none() };
    let Some(page) = &mut state.shell else { return Command::none() };
    let (Some(container), Stage::Leftover(_)) = (page.container.clone(), &page.stage) else { return Command::none() };
    page.stage = Stage::Opening;
    Command::perform(move || {
        let entered = spawn(&shell::enter(&engine, &container, false));
        Msg::Shell(ShellMsg::Opened(entered.map(|session| (container, session))))
    })
}

/// Starts the administrator's terminal in the shell's container.
fn open_admin(state: &mut Profiles) -> Command<Msg> {
    let Some(engine) = state.engine.clone() else { return Command::none() };
    let Some(page) = &mut state.shell else { return Command::none() };
    if page.admin.is_some() {
        page.showing = Side::Admin;
        return Command::focus("shell-terminal");
    }
    let (Some(container), Stage::Open) = (page.container.clone(), &page.stage) else { return Command::none() };
    Command::perform(move || Msg::Shell(ShellMsg::AdminOpened(spawn(&shell::enter(&engine, &container, true)))))
}

/// A terminal session running `command`.
fn spawn(command: &crate::engine::EngineCommand) -> Result<TerminalSession, Problem> {
    TerminalSession::spawn(command.program.as_os_str(), &command.args, &std::env::temp_dir())
        .map_err(|error| Problem::Machine(error.to_string()))
}

/// Keeps watching a terminal. The person's shell ending is the shell being closed; the
/// administrator's ending only puts the person's back in front.
fn watch(state: &mut Profiles, side: Side, event: TerminalEvent) -> Command<Msg> {
    let Some(page) = &mut state.shell else { return Command::none() };
    if !matches!(page.stage, Stage::Open) {
        return Command::none();
    }
    let session = match side {
        Side::You => page.you.as_ref(),
        Side::Admin => page.admin.as_ref(),
    };
    let Some(session) = session else { return Command::none() };
    match event {
        TerminalEvent::Output => {
            let watch = session.watch();
            Command::perform(move || Msg::Shell(ShellMsg::Event(side, watch.next())))
        }
        TerminalEvent::Exited(_) if side == Side::Admin => {
            page.admin = None;
            page.showing = Side::You;
            Command::focus("shell-terminal")
        }
        TerminalEvent::Exited(_) => close(state),
    }
}

/// Ends both terminals and reads what the shell changed.
fn close(state: &mut Profiles) -> Command<Msg> {
    let Some(engine) = state.engine.clone() else { return Command::none() };
    let Some(page) = &mut state.shell else { return Command::none() };
    if !matches!(page.stage, Stage::Open) {
        // Nothing was made to look at, so there is nothing to ask: the page goes. A container
        // this page never had — one a shell left open, which it failed to open again — stays.
        if matches!(page.stage, Stage::Failed(_)) {
            let name = page.profile.name.clone();
            let ours = page.container.is_some();
            state.shell = None;
            if !ours {
                return Command::none();
            }
            return Command::perform(move || {
                shell::discard(&engine, &name);
                Msg::Shell(ShellMsg::Discarded)
            });
        }
        return Command::none();
    }
    page.kill();
    page.you = None;
    page.admin = None;
    page.stage = Stage::Surveying;
    let name = page.profile.name.clone();
    Command::task(Task::new(t!("profiles.shell.surveying"), move |_| {
        Ok(Msg::Shell(ShellMsg::Surveyed(shell::survey(&engine, &name))))
    }))
}

/// Says what the shell changed and asks whether it becomes the profile's; a shell that changed
/// nothing is simply put away.
fn surveyed(state: &mut Profiles, result: Result<Session, Problem>) -> Command<Msg> {
    let Some(page) = &mut state.shell else { return Command::none() };
    let session = match result {
        Ok(session) => session,
        Err(problem) => {
            page.stage = Stage::Failed(problem);
            return Command::none();
        }
    };
    if session.changes.is_empty() && session.steps.is_empty() {
        page.stage = Stage::Adding;
        let engine = state.engine.clone();
        let name = page.profile.name.clone();
        return Command::perform(move || {
            if let Some(engine) = engine {
                shell::discard(&engine, &name);
            }
            Msg::Shell(ShellMsg::Discarded)
        });
    }
    let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    let mut message = vec![t!(
        "profiles.shell.changed",
        home = count(session.changes.home.len()).to_string(),
        system = count(session.changes.system.len()).to_string()
    )];
    if !session.steps.is_empty() {
        message.push(t!("profiles.shell.steps", n = count(session.steps.len())));
        message.push(session.steps.join("\n"));
    }
    message.push(t!("profiles.shell.reach"));
    let name = page.profile.name.to_string();
    page.stage = Stage::Deciding(session);
    Command::confirm(
        Confirm::new(t!("profiles.shell.changed-title", name = name.as_str()), Msg::Shell(ShellMsg::Add))
            .message(message.join("\n\n"))
            .confirm_label(t!("profiles.shell.add"))
            .cancel_label(t!("profiles.shell.discard"))
            .on_cancel(Msg::Shell(ShellMsg::Discard)),
    )
}

/// Makes what the shell holds the profile's.
fn add(state: &mut Profiles) -> Command<Msg> {
    let (Some(engine), Some(store)) = (state.engine.clone(), state.store()) else { return Command::none() };
    let Some(page) = &mut state.shell else { return Command::none() };
    let Stage::Deciding(session) = &page.stage else { return Command::none() };
    let steps = session.steps.clone();
    let profile = page.profile.clone();
    page.stage = Stage::Adding;
    let profiles = store.profiles_dir();
    Command::task(Task::new(t!("profiles.shell.adding"), move |_| {
        Ok(Msg::Shell(ShellMsg::Added(shell::add(&engine, &profile, &profiles, steps))))
    }))
}

/// Takes one own step off the chosen profile's list.
fn trim(state: &Profiles, index: usize) -> Command<Msg> {
    let (Some(store), Some(row)) = (state.store(), state.selected()) else { return Command::none() };
    let name = row.profile.name.clone();
    let profiles = store.profiles_dir();
    Command::perform(move || {
        let (mut own, _) = Own::load(&profiles, &name);
        if index < own.steps.len() {
            own.steps.remove(index);
            if let Err(problem) = own.save(&profiles, &name) {
                return Msg::Shell(ShellMsg::TrimFailed(Problem::Machine(problem.message)));
            }
        }
        Msg::Shell(ShellMsg::OwnRead(read_all(&profiles, [name.to_string()])))
    })
}

/// Reads the own steps of `names` from the store.
pub(super) fn read_all(profiles: &std::path::Path, names: impl IntoIterator<Item = String>) -> HashMap<String, Own> {
    names
        .into_iter()
        .filter_map(|name| {
            let safe = crate::profile::SafeName::parse(&name)?;
            let (own, _) = Own::load(profiles, &safe);
            Some((name, own))
        })
        .collect()
}

/// The own steps of every profile of the store, read in the background.
pub(super) fn load(store: Option<Store>, profiles: &[Profile]) -> Command<Msg> {
    let Some(store) = store else { return Command::none() };
    let names: Vec<String> = profiles.iter().map(|profile| profile.name.to_string()).collect();
    let folder: PathBuf = store.profiles_dir();
    Command::perform(move || Msg::Shell(ShellMsg::OwnRead(read_all(&folder, names))))
}

/// Removes a shell's container nobody is looking at any more.
fn discard_container(engine: Option<crate::engine::Engine>, container: &str) -> Command<Msg> {
    let Some(engine) = engine else { return Command::none() };
    let container = container.to_owned();
    Command::perform(move || {
        let _ = crate::engine::run::capture(&engine.remove_container(&container));
        Msg::Shell(ShellMsg::Discarded)
    })
}

/// The button that opens a profile's shell.
pub(super) fn offer(state: &Profiles, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    ui.add(Text::new(t!("profiles.shell.offer")).role("secondary")).fill_width();
    let mut button = Button::new(t!("profiles.shell.open")).disabled(engineless);
    if !engineless {
        button = button.on_press(Msg::Shell(ShellMsg::Asked));
    }
    ui.add(button).id("profile-shell");
}

/// The profile's own steps, under its row in the list, each with the way to take it off.
pub(super) fn draw_own(own: &Own, ui: &mut View<'_, Msg>) {
    if own.is_empty() {
        return;
    }
    let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    ui.add(Text::new(t!("profiles.own.title", n = count(own.steps.len()))).bold()).fill_width();
    for (index, step) in own.steps.iter().enumerate() {
        ui.row(|ui| {
            ui.add(Text::new(step.clone())).fill_width();
            ui.add(Button::new(t!("profiles.own.remove")).on_press(Msg::Shell(ShellMsg::Trim(index))))
                .id(format!("own-step-{index}"));
        })
        .gap(2)
        .fill_width();
    }
    if !own.home.is_empty() {
        ui.add(Text::new(t!("profiles.own.home", n = count(own.home.len()))).role("secondary")).fill_width();
    }
    ui.add(Text::new(t!("profiles.own.note")).role("secondary")).fill_width();
}

/// The shell's page.
pub(super) fn draw(page: &ShellPage, ui: &mut View<'_, Msg>) {
    let name = page.profile.name.to_string();
    ui.column(|ui| {
        ui.add(Text::new(t!("profiles.shell.title", name = name.as_str())).bold());
        ui.add(Text::new(t!("profiles.shell.lead")).role("secondary")).fill_width();
        match &page.stage {
            Stage::Opening => {
                ui.add(ShimmerText::new(t!("profiles.shell.opening"))).id("shell-working");
            }
            Stage::Open => {
                ui.row(|ui| {
                    let you = Button::new(t!("profiles.shell.you")).on_press(Msg::Shell(ShellMsg::Show(Side::You)));
                    let you = if page.showing == Side::You { you.variant("primary") } else { you };
                    ui.add(you).id("shell-you");
                    let (label, message) = if page.admin.is_some() {
                        (t!("profiles.shell.admin"), ShellMsg::Show(Side::Admin))
                    } else {
                        (t!("profiles.shell.admin"), ShellMsg::Admin)
                    };
                    let admin = Button::new(label).on_press(Msg::Shell(message));
                    let admin = if page.showing == Side::Admin { admin.variant("primary") } else { admin };
                    ui.add(admin).id("shell-admin");
                    ui.spacer();
                })
                .gap(2)
                .fill_width();
                if page.showing == Side::Admin {
                    ui.add(Text::new(t!("profiles.shell.admin-note")).color("warning")).fill_width();
                }
                let session = match page.showing {
                    Side::Admin => page.admin.as_ref().or(page.you.as_ref()),
                    Side::You => page.you.as_ref(),
                };
                if let Some(session) = session {
                    ui.add(Terminal::new(session))
                        .id("shell-terminal")
                        .fill_width()
                        .height(Length::Cells(VIEWPORT_ROWS));
                }
            }
            Stage::Leftover(session) => {
                ui.add(Text::new(t!("profiles.shell.leftover.lead")).color("warning")).fill_width();
                let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX).to_string();
                let changed = t!(
                    "profiles.shell.changed",
                    home = count(session.changes.home.len()),
                    system = count(session.changes.system.len())
                );
                ui.add(Text::new(changed).role("secondary")).fill_width();
                if !session.steps.is_empty() {
                    let n = i64::try_from(session.steps.len()).unwrap_or(i64::MAX);
                    ui.add(Text::new(t!("profiles.shell.steps", n = n)).role("secondary")).fill_width();
                    ui.add(Text::new(session.steps.join("\n"))).fill_width();
                }
                ui.row(|ui| {
                    let carry = Button::new(t!("profiles.shell.leftover.continue"))
                        .variant("primary")
                        .on_press(Msg::Shell(ShellMsg::Continue));
                    ui.add(carry).id("shell-continue");
                    let add =
                        Button::new(t!("profiles.shell.leftover.add")).on_press(Msg::Shell(ShellMsg::AddLeftover));
                    ui.add(add).id("shell-add-leftover");
                    let discard = Button::new(t!("profiles.shell.leftover.discard"))
                        .variant("danger")
                        .on_press(Msg::Shell(ShellMsg::Discard));
                    ui.add(discard).id("shell-discard-leftover");
                    ui.spacer();
                })
                .gap(2)
                .fill_width();
            }
            Stage::Surveying => {
                ui.add(ShimmerText::new(t!("profiles.shell.surveying"))).id("shell-working");
            }
            Stage::Deciding(_) => {
                ui.add(Text::new(t!("profiles.shell.deciding")).role("secondary")).fill_width();
            }
            Stage::Adding => {
                ui.add(ShimmerText::new(t!("profiles.shell.adding"))).id("shell-working");
            }
            Stage::Failed(problem) => {
                ui.add(Text::new(t!("profiles.shell.failed")).color("danger")).fill_width();
                if let Some(output) = problem.output() {
                    ui.add(Text::new(output.to_owned()).role("secondary")).fill_width();
                }
            }
        }
        ui.row(|ui| {
            ui.spacer();
            let closable = matches!(page.stage, Stage::Open | Stage::Failed(_));
            let mut close = Button::new(t!("profiles.shell.close")).variant("primary").disabled(!closable);
            if closable {
                close = close.on_press(Msg::Shell(ShellMsg::Close));
            }
            ui.add(close).id("shell-close");
        })
        .fill_width();
    })
    .gap(1)
    .width(Length::Cells(super::PAGE_WIDTH));
}
