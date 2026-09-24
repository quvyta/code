//! Deleting a workspace: finding everything it owns, and taking exactly that away.
//!
//! A workspace owns its folder under `Workspaces/`, the containers named after it and the home
//! volumes its profiles keep there. Nothing else is touched: the profiles, their images and their
//! logins belong to every workspace that uses them, and a folder the workspace was copied from was
//! never moved into it.
//!
//! Container and volume names join the workspace's identifier and a profile's name with a dash,
//! and both may hold dashes of their own, so `qcode-a-b-c` could be the workspace `a` with the
//! profile `b-c` or the workspace `a-b` with the profile `c`. A name another workspace of the store
//! could also claim is left where it is: leaving something behind can be put right later, and
//! removing another workspace's container cannot.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::names;
use crate::engine::run::{EngineError, capture};
use crate::engine::{Container, Engine};
use crate::store::{Store, WorkspaceEntry, WorkspaceId};

/// Everything one workspace owns, as it was found when the person asked to delete it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// The name the list shows the workspace by.
    pub name: String,
    /// The workspace's own folder.
    pub dir: PathBuf,
    /// The workspace's identifier, when its folder name is one. A folder whose name is not has no
    /// containers or volumes named after it.
    pub id: Option<WorkspaceId>,
    /// What the engine answered.
    pub engine: Reach,
}

/// What the engine answered about a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// There is no engine, so whatever it holds of the workspace stays there.
    Absent,
    /// The engine could not be asked; its own words.
    Unreachable(String),
    /// The engine was asked.
    Listed {
        /// The workspace's containers.
        containers: Vec<String>,
        /// The ones among them that are running.
        running: Vec<String>,
        /// The workspace's volumes.
        volumes: Vec<String>,
    },
}

impl Survey {
    /// The containers of the workspace that are running, which is what stops the deletion.
    #[must_use]
    pub fn running(&self) -> &[String] {
        match &self.engine {
            Reach::Listed { running, .. } => running,
            Reach::Absent | Reach::Unreachable(_) => &[],
        }
    }
}

/// What came of a deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Everything the survey found is gone.
    Deleted,
    /// A container of the workspace was started after the question was asked, so nothing was
    /// removed.
    Running(Vec<String>),
    /// Some of it could not be removed: each line names what is left and why.
    Partly(Vec<String>),
}

/// Finds everything `entry` owns: its folder, and what `engine` holds under its name.
///
/// Runs the engine, so it belongs on a background thread.
#[must_use]
pub fn survey(store: &Store, engine: Option<&Engine>, entry: &WorkspaceEntry, name: String) -> Survey {
    let id = folder_id(&entry.dir);
    let engine = match (engine, &id) {
        (None, _) => Reach::Absent,
        // A folder that names no workspace has nothing in the engine named after it.
        (Some(_), None) => Reach::Listed { containers: Vec::new(), running: Vec::new(), volumes: Vec::new() },
        (Some(engine), Some(id)) => listed(store, engine, id),
    };
    Survey { name, dir: entry.dir.clone(), id, engine }
}

/// Removes everything `survey` found, containers first, so no volume is still in use when its
/// turn comes, and the folder last, so a failure in the engine leaves the workspace on the list to
/// be deleted again.
///
/// The engine is asked once more first: a container started since the question was asked is not
/// stopped behind the person's back.
///
/// Runs the engine and the disk, so it belongs on a background thread.
#[must_use]
pub fn remove(store: &Store, engine: Option<&Engine>, survey: &Survey) -> Outcome {
    let mut left = Vec::new();
    if let (Some(engine), Some(id)) = (engine, &survey.id)
        && let Reach::Listed { containers, volumes, .. } = &survey.engine
    {
        if let Reach::Listed { running, .. } = listed(store, engine, id)
            && !running.is_empty()
        {
            return Outcome::Running(running);
        }
        for container in containers {
            if let Err(error) = capture(&engine.remove_container(container)) {
                left.push(format!("{container}: {}", said(&error)));
            }
        }
        for volume in volumes {
            if let Err(error) = capture(&engine.remove_volume(volume)) {
                left.push(format!("{volume}: {}", said(&error)));
            }
        }
    }
    if let Err(reason) = remove_folder(store, &survey.dir) {
        left.push(format!("{}: {reason}", survey.dir.display()));
    }
    if left.is_empty() { Outcome::Deleted } else { Outcome::Partly(left) }
}

