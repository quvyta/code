//! Where things are inside every QCode container, what a container runs to stay up, and the shell
//! that gives the person running it a name when the image has none for them.
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

/// The shell that gives the person a name inside a container whose image has none for them, run as
/// root in the container once it is up.
///
/// On docker the daemon runs the container as the ids QCode hands it (`--user uid:gid`), and an
/// image can only name the uid it was built with: [`USER`] at 1000. A person whose uid is another
/// one then has no name inside, and everything that looks one up fails on it — `whoami` cannot
/// answer, `git commit` refuses to write an author, Node's `os.userInfo()` throws. The home
/// directory and the person's own files are right; only the name is missing. Podman rootless maps
/// the person into the container's user namespace and writes that entry itself, so nothing is
/// missing there and nothing here runs.
///
/// Run as `sh -c <this> sh <uid> <gid>`, with the ids as arguments rather than written into the
/// script, and the two files after them so that the same shell can be run against a copy of them:
/// `passwd` and `group` default to `/etc/passwd` and `/etc/group`.
///
/// Each line is looked for by the id in its own field and not by the name beside it, because the
/// name is what this writes and the image may have written another one: so an image that already
/// knows the uid is left as it is, and a container brought up twice is written once.
///
/// Nothing but `sh`, `grep` and `echo` is used, because every base QCode offers is somebody else's
/// and none of the four carries the same set of programs.
pub const NAME_THE_USER: &str = "passwd=${3:-/etc/passwd}; group=${4:-/etc/group}; \
     grep -q \"^[^:]*:[^:]*:$2:\" \"$group\" || echo \"qcode-$2:x:$2:\" >> \"$group\"; \
     grep -q \"^[^:]*:[^:]*:$1:\" \"$passwd\" || echo \"qcode-$1:x:$1:$2:QCode:/home/qcode:/bin/sh\" >> \"$passwd\"";

#[cfg(test)]
mod tests {
    use super::{
        ASSETS_DIR, BACKUP_DIR, CODE_DIR, HOME_DIR, KEEP_ALIVE, LEGACY_CODE_DIR, MCP_DIR, NAME_THE_USER, OPEN_HOME,
        USER,
    };
    use crate::base::CONTAINERFILE;
    #[cfg(unix)]
    use std::{
        ffi::OsString,
        path::{Path, PathBuf},
    };

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

    #[test]
    fn the_entry_the_person_is_given_is_of_the_contract_s_own_home_and_shell() {
        // The line is the one the image gives the user it was built with, with the ids in it: a
        // name of QCode's own, the home the image opens and a shell every base carries. An entry
        // naming another home sends a harness's login where nothing reads it, and one whose shell
        // a base does not have stops the person at their first command.
        let entry = format!("{USER}-$1:x:$1:$2:QCode:{HOME_DIR}:/bin/sh");
        assert!(NAME_THE_USER.contains(&entry), "{NAME_THE_USER}");
    }

    /// The shell run for real, against a copy of the two files it writes, so what it appends and
    /// what it leaves alone is read here rather than believed. Bounded like every other command
    /// QCode runs, and through the runner the engine layer uses, so a shell that does not answer
    /// fails the test rather than hanging the run.
    #[cfg(unix)]
    fn named(folder: &Path, uid: u32, gid: u32) {
        let (uid, gid) = (uid.to_string(), gid.to_string());
        let mut args: Vec<OsString> = ["-c", NAME_THE_USER, "sh", &uid, &gid].iter().map(OsString::from).collect();
        args.push(folder.join("passwd").into_os_string());
        args.push(folder.join("group").into_os_string());
        crate::engine::run::capture(&crate::engine::EngineCommand {
            program: PathBuf::from("/bin/sh"),
            args,
            deadline: Some(crate::engine::ANSWER_WITHIN),
        })
        .expect("the shell answers");
    }

    /// The image names uid 1000 and nobody else, and the container is started again every time it
    /// is stopped, so the two lines are written once and once only. A password file with two names
    /// for one uid sends whichever lookup comes second to the wrong home.
    #[cfg(unix)]
    #[test]
    fn the_naming_shell_writes_one_line_per_file_and_leaves_an_entry_the_image_already_knows() {
        use crate::engine::scratch::Scratch;

        let scratch = Scratch::new("name-the-user").expect("a folder of its own");
        let (passwd, group) = (scratch.path().join("passwd"), scratch.path().join("group"));
        // The two files as the image's own build leaves them: `useradd` was given no comment, so
        // the entry QCode gave uid 1000 carries no name of its own either.
        std::fs::write(&passwd, "root:x:0:0:root:/root:/bin/sh\nqcode:x:1000:1000::/home/qcode:/bin/bash\n")
            .expect("a copy of the image's password file");
        std::fs::write(&group, "root:x:0:\nqcode:x:1000:\n").expect("a copy of the image's group file");

        named(scratch.path(), 1234, 1234);
        named(scratch.path(), 1234, 1234);

        let entry = format!("qcode-1234:x:1234:1234:QCode:{HOME_DIR}:/bin/sh");
        let written = std::fs::read_to_string(&passwd).expect("the password file was read");
        assert_eq!(written.matches(&entry).count(), 1, "{written}");
        let written = std::fs::read_to_string(&group).expect("the group file was read");
        assert_eq!(written.matches("qcode-1234:x:1234:").count(), 1, "{written}");

        // The very common case is the one the image already answers for, and nothing is written
        // into it: a second name for uid 1000 would send the person to a home that is not theirs.
        let (before, was) =
            (std::fs::read_to_string(&passwd).expect("read"), std::fs::read_to_string(&group).expect("read"));
        named(scratch.path(), 1000, 1000);
        assert_eq!(std::fs::read_to_string(&passwd).expect("read"), before);
        assert_eq!(std::fs::read_to_string(&group).expect("read"), was);
    }
}
