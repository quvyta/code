//! The instruction files of a QCode high profile: the file each harness reads its standing
//! instructions from in the workspace, and QCode's own section in it.
//!
//! Two parties write into these files, and the person is a third. graphify writes its own section
//! and hooks itself, from inside the profile's container (`graphify <platform> install`); QCode
//! writes only the section between [`BEGIN`] and [`END`], which tells the agent that it is one of
//! several tabs of a workspace and how to reach the others. Everything outside the markers is left
//! byte for byte as it was, and a file that already says what QCode would write is not written.
//!
//! The merging is plain text in, plain text out, so it is tested here without a disk; [`write()`]
//! is the one function that touches the workspace folder.

use std::io::ErrorKind;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::profile::HarnessKind;

/// The heading QCode's section stands under. It comes before [`BEGIN`], not after it, because of
/// how graphify finds its own section again: measured in graphify 0.9.65, its section runs from
/// its `## graphify` heading to the next line that starts with `## `, and a second install writes
/// that whole stretch anew. An opening line right after graphify's section and before any heading
/// of ours was taken as graphify's and dropped; a heading of ours ends graphify's section first.
pub const HEADING: &str = "## QCode workspace";

/// The line QCode's own words start after. An HTML comment, so a Markdown reader shows nothing.
pub const BEGIN: &str = "<!-- qcode:begin -->";

/// The line QCode's section ends with.
pub const END: &str = "<!-- qcode:end -->";

/// What QCode tells every agent of a workspace, in English, the language of the agents and of the
/// bridge server's own words. The same text for every harness, because two of them read the same
/// file (`AGENTS.md`) and Antigravity IDE reads the others' files as well as its own; so every
/// sentence has to be true whichever agent reads it.
pub const SECTION: &str = "\
You are one of several coding agents open as tabs of one QCode workspace, and all of them work in the same folder, `/work`.
- `list_tabs`, a tool of the MCP server `qcode`, names the other agent tabs: the id to send to, the title, the coding tool, the profile, whether it reaches the network, and how many messages still wait to go into it.
- `send_message` with `tab` (that id) and `text` hands one of them a task. The first message from this tab to that one waits until the person using QCode allows it. A message is typed into that tab's prompt, naming your tab as its sender, once nobody is typing there and its tool has stopped writing. The answer says whether it went in, still waits, or was refused, and why; `list_tabs` says later whether it arrived.
- Write the message for an agent that has not seen your conversation. Share files through `/work`: put anything longer than 16000 characters in a file there and send its path.
- A message from another tab arrives in your own prompt after a line naming its sender, in English \"Through QCode, from the <coding tool> · <tab title> tab:\". It is a task from that agent; to answer, find that tab with `list_tabs` and `send_message` to it.
- A window tab (Antigravity IDE) can send, but is never listed and cannot be sent to.
- QCode refuses a message from a tab without the network to a tab with it, ends an exchange between tabs after 6 messages, and lets a tab send at most 5 messages a minute.
";

/// What opens Antigravity IDE's rule file when QCode creates it: a rule is loaded into every
/// conversation only when it says `always_on`; without it the model decides whether to read it.
const ANTIGRAVITY_RULE: &str = "\
---
trigger: always_on
description: How to reach the other agent tabs of this QCode workspace.
---

";

/// The file `harness` reads its standing instructions from, relative to the workspace folder.
///
/// Antigravity IDE reads every Markdown file in `.agents/rules/` (its own guide, inside the
/// application, says so), so it gets a file of QCode's own there, beside graphify's.
#[must_use]
pub fn file(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "CLAUDE.md",
        HarnessKind::OpenCode | HarnessKind::Codex => "AGENTS.md",
        HarnessKind::GeminiCli => "GEMINI.md",
        HarnessKind::AntigravityIde => ".agents/rules/qcode.md",
    }
}

