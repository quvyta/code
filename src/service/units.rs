//! The files that install the background service, and the steps that put them in place or take
//! them away.
//!
//! Linux gets two systemd user units: `qcode-reaper.path` watches the list of QCode's containers
//! and starts `qcode-reaper.service` when QCode writes to it, and the service runs `qcode reaper`
//! once and exits. macOS gets one launchd job that does the same with `WatchPaths`. Windows gets
//! nothing, because what the service waits on is a lock Windows does not give QCode.
//!
//! Every file is text made by a pure function of a [`ServiceHost`], and installing is a list of
//! [`Step`]s — files to write, files to remove, commands to run — so the whole of it is read back
//! in tests against a temporary folder, with the commands collected rather than run. Only
//! [`perform`] with [`run_host`] touches the machine, and only the settings screen's button calls
//! it.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use qframe::storage::config_dir;

use crate::engine::HostUser;
use crate::store::{Platform, Registry};

/// The unit that runs the reaper once.
pub const SERVICE_UNIT: &str = "qcode-reaper.service";

/// The unit that watches the list and starts [`SERVICE_UNIT`].
pub const PATH_UNIT: &str = "qcode-reaper.path";

/// The launchd label of the job on macOS.
pub const LAUNCHD_LABEL: &str = "io.quvyta.code.reaper";

/// The word `qcode` is started with to run the reaper.
pub const REAPER_ARG: &str = "reaper";

/// Everything about the machine the service files are made for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceHost {
    /// Which platform's service manager to write for: Linux or macOS.
    pub platform: Platform,
    /// The folder the unit files go into: systemd's user unit folder on Linux, the person's
    /// `LaunchAgents` on macOS.
    pub units: PathBuf,
    /// The list of QCode's containers, which is what the service watches.
    pub registry: PathBuf,
    /// The `qcode` binary the service runs.
    pub program: PathBuf,
    /// The person's user id, which names launchd's per-user domain on macOS.
    pub uid: Option<u32>,
}

/// One thing installing or removing the service does, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Write this text to this file, making its folder first.
    Write(PathBuf, String),
    /// Remove this file; one that is not there is already what was wanted.
    Remove(PathBuf),
    /// Run this command.
    Run(HostCommand),
}

/// A program and its arguments, run on the machine itself: `systemctl` or `launchctl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCommand {
    /// The program, looked up on the `PATH`.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
}

impl HostCommand {
    fn new(program: &str, args: &[&str]) -> Self {
        Self { program: program.to_owned(), args: args.iter().map(|arg| (*arg).to_owned()).collect() }
    }

    /// The command written out the way it would be typed.
    #[must_use]
    pub fn written(&self) -> String {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

impl ServiceHost {
    /// The machine this program runs on, or `None` where no service is installed: on Windows,
    /// and on a machine that names no home to keep the files in.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let platform = Platform::host();
        let units = match platform {
            Platform::Windows => return None,
            Platform::Linux => config_dir("systemd/user")?,
            Platform::MacOs => {
                let home = std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute())?;
                home.join("Library").join("LaunchAgents")
            }
        };
        let uid = match HostUser::current() {
            Ok(HostUser::Ids { uid, .. }) => Some(uid),
            _ => None,
        };
        Some(Self { platform, units, registry: Registry::file()?, program: std::env::current_exe().ok()?, uid })
    }

    /// The files the service is made of, with their text.
    #[must_use]
    pub fn files(&self) -> Vec<(PathBuf, String)> {
        match self.platform {
            Platform::MacOs => vec![(self.units.join(format!("{LAUNCHD_LABEL}.plist")), self.plist())],
            Platform::Linux | Platform::Windows => vec![
                (self.units.join(SERVICE_UNIT), self.service_unit()),
                (self.units.join(PATH_UNIT), self.path_unit()),
            ],
        }
    }

