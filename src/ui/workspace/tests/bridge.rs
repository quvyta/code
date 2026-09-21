//! The bridge between tabs on the screen: which tab asks, the person's approval, the rules that
//! refuse a message and say why, and the messages that wait in the tab they were sent to.
//!
//! No socket is opened here: a harness would wait on it forever. The calls a socket would carry
//! are handed to the screen by the test application below, the way the screen's own listener
//! hands them over, and their answers are read from the other end of each call.
//!
//! Delivery is driven against a real program on a real pseudo-terminal, started by the test
//! application the way a container's harness is: what reaches its prompt is read back from the
//! file it writes everything into. The moment a delivery is tried at is given rather than waited
//! for, so a test says exactly which of the two silences it is about.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use qframe::widgets::TerminalSession;

use super::*;

use crate::bridge::protocol::{Answer, Malformed, Question, Request};
use crate::bridge::rules::{MOST_HOPS, MOST_PER_WINDOW};
use crate::bridge::socket::Call;
use crate::ui::workspace::bridge::{ASK_WAIT, QUIET, RETRY_EVERY};
use crate::ui::workspace::{Letter, Undelivered};

/// The workspace screen, with one more way in: a call as the workspace's socket would bring it.
struct Bridged(WorkspaceScreen);

#[derive(Debug, Clone)]
enum Test {
    Screen(Msg),
    Call(Call),
    /// Gives the tab at this place a running program to be typed into, as a container that came
    /// up gives a tab its session.
    Attach(usize, TerminalSession),
    /// Looks at the tabs holding messages as of this moment.
    Deliver(Instant),
}

impl App for Bridged {
    type Msg = Test;

    fn update(&mut self, message: Test) -> Command<Test> {
        match message {
            Test::Screen(message) => super::super::update(&mut self.0, message).map(Test::Screen),
            Test::Call(call) => {
                super::super::bridge::answer(&mut self.0, "firefly", call, Instant::now()).map(Test::Screen)
            }
            Test::Attach(index, session) => {
                let active = self.0.active;
                if let Some(workspace) = self.0.workspaces.get_mut(active) {
                    workspace.tabs[index].attached(session);
                }
                Command::none()
            }
            Test::Deliver(now) => super::super::bridge::deliver(&mut self.0, now).map(Test::Screen),
        }
    }

    fn view(&self, ui: &mut View<'_, Test>) {
        super::super::view(&self.0, ui, Test::Screen, |_| {}, |_| {}, |_| {});
    }
}

