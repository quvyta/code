//! What a person installs as the administrator in a workspace stays in that workspace.
//!
//! A workspace's container is made again whenever its plan changes or its profile's image is
//! rebuilt, and everything outside the home and the workspace's folders goes with it. So before a
//! container with changes to the system is removed, it is committed to an image of the workspace's
//! own ([`image`]), and the next container of the same profile is made from that image while the
//! profile's image under it is the same one. Nothing of it ever goes back to the profile or to
//! another workspace: the image is named after both, and only this workspace's container is made
//! from it.
//!
//! When the profile's image is rebuilt the old layer cannot be carried onto the new image. The
//! administrator's commands, recorded in the workspace's home ([`HISTORY`]), are run again on the
//! new image in a short-lived container and that one is committed instead. A command that fails
//! leaves the workspace on its old image, and the person is told which one it was.
//!
//! Everything here runs engine commands and waits for them, on a background thread.

use std::path::Path;

use crate::base::paths::{HOME_DIR, KEEP_ALIVE, USER};
use crate::engine::run::{EngineError, capture};
use crate::engine::{Access, ContainerCreate, Engine, EngineCommand, Exec, HostUser, Mount, MountSource, names};
use crate::profile::changes::Changes;

use super::plan::ContainerPlan;

/// Where the administrator's commands in a workspace are recorded: in the workspace's own home,
/// so they last as long as the workspace, go with a copy of it, and are nobody else's.
pub const HISTORY: &str = "/home/qcode/.qcode-admin-history";

/// The label a workspace's image carries the identity of the profile's image it was made on in.
pub const BASE_LABEL: &str = "qcode.workspace.base";

/// The image a workspace keeps what its administrator installed for one of its profiles in.
///
/// Both names are in it as folders of their own, so two pairs whose names joined the same way
/// (`a-b` with `c`, and `a` with `b-c`) never share an image.
#[must_use]
pub fn image(workspace: &str, profile: &str) -> String {
    format!("qcode/workspace/{workspace}/{profile}")
}

/// The command an "As administrator" tab runs: `bash` as root in the container of `plan`,
/// recording each command in the workspace's home as it is run.
#[must_use]
pub fn enter_admin(engine: &Engine, plan: &ContainerPlan) -> EngineCommand {
    let env = [("HISTFILE", HISTORY), ("PROMPT_COMMAND", "history -a")];
    engine.exec_as_root(&Exec { container: &plan.name, command: &["bash"] }, &env)
}

/// Whether a plan is one a workspace keeps an image of its own for: a profile's command-line
/// container. The shell's container is the base image's and a window's is made afresh each time.
fn kept(plan: &ContainerPlan) -> Option<(&str, &str)> {
    if plan.home.is_none() || plan.window.is_some() {
        return None;
    }
    let home = plan.home.as_ref()?;
    Some((home.workspace().as_str(), home.profile().as_str()))
}

/// What the container of `plan` is to be made from, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Made {
    /// The profile's image: the workspace keeps nothing of its own for this profile.
    Profile,
    /// The workspace's own image, made on the profile's image as it is now.
    Own(String),
    /// The workspace's own image, made on an earlier build of the profile's image, because running
    /// the recorded commands again on the new one failed at this command.
    Behind {
        /// The workspace's image.
        image: String,
        /// The command that failed.
        failed: String,
    },
}

/// Keeps what the stopped container of `plan` changed in the system before it is removed: when
/// its `diff` shows such a change, the container is committed onto the workspace's image, which
/// is labelled with the profile's image it descends from. A container without such changes, or a
/// plan the workspace keeps nothing for, is left alone.
///
/// # Errors
///
/// When the engine refuses to list or commit.
pub fn keep(engine: &Engine, plan: &ContainerPlan) -> Result<(), EngineError> {
    let Some((workspace, profile)) = kept(plan) else { return Ok(()) };
    let changes = Changes::parse(&capture(&engine.container_diff(&plan.name))?);
    if changes.system.is_empty() {
        return Ok(());
    }
    let own = image(workspace, profile);
    // Made from the workspace's image, the container descends from what that one did; made from
    // the profile's, from that one.
    let made_from = capture(&engine.container_image(&plan.name))?;
    let own_id = capture(&engine.image_exists(&own)).ok();
    let base = if own_id.as_deref().map(str::trim) == Some(made_from.trim()) {
        label(engine, &own).unwrap_or_default()
    } else {
        made_from.trim().to_owned()
    };
    capture(&engine.commit_container(&plan.name, &own, USER, &[(BASE_LABEL, &base)]))?;
    Ok(())
}

