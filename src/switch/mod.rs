//! Moving from one container engine to the other: what the new engine lacks, what of the old one
//! QCode carries over, and what it leaves behind.
//!
//! The two engines keep separate stores. A profile image built with docker is not in podman,
//! and a workspace's home volume — the harness's settings, history and login — made in podman
//! is not in docker. Nothing here moves anything: it copies. The old engine keeps every volume
//! it had, so the person can switch back and find everything as it was; removing what is left
//! there is the person's own choice, and only containers are ever offered for removal.
//!
//! Everything here runs engine commands and waits for them, so it belongs on a background thread.

use std::path::Path;

#[cfg(test)]
mod live;

use crate::base::paths::HOME_DIR;
use crate::base::{self, Presence};
use crate::engine::names::BASE_IMAGE;
use crate::engine::run::{self, EngineError, capture};
use crate::engine::{Access, Engine, EngineKind, HostUser, Mount, MountSource, Network, RunOnce};
use crate::profile::{Profile, SafeName};
use crate::ui::workspace::PLAN_LABEL;

/// The volumes a switch carries over: every workspace's home of a profile, which holds the
/// harness's settings, history and memory, and every profile's stored login, which the profiles
/// screen reads as "signed in" and which a new workspace's home is filled from.
const CARRIED: [&str; 2] = ["qcode-home-", "qcode-cred-"];

/// Whether the volume `name` is one QCode carries from one engine to the other.
#[must_use]
pub fn carried(name: &str) -> bool {
    CARRIED.iter().any(|prefix| name.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty()))
}

/// The engine to offer at start, when the saved one does not answer and the other one does.
///
/// It is the only case QCode notices by itself. The saved engine answering is the person's
/// choice standing; neither answering is not a switch but a missing engine, which the repair
/// strip is for. Anything else — an engine that answers again later, a second engine installed
/// beside the first — is left alone: switching is the person's to ask for, in the settings.
#[must_use]
pub fn offer_at_start(saved: EngineKind, saved_answers: bool, other_answers: bool) -> Option<EngineKind> {
    (!saved_answers && other_answers).then_some(other(saved))
}

/// The engine that is not `kind`.
#[must_use]
pub fn other(kind: EngineKind) -> EngineKind {
    match kind {
        EngineKind::Podman => EngineKind::Docker,
        EngineKind::Docker => EngineKind::Podman,
    }
}

/// A volume of the old engine that a switch can carry over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    /// The volume's name, the same on both engines.
    pub name: String,
    /// Whether the new engine already has a volume of that name, which is never written over
    /// without asking.
    pub taken: bool,
}

/// The containers QCode made that are still in the old engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    /// Their names.
    pub containers: Vec<String>,
    /// How many bytes they hold of their own, above their images.
    pub bytes: u64,
}

/// What a switch finds: what the new engine lacks and what the old one holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// The profiles whose image the new engine does not have.
    pub missing: Vec<SafeName>,
    /// The volumes the old engine holds that can be carried over; `None` when the old engine
    /// does not answer, so nothing can be read from it.
    pub volumes: Option<Vec<Carried>>,
    /// QCode's containers still in the old engine; `None` when it does not answer.
    pub leftover: Option<Leftover>,
}

/// Asks both engines what a switch from `old` to `new` involves for `profiles`.
///
/// `old` is `None` when the engine switched from does not answer. A listing that fails is read as
/// empty: the survey says what it could find, and every action it offers asks again for itself.
#[must_use]
pub fn survey(old: Option<&Engine>, new: &Engine, profiles: &[Profile]) -> Survey {
    let missing = profiles
        .iter()
        .filter(|profile| capture(&new.image_exists(&profile.image())).is_err())
        .map(|profile| profile.name.clone())
        .collect();
    let listed = |engine: &Engine| -> Vec<String> {
        capture(&engine.list_volumes())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|name| carried(name))
            .map(str::to_owned)
            .collect()
    };
    let there = listed(new);
    let volumes =
        old.map(|old| listed(old).into_iter().map(|name| Carried { taken: there.contains(&name), name }).collect());
    let leftover = old.map(|old| {
        let containers: Vec<String> = capture(&old.list_labelled(PLAN_LABEL))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        let bytes = containers
            .iter()
            .filter_map(|name| capture(&old.container_size(name)).ok())
            .filter_map(|size| size.trim().parse::<u64>().ok())
            .sum();
        Leftover { containers, bytes }
    });
    Survey { missing, volumes, leftover }
}

