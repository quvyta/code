//! opencode's shared server: one per profile container, and every opencode tab of the profile an
//! interface attached to it.
//!
//! opencode's own program is a server and an interface in one process. Started once per tab, the
//! server part, which carries the plugins, the MCP servers and the model's work, is started once
//! per tab too. Seven opencode tabs of one profile under QCode recommended were measured on a
//! Raspberry Pi 5 at 5.0 GB and 241 threads that way, and at 2.4 GB and 112 threads with one
//! server and seven interfaces attached to it (the sum of each process's share of memory; one
//! interface is about 250 MB, the server about 500 MB). So the container runs the server once and
//! each tab runs only `opencode attach`.
//!
//! Inside the container one keeper per container ([`SCRIPT`], run as `keep`) starts the server and
//! watches it and the tabs: a server that stops is started again and every interface sent back to
//! its conversation; a tab that ended has its agent's work stopped and its conversation removed
//! when nothing was said in it; the server stops a while after the last tab. Before a tab starts,
//! the same script, run as `ready`, brings the keeper and the server up and prints the conversation
//! the tab is to show. The tab itself is a shell loop around the interface ([`tab_program`]), so it
//! costs nothing beside it. QCode's part is the environment it hands over: the server's own token,
//! never a tab's (a closed tab's processes are ended by the tab's token, and the first tab closing
//! must not take the server with it), opencode's configuration, and the relay when the profile runs
//! on a provider.
//!
//! The server starts the MCP servers once for every tab, so the bridge's server could not tell the
//! tabs apart by the token in its environment. Instead the configuration switches it off and
//! loads the bridge as a plugin ([`PLUGIN`]), which is told the conversation each call comes
//! from; a question carries the server's token and that conversation, and
//! [`super::bridge`] finds the tab showing it among the tabs of the server's profile.
//!
//! Only a profile under a QCode template shares its server: the template writes opencode's
//! permission to do everything, which an attached interface needs because `opencode attach` takes
//! no `--auto`. A profile under base is opencode as it comes, one process per tab.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::Path;

use serde_json::{Value, json};

use super::plan::LaunchFailure;
use crate::base::paths::{CODE_DIR, MCP_DIR};
use crate::engine::run::capture;
use crate::engine::{Engine, Exec};
use crate::profile::{HarnessKind, Profile, Template};

/// The script a shared opencode tab runs, carried inside the binary and written into each open
/// workspace's `Containers/MCP/` folder beside the bridge's server.
pub const SCRIPT: &str = include_str!("../../../assets/opencode/qcode-opencode.mjs");

/// The name of [`SCRIPT`] in `Containers/MCP/`.
pub const SCRIPT_NAME: &str = "qcode-opencode.mjs";

/// The bridge as an opencode plugin, written beside the bridge's server, whose tools it imports.
pub const PLUGIN: &str = include_str!("../../../assets/bridge/qcode-opencode-plugin.mjs");

/// The name of [`PLUGIN`] in `Containers/MCP/`.
pub const PLUGIN_NAME: &str = "qcode-opencode-plugin.mjs";

/// The variable a shared tab carries the server's token in, beside its own in
/// [`crate::bridge::TOKEN_VARIABLE`].
pub const SERVER_VARIABLE: &str = "QCODE_SERVER";

/// The variable that names the relay's script to the server's start, for a profile that runs on
/// a provider: the relay then starts the server, the way it starts a harness of its own.
pub const RELAY_VARIABLE: &str = "QCODE_RELAY";

/// The port the server listens on inside the container, on its loopback interface alone, next to
/// the relay's. Handed to the script in [`PORT_VARIABLE`], with the workspace's folder in
/// [`DIR_VARIABLE`]; the script falls back on the same two, which a test holds together.
pub const PORT: u16 = 41418;

/// The variable the script reads the server's port from.
pub const PORT_VARIABLE: &str = "QCODE_OPENCODE_PORT";

/// The variable the script reads the folder the server works in from: the workspace, where every
/// tab of the profile works.
pub const DIR_VARIABLE: &str = "QCODE_OPENCODE_DIR";

