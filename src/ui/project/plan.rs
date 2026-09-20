//! What a tab runs, and where.
//!
//! A tab never starts a program on the machine QCode runs on. It starts the engine binary, and
//! the engine carries the program into a container. Everything in this module is therefore a
//! plan rather than an action: a [`ContainerPlan`] knows a container's name, image, mounts and
//! network, and turns them into the exact [`EngineCommand`] that creates it or runs something
//! inside it. The commands can be read back in tests on a machine with no engine at all, which
//! is how the promise "nothing runs on the host" is checked rather than assumed.
//!
//! [`ensure_running`] is the one place that actually runs an engine; it belongs to a background
//! thread, never to `update` or `view`.

use std::path::{Path, PathBuf};

use crate::base;
use crate::desktop::{self, Display};
use crate::engine::{
    Access, Container, ContainerCreate, ContainerState, Engine, EngineCommand, Exec, HostUser, Mount, MountSource,
    Network, RunOnce, RunWindow, Socket, Tmpfs, names,
    run::{self, EngineError},
};
use crate::profile::identity::{self, Home};
use crate::profile::{Desktop, HarnessKind, MountAccess, NetworkMode, Profile};
use crate::workspace::{ProjectId, ProjectPaths};

/// The places every container agrees on, taken from the one contract the base image is built to.
///
/// They are re-exported here because the plan is what callers and tests reach for when they ask
/// where a mount lands; the definition stays with the image so the two can never disagree.
pub use crate::base::paths::{ASSETS_DIR, HOME_DIR, KEEP_ALIVE, MCP_DIR, PROJECT_DIR};

/// The label a container carries the digest of the plan it was made from in.
pub const PLAN_LABEL: &str = "qcode.plan";

/// What an empty terminal tab runs: a login shell in the project's base container.
pub const SHELL: &[&str] = &["sh", "-l"];

/// A container QCode keeps for a project: everything needed to create it and to enter it.
///
/// One plan per project for the plain shell ([`ContainerPlan::base`]) and one per profile the
/// project carries ([`ContainerPlan::profile`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPlan {
    /// The container's name, which is how QCode finds it again after a restart.
    pub name: String,
    /// The image it is created from.
    pub image: String,
    /// The project's own copy of the profile's home, mounted at [`HOME_DIR`], for a profile
    /// container; the base container has none because no harness lives in it.
    pub home: Option<Home>,
    /// The project's own files on the host, mounted writable at [`PROJECT_DIR`].
    pub project: PathBuf,
    /// The project's other material on the host, mounted at [`ASSETS_DIR`].
    pub assets: PathBuf,
    /// Whether the container may write to [`ASSETS_DIR`].
    pub assets_access: Access,
    /// Whether the container reaches the network.
    pub network: Network,
    /// The window this container opens, for the container of a desktop profile; `None` for every
    /// container a tab enters with a terminal.
    pub window: Option<&'static Desktop>,
    /// The bridge between the project's tabs, for a profile container: the project's
    /// `Containers/MCP/` on the host, mounted read-only at [`MCP_DIR`], and the harness whose
    /// settings the bridge's server is registered in. The base container has none, because no
    /// harness runs in it.
    pub bridge: Option<Bridge>,
    /// Where a window's container leaves the web addresses it wants opened: the project's
    /// `Containers/Browser/` on the host, mounted writable at [`desktop::signin::OPEN_DIR`].
    /// `None` for every container that opens no window.
    pub browser: Option<PathBuf>,
}

/// The bridge a profile container carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    /// The project's `Containers/MCP/` on the host.
    pub folder: PathBuf,
    /// The harness of the profile.
    pub harness: HarnessKind,
}

impl ContainerPlan {
    /// The project's plain shell container: the base image, the project's own folders, and no
    /// harness home because no harness runs in it.
    #[must_use]
    pub fn base(project: &str, paths: &ProjectPaths) -> Self {
        Self {
            name: names::base_container(project),
            image: names::BASE_IMAGE.to_owned(),
            home: None,
            project: paths.project.clone(),
            assets: paths.assets.clone(),
            assets_access: Access::ReadWrite,
            network: Network::Full,
            window: None,
            bridge: None,
            browser: None,
        }
    }

