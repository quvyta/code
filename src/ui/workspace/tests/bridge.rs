//! The bridge between tabs on the screen: which tab asks, the person's approval when they turned
//! asking on, the rules that refuse a message and say why, and the messages that wait in the tab
//! they were sent to.
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

use crate::bridge::protocol::{Answer, Kind, Malformed, Question, Request, To};
use crate::bridge::rules::{MOST_HOPS, MOST_PER_WINDOW};
use crate::bridge::socket::Call;
use crate::ui::workspace::bridge::{ASK_WAIT, QUIET, RETRY_EVERY};
use crate::ui::workspace::{Letter, Undelivered};

/// The workspace screen, with one more way in: a call as the workspace's socket would bring it.
///
/// The settings screen can stand over it, the way QCode opens it from the rail, so that a switch
/// the person moves there is the one the bridge obeys.
struct Bridged(WorkspaceScreen, Option<crate::ui::settings::Settings>);

#[derive(Debug, Clone)]
enum Test {
    Screen(Msg),
    Call(Call),
    /// Gives the tab at this place a running program to be typed into, as a container that came
    /// up gives a tab its session.
    Attach(usize, TerminalSession),
    /// Looks at the tabs holding messages as of this moment.
    Deliver(Instant),
    /// Opens the settings over the screen, or closes them.
    ShowSettings(bool),
    /// Something happened on the settings screen.
    Settings(crate::ui::settings::Msg),
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
            Test::ShowSettings(open) => {
                // The screen shows what the settings file holds, which is what the workspace screen
                // was last given.
                let file = if self.0.ask_first() { "[bridge]\nask-first = true\n" } else { "" };
                self.1 = open.then(|| {
                    let health = crate::ui::settings::engine::Health::Working;
                    crate::ui::settings::testing::from_config(file, EngineKind::Podman, health)
                });
                Command::none()
            }
            Test::Settings(message) => {
                let Some(settings) = self.1.as_mut() else { return Command::none() };
                let (command, request) = crate::ui::settings::update(settings, message);
                // What QCode does with the request: the switch reaches the open workspace screen.
                if let Some(crate::ui::settings::Request::AskFirst(ask)) = request {
                    self.0.set_ask_first(ask);
                }
                command.map(Test::Settings)
            }
        }
    }

    fn view(&self, ui: &mut View<'_, Test>) {
        match &self.1 {
            Some(settings) => {
                ui.map(Test::Settings, |ui| crate::ui::settings::view(settings, ui)).fill();
            }
            None => super::super::view(&self.0, ui, Test::Screen, |_| {}, |_| {}, |_| {}),
        }
    }
}

/// A workspace with a Claude Code tab and a Codex tab, the first with the network as `claude`
/// says and the second with it as `codex` says, and a shell tab between them. Nobody is asked
/// before a message, which is how QCode starts.
fn two_agents(scratch: &Scratch, claude: NetworkMode, codex: NetworkMode) -> Harness<Bridged> {
    agents(scratch, claude, codex, false)
}

/// [`two_agents`] with asking turned on, as the person turns it on in the settings.
fn two_agents_asking(scratch: &Scratch, claude: NetworkMode, codex: NetworkMode) -> Harness<Bridged> {
    agents(scratch, claude, codex, true)
}

fn agents(scratch: &Scratch, claude: NetworkMode, codex: NetworkMode, ask: bool) -> Harness<Bridged> {
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
    screen.set_ask_first(ask);
    let mut harness = Harness::with_env(Bridged(screen, None), env(), SIZE.0, SIZE.1);
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
    let (call, answers) = Call::new(Ok(Question { token, session: None, request }));
    harness.send(Test::Call(call));
    harness.render();
    answers
}

/// A message from the Claude Code tab to the Codex tab.
fn to_codex(harness: &mut Harness<Bridged>, text: &str) -> Receiver<Answer> {
    let codex = tab(harness, 2).number().to_string();
    ask(harness, 0, Request::Send { to: To::Tab(codex), text: text.to_owned(), kind: Kind::Info })
}

/// A message from the Codex tab to the Claude Code tab.
fn to_claude(harness: &mut Harness<Bridged>, text: &str) -> Receiver<Answer> {
    let claude = tab(harness, 0).number().to_string();
    ask(harness, 2, Request::Send { to: To::Tab(claude), text: text.to_owned(), kind: Kind::Info })
}

/// A workspace with a Claude Code tab and, beside it, the window of a desktop profile, with the
/// person asked before a first message.
fn an_agent_and_a_window(scratch: &Scratch) -> Harness<Bridged> {
    an_agent_and_a_window_asking(scratch, true)
}

/// [`an_agent_and_a_window`], with the person asked before a first message only when `ask`.
fn an_agent_and_a_window_asking(scratch: &Scratch, ask: bool) -> Harness<Bridged> {
    let profiles = vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("anti", HarnessKind::AntigravityIde)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    open(&mut screen, Choice::Window("anti".to_owned()));
    screen.set_ask_first(ask);
    let mut harness = Harness::with_env(Bridged(screen, None), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Test::Screen(Msg::TogglePanel(false))).send(Test::Screen(Msg::OpenTab(0)));
    harness.advance(Duration::from_secs(1)).render();
    harness
}

