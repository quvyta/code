//! Everything podman and docker spell differently, kept as data.
//!
//! The two engines are not two behaviours, so there is no trait and no second implementation of
//! anything: one table holds the differences and [`EngineKind::dialect`] is the only place that
//! branches on which engine is in hand.

use super::EngineKind;

/// How an engine is told to run the container as the person sitting at the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UserMapping {
    /// `--userns=keep-id`. Podman is rootless: it already runs as the person, and keep-id maps
    /// that id onto the image's user inside the container's user namespace.
    KeepId,
    /// `--user <uid>:<gid>`. Docker's daemon runs as root, so the ids have to be named outright
    /// or everything written into a bind mount ends up owned by root.
    Ids,
}

/// What a mount option list ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relabel {
    /// Podman's `z`: relabel the mounted content so an SELinux host (Fedora, RHEL) lets the
    /// container read it. It is ignored where SELinux is not enforcing.
    Shared,
    /// Docker takes no relabel option on the platforms QCode targets.
    No,
}

/// The differences between the engines, as values rather than code.
pub(crate) struct Dialect {
    /// The binary to look for, without the platform's executable suffix.
    pub(crate) binary: &'static str,
    /// How the host user reaches the container.
    pub(crate) user_mapping: UserMapping,
    /// What to append to a mount's options.
    pub(crate) relabel: Relabel,
}

const PODMAN: Dialect = Dialect { binary: "podman", user_mapping: UserMapping::KeepId, relabel: Relabel::Shared };

const DOCKER: Dialect = Dialect { binary: "docker", user_mapping: UserMapping::Ids, relabel: Relabel::No };

impl EngineKind {
    /// The one place in QCode that branches on which engine is in use.
    pub(crate) fn dialect(self) -> &'static Dialect {
        match self {
            Self::Podman => &PODMAN,
            Self::Docker => &DOCKER,
        }
    }
}
