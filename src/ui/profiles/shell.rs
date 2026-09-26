//! The profile's shell: a container of the profile's image the person works in by hand, and what
//! becomes of it afterwards.
//!
//! The container reaches the network, because it exists to install things, and mounts nothing of
//! this machine: no workspace, no home volume, no login. So everything done in it, in the home and
//! in the system, is in its own layer, and nothing done in it can touch a file of the person's.
//! When it closes the engine's `diff` says what changed; added to the profile, the layer becomes
//! the profile's image (`commit`) and the administrator's commands and the home's files are kept
//! in the store so a rebuild can put them back ([`Own`]).
//!
//! Everything here blocks and runs on a task thread.

use std::path::{Path, PathBuf};

use crate::base::paths::{HOME_DIR, KEEP_ALIVE, USER};
use crate::engine::run::capture;
use crate::engine::{ContainerCreate, CopyOut, Engine, EngineCommand, Exec, HostUser, Network, names};
use crate::profile::account::{self, MERGE_DIR};
use crate::profile::changes::Changes;
use crate::profile::own::Own;
use crate::profile::{Profile, SafeName};

use super::work::Problem;

/// Where the person's own commands are recorded, outside the home so the record never becomes
/// part of it.
pub const HISTORY: &str = "/tmp/qcode-shell-history";

/// Where the administrator's commands are recorded: apart, because they are the ones a rebuild
/// runs again.
pub const ROOT_HISTORY: &str = "/tmp/qcode-shell-root-history";

/// Where the archive of the home is made before it is copied out.
const ARCHIVE: &str = "/tmp/qcode-own-home.tar";

/// The list of the home's paths the archive is made of.
const LIST: &str = "/tmp/qcode-own-home.list";

/// The container of a profile's shell.
///
/// The dot is what no workspace id and no profile name can hold, so this can never be the
/// container of a profile in a workspace called `shell`.
#[must_use]
pub fn container(profile: &SafeName) -> String {
    format!("qcode-shell.{profile}")
}

/// What opening a profile's shell found.
#[derive(Debug)]
pub enum Opened {
    /// A new container, up, of this name.
    Fresh(String),
    /// The container of a shell that was never closed — QCode ended while it was open — holding
    /// work that was neither added nor discarded. It is running again, and what it holds is said;
    /// only the person can say what becomes of it.
    Leftover(String, Session),
}

/// `command` run as root in the shell's container `name`, waited for however long it takes: what
/// it removes or archives is as large as what the person installed.
fn as_root(engine: &Engine, name: &str, command: &[&str]) -> EngineCommand {
    engine.exec_as_root_without_terminal(&Exec { container: name, command }).unbounded()
}

/// Opens the shell's container of `profile`, from its image, with the network and without any
/// mount.
///
/// A container left behind by a shell that was never closed is looked at first. When it changed
/// nothing it is removed without a word; when it changed something it is left standing and
/// answered as [`Opened::Leftover`], because what was done in it would otherwise be lost without
/// anyone having chosen to lose it.
///
/// # Errors
///
/// When the engine refuses. A container this made is removed again; a leftover one never is.
pub fn open(engine: &Engine, profile: &Profile, user: HostUser) -> Result<Opened, Problem> {
    let name = container(&profile.name);
    // Both engines answer a name they do not know with an error.
    if capture(&engine.container_state(&name)).is_ok() {
        // A shell's container outlives the machine's restarts stopped, and its record is read
        // from inside it.
        capture(&engine.start_container(&name))?;
        let session = survey(engine, &profile.name)?;
        if !(session.changes.is_empty() && session.steps.is_empty()) {
            return Ok(Opened::Leftover(name, session));
        }
        capture(&engine.remove_container(&name))?;
    }
    let request = ContainerCreate {
        name: &name,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &profile.image(),
        mounts: &[],
        network: Network::Full,
        user,
        workdir: Some(Path::new(HOME_DIR)),
        command: KEEP_ALIVE,
    };
    let made = capture(&engine.create_container(&request)).and_then(|_| capture(&engine.start_container(&name)));
    if let Err(error) = made {
        let _ = capture(&engine.remove_container(&name));
        return Err(error.into());
    }
    Ok(Opened::Fresh(name))
}

/// The command a shell's terminal runs: `bash` as the person, or as root for the administrator,
/// each recording its commands as they are run.
#[must_use]
pub fn enter(engine: &Engine, container: &str, admin: bool) -> EngineCommand {
    let history = if admin { ROOT_HISTORY } else { HISTORY };
    // Written after every command, not when the shell ends: a shell closed from outside never
    // gets to write its history, and those commands are the whole record.
    let env = [("HISTFILE", history), ("PROMPT_COMMAND", "history -a")];
    let request = Exec { container, command: &["bash"] };
    if admin { engine.exec_as_root(&request, &env) } else { engine.exec_with_env(&request, &env) }
}

/// What the shell changed, and the commands the administrator ran in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    /// What changed, the home apart from the system.
    pub changes: Changes,
    /// The administrator's commands, in order, as typed.
    pub steps: Vec<String>,
}

