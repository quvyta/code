//! Providers: the model services a person adds themselves, and what QCode asks each of them.
//!
//! A provider is the person's own tag, a kind, an address, the shape it speaks, the models it
//! was last seen to offer and the key it is reached with. Every kind QCode knows speaks the
//! Anthropic message shape and the OpenAI one directly, so a harness is pointed at the address in
//! the shape it speaks itself — Claude Code the one, opencode the other — and nothing translates
//! in between.
//!
//! Three rules run through everything here.
//!
//! **The key lives apart.** It is written to `providers.toml` in QCode's data folder, with mode
//! 600 in a folder of mode 700, and never into the settings file, which people copy between
//! machines and paste into reports. It is a [`Key`], which has no `Display` and whose `Debug` is
//! the last four characters, so nothing can print it by accident.
//!
//! **Nothing goes out unasked.** Every request passes through [`Web`], the seam a test stands in
//! for; no test in this crate can reach the network, and the page says where a request will go
//! before the person presses the thing that sends it.
//!
//! **What a server claims is not what it gives.** A model's record can say 262 144 tokens while
//! the endpoint a harness really uses pins the window to 4 096 and throws the front of every
//! larger prompt away without a word. [`ask::measure`] finds that edge by watching where the
//! reported input token count stops growing, and the page shows both numbers.

pub mod ask;
pub mod relay;

mod file;
mod key;
#[cfg(test)]
mod live;
mod record;
#[cfg(test)]
mod relay_live;
mod tag;
#[cfg(test)]
mod tests;

pub use ask::{Answer, Ask, AskError, Method, Reached, Secret, Web};
pub use file::{AddError, PermissionProblem, Providers, permission_problems};
pub use key::Key;
pub use record::{CRAMPED, Measured, Model, ProviderEntry, ProviderKind, Published, Region, Wire, trim_base};
pub use relay::{Event as RelayEvent, Listener as RelayListener, Upstream};
pub use tag::{Tag, TagError};
