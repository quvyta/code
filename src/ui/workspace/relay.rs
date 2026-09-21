//! The provider relay of each open workspace: opened beside the bridge's own socket, so a tab of
//! a provider profile can be pointed at it, and told which provider entry answers for which tab.
//!
//! This mirrors [`super::bridge::follow`] exactly, for the socket a harness's own request travels
//! over rather than the one its agent's calls travel over: opened once per open workspace, left
//! alone once it answers, and failing once and quietly rather than on every frame.
//!
//! Resolving a token is synchronous, unlike the bridge's calls: a harness waiting on an answer to
//! forward to the provider cannot wait on this screen's own message loop, so [`Listener::open`]
//! takes a `resolve` closure that answers for itself. It closes over [`Tokens`], the map from a
//! tab's token to its profile's provider tag that this module keeps current as tabs open and
//! close, and reads `providers.toml` fresh on every call: a key rotated on the Providers page
//! must reach the next request, not the request after QCode is restarted.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::provider::{ProviderEntry, RelayListener, Upstream};

use super::{OpenWorkspace, WorkspaceScreen};

/// The token of a harness tab, mapped to the tag of the provider its profile names.
pub(super) type Tokens = Arc<Mutex<HashMap<String, String>>>;

/// Whether a workspace's provider relay is listening.
#[derive(Debug, Default)]
pub(super) enum Link {
    /// Not yet: the screen has not come round to this workspace, or it carries no profile that
    /// could ever need the relay.
    #[default]
    Off,
    /// The socket could not be opened.
    Failed,
    /// Listening, with the map a tab of a provider profile is entered into.
    On {
        /// The socket. Never read again once opened: unlike the bridge, a relay answers a
        /// harness's own request on a thread of its own rather than through this screen's
        /// message loop, so this is held for nothing but its `Drop`, which closes the socket
        /// when the workspace does.
        _listener: RelayListener,
        /// The tokens the relay's `resolve` closure was given at open, and this screen still
        /// writes into as tabs open and close.
        tokens: Tokens,
    },
}

/// Opens the relay of every open workspace that carries a provider profile and has none yet.
///
/// Unlike the bridge, this is not gated on the person asking for it: a provider profile is
/// useless without it, so it is opened the moment such a profile is in a workspace, the way the
/// container it points at is made without being asked for by name either.
pub(super) fn follow(screen: &mut WorkspaceScreen) {
    for workspace in &mut screen.workspaces {
        if !matches!(workspace.relay, Link::Off) {
            continue;
        }
        if !workspace.profiles.iter().any(|profile| profile.provider.is_some()) {
            continue;
        }
        let providers_path = screen.providers_path.clone();
        let tokens: Tokens = Arc::new(Mutex::new(HashMap::new()));
        let resolve_tokens = Arc::clone(&tokens);
        let resolve_provider = move |token: &str| resolve(&resolve_tokens, providers_path.as_deref(), token);
        match RelayListener::open(&workspace.paths.mcp(), resolve_provider, Upstream::network(), |_| {}) {
            Ok(listener) => workspace.relay = Link::On { _listener: listener, tokens },
            Err(_) => workspace.relay = Link::Failed,
        }
    }
}

/// The provider entry `token` names, read fresh from `providers_path` every time: a key rotated
/// on the Providers page must reach the very next request the relay carries.
fn resolve(tokens: &Tokens, providers_path: Option<&Path>, token: &str) -> Option<ProviderEntry> {
    let tag = tokens.lock().ok()?.get(token).cloned()?;
    let providers = match providers_path {
        Some(path) => crate::provider::Providers::open(path).value,
        None => crate::provider::Providers::in_memory(),
    };
    providers.get(&tag).cloned()
}

/// Enters `token` into `workspace`'s relay map under the provider tag `tag`, when the relay is
/// open: from here on, a request carrying this token is carried to that provider.
pub(super) fn entered(workspace: &OpenWorkspace, token: &str, tag: &str) {
    if let Link::On { tokens, .. } = &workspace.relay
        && let Ok(mut tokens) = tokens.lock()
    {
        tokens.insert(token.to_owned(), tag.to_owned());
    }
}

/// Forgets the closed tabs' tokens `closed`, so a token nobody carries any more resolves to
/// nothing rather than lingering in the map.
pub(super) fn closed(workspace: &OpenWorkspace, closed: &[String]) {
    let Link::On { tokens, .. } = &workspace.relay else { return };
    let Ok(mut tokens) = tokens.lock() else { return };
    for token in closed {
        tokens.remove(token);
    }
}