    /// The container one profile of the project lives in: the profile's image, the profile's
    /// permissions and the project's own copy of the profile's home.
    #[must_use]
    pub fn profile(project: &ProjectId, paths: &ProjectPaths, profile: &Profile) -> Self {
        Self {
            name: names::profile_container(project.as_str(), profile.name.as_str()),
            image: profile.image(),
            home: Some(Home::new(profile.name.clone(), project.clone())),
            project: paths.project.clone(),
            assets: paths.assets.clone(),
            assets_access: access(profile.assets),
            network: network(profile.network),
            window: None,
            bridge: Some(Bridge { folder: paths.mcp(), harness: profile.harness }),
            browser: None,
        }
    }

    /// The container the window of a desktop profile is open in: the same image, permissions and
    /// home volume as that profile's command-line container would have, under a name of its own.
    ///
    /// A window gets a container to itself rather than being started inside the long-lived one.
    /// Then the container's only program is the application: "the window closed" and "the
    /// container ended" become one fact, which the engine will tell QCode by itself, and nothing
    /// has to ask a compositor what is on screen. The two can stand side by side on the same home
    /// volume, so a profile's window and its command-line tabs share their settings and history.
    ///
    /// `None` for a profile whose harness draws in a terminal.
    #[must_use]
    pub fn window(project: &ProjectId, paths: &ProjectPaths, profile: &Profile) -> Option<Self> {
        let desktop = profile.harness.desktop()?;
        Some(Self {
            name: names::desktop_container(project.as_str(), profile.name.as_str()),
            window: Some(desktop),
            browser: Some(paths.browser()),
            // No bridge: what runs in this container is the application itself, not an agent of
            // QCode's, so there is nothing to answer the socket and no reason to let the
            // application see it.
            bridge: None,
            ..Self::profile(project, paths, profile)
        })
    }

    /// What the container can see, in the order the command lists it.
    fn mounts<'a>(&'a self, home: Option<&'a str>) -> Vec<Mount<'a>> {
        let mut mounts = vec![
            Mount {
                source: MountSource::Path(&self.project),
                target: Path::new(PROJECT_DIR),
                access: Access::ReadWrite,
            },
            Mount {
                source: MountSource::Path(&self.assets),
                target: Path::new(ASSETS_DIR),
                access: self.assets_access,
            },
        ];
        if let Some(home) = home {
            mounts.push(Mount {
                source: MountSource::Volume(home),
                target: Path::new(HOME_DIR),
                access: Access::ReadWrite,
            });
        }
        if let Some(browser) = &self.browser {
            // Writable, because the address the application wants opened is written here; it is
            // the only thing of the machine this container may write to besides the project.
            mounts.push(Mount {
                source: MountSource::Path(browser),
                target: Path::new(desktop::signin::OPEN_DIR),
                access: Access::ReadWrite,
            });
        }
        if let Some(bridge) = &self.bridge {
            mounts.push(Mount {
                source: MountSource::Path(&bridge.folder),
                target: Path::new(MCP_DIR),
                access: Access::ReadOnly,
            });
        }
        mounts
    }

    /// The command that creates the container, without starting it. The container carries the
    /// [`digest`](Self::digest) of this plan in [`PLAN_LABEL`].
    #[must_use]
    pub fn create(&self, engine: &Engine, user: HostUser) -> EngineCommand {
        let digest = self.digest(engine, user);
        self.create_labelled(engine, user, &[(PLAN_LABEL, &digest)])
    }

    /// What tells this plan from any other: a digest of the command that creates its container,
    /// label aside. A container made from another plan (other mounts, another network, a bridge
    /// it did not have yet) carries another digest, which is how `ensure_running` knows to make
    /// it again.
    #[must_use]
    pub fn digest(&self, engine: &Engine, user: HostUser) -> String {
        let command = self.create_labelled(engine, user, &[]);
        let mut spelled = command.program.as_os_str().to_string_lossy().into_owned();
        for arg in &command.args {
            spelled.push('\0');
            spelled.push_str(&arg.to_string_lossy());
        }
        format!("{:016x}", base::digest(&spelled))
    }

