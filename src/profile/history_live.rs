//! The part of reading a harness's conversations no unit test can answer for: that each
//! harness's script, run by the Node in the base image, finds the records where the harness
//! writes them, reads them the way the harness does, and keeps out every other project's.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test history_live -- --ignored --test-threads=1
//! ```
//!
//! Claude Code, Gemini CLI and Codex are not installed for this: their scripts read files, so
//! the records are written into a container's home the way those harnesses write them, from
//! lines each of them wrote in a throwaway container (see the comments on the scripts in
//! `history.rs`). opencode keeps a database and is asked through its own command, so its test
//! installs it and makes real conversations with the free model it offers; that one needs the
//! network.
//!
//! Every container here is named `qcode-historytest-…` and is removed again, also when a check
//! fails. The machine's own `qcode/base` is built when it is missing and left, as
//! [`crate::base::ensure`] means it to be.

use std::path::Path;

use super::HarnessKind;
use super::history::{self, Conversation};
use crate::base::paths::{KEEP_ALIVE, PROJECT_DIR};
use crate::engine::names::{BASE_IMAGE, HOSTNAME};
use crate::engine::run::capture;
use crate::engine::{ContainerCreate, ContainerState, Engine, EngineKind, Exec, HostUser, Network, detect};

/// Another project's folder, whose records must never show up in [`PROJECT_DIR`]'s list.
const OTHER_DIR: &str = "/work/Other";

/// A time the fixtures' change times count from, in seconds since the Unix epoch.
const EPOCH: i64 = 1_789_700_000;

/// The engines installed on this machine, or nothing at all when the tests are switched off.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let found: Vec<Engine> =
        [EngineKind::Podman, EngineKind::Docker].into_iter().filter_map(|kind| detect(kind).ok()).collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// A running container of the base image, as the person, and what it is to be read as.
struct Lab {
    engine: Engine,
    container: String,
    harness: HarnessKind,
}

impl Lab {
    /// Makes and starts the container, leaving out the network unless the harness has to be
    /// installed into it.
    fn open(engine: Engine, harness: HarnessKind, network: Network) -> Self {
        let container = format!("qcode-historytest-{}", harness.record().id);
        let _ = capture(&engine.remove_container(&container));
        crate::base::ensure(&engine, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("the base image does not build on {:?}: {error:?}", engine.kind()));
        capture(&engine.create_container(&ContainerCreate {
            name: &container,
            hostname: HOSTNAME,
            image: BASE_IMAGE,
            mounts: &[],
            network,
            user: HostUser::current().expect("the current user"),
            workdir: Some(Path::new(PROJECT_DIR)),
            command: KEEP_ALIVE,
        }))
        .expect("the container is made");
        capture(&engine.start_container(&container)).expect("the container starts");
        Self { engine, container, harness }
    }

    /// Runs `script` with a shell, `args` as its positional parameters, and answers what it
    /// printed; fails the test with the shell's own words when it refuses.
    fn sh(&self, script: &str, args: &[&str]) -> String {
        let mut command = vec!["sh", "-c", script, "sh"];
        command.extend_from_slice(args);
        capture(&self.engine.exec_without_terminal(&Exec { container: &self.container, command: &command }))
            .unwrap_or_else(|error| panic!("`{script}` on {:?} failed: {error:?}", self.engine.kind()))
    }

    /// Writes `lines` into the file at `path` under the home directory, last changed `at`
    /// seconds after [`EPOCH`]. The contents travel as an argument, so nothing in them is ever
    /// read by a shell.
    fn write(&self, path: &str, lines: &[&str], at: i64) {
        let contents = format!("{}\n", lines.join("\n"));
        let stamp = format!("@{}", EPOCH + at);
        self.sh(
            r#"f="$HOME/$1"; mkdir -p "${f%/*}" && printf '%s' "$2" > "$f" && touch -d "$3" "$f""#,
            &[path, &contents, &stamp],
        );
    }