/// The configuration variable opencode reads beside its settings file.
const CONFIG_VARIABLE: &str = crate::profile::OPENCODE_CONFIG_CONTENT;

/// Whether the opencode tabs of `profile` share one server.
#[must_use]
pub fn shares(profile: &Profile) -> bool {
    profile.harness == HarnessKind::OpenCode && profile.template != Template::Base
}

/// Where [`SCRIPT`] is inside a profile container.
#[must_use]
pub fn script_in_container() -> String {
    format!("{MCP_DIR}/{SCRIPT_NAME}")
}

/// Where [`PLUGIN`] is inside a profile container.
#[must_use]
pub fn plugin_in_container() -> String {
    format!("{MCP_DIR}/{PLUGIN_NAME}")
}

/// What a shared tab runs: opencode's interface attached to the profile's server, showing
/// `conversation`, in a loop that attaches it again when the keeper ([`SCRIPT`]) ended it after
/// starting a server that went away, and ends with it otherwise. The server is made sure of
/// first, since it stops a while after the last tab of the profile ended.
///
/// A shell and not the script, so a tab costs one interface and nothing beside it. The
/// conversation is always known by the time a tab starts, since [`ready_command`] made or found it
/// first; a tab without one refuses and says so rather than guess.
#[must_use]
pub fn tab_program(conversation: Option<&str>) -> Vec<String> {
    ["sh", "-c", TAB_LOOP, "sh", &script_in_container(), conversation.unwrap_or_default()].map(str::to_owned).to_vec()
}

/// The loop of [`tab_program`]: `$1` the script, `$2` the conversation. The note the keeper leaves
/// is read by the same rule the script writes it by.
pub const TAB_LOOP: &str = "[ -n \"$2\" ] || { echo 'qcode-opencode: no conversation to show' >&2; exit 1; }; \
node \"$1\" ensure || exit 1; \
notes=\"${QCODE_OPENCODE_NOTES:-/tmp/qcode-opencode-$QCODE_OPENCODE_PORT}\"; \
while :; do \
opencode attach \"http://127.0.0.1:$QCODE_OPENCODE_PORT\" --dir \"$QCODE_OPENCODE_DIR\" --session \"$2\"; \
code=$?; [ -e \"$notes/again-$2\" ] || exit $code; rm -f \"$notes/again-$2\"; \
done";

/// What QCode runs in the container before a shared tab starts, without a terminal: the server
/// comes up unless it is up already, and the one line printed is the conversation the tab is to
/// show — `conversation` while the server still has it, a new one otherwise. `environment` is the
/// server's ([`server_environment`]), handed over through `env` because running a program without
/// a terminal takes no environment of its own.
#[must_use]
pub fn ready_command(environment: &[(String, String)], conversation: Option<&str>) -> Vec<String> {
    let mut command = vec!["env".to_owned()];
    command.extend(environment.iter().map(|(name, value)| format!("{name}={value}")));
    command.extend(["node".to_owned(), script_in_container(), "ready".to_owned()]);
    command.extend(conversation.map(str::to_owned));
    command
}

/// The environment the server starts with, less the tab's own token: the server's token, opencode's
/// configuration, and the relay's script when `provider` holds the provider's own configuration
/// (made for the server's token, see [`crate::profile::ProviderChoice::environment`]).
#[must_use]
pub fn server_environment(token: &str, provider: &[(String, String)]) -> Vec<(String, String)> {
    let provided = provider
        .iter()
        .find(|(name, _)| name == CONFIG_VARIABLE)
        .and_then(|(_, value)| serde_json::from_str(value).ok());
    let mut environment = vec![
        (SERVER_VARIABLE.to_owned(), token.to_owned()),
        (PORT_VARIABLE.to_owned(), PORT.to_string()),
        (DIR_VARIABLE.to_owned(), CODE_DIR.to_owned()),
        (CONFIG_VARIABLE.to_owned(), configuration(provided)),
    ];
    environment.extend(provider.iter().filter(|(name, _)| name != CONFIG_VARIABLE).cloned());
    if !provider.is_empty() {
        environment.push((RELAY_VARIABLE.to_owned(), crate::provider::relay::script_in_container()));
    }
    environment
}