/// A message for information from the Claude Code tab, whose id is `tab`.
fn from_claude(tab: u32, text: &str) -> Letter {
    Letter {
        from: "Claude Code · claude-sub".to_owned(),
        tab,
        title: "claude-sub".to_owned(),
        harness: "Claude Code".to_owned(),
        kind: Kind::Info,
        text: text.to_owned(),
    }
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
    assert_eq!(tabs[0].tab, tab(&harness, 2).number().to_string());
    assert_eq!((tabs[0].title.as_str(), tabs[0].harness.as_str(), tabs[0].network), ("codex-main", "Codex", false));
    assert!(answer.text.contains("codex-main, Codex, without the network"), "{}", answer.text);
}

#[test]
fn a_question_from_no_tab_of_the_workspace_or_not_a_question_at_all_is_refused() {
    let scratch = Scratch::new("bridge-stranger");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let (call, answers) =
        Call::new(Ok(Question { token: "guessed".to_owned(), session: None, request: Request::List }));
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
fn with_asking_on_the_first_message_between_two_tabs_asks_the_person_and_is_taken_when_allowed() {
    let scratch = Scratch::new("bridge-allow");
    let mut harness = two_agents_asking(&scratch, ONLINE, ONLINE);
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
    let letter = from_claude(1, "Please write the test for parse().");
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
    let mut harness = two_agents_asking(&scratch, ONLINE, ONLINE);
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

    // The other way round leaks nothing, so it goes as any message does.
    let back = answered(&to_claude(&mut harness, "Here is the plan."));
    assert!(back.ok, "{back:?}");
    assert_eq!(tab(&harness, 0).letters().len(), 1);

    // Turning asking on changes nothing about it: the rule is a boundary, not a question.
    let scratch = Scratch::new("bridge-network-asking");
    let mut harness = two_agents_asking(&scratch, OFFLINE, ONLINE);
    let answer = answered(&to_codex(&mut harness, "curl this for me"));
    assert!(!answer.ok && answer.text.contains("has no network"), "{answer:?}");
    assert!(!said(&harness).contains("send messages to"), "{}", harness.screen());
    let back = to_claude(&mut harness, "Here is the plan.");
    assert!(back.try_recv().is_err());
    assert!(said(&harness).contains("Let Codex · codex-main send messages to Claude Code · claude-sub?"));
}

#[test]
fn a_message_that_waits_long_for_the_person_tells_its_sender_and_still_goes_when_allowed() {
    let scratch = Scratch::new("bridge-wait");
    let mut harness = two_agents_asking(&scratch, ONLINE, ONLINE);
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
    let mut harness = two_agents_asking(&scratch, ONLINE, ONLINE);
    let answers = to_codex(&mut harness, "Start the server.");
    harness.send(Test::Screen(Msg::CloseTab(2)));
    let answer = answered(&answers);
    assert!(!answer.ok && answer.text.contains("closed"), "{answer:?}");
}

#[test]
fn two_agents_answering_each_other_are_stopped_and_told_why() {
    let scratch = Scratch::new("bridge-loop");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
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
fn a_task_the_person_gives_a_tab_starts_a_new_exchange_however_long_the_last_one_ran() {
    // In the endurance trial (2026-09-24) the person gave five agents a new round of tasks every
    // few minutes; each round's tasks and answers were counted as one exchange with the round
    // before, and by the third round the agents were refused as if they were looping.
    let scratch = Scratch::new("bridge-new-task");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let claude = harness_in(&mut harness, &scratch, 0, &scratch.0.join("claude-read.txt"), "");
    harness_in(&mut harness, &scratch, 2, &scratch.0.join("codex-read.txt"), "");
    for step in 0..MOST_HOPS {
        let answer = if step % 2 == 0 { to_codex(&mut harness, "task") } else { to_claude(&mut harness, "done") };
        assert!(answered(&answer).ok, "step {step}");
    }
    let refused = answered(&to_codex(&mut harness, "one more"));
    assert!(!refused.ok, "the agents alone are stopped: {refused:?}");

    // The person types a new task into the Claude Code tab: what it sends now is theirs.
    claude.write(b"ROUND 2\r").expect("the person types");
    let answer = answered(&to_codex(&mut harness, "task of round 2"));
    assert!(answer.ok, "a new task is not the old exchange: {answer:?}");
    // And the answer to it is the second step of the new exchange, not the eighth of the old.
    assert!(answered(&to_claude(&mut harness, "done 2")).ok);
}

#[test]
fn an_exchange_the_loop_limit_ends_is_said_under_both_tabs_and_once_wherever_the_person_looks() {
    let scratch = Scratch::new("bridge-loop-line");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    // The person is looking at the shell between the two, so neither tab is on screen. Motion is
    // off, so a notice is on screen as soon as it is sent.
    harness.set_reduced_motion(true);
    harness.send(Test::Screen(Msg::OpenTab(1))).resize(180, SIZE.1).render();
    let mut turn = 0;
    loop {
        let answers =
            if turn % 2 == 0 { to_codex(&mut harness, "your turn") } else { to_claude(&mut harness, "yours") };
        if !answered(&answers).ok {
            break;
        }
        turn += 1;
        assert!(turn <= MOST_HOPS, "the exchange was never cut");
    }
    // Before the cut, neither tab said anything of the kind.
    let screen = said(&harness);
    // The notice wraps its words, so they are looked for in parts.
    for part in ["Claude Code · claude-sub and Codex", "codex-main stopped sending each other messages"] {
        assert!(screen.contains(part), "the person is told where they are looking:\n{}", harness.screen());
    }
    assert!(screen.contains(&format!("after {MOST_HOPS} messages")), "{screen}");
    assert_eq!(tab(&harness, 0).stopped(), Some("Codex · codex-main"));
    assert_eq!(tab(&harness, 2).stopped(), Some("Claude Code · claude-sub"));

    let line = |other: &str| {
        format!("The exchange of messages between this tab and {other} was ended after {MOST_HOPS} messages")
    };
    harness.send(Test::Screen(Msg::OpenTab(0))).render();
    assert!(said(&harness).contains(&line("Codex · codex-main")), "{}", harness.screen());
    harness.send(Test::Screen(Msg::OpenTab(2))).render();
    assert!(said(&harness).contains(&line("Claude Code · claude-sub")), "{}", harness.screen());
    harness.send(Test::Screen(Msg::OpenTab(1))).render();
    assert!(!said(&harness).contains("The exchange of messages"), "the shell took no part:\n{}", harness.screen());

    // An agent that keeps trying is refused again, and the notice is not given a second time.
    harness.advance(Duration::from_secs(30)).render();
    assert!(!answered(&to_codex(&mut harness, "one more")).ok);
    assert!(!said(&harness).contains("stopped sending each other"), "told once:\n{}", harness.screen());

    // The line stays until the person has read it, in each tab on its own.
    harness.send(Test::Screen(Msg::OpenTab(2))).render();
    harness.click_text("Got it").render();
    assert!(!said(&harness).contains("The exchange of messages"), "{}", harness.screen());
    assert!(tab(&harness, 2).stopped().is_none());
    assert_eq!(tab(&harness, 0).stopped(), Some("Codex · codex-main"), "the other tab still says so");
}

#[test]
fn the_line_of_an_ended_exchange_reads_in_turkish_too() {
    let scratch = Scratch::new("bridge-loop-tr");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    harness.set_locale("tr");
    harness.resize(180, SIZE.1).render();
    let mut turn = 0;
    while answered(&if turn % 2 == 0 {
        to_codex(&mut harness, "sıra sende")
    } else {
        to_claude(&mut harness, "sende")
    })
    .ok
    {
        turn += 1;
        assert!(turn <= MOST_HOPS, "the exchange was never cut");
    }
    harness.send(Test::Screen(Msg::OpenTab(2))).render();
    let screen = said(&harness);
    assert!(
        screen.contains(&format!(
            "Bu sekmeyle Claude Code · claude-sub arasındaki yazışma {MOST_HOPS} mesajdan sonra bitirildi"
        )),
        "{screen}"
    );
    assert!(screen.contains("Anladım"), "{screen}");
}

#[test]
fn a_tab_sending_too_fast_is_slowed_down_and_told_so() {
    let scratch = Scratch::new("bridge-pace");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
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
    let answer = answered(&ask(
        &mut harness,
        0,
        Request::Send { to: To::Tab("99".to_owned()), text: "hi".to_owned(), kind: Kind::Info },
    ));
    assert!(!answer.ok && answer.text.contains("no other agent tab 99"), "{answer:?}");
    let own = tab(&harness, 0).number().to_string();
    let answer =
        answered(&ask(&mut harness, 0, Request::Send { to: To::Tab(own), text: "hi".to_owned(), kind: Kind::Info }));
    assert!(!answer.ok, "a tab does not send to itself: {answer:?}");
    let answer = answered(&to_codex(&mut harness, "   "));
    assert!(!answer.ok && answer.text.contains("empty"), "{answer:?}");
    let long = "x".repeat(crate::bridge::protocol::MOST_TEXT + 1);
    let answer = answered(&to_codex(&mut harness, &long));
    assert!(!answer.ok && answer.text.contains("longer than"), "{answer:?}");
    // By its title, when only one tab has it.
    let answer = answered(&ask(
        &mut harness,
        0,
        Request::Send { to: To::Tab("codex-main".to_owned()), text: "hi".to_owned(), kind: Kind::Info },
    ));
    assert!(answer.ok, "the title names the tab: {answer:?}");
    assert_eq!(tab(&harness, 2).letters().len(), 1);
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
    let mut harness = two_agents_asking(&scratch, ONLINE, ONLINE);
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
    assert!(read.contains("Through QCode, from tab 1 «claude-sub» (Claude Code) of this workspace:"), "{read:?}");
    assert!(read.contains("Please write the test for parse()."), "{read:?}");
    assert!(read.ends_with('\n'), "Return was written after it: {read:?}");
    assert!(tab(&harness, 2).letters().is_empty(), "it is gone from the queue");
    assert!(tab(&harness, 2).undelivered().is_none());
    assert!(!said(&harness).contains("waits here"), "{}", harness.screen());
}

#[test]
fn a_message_waits_while_the_person_has_a_line_of_their_own_started_however_long_they_pause() {
    // In the endurance trial (2026-09-24) a person stopped halfway through a line to think, and a
    // message from another tab was typed in after it and sent off with their Return.
    let scratch = Scratch::new("bridge-quiet-input");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    allow_pair(&mut harness);
    let file = scratch.0.join("codex-read.txt");
    let session = harness_in(&mut harness, &scratch, 2, &file, "");
    // The person's hand: the tab on the strip, then its terminal, then the keys.
    harness.click_text("codex-main").render();
    harness.click(i32::from(SIZE.0 / 2), i32::from(SIZE.1 / 2)).render();
    harness.type_text("half a line");
    assert!(answered(&to_codex(&mut harness, "Run the tests.")).ok);
    // The line saying a message waits appears under the terminal; the keyboard stays where the
    // person is typing (it did not, before: the terminal was rebuilt and lost it).
    harness.type_text(" and more");

    // The tool is silent and the person has stopped typing; only their unsent line remains.
    let typed = session.last_input().max(session.last_output());
    for pause in [QUIET * 2, Duration::from_secs(60), Duration::from_secs(600)] {
        deliver_at(&mut harness, typed + pause);
        assert_eq!(tab(&harness, 2).letters().len(), 1, "a line paused over for {pause:?} is still the person's");
    }
    assert_eq!(settled(&file), b"", "nothing was written into the half line");

    // The person sends their line; the message goes in on a line of its own.
    harness.press("enter");
    let sent = session.last_input();
    deliver_at(&mut harness, sent + QUIET);
    let read = written(&file, "Run the tests.");
    assert!(read.starts_with("half a line and more\n"), "the person's line went out alone: {read:?}");
    assert!(read.contains("Through QCode, from tab 1 «claude-sub» (Claude Code) of this workspace:"), "{read:?}");
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

    // QCode's own Return is not the person typing: once the tool has been quiet as long as it
    // must be, the next goes in, without the half minute a person's line is given.
    let session = tab(&harness, 2).session().cloned().expect("the tab runs");
    deliver_at(&mut harness, session.last_input().max(session.last_output()) + QUIET);
    assert!(written(&file, "Then the writer.").contains("Then the writer."));
    assert!(tab(&harness, 2).letters().is_empty());
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
    assert!(
        read.contains("QCode aracılığıyla, bu çalışma alanının 1 numaralı sekmesinden «claude-sub» (Claude Code):"),
        "{read:?}"
    );
    assert!(read.contains("parse() için testi yaz."), "{read:?}");
}

#[test]
fn an_agent_lists_the_window_tab_and_is_told_it_takes_messages_from_its_inbox() {
    // The owner asked an opencode tab to write to the Antigravity window beside it, and it was told
    // there was no other agent tab. A window has no prompt to type into, but its agent can take
    // what waits for it, so it is listed, as a tab whose agent reads its inbox.
    let scratch = Scratch::new("bridge-window");
    let mut harness = an_agent_and_a_window_asking(&scratch, false);
    assert!(matches!(tab(&harness, 1).kind(), TabKind::Desktop(_)), "{:?}", tab(&harness, 1).kind());

    let answer = answered(&ask(&mut harness, 0, Request::List));
    assert!(answer.ok, "{answer:?}");
    let tabs = answer.tabs.expect("a list");
    assert_eq!(tabs.len(), 1, "the window is listed: {tabs:?}");
    assert_eq!(tabs[0].tab, tab(&harness, 1).number().to_string());
    assert_eq!(tabs[0].harness, "Antigravity IDE");
    assert!(tabs[0].inbox, "the list says it reads no prompt: {tabs:?}");
    assert!(answer.text.contains("check_inbox"), "the words say so too: {}", answer.text);
}

#[test]
fn a_message_to_a_window_waits_for_its_inbox_and_its_sender_is_told_so_not_that_it_went_in() {
    let scratch = Scratch::new("bridge-window-waits");
    let mut harness = an_agent_and_a_window_asking(&scratch, false);
    let window = tab(&harness, 1).number().to_string();
    let answer = answered(&ask(
        &mut harness,
        0,
        Request::Send { to: To::Tab(window), text: "Count the files.".to_owned(), kind: Kind::Info },
    ));
    assert!(answer.ok, "{answer:?}");
    assert!(answer.text.contains("check_inbox") && !answer.text.contains("Delivered"), "{}", answer.text);
    let letter = from_claude(1, "Count the files.");
    assert_eq!(tab(&harness, 1).letters(), [letter], "the message waits in the window's tab");
    assert_eq!(tab(&harness, 1).undelivered(), Some(Undelivered::Inbox));

    // By its title as well, the way an agent that read the list may name it.
    let answer = answered(&ask(
        &mut harness,
        0,
        Request::Send { to: To::Tab("anti window".to_owned()), text: "And the dirs.".to_owned(), kind: Kind::Info },
    ));
    assert!(answer.ok, "{answer:?}");
    assert_eq!(tab(&harness, 1).letters().len(), 2);

    // The list says so to the sender, with what the messages are waiting for.
    let listed = answered(&ask(&mut harness, 0, Request::List));
    let tabs = listed.tabs.expect("a list");
    assert_eq!(tabs[0].waiting, 2);
    assert!(tabs[0].trouble.as_deref().is_some_and(|why| why.contains("check_inbox")), "{tabs:?}");

    // The person sees it under the window's tab, which they open on the strip.
    harness.click_text("anti window").render();
    let screen = said(&harness);
    assert!(screen.contains("2 messages wait for this window's agent to check its inbox"), "{screen}");
}

#[test]
fn the_agent_in_a_window_takes_its_messages_from_its_inbox_once_and_answers_them() {
    let scratch = Scratch::new("bridge-window-inbox");
    let mut harness = an_agent_and_a_window_asking(&scratch, false);
    let window = tab(&harness, 1).number().to_string();
    for text in ["Count the files.", "Then the dirs."] {
        let answers = ask(
            &mut harness,
            0,
            Request::Send { to: To::Tab(window.clone()), text: text.to_owned(), kind: Kind::Info },
        );
        assert!(answered(&answers).ok);
    }

    let inbox = answered(&ask(&mut harness, 1, Request::Inbox));
    assert!(inbox.ok, "{inbox:?}");
    let messages = inbox.messages.expect("the messages");
    let got: Vec<(&str, &str)> = messages.iter().map(|m| (m.from.as_str(), m.text.as_str())).collect();
    assert_eq!(
        got,
        [("Claude Code · claude-sub", "Count the files."), ("Claude Code · claude-sub", "Then the dirs.")],
        "every message, oldest first, with its sender"
    );
    assert!(
        inbox.text.contains("Count the files.") && inbox.text.contains("from tab 1 «claude-sub» (Claude Code)"),
        "{}",
        inbox.text
    );
    assert!(messages.iter().all(|m| m.tab == "1" && m.kind == Kind::Info), "{messages:?}");
    assert!(tab(&harness, 1).letters().is_empty(), "taken is delivered: nothing waits any more");
    assert_eq!(tab(&harness, 1).undelivered(), None);

    // Asked again, nothing comes twice.
    let again = answered(&ask(&mut harness, 1, Request::Inbox));
    assert!(again.ok && again.messages.as_ref().is_some_and(Vec::is_empty), "{again:?}");
    assert!(again.text.contains("No message waits"), "{}", again.text);

    // And the window's agent answers the way every agent does.
    let claude = tab(&harness, 0).number().to_string();
    let reply = answered(&ask(
        &mut harness,
        1,
        Request::Send { to: To::Tab(claude), text: "12 files.".to_owned(), kind: Kind::Info },
    ));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(tab(&harness, 0).letters().last().map(|letter| letter.text.as_str()), Some("12 files."));
}

#[test]
fn a_window_looking_at_its_inbox_sees_what_waits_and_leaves_it_for_its_agent() {
    // The window's own watcher looks every few seconds to know when to tell its agent; looking must
    // never be taking, or a prompt that reached no agent would have lost the message.
    let scratch = Scratch::new("bridge-window-peek");
    let mut harness = an_agent_and_a_window_asking(&scratch, false);
    let empty = answered(&ask(&mut harness, 1, Request::Peek));
    assert!(empty.ok && empty.messages.as_ref().is_some_and(Vec::is_empty), "{empty:?}");

    let window = tab(&harness, 1).number().to_string();
    assert!(
        answered(&ask(
            &mut harness,
            0,
            Request::Send { to: To::Tab(window), text: "Count the files.".to_owned(), kind: Kind::Info }
        ))
        .ok
    );
    for _ in 0..2 {
        let looked = answered(&ask(&mut harness, 1, Request::Peek));
        let messages = looked.messages.expect("the waiting messages");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].from, "Claude Code · claude-sub");
        assert_eq!(tab(&harness, 1).letters().len(), 1, "still there for the agent");
    }
    let taken = answered(&ask(&mut harness, 1, Request::Inbox));
    assert_eq!(taken.messages.map(|messages| messages.len()), Some(1), "the agent takes it once");
}

