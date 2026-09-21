//! What the person has to run to get a container engine, on the system they are actually on.
//!
//! QCode works out the one line that installs the engine on this machine and offers it two
//! ways: the person copies it and runs it themselves, or QCode runs that very line for them on
//! a terminal they watch. Either way it is the one command they chose, run with the rights they
//! already have; QCode never raises its own. Working the line out is a pure function of
//! [`InstallHost`], which is plain data, so every system's guidance is checked from any other
//! system, and starting it goes through [`Installer`], so a test drives the whole path without
//! a package manager anywhere near it.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use qframe::widgets::TerminalSession;

use crate::engine::EngineKind;
use crate::store::Platform;

use super::gates::EngineProblem;

/// The installation guide of each engine, for a system whose package manager QCode does not
/// know.
const PODMAN_DOCS: &str = "https://podman.io/docs/installation";
/// See [`PODMAN_DOCS`].
const DOCKER_DOCS: &str = "https://docs.docker.com/engine/install/";

/// The program that installs software on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    /// Arch and its family, through the AUR helper the person already has.
    Paru,
    /// Arch and its family.
    Pacman,
    /// Debian, Ubuntu and their family.
    Apt,
    /// Fedora, RHEL and their family.
    Dnf,
    /// openSUSE.
    Zypper,
    /// macOS, through Homebrew.
    Brew,
    /// Windows.
    Winget,
}

impl PackageManager {
    /// The line that installs `package`, with the rights raising the system itself needs.
    fn install(self, package: &str) -> String {
        match self {
            Self::Paru => format!("paru -S {package}"),
            Self::Pacman => format!("sudo pacman -S {package}"),
            Self::Apt => format!("sudo apt install {package}"),
            Self::Dnf => format!("sudo dnf install {package}"),
            Self::Zypper => format!("sudo zypper install {package}"),
            // Docker on macOS is an application, not a formula, and casks say so.
            Self::Brew if package == "docker" => "brew install --cask docker".to_owned(),
            Self::Brew => format!("brew install {package}"),
            Self::Winget => format!("winget install {package}"),
        }
    }

    /// What this manager calls `kind`. The name is the manager's, not the engine's: Debian's
    /// docker is `docker.io`, and winget wants the publisher's identifier.
    fn package(self, kind: EngineKind) -> &'static str {
        match (self, kind) {
            (Self::Winget, EngineKind::Podman) => "RedHat.Podman",
            (Self::Winget, EngineKind::Docker) => "Docker.DockerDesktop",
            (Self::Apt, EngineKind::Docker) => "docker.io",
            (_, EngineKind::Podman) => "podman",
            (_, EngineKind::Docker) => "docker",
        }
    }
}

/// The machine the guidance is written for.
///
/// Carried as data rather than read from `cfg!` where it is needed, so the guidance for all
/// three operating systems is checked on one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallHost {
    /// Which system's rules to follow.
    pub platform: Platform,
    /// The program that installs software here, when QCode recognises one.
    pub manager: Option<PackageManager>,
    /// The engines already on this machine, Podman before Docker. It is what the wizard ticks
    /// when the settings file names no engine: offering to install what is already installed
    /// would be the wizard ignoring the machine it is standing on.
    pub engines: Vec<EngineKind>,
}

/// The engines QCode offers, in the order it offers them.
const ENGINES: [EngineKind; 2] = [EngineKind::Podman, EngineKind::Docker];

impl InstallHost {
    /// Reads the machine this program runs on.
    #[must_use]
    pub fn detect() -> Self {
        let platform = Platform::host();
        let release = (platform == Platform::Linux).then(|| std::fs::read_to_string("/etc/os-release").ok()).flatten();
        // The engine layer's own search, not the bare `PATH`: a terminal started from a desktop
        // launcher has a `PATH` that names neither Homebrew directory, and Docker Desktop puts
        // its binary where only its installer knows. Asking `PATH` alone would tell the person
        // to install an engine they already have, and QCode would then fail to find it twice
        // for two different reasons.
        Self::read(platform, release.as_deref(), crate::engine::installed)
    }