/// Reads what the shell of `profile` changed and what its administrator ran.
///
/// # Errors
///
/// When the engine refuses.
pub fn survey(engine: &Engine, profile: &SafeName) -> Result<Session, Problem> {
    let name = container(profile);
    let changes = Changes::parse(&capture(&engine.container_diff(&name))?);
    let steps = root_commands(engine, &name)?;
    Ok(Session { changes, steps })
}

/// The administrator's commands, read from their record. A shell nobody opened as administrator
/// has no record, which is no commands.
fn root_commands(engine: &Engine, container: &str) -> Result<Vec<String>, Problem> {
    let script = format!("cat {ROOT_HISTORY} 2>/dev/null || true");
    let said = capture(&engine.exec_as_root_without_terminal(&Exec { container, command: &["sh", "-c", &script] }))?;
    Ok(said
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !matches!(*line, "exit" | "logout") && !line.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

/// Makes what the shell of `profile` holds the profile's image, and keeps `steps` — the
/// administrator's commands as the person let them stand — and the home's changed files in the
/// store under `profiles`, beside what earlier shells added.
///
/// The login goes first: whatever the harness wrote to sign in inside the shell is taken out, so
/// no image ever carries one. Then the records and the scratch files go, the home's files are
/// archived and copied out, and the container is committed onto the image's name. The image it
/// replaces goes once nothing is made from it; a workspace container still made from it is made
/// again from the new one the next time it is stopped.
///
/// # Errors
///
/// When the engine refuses a step, or the store cannot be written. Nothing is kept in the store
/// unless the image was made.
pub fn add(engine: &Engine, profile: &Profile, profiles: &Path, steps: Vec<String>) -> Result<Own, Problem> {
    let name = container(&profile.name);
    let root = |script: &str| capture(&as_root(engine, &name, &["sh", "-c", script]));
    let mut forget = vec![format!("rm -rf '{HOME_DIR}/{MERGE_DIR}' {HISTORY} {ROOT_HISTORY} /var/lib/apt/lists/*")];
    forget.extend(profile.harness.record().identity.iter().map(|path| format!("rm -f '{HOME_DIR}/{path}'")));
    root(&forget.join("; "))?;
    if let Some(words) = account::strip(profile.harness) {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        capture(&as_root(engine, &name, &words))?;
    }

    let changes = Changes::parse(&capture(&engine.container_diff(&name))?);
    let (mut own, _) = Own::load(profiles, &profile.name);
    own.absorb(steps, changes.home);

    let archive = Own::archive(profiles, &profile.name);
    let staged = archive.with_extension("tar.new");
    if own.home.is_empty() {
        let _ = std::fs::remove_file(&archive);
    } else {
        std::fs::create_dir_all(Own::folder(profiles, &profile.name))
            .map_err(|error| Problem::Machine(error.to_string()))?;
        // Every path added so far, as the image has it now: an earlier shell's files are in the
        // image this shell started from, changed or not since. One that was removed is left out.
        let script = format!(
            "cd {HOME_DIR} && for p do if [ -e \"$p\" ] || [ -L \"$p\" ]; then printf '%s\\n' \"$p\"; fi; done > {LIST} \
             && tar -cf {ARCHIVE} -T {LIST} && rm -f {LIST}"
        );
        let mut words = vec!["sh", "-c", script.as_str(), "sh"];
        words.extend(own.home.iter().map(String::as_str));
        capture(&as_root(engine, &name, &words))?;
        capture(&engine.copy_out(&CopyOut { container: &name, source: Path::new(ARCHIVE), host: &staged }))?;
        root(&format!("rm -f {ARCHIVE}"))?;
    }

    let before = capture(&engine.image_exists(&profile.image())).ok();
    if let Err(error) = capture(&engine.commit_container(&name, &profile.image(), USER, &[])) {
        let _ = std::fs::remove_file(&staged);
        return Err(error.into());
    }
    if let Some(before) = before.as_deref().map(str::trim) {
        let _ = capture(&engine.remove_unused_image(before));
    }
    let _ = capture(&engine.remove_container(&name));
    if staged.exists() {
        std::fs::rename(&staged, &archive).map_err(|error| Problem::Machine(error.to_string()))?;
    }
    own.save(profiles, &profile.name).map_err(|problem| Problem::Machine(problem.message))?;
    Ok(own)
}

/// Removes the shell's container and everything done in it; the image is as it was.
pub fn discard(engine: &Engine, profile: &SafeName) {
    let _ = capture(&engine.remove_container(&container(profile)));
}

/// The folder a build context gets the home's archive from, when the profile has one.
#[must_use]
pub fn archive_of(profiles: &Path, profile: &SafeName) -> Option<PathBuf> {
    Some(Own::archive(profiles, profile)).filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use crate::engine::{Engine, EngineKind};

    #[test]
    fn what_adding_a_shell_runs_as_root_is_waited_for_however_long_it_takes() {
        let engine = Engine::new(EngineKind::Podman, "/usr/bin/podman");
        let command = super::as_root(&engine, "qcode-shell.p", &["sh", "-c", "tar -cf /tmp/a -T /tmp/l"]);
        assert_eq!(command.deadline, None, "a home of installed packages takes long to archive");
    }
}
