//! That each harness's history script still finds a conversation recorded under the directory an
//! older QCode mounted the workspace's files on.
//!
//! The scripts run inside a container in the application, but they are plain Node and nothing
//! about reading two directories needs one, so these run them with the Node of this machine over
//! a home folder written for the test. That keeps the check in the ordinary gate, where removing
//! a script's second directory turns it red at once.
//!
//! The directories are the test's own folders rather than [`CODE_DIR`] and [`LEGACY_CODE_DIR`]:
//! a script is told where to look and nothing in it is written down, and opencode is asked from
//! inside the workspace, which has to be a folder that is really there. That the application
//! hands the two constants over is [`super::history`]'s own test.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::HarnessKind;
use super::history::{self, Conversation};

/// A home folder for one harness's script, with the three workspace folders the tests use.
struct Home {
    root: PathBuf,
    /// Where the workspace's files are mounted today.
    today: PathBuf,
    /// Where a QCode written before mounted them.
    before: PathBuf,
    /// A third workspace, whose conversations must never be listed with the other two.
    elsewhere: PathBuf,
}

impl Home {
    /// An empty home folder, and the three workspace folders made.
    fn new(what: &str) -> Self {
        let root = crate::testing::scratch(&format!("history-script-{what}"));
        let _ = fs::remove_dir_all(&root);
        let home = Self {
            today: root.join("work"),
            before: root.join("work/Project"),
            elsewhere: root.join("elsewhere"),
            root,
        };
        for dir in [&home.root.join("bin"), &home.today, &home.before, &home.elsewhere] {
            fs::create_dir_all(dir).expect("the test's folders");
        }
        home
    }

    /// Writes `lines` as the file `path` under the home folder.
    fn write(&self, path: &str, lines: &[&str]) {
        let file = self.root.join(path);
        fs::create_dir_all(file.parent().expect("a file is in a folder")).expect("the folder");
        fs::write(&file, format!("{}\n", lines.join("\n"))).expect("the file");
    }

    /// Puts a program named `name` on the script's path, running `script` with a shell.
    fn program(&self, name: &str, script: &str) {
        let file = self.root.join("bin").join(name);
        fs::write(&file, format!("#!/bin/sh\n{script}\n")).expect("the program");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("it can be run");
    }

    /// The name of a path as a script's argument.
    fn word(path: &Path) -> String {
        path.to_str().expect("the test's folders are plain text").to_owned()
    }

