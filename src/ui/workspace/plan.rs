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
use crate::bridge::config::{self, Unregistered};
use crate::desktop::{self, Display};
use crate::engine::{
    Access, Container, ContainerCreate, ContainerState, Engine, EngineCommand, Exec, HostUser, Mount, MountSource,
    Network, RunOnce, RunWindow, Socket, Tmpfs,
    known::{self, Known},
    names,
    run::{self, EngineError},
};
use crate::profile::guidance::{self, Unguided};
use crate::profile::identity::{self, Home};
use crate::profile::{Desktop, Extra, HarnessKind, MountAccess, NetworkMode, Profile, Template};
use crate::store::{WorkspaceId, WorkspacePaths};

/// The places every container agrees on, taken from the one contract the base image is built to.
///
/// They are re-exported here because the plan is what callers and tests reach for when they ask
/// where a mount lands; the definition stays with the image so the two can never disagree.
pub use crate::base::paths::{ASSETS_DIR, CODE_DIR, HOME_DIR, KEEP_ALIVE, MCP_DIR};

/// The label a container carries the digest of the plan it was made from in.
pub const PLAN_LABEL: &str = "qcode.plan";

/// What an empty terminal tab runs: a login shell in the workspace's base container.
pub const SHELL: &[&str] = &["sh", "-l"];

/// A container QCode keeps for a workspace: everything needed to create it and to enter it.
///
/// One plan per workspace for the plain shell ([`ContainerPlan::base`]) and one per profile the
/// workspace carries ([`ContainerPlan::profile`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPlan {
    /// The container's name, which is how QCode finds it again after a restart.
    pub name: String,
    /// The image it is created from.
    pub image: String,
    /// The workspace's own copy of the profile's home, mounted at [`HOME_DIR`], for a profile
    /// container; the base container has none because no harness lives in it.
    pub home: Option<Home>,
    /// The workspace's own files on the host, mounted writable at [`CODE_DIR`].
    pub code: PathBuf,
    /// The workspace's other material on the host, mounted at [`ASSETS_DIR`].
    pub assets: PathBuf,
    /// Whether the container may write to [`ASSETS_DIR`].
    pub assets_access: Access,
    /// Whether the container reaches the network.
    pub network: Network,
    /// The window this container opens, for the container of a desktop profile; `None` for every
    /// container a tab enters with a terminal.
    pub window: Option<&'static Desktop>,
    /// The bridge between the workspace's tabs, for a profile container: the workspace's
    /// `Containers/MCP/` on the host, mounted read-only at [`MCP_DIR`], and the harness whose
    /// settings the bridge's server is registered in. The base container has none, because no
    /// harness runs in it.
    pub bridge: Option<Bridge>,
    /// Where a window's container leaves the web addresses it wants opened: the workspace's
    /// `Containers/Browser/` on the host, mounted writable at [`desktop::signin::OPEN_DIR`].
    /// `None` for every container that opens no window.
    pub browser: Option<PathBuf>,
    /// The harness whose instruction files in the workspace are brought up to date when the
    /// container comes up — graphify's section and hooks, and QCode's own section — for a profile
    /// on QCode high; `None` for every other container, which leaves the workspace's files alone.
    pub guidance: Option<HarnessKind>,
    /// Whether graphify is in the profile's image, so that its installer runs in the workspace and
    /// its map is built there when the container comes up. False for every container but that of
    /// a QCode high profile that kept graphify.
    pub graphify: bool,
}

/// The bridge a profile container carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    /// The workspace's `Containers/MCP/` on the host.
    pub folder: PathBuf,
    /// The harness of the profile.
    pub harness: HarnessKind,
}

impl ContainerPlan {
    /// The workspace's plain shell container: the base image, the workspace's own folders, and no
    /// harness home because no harness runs in it.
    #[must_use]
    pub fn base(workspace: &str, paths: &WorkspacePaths) -> Self {
        Self {
            name: names::base_container(workspace),
            image: names::BASE_IMAGE.to_owned(),
            home: None,
            code: paths.code.clone(),
            assets: paths.assets.clone(),
            assets_access: Access::ReadWrite,
            network: Network::Full,
            window: None,
            bridge: None,
            browser: None,
            guidance: None,
            graphify: false,
        }
    }

