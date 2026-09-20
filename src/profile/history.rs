//! A harness's own record of the conversations it had in a project, and the line that opens one
//! of them again.
//!
//! Every harness keeps its conversations under its home directory, which in QCode is the
//! project's home volume for the profile. The host cannot read a volume by itself (Docker keeps
//! its volumes where only root reaches), so the reading happens inside the profile's own
//! container: the base image carries Node, and each harness has one short script, run with
//! `node -e`, that knows where that harness writes and prints what it found. The script is the
//! only part that knows a harness's file format; everything it prints has one shape for all of
//! them, and [`parse`] is where that shape is checked.
//!
//! What a script prints, one line per conversation of the project in [`PROJECT_DIR`] and of no
//! other: the conversation's id, a tab, when it was last used in Unix milliseconds, a tab, and
//! its title, which may be empty. A missing folder, an unreadable file or a record in a shape the
//! script does not know is left out rather than guessed at, and nothing a script meets makes it
//! fail: a harness that was never used in the project has simply had no conversations there.
//! Each script reads files a line at a time, never holds a line longer than a mebibyte and stops
//! a file after 256 MiB, so a transcript grown huge costs time but never the container's memory.
//!
//! Where each harness keeps its conversations was read from its documentation and source and
//! checked in throwaway containers against Claude Code 2.1.276, opencode 1.18.31, Gemini CLI
//! 0.60.0 and Codex 0.155.0; the script of each one says what it relies on, and
//! `history_live.rs` runs every script against records written the way those versions write
//! them.

use super::HarnessKind;
use crate::base::paths::PROJECT_DIR;
use crate::engine::run::{EngineError, capture};
use crate::engine::{ContainerState, Engine, EngineCommand, Exec};
use std::collections::HashSet;

/// The most characters of a title that are kept. A title is a line in a list; the first words
/// say which conversation it is, and a first prompt pasted whole would say nothing more.
pub const TITLE_CHARS: usize = 200;

/// The longest id that is taken. Every harness here writes ids of 36 characters or fewer; one
/// far longer is not an id any of them made.
const ID_CHARS: usize = 128;

/// One conversation a harness had in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// The harness's own id for it, the word its resume argument takes.
    pub id: String,
    /// What the harness or the person named it, or failing that its first prompt; `None` when
    /// it has neither.
    pub title: Option<String>,
    /// When it was last used, in milliseconds since the Unix epoch.
    pub used_ms: i64,
}

