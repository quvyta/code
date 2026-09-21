//! Stopping the containers no QCode owns any more.
//!
//! This is the whole work of the background service, and the one a closing QCode does itself when
//! the service is not installed. It runs once nothing can be using the containers: the caller
//! makes sure of that before calling it. What it does is spelled out as engine commands and run
//! through a runner it is handed, so every decision — what is listed, what is stopped, what stays
//! listed — is checked in tests with no engine anywhere near them.

use std::io;
use std::path::Path;

use qframe::diagnostics::Diagnostic;

use crate::engine::run::EngineError;
use crate::engine::{Container, Engine, EngineCommand, EngineKind};
use crate::store::{Loaded, OnClose, PREFIX, Registered, Registry};

/// What came of looking at the list of QCode's containers.
#[derive(Debug, Clone, PartialEq)]
pub enum Reaped {
    /// The person asked for the containers to be left running, so nothing was touched and the
    /// list was kept.
    Kept,
    /// The list could not be read cleanly, so nothing was stopped and the list was left as it is
    /// for the person to see. These are its problems.
    Broken(Vec<Diagnostic>),
    /// The containers were looked at, and those that ran were stopped.
    Done(Stopping),
}

/// What stopping did, container by container.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stopping {
    /// The containers that were running and are stopped now.
    pub stopped: Vec<String>,
    /// The containers the engine would not stop, with its own words.
    pub failed: Vec<(String, String)>,
    /// The engines that could not be asked which of their containers run, with their words.
    pub unreachable: Vec<(EngineKind, String)>,
    /// What stays listed: every container that could not be stopped or whose engine could not be
    /// asked, so the next run tries again. A container that was not running, or is gone, leaves
    /// the list: there is nothing of it left to stop.
    pub remaining: Registry,
}

/// The commands that stop the containers of `registered` that `listing` shows running in
/// `engine`, one per container, with its name.
///
/// Only names with QCode's own prefix are ever considered, whatever the list says: the list is a
/// file on disk and could hold anything by the time it is read.
#[must_use]
pub fn stop_commands(
    engine: &Engine,
    registered: &[Registered],
    listing: &[Container],
) -> Vec<(String, EngineCommand)> {
    registered
        .iter()
        .filter(|known| known.engine == engine.kind() && known.name.starts_with(PREFIX))
        .filter(|known| listing.iter().any(|container| container.name == known.name && container.state.is_running()))
        .map(|known| (known.name.clone(), engine.stop_container(&known.name)))
        .collect()
}

/// Stops every container of `registry` that is running, when `on_close` says to, and answers
/// what came of it.
///
/// `engine_for` finds the engine of a kind, or `None` when it is not there; `run` runs one
/// command and answers what it printed. The real caller passes [`crate::engine::detect`] and
/// [`crate::engine::run::capture`]; a test passes its own. Stopping is `stop`, never `rm`: the
/// next QCode starts the same container again with everything in it.
pub fn stop_orphans(
    registry: &Loaded<Registry>,
    on_close: OnClose,
    engine_for: &dyn Fn(EngineKind) -> Option<Engine>,
    run: &mut dyn FnMut(&EngineCommand) -> Result<String, EngineError>,
) -> Reaped {
    if on_close == OnClose::Keep {
        return Reaped::Kept;
    }
    if !registry.is_clean() {
        return Reaped::Broken(registry.diagnostics.clone());
    }
    let mut stopping = Stopping::default();
    for kind in [EngineKind::Podman, EngineKind::Docker] {
        let registered: Vec<Registered> =
            registry.value.containers.iter().filter(|known| known.engine == kind).cloned().collect();
        if registered.is_empty() {
            continue;
        }
        let keep_all = |stopping: &mut Stopping| {
            for known in &registered {
                stopping.remaining.add(&known.name, known.engine);
            }
        };
        let Some(engine) = engine_for(kind) else {
            stopping.unreachable.push((kind, String::new()));
            keep_all(&mut stopping);
            continue;
        };
        let listing = match run(&engine.list_containers()) {
            Ok(output) => Container::parse_list(&output),
            Err(error) => {
                stopping.unreachable.push((kind, said(&error)));
                keep_all(&mut stopping);
                continue;
            }
        };
        for (name, command) in stop_commands(&engine, &registered, &listing) {
            match run(&command) {
                Ok(_) => stopping.stopped.push(name),
                Err(error) => {
                    stopping.remaining.add(&name, kind);
                    stopping.failed.push((name, said(&error)));
                }
            }
        }
    }
    Reaped::Done(stopping)
}

