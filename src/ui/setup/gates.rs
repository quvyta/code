//! The gates of the setup wizard: what each step needs before the wizard may leave it.
//!
//! The rule of the design is that the wizard never jumps to the step the config file
//! remembers. Every gate is asked in the order of the steps, and the first one that does not
//! hold is where the wizard opens. [`Gates`] is plain data and [`Gates::entry`] is a pure
//! function of it, so a test hands in any answer it likes without a disk or a container engine
//! anywhere near it. Only [`Gates::probe`], [`check_engine`] and [`check_location`] touch the
//! machine.

use std::path::{Path, PathBuf};

use crate::engine::{EngineKind, Unavailable, detect};
use crate::workspace::{Config, SetupStep, Workspace};

/// Why the chosen engine cannot be used.
///
/// The same distinctions the engine layer makes, kept in a shape the screen can hold on to and
/// show: each one asks something different of the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineProblem {
    /// No binary anywhere QCode looked; the engine has to be installed.
    NotInstalled,
    /// Docker is installed and its daemon is not answering; it has to be started.
    DaemonStopped {
        /// What docker said.
        output: String,
    },
    /// Podman is installed and the Linux virtual machine it works through is not running.
    MachineStopped {
        /// What podman said.
        output: String,
    },
    /// The engine ran and refused for a reason QCode does not recognise.
    Refused {
        /// Its exit code, when it had one.
        code: Option<i32>,
        /// What it said.
        output: String,
    },
    /// A binary was found and could not be started at all.
    NotRunnable {
        /// The binary QCode found.
        bin: PathBuf,
        /// What the operating system said about starting it.
        message: String,
    },
}

impl EngineProblem {
    /// The engine's own words, when it had any. They are shown unchanged: a refusal QCode
    /// cannot read is still readable by the person.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        match self {
            Self::NotInstalled => None,
            Self::DaemonStopped { output } | Self::MachineStopped { output } | Self::Refused { output, .. } => {
                Some(output).filter(|text| !text.is_empty()).map(String::as_str)
            }
            Self::NotRunnable { message, .. } => Some(message),
        }
    }
}

impl From<Unavailable> for EngineProblem {
    fn from(unavailable: Unavailable) -> Self {
        match unavailable {
            Unavailable::NotInstalled => Self::NotInstalled,
            Unavailable::DaemonStopped { output } => Self::DaemonStopped { output },
            Unavailable::MachineStopped { output } => Self::MachineStopped { output },
            Unavailable::InfoFailed { code, output } => Self::Refused { code, output },
            Unavailable::NotRunnable { bin, error } => Self::NotRunnable { bin, message: error.to_string() },
        }
    }
}

/// What the machine answered when the chosen engine was asked whether it works.
///
/// It is the engine step's whole state as well as its gate: nothing asked yet, being asked,
/// working, or broken with the reason.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EngineCheck {
    /// Nobody has asked yet.
    #[default]
    Unknown,
    /// The question is out; the answer has not come back.
    Running,
    /// The engine answered: it is there and it works.
    Working,
    /// The engine cannot be used.
    Broken(EngineProblem),
}

/// Why the workspace folder cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationProblem {
    /// No folder has been chosen yet.
    Unset,
    /// The path names something that is not a folder.
    NotAFolder,
    /// The folder could not be made, read or written, in the system's own words.
    Blocked(String),
}

/// What the machine answered about the workspace folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LocationCheck {
    /// Nobody has asked yet.
    #[default]
    Unknown,
    /// The question is out; the answer has not come back.
    Running,
    /// The folder is there and QCode can write in it.
    Usable,
    /// The folder cannot be used.
    Broken(LocationProblem),
}

/// What each step of the wizard needs, in the order the steps are walked.
///
/// This is data on purpose. [`entry`](Self::entry) reads it and nothing else, so every path
/// through the wizard's opening rule is a test that needs neither a disk nor an engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gates {
    /// Whether the config file names a language QCode has words for.
    pub language: bool,
    /// What the chosen engine answered.
    pub engine: EngineCheck,
    /// What the workspace folder answered.
    pub location: LocationCheck,
}

