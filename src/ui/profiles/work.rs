//! The work the profiles screen asks of the disk and of the engine.
//!
//! Everything here blocks, so everything here runs on a task thread and talks back in messages.
//! Nothing decides what the person is told: a failure carries what went wrong and, where the
//! engine spoke, the engine's own words, and the screen puts the sentence around it.

use std::path::{Path, PathBuf};

use qframe::widgets::TerminalSession;

use crate::base;
use crate::engine::names;
use crate::engine::run::{EngineError, build_image, capture, stream};
use crate::engine::{Access, ContainerCreate, CopyIn, Engine, Exec, HostUser, ImageBuild, Mount, MountSource, Network};
use crate::profile::identity::{self, STORE_DIR};
use crate::profile::{Profile, SafeName};

use super::recipe;
use super::status::{Readiness, Revision, Status};

/// How long a container that only waits to be `exec`'d into stays up: a day, which outlives any
/// login and still lets a container forgotten after a crash disappear by itself.
const IDLE: &str = "86400";

/// Why a piece of work did not do what was asked. The words shown to the person come from the
/// language files; what is carried here is either the engine's own output or the operating
/// system's message, both of which are shown as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The engine ran and refused. The text is everything it printed.
    Refused(String),
    /// The engine could not be started at all.
    Unreachable(String),
    /// The person stopped it.
    Cancelled,
    /// The machine would not do it: a directory that cannot be made, a file that cannot be
    /// written.
    Machine(String),
    /// The container was `exec`'d into and the login was not there afterwards.
    NoLogin,
}

impl From<EngineError> for Problem {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::NotRunnable { error, .. } => Self::Unreachable(error.to_string()),
            EngineError::Failed(failure) => Self::Refused(failure.output),
            EngineError::Cancelled { .. } => Self::Cancelled,
        }
    }
}

impl From<base::Failure> for Problem {
    fn from(failure: base::Failure) -> Self {
        match failure {
            base::Failure::Host(error) => Self::Machine(error.to_string()),
            base::Failure::Engine(error) => error.into(),
        }
    }
}

impl Problem {
    /// The engine's or the machine's own words, when there are any to show.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        match self {
            Self::Refused(text) | Self::Unreachable(text) | Self::Machine(text) => Some(text),
            Self::Cancelled | Self::NoLogin => None,
        }
    }
}

/// Asks the engine about every profile: whether its image is built and from which recipe, and
/// whether a login is kept for it.
#[must_use]
pub fn probe(engine: &Engine, profiles: &[Profile]) -> Vec<Status> {
    let volumes = capture(&engine.list_volumes()).ok();
    profiles
        .iter()
        .map(|profile| {
            // The label is asked for rather than the image's identity: the engine answers only
            // when the image is there, so the one question says both whether it is and what
            // it was built from.
            let asked = capture(&engine.image_label(&profile.image(), recipe::REVISION_LABEL));
            let (image, revision) = match asked {
                Ok(label) if label.trim() == recipe::image(profile).revision() => {
                    (Readiness::Present, Revision::Current)
                }
                Ok(_) => (Readiness::Present, Revision::Earlier),
                Err(_) => (Readiness::Missing, Revision::Unknown),
            };
            let identity = match &volumes {
                Some(listing) => Readiness::of(identity::is_stored(listing, &profile.name)),
                None => Readiness::Unknown,
            };
            Status { name: profile.name.clone(), image, revision, identity }
        })
        .collect()
}

/// Builds a profile's image, handing every line of the build to `line` and stopping when
/// `cancel` says so. A build that is stopped or fails leaves no image behind.
///
/// # Errors
///
/// When the context cannot be written, or the engine refuses, fails or is stopped.
pub fn build(
    engine: &Engine,
    profile: &Profile,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    build_from(engine, &profile.image(), &recipe::image(profile), Fresh::No, cancel, line)
}

/// Whether a build may reuse the layers an earlier one left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fresh {
    /// It may: the image is not there, and whatever an earlier build left is as good as new.
    No,
    /// It may not, and a build that does not finish leaves the image that was there before.
    Rebuild,
}

/// Writes the build context of `recipe` and builds `image` from it.
fn build_from(
    engine: &Engine,
    image: &str,
    recipe: &recipe::Recipe,
    fresh: Fresh,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    // The image's name as a folder name: `qcode/profile/x` has slashes a folder cannot.
    let context = scratch(&format!("build-{}", image.replace('/', "-")))?;
    let containerfile = context.join("Containerfile");
    let written = std::fs::write(&containerfile, recipe.labelled()).and_then(|()| {
        for (path, contents) in &recipe.files {
            let target = context.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, contents)?;
        }
        Ok(())
    });
    let result = match written {
        Ok(()) => {
            let request = ImageBuild { image, containerfile: &containerfile, context: &context };
            match fresh {
                Fresh::No => build_image(engine, &request, cancel, line).map_err(Problem::from),
                // Not `build_image`, whose rule is to take the name away from anything but a
                // finished build: here the name is on the image the person has been working in,
                // and a rebuild that fails or is stopped must leave them that one. Measured on
                // both engines: a build that fails at a step, or is killed halfway, never moves
                // the name, which only a finished build does.
                Fresh::Rebuild => stream(&engine.rebuild_image(&request), cancel, line).map_err(Problem::from),
            }
        }
        Err(error) => Err(Problem::Machine(error.to_string())),
    };
    let _ = std::fs::remove_dir_all(&context);
    result
}

