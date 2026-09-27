//! What goes over the workspace's socket: one line of JSON from the server in a container, one
//! line of JSON back from QCode, and the connection closes.
//!
//! A question:
//!
//! ```text
//! {"token":"…","op":"list"}
//! {"token":"…","op":"send","tab":"3","text":"Please run the tests.","kind":"report"}
//! {"token":"…","op":"send","tab":"all","text":"I am taking the parser; leave it to me."}
//! {"token":"…","op":"inbox"}
//! {"token":"…","op":"peek"}
//! {"token":"…","session":"ses_…","op":"list"}
//! ```
//!
//! A question may carry the conversation it was asked from in `session`. opencode's shared server
//! speaks for every tab of its profile with one token, and each of those tabs shows a
//! conversation of its own, so the two together name the tab (see
//! [`crate::ui::workspace`]'s shared server).
//!
//! A message has a [`Kind`], `info` when the question names none, so a server written before
//! kinds existed is still understood. `"tab":"all"` hands it to every other agent tab of the
//! workspace.
//!
//! An answer carries the words the agent is shown, whether it was done, for a list the asking tab
//! itself and the others, and for a message to every tab what happened at each:
//!
//! ```text
//! {"ok":true,"text":"…","you":{"tab":"2","title":"reviewer","harness":"Claude Code","profile":"claude-sub","workspace":"Firefly"},"tabs":[{"tab":"3","title":"codex-main","harness":"Codex","profile":"codex-main","network":true,"inbox":false,"waiting":0,"trouble":null}]}
//! {"ok":true,"text":"…","messages":[{"from":"Codex · codex-main","tab":"3","kind":"question","text":"Please run the tests."}]}
//! {"ok":true,"text":"…","sent":[{"tab":"3","title":"codex-main","ok":true,"text":"Delivered. …"}]}
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
/// act on, a task or an answer; a whole file does not belong in one, the workspace folder is where
/// both agents read files.
pub const MOST_TEXT: usize = 16_000;

/// A question from a tab's server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The token of the tab asking, as its server found it.
    pub token: String,
    /// The conversation the question was asked from, when the server asking speaks for more than
    /// one tab and tells them apart by it.
    pub session: Option<String>,
    /// What it asks.
    pub request: Request,
}

/// What a tab's server can ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The other agent tabs of the workspace.
    List,
    /// Hand `text` to the tab named `to`, or to every other agent tab.
    Send {
        /// Where it goes.
        to: To,
        /// The message.
        text: String,
        /// What the receiving agent is asked to do with it.
        kind: Kind,
    },
    /// Every message waiting for the asking tab, taken out of it: how the agent of a tab that has
    /// no prompt to type into (a window) receives what was sent to it.
    Inbox,
    /// The messages waiting for the asking tab, left where they are: how a window's own watcher
    /// learns that there is something for its agent to take, without taking it.
    Peek,
}

/// Where a message goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum To {
    /// One tab, by the id [`Request::List`] gave or by its title.
    Tab(String),
    /// Every other agent tab of the workspace, each by the same rules as a message to it alone.
    All,
}

/// The word that sends a message to every other agent tab rather than to one.
pub const ALL: &str = "all";

/// What the sender asks of the agent that receives a message. It is said in the header the
/// message arrives under, with how to answer, so that an agent knows whether anyone waits on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// For the receiver's information; nobody waits for an answer. What a message is when its
    /// sender does not say.
    #[default]
    Info,
    /// A question: the sender waits for the answer.
    Question,
    /// A task: the sender waits for a report of the result once the receiver has finished it.
    Report,
}

impl Kind {
    /// The word the protocol and the tools know the kind by.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Question => "question",
            Self::Report => "report",
        }
    }

    /// The kind `word` names, when it names one.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [Self::Info, Self::Question, Self::Report].into_iter().find(|kind| kind.word() == word)
    }
}

/// A line that is not a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