impl Gates {
    /// The step the wizard opens on: the first gate that does not hold, or `None` when they all
    /// hold and the wizard has nothing left to ask.
    ///
    /// The step the config file remembers is never jumped to. A person who got as far as the
    /// last step and then lost their engine opens on the engine step again.
    #[must_use]
    pub fn entry(&self) -> Option<SetupStep> {
        if !self.language {
            return Some(SetupStep::Language);
        }
        if self.engine != EngineCheck::Working {
            return Some(SetupStep::Engine);
        }
        if self.location != LocationCheck::Usable {
            return Some(SetupStep::Location);
        }
        None
    }

    /// Asks the machine every gate of `config`, in order, and stops at the first one that does
    /// not hold: the gates after it stay [`Unknown`](EngineCheck::Unknown), because their
    /// answer could not change where the wizard opens and asking costs a container engine or a
    /// disk.
    ///
    /// Blocks on both, so it belongs on a background thread, never in `view`.
    #[must_use]
    pub fn probe(config: &Config) -> Self {
        let mut gates = Self {
            language: config.settings().language().is_some(),
            engine: EngineCheck::Unknown,
            location: LocationCheck::Unknown,
        };
        if !gates.language {
            return gates;
        }
        gates.engine = match config.engine_kind().and_then(EngineKind::from_name) {
            Some(kind) => check_engine(kind),
            None => EngineCheck::Unknown,
        };
        if gates.engine != EngineCheck::Working {
            return gates;
        }
        gates.location = match config.workspace_path() {
            Some(path) => check_location(&path),
            None => LocationCheck::Broken(LocationProblem::Unset),
        };
        gates
    }
}

/// Asks `kind` whether it is installed and working.
///
/// Blocks on starting the engine, so it belongs on a background thread.
#[must_use]
pub fn check_engine(kind: EngineKind) -> EngineCheck {
    match detect(kind) {
        Ok(_) => EngineCheck::Working,
        Err(unavailable) => EngineCheck::Broken(EngineProblem::from(unavailable)),
    }
}

