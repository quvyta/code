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

/// Who owns a private filesystem the engine makes inside a container.
///
/// A window's runtime directory has to belong to whoever runs inside, or the application refuses
/// to use it; a tmpfs belongs to root until it is said otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TmpfsOwner {
    /// Podman's `U`: give it to the user the container is mapped to, whoever that turns out to be.
    Mapped,
    /// Docker's `uid=` and `gid=`: the ids have to be named, as everywhere on docker.
    Ids,
}

/// Whether the engine needs to be handed a seccomp profile for a container whose program sets up
/// sandboxes of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sandboxing {
    /// Podman rootless already allows a nested unprivileged user namespace, so its own default
    /// profile is left in place: the application's sandbox came up under it untouched.
    Default,
    /// Docker's default profile allows `unshare`, `setns` and a namespace-flagged `clone` only to
    /// CAP_SYS_ADMIN, so a Chromium-based application cannot build its sandbox and dies. It is
    /// given a profile that is the default plus those three.
    NeedsProfile,
}

/// The differences between the engines, as values rather than code.
pub(crate) struct Dialect {
    /// The binary to look for, without the platform's executable suffix.
    pub(crate) binary: &'static str,
    /// How the host user reaches the container.
    pub(crate) user_mapping: UserMapping,
    /// What to append to a mount's options.
    pub(crate) relabel: Relabel,
    /// How a private filesystem is given to the user inside.
    pub(crate) tmpfs_owner: TmpfsOwner,
    /// Whether a program that builds its own sandbox needs a seccomp profile passed in.
    pub(crate) sandboxing: Sandboxing,
}

const PODMAN: Dialect = Dialect {
    binary: "podman",
    user_mapping: UserMapping::KeepId,
    relabel: Relabel::Shared,
    tmpfs_owner: TmpfsOwner::Mapped,
    sandboxing: Sandboxing::Default,
};

const DOCKER: Dialect = Dialect {
    binary: "docker",
    user_mapping: UserMapping::Ids,
    relabel: Relabel::No,
    tmpfs_owner: TmpfsOwner::Ids,
    sandboxing: Sandboxing::NeedsProfile,
};

impl EngineKind {
    /// The one place in QCode that branches on which engine is in use.
    pub(crate) fn dialect(self) -> &'static Dialect {
        match self {
            Self::Podman => &PODMAN,
            Self::Docker => &DOCKER,
        }
    }
}
