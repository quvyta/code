//! Signing in from a window: how the address the application wants to open reaches the person's
//! own browser.
//!
//! An application in a container asks the desktop to open a web address the way every Linux
//! program does, by running `xdg-open`. Inside a container that is nobody: there is no browser,
//! and until this was measured there was not even an `xdg-open`, so the call returned success and
//! did nothing at all — the person pressed "sign in" and the machine stayed silent.
//!
//! So the image carries `xdg-utils` and a program of QCode's own, and the container is told
//! through `BROWSER` to use it. That program writes the address into a folder the window's
//! container shares with QCode, and QCode opens it in the person's real browser, where their
//! account is already signed in. Nothing of the machine is handed to the container for this: the
//! folder is the workspace's own, and the only thing that travels through it is a line of text.
//!
//! The address is written to a temporary name and moved into place, so QCode never reads half a
//! line; and every address QCode takes is removed as it is read, so a folder left behind cannot
//! open yesterday's page tomorrow.
//!
//! Opening the address on this machine is the one thing here that reaches out of QCode, and it
//! goes through the framework rather than being spawned from our own code: `Command::open` is the
//! single door to the desktop, and a test run records the opening instead of carrying it out, so
//! no test can open a browser on the person's screen.
//!
//! That opening never takes the screen. The browser starts beside QCode with its streams thrown
//! away and a process group of its own, so a cold browser that takes half a minute to come up
//! holds nothing back and its warnings are never drawn over the page the person is reading. Only
//! an `http` or `https` address is handed over at all; everything else is said on the tab instead.

use std::path::{Path, PathBuf};

/// The folder the window's container writes addresses into, seen from inside that container.
pub const OPEN_DIR: &str = "/run/qcode-open";

/// The program the container runs to open an address, seen from inside the container.
pub const OPEN_PROGRAM: &str = "/usr/local/bin/qcode-open";

/// What marks a file in that folder as an address waiting to be opened.
const SUFFIX: &str = ".url";

/// The program the image installs at [`OPEN_PROGRAM`].
///
/// It is a shell script rather than anything larger because the base image is the only thing it
/// may rely on. It writes beside the final name and moves it into place so that a reader never
/// sees half an address, and it says nothing on its output: whatever it printed would land in the
/// application's own log, not in front of the person.
pub const SCRIPT: &str = "#!/bin/sh\n\
                          # Hands the address QCode, which opens it in the person's own browser.\n\
                          set -eu\n\
                          [ -n \"${1:-}\" ] || exit 1\n\
                          [ -d \"$0_DIR\" ] || exit 1\n\
                          name=\"$0_DIR/$(date +%s%N)\"\n\
                          printf '%s\\n' \"$1\" > \"$name.part\"\n\
                          mv \"$name.part\" \"$name.url\"\n";

/// The script with its folder filled in, ready to be written into an image.
#[must_use]
pub fn script() -> String {
    SCRIPT.replace("$0_DIR", OPEN_DIR)
}

/// Takes every address waiting in `folder`, oldest first, and removes each one as it is taken.
///
/// A file that cannot be read is removed too rather than left to be tried again forever; an
/// address the person never sees is better than a tab that keeps stumbling over the same file.
#[must_use]
pub fn taken(folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_str().is_some_and(|name| name.ends_with(SUFFIX)))
        .collect();
    // The names count nanoseconds, so their order is the order they were written in.
    files.sort();
    let mut addresses = Vec::new();
    for file in files {
        let read = std::fs::read_to_string(&file);
        let _ = std::fs::remove_file(&file);
        if let Ok(text) = read {
            let address = text.trim().to_owned();
            if !address.is_empty() {
                addresses.push(address);
            }
        }
    }
    addresses
}

/// Whether an address is one QCode will hand a browser.
///
/// Only `http` and `https`. The container is on the other side of this folder, and a line of text
/// from it must never become a program to run or a file to open: `file:`, `javascript:` and
/// anything else are refused and said aloud rather than opened quietly.
#[must_use]
pub fn is_web(address: &str) -> bool {
    let lower = address.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://")) && !address.contains(['\n', '\r', '\0'])
}

#[cfg(test)]
mod tests {
    use super::{is_web, script, taken};
    use std::path::PathBuf;

    fn folder(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-signin-{name}-{stamp}"));
        std::fs::create_dir_all(&path).expect("a folder of this test's own");
        path
    }

    #[test]
    fn the_script_names_the_folder_and_moves_the_address_into_place() {
        let text = script();
        assert!(text.starts_with("#!/bin/sh\n"), "{text}");
        assert!(text.contains(super::OPEN_DIR), "the folder is filled in: {text}");
        assert!(!text.contains("$0_DIR"), "nothing is left to fill in: {text}");
        assert!(text.contains(".part\"\nmv "), "it moves the finished file into place: {text}");
    }

    #[test]
    fn addresses_are_taken_oldest_first_and_never_twice() {
        let dir = folder("taken");
        std::fs::write(dir.join("200.url"), "https://example.com/second\n").expect("an address");
        std::fs::write(dir.join("100.url"), "https://example.com/first\n").expect("an address");
        // Not an address yet: the writer has not moved it into place.
        std::fs::write(dir.join("300.url.part"), "https://example.com/half").expect("a half-written file");
        assert_eq!(taken(&dir), ["https://example.com/first", "https://example.com/second"]);
        assert_eq!(taken(&dir), Vec::<String>::new(), "each one is taken once");
        assert!(dir.join("300.url.part").exists(), "a half-written file is left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_or_unreadable_file_leaves_nothing_behind() {
        let dir = folder("empty");
        std::fs::write(dir.join("100.url"), "   \n").expect("an empty address");
        assert_eq!(taken(&dir), Vec::<String>::new());
        assert!(!dir.join("100.url").exists(), "it is not tried again forever");
        assert_eq!(taken(&folder("none").join("gone")), Vec::<String>::new(), "a folder that is not there is quiet");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_web_addresses_are_opened() {
        assert!(is_web("https://accounts.google.com/o/oauth2/auth?client_id=x"));
        assert!(is_web("http://localhost:45049/oauth-callback"));
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "antigravity-ide://open",
            "ssh://host",
            "https://example.com/\nfile:///etc/passwd",
            "",
        ] {
            assert!(!is_web(refused), "{refused}");
        }
    }
}
