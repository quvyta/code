//! What goes over the project's socket: one line of JSON from the server in a container, one
//! line of JSON back from QCode, and the connection closes.
//!
//! A question:
//!
//! ```text
//! {"token":"…","op":"list"}
//! {"token":"…","op":"send","tab":"3","text":"Please run the tests."}
//! ```
//!
//! An answer carries the words the agent is shown, whether it was done, and for a list the tabs:
//!
//! ```text
//! {"ok":true,"text":"…","tabs":[{"tab":"3","title":"codex-main","harness":"Codex","profile":"codex-main","network":true}]}
//! {"ok":false,"text":"…"}
//! ```
//!
//! Everything a question holds came out of a container and is read as untrusted: a line that is
//! too long, not JSON, or not one of the two shapes is [`Malformed`], and nothing in it is
//! acted on.

use serde_json::{Map, Value, json};

/// The longest question read, in bytes. A message longer than [`MOST_TEXT`] is refused anyway;
/// the room above that is for the JSON around it and for escapes.
pub const MOST_LINE: usize = 256 * 1024;

/// The most characters a message may have. A message is something one agent hands another to
/// act on, a task or an answer; a whole file does not belong in one, the project folder is where
/// both agents read files.
pub const MOST_TEXT: usize = 16_000;

/// A question from a tab's server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The token of the tab asking, as its server found it.
    pub token: String,
    /// What it asks.
    pub request: Request,
}

/// What a tab's server can ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The other agent tabs of the project.
    List,
    /// Hand `text` to the tab named `tab`.
    Send {
        /// The tab, by the id [`Request::List`] gave or by its title.
        tab: String,
        /// The message.
        text: String,
    },
}

/// A line that is not a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

/// Reads one question.
///
/// # Errors
///
/// [`Malformed`] when the line is not JSON, not an object, lacks the token or the operation,
/// names an operation there is not, or a send lacks its tab or its text.
pub fn parse(line: &str) -> Result<Question, Malformed> {
    if line.len() > MOST_LINE {
        return Err(Malformed);
    }
    let value: Value = serde_json::from_str(line).map_err(|_| Malformed)?;
    let object = value.as_object().ok_or(Malformed)?;
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned).ok_or(Malformed);
    let token = text("token")?;
    let request = match object.get("op").and_then(Value::as_str) {
        Some("list") => Request::List,
        Some("send") => Request::Send { tab: text("tab")?, text: text("text")? },
        _ => return Err(Malformed),
    };
    Ok(Question { token, request })
}

/// One tab a message can be sent to, as a list answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The id to send to.
    pub tab: String,
    /// The title the tab strip shows.
    pub title: String,
    /// The harness, as it names itself.
    pub harness: String,
    /// The profile the tab runs.
    pub profile: String,
    /// Whether the tab's container reaches the network.
    pub network: bool,
}

/// QCode's answer to a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Whether what was asked was done: the list given, the message taken. A refusal is `false`.
    pub ok: bool,
    /// The words the agent is shown.
    pub text: String,
    /// The tabs, for an answer to [`Request::List`].
    pub tabs: Option<Vec<Listed>>,
}

impl Answer {
    /// A question answered: `text` says what was done.
    #[must_use]
    pub fn done(text: String) -> Self {
        Self { ok: true, text, tabs: None }
    }

    /// A question refused: `text` says why.
    #[must_use]
    pub fn refused(text: String) -> Self {
        Self { ok: false, text, tabs: None }
    }

    /// The list of `tabs`, told in `text` too.
    #[must_use]
    pub fn listed(text: String, tabs: Vec<Listed>) -> Self {
        Self { ok: true, text, tabs: Some(tabs) }
    }

    /// The answer as the line written back: JSON on one line, ending in a newline. JSON never
    /// needs a raw line break, so text with line breaks still makes one line.
    #[must_use]
    pub fn line(&self) -> String {
        let mut object = Map::new();
        object.insert("ok".to_owned(), Value::Bool(self.ok));
        object.insert("text".to_owned(), Value::String(self.text.clone()));
        if let Some(tabs) = &self.tabs {
            let tabs = tabs
                .iter()
                .map(|tab| {
                    json!({
                        "tab": tab.tab,
                        "title": tab.title,
                        "harness": tab.harness,
                        "profile": tab.profile,
                        "network": tab.network,
                    })
                })
                .collect();
            object.insert("tabs".to_owned(), Value::Array(tabs));
        }
        let mut line = Value::Object(object).to_string();
        line.push('\n');
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_questions() {
        assert_eq!(
            parse(r#"{"token":"abc","op":"list"}"#),
            Ok(Question { token: "abc".to_owned(), request: Request::List })
        );
        assert_eq!(
            parse(r#"{"op":"send","token":"abc","tab":"3","text":"run the tests\nplease"}"#),
            Ok(Question {
                token: "abc".to_owned(),
                request: Request::Send { tab: "3".to_owned(), text: "run the tests\nplease".to_owned() }
            })
        );
    }

    #[test]
    fn anything_else_is_malformed_and_never_half_read() {
        for line in [
            "",
            "not json",
            "[]",
            r#""list""#,
            r#"{"op":"list"}"#,
            r#"{"token":7,"op":"list"}"#,
            r#"{"token":"abc"}"#,
            r#"{"token":"abc","op":"delete"}"#,
            r#"{"token":"abc","op":"send","text":"hi"}"#,
            r#"{"token":"abc","op":"send","tab":"3"}"#,
            r#"{"token":"abc","op":"send","tab":3,"text":"hi"}"#,
        ] {
            assert_eq!(parse(line), Err(Malformed), "{line}");
        }
    }

    #[test]
    fn a_line_longer_than_the_limit_is_not_even_parsed() {
        let text = "x".repeat(MOST_LINE);
        assert_eq!(parse(&format!(r#"{{"token":"a","op":"send","tab":"1","text":"{text}"}}"#)), Err(Malformed));
    }

    #[test]
    fn an_answer_is_one_line_of_json_whatever_its_text_holds() {
        let answer = Answer::refused("two\nlines and a \"quote\"".to_owned());
        let line = answer.line();
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "{line}");
        let back: Value = serde_json::from_str(line.trim_end()).expect("the answer is JSON");
        assert_eq!(back["ok"], Value::Bool(false));
        assert_eq!(back["text"], Value::String("two\nlines and a \"quote\"".to_owned()));
        assert!(back.get("tabs").is_none());
    }

    #[test]
    fn a_list_carries_every_tab_with_its_details() {
        let tab = Listed {
            tab: "3".to_owned(),
            title: "codex-main".to_owned(),
            harness: "Codex".to_owned(),
            profile: "codex-main".to_owned(),
            network: false,
        };
        let line = Answer::listed("one tab".to_owned(), vec![tab]).line();
        let back: Value = serde_json::from_str(line.trim_end()).expect("the answer is JSON");
        assert_eq!(back["ok"], Value::Bool(true));
        assert_eq!(back["tabs"][0]["tab"], "3");
        assert_eq!(back["tabs"][0]["harness"], "Codex");
        assert_eq!(back["tabs"][0]["network"], Value::Bool(false));
    }
}
