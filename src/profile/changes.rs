//! What a container changed above its image, as the engine's `diff` lists it, told apart into the
//! home and the rest of the system.
//!
//! The engine lists every folder on the way to a changed file as changed too; only the ends of
//! those paths are counted, so installing one program is a handful of files and not every folder
//! above them. Places whose changes are nobody's work are left out: the scratch folders, the
//! workspace's own mounts, the package manager's lists and caches, the logs, and the files the
//! engine itself writes into `/etc` for every container.

use crate::base::paths::{ASSETS_DIR, CODE_DIR, HOME_DIR};

/// Folders whose changes are never counted: what is under them is not kept, not the person's
/// work, or not the container's at all.
const IGNORED: [&str; 12] = [
    "/tmp",
    "/var/tmp",
    "/run",
    "/proc",
    "/sys",
    "/dev",
    "/var/lib/apt/lists",
    "/var/cache/apt",
    "/var/cache/debconf",
    "/var/cache/npm",
    "/var/log",
    "/root/.npm",
];

/// Files both engines write into every container they make.
const ENGINE_FILES: [&str; 4] = ["/etc", "/etc/hosts", "/etc/hostname", "/etc/resolv.conf"];

/// What changed, as paths: the home's relative to the home, the system's as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Files and folders of the home that were added or changed, relative to the home.
    pub home: Vec<String>,
    /// Everything else that was added, changed or removed.
    pub system: Vec<String>,
}

impl Changes {
    /// Reads the engine's `diff`. A line it cannot make sense of is left out.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let entries: Vec<(char, &str)> = text
            .lines()
            .filter_map(|line| {
                let (kind, path) = line.split_once(' ')?;
                let kind = kind.chars().next().filter(|kind| matches!(kind, 'A' | 'C' | 'D'))?;
                path.starts_with('/').then_some((kind, path.trim_end_matches('/')))
            })
            .collect();
        let mut changes = Self::default();
        for (index, (kind, path)) in entries.iter().enumerate() {
            let parent_of_another =
                entries.iter().enumerate().any(|(other, (_, below))| other != index && is_below(below, path));
            if parent_of_another || ignored(path) {
                continue;
            }
            if let Some(relative) = path.strip_prefix(HOME_DIR).and_then(|rest| rest.strip_prefix('/')) {
                // A file removed from the home is not something to put back.
                if *kind != 'D' {
                    changes.home.push(relative.to_owned());
                }
            } else if *path != HOME_DIR {
                changes.system.push((*path).to_owned());
            }
        }
        changes
    }

    /// Whether nothing was changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.home.is_empty() && self.system.is_empty()
    }
}

/// Whether `path` is inside the folder `folder`.
fn is_below(path: &str, folder: &str) -> bool {
    path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// Whether a change at `path` is one nobody made on purpose.
fn ignored(path: &str) -> bool {
    ENGINE_FILES.contains(&path)
        || [CODE_DIR, ASSETS_DIR].into_iter().chain(IGNORED).any(|folder| path == folder || is_below(path, folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_installed_as_root_and_a_file_in_the_home_are_counted_apart_and_only_at_their_ends() {
        // As podman listed it after `apt-get install -y jq` as root and a note written in the
        // home, trimmed.
        let diff = "C /etc\nC /home\nC /home/qcode\nA /home/qcode/notes.txt\nC /usr\nC /usr/bin\nA /usr/bin/jq\n\
                    C /usr/lib\nA /usr/lib/x86_64-linux-gnu/libjq.so.1\nC /var/lib/apt/lists\n\
                    A /var/lib/apt/lists/deb.debian.org_debian_dists_trixie_InRelease\nC /var/log\nA /var/log/apt\n\
                    C /tmp\nA /tmp/qcode-shell-root\nC /var/lib/dpkg\nC /var/lib/dpkg/status\n";
        let changes = Changes::parse(diff);
        assert_eq!(changes.home, ["notes.txt"]);
        assert_eq!(changes.system, ["/usr/bin/jq", "/usr/lib/x86_64-linux-gnu/libjq.so.1", "/var/lib/dpkg/status"]);
    }

    #[test]
    fn a_container_nobody_worked_in_has_no_changes_on_either_engine() {
        // podman: the files it writes into /etc; both: the scratch folder.
        assert!(Changes::parse("C /etc\nC /tmp\nA /tmp/t\n").is_empty());
        assert!(Changes::parse("C /run\nA /run/qcode-mcp\nC /work\n").is_empty());
    }

    #[test]
    fn a_file_removed_from_the_system_counts_and_one_removed_from_the_home_is_not_put_back() {
        let changes = Changes::parse("C /etc\nD /etc/motd\nC /home/qcode\nD /home/qcode/old\n");
        assert_eq!(changes.system, ["/etc/motd"]);
        assert!(changes.home.is_empty());
    }
}