/// A workspace with a Claude Code tab and a Codex tab, the first with the network as `claude`
/// says and the second with it as `codex` says, and a shell tab between them.
fn two_agents(scratch: &Scratch, claude: NetworkMode, codex: NetworkMode) -> Harness<Bridged> {
    let profiles = vec![
        Profile { network: claude, ..profile("claude-sub", HarnessKind::ClaudeCode) },
        Profile { network: codex, ..profile("codex-main", HarnessKind::Codex) },
    ];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    open(&mut screen, Choice::Shell);
    open(&mut screen, Choice::NewChat("codex-main".to_owned()));
    let mut harness = Harness::with_env(Bridged(screen), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    // The panel is closed so that a dialog's lines read as sentences, with nothing beside them.
    harness.send(Test::Screen(Msg::TogglePanel(false))).send(Test::Screen(Msg::OpenTab(0)));
    harness.advance(Duration::from_secs(1)).render();
    harness
}

/// The screen as one line of words, so a sentence a dialog wrapped reads whole.
fn said(harness: &Harness<Bridged>) -> String {
    harness.screen().replace('▌', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tab(harness: &Harness<Bridged>, index: usize) -> &Tab {
    &harness.app().0.workspace().expect("a workspace is open").tabs()[index]
}

/// Sends `request` from the tab at `index`, and answers the other end of the call.
fn ask(harness: &mut Harness<Bridged>, index: usize, request: Request) -> Receiver<Answer> {
    let token = tab(harness, index).token().to_owned();
    let (call, answers) = Call::new(Ok(Question { token, request }));
    harness.send(Test::Call(call));
    harness.render();
    answers
}

/// A message from the Claude Code tab to the Codex tab.
fn to_codex(harness: &mut Harness<Bridged>, text: &str) -> Receiver<Answer> {
    let codex = tab(harness, 2).key().0.to_string();
    ask(harness, 0, Request::Send { tab: codex, text: text.to_owned() })
}

/// A message from the Codex tab to the Claude Code tab.
fn to_claude(harness: &mut Harness<Bridged>, text: &str) -> Receiver<Answer> {
    let claude = tab(harness, 0).key().0.to_string();
    ask(harness, 2, Request::Send { tab: claude, text: text.to_owned() })
}

/// A workspace with a Claude Code tab and, beside it, the window of a desktop profile.
fn an_agent_and_a_window(scratch: &Scratch) -> Harness<Bridged> {
    let profiles = vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("anti", HarnessKind::AntigravityIde)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    open(&mut screen, Choice::Window("anti".to_owned()));
    let mut harness = Harness::with_env(Bridged(screen), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Test::Screen(Msg::TogglePanel(false))).send(Test::Screen(Msg::OpenTab(0)));
    harness.advance(Duration::from_secs(1)).render();
    harness
}

fn answered(answers: &Receiver<Answer>) -> Answer {
    answers.try_recv().expect("the call was answered")
}

const ONLINE: NetworkMode = NetworkMode::Full;
const OFFLINE: NetworkMode = NetworkMode::None;

#[test]
fn a_harness_tab_is_started_with_its_token_and_a_shell_tab_without_one() {
    let scratch = Scratch::new("bridge-token");
    let harness = two_agents(&scratch, ONLINE, ONLINE);
    let screen = &harness.app().0;
    let words = |index: usize| -> Vec<String> {
        let key = tab(&harness, index).key();
        screen
            .launch_command(key)
            .expect("a command")
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    };
    let claude = words(0);
    assert_eq!(claude[..3], ["exec", "--interactive", "--tty"]);
    assert_eq!(claude[3], "--env");
    assert_eq!(claude[4], format!("QCODE_BRIDGE={}", tab(&harness, 0).token()));
    assert_ne!(tab(&harness, 0).token(), tab(&harness, 2).token(), "every tab has its own");
    assert!(!words(1).iter().any(|word| word.starts_with("QCODE_BRIDGE")), "a shell has no agent: {:?}", words(1));
}

#[test]
fn an_agent_lists_the_other_agent_tabs_and_never_a_shell() {
    let scratch = Scratch::new("bridge-list");
    let mut harness = two_agents(&scratch, ONLINE, OFFLINE);
    let answer = answered(&ask(&mut harness, 0, Request::List));
    assert!(answer.ok, "{answer:?}");
    let tabs = answer.tabs.expect("a list");
    assert_eq!(tabs.len(), 1, "only the Codex tab: {tabs:?}");
    assert_eq!(tabs[0].tab, tab(&harness, 2).key().0.to_string());
    assert_eq!((tabs[0].title.as_str(), tabs[0].harness.as_str(), tabs[0].network), ("codex-main", "Codex", false));
    assert!(answer.text.contains("codex-main, Codex, without the network"), "{}", answer.text);
}

#[test]
fn a_question_from_no_tab_of_the_workspace_or_not_a_question_at_all_is_refused() {
    let scratch = Scratch::new("bridge-stranger");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let (call, answers) = Call::new(Ok(Question { token: "guessed".to_owned(), request: Request::List }));
    harness.send(Test::Call(call));
    let answer = answered(&answers);
    assert!(!answer.ok && answer.text.contains("does not know which tab"), "{answer:?}");

    let (call, answers) = Call::new(Err(Malformed));
    harness.send(Test::Call(call));
    assert!(!answered(&answers).ok);

    // The shell's token is no agent's.
    let answer = answered(&ask(&mut harness, 1, Request::List));
    assert!(!answer.ok, "{answer:?}");
}

#[test]
fn the_first_message_between_two_tabs_asks_the_person_and_is_taken_when_allowed() {
    let scratch = Scratch::new("bridge-allow");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let answers = to_codex(&mut harness, "Please write the test for parse().");
    let screen = said(&harness);
    assert!(screen.contains("Let Claude Code · claude-sub send messages to Codex · codex-main?"), "{screen}");
    assert!(screen.contains("Please write the test for parse()."), "the first message is shown:\n{screen}");
    assert!(screen.contains("Allow") && screen.contains("Deny"), "{screen}");
    assert!(answers.try_recv().is_err(), "the agent waits for the person");

    harness.click_text("Allow").render();
    let answer = answered(&answers);
    // No container came up in this test, so there is nothing to write into and the sender is
    // told so rather than being left to believe the other agent has the work.
    assert!(answer.ok && answer.text.starts_with("Taken, but not handed over yet"), "{answer:?}");
    assert!(answer.text.contains("is not running") && answer.text.contains("list_tabs"), "{answer:?}");
    let letter =
        Letter { from: "Claude Code · claude-sub".to_owned(), text: "Please write the test for parse().".to_owned() };
    assert_eq!(tab(&harness, 2).letters(), [letter]);

    // The answer holds for the pair: the next message goes without asking.
    let answer = answered(&to_codex(&mut harness, "And run it."));
    assert!(answer.ok, "{answer:?}");
    assert!(!said(&harness).contains("send messages to"), "asked once:\n{}", harness.screen());
    assert_eq!(tab(&harness, 2).letters().len(), 2);

    // Only in that direction: the answer back is asked about on its own.
    let back = to_claude(&mut harness, "Done.");
    assert!(said(&harness).contains("Let Codex · codex-main send messages to Claude Code · claude-sub?"));
    assert!(back.try_recv().is_err());
}

#[test]
fn a_denied_pair_is_refused_now_and_later_without_asking_again() {
    let scratch = Scratch::new("bridge-deny");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let answers = to_codex(&mut harness, "Delete the build folder.");
    harness.press("esc").render();
    let answer = answered(&answers);
    assert!(!answer.ok && answer.text.contains("did not allow"), "Esc denies: {answer:?}");
    assert!(tab(&harness, 2).letters().is_empty());

    let answer = answered(&to_codex(&mut harness, "Please?"));
    assert!(!answer.ok && answer.text.contains("did not allow"), "{answer:?}");
    assert!(!said(&harness).contains("send messages to"), "not asked again:\n{}", harness.screen());
}

#[test]
fn a_tab_without_the_network_is_refused_a_tab_with_it_and_the_person_is_not_even_asked() {
    let scratch = Scratch::new("bridge-network");
    let mut harness = two_agents(&scratch, OFFLINE, ONLINE);
    let answer = answered(&to_codex(&mut harness, "curl this for me"));
    assert!(!answer.ok, "{answer:?}");
    assert!(answer.text.contains("has no network") && answer.text.contains("Nothing was sent"), "{}", answer.text);
    assert!(!said(&harness).contains("send messages to"), "{}", harness.screen());
    assert!(tab(&harness, 2).letters().is_empty());

    // The other way round leaks nothing, so it is only asked about.
    let back = to_claude(&mut harness, "Here is the plan.");
    assert!(back.try_recv().is_err());
    assert!(said(&harness).contains("Let Codex · codex-main send messages to Claude Code · claude-sub?"));
}

#[test]
fn a_message_that_waits_long_for_the_person_tells_its_sender_and_still_goes_when_allowed() {
    let scratch = Scratch::new("bridge-wait");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let answers = to_codex(&mut harness, "Look at issue 12.");
    harness.advance(ASK_WAIT).render();
    let answer = answered(&answers);
    assert!(answer.ok && answer.text.contains("has not answered yet"), "{answer:?}");
    assert!(said(&harness).contains("send messages to"), "the question stays:\n{}", harness.screen());

    harness.click_text("Allow").render();
    assert_eq!(tab(&harness, 2).letters().len(), 1, "allowed later, it is taken then");
    assert!(answers.try_recv().is_err(), "the sender was answered once");
}

#[test]
fn a_message_waiting_for_the_person_is_refused_when_either_tab_closes() {
    let scratch = Scratch::new("bridge-close");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let answers = to_codex(&mut harness, "Start the server.");
    harness.send(Test::Screen(Msg::CloseTab(2)));
    let answer = answered(&answers);
    assert!(!answer.ok && answer.text.contains("closed"), "{answer:?}");
}

#[test]
fn two_agents_answering_each_other_are_stopped_and_told_why() {
    let scratch = Scratch::new("bridge-loop");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let (claude, codex) = (tab(&harness, 0).key(), tab(&harness, 2).key());
    harness.send(Test::Screen(Msg::Allow { from: claude, to: codex, allow: true }));
    harness.send(Test::Screen(Msg::Allow { from: codex, to: claude, allow: true }));
    let mut taken = 0;
    let refusal = loop {
        let answers =
            if taken % 2 == 0 { to_codex(&mut harness, "your turn") } else { to_claude(&mut harness, "yours") };
        let answer = answered(&answers);
        if !answer.ok {
            break answer;
        }
        taken += 1;
        assert!(taken <= MOST_HOPS, "the exchange was never cut");
    };
    assert_eq!(taken, MOST_HOPS, "every step up to the longest chain is taken");
    assert!(
        refusal.text.contains(&format!("message {}", MOST_HOPS + 1)) && refusal.text.contains("forever"),
        "{}",
        refusal.text
    );
}

#[test]
fn a_tab_sending_too_fast_is_slowed_down_and_told_so() {
    let scratch = Scratch::new("bridge-pace");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let (claude, codex) = (tab(&harness, 0).key(), tab(&harness, 2).key());
    harness.send(Test::Screen(Msg::Allow { from: claude, to: codex, allow: true }));
    for n in 0..MOST_PER_WINDOW {
        assert!(answered(&to_codex(&mut harness, &format!("task {n}"))).ok);
    }
    let answer = answered(&to_codex(&mut harness, "one more"));
    assert!(!answer.ok && answer.text.contains(&format!("sent {MOST_PER_WINDOW} messages")), "{answer:?}");
}

#[test]
fn messages_that_cannot_be_sent_say_why_and_nothing_is_sent() {
    let scratch = Scratch::new("bridge-refusals");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let answer = answered(&ask(&mut harness, 0, Request::Send { tab: "99".to_owned(), text: "hi".to_owned() }));
    assert!(!answer.ok && answer.text.contains("no other agent tab 99"), "{answer:?}");
    let own = tab(&harness, 0).key().0.to_string();
    let answer = answered(&ask(&mut harness, 0, Request::Send { tab: own, text: "hi".to_owned() }));
    assert!(!answer.ok, "a tab does not send to itself: {answer:?}");
    let answer = answered(&to_codex(&mut harness, "   "));
    assert!(!answer.ok && answer.text.contains("empty"), "{answer:?}");
    let long = "x".repeat(crate::bridge::protocol::MOST_TEXT + 1);
    let answer = answered(&to_codex(&mut harness, &long));
    assert!(!answer.ok && answer.text.contains("longer than"), "{answer:?}");
    // By its title, when only one tab has it.
    let answer = ask(&mut harness, 0, Request::Send { tab: "codex-main".to_owned(), text: "hi".to_owned() });
    assert!(answer.try_recv().is_err() && said(&harness).contains("send messages to"), "the title names the tab");
}

#[test]
fn a_waiting_message_is_shown_in_its_tab_read_there_and_discarded() {
    let scratch = Scratch::new("bridge-letters");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let (claude, codex) = (tab(&harness, 0).key(), tab(&harness, 2).key());
    harness.send(Test::Screen(Msg::Allow { from: claude, to: codex, allow: true }));
    assert!(answered(&to_codex(&mut harness, "Please review src/parse.rs.")).ok);
    assert!(!said(&harness).contains("waits here"), "the sender's own tab shows nothing:\n{}", harness.screen());

    harness.send(Test::Screen(Msg::OpenTab(2))).render();
    let screen = said(&harness);
    assert!(screen.contains("A message from Claude Code · claude-sub waits here"), "{screen}");
    assert!(!screen.contains("Please review"), "it is read on asking:\n{screen}");

    harness.click_text("Read").render();
    let screen = said(&harness);
    assert!(
        screen.contains("From Claude Code · claude-sub") && screen.contains("Please review src/parse.rs."),
        "{screen}"
    );
    assert!(screen.contains("Hide"), "{screen}");

    assert!(answered(&to_codex(&mut harness, "And the tests.")).ok);
    harness.render();
    assert!(
        said(&harness).contains("2 messages wait here, the last from Claude Code · claude-sub"),
        "{}",
        harness.screen()
    );

    harness.click_text("Discard").render();
    assert!(!said(&harness).contains("wait here"), "{}", harness.screen());
    assert!(tab(&harness, 2).letters().is_empty());
}

#[test]
fn on_a_narrow_screen_the_line_gives_way_before_its_buttons() {
    let scratch = Scratch::new("bridge-narrow");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let (claude, codex) = (tab(&harness, 0).key(), tab(&harness, 2).key());
    harness.send(Test::Screen(Msg::Allow { from: claude, to: codex, allow: true }));
    assert!(answered(&to_codex(&mut harness, "hi")).ok);
    harness.send(Test::Screen(Msg::OpenTab(2))).send(Test::Screen(Msg::TogglePanel(true)));
    harness.resize(64, 24).advance(Duration::from_secs(1)).render();
    let screen = harness.screen();
    assert!(screen.contains("Read") && screen.contains("Discard"), "both buttons whole:\n{screen}");
    assert!(screen.contains("A…"), "the sentence is cut short instead:\n{screen}");
}

#[test]
fn the_line_of_waiting_messages_reads_in_turkish_too() {
    let scratch = Scratch::new("bridge-turkish");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    harness.set_locale("tr");
    let answers = to_codex(&mut harness, "Merhaba");
    assert!(
        said(&harness).contains("Claude Code · claude-sub, Codex · codex-main sekmesine mesaj gönderebilsin mi?"),
        "{}",
        harness.screen()
    );
    harness.click_text("İzin ver").render();
    assert!(answered(&answers).text.starts_with("Alındı, ama henüz iletilmedi"));
    harness.send(Test::Screen(Msg::OpenTab(2))).render();
    assert!(
        said(&harness).contains("Claude Code · claude-sub sekmesinden bir mesaj burada bekliyor"),
        "{}",
        harness.screen()
    );
    harness.resize(180, 24).render();
    assert!(said(&harness).contains("iletilemedi, bu sekmedeki kodlama aracı çalışmıyor"), "{}", harness.screen());
}

#[cfg(unix)]
#[test]
fn a_bridged_screen_listens_in_the_workspaces_folder_and_answers_a_real_connection() {
    use std::io::{BufRead, BufReader, Write};

    let scratch = Scratch::new("bridge-socket");
    let mut screen = one_workspace(&scratch).bridging(true);
    open(&mut screen, claude());
    drop(super::super::bridge::follow(&mut screen));
    let workspace = screen.workspace().expect("a workspace is open");
    assert!(workspace.is_bridged(), "the workspace listens");
    let super::super::bridge::Link::On { listener, run } = &workspace.link else { panic!("listening") };
    let (inbox, run) = (listener.inbox(), *run);
    let socket = scratch.0.join("Containers").join("MCP").join("bridge.sock");
    assert_eq!(listener.socket(), socket);
    assert!(scratch.0.join("Containers").join("MCP").join("qcode-bridge.mjs").exists());

    let token = workspace.tabs()[0].token().to_owned();
    let asking = std::thread::spawn(move || {
        let mut stream = std::os::unix::net::UnixStream::connect(&socket).expect("the socket answers");
        stream.write_all(format!("{{\"token\":\"{token}\",\"op\":\"list\"}}\n").as_bytes()).expect("asked");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("answered");
        line
    });
    let call = inbox.next();
    drop(super::super::bridge::called(&mut screen, "firefly", run, call));
    let line = asking.join().expect("the asking side ends");
    // No language is loaded outside a harness, so the list is read rather than its words.
    assert!(line.starts_with("{\"ok\":true,") && line.ends_with(",\"tabs\":[]}\n"), "{line}");

    // A second follow opens nothing new, and closing the workspace takes the socket away.
    drop(super::super::bridge::follow(&mut screen));
    apply(&mut screen, Msg::CloseWorkspace(0));
    assert!(!scratch.0.join("Containers").join("MCP").join("bridge.sock").exists());
}

/// A real program in the tab at `index`, standing in for the harness a container would run: it
/// never echoes what is typed, so the two silences can be told apart, and it writes everything
/// that reaches it into `file`. `preface` runs first, which is how a program asks for bracketed
/// paste or takes its time before speaking.
fn harness_in(
    harness: &mut Harness<Bridged>,
    scratch: &Scratch,
    index: usize,
    file: &Path,
    preface: &str,
) -> TerminalSession {
    let script = format!("stty -echo; {preface} cat > {}", file.display());
    let session = TerminalSession::spawn(OsStr::new("/bin/sh"), &["-c", script.as_str()], &scratch.0)
        .expect("a pseudo-terminal for the tab");
    harness.send(Test::Attach(index, session.clone()));
    session
}

/// Tries a delivery as of `now`, the moment the screen's own timer would reach.
fn deliver_at(harness: &mut Harness<Bridged>, now: Instant) {
    harness.send(Test::Deliver(now)).render();
}

/// Reads `file` until it holds `wanted`, and gives up only after a wait long enough for a loaded
/// machine: a short bound would buy no correctness and fail on a busy one.
fn written(file: &Path, wanted: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let read = std::fs::read_to_string(file).unwrap_or_default();
        if read.contains(wanted) || Instant::now() > deadline {
            return read;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What reached the program once it has had time to write it down, for a test that expects
/// nothing to have reached it.
fn settled(file: &Path) -> Vec<u8> {
    std::thread::sleep(Duration::from_millis(300));
    std::fs::read(file).unwrap_or_default()
}

/// Lets the Claude Code tab send to the Codex tab without asking the person each time.
fn allow_pair(harness: &mut Harness<Bridged>) {
    let (claude, codex) = (tab(harness, 0).key(), tab(harness, 2).key());
    harness.send(Test::Screen(Msg::Allow { from: claude, to: codex, allow: true }));
}

#[test]
fn a_message_goes_in_when_the_tab_falls_quiet_and_never_while_its_tool_is_still_writing() {
    let scratch = Scratch::new("bridge-quiet-output");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    allow_pair(&mut harness);
    let file = scratch.0.join("codex-read.txt");
    // The stand-in speaks once, a while after it starts, the way an agent answers.
    let session = harness_in(&mut harness, &scratch, 2, &file, "sleep 1; printf 'thinking\\n';");

    let answer = answered(&to_codex(&mut harness, "Please write the test for parse()."));
    assert!(answer.ok && answer.text.starts_with("Taken. The message will be written"), "{answer:?}");
    assert_eq!(tab(&harness, 2).letters().len(), 1, "it is still waiting");

    // Wait for the program's own line, and try the moment its silence is only half grown.
    while session.watch().next() != TerminalEvent::Output {}
    let spoke = session.last_output();
    deliver_at(&mut harness, spoke + QUIET / 2);
    assert_eq!(tab(&harness, 2).letters().len(), 1, "a tool still writing is not written into");
    assert!(settled(&file).is_empty(), "nothing reached the tool: {:?}", settled(&file));

    deliver_at(&mut harness, spoke + QUIET);
    let read = written(&file, "parse()");
    assert!(read.contains("Through QCode, from the Claude Code · claude-sub tab:"), "{read:?}");
    assert!(read.contains("Please write the test for parse()."), "{read:?}");
    assert!(read.ends_with('\n'), "Return was written after it: {read:?}");
    assert!(tab(&harness, 2).letters().is_empty(), "it is gone from the queue");
    assert!(tab(&harness, 2).undelivered().is_none());
    assert!(!said(&harness).contains("waits here"), "{}", harness.screen());
}

#[test]
fn a_message_waits_while_the_person_is_typing_in_the_tab() {
    let scratch = Scratch::new("bridge-quiet-input");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    allow_pair(&mut harness);
    let file = scratch.0.join("codex-read.txt");
    let session = harness_in(&mut harness, &scratch, 2, &file, "");
    assert!(answered(&to_codex(&mut harness, "Run the tests.")).ok);

    // The tool has been silent from the start; let that silence grow past the wait, so that only
    // the person's typing can still hold the message back.
    std::thread::sleep(QUIET);
    session.write(b"half a line").expect("the person types");
    let typed = session.last_input();

    deliver_at(&mut harness, typed + QUIET / 2);
    assert_eq!(tab(&harness, 2).letters().len(), 1, "a half-written line is not cut in half");

    deliver_at(&mut harness, typed + QUIET);
    let read = written(&file, "Run the tests.");
    assert!(read.contains("half a line"), "the person's own line is still theirs: {read:?}");
    assert!(read.contains("Through QCode, from the Claude Code · claude-sub tab:"), "{read:?}");
    assert!(tab(&harness, 2).letters().is_empty());
}

#[test]
fn a_message_stays_in_the_tab_when_its_tool_has_ended_and_goes_in_once_it_is_started_again() {
    let scratch = Scratch::new("bridge-ended");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    allow_pair(&mut harness);
    let file = scratch.0.join("codex-read.txt");
    let session = harness_in(&mut harness, &scratch, 2, &file, "");
    assert!(answered(&to_codex(&mut harness, "Look at issue 12.")).ok);

    session.kill();
    while !matches!(session.watch().next(), TerminalEvent::Exited(_)) {}
    deliver_at(&mut harness, session.last_output() + QUIET * 4);
    assert_eq!(tab(&harness, 2).letters().len(), 1, "a message is never dropped on the way");
    assert_eq!(tab(&harness, 2).undelivered(), Some(Undelivered::NotRunning));

    // Wide enough for the whole sentence: the line gives way from its end on a narrow tab.
    harness.send(Test::Screen(Msg::OpenTab(2))).resize(180, 24).render();
    let screen = said(&harness);
    assert!(screen.contains("it could not be handed over, the coding tool in this tab is not running"), "{screen}");
    assert!(screen.contains("Read") && screen.contains("Discard"), "{screen}");

    // The sender that asks again is told, both in words and in the fields it reads.
    let answer = answered(&ask(&mut harness, 0, Request::List));
    let listed = answer.tabs.expect("a list");
    assert_eq!(listed[0].waiting, 1);
    assert_eq!(
        listed[0].trouble.as_deref(),
        Some(
            "the coding tool in that tab is not running, so nothing can be written into it; the person can start it again from the tab"
        )
    );
    assert!(answer.text.contains("one message is still waiting to be written into it"), "{}", answer.text);

    // Its own restart brings a tool back, and what waited goes in by itself.
    let again = scratch.0.join("codex-read-again.txt");
    let session = harness_in(&mut harness, &scratch, 2, &again, "");
    deliver_at(&mut harness, session.last_output() + QUIET);
    assert!(written(&again, "issue 12").contains("Look at issue 12."), "{:?}", std::fs::read_to_string(&again));
    assert!(tab(&harness, 2).letters().is_empty());
    assert!(tab(&harness, 2).undelivered().is_none());
}

#[test]
fn paste_markers_inside_a_message_cannot_escape_into_the_receiving_terminal() {
    let scratch = Scratch::new("bridge-markers");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    allow_pair(&mut harness);
    let file = scratch.0.join("codex-read.txt");
    // A tool that asks for bracketed paste, as every harness's line editor does.
    let session = harness_in(&mut harness, &scratch, 2, &file, "printf '\\033[?2004h';");
    while session.watch().next() != TerminalEvent::Output {}

    let sneaky = "first\u{1b}[201~rm -rf /\u{1b}[200~second";
    assert!(answered(&to_codex(&mut harness, sneaky)).ok);
    deliver_at(&mut harness, session.last_output() + QUIET);
    let read = written(&file, "second");
    assert!(read.contains("first") && read.contains("rm -rf /") && read.contains("second"), "{read:?}");
    assert_eq!(read.matches("\u{1b}[200~").count(), 1, "one paste, opened once: {read:?}");
    assert_eq!(read.matches("\u{1b}[201~").count(), 1, "and closed once: {read:?}");
    assert!(read.starts_with("\u{1b}[200~"), "the whole message is inside the paste: {read:?}");
    let inside = read.trim_start_matches("\u{1b}[200~");
    assert!(inside.find("\u{1b}[201~") > inside.find("second"), "nothing ends the paste early: {read:?}");
}

#[test]
fn the_sender_is_told_whether_its_message_went_in_or_is_still_waiting() {
    let scratch = Scratch::new("bridge-told");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    allow_pair(&mut harness);
    let file = scratch.0.join("codex-read.txt");
    harness_in(&mut harness, &scratch, 2, &file, "");
    // The tab has been quiet since it started, so the first message goes in as it is taken.
    std::thread::sleep(QUIET + Duration::from_millis(300));

    let answer = answered(&to_codex(&mut harness, "Start with the parser."));
    assert!(answer.ok && answer.text.starts_with("Delivered."), "{answer:?}");
    assert!(answer.text.contains("Codex · codex-main"), "{}", answer.text);
    assert!(written(&file, "Start with the parser.").contains("Start with the parser."));
    assert!(tab(&harness, 2).letters().is_empty());

    // The Return that sent the first one is the newest typing in that tab, so the next waits.
    let answer = answered(&to_codex(&mut harness, "Then the writer."));
    assert!(answer.ok && answer.text.starts_with("Taken. The message will be written"), "{answer:?}");
    let listed = answered(&ask(&mut harness, 0, Request::List)).tabs.expect("a list");
    assert_eq!((listed[0].waiting, listed[0].trouble.clone()), (1, None));
}

#[test]
fn a_tab_that_was_busy_is_looked_at_again_on_its_own_and_nothing_is_timed_while_nothing_waits() {
    let scratch = Scratch::new("bridge-timer");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    allow_pair(&mut harness);
    assert!(!harness.app().0.delivering, "no tab holds a message");
    let file = scratch.0.join("codex-read.txt");
    harness_in(&mut harness, &scratch, 2, &file, "");

    assert!(answered(&to_codex(&mut harness, "Read the note first.")).ok);
    assert!(harness.app().0.delivering, "a message waits, so another look is timed");
    assert_eq!(tab(&harness, 2).letters().len(), 1);

    std::thread::sleep(QUIET + Duration::from_millis(300));
    harness.advance(RETRY_EVERY).render();
    assert!(written(&file, "Read the note first.").contains("Read the note first."));
    assert!(tab(&harness, 2).letters().is_empty(), "the screen delivered it without being asked");
}

#[test]
fn the_message_written_into_a_tab_names_its_sender_in_turkish_too() {
    let scratch = Scratch::new("bridge-teslim-tr");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    harness.set_locale("tr");
    allow_pair(&mut harness);
    let file = scratch.0.join("codex-read.txt");
    let session = harness_in(&mut harness, &scratch, 2, &file, "");
    let answer = answered(&to_codex(&mut harness, "parse() için testi yaz."));
    assert!(answer.text.starts_with("Alındı. Mesaj"), "{answer:?}");
    deliver_at(&mut harness, session.last_output() + QUIET);
    let read = written(&file, "parse()");
    assert!(read.contains("QCode aracılığıyla Claude Code · claude-sub sekmesinden:"), "{read:?}");
    assert!(read.contains("parse() için testi yaz."), "{read:?}");
}

#[test]
fn a_window_tab_is_never_a_tab_a_message_can_be_sent_to() {
    // Antigravity IDE opens a window instead of drawing in a tab's terminal. There is no prompt
    // to write a message into and no way to push one to it, so it is not offered as somewhere to
    // send to: an agent that was given the choice would believe it had handed work over.
    let scratch = Scratch::new("bridge-window");
    let profiles =
        vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("antigravity", HarnessKind::AntigravityIde)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    open(&mut screen, Choice::Window("antigravity".to_owned()));
    let mut harness = Harness::with_env(Bridged(screen), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.advance(Duration::from_secs(1)).render();
    assert!(matches!(tab(&harness, 1).kind(), TabKind::Desktop(_)), "{:?}", tab(&harness, 1).kind());

    let answer = answered(&ask(&mut harness, 0, Request::List));
    assert!(answer.ok, "{answer:?}");
    assert!(answer.tabs.expect("a list").is_empty(), "the window is not somewhere to send to");
    assert!(answer.text.contains("No other tab"), "{}", answer.text);

    let window = tab(&harness, 1).key().0.to_string();
    let answer = answered(&ask(&mut harness, 0, Request::Send { tab: window, text: "take this".to_owned() }));
    assert!(!answer.ok && answer.text.contains("Nothing was sent"), "{answer:?}");
    assert!(tab(&harness, 1).letters().is_empty(), "nothing waits in a window");
}

#[test]
fn the_agent_in_a_window_lists_the_terminal_tabs_and_gives_one_of_them_work() {
    let scratch = Scratch::new("bridge-window-sends");
    let mut harness = an_agent_and_a_window(&scratch);
    assert!(matches!(tab(&harness, 1).kind(), TabKind::Desktop(_)), "the second tab is the window");

    let answer = answered(&ask(&mut harness, 1, Request::List));
    assert!(answer.ok, "the window's agent speaks for its own tab: {answer:?}");
    let tabs = answer.tabs.expect("a list");
    assert_eq!(tabs.len(), 1, "only the Claude Code tab: {tabs:?}");
    assert_eq!(tabs[0].tab, tab(&harness, 0).key().0.to_string());

    let claude = tab(&harness, 0).key().0.to_string();
    let answers = ask(&mut harness, 1, Request::Send { tab: claude, text: "Rename parse() to read().".to_owned() });
    let screen = said(&harness);
    assert!(
        screen.contains("Let Antigravity IDE · anti window send messages to Claude Code · claude-sub?"),
        "{screen}"
    );
    harness.click_text("Allow").render();
    assert!(answered(&answers).ok);
    let letter =
        Letter { from: "Antigravity IDE · anti window".to_owned(), text: "Rename parse() to read().".to_owned() };
    assert_eq!(tab(&harness, 0).letters(), [letter], "the work is waiting in the terminal tab");
}

#[test]
fn a_window_is_never_somewhere_a_message_can_be_left() {
    let scratch = Scratch::new("bridge-window-target");
    let mut harness = an_agent_and_a_window(&scratch);
    let answer = answered(&ask(&mut harness, 0, Request::List));
    assert!(answer.ok && answer.tabs.expect("a list").is_empty(), "a window is not a tab to send to");
    assert!(answer.text.contains("No other tab of this workspace runs an agent"), "{}", answer.text);

    // Not by its number and not by its title either: a window draws no prompt, so a message left
    // there would be one the sending agent believes it handed over and nobody ever reads.
    for named in [tab(&harness, 1).key().0.to_string(), "anti window".to_owned()] {
        let answers = ask(&mut harness, 0, Request::Send { tab: named.clone(), text: "Open the diff.".to_owned() });
        let answer = answered(&answers);
        assert!(!answer.ok, "{named}: {answer:?}");
        assert!(answer.text.contains("There is no other agent tab"), "{named}: {}", answer.text);
        assert!(!said(&harness).contains("send messages to"), "the person is not even asked: {}", harness.screen());
        assert!(tab(&harness, 1).letters().is_empty(), "{named}");
    }
}