/// Whether `id` can be handed to a harness on its command line as a conversation's id.
///
/// The id is read from files the harness and whatever ran inside the container wrote, so it is
/// held to the shapes the harnesses really make (UUIDs, and opencode's `ses_` followed by
/// letters and digits) rather than trusted: only ASCII letters, digits, `-` and `_`, starting
/// with a letter or a digit so it can never be read as an option, and no longer than any of
/// them writes.
#[must_use]
pub fn is_safe_id(id: &str) -> bool {
    id.len() <= ID_CHARS
        && id.chars().next().is_some_and(|first| first.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Reads what a harness's script printed: every conversation it names, newest first.
///
/// A line that is not an id, a time and a title is left out, and so is one whose id could not
/// be passed on safely ([`is_safe_id`]). A title has its control characters turned into spaces
/// again, its runs of white space made one space, and is cut to [`TITLE_CHARS`]; one that is
/// empty after that is no title. An id that comes twice is kept once, with its newest use.
#[must_use]
pub fn parse(output: &str) -> Vec<Conversation> {
    let mut found: Vec<Conversation> = output.lines().filter_map(conversation).collect();
    found.sort_by(|a, b| b.used_ms.cmp(&a.used_ms).then_with(|| a.id.cmp(&b.id)));
    let mut seen = HashSet::new();
    found.retain(|conversation| seen.insert(conversation.id.clone()));
    found
}

/// One line of a script's output, when it is one.
fn conversation(line: &str) -> Option<Conversation> {
    let mut fields = line.splitn(3, '\t');
    let id = fields.next()?.trim();
    let used_ms = fields.next()?.trim().parse().ok()?;
    let title = fields.next().unwrap_or_default();
    is_safe_id(id).then(|| Conversation { id: id.to_owned(), title: tidy(title), used_ms })
}

/// A title as a list shows it: one line, single spaces, not too long.
fn tidy(title: &str) -> Option<String> {
    let words: Vec<&str> =
        title.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let joined = words.join(" ");
    let cut: String = joined.chars().take(TITLE_CHARS).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_owned())
}

/// What every script starts with: where to look, how to print, and a line reader that neither
/// throws nor holds more than it must.
macro_rules! prelude {
    () => {
        r#"const fs = require('fs'), path = require('path'), os = require('os');
const project = process.argv[1] || '', home = os.homedir();
const LINE_CAP = 1 << 20, FILE_CAP = 256 << 20, TITLE_CAP = 1000;
const clean = (s) => typeof s === 'string' ? s.replace(/[\p{Cc}\p{Zl}\p{Zp}]+/gu, ' ').trim().slice(0, TITLE_CAP) : '';
const emit = (id, ms, title) => {
  if (typeof id !== 'string' || !id) return;
  const at = Number.isFinite(ms) ? Math.floor(ms) : 0;
  process.stdout.write(clean(id) + '\t' + at + '\t' + clean(title) + '\n');
};
const json = (s) => { try { return JSON.parse(s); } catch (e) { return undefined; } };
const text = (c) => typeof c === 'string' ? c : Array.isArray(c) ? c.map((p) => p && typeof p.text === 'string' ? p.text : '').join('') : '';
const stat = (f) => { try { return fs.statSync(f); } catch (e) { return undefined; } };
const list = (d) => { try { return fs.readdirSync(d); } catch (e) { return []; } };
const small = (f) => { const s = stat(f); if (!s || !s.isFile() || s.size > LINE_CAP) return ''; try { return fs.readFileSync(f, 'utf8'); } catch (e) { return ''; } };
const lines = (file, each) => {
  let fd;
  try { fd = fs.openSync(file, 'r'); } catch (e) { return; }
  const buf = Buffer.alloc(1 << 16);
  let parts = [], size = 0, over = false, total = 0;
  const take = (piece) => { if (over) return; if (size + piece.length > LINE_CAP) { over = true; parts = []; size = 0; return; } parts.push(Buffer.from(piece)); size += piece.length; };
  const end = () => { const line = over ? null : Buffer.concat(parts).toString('utf8'); parts = []; size = 0; over = false; return line === null ? undefined : each(line); };
  try {
    while (total < FILE_CAP) {
      const n = fs.readSync(fd, buf, 0, buf.length, null);
      if (n <= 0) break;
      total += n;
      let start = 0;
      for (let i = 0; i < n; i++) {
        if (buf[i] !== 10) continue;
        take(buf.subarray(start, i));
        start = i + 1;
        if (end() === false) return;
      }
      take(buf.subarray(start, n));
    }
    if (size > 0) end();
  } catch (e) {
  } finally {
    try { fs.closeSync(fd); } catch (e) {}
  }
};
"#
    };
}

/// Claude Code keeps one file per conversation, `~/.claude/projects/<project>/<id>.jsonl`, where
/// `<project>` is the working directory with every character that is not a letter or a digit
/// made `-` (`/work/Project` is `-work-Project`; a path over 200 characters would be cut and
/// hashed, which [`PROJECT_DIR`] never is). The session docs
/// (`code.claude.com/docs/en/sessions`) name the picker's order: a name the person gave, a
/// summary, the first prompt. A name is a `{"type":"custom-title","customTitle":...}` line (the
/// last one counts), older versions wrote `{"type":"summary","summary":...}`, and the first
/// prompt is the first `{"type":"user","message":{"content":...}}` line whose content is text,
/// as a string or as `{"type":"text","text":...}` parts; one starting with `<` is a command's
/// output the harness put there, not something the person typed. A file with no line naming a
/// `sessionId` holds no conversation the harness could open. The file's change time is when the
/// conversation was last used.
const CLAUDE_CODE: &str = concat!(
    prelude!(),
    r#"try {
  const dir = path.join(home, '.claude', 'projects', project.replace(/[^A-Za-z0-9]/g, '-'));
  for (const name of list(dir)) {
    const file = path.join(dir, name), s = stat(file);
    if (!name.endsWith('.jsonl') || !s || !s.isFile()) continue;
    let custom = '', summary = '', first = '', any = false;
    lines(file, (l) => {
      any = any || l.includes('"sessionId"');
      if (!l.includes('"custom-title"') && !l.includes('"summary"') && (first || !l.includes('"user"'))) return;
      const r = json(l);
      if (!r || typeof r !== 'object') return;
      if (r.type === 'custom-title' && typeof r.customTitle === 'string') custom = r.customTitle;
      else if (r.type === 'summary' && typeof r.summary === 'string') summary = r.summary;
      else if (r.type === 'user' && !first && !r.isMeta && !r.isSidechain && r.message) {
        const t = text(r.message.content).trim();
        if (t && !t.startsWith('<')) first = t;
      }
    });
    if (any) emit(name.slice(0, -'.jsonl'.length), s.mtimeMs, clean(custom) || clean(summary) || first);
  }
} catch (e) {}
"#
);

