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
//! folder is the project's own, and the only thing that travels through it is a line of text.
//!
//! The address is written to a temporary name and moved into place, so QCode never reads half a
//! line; and every address QCode takes is removed as it is read, so a folder left behind cannot
//! open yesterday's page tomorrow.
//!
//! Opening the address on this machine is the one thing here that reaches out of QCode, and it
//! goes through the framework's handoff rather than being spawned from our own code: the handoff
//! is the single door to the desktop, and a test run records it instead of running it, so no test
//! can open a browser on the person's screen.
//!
//! What the handoff runs is a shell rather than `xdg-open` itself, for two reasons. `xdg-open`
//! with a cold browser does not return until that browser ends, and a handoff waiting for it
//! would hold QCode's screen for the whole session; and a browser that inherited the terminal
//! would draw its warnings over the screen we just took back. The shell starts `xdg-open` in the
//! background with both of its streams thrown away and ends at once, so the screen comes back in
//! milliseconds. The address never enters the script's text — it is handed over as an argument,
//! where nothing in it can become a command.

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

/// The program a handoff runs to open an address here. See the module's own words for why it is
/// a shell and not `xdg-open`.
pub const OPEN_HERE: &str = "sh";

/// What that shell runs: `xdg-open` in the background with its streams thrown away, and nothing
/// left for the handoff to wait for. `$0` is the address, handed over as an argument.
///
/// A machine without `xdg-open` ends with a code of its own, so the tab can say the address was
/// not opened rather than claim a browser that never came up.
const OPEN_HERE_SCRIPT: &str = "command -v xdg-open >/dev/null 2>&1 || exit 1\n\
                                xdg-open \"$0\" >/dev/null 2>&1 &\n\
                                exit 0\n";

/// The arguments that open `address` in the browser of the person at this machine, or `None`
/// when it is not an address QCode will hand a browser.
///
/// The caller puts these into a detached handoff; the tab shows the address either way, so a
/// person whose machine opened nothing can still read it.
#[must_use]
pub fn open_here(address: &str) -> Option<[String; 3]> {
    is_web(address).then(|| ["-c".to_owned(), OPEN_HERE_SCRIPT.to_owned(), address.to_owned()])
}

#[cfg(test)]
mod tests {
    use super::{OPEN_HERE_SCRIPT, is_web, open_here, script, taken};
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
            assert!(open_here(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn the_address_is_an_argument_of_the_opening_and_never_part_of_its_script() {
        // A line of text from the container must not become a command, so it is handed over as
        // an argument and the script is the same every time.
        let address = "https://example.com/; rm -rf $HOME";
        let words = open_here(address).expect("a web address is opened");
        assert_eq!(words, ["-c".to_owned(), OPEN_HERE_SCRIPT.to_owned(), address.to_owned()]);
        assert!(!OPEN_HERE_SCRIPT.contains("example.com"), "the address is not in the script");
        // Nothing is left running for a waiter to hold the screen for.
        assert!(OPEN_HERE_SCRIPT.contains(">/dev/null 2>&1 &"), "{OPEN_HERE_SCRIPT}");
        assert!(OPEN_HERE_SCRIPT.ends_with("exit 0\n"), "{OPEN_HERE_SCRIPT}");
    }
}
