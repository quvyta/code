//! The bridge between tabs on the screen: which tab asks, the person's approval, the rules that
//! refuse a message and say why, and the messages that wait in the tab they were sent to.
//!
//! No socket is opened here: a harness would wait on it forever. The calls a socket would carry
//! are handed to the screen by the test application below, the way the screen's own listener
//! hands them over, and their answers are read from the other end of each call.

use std::sync::mpsc::Receiver;
use std::time::Instant;

use super::*;

use crate::bridge::protocol::{Answer, Malformed, Question, Request};
use crate::bridge::rules::{MOST_HOPS, MOST_PER_WINDOW};
use crate::bridge::socket::Call;
use crate::ui::project::Letter;
use crate::ui::project::bridge::ASK_WAIT;

/// The project screen, with one more way in: a call as the project's socket would bring it.
struct Bridged(ProjectScreen);

#[derive(Debug, Clone)]
enum Test {
    Screen(Msg),
    Call(Call),
}

impl App for Bridged {
    type Msg = Test;

    fn update(&mut self, message: Test) -> Command<Test> {
        match message {
            Test::Screen(message) => super::super::update(&mut self.0, message).map(Test::Screen),
            Test::Call(call) => {
                super::super::bridge::answer(&mut self.0, "firefly", call, Instant::now()).map(Test::Screen)
            }
        }
    }

    fn view(&self, ui: &mut View<'_, Test>) {
        super::super::view(&self.0, ui, Test::Screen, |_| {}, |_| {});
    }
}

/// A project with a Claude Code tab and a Codex tab, the first with the network as `claude`
/// says and the second with it as `codex` says, and a shell tab between them.
fn two_agents(scratch: &Scratch, claude: NetworkMode, codex: NetworkMode) -> Harness<Bridged> {
    let profiles = vec![
        Profile { network: claude, ..profile("claude-sub", HarnessKind::ClaudeCode) },
        Profile { network: codex, ..profile("codex-main", HarnessKind::Codex) },
    ];
    let mut screen = ProjectScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![project("firefly", "Firefly", scratch.paths(), profiles)],
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
    &harness.app().0.project().expect("a project is open").tabs()[index]
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
fn a_question_from_no_tab_of_the_project_or_not_a_question_at_all_is_refused() {
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
    assert!(answer.ok && answer.text.starts_with("Taken. The message waits in Codex · codex-main"), "{answer:?}");
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
    assert!(answered(&answers).text.starts_with("Alındı."));
    harness.send(Test::Screen(Msg::OpenTab(2))).render();
    assert!(
        said(&harness).contains("Claude Code · claude-sub sekmesinden bir mesaj burada bekliyor"),
        "{}",
        harness.screen()
    );
}

#[cfg(unix)]
#[test]
fn a_bridged_screen_listens_in_the_projects_folder_and_answers_a_real_connection() {
    use std::io::{BufRead, BufReader, Write};

    let scratch = Scratch::new("bridge-socket");
    let mut screen = one_project(&scratch).bridging(true);
    open(&mut screen, claude());
    drop(super::super::bridge::follow(&mut screen));
    let project = screen.project().expect("a project is open");
    assert!(project.is_bridged(), "the project listens");
    let super::super::bridge::Link::On { listener, run } = &project.link else { panic!("listening") };
    let (inbox, run) = (listener.inbox(), *run);
    let socket = scratch.0.join("Containers").join("MCP").join("bridge.sock");
    assert_eq!(listener.socket(), socket);
    assert!(scratch.0.join("Containers").join("MCP").join("qcode-bridge.mjs").exists());

    let token = project.tabs()[0].token().to_owned();
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

    // A second follow opens nothing new, and closing the project takes the socket away.
    drop(super::super::bridge::follow(&mut screen));
    apply(&mut screen, Msg::CloseProject(0));
    assert!(!scratch.0.join("Containers").join("MCP").join("bridge.sock").exists());
}