/// Builds a profile's image the whole way: its system's base image first when the engine does
/// not have that one either, then the profile's own on top of it.
///
/// It is the build the profile wizard runs, and the one a workspace runs when it opens a tab of
/// a profile whose image this engine lacks, so the two can never build a profile differently.
/// `base_started` is told once, at the first line of a base image build, because a base that is
/// already there builds silently and a log that suddenly shows another image needs saying why.
///
/// # Errors
///
/// When a build context cannot be written, or the engine refuses, fails or is stopped.
pub fn build_whole(
    engine: &Engine,
    profile: &Profile,
    cancel: &dyn Fn() -> bool,
    base_started: &mut dyn FnMut(),
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    ensure_base(engine, profile, cancel, base_started, line)?;
    build(engine, profile, cancel, line)
}

/// Builds the image of a profile that has one again, from the recipe this QCode writes for it
/// and from its first step, so that what the recipe installs is fetched again too.
///
/// Nothing of the person's is in an image: the homes of the workspaces, the login and the
/// conversations are volumes, and a container made from the new image mounts the same ones. A
/// container still running from the old image goes on running from it; the next time one of the
/// profile's containers is made the new image is used, and the old one is removed once no
/// container is made from it. A rebuild that fails or is stopped leaves the old image as it was.
///
/// # Errors
///
/// When a build context cannot be written, or the engine refuses, fails or is stopped.
pub fn rebuild(
    engine: &Engine,
    profile: &Profile,
    cancel: &dyn Fn() -> bool,
    base_started: &mut dyn FnMut(),
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    ensure_base(engine, profile, cancel, base_started, line)?;
    rebuild_from(engine, &profile.image(), &recipe::image(profile), cancel, line)
}

/// Builds `image` again from `recipe`, from its first step, and lets the image it replaces go
/// when nothing is made from it; the whole of [`rebuild`] but the base image.
///
/// # Errors
///
/// When a build context cannot be written, or the engine refuses, fails or is stopped.
pub(crate) fn rebuild_from(
    engine: &Engine,
    image: &str,
    recipe: &recipe::Recipe,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    let before = capture(&engine.image_exists(image)).ok();
    build_from(engine, image, recipe, Fresh::Rebuild, cancel, line)?;
    // The old image keeps its layers on the disk under no name at all. A running container still
    // holds it and the engine then refuses, which is the answer wanted: the container goes on,
    // and the next workspace container made for the profile removes the image after it.
    let after = capture(&engine.image_exists(image)).ok();
    if let Some(before) = before.as_deref().map(str::trim)
        && after.as_deref().map(str::trim) != Some(before)
    {
        let _ = capture(&engine.remove_unused_image(before));
    }
    Ok(())
}

/// Makes sure the base image of the profile's system is there and current, telling
/// `base_started` once when it has to be built.
fn ensure_base(
    engine: &Engine,
    profile: &Profile,
    cancel: &dyn Fn() -> bool,
    base_started: &mut dyn FnMut(),
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    let mut announced = false;
    base::ensure_os(engine, profile.os, cancel, &mut |text| {
        if !announced {
            announced = true;
            base_started();
        }
        line(text);
    })?;
    Ok(())
}

/// A container opened for a login, and the directory it drops the login into.
#[derive(Debug)]
pub struct LoginContainer {
    /// The container the harness signs in from.
    pub name: String,
    /// The directory on this machine the container copies the login into.
    pub capture: PathBuf,
}

/// Creates and starts the container a login happens in.
///
/// The container always reaches the network, whatever the profile says: a login that cannot
/// reach the account it signs in to is not a login. It sees one directory of this machine, which
/// is where the login leaves the container afterwards.
///
/// # Errors
///
/// When the directory cannot be made or the engine refuses.
pub fn open_login(engine: &Engine, profile: &Profile) -> Result<LoginContainer, Problem> {
    let name = login_container(&profile.name);
    // A container left behind by a login that was interrupted must not be signed in to again.
    let _ = capture(&engine.remove_container(&name));
    let folder = scratch(&format!("login-{}", profile.name))?;
    let user = HostUser::current().map_err(|error| Problem::Machine(error.to_string()))?;
    let mounts = [Mount {
        source: MountSource::Path(&folder),
        target: Path::new(recipe::CAPTURE_DIR),
        access: Access::ReadWrite,
    }];
    let request = ContainerCreate {
        name: &name,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &profile.image(),
        mounts: &mounts,
        network: Network::Full,
        user,
        workdir: None,
        command: &["sleep", IDLE],
    };
    let started = capture(&engine.create_container(&request)).and_then(|_| capture(&engine.start_container(&name)));
    match started {
        Ok(_) => Ok(LoginContainer { name, capture: folder }),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&folder);
            Err(error.into())
        }
    }
}