    /// The host `platform` describes, with `release` the text of the Linux `os-release` file and
    /// `installed` answering whether a program is on the person's `PATH`.
    ///
    /// Both are parameters so a test can describe any distribution from any machine.
    #[must_use]
    pub fn read(platform: Platform, release: Option<&str>, installed: impl Fn(&str) -> bool) -> Self {
        let manager = match platform {
            Platform::MacOs => Some(PackageManager::Brew),
            Platform::Windows => Some(PackageManager::Winget),
            // A distribution QCode has never heard of gets no invented command: it is sent to
            // the engine's own guide instead, which is always right.
            Platform::Linux => family(release.unwrap_or_default()).map(|family| match family {
                PackageManager::Pacman if installed("paru") => PackageManager::Paru,
                manager => manager,
            }),
        };
        let engines = ENGINES.into_iter().filter(|kind| installed(kind.dialect().binary)).collect();
        Self { platform, manager, engines }
    }
}

/// How the install command the person chose is started.
///
/// It is a function rather than a call to [`TerminalSession::spawn`] where the command is run,
/// so the whole path — the button, the terminal inside the page, the output on it and the end
/// of the command — is driven by a test that starts a harmless script instead. Nothing a test
/// runs is ever an installation.
#[derive(Clone)]
pub struct Installer(Arc<Start>);

/// What an [`Installer`] does with a command line: it starts it and hands back the terminal it
/// is running on, or says why it could not be started at all.
type Start = dyn Fn(&str) -> std::io::Result<TerminalSession> + Send + Sync;

impl std::fmt::Debug for Installer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Installer")
    }
}

impl Installer {
    /// The real one: the line is handed to the person's own shell with the rights they already
    /// have. Nothing here raises them. A command that needs more asks for it itself, on the
    /// terminal, where the person is the one who answers.
    #[must_use]
    pub fn shell() -> Self {
        Self::new(|command| {
            let (program, flag): (OsString, &str) = if cfg!(windows) {
                (OsString::from("cmd"), "/C")
            } else {
                (std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into()), "-c")
            };
            let folder = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            TerminalSession::spawn(&program, &[flag, command], &folder)
        })
    }

    /// An installer that starts commands the way `start` says.
    #[must_use]
    pub fn new(start: impl Fn(&str) -> std::io::Result<TerminalSession> + Send + Sync + 'static) -> Self {
        Self(Arc::new(start))
    }

    /// Starts `command` on a terminal of its own.
    ///
    /// # Errors
    ///
    /// When no pseudo-terminal can be opened or the command cannot be started at all.
    pub fn start(&self, command: &str) -> std::io::Result<TerminalSession> {
        (self.0)(command)
    }
}

/// The family of a Linux distribution, from its own `ID` and the families it says it is like.
///
/// `ID_LIKE` is what derivatives are for: Linux Mint says `ubuntu debian`, Rocky says
/// `rhel centos fedora`, and reading it means a distribution QCode has never seen still gets
/// the right command as long as it names its family honestly.
fn family(release: &str) -> Option<PackageManager> {
    let mut ids = Vec::new();
    for line in release.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        if !matches!(key.trim(), "ID" | "ID_LIKE") {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        ids.extend(value.split_whitespace().map(str::to_lowercase));
    }
    ids.iter().find_map(|id| match id.as_str() {
        "arch" | "archlinux" | "manjaro" | "endeavouros" | "cachyos" | "garuda" => Some(PackageManager::Pacman),
        "debian" | "ubuntu" | "linuxmint" | "pop" | "raspbian" => Some(PackageManager::Apt),
        "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => Some(PackageManager::Dnf),
        "opensuse" | "opensuse-tumbleweed" | "opensuse-leap" | "suse" | "sles" => Some(PackageManager::Zypper),
        _ => None,
    })
}