/// Clicks the close mark of the tab titled `title` on the strip, where the person's hand is.
fn click_close(harness: &mut Harness<Bridged>, title: &str) {
    let line: Vec<char> = harness.screen().lines().next().unwrap_or_default().chars().collect();
    let label: Vec<char> = title.chars().collect();
    let at = line.windows(label.len()).position(|cells| cells == label.as_slice()).expect("the tab is on the strip");
    let mark = (at + label.len()..line.len()).find(|&x| line[x] == '×').expect("the tab has a close mark");
    harness.click(i32::try_from(mark).expect("a column"), 0).render();
}

#[test]
fn closing_a_tab_with_messages_waiting_in_it_asks_before_they_are_thrown_away() {
    // In the endurance trial (2026-09-24) a tab with four messages waiting was closed from the strip,
    // and the four went with it while their senders had been told they were taken.
    let scratch = Scratch::new("bridge-close-waiting");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    for text in ["Run the tests.", "Then the linter."] {
        assert!(answered(&to_codex(&mut harness, text)).ok);
    }
    assert_eq!(tab(&harness, 2).letters().len(), 2, "nothing runs in the tab, so they wait");

    click_close(&mut harness, "codex-main");
    let screen = said(&harness);
    assert!(screen.contains("Close codex-main?"), "{screen}");
    assert!(screen.contains("2 messages from other tabs wait in this tab"), "{screen}");
    harness.click_text("Keep the tab").render();
    assert_eq!(harness.app().0.workspace().expect("a workspace").tabs().len(), 3, "the tab stays");
    assert_eq!(tab(&harness, 2).letters().len(), 2, "and so do its messages");

    click_close(&mut harness, "codex-main");
    harness.click_text("Close and discard").render();
    let titles: Vec<String> = harness
        .app()
        .0
        .workspace()
        .expect("a workspace")
        .tabs()
        .iter()
        .map(|tab| format!("{:?}", tab.kind()))
        .collect();
    assert_eq!(titles.len(), 2, "closed once the person said so: {titles:?}");

    // A tab with nothing waiting closes at once, as before.
    click_close(&mut harness, "claude-sub");
    assert_eq!(harness.app().0.workspace().expect("a workspace").tabs().len(), 1, "{}", harness.screen());
}

