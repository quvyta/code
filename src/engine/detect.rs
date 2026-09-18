//! Finding out whether an engine is there and working.
//!
//! Two questions, and they are not the same one. Is the binary there at all, and does it answer?
//! Docker is often installed while its daemon is stopped, and on macOS and Windows podman is
//! often installed while its Linux virtual machine is not started. Neither of those is "no
//! engine": the person has already done the installing and only has to start something, so they
//! are told that and not something else.

use super::command::EngineCommand;
use super::run::{EngineError, capture};
use super::{Engine, EngineKind};
use std::path::{Path, PathBuf};

/// Why an engine cannot be used. Each one asks something different of the person, so each one is
/// kept apart; the words are in the language files.
#[derive(Debug)]
pub enum Unavailable {
    /// No binary anywhere QCode looked. The person has to install the engine.
    NotInstalled,
    /// Docker is installed and its daemon is not answering. The person has to start Docker.
    DaemonStopped {
        /// What docker said, for the person to read.
        output: String,
    },
    /// Podman is installed and the Linux virtual machine it needs on macOS and Windows is not
    /// running. The person has to start the machine.
    MachineStopped {
        /// What podman said, for the person to read.
        output: String,
    },
    /// The engine ran and refused for some other reason.
    InfoFailed {
        /// Its exit code, when it had one.
        code: Option<i32>,
        /// What it said.
        output: String,
    },
    /// A binary was found and could not be started.
    NotRunnable {
        /// The binary QCode found.
        bin: PathBuf,
        /// What the operating system said.
        error: std::io::Error,
    },
}

/// Looks for `kind` and makes sure it works.
///
/// # Errors
///
/// When the engine is missing, or is there and not working, with the two told apart.
pub fn detect(kind: EngineKind) -> Result<Engine, Unavailable> {
    let name = format!("{}{}", kind.dialect().binary, std::env::consts::EXE_SUFFIX);
    let Some(bin) = lookup(&name, &search_dirs(), &|path| path.is_file()) else {
        return Err(Unavailable::NotInstalled);
    };
    let engine = Engine::new(kind, bin);
    match capture(&engine.info()) {
        Ok(_) => Ok(engine),
        Err(EngineError::Failed(failure)) => Err(classify(kind, failure.code, &failure.output)),
        Err(EngineError::NotRunnable { command, error }) => {
            Err(Unavailable::NotRunnable { bin: command.program, error })
        }
        // `capture` never cancels: nothing was given to it to cancel with.
        Err(EngineError::Cancelled { .. }) => Err(Unavailable::InfoFailed { code: None, output: String::new() }),
    }
}

impl Engine {
    /// Asks the engine to describe itself. It answers only when it is really working, which is
    /// what makes it the question [`detect`] asks.
    #[must_use]
    pub fn info(&self) -> EngineCommand {
        EngineCommand { program: self.bin.clone(), args: vec!["info".into()] }
    }
}

/// Reads a failed `info` and works out what the person has to do about it.
///
/// The matching is on what the engines say when what they need is not running. The words are
/// theirs, and they are matched loosely on purpose: a new release rewording its sentence should
/// cost the person a vaguer message, never a wrong one, so anything unrecognised stays
/// [`Unavailable::InfoFailed`] with the engine's own text in it.
fn classify(kind: EngineKind, code: Option<i32>, output: &str) -> Unavailable {
    let said = output.to_lowercase();
    let says = |marks: &[&str]| marks.iter().any(|mark| said.contains(mark));
    match kind {
        EngineKind::Docker
            if says(&[
                "cannot connect to the docker daemon",
                "failed to connect to the docker api",
                "is the docker daemon running",
                "error during connect",
                "docker_engine",
                "dockerdesktoplinuxengine",
            ]) =>
        {
            Unavailable::DaemonStopped { output: output.to_owned() }
        }
        EngineKind::Podman if says(&["cannot connect to podman", "podman machine start", "podman machine init"]) => {
            Unavailable::MachineStopped { output: output.to_owned() }
        }
        _ => Unavailable::InfoFailed { code, output: output.to_owned() },
    }
}

/// Finds `name` in `dirs`, in order, asking `exists` about each place.
///
/// `exists` is a parameter so that the search itself is checked without a binary having to be
/// installed on the machine running the test.
fn lookup(name: &str, dirs: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(name)).find(|candidate| exists(candidate))
}

