//! What this machine offers a window, and what an engine has to be told before it opens one.
//!
//! A desktop harness draws on the person's own screen, so unlike every other container QCode
//! starts, its container needs something of the machine: the compositor's socket and, where there
//! is one, the graphics card. This module is the whole of what QCode looks at outside the
//! container, and it is deliberately small — the rule it keeps is that the container is given the
//! one socket file and nothing beside it.
//!
//! Why not the runtime folder itself: that folder also holds the engine's own socket, the session
//! bus, the sound server and the keyring. A container that could reach the engine's socket could
//! start any container it liked on the machine, with any mount. So the container gets a private
//! runtime folder of its own with a single file bind-mounted into it. The price is that a
//! compositor restart changes the socket and cuts an open window off; reopening the tab is the
//! way back, and the tab says so.

pub mod seccomp;

#[cfg(test)]
mod live;

use std::path::{Path, PathBuf};

/// Where a window's runtime folder is inside its container.
///
/// A fixed path rather than `/run/user/<uid>`: podman's `--userns=keep-id` leaves the person's own
/// id inside the container, docker's `--user` names it, and neither is the image's 1000 on every
/// machine. Nothing but the application reads this folder, and it only asks that the folder be
/// its own and unreadable to others, which the mode below gives it.
pub const RUNTIME_DIR: &str = "/run/qcode-display";

/// The permissions that runtime folder is made with. A compositor library warns and some
/// applications refuse when the folder is open to more than its owner.
pub const RUNTIME_MODE: &str = "0700";

/// How much shared memory a window's container gets.
///
/// The engines give 64 MB. The trial watched a browser engine fill that within seconds of
/// starting: the window stopped answering, the update check failed for want of resources, and the
/// program stopped answering the stop signal. With this it used 65 MB at startup and none of that
/// happened.
pub const SHM_SIZE: &str = "1g";

/// The graphics device a container is given when the machine has one.
pub const GRAPHICS_DEVICE: &str = "/dev/dri";

/// How long a window is given to close by itself before it is killed, in seconds.
///
/// The trial measured a clean close in 1.7 to 2.3 seconds, with the application writing its state
/// out; ten leaves room for a machine under load, and it is what the engines default to anyway. It
/// is stated rather than left to the engine so that it does not differ from machine to machine.
pub const WINDOW_GRACE: u32 = 10;

/// What this machine offers a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// The compositor's socket on this machine, the one file the container is given.
    pub socket: PathBuf,
    /// What that socket is called, which is both where it lands inside the container and what the
    /// application is told to look for.
    pub name: String,
    /// The graphics device, when the machine has one to lend. Without it the application draws in
    /// software, which the trial found perfectly usable; with it, it may not, and asking for
    /// hardware anyway is what left the window empty.
    pub device: Option<PathBuf>,
}

impl Display {
    /// Where the socket appears inside the container.
    #[must_use]
    pub fn target(&self) -> PathBuf {
        Path::new(RUNTIME_DIR).join(&self.name)
    }

    /// The variables the application is given so that it finds the socket and nothing else.
    #[must_use]
    pub fn environment(&self) -> Vec<(String, String)> {
        vec![("XDG_RUNTIME_DIR".to_owned(), RUNTIME_DIR.to_owned()), ("WAYLAND_DISPLAY".to_owned(), self.name.clone())]
    }
}

/// Why this machine cannot show a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoDisplay {
    /// The session names no compositor: there is no Wayland display to join. QCode does not fall
    /// back to X11, where every client on the screen can read every other client's windows and
    /// keys.
    NoWayland,
    /// The session names a compositor whose socket is not at that path.
    NoSocket(PathBuf),
}