/// Removes the workspace folder `dir`, and only when it is a folder of the store's `Workspaces/`.
///
/// A link standing there is removed as a link: what it points at is not the store's.
fn remove_folder(store: &Store, dir: &Path) -> Result<(), String> {
    if dir.parent() != Some(store.workspaces_dir().as_path()) {
        return Err("not a folder of the QCode folder's Workspaces".to_owned());
    }
    let kind = match fs::symlink_metadata(dir) {
        Ok(meta) => meta.file_type(),
        // Already gone is what was asked for.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let removed = if kind.is_dir() { fs::remove_dir_all(dir) } else { fs::remove_file(dir) };
    removed.map_err(|error| error.to_string())
}

/// The workspace identifier a folder is named by, if it is one.
fn folder_id(dir: &Path) -> Option<WorkspaceId> {
    WorkspaceId::parse(&dir.file_name()?.to_string_lossy()).ok()
}

/// Asks the engine for the containers and volumes of the workspace `id`.
fn listed(store: &Store, engine: &Engine, id: &WorkspaceId) -> Reach {
    let containers = match capture(&engine.list_containers()) {
        Ok(output) => Container::parse_list(&output),
        Err(error) => return Reach::Unreachable(said(&error)),
    };
    let volumes: Vec<String> = match capture(&engine.list_volumes()) {
        Ok(output) => output.lines().map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned).collect(),
        Err(error) => return Reach::Unreachable(said(&error)),
    };
    let names = Names::read(store);
    let running = containers
        .iter()
        .filter(|container| container.state.is_running() && names.owns_container(id, &container.name))
        .map(|container| container.name.clone())
        .collect();
    let containers = containers
        .into_iter()
        .filter(|container| names.owns_container(id, &container.name))
        .map(|container| container.name)
        .collect();
    let volumes = volumes.into_iter().filter(|volume| names.owns_volume(id, volume)).collect();
    Reach::Listed { containers, running, volumes }
}

/// The names that go into the engine's names: every workspace of the store and every profile any
/// of them may have used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Names {
    /// Every workspace identifier the store's folders are named by.
    pub(crate) workspaces: BTreeSet<String>,
    /// Every profile a workspace may have a container or a home of: the store's profiles, the
    /// ones a workspace file names, and the ones a workspace keeps a folder for.
    pub(crate) profiles: BTreeSet<String>,
}

impl Names {
    /// Reads the names out of `store`.
    pub(crate) fn read(store: &Store) -> Self {
        let mut names = Self::default();
        names.profiles.extend(store.profiles().value.into_iter().map(|profile| profile.name.as_str().to_owned()));
        for entry in store.workspaces().value {
            let Some(id) = folder_id(&entry.dir) else { continue };
            if let Some(file) = &entry.file {
                names.profiles.extend(file.profiles.iter().map(|profile| profile.name.clone()));
            }
            if let Ok(folders) = fs::read_dir(store.workspace_paths(&id).harness) {
                names
                    .profiles
                    .extend(folders.flatten().map(|folder| folder.file_name().to_string_lossy().into_owned()));
            }
            names.workspaces.insert(id.as_str().to_owned());
        }
        names
    }

    /// Whether the container `name` is the workspace `id`'s, and no other workspace's.
    pub(crate) fn owns_container(&self, id: &WorkspaceId, name: &str) -> bool {
        let claimants = self.container_claimants(name);
        !claimants.is_empty() && claimants.iter().all(|(workspace, _)| workspace == id.as_str())
    }

    /// Whether the volume `name` is the workspace `id`'s, and no other workspace's.
    pub(crate) fn owns_volume(&self, id: &WorkspaceId, name: &str) -> bool {
        let claimants = self.volume_claimants(name);
        !claimants.is_empty() && claimants.iter().all(|(workspace, _)| workspace == id.as_str())
    }

    /// Whether the container `name` is one a workspace made for the profile `profile`, and could
    /// not be any other profile's or a workspace's own shell.
    pub(crate) fn profile_owns_container(&self, profile: &str, name: &str) -> bool {
        let claimants = self.container_claimants(name);
        !claimants.is_empty() && claimants.iter().all(|(_, owner)| owner.as_deref() == Some(profile))
    }

    /// Whether the volume `name` is a workspace's home of the profile `profile`, and could not be
    /// any other profile's.
    pub(crate) fn profile_owns_volume(&self, profile: &str, name: &str) -> bool {
        let claimants = self.volume_claimants(name);
        !claimants.is_empty() && claimants.iter().all(|(_, owner)| owner.as_deref() == Some(profile))
    }

