//! Which tab is which: every tab has an id of its own inside its workspace, a tab's agent learns
//! its own id and the others' only from QCode, and the person can give a tab a name of their own.
//!
//! The calls a workspace's socket would carry are handed to the screen the way its listener hands
//! them over, each on the socket of the workspace it names; no socket is opened here.

use std::sync::mpsc::Receiver;

use super::*;

use crate::bridge::protocol::{Answer, Kind, Question, Request, To};
use crate::bridge::socket::Call;
use crate::profile::history::Conversation;
use crate::ui::workspace::{HistoryKey, shared};

/// The workspace screen with the keys of the application, the keymap's actions reaching the screen
/// the way QCode hands them over, and one more way in: a call as a workspace's socket brings it.
struct Named(WorkspaceScreen);

#[derive(Debug, Clone)]
enum Test {
    Screen(Msg),
    /// A call on the socket of the workspace of this id.
    Call(&'static str, Call),
}

impl App for Named {
    type Msg = Test;

    fn update(&mut self, message: Test) -> Command<Test> {
        match message {
            Test::Screen(message) => super::super::update(&mut self.0, message).map(Test::Screen),
            Test::Call(id, call) => {
                super::super::bridge::answer(&mut self.0, id, call, std::time::Instant::now()).map(Test::Screen)
            }
        }
    }

    fn view(&self, ui: &mut View<'_, Test>) {
        super::super::view(&self.0, ui, Test::Screen, |_| {}, |_| {}, |_| {});
    }

    fn action(&self, name: &str) -> Option<Test> {
        super::super::action(name).map(Test::Screen)
    }
}

/// A harness of [`Named`] showing `screen`, its panel closed so that the strip and a dialog read
/// alone.
fn named(screen: WorkspaceScreen) -> Harness<Named> {
    let mut harness = Harness::with_env(Named(screen), env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Test::Screen(Msg::OpenWorkspace(0))).send(Test::Screen(Msg::TogglePanel(false))).render();
    harness
}

/// The open workspace of `harness`.
fn shown(harness: &Harness<Named>) -> &OpenWorkspace {
    harness.app().0.workspace().expect("a workspace is open")
}

/// The first line of the screen, where the tab strip is.
fn strip(harness: &Harness<Named>) -> String {
    harness.screen().lines().next().unwrap_or_default().to_owned()
}

/// Clicks the `×` of the tab titled `title` on the strip, the way the person closes it.
fn click_close(harness: &mut Harness<Named>, title: &str) {
    let line: Vec<char> = strip(harness).chars().collect();
    let label: Vec<char> = title.chars().collect();
    let at = line.windows(label.len()).position(|cells| cells == label.as_slice()).expect("the tab is on the strip");
    let mark = (at + label.len()..line.len()).find(|&x| line[x] == '×').expect("the tab has a close mark");
    harness.click(i32::try_from(mark).expect("a column"), 0).render();
}

/// Right-clicks the tab titled `title` on the strip, which opens its menu.
fn right_click(harness: &mut Harness<Named>, title: &str) {
    let (x, y) = harness.find(title).unwrap_or_else(|| panic!("`{title}` is on screen:\n{}", harness.screen()));
    harness.mouse(MouseKind::Down(MouseButton::Right), x + 1, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x + 1, y);
    harness.render();
}

/// Asks, from the tab at `index` of the open workspace, `request` on Firefly's socket.
fn ask_from(harness: &mut Harness<Named>, index: usize, request: Request) -> Answer {
    let token = shown(harness).tabs()[index].token().to_owned();
    let (call, answers) = Call::new(Ok(Question { token, session: None, request }));
    harness.send(Test::Call("firefly", call)).render();
    answer(&answers)
}

/// The profiles both workspaces offer: two whose tabs each run a harness of their own, and two
/// opencode profiles of a QCode template, whose tabs share one server per profile.
fn profiles() -> Vec<Profile> {
    vec![
        profile("claude-sub", HarnessKind::ClaudeCode),
        profile("codex-main", HarnessKind::Codex),
        profile("opencode", HarnessKind::OpenCode),
        profile("opencode-two", HarnessKind::OpenCode),
    ]
}

/// Firefly with five agent tabs and Moth with four, plain and shared ones mixed, every shared tab
/// showing a conversation of its own.
fn two_workspaces(firefly: &Scratch, moth: &Scratch) -> WorkspaceScreen {
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![
            workspace("firefly", "Firefly", firefly.paths(), profiles()),
            workspace("moth", "Moth", moth.paths(), profiles()),
        ],
    );
    let opened: [(usize, &[&str]); 2] = [
        (0, &["claude-sub", "opencode", "codex-main", "opencode", "opencode-two"]),
        (1, &["opencode", "claude-sub", "opencode", "codex-main"]),
    ];
    for (index, names) in opened {
        apply(&mut screen, Msg::OpenWorkspace(index));
        for name in names {
            open(&mut screen, Choice::NewChat((*name).to_owned()));
        }
        // What each shared tab's server chose for it, the way the tab's start hands it over.
        let shown: Vec<(TabKey, u64, String)> = screen.workspaces()[index]
            .tabs()
            .iter()
            .filter(|tab| matches!(tab.kind(), TabKind::Profile(name) if name.starts_with("opencode")))
            .map(|tab| (tab.key(), tab.run(), format!("ses_{index}_{}", tab.number())))
            .collect();
        for (key, run, conversation) in shown {
            apply(&mut screen, Msg::Woken(key, run, Ok(()), None, Some(conversation)));
        }
    }
    screen
}

