//! Signing in from a window: where the page the application wants opened is shown, and how the
//! sign-in finds its way back.
//!
//! An application in a container asks the desktop to open a web address the way every Linux
//! program does, by running `xdg-open`. Inside a container that is nobody: there is no browser,
//! and until this was measured there was not even an `xdg-open`, so the call returned success and
//! did nothing at all — the person pressed "sign in" and the machine stayed silent.
//!
//! So the image carries `xdg-utils` and a program of QCode's own, and the container is told
//! through `BROWSER` to use it. That program writes the address into a folder the window's
//! container shares with QCode, and nothing else: the folder is the workspace's own, and the only
//! thing that travels through it is a line of text.
//!
//! QCode then shows the page **inside the same container**, in a small browser window of its own.
//! That is the whole point of the place: Google's sign-in ends by sending the browser to a
//! `localhost` address the application listens on, and `localhost` is the container's. A browser
//! on the person's own machine lands on its own `localhost`, where nobody listens, and the
//! sign-in never completes. A browser in the container lands where the application is, and the
//! sign-in completes by itself, with no port carried across.
//!
//! The browser costs the image next to nothing: it is the application's own Electron — a whole
//! Chromium — started with a twenty-line program of QCode's instead of the application's. Electron
//! finds the program to run beside the executable it was started as, and resolves a symbolic link
//! to the real file first, which opened the editor itself when that was tried; a hard link is a
//! file of its own name and keeps [`BROWSER_DIR`] as the place Electron looks. The links are made
//! in the same build step that unpacks the archive, which is what keeps them free: made in a step
//! of their own, the layer copies the 200 MB executable.
//!
//! The price is that this browser's cookie jar starts empty: the first time, the person signs in
//! to Google there with their password and second step, once per profile in a workspace. The jar
//! is kept in the profile's home volume, so it is once.
//!
//! When that window cannot be started — an image built before it existed has none — the page is
//! opened in the person's own browser instead, through the framework rather than spawned from our
//! own code: `Command::open` is the single door to the desktop, and a test run records the opening
//! instead of carrying it out. A sign-in from there cannot come back, and the tab says so.
//!
//! The address is written to a temporary name and moved into place, so QCode never reads half a
//! line; and every address QCode takes is removed as it is read, so a folder left behind cannot
//! open yesterday's page tomorrow. Only an `http` or `https` address is shown at all; everything
//! else is said on the tab instead.

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
                          # Hands the address to QCode, which shows it in a window of this container.\n\
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

/// Where the sign-in window's browser lives in the image: hard links to the application's own
/// Electron and its files, beside a program of QCode's own.
pub const BROWSER_DIR: &str = "/opt/qcode-browser";

/// The browser's executable, a hard link to the application's.
pub const BROWSER_PROGRAM: &str = "/opt/qcode-browser/qcode-browser";

/// What Electron reads first: the program's name, which is also the folder its cookies are kept
/// in under `~/.config`, and the file it starts.
pub const BROWSER_PACKAGE: &str = r#"{ "name": "qcode-browser", "version": "1.0.0", "main": "main.js" }
"#;

/// The browser itself: one window, showing the address it was started with.
///
/// A second start with another address hands it to the window already open rather than opening a
/// second one, because two Chromiums cannot share one profile folder. The name of this program
/// and of Electron are taken out of the user agent, leaving the Chromium it is; Google's sign-in
/// is known to turn away browsers that call themselves embedded, and what is left is true.
///
/// Written without a backslash anywhere, so the one layer of escaping that takes it into the
/// image cannot change a character of it.
pub const BROWSER_MAIN: &str = r#"'use strict';
const { app, BrowserWindow } = require('electron');
const page = (words) => words.slice(1).reverse().find((word) => word.startsWith('https://') || word.startsWith('http://'));
app.userAgentFallback = app.userAgentFallback.split(' ').filter((word) => !word.startsWith('Electron/') && !word.startsWith('qcode-browser/')).join(' ');
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  let shown = null;
  app.on('second-instance', (event, words) => {
    const next = page(words);
    if (shown && next) {
      shown.loadURL(next);
      shown.show();
      shown.focus();
    }
  });
  app.whenReady().then(() => {
    shown = new BrowserWindow({ width: 560, height: 760, autoHideMenuBar: true });
    shown.setMenu(null);
    const first = page(process.argv);
    if (first) {
      shown.loadURL(first);
    }
  });
  app.on('window-all-closed', () => app.quit());
}
"#;

