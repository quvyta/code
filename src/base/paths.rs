//! Where things are inside every QCode container, and what a container runs to stay up.
//!
//! These are one contract with one owner. The image below makes the directories and hands them
//! to whoever the container runs as; the mounts, the working directory and the keep-alive
//! command of every layer above are read from here. Written out twice, they drift: an image that
//! opens `/home/qcode` and a mount that lands on `/home/user` cost a login that vanishes, and
//! nothing fails loudly enough to say so.

/// Where the workspace's own files appear, mounted from the host and always writable. It is the
/// working directory of everything a container runs, and it carries the name the person sees the
/// folder under at home, `Work/`, so the two halves read the same.
pub const CODE_DIR: &str = "/work";

/// What a QCode written before this mounted the workspace's own files on.
///
/// Nothing is mounted here any more, but every conversation the harnesses recorded before the
/// change names this path as the directory it was had in, so the history scripts look for a
/// workspace under this name as well as under [`CODE_DIR`]. Without it a person who has used
/// QCode before opens the history list and finds it empty.
pub const LEGACY_CODE_DIR: &str = "/work/Project";

/// Where the workspace's other material appears, mounted from the host, writable or not as the
/// profile says.
pub const ASSETS_DIR: &str = "/assets";

/// Where a workspace's `Backup/` folder appears in the one-off container that takes or restores a
/// backup. No lasting container mounts it: a harness that could write there could rewrite the
/// history it would be restored from.
pub const BACKUP_DIR: &str = "/backup";

/// The home directory of whoever the container runs as, and the one place a harness keeps its
/// login, its settings, its history and its memory.
///
/// A profile container mounts the workspace's own home volume here, so none of that reaches
/// another workspace.
pub const HOME_DIR: &str = "/home/qcode";

/// Where a profile container sees the workspace's `Containers/MCP/` folder: the socket QCode
/// answers the bridge between tabs on, and the server a harness starts to reach it.
///
/// Outside the home directory, and outside [`CODE_DIR`] and [`ASSETS_DIR`], which are the
/// workspace's own material. It is mounted read-only: the container talks to the socket,
/// which a read-only mount allows, and may not replace the server every harness of the workspace
/// starts. The image does not make it; the engine makes the place a mount lands on.
pub const MCP_DIR: &str = "/run/qcode-mcp";

/// The user the image belongs to.
///
/// Not who the container runs as on Linux — that is the person's own user — but who owns what
/// the image build leaves behind, and the name a tool finds when it looks uid 1000 up.
pub const USER: &str = "qcode";

/// What a lasting container runs so that it stays up between tabs.
///
/// Tabs enter a container that is already running, so it needs something that never ends and
/// costs nothing, and it has to go away the moment it is told to. The two do not come for free
/// together: the command is the container's first process, and a first process only sees a
/// signal it has a handler for, so a bare `sleep` or a bare loop ignores the engine's stop and
/// is killed only after the engine's own timeout, ten seconds later, every time a container is
/// stopped or removed. So the loop traps the stop signal and leaves at once, and each `sleep`
/// is put in the background and waited for, because a trap fires while the shell waits but not
/// while it runs a child.
///
/// A shell loop rather than `sleep infinity`: `sleep` in the image is the one from coreutils,
/// which takes `infinity`, but the loop is what every shell understands and it is the
/// container's whole purpose, so it is spelled out.
pub const KEEP_ALIVE: &[&str] = &["sh", "-c", "trap 'exit 0' TERM INT; while :; do sleep 3600 & wait $!; done"];

/// The program a Containerfile built on the base image runs as its last step, so that what the
/// build wrote into [`HOME_DIR`] can be written to by the user the container is later run as.
///
/// See the block that installs it in the Containerfile for what goes wrong without it.
pub const OPEN_HOME: &str = "qcode-open-home";

#[cfg(test)]
mod tests {
    use super::{ASSETS_DIR, BACKUP_DIR, CODE_DIR, HOME_DIR, KEEP_ALIVE, LEGACY_CODE_DIR, MCP_DIR, OPEN_HOME, USER};
    use crate::base::CONTAINERFILE;

    #[test]
    fn every_path_is_absolute_and_no_two_of_them_are_the_same_place() {
        for path in [CODE_DIR, ASSETS_DIR, BACKUP_DIR, HOME_DIR, MCP_DIR, LEGACY_CODE_DIR] {
            assert!(path.starts_with('/'), "{path}");
            assert!(!path.ends_with('/'), "{path}");
        }
        // Each mount is its own root, so none of them may sit inside another: a mount landing on
        // a directory of another mount hides it, and the workspace loses half its material.
        let roots = [CODE_DIR, ASSETS_DIR, BACKUP_DIR, HOME_DIR, MCP_DIR];
        for (index, path) in roots.iter().enumerate() {
            for other in roots.iter().skip(index + 1) {
                assert_ne!(path, other);
                assert!(!path.starts_with(&format!("{other}/")), "{path} is inside {other}");
                assert!(!other.starts_with(&format!("{path}/")), "{other} is inside {path}");
            }
        }
        assert_ne!(CODE_DIR, LEGACY_CODE_DIR, "the old name is only worth looking for while it differs");
    }

    #[test]
    fn the_image_makes_every_place_the_contract_names() {
        // The two halves of the contract are the constants and the image. A path changed in one
        // and not the other is a login written where nothing reads it, so they are checked
        // against each other rather than trusted.
        for path in [CODE_DIR, ASSETS_DIR, HOME_DIR] {
            assert!(CONTAINERFILE.contains(path), "the image never mentions {path}");
        }
        assert!(CONTAINERFILE.contains(&format!("WORKDIR {CODE_DIR}")), "a shell starts somewhere else");
        assert!(CONTAINERFILE.contains(&format!("ENV HOME={HOME_DIR}")), "the image names another home");
        assert!(CONTAINERFILE.contains(&format!("USER {USER}")), "the image belongs to another user");
        assert!(CONTAINERFILE.contains(&format!("/usr/local/bin/{OPEN_HOME}")), "the image installs no {OPEN_HOME}");
    }

    #[test]
    fn what_keeps_a_container_up_never_ends_and_needs_nothing_but_a_shell() {
        assert_eq!(KEEP_ALIVE.first(), Some(&"sh"));
        assert_eq!(KEEP_ALIVE.get(1), Some(&"-c"));
        let script = KEEP_ALIVE.get(2).expect("the loop is the third word");
        assert!(script.contains("while"), "{script}");
        assert!(script.contains("sleep"), "{script}");
    }

    #[test]
    fn what_keeps_a_container_up_leaves_the_moment_it_is_told_to() {
        // The first process of a container sees only the signals it handles, so a loop without
        // a trap costs the engine's whole timeout at every stop. The trap is the promise, and
        // the sleep in the background is what lets it fire.
        let script = KEEP_ALIVE.get(2).expect("the loop is the third word");
        assert!(script.contains("trap 'exit 0' TERM"), "{script}");
        assert!(script.contains("sleep 3600 & wait"), "{script}");
    }
}