    /// The container one profile of the workspace lives in: the profile's image, the profile's
    /// permissions and the workspace's own copy of the profile's home.
    #[must_use]
    pub fn profile(workspace: &WorkspaceId, paths: &WorkspacePaths, profile: &Profile) -> Self {
        Self {
            name: names::profile_container(workspace.as_str(), profile.name.as_str()),
            image: profile.image(),
            home: Some(Home::new(profile.name.clone(), workspace.clone())),
            code: paths.code.clone(),
            assets: paths.assets.clone(),
            assets_access: access(profile.assets),
            network: network(profile.network),
            window: None,
            bridge: Some(Bridge { folder: paths.mcp(), harness: profile.harness }),
            browser: None,
            guidance: (profile.template == Template::High).then_some(profile.harness),
            graphify: profile.has(Extra::Graphify),
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
    /// The bridge comes along, because the agent inside the window is an agent like any other:
    /// it starts the same server and asks over the same socket. Only the way back is missing,
    /// there being no prompt on a window to type an answer into.
    ///
    /// `None` for a profile whose harness draws in a terminal.
    #[must_use]
    pub fn window(workspace: &WorkspaceId, paths: &WorkspacePaths, profile: &Profile) -> Option<Self> {
        let desktop = profile.harness.desktop()?;
        Some(Self {
            name: names::desktop_container(workspace.as_str(), profile.name.as_str()),
            window: Some(desktop),
            browser: Some(paths.browser()),
            ..Self::profile(workspace, paths, profile)
        })
    }

    /// What the container can see, in the order the command lists it.
    fn mounts<'a>(&'a self, home: Option<&'a str>) -> Vec<Mount<'a>> {
        let mut mounts = vec![
            Mount { source: MountSource::Path(&self.code), target: Path::new(CODE_DIR), access: Access::ReadWrite },
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
            // the only thing of the machine this container may write to besides the workspace.
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
            workdir: Some(Path::new(CODE_DIR)),
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
        let program = desktop.command_line(CODE_DIR);
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
                workdir: Some(Path::new(CODE_DIR)),
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
        let program = desktop.command_line(CODE_DIR);
        let command: Vec<&str> = program.iter().map(String::as_str).collect();
        Some(engine.exec_without_terminal(&Exec { container: &self.name, command: &command }))
    }

    /// The command that shows `address` in the sign-in window, inside the container the window
    /// is open in: `localhost` there is where the application waits for the sign-in to come
    /// back. Started with the application's own flags, so it reaches the same compositor.
    ///
    /// `None` for a plan that opens no window.
    #[must_use]
    pub fn open_page(&self, engine: &Engine, address: &str) -> Option<EngineCommand> {
        let desktop = self.window?;
        let command = desktop::signin::page_command(desktop.flags, address);
        let command: Vec<&str> = command.iter().map(String::as_str).collect();
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
    /// Whether what failed is that the engine does not have the profile's image, which was asked
    /// before anything was made: the one failure the tab can put right by itself, by building it.
    pub image_missing: bool,
}

impl LaunchFailure {
    /// The failure of a command that could not even be started in a pseudo-terminal, which is
    /// how a tab learns that the engine binary went missing between two frames.
    #[must_use]
    pub fn spawn(command: &EngineCommand, error: &std::io::Error) -> Self {
        Self { command: written(command), output: error.to_string(), image_missing: false }
    }

    /// The failure of the base image build, in the words the person is shown.
    ///
    /// A build context that could not be written has no engine command behind it, so the
    /// command is left empty and only the machine's words are shown.
    pub(super) fn base(failure: &base::Failure) -> Self {
        match failure {
            base::Failure::Host(error) => {
                Self { command: String::new(), output: error.to_string(), image_missing: false }
            }
            base::Failure::Engine(error) => Self::from(error),
        }
    }

    /// The failure of this machine itself rather than of the engine, which has no command to show
    /// above its words.
    pub(super) fn from_host(error: &std::io::Error) -> Self {
        Self { command: String::new(), output: error.to_string(), image_missing: false }
    }

    /// The failure of an engine command, in the words the person is shown.
    pub(super) fn from(error: &EngineError) -> Self {
        match error {
            EngineError::NotRunnable { command, error } => {
                Self { command: written(command), output: error.to_string(), image_missing: false }
            }
            EngineError::Failed(failure) => {
                Self { command: written(&failure.command), output: failure.output.clone(), image_missing: false }
            }
            EngineError::Cancelled { command } => {
                Self { command: written(command), output: String::new(), image_missing: false }
            }
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
/// workspace does not ask for a login that was made already. A home that was there before the
/// container is left as it is: it is the workspace's own copy, and only the person may have it
/// rewritten.
///
/// A container takes its mounts and its network when it is made, so a stopped container made
/// from another plan than this one (its [`PLAN_LABEL`] says so) is made again: removed and
/// created anew, which keeps every named volume and so the home with the harness's login,
/// history and settings in it. What was only in the container itself, outside the home and the
/// workspace's folders, goes with it, as it does when the container is removed by hand. A running
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
        // A profile's image is QCode's to build, and only the person can say it may take the
        // minutes that takes; so an engine without it is asked before anything is made, and the
        // tab offers the build rather than failing on `create` with the engine's riddle about it.
        if plan.home.is_some()
            && let Err(error) = run::capture(&engine.image_exists(&plan.image))
        {
            let missing = matches!(&error, EngineError::Failed(failure)
                if known::recognise(&failure.output) == Some(Known::ImageMissing));
            return Err(LaunchFailure { image_missing: missing, ..LaunchFailure::from(&error) });
        }
        if plan.image == names::BASE_IMAGE {
            base::ensure(engine, &|| false, &mut |_| {}).map_err(|failure| LaunchFailure::base(&failure))?;
        }
        // An engine mounting a folder that is not there fails, or makes it as its own user; the
        // folder is made here, as the person.
        if let Some(bridge) = &plan.bridge {
            std::fs::create_dir_all(&bridge.folder).map_err(|error| LaunchFailure {
                command: String::new(),
                output: format!("{}: {error}", bridge.folder.display()),
                image_missing: false,
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
    // The folders the window mounts have to be there before the container starts: an engine
    // refuses to mount a path that does not exist. One is where the window writes the addresses
    // it wants opened, the other is where the bridge's socket and server live.
    for folder in [plan.browser.as_ref(), plan.bridge.as_ref().map(|bridge| &bridge.folder)].into_iter().flatten() {
        std::fs::create_dir_all(folder).map_err(|error| LaunchFailure::from_host(&error))?;
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
    // In the window's own container rather than the one that prepared it: that one is taken away
    // at once, and the map with it, while this one lives as long as the window.
    if plan.graphify {
        build_map(engine, &plan.name, &plan.code);
    }
    Ok(Window::Opened)
}

/// What was made ready for a window before it opened: the bridge's server registered in its
/// settings, and its instruction files in the workspace; each of them either done or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The bridge's registration.
    pub bridge: Result<(), Unregistered>,
    /// The instruction files, for a profile on QCode high.
    pub guidance: Result<(), Unguided>,
}

/// Makes `plan`'s window ready before it opens, from a container made for that and nothing else:
/// registers the bridge's server in the settings on its home volume, for the tab whose token is
/// `token`, and, for a profile on QCode high, brings its instruction files in the workspace up to
/// date the way [`guide`] does for a terminal's container.
///
/// A window's own container cannot be written to from the inside: it is created and started in one
/// go with the application as its only program, so by the time it is there the settings have been
/// read. This runs before the window opens instead, on the same volume and workspace and from the
/// same image, so what it writes is what the application finds. The container goes away again
/// whatever happened, and one left behind by an interrupted run is removed first, so it cannot
/// block the next.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
#[must_use]
pub fn prepare_window(engine: &Engine, plan: &ContainerPlan, user: HostUser, token: &str) -> Prepared {
    let mut prepared = Prepared { bridge: Ok(()), guidance: Ok(()) };
    let Some(home) = &plan.home else { return prepared };
    if plan.bridge.is_none() && plan.guidance.is_none() {
        return prepared;
    }
    let name = home.settings_container();
    let volume = home.volume();
    let mut mounts =
        vec![Mount { source: MountSource::Volume(&volume), target: Path::new(HOME_DIR), access: Access::ReadWrite }];
    if plan.guidance.is_some() {
        mounts.push(Mount {
            source: MountSource::Path(&plan.code),
            target: Path::new(CODE_DIR),
            access: Access::ReadWrite,
        });
    }
    let create = engine.create_container(&ContainerCreate {
        name: &name,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &plan.image,
        mounts: &mounts,
        network: Network::None,
        user,
        workdir: None,
        command: KEEP_ALIVE,
    });
    let remove = engine.remove_container(&name);
    let _ = run::capture(&remove);
    let up = run::capture(&create).and_then(|_| run::capture(&engine.start_container(&name)));
    match up {
        Ok(_) => {
            if let Some(bridge) = &plan.bridge {
                prepared.bridge = config::register(engine, &name, bridge.harness, token);
            }
            if let Some(harness) = plan.guidance {
                prepared.guidance = guide(engine, &name, &plan.code, harness, plan.graphify);
            }
        }
        Err(error) => {
            let words = config::words(&error);
            if plan.bridge.is_some() {
                prepared.bridge = Err(Unregistered::Engine(words.clone()));
            }
            if plan.guidance.is_some() {
                prepared.guidance = Err(Unguided::Graphify(words));
            }
        }
    }
    let _ = run::capture(&remove);
    prepared
}

/// Brings the instruction files of `harness` in the workspace up to date, from the running
/// container `container` whose workspace folder is `code` on this machine; graphify's part only
/// when `graphify` says it is in the image, since a profile that went without it has no graphify
/// to run.
///
/// graphify goes first and writes its own part itself — its section, its hooks, and for some
/// harnesses a skill in the home — by its own installer inside the container, in [`CODE_DIR`],
/// the folder the agent works in: measured, it needs no network and a second run writes nothing
/// new. QCode's section is written next, on this machine, into the same folder, and only when it
/// is not already there as it would be written. The two happen under [`guidance::lock`], so two
/// tabs coming up at once in one workspace cannot write one file over the other's.
///
/// Runs engine commands and writes the disk, so it belongs on a background thread.
///
/// # Errors
///
/// [`Unguided`] when graphify's installer or the engine refuses, or QCode's section cannot be
/// written. QCode's section is still written when graphify's failed, so the agent learns of the
/// other tabs either way.
pub fn guide(
    engine: &Engine,
    container: &str,
    code: &Path,
    harness: HarnessKind,
    graphify: bool,
) -> Result<(), Unguided> {
    let _writing = guidance::lock();
    let install =
        ["sh", "-c", "cd \"$1\" && exec graphify \"$2\" install", "sh", CODE_DIR, guidance::platform(harness)];
    let graphify = if graphify {
        run::capture(&engine.exec_without_terminal(&Exec { container, command: &install }))
            .map(|_| ())
            .map_err(|error| Unguided::Graphify(config::words(&error)))
    } else {
        Ok(())
    };
    let ours = guidance::write(code, harness).map(|_| ());
    graphify.and(ours)
}

/// graphify's map of the workspace's code, relative to the workspace folder: what graphify's
/// section and hooks send the agent to.
pub const MAP: &str = "graphify-out/graph.json";

/// Starts graphify building its map of the workspace's code in the running container `container`,
/// whose workspace folder is `code` on this machine, when there is no map there yet; does nothing
/// when there is one.
///
/// graphify's installer writes a section and hooks that tell the agent to ask the map before it
/// reads files, but it builds no map, so without this the agent is pointed at a file that is not
/// there. `graphify update .` builds it from the code alone: measured, it needs no network and no
/// model. It is left running on its own inside the container and not waited for, because a large
/// workspace takes minutes and the harness must not wait for it; its output goes nowhere, since
/// nobody is there to read it. Once the map is there, graphify's own hooks keep it current, so a
/// map that exists is never rebuilt here.
///
/// A map that fails to be built is not said on screen: graphify's section tells the agent how to
/// build it, and the agent works without one.
///
/// Runs an engine command, so it belongs on a background thread.
pub fn build_map(engine: &Engine, container: &str, code: &Path) {
    if code.join(MAP).exists() {
        return;
    }
    // `flock -n` so that two tabs of one profile coming up at once, which share its container,
    // start one build between them and not two writing the same files.
    let build = [
        "sh",
        "-c",
        "cd \"$1\" && nohup flock -n /tmp/qcode-map graphify update . </dev/null >/dev/null 2>&1 &",
        "sh",
        CODE_DIR,
    ];
    let _ = run::capture(&engine.exec_without_terminal(&Exec { container, command: &build }));
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

/// Shows `address` in the sign-in window inside the container of `plan`, and answers whether it
/// is up: `false` when the image has no sign-in window, which an image built before it existed
/// does not.
///
/// Runs an engine command and waits for it, which takes as long as starting a program in the
/// container and no longer: the window is left running on its own. So it belongs on a background
/// thread.
///
/// # Errors
///
/// The engine's own words when the container is not there any more or refuses.
pub fn open_page(engine: &Engine, plan: &ContainerPlan, address: &str) -> Result<bool, LaunchFailure> {
    let Some(command) = plan.open_page(engine, address) else { return Ok(false) };
    let said = run::capture(&command).map_err(|error| LaunchFailure::from(&error))?;
    Ok(!said.contains(desktop::signin::NO_BROWSER))
}

/// Whether the container of `plan` is running right now.
///
/// Runs an engine command, so it belongs on a background thread. An engine that cannot answer
/// says "not running", which is the answer that offers the person a way out.
#[must_use]
pub fn is_running(engine: &Engine, plan: &ContainerPlan) -> bool {
    run::capture(&engine.container_state(&plan.name)).is_ok_and(|word| ContainerState::parse(&word).is_running())
}

/// Every container the engine knows that belongs to `workspace`, in the engine's order.
///
/// Runs an engine command, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it cannot be asked.
pub fn workspace_containers(engine: &Engine, workspace: &str) -> Result<Vec<Container>, LaunchFailure> {
    let listing = run::capture(&engine.list_containers()).map_err(|error| LaunchFailure::from(&error))?;
    let prefix = format!("qcode-{workspace}-");
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