/// opencode's configuration for the server, on top of its settings file: the provider's, when the
/// profile runs on one, with the bridge's plugin added and the bridge's MCP server switched off.
///
/// opencode joins the plugins of this configuration to those of the settings file (1.18.32), so
/// oh-my-openagent stays; the server entry is written whole, because a settings file that never
/// had it would otherwise be given an entry with nothing but `enabled` in it.
fn configuration(provided: Option<Value>) -> String {
    let mut config = match provided {
        Some(Value::Object(object)) => Value::Object(object),
        _ => json!({}),
    };
    config["plugin"] = json!([format!("file://{}", plugin_in_container())]);
    config["mcp"] = json!({
        crate::bridge::SERVER_NAME: {
            "type": "local",
            "command": ["node", crate::bridge::script_in_container()],
            "enabled": false,
        }
    });
    config.to_string()
}

/// The server tokens of one open workspace, one per profile, the same every time a profile is
/// asked for and unknown outside this process.
///
/// Kept as the standard library's hasher, keyed from the operating system's randomness when the
/// workspace was opened: a profile's token is that key applied to the profile's name, so it needs
/// no map to be written into when a profile is first used, and nothing in a container can work it
/// out.
#[derive(Debug, Clone, Default)]
pub struct ServerTokens(RandomState);

impl ServerTokens {
    /// The token of the server of `profile`.
    #[must_use]
    pub fn token(&self, profile: &str) -> String {
        let mut words = [0u64; 2];
        for (index, word) in words.iter_mut().enumerate() {
            let mut hasher = self.0.build_hasher();
            hasher.write(b"qcode-opencode-server");
            hasher.write(profile.as_bytes());
            hasher.write_usize(index);
            *word = hasher.finish();
        }
        format!("{:016x}{:016x}", words[0], words[1])
    }
}

/// Writes the script, the plugin and the bridge's server the plugin imports into `folder`,
/// unless each is there already as this QCode carries it. The folder is the one the profile
/// containers see at [`MCP_DIR`], kept to the person's own processes the way the bridge keeps it.
///
/// # Errors
///
/// The error of creating the folder or writing a file.
pub fn write_scripts(folder: &Path) -> io::Result<()> {
    std::fs::create_dir_all(folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
    }
    for (name, text) in
        [(SCRIPT_NAME, SCRIPT), (PLUGIN_NAME, PLUGIN), (crate::bridge::SCRIPT_NAME, crate::bridge::SCRIPT)]
    {
        let path = folder.join(name);
        if std::fs::read(&path).is_ok_and(|written| written == text.as_bytes()) {
            continue;
        }
        qframe::storage::atomic_write(&path, text.as_bytes())?;
    }
    Ok(())
}

/// Brings the server of the running container `container` up and answers the conversation a tab
/// is to show ([`ready_command`]), after writing the scripts into `folder`. Runs engine commands
/// and waits for them, so it belongs on a background thread; the script bounds its own wait.
///
/// # Errors
///
/// A [`LaunchFailure`] with the script's own words when the scripts cannot be written, the server
/// does not come up, or what the script printed is not a conversation's id.
pub(super) fn ready(
    engine: &Engine,
    container: &str,
    folder: &Path,
    environment: &[(String, String)],
    conversation: Option<&str>,
) -> Result<String, LaunchFailure> {
    write_scripts(folder).map_err(|error| LaunchFailure::from_host(&error))?;
    let command = ready_command(environment, conversation);
    let parts: Vec<&str> = command.iter().map(String::as_str).collect();
    let exec = engine.exec_without_terminal(&Exec { container, command: &parts }).unbounded();
    let said = capture(&exec).map_err(|error| LaunchFailure::from(&error))?;
    let chosen = said.lines().rev().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    if crate::profile::history::is_safe_id(chosen) {
        Ok(chosen.to_owned())
    } else {
        Err(LaunchFailure { command: parts.join(" "), output: said, image_missing: false })
    }
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "shared_live.rs"]
mod live;