/// Reads the list at `path`, stops what [`stop_orphans`] says to, and writes back what stays
/// listed.
///
/// A list that was kept or could not be read is not written at all: the first because the person
/// asked for the containers to stay, the second because it is the person's to see as it is. Nor
/// is a list that comes out holding what it held: a write to this file is what starts the
/// service, so a run that wrote it back unchanged would start the next one, and that the next.
///
/// # Errors
///
/// Returns the I/O error when what stays listed cannot be written.
pub fn reap(
    path: &Path,
    on_close: OnClose,
    engine_for: &dyn Fn(EngineKind) -> Option<Engine>,
    run: &mut dyn FnMut(&EngineCommand) -> Result<String, EngineError>,
) -> io::Result<Reaped> {
    let registry = Registry::load(path);
    // Nothing was ever started, or everything was stopped the last time: there is nothing to ask
    // any engine about and nothing to write.
    if registry.is_clean() && registry.value.is_empty() {
        return Ok(Reaped::Done(Stopping::default()));
    }
    let reaped = stop_orphans(&registry, on_close, engine_for, run);
    if let Reaped::Done(stopping) = &reaped
        && !same_containers(&stopping.remaining, &registry.value)
    {
        Registry::replace(path, &stopping.remaining)?;
    }
    Ok(reaped)
}

/// Whether `a` and `b` list the same containers, in whatever order: what stays listed is
/// gathered engine by engine, so its order can differ from the file's without anything changing.
fn same_containers(a: &Registry, b: &Registry) -> bool {
    a.containers.len() == b.containers.len() && a.containers.iter().all(|known| b.containers.contains(known))
}

