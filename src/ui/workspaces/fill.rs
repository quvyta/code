//! Filling a workspace that has just been made: copying a folder into it, or cloning a git
//! address inside a container.
//!
//! Both run on a task's thread, report every line they are told and stop when the person asks.
//! Whatever happens, they leave nothing half-made: a run that fails or is stopped removes the
//! workspace folder it was filling, so the list never grows a workspace that is not a workspace.
//!
//! No text here is shown as itself. A failure carries what the file system or the engine said,
//! which the screen shows under a sentence of its own from the language files; translating on
//! this thread is not possible anyway, because the translator belongs to the drawing thread.

use std::fs;
use std::path::{Path, PathBuf};

use qframe::runtime::TaskCx;

use crate::base::{self, paths::CODE_DIR, paths::KEEP_ALIVE};
use crate::engine::run::{EngineError, capture, stream};
use crate::engine::{
    Access, ContainerCreate, Engine, EngineCommand, Exec, HostUser, Mount, MountSource, Network, names,
};

use super::Msg;

/// How a new workspace's `Work/` folder is filled.
#[derive(Debug)]
pub enum Fill {
    /// Copy a folder of the person's own. Their folder is only read.
    Copy {
        /// The folder to copy.
        from: PathBuf,
    },
    /// Clone a git address inside a container, so the person's machine needs no git.
    Clone {
        /// The engine the container runs on.
        engine: Engine,
        /// The address to clone.
        url: String,
    },
}

/// One filling run: what to do, and everything it needs to undo itself.
#[derive(Debug)]
pub struct Job {
    /// What to do.
    pub fill: Fill,
    /// The workspace's own folder, removed whole when anything goes wrong.
    pub root: PathBuf,
    /// The folder the content lands in.
    pub into: PathBuf,
    /// The workspace's identifier, which names the container the clone borrows.
    pub id: String,
}

/// Runs `job`, sending every line it hears to the screen.
///
/// # Errors
///
/// What the file system or the engine said, when the work did not finish.
pub fn run(job: &Job, cx: &TaskCx<Msg>) -> Result<Msg, String> {
    let outcome = match &job.fill {
        Fill::Copy { from } => copy_tree(from, &job.into, cx),
        Fill::Clone { engine, url } => clone(engine, url, &job.into, &job.id, cx),
    };
    // Cancelling is not an error and still leaves a workspace that was never filled, so the
    // rollback asks about both.
    if outcome.is_err() || cx.is_cancelled() {
        let _ = fs::remove_dir_all(&job.root);
    }
    outcome.map(|()| Msg::Filled)
}

/// Copies everything below `from` into `into`, one folder at a time.
///
/// Symbolic links are named and skipped rather than followed: a link into a parent folder would
/// make the copy endless, and a link out of the tree would copy something the person did not
/// choose. Stopping is checked before every folder, so a stop is noticed even in a deep tree.
fn copy_tree(from: &Path, into: &Path, cx: &TaskCx<Msg>) -> Result<(), String> {
    let mut todo = vec![(from.to_path_buf(), into.to_path_buf())];
    while let Some((source, target)) = todo.pop() {
        if cx.is_cancelled() {
            return Ok(());
        }
        cx.note(source.display().to_string());
        let entries = fs::read_dir(&source).map_err(|error| at(&source, &error))?;
        fs::create_dir_all(&target).map_err(|error| at(&target, &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| at(&source, &error))?;
            let kind = entry.file_type().map_err(|error| at(&source, &error))?;
            let (source, target) = (entry.path(), target.join(entry.file_name()));
            if kind.is_symlink() {
                cx.send(Msg::Line(format!("{}: a link, left where it is", source.display())));
            } else if kind.is_dir() {
                todo.push((source, target));
            } else if kind.is_file() {
                fs::copy(&source, &target).map_err(|error| at(&source, &error))?;
                cx.send(Msg::Line(target.display().to_string()));
            } else {
                cx.send(Msg::Line(format!("{}: neither a file nor a folder, left where it is", source.display())));
            }
        }
    }
    Ok(())
}

/// The two engine commands a clone is made of: the container the clone borrows, and the clone
/// itself.
///
/// Spelled out apart from running them so that what is asked of the engine can be read on a
/// machine without one.
struct ClonePlan {
    /// The container's name, which is the workspace's own base container.
    container: String,
    /// Creates the container, stopped.
    create: EngineCommand,
    /// Runs the clone inside it.
    exec: EngineCommand,
}

