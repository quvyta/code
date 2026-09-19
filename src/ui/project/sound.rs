//! The tab of a sound: played by sox in a container made for it alone, or described when it
//! cannot or should not be played.
//!
//! Sound leaves a container only through the machine's sound server, whose socket has to be
//! mounted into the container when it is made. The project's own container was made long before
//! and cannot be given a mount afterwards, and should not be: it runs a shell and whatever the
//! person starts in it. So a sound plays in a one-off container of the base image that runs
//! `play` on that one file and nothing else, sees the project read-only, reaches no network, and
//! is removed the moment `play` ends. When the settings say "details only", or this machine has
//! no sound server, nothing is mounted anywhere: the project's own container reads the file's
//! header with `soxi`, and the tab shows that with a line saying why it does not play.

use std::path::{Path, PathBuf};

use qframe::keymap::KeyChord;
use qframe::prelude::*;

use crate::base::apps::{Hearing, Quiet};
use crate::base::{self, apps};
use crate::engine::names::{BASE_IMAGE, sound_container};
use crate::engine::run::capture;
use crate::engine::{
    Access, Engine, EngineCommand, Exec, HostUser, Mount, MountSource, Network, RunAttached, RunOnce, Socket,
};

use super::plan::{self, ContainerPlan, LaunchFailure, PROJECT_DIR};
use super::viewer::{self, Taken};
use super::{Msg, OpenProject, ProjectScreen, Tab, TabKey, TabKind, TabState, inside_container};

/// The command that plays the sound of the tab `tab` of `project` through `socket`: the engine
/// running `play` in a one-off container of the base image, attached to the tab's terminal.
pub(super) fn play_command(
    engine: &Engine,
    user: HostUser,
    project: &OpenProject,
    tab: &Tab,
    socket: &Path,
) -> Option<EngineCommand> {
    let TabKind::Sound(file) = tab.kind() else { return None };
    let name = sound_container(project.id(), tab.key().0);
    // Read-only: a player has nothing to write, and a sound that can change the project is a
    // permission nobody asked for.
    let mounts = [Mount {
        source: MountSource::Path(&project.paths().project),
        target: Path::new(PROJECT_DIR),
        access: Access::ReadOnly,
    }];
    let program = apps::play(&inside_container(file));
    let command: Vec<&str> = program.iter().map(String::as_str).collect();
    let server = format!("unix:{}", apps::SOUND_SOCKET);
    let sockets = [Socket { host: socket, target: Path::new(apps::SOUND_SOCKET) }];
    Some(engine.run_attached(&RunAttached {
        name: &name,
        once: RunOnce {
            image: BASE_IMAGE,
            mounts: &mounts,
            network: Network::None,
            user,
            workdir: Some(Path::new(PROJECT_DIR)),
            command: &command,
        },
        // The client looks for a login cookie in a home this container does not have and says so
        // on the tab's terminal; the sound server this reaches (PipeWire's, or PulseAudio's own
        // socket of this user) does not ask for one, so the cookie is sent to a place it can be
        // made and the tab stays quiet.
        env: &[("PULSE_SERVER", &server), ("PULSE_COOKIE", "/tmp/qcode-pulse-cookie")],
        sockets: &sockets,
    }))
}

/// The command that describes the sound of the tab `key`: `soxi` in the project's own
/// container, without a terminal.
#[must_use]
pub(super) fn details_command(screen: &ProjectScreen, key: TabKey) -> Option<EngineCommand> {
    let engine = screen.engine.as_ref()?;
    let project = screen.projects.iter().find(|project| project.tabs.iter().any(|tab| tab.key() == key))?;
    let TabKind::Sound(file) = project.tabs.iter().find(|tab| tab.key() == key)?.kind() else { return None };
    let plan = ContainerPlan::base(project.id(), project.paths());
    let program = apps::sound_details(&inside_container(file));
    let parts: Vec<&str> = program.iter().map(String::as_str).collect();
    Some(engine.exec_without_terminal(&Exec { container: &plan.name, command: &parts }))
}

/// Meets the sound of the tab `key` in its run `run`, on a background thread: the file is looked
/// for first, then the settings and this machine decide between playing and describing.
///
/// To play, the base image has to be there, which is all a one-off container needs; the
/// container is noted like every container QCode starts, so it is stopped with the others once
/// no QCode is open, even when it was left playing. To describe, the project's own container is
/// brought up and asked.
pub(super) fn hear(screen: &ProjectScreen, key: TabKey, run: u64) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let Some(project) = screen.projects.iter().find(|project| project.tabs.iter().any(|tab| tab.key() == key)) else {
        return Command::none();
    };
    let Some(tab) = project.tabs.iter().find(|tab| tab.key() == key) else { return Command::none() };
    let Some(file) = tab.kind().file().map(|file| project.files.path(file)) else { return Command::none() };
    let (Some(details), held) = (details_command(screen, key), tab.is_held()) else { return Command::none() };
    let plan = ContainerPlan::base(project.id(), project.paths());
    let name = sound_container(project.id(), key.0);
    let (user, registry, choice, runtime) =
        (screen.user, screen.registry.clone(), screen.sound, screen.runtime.clone());
    Command::perform(move || {
        if !file.exists() {
            return Msg::Missing(key, run);
        }
        match apps::hearing(choice, apps::sound_socket(runtime.as_deref())) {
            Hearing::Play(socket) => {
                if let Err(failure) = base::ensure(&engine, &|| false, &mut |_| {}) {
                    return Msg::Playable(key, run, Err(LaunchFailure::base(&failure)));
                }
                let message = Msg::Playable(key, run, Ok(socket));
                if held { message } else { super::noted(registry.as_deref(), engine.kind(), &name, message) }
            }
            Hearing::Details(why) => {
                if let Err(failure) = plan::ensure_running(&engine, &plan, user) {
                    return Msg::Described(key, run, why, Err(failure));
                }
                let read = capture(&details).map(Taken::new).map_err(|error| LaunchFailure::from(&error));
                super::noted(registry.as_deref(), engine.kind(), &plan.name, Msg::Described(key, run, why, read))
            }
        }
    })
}