    /// Every workspace, and profile where there is one, that could have made the container `name`.
    fn container_claimants(&self, name: &str) -> Vec<(String, Option<String>)> {
        let mut claimants = Vec::new();
        for workspace in &self.workspaces {
            // A sound's container is told apart by a dot right after the workspace, which neither
            // a workspace identifier nor a profile name can hold, so its prefix is the workspace's
            // alone.
            if name == names::base_container(workspace) || name.starts_with(&format!("qcode-{workspace}.play-")) {
                claimants.push((workspace.clone(), None));
            }
            for profile in &self.profiles {
                let made = [
                    names::profile_container(workspace, profile),
                    names::desktop_container(workspace, profile),
                    names::settings_container(workspace, profile),
                ];
                if made.iter().any(|own| own == name) {
                    claimants.push((workspace.clone(), Some(profile.clone())));
                }
            }
        }
        claimants
    }

    /// Every workspace and profile whose home could be the volume `name`.
    fn volume_claimants(&self, name: &str) -> Vec<(String, Option<String>)> {
        self.workspaces
            .iter()
            .flat_map(|workspace| self.profiles.iter().map(move |profile| (workspace, profile)))
            .filter(|(workspace, profile)| names::home_volume(workspace, profile) == name)
            .map(|(workspace, profile)| (workspace.clone(), Some(profile.clone())))
            .collect()
    }
}

/// What the engine said when it would not do what was asked.
fn said(error: &EngineError) -> String {
    match error {
        EngineError::NotRunnable { error, .. } => error.to_string(),
        EngineError::Failed(failure) => failure.output.trim().to_owned(),
        EngineError::Cancelled { .. } => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::Names;
    use crate::store::WorkspaceId;

    fn names(workspaces: &[&str], profiles: &[&str]) -> Names {
        Names {
            workspaces: workspaces.iter().map(|name| (*name).to_owned()).collect(),
            profiles: profiles.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    fn id(text: &str) -> WorkspaceId {
        WorkspaceId::parse(text).expect("a workspace identifier")
    }

    #[test]
    fn a_workspace_owns_its_shell_its_profiles_windows_and_sounds_and_nothing_of_another() {
        let known = names(&["firefly", "serenity"], &["claude", "gemini"]);
        let firefly = id("firefly");
        for own in [
            "qcode-firefly-base",
            "qcode-firefly-claude",
            "qcode-firefly-gemini.desk",
            "qcode-firefly-claude.mcp",
            "qcode-firefly.play-7",
        ] {
            assert!(known.owns_container(&firefly, own), "{own}");
        }
        for other in ["qcode-serenity-base", "qcode-serenity-claude", "qcode-firefly-unknown", "podman-thing"] {
            assert!(!known.owns_container(&firefly, other), "{other}");
        }
        assert!(known.owns_volume(&firefly, "qcode-home-firefly-claude"));
        assert!(!known.owns_volume(&firefly, "qcode-home-serenity-claude"));
        assert!(!known.owns_volume(&firefly, "qcode-cred-claude"), "a profile's login is every workspace's");
    }

    #[test]
    fn a_profile_owns_its_containers_and_homes_in_every_workspace_and_nothing_else() {
        let known = names(&["firefly", "serenity"], &["claude", "gemini", "base"]);
        for own in ["qcode-firefly-claude", "qcode-serenity-claude.desk", "qcode-firefly-claude.mcp"] {
            assert!(known.profile_owns_container("claude", own), "{own}");
        }
        for other in ["qcode-firefly-gemini", "qcode-firefly.play-3", "podman-thing"] {
            assert!(!known.profile_owns_container("claude", other), "{other}");
        }
        assert!(known.profile_owns_volume("claude", "qcode-home-serenity-claude"));
        assert!(!known.profile_owns_volume("claude", "qcode-cred-claude"), "the login is found by its own name");
        // A profile called `base` does not take the shell every workspace has under that name.
        assert!(!known.profile_owns_container("base", "qcode-firefly-base"));
    }

    #[test]
    fn a_name_two_workspaces_could_both_claim_is_left_to_neither() {
        // `qcode-a-b-c` is the workspace `a` with the profile `b-c`, or `a-b` with `c`.
        let known = names(&["a", "a-b"], &["b-c", "c"]);
        assert!(!known.owns_container(&id("a"), "qcode-a-b-c"));
        assert!(!known.owns_container(&id("a-b"), "qcode-a-b-c"));
        assert!(!known.owns_volume(&id("a"), "qcode-home-a-b-c"));
        // Without the second workspace the name is plainly the first one's.
        let alone = names(&["a"], &["b-c", "c"]);
        assert!(alone.owns_container(&id("a"), "qcode-a-b-c"));
        assert!(alone.owns_volume(&id("a"), "qcode-home-a-b-c"));
    }
}