    /// Runs `harness`'s script over this home folder, told about [`Self::today`] and
    /// [`Self::before`], and reads what it printed the way QCode does.
    fn read(&self, harness: HarnessKind) -> Vec<Conversation> {
        let script = harness.history_script().expect("a command-line harness has a script");
        let path = format!("{}:{}", Self::word(&self.root.join("bin")), std::env::var("PATH").unwrap_or_default());
        let output = Command::new("node")
            .args(["-e", script, &Self::word(&self.today), &Self::word(&self.before)])
            .env("HOME", &self.root)
            .env("PATH", path)
            .env_remove("CODEX_HOME")
            .current_dir(&self.root)
            .output()
            .expect("the Node of this machine runs the script");
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{harness:?}: {printed}{}", String::from_utf8_lossy(&output.stderr));
        history::parse(&printed)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The ids of `found`, in its order.
fn ids(found: &[Conversation]) -> Vec<&str> {
    found.iter().map(|conversation| conversation.id.as_str()).collect()
}

/// Whether this machine has a Node to run the scripts with. Without one the checks below say so
/// and pass: the scripts themselves are Node's business and the base image carries it.
fn node_is_here() -> bool {
    let found = Command::new("node").arg("--version").output().is_ok();
    if !found {
        eprintln!("no node on this machine: the history scripts were not run");
    }
    found
}

/// Claude Code's folder per workspace, its name made from the path.
fn claude_dir(path: &Path) -> String {
    let word: String = Home::word(path).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    format!(".claude/projects/{word}")
}

/// One conversation of Claude Code's, named by its first prompt.
fn claude_talk(id: &str, prompt: &str) -> String {
    format!(r#"{{"type":"user","message":{{"role":"user","content":"{prompt}"}},"sessionId":"{id}"}}"#)
}

#[test]
fn claude_code_finds_a_conversation_had_before_the_workspace_moved() {
    if !node_is_here() {
        return;
    }
    let home = Home::new("claude");
    let before = claude_talk("11111111-0000-4000-8000-000000000001", "before the rename");
    let today = claude_talk("22222222-0000-4000-8000-000000000002", "after the rename");
    let other = claude_talk("33333333-0000-4000-8000-000000000003", "another workspace");
    home.write(&format!("{}/11111111-0000-4000-8000-000000000001.jsonl", claude_dir(&home.before)), &[&before]);
    home.write(&format!("{}/22222222-0000-4000-8000-000000000002.jsonl", claude_dir(&home.today)), &[&today]);
    home.write(&format!("{}/33333333-0000-4000-8000-000000000003.jsonl", claude_dir(&home.elsewhere)), &[&other]);

    let found = home.read(HarnessKind::ClaudeCode);
    let listed = ids(&found);
    assert!(listed.contains(&"11111111-0000-4000-8000-000000000001"), "the one from before is gone: {listed:?}");
    assert!(listed.contains(&"22222222-0000-4000-8000-000000000002"), "{listed:?}");
    assert!(!listed.contains(&"33333333-0000-4000-8000-000000000003"), "another workspace crept in: {listed:?}");
}

#[test]
fn opencode_finds_a_conversation_had_before_the_workspace_moved() {
    if !node_is_here() {
        return;
    }
    let home = Home::new("opencode");
    let sessions = format!(
        r#"[{{"id":"ses_before","title":"Before the rename","updated":1789745000000,"directory":"{}"}},
            {{"id":"ses_today","title":"After the rename","updated":1789746000000,"directory":"{}"}},
            {{"id":"ses_other","title":"Another workspace","updated":1789744000000,"directory":"{}"}}]"#,
        Home::word(&home.before),
        Home::word(&home.today),
        Home::word(&home.elsewhere),
    );
    // opencode keeps its conversations in a database of its own and is asked for them, so what
    // stands in for it here is a program of that name printing what it prints.
    home.program("opencode", &format!("cat <<'LIST'\n{sessions}\nLIST"));

    let found = home.read(HarnessKind::OpenCode);
    let listed = ids(&found);
    assert!(listed.contains(&"ses_before"), "the one from before is gone: {listed:?}");
    assert!(listed.contains(&"ses_today"), "{listed:?}");
    assert!(!listed.contains(&"ses_other"), "another workspace crept in: {listed:?}");
}

#[test]
fn gemini_cli_finds_a_conversation_had_before_the_workspace_moved() {
    if !node_is_here() {
        return;
    }
    let home = Home::new("gemini");
    home.write(
        ".gemini/projects.json",
        &[&format!(
            r#"{{"projects":{{"{}":"before","{}":"today","{}":"other"}}}}"#,
            Home::word(&home.before),
            Home::word(&home.today),
            Home::word(&home.elsewhere),
        )],
    );
    let talk = |id: &str, said: &str| {
        [
            format!(
                r#"{{"sessionId":"{id}","projectHash":"h","startTime":"2026-09-18T15:00:00.000Z","lastUpdated":"2026-09-18T15:00:00.000Z","kind":"main"}}"#
            ),
            format!(r#"{{"id":"m1","type":"user","content":[{{"text":"{said}"}}]}}"#),
        ]
    };
    for (slug, id, said) in [
        ("before", "aaaa1111-2222-4333-8444-000000000001", "before the rename"),
        ("today", "bbbb2222-2222-4333-8444-000000000002", "after the rename"),
        ("other", "cccc3333-2222-4333-8444-000000000003", "another workspace"),
    ] {
        let lines = talk(id, said);
        home.write(
            &format!(".gemini/tmp/{slug}/chats/session-2026-09-18T15-00-{slug}.jsonl"),
            &[lines[0].as_str(), lines[1].as_str()],
        );
    }

    let found = home.read(HarnessKind::GeminiCli);
    let listed = ids(&found);
    assert!(listed.contains(&"aaaa1111-2222-4333-8444-000000000001"), "the one from before is gone: {listed:?}");
    assert!(listed.contains(&"bbbb2222-2222-4333-8444-000000000002"), "{listed:?}");
    assert!(!listed.contains(&"cccc3333-2222-4333-8444-000000000003"), "another workspace crept in: {listed:?}");
}

#[test]
fn gemini_cli_finds_the_folder_of_before_by_its_root_file() {
    if !node_is_here() {
        return;
    }
    // Without the registry the folder is found by the workspace it names as its root, which is
    // the other half of the script's way in and has to know both directories as well.
    let home = Home::new("gemini-root");
    home.write(".gemini/tmp/before/.project_root", &[&Home::word(&home.before)]);
    home.write(".gemini/tmp/other/.project_root", &[&Home::word(&home.elsewhere)]);
    home.write(
        ".gemini/tmp/before/chats/session-2026-09-18T15-00-before.jsonl",
        &[
            r#"{"sessionId":"aaaa1111-2222-4333-8444-000000000001","projectHash":"h","startTime":"2026-09-18T15:00:00.000Z","lastUpdated":"2026-09-18T15:00:00.000Z","kind":"main"}"#,
            r#"{"id":"m1","type":"user","content":[{"text":"before the rename"}]}"#,
        ],
    );

    assert_eq!(ids(&home.read(HarnessKind::GeminiCli)), ["aaaa1111-2222-4333-8444-000000000001"]);
}

#[test]
fn codex_finds_a_conversation_had_before_the_workspace_moved() {
    if !node_is_here() {
        return;
    }
    let home = Home::new("codex");
    for (id, dir, said) in [
        ("01a0b53a-7904-7862-b8f4-02774d17df35", &home.before, "before the rename"),
        ("02b0b53a-7904-7862-b8f4-02774d17df35", &home.today, "after the rename"),
        ("03c0b53a-7904-7862-b8f4-02774d17df35", &home.elsewhere, "another workspace"),
    ] {
        let meta = format!(
            r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{}","cli_version":"0.155.0"}}}}"#,
            Home::word(dir)
        );
        let message = format!(
            r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{said}"}}]}}}}"#
        );
        home.write(
            &format!(".codex/sessions/2026/09/18/rollout-2026-09-18T15-55-03-{id}.jsonl"),
            &[meta.as_str(), message.as_str()],
        );
    }

    let found = home.read(HarnessKind::Codex);
    let listed = ids(&found);
    assert!(listed.contains(&"01a0b53a-7904-7862-b8f4-02774d17df35"), "the one from before is gone: {listed:?}");
    assert!(listed.contains(&"02b0b53a-7904-7862-b8f4-02774d17df35"), "{listed:?}");
    assert!(!listed.contains(&"03c0b53a-7904-7862-b8f4-02774d17df35"), "another workspace crept in: {listed:?}");
}
