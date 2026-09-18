//! The container engine layer: finding a working engine, spelling out its commands and running
//! them.
//!
//! Building a command is pure: a request goes in, a program and an argument list come out, so
//! every command is checked in tests without a container runtime anywhere near them. Running is
//! separate, in [`run`].

mod command;
mod detect;
mod dialect;
#[cfg(test)]
mod live;
pub mod names;
pub mod run;
mod state;

pub use command::{
    Access, ContainerCreate, CopyIn, CopyOut, EngineCommand, Exec, HostUser, ImageBuild, Mount, MountSource, Network,
};
pub use detect::{Unavailable, detect};
pub use state::{Container, ContainerState};

use std::path::{Path, PathBuf};

/// Which container engine: the two QCode supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    /// Rootless by default, no daemon.
    Podman,
    /// Talks to a daemon, which may be stopped while the binary is installed.
    Docker,
}

/// The word each engine is written with wherever QCode stores or reads a choice of engine.
///
/// The engine layer is where the two kinds are defined, so it is also where their names live:
/// a config file, a settings screen and the setup wizard all mean the same two words, and none
/// of them spells them out a second time.
const NAMES: [(EngineKind, &str); 2] = [(EngineKind::Podman, "podman"), (EngineKind::Docker, "docker")];

impl EngineKind {
    /// How this engine is written down.
    #[must_use]
    pub fn name(self) -> &'static str {
        NAMES.iter().find(|(known, _)| *known == self).map_or("podman", |(_, name)| *name)
    }

    /// The engine a stored word names, if it names one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        NAMES.iter().find(|(_, known)| *known == name).map(|(kind, _)| *kind)
    }
}

/// A container engine QCode can use: its kind and the binary that was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engine {
    kind: EngineKind,
    bin: PathBuf,
}

impl Engine {
    /// An engine of `kind` run through the binary at `bin`.
    #[must_use]
    pub fn new(kind: EngineKind, bin: impl Into<PathBuf>) -> Self {
        Self { kind, bin: bin.into() }
    }

    /// Which engine this is.
    #[must_use]
    pub fn kind(&self) -> EngineKind {
        self.kind
    }

    /// The binary every command is run through.
    #[must_use]
    pub fn bin(&self) -> &Path {
        &self.bin
    }
}