/// Makes sure the workspace at `path` is there and can be written in.
///
/// The folder is made when it is missing: placing the workspace is the wizard's whole job, and
/// the same call at every later start repairs a workspace whose folders were removed. Writing
/// is proven by writing, not guessed from permission bits, because a read-only mount and a full
/// disk both look writable until something is written.
///
/// Blocks on the file system, so it belongs on a background thread.
#[must_use]
pub fn check_location(path: &Path) -> LocationCheck {
    if path.is_file() {
        return LocationCheck::Broken(LocationProblem::NotAFolder);
    }
    let workspace = Workspace::new(path);
    if let Err(diagnostic) = workspace.prepare() {
        return LocationCheck::Broken(LocationProblem::Blocked(diagnostic.message));
    }
    let probe = path.join(format!(".qcode-write-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            LocationCheck::Usable
        }
        Err(error) => LocationCheck::Broken(LocationProblem::Blocked(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Config, SetupStep};

    fn working() -> Gates {
        Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Usable }
    }

    #[test]
    fn a_wizard_whose_gates_all_hold_has_no_entry_step() {
        assert_eq!(working().entry(), None);
    }

    #[test]
    fn the_language_gate_is_asked_first() {
        let gates = Gates { language: false, ..working() };
        assert_eq!(gates.entry(), Some(SetupStep::Language));
    }

    #[test]
    fn a_broken_engine_is_the_entry_even_when_every_later_gate_holds() {
        let gates = Gates { engine: EngineCheck::Broken(EngineProblem::NotInstalled), ..working() };
        assert_eq!(gates.entry(), Some(SetupStep::Engine));
    }

    #[test]
    fn an_engine_that_was_never_asked_does_not_pass_its_gate() {
        let gates = Gates { engine: EngineCheck::Unknown, ..working() };
        assert_eq!(gates.entry(), Some(SetupStep::Engine));
        let gates = Gates { engine: EngineCheck::Running, ..working() };
        assert_eq!(gates.entry(), Some(SetupStep::Engine));
    }

    #[test]
    fn an_unusable_location_is_the_entry_when_everything_before_it_holds() {
        let gates = Gates { location: LocationCheck::Broken(LocationProblem::Unset), ..working() };
        assert_eq!(gates.entry(), Some(SetupStep::Location));
    }

    #[test]
    fn a_recorded_step_is_never_jumped_to_over_a_gate_that_fell() {
        // The config may say the person got as far as the location; the engine gate still wins.
        let gates = Gates {
            language: true,
            engine: EngineCheck::Broken(EngineProblem::DaemonStopped { output: "no socket".to_owned() }),
            location: LocationCheck::Usable,
        };
        assert_eq!(gates.entry(), Some(SetupStep::Engine));
    }

    #[test]
    fn probing_a_fresh_config_stops_at_the_language_gate() {
        // Nothing after the first gate that falls is asked, so no engine is started and no disk
        // is touched by this test.
        let gates = Gates::probe(&Config::parse_str("code.conf", ""));
        assert!(!gates.language);
        assert_eq!(gates.engine, EngineCheck::Unknown);
        assert_eq!(gates.location, LocationCheck::Unknown);
        assert_eq!(gates.entry(), Some(SetupStep::Language));
    }

    #[test]
    fn probing_a_config_without_an_engine_stops_at_the_engine_gate() {
        let gates = Gates::probe(&Config::parse_str("code.conf", "language = \"tr\"\n"));
        assert!(gates.language);
        assert_eq!(gates.engine, EngineCheck::Unknown);
        assert_eq!(gates.location, LocationCheck::Unknown);
    }

    #[test]
    fn a_folder_that_can_be_made_and_written_passes_the_location_gate() {
        let dir = std::env::temp_dir().join(format!("qcode-setup-usable-{}", std::process::id()));
        assert_eq!(check_location(&dir), LocationCheck::Usable);
        assert!(dir.join("Projects").is_dir(), "the workspace tree is made while it is checked");
        std::fs::remove_dir_all(&dir).expect("the test cleans up after itself");
    }

    #[test]
    fn a_path_that_names_a_file_is_not_a_folder() {
        let file = std::env::temp_dir().join(format!("qcode-setup-file-{}", std::process::id()));
        std::fs::write(&file, "not a folder").expect("the temporary directory takes a file");
        assert_eq!(check_location(&file), LocationCheck::Broken(LocationProblem::NotAFolder));
        std::fs::remove_file(&file).expect("the test cleans up after itself");
    }

    #[test]
    fn a_folder_that_cannot_be_made_carries_the_systems_own_reason() {
        let file = std::env::temp_dir().join(format!("qcode-setup-block-{}", std::process::id()));
        std::fs::write(&file, "in the way").expect("the temporary directory takes a file");
        let LocationCheck::Broken(LocationProblem::Blocked(reason)) = check_location(&file.join("QCode")) else {
            panic!("a folder below a file cannot be made")
        };
        assert!(!reason.is_empty(), "the reason is the system's own words");
        std::fs::remove_file(&file).expect("the test cleans up after itself");
    }

    #[test]
    fn every_reason_an_engine_gives_keeps_what_the_screen_has_to_say_about_it() {
        let problem = EngineProblem::from(Unavailable::NotInstalled);
        assert_eq!(problem, EngineProblem::NotInstalled);
        assert_eq!(problem.output(), None);

        let stopped = EngineProblem::from(Unavailable::DaemonStopped { output: "no socket".to_owned() });
        assert_eq!(stopped.output(), Some("no socket"));

        let machine = EngineProblem::from(Unavailable::MachineStopped { output: "no machine".to_owned() });
        assert_eq!(machine.output(), Some("no machine"));

        let refused = EngineProblem::from(Unavailable::InfoFailed { code: Some(125), output: "no tty".to_owned() });
        assert_eq!(refused, EngineProblem::Refused { code: Some(125), output: "no tty".to_owned() });

        let error = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let unrunnable = EngineProblem::from(Unavailable::NotRunnable { bin: "/usr/bin/podman".into(), error });
        let EngineProblem::NotRunnable { bin, message } = unrunnable else { panic!("it stays unrunnable") };
        assert_eq!(bin, std::path::PathBuf::from("/usr/bin/podman"));
        assert!(!message.is_empty());
    }
}