#[test]
fn a_question_for_an_inbox_from_no_tab_of_the_workspace_takes_nothing() {
    let scratch = Scratch::new("bridge-window-stranger");
    let mut harness = an_agent_and_a_window_asking(&scratch, false);
    let window = tab(&harness, 1).number().to_string();
    assert!(
        answered(&ask(
            &mut harness,
            0,
            Request::Send { to: To::Tab(window), text: "Mine.".to_owned(), kind: Kind::Info }
        ))
        .ok
    );
    let (call, answers) =
        Call::new(Ok(Question { token: "not-a-tab".to_owned(), session: None, request: Request::Inbox }));
    harness.send(Test::Call(call));
    let answer = answered(&answers);
    assert!(!answer.ok, "{answer:?}");
    assert_eq!(tab(&harness, 1).letters().len(), 1, "a stranger takes nobody's messages");
}

#[test]
fn nothing_is_timed_for_messages_that_wait_for_a_windows_inbox() {
    let scratch = Scratch::new("bridge-window-untimed");
    let mut harness = an_agent_and_a_window_asking(&scratch, false);
    let window = tab(&harness, 1).number().to_string();
    assert!(
        answered(&ask(
            &mut harness,
            0,
            Request::Send { to: To::Tab(window), text: "Wait.".to_owned(), kind: Kind::Info }
        ))
        .ok
    );
    assert!(!harness.app().0.delivering, "no retry is set going for a message nobody will type in");
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
    assert_eq!(tabs[0].tab, tab(&harness, 0).number().to_string());

    let claude = tab(&harness, 0).number().to_string();
    let answers = ask(
        &mut harness,
        1,
        Request::Send { to: To::Tab(claude), text: "Rename parse() to read().".to_owned(), kind: Kind::Info },
    );
    let screen = said(&harness);
    assert!(
        screen.contains("Let Antigravity IDE · anti window send messages to Claude Code · claude-sub?"),
        "{screen}"
    );
    harness.click_text("Allow").render();
    assert!(answered(&answers).ok);
    let letter = Letter {
        from: "Antigravity IDE · anti window".to_owned(),
        tab: 2,
        title: "anti window".to_owned(),
        harness: "Antigravity IDE".to_owned(),
        kind: Kind::Info,
        text: "Rename parse() to read().".to_owned(),
    };
    assert_eq!(tab(&harness, 0).letters(), [letter], "the work is waiting in the terminal tab");
}