/// What the engine said, in one line of words.
fn said(error: &EngineError) -> String {
    match error {
        EngineError::NotRunnable { command, error } => format!("{}: {error}", command.program.display()),
        EngineError::Failed(failure) => failure.output.trim().to_owned(),
        EngineError::Cancelled { command } => command.program.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::engine::ContainerState;
    use crate::engine::run::Failure;

    /// A binary that cannot be run: the tests hand their own runner, so it is only a name.
    const NO_ENGINE: &str = "/nonexistent/qcode-test-engine";

    fn engine(kind: EngineKind) -> Option<Engine> {
        Some(Engine::new(kind, NO_ENGINE))
    }

    fn args(command: &EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    fn registry(names: &[(&str, EngineKind)]) -> Loaded<Registry> {
        let mut registry = Registry::default();
        for (name, kind) in names {
            registry.add(name, *kind);
        }
        Loaded { value: registry, diagnostics: Vec::new() }
    }

    /// A runner that answers `ps` with `listing` and every other command with success, and keeps
    /// every command it was given.
    fn runner<'a>(
        listing: &'a str,
        seen: &'a mut Vec<Vec<String>>,
    ) -> impl FnMut(&EngineCommand) -> Result<String, EngineError> + 'a {
        move |command| {
            let args = args(command);
            let answer = if args[0] == "ps" { listing.to_owned() } else { String::new() };
            seen.push(args);
            Ok(answer)
        }
    }

    #[test]
    fn only_registered_running_qcode_containers_are_stopped() {
        let podman = Engine::new(EngineKind::Podman, NO_ENGINE);
        let registered = registry(&[
            ("qcode-a-base", EngineKind::Podman),
            ("qcode-a-codex", EngineKind::Podman),
            ("qcode-b-base", EngineKind::Docker),
        ])
        .value
        .containers;
        let mut foreign = registered.clone();
        // A name that slipped into the list without the prefix is still never acted on.
        foreign.push(Registered { name: "postgres".to_owned(), engine: EngineKind::Podman });
        let listing = Container::parse_list(
            "qcode-a-base\trunning\nqcode-a-codex\texited\npostgres\trunning\nqcode-c-base\trunning\n",
        );
        let commands = stop_commands(&podman, &foreign, &listing);
        let spelled: Vec<(String, Vec<String>)> =
            commands.iter().map(|(name, command)| (name.clone(), args(command))).collect();
        assert_eq!(spelled, [("qcode-a-base".to_owned(), vec!["stop".to_owned(), "qcode-a-base".to_owned()])]);
        assert_eq!(commands[0].1.program, PathBuf::from(NO_ENGINE));
    }

    #[test]
    fn stopping_stops_what_runs_and_empties_the_list() {
        let list = registry(&[("qcode-a-base", EngineKind::Podman), ("qcode-a-codex", EngineKind::Podman)]);
        let mut seen = Vec::new();
        let reaped = stop_orphans(
            &list,
            OnClose::Stop,
            &engine,
            &mut runner("qcode-a-base\trunning\nqcode-a-codex\texited\n", &mut seen),
        );
        assert_eq!(seen, [vec!["ps", "--all", "--format", "{{.Names}}\t{{.State}}"], vec!["stop", "qcode-a-base"],]);
        let Reaped::Done(stopping) = reaped else { panic!("the list was clean and stopping was asked for") };
        assert_eq!(stopping.stopped, ["qcode-a-base"]);
        assert!(stopping.remaining.is_empty(), "the stopped one and the one that was not running both leave the list");
    }

    #[test]
    fn keeping_touches_nothing() {
        let list = registry(&[("qcode-a-base", EngineKind::Podman)]);
        let mut seen = Vec::new();
        let reaped = stop_orphans(&list, OnClose::Keep, &engine, &mut runner("qcode-a-base\trunning\n", &mut seen));
        assert_eq!(reaped, Reaped::Kept);
        assert!(seen.is_empty(), "not even a question is asked: {seen:?}");
    }

    #[test]
    fn a_broken_list_stops_nothing() {
        let mut list = registry(&[("qcode-a-base", EngineKind::Podman)]);
        list.diagnostics.push(Diagnostic::error(None, "containers.toml:3:1 unexpected"));
        let mut seen = Vec::new();
        let reaped = stop_orphans(&list, OnClose::Stop, &engine, &mut runner("qcode-a-base\trunning\n", &mut seen));
        assert!(matches!(reaped, Reaped::Broken(ref problems) if problems.len() == 1), "{reaped:?}");
        assert!(seen.is_empty(), "{seen:?}");
    }

    #[test]
    fn each_engine_is_asked_about_its_own_containers() {
        let list = registry(&[("qcode-a-base", EngineKind::Podman), ("qcode-b-base", EngineKind::Docker)]);
        let mut seen = Vec::new();
        let mut run = |command: &EngineCommand| {
            seen.push((command.program.clone(), args(command)));
            Ok(if args(command)[0] == "ps" {
                "qcode-a-base\trunning\nqcode-b-base\trunning\n".to_owned()
            } else {
                String::new()
            })
        };
        let engines = |kind: EngineKind| {
            Some(Engine::new(kind, if kind == EngineKind::Podman { "/bin/podman-x" } else { "/bin/docker-x" }))
        };
        let Reaped::Done(stopping) = stop_orphans(&list, OnClose::Stop, &engines, &mut run) else {
            panic!("stopping was asked for")
        };
        assert_eq!(stopping.stopped, ["qcode-a-base", "qcode-b-base"]);
        let stops: Vec<(PathBuf, String)> = seen
            .iter()
            .filter(|(_, args)| args[0] == "stop")
            .map(|(program, args)| (program.clone(), args[1].clone()))
            .collect();
        assert_eq!(
            stops,
            [
                (PathBuf::from("/bin/podman-x"), "qcode-a-base".to_owned()),
                (PathBuf::from("/bin/docker-x"), "qcode-b-base".to_owned()),
            ]
        );
    }

    #[test]
    fn what_could_not_be_stopped_stays_listed_for_the_next_time() {
        let list = registry(&[
            ("qcode-a-base", EngineKind::Podman),
            ("qcode-a-codex", EngineKind::Podman),
            ("qcode-b-base", EngineKind::Docker),
        ]);
        let mut run = |command: &EngineCommand| {
            let args = args(command);
            match (args[0].as_str(), args.get(1).map(String::as_str)) {
                ("ps", _) => Ok("qcode-a-base\trunning\nqcode-a-codex\trunning\n".to_owned()),
                ("stop", Some("qcode-a-codex")) => Err(EngineError::Failed(Failure {
                    command: command.clone(),
                    code: Some(125),
                    output: "container is paused\n".to_owned(),
                })),
                _ => Ok(String::new()),
            }
        };
        // Docker is not there at all.
        let engines = |kind: EngineKind| (kind == EngineKind::Podman).then(|| Engine::new(kind, NO_ENGINE));
        let Reaped::Done(stopping) = stop_orphans(&list, OnClose::Stop, &engines, &mut run) else {
            panic!("stopping was asked for")
        };
        assert_eq!(stopping.stopped, ["qcode-a-base"]);
        assert_eq!(stopping.failed, [("qcode-a-codex".to_owned(), "container is paused".to_owned())]);
        assert_eq!(stopping.unreachable, [(EngineKind::Docker, String::new())]);
        let left: Vec<&str> = stopping.remaining.containers.iter().map(|known| known.name.as_str()).collect();
        assert_eq!(left, ["qcode-a-codex", "qcode-b-base"]);
    }

    #[test]
    fn an_engine_that_cannot_list_keeps_all_of_its_containers_listed() {
        let list = registry(&[("qcode-a-base", EngineKind::Podman)]);
        let mut run = |command: &EngineCommand| {
            Err(EngineError::NotRunnable { command: command.clone(), error: io::Error::from(io::ErrorKind::NotFound) })
        };
        let Reaped::Done(stopping) = stop_orphans(&list, OnClose::Stop, &engine, &mut run) else {
            panic!("stopping was asked for")
        };
        assert!(stopping.stopped.is_empty());
        assert_eq!(stopping.unreachable.len(), 1);
        assert_eq!(stopping.remaining.containers.len(), 1);
    }

    #[test]
    fn reaping_writes_back_only_what_stays_and_leaves_a_kept_or_broken_list_alone() {
        let dir = std::env::temp_dir().join(format!("qcode-reap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("containers.toml");

        let mut seen = Vec::new();
        let nothing = reap(&path, OnClose::Stop, &engine, &mut runner("", &mut seen)).expect("nothing to write");
        assert_eq!(nothing, Reaped::Done(Stopping::default()));
        assert!(!path.exists() && seen.is_empty(), "no list, no question and no file");

        Registry::record(&path, "qcode-a-base", EngineKind::Podman).expect("written");
        Registry::record(&path, "qcode-a-codex", EngineKind::Podman).expect("written");
        let listing = "qcode-a-base\trunning\nqcode-a-codex\texited\n";
        assert_eq!(reap(&path, OnClose::Keep, &engine, &mut runner(listing, &mut seen)).expect("read"), Reaped::Kept);
        assert_eq!(Registry::load(&path).value.containers.len(), 2, "a kept list is not emptied");

        reap(&path, OnClose::Stop, &engine, &mut runner(listing, &mut seen)).expect("written");
        let after = Registry::load(&path);
        assert!(after.is_clean() && after.value.is_empty(), "{after:?}");
        assert_eq!(std::fs::read_to_string(&path).expect("the file stays"), "");

        std::fs::write(&path, "[[container]]\nname = \n").expect("a broken list");
        let broken = reap(&path, OnClose::Stop, &engine, &mut runner(listing, &mut seen)).expect("read");
        assert!(matches!(broken, Reaped::Broken(_)), "{broken:?}");
        assert_eq!(std::fs::read_to_string(&path).expect("untouched"), "[[container]]\nname = \n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_list_that_comes_out_the_same_is_not_written_again() {
        // The service is started by a write to this file; a run that writes the file it was
        // started by, with nothing changed, would start the next run, and that one the next.
        use std::os::unix::fs::MetadataExt as _;
        let dir = std::env::temp_dir().join(format!("qcode-reap-same-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("containers.toml");
        let inode = || std::fs::metadata(&path).expect("the list is there").ino();

        Registry::record(&path, "qcode-a-base", EngineKind::Podman).expect("written");
        let before = inode();
        // The engine is not there, so the container stays listed as it was.
        let reaped = reap(&path, OnClose::Stop, &|_| None, &mut |_| Ok(String::new())).expect("read");
        assert!(matches!(reaped, Reaped::Done(ref stopping) if stopping.remaining.containers.len() == 1));
        assert_eq!(inode(), before, "the same list is not written again");

        Registry::replace(&path, &Registry::default()).expect("emptied");
        let before = inode();
        let mut seen = Vec::new();
        reap(&path, OnClose::Stop, &engine, &mut runner("", &mut seen)).expect("read");
        assert_eq!(inode(), before, "an empty list is not written again");
        assert!(seen.is_empty(), "and no engine is asked about nothing: {seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_state_the_engine_calls_by_another_word_is_not_running() {
        let podman = Engine::new(EngineKind::Podman, NO_ENGINE);
        let registered = registry(&[("qcode-a-base", EngineKind::Podman)]).value.containers;
        let listing = [Container { name: "qcode-a-base".to_owned(), state: ContainerState::Paused }];
        assert!(stop_commands(&podman, &registered, &listing).is_empty());
    }
}