/// What the person has to do before the chosen engine works.
///
/// The words belong to the screen, which knows from the [`EngineProblem`] itself what to say;
/// what is here is only the one line the person may want to run, and never a command QCode is
/// not sure of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remedy {
    /// The engine is not there and has to be installed.
    Install {
        /// The line that installs it here, when QCode knows this system's package manager.
        command: Option<String>,
        /// The engine's own installation guide, which is right on every system.
        docs: &'static str,
    },
    /// The engine is installed and something it needs has to be started.
    Start {
        /// The line that starts it, when this system has one QCode can promise.
        command: Option<String>,
    },
    /// The engine refused for a reason QCode cannot read; only its own words help.
    Unknown,
}

impl Remedy {
    /// What to do about `problem` with `kind` on `host`.
    #[must_use]
    pub fn for_problem(problem: &EngineProblem, kind: EngineKind, host: &InstallHost) -> Self {
        match problem {
            EngineProblem::NotInstalled | EngineProblem::NotRunnable { .. } => Self::Install {
                command: host.manager.map(|manager| manager.install(manager.package(kind))),
                docs: match kind {
                    EngineKind::Podman => PODMAN_DOCS,
                    EngineKind::Docker => DOCKER_DOCS,
                },
            },
            EngineProblem::DaemonStopped { .. } => Self::Start {
                command: match host.platform {
                    Platform::Linux => Some("sudo systemctl start docker".to_owned()),
                    Platform::MacOs => Some("open -a Docker".to_owned()),
                    // Docker Desktop on Windows is started from the desktop; there is no line
                    // QCode can promise, so it shows none and says so instead.
                    Platform::Windows => None,
                },
            },
            // The machine is podman's own, and the command is the same on every system.
            EngineProblem::MachineStopped { .. } => Self::Start { command: Some("podman machine start".to_owned()) },
            EngineProblem::Refused { .. } => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineKind;
    use crate::store::Platform;

    /// A Linux host whose `os-release` says `id`, with `paru` installed or not.
    fn linux(id: &str, paru: bool) -> InstallHost {
        let text = format!("NAME=\"Something\"\nID={id}\n");
        InstallHost::read(Platform::Linux, Some(&text), |tool| paru && tool == "paru")
    }

    fn install(host: &InstallHost, kind: EngineKind) -> Option<String> {
        match Remedy::for_problem(&EngineProblem::NotInstalled, kind, host) {
            Remedy::Install { command, .. } => command,
            other => panic!("a missing engine is installed, not {other:?}"),
        }
    }

    #[test]
    fn arch_installs_with_paru_when_it_is_there() {
        let host = linux("arch", true);
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("paru -S podman"));
    }

    #[test]
    fn arch_falls_back_to_pacman_when_paru_is_not_installed() {
        let host = linux("arch", false);
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("sudo pacman -S podman"));
    }