/// Removes the workspace image `own` had before a commit, once nothing is made from it.
pub fn let_go(engine: &Engine, old: Option<&str>) {
    if let Some(old) = old {
        let _ = capture(&engine.remove_unused_image(old));
    }
}

/// The identity of the workspace's image of `plan`, when it has one.
#[must_use]
pub fn current(engine: &Engine, plan: &ContainerPlan) -> Option<String> {
    let (workspace, profile) = kept(plan)?;
    capture(&engine.image_exists(&image(workspace, profile))).ok().map(|id| id.trim().to_owned())
}

/// The workspace's image of `plan` when there is one, and whether it was made on the profile's
/// image as it is now; asked without running anything, to tell whether a stopped container is
/// still the one to start.
#[must_use]
pub fn standing(engine: &Engine, plan: &ContainerPlan) -> Option<(String, bool)> {
    let (workspace, profile) = kept(plan)?;
    let own = image(workspace, profile);
    capture(&engine.image_exists(&own)).ok()?;
    let now = capture(&engine.image_exists(&plan.image)).ok();
    let fresh = now.is_some_and(|now| label(engine, &own).as_deref() == Some(now.trim()));
    Some((own, fresh))
}

/// What the next container of `plan` is made from: the workspace's image when there is one made
/// on the profile's image as it is; when the profile's image was rebuilt since, the recorded
/// commands run again on it first, and the workspace stays on the image it had when one fails.
#[must_use]
pub fn choose(engine: &Engine, plan: &ContainerPlan, user: HostUser) -> Made {
    let Some((workspace, profile)) = kept(plan) else { return Made::Profile };
    let own = image(workspace, profile);
    if capture(&engine.image_exists(&own)).is_err() {
        return Made::Profile;
    }
    let Ok(now) = capture(&engine.image_exists(&plan.image)) else { return Made::Own(own) };
    if label(engine, &own).as_deref() == Some(now.trim()) {
        return Made::Own(own);
    }
    match replay(engine, plan, &own, now.trim(), user) {
        Ok(()) => Made::Own(own),
        Err(failed) => Made::Behind { image: own, failed },
    }
}

/// The value of [`BASE_LABEL`] on `image`.
fn label(engine: &Engine, image: &str) -> Option<String> {
    let said = capture(&engine.image_label(image, BASE_LABEL)).ok()?;
    let said = said.trim();
    (!said.is_empty() && said != "<no value>").then(|| said.to_owned())
}

/// Runs the workspace's recorded administrator commands again on the profile's rebuilt image in a
/// container of its own, and commits it onto the workspace's image. Answers the command that
/// failed, or the engine's words when the engine itself refused; the workspace's image is then
/// as it was.
fn replay(engine: &Engine, plan: &ContainerPlan, own: &str, base: &str, user: HostUser) -> Result<(), String> {
    let Some(home) = plan.home.as_ref() else { return Ok(()) };
    let name = format!("{}.replay", plan.name);
    let volume = home.volume();
    let mounts =
        [Mount { source: MountSource::Volume(&volume), target: Path::new(HOME_DIR), access: Access::ReadOnly }];
    let request = ContainerCreate {
        name: &name,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &plan.image,
        mounts: &mounts,
        network: plan.network,
        user,
        workdir: None,
        command: KEEP_ALIVE,
    };
    let _ = capture(&engine.remove_container(&name));
    let said = |error: EngineError| match error {
        EngineError::Failed(failure) => failure.output,
        other => format!("{other:?}"),
    };
    let result = capture(&engine.create_container(&request))
        .and_then(|_| capture(&engine.start_container(&name)))
        .map_err(said)
        .and_then(|_| match capture(&replay_command(engine, &name)) {
            Ok(_) => Ok(()),
            Err(EngineError::Failed(failure)) => Err(failure.output.trim().to_owned()),
            Err(other) => Err(format!("{other:?}")),
        })
        .and_then(|()| {
            let old = capture(&engine.image_exists(own)).ok();
            let old_base = label(engine, own);
            capture(&engine.commit_container(&name, own, USER, &[(BASE_LABEL, base)])).map_err(said)?;
            let _ = capture(&engine.remove_container(&name));
            // The workspace's old image, and then the profile's image from before the rebuild,
            // which only that one still held.
            let_go(engine, old.as_deref().map(str::trim));
            let_go(engine, old_base.as_deref());
            Ok(())
        });
    let _ = capture(&engine.remove_container(&name));
    result
}

