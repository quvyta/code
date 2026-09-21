//! Profiles: a harness, the template it is set up with, the account it signs in with and the
//! permissions it runs under.
//!
//! A profile is a definition file in the store (`Profiles/<profile>.toml`), an image built
//! from that definition, and a login that lives in a volume of its own. The modules here hold
//! the definition and the facts every harness brings with it; building images and running
//! containers is the engine's work.

pub mod guidance;
pub mod history;
pub mod identity;

mod definition;
mod harness;
#[cfg(test)]
pub(crate) mod harness_live;
#[cfg(test)]
mod history_live;
#[cfg(all(test, unix))]
mod history_scripts;
#[cfg(test)]
mod live;
mod name;
mod template;

pub use definition::{ASSUMED_CONTEXT_TOKENS, Loaded, MountAccess, NetworkMode, Profile, ProviderChoice};
pub use harness::{AccountKind, ConfigFile, Desktop, Harness, HarnessKind, McpSettings, McpShape, Resume, Surface};
pub use name::SafeName;
pub use template::{
    Addition, CLAUDE_MARKETPLACES, CLAUDE_PLUGINS, Extra, GRAPHIFY_HOME, GRAPHIFY_PACKAGE, OH_MY_OPENAGENT, Template,
};