/// Why a volume could not be copied, in words the person is shown as they are.
pub type Unmoved = String;

/// Copies the volume `name` from `old` into `new`, as `user`.
///
/// A short-lived container on each side, both of QCode's own base image with no network: the
/// old one packs the volume with `tar` and the new one unpacks it, the bytes passing straight
/// from one to the other ([`run::pipe`]). Both run as the person, the way QCode's own containers
/// do, so what is unpacked is owned the way the harness in the new engine expects to find it.
/// The volume is mounted where a home is mounted, so a fresh one is made the way the new engine
/// makes any home: from the image's own home folder.
///
/// The old volume is only read. With `replace`, a volume of that name already in the new engine
/// is removed first; the caller asks the person before passing it.
///
/// # Errors
///
/// The engine's own words when either side refuses, and the build's when the base image one of
/// them lacks cannot be built.
pub fn copy(old: &Engine, new: &Engine, name: &str, user: HostUser, replace: bool) -> Result<(), Unmoved> {
    for engine in [old, new] {
        // Any base image will do for `tar`, so an older one is used as it is rather than rebuilt.
        if base::presence(engine) == Presence::Missing {
            base::ensure(engine, &|| false, &mut |_| {}).map_err(|failure| match failure {
                base::Failure::Host(error) => error.to_string(),
                base::Failure::Engine(error) => said(&error),
            })?;
        }
    }
    if replace {
        capture(&new.remove_volume(name)).map_err(|error| said(&error))?;
    }
    capture(&new.create_volume(name)).map_err(|error| said(&error))?;
    let home = Path::new(HOME_DIR);
    let reading = [Mount { source: MountSource::Volume(name), target: home, access: Access::ReadOnly }];
    let writing = [Mount { source: MountSource::Volume(name), target: home, access: Access::ReadWrite }];
    let pack = ["tar", "-C", HOME_DIR, "-cf", "-", "."];
    let unpack = ["tar", "-C", HOME_DIR, "-xf", "-"];
    let from = old.run_once(&RunOnce {
        image: BASE_IMAGE,
        mounts: &reading,
        network: Network::None,
        user,
        workdir: None,
        command: &pack,
    });
    let to = new.run_fed(&RunOnce {
        image: BASE_IMAGE,
        mounts: &writing,
        network: Network::None,
        user,
        workdir: None,
        command: &unpack,
    });
    run::pipe(&from, &to).map_err(|error| said(&error))
}

/// Removes QCode's `containers` from `old`. Only containers: every volume stays, so switching
/// back finds every home and login where it was.
///
/// # Errors
///
/// The engine's own words for the first container it would not remove.
pub fn remove(old: &Engine, containers: &[String]) -> Result<(), Unmoved> {
    for name in containers {
        capture(&old.remove_container(name)).map_err(|error| said(&error))?;
    }
    Ok(())
}

/// The words an engine error is shown in: what the engine said, or what the system said when it
/// could not be started.
fn said(error: &EngineError) -> String {
    match error {
        EngineError::NotRunnable { error, .. } => error.to_string(),
        EngineError::Failed(failure) => failure.output.clone(),
        EngineError::Cancelled { .. } => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{carried, offer_at_start};
    use crate::engine::EngineKind;

    #[test]
    fn homes_and_logins_are_carried_and_nothing_else_of_the_engine() {
        assert!(carried("qcode-home-ads-claude-code"));
        assert!(carried("qcode-cred-claude-code"));
        // Volumes of the person's own, or of other tools, that merely start with the same word.
        for other in ["qcode-cache", "qcode-git-mirror", "qcode-homework", "qcode-home-", "pgdata"] {
            assert!(!carried(other), "{other}");
        }
    }

    #[test]
    fn a_switch_is_offered_at_start_only_when_the_saved_engine_is_silent_and_the_other_answers() {
        use EngineKind::{Docker, Podman};
        assert_eq!(offer_at_start(Podman, false, true), Some(Docker));
        assert_eq!(offer_at_start(Docker, false, true), Some(Podman));
        assert_eq!(offer_at_start(Podman, true, true), None, "the saved choice stands while it answers");
        assert_eq!(offer_at_start(Podman, false, false), None, "no engine at all is the repair strip's");
        assert_eq!(offer_at_start(Docker, true, false), None);
    }
}