    fn create_labelled(&self, engine: &Engine, user: HostUser, labels: &[(&str, &str)]) -> EngineCommand {
        let home = self.home.as_ref().map(Home::volume);
        let mounts = self.mounts(home.as_deref());
        engine.create_container(&ContainerCreate {
            name: &self.name,
            hostname: names::HOSTNAME,
            labels,
            image: &self.image,
            mounts: &mounts,
            network: self.network,
            user,
            workdir: Some(Path::new(PROJECT_DIR)),
            command: KEEP_ALIVE,
        })
    }

    /// The command that runs `command` inside the container, attached to a terminal.
    ///
    /// This is the only command a tab ever spawns, which is what keeps every tab inside a
    /// container: the program is an argument of the engine, never something started here.
    #[must_use]
    pub fn enter(&self, engine: &Engine, command: &[&str]) -> EngineCommand {
        self.enter_with(engine, command, &[])
    }

    /// [`ContainerPlan::enter`] with environment variables for the program, as `(name, value)`.
    #[must_use]
    pub fn enter_with(&self, engine: &Engine, command: &[&str], env: &[(&str, &str)]) -> EngineCommand {
        engine.exec_with_env(&Exec { container: &self.name, command }, env)
    }

    /// The command that opens the window, given what this machine offers it and, for the engine
    /// that needs one, the seccomp profile at `seccomp`.
    ///
    /// `None` for a plan that opens no window. Every option and why it is there is in
    /// [`RunWindow`] and in [`crate::desktop`]; the shape of the call is the trial's, measured.
    #[must_use]
    pub fn open_window(
        &self,
        engine: &Engine,
        user: HostUser,
        display: &Display,
        seccomp: Option<&Path>,
    ) -> Option<EngineCommand> {
        let desktop = self.window?;
        let home = self.home.as_ref().map(Home::volume);
        let mounts = self.mounts(home.as_deref());
        let program = desktop.command_line(PROJECT_DIR);
        let command: Vec<&str> = program.iter().map(String::as_str).collect();
        // The one file of the machine's runtime folder the window needs, inside a folder of the
        // container's own, so that nothing else living beside it comes along.
        let target = display.target();
        let sockets = [Socket { host: &display.socket, target: &target }];
        let tmpfs = [Tmpfs { target: Path::new(desktop::RUNTIME_DIR), mode: desktop::RUNTIME_MODE }];
        let devices: Vec<&Path> = display.device.iter().map(PathBuf::as_path).collect();
        let mut environment = display.environment();
        // What the application runs when it wants a web address opened. Without it the call goes
        // to `xdg-open`, which inside a container finds no browser and quietly does nothing.
        environment.push(("BROWSER".to_owned(), desktop::signin::OPEN_PROGRAM.to_owned()));
        let env: Vec<(&str, &str)> = environment.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
        Some(engine.run_window(&RunWindow {
            name: &self.name,
            once: RunOnce {
                image: &self.image,
                mounts: &mounts,
                network: self.network,
                user,
                workdir: Some(Path::new(PROJECT_DIR)),
                command: &command,
            },
            env: &env,
            sockets: &sockets,
            tmpfs: &tmpfs,
            devices: &devices,
            shm: desktop::SHM_SIZE,
            seccomp,
        }))
    }

    /// The command that asks the open window to show itself: the application started a second time
    /// inside the container it already runs in.
    ///
    /// A Wayland application cannot raise its own window without an activation token from the
    /// compositor, and QCode, being a terminal application, has none to hand it. What this does is
    /// what a second start of an editor of this family does: it finds the instance already running
    /// on the same data folder, tells it, and exits. Whether the window then comes forward or is
    /// only marked as asking for attention is the compositor's to decide, and it differs between
    /// them; either way the person is pointed at the window they asked for.
    ///
    /// `None` for a plan that opens no window.
    #[must_use]
    pub fn raise_window(&self, engine: &Engine) -> Option<EngineCommand> {
        let desktop = self.window?;
        let program = desktop.command_line(PROJECT_DIR);
        let command: Vec<&str> = program.iter().map(String::as_str).collect();
        Some(engine.exec_without_terminal(&Exec { container: &self.name, command: &command }))
    }
}