    /// Whether the service is installed, which is whether its files are there. Reads the disk.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.files().iter().all(|(path, _)| path.is_file())
    }

    /// The steps that install the service and start watching.
    #[must_use]
    pub fn install(&self) -> Vec<Step> {
        let mut steps: Vec<Step> = self.files().into_iter().map(|(path, text)| Step::Write(path, text)).collect();
        match self.platform {
            Platform::MacOs => {
                let plist = self.units.join(format!("{LAUNCHD_LABEL}.plist"));
                let plist = plist.display().to_string();
                steps.push(Step::Run(HostCommand::new("launchctl", &["bootstrap", &self.domain(), &plist])));
            }
            Platform::Linux | Platform::Windows => {
                steps.push(Step::Run(HostCommand::new("systemctl", &["--user", "daemon-reload"])));
                steps.push(Step::Run(HostCommand::new("systemctl", &["--user", "enable", "--now", PATH_UNIT])));
            }
        }
        steps
    }

    /// The steps that stop the service and take its files away.
    #[must_use]
    pub fn uninstall(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        match self.platform {
            Platform::MacOs => {
                let target = format!("{}/{LAUNCHD_LABEL}", self.domain());
                steps.push(Step::Run(HostCommand::new("launchctl", &["bootout", &target])));
            }
            Platform::Linux | Platform::Windows => {
                steps.push(Step::Run(HostCommand::new("systemctl", &["--user", "disable", "--now", PATH_UNIT])));
                // A reaper that is waiting for the last QCode goes with the watcher.
                steps.push(Step::Run(HostCommand::new("systemctl", &["--user", "stop", SERVICE_UNIT])));
            }
        }
        steps.extend(self.files().into_iter().map(|(path, _)| Step::Remove(path)));
        if self.platform != Platform::MacOs {
            steps.push(Step::Run(HostCommand::new("systemctl", &["--user", "daemon-reload"])));
        }
        steps
    }

    /// launchd's domain of the person's own graphical session.
    fn domain(&self) -> String {
        match self.uid {
            Some(uid) => format!("gui/{uid}"),
            // Without an id there is no domain to name; launchctl then says so in its own words.
            None => "gui".to_owned(),
        }
    }

    /// `qcode-reaper.service`: runs the reaper once, and is done when it exits.
    ///
    /// `Type=exec` rather than `oneshot`: the reaper may wait hours for the last QCode to close,
    /// and a oneshot unit would be killed by its start timeout long before that.
    fn service_unit(&self) -> String {
        let mut text = String::from("[Unit]\nDescription=Stops the containers QCode started once no QCode is open\n");
        let _ = write!(
            text,
            "\n[Service]\nType=exec\nExecStart={} {REAPER_ARG}\n",
            systemd_quoted(&self.program.display().to_string())
        );
        text
    }

    /// `qcode-reaper.path`: starts the service whenever QCode writes to the list.
    fn path_unit(&self) -> String {
        let mut text = String::from("[Unit]\nDescription=Watches the list of containers QCode started\n");
        let _ = write!(
            text,
            "\n[Path]\nPathChanged={}\nUnit={SERVICE_UNIT}\n\n[Install]\nWantedBy=default.target\n",
            systemd_escaped(&self.registry.display().to_string())
        );
        text
    }

    /// The launchd job: runs the reaper whenever the list changes.
    fn plist(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{LAUNCHD_LABEL}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>{}</string>\n\
             \t\t<string>{REAPER_ARG}</string>\n\
             \t</array>\n\
             \t<key>WatchPaths</key>\n\
             \t<array>\n\
             \t\t<string>{}</string>\n\
             \t</array>\n\
             </dict>\n\
             </plist>\n",
            xml_escaped(&self.program.display().to_string()),
            xml_escaped(&self.registry.display().to_string()),
        )
    }
}

/// Text systemd reads literally where it expands specifiers: `%` introduces one.
fn systemd_escaped(text: &str) -> String {
    text.replace('%', "%%")
}

