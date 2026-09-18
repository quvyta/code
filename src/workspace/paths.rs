//! Where the workspace lives by default, on Linux, macOS and Windows.
//!
//! Resolution is split in two so that all three platforms can be exercised on one machine:
//! [`HostDirs`] is plain data — the home directory, a hint and the text of a file — and
//! [`HostDirs::default_workspace`] is a pure function of it. Only [`HostDirs::detect`] touches
//! the real environment.

use std::path::{Path, PathBuf};
use std::process::Command;

use qframe::diagnostics::{Diagnostic, Location};

use super::Loaded;

/// The name of the workspace folder, the same in every language; only the folder above it
/// follows the platform and the user's language.
pub const WORKSPACE_DIR_NAME: &str = "QCode";

/// The file the XDG user directories are configured in, below the config directory.
const USER_DIRS_FILE: &str = "user-dirs.dirs";

/// The key of the documents directory, both as an environment variable and in [`USER_DIRS_FILE`].
const DOCUMENTS_KEY: &str = "XDG_DOCUMENTS_DIR";

/// The operating system the paths are resolved for.
///
/// Carried as data rather than read from `cfg!` at every step, so a test can ask for the
/// behaviour of a platform it is not running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Linux and the other systems that follow the XDG base directory specification.
    Linux,
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
}

impl Platform {
    /// The platform this program was compiled for.
    #[must_use]
    pub fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// The directories of the machine a workspace path is resolved from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDirs {
    /// Which platform's rules to follow.
    pub platform: Platform,
    /// The user's home directory, if there is one.
    pub home: Option<PathBuf>,
    /// The documents directory as the system itself already names it: the `XDG_DOCUMENTS_DIR`
    /// variable on Linux and the `Personal` known folder on Windows. Ignored on macOS, where
    /// the folder is always `Documents` below the home directory.
    pub documents_hint: Option<PathBuf>,
    /// The name and text of the XDG `user-dirs.dirs` file, read on Linux only.
    pub user_dirs: Option<(String, String)>,
}

impl HostDirs {
    /// Reads the directories of the machine this program runs on.
    #[must_use]
    pub fn detect() -> Self {
        let platform = Platform::host();
        let mut dirs = Self::detect_with(
            platform,
            |name| std::env::var_os(name).map(PathBuf::from),
            |path| std::fs::read_to_string(path).ok(),
        );
        if platform == Platform::Windows
            && dirs.documents_hint.is_none()
            && let Some(home) = dirs.home.clone()
        {
            dirs.documents_hint = windows_documents(&home);
        }
        dirs
    }

    /// Collects the directories the given platform keeps its answers in, reading environment
    /// variables through `env` and file contents through `read`.
    ///
    /// [`detect`](Self::detect) passes the real process environment and file system; a test
    /// passes its own, which is how all three platforms are covered on one machine.
    #[must_use]
    pub fn detect_with(
        platform: Platform,
        env: impl Fn(&str) -> Option<PathBuf>,
        read: impl Fn(&Path) -> Option<String>,
    ) -> Self {
        let present = |name: &str| env(name).filter(|path| !path.as_os_str().is_empty());
        let mut dirs = Self { platform, home: present("HOME"), documents_hint: None, user_dirs: None };
        match platform {
            Platform::Linux => {
                dirs.documents_hint = present(DOCUMENTS_KEY);
                let config = present("XDG_CONFIG_HOME")
                    .filter(|path| path.is_absolute())
                    .or_else(|| dirs.home.as_ref().map(|home| home.join(".config")));
                if let Some(config) = config {
                    let file = config.join(USER_DIRS_FILE);
                    dirs.user_dirs = read(&file).map(|text| (USER_DIRS_FILE.to_owned(), text));
                }
            }
            Platform::MacOs => {}
            Platform::Windows => {
                dirs.home = present("USERPROFILE").or_else(|| {
                    let drive = present("HOMEDRIVE")?;
                    let path = present("HOMEPATH")?;
                    Some(PathBuf::from(format!("{}{}", drive.display(), path.display())))
                });
            }
        }
        dirs
    }