/// An engine command that did not do what was asked, kept as text so a tab can show the engine's
/// own words.
///
/// The engine layer's error carries an [`std::io::Error`], which cannot be cloned or sent
/// through a message, so it is turned into its words at this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFailure {
    /// The command that was run, written out as program and arguments.
    pub command: String,
    /// Everything the engine said, or the reason it could not be started.
    pub output: String,
}

impl LaunchFailure {
    /// The failure of a command that could not even be started in a pseudo-terminal, which is
    /// how a tab learns that the engine binary went missing between two frames.
    #[must_use]
    pub fn spawn(command: &EngineCommand, error: &std::io::Error) -> Self {
        Self { command: written(command), output: error.to_string() }
    }

    /// The failure of the base image build, in the words the person is shown.
    ///
    /// A build context that could not be written has no engine command behind it, so the
    /// command is left empty and only the machine's words are shown.
    pub(super) fn base(failure: &base::Failure) -> Self {
        match failure {
            base::Failure::Host(error) => Self { command: String::new(), output: error.to_string() },
            base::Failure::Engine(error) => Self::from(error),
        }
    }

    /// The failure of this machine itself rather than of the engine, which has no command to show
    /// above its words.
    pub(super) fn from_host(error: &std::io::Error) -> Self {
        Self { command: String::new(), output: error.to_string() }
    }

    /// The failure of an engine command, in the words the person is shown.
    pub(super) fn from(error: &EngineError) -> Self {
        match error {
            EngineError::NotRunnable { command, error } => {
                Self { command: written(command), output: error.to_string() }
            }
            EngineError::Failed(failure) => Self { command: written(&failure.command), output: failure.output.clone() },
            EngineError::Cancelled { command } => Self { command: written(command), output: String::new() },
        }
    }
}

/// A command written out the way it would be typed, for showing beside its output.
fn written(command: &EngineCommand) -> String {
    let mut text = command.program.display().to_string();
    for arg in &command.args {
        text.push(' ');
        text.push_str(&arg.to_string_lossy());
    }
    text
}