/// Where to look: what the person's `PATH` says, then the places each platform puts an engine
/// that `PATH` may not mention.
///
/// The extra places are not a nicety. A terminal started from a desktop launcher on macOS
/// inherits a `PATH` with neither Homebrew directory in it, and Docker Desktop on Windows puts
/// its binary somewhere only its own installer knows.
fn search_dirs() -> Vec<PathBuf> {
    let on_path: Vec<PathBuf> =
        std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
    on_path.into_iter().chain(platform_dirs()).collect()
}

/// The places this platform's installers use, on top of `PATH`.
fn platform_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/Applications/Docker.app/Contents/Resources/bin"),
        ]
    }
    #[cfg(windows)]
    {
        let Some(files) = std::env::var_os("ProgramFiles").map(PathBuf::from) else { return Vec::new() };
        vec![files.join("Docker").join("Docker").join("resources").join("bin"), files.join("RedHat").join("Podman")]
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // A Linux package manager puts the binary where `PATH` already points.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Unavailable, classify, lookup, search_dirs};
    use crate::engine::{Engine, EngineKind};
    use std::path::{Path, PathBuf};

    #[test]
    fn asks_the_engine_to_describe_itself() {
        let command = Engine::new(EngineKind::Podman, "/usr/bin/podman").info();
        assert_eq!(command.args, ["info"]);
    }

    #[test]
    fn takes_the_first_place_that_has_the_binary() {
        let dirs = [PathBuf::from("/opt/nothing"), PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin")];
        let found = lookup("podman", &dirs, &|path| path == Path::new("/usr/bin/podman"));
        assert_eq!(found, Some(PathBuf::from("/usr/bin/podman")));
    }

    #[test]
    fn finds_nothing_when_no_place_has_it() {
        assert_eq!(lookup("podman", &[PathBuf::from("/usr/bin")], &|_| false), None);
    }

    #[test]
    fn the_path_is_looked_through() {
        let dirs = search_dirs();
        let from_path: Vec<PathBuf> =
            std::env::split_paths(&std::env::var_os("PATH").expect("a shell sets PATH")).collect();
        assert!(from_path.iter().all(|dir| dirs.contains(dir)));
    }

    #[test]
    fn a_stopped_docker_daemon_is_told_apart_from_a_missing_docker() {
        // What docker 29 says when the socket it is pointed at is not there.
        let said = "failed to connect to the docker API at unix:///run/docker.sock; check if the path is \
                    correct and if the daemon is running: dial unix /run/docker.sock: connect: no such file";
        assert!(matches!(classify(EngineKind::Docker, Some(1), said), Unavailable::DaemonStopped { .. }));
        // What earlier docker releases say, and what Docker Desktop on Windows says.
        let classic = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. \
                       Is the docker daemon running?";
        assert!(matches!(classify(EngineKind::Docker, Some(1), classic), Unavailable::DaemonStopped { .. }));
        let desktop = "error during connect: Get \"http://%2F%2F.%2Fpipe%2FdockerDesktopLinuxEngine/v1.51/info\": \
                       open //./pipe/dockerDesktopLinuxEngine: The system cannot find the file specified.";
        assert!(matches!(classify(EngineKind::Docker, Some(1), desktop), Unavailable::DaemonStopped { .. }));
    }

    #[test]
    fn a_podman_machine_that_is_not_running_is_its_own_answer() {
        // What podman says on macOS and Windows when its virtual machine is not up.
        let said = "Cannot connect to Podman. Please verify your connection to the Linux system using \
                    `podman system connection list`, or try `podman machine init` and `podman machine start` \
                    to manage a new Linux VM";
        assert!(matches!(classify(EngineKind::Podman, Some(125), said), Unavailable::MachineStopped { .. }));
    }

    #[test]
    fn an_unfamiliar_refusal_keeps_the_engines_own_words() {
        let said = "Error: short-name resolution enforced but cannot prompt without a TTY";
        let Unavailable::InfoFailed { code, output } = classify(EngineKind::Podman, Some(125), said) else {
            panic!("an unknown refusal is not one of the known ones")
        };
        assert_eq!(code, Some(125));
        assert_eq!(output, said);
    }

    #[test]
    fn a_daemon_message_is_not_read_into_the_other_engine() {
        let said = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.";
        assert!(matches!(classify(EngineKind::Podman, Some(1), said), Unavailable::InfoFailed { .. }));
    }
}