impl ClonePlan {
    /// Plans a clone of `url` into `into`, in the base container of the workspace `id`.
    fn new(engine: &Engine, url: &str, into: &Path, id: &str, user: HostUser) -> Self {
        let container = names::base_container(id);
        // The workspace folder lands where every later container will see it, so the clone is
        // already in the place the contract names rather than moved there afterwards.
        let workdir = Path::new(CODE_DIR);
        let mounts = [Mount { source: MountSource::Path(into), target: workdir, access: Access::ReadWrite }];
        let create = engine.create_container(&ContainerCreate {
            name: &container,
            hostname: names::HOSTNAME,
            labels: &[],
            image: names::BASE_IMAGE,
            mounts: &mounts,
            // A clone reaches the network by definition; a profile's own setting is another matter.
            network: Network::Full,
            user,
            workdir: Some(workdir),
            command: KEEP_ALIVE,
        });
        // `--` keeps an address that begins with a dash from reaching git as an option. The
        // dialog refuses one anyway; both hold, because only one of them is in sight when reading
        // either. No terminal is asked for: the lines are read from a pipe, and docker refuses a
        // terminal when nothing behind it is one.
        let command = ["git", "clone", "--progress", "--", url, "."];
        let exec = engine.exec_without_terminal(&Exec { container: &container, command: &command });
        Self { container, create, exec }
    }
}

/// Clones `url` into `into` from inside a container.
///
/// The container is the workspace's own base container: nothing new has to be named for this, and
/// it is removed again afterwards so the workspace screen creates it later with the mounts it
/// really wants. The workspace folder is mounted read-write and is the working folder, so the
/// clone lands in it directly and belongs to the person, not to the container.
///
/// The base image is made sure of first, and a build of it is shown in the same log as the
/// clone: on a machine that has never built one, the clone is the first thing to need it.
fn clone(engine: &Engine, url: &str, into: &Path, id: &str, cx: &TaskCx<Msg>) -> Result<(), String> {
    let user = HostUser::current().map_err(|error| format!("{error}"))?;
    let plan = ClonePlan::new(engine, url, into, id, user);
    let cancel = || cx.is_cancelled();
    let mut line = |line: &str| cx.send(Msg::Line(line.to_owned()));
    match base::ensure(engine, &cancel, &mut line) {
        Ok(_) => {}
        Err(base::Failure::Host(error)) => return Err(error.to_string()),
        Err(base::Failure::Engine(error)) => return Err(reason(error)),
    }
    let result = capture(&plan.create)
        .and_then(|_| capture(&engine.start_container(&plan.container)))
        .map_err(reason)
        .and_then(|_| stream(&plan.exec, &cancel, &mut line).map_err(reason));
    // The container has done all it was for, whether the clone worked or not.
    let _ = capture(&engine.remove_container(&plan.container));
    result
}

/// What the engine said, as the person is shown it.
fn reason(error: EngineError) -> String {
    match error {
        EngineError::NotRunnable { command, error } => format!("{}: {error}", command.program.display()),
        EngineError::Failed(failure) if !failure.output.trim().is_empty() => failure.output,
        EngineError::Failed(failure) => match failure.code {
            Some(code) => format!("{}: {code}", failure.command.program.display()),
            None => failure.command.program.display().to_string(),
        },
        // A stopped run is not a failure; `run` has already seen that it was cancelled.
        EngineError::Cancelled { .. } => String::new(),
    }
}

/// What the file system said about a path, in the shape every loader in QCode reports.
fn at(path: &Path, error: &std::io::Error) -> String {
    format!("{}: {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineKind;

    fn words(command: &EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_clone_lands_where_every_later_container_looks_for_the_workspace() {
        let engine = Engine::new(EngineKind::Docker, "/usr/bin/docker");
        let plan = ClonePlan::new(
            &engine,
            "https://example.org/team/app.git",
            Path::new("/w/Work"),
            "app",
            HostUser::Ids { uid: 1500, gid: 1500 },
        );
        let create = words(&plan.create);
        assert_eq!(plan.container, "qcode-app-base");
        assert!(create.iter().any(|word| word == &format!("/w/Work:{CODE_DIR}:rw")), "{create:?}");
        assert!(create.windows(2).any(|pair| pair[0] == "--workdir" && pair[1] == CODE_DIR), "{create:?}");
        assert!(create.ends_with(&KEEP_ALIVE.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>()), "{create:?}");
    }

    #[test]
    fn the_clone_asks_for_no_terminal_because_its_lines_are_read_from_a_pipe() {
        // docker refuses `--tty` when standard input is not a terminal, and on a task thread it
        // never is; the clone's progress is read line by line instead.
        let engine = Engine::new(EngineKind::Docker, "/usr/bin/docker");
        let plan = ClonePlan::new(
            &engine,
            "https://example.org/team/app.git",
            Path::new("/w/Work"),
            "app",
            HostUser::Ids { uid: 1500, gid: 1500 },
        );
        let exec = words(&plan.exec);
        assert_eq!(
            exec,
            ["exec", "qcode-app-base", "git", "clone", "--progress", "--", "https://example.org/team/app.git", "."]
        );
    }
}
