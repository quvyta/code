//! The shared server's parts, and its script run by Node against a stand-in for opencode.
//!
//! The stand-in answers the few requests of opencode's server the script makes, keeps its
//! conversations in a file the test reads, and as `attach` notes every start in another file and
//! then waits, the way an interface waits for its person. The script is the one QCode writes into
//! a workspace, run unchanged; only its port, folders and log are its own, so that runs beside
//! each other never meet. A machine without Node fails these tests and says how to go on, for a
//! pass without the script would read as checked; only `QCODE_SKIP_NODE=1` skips them, and says
//! so in the log.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::*;
use crate::profile::{AccountKind, MountAccess, NetworkMode, SafeName};

fn profile(harness: HarnessKind, template: Template) -> Profile {
    Profile {
        name: SafeName::parse("p").expect("a name"),
        harness,
        template,
        account: AccountKind::Free,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

#[test]
fn only_opencode_under_a_qcode_template_shares_its_server() {
    for template in [Template::Recommended, Template::High, Template::QuvytaDev, Template::Slim] {
        assert!(shares(&profile(HarnessKind::OpenCode, template)), "{template:?}");
    }
    assert!(!shares(&profile(HarnessKind::OpenCode, Template::Base)), "base is opencode as it comes");
    for harness in HarnessKind::ALL.into_iter().filter(|harness| *harness != HarnessKind::OpenCode) {
        assert!(!shares(&profile(harness, Template::Recommended)), "{harness:?}");
    }
}

#[test]
fn the_script_falls_back_on_the_port_and_folder_qcode_hands_it() {
    assert!(SCRIPT.contains(&format!("process.env.{PORT_VARIABLE} || {PORT})")), "the port");
    assert!(SCRIPT.contains(&format!("process.env.{DIR_VARIABLE} || \"{CODE_DIR}\"")), "the folder");
    assert!(SCRIPT.contains(&format!("process.env.{SERVER_VARIABLE}")));
    assert!(SCRIPT.contains(&format!("process.env.{RELAY_VARIABLE}")));
    assert_ne!(PORT, crate::provider::relay::PORT);
}

#[test]
fn a_server_without_a_provider_is_told_of_the_plugin_and_not_of_the_relay() {
    let environment = server_environment("srv", &[]);
    let value = |name: &str| environment.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    assert_eq!(value(SERVER_VARIABLE).as_deref(), Some("srv"));
    assert_eq!(value(RELAY_VARIABLE), None);
    assert_eq!(value(crate::bridge::TOKEN_VARIABLE), None, "a tab's token is never the server's");
    let config: Value = serde_json::from_str(&value(CONFIG_VARIABLE).expect("the configuration")).expect("JSON");
    assert_eq!(config["plugin"], serde_json::json!(["file:///run/qcode-mcp/qcode-opencode-plugin.mjs"]));
    assert_eq!(
        config["mcp"]["qcode"],
        serde_json::json!({"type": "local", "command": ["node", "/run/qcode-mcp/qcode-bridge.mjs"], "enabled": false})
    );
}

#[test]
fn every_profile_has_a_token_of_its_own_that_stays_the_same() {
    let tokens = ServerTokens::default();
    assert_eq!(tokens.token("one"), tokens.token("one"));
    assert_ne!(tokens.token("one"), tokens.token("two"));
    assert_ne!(tokens.token("one"), ServerTokens::default().token("one"), "another QCode, another token");
    assert!(tokens.token("one").chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn ready_hands_the_servers_environment_over_through_env() {
    let command = ready_command(&[("QCODE_SERVER".to_owned(), "srv".to_owned())], Some("ses_a"));
    assert_eq!(command, ["env", "QCODE_SERVER=srv", "node", "/run/qcode-mcp/qcode-opencode.mjs", "ready", "ses_a"]);
    let program = tab_program(Some("ses_a"));
    assert_eq!(program[..2], ["sh", "-c"]);
    assert_eq!(program[3..], ["sh", "/run/qcode-mcp/qcode-opencode.mjs", "ses_a"]);
}

#[test]
fn the_scripts_are_written_beside_the_bridge_server_the_plugin_imports() {
    let folder = scratch("write");
    write_scripts(&folder).expect("written");
    assert_eq!(std::fs::read_to_string(folder.join(SCRIPT_NAME)).expect("the script"), SCRIPT);
    assert_eq!(std::fs::read_to_string(folder.join(PLUGIN_NAME)).expect("the plugin"), PLUGIN);
    assert_eq!(
        std::fs::read_to_string(folder.join(crate::bridge::SCRIPT_NAME)).expect("the bridge"),
        crate::bridge::SCRIPT
    );
    assert!(PLUGIN.contains(&format!("from \"./{}\"", crate::bridge::SCRIPT_NAME)));
    let _ = std::fs::remove_dir_all(&folder);
}

fn scratch(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-shared-{name}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("a scratch folder");
    folder
}

/// opencode as far as the script uses it: `serve` answers health, conversations, their status,
/// messages, stopping one and removing one, keeping the conversations in `state.json`; `attach`
/// notes its start in `attached.log` and waits.
const STAND_IN: &str = r#"#!/usr/bin/env node
const fs = require("fs");
const http = require("http");
const [what, ...rest] = process.argv.slice(2);
const folder = process.env.STAND_IN;
const state = `${folder}/state.json`;
const read = () => { try { return JSON.parse(fs.readFileSync(state, "utf8")); } catch { return { sessions: {}, next: 1 }; } };
const write = (value) => fs.writeFileSync(state, JSON.stringify(value));
if (what === "serve") {
  const port = Number(rest[rest.indexOf("--port") + 1]);
  fs.appendFileSync(`${folder}/served.log`, `${process.pid} ${process.env.QCODE_BRIDGE}\n`);
  http.createServer((req, res) => {
    const send = (code, body) => { res.writeHead(code, { "content-type": "application/json" }); res.end(JSON.stringify(body)); };
    const s = read();
    const parts = req.url.split("/").filter(Boolean);
    if (req.url === "/global/health") return send(200, { healthy: true });
    if (req.method === "POST" && req.url === "/session") {
      const id = `ses_${s.next}`; s.next += 1; s.sessions[id] = { messages: 0, busy: false }; write(s); return send(200, { id });
    }
    if (req.url === "/session/status") {
      const out = {}; for (const [id, v] of Object.entries(s.sessions)) if (v.busy) out[id] = { type: "busy" }; return send(200, out);
    }
    const id = parts[1];
    if (!s.sessions[id]) return send(404, {});
    if (parts.length === 2 && req.method === "GET") return send(200, { id });
    if (parts.length === 2 && req.method === "DELETE") { delete s.sessions[id]; write(s); return send(200, true); }
    if (parts[2] === "message") return send(200, Array.from({ length: s.sessions[id].messages }, () => ({})));
    if (parts[2] === "abort") { s.sessions[id].busy = false; s.sessions[id].aborted = true; write(s); return send(200, true); }
    send(404, {});
  }).listen(port, "127.0.0.1");
} else if (what === "attach") {
  fs.appendFileSync(`${folder}/attached.log`, `${process.pid} ${rest[rest.indexOf("--session") + 1]} ${process.env.QCODE_BRIDGE}\n`);
  setInterval(() => {}, 1000);
}
"#;

/// A run of the script against the stand-in: its own port, folders and log.
struct Rig {
    node: PathBuf,
    folder: PathBuf,
    port: u16,
}

impl Rig {
    /// A rig under this machine's own Node, or `None` where the machine is said to have none
    /// knowingly.
    fn new(name: &str) -> Option<Self> {
        let node = crate::testing::node("running the shared server's script")?;
        let folder = scratch(name);
        std::fs::create_dir_all(folder.join("bin")).expect("a folder for the stand-in");
        std::fs::create_dir_all(folder.join("work")).expect("a workspace folder");
        let program = folder.join("bin").join("opencode");
        std::fs::write(&program, STAND_IN).expect("the stand-in");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        }
        std::fs::write(folder.join("qcode-opencode.mjs"), SCRIPT).expect("the script");
        // A port of this run's own, away from the ones QCode uses.
        let port =
            std::net::TcpListener::bind("127.0.0.1:0").expect("a free port").local_addr().expect("its port").port();
        Some(Self { node, folder, port })
    }

    /// What QCode hands the script, pointed at this rig.
    fn environment(&self, token: &str) -> Vec<(String, String)> {
        let path = format!("{}:{}", self.folder.join("bin").display(), std::env::var("PATH").unwrap_or_default());
        let folder = |name: &str| self.folder.join(name).display().to_string();
        vec![
            ("PATH".to_owned(), path),
            ("STAND_IN".to_owned(), self.folder.display().to_string()),
            (SERVER_VARIABLE.to_owned(), token.to_owned()),
            (PORT_VARIABLE.to_owned(), self.port.to_string()),
            (DIR_VARIABLE.to_owned(), folder("work")),
            ("QCODE_OPENCODE_NOTES".to_owned(), folder("notes")),
            ("QCODE_OPENCODE_IDLE_MS".to_owned(), "1500".to_owned()),
        ]
    }

    fn command(&self, token: &str, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.node);
        command.arg(self.folder.join("qcode-opencode.mjs")).args(arguments);
        command.envs(self.environment(token)).env_remove(crate::bridge::TOKEN_VARIABLE);
        command
    }

    fn ready(&self, token: &str, conversation: Option<&str>) -> String {
        let mut arguments = vec!["ready"];
        arguments.extend(conversation);
        let output = self.command(token, &arguments).output().expect("the script runs");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A tab as QCode starts it: the loop of [`tab_program`], carrying the tab's token.
    fn attach(&self, token: &str, tab: &str, conversation: &str) -> Child {
        let program = tab_program(Some(conversation));
        let mut tab_command = Command::new(&program[0]);
        tab_command.args(&program[1..4]).arg(self.folder.join("qcode-opencode.mjs")).arg(&program[5]);
        tab_command.envs(self.environment(token));
        tab_command
            .env(crate::bridge::TOKEN_VARIABLE, tab)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the tab starts")
    }

    /// Ends the tab the way QCode does: every process carrying its token, the loop and the
    /// interface it started.
    fn close(&self, tab: &mut Child, token: &str) {
        let interfaces: Vec<u32> = self
            .lines("attached.log")
            .iter()
            .filter(|line| line.ends_with(&format!(" {token}")))
            .filter_map(|line| line.split_whitespace().next()?.parse().ok())
            .collect();
        signal(tab.id(), rustix::process::Signal::TERM);
        for pid in interfaces {
            signal(pid, rustix::process::Signal::TERM);
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while tab.try_wait().ok().flatten().is_none() {
            assert!(Instant::now() < deadline, "the tab never ended");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn lines(&self, file: &str) -> Vec<String> {
        std::fs::read_to_string(self.folder.join(file)).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// The servers of this rig still running, by process id.
    fn servers(&self) -> Vec<u32> {
        self.lines("served.log")
            .iter()
            .filter_map(|line| line.split_whitespace().next()?.parse().ok())
            .filter(|pid| Path::new(&format!("/proc/{pid}")).exists() && !zombie(*pid))
            .collect()
    }

    fn state(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.folder.join("state.json")).unwrap_or_default())
            .unwrap_or(Value::Null)
    }

    fn set(&self, conversation: &str, field: &str, value: Value) {
        let mut state = self.state();
        state["sessions"][conversation][field] = value;
        std::fs::write(self.folder.join("state.json"), state.to_string()).expect("the stand-in's state");
    }
}

impl Rig {
    /// Every process of this rig still running: the script, the keeper it leaves behind, the
    /// stand-in's server and interfaces, and the tabs' loops. The keeper is started detached, in a
    /// session of its own, and outlives the `ready` that started it, so it is no child of the
    /// test's; each of them carries the rig's folder in its environment, and is found by that.
    fn processes(&self) -> Vec<u32> {
        let mark = format!("STAND_IN={}", self.folder.display());
        let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| *pid != std::process::id() && !zombie(*pid))
            .filter(|pid| {
                std::fs::read(format!("/proc/{pid}/environ"))
                    .is_ok_and(|environment| environment.split(|byte| *byte == 0).any(|each| each == mark.as_bytes()))
            })
            .collect()
    }

    /// The keepers of this rig.
    fn keepers(&self) -> Vec<u32> {
        self.processes()
            .into_iter()
            .filter(|pid| {
                std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|words| words.split(|byte| *byte == 0).any(|word| word == b"keep"))
            })
            .collect()
    }
}

/// Whether the test passes, fails or panics, nothing of the rig outlives it: a keeper left behind
/// starts its server again for ever once the folder is gone, and asks ports other tests take
/// later whether they are its server.
impl Drop for Rig {
    fn drop(&mut self) {
        // A tab's loop may start a process between one look and the next, so it is looked for
        // until none is left, for a while that is generous but not endless.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = self.processes();
            if left.is_empty() || Instant::now() > deadline {
                break;
            }
            for pid in left {
                signal(pid, rustix::process::Signal::KILL);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// Sends `signal` to the process `pid` directly: the tests lean on no `kill` program.
fn signal(pid: u32, signal: rustix::process::Signal) {
    if let Some(pid) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) {
        let _ = rustix::process::kill_process(pid, signal);
    }
}

fn zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| stat.rsplit_once(')').is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')))
        .unwrap_or(true)
}

/// Waits, generously but not forever, for `done`.
fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn ready_starts_one_server_and_keeps_a_conversation_the_server_still_has() {
    let Some(rig) = Rig::new("ready") else { return };
    let first = rig.ready("srv", None);
    assert!(first.starts_with("ses_"), "{first}");
    assert_eq!(rig.ready("srv", Some(&first)), first, "the tab opens its conversation again");
    let fresh = rig.ready("srv", Some("ses_gone"));
    assert!(fresh.starts_with("ses_") && fresh != first, "a conversation the server lost: {fresh}");
    assert_eq!(rig.servers().len(), 1, "one server however many tabs start");
    assert_eq!(rig.lines("served.log")[0].split_whitespace().nth(1), Some("srv"), "the server's token");
}

#[test]
fn a_server_an_earlier_qcode_left_is_replaced_by_one_of_this_qcodes_token() {
    let Some(rig) = Rig::new("stale") else { return };
    rig.ready("old", None);
    let old = rig.servers();
    rig.ready("new", None);
    until("the old server ends", || rig.servers().iter().all(|pid| !old.contains(pid)));
    let tokens: Vec<String> =
        rig.lines("served.log").iter().filter_map(|line| line.split_whitespace().nth(1).map(str::to_owned)).collect();
    assert_eq!(tokens, ["old", "new"]);
    assert_eq!(rig.servers().len(), 1);
}

#[test]
fn a_keeper_whose_server_will_not_come_back_ends_once_no_tab_is_left() {
    let Some(rig) = Rig::new("lonely") else { return };
    rig.ready("srv", None);
    until("the keeper runs", || !rig.keepers().is_empty());
    // opencode now ends as soon as it starts, and the server running is ended under a keeper no
    // tab uses.
    std::fs::write(rig.folder.join("bin").join("opencode"), "#!/bin/sh\nexit 1\n").expect("the stand-in");
    for pid in rig.servers() {
        signal(pid, rustix::process::Signal::KILL);
    }
    until("the keeper ends", || rig.keepers().is_empty());
}

#[test]
fn a_tab_outlives_its_servers_end_and_the_last_tab_takes_the_server_with_it() {
    let Some(rig) = Rig::new("attach") else { return };
    let first = rig.ready("srv", None);
    let second = rig.ready("srv", None);
    let mut one = rig.attach("srv", "tab-one", &first);
    let mut two = rig.attach("srv", "tab-two", &second);
    until("both interfaces attach", || rig.lines("attached.log").len() == 2);
    // The interfaces carry their tab's token, so QCode ends a closed tab's; the server never does.
    let attached = rig.lines("attached.log");
    assert!(attached.iter().any(|line| line.ends_with(&format!("{first} tab-one"))), "{attached:?}");
    assert!(attached.iter().any(|line| line.ends_with(&format!("{second} tab-two"))), "{attached:?}");

    // The server goes away under both tabs: it comes back, and both interfaces attach again to
    // the conversation each showed.
    let server = rig.servers();
    assert_eq!(server.len(), 1);
    signal(server[0], rustix::process::Signal::KILL);
    until("the interfaces attach again", || rig.lines("attached.log").len() == 4);
    assert_eq!(rig.servers().len(), 1, "one server again");
    let again = &rig.lines("attached.log")[2..];
    assert!(again.iter().any(|line| line.contains(&format!(" {first} "))), "{again:?}");
    assert!(again.iter().any(|line| line.contains(&format!(" {second} "))), "{again:?}");

    // The first tab closes while its agent works: its work is stopped, its conversation, in which
    // something was said, stays, and the server stays for the other tab.
    rig.set(&first, "busy", Value::Bool(true));
    rig.set(&first, "messages", Value::from(2));
    rig.close(&mut one, "tab-one");
    until("the first tab's work is stopped", || rig.state()["sessions"][&first]["aborted"] == Value::Bool(true));
    assert_eq!(rig.state()["sessions"][&first]["messages"], Value::from(2), "{}", rig.state());
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(rig.servers().len(), 1, "the other tab still uses the server");

    // The last tab closes without a word said: its conversation goes, and so does the server.
    rig.close(&mut two, "tab-two");
    until("the empty conversation goes", || rig.state()["sessions"].get(&second).is_none());
    until("the server ends with the last tab", || rig.servers().is_empty());
}