#[test]
fn by_default_an_agent_sends_to_another_tab_without_asking_and_the_message_lands_in_its_prompt() {
    let scratch = Scratch::new("bridge-no-asking");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let file = scratch.0.join("codex-read.txt");
    harness_in(&mut harness, &scratch, 2, &file, "");
    // The tab has been quiet since it started, so the message goes in as it is taken.
    std::thread::sleep(QUIET + Duration::from_millis(300));

    let answer = answered(&to_codex(&mut harness, "Please write the test for parse()."));
    assert!(answer.ok && answer.text.starts_with("Delivered."), "no question stood in the way: {answer:?}");
    assert!(!said(&harness).contains("send messages to"), "nobody was asked:\n{}", harness.screen());
    assert!(!said(&harness).contains("Allow"), "{}", harness.screen());
    let read = written(&file, "parse()");
    assert!(read.contains("Through QCode, from tab 1 «claude-sub» (Claude Code) of this workspace:"), "{read:?}");
    assert!(read.contains("Please write the test for parse()."), "{read:?}");

    // The answer back goes the same way.
    assert!(answered(&to_claude(&mut harness, "Done.")).ok);
    assert!(!said(&harness).contains("send messages to"), "{}", harness.screen());
    assert_eq!(tab(&harness, 0).letters().len(), 1);
}