/// opencode keeps its conversations in a database of its own
/// (`~/.local/share/opencode/opencode.db`), so it is asked instead: `opencode session list
/// --format json` (`opencode.ai/docs/cli`) prints `[{"id","title","updated","created",
/// "projectId","directory"}]` with times in milliseconds, and nothing at all when there are
/// none. It lists by project, and a folder git will not vouch for (the mounted project belongs
/// to another user as far as git inside can tell) falls into one `global` project shared with
/// every other such folder, so the list is narrowed to `directory` here. A conversation nobody
/// named keeps the title `New session - <ISO time>` (`Child session - ` for one the harness
/// opened on its own), which says nothing a list's time column does not, so it counts as no title.
/// `--pure` keeps plugins the person configured from loading just to list sessions.
const OPENCODE: &str = concat!(
    prelude!(),
    r#"try {
  let out = '';
  try {
    out = require('child_process').execFileSync('opencode', ['session', 'list', '--format', 'json', '--pure'], {
      cwd: project, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], maxBuffer: 64 << 20, timeout: 60000,
    });
  } catch (e) {}
  const all = json(out.slice(Math.max(out.indexOf('['), 0)));
  for (const s of Array.isArray(all) ? all : []) {
    if (!s || s.directory !== project) continue;
    const placeholder = typeof s.title === 'string' && /^(New|Child) session - \d{4}-\d\d-\d\dT[\d:.]+Z$/.test(s.title);
    emit(s.id, s.updated, placeholder ? '' : s.title);
  }
} catch (e) {}
"#
);