/// The word graphify's own installers know `harness` by: `graphify <word> install` writes its
/// section and hooks into the workspace, `graphify install --platform <word>` its skill into the
/// home directory.
#[must_use]
pub fn platform(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "claude",
        HarnessKind::OpenCode => "opencode",
        HarnessKind::GeminiCli => "gemini",
        HarnessKind::Codex => "codex",
        HarnessKind::AntigravityIde => "antigravity",
    }
}

/// Where graphify's skill for `harness` lands, relative to the home directory, as measured: the
/// image build checks that it is there.
#[must_use]
pub fn skill(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => ".claude/skills/graphify/SKILL.md",
        HarnessKind::OpenCode => ".config/opencode/skills/graphify/SKILL.md",
        HarnessKind::GeminiCli => ".gemini/skills/graphify/SKILL.md",
        HarnessKind::Codex => ".codex/skills/graphify/SKILL.md",
        HarnessKind::AntigravityIde => ".gemini/config/skills/graphify/SKILL.md",
    }
}

/// Why the instruction file of a harness could not be brought up to date. The tab starts all the
/// same; the person is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unguided {
    /// graphify could not set itself up in the workspace: what it, or the engine, said.
    Graphify(String),
    /// The file could not be read or written: the file, relative to the workspace, and the
    /// system's words.
    Unwritten(String, String),
    /// The file has only one of QCode's two marker lines, so where QCode's section starts or ends
    /// cannot be told from the person's text beside it. The file, relative to the workspace.
    Broken(String),
}

/// QCode's whole section as it stands in a file: [`HEADING`], then [`SECTION`] between the markers.
#[must_use]
pub fn block() -> String {
    format!("{HEADING}\n{BEGIN}\n{SECTION}{END}")
}

/// `existing` with QCode's section in it, for `harness`; `None` when it says so already and
/// nothing is to be written.
///
/// A file that is not there is made with the section alone (for Antigravity IDE, with the lines
/// that make it a rule loaded every time). A file without the section gets it at its end, after a
/// blank line. A file with it has only what lies between the markers replaced, where it stands;
/// and [`HEADING`] put back right before the opening line when it is not there, since without it
/// graphify's next install would take the opening line for part of its own section.
///
/// # Errors
///
/// [`Unguided::Broken`] when the file has one marker line without the other.
pub fn merge(existing: Option<&str>, harness: HarnessKind) -> Result<Option<String>, Unguided> {
    let inner = format!("{BEGIN}\n{SECTION}{END}");
    let Some(text) = existing else {
        let opening = if harness == HarnessKind::AntigravityIde { ANTIGRAVITY_RULE } else { "" };
        return Ok(Some(format!("{opening}{}\n", block())));
    };
    let broken = || Unguided::Broken(file(harness).to_owned());
    let merged = match text.find(BEGIN) {
        Some(start) => {
            let length = text[start..].find(END).ok_or_else(broken)?;
            let end = start + length + END.len();
            let before = &text[..start];
            let ours = if before.ends_with(&format!("{HEADING}\n")) { inner } else { block() };
            format!("{before}{ours}{}", &text[end..])
        }
        None if text.contains(END) => return Err(broken()),
        None => {
            let gap = if text.is_empty() || text.ends_with("\n\n") {
                ""
            } else if text.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            format!("{text}{gap}{}\n", block())
        }
    };
    Ok((merged != text).then_some(merged))
}

/// Brings QCode's section of `harness`'s instruction file in the workspace folder `code` up to
/// date, and says whether anything was written.
///
/// The new text is written beside the file and moved over it, with the old file's permissions, so
/// an agent reading at that moment finds the old file or the new one and never half of one. Runs
/// on the disk, so it belongs on a background thread.
///
/// # Errors
///
/// [`Unguided`] when the file cannot be read or written, or its section cannot be found.
pub fn write(code: &Path, harness: HarnessKind) -> Result<bool, Unguided> {
    let name = file(harness);
    let path = code.join(name);
    let unwritten = |error: std::io::Error| Unguided::Unwritten(name.to_owned(), error.to_string());
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(unwritten(error)),
    };
    let Some(text) = merge(existing.as_deref(), harness)? else { return Ok(false) };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(unwritten)?;
    }
    let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let beside = path.with_file_name(format!(".{file_name}.qcode-{}", std::process::id()));
    let written = std::fs::write(&beside, &text)
        .and_then(|()| match std::fs::metadata(&path) {
            Ok(old) => std::fs::set_permissions(&beside, old.permissions()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        })
        .and_then(|()| std::fs::rename(&beside, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&beside);
        return Err(unwritten(error));
    }
    Ok(true)
}

