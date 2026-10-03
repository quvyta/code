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
    Network, RunOnce,
    known::{self, Known},
    names,
    run::{self, EngineError},
};
use crate::profile::guidance::{self, Unguided};
use crate::profile::identity::{self, Home};
use crate::profile::{Desktop, Extra, HarnessKind, MountAccess, NetworkMode, Profile};
use crate::store::{WorkspaceId, WorkspacePaths};

use super::keep;

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
    /// Whether graphify is in the profile's image, so that its map is built in the workspace when
    /// the container comes up, and, for a profile on QCode high, its installer runs there. False
    /// for the base container and for a profile on base or one that went without graphify.
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
            guidance: profile.template.carries_high().then_some(profile.harness),
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
    /// [`crate::engine::RunWindow`] and in [`crate::desktop`]; the shape of the call is the trial's, measured.
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
        let once = RunOnce {
            image: &self.image,
            mounts: &mounts,
            network: self.network,
            user,
            workdir: Some(Path::new(CODE_DIR)),
            command: &command,
        };
        Some(desktop::run_command(engine, &self.name, once, display, seccomp))
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

    /// The command that ends, inside this container, every process the tab whose token is `token`
    /// started: its harness, the relay in front of it, and whatever those started in turn. Each of
    /// them carries the tab's token in its environment, and nothing else does.
    ///
    /// Closing a tab stops the engine's command that was attached to its terminal, but the engine
    /// leaves what that command started inside the container running: in the endurance trial
    /// (2026-09-24) a closed tab's Claude Code and relay were still running twenty-five minutes later,
    /// able to go on working and spending, and holding the relay's port so that the tab could not
    /// be opened again.
    #[must_use]
    pub fn end_tab(&self, engine: &Engine, token: &str) -> EngineCommand {
        let script = "for p in /proc/[0-9]*; do \
                      tr '\\000' '\\n' < \"$p/environ\" 2>/dev/null | grep -qxF \"$1=$2\" \
                      && kill -TERM \"${p#/proc/}\" 2>/dev/null; done; true";
        let command = ["sh", "-c", script, "sh", crate::bridge::TOKEN_VARIABLE, token];
        engine.exec_without_terminal(&Exec { container: &self.name, command: &command })
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
            EngineError::TimedOut { command, after } => {
                Self { command: written(command), output: run::timed_out(command, *after), image_missing: false }
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
/// A frozen container is woken instead, for the same reason and with more to lose: it holds every
/// tab of the profile, each with its own process and its own terminal, so a person coming back to
/// it finds the work exactly as it was. Making it again would end all of them at once, so an
/// engine that will not wake it is a failure the person is shown rather than a container QCode
/// throws away and makes anew.
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
    ensure_running_noting(engine, plan, user).map(|_| ())
}

/// [`ensure_running`], answering the administrator's command that could not be run again when the
/// profile's image was rebuilt under a workspace that installed things of its own: the workspace
/// then stays on the image it had, and the person is to be told which command it was.
///
/// Before a stopped container is made again, what its administrator changed in the system is
/// committed to the workspace's own image, and the container is made again from that one
/// (the `keep` module). Nothing of it reaches the profile or another workspace.
///
/// # Errors
///
/// As [`ensure_running`].
pub fn ensure_running_noting(
    engine: &Engine,
    plan: &ContainerPlan,
    user: HostUser,
) -> Result<Option<String>, LaunchFailure> {
    // A container QCode cannot ask after is one that is not there: both engines answer an
    // unknown name with an error rather than with a state.
    let state = run::capture(&engine.container_state(&plan.name)).map(|word| ContainerState::parse(&word)).ok();
    let exists = match state {
        Some(ContainerState::Running) => return Ok(None),
        // A frozen container is not a stopped one: it is up, with every tab in it still running
        // and everything they wrote still in memory. So it is woken and nothing else, the same
        // answer a running container gives. Making it again would end all of them at once, so an
        // engine that refuses to wake it is said to the person rather than acted on.
        Some(ContainerState::Paused) => {
            run::capture(&engine.unpause_container(&plan.name)).map_err(|error| LaunchFailure::from(&error))?;
            return Ok(None);
        }
        Some(_) => {
            let Stopped { current, replaced, before } = stopped(engine, plan, user);
            if !current {
                // What the administrator installed goes into the workspace's image before the
                // container that holds it goes.
                keep::keep(engine, plan).map_err(|error| LaunchFailure::from(&error))?;
                run::capture(&engine.remove_container(&plan.name)).map_err(|error| LaunchFailure::from(&error))?;
                // The image a rebuild left without a name goes with its last container; while
                // another container still holds it the engine refuses, and that one takes it.
                if let Some(old) = replaced {
                    let _ = run::capture(&engine.remove_unused_image(&old));
                }
                if keep::current(engine, plan) != before {
                    keep::let_go(engine, before.as_deref());
                }
            }
            current
        }
        None => false,
    };
    let mut behind = None;
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
        let made = match keep::choose(engine, plan, user) {
            keep::Made::Profile => plan.clone(),
            keep::Made::Own(image) => ContainerPlan { image, ..plan.clone() },
            keep::Made::Behind { image, failed } => {
                behind = Some(failed);
                ContainerPlan { image, ..plan.clone() }
            }
        };
        run::capture(&made.create(engine, user)).map_err(|error| LaunchFailure::from(&error))?;
        if let Some(home) = new_home {
            identity::first_fill(engine, home, user).map_err(|error| LaunchFailure::from(&error))?;
        }
    }
    run::capture(&engine.start_container(&plan.name)).map_err(|error| LaunchFailure::from(&error))?;
    name_the_user(engine, &plan.name, user);
    Ok(behind)
}