/// Gemini CLI (`geminicli.com/docs/cli/session-management`) keeps a folder per project,
/// `~/.gemini/tmp/<slug>/chats/`, with the slug for a path in `~/.gemini/projects.json`
/// (`{"projects":{"<path>":"<slug>"}}`) and the path again in `~/.gemini/tmp/<slug>/.project_root`.
/// A conversation is a `session-*.jsonl` file (`.json` in older versions, one object); its first
/// line holds `sessionId`, `projectHash`, `startTime` and `lastUpdated`, each message is a line
/// with an `id`, `{"$set":{...}}` lines update the first (`summary`, `lastUpdated`) and
/// `{"$rewindTo":<id>}` drops that message and every one after it. This follows
/// `loadConversationRecord` and `getAllSessionFiles` in the 0.60.0 bundle: a conversation
/// without a message worth resuming (a user message that is not empty and not a `/` or `?`
/// command, or an answer) is one the harness itself would not offer, conversations whose
/// `kind` is not `main` (the harness's own helpers) are left out, and the title is the summary or else the first such user message.
const GEMINI_CLI: &str = concat!(
    prelude!(),
    r#"try {
  const root = path.join(home, '.gemini');
  const known = json(small(path.join(root, 'projects.json')));
  let slug = known && known.projects && typeof known.projects[project] === 'string' ? known.projects[project] : '';
  if (!slug) slug = list(path.join(root, 'tmp')).find((d) => small(path.join(root, 'tmp', d, '.project_root')).trim() === project) || '';
  const dir = slug && !slug.includes('/') && slug !== '.' && slug !== '..' ? path.join(root, 'tmp', slug, 'chats') : '';
  const message = (m) => {
    const t = text(m.content).trim();
    const real = m.type === 'user'
      ? t.length > 0 && !/^(\/|\?|<session_context>|<hook_context>)/.test(t)
      : m.type === 'gemini' && (t.length > 0 || (Array.isArray(m.toolCalls) && m.toolCalls.length > 0) || (Array.isArray(m.thoughts) && m.thoughts.length > 0));
    return { id: m.id, user: m.type === 'user', text: t, real };
  };
  const messages = (all) => Array.isArray(all) ? all.filter((m) => m && typeof m.id === 'string').map(message) : [];
  for (const name of dir ? list(dir) : []) {
    const file = path.join(dir, name), s = stat(file);
    if (!name.startsWith('session-') || !/\.jsonl?$/.test(name) || !s || !s.isFile()) continue;
    let meta = {}, said = [];
    const record = (r) => {
      if (!r || typeof r !== 'object') return;
      if (typeof r.$rewindTo === 'string') {
        const at = said.findIndex((m) => m.id === r.$rewindTo);
        said = at >= 0 ? said.slice(0, at) : [];
      } else if (typeof r.id === 'string') {
        said.push(message(r));
      } else if (r.$set && typeof r.$set === 'object') {
        if (Array.isArray(r.$set.messages)) said = messages(r.$set.messages);
        meta = { ...meta, ...r.$set, messages: undefined };
      } else if (typeof r.sessionId === 'string' && typeof r.projectHash === 'string') {
        said = said.concat(messages(r.messages));
        meta = { ...meta, ...r, messages: undefined };
      }
    };
    lines(file, (l) => { if (l.trim()) record(json(l)); });
    if ((!meta.sessionId || !meta.projectHash) && s.size <= FILE_CAP / 4) {
      meta = {};
      said = [];
      try { record(json(fs.readFileSync(file, 'utf8'))); } catch (e) {}
    }
    if (typeof meta.sessionId !== 'string' || (meta.kind !== undefined && meta.kind !== 'main') || !said.some((m) => m.real)) continue;
    const first = said.find((m) => m.user && m.real);
    const used = Date.parse(meta.lastUpdated) || Date.parse(meta.startTime) || s.mtimeMs;
    emit(meta.sessionId, used, clean(meta.summary) || (first ? first.text : ''));
  }
} catch (e) {}
"#
);

/// Codex keeps every project's conversations together, as
/// `~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl` (`codex-rs/rollout/src/lib.rs` in
/// `github.com/openai/codex`), so each file's first line is read: `{"type":"session_meta",
/// "payload":{"id","cwd",...}}`, and only those whose `cwd` is the project count. A name given
/// with `/rename` is in `~/.codex/session_index.jsonl` as `{"id","thread_name","updated_at"}`,
/// the last line for an id winning (`codex-rs/rollout/src/session_index.rs`). Without one the
/// title is the first thing the person said: a `user_message` event, or a user
/// `response_item` message that is not one of the blocks Codex puts in itself, which all start
/// with a tag such as `<environment_context>` or with a heading naming an instructions file
/// (`# <FILE>.md instructions`). The file's
/// change time is when the conversation was last used. `CODEX_HOME` moves the whole folder, as
/// it does for Codex.
const CODEX: &str = concat!(
    prelude!(),
    r#"try {
  const root = process.env.CODEX_HOME || path.join(home, '.codex');
  const names = new Map();
  lines(path.join(root, 'session_index.jsonl'), (l) => {
    const r = json(l);
    if (r && typeof r.id === 'string' && typeof r.thread_name === 'string') names.set(r.id, r.thread_name);
  });
  const injected = /^(<[A-Za-z_][\w-]*>|# [A-Za-z_-]+\.md instructions)/;
  const walk = (dir, depth) => {
    for (const name of list(dir)) {
      const file = path.join(dir, name), s = stat(file);
      if (!s) continue;
      if (s.isDirectory()) { if (depth < 4) walk(file, depth + 1); continue; }
      if (!s.isFile() || !name.startsWith('rollout-') || !name.endsWith('.jsonl')) continue;
      let id = '', first = '', seen = 0;
      lines(file, (l) => {
        const r = json(l);
        if (seen++ === 0) {
          const p = r && r.type === 'session_meta' && r.payload;
          if (!p || p.cwd !== project || typeof p.id !== 'string') return false;
          id = p.id;
          return names.has(id) ? false : undefined;
        }
        const p = r && r.payload;
        if (!p) return;
        let t = '';
        if (r.type === 'event_msg' && p.type === 'user_message') t = typeof p.message === 'string' ? p.message.trim() : '';
        else if (r.type === 'response_item' && p.type === 'message' && p.role === 'user') t = text(p.content).trim();
        if (t && !injected.test(t)) { first = t; return false; }
      });
      if (id) emit(id, s.mtimeMs, clean(names.get(id)) || first);
    }
  };
  walk(path.join(root, 'sessions'), 0);
} catch (e) {}
"#
);

