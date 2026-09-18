//! Profiles: a harness, the template it is set up with, the account it signs in with and the
//! permissions it runs under.
//!
//! A profile is a definition file in the workspace (`Profiles/<profile>.toml`), an image built
//! from that definition, and a login that lives in a volume of its own. The modules here hold
//! the definition and the facts every harness brings with it; building images and running
//! containers is the engine's work.

pub mod identity;

mod definition;
mod harness;
#[cfg(test)]
mod harness_live;
#[cfg(test)]
mod live;
mod name;
mod template;

pub use definition::{Loaded, MountAccess, NetworkMode, Profile};
pub use harness::{AccountKind, ConfigFile, Harness, HarnessKind};
pub use name::SafeName;
pub use template::Template;