/// Moves the switch of the settings row labelled `label`, with the pointer, where it is drawn: at
/// the right edge of the settings column, which the language drop-down's arrow marks. A switch
/// is drawn in colour alone, so there is no glyph of its own to look for.
fn click_switch(harness: &mut Harness<Bridged>, label: &str) {
    let (_, y) = harness.find(label).unwrap_or_else(|| panic!("`{label}` is on screen:\n{}", harness.screen()));
    let (edge, _) = harness.find("▾").expect("the language drop-down marks the column's edge");
    harness.click(edge - 1, y).render();
}

#[test]
fn turning_asking_on_in_the_settings_brings_the_question_back() {
    let scratch = Scratch::new("bridge-ask-setting");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    assert!(!harness.app().0.ask_first(), "QCode starts without asking");

    harness.send(Test::ShowSettings(true)).resize(SIZE.0, 70).render();
    let screen = said(&harness);
    assert!(screen.contains("MESSAGES BETWEEN TABS") && screen.contains("Ask before the first message"), "{screen}");
    click_switch(&mut harness, "Ask before the first message");
    assert!(harness.app().0.ask_first(), "the switch reached the workspace screen:\n{}", harness.screen());
    harness.send(Test::ShowSettings(false)).resize(SIZE.0, SIZE.1).render();

    let answers = to_codex(&mut harness, "Please write the test for parse().");
    assert!(
        said(&harness).contains("Let Claude Code · claude-sub send messages to Codex · codex-main?"),
        "{}",
        harness.screen()
    );
    assert!(answers.try_recv().is_err(), "the agent waits for the person");
    harness.click_text("Allow").render();
    assert!(answered(&answers).ok);
    assert_eq!(tab(&harness, 2).letters().len(), 1);

    // Off again, a pair nobody was asked about goes without a question.
    harness.send(Test::ShowSettings(true)).resize(SIZE.0, 70).render();
    click_switch(&mut harness, "Ask before the first message");
    assert!(!harness.app().0.ask_first());
    harness.send(Test::ShowSettings(false)).resize(SIZE.0, SIZE.1).render();
    assert!(answered(&to_claude(&mut harness, "Done.")).ok);
    assert!(!said(&harness).contains("send messages to"), "{}", harness.screen());
}

