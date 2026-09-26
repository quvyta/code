//! The extension a window's image carries, run by Node against a stand-in QCode socket and a
//! stand-in `vscode` module: what it asks QCode, and when it tells the agent panel. The
//! application itself cannot be driven here without a Google sign-in, so the panel is the one part
//! that is stood in for; everything the extension does on its own side is its real code.

use std::path::{Path, PathBuf};

use super::recipe::INBOX_EXTENSION;

/// Node, when this machine has one. The extension runs in a window's image, which always has it; a
/// machine building QCode need not.
fn node() -> Option<PathBuf> {
    let found = std::process::Command::new("sh").args(["-c", "command -v node"]).output().ok()?;
    found.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&found.stdout).trim()))
}

/// Runs the extension for about twenty seconds against a socket that answers every question with
/// what is waiting at that moment, changing what waits as a real workspace would, and prints what
/// it asked and every prompt it sent.
const DRIVE: &str = r#"
const Module = require('module'); const net = require('net');
const [sock, ext] = process.argv.slice(1);
const calls = [], seen = [];
const load = Module._load;
Module._load = function (request, ...rest) {
  if (request === 'vscode') return { commands: { executeCommand: async (id, text) => { calls.push([id, text]); } } };
  return load.call(this, request, ...rest);
};
let waiting = [];
const server = net.createServer((c) => { let d = ''; c.on('data', (chunk) => { d += chunk; if (!d.includes('\n')) return;
  seen.push(JSON.parse(d)); c.end(JSON.stringify({ ok: true, text: 'x', messages: waiting }) + '\n'); }); });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
server.listen(sock, async () => {
  const connect = net.createConnection; net.createConnection = () => connect(sock);
  const subscriptions = [];
  require(ext).activate({ subscriptions });
  await sleep(4000);
  waiting = [{ from: 'Claude Code · claude-sub', text: 'one' }];
  await sleep(7000);
  waiting = [...waiting, { from: 'Codex · cx-mimo', text: 'two' }];
  await sleep(4000);
  waiting = [];
  await sleep(4000);
  subscriptions.forEach((s) => s.dispose()); server.close();
  console.log(JSON.stringify({ calls, tokens: [...new Set(seen.map((q) => q.token))], ops: [...new Set(seen.map((q) => q.op))] }));
});
"#;

fn scratch(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let path = std::env::temp_dir().join(format!("qcode-inbox-{name}-{stamp}"));
    std::fs::create_dir_all(path.join("home/.gemini/config")).expect("a home");
    std::fs::create_dir_all(path.join("tmp")).expect("a temporary folder");
    path
}

fn run(node: &Path, folder: &Path, again_ms: u64) -> serde_json::Value {
    let extension =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/desktop").join(INBOX_EXTENSION).join("extension.js");
    let out = std::process::Command::new(node)
        .args(["-e", DRIVE, &folder.join("s.sock").display().to_string(), &extension.display().to_string()])
        .env("HOME", folder.join("home"))
        .env("QCODE_INBOX_AGAIN_MS", again_ms.to_string())
        .env("TMPDIR", folder.join("tmp"))
        .output()
        .expect("node runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("the drive prints JSON")
}

#[test]
fn a_window_is_told_once_for_each_new_message_and_nothing_is_taken_by_looking() {
    let Some(node) = node() else { return };
    let folder = scratch("told");
    let settings = r#"{"mcpServers":{"qcode":{"command":"node","args":["/run/qcode-mcp/qcode-bridge.mjs"],"env":{"QCODE_BRIDGE":"7a1c3e"}}}}"#;
    std::fs::write(folder.join("home/.gemini/config/mcp_config.json"), settings).expect("the window's settings");
    let seen = run(&node, &folder, 600_000);
    let _ = std::fs::remove_dir_all(&folder);

    let calls = seen["calls"].as_array().expect("the prompts");
    assert_eq!(
        calls.len(),
        2,
        "one prompt when a message came, one when a second did, none while nothing changed: {seen}"
    );
    assert_eq!(calls[0][0], "antigravity.sendPromptToAgentPanel");
    let first = calls[0][1].as_str().expect("a prompt");
    assert!(
        first.contains("A message") && first.contains("Claude Code · claude-sub") && first.contains("check_inbox"),
        "{first}"
    );
    let second = calls[1][1].as_str().expect("a prompt");
    assert!(second.contains("2 messages") && second.contains("Codex · cx-mimo"), "{second}");
    assert_eq!(seen["tokens"], serde_json::json!(["7a1c3e"]), "it speaks for its own tab");
    assert_eq!(seen["ops"], serde_json::json!(["peek"]), "it only looks; the agent takes them");
}

#[test]
fn a_window_qcode_wrote_no_token_for_asks_nothing() {
    let Some(node) = node() else { return };
    let folder = scratch("tokenless");
    let seen = run(&node, &folder, 600_000);
    let _ = std::fs::remove_dir_all(&folder);
    assert_eq!(seen["calls"], serde_json::json!([]), "{seen}");
    assert_eq!(seen["ops"], serde_json::json!([]), "{seen}");
}

#[test]
fn a_window_whose_messages_go_on_waiting_is_told_again() {
    // The prompt may reach a panel that was not listening yet, or an agent that finished its turn
    // without looking; messages still waiting a while later are told again.
    let Some(node) = node() else { return };
    let folder = scratch("again");
    let settings = r#"{"mcpServers":{"qcode":{"env":{"QCODE_BRIDGE":"7a1c3e"}}}}"#;
    std::fs::write(folder.join("home/.gemini/config/mcp_config.json"), settings).expect("the window's settings");
    let seen = run(&node, &folder, 2_500);
    let _ = std::fs::remove_dir_all(&folder);
    let calls = seen["calls"].as_array().expect("the prompts");
    // One at the first message, one at the second, and at least one more while both still waited;
    // none once nothing waits.
    assert!(calls.len() >= 3, "{seen}");
}