    /// The documents directory these host directories point at.
    ///
    /// On Linux the `XDG_DOCUMENTS_DIR` variable wins, then the same key in `user-dirs.dirs`,
    /// then `Documents` below the home directory. macOS always uses `Documents`. Windows uses
    /// the known folder when there is one and `Documents` below the profile otherwise.
    ///
    /// Without a home directory there is no answer: the value is `None` and the diagnostic
    /// says so. Nothing here panics.
    #[must_use]
    pub fn documents(&self) -> Loaded<Option<PathBuf>> {
        let mut diagnostics = Vec::new();
        // What counts as absolute is the running system's rule, which is also the rule that
        // matters: the hint is read from that same system. On Windows the known folder is
        // absolute by construction, so only the Linux variable, which anyone may set, is checked.
        let usable_hint = match self.platform {
            Platform::Linux => self.documents_hint.as_ref().filter(|path| path.is_absolute()),
            Platform::MacOs => None,
            Platform::Windows => self.documents_hint.as_ref(),
        };
        if let Some(hint) = usable_hint {
            return Loaded { value: Some(hint.clone()), diagnostics };
        }
        let Some(home) = self.home.as_ref() else {
            diagnostics.push(Diagnostic::error(None, "no home directory: the default workspace cannot be placed"));
            return Loaded { value: None, diagnostics };
        };
        if self.platform == Platform::Linux
            && let Some((file, text)) = &self.user_dirs
            && let Some(path) = documents_from_user_dirs(file, text, home, &mut diagnostics)
        {
            return Loaded { value: Some(path), diagnostics };
        }
        Loaded { value: Some(home.join("Documents")), diagnostics }
    }

    /// The default workspace: the [`WORKSPACE_DIR_NAME`] folder inside
    /// [`documents`](Self::documents).
    #[must_use]
    pub fn default_workspace(&self) -> Loaded<Option<PathBuf>> {
        let found = self.documents();
        Loaded { value: found.value.map(|path| path.join(WORKSPACE_DIR_NAME)), diagnostics: found.diagnostics }
    }
}

/// Reads the documents directory out of an XDG `user-dirs.dirs` file.
///
/// The file is a shell fragment; the last assignment of the key wins, as it would in a shell.
/// A value that is not an absolute path becomes a located warning and no answer, so the caller
/// falls back instead of building a path out of nonsense.
fn documents_from_user_dirs(file: &str, text: &str, home: &Path, diagnostics: &mut Vec<Diagnostic>) -> Option<PathBuf> {
    let mut found = None;
    for line in lines(text) {
        let (key, value) = match line.text.split_once('=') {
            Some((key, value)) => (key.trim(), value),
            None => continue,
        };
        if key != DOCUMENTS_KEY {
            continue;
        }
        let at = line.start + (line.text.len() - value.len());
        let (raw, offset) = unquote(value);
        let location = || Some(Location::from_offset(file, text, at + offset));
        if raw.is_empty() {
            diagnostics.push(Diagnostic::warning(location(), format!("`{DOCUMENTS_KEY}` is empty; it is ignored")));
            found = None;
            continue;
        }
        let path = if let Some(rest) = raw.strip_prefix("$HOME") {
            home.join(rest.trim_start_matches('/'))
        } else {
            PathBuf::from(&raw)
        };
        if path.is_absolute() {
            found = Some(path);
        } else {
            diagnostics.push(Diagnostic::warning(
                location(),
                format!("`{DOCUMENTS_KEY}` must be an absolute path or start with `$HOME`; it is ignored"),
            ));
            found = None;
        }
    }
    found
}

/// One line of a file, with the byte offset it starts at.
struct Line<'a> {
    start: usize,
    text: &'a str,
}

/// The lines of `text` that carry something other than a comment.
fn lines(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut start = 0;
    text.split_inclusive('\n').filter_map(move |raw| {
        let at = start;
        start += raw.len();
        let text = raw.trim_end_matches(['\n', '\r']);
        (!text.trim_start().starts_with('#') && !text.trim().is_empty()).then_some(Line { start: at, text })
    })
}

/// Removes the shell quoting around a value and says how many bytes were dropped in front, so
/// a diagnostic can point at the value itself rather than at its quote.
fn unquote(value: &str) -> (String, usize) {
    let trimmed = value.trim_start();
    let offset = value.len() - trimmed.len();
    let Some(inner) = trimmed.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) else {
        return (trimmed.trim_end().to_owned(), offset);
    };
    let mut out = String::with_capacity(inner.len());
    let mut escaped = false;
    for character in inner.chars() {
        match character {
            '\\' if !escaped => escaped = true,
            _ => {
                out.push(character);
                escaped = false;
            }
        }
    }
    (out, offset + 1)
}

