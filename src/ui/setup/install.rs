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

/// What starts Docker's daemon on Linux, now and at every boot after.
const DOCKER_SERVICE: &str = "sudo systemctl enable --now docker";

/// What lets this account open Docker's socket. `$USER` is the shell's, which knows the name.
const DOCKER_GROUP: &str = "sudo usermod -aG docker $USER";

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
        /// The line that installs it here, when QCode knows this system's package manager, with
        /// what else the engine needs before it answers.
        command: Option<String>,
        /// The engine's own installation guide, which is right on every system.
        docs: &'static str,
        /// Whether the person has to log out and back in afterwards, as a new group needs.
        relogin: bool,
    },
    /// The engine is installed and something it needs has to be started.
    Start {
        /// The line that starts it, when this system has one QCode can promise.
        command: Option<String>,
    },
    /// The engine works and this account is not set up for it yet: it has to join the `docker`
    /// group, or be given id ranges for podman. The line does it.
    Grant {
        /// The line that does it.
        command: String,
        /// Whether the change only takes hold at the next login, as a new group does.
        relogin: bool,
    },
    /// The account is set up already and this login began before it was: logging out and back
    /// in is all that is left, and no command does that.
    Relogin,
    /// The engine refused for a reason QCode cannot read; only its own words help.
    Unknown,
}

impl Remedy {
    /// The line QCode can run for the person on a terminal in the page, when there is one.
    #[must_use]
    pub fn runnable(&self) -> Option<&str> {
        match self {
            Self::Install { command, .. } | Self::Start { command } => command.as_deref(),
            Self::Grant { command, .. } => Some(command),
            Self::Relogin | Self::Unknown => None,
        }
    }

    /// What to do about `problem` with `kind` on `host`.
    #[must_use]
    pub fn for_problem(problem: &EngineProblem, kind: EngineKind, host: &InstallHost) -> Self {
        match problem {
            EngineProblem::NotInstalled | EngineProblem::NotRunnable { .. } => Self::Install {
                command: host.manager.map(|manager| {
                    let install = manager.install(manager.package(kind));
                    // Docker on Linux is a service with a socket only its group may open, and the
                    // package sets up neither: without the rest, the next thing the person reads
                    // is that the daemon is not running, and then that they may not use it.
                    match (kind, host.platform) {
                        (EngineKind::Docker, Platform::Linux) => {
                            format!("{install} && {DOCKER_SERVICE} && {DOCKER_GROUP}")
                        }
                        _ => install,
                    }
                }),
                docs: match kind {
                    EngineKind::Podman => PODMAN_DOCS,
                    EngineKind::Docker => DOCKER_DOCS,
                },
                relogin: kind == EngineKind::Docker && host.platform == Platform::Linux,
            },
            EngineProblem::NoPermission { in_group: true, .. } => Self::Relogin,
            EngineProblem::NoPermission { in_group: false, .. } => {
                Self::Grant { command: DOCKER_GROUP.to_owned(), relogin: true }
            }
            EngineProblem::NoIdRanges { line, .. } => Self::Grant { command: line.clone(), relogin: false },
            EngineProblem::DaemonStopped { .. } => Self::Start {
                command: match host.platform {
                    // Enabled as well as started, so it is still there after the next boot.
                    Platform::Linux => Some(DOCKER_SERVICE.to_owned()),
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

/// The first id a range of subordinate ids is given from when a machine has none yet, and how
/// many ids a range holds: what `useradd` itself hands a new account on the common
/// distributions, so a range added here looks like one the system made.
const FIRST_SUBORDINATE_ID: u64 = 100_000;
/// See [`FIRST_SUBORDINATE_ID`].
const SUBORDINATE_IDS: u64 = 65_536;

/// The account QCode runs as and what `/etc/passwd`, `/etc/subuid` and `/etc/subgid` say, which
/// is everything needed to know whether rootless podman can map the ids an image uses.
///
/// Plain data, so every case is checked without touching the machine's files; only
/// [`IdRanges::here`] reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdRanges {
    /// The account's name.
    pub user: String,
    /// Whether `user` is the name the environment gave. It is not when the environment named
    /// nobody, which is the case for a qcode a service or `env -i` started: the name then comes
    /// from `/etc/passwd`, so [`IdRanges::line`] writes it out in full instead of leaving
    /// `$USER` to a shell that has none.
    pub from_env: bool,
    /// The account's numeric id; either may stand at the start of a line.
    pub uid: u32,
    /// The text of `/etc/subuid`, empty when it is missing.
    pub subuid: String,
    /// The text of `/etc/subgid`, empty when it is missing.
    pub subgid: String,
}

impl IdRanges {
    /// Reads this machine: the account from the environment, or from `/etc/passwd` when the
    /// environment names nobody, the owner of this process, and the two id files as they are. A
    /// file that cannot be read counts as empty, which is what podman makes of it too.
    #[must_use]
    pub fn here() -> Self {
        #[cfg(unix)]
        let uid = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata("/proc/self").map(|meta| meta.uid()).unwrap_or(u32::MAX)
        };
        #[cfg(not(unix))]
        let uid = u32::MAX;
        let environment =
            std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).ok().filter(|name| !name.is_empty());
        Self::of(
            environment.as_deref(),
            uid,
            &std::fs::read_to_string("/etc/passwd").unwrap_or_default(),
            &std::fs::read_to_string("/etc/subuid").unwrap_or_default(),
            &std::fs::read_to_string("/etc/subgid").unwrap_or_default(),
        )
    }

    /// The ranges of the account `environment` names, or of the one the `passwd` file holds for
    /// `uid` when the environment names nobody.
    ///
    /// `/etc/subuid` names an account by name far more often than by number, so an empty name
    /// makes a machine on which podman works look like one that has no id ranges for it: the
    /// account is read from `passwd`, whose own lines name every account there is.
    ///
    /// The files are parameters so a test can describe any machine from any other one.
    #[must_use]
    pub fn of(environment: Option<&str>, uid: u32, passwd: &str, subuid: &str, subgid: &str) -> Self {
        let (user, from_env) = match environment {
            Some(name) if !name.is_empty() => (name.to_owned(), true),
            _ => (passwd_name(passwd, uid).unwrap_or_default(), false),
        };
        Self { user, from_env, uid, subuid: subuid.to_owned(), subgid: subgid.to_owned() }
    }

    /// Whether both files give this account a range.
    #[must_use]
    pub fn present(&self) -> bool {
        self.has(&self.subuid) && self.has(&self.subgid)
    }

    fn has(&self, text: &str) -> bool {
        let uid = self.uid.to_string();
        entries(text).any(|(owner, _, count)| (owner == self.user || owner == uid) && count > 0)
    }

    /// The line that gives this account a range in both files, then tells podman to take it up.
    ///
    /// The range starts after every range either file already hands out, so it never overlaps
    /// another account's: two accounts sharing ids could read each other's container files.
    /// `$USER` is left for the shell, which knows the name for certain; a name `/etc/passwd` gave
    /// is written out instead, because a shell as bare as the one this program was started from
    /// would leave `$USER` empty and `usermod` would range nobody.
    #[must_use]
    pub fn line(&self) -> String {
        let taken = entries(&self.subuid).chain(entries(&self.subgid)).map(|(_, start, count)| start + count).max();
        let start = taken.unwrap_or(0).max(FIRST_SUBORDINATE_ID);
        let end = start + SUBORDINATE_IDS - 1;
        let account = if self.from_env { "$USER" } else { self.user.as_str() };
        format!(
            "sudo usermod --add-subuids {start}-{end} --add-subgids {start}-{end} {account} && podman system migrate"
        )
    }
}

/// The name the `passwd` file gives `uid`: the first field of the line whose third field is
/// `uid`, as every account a machine has is in there.
///
/// A line that does not parse is skipped, so one unreadable line does not hide the accounts the
/// rest of the file names; a `uid` the file holds no line for names nobody, which is what an
/// empty name already meant.
fn passwd_name(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        (fields.next()?.trim().parse::<u32>().ok()? == uid).then_some(name.to_owned())
    })
}