/// What an agent in `tab` hands the bridge to say which tab it is: its own token, or its shared
/// server's token and the conversation it shows.
fn credentials(screen: &WorkspaceScreen, tab: &Tab) -> (String, Option<String>) {
    let serving = screen
        .launch_command(tab.key())
        .and_then(|command| envs(&command).remove(shared::SERVER_VARIABLE))
        .filter(|_| tab.kind().profile().is_some_and(|name| name.starts_with("opencode")));
    match serving {
        Some(server) => (server, tab.conversation().map(str::to_owned)),
        None => (tab.token().to_owned(), None),
    }
}

/// Asks the screen `request` on the socket of the workspace `id`.
fn ask(
    screen: &mut WorkspaceScreen,
    id: &str,
    (token, session): (String, Option<String>),
    request: Request,
) -> Receiver<Answer> {
    let (call, answers) = Call::new(Ok(Question { token, session, request }));
    drop(super::super::bridge::answer(screen, id, call, std::time::Instant::now()));
    answers
}

fn answer(answers: &Receiver<Answer>) -> Answer {
    answers.try_recv().expect("the call was answered")
}

const IDS: [&str; 2] = ["firefly", "moth"];

#[test]
fn every_tab_of_two_workspaces_learns_its_own_id_and_lists_only_its_own_workspace() {
    let (firefly, moth) = (Scratch::new("identity-a"), Scratch::new("identity-b"));
    let mut screen = two_workspaces(&firefly, &moth);
    for (index, (id, name)) in IDS.into_iter().zip(["Firefly", "Moth"]).enumerate() {
        let numbers: Vec<u32> = screen.workspaces()[index].tabs().iter().map(Tab::number).collect();
        let count = u32::try_from(numbers.len()).expect("a few tabs");
        assert_eq!(numbers, (1..=count).collect::<Vec<_>>(), "{id} counts its own tabs from one");
        let asking: Vec<(u32, String, (String, Option<String>))> = screen.workspaces()[index]
            .tabs()
            .iter()
            .enumerate()
            .map(|(place, tab)| (tab.number(), screen.workspaces()[index].tab_label(place), credentials(&screen, tab)))
            .collect();
        for (number, title, credentials) in asking {
            let listed = answer(&ask(&mut screen, id, credentials, Request::List));
            assert!(listed.ok, "{id} tab {number}: {listed:?}");
            let you = listed.you.clone().expect("the asking tab");
            assert_eq!((you.tab.as_str(), you.workspace.as_str()), (number.to_string().as_str(), name), "{listed:?}");
            assert_eq!(you.title, title);
            let first = listed.text.lines().next().unwrap_or_default();
            let says = t!("bridge.answer.you", tab = number.to_string(), title = title.as_str(), workspace = name);
            assert_eq!(first, says, "the words say it before the fields do");
            let mut others: Vec<String> = listed.tabs.expect("the others").into_iter().map(|tab| tab.tab).collect();
            others.sort();
            let expected: Vec<String> = (1..=count).filter(|&other| other != number).map(|n| n.to_string()).collect();
            assert_eq!(others, expected, "{id} tab {number} lists exactly the other tabs of its workspace");
        }
    }
}