/// Gives the person a name inside the container `container` on the engine whose daemon runs it as
/// the ids QCode hands it; does nothing on the engine that maps the person into the container's own
/// user namespace, nor for a host that has no POSIX ids to map.
///
/// On docker the container is created with `--user uid:gid` and the image names nobody but the uid
/// it was built with, so a person whose uid is another one comes up as a number: `whoami` cannot
/// answer, `git commit` refuses to write an author, and Node's `os.userInfo()` throws on it. The
/// home directory and the person's own files are right, so this is the whole of what is missing.
/// Whether the uid needs a name at all is asked of the container's own password file rather than
/// of the image's build: the shell looks for the uid in its own field and writes nothing into a
/// container that already knows the person, which is every container of an image made for the uid
/// the base was built with.
///
/// An engine that will not write it is not said on screen and does not stop the tab: a name is
/// worth less than a tab that comes up, and the tab works either way.
fn name_the_user(engine: &Engine, container: &str, user: HostUser) {
    if !engine.needs_a_user_entry() {
        return;
    }
    let HostUser::Ids { uid, gid } = user else { return };
    let (uid, gid) = (uid.to_string(), gid.to_string());
    let command = ["sh", "-c", base::paths::NAME_THE_USER, "sh", &uid, &gid];
    let _ = run::capture(&engine.exec_as_root_without_terminal(&Exec { container, command: &command }));
}

/// Makes a stopped container of a profile anew when it is not the one its plan asks for now —
/// the profile's image was built again since, or the profile's plan changed — and starts it;
/// answers whether it did, and the administrator's command that could not be run again on a
/// rebuilt image as [`ensure_running_noting`] does.
///
/// The page of a new tab reads the conversations out of a profile's container and starts a
/// stopped one to do it, and a tab then finds it running and takes it as it is. Without this
/// the container made from the old image or the old plan would be started there and never
/// replaced: a rebuild, a changed network or what the administrator installed would reach no
/// workspace that had run the profile before. Nothing of the person's is lost: the conversations
/// are in the home volume, which the new one mounts, and what the administrator changed in the
/// system is kept first (the `keep` module).
///
/// A frozen container is woken, never made anew, and answered as renewed: it is up, and making a
/// new one beside it would end every tab in it.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
pub fn renew_stale(engine: &Engine, plan: &ContainerPlan, user: HostUser) -> (bool, Option<String>) {
    let state = run::capture(&engine.container_state(&plan.name)).map(|word| ContainerState::parse(&word)).ok();
    // A frozen container is up, so it is never made anew: every tab in it is still running and
    // everything they hold is still in memory, and a container made from a newer image or plan
    // would end all of them at once. It is not started either, which would be refused; it is
    // woken, so the conversation this page is about to read is read from the same container the
    // tabs are in. A frozen container whose plan has gone stale keeps it, exactly as a running
    // one does, until it is next stopped.
    if matches!(state, Some(ContainerState::Paused)) {
        return match run::capture(&engine.unpause_container(&plan.name)) {
            Ok(_) => (true, None),
            // Left frozen: the engine's own words are the caller's to show, and a container QCode
            // cannot wake is not one it may throw away either.
            Err(_) => (false, None),
        };
    }
    let stopped_now = matches!(state, Some(state) if state != ContainerState::Running);
    if !stopped_now || stopped(engine, plan, user).current {
        return (false, None);
    }
    match ensure_running_noting(engine, plan, user) {
        Ok(behind) => (true, behind),
        Err(_) => (false, None),
    }
}

/// What a stopped container of `plan` is, against what the plan asks for now.
struct Stopped {
    /// It is the container the plan asks for, and can be started as it is.
    current: bool,
    /// The image it was made from, when a rebuild has replaced that one since.
    replaced: Option<String>,
    /// The workspace's own image of the plan as it is before anything is made again.
    before: Option<String>,
}

fn stopped(engine: &Engine, plan: &ContainerPlan, user: HostUser) -> Stopped {
    // The plan as the stopped container should have been made: from the workspace's own image
    // when it has one made on the profile's image as it is.
    let (made, fresh) = as_made(engine, plan);
    let made_from = run::capture(&engine.container_label(&plan.name, PLAN_LABEL)).unwrap_or_default();
    // The workspace's own image is never an image a rebuild left behind, whatever the profile's
    // image under it has become: it is what the new container is made from.
    let before = keep::current(engine, plan);
    let replaced = replaced_image(engine, &made).filter(|old| before.as_deref() != Some(old.trim()));
    let current = fresh && made_from.trim() == made.digest(engine, user) && replaced.is_none();
    Stopped { current, replaced, before }
}

/// The plan as the container of `plan` is to be made: from the workspace's own image when it has
/// one made on the profile's image as it is now; and whether it has none or that one. A container
/// made from the workspace's image is not made from a replaced image, whatever the profile's
/// image under it has become.
fn as_made(engine: &Engine, plan: &ContainerPlan) -> (ContainerPlan, bool) {
    match keep::standing(engine, plan) {
        Some((own, true)) => (ContainerPlan { image: own, ..plan.clone() }, true),
        Some((_, false)) => (plan.clone(), false),
        None => (plan.clone(), true),
    }
}

/// The image a stopped container of a profile was made from, when the profile's image has been
/// built again since: the plan names the image, and a rebuild keeps the name while the image
/// under it changes, so the plan's digest alone would start the old container again and the
/// rebuild would never reach a workspace. `None` when the image is the same, and when either
/// answer cannot be had: a container is then kept as it was before QCode asked.
fn replaced_image(engine: &Engine, plan: &ContainerPlan) -> Option<String> {
    plan.home.as_ref()?;
    let made_from = run::capture(&engine.container_image(&plan.name)).ok()?;
    let now = run::capture(&engine.image_exists(&plan.image)).ok()?;
    let (made_from, now) = (made_from.trim(), now.trim());
    (!made_from.is_empty() && !now.is_empty() && made_from != now).then(|| made_from.to_owned())
}