/// The `owner:start:count` lines of a subordinate id file, skipping anything that is not one.
fn entries(text: &str) -> impl Iterator<Item = (&str, u64, u64)> {
    text.lines().filter_map(|line| {
        let mut parts = line.trim().split(':');
        let owner = parts.next()?;
        let start = parts.next()?.trim().parse().ok()?;
        let count = parts.next()?.trim().parse().ok()?;
        (!owner.is_empty() && !owner.starts_with('#')).then_some((owner, start, count))
    })
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

    /// `ada`'s ranges, an account the environment named, with the two id files as given.
    fn ranges(subuid: &str, subgid: &str) -> IdRanges {
        IdRanges::of(Some("ada"), 1001, "", subuid, subgid)
    }

    /// The text of `/etc/passwd` on a machine with two accounts and a line nothing can read.
    fn passwd() -> &'static str {
        "root:x:0:0:root:/root:/bin/bash\nnot an account\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n"
    }

    #[test]
    fn an_account_has_ranges_only_when_both_files_name_it() {
        assert!(ranges("ada:100000:65536\n", "ada:100000:65536\n").present());
        // By number as well as by name, the way shadow-utils reads them.
        assert!(ranges("1001:100000:65536\n", "ada:100000:65536\n").present());
        assert!(!ranges("ada:100000:65536\n", "").present(), "a group range is needed too");
        assert!(!ranges("bob:100000:65536\n", "bob:100000:65536\n").present(), "another account's range");
        assert!(!ranges("ada:100000:0\n", "ada:100000:0\n").present(), "a range of nothing");
    }

    #[test]
    fn the_name_a_uid_has_in_passwd_is_found_past_a_line_that_does_not_parse() {
        assert_eq!(passwd_name(passwd(), 1000).as_deref(), Some("alice"));
        assert_eq!(passwd_name(passwd(), 0).as_deref(), Some("root"));
        assert_eq!(passwd_name(passwd(), 4242), None, "a uid the file holds no line for names nobody");
        assert_eq!(passwd_name("", 0), None, "a passwd file that could not be read names nobody");
    }

    #[test]
    fn an_account_the_environment_leaves_unnamed_is_found_in_passwd() {
        // A qcode a service or `env -i` started has neither `$USER` nor `$LOGNAME`, and the id
        // files name an account by name, so an empty name reads as an account podman has no
        // ranges for on a machine where it works.
        let ranges = IdRanges::of(None, 1000, passwd(), "alice:100000:65536\n", "alice:100000:65536\n");
        assert_eq!(ranges.user, "alice", "the uid is the one thing qcode knows of its own account");
        assert!(ranges.present());
        assert_eq!(IdRanges::of(Some(""), 1000, passwd(), "", "").user, "alice", "an empty name is no name");
    }

    #[test]
    fn the_range_added_starts_after_every_range_already_handed_out() {
        assert_eq!(
            ranges("", "").line(),
            "sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 $USER && podman system migrate"
        );
        let crowded = ranges("bob:100000:65536\n# a note\ncarol:165536:65536\n", "bob:100000:65536\n");
        assert!(
            crowded.line().contains("--add-subuids 231072-296607 --add-subgids 231072-296607"),
            "{}",
            crowded.line()
        );
    }

    #[test]
    fn the_line_names_the_account_the_way_the_shell_that_runs_it_will_know_it() {
        let from_passwd = IdRanges::of(None, 1000, passwd(), "", "");
        let line = from_passwd.line();
        assert!(line.contains(" --add-subgids 100000-165535 alice &&"), "{line}");
        assert!(!line.contains("$USER"), "a shell as bare as the one qcode was started from has none: {line}");
        assert!(ranges("", "").line().contains("$USER"), "a name the environment gave is left to the shell");
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
        let line = install(&host, EngineKind::Docker).expect("a line");
        assert!(line.starts_with("sudo apt install docker.io && "), "{line}");
    }

    #[test]
    fn docker_on_linux_is_installed_started_and_let_in_by_one_line() {
        let host = linux("arch", true);
        assert_eq!(
            install(&host, EngineKind::Docker).as_deref(),
            Some("paru -S docker && sudo systemctl enable --now docker && sudo usermod -aG docker $USER")
        );
        let Remedy::Install { relogin, .. } =
            Remedy::for_problem(&EngineProblem::NotInstalled, EngineKind::Docker, &host)
        else {
            panic!("a missing engine is installed")
        };
        assert!(relogin, "the new group only holds after a new login, and the person is told so");
        // Podman needs no service and no group; its line is the package alone.
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("paru -S podman"));
        // Docker Desktop on macOS is an application that does both itself.
        let mac = InstallHost::read(Platform::MacOs, None, |_| false);
        assert_eq!(install(&mac, EngineKind::Docker).as_deref(), Some("brew install --cask docker"));
    }

    #[test]
    fn an_account_docker_will_not_let_in_is_added_to_the_group_or_only_asked_to_log_in_again() {
        let host = linux("arch", true);
        let outside = EngineProblem::NoPermission { output: String::new(), in_group: false };
        let remedy = Remedy::for_problem(&outside, EngineKind::Docker, &host);
        assert_eq!(remedy, Remedy::Grant { command: "sudo usermod -aG docker $USER".to_owned(), relogin: true });
        assert_eq!(remedy.runnable(), Some("sudo usermod -aG docker $USER"));
        let inside = EngineProblem::NoPermission { output: String::new(), in_group: true };
        assert_eq!(Remedy::for_problem(&inside, EngineKind::Docker, &host), Remedy::Relogin);
        assert_eq!(Remedy::Relogin.runnable(), None, "no command logs anyone out");
    }

    #[test]
    fn fedora_and_its_family_install_with_dnf() {
        assert_eq!(install(&linux("fedora", false), EngineKind::Podman).as_deref(), Some("sudo dnf install podman"));
        let rocky = InstallHost::read(Platform::Linux, Some("ID=rocky\nID_LIKE=\"rhel centos fedora\"\n"), |_| false);
        assert!(
            install(&rocky, EngineKind::Docker).is_some_and(|line| line.starts_with("sudo dnf install docker && "))
        );
    }

    #[test]
    fn opensuse_installs_with_zypper() {
        let host = linux("opensuse-tumbleweed", false);
        assert_eq!(install(&host, EngineKind::Podman).as_deref(), Some("sudo zypper install podman"));
    }

    #[test]
    fn a_system_qcode_does_not_know_is_sent_to_the_engines_own_guide() {
        let host = InstallHost::read(Platform::Linux, None, |_| false);
        let Remedy::Install { command, docs, .. } =
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
        assert!(matches!(linux, Remedy::Start { command: Some(ref c) } if c == "sudo systemctl enable --now docker"));

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
