//! A profile's stored login, and the copy that carries it into a project.
//!
//! A login is made once, in the profile's own credentials volume. Every project that uses the
//! profile is given a copy in its own home volume, and from then on the copy lives its own life:
//! a harness that rewrites its login in one project touches no other. The copy is made when a
//! project's container for the profile is first made, and again whenever the person asks to
//! refresh the identity from the profile.
//!
//! The copying is the engine's work. A volume has no door of its own, so a container that mounts
//! both volumes is made, told to copy, and removed again. What is spelled out here are its
//! commands, so that a machine with no engine can read them back in tests.

use crate::base::paths::{HOME_DIR, KEEP_ALIVE};
use crate::engine::names;
use crate::engine::run::{EngineError, capture};
use crate::engine::{
    Access, ContainerCreate, ContainerState, Engine, EngineCommand, Exec, HostUser, Mount, MountSource, Network,
};
use crate::profile::SafeName;
use crate::workspace::ProjectId;

/// Where a profile's credentials volume is mounted in every container that reads or writes it:
/// the one a login is put into, and the one a copy is taken from.
pub const STORE_DIR: &str = "/qcode-credentials";

/// The volume to remove when the person signs a profile out. Projects keep the copies they were
/// given; signing out takes away what new projects would have been given.
#[must_use]
pub fn sign_out(profile: &SafeName) -> String {
    names::credential_volume(profile.as_str())
}

/// Whether `listing`, as `volume ls` prints it, names the volume that holds `profile`'s login.
///
/// The volume only ever comes into being when a login was captured into it, so its presence is
/// the answer to "is this profile signed in?".
#[must_use]
pub fn is_stored(listing: &str, profile: &SafeName) -> bool {
    listed(listing, &names::credential_volume(profile.as_str()))
}

/// Whether `listing` names `volume`.
fn listed(listing: &str, volume: &str) -> bool {
    listing.lines().any(|line| line.trim() == volume)
}

/// One project's copy of one profile's home: the volume the profile's login is written into,
/// and the container that lives on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    profile: SafeName,
    project: ProjectId,
}

impl Home {
    /// The home `project` keeps for `profile`.
    #[must_use]
    pub fn new(profile: SafeName, project: ProjectId) -> Self {
        Self { profile, project }
    }

    /// The profile whose login the home holds a copy of.
    #[must_use]
    pub fn profile(&self) -> &SafeName {
        &self.profile
    }

    /// The project the home belongs to.
    #[must_use]
    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    /// The volume itself, mounted at [`HOME_DIR`] in the project's container for the profile.
    #[must_use]
    pub fn volume(&self) -> String {
        names::home_volume(self.project.as_str(), self.profile.as_str())
    }

    /// The container the harness runs in, which is the one that must not be running while its
    /// login is rewritten under it.
    #[must_use]
    pub fn container(&self) -> String {
        names::profile_container(self.project.as_str(), self.profile.as_str())
    }

    /// The short-lived container that carries the login from one volume to the other.
    fn courier(&self) -> String {
        format!("qcode-refresh-{}-{}", self.project, self.profile)
    }
}

/// The commands that write a profile's stored login into one project's home volume, in the
/// order they run.
///
/// The courier container mounts the credentials volume read-only and the home volume writable,
/// reaches no network, and runs as the same user the harness container runs as, so the copies
/// belong to whoever will read them there. The copy is the whole content of the store: the
/// store holds nothing but the login, put there by the capture that made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    /// Makes the courier.
    pub create: EngineCommand,
    /// Starts it, because a copy is an `exec` and an `exec` needs a running container.
    pub start: EngineCommand,
    /// Copies the store into the home.
    pub copy: EngineCommand,
    /// Removes the courier, whether or not the copy succeeded.
    pub remove: EngineCommand,
}

/// Spells out the commands that give `home` its profile's stored login.
#[must_use]
pub fn rewrite(engine: &Engine, home: &Home, user: HostUser) -> Rewrite {
    let courier = home.courier();
    let store = names::credential_volume(home.profile.as_str());
    let volume = home.volume();
    let mounts = [
        Mount { source: MountSource::Volume(&store), target: STORE_DIR.as_ref(), access: Access::ReadOnly },
        Mount { source: MountSource::Volume(&volume), target: HOME_DIR.as_ref(), access: Access::ReadWrite },
    ];
    let create = engine.create_container(&ContainerCreate {
        name: &courier,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &names::profile_image(home.profile.as_str()),
        mounts: &mounts,
        network: Network::None,
        user,
        workdir: None,
        command: KEEP_ALIVE,
    });
    let script = format!("cp -R {STORE_DIR}/. {HOME_DIR}/");
    let copy = engine.exec_without_terminal(&Exec { container: &courier, command: &["sh", "-c", &script] });
    Rewrite { create, start: engine.start_container(&courier), copy, remove: engine.remove_container(&courier) }
}