/// What opening a window came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// A container was started and the window is on its way up.
    Opened,
    /// A container of that name was already running, so the tab took that window rather than
    /// opening a second one. This is what a QCode that crashed with a window open leaves behind,
    /// what a second tab of the same profile would otherwise duplicate, and what a frozen
    /// container is treated as once it has been woken.
    Adopted,
}

/// Opens the window of `plan` and says whether it was started or found already up.
///
/// A container of the same name that is not running is in nobody's way and is removed: it is the
/// remains of a window that closed while no QCode was watching, or of one a closing QCode stopped,
/// and the window is opened fresh. Nothing of the person's is in it — the settings, the history and
/// the login are all in the home volume, which the new container mounts.
///
/// A frozen container is not such a remainder: its window is still there, with the work in it, so
/// it is woken and its window taken rather than taken away. A window profile is never frozen by
/// QCode's own rules, so this is here for a container something else froze.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it cannot be started, or refuses to wake, remove or start the
/// container, or to run the new one; and the machine's when the seccomp profile cannot be written.
pub fn open_window(
    engine: &Engine,
    plan: &ContainerPlan,
    user: HostUser,
    display: &Display,
) -> Result<Window, LaunchFailure> {
    let state = run::capture(&engine.container_state(&plan.name)).map(|word| ContainerState::parse(&word)).ok();
    match state {
        Some(ContainerState::Running) => return Ok(Window::Adopted),
        Some(ContainerState::Paused) => {
            run::capture(&engine.unpause_container(&plan.name)).map_err(|error| LaunchFailure::from(&error))?;
            return Ok(Window::Adopted);
        }
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

/// Gives the home of `plan`'s window the profile's stored login, before the window opens, when
/// the home has no login of its own.
///
/// A window's container is made and started in one go with the application as its only program,
/// so this cannot wait for [`ensure_running`], which gives a terminal's home its login when the
/// home is first made. It runs every time a window opens instead: a workspace made before the
/// profile was signed in gets the login the next time its window opens, and one that already has
/// a login, its own or an earlier copy, keeps it untouched.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it cannot list its volumes or refuses the copy.
pub fn give_window_login(engine: &Engine, plan: &ContainerPlan, user: HostUser) -> Result<(), LaunchFailure> {
    let (Some(_), Some(home)) = (plan.window, &plan.home) else { return Ok(()) };
    identity::first_fill(engine, home, user).map_err(|error| LaunchFailure::from(&error))
}

/// What was made ready for a window before it opened: the agent's own answers written into its
/// home, the bridge's server registered in its settings, and its instruction files in the
/// workspace; each of them either done or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The agent's approvals and permission grants, for a window's home whatever else it holds.
    pub approvals: Result<(), String>,
    /// The bridge's registration.
    pub bridge: Result<(), Unregistered>,
    /// The instruction files, for a profile on QCode high.
    pub guidance: Result<(), Unguided>,
}

/// Makes `plan`'s window ready before it opens, from a container made for that and nothing else:
/// writes what the window's agent may do without asking into the home on its volume, registers the
/// bridge's server in the settings there for the tab whose token is `token`, and, for a profile on
/// QCode high, brings its instruction files in the workspace up to date the way [`guide`] does for
/// a terminal's container.
///
/// The agent's answers come first, since the application reads its database as it starts and a
/// window that asks about every file read, every command and every edit is the one thing a
/// container is not supposed to cost. They are written for every window with a home, under every
/// template and into a home that was signed in to by hand, and merged into what is there rather
/// than written over it, so the settings a person made in the window are kept.
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
    let mut prepared = Prepared { approvals: Ok(()), bridge: Ok(()), guidance: Ok(()) };
    let Some(home) = &plan.home else { return prepared };
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
            prepared.approvals = answer(engine, &desktop::login::approve_command(), &name);
            if let Some(bridge) = &plan.bridge {
                prepared.bridge = config::register(engine, &name, bridge.harness, token);
            }
            if let Some(harness) = plan.guidance {
                prepared.guidance = guide(engine, &name, &plan.code, harness, plan.graphify);
            }
        }
        Err(error) => {
            let words = config::words(&error);
            prepared.approvals = Err(words.clone());
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

/// Runs `command` in the container `container` and answers the engine's own words when it refuses.
fn answer(engine: &Engine, command: &[String], container: &str) -> Result<(), String> {
    let command: Vec<&str> = command.iter().map(String::as_str).collect();
    run::capture(&engine.exec_without_terminal(&Exec { container, command: &command }))
        .map(|_| ())
        .map_err(|error| config::words(&error))
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
/// graphify's section and hooks — in the home under QCode basic, in the workspace under QCode
/// high — tell the agent to ask the map before it reads files, but nothing builds the map, so
/// without this the agent is pointed at a file that is not there. The map folder is kept out of
/// git first ([`exclude_map`]). `graphify update .` builds it from the code alone: measured, it needs no network and no
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
    // Before the map is asked after, so a map an earlier QCode built is kept out of git too.
    let _ = exclude_map(code);
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

/// The line that keeps graphify's map folder out of git: anchored at the top of the repository, so
/// a folder of that name deeper in the person's code is left alone.
pub const MAP_EXCLUDED: &str = "/graphify-out/";

/// Keeps graphify's map out of git when the workspace folder `code` is a git repository, and says
/// whether a line had to be added: the line [`MAP_EXCLUDED`] in the repository's own
/// `info/exclude`, which lives inside `.git` and is never committed, rather than in a `.gitignore`
/// of the person's that would show up in their next commit. Measured with graphify 0.9.67 in a
/// fresh repository, `graphify update .` leaves `graphify-out/` as an untracked folder and writes
/// no ignore file of its own.
///
/// A `.git` that is a file, as in a worktree, names the folder git keeps it in; a worktree reads
/// the exclude file of the repository it was made from, which that folder's `commondir` names.
/// A folder that is no repository, or a line that is there already in any of its spellings, writes
/// nothing.
///
/// # Errors
///
/// The machine's words when the exclude file cannot be read or written. Nobody is told: the map
/// is then only an untracked folder, which is what git would show without QCode.
pub fn exclude_map(code: &Path) -> std::io::Result<bool> {
    let Some(git) = git_dir(code) else { return Ok(false) };
    let info = git.join("info");
    let file = info.join("exclude");
    let existing = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let spellings = ["graphify-out", "graphify-out/", "/graphify-out", MAP_EXCLUDED];
    if existing.lines().any(|line| spellings.contains(&line.trim())) {
        return Ok(false);
    }
    std::fs::create_dir_all(&info)?;
    let gap = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&file, format!("{existing}{gap}{MAP_EXCLUDED}\n"))?;
    Ok(true)
}

/// The folder git keeps the repository at `code` in, if `code` is one.
fn git_dir(code: &Path) -> Option<PathBuf> {
    let dot = code.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let own = code.join(text.trim().strip_prefix("gitdir:")?.trim());
    match std::fs::read_to_string(own.join("commondir")) {
        Ok(common) => Some(own.join(common.trim())),
        Err(_) => Some(own),
    }
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
/// A frozen container is woken first, because a stop is refused on one and the removal after it
/// would take a window that never got the grace to write its state out. What is closed here is
/// what the person asked to be closed; the wake only lets the closing happen the way it does for
/// every other window.
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
    let Ok(word) = run::capture(&engine.container_state(name)) else { return Ok(()) };
    if matches!(ContainerState::parse(&word), ContainerState::Paused) {
        let _ = run::capture(&engine.unpause_container(name));
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

/// Whether the container of `plan` is there right now, which is what a tab asks about the moment
/// its terminal ends.
///
/// A frozen container is there: nothing took it away, and the tabs in it are all still running.
/// Answering "no" would call the tab stopped and take its container off the list while the person
/// is away from the desk.
///
/// Runs an engine command, so it belongs on a background thread. An engine that cannot answer
/// says "not running", which is the answer that offers the person a way out.
#[must_use]
pub fn is_running(engine: &Engine, plan: &ContainerPlan) -> bool {
    run::capture(&engine.container_state(&plan.name))
        .is_ok_and(|word| matches!(ContainerState::parse(&word), ContainerState::Running | ContainerState::Paused))
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
/// A frozen container is woken first: a stop of a paused container is refused by the engine, so the
/// panel's Stop would say the engine's words and leave the container as it is.
///
/// Runs an engine command, so it belongs on a background thread.
///
/// # Errors
///
/// The engine's own words when it refuses.
pub fn stop(engine: &Engine, name: &str) -> Result<(), LaunchFailure> {
    wake(engine, name);
    run::capture(&engine.stop_container(name)).map(|_| ()).map_err(|error| LaunchFailure::from(&error))
}

/// Stops a container and starts it again.
///
/// Runs an engine command, so it belongs on a background thread. A container that was already
/// stopped is only started, so a restart never fails for having nothing to stop. A frozen one is
/// woken, for the same reason [`stop`] wakes it.
///
/// # Errors
///
/// The engine's own words when it refuses to start the container.
pub fn restart(engine: &Engine, name: &str) -> Result<(), LaunchFailure> {
    wake(engine, name);
    let _ = run::capture(&engine.stop_container(name));
    run::capture(&engine.start_container(name)).map(|_| ()).map_err(|error| LaunchFailure::from(&error))
}

/// Wakes `container` when the engine says it is frozen, and says nothing about whether it woke: a
/// container that is not frozen is what this is asked about most of the time, and one the engine
/// would not wake is the caller's own work to report or not.
///
/// Runs engine commands, so it belongs on a background thread.
fn wake(engine: &Engine, container: &str) {
    let state = run::capture(&engine.container_state(container)).ok().map(|word| ContainerState::parse(&word));
    if matches!(state, Some(ContainerState::Paused)) {
        let _ = run::capture(&engine.unpause_container(container));
    }
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

#[cfg(test)]
mod own_copy {
    //! Each workspace has its own copy of a profile's home, and nothing a workspace's container
    //! can reach is the profile's own: its definition file or its stored login.

    use super::*;
    use crate::base::paths::NAME_THE_USER;
    use crate::engine::EngineKind;
    use crate::profile::{AccountKind, SafeName, Template};

    fn paths(root: &Path) -> WorkspacePaths {
        WorkspacePaths {
            root: root.to_owned(),
            file: root.join("workspace.qcode"),
            code: root.join("Work"),
            assets: root.join("Assets"),
            harness: root.join("Containers").join("Harness"),
        }
    }

    fn profile() -> Profile {
        Profile {
            name: SafeName::parse("claude-sub").expect("a safe name"),
            harness: HarnessKind::ClaudeCode,
            template: Template::Recommended,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        }
    }

    /// Every word of the command that makes `plan`'s container, as the engine is given it.
    fn words(plan: &ContainerPlan, engine: &Engine) -> Vec<String> {
        let user = HostUser::Ids { uid: 1000, gid: 1000 };
        plan.create(engine, user).args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn quvyta_development_writes_the_workspaces_instructions_as_qcode_high_does() {
        // QCode high writes graphify's and QCode's sections into the workspace when a container
        // comes up; Quvyta development is QCode high and more, so it does too.
        let id = WorkspaceId::parse("firefly").expect("an id");
        let paths = paths(Path::new("/qcode-store/Workspaces/firefly"));
        for (template, guided) in [
            (Template::Recommended, None),
            (Template::High, Some(HarnessKind::ClaudeCode)),
            (Template::QuvytaDev, Some(HarnessKind::ClaudeCode)),
        ] {
            let plan = ContainerPlan::profile(&id, &paths, &Profile { template, ..profile() });
            assert_eq!(plan.guidance, guided, "{template:?}");
        }
    }

    #[test]
    fn two_workspaces_of_one_profile_get_homes_of_their_own_and_reach_nothing_of_the_profiles() {
        let store = Path::new("/qcode-store");
        let profile = profile();
        let a = ContainerPlan::profile(
            &WorkspaceId::parse("firefly").expect("an id"),
            &paths(&store.join("Workspaces").join("firefly")),
            &profile,
        );
        let b = ContainerPlan::profile(
            &WorkspaceId::parse("serenity").expect("an id"),
            &paths(&store.join("Workspaces").join("serenity")),
            &profile,
        );
        let (home_a, home_b) = (a.home.as_ref().expect("a home").volume(), b.home.as_ref().expect("a home").volume());
        assert_ne!(home_a, home_b, "each workspace has its own copy");
        assert_ne!(a.name, b.name);
        assert_eq!(a.image, b.image, "both start from the one image");
        let login = crate::engine::names::credential_volume(profile.name.as_str());
        let definitions = store.join("Profiles");
        for engine in [Engine::new(EngineKind::Podman, "/no/podman"), Engine::new(EngineKind::Docker, "/no/docker")] {
            for (plan, own, other) in [(&a, &home_a, &home_b), (&b, &home_b, &home_a)] {
                let words = words(plan, &engine);
                let joined = words.join(" ");
                assert!(joined.contains(&format!("{own}:{HOME_DIR}")), "{:?}: {joined}", engine.kind());
                assert!(!joined.contains(other.as_str()), "{:?}: the other workspace's home: {joined}", engine.kind());
                assert!(!joined.contains(&login), "{:?}: the stored login is never mounted: {joined}", engine.kind());
                assert!(
                    !joined.contains(&definitions.display().to_string()),
                    "{:?}: the definition files are never mounted: {joined}",
                    engine.kind()
                );
            }
            // The one command that reads the stored login copies it into the home, and reads it
            // only: the login's volume is mounted read-only.
            let rewrite = identity::rewrite(
                &engine,
                a.home.as_ref().expect("a home"),
                HostUser::Ids { uid: 1000, gid: 1000 },
                crate::desktop::login::Fill::Keep,
            );
            let create: Vec<String> =
                rewrite.create.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
            let mount = create
                .iter()
                .find(|word| word.starts_with(&format!("{login}:")))
                .unwrap_or_else(|| panic!("the courier reads the login: {create:?}"));
            assert!(mount.ends_with(":ro") || mount.contains(":ro,"), "{:?}: {mount}", engine.kind());
            let copy: Vec<String> = rewrite.copy.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
            assert!(copy.iter().any(|script| script.ends_with(&format!(" {HOME_DIR}/; fi"))), "{copy:?}");
        }
    }

    /// A stand-in engine that writes down every call and lists `volumes` when asked for them.
    fn recording(name: &str, volumes: &str) -> (Engine, PathBuf, PathBuf) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-plan-{name}-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a folder");
        let (binary, calls) = (folder.join("engine"), folder.join("calls"));
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\n[ \"$1 $2\" = 'volume ls' ] && {{ printf '{volumes}'; exit 0; }}\nexit 0\n",
            calls = calls.display(),
        );
        std::fs::write(&binary, script).expect("the stand-in engine");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        (Engine::new(EngineKind::Podman, &binary), calls, folder)
    }

    fn window_profile() -> Profile {
        Profile { name: SafeName::parse("anti").expect("safe"), harness: HarnessKind::AntigravityIde, ..profile() }
    }

    #[test]
    fn a_window_is_given_the_profiles_login_before_it_opens_and_never_over_one_of_its_own() {
        let (engine, calls, folder) = recording("window-login", "qcode-cred-anti\\nqcode-home-firefly-anti\\n");
        let id = WorkspaceId::parse("firefly").expect("an id");
        let plan = ContainerPlan::window(&id, &paths(&folder), &window_profile()).expect("a window");
        give_window_login(&engine, &plan, HostUser::Ids { uid: 1000, gid: 1000 }).expect("given");
        let written = std::fs::read_to_string(&calls).expect("calls");
        let create = written
            .lines()
            .find(|call| call.starts_with("create --name qcode-refresh-firefly-anti "))
            .unwrap_or_else(|| panic!("a courier carries the login: {written}"));
        assert!(create.contains("qcode-cred-anti:/qcode-credentials:ro"), "{create}");
        assert!(create.contains("qcode-home-firefly-anti:/home/qcode:rw"), "{create}");
        // Every opening, even of a home that was there before, and only where it has no login.
        assert!(written.contains("exec qcode-refresh-firefly-anti sh -c "), "{written}");
        // The program runs to many lines; the word after its last one is the kind of copy.
        assert!(written.contains("process.exit(2);\n keep\n"), "{written}");
        assert!(written.lines().any(|call| call == "rm --force qcode-refresh-firefly-anti"), "{written}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_window_of_a_profile_on_base_is_told_what_its_agent_may_do_before_it_opens() {
        // Nothing but the home: no bridge and no guidance to bring the container up for, and a
        // window's own container cannot be written to once the application has read its database.
        // So the container comes up anyway, and the answers are written into the home first.
        let (engine, calls, folder) = recording("window-answers", "qcode-home-firefly-anti\\n");
        let id = WorkspaceId::parse("firefly").expect("an id");
        let profile = Profile { template: Template::Base, ..window_profile() };
        let plan = ContainerPlan::window(&id, &paths(&folder), &profile).expect("a window");
        let prepared = prepare_window(&engine, &plan, HostUser::Ids { uid: 1000, gid: 1000 }, "tok");
        assert_eq!(prepared, Prepared { approvals: Ok(()), bridge: Ok(()), guidance: Ok(()) });
        let written = std::fs::read_to_string(&calls).expect("calls");
        // The program is many lines, so what the call is holds is read out of the whole of it.
        let database = format!("{HOME_DIR}/{}", crate::desktop::login::DATABASE);
        let answered = written
            .find(&format!(" approve {database}"))
            .unwrap_or_else(|| panic!("the answers are written: {written}"));
        let made = written.find("create --name qcode-firefly-anti.mcp ").expect("the container is made");
        assert!(made < answered, "made before it is answered: {written}");
        assert!(written.contains("qcode-home-firefly-anti:/home/qcode:rw"), "on the home volume: {written}");
        assert!(written.lines().any(|call| call == "rm --force qcode-firefly-anti.mcp"), "{written}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_window_of_a_profile_never_signed_in_and_a_terminal_are_given_nothing_here() {
        let (engine, calls, folder) = recording("window-none", "qcode-home-firefly-anti\\n");
        let id = WorkspaceId::parse("firefly").expect("an id");
        let window = ContainerPlan::window(&id, &paths(&folder), &window_profile()).expect("a window");
        give_window_login(&engine, &window, HostUser::Ids { uid: 1000, gid: 1000 }).expect("nothing to give");
        let terminal = ContainerPlan::profile(&id, &paths(&folder), &profile());
        give_window_login(&engine, &terminal, HostUser::Ids { uid: 1000, gid: 1000 }).expect("not a window");
        let written = std::fs::read_to_string(&calls).unwrap_or_default();
        assert_eq!(written.lines().collect::<Vec<_>>(), ["volume ls --format {{.Name}}"], "{written}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A stand-in engine that answers for a container frozen in place: its state is `paused`, the plan
    /// label it carries is the digest of `plan` — so the engine sees the very container the plan
    /// asks for, not an old one — its volumes are `volumes`, and it has no images at all, since a
    /// container made long ago was not made from an image this stand-in has. Every call is written
    /// down, and a call named in `refused` is refused in an engine's own words.
    ///
    /// A frozen container is the case that needs one: the engine has to say `paused` and QCode has
    /// to be watched through what it does next, on a machine with no container runtime at all.
    fn frozen_engine(folder: &Path, plan: &ContainerPlan, volumes: &str, refused: &str) -> (Engine, PathBuf) {
        let (binary, calls) = (folder.join("engine"), folder.join("calls"));
        let digest = plan.digest(&Engine::new(EngineKind::Podman, &binary), user());
        let refusing = refused
            .split(' ')
            .filter(|word| !word.is_empty())
            .map(|word| format!("  {word}) printf 'Error: the stand-in refuses {word}\\n' >&2; exit 125 ;;"))
            .collect::<Vec<String>>()
            .join("\n");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\ncase \"$1\" in\n{refusing}\nesac\ncase \"$1\" in\n  \
             image) printf 'Error: no such image\\n' >&2; exit 125 ;;\nesac\ncase \"$1 $2\" in\n  'volume ls') \
             printf '{volumes}';;\n  'container inspect') case \"$4\" in\n    *Labels*) printf '{digest}\\n';;\n    \
             *) printf 'paused\\n';;\n  esac;;\nesac\nexit 0\n",
            calls = calls.display(),
        );
        std::fs::write(&binary, script).expect("the stand-in engine");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        (Engine::new(EngineKind::Podman, &binary), calls)
    }

    /// A folder of this test's own for a workspace and the stand-in engine in it.
    fn scratch(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-plan-{name}-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a folder");
        folder
    }

    /// Every call the stand-in engine was given, in order.
    fn recorded(calls: &Path) -> Vec<String> {
        std::fs::read_to_string(calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn user() -> HostUser {
        HostUser::Ids { uid: 1000, gid: 1000 }
    }

    /// The plan of a profile in `folder`'s own workspace.
    fn profile_plan(folder: &Path) -> ContainerPlan {
        ContainerPlan::profile(&WorkspaceId::parse("firefly").expect("an id"), &paths(folder), &profile())
    }

    /// The window plan of a desktop profile in `folder`'s own workspace.
    fn window_plan(folder: &Path) -> ContainerPlan {
        ContainerPlan::window(&WorkspaceId::parse("firefly").expect("an id"), &paths(folder), &window_profile())
            .expect("a window")
    }

    /// A display of this test's own, so the window tests name one that need not be there.
    fn display(folder: &Path) -> crate::desktop::Display {
        crate::desktop::Display { socket: folder.join("wayland-0"), name: "wayland-0".to_owned(), device: None }
    }

    /// Every call QCode must never make about a container it is only waking.
    const NEVER: [&str; 5] = ["rm ", "stop ", "commit ", "create ", "start "];

    /// A frozen container is woken, and nothing about it is thrown away and made again. Every tab
    /// of the profile is still running in it, so a `rm`, a `stop` or a `commit` here would end a
    /// conversation the person was in the middle of.
    #[test]
    fn a_frozen_container_is_woken_and_never_made_again() {
        let folder = scratch("frozen");
        let plan = profile_plan(&folder);
        let (engine, calls) = frozen_engine(&folder, &plan, "", "");
        assert_eq!(ensure_running_noting(&engine, &plan, user()).expect("woken"), None, "nothing was made anew");
        let asked = recorded(&calls);
        assert!(asked.iter().any(|call| call == &format!("unpause {}", plan.name)), "{asked:?}");
        for never in NEVER {
            assert!(!asked.iter().any(|call| call.starts_with(never)), "not {never}{asked:?}");
        }
        // The same question twice: a container that has just been woken is running, so a tab asked
        // while its tabs are still thawing is answered the same way rather than woken again.
        ensure_running_noting(&engine, &plan, user()).expect("woken again");
        let twice = recorded(&calls);
        assert_eq!(twice.iter().filter(|call| call.starts_with("unpause ")).count(), 2, "woken each time: {twice:?}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// An engine that will not wake a frozen container is a failure the person is shown, and never
    /// a reason for QCode to make the container again: every tab in it would end at once.
    #[test]
    fn a_container_that_cannot_be_woken_says_so_instead_of_being_made_again() {
        let folder = scratch("frozen-refused");
        let plan = profile_plan(&folder);
        let (engine, calls) = frozen_engine(&folder, &plan, "", "unpause");
        let failure = ensure_running_noting(&engine, &plan, user()).expect_err("an engine that refuses is a failure");
        assert!(failure.output.contains("refuses unpause"), "{failure:?}");
        assert!(failure.command.contains("unpause"), "the command that failed is named: {failure:?}");
        let asked = recorded(&calls);
        for never in NEVER {
            assert!(!asked.iter().any(|call| call.starts_with(never)), "not {never}{asked:?}");
        }
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// The page of a new tab reads a profile's conversations. A frozen container holds them, and
    /// the tabs reading them, so it is woken rather than renewed: a container made anew from a
    /// newer image would end every tab in it.
    #[test]
    fn a_frozen_container_is_woken_for_the_conversations_rather_than_renewed() {
        let folder = scratch("frozen-renew");
        let plan = profile_plan(&folder);
        let (engine, calls) = frozen_engine(&folder, &plan, "qcode-home-firefly-claude-sub\n", "");
        let (renewed, behind) = renew_stale(&engine, &plan, user());
        assert!(renewed && behind.is_none(), "the container is left running, as one read into is");
        let asked = recorded(&calls);
        assert!(asked.iter().any(|call| call == &format!("unpause {}", plan.name)), "{asked:?}");
        for never in NEVER {
            assert!(!asked.iter().any(|call| call.starts_with(never)), "not {never}{asked:?}");
        }
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A window's container that is frozen still holds that window and everything open in it, so
    /// opening it wakes the container and takes that window rather than taking the container away
    /// and starting the application over.
    #[test]
    fn a_frozen_windows_container_is_woken_and_its_window_taken() {
        let folder = scratch("frozen-window");
        let plan = window_plan(&folder);
        let (engine, calls) = frozen_engine(&folder, &plan, "", "");
        let opened = open_window(&engine, &plan, user(), &display(&folder)).expect("the window is up");
        assert_eq!(opened, Window::Adopted, "the window that was there is the one that is opened");
        let asked = recorded(&calls);
        assert!(asked.iter().any(|call| call == &format!("unpause {}", plan.name)), "{asked:?}");
        for never in NEVER {
            assert!(!asked.iter().any(|call| call.starts_with(never)), "not {never}{asked:?}");
        }
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Closing a window gives its application the grace to write its state out, and a stop is
    /// refused on a frozen container; so it is woken first, and the closing that follows is the one
    /// the person asked for.
    #[test]
    fn a_frozen_windows_container_is_woken_before_it_is_closed() {
        let folder = scratch("frozen-close");
        let plan = window_plan(&folder);
        let (engine, calls) = frozen_engine(&folder, &plan, "", "");
        close_window(&engine, &plan.name).expect("closed");
        let asked = recorded(&calls);
        let woken = asked.iter().position(|call| call == &format!("unpause {}", plan.name)).expect("woken first");
        let stopped = asked.iter().position(|call| call.starts_with("stop ")).expect("then stopped");
        assert!(woken < stopped, "a stop is refused on a frozen container: {asked:?}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A tab whose terminal ends asks whether its container is still there. A frozen one is: it
    /// holds every tab of the profile, and telling the tab its container was taken away would mark
    /// it stopped while the person is away from the desk.
    #[test]
    fn a_frozen_container_is_still_there_when_a_tabs_terminal_ends() {
        let folder = scratch("frozen-there");
        let plan = profile_plan(&folder);
        let (engine, _calls) = frozen_engine(&folder, &plan, "", "");
        assert!(is_running(&engine, &plan), "a frozen container is up, and takes nothing away");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// The naming command as the engine is given it, all of it one line: `exec --user 0` and the
    /// container, then the script with the ids as its arguments.
    fn naming_call(container: &str, uid: u32, gid: u32) -> String {
        format!("exec --user 0 {container} sh -c {NAME_THE_USER} sh {uid} {gid}")
    }

    /// A stand-in engine of `kind` answering for a stopped container of `plan`: it is the very
    /// container the plan asks for as `as_user` — its plan label is the digest of that plan — so
    /// bringing it up is a `start` and then whatever QCode does next, which is where the naming
    /// belongs, and a second call finds the container already running and is answered by it.
    /// Every call is written down, and a call named in `refused` is refused in an engine's own
    /// words.
    fn stopped_engine(
        kind: EngineKind,
        folder: &Path,
        plan: &ContainerPlan,
        as_user: HostUser,
        refused: &str,
    ) -> (Engine, PathBuf) {
        let (binary, calls, up) = (folder.join("engine"), folder.join("calls"), folder.join("up"));
        let digest = plan.digest(&Engine::new(kind, &binary), as_user);
        let refusing = refused
            .split(' ')
            .filter(|word| !word.is_empty())
            .map(|word| format!("  {word}) printf 'Error: the stand-in refuses {word}\\n' >&2; exit 125 ;;"))
            .collect::<Vec<String>>()
            .join("\n");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\ncase \"$1\" in\n{refusing}\nesac\ncase \"$1\" in\n  \
             image) printf 'Error: no such image\\n' >&2; exit 125 ;;\n  start) : > '{up}' ;;\nesac\ncase \
             \"$1 $2\" in\n  'container inspect') case \"$4\" in\n    *Labels*) printf '{digest}\\n';;\n    \
             *) if [ -f '{up}' ]; then printf 'running\\n'; else printf 'exited\\n'; fi;;\n  esac;;\nesac\nexit 0\n",
            calls = calls.display(),
            up = up.display(),
        );
        std::fs::write(&binary, script).expect("the stand-in engine");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        (Engine::new(kind, &binary), calls)
    }

    /// Docker names no one: its daemon runs the container as the ids it is handed, and the image
    /// knows nobody but the uid it was built with. So a person whose uid is another one is given
    /// an entry of their own, once, as root, right after the container comes up. The home, the
    /// files and the mounts are already right; what had nothing to read was the name that
    /// `whoami`, `git commit` and Node's `os.userInfo()` look up.
    #[test]
    fn a_person_the_image_does_not_know_is_named_inside_the_container_on_docker() {
        let folder = scratch("named-docker");
        let plan = profile_plan(&folder);
        let someone = HostUser::Ids { uid: 1234, gid: 1234 };
        let (engine, calls) = stopped_engine(EngineKind::Docker, &folder, &plan, someone, "");
        ensure_running(&engine, &plan, someone).expect("the container comes up");
        let asked = recorded(&calls);
        let started = asked.iter().position(|call| call == &format!("start {}", plan.name)).expect("it is started");
        // Every word of it: as root, in the container, with the ids as arguments of the shell
        // rather than written into it.
        let naming = naming_call(&plan.name, 1234, 1234);
        let named = asked.iter().position(|call| call == &naming).unwrap_or_else(|| panic!("named: {asked:#?}"));
        assert!(started < named, "named once the container is up, not before: {asked:#?}");
        // And once per container: a container that is already up is not written into again, so
        // every later tab of it costs no further command.
        ensure_running(&engine, &plan, someone).expect("the running container is left as it is");
        let again = recorded(&calls);
        assert_eq!(again.iter().filter(|call| call.as_str() == naming).count(), 1, "{again:#?}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Podman writes that entry itself: `--userns=keep-id` maps the person into the container's
    /// user namespace, and podman puts the matching line in the password file as it starts the
    /// container. So there is nothing for QCode to do there, and it asks no root command of an
    /// engine that would only be second at its own work.
    #[test]
    fn podman_names_the_person_itself_and_qcode_runs_nothing() {
        let folder = scratch("named-podman");
        let plan = profile_plan(&folder);
        let someone = HostUser::Ids { uid: 1234, gid: 1234 };
        let (engine, calls) = stopped_engine(EngineKind::Podman, &folder, &plan, someone, "");
        ensure_running(&engine, &plan, someone).expect("the container comes up");
        let asked = recorded(&calls);
        assert!(asked.iter().any(|call| call == &format!("start {}", plan.name)), "{asked:#?}");
        assert!(!asked.iter().any(|call| call.contains("--user 0")), "no root command: {asked:#?}");
        assert!(!asked.iter().any(|call| call.contains(NAME_THE_USER)), "no script: {asked:#?}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A host with no POSIX ids of its own — Windows, where the engine's own machine owns that
    /// side of a mount — has no uid to name, so there is nothing to write and nothing to ask.
    #[test]
    fn a_host_with_no_ids_of_its_own_is_named_nowhere() {
        let folder = scratch("named-none");
        let plan = profile_plan(&folder);
        let nobody = HostUser::ImageDefault;
        let (engine, calls) = stopped_engine(EngineKind::Docker, &folder, &plan, nobody, "");
        ensure_running(&engine, &plan, nobody).expect("the container comes up");
        let asked = recorded(&calls);
        assert!(asked.iter().any(|call| call == &format!("start {}", plan.name)), "{asked:#?}");
        assert!(!asked.iter().any(|call| call.contains(NAME_THE_USER)), "{asked:#?}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// An engine that will not write the entry is not a reason to fail a tab. The person comes up
    /// as a number, which is what they came up as before, and a tab that does not open at all is
    /// worse than a tab whose `whoami` cannot answer. Nothing is said on screen either: there is
    /// nothing the person could do about it, and the common case of a uid the image knows shows
    /// them no sign of it at all.
    #[test]
    fn an_engine_that_will_not_name_the_person_still_brings_the_tab_up() {
        let folder = scratch("named-refused");
        let plan = profile_plan(&folder);
        let someone = HostUser::Ids { uid: 1234, gid: 1234 };
        let (engine, calls) = stopped_engine(EngineKind::Docker, &folder, &plan, someone, "exec");
        ensure_running(&engine, &plan, someone).expect("the container comes up");
        assert!(recorded(&calls).iter().any(|call| call.starts_with("exec --user 0")), "it was asked: {calls:?}");
        let _ = std::fs::remove_dir_all(&folder);
    }
}