impl HarnessKind {
    /// The Node script that prints this harness's conversations in the project whose folder is
    /// its first argument, in the shape [`parse`] reads.
    ///
    /// `None` for a harness whose conversations QCode cannot list: the window harness keeps its
    /// own, under [`HarnessKind::conversation_paths`], in a shape that was never read, and a list
    /// guessed at would offer conversations that do not open.
    #[must_use]
    pub fn history_script(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode => Some(CLAUDE_CODE),
            Self::OpenCode => Some(OPENCODE),
            Self::GeminiCli => Some(GEMINI_CLI),
            Self::Codex => Some(CODEX),
            Self::AntigravityIde => None,
        }
    }

    /// Where this harness keeps its conversations, relative to the home directory: the folders
    /// and files the scripts above read, and nothing beside them.
    ///
    /// This is what a backup of a profile's conversations takes, and it is a list of what may
    /// go rather than of what may not: the backup is a plain folder on the machine, and a new
    /// harness version that puts its login somewhere new would slip past a list of what to
    /// leave out, never past this one. opencode's list is its database and the two journals
    /// SQLite keeps beside it, because the three only make one database together.
    #[must_use]
    pub fn conversation_paths(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeCode => &[".claude/projects"],
            Self::OpenCode => &[
                ".local/share/opencode/opencode.db",
                ".local/share/opencode/opencode.db-wal",
                ".local/share/opencode/opencode.db-shm",
            ],
            Self::GeminiCli => &[".gemini/tmp", ".gemini/projects.json"],
            Self::Codex => &[".codex/sessions", ".codex/session_index.jsonl"],
            // The agent inside the window keeps its work here: the trial found `conversations/`,
            // `brain/`, `knowledge/` and `html_artifacts/` under it after one session. QCode does
            // not read the shape of any of it; it only knows the folder, which is what a backup
            // needs. The editor's own state lives elsewhere and stays out.
            Self::AntigravityIde => &[".gemini/antigravity-ide"],
        }
    }

    /// The database this harness keeps its conversations in, when it keeps them in one, relative
    /// to the home directory. Its journals are the same path with `-wal` and `-shm` after it.
    ///
    /// A database copied while the harness writes to it can be a torn copy, and its journal only
    /// fits the database it was written with; so a harness with one is only backed up and
    /// brought back while it is stopped.
    #[must_use]
    pub fn conversation_database(self) -> Option<&'static str> {
        match self {
            Self::OpenCode => Some(".local/share/opencode/opencode.db"),
            Self::ClaudeCode | Self::GeminiCli | Self::Codex | Self::AntigravityIde => None,
        }
    }
}

/// The commands that read a harness's conversations out of a project's container for its
/// profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Asks whether the container is there and whether it runs.
    pub state: EngineCommand,
    /// Starts it, when it is there but stopped.
    pub start: EngineCommand,
    /// Runs the harness's script in it, as the user the container runs as, without a terminal.
    pub list: EngineCommand,
}

/// Spells out the commands that read `harness`'s conversations from `container`, or `None` for a
/// harness whose conversations QCode cannot list.
#[must_use]
pub fn reading(engine: &Engine, container: &str, harness: HarnessKind) -> Option<Reading> {
    let command = ["node", "-e", harness.history_script()?, PROJECT_DIR];
    Some(Reading {
        state: engine.container_state(container),
        start: engine.start_container(container),
        list: engine.exec_without_terminal(&Exec { container, command: &command }),
    })
}