/// What this machine offers a window, read from the session `qcode` was started in.
///
/// `runtime` is the runtime folder of the session and `wayland` the display it names — the two
/// variables a Wayland session sets. A display named as a bare name is a file in that folder,
/// which is how every compositor writes it; one named as a path is taken as it is, which a few
/// setups do. `device` is where the graphics device would be, so a test can point elsewhere.
///
/// Looks at the disk, which is all it does: everything else here is a value.
///
/// # Errors
///
/// When the session names no compositor, or names one whose socket is not there.
pub fn display(runtime: Option<&Path>, wayland: Option<&str>, device: &Path) -> Result<Display, NoDisplay> {
    let named = wayland.map(str::trim).filter(|name| !name.is_empty()).ok_or(NoDisplay::NoWayland)?;
    let path = match Path::new(named) {
        absolute if absolute.is_absolute() => absolute.to_path_buf(),
        relative => runtime.ok_or(NoDisplay::NoWayland)?.join(relative),
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| NoDisplay::NoSocket(path.clone()))?
        .to_owned();
    if !path.exists() {
        return Err(NoDisplay::NoSocket(path));
    }
    // Missing rather than unusable: a machine with no graphics card draws in software, and the
    // trial found no visible difference in the editor either way.
    let device = device.exists().then(|| device.to_path_buf());
    Ok(Display { socket: path, name, device })
}

/// What this machine offers a window, read from the variables of this process.
///
/// # Errors
///
/// As [`display`].
pub fn current() -> Result<Display, NoDisplay> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    display(runtime.as_deref(), wayland.as_deref(), Path::new(GRAPHICS_DEVICE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qcode-display-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        dir
    }

    #[test]
    fn a_named_display_is_a_file_of_the_session_folder() {
        let dir = folder("named");
        std::fs::write(dir.join("wayland-1"), "").expect("a stand-in socket");
        let found = display(Some(&dir), Some("wayland-1"), Path::new("/qcode-no-device")).expect("a display");
        assert_eq!(found.socket, dir.join("wayland-1"));
        assert_eq!(found.name, "wayland-1");
        assert_eq!(found.device, None, "a machine without the device draws in software");
        assert_eq!(found.target(), Path::new("/run/qcode-display/wayland-1"));
        assert_eq!(
            found.environment(),
            [
                ("XDG_RUNTIME_DIR".to_owned(), "/run/qcode-display".to_owned()),
                ("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned()),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_display_named_as_a_path_is_taken_as_it_is() {
        let dir = folder("path");
        let socket = dir.join("wayland-0");
        std::fs::write(&socket, "").expect("a stand-in socket");
        let found =
            display(None, Some(socket.to_str().expect("a path")), Path::new("/qcode-no-device")).expect("a display");
        assert_eq!(found.socket, socket);
        assert_eq!(found.name, "wayland-0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_graphics_device_is_lent_when_the_machine_has_one() {
        let dir = folder("device");
        std::fs::write(dir.join("wayland-1"), "").expect("a stand-in socket");
        let device = dir.join("dri");
        std::fs::create_dir(&device).expect("a stand-in device");
        let found = display(Some(&dir), Some("wayland-1"), &device).expect("a display");
        assert_eq!(found.device, Some(device));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_without_a_compositor_has_no_window_to_open() {
        let dir = folder("none");
        assert_eq!(display(Some(&dir), None, Path::new("/qcode-no-device")), Err(NoDisplay::NoWayland));
        assert_eq!(display(Some(&dir), Some("   "), Path::new("/qcode-no-device")), Err(NoDisplay::NoWayland));
        // A name with no folder to look in cannot be turned into a path either.
        assert_eq!(display(None, Some("wayland-1"), Path::new("/qcode-no-device")), Err(NoDisplay::NoWayland));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_compositor_that_left_says_which_socket_is_gone() {
        let dir = folder("gone");
        let expected = dir.join("wayland-9");
        assert_eq!(
            display(Some(&dir), Some("wayland-9"), Path::new("/qcode-no-device")),
            Err(NoDisplay::NoSocket(expected))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_runtime_folder_inside_is_the_containers_own_and_open_to_nobody_else() {
        assert!(RUNTIME_DIR.starts_with('/') && !RUNTIME_DIR.starts_with("/run/user"));
        assert_eq!(RUNTIME_MODE, "0700");
        // 64 MB is what both engines give and what the trial watched fill up.
        assert_eq!(SHM_SIZE, "1g");
    }
}