/// The `Personal` known folder of the current user, as the registry records it.
///
/// Read through `reg.exe` rather than the Win32 API: `SHGetKnownFolderPath` needs `unsafe`,
/// which this program forbids, and the registry value is what the API reads anyway. It is only
/// ever run on Windows, and a missing or unreadable value simply has no answer.
fn windows_documents(home: &Path) -> Option<PathBuf> {
    let output = Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders", "/v", "Personal"])
        .output()
        .ok()?;
    shell_folder(&String::from_utf8_lossy(&output.stdout), home)
}

/// Reads the path out of the output of a `reg query … /v Personal`, expanding the one variable
/// a `REG_EXPAND_SZ` value uses here.
fn shell_folder(output: &str, home: &Path) -> Option<PathBuf> {
    for line in output.lines() {
        let Some(rest) = line.split_once("Personal").map(|(_, rest)| rest.trim_start()) else {
            continue;
        };
        let expand = rest.starts_with("REG_EXPAND_SZ");
        let Some(value) = rest.strip_prefix("REG_EXPAND_SZ").or_else(|| rest.strip_prefix("REG_SZ")) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let value = if expand { value.replace("%USERPROFILE%", &home.display().to_string()) } else { value.to_owned() };
        return Some(PathBuf::from(value));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn linux(home: &str, user_dirs: Option<&str>) -> HostDirs {
        HostDirs {
            platform: Platform::Linux,
            home: Some(PathBuf::from(home)),
            documents_hint: None,
            user_dirs: user_dirs.map(|text| (String::from("user-dirs.dirs"), text.to_owned())),
        }
    }

    #[test]
    fn linux_reads_the_documents_directory_from_user_dirs() {
        let dirs = linux("/home/ada", Some("XDG_DOCUMENTS_DIR=\"$HOME/Belgeler\"\n"));
        let found = dirs.default_workspace();
        assert!(found.is_clean(), "{:?}", found.diagnostics);
        assert_eq!(found.value, Some(PathBuf::from("/home/ada/Belgeler/QCode")));
    }

    #[test]
    fn linux_accepts_an_absolute_documents_directory() {
        let dirs = linux("/home/ada", Some("# set by xdg\nXDG_DOCUMENTS_DIR=\"/srv/docs\"\n"));
        assert_eq!(dirs.default_workspace().value, Some(PathBuf::from("/srv/docs/QCode")));
    }

    #[test]
    fn the_environment_variable_wins_over_the_user_dirs_file() {
        let mut dirs = linux("/home/ada", Some("XDG_DOCUMENTS_DIR=\"$HOME/Belgeler\"\n"));
        dirs.documents_hint = Some(PathBuf::from("/tmp/docs"));
        assert_eq!(dirs.default_workspace().value, Some(PathBuf::from("/tmp/docs/QCode")));
    }

    #[test]
    fn linux_falls_back_to_the_english_documents_folder() {
        assert_eq!(
            linux("/home/ada", None).default_workspace().value,
            Some(PathBuf::from("/home/ada/Documents/QCode"))
        );
        let empty = linux("/home/ada", Some("XDG_DOWNLOAD_DIR=\"$HOME/İndirilenler\"\n"));
        assert_eq!(empty.default_workspace().value, Some(PathBuf::from("/home/ada/Documents/QCode")));
    }

    #[test]
    fn a_broken_user_dirs_entry_is_located_and_does_not_stop_the_fallback() {
        let text = "XDG_DESKTOP_DIR=\"$HOME/Masaüstü\"\nXDG_DOCUMENTS_DIR=\"Belgeler\"\n";
        let found = linux("/home/ada", Some(text)).default_workspace();
        assert_eq!(found.value, Some(PathBuf::from("/home/ada/Documents/QCode")));
        assert_eq!(found.diagnostics.len(), 1);
        assert_eq!(found.diagnostics[0].location.as_ref().map(ToString::to_string), Some("user-dirs.dirs:2:20".into()));

        let unset = linux("/home/ada", Some("XDG_DOCUMENTS_DIR=\"\"\n")).default_workspace();
        assert_eq!(unset.value, Some(PathBuf::from("/home/ada/Documents/QCode")));
        assert_eq!(unset.diagnostics.len(), 1, "an empty value disables the directory");
    }

    #[test]
    fn the_last_documents_entry_wins_as_it_would_in_a_shell() {
        let text = "XDG_DOCUMENTS_DIR=\"$HOME/A\"\nXDG_DOCUMENTS_DIR=\"$HOME/B\"\n";
        assert_eq!(linux("/home/ada", Some(text)).default_workspace().value, Some("/home/ada/B/QCode".into()));
    }

    #[test]
    fn macos_always_uses_the_home_documents_folder() {
        let dirs = HostDirs {
            platform: Platform::MacOs,
            home: Some(PathBuf::from("/Users/ada")),
            // A stray XDG variable must not move a macOS workspace.
            documents_hint: Some(PathBuf::from("/srv/docs")),
            user_dirs: None,
        };
        assert_eq!(dirs.default_workspace().value, Some(PathBuf::from("/Users/ada/Documents/QCode")));
    }

    #[test]
    fn windows_prefers_the_known_folder_and_falls_back_to_the_profile() {
        let mut dirs = HostDirs {
            platform: Platform::Windows,
            home: Some(PathBuf::from(r"C:\Users\ada")),
            documents_hint: Some(PathBuf::from(r"D:\OneDrive\Belgeler")),
            user_dirs: None,
        };
        // Built by joining rather than written out: the separator is the running system's.
        let expected = PathBuf::from(r"D:\OneDrive\Belgeler").join(WORKSPACE_DIR_NAME);
        assert_eq!(dirs.default_workspace().value, Some(expected));
        dirs.documents_hint = None;
        let expected = PathBuf::from(r"C:\Users\ada").join("Documents").join(WORKSPACE_DIR_NAME);
        assert_eq!(dirs.default_workspace().value, Some(expected));
    }

    #[test]
    fn a_missing_home_directory_is_a_diagnostic_and_not_a_panic() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let dirs = HostDirs { platform, home: None, documents_hint: None, user_dirs: None };
            let found = dirs.default_workspace();
            assert_eq!(found.value, None, "{platform:?}");
            assert_eq!(found.diagnostics.len(), 1, "{platform:?}");
        }
    }

    #[test]
    fn detection_reads_the_variables_each_platform_uses() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| PathBuf::from(*value))
        };
        let read = |path: &Path| {
            (path == Path::new("/home/ada/.config/user-dirs.dirs"))
                .then(|| "XDG_DOCUMENTS_DIR=\"$HOME/Belgeler\"\n".to_owned())
        };

        let linux = HostDirs::detect_with(Platform::Linux, env(&[("HOME", "/home/ada")]), read);
        assert_eq!(linux.home, Some(PathBuf::from("/home/ada")));
        assert_eq!(linux.default_workspace().value, Some(PathBuf::from("/home/ada/Belgeler/QCode")));

        let moved = env(&[("HOME", "/home/ada"), ("XDG_CONFIG_HOME", "/cfg")]);
        assert!(HostDirs::detect_with(Platform::Linux, moved, read).user_dirs.is_none(), "config home is honoured");

        let mac = HostDirs::detect_with(Platform::MacOs, env(&[("HOME", "/Users/ada")]), |_| None);
        assert_eq!(mac.default_workspace().value, Some(PathBuf::from("/Users/ada/Documents/QCode")));

        let split = env(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\ada")]);
        let windows = HostDirs::detect_with(Platform::Windows, split, |_| None);
        let expected = PathBuf::from(r"C:\Users\ada").join("Documents").join(WORKSPACE_DIR_NAME);
        assert_eq!(windows.default_workspace().value, Some(expected));
    }

    #[test]
    fn the_windows_known_folder_is_read_out_of_a_registry_query() {
        let plain = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders\r\n    Personal    REG_SZ    D:\\My Documents\r\n\r\n";
        let home = PathBuf::from(r"C:\Users\ada");
        assert_eq!(shell_folder(plain, &home), Some(PathBuf::from(r"D:\My Documents")));

        let expand = "    Personal    REG_EXPAND_SZ    %USERPROFILE%\\Belgeler\r\n";
        assert_eq!(shell_folder(expand, &home), Some(PathBuf::from(r"C:\Users\ada\Belgeler")));

        assert_eq!(shell_folder("ERROR: The system was unable to find the specified key\r\n", &home), None);
        assert_eq!(shell_folder("    Personal    REG_SZ    \r\n", &home), None);
    }

    #[test]
    fn the_folder_name_is_qcode_on_every_platform() {
        assert_eq!(WORKSPACE_DIR_NAME, "QCode");
    }
}
