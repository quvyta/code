//! The gates of the setup wizard: what each step needs before the wizard may leave it.
//!
//! The rule of the design is that the wizard never jumps to the step the config file
//! remembers. Every gate is asked in the order of the steps, and the first one that does not
//! hold is where the wizard opens. [`Gates`] is plain data and [`Gates::entry`] is a pure
//! function of it, so a test hands in any answer it likes without a disk or a container engine
//! anywhere near it. Only [`Gates::probe`], [`check_engine`] and [`check_location`] touch the
//! machine.

use std::path::{Path, PathBuf};

use crate::engine::known::{self, Known};
use crate::engine::{EngineKind, Unavailable, detect};

use super::install::IdRanges;
use crate::store::{Config, SetupStep, Store};

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
    /// Docker answers and this account may not use it: it is not in the `docker` group, or it
    /// was just added and this login began before that.
    NoPermission {
        /// What docker said.
        output: String,
        /// Whether `/etc/group` already names the account in `docker`, so only a new login is
        /// missing.
        in_group: bool,
    },
    /// Podman works without root and this account has no subordinate id ranges, which every
    /// image with more than one user needs.
    NoIdRanges {
        /// The line that adds them, with a range no other account holds.
        line: String,
        /// What podman said, when it was a refusal that told; empty when the wizard found it
        /// in the files before podman had to refuse anything.
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
            Self::DaemonStopped { output }
            | Self::MachineStopped { output }
            | Self::Refused { output, .. }
            | Self::NoPermission { output, .. }
            | Self::NoIdRanges { output, .. } => Some(output).filter(|text| !text.is_empty()).map(String::as_str),
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

/// Why the store folder cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationProblem {
    /// No folder has been chosen yet.
    Unset,
    /// The path names something that is not a folder.
    NotAFolder,
    /// The folder could not be made, read or written, in the system's own words.
    Blocked(String),
}

/// What the machine answered about the store folder.
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
    /// What the store folder answered.
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
        gates.location = match config.folder_path() {
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
        Err(unavailable) => {
            let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
            let user = IdRanges::here().user;
            EngineCheck::Broken(refine(EngineProblem::from(unavailable), &group, &user))
        }
    }
}

/// What the setup wizard's engine step asks: [`check_engine`], and for podman on Linux also
/// whether the account has the id ranges podman needs to run QCode's images without root.
///
/// Only the wizard asks the second question. An engine that answers is left working everywhere
/// else, and a container it then cannot make says why in a sentence of its own; the wizard is
/// where the person is putting their engine in order, and where the line that adds the ranges
/// can be run for them.
#[must_use]
pub fn check_engine_for_setup(kind: EngineKind) -> EngineCheck {
    let check = check_engine(kind);
    let linux = crate::store::Platform::host() == crate::store::Platform::Linux;
    if check == EngineCheck::Working && kind == EngineKind::Podman && linux {
        return with_ranges(check, &IdRanges::here());
    }
    check
}

/// A working podman on an account without id ranges is not working yet.
fn with_ranges(check: EngineCheck, ranges: &IdRanges) -> EngineCheck {
    if check == EngineCheck::Working && !ranges.present() {
        return EngineCheck::Broken(EngineProblem::NoIdRanges { line: ranges.line(), output: String::new() });
    }
    check
}

/// Reads a refusal whose words QCode recognises as the problem it is: an account docker will
/// not let in, told apart by whether `/etc/group` (`group`) already names `user` in `docker`;
/// or an account podman has no id ranges for.
fn refine(problem: EngineProblem, group: &str, user: &str) -> EngineProblem {
    let EngineProblem::Refused { output, .. } = &problem else { return problem };
    match known::recognise(output) {
        Some(Known::NoPermission) => {
            EngineProblem::NoPermission { output: output.clone(), in_group: in_group(group, "docker", user) }
        }
        Some(Known::NoIdRanges) => EngineProblem::NoIdRanges { line: IdRanges::here().line(), output: output.clone() },
        _ => problem,
    }
}

/// Whether the text of `/etc/group` names `user` among the members of `name`.
fn in_group(text: &str, name: &str, user: &str) -> bool {
    text.lines().any(|line| {
        let mut fields = line.split(':');
        fields.next() == Some(name)
            && fields.nth(2).is_some_and(|members| members.split(',').any(|member| member.trim() == user))
    })
}

/// Makes sure the store at `path` is there and can be written in.
///
/// The folder is made when it is missing: placing the store is the wizard's whole job, and
/// the same call at every later start repairs a store whose folders were removed. Writing
/// is proven by writing, not guessed from permission bits, because a read-only mount and a full
/// disk both look writable until something is written.
///
/// Blocks on the file system, so it belongs on a background thread.
#[must_use]
pub fn check_location(path: &Path) -> LocationCheck {
    if path.is_file() {
        return LocationCheck::Broken(LocationProblem::NotAFolder);
    }
    let store = Store::new(path);
    if let Err(diagnostic) = store.prepare() {
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
    use crate::store::{Config, SetupStep};

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
        assert!(dir.join("Workspaces").is_dir(), "the store tree is made while it is checked");
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

    #[test]
    fn a_docker_socket_this_account_may_not_open_asks_for_the_group_or_only_for_a_new_login() {
        let output = "permission denied while trying to connect to the docker API at unix:///var/run/docker.sock";
        let refused = || EngineProblem::Refused { code: Some(1), output: output.to_owned() };
        let group = "wheel:x:998:ada\ndocker:x:951:bob,ada\n";
        assert_eq!(
            super::refine(refused(), group, "ada"),
            EngineProblem::NoPermission { output: output.to_owned(), in_group: true }
        );
        assert_eq!(
            super::refine(refused(), group, "carol"),
            EngineProblem::NoPermission { output: output.to_owned(), in_group: false }
        );
        let other = EngineProblem::Refused { code: Some(1), output: "no tty".to_owned() };
        assert_eq!(super::refine(other.clone(), group, "ada"), other, "anything else is left as it was");
    }

    #[test]
    fn a_working_podman_on_an_account_without_id_ranges_is_not_ready_yet() {
        let ranges = |subuid: &str| super::IdRanges {
            user: "ada".to_owned(),
            uid: 1001,
            subuid: subuid.to_owned(),
            subgid: subuid.to_owned(),
        };
        let missing = super::with_ranges(EngineCheck::Working, &ranges(""));
        let EngineCheck::Broken(EngineProblem::NoIdRanges { line, .. }) = missing else { panic!("{missing:?}") };
        assert!(line.contains("--add-subuids 100000-165535"), "{line}");
        assert_eq!(super::with_ranges(EngineCheck::Working, &ranges("ada:100000:65536\n")), EngineCheck::Working);
    }
}