/// Runs a [`Rewrite`] to the end, taking the courier away again whatever happened.
///
/// A courier left behind by a copy that was interrupted is removed first: it would otherwise
/// keep the next copy from being made under the same name.
fn run(rewrite: &Rewrite) -> Result<(), EngineError> {
    let _ = capture(&rewrite.remove);
    let copied =
        capture(&rewrite.create).and_then(|_| capture(&rewrite.start)).and_then(|_| capture(&rewrite.copy)).map(|_| ());
    let _ = capture(&rewrite.remove);
    copied
}

/// Whether `home`'s volume exists yet. The container's creation makes it, so this is asked
/// before that, and the answer is what decides whether the login is copied in afterwards.
///
/// # Errors
///
/// When the engine cannot list its volumes.
pub fn home_exists(engine: &Engine, home: &Home) -> Result<bool, EngineError> {
    Ok(listed(&capture(&engine.list_volumes())?, &home.volume()))
}

/// Gives a home that was just made its profile's stored login.
///
/// A profile with no stored login gives nothing, and that is not a failure: the harness asks
/// for its login the first time it runs, as it would on any machine. Nothing is made for it
/// here, because a credentials volume that comes into being empty would read as a login from
/// then on.
///
/// # Errors
///
/// When the engine cannot list its volumes or refuses the copy.
pub fn first_fill(engine: &Engine, home: &Home, user: HostUser) -> Result<(), EngineError> {
    if !is_stored(&capture(&engine.list_volumes())?, &home.profile) {
        return Ok(());
    }
    run(&rewrite(engine, home, user))
}

/// What refreshing a profile's login across its projects came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    /// The projects whose copy was rewritten.
    pub written: Vec<ProjectId>,
    /// The projects that were left alone because their container for the profile is running.
    pub running: Vec<ProjectId>,
}

/// Why a refresh did not happen at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshError {
    /// No login is stored for the profile, so there is nothing to give.
    NotStored,
    /// The engine would not do it. The text is its own words, or the command it stopped at when
    /// it said nothing.
    Engine(String),
}

impl From<EngineError> for RefreshError {
    fn from(error: EngineError) -> Self {
        Self::Engine(match error {
            EngineError::NotRunnable { error, .. } => error.to_string(),
            EngineError::Failed(failure) => failure.output,
            EngineError::Cancelled { command } => {
                format!("{} {}", command.program.display(), command.args.join(" ".as_ref()).to_string_lossy())
            }
        })
    }
}