/// Reads one question.
///
/// # Errors
///
/// [`Malformed`] when the line is not JSON, not an object, lacks the token or the operation,
/// names an operation there is not, or a send lacks its tab or its text or names a kind there is
/// not.
pub fn parse(line: &str) -> Result<Question, Malformed> {
    if line.len() > MOST_LINE {
        return Err(Malformed);
    }
    let value: Value = serde_json::from_str(line).map_err(|_| Malformed)?;
    let object = value.as_object().ok_or(Malformed)?;
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned).ok_or(Malformed);
    let token = text("token")?;
    let session = match object.get("session") {
        None => None,
        Some(Value::String(session)) => Some(session.clone()),
        Some(_) => return Err(Malformed),
    };
    let request = match object.get("op").and_then(Value::as_str) {
        Some("list") => Request::List,
        Some("send") => {
            let kind = match object.get("kind") {
                None => Kind::Info,
                Some(Value::String(word)) => Kind::parse(word).ok_or(Malformed)?,
                Some(_) => return Err(Malformed),
            };
            let tab = text("tab")?;
            let to = if tab.trim() == ALL { To::All } else { To::Tab(tab) };
            Request::Send { to, text: text("text")?, kind }
        }
        Some("inbox") => Request::Inbox,
        Some("peek") => Request::Peek,
        _ => return Err(Malformed),
    };
    Ok(Question { token, session, request })
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
    /// Whether the tab takes messages only when its agent checks its inbox, because it has no
    /// prompt they could be typed into: a window. A sender reads it to know that a message it
    /// leaves there is not in front of that agent until then.
    pub inbox: bool,
    /// How many messages are still waiting to be typed into the tab's harness. A sending agent
    /// reads it to see that what it sent has not arrived yet.
    pub waiting: usize,
    /// Why the waiting messages have not been typed in, when something is stopping them, in the
    /// words the agent is shown. `None` while nothing is.
    pub trouble: Option<String>,
}

/// The tab that asked, as a list answers it: an agent learns which tab it is itself only from
/// this, never from what another agent tells it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct You {
    /// Its id, the one the other tabs send to.
    pub tab: String,
    /// The title the tab strip shows.
    pub title: String,
    /// The harness, as it names itself.
    pub harness: String,
    /// The profile the tab runs.
    pub profile: String,
    /// The name of the workspace the tab is in.
    pub workspace: String,
}

/// A message taken out of a tab's inbox, as the inbox answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// Who sent it: the harness and the title of the sending tab.
    pub from: String,
    /// The id of the sending tab, the one to answer to.
    pub tab: String,
    /// What the sender asks of the receiver.
    pub kind: Kind,
    /// The message.
    pub text: String,
}

/// What happened to a message sent to every tab, at one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The tab's id.
    pub tab: String,
    /// The title the tab strip shows.
    pub title: String,
    /// Whether it took the message: handed over, or waiting to be.
    pub ok: bool,
    /// What happened there, in the words a message to that tab alone would have been answered in.
    pub text: String,
}

/// QCode's answer to a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Whether what was asked was done: the list given, the message taken. A refusal is `false`.
    pub ok: bool,
    /// The words the agent is shown.
    pub text: String,
    /// The asking tab itself, for an answer to [`Request::List`].
    pub you: Option<You>,
    /// The tabs, for an answer to [`Request::List`].
    pub tabs: Option<Vec<Listed>>,
    /// The messages, for an answer to [`Request::Inbox`].
    pub messages: Option<Vec<Received>>,
    /// What happened at each tab, for a message sent to every tab.
    pub sent: Option<Vec<Outcome>>,
}

impl Answer {
    /// A question answered: `text` says what was done.
    #[must_use]
    pub fn done(text: String) -> Self {
        Self { ok: true, text, you: None, tabs: None, messages: None, sent: None }
    }

    /// A question refused: `text` says why.
    #[must_use]
    pub fn refused(text: String) -> Self {
        Self { ok: false, text, you: None, tabs: None, messages: None, sent: None }
    }

    /// The list of `tabs` other than `you`, the tab that asked, told in `text` too.
    #[must_use]
    pub fn listed(text: String, you: You, tabs: Vec<Listed>) -> Self {
        Self { ok: true, text, you: Some(you), tabs: Some(tabs), messages: None, sent: None }
    }

    /// The `messages` taken out of the asking tab's inbox, told in `text` too.
    #[must_use]
    pub fn received(text: String, messages: Vec<Received>) -> Self {
        Self { ok: true, text, you: None, tabs: None, messages: Some(messages), sent: None }
    }

    /// What happened at each tab a message to every tab went to, told in `text` too; `ok` when
    /// at least one of them took it.
    #[must_use]
    pub fn broadcast(text: String, sent: Vec<Outcome>) -> Self {
        let ok = sent.iter().any(|outcome| outcome.ok);
        Self { ok, text, you: None, tabs: None, messages: None, sent: Some(sent) }
    }

