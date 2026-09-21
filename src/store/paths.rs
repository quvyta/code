//! Where the store lives by default, and the platform the install commands are written for.
//!
//! The default place is the framework's answer for every application of the family:
//! `<Documents>/Quvyta/Code`, in the Documents folder under the name the person's desktop gave
//! it (`~/Belgeler/Quvyta/Code` on a Turkish Linux desktop). QCode does not work the Documents
//! folder out itself; [`HostDirs`] only carries the answer as data, so the wizard can be shown
//! on any machine a test describes and only [`HostDirs::detect`] reads the real one.

use std::path::PathBuf;

use qframe::storage::{Family, documents_dir};

/// QCode's name in its family, as the person reads it: the store is the folder of this name
/// under the family's folder in Documents.
pub const APP_TITLE: &str = "Code";

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

/// The places of the machine the wizard offers a store from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDirs {
    /// The store QCode offers when the person has not chosen one, or `None` on a machine
    /// without a home folder, where the person has to pick the place.
    pub store: Option<PathBuf>,
    /// The Documents folder, where the folder browser opens for a person who picks their own.
    pub documents: Option<PathBuf>,
}

impl HostDirs {
    /// The places of the machine this program runs on, as the framework finds them.
    #[must_use]
    pub fn detect() -> Self {
        Self { store: Family::QUVYTA.workspace_dir(APP_TITLE), documents: documents_dir() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run with a home folder of its own, so the Documents folder is the one this test's desktop
    /// names and never the developer's.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_default_store_follows_the_name_the_desktop_gave_documents() {
        const PRINT: &str = "QCODE_TEST_PRINT_FOLDER";
        if std::env::var_os(PRINT).is_some() {
            println!("store={}", HostDirs::detect().store.map(|path| path.display().to_string()).unwrap_or_default());
            return;
        }
        let home = crate::testing::scratch("desktop-home");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".config")).expect("folder");
        std::fs::write(home.join(".config/user-dirs.dirs"), "XDG_DOCUMENTS_DIR=\"$HOME/Belgeler\"\n").expect("file");

        let printed = crate::testing::in_child(
            "store::paths::tests::the_default_store_follows_the_name_the_desktop_gave_documents",
            &[(PRINT, "1".as_ref()), ("HOME", home.as_os_str())],
        );

        let expected = home.join("Belgeler").join("Quvyta").join("Code");
        assert!(printed.contains(&format!("store={}\n", expected.display())), "{printed}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