/// Makes sure the container of `plan` is up, creating it when it has never existed and starting
/// it when it is only stopped.
///
/// A plain shell container is made from the base image, which a machine that has never built a
/// profile does not have yet; it is built here first, so the first tab ever opened comes up
/// rather than failing with "no such image". A tab has no log, so the build's own lines go
/// nowhere and the tab shows that it is starting for as long as the build takes; a failure is
/// shown with the engine's words like any other. A profile container is made from the profile's
/// own image, which the profiles screen builds, so nothing is built here for one.
///
/// A profile container's home volume is made along with the container, and a home made here is
/// given the profile's stored login before the container starts, so the first tab opened in a
/// project does not ask for a login that was made already. A home that was there before the
/// container is left as it is: it is the project's own copy, and only the person may have it
/// rewritten.
///
/// A container takes its mounts and its network when it is made, so a stopped container made
/// from another plan than this one (its [`PLAN_LABEL`] says so) is made again: removed and
/// created anew, which keeps every named volume and so the home with the harness's login,
/// history and settings in it. What was only in the container itself, outside the home and the
/// project's folders, goes with it, as it does when the container is removed by hand. A running
/// container is never made again, because tabs are working in it; it keeps its old plan until
/// it is next stopped, which happens by the latest when no QCode is open.
///
/// This runs engine commands and waits for them, so it belongs on a background thread: give it
/// to `Command::perform`, never to `update` or `view`.
///
/// # Errors
///
/// The engine's own words when it cannot be started, or refuses to build the base image, to
/// create or start the container, or to copy the login in; the machine's when the bridge's
/// folder cannot be made.
pub fn ensure_running(engine: &Engine, plan: &ContainerPlan, user: HostUser) -> Result<(), LaunchFailure> {
    // A container QCode cannot ask after is one that is not there: both engines answer an
    // unknown name with an error rather than with a state.
    let state = run::capture(&engine.container_state(&plan.name)).map(|word| ContainerState::parse(&word)).ok();
    let exists = match state {
        Some(ContainerState::Running) => return Ok(()),
        Some(_) => {
            let made_from = run::capture(&engine.container_label(&plan.name, PLAN_LABEL)).unwrap_or_default();
            let current = made_from.trim() == plan.digest(engine, user);
            if !current {
                run::capture(&engine.remove_container(&plan.name)).map_err(|error| LaunchFailure::from(&error))?;
            }
            current
        }
        None => false,
    };
    if !exists {
        if plan.image == names::BASE_IMAGE {
            base::ensure(engine, &|| false, &mut |_| {}).map_err(|failure| LaunchFailure::base(&failure))?;
        }
        // An engine mounting a folder that is not there fails, or makes it as its own user; the
        // folder is made here, as the person.
        if let Some(bridge) = &plan.bridge {
            std::fs::create_dir_all(&bridge.folder).map_err(|error| LaunchFailure {
                command: String::new(),
                output: format!("{}: {error}", bridge.folder.display()),
            })?;
        }
        // Asked before the creation, because the creation is what makes the volume.
        let mut new_home = None;
        if let Some(home) = &plan.home
            && !identity::home_exists(engine, home).map_err(|error| LaunchFailure::from(&error))?
        {
            new_home = Some(home);
        }
        run::capture(&plan.create(engine, user)).map_err(|error| LaunchFailure::from(&error))?;
        if let Some(home) = new_home {
            identity::first_fill(engine, home, user).map_err(|error| LaunchFailure::from(&error))?;
        }
    }
    run::capture(&engine.start_container(&plan.name)).map_err(|error| LaunchFailure::from(&error))?;
    Ok(())
}

/// What opening a window came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// A container was started and the window is on its way up.
    Opened,
    /// A container of that name was already running, so the tab took that window rather than
    /// opening a second one. This is what a QCode that crashed with a window open leaves behind,
    /// and what a second tab of the same profile would otherwise duplicate.
    Adopted,
}

/// Opens the window of `plan` and says whether it was started or found already up.
///
/// A container of the same name that is not running is in nobody's way and is removed: it is the
/// remains of a window that closed while no QCode was watching, or of one a closing QCode stopped,
/// and the window is opened fresh. Nothing of the person's is in it — the settings, the history and
/// the login are all in the home volume, which the new container mounts.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it cannot be started, or refuses to remove the old container or to
/// run the new one; and the machine's when the seccomp profile cannot be written.
pub fn open_window(
    engine: &Engine,
    plan: &ContainerPlan,
    user: HostUser,
    display: &Display,
) -> Result<Window, LaunchFailure> {
    let state = run::capture(&engine.container_state(&plan.name)).map(|word| ContainerState::parse(&word)).ok();
    match state {
        Some(ContainerState::Running) => return Ok(Window::Adopted),
        Some(_) => {
            run::capture(&engine.remove_container(&plan.name)).map_err(|error| LaunchFailure::from(&error))?;
        }
        None => {}
    }
    // The folder the window writes addresses into has to be there before the container starts:
    // an engine refuses to mount a path that does not exist.
    if let Some(browser) = &plan.browser {
        std::fs::create_dir_all(browser).map_err(|error| LaunchFailure::from_host(&error))?;
    }
    let seccomp = if engine.needs_sandbox_profile() {
        Some(desktop::seccomp::file().map_err(|error| LaunchFailure::from_host(&error))?)
    } else {
        None
    };
    let Some(command) = plan.open_window(engine, user, display, seccomp.as_deref()) else {
        return Ok(Window::Opened);
    };
    run::capture(&command).map_err(|error| LaunchFailure::from(&error))?;
    Ok(Window::Opened)
}