    /// Reads the conversations the way QCode does, through [`history::read`], from a container
    /// that was stopped first: the read has to start it, and leave it running.
    fn read(&self) -> Vec<Conversation> {
        capture(&self.engine.stop_container(&self.container)).expect("the container stops");
        let found = history::read(&self.engine, &self.container, self.harness)
            .unwrap_or_else(|error| panic!("{:?} on {:?}: {error:?}", self.harness, self.engine.kind()));
        let state = capture(&self.engine.container_state(&self.container)).expect("the container is there");
        assert!(ContainerState::parse(&state).is_running(), "the read left the container {state}");
        found
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = capture(&self.engine.remove_container(&self.container));
    }
}

/// The ids and titles of `found`, in its order.
fn listed(found: &[Conversation]) -> Vec<(&str, Option<&str>)> {
    found.iter().map(|conversation| (conversation.id.as_str(), conversation.title.as_deref())).collect()
}

/// Milliseconds since the Unix epoch of a fixture changed `at` seconds after [`EPOCH`].
fn changed(at: i64) -> i64 {
    (EPOCH + at) * 1000
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_container_that_is_not_there_has_no_conversations_and_is_not_made() {
    for engine in engines() {
        let name = "qcode-historytest-absent";
        let _ = capture(&engine.remove_container(name));
        assert_eq!(history::read(&engine, name, HarnessKind::ClaudeCode).expect("nothing to read"), []);
        assert!(capture(&engine.container_state(name)).is_err(), "{:?} made a container to read", engine.kind());
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_conversations_are_read_from_the_projects_folder() {
    for engine in engines() {
        let lab = Lab::open(engine, HarnessKind::ClaudeCode, Network::None);
        assert_eq!(lab.read(), [], "a harness never used has no conversations");

        let dir = ".claude/projects/-work-Project";
        // Named twice with /rename: the last name counts.
        lab.write(
            &format!("{dir}/2afe99eb-008a-4542-b160-1aa5b29bb95f.jsonl"),
            &[
                r#"{"type":"custom-title","customTitle":"Named by qcode","sessionId":"2afe99eb-008a-4542-b160-1aa5b29bb95f"}"#,
                r#"{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"first prompt A"},"uuid":"4897eb82-b5f0-42f8-ab33-4fa47168211d","timestamp":"2026-09-18T15:52:58.302Z","cwd":"/work/Project","sessionId":"2afe99eb-008a-4542-b160-1aa5b29bb95f"}"#,
                r#"{"type":"last-prompt","lastPrompt":"first prompt A","leafUuid":"5197f4f7-f11b-4932-bc9a-bb7d72dd2abe","sessionId":"2afe99eb-008a-4542-b160-1aa5b29bb95f"}"#,
                r#"{"type":"custom-title","customTitle":"Renamed later","sessionId":"2afe99eb-008a-4542-b160-1aa5b29bb95f"}"#,
            ],
            100,
        );
        // No name: the first thing the person typed, not the command output before it.
        lab.write(
            &format!("{dir}/b14908f1-92ef-4c9b-ad54-85a7e0b46045.jsonl"),
            &[
                r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-09-18T15:53:58.463Z","sessionId":"b14908f1-92ef-4c9b-ad54-85a7e0b46045","content":"second prompt B"}"#,
                r#"{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"},"sessionId":"b14908f1-92ef-4c9b-ad54-85a7e0b46045"}"#,
                r#"{"parentUuid":"x","isSidechain":false,"type":"user","isMeta":true,"message":{"role":"user","content":"a note the harness wrote"},"sessionId":"b14908f1-92ef-4c9b-ad54-85a7e0b46045"}"#,
                r#"{"parentUuid":"y","isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"second prompt B"}]},"sessionId":"b14908f1-92ef-4c9b-ad54-85a7e0b46045"}"#,
            ],
            300,
        );
        // An older version's summary line.
        lab.write(
            &format!("{dir}/c0ffee00-0000-4000-8000-000000000001.jsonl"),
            &[
                r#"{"type":"summary","summary":"An older summary","leafUuid":"4897eb82-b5f0-42f8-ab33-4fa47168211d"}"#,
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"not the title"},"sessionId":"c0ffee00-0000-4000-8000-000000000001"}"#,
            ],
            200,
        );
        // A tab and a line break in a prompt reach the list as spaces.
        lab.write(
            &format!("{dir}/f0f0f0f0-0000-4000-8000-000000000004.jsonl"),
            &[
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"line one\tstill\nline two"},"sessionId":"f0f0f0f0-0000-4000-8000-000000000004"}"#,
            ],
            10,
        );
        // A first prompt too long to hold is passed over for the next one.
        lab.sh(
            r#"f="$HOME/$1"; { printf '%s' '{"type":"user","message":{"content":"'; head -c 1200000 /dev/zero | tr '\0' x; printf '%s\n' '"},"sessionId":"e0e0e0e0-0000-4000-8000-000000000003"}'; printf '%s\n' '{"type":"user","message":{"content":"after the long one"},"sessionId":"e0e0e0e0-0000-4000-8000-000000000003"}'; } > "$f" && touch -d "$2" "$f""#,
            &[&format!("{dir}/e0e0e0e0-0000-4000-8000-000000000003.jsonl"), &format!("@{}", EPOCH + 50)],
        );
        // Not a transcript, a name that is an option, a file in a subfolder, another project.
        lab.write(&format!("{dir}/d0d0d0d0-0000-4000-8000-000000000002.jsonl"), &["not json at all", "{"], 400);
        lab.write(
            &format!("{dir}/--resume.jsonl"),
            &[r#"{"type":"user","message":{"content":"x"},"sessionId":"s"}"#],
            500,
        );
        lab.write(
            &format!("{dir}/2afe99eb-008a-4542-b160-1aa5b29bb95f/helpers/one.jsonl"),
            &[r#"{"type":"user","message":{"content":"a helper"},"sessionId":"2afe99eb-008a-4542-b160-1aa5b29bb95f"}"#],
            600,
        );
        lab.write(
            ".claude/projects/-work-Other/aaaaaaaa-0000-4000-8000-000000000005.jsonl",
            &[r#"{"type":"user","message":{"content":"another project"},"cwd":"/work/Other","sessionId":"aaaaaaaa-0000-4000-8000-000000000005"}"#],
            700,
        );

        let found = lab.read();
        assert_eq!(
            listed(&found),
            [
                ("b14908f1-92ef-4c9b-ad54-85a7e0b46045", Some("second prompt B")),
                ("c0ffee00-0000-4000-8000-000000000001", Some("An older summary")),
                ("2afe99eb-008a-4542-b160-1aa5b29bb95f", Some("Renamed later")),
                ("e0e0e0e0-0000-4000-8000-000000000003", Some("after the long one")),
                ("f0f0f0f0-0000-4000-8000-000000000004", Some("line one still line two")),
            ],
            "{:?}",
            lab.engine.kind()
        );
        assert_eq!(found[0].used_ms, changed(300));
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn gemini_cli_conversations_are_read_from_the_projects_chats() {
    for engine in engines() {
        let lab = Lab::open(engine, HarnessKind::GeminiCli, Network::None);
        assert_eq!(lab.read(), []);

        lab.write(".gemini/projects.json", &[r#"{"projects":{"/work/Project":"project","/work/Other":"other"}}"#], 0);
        let chats = ".gemini/tmp/project/chats";
        // A summary set later wins over the first prompt, and its time over the first line's.
        lab.write(
            &format!("{chats}/session-2026-09-18T15-00-aaaa1111.jsonl"),
            &[
                r#"{"sessionId":"aaaa1111-2222-4333-8444-000000000001","projectHash":"h","startTime":"2026-09-18T15:00:00.000Z","lastUpdated":"2026-09-18T15:00:00.000Z","kind":"main"}"#,
                r#"{"id":"m1","timestamp":"2026-09-18T15:00:01.000Z","type":"user","content":[{"text":"/help"}]}"#,
                r#"{"id":"m2","timestamp":"2026-09-18T15:00:02.000Z","type":"user","content":[{"text":"real question"}]}"#,
                r#"{"id":"m3","timestamp":"2026-09-18T15:00:03.000Z","type":"gemini","content":"an answer"}"#,
                r#"{"$set":{"summary":"Summarised by gemini","lastUpdated":"2026-09-18T16:00:00.000Z"}}"#,
            ],
            0,
        );
        // A rewind takes the first question away; the one asked after it names the conversation.
        lab.write(
            &format!("{chats}/session-2026-09-18T14-00-bbbb2222.jsonl"),
            &[
                r#"{"sessionId":"bbbb2222-2222-4333-8444-000000000002","projectHash":"h","startTime":"2026-09-18T14:00:00.000Z","lastUpdated":"2026-09-18T14:00:00.000Z"}"#,
                r#"{"id":"m1","type":"user","content":"first question"}"#,
                r#"{"id":"m2","type":"gemini","content":"first answer"}"#,
                r#"{"$rewindTo":"m1"}"#,
                r#"{"id":"m3","type":"user","content":[{"text":"rewritten question"}]}"#,
                r#"{"id":"m4","type":"gemini","content":"second answer"}"#,
            ],
            0,
        );
        // An older version's single object, written over many lines.
        lab.write(
            &format!("{chats}/session-2026-09-18T13-00-cccc3333.json"),
            &[
                "{",
                r#"  "sessionId": "cccc3333-2222-4333-8444-000000000003","#,
                r#"  "projectHash": "h","#,
                r#"  "startTime": "2026-09-18T13:00:00.000Z","#,
                r#"  "lastUpdated": "2026-09-18T13:00:00.000Z","#,
                r#"  "messages": ["#,
                r#"    {"id": "m1", "type": "user", "content": [{"text": "legacy question"}]},"#,
                r#"    {"id": "m2", "type": "gemini", "content": "legacy answer"}"#,
                "  ]",
                "}",
            ],
            0,
        );
        // Nothing said yet, a helper's of the harness, one in a subfolder, and another project's.
        lab.write(
            &format!("{chats}/session-2026-09-18T17-00-dddd4444.jsonl"),
            &[r#"{"sessionId":"dddd4444-2222-4333-8444-000000000004","projectHash":"h","startTime":"2026-09-18T17:00:00.000Z","lastUpdated":"2026-09-18T17:00:00.000Z"}"#],
            0,
        );
        lab.write(
            &format!("{chats}/session-2026-09-18T18-00-eeee5555.jsonl"),
            &[
                r#"{"sessionId":"eeee5555-2222-4333-8444-000000000005","projectHash":"h","startTime":"2026-09-18T18:00:00.000Z","lastUpdated":"2026-09-18T18:00:00.000Z","kind":"helper"}"#,
                r#"{"id":"m1","type":"user","content":"a helper's task"}"#,
            ],
            0,
        );
        lab.write(
            &format!("{chats}/aaaa1111-2222-4333-8444-000000000001/session-2026-09-18T19-00-12121212.jsonl"),
            &[
                r#"{"sessionId":"12121212-2222-4333-8444-000000000006","projectHash":"h","lastUpdated":"2026-09-18T19:00:00.000Z"}"#,
                r#"{"id":"m1","type":"user","content":"in a subfolder"}"#,
            ],
            0,
        );
        lab.write(
            ".gemini/tmp/other/chats/session-2026-09-18T20-00-ffff6666.jsonl",
            &[
                r#"{"sessionId":"ffff6666-2222-4333-8444-000000000007","projectHash":"o","lastUpdated":"2026-09-18T20:00:00.000Z"}"#,
                r#"{"id":"m1","type":"user","content":"another project"}"#,
            ],
            0,
        );

        let expected = [
            ("aaaa1111-2222-4333-8444-000000000001", Some("Summarised by gemini")),
            ("bbbb2222-2222-4333-8444-000000000002", Some("rewritten question")),
            ("cccc3333-2222-4333-8444-000000000003", Some("legacy question")),
        ];
        let found = lab.read();
        assert_eq!(listed(&found), expected, "{:?}", lab.engine.kind());
        // 2026-09-18T16:00:00Z.
        assert_eq!(found[0].used_ms, 1_789_747_200_000);

        // Without the registry, the folder that names the project as its root is the one.
        lab.sh(r#"rm "$HOME/.gemini/projects.json""#, &[]);
        lab.write(".gemini/tmp/other/.project_root", &[OTHER_DIR], 0);
        lab.write(".gemini/tmp/project/.project_root", &[PROJECT_DIR], 0);
        assert_eq!(listed(&lab.read()), expected, "{:?}: by the project root", lab.engine.kind());
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn codex_conversations_are_the_projects_rollouts() {
    for engine in engines() {
        let lab = Lab::open(engine, HarnessKind::Codex, Network::None);
        assert_eq!(lab.read(), []);

        let day = ".codex/sessions/2026/09/18";
        let meta = |id: &str, cwd: &str| {
            format!(
                r#"{{"timestamp":"2026-09-18T15:55:03.339Z","type":"session_meta","payload":{{"id":"{id}","timestamp":"2026-09-18T15:55:03.304Z","cwd":"{cwd}","originator":"codex_exec","cli_version":"0.155.0","source":"exec"}}}}"#
            )
        };
        let environment = r#"{"timestamp":"2026-09-18T15:55:03.818Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/work/Project</cwd>\n</environment_context>"}]}}"#;
        let developer = r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<skills_instructions>\n## Skills"}]}}"#;
        let said = |text: &str| {
            format!(
                r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
            )
        };

        let hello = "01a0b53a-7904-7862-b8f4-02774d17df35";
        lab.write(
            &format!("{day}/rollout-2026-09-18T15-55-03-{hello}.jsonl"),
            &[&meta(hello, PROJECT_DIR), developer, environment, &said("hello from codex test")],
            100,
        );
        let renamed = "02b0b53a-7904-7862-b8f4-02774d17df35";
        lab.write(
            &format!(".codex/sessions/2026/09/17/rollout-2026-09-17T10-00-00-{renamed}.jsonl"),
            &[&meta(renamed, PROJECT_DIR), &said("will be renamed")],
            200,
        );
        lab.write(
            ".codex/session_index.jsonl",
            &[
                &format!(r#"{{"id":"{renamed}","thread_name":"First name","updated_at":"2026-09-18T10:00:00Z"}}"#),
                "not json",
                &format!(r#"{{"id":"{renamed}","thread_name":"Renamed thread","updated_at":"2026-09-18T11:00:00Z"}}"#),
            ],
            0,
        );
        let event = "03c0b53a-7904-7862-b8f4-02774d17df35";
        lab.write(
            &format!("{day}/rollout-2026-09-18T16-00-00-{event}.jsonl"),
            &[
                &meta(event, PROJECT_DIR),
                r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"from the event"}}"#,
            ],
            300,
        );
        let instructed = "04d0b53a-7904-7862-b8f4-02774d17df35";
        lab.write(
            &format!("{day}/rollout-2026-09-18T17-00-00-{instructed}.jsonl"),
            &[
                &meta(instructed, PROJECT_DIR),
                &said(r"# NOTES.md instructions for /work/Project\n\n<INSTRUCTIONS>\nbe brief\n</INSTRUCTIONS>"),
                &said("after the instructions"),
            ],
            50,
        );
        // Another project's, one whose first line is not its meta, and an archived one.
        let other = "05e0b53a-7904-7862-b8f4-02774d17df35";
        lab.write(
            &format!("{day}/rollout-2026-09-18T18-00-00-{other}.jsonl"),
            &[&meta(other, OTHER_DIR), &said("x")],
            400,
        );
        let headless = "06f0b53a-7904-7862-b8f4-02774d17df35";
        lab.write(
            &format!("{day}/rollout-2026-09-18T19-00-00-{headless}.jsonl"),
            &[&said("no meta first"), &meta(headless, PROJECT_DIR)],
            500,
        );
        let archived = "07a0b53a-7904-7862-b8f4-02774d17df35";
        lab.write(
            &format!(".codex/archived_sessions/rollout-2026-09-18T20-00-00-{archived}.jsonl"),
            &[&meta(archived, PROJECT_DIR), &said("archived")],
            600,
        );

        let found = lab.read();
        assert_eq!(
            listed(&found),
            [
                (event, Some("from the event")),
                (renamed, Some("Renamed thread")),
                (hello, Some("hello from codex test")),
                (instructed, Some("after the instructions")),
            ],
            "{:?}",
            lab.engine.kind()
        );
        assert_eq!(found[2].used_ms, changed(100));
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_conversations_are_what_its_own_list_says_for_the_project() {
    for engine in engines() {
        let lab = Lab::open(engine, HarnessKind::OpenCode, Network::Full);
        let kind = lab.engine.kind();
        for step in HarnessKind::OpenCode.record().install {
            lab.sh(step, &[]);
        }
        assert_eq!(lab.read(), [], "{kind:?}: the empty list opencode prints is no conversations");

        // Real conversations on the free model: one named, one left to the placeholder, and one
        // in another folder. A model that does not answer still leaves its session behind, so a
        // failed run is not a failed test; what the list holds afterwards is the measure.
        let run = r#"d="$1"; shift; cd "$d" && timeout 180 opencode run "$@" >/dev/null 2>&1; true"#;
        lab.sh(run, &[PROJECT_DIR, "--title", "History test", "reply with the word ok"]);
        lab.sh(run, &[PROJECT_DIR, "reply with the word ok"]);
        lab.sh(&format!(r#"mkdir -p /tmp/other && {run}"#), &["/tmp/other", "reply with the word ok"]);

        // opencode's own list, narrowed to the project, as `id<TAB>title` lines.
        let own = lab.sh(
            r#"cd "$1" && opencode session list --format json | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{for(const x of JSON.parse(s||"[]"))if(x.directory===process.argv[1])console.log(x.id+"\t"+x.title)})' "$1""#,
            &[PROJECT_DIR],
        );
        let found = lab.read();
        let mut ids: Vec<&str> = found.iter().map(|conversation| conversation.id.as_str()).collect();
        let mut own_ids: Vec<&str> = own.lines().filter_map(|line| line.split('\t').next()).collect();
        ids.sort_unstable();
        own_ids.sort_unstable();
        assert_eq!(ids, own_ids, "{kind:?}: every conversation of the project and nothing else");
        if own.lines().count() < 2 {
            eprintln!(
                "{kind:?}: opencode made {} of the two conversations; the titles were not checked",
                own.lines().count()
            );
            continue;
        }
        let titles: Vec<Option<&str>> = found.iter().map(|conversation| conversation.title.as_deref()).collect();
        assert!(titles.contains(&Some("History test")), "{kind:?}: {titles:?}");
        assert!(titles.contains(&None), "{kind:?}: the placeholder title is no title: {titles:?}");
        assert!(found.windows(2).all(|pair| pair[0].used_ms >= pair[1].used_ms), "{kind:?}: newest first");
    }
}