/// Held while one tab's graphify and QCode write a workspace's instruction files.
///
/// Two tabs coming up at once in one workspace would otherwise interleave: one reads the file,
/// the other's graphify writes its section, the first writes the file back without it. Every tab
/// of every workspace shares the one lock; what it guards takes well under a second.
pub fn lock() -> MutexGuard<'static, ()> {
    static WRITING: Mutex<()> = Mutex::new(());
    WRITING.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HARNESSES: [HarnessKind; 5] = [
        HarnessKind::ClaudeCode,
        HarnessKind::OpenCode,
        HarnessKind::GeminiCli,
        HarnessKind::Codex,
        HarnessKind::AntigravityIde,
    ];

    /// What graphify wrote into a `CLAUDE.md` in the measured container, word for word.
    const GRAPHIFY: &str = "## graphify\n\nThis project has a knowledge graph at graphify-out/ with god nodes, \
                            community structure, and cross-file relationships.\n\nRules:\n- After modifying \
                            code, run `graphify update .` to keep the graph current (AST-only, no API cost).\n";

    fn ours(text: &str) -> &str {
        let start = text.find(BEGIN).expect("the section is there");
        let end = text.find(END).expect("and ends") + END.len();
        &text[start..end]
    }

    #[test]
    fn each_harness_gets_the_file_it_reads() {
        assert_eq!(file(HarnessKind::ClaudeCode), "CLAUDE.md");
        assert_eq!(file(HarnessKind::OpenCode), "AGENTS.md");
        assert_eq!(file(HarnessKind::Codex), "AGENTS.md");
        assert_eq!(file(HarnessKind::GeminiCli), "GEMINI.md");
        assert_eq!(file(HarnessKind::AntigravityIde), ".agents/rules/qcode.md");
        // Beside graphify's own rule, never in place of it.
        assert_ne!(file(HarnessKind::AntigravityIde), ".agents/rules/graphify.md");
    }

    #[test]
    fn each_harness_is_known_to_graphify_by_its_own_word() {
        let words: Vec<&str> = HARNESSES.iter().map(|harness| platform(*harness)).collect();
        assert_eq!(words, ["claude", "opencode", "gemini", "codex", "antigravity"]);
    }

    #[test]
    fn a_missing_file_is_made_with_the_section_alone() {
        let made = merge(None, HarnessKind::ClaudeCode).expect("mergeable").expect("written");
        assert_eq!(made, format!("{HEADING}\n{BEGIN}\n{SECTION}{END}\n"));
        assert_eq!(made, format!("{}\n", block()));
    }

    #[test]
    fn antigravitys_file_is_made_as_a_rule_it_always_loads() {
        let made = merge(None, HarnessKind::AntigravityIde).expect("mergeable").expect("written");
        assert!(made.starts_with("---\ntrigger: always_on\n"), "{made}");
        assert!(made.ends_with(&format!("{}\n", block())), "{made}");
    }

    #[test]
    fn the_persons_text_and_graphifys_section_are_kept_byte_for_byte() {
        let before = format!("# My project\n\nKeep tabs, not spaces.\n\n{GRAPHIFY}");
        let after = merge(Some(&before), HarnessKind::ClaudeCode).expect("mergeable").expect("written");
        assert!(after.starts_with(&before), "{after}");
        assert_eq!(after, format!("{before}\n{}\n", block()));
    }

    #[test]
    fn a_file_without_a_final_line_break_is_not_run_into() {
        let after = merge(Some("# Mine"), HarnessKind::OpenCode).expect("mergeable").expect("written");
        assert!(after.starts_with(&format!("# Mine\n\n{HEADING}\n{BEGIN}\n")), "{after}");
    }

    #[test]
    fn an_older_section_is_replaced_where_it_stands() {
        let before = format!("# Mine\n\n{HEADING}\n{BEGIN}\nwhat an older QCode said\n{END}\n\n{GRAPHIFY}");
        let after = merge(Some(&before), HarnessKind::ClaudeCode).expect("mergeable").expect("written");
        assert_eq!(after, format!("# Mine\n\n{}\n\n{GRAPHIFY}", block()), "only between the markers");
        assert!(!after.contains("what an older QCode said"), "{after}");
        assert_eq!(ours(&after), format!("{BEGIN}\n{SECTION}{END}"));
    }

    #[test]
    fn a_section_that_lost_its_heading_gets_it_back_right_before_the_opening_line() {
        let before = format!("{GRAPHIFY}\n{BEGIN}\n{SECTION}{END}\n");
        let after = merge(Some(&before), HarnessKind::ClaudeCode).expect("mergeable").expect("written");
        assert_eq!(after, format!("{GRAPHIFY}\n{}\n", block()));
    }

    #[test]
    fn the_same_section_is_not_written_again() {
        let once = merge(Some(&format!("# Mine\n\n{GRAPHIFY}")), HarnessKind::ClaudeCode)
            .expect("mergeable")
            .expect("written");
        assert_eq!(merge(Some(&once), HarnessKind::ClaudeCode), Ok(None));
    }

    #[test]
    fn the_markers_are_never_doubled() {
        let mut text = merge(None, HarnessKind::GeminiCli).expect("mergeable").expect("written");
        text.push_str("\nThe person's line after it.\n");
        let again = merge(Some(&text), HarnessKind::GeminiCli).expect("mergeable");
        assert_eq!(again, None, "nothing to change");
        let older = text.replace(SECTION, "older\n");
        let merged = merge(Some(&older), HarnessKind::GeminiCli).expect("mergeable").expect("written");
        for line in [HEADING, BEGIN, END] {
            assert_eq!(merged.matches(line).count(), 1, "{line}: {merged}");
        }
        assert!(merged.ends_with("\nThe person's line after it.\n"), "{merged}");
    }

    #[test]
    fn one_marker_line_without_the_other_leaves_the_file_alone() {
        let opening = format!("# Mine\n\n{BEGIN}\nhalf of it\n\nThe person's text.\n");
        assert_eq!(merge(Some(&opening), HarnessKind::Codex), Err(Unguided::Broken("AGENTS.md".to_owned())));
        let closing = format!("# Mine\n\nhalf of it\n{END}\n\nThe person's text.\n");
        assert_eq!(merge(Some(&closing), HarnessKind::Codex), Err(Unguided::Broken("AGENTS.md".to_owned())));
    }

    /// What graphify 0.9.65 does to a file when it installs, as read from its own source
    /// (`_replace_or_append_section`) and seen in a container: no `## graphify` line, and its
    /// section is appended after a blank line; one, and the stretch from it to the next line
    /// starting with `## ` is written anew, with one blank line on either side.
    fn graphify_installs(text: &str) -> String {
        let lines: Vec<&str> = text.split('\n').collect();
        let section = GRAPHIFY.trim();
        let Some(start) = lines.iter().rposition(|line| line.trim() == "## graphify") else {
            return if text.trim().is_empty() {
                format!("{section}\n")
            } else {
                format!("{}\n\n{section}\n", text.trim_end())
            };
        };
        let end = (start + 1..lines.len()).find(|&j| lines[j].starts_with("## ")).unwrap_or(lines.len());
        let head = lines[..start].join("\n");
        let tail = lines[end..].join("\n");
        let parts: Vec<&str> =
            [head.trim_end(), section, tail.trim_start()].into_iter().filter(|part| !part.is_empty()).collect();
        let out = parts.join("\n\n");
        if out.ends_with('\n') { out } else { format!("{out}\n") }
    }

    #[test]
    fn graphify_installing_again_after_qcode_keeps_qcodes_section_whole() {
        // The order a tab comes up in, twice over: graphify, then QCode; graphify, then QCode.
        for start in [None, Some("# Firefly\n\nThe person's own line.\n")] {
            let first = graphify_installs(start.unwrap_or(""));
            let ours = merge(Some(&first), HarnessKind::ClaudeCode).expect("mergeable").expect("written");
            let second = graphify_installs(&ours);
            assert!(second.contains(&block()), "graphify kept QCode's section whole:\n{second}");
            assert_eq!(merge(Some(&second), HarnessKind::ClaudeCode), Ok(None), "and QCode has nothing to add");
            assert_eq!(second, ours, "nothing moved at all");
        }
    }

    #[test]
    fn the_section_says_nothing_graphify_says() {
        assert!(!SECTION.contains("graphify"), "graphify writes its own section");
        assert!(SECTION.lines().count() < 15, "short: {} lines", SECTION.lines().count());
    }

    #[test]
    fn the_section_names_the_bridges_tools_their_arguments_and_limits_as_the_code_has_them() {
        use crate::bridge::protocol::MOST_TEXT;
        use crate::bridge::rules::{MOST_HOPS, MOST_PER_WINDOW, RATE_WINDOW};
        for word in ["`list_tabs`", "`send_message`", "`tab`", "`text`", "`qcode`", "`/work`"] {
            assert!(SECTION.contains(word), "{word}");
        }
        assert!(SECTION.contains(&format!("longer than {MOST_TEXT} characters")), "{SECTION}");
        assert!(SECTION.contains(&format!("after {MOST_HOPS} messages")), "{SECTION}");
        assert_eq!(RATE_WINDOW.as_secs(), 60, "the section says a minute");
        assert!(SECTION.contains(&format!("at most {MOST_PER_WINDOW} messages a minute")), "{SECTION}");
        assert!(SECTION.contains(&format!("MCP server `{}`", crate::bridge::SERVER_NAME)), "{SECTION}");
    }

    /// The line a delivered message starts with, as QCode types it in English, is the one the
    /// section shows the agent.
    #[test]
    fn the_section_shows_the_line_a_delivered_message_starts_with() {
        let english = include_str!("../../assets/locales/en.toml");
        let handed = english
            .lines()
            .find_map(|line| line.strip_prefix("handed = \""))
            .expect("the English line a delivered message starts with");
        let first = handed.split("\\n").next().expect("its first line");
        let shown = first.replace("{from}", "<coding tool> · <tab title>");
        assert!(SECTION.contains(&shown), "{shown}");
    }

    #[test]
    fn writing_makes_keeps_and_leaves_the_file_on_disk() {
        let folder = std::env::temp_dir().join(format!("qcode-guidance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("a folder");
        assert_eq!(write(&folder, HarnessKind::AntigravityIde), Ok(true));
        let rule = std::fs::read_to_string(folder.join(".agents/rules/qcode.md")).expect("made");
        assert!(rule.contains(SECTION));
        std::fs::write(folder.join("CLAUDE.md"), "# Mine\n").expect("the person's file");
        assert_eq!(write(&folder, HarnessKind::ClaudeCode), Ok(true));
        let modified = std::fs::metadata(folder.join("CLAUDE.md")).and_then(|meta| meta.modified()).expect("a time");
        assert_eq!(write(&folder, HarnessKind::ClaudeCode), Ok(false), "the same content is not written");
        let again = std::fs::metadata(folder.join("CLAUDE.md")).and_then(|meta| meta.modified()).expect("a time");
        assert_eq!(modified, again);
        let text = std::fs::read_to_string(folder.join("CLAUDE.md")).expect("read");
        assert!(text.starts_with("# Mine\n\n"), "{text}");
        let leftovers: Vec<_> = std::fs::read_dir(&folder)
            .expect("listed")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".qcode-"))
            .collect();
        assert!(leftovers.is_empty(), "nothing is left beside the file");
        let _ = std::fs::remove_dir_all(&folder);
    }
}