#[test]
fn a_tab_of_one_workspace_is_nobody_on_the_socket_of_another() {
    let (firefly, moth) = (Scratch::new("identity-cross-a"), Scratch::new("identity-cross-b"));
    let mut screen = two_workspaces(&firefly, &moth);
    let stranger = t!("bridge.answer.stranger");
    for (index, other) in [(0, "moth"), (1, "firefly")] {
        let asking: Vec<(String, Option<String>)> =
            screen.workspaces()[index].tabs().iter().map(|tab| credentials(&screen, tab)).collect();
        for credentials in asking {
            let listed = answer(&ask(&mut screen, other, credentials.clone(), Request::List));
            assert_eq!((listed.ok, listed.text.as_str()), (false, stranger.as_str()), "{credentials:?} on {other}");
        }
    }
}

#[test]
fn the_tabs_of_one_shared_server_each_resolve_to_themselves_and_never_to_their_twin() {
    let (firefly, moth) = (Scratch::new("identity-twins-a"), Scratch::new("identity-twins-b"));
    let mut screen = two_workspaces(&firefly, &moth);
    // Tabs 2 and 4 of Firefly are both of the profile `opencode`, attached to one server.
    let tabs = screen.workspaces()[0].tabs();
    let (second, fourth) = (credentials(&screen, &tabs[1]), credentials(&screen, &tabs[3]));
    assert_eq!(second.0, fourth.0, "one server speaks for both");
    assert_ne!(second.1, fourth.1, "each shows a conversation of its own");
    for (credentials, number) in [(second, "2"), (fourth, "4")] {
        let you = answer(&ask(&mut screen, "firefly", credentials, Request::List)).you.expect("the asking tab");
        assert_eq!(you.tab, number);
    }
}

#[test]
fn a_closed_tabs_id_is_never_given_to_another() {
    let scratch = Scratch::new("identity-closed");
    let mut screen = one_workspace(&scratch);
    for _ in 0..3 {
        open(&mut screen, claude());
    }
    let mut harness = named(screen);
    click_close(&mut harness, "claude-sub 3");
    let numbers = |harness: &Harness<Named>| -> Vec<u32> { shown(harness).tabs().iter().map(Tab::number).collect() };
    assert_eq!(numbers(&harness), [1, 2], "the third tab was closed:\n{}", harness.screen());
    harness.press("ctrl+t").render();
    harness.click_text("New chat").render();
    assert_eq!(numbers(&harness), [1, 2, 4], "the new tab is the fourth, never the third again:\n{}", harness.screen());
    let listed = ask_from(&mut harness, 0, Request::List);
    let listed: Vec<String> = listed.tabs.expect("the others").into_iter().map(|tab| tab.tab).collect();
    assert_eq!(listed, ["2", "4"]);
}