/// Waits for the window's container to end and answers its exit code, or `None` when the engine
/// said nothing a code could be read from.
///
/// This is how the person closing the window reaches QCode: the container's only program is the
/// application, so the container ends when the window does. It blocks for as long as the window is
/// open, so it belongs on a background thread of its own and on no other.
#[must_use]
pub fn await_window(engine: &Engine, name: &str) -> Option<u32> {
    run::capture(&engine.wait_container(name)).ok()?.trim().lines().last()?.trim().parse().ok()
}

/// Closes the window of `name` and takes its container away.
///
/// The stop is what closes the window: the application answers the signal and writes its state
/// out, which is why it is given [`desktop::WINDOW_GRACE`]. A container that had already ended is not
/// there
/// to stop, and saying so is not a failure; the removal is what must succeed.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it will not remove the container.
pub fn close_window(engine: &Engine, name: &str) -> Result<(), LaunchFailure> {
    // Closing happens twice over: the person presses Close, and the tab's own wait on the
    // container answers the moment it is gone and clears up after it. Whichever arrives second
    // finds nothing, and must not report the first one's work as a failure — docker refuses to
    // remove a container it does not know, where podman shrugs.
    if run::capture(&engine.container_state(name)).is_err() {
        return Ok(());
    }
    let _ = run::capture(&engine.stop_container_within(name, desktop::WINDOW_GRACE));
    run::capture(&engine.remove_container(name)).map(|_| ()).map_err(|error| LaunchFailure::from(&error))
}

/// Asks the open window of `plan` to show itself.
///
/// Runs an engine command and waits for it, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when the container is not there any more or refuses.
pub fn raise_window(engine: &Engine, plan: &ContainerPlan) -> Result<(), LaunchFailure> {
    let Some(command) = plan.raise_window(engine) else { return Ok(()) };
    run::capture(&command).map(|_| ()).map_err(|error| LaunchFailure::from(&error))
}

/// Whether the container of `plan` is running right now.
///
/// Runs an engine command, so it belongs on a background thread. An engine that cannot answer
/// says "not running", which is the answer that offers the person a way out.
#[must_use]
pub fn is_running(engine: &Engine, plan: &ContainerPlan) -> bool {
    run::capture(&engine.container_state(&plan.name)).is_ok_and(|word| ContainerState::parse(&word).is_running())
}

/// Every container the engine knows that belongs to `project`, in the engine's order.
///
/// Runs an engine command, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it cannot be asked.
pub fn project_containers(engine: &Engine, project: &str) -> Result<Vec<Container>, LaunchFailure> {
    let listing = run::capture(&engine.list_containers()).map_err(|error| LaunchFailure::from(&error))?;
    let prefix = format!("qcode-{project}-");
    Ok(Container::parse_list(&listing).into_iter().filter(|container| container.name.starts_with(&prefix)).collect())
}

/// Stops a container and waits for it to be stopped.
///
/// Runs an engine command, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it refuses.
pub fn stop(engine: &Engine, name: &str) -> Result<(), LaunchFailure> {
    run::capture(&engine.stop_container(name)).map(|_| ()).map_err(|error| LaunchFailure::from(&error))
}

/// Stops a container and starts it again.
///
/// Runs an engine command, so it belongs on a background thread. A container that was already
/// stopped is only started, so a restart never fails for having nothing to stop.
///
/// # Errors
///
/// The engine's own words when it refuses to start the container.
pub fn restart(engine: &Engine, name: &str) -> Result<(), LaunchFailure> {
    let _ = run::capture(&engine.stop_container(name));
    run::capture(&engine.start_container(name)).map(|_| ()).map_err(|error| LaunchFailure::from(&error))
}

/// The engine's word for a profile's access to `Assets/`.
fn access(access: MountAccess) -> Access {
    match access {
        MountAccess::ReadWrite => Access::ReadWrite,
        MountAccess::ReadOnly => Access::ReadOnly,
    }
}

/// The engine's word for a profile's network permission.
fn network(mode: NetworkMode) -> Network {
    match mode {
        NetworkMode::Full => Network::Full,
        NetworkMode::None => Network::None,
    }
}