/// Starts the harness in `container` on a pseudo-terminal, so the person signs in with the
/// harness's own flow.
///
/// The harness is started plainly, without the arguments that let it work unattended: this run
/// exists to sign in, not to do work.
///
/// # Errors
///
/// When no pseudo-terminal can be opened or the engine cannot be started.
pub fn start_harness(engine: &Engine, profile: &Profile, container: &str) -> Result<TerminalSession, Problem> {
    let command = [profile.harness.record().command];
    let request = engine.exec(&Exec { container, command: &command });
    TerminalSession::spawn(request.program.as_os_str(), &request.args, &std::env::temp_dir())
        .map_err(|error| Problem::Machine(error.to_string()))
}

/// Takes the login the person just made out of `container` and puts it in the profile's
/// credentials volume, and answers how many files it stored.
///
/// Nothing is taken on trust. The copy runs inside the container and fails when the login file
/// is not there, so a login that did not happen cannot be mistaken for one that did; the volume
/// is only created once there is something to put in it, which is why the volume's existence is
/// what the list reads as "signed in".
///
/// # Errors
///
/// When the login is not there, or the engine refuses.
pub fn store_login(engine: &Engine, profile: &Profile, container: &LoginContainer) -> Result<usize, Problem> {
    let script = recipe::capture_script(profile);
    let command = ["sh", "-c", script.as_str()];
    // The script ends with a failure when the login file is not there, so a refusal from the
    // engine here is the script's own answer rather than the engine's.
    match capture(&engine.exec_without_terminal(&Exec { container: &container.name, command: &command })) {
        Ok(_) => {}
        Err(EngineError::Failed(_)) => return Err(Problem::NoLogin),
        Err(error) => return Err(error.into()),
    }
    let stored = count_files(&container.capture);
    if stored == 0 {
        return Err(Problem::NoLogin);
    }

    let holder = credentials_container(&profile.name);
    let _ = capture(&engine.remove_container(&holder));
    let volume = names::credential_volume(profile.name.as_str());
    let user = HostUser::current().map_err(|error| Problem::Machine(error.to_string()))?;
    let mounts =
        [Mount { source: MountSource::Volume(&volume), target: Path::new(STORE_DIR), access: Access::ReadWrite }];
    let request = ContainerCreate {
        name: &holder,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &profile.image(),
        mounts: &mounts,
        network: Network::None,
        user,
        workdir: None,
        command: &["sleep", IDLE],
    };
    let result = capture(&engine.create_container(&request))
        .and_then(|_| capture(&engine.start_container(&holder)))
        .and_then(|_| {
            capture(&engine.copy_in(&CopyIn {
                host: &container.capture.join("."),
                container: &holder,
                target: Path::new(STORE_DIR),
            }))
        });
    let _ = capture(&engine.remove_container(&holder));
    match result {
        Ok(_) => Ok(stored),
        Err(error) => {
            // Nothing reached the volume, so nothing may be left claiming a login is there.
            let _ = capture(&engine.remove_volume(&volume));
            Err(error.into())
        }
    }
}

/// Removes the container a login was made in and the directory it used, whether the login
/// finished, failed or was interrupted. Anything that cannot be removed is left rather than
/// reported: the person is done with it either way, and the next login starts by clearing it.
pub fn close_login(engine: &Engine, container: &LoginContainer) {
    let _ = capture(&engine.remove_container(&container.name));
    let _ = std::fs::remove_dir_all(&container.capture);
}

/// Removes a profile's credentials volume, which is what signing out means.
///
/// # Errors
///
/// When the engine refuses, for instance because a container still holds the volume.
pub fn sign_out(engine: &Engine, profile: &SafeName) -> Result<(), Problem> {
    let volume = identity::sign_out(profile);
    match capture(&engine.remove_volume(&volume)) {
        Ok(_) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// How many files ended up in a captured login.
fn count_files(folder: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() { count_files(&path) } else { 1 }
        })
        .sum()
}

/// A directory of this machine for one piece of work, emptied when the work is over.
fn scratch(purpose: &str) -> Result<PathBuf, Problem> {
    let folder = std::env::temp_dir().join(format!("qcode-{purpose}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).map_err(|error| Problem::Machine(error.to_string()))?;
    Ok(folder)
}

/// The container a profile's login is made in.
fn login_container(profile: &SafeName) -> String {
    format!("qcode-login-{profile}")
}

/// The container that holds a profile's credentials volume open while a login is put into it.
fn credentials_container(profile: &SafeName) -> String {
    format!("qcode-store-{profile}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::names::BASE_IMAGE;

    #[test]
    fn a_problem_carries_the_engines_own_words() {
        let refused = Problem::Refused("Error: no such image qcode/base".to_owned());
        assert_eq!(refused.output(), Some("Error: no such image qcode/base"));
        assert_eq!(Problem::NoLogin.output(), None);
        assert_eq!(Problem::Cancelled.output(), None);
    }

    #[test]
    fn the_base_image_is_what_a_profile_image_is_built_on() {
        // The name is a contract with the engine layer; a profile image cannot exist without it.
        assert_eq!(BASE_IMAGE, "qcode/base");
    }
}