/// Writes `profile`'s stored login into the home of every project in `projects` that is not
/// running the profile right now, and says which were written and which were left.
///
/// A project whose container for the profile is running is skipped rather than stopped. A
/// running container is a harness someone may be in the middle of using; stopping it would end
/// that work unasked, and rewriting the login under it would leave the harness holding one it
/// no longer has. Skipping loses nothing: the person stops the container from the project
/// screen and refreshes again, and the copy that was there stays whole in the meantime.
///
/// The first project the engine refuses ends the round; what was written before it stays
/// written.
///
/// # Errors
///
/// [`RefreshError::NotStored`] when the profile has no login, before anything is touched;
/// [`RefreshError::Engine`] when the engine cannot be asked or refuses a copy.
pub fn refresh_all(
    engine: &Engine,
    profile: &SafeName,
    projects: &[ProjectId],
    user: HostUser,
) -> Result<Refreshed, RefreshError> {
    if !is_stored(&capture(&engine.list_volumes())?, profile) {
        return Err(RefreshError::NotStored);
    }
    let mut done = Refreshed { written: Vec::new(), running: Vec::new() };
    for project in projects {
        let home = Home::new(profile.clone(), project.clone());
        // A container the engine cannot answer for is one that is not there, and one that is
        // not there is not running.
        let running = capture(&engine.container_state(&home.container()))
            .is_ok_and(|word| ContainerState::parse(&word).is_running());
        if running {
            done.running.push(project.clone());
            continue;
        }
        run(&rewrite(engine, &home, user))?;
        done.written.push(project.clone());
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineKind;

    fn args(command: &EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    fn home() -> Home {
        Home::new(
            SafeName::parse("claude-sub").expect("the name is safe"),
            ProjectId::parse("my-app").expect("the id is legal"),
        )
    }

    #[test]
    fn a_home_is_named_after_its_project_and_its_profile() {
        let home = home();
        assert_eq!(home.volume(), "qcode-home-my-app-claude-sub");
        assert_eq!(home.container(), "qcode-my-app-claude-sub");
        assert_eq!(home.courier(), "qcode-refresh-my-app-claude-sub");
        assert_eq!(home.profile().as_str(), "claude-sub");
        assert_eq!(home.project().as_str(), "my-app");
    }

    #[test]
    fn signing_out_names_the_volume_that_holds_the_login() {
        let profile = SafeName::from_display("claude-sub").expect("has letters");
        assert_eq!(sign_out(&profile), names::credential_volume(profile.as_str()));
    }

    #[test]
    fn a_login_is_stored_when_the_listing_names_its_volume() {
        let profile = SafeName::parse("claude-sub").expect("the name is safe");
        assert!(is_stored("qcode-cred-codex\nqcode-cred-claude-sub\n", &profile));
        assert!(!is_stored("qcode-cred-claude-sub-old\nqcode-home-p-claude-sub\n", &profile));
        assert!(!is_stored("", &profile));
    }

    #[test]
    fn the_courier_mounts_the_store_read_only_and_the_home_writable_off_the_network() {
        let engine = Engine::new(EngineKind::Podman, "/usr/bin/podman");
        let rewrite = rewrite(&engine, &home(), HostUser::Ids { uid: 1000, gid: 1000 });
        assert_eq!(
            args(&rewrite.create),
            [
                "create",
                "--name",
                "qcode-refresh-my-app-claude-sub",
                "--hostname",
                names::HOSTNAME,
                "--userns=keep-id",
                "--network=none",
                "--volume",
                "qcode-cred-claude-sub:/qcode-credentials:ro,z",
                "--volume",
                "qcode-home-my-app-claude-sub:/home/qcode:rw,z",
                "qcode/profile/claude-sub",
                KEEP_ALIVE[0],
                KEEP_ALIVE[1],
                KEEP_ALIVE[2],
            ]
        );
        assert_eq!(args(&rewrite.start), ["start", "qcode-refresh-my-app-claude-sub"]);
        assert_eq!(
            args(&rewrite.copy),
            ["exec", "qcode-refresh-my-app-claude-sub", "sh", "-c", "cp -R /qcode-credentials/. /home/qcode/"]
        );
        assert_eq!(args(&rewrite.remove), ["rm", "--force", "qcode-refresh-my-app-claude-sub"]);
    }

    #[test]
    fn docker_spells_the_same_courier_with_its_own_user_flag() {
        let engine = Engine::new(EngineKind::Docker, "/usr/bin/docker");
        let rewrite = rewrite(&engine, &home(), HostUser::Ids { uid: 1000, gid: 100 });
        let create = args(&rewrite.create);
        assert_eq!(
            &create[..7],
            [
                "create",
                "--name",
                "qcode-refresh-my-app-claude-sub",
                "--hostname",
                names::HOSTNAME,
                "--user",
                "1000:100"
            ]
        );
        assert!(create.contains(&"qcode-cred-claude-sub:/qcode-credentials:ro".to_owned()), "{create:?}");
        assert!(create.contains(&"qcode-home-my-app-claude-sub:/home/qcode:rw".to_owned()), "{create:?}");
        assert_eq!(rewrite.copy.program, std::path::Path::new("/usr/bin/docker"));
    }

    #[test]
    fn the_copy_runs_without_a_terminal_so_docker_takes_it_from_a_pipe() {
        let engine = Engine::new(EngineKind::Docker, "/usr/bin/docker");
        let rewrite = rewrite(&engine, &home(), HostUser::ImageDefault);
        let copy = args(&rewrite.copy);
        assert!(!copy.contains(&"--tty".to_owned()), "{copy:?}");
        assert!(!copy.contains(&"--interactive".to_owned()), "{copy:?}");
    }

    #[test]
    fn an_engine_that_is_not_there_answers_with_the_machines_words() {
        // A binary that does not exist: the round stops at the volume listing, before any
        // project is touched, and the words are the operating system's.
        let engine = Engine::new(EngineKind::Podman, "/qcode/no/such/engine");
        let profile = SafeName::parse("claude-sub").expect("the name is safe");
        let outcome = refresh_all(&engine, &profile, &[ProjectId::parse("p").expect("legal")], HostUser::ImageDefault);
        match outcome {
            Err(RefreshError::Engine(words)) => assert!(!words.is_empty()),
            other => panic!("expected the engine's failure, got {other:?}"),
        }
        assert!(matches!(first_fill(&engine, &home(), HostUser::ImageDefault), Err(EngineError::NotRunnable { .. })));
        assert!(matches!(home_exists(&engine, &home()), Err(EngineError::NotRunnable { .. })));
    }
}