/// Takes the answer that the sound of the tab `key` can play through `socket`: it starts playing,
/// unless the tab was brought back from the last session, which waits to be asked.
pub(super) fn playable(
    screen: &mut ProjectScreen,
    key: TabKey,
    run: u64,
    answer: Result<PathBuf, LaunchFailure>,
) -> Command<Msg> {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|project| project.find(key)) else {
        return Command::none();
    };
    if tab.run() != run || !tab.state().is_starting() {
        return Command::none();
    }
    let socket = match answer {
        Ok(socket) => socket,
        Err(failure) => {
            tab.settled(TabState::Failed(failure));
            return Command::none();
        }
    };
    tab.play_through(socket);
    if tab.is_held() {
        tab.settled(TabState::Ended { code: None });
        return Command::none();
    }
    super::ready(screen, key, run, &Ok(()))
}

/// Takes the details of the sound of the tab `key`, and why they are shown instead of the sound.
pub(super) fn described(
    screen: &mut ProjectScreen,
    key: TabKey,
    run: u64,
    why: Quiet,
    answer: Result<Taken, LaunchFailure>,
) -> Command<Msg> {
    if let Some((_, tab)) = screen.owner_mut(key).and_then(|project| project.find(key))
        && tab.run() == run
    {
        tab.keep_quiet(why);
    }
    viewer::text_read(screen, key, run, answer)
}

/// Takes away the containers the sound tabs of `tabs` still play in, on a background thread.
///
/// Ending a tab's session ends the engine's client, not the container: `play` would go on to the
/// end of the file with no tab left to stop it. Removing the container stops it at once, and a
/// container that ended already is simply not there to remove.
pub(super) fn silence(screen: &ProjectScreen, project: &OpenProject, tabs: &[&Tab]) -> Command<Msg> {
    let Some(engine) = screen.engine.clone() else { return Command::none() };
    let names = playing(project, tabs);
    if names.is_empty() {
        return Command::none();
    }
    Command::perform(move || {
        for name in &names {
            let _ = capture(&engine.remove_container(name));
        }
        Msg::Silenced
    })
}

/// The containers the sound tabs of `tabs` play in right now.
pub(super) fn playing(project: &OpenProject, tabs: &[&Tab]) -> Vec<String> {
    tabs.iter()
        .filter(|tab| matches!(tab.kind(), TabKind::Sound(_)) && tab.session().is_some())
        .filter(|tab| tab.state() == &TabState::Running)
        .map(|tab| sound_container(project.id(), tab.key().0))
        .collect()
}

/// Whether this module draws the tab `tab`: a sound tab that plays, played, or shows its details.
pub(super) fn shows(tab: &Tab) -> bool {
    matches!(tab.kind(), TabKind::Sound(_)) && matches!(tab.state(), TabState::Running | TabState::Ended { .. })
}

/// A sound tab: the player in its terminal while it plays, with the way to stop it; the player's
/// last screen and the way to play it again once it ended; or the details and why it does not
/// play.
pub(super) fn view(tab: &Tab, file: &str, ui: &mut View<'_, Msg>) {
    let key = tab.key();
    if let Some(why) = tab.quiet() {
        let reason = match why {
            Quiet::Chosen => t!("project.sound.chosen"),
            Quiet::NoServer => t!("project.sound.no-server"),
        };
        viewer::text_page(tab, file, None, Some(reason), ui);
        return;
    }
    ui.column(|ui| {
        match tab.session() {
            Some(session) => super::terminal(ui, session),
            None => {
                ui.column(|ui| {
                    ui.add(Text::new(super::files::name(file).to_owned()));
                })
                .fill()
                .align(Align::Center)
                .justify(Align::Center);
            }
        }
        ui.row(|ui| {
            if tab.state() == &TabState::Running {
                let key = "ctrl+c".parse::<KeyChord>().map_or_else(|_| "ctrl+c".to_owned(), |chord| chord.label());
                ui.add(Text::new(t!("project.sound.stop", key = key)).role("faint"));
            } else {
                let again = if tab.session().is_some() { t!("project.sound.again") } else { t!("project.sound.play") };
                ui.add(Button::new(again).variant("primary").on_press(Msg::Restart(key))).id("project-play");
            }
        })
        .gap(2)
        .padding(Padding { left: 2, right: 1, ..Padding::default() })
        .fill_width();
    })
    .gap(1)
    .fill()
    .id("project-sound");
}