/// The loop that runs each command of the record at `history` again, one at a time, and on the
/// first that fails prints it and stops.
///
/// The commands were typed into `bash` by a person who answered what they asked, so each runs in
/// `bash` with every question answered yes: `apt install jq` asks before it installs, and with
/// nothing to read from it would give up. `exit` only ended the shell it was typed in.
/// The command that runs the recorded administrator commands again in the container `name`.
/// It is waited for however long it takes: each command may install packages, and may take up to
/// the quarter of an hour [`replay_loop`] gives it.
fn replay_command(engine: &Engine, name: &str) -> EngineCommand {
    let script = format!("{}; rm -rf /var/lib/apt/lists/*", replay_loop(HISTORY));
    engine.exec_as_root_without_terminal(&Exec { container: name, command: &["sh", "-c", &script] }).unbounded()
}

fn replay_loop(history: &str) -> String {
    format!(
        "while IFS= read -r line; do case \"$line\" in ''|exit|logout|'#'*) continue ;; esac; \
         yes 2>/dev/null | DEBIAN_FRONTEND=noninteractive timeout 900 bash -c \"$line\" >/dev/null 2>&1 \
         || {{ printf '%s' \"$line\"; exit 1; }}; done < {history}"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn installing_again_on_a_rebuilt_image_is_waited_for_however_long_it_takes() {
        let engine = Engine::new(crate::engine::EngineKind::Podman, "/usr/bin/podman");
        let command = super::replay_command(&engine, "qcode-a-b.replay");
        assert_eq!(command.deadline, None, "apt can take minutes; a question's limit would cut it off");
    }

    use super::*;

    #[test]
    fn a_workspaces_image_names_the_workspace_and_the_profile_apart() {
        assert_eq!(image("site", "claude"), "qcode/workspace/site/claude");
        assert_ne!(image("a-b", "c"), image("a", "b-c"));
    }

    /// Runs the loop over `record` on this machine, which has `sh`, `bash`, `yes` and `timeout`
    /// like every image; answers what it printed and whether it went through.
    fn replayed(record: &str) -> (String, bool) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let history = std::env::temp_dir().join(format!("qcode-replay-{stamp}"));
        std::fs::write(&history, record).expect("the record");
        let output = std::process::Command::new("sh")
            .args(["-c", &replay_loop(&history.display().to_string())])
            .output()
            .expect("sh runs");
        let _ = std::fs::remove_file(&history);
        (String::from_utf8_lossy(&output.stdout).into_owned(), output.status.success())
    }

    #[test]
    fn a_command_that_asks_before_it_goes_on_is_answered_yes_when_it_runs_again() {
        let (said, through) = replayed("true\nread answer; [ \"$answer\" = y ]\nexit\n");
        assert!(through, "{said}");
    }

    #[test]
    fn the_first_command_that_fails_is_named_and_nothing_after_it_runs() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let after = std::env::temp_dir().join(format!("qcode-replay-after-{stamp}"));
        let record = format!("[[ 1 == 1 ]]\nfalse && echo never\ntouch {}\n", after.display());
        let (said, through) = replayed(&record);
        assert!(!through);
        assert_eq!(said, "false && echo never");
        assert!(!after.exists(), "a command after the failed one ran");
    }
}
