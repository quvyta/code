//! The bridge between tabs: the agent of one tab lists the other agent tabs of its workspace
//! and sends one of them a message.
//!
//! Three parts, each in its own place:
//!
//! - a small MCP server ([`SCRIPT`]) a harness starts inside its container, with two tools,
//!   `list_tabs` and `send_message`. It decides nothing; it asks QCode through a socket;
//! - the socket, one per open workspace in the workspace's `Containers/MCP/` folder, which profile
//!   containers see read-only at [`MCP_DIR`] ([`socket`]), and the
//!   line of JSON each question and answer is ([`protocol`]);
//! - the rules QCode answers by ([`rules`]): the person approves the first message between two
//!   tabs, a tab without the network never sends to one with it, and a chain of messages and a
//!   busy sender are both cut off.
//!
//! The server is registered in each harness's own user settings in the workspace's home volume
//! ([`config`]), so the harness starts it like any server the person added.
//!
//! Which tab asks is known from a token: every harness tab is started with one in
//! [`TOKEN_VARIABLE`], the server hands it back with every question, and QCode looks the tab up
//! by it. A token is random rather than the tab's number, so an agent cannot speak for a tab it
//! is not in by guessing.

pub mod config;
pub mod protocol;
pub mod rules;
pub mod socket;

#[cfg(test)]
mod live;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use crate::base::paths::MCP_DIR;

/// The MCP server a harness starts, carried inside the binary and written into each open
/// workspace's `Containers/MCP/` folder, so the server a container starts is always the one this
/// QCode speaks with.
pub const SCRIPT: &str = include_str!("../../assets/bridge/qcode-bridge.mjs");

/// The name of the server's file in `Containers/MCP/`.
pub const SCRIPT_NAME: &str = "qcode-bridge.mjs";

/// The name of the socket in `Containers/MCP/`. The server finds it beside itself.
pub const SOCKET_NAME: &str = "bridge.sock";

/// The environment variable a harness tab carries its token in.
pub const TOKEN_VARIABLE: &str = "QCODE_BRIDGE";

/// The name the server is registered under in every harness's settings.
pub const SERVER_NAME: &str = "qcode";

/// Where the server is inside a profile container.
#[must_use]
pub fn script_in_container() -> String {
    format!("{MCP_DIR}/{SCRIPT_NAME}")
}

/// A new token for a tab: 128 bits nobody outside this process can predict.
///
/// The standard library's hasher is keyed from the operating system's randomness once per
/// process and never shows its keys, so what it makes of a counter cannot be told in advance by
/// anything in a container. That is all a token has to be, and it needs no dependency.
#[must_use]
pub fn token() -> String {
    let state = RandomState::new();
    let mut words = [0u64; 2];
    for (index, word) in words.iter_mut().enumerate() {
        let mut hasher = state.build_hasher();
        hasher.write_usize(index);
        hasher.write_u128(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
        );
        *word = hasher.finish();
    }
    format!("{:016x}{:016x}", words[0], words[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_never_the_same_twice() {
        let many: std::collections::HashSet<String> = (0..1000).map(|_| token()).collect();
        assert_eq!(many.len(), 1000);
        assert!(many.iter().all(|token| token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit())));
    }

    #[test]
    fn the_server_sits_in_the_mounted_folder_beside_the_socket_it_looks_for() {
        assert_eq!(script_in_container(), "/run/qcode-mcp/qcode-bridge.mjs");
        assert!(SCRIPT.contains(&format!("\"{SOCKET_NAME}\"")), "the server looks for another socket");
        assert!(SCRIPT.contains(TOKEN_VARIABLE), "the server reads another variable");
    }

    #[test]
    fn the_server_names_both_tools_and_speaks_both_eras_of_the_protocol() {
        for word in ["\"list_tabs\"", "\"send_message\"", "\"initialize\"", "\"server/discover\"", "\"tools/call\""] {
            assert!(SCRIPT.contains(word), "{word}");
        }
        assert!(SCRIPT.contains("2026-07-28") && SCRIPT.contains("2025-11-25"));
    }
}