/// Firefly with a Claude Code tab and a Codex tab, the Codex one open.
fn claude_and_codex(scratch: &Scratch) -> Harness<Named> {
    let profiles = vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("codex-main", HarnessKind::Codex)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    open(&mut screen, claude());
    open(&mut screen, Choice::NewChat("codex-main".to_owned()));
    named(screen)
}

#[test]
fn a_tab_renamed_from_its_menu_reads_the_new_name_on_the_strip_in_the_list_and_on_what_it_sends() {
    let scratch = Scratch::new("identity-rename-menu");
    let mut harness = claude_and_codex(&scratch);
    right_click(&mut harness, "codex-main");
    harness.click_text("Rename").render();
    assert!(harness.screen().contains("Rename codex-main"), "the dialog names the tab:\n{}", harness.screen());
    // The name is selected, so what is typed replaces it.
    harness.type_text("reviewer").press("enter").render();
    assert!(strip(&harness).contains("reviewer"), "{}", harness.screen());
    assert!(!strip(&harness).contains("codex-main"), "{}", harness.screen());
    assert_eq!(shown(&harness).tabs()[1].name(), Some("reviewer"));

    let listed = ask_from(&mut harness, 0, Request::List);
    let tabs = listed.tabs.expect("the others");
    assert_eq!((tabs[0].tab.as_str(), tabs[0].title.as_str()), ("2", "reviewer"), "{}", listed.text);
    assert_eq!(ask_from(&mut harness, 1, Request::List).you.expect("the asking tab").title, "reviewer");

    let sent = Request::Send { to: To::Tab("1".to_owned()), text: "Look at parse().".to_owned(), kind: Kind::Question };
    assert!(ask_from(&mut harness, 1, sent).ok);
    let letter = shown(&harness).tabs()[0].letters()[0].clone();
    // The header the message is typed in under is made of these; `bridge.rs` reads it whole.
    assert_eq!((letter.tab, letter.title.as_str(), letter.kind), (2, "reviewer", Kind::Question));
}

#[test]
fn f2_renames_the_open_tab_esc_leaves_it_and_an_empty_name_gives_the_automatic_one_back() {
    let scratch = Scratch::new("identity-rename-key");
    let mut harness = claude_and_codex(&scratch);
    harness.press("f2").render();
    assert!(harness.screen().contains("Rename codex-main"), "{}", harness.screen());
    harness.type_text("tester").press("enter").render();
    assert!(strip(&harness).contains("tester"), "{}", harness.screen());

    harness.press("f2").render();
    harness.type_text("never kept").press("esc").render();
    assert!(!harness.screen().contains("Rename tester"), "Esc closes the dialog:\n{}", harness.screen());
    assert_eq!(shown(&harness).tabs()[1].name(), Some("tester"), "and keeps the name it had");

    harness.press("f2").render();
    harness.press("backspace").press("enter").render();
    assert_eq!(shown(&harness).tabs()[1].name(), None);
    assert!(strip(&harness).contains("codex-main"), "the automatic name is back:\n{}", harness.screen());
}

#[test]
fn enter_on_an_automatic_name_leaves_it_automatic() {
    let scratch = Scratch::new("identity-rename-same");
    let mut harness = claude_and_codex(&scratch);
    harness.press("f2").render();
    harness.press("enter").render();
    assert_eq!(shown(&harness).tabs()[1].name(), None, "nothing was named, so the tab still follows its conversation");
}

#[test]
fn tabs_of_one_profile_read_their_ids_and_never_all_the_same_name() {
    let scratch = Scratch::new("identity-alike");
    let profiles = vec![profile("opencode", HarnessKind::OpenCode), profile("claude-sub", HarnessKind::ClaudeCode)];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    for name in ["opencode", "claude-sub", "opencode", "opencode"] {
        open(&mut screen, Choice::NewChat(name.to_owned()));
    }
    let mut harness = named(screen);
    harness.resize(160, SIZE.1).render();
    let line = strip(&harness);
    for label in ["opencode 1", "claude-sub", "opencode 3", "opencode 4"] {
        assert!(line.contains(label), "{label}: {line}");
    }
    assert!(!line.contains("claude-sub 2"), "a tab that reads like no other keeps its plain name: {line}");
    let titles: Vec<String> =
        ask_from(&mut harness, 1, Request::List).tabs.expect("the others").into_iter().map(|tab| tab.title).collect();
    assert_eq!(titles, ["opencode 1", "opencode 3", "opencode 4"], "the agents read the same names");
}