/// The shell line that writes `contents` to `path`, for a build step.
///
/// A build step is one line, so the text travels with its line breaks as `\n`, inside single
/// quotes, and `printf '%b'` turns them back into breaks.
#[must_use]
pub fn written(contents: &str, path: &str) -> String {
    let quoted = contents.replace('\\', "\\\\").replace('\'', "'\\''").replace('\n', "\\n");
    format!("printf '%b' '{quoted}' > '{path}'")
}

/// The commands that put the sign-in window's browser into an image, given the folder the
/// application was unpacked into and its executable's name there. They belong in the very build
/// step that unpacks it, after the unpacking, so that every link costs nothing.
///
/// Everything of the application is linked but its `resources`, which is where Electron would find
/// the application itself; that folder is QCode's own here. The set-user-id helper Chromium's
/// sandbox falls back to is linked too, and a hard link is the same file, bit and owner alike.
#[must_use]
pub fn browser_install(install_dir: &str, program: &str) -> Vec<String> {
    vec![
        format!("mkdir -p '{BROWSER_DIR}/resources/app'"),
        format!(
            "for part in '{install_dir}'/*; do [ \"${{part##*/}}\" = resources ] || cp -al \"$part\" '{BROWSER_DIR}/'; done"
        ),
        format!("mv '{BROWSER_DIR}/{program}' '{BROWSER_PROGRAM}'"),
        written(BROWSER_PACKAGE, &format!("{BROWSER_DIR}/resources/app/package.json")),
        written(BROWSER_MAIN, &format!("{BROWSER_DIR}/resources/app/main.js")),
        format!("test -x '{BROWSER_PROGRAM}'"),
    ]
}

/// What an exec into the window's container answers when its image has no sign-in window.
pub const NO_BROWSER: &str = "qcode-no-browser";