    #[test]
    fn a_distribution_is_recognised_by_what_it_is_like() {
        // Derivatives name their own ID and say which family they follow.
        let text = "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
        let host = InstallHost::read(Platform::Linux, Some(text), |_| false);
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("sudo apt install podman"));
    }

    #[test]
    fn debian_installs_docker_under_the_name_debian_gives_it() {
        let host = linux("debian", false);
        assert_eq!(install(&host, EngineKind::Docker).as_deref(), Some("sudo apt install docker.io"));
    }

    #[test]
    fn fedora_and_its_family_install_with_dnf() {
        assert_eq!(install(&linux("fedora", false), EngineKind::Podman).as_deref(), Some("sudo dnf install podman"));
        let rocky = InstallHost::read(Platform::Linux, Some("ID=rocky\nID_LIKE=\"rhel centos fedora\"\n"), |_| false);
        assert_eq!(install(&rocky, EngineKind::Docker).as_deref(), Some("sudo dnf install docker"));
    }

    #[test]
    fn opensuse_installs_with_zypper() {
        let host = linux("opensuse-tumbleweed", false);
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("sudo zypper install podman"));
    }

    #[test]
    fn a_system_qcode_does_not_know_is_sent_to_the_engines_own_guide() {
        let host = InstallHost::read(Platform::Linux, None, |_| false);
        let Remedy::Install { command, docs } =
            Remedy::for_problem(&EngineProblem::NotInstalled, EngineKind::Podman, &host)
        else {
            panic!("a missing engine is installed")
        };
        assert_eq!(command, None, "no command is invented for a package manager nobody found");
        assert!(docs.starts_with("https://"), "the guide is a link the person can open: {docs}");
    }

    #[test]
    fn macos_installs_with_homebrew_and_knows_docker_is_an_application() {
        let host = InstallHost::read(Platform::MacOs, None, |_| false);
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("brew install podman"));
        assert_eq!(install(&host, EngineKind::Docker).as_deref(), Some("brew install --cask docker"));
    }

    #[test]
    fn windows_installs_with_winget() {
        let host = InstallHost::read(Platform::Windows, None, |_| false);
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("winget install RedHat.Podman"));
        assert_eq!(install(&host, EngineKind::Docker).as_deref(), Some("winget install Docker.DockerDesktop"));
    }

    #[test]
    fn a_stopped_docker_daemon_is_started_the_way_each_system_starts_it() {
        let problem = EngineProblem::DaemonStopped { output: String::new() };
        let linux = Remedy::for_problem(&problem, EngineKind::Docker, &linux("debian", false));
        assert!(matches!(linux, Remedy::Start { command: Some(ref c) } if c == "sudo systemctl start docker"));

        let mac = InstallHost::read(Platform::MacOs, None, |_| false);
        let mac = Remedy::for_problem(&problem, EngineKind::Docker, &mac);
        assert!(matches!(mac, Remedy::Start { command: Some(ref c) } if c == "open -a Docker"));

        // Docker Desktop on Windows has no command QCode can promise, so none is shown; the
        // screen says to start the application instead of printing something that may not work.
        let windows = InstallHost::read(Platform::Windows, None, |_| false);
        let windows = Remedy::for_problem(&problem, EngineKind::Docker, &windows);
        assert!(matches!(windows, Remedy::Start { command: None }));
    }

    #[test]
    fn a_podman_machine_that_is_not_running_is_started_the_same_way_everywhere() {
        let problem = EngineProblem::MachineStopped { output: String::new() };
        let host = InstallHost::read(Platform::MacOs, None, |_| false);
        let remedy = Remedy::for_problem(&problem, EngineKind::Podman, &host);
        assert!(matches!(remedy, Remedy::Start { command: Some(ref c) } if c == "podman machine start"));
    }

    #[test]
    fn the_engines_this_machine_already_has_are_read_with_the_rest_of_it() {
        let both = InstallHost::read(Platform::Linux, Some("ID=arch\n"), |tool| matches!(tool, "podman" | "docker"));
        assert_eq!(both.engines, [EngineKind::Podman, EngineKind::Docker], "podman is named before docker");
        let docker = InstallHost::read(Platform::Linux, Some("ID=arch\n"), |tool| tool == "docker");
        assert_eq!(docker.engines, [EngineKind::Docker]);
        let bare = InstallHost::read(Platform::Linux, Some("ID=arch\n"), |_| false);
        assert!(bare.engines.is_empty(), "a machine with neither engine says so");
    }

    #[test]
    fn a_refusal_qcode_cannot_read_offers_no_command_at_all() {
        let host = linux("arch", true);
        let problem = EngineProblem::Refused { code: Some(125), output: "no tty".to_owned() };
        assert!(matches!(Remedy::for_problem(&problem, EngineKind::Podman, &host), Remedy::Unknown));
    }
}