    /// The answer as the line written back: JSON on one line, ending in a newline. JSON never
    /// needs a raw line break, so text with line breaks still makes one line.
    #[must_use]
    pub fn line(&self) -> String {
        let mut object = Map::new();
        object.insert("ok".to_owned(), Value::Bool(self.ok));
        object.insert("text".to_owned(), Value::String(self.text.clone()));
        if let Some(you) = &self.you {
            let you = json!({
                "tab": you.tab,
                "title": you.title,
                "harness": you.harness,
                "profile": you.profile,
                "workspace": you.workspace,
            });
            object.insert("you".to_owned(), you);
        }
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
                        "inbox": tab.inbox,
                        "waiting": tab.waiting,
                        "trouble": tab.trouble,
                    })
                })
                .collect();
            object.insert("tabs".to_owned(), Value::Array(tabs));
        }
        if let Some(messages) = &self.messages {
            let messages = messages
                .iter()
                .map(|message| {
                    json!({ "from": message.from, "tab": message.tab, "kind": message.kind.word(), "text": message.text })
                })
                .collect();
            object.insert("messages".to_owned(), Value::Array(messages));
        }
        if let Some(sent) = &self.sent {
            let sent = sent
                .iter()
                .map(|outcome| json!({ "tab": outcome.tab, "title": outcome.title, "ok": outcome.ok, "text": outcome.text }))
                .collect();
            object.insert("sent".to_owned(), Value::Array(sent));
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
    fn a_question_may_name_the_conversation_it_was_asked_from() {
        assert_eq!(
            parse(r#"{"token":"srv","session":"ses_f1ce","op":"list"}"#),
            Ok(Question { token: "srv".to_owned(), session: Some("ses_f1ce".to_owned()), request: Request::List })
        );
        assert_eq!(parse(r#"{"token":"srv","session":7,"op":"list"}"#), Err(Malformed));
    }

    #[test]
    fn reads_both_questions() {
        assert_eq!(
            parse(r#"{"token":"abc","op":"list"}"#),
            Ok(Question { token: "abc".to_owned(), session: None, request: Request::List })
        );
        assert_eq!(
            parse(r#"{"op":"send","token":"abc","tab":"3","text":"run the tests\nplease"}"#),
            Ok(Question {
                session: None,
                token: "abc".to_owned(),
                request: Request::Send {
                    to: To::Tab("3".to_owned()),
                    text: "run the tests\nplease".to_owned(),
                    kind: Kind::Info
                }
            })
        );
        assert_eq!(
            parse(r#"{"token":"abc","op":"inbox"}"#),
            Ok(Question { token: "abc".to_owned(), session: None, request: Request::Inbox })
        );
        assert_eq!(
            parse(r#"{"token":"abc","op":"peek"}"#),
            Ok(Question { token: "abc".to_owned(), session: None, request: Request::Peek })
        );
    }

    fn send(line: &str) -> Result<(To, Kind), Malformed> {
        match parse(line)?.request {
            Request::Send { to, kind, .. } => Ok((to, kind)),
            other => panic!("not a send: {other:?}"),
        }
    }

    #[test]
    fn a_message_says_its_kind_and_one_that_names_none_is_for_information() {
        let tab = || To::Tab("4".to_owned());
        assert_eq!(send(r#"{"token":"a","op":"send","tab":"4","text":"hi"}"#), Ok((tab(), Kind::Info)));
        assert_eq!(send(r#"{"token":"a","op":"send","tab":"4","text":"hi","kind":"info"}"#), Ok((tab(), Kind::Info)));
        assert_eq!(
            send(r#"{"token":"a","op":"send","tab":"4","text":"hi","kind":"question"}"#),
            Ok((tab(), Kind::Question))
        );
        assert_eq!(
            send(r#"{"token":"a","op":"send","tab":"4","text":"hi","kind":"report"}"#),
            Ok((tab(), Kind::Report))
        );
        for kind in [r#""urgent""#, r#""Question""#, r#""""#, "7", "null", r#"["info"]"#] {
            let line = format!(r#"{{"token":"a","op":"send","tab":"4","text":"hi","kind":{kind}}}"#);
            assert_eq!(parse(&line), Err(Malformed), "{line}");
        }
    }

    #[test]
    fn all_sends_to_every_tab_and_any_other_word_names_one() {
        assert_eq!(
            send(r#"{"token":"a","op":"send","tab":"all","text":"hi","kind":"question"}"#),
            Ok((To::All, Kind::Question))
        );
        assert_eq!(send(r#"{"token":"a","op":"send","tab":" all ","text":"hi"}"#), Ok((To::All, Kind::Info)));
        assert_eq!(
            send(r#"{"token":"a","op":"send","tab":"allison","text":"hi"}"#),
            Ok((To::Tab("allison".to_owned()), Kind::Info))
        );
    }

    #[test]
    fn an_inbox_carries_each_message_with_its_sender_and_kind() {
        let messages = vec![
            Received {
                from: "Codex · codex-main".to_owned(),
                tab: "3".to_owned(),
                kind: Kind::Report,
                text: "run the tests\nplease".to_owned(),
            },
            Received {
                from: "opencode · oc".to_owned(),
                tab: "5".to_owned(),
                kind: Kind::Info,
                text: "done".to_owned(),
            },
        ];
        let line = Answer::received("two messages".to_owned(), messages).line();
        let back: Value = serde_json::from_str(line.trim_end()).expect("the answer is JSON");
        assert_eq!(back["messages"][0]["from"], "Codex · codex-main");
        assert_eq!(back["messages"][0]["tab"], "3");
        assert_eq!(back["messages"][0]["kind"], "report");
        assert_eq!(back["messages"][0]["text"], "run the tests\nplease");
        assert_eq!(back["messages"][1]["from"], "opencode · oc");
        assert_eq!(back["messages"][1]["kind"], "info");
        assert!(back.get("tabs").is_none());
    }

    #[test]
    fn a_message_to_every_tab_is_answered_tab_by_tab() {
        let sent = vec![
            Outcome { tab: "2".to_owned(), title: "a".to_owned(), ok: false, text: "refused".to_owned() },
            Outcome { tab: "4".to_owned(), title: "b".to_owned(), ok: true, text: "delivered".to_owned() },
        ];
        let answer = Answer::broadcast("two tabs".to_owned(), sent);
        assert!(answer.ok, "one tab took it");
        let back: Value = serde_json::from_str(answer.line().trim_end()).expect("the answer is JSON");
        assert_eq!(back["sent"][0]["tab"], "2");
        assert_eq!(back["sent"][0]["ok"], Value::Bool(false));
        assert_eq!(back["sent"][1]["text"], "delivered");
        let none = vec![Outcome { tab: "2".to_owned(), title: "a".to_owned(), ok: false, text: "refused".to_owned() }];
        assert!(!Answer::broadcast("one tab".to_owned(), none).ok, "nobody took it");
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

    fn you() -> You {
        You {
            tab: "2".to_owned(),
            title: "reviewer".to_owned(),
            harness: "Claude Code".to_owned(),
            profile: "claude-sub".to_owned(),
            workspace: "Firefly".to_owned(),
        }
    }

    #[test]
    fn a_list_carries_every_tab_with_its_details() {
        let tab = Listed {
            tab: "3".to_owned(),
            title: "codex-main".to_owned(),
            harness: "Codex".to_owned(),
            profile: "codex-main".to_owned(),
            network: false,
            inbox: true,
            waiting: 2,
            trouble: Some("the harness in that tab is not running".to_owned()),
        };
        let line = Answer::listed("one tab".to_owned(), you(), vec![tab]).line();
        let back: Value = serde_json::from_str(line.trim_end()).expect("the answer is JSON");
        assert_eq!(back["ok"], Value::Bool(true));
        assert_eq!(back["you"]["tab"], "2");
        assert_eq!(back["you"]["title"], "reviewer");
        assert_eq!(back["you"]["harness"], "Claude Code");
        assert_eq!(back["you"]["profile"], "claude-sub");
        assert_eq!(back["you"]["workspace"], "Firefly");
        assert_eq!(back["tabs"][0]["tab"], "3");
        assert_eq!(back["tabs"][0]["harness"], "Codex");
        assert_eq!(back["tabs"][0]["network"], Value::Bool(false));
        assert_eq!(back["tabs"][0]["waiting"], 2);
        assert_eq!(back["tabs"][0]["inbox"], Value::Bool(true));
        assert_eq!(back["tabs"][0]["trouble"], "the harness in that tab is not running");
    }

    #[test]
    fn a_tab_with_nothing_waiting_says_so_rather_than_leaving_it_out() {
        let tab = Listed {
            tab: "3".to_owned(),
            title: "codex-main".to_owned(),
            harness: "Codex".to_owned(),
            profile: "codex-main".to_owned(),
            network: true,
            inbox: false,
            waiting: 0,
            trouble: None,
        };
        let line = Answer::listed("one tab".to_owned(), you(), vec![tab]).line();
        let back: Value = serde_json::from_str(line.trim_end()).expect("the answer is JSON");
        assert_eq!(back["tabs"][0]["waiting"], 0);
        assert_eq!(back["tabs"][0]["trouble"], Value::Null);
    }
}