/// The command, run inside the window's container, that shows `address` in the sign-in window,
/// started with the application's own `flags` so that it reaches the same compositor the same
/// way.
///
/// The browser is left running on its own and not waited for: it lives as long as the person
/// keeps it open, or as long as the container does. An image with no browser says [`NO_BROWSER`]
/// rather than failing, so the one reason that has a remedy is told apart from an engine that
/// refused.
#[must_use]
pub fn page_command(flags: &[&str], address: &str) -> Vec<String> {
    let mut command = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        format!("[ -x \"$1\" ] || {{ echo {NO_BROWSER}; exit 0; }}; nohup \"$@\" </dev/null >/dev/null 2>&1 &"),
        "sh".to_owned(),
        BROWSER_PROGRAM.to_owned(),
    ];
    command.extend(flags.iter().map(|flag| (*flag).to_owned()));
    command.push(address.to_owned());
    command
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
    use super::{BROWSER_DIR, BROWSER_MAIN, BROWSER_PROGRAM, browser_install, is_web, page_command, script, taken};
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

    #[test]
    fn the_browser_is_the_applications_own_executable_linked_not_copied_beside_a_program_of_ours() {
        // A stand-in of the unpacked application: its executable, the sandbox helper, a data file
        // Electron needs beside the executable, and the application's own `resources`.
        let root = folder("browser");
        let app = root.join("app");
        std::fs::create_dir_all(app.join("resources/app")).expect("the application's resources");
        std::fs::create_dir_all(app.join("locales")).expect("a folder of data files");
        std::fs::write(app.join("antigravity-ide"), "#!/bin/sh\n").expect("an executable");
        std::fs::set_permissions(app.join("antigravity-ide"), std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("it can be run");
        std::fs::write(app.join("chrome-sandbox"), "helper").expect("the sandbox helper");
        std::fs::write(app.join("locales/en-US.pak"), "words").expect("a data file");
        std::fs::write(app.join("resources/app/package.json"), "{\"name\": \"the editor\"}").expect("the editor");

        // The commands as the image runs them, with only the two folders moved into this test's own.
        let browser = root.join("browser");
        let here = |command: &String| {
            command
                .replace(BROWSER_DIR, &browser.display().to_string())
                .replace("/opt/antigravity-ide", &app.display().to_string())
        };
        let commands: Vec<String> =
            browser_install("/opt/antigravity-ide", "antigravity-ide").iter().map(here).collect();
        let ran = std::process::Command::new("sh").arg("-c").arg(commands.join(" && ")).status().expect("a shell");
        assert!(ran.success(), "{commands:?}");

        use std::os::unix::fs::MetadataExt;
        let inode = |path: &std::path::Path| std::fs::metadata(path).expect("the file is there").ino();
        let program = browser.join(BROWSER_PROGRAM.rsplit('/').next().expect("a file name"));
        // A hard link, not a symbolic one: Electron resolves a symbolic link to the real file and
        // then runs the editor that lives beside it.
        assert_eq!(inode(&program), inode(&app.join("antigravity-ide")), "the executable is linked");
        assert!(!std::fs::symlink_metadata(&program).expect("it is there").file_type().is_symlink());
        assert_eq!(inode(&browser.join("chrome-sandbox")), inode(&app.join("chrome-sandbox")));
        assert_eq!(inode(&browser.join("locales/en-US.pak")), inode(&app.join("locales/en-US.pak")));
        // The program Electron finds beside it is ours, and the editor's is nowhere near it.
        let main = std::fs::read_to_string(browser.join("resources/app/main.js")).expect("our program");
        assert_eq!(main, BROWSER_MAIN, "written into the image exactly as it is here");
        let package = std::fs::read_to_string(browser.join("resources/app/package.json")).expect("its package");
        assert!(package.contains("\"qcode-browser\"") && package.contains("\"main.js\""), "{package}");
        assert!(!package.contains("editor"), "{package}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_browsers_program_survives_the_one_layer_of_escaping_unchanged() {
        assert!(!BROWSER_MAIN.contains('\\'), "a backslash would be read by printf");
        // The name that decides where its cookies are kept, in the profile's home volume.
        assert!(super::BROWSER_PACKAGE.contains("\"name\": \"qcode-browser\""));
        // One window per profile folder; a second address goes to the window already open.
        assert!(BROWSER_MAIN.contains("requestSingleInstanceLock"));
        assert!(BROWSER_MAIN.contains("'second-instance'"));
    }

    #[test]
    fn a_page_is_shown_by_the_sign_in_window_left_running_on_its_own() {
        let address = "https://accounts.google.com/o/oauth2/auth?a=1&redirect_uri=http%3A%2F%2Flocalhost%3A1";
        let command = page_command(&["--ozone-platform=wayland"], address);
        assert_eq!(command[..2], ["sh", "-c"]);
        assert_eq!(command[3..], ["sh", BROWSER_PROGRAM, "--ozone-platform=wayland", address]);

        // Run as the container would, with a stand-in browser: it starts with the address as one
        // word, and the shell does not wait for it.
        let dir = folder("page");
        let stand_in = dir.join("browser");
        let seen = dir.join("seen");
        std::fs::write(&stand_in, format!("#!/bin/sh\nsleep 1\nprintf '%s\\n' \"$@\" > '{}'\n", seen.display()))
            .expect("a stand-in browser");
        std::fs::set_permissions(&stand_in, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        let mut here = command.clone();
        here[4] = stand_in.display().to_string();
        let started = std::time::Instant::now();
        let out = std::process::Command::new(&here[0]).args(&here[1..]).output().expect("a shell");
        assert!(out.status.success());
        assert!(started.elapsed() < std::time::Duration::from_millis(900), "the browser is not waited for");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "a browser that is there says nothing");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !seen.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let words = std::fs::read_to_string(&seen).expect("the stand-in browser ran");
        assert_eq!(words, format!("--ozone-platform=wayland\n{address}\n"));

        // An image with no browser says so rather than failing.
        here[4] = dir.join("nothing-here").display().to_string();
        let out = std::process::Command::new(&here[0]).args(&here[1..]).output().expect("a shell");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), super::NO_BROWSER);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