#[test]
fn a_pair_the_person_denied_while_asking_was_on_stays_denied_once_it_is_off() {
    let scratch = Scratch::new("bridge-denied-kept");
    let mut harness = two_agents_asking(&scratch, ONLINE, ONLINE);
    let answers = to_codex(&mut harness, "Delete the build folder.");
    harness.click_text("Deny").render();
    assert!(!answered(&answers).ok);

    harness.send(Test::ShowSettings(true)).resize(SIZE.0, 70).render();
    click_switch(&mut harness, "Ask before the first message");
    harness.send(Test::ShowSettings(false)).resize(SIZE.0, SIZE.1).render();
    assert!(!harness.app().0.ask_first());
    let answer = answered(&to_codex(&mut harness, "Please?"));
    assert!(!answer.ok && answer.text.contains("did not allow"), "the person's no still holds: {answer:?}");
    assert!(tab(&harness, 2).letters().is_empty());
}

/// Keys given as one read, the way tmux, a slow link or dictation send them: every one of them
/// at the same moment.
fn burst(text: &str) -> Vec<qframe::event::Event> {
    text.chars()
        .map(|c| match c {
            ' ' => "space".to_owned(),
            '\n' => "enter".to_owned(),
            c => c.to_string(),
        })
        .map(|chord| qframe::event::Event::Key(qframe::event::KeyEvent::press(&chord)))
        .collect()
}

#[test]
fn text_that_arrives_all_at_once_reaches_the_program_in_a_tab_with_every_space_and_return() {
    // In the endurance trial (2026-09-24) text sent into a harness tab through tmux lost its
    // second space of every pair, and a second Return in a row was dropped: the framework took
    // them for a held key. The program in the tab must get every key, however close together.
    let scratch = Scratch::new("burst-typed");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let file = scratch.0.join("typed.txt");
    harness_in(&mut harness, &scratch, 2, &file, "");
    harness.click_text("codex-main").render();
    // Into the terminal, where the person's hand goes.
    harness.click(i32::from(SIZE.0 / 2), i32::from(SIZE.1 / 2)).render();
    harness.events(&burst("one  two   three\n\nfour\n"));
    let read = written(&file, "four");
    assert_eq!(read, "one  two   three\n\nfour\n", "every space and every Return reached the program");
}

#[test]
fn a_question_arrives_under_a_header_naming_its_sender_its_kind_and_how_to_answer() {
    let scratch = Scratch::new("bridge-question");
    let mut harness = two_agents(&scratch, ONLINE, ONLINE);
    let file = scratch.0.join("codex-read.txt");
    harness_in(&mut harness, &scratch, 2, &file, "");
    std::thread::sleep(QUIET + Duration::from_millis(300));

    let codex = tab(&harness, 2).number().to_string();
    let question = Request::Send { to: To::Tab(codex), text: "Which test fails?".to_owned(), kind: Kind::Question };
    let answer = answered(&ask(&mut harness, 0, question));
    assert!(answer.ok && answer.text.starts_with("Delivered."), "{answer:?}");
    let read = written(&file, "Which test fails?");
    let expected = "Through QCode, from tab 1 «claude-sub» (Claude Code) of this workspace: a question, the sender \
                    waits for your answer.\nAnswer with send_message, tab \"1\", kind \"info\".\n\nWhich test fails?";
    assert!(read.starts_with(expected), "{read:?}");
}

