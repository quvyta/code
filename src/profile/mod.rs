//! Profiles: a harness, the template it is set up with, the account it signs in with and the
//! permissions it runs under.
//!
//! A profile is a definition file in the store (`Profiles/<profile>.toml`), an image built
//! from that definition, and a login that lives in a volume of its own. The modules here hold
//! the definition and the facts every harness brings with it; building images and running
//! containers is the engine's work.

pub mod account;
pub mod changes;
pub mod guidance;
pub mod history;
pub mod identity;

#[cfg(test)]
mod carry_live;
mod definition;
mod harness;
#[cfg(test)]
pub(crate) mod harness_live;
#[cfg(test)]
mod history_live;
#[cfg(all(test, unix))]
mod history_scripts;
#[cfg(test)]
pub(crate) mod keep_live;
#[cfg(test)]
mod live;
#[cfg(test)]
mod memory_live;
mod name;
pub mod opencode_packages;
#[cfg(test)]
mod opencode_packages_live;
pub mod own;
#[cfg(test)]
mod permission_live;
#[cfg(test)]
mod plugins_live;
mod template;
pub mod unattended;

pub use definition::{
    ASSUMED_CONTEXT_TOKENS, Loaded, MountAccess, NetworkMode, OPENCODE_CONFIG_CONTENT, Pick, Profile, ProviderChoice,
};
pub use harness::{
    AccountKind, Archive, ConfigFile, Desktop, Harness, HarnessKind, McpSettings, McpShape, Resume, Surface, SystemFile,
};
pub use name::SafeName;
pub use template::{
    ACCESSLINT_PLUGIN, ACCESSLINT_PROGRAM, ACCESSLINT_SERVER, AGENT_SDK, AGENT_SDK_VENV, Addition, CARGO_HOME,
    CLAUDE_EXTRA_PLUGINS, CLAUDE_MARKETPLACES, CLAUDE_PLUGINS, CLAUDE_STARTER_PLUGINS, Extra, GRAPHIFY_HOME,
    GRAPHIFY_PACKAGE, OH_MY_OPENAGENT, OH_MY_OPENCODE_SLIM, OPENCODE_PLUGIN_SETTINGS, OPENCODE_SLIM_SETTINGS,
    RUST_ANALYZER_PLUGIN, RUSTUP_HOME, SECURITY_GUIDANCE_PLUGIN, SLIM_OWN_SETTINGS, Template,
};