/// Hands the page of the blank tab the conversations of `claude-sub`, as the reading answers.
fn conversations(harness: &mut Harness<Named>, found: Vec<Conversation>) {
    let generation = harness.app().0.reading("claude-sub");
    let key = HistoryKey { workspace: "firefly".to_owned(), profile: "claude-sub".to_owned() };
    harness.send(Test::Screen(Msg::HistoryRead(key, generation, Ok(found)))).render();
}

#[test]
fn a_tab_takes_its_conversations_title_but_the_persons_name_always_wins() {
    let scratch = Scratch::new("identity-title");
    let mut harness = named(one_workspace(&scratch));
    harness.press("ctrl+t").render();
    let title = "Fix the parser\nso that it reads the whole file";
    conversations(&mut harness, vec![Conversation { id: "c-7".to_owned(), title: Some(title.to_owned()), used_ms: 1 }]);
    harness.click_text("Fix the parser").render();
    assert_eq!(shown(&harness).tabs()[0].conversation(), Some("c-7"), "{}", harness.screen());
    assert!(
        strip(&harness).contains("Fix the parser"),
        "the conversation's first line names the tab:\n{}",
        harness.screen()
    );

    harness.press("f2").render();
    harness.type_text("mine").press("enter").render();
    assert!(strip(&harness).contains("mine"), "{}", harness.screen());
    // The page reads the conversations again whenever it is shown; a title it brings back does
    // not take the tab from the person.
    conversations(&mut harness, vec![Conversation { id: "c-7".to_owned(), title: Some(title.to_owned()), used_ms: 2 }]);
    assert!(!strip(&harness).contains("Fix the parser"), "{}", harness.screen());
    assert_eq!(ask_from(&mut harness, 0, Request::List).you.expect("the asking tab").title, "mine");

    harness.press("f2").render();
    harness.press("backspace").press("enter").render();
    assert!(strip(&harness).contains("Fix the parser"), "a cleared name gives the title back:\n{}", harness.screen());
}

#[test]
fn a_tab_brought_back_keeps_its_id_and_its_name_and_a_new_one_comes_after_them() {
    let scratch = Scratch::new("identity-restored");
    let mut screen = one_workspace(&scratch);
    let tab = |number: u32, name: Option<&str>, opened: u64| SessionTab {
        kind: SessionTabKind::Profile("claude-sub".to_owned()),
        conversation: None,
        opened,
        number: Some(number),
        name: name.map(str::to_owned),
    };
    let record = SessionWorkspace {
        id: WorkspaceId::parse("firefly").expect("a usable id"),
        active_tab: 0,
        // The third names an id the first already has, which a hand-edited file could.
        tabs: vec![tab(4, None, 1), tab(9, Some("reviewer"), 2), tab(4, None, 3)],
    };
    screen.restore_tabs(0, &record);
    let mut harness = named(screen);
    let numbers = |harness: &Harness<Named>| -> Vec<u32> { shown(harness).tabs().iter().map(Tab::number).collect() };
    assert_eq!(numbers(&harness), [4, 9, 10], "the ids come back, and a repeated one is given a new one");
    assert!(strip(&harness).contains("reviewer"), "{}", harness.screen());
    harness.press("ctrl+t").render();
    assert_eq!(numbers(&harness), [4, 9, 10, 11]);
    assert_eq!(ask_from(&mut harness, 1, Request::List).you.expect("the asking tab").tab, "9");
}