/// A path as one word of an `ExecStart=` line: quoted, so a folder with a space in it stays one
/// argument, with the quote and the backslash escaped and `%` and `$` kept from being expanded.
fn systemd_quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Text inside an XML element.
fn xml_escaped(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Carries out `steps` in order, stopping at the first that fails, and answers why it failed in
/// words the person can read: the command's own when a command failed.
///
/// `run` runs one command; [`run_host`] is the real one, and a test passes its own.
///
/// # Errors
///
/// The words of the first step that failed.
pub fn perform(steps: &[Step], run: &mut dyn FnMut(&HostCommand) -> Result<(), String>) -> Result<(), String> {
    for step in steps {
        match step {
            Step::Write(path, text) => write(path, text).map_err(|error| format!("{}: {error}", path.display()))?,
            Step::Remove(path) => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
            },
            Step::Run(command) => run(command)?,
        }
    }
    Ok(())
}

/// Writes `text` to `path`, making its folder first.
fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    qframe::storage::atomic_write(path, text.as_bytes())
}

/// Runs `command` on this machine and waits for it.
///
/// This changes the person's service manager, so it runs only when the person presses the
/// button that asks for it, and on a background thread.
///
/// # Errors
///
/// What the command printed when it failed, or why it could not be started.
pub fn run_host(command: &HostCommand) -> Result<(), String> {
    let output = Command::new(&command.program)
        .args(&command.args)
        .output()
        .map_err(|error| format!("{}: {error}", command.program))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let said = if said.is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_owned() } else { said };
    Err(if said.is_empty() { command.written() } else { format!("{}: {said}", command.written()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(root: &Path) -> ServiceHost {
        ServiceHost {
            platform: Platform::Linux,
            units: root.join("config").join("systemd").join("user"),
            registry: PathBuf::from("/home/ada/.local/share/quvyta/code/containers.toml"),
            program: PathBuf::from("/home/ada/.cargo/bin/qcode"),
            uid: Some(1000),
        }
    }

    fn mac(root: &Path) -> ServiceHost {
        ServiceHost {
            platform: Platform::MacOs,
            units: root.join("Library").join("LaunchAgents"),
            registry: PathBuf::from("/Users/ada/Library/Application Support/quvyta/code/containers.toml"),
            program: PathBuf::from("/opt/homebrew/bin/qcode"),
            uid: Some(501),
        }
    }

    /// A folder of this test's own, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("qcode-service-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn commands(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Run(command) => Some(command.written()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_systemd_units_are_exactly_these() {
        let host = linux(Path::new("/tmp/x"));
        let files = host.files();
        assert_eq!(files[0].0, Path::new("/tmp/x/config/systemd/user/qcode-reaper.service"));
        assert_eq!(
            files[0].1,
            "[Unit]\nDescription=Stops the containers QCode started once no QCode is open\n\n\
             [Service]\nType=exec\nExecStart=\"/home/ada/.cargo/bin/qcode\" reaper\n"
        );
        assert_eq!(files[1].0, Path::new("/tmp/x/config/systemd/user/qcode-reaper.path"));
        assert_eq!(
            files[1].1,
            "[Unit]\nDescription=Watches the list of containers QCode started\n\n\
             [Path]\nPathChanged=/home/ada/.local/share/quvyta/code/containers.toml\nUnit=qcode-reaper.service\n\n\
             [Install]\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn a_path_systemd_would_expand_or_split_is_written_so_it_is_not() {
        let mut host = linux(Path::new("/tmp/x"));
        host.program = PathBuf::from("/home/ada/My \"Apps\" 100%/$bin\\qcode");
        host.registry = PathBuf::from("/home/ada/50% data/containers.toml");
        let files = host.files();
        assert!(
            files[0].1.contains("ExecStart=\"/home/ada/My \\\"Apps\\\" 100%%/$$bin\\\\qcode\" reaper\n"),
            "{}",
            files[0].1
        );
        assert!(files[1].1.contains("PathChanged=/home/ada/50%% data/containers.toml\n"), "{}", files[1].1);
    }

    #[test]
    fn the_launchd_agent_is_exactly_this() {
        let host = mac(Path::new("/tmp/x"));
        let files = host.files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, Path::new("/tmp/x/Library/LaunchAgents/io.quvyta.code.reaper.plist"));
        assert_eq!(
            files[0].1,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>Label</key>\n\t<string>io.quvyta.code.reaper</string>\n\
             \t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/opt/homebrew/bin/qcode</string>\n\t\t<string>reaper</string>\n\t</array>\n\
             \t<key>WatchPaths</key>\n\t<array>\n\t\t<string>/Users/ada/Library/Application Support/quvyta/code/containers.toml</string>\n\t</array>\n\
             </dict>\n</plist>\n"
        );
        let mut odd = host;
        odd.program = PathBuf::from("/Users/a&b/<qcode>");
        assert!(odd.plist().contains("<string>/Users/a&amp;b/&lt;qcode&gt;</string>"), "{}", odd.plist());
    }

    #[test]
    fn installing_on_linux_writes_both_units_then_enables_the_watcher() {
        let scratch = Scratch::new("linux-install");
        let host = linux(&scratch.0);
        let steps = host.install();
        assert_eq!(
            commands(&steps),
            ["systemctl --user daemon-reload", "systemctl --user enable --now qcode-reaper.path"]
        );
        assert!(!host.is_installed());
        let mut ran = Vec::new();
        perform(&steps, &mut |command| {
            ran.push(command.written());
            Ok(())
        })
        .expect("everything is written");
        assert_eq!(ran, commands(&steps), "the commands run after the files are there, in order");
        assert!(host.is_installed());
        for (path, text) in host.files() {
            assert_eq!(std::fs::read_to_string(path).expect("written"), text);
        }
    }

    #[test]
    fn removing_on_linux_stops_first_then_takes_the_files_away() {
        let scratch = Scratch::new("linux-remove");
        let host = linux(&scratch.0);
        perform(&host.install(), &mut |_| Ok(())).expect("installed");
        let steps = host.uninstall();
        assert_eq!(
            commands(&steps),
            [
                "systemctl --user disable --now qcode-reaper.path",
                "systemctl --user stop qcode-reaper.service",
                "systemctl --user daemon-reload",
            ]
        );
        assert!(matches!(steps.last(), Some(Step::Run(_))), "the reload comes after the files are gone");
        perform(&steps, &mut |_| Ok(())).expect("removed");
        assert!(!host.is_installed());
        assert!(host.files().iter().all(|(path, _)| !path.exists()));
        perform(&steps, &mut |_| Ok(())).expect("removing what is gone already is not a failure");
    }

    #[test]
    fn launchd_is_asked_in_the_persons_own_domain() {
        let scratch = Scratch::new("mac");
        let host = mac(&scratch.0);
        let plist = scratch.0.join("Library/LaunchAgents/io.quvyta.code.reaper.plist");
        assert_eq!(commands(&host.install()), [format!("launchctl bootstrap gui/501 {}", plist.display())]);
        assert_eq!(commands(&host.uninstall()), ["launchctl bootout gui/501/io.quvyta.code.reaper"]);
        perform(&host.install(), &mut |_| Ok(())).expect("installed");
        assert!(host.is_installed());
        perform(&host.uninstall(), &mut |_| Ok(())).expect("removed");
        assert!(!plist.exists());
    }

    #[test]
    fn a_command_that_fails_stops_the_rest_with_its_own_words() {
        let scratch = Scratch::new("fail");
        let host = linux(&scratch.0);
        let mut ran = 0;
        let failed = perform(&host.install(), &mut |command| {
            ran += 1;
            Err(format!("{}: Failed to connect to bus: No medium found", command.written()))
        });
        assert_eq!(failed, Err("systemctl --user daemon-reload: Failed to connect to bus: No medium found".to_owned()));
        assert_eq!(ran, 1, "nothing is enabled after a reload that failed");
    }

    #[cfg(unix)]
    #[test]
    fn a_host_command_that_fails_answers_with_what_it_printed() {
        let failed = run_host(&HostCommand::new("/bin/sh", &["-c", "echo nope >&2; exit 1"]));
        assert_eq!(failed, Err("/bin/sh -c echo nope >&2; exit 1: nope".to_owned()));
        assert_eq!(run_host(&HostCommand::new("/bin/sh", &["-c", "true"])), Ok(()));
        assert!(run_host(&HostCommand::new("/nonexistent/qcode-test-ctl", &[])).is_err());
    }
}