/// A workspace whose tab without the network sends to four others: a Codex tab with the network,
/// the window of a desktop profile, and two tabs without the network whose programs are running.
fn five_agents(scratch: &Scratch) -> Harness<Bridged> {
    let profiles = vec![
        Profile { network: OFFLINE, ..profile("claude-sub", HarnessKind::ClaudeCode) },
        Profile { network: ONLINE, ..profile("codex-main", HarnessKind::Codex) },
        Profile { network: OFFLINE, ..profile("anti", HarnessKind::AntigravityIde) },
        Profile { network: OFFLINE, ..profile("kimi", HarnessKind::KimiCode) },
        Profile { network: OFFLINE, ..profile("qwen", HarnessKind::QwenCode) },
    ];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    open(&mut screen, Choice::NewChat("codex-main".to_owned()));
    open(&mut screen, Choice::Window("anti".to_owned()));
    open(&mut screen, Choice::NewChat("kimi".to_owned()));
    open(&mut screen, Choice::NewChat("qwen".to_owned()));
    let mut harness = Harness::with_env(Bridged(screen, None), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Test::Screen(Msg::TogglePanel(false))).send(Test::Screen(Msg::OpenTab(0)));
    harness.advance(Duration::from_secs(1)).render();
    harness
}

#[test]
fn a_message_to_all_goes_to_every_other_tab_by_its_own_rules_and_is_answered_tab_by_tab() {
    let scratch = Scratch::new("bridge-all");
    let mut harness = five_agents(&scratch);
    let (kimi, qwen) = (scratch.0.join("kimi-read.txt"), scratch.0.join("qwen-read.txt"));
    harness_in(&mut harness, &scratch, 3, &kimi, "");
    harness_in(&mut harness, &scratch, 4, &qwen, "");
    std::thread::sleep(QUIET + Duration::from_millis(300));

    let all = Request::Send { to: To::All, text: "I am taking the parser.".to_owned(), kind: Kind::Report };
    let answer = answered(&ask(&mut harness, 0, all));
    assert!(answer.ok, "three of the four took it: {answer:?}");
    let sent = answer.sent.clone().expect("what happened at each tab");
    let at: Vec<(&str, bool)> = sent.iter().map(|outcome| (outcome.tab.as_str(), outcome.ok)).collect();
    assert_eq!(at, [("2", false), ("3", true), ("4", true), ("5", true)], "{}", answer.text);
    assert!(sent[0].text.contains("has no network"), "the network rule, as for a message to it alone: {sent:?}");
    assert!(sent[1].text.contains("check_inbox"), "the window waits for its inbox: {sent:?}");
    assert!(sent[2].text.starts_with("Delivered.") && sent[3].text.starts_with("Delivered."), "{sent:?}");
    // An agent that reads only the words learns the same, tab by tab.
    assert!(answer.text.starts_with("Each of the 4 other agent tabs of this workspace:"), "{}", answer.text);
    for outcome in &sent {
        let line = format!("tab {} «{}»: {}", outcome.tab, outcome.title, outcome.text);
        assert!(answer.text.contains(&line), "{line}\n{}", answer.text);
    }

    assert!(tab(&harness, 1).letters().is_empty(), "nothing reached the tab with the network");
    assert_eq!(tab(&harness, 2).letters().len(), 1, "the window holds it for its agent");
    for file in [&kimi, &qwen] {
        let read = written(file, "the parser");
        assert!(
            read.starts_with("Through QCode, from tab 1 «claude-sub» (Claude Code) of this workspace: a task"),
            "{read:?}"
        );
    }
}

#[test]
fn a_message_to_all_counts_once_against_the_pace() {
    let scratch = Scratch::new("bridge-all-pace");
    let mut harness = five_agents(&scratch);
    let all = || Request::Send { to: To::All, text: "Status?".to_owned(), kind: Kind::Question };
    // Three tabs take each of these; counted per tab, the second would already be refused.
    for round in 1..=MOST_PER_WINDOW {
        let answer = answered(&ask(&mut harness, 0, all()));
        assert!(answer.ok, "message {round}: {answer:?}");
    }
    let answer = answered(&ask(&mut harness, 0, all()));
    assert!(!answer.ok, "{answer:?}");
    // The tab with the network is refused by the network rule first, as it was every time.
    let paced: Vec<String> =
        answer.sent.expect("each tab").into_iter().filter(|o| o.tab != "2").map(|o| o.text).collect();
    assert_eq!(paced.len(), 3);
    assert!(paced.iter().all(|text| text.contains("messages in the last")), "{paced:?}");
}

#[test]
fn a_message_to_all_with_nobody_else_is_refused_and_says_why() {
    let scratch = Scratch::new("bridge-all-alone");
    let profiles = vec![profile("claude-sub", HarnessKind::ClaudeCode)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    let mut harness = Harness::with_env(Bridged(screen, None), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").render();
    let answer =
        answered(&ask(&mut harness, 0, Request::Send { to: To::All, text: "hi".to_owned(), kind: Kind::Info }));
    assert!(!answer.ok && answer.text.contains("No other tab"), "{answer:?}");
}

#[test]
fn with_asking_on_a_message_to_all_says_which_tabs_wait_for_the_person_and_goes_when_allowed() {
    let scratch = Scratch::new("bridge-all-asking");
    let mut harness = two_agents_asking(&scratch, ONLINE, ONLINE);
    let answer =
        answered(&ask(&mut harness, 0, Request::Send { to: To::All, text: "Hello.".to_owned(), kind: Kind::Info }));
    let sent = answer.sent.clone().expect("each tab");
    assert_eq!(sent.len(), 1, "{answer:?}");
    assert!(answer.ok && sent[0].text.contains("has not answered yet"), "answered at once, not held: {answer:?}");
    harness.click_text("Allow").render();
    assert_eq!(tab(&harness, 2).letters().len(), 1, "allowed, the message is taken");
}