/// What reading takes, given what the engine said about the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Steps {
    /// The container is not there: the project has never run the profile, so it has had no
    /// conversations with it, and nothing is made just to find that out.
    Nothing,
    /// It runs: the script is run in it.
    List,
    /// It is there and not running: it is started, and then the script is run.
    StartThenList,
}

/// The steps for a container in `state`; `None` is a container the engine could not answer for.
fn steps(state: Option<&ContainerState>) -> Steps {
    match state {
        None => Steps::Nothing,
        Some(ContainerState::Running) => Steps::List,
        Some(_) => Steps::StartThenList,
    }
}

/// Reads `harness`'s conversations in the project out of `container`, newest first.
///
/// A container that is not there has had no conversations, and nothing is created for the
/// question. One that is stopped is started and left running: the answer is read to open one of
/// the conversations in it, and that is a tab in the same container.
///
/// This runs engine commands and waits for them, so it belongs on a background thread.
///
/// # Errors
///
/// When the engine cannot be started, or refuses to start the container or to run the script.
pub fn read(engine: &Engine, container: &str, harness: HarnessKind) -> Result<Vec<Conversation>, EngineError> {
    read_starting(engine, container, harness, &mut || {})
}

/// [`read`], calling `started` when the container was stopped and had to be started for it, so
/// the caller can note a container QCode left running.
///
/// # Errors
///
/// When the engine cannot be started, or refuses to start the container or to run the script.
pub fn read_starting(
    engine: &Engine,
    container: &str,
    harness: HarnessKind,
    started: &mut dyn FnMut(),
) -> Result<Vec<Conversation>, EngineError> {
    // A harness whose conversations QCode cannot list has none to show, and nothing is run to
    // find that out: no container is started and no engine is asked.
    let Some(reading) = reading(engine, container, harness) else { return Ok(Vec::new()) };
    // Both engines answer a name they do not know with an error rather than with a state.
    let state = capture(&reading.state).ok().map(|word| ContainerState::parse(&word));
    match steps(state.as_ref()) {
        Steps::Nothing => return Ok(Vec::new()),
        Steps::List => {}
        Steps::StartThenList => {
            capture(&reading.start)?;
            started();
        }
    }
    Ok(parse(&capture(&reading.list)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineKind;

    fn args(command: &EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    fn ids(found: &[Conversation]) -> Vec<&str> {
        found.iter().map(|conversation| conversation.id.as_str()).collect()
    }

    fn script(harness: HarnessKind) -> &'static str {
        harness.history_script().expect("a command-line harness has a script")
    }

    #[test]
    fn reads_every_line_newest_first() {
        let output = "2afe99eb-008a-4542-b160-1aa5b29bb95f\t1789746000000\tNamed by qcode\n\
                      ses_f4acc7e75ffeEArYIV9UJooqnn\t1789746449823\tMy explicit title\n\
                      01a0b53a-7904-7862-b8f4-02774d17df35\t1789746926000\t\n";
        assert_eq!(
            parse(output),
            [
                Conversation {
                    id: "01a0b53a-7904-7862-b8f4-02774d17df35".to_owned(),
                    title: None,
                    used_ms: 1_789_746_926_000
                },
                Conversation {
                    id: "ses_f4acc7e75ffeEArYIV9UJooqnn".to_owned(),
                    title: Some("My explicit title".to_owned()),
                    used_ms: 1_789_746_449_823
                },
                Conversation {
                    id: "2afe99eb-008a-4542-b160-1aa5b29bb95f".to_owned(),
                    title: Some("Named by qcode".to_owned()),
                    used_ms: 1_789_746_000_000
                },
            ]
        );
    }

    #[test]
    fn nothing_printed_is_no_conversations() {
        assert_eq!(parse(""), []);
        assert_eq!(parse("\n\n"), []);
    }

    #[test]
    fn a_line_that_is_not_a_conversation_is_left_out() {
        let output = "just words\n\
                      abc\tnot-a-time\ttitle\n\
                      \t1789746000000\tno id\n\
                      abc\t1789746000000.5\tfractional time\n\
                      abc\n\
                      good\t5\n\
                      Error: something went wrong\n";
        let found = parse(output);
        assert_eq!(ids(&found), ["good"]);
        assert_eq!(found[0].title, None, "a line without a title field has no title");
    }

    #[test]
    fn an_id_that_could_be_read_as_anything_but_an_id_is_refused() {
        let output = "--dangerously-skip-permissions\t9\tan option\n\
                      -r\t9\ta short option\n\
                      _hidden\t9\tstarts with a mark\n\
                      two words\t9\twhite space\n\
                      a;rm -rf /\t9\ta shell\n\
                      ../../etc\t9\ta path\n\
                      \u{0430}bc\t9\tnot ascii\n\
                      $(id)\t9\ta substitution\n\
                      ok-1_2\t1\tfine\n";
        assert_eq!(ids(&parse(output)), ["ok-1_2"]);
        assert!(!is_safe_id(&"a".repeat(ID_CHARS + 1)));
        assert!(is_safe_id(&"a".repeat(ID_CHARS)));
        assert!(!is_safe_id(""));
    }

    #[test]
    fn a_title_is_one_tidy_line_of_bounded_length() {
        let output = "a\t3\t  line one\u{1b}[31m\u{7}\r\u{85}line  two\u{2028}end  \n\
                     b\t2\t\u{1}\u{2}   \u{7f}\n\
                     c\t1\t";
        let long = "x".repeat(TITLE_CHARS * 50);
        let output = format!("{output}{long}\nd\t0\t{}\n", "é".repeat(TITLE_CHARS + 3));
        let found = parse(&output);
        assert_eq!(found[0].title.as_deref(), Some("line one [31m line two end"));
        assert_eq!(found[1].title, None, "nothing but control characters is no title");
        assert_eq!(found[2].title.as_deref().map(str::len), Some(TITLE_CHARS));
        assert_eq!(found[3].title.as_deref().map(|title| title.chars().count()), Some(TITLE_CHARS));
    }

    #[test]
    fn a_tab_inside_a_title_stays_part_of_the_title() {
        assert_eq!(parse("a\t1\tone\ttwo")[0].title.as_deref(), Some("one two"));
    }

    #[test]
    fn an_id_named_twice_is_kept_once_with_its_newest_use() {
        let output = "same\t10\told name\nother\t20\t\nsame\t30\tnew name\nsame\t5\toldest\n";
        let found = parse(output);
        assert_eq!(ids(&found), ["same", "other"]);
        assert_eq!(found[0].title.as_deref(), Some("new name"));
        assert_eq!(found[0].used_ms, 30);
    }

    #[test]
    fn equal_times_keep_one_order_every_time() {
        assert_eq!(ids(&parse("b\t1\t\na\t1\t\nc\t1\t\n")), ["a", "b", "c"]);
    }

    #[test]
    fn a_time_before_the_epoch_or_far_ahead_still_sorts() {
        assert_eq!(ids(&parse("past\t-5\t\nfuture\t9223372036854775807\t\nnow\t0\t\n")), ["future", "now", "past"]);
    }

    #[test]
    fn every_harness_has_a_script_that_reads_the_project_it_is_given() {
        for harness in HarnessKind::TERMINAL {
            let script = harness.history_script().expect("a command-line harness has a script");
            assert!(script.starts_with(prelude!()), "{harness:?}");
            assert!(script.len() > prelude!().len(), "{harness:?} has only the prelude");
            assert!(script.contains("process.argv[1]"), "{harness:?}");
            assert!(script.contains("emit("), "{harness:?} never prints");
            assert!(!script.contains("process.exit("), "{harness:?}: an early exit could cut the output short");
        }
    }

    #[test]
    fn each_script_looks_where_its_harness_writes() {
        assert!(script(HarnessKind::ClaudeCode).contains("'.claude', 'projects'"));
        assert!(script(HarnessKind::OpenCode).contains("'session', 'list', '--format', 'json'"));
        assert!(script(HarnessKind::GeminiCli).contains("'projects.json'"));
        assert!(script(HarnessKind::Codex).contains("'session_index.jsonl'"));
    }

    #[test]
    fn a_window_harness_has_no_list_and_nothing_is_run_to_find_that_out() {
        // The shape of what the agent in the window writes was never read, so QCode offers
        // nothing rather than a list of conversations that might not open. Asking costs no
        // engine command at all: a stopped container is not started for the question.
        assert_eq!(HarnessKind::AntigravityIde.history_script(), None);
        let engine = Engine::new(EngineKind::Podman, "/usr/bin/podman");
        assert_eq!(reading(&engine, "qcode-p-anti", HarnessKind::AntigravityIde), None);
    }

    #[test]
    fn a_conversation_backup_takes_where_the_scripts_read_and_never_a_login() {
        assert!(script(HarnessKind::ClaudeCode).contains("'.claude', 'projects'"));
        assert!(script(HarnessKind::GeminiCli).contains("'.gemini'") && GEMINI_CLI.contains("'tmp'"));
        assert!(script(HarnessKind::Codex).contains("'.codex'") && CODEX.contains("'sessions'"));
        for harness in HarnessKind::ALL {
            let paths = harness.conversation_paths();
            assert!(!paths.is_empty(), "{harness:?}");
            for path in paths {
                assert!(!path.starts_with('/') && !path.starts_with('~') && !path.starts_with('-'), "{path}");
                assert!(!path.split('/').any(|part| part == ".." || part == "." || part.is_empty()), "{path}");
                for login in harness.record().identity {
                    let inside = |outer: &str, inner: &str| inner == outer || inner.starts_with(&format!("{outer}/"));
                    assert!(!inside(path, login) && !inside(login, path), "{harness:?}: {path} and {login}");
                }
            }
            if let Some(database) = harness.conversation_database() {
                for journal in ["", "-wal", "-shm"] {
                    assert!(paths.contains(&format!("{database}{journal}").as_str()), "{harness:?}: {journal}");
                }
            }
        }
        assert_eq!(HarnessKind::OpenCode.conversation_database(), Some(".local/share/opencode/opencode.db"));
    }

    #[test]
    fn the_script_runs_without_a_terminal_in_the_project_folder() {
        let engine = Engine::new(EngineKind::Podman, "/usr/bin/podman");
        let reading = reading(&engine, "qcode-my-app-claude-sub", HarnessKind::ClaudeCode).expect("a script");
        assert_eq!(
            args(&reading.state),
            ["container", "inspect", "--format", "{{.State.Status}}", "qcode-my-app-claude-sub"]
        );
        assert_eq!(args(&reading.start), ["start", "qcode-my-app-claude-sub"]);
        let list = args(&reading.list);
        assert_eq!(list[..4], ["exec", "qcode-my-app-claude-sub", "node", "-e"]);
        assert_eq!(list[4], script(HarnessKind::ClaudeCode));
        assert_eq!(list[5], PROJECT_DIR);
        assert_eq!(list.len(), 6);
        assert!(!list.iter().any(|arg| arg == "--tty" || arg == "--interactive" || arg == "--user"), "{list:?}");
    }

    #[test]
    fn docker_reads_the_same_way() {
        let engine = Engine::new(EngineKind::Docker, "/usr/bin/docker");
        let reading = reading(&engine, "qcode-p-codex", HarnessKind::Codex).expect("a script");
        assert_eq!(reading.list.program, std::path::Path::new("/usr/bin/docker"));
        assert_eq!(args(&reading.list)[..2], ["exec", "qcode-p-codex"]);
    }

    #[test]
    fn a_container_that_is_not_there_is_not_made_or_started() {
        assert_eq!(steps(None), Steps::Nothing);
    }

    #[test]
    fn a_running_container_is_only_read() {
        assert_eq!(steps(Some(&ContainerState::Running)), Steps::List);
    }

    #[test]
    fn a_stopped_container_is_started_before_it_is_read() {
        for state in [ContainerState::Exited, ContainerState::Created, ContainerState::Unknown("stopping".to_owned())] {
            assert_eq!(steps(Some(&state)), Steps::StartThenList, "{state:?}");
        }
    }

    #[test]
    fn an_engine_that_is_not_there_has_no_container_to_read() {
        // The state question fails before anything else, and a container the engine cannot
        // answer for is one that is not there.
        let engine = Engine::new(EngineKind::Podman, "/qcode/no/such/engine");
        assert_eq!(read(&engine, "qcode-p-codex", HarnessKind::Codex).expect("nothing to read"), []);
    }
}
