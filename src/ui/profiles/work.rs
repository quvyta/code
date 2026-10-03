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
use crate::engine::scratch::Scratch;
use crate::engine::{Access, ContainerCreate, CopyIn, Engine, Exec, HostUser, ImageBuild, Mount, MountSource, Network};
use crate::profile::identity::{self, STORE_DIR};
use crate::profile::own::{self, Own};
use crate::profile::{Profile, SafeName, account};

use super::recipe;
use super::shell;
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
            EngineError::TimedOut { command, after } => {
                Self::Unreachable(crate::engine::run::timed_out(&command, after))
            }
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
    build_from(engine, &profile.image(), &recipe::image(profile), None, Fresh::No, cancel, line)
}

/// What a person added to a profile in its shell, as a build puts it back after the recipe.
#[derive(Debug, Clone, Default)]
pub struct Additions {
    /// The commands and the home's paths.
    pub own: Own,
    /// The archive of the home's files, when there is one.
    pub archive: Option<PathBuf>,
}

impl Additions {
    /// What was added to `profile`, read from the store's `Profiles/` folder at `profiles`. A
    /// file that cannot be read adds nothing, and the rebuild's log is not the place it is
    /// reported: the profile's list says so.
    #[must_use]
    pub fn of(profiles: &Path, profile: &SafeName) -> Self {
        Self { own: Own::load(profiles, profile).0, archive: shell::archive_of(profiles, profile) }
    }
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
    additions: Option<&Additions>,
    fresh: Fresh,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    // The image's name as a folder name: `qcode/profile/x` has slashes a folder cannot.
    let folder = Scratch::new(&format!("build-{}", image.replace('/', "-")))
        .map_err(|error| Problem::Machine(error.to_string()))?;
    let context = folder.path();
    let containerfile = context.join("Containerfile");
    // The additions come after the label, so a profile's image says it was built from its
    // recipe whatever was added to it, as an image the shell committed does.
    let tail = additions.map(|added| added.own.containerfile_tail(added.archive.is_some())).unwrap_or_default();
    let written = std::fs::write(&containerfile, recipe.labelled() + &tail).and_then(|()| {
        if let Some(archive) = additions.and_then(|added| added.archive.as_ref()) {
            std::fs::copy(archive, context.join(own::HOME_ARCHIVE))?;
        }
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
            let request = ImageBuild { image, containerfile: &containerfile, context };
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
    drop(folder);
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

/// [`build_whole`] of a profile of the store whose `Profiles/` folder is `profiles`, with what
/// the person added to it in its shell put back after the recipe: an engine that never had the
/// image, or lost it, gets the one the person made.
///
/// # Errors
///
/// As [`build_whole`].
pub fn build_whole_in(
    engine: &Engine,
    profile: &Profile,
    profiles: &Path,
    cancel: &dyn Fn() -> bool,
    base_started: &mut dyn FnMut(),
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    ensure_base(engine, profile, cancel, base_started, line)?;
    let additions = Additions::of(profiles, &profile.name);
    build_from(engine, &profile.image(), &recipe::image(profile), Some(&additions), Fresh::No, cancel, line)
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
/// After the recipe come what the person added in the profile's shell, read from the store's
/// `Profiles/` folder at `profiles`: every command they ran there as the administrator, then the
/// home's files they changed. A command that fails fails the rebuild, the engine's log names it,
/// and the image that was there stays.
///
/// # Errors
///
/// When a build context cannot be written, or the engine refuses, fails or is stopped.
pub fn rebuild(
    engine: &Engine,
    profile: &Profile,
    profiles: &Path,
    cancel: &dyn Fn() -> bool,
    base_started: &mut dyn FnMut(),
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    ensure_base(engine, profile, cancel, base_started, line)?;
    let additions = Additions::of(profiles, &profile.name);
    rebuild_from(engine, &profile.image(), &recipe::image(profile), Some(&additions), cancel, line)
}

/// Builds `image` again from `recipe`, from its first step, lets the image it replaces go when
/// nothing is made from it, and takes away the images of ours that earlier rebuilds left behind;
/// the whole of [`rebuild`] but the base image.
///
/// # Errors
///
/// When a build context cannot be written, or the engine refuses, fails or is stopped.
pub(crate) fn rebuild_from(
    engine: &Engine,
    image: &str,
    recipe: &recipe::Recipe,
    additions: Option<&Additions>,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<(), Problem> {
    let before = capture(&engine.image_exists(image)).ok();
    build_from(engine, image, recipe, additions, Fresh::Rebuild, cancel, line)?;
    // The old image keeps its layers on the disk under no name at all. A running container still
    // holds it and the engine then refuses, which is the answer wanted: the container goes on,
    // and the next workspace container made for the profile removes the image after it.
    let after = capture(&engine.image_exists(image)).ok();
    if let Some(before) = before.as_deref().map(str::trim)
        && after.as_deref().map(str::trim) != Some(before)
    {
        let _ = capture(&engine.remove_unused_image(before));
    }
    // A build is the moment images of ours pile up: every one a rebuild replaced kept its gigabytes
    // under no name, and the ones this build could not account for — a container still holding
    // them, a QCode closed mid-build — are what it leaves. So it asks which of ours are left and
    // takes them away, and only those: a build that failed or was stopped above returned already.
    let _ = clear_leftovers(engine);
    Ok(())
}

/// Takes away the profile images of ours that a rebuild left without a name, and answers how many
/// of them went.
///
/// Only untagged images carrying [`PROFILE_LABEL`](crate::engine::names::PROFILE_LABEL) are ever
/// asked about, and each is removed without a `--force`: the engine refuses the one a container is
/// still made from, and that refusal is the answer wanted, since the container goes on running and
/// the image goes with the last of them. A refusal is otherwise not worth a word, so it is
/// ignored; an engine that cannot say which images are ours at all is asked nothing else either,
/// because there is no wider question to fall back on.
#[must_use]
pub fn clear_leftovers(engine: &Engine) -> usize {
    let Ok(listed) = capture(&engine.our_leftover_images()) else { return 0 };
    listed
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .filter(|id| capture(&engine.remove_unused_image(id)).is_ok())
        .count()
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
    /// The private folder `capture` is, when this login made it; it goes when this does, so a
    /// login that ends any way at all leaves no token on the machine.
    folder: Option<Scratch>,
}

impl LoginContainer {
    /// A container that copies its login into `capture`, a folder somebody else made and takes
    /// away.
    #[must_use]
    pub fn into_folder(name: String, capture: PathBuf) -> Self {
        Self { name, capture, folder: None }
    }
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
    let folder =
        Scratch::new(&format!("login-{}", profile.name)).map_err(|error| Problem::Machine(error.to_string()))?;
    let user = HostUser::current().map_err(|error| Problem::Machine(error.to_string()))?;
    let mounts = [Mount {
        source: MountSource::Path(folder.path()),
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
        Ok(_) => Ok(LoginContainer { name, capture: folder.path().to_owned(), folder: Some(folder) }),
        Err(error) => Err(error.into()),
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
    // The half of the login a harness keeps among its other settings goes along, or a workspace
    // given the files alone is asked to sign in again (see `profile::account`).
    if let Some(take) = account::take(profile.harness, recipe::CAPTURE_DIR) {
        let words: Vec<&str> = take.iter().map(String::as_str).collect();
        capture(&engine.exec_without_terminal(&Exec { container: &container.name, command: &words }))?;
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
    // Only a folder this login made is its to remove; one it was handed goes with its maker.
    if let Some(folder) = &container.folder {
        let _ = std::fs::remove_dir_all(folder.path());
    }
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

    /// A stand-in engine that writes every call down, answers `listed` when it is asked for the
    /// images of ours that carry no name, and refuses each call in `refused` the way an engine
    /// refuses an image a container is made from.
    ///
    /// The refusal is the whole rule: the image stays, the call fails, and the one beside it goes.
    fn recording(name: &str, listed: &str, refused: &[&str]) -> (Engine, PathBuf, PathBuf) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-work-{name}-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a folder");
        let (binary, calls) = (folder.join("engine"), folder.join("calls"));
        let refusing = refused
            .iter()
            .map(|call| format!("  '{call}'*) printf 'Error: the image is in use by a container\\n' >&2; exit 125 ;;"))
            .collect::<Vec<String>>()
            .join("\n");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\ncase \"$1 $2 $3\" in\n{refusing}\nesac\n\
             case \"$1 $2\" in\n  'images --quiet') printf '{listed}';;\nesac\nexit 0\n",
            calls = calls.display(),
        );
        std::fs::write(&binary, script).expect("the stand-in engine");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        (Engine::new(crate::engine::EngineKind::Podman, &binary), calls, folder)
    }

    /// Every call the stand-in engine was given, in order.
    fn asked(calls: &Path) -> Vec<String> {
        std::fs::read_to_string(calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// The whole rule in one sweep: an image of ours with no name and no container made from it
    /// goes, the one a container holds stays, and nothing but those two images is ever touched.
    #[test]
    fn an_image_of_ours_goes_only_while_no_container_is_made_from_it() {
        let (engine, calls, folder) = recording("leftovers", "sha-one\\nsha-two\\n", &["image rm sha-two"]);
        assert_eq!(clear_leftovers(&engine), 1, "one of the two went");
        assert_eq!(
            asked(&calls),
            [
                "images --quiet --no-trunc --filter dangling=true --filter label=qcode.profile",
                "image rm sha-one",
                "image rm sha-two",
            ],
            "the listing and one plain removal each: {:#?}",
            asked(&calls)
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// An engine that cannot say which images are ours is asked nothing else: there is no wider
    /// question behind this one, and a person who never made an image should never feel one.
    #[test]
    fn an_engine_that_cannot_list_the_leftovers_is_asked_to_take_nothing_away() {
        let (engine, calls, folder) = recording("no-leftovers", "", &["images --quiet"]);
        assert_eq!(clear_leftovers(&engine), 0);
        assert_eq!(
            asked(&calls),
            ["images --quiet --no-trunc --filter dangling=true --filter label=qcode.profile"],
            "{:#?}",
            asked(&calls)
        );
        let _ = std::fs::remove_dir_all(&folder);
    }
}
