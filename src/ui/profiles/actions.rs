//! What the profiles screen does beyond changing its own state: deleting a profile, reading the
//! store again, signing out, moving through the wizard, building an image and signing in, in a
//! terminal or in a window.

use std::sync::Arc;

use qframe::prelude::*;
use qframe::runtime::Task;
use qframe::widgets::{LogBuffer, LogLevel, LogLine, TerminalEvent, Toast};

use crate::desktop::login::{self, Seen, SignIn};
use crate::desktop::{NoDisplay, callback, signin};
use crate::engine::Engine;
use crate::profile::Profile;
use crate::provider::Providers;

use super::{
    Blocked, Build, Draft, Listing, Login, Msg, Page, Problem, Profiles, Readiness, Stage, Unfinished, WindowBack,
    WindowLogin, builds, remove, work,
};

/// Starts deleting the chosen profile by asking the engine what it holds of it, so the question
/// can name everything that goes.
pub(super) fn ask_delete(state: &mut Profiles) -> Command<Msg> {
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
pub(super) fn delete_question(state: &mut Profiles, survey: Box<remove::Survey>) -> Command<Msg> {
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
pub(super) fn delete(state: &mut Profiles, homes: bool) -> Command<Msg> {
    let (Some(store), Some(survey)) = (state.store(), state.doomed.take()) else { return Command::none() };
    let engine = state.engine.clone();
    Command::perform(move || {
        let outcome = remove::remove(&store, engine.as_ref(), &survey, homes);
        Msg::Deleted(survey, outcome)
    })
}

/// Says how the deletion went and reads the list again, which is the honest account of what is
/// left.
pub(super) fn deleted(state: &mut Profiles, survey: &remove::Survey, outcome: remove::Outcome) -> Command<Msg> {
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
pub(super) fn reload(state: &mut Profiles) -> Command<Msg> {
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
pub(super) fn load_providers(state: &Profiles) -> Command<Msg> {
    let path = state.providers_file.clone();
    Command::perform(move || {
        let providers = path.map(|path| Providers::open(&path).value).unwrap_or_else(Providers::in_memory);
        Msg::ProvidersLoaded(providers.entries().to_vec())
    })
}

/// Asks the engine whether each profile has an image and a login.
pub(super) fn probe(state: &Profiles, profiles: Vec<Profile>) -> Command<Msg> {
    let (Some(engine), false) = (state.engine.clone(), profiles.is_empty()) else { return Command::none() };
    Command::task(Task::new(t!("profiles.probing"), move |_| Ok(Msg::Probed(work::probe(&engine, &profiles)))))
}

/// Asks before removing a login, saying plainly what goes and what stays.
pub(super) fn ask_sign_out(state: &Profiles) -> Command<Msg> {
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
pub(super) fn ask_rebuild(state: &Profiles) -> Command<Msg> {
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
pub(super) fn sign_out(state: &mut Profiles) -> Command<Msg> {
    let (Some(engine), Some(row)) = (state.engine.clone(), state.selected()) else { return Command::none() };
    let name = row.profile.name.clone();
    state.signing_out = true;
    Command::task(Task::new(t!("profiles.sign-out"), move |_| Ok(Msg::SignedOut(work::sign_out(&engine, &name)))))
}

/// Goes on a page, or says why it cannot.
pub(super) fn next(state: &mut Profiles) -> Command<Msg> {
    let Some(draft) = &mut state.draft else { return Command::none() };
    match draft.advance() {
        Some(Blocked::NameEmpty | Blocked::NameTaken) => Command::focus("profile-name"),
        Some(Blocked::NoImage) => Command::focus("build-start"),
        Some(Blocked::NoProvider) => Command::focus("profile-provider"),
        Some(Blocked::Unsupported(_)) => Command::focus("profile-system"),
        Some(Blocked::NeedsNetwork) => Command::focus("profile-network"),
        None if draft.stage == Stage::Image && draft.build == Build::Waiting => start_build(state),
        None => arrive(draft),
    }
}

/// Puts the keyboard on the question of the page just reached, the way the first-start wizard
/// does, rather than leaving it on a button of the page left. The image page has no question: its
/// build starts by itself, and Next, where the keyboard already is, is what follows it.
pub(super) fn arrive(draft: &Draft) -> Command<Msg> {
    let control = match draft.stage {
        Stage::Harness => "profile-harness",
        Stage::System => "profile-system",
        Stage::Template => "profile-template",
        Stage::Account => "profile-account",
        Stage::Permissions => "profile-assets",
        Stage::Image => return Command::none(),
        Stage::Login => "login-open",
    };
    Command::focus(control)
}

/// Leaves the wizard. Whatever the engine has already made — the image, the login — stays; what
/// is still running is stopped and cleared away.
pub(super) fn leave(state: &mut Profiles) -> Command<Msg> {
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
pub(super) fn start_build(state: &mut Profiles) -> Command<Msg> {
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
        draft.building(task.id());
        return Command::batch([Command::task(task), builds::follow(state)]);
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
    draft.building(task.id());
    Command::batch([Command::task(task), builds::follow(state)])
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
    draft.building(task.id());
    Command::batch([Command::task(task), builds::follow(state)])
}

/// Opens the container the login happens in and starts the harness on a terminal.
pub(super) fn start_login(state: &mut Profiles) -> Command<Msg> {
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
pub(super) fn login_event(state: &mut Profiles, event: TerminalEvent) -> Command<Msg> {
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
pub(super) fn store_login(state: &mut Profiles) -> Command<Msg> {
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
pub(super) fn cancel_login(state: &mut Profiles) -> Command<Msg> {
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
pub(super) fn discard(engine: Option<Engine>, container: Arc<work::LoginContainer>) -> Command<Msg> {
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
pub(super) fn window_opened(state: &mut Profiles, run: u64, sign_in: Arc<SignIn>) -> Command<Msg> {
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
pub(super) fn current_window(state: &mut Profiles, run: u64) -> Option<&mut WindowLogin> {
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
pub(super) fn window_asked(state: &mut Profiles, run: u64, addresses: &[String]) -> Command<Msg> {
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
pub(super) fn report(log: &mut LogBuffer, problem: &Problem) {
    if let Some(output) = problem.output() {
        for line in output.lines() {
            log.push(LogLine::new(LogLevel::Error, line));
        }
    }
}

/// A toast that says what failed and shows the first line the engine said about it.
pub(super) fn failure_toast(key: &str, problem: &Problem) -> Toast<Msg> {
    let toast = Toast::danger(t!(key));
    match problem.output().and_then(|output| output.lines().next()) {
        Some(line) => toast.body(line.to_owned()),
        None => toast,
    }
}
