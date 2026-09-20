//! The conversations a blank tab's page offers, and the conversation a harness tab opens.
//!
//! The engine of these tests cannot be run, so a reading the screen starts answers at once with
//! no conversation at all; the conversations a test needs are handed to the screen in the same
//! message a real reading answers with.

use super::*;

use crate::profile::history::Conversation;
use crate::ui::project::HistoryKey;

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// Noon of today where this machine stands, in milliseconds since the Unix epoch.
///
/// The page writes "today" for a conversation used today, and reads the real clock to know what
/// today is. A test that counted back from the very moment it runs wrote "an hour ago", which is
/// yesterday when the test runs between midnight and one; the page then said "yesterday" and the
/// test failed. Counting back from noon keeps a day's worth of hours on either side inside today.
fn now_ms() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let real = i64::try_from(since.as_millis()).unwrap_or(i64::MAX);
    let offset = i64::from(qframe::date::local_offset_minutes()) * 60_000;
    let noon_here = (real + offset).div_euclid(DAY_MS) * DAY_MS + 12 * HOUR_MS;
    noon_here - offset
}

fn conversation(id: &str, title: Option<&str>, used_ms: i64) -> Conversation {
    Conversation { id: id.to_owned(), title: title.map(str::to_owned), used_ms }
}

/// `count` conversations titled `Chat 1`, `Chat 2`… newest first, an hour apart.
fn chats(count: usize) -> Vec<Conversation> {
    let now = now_ms();
    (1..=count)
        .map(|n| {
            let back = i64::try_from(n).unwrap_or(0) * HOUR_MS;
            conversation(&format!("c-{n}"), Some(&format!("Chat {n}")), now - back)
        })
        .collect()
}

fn shelf_key(profile: &str) -> HistoryKey {
    HistoryKey { project: "firefly".to_owned(), profile: profile.to_owned() }
}

/// The number of the reading the screen last started for `profile` of the open project.
fn generation(screen: &ProjectScreen, profile: &str) -> u64 {
    screen
        .project()
        .and_then(|project| project.history.get(profile))
        .map_or(0, crate::ui::project::history::Shelf::generation)
}

/// Answers the reading under way for `profile` with `answer`, the way the reading itself does.
fn answer(harness: &mut Harness<Screen>, profile: &str, answer: Result<Vec<Conversation>, String>) {
    let generation = generation(&harness.app().0, profile);
    harness.send(Msg::HistoryRead(shelf_key(profile), generation, answer));
}

/// A screen of `one_project` with a blank tab open and its page shown.
fn page(scratch: &Scratch) -> Harness<Screen> {
    let mut harness = harness(one_project(scratch), SIZE.0, SIZE.1);
    harness.send(Msg::NewTab);
    harness
}

/// The row of the screen `needle` is on.
fn row(harness: &Harness<Screen>, needle: &str) -> i32 {
    harness.find(needle).unwrap_or_else(|| panic!("`{needle}` is on the page:\n{}", harness.screen())).1
}

#[test]
fn a_section_shows_the_three_newest_then_the_rest_on_asking() {
    let scratch = Scratch::new("history-rows");
    let mut harness = page(&scratch);
    answer(&mut harness, "claude-sub", Ok(chats(5)));
    let text = harness.screen();
    let (chat, first, second, third) =
        (row(&harness, "New chat"), row(&harness, "Chat 1"), row(&harness, "Chat 2"), row(&harness, "Chat 3"));
    assert_eq!(
        [first, second, third],
        [chat + 1, chat + 2, chat + 3],
        "newest first, right under the new chat:\n{text}"
    );
    assert!(!text.contains("Chat 4"), "only three before asking:\n{text}");
    assert_eq!(row(&harness, "Show all • 2 more"), third + 1, "{text}");

    let (new_chat_x, _) = harness.find("New chat").expect("the new chat");
    let (chat_x, _) = harness.find("Chat 1").expect("a conversation");
    assert_eq!(chat_x, new_chat_x, "a conversation's title starts under the new chat's label:\n{text}");

    harness.click_text("Show all").advance(Duration::from_millis(300));
    let text = harness.screen();
    assert_eq!(row(&harness, "Chat 5"), row(&harness, "Chat 4") + 1, "the section opens in place:\n{text}");
    assert!(!text.contains("Show all"), "and the row that opened it is gone:\n{text}");
    assert_eq!(kinds(&harness.app().0), [TabKind::New], "the tab is still blank");
}

#[test]
fn a_conversation_without_a_title_and_its_last_use_are_written_plainly() {
    let scratch = Scratch::new("history-titles");
    let mut harness = page(&scratch);
    let now = now_ms();
    let found = vec![
        conversation("c-1", None, now),
        conversation("c-2", Some("Tidy the login screen"), now - DAY_MS),
        conversation("c-3", Some("Write the release notes"), now - 10 * DAY_MS),
    ];
    answer(&mut harness, "claude-sub", Ok(found));
    let text = harness.screen();
    let line = |needle: &str| {
        text.lines().nth(usize::try_from(row(&harness, needle)).unwrap_or(0)).unwrap_or_default().to_owned()
    };
    assert!(line("Untitled chat").contains("today "), "{text}");
    assert!(line("Tidy the login screen").contains("yesterday "), "{text}");
    let older = qframe::date::DateTime::from_unix((now - 10 * DAY_MS) / 1000, qframe::date::local_offset_minutes());
    assert!(line("Write the release notes").contains(&older.date.to_string()), "an older one shows its date:\n{text}");
}

#[test]
fn a_profile_with_no_conversation_says_so_and_the_row_does_nothing() {
    let scratch = Scratch::new("history-none");
    let mut harness = page(&scratch);
    // The engine of these tests cannot be run, and a container that cannot be asked after is one
    // that is not there: the page says there is nothing yet, without making anything.
    let text = harness.screen();
    assert!(text.contains("No chats yet"), "{text}");
    let working = harness.fg(
        u16::try_from(harness.find("Shell").expect("the shell").0).unwrap_or(0),
        u16::try_from(row(&harness, "Shell")).unwrap_or(0),
    );
    let (x, y) = harness.find("No chats yet").expect("the row");
    assert_ne!(harness.fg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0)), working, "it is faint");
    harness.click(x, y).advance(Duration::from_millis(300));
    assert_eq!(kinds(&harness.app().0), [TabKind::New], "choosing it opens nothing");
    harness.press("enter");
    assert_eq!(kinds(&harness.app().0), [TabKind::New], "neither by the keyboard");
}

#[test]
fn a_reading_that_fails_says_why_and_the_page_still_works() {
    let scratch = Scratch::new("history-failed");
    let mut harness = page(&scratch);
    answer(&mut harness, "claude-sub", Err("Error: no such container".to_owned()));
    let text = harness.screen();
    let line = text
        .lines()
        .nth(usize::try_from(row(&harness, "The chats could not be read")).unwrap_or(0))
        .unwrap_or_default();
    assert!(line.contains("Error: no such container"), "the reason is on its row:\n{text}");
    harness.click_text("New chat").advance(Duration::from_millis(300));
    assert_eq!(kinds(&harness.app().0), [TabKind::Profile("claude-sub".to_owned())], "{}", harness.screen());
}

#[test]
fn a_quick_reading_is_never_shown() {
    let scratch = Scratch::new("history-quick");
    let mut harness = page(&scratch);
    for _ in 0..4 {
        harness.advance(Duration::from_millis(100));
        assert!(!harness.screen().contains("Reading"), "{}", harness.screen());
    }
}

#[test]
fn a_slow_reading_shows_after_its_delay_and_stays_long_enough_to_be_read() {
    let scratch = Scratch::new("history-slow");
    let mut screen = one_project(&scratch);
    // Opened without its work running, so the reading stays under way for as long as the test
    // says.
    apply(&mut screen, Msg::NewTab);
    let generation = generation(&screen, "claude-sub");
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    assert!(!harness.screen().contains("Reading"), "nothing during the first 300 ms:\n{}", harness.screen());
    assert!(!harness.screen().contains("No chats yet"), "{}", harness.screen());

    harness.send(Msg::HistorySlow(shelf_key("claude-sub"), generation));
    let text = harness.screen();
    assert_eq!(row(&harness, "Reading…"), row(&harness, "New chat") + 1, "then the section says so:\n{text}");

    harness.send(Msg::HistoryRead(shelf_key("claude-sub"), generation, Ok(chats(1))));
    assert!(harness.screen().contains("Reading…"), "an answer right after it waits:\n{}", harness.screen());
    harness.advance(Duration::from_millis(250));
    assert!(harness.screen().contains("Reading…"), "{}", harness.screen());
    harness.advance(Duration::from_millis(300));
    let text = harness.screen();
    assert!(!text.contains("Reading"), "half a second later the answer takes its place:\n{text}");
    assert!(text.contains("Chat 1"), "{text}");
}

#[test]
fn showing_the_page_again_reads_again_and_keeps_the_rows_meanwhile() {
    let scratch = Scratch::new("history-again");
    let mut harness = page(&scratch);
    answer(&mut harness, "claude-sub", Ok(chats(2)));
    let before = generation(&harness.app().0, "claude-sub");
    harness.send(Msg::NewTab);
    let screen = &harness.app().0;
    assert!(generation(screen, "claude-sub") > before, "a page coming into view reads again");
    harness.send(Msg::OpenTab(0));
    let again = generation(&harness.app().0, "claude-sub");
    assert!(again > before + 1, "so does switching back to one");
    harness.send(Msg::HighlightChoice(0));
    assert_eq!(generation(&harness.app().0, "claude-sub"), again, "moving on the page is not showing it again");
}

#[test]
fn the_keyboard_stays_on_its_row_when_a_section_above_fills_in() {
    let scratch = Scratch::new("history-keep");
    let open = OpenProject::new(
        &file("firefly", "Firefly", &["claude-sub", "opencode"]),
        scratch.paths(),
        vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("opencode", HarnessKind::OpenCode)],
    );
    let mut screen = ProjectScreen::new(Some(engine()), HostUser::ImageDefault, vec![open]);
    apply(&mut screen, Msg::NewTab);
    // Shell, gap, heading, new chat, gap, heading, new chat: the second new chat is row 6.
    apply(&mut screen, Msg::HighlightChoice(6));
    let generation = generation(&screen, "claude-sub");
    apply(&mut screen, Msg::HistoryRead(shelf_key("claude-sub"), generation, Ok(chats(5))));
    assert_eq!(screen.blank_row, 10, "three conversations and the row for the rest came in above it");
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    let heading = usize::try_from(row(&harness, "opencode • opencode")).unwrap_or(0);
    let under = text.lines().nth(heading + 1).unwrap_or_default();
    assert!(under.contains('▌') && under.contains("New chat"), "the pillar is still on its new chat:\n{text}");
}

/// The words of the command the tab `key` spawns.
fn words(screen: &ProjectScreen, key: TabKey) -> Vec<String> {
    let command = screen.launch_command(key).expect("a chosen tab has a command");
    assert_eq!(command.program, Path::new(NO_ENGINE), "only the engine binary is ever started");
    spelled(&command)
}

#[test]
fn choosing_a_conversation_resumes_it_inside_the_container() {
    let scratch = Scratch::new("history-resume");
    let mut harness = page(&scratch);
    let id = "2afe99eb-008a-4542-b160-1aa5b29bb95f";
    answer(&mut harness, "claude-sub", Ok(vec![conversation(id, Some("Tidy the login screen"), now_ms())]));
    let tab = key(&harness.app().0, 0);
    harness.click_text("Tidy the login screen").advance(Duration::from_millis(300));
    let screen = &harness.app().0;
    let chosen = &screen.project().expect("a project").tabs()[0];
    assert_eq!((chosen.key(), chosen.kind()), (tab, &TabKind::Profile("claude-sub".to_owned())), "the same tab");
    assert_eq!(chosen.conversation(), Some(id));
    assert_eq!(
        words(screen, tab),
        [
            "exec",
            "--interactive",
            "--tty",
            "qcode-firefly-claude-sub",
            "claude",
            "--dangerously-skip-permissions",
            "--resume",
            id
        ],
        "the harness resumes it, as an argument of the engine"
    );
    let recorded = screen.session();
    assert_eq!(recorded.projects[0].tabs[0].conversation.as_deref(), Some(id), "the session keeps it");
}

#[test]
fn a_codex_conversation_is_resumed_with_its_subcommand() {
    let scratch = Scratch::new("history-codex");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), vec![profile("codex", HarnessKind::Codex)])];
    let mut screen = ProjectScreen::new(Some(engine()), HostUser::ImageDefault, projects);
    let id = "01a0b53a-7904-7862-b8f4-02774d17df35";
    apply(&mut screen, Msg::NewTab);
    let tab = key(&screen, 0);
    apply(&mut screen, Msg::Choose(tab, Choice::Resume("codex".to_owned(), id.to_owned())));
    assert_eq!(
        words(&screen, tab),
        [
            "exec",
            "--interactive",
            "--tty",
            "qcode-firefly-codex",
            "codex",
            "resume",
            "--dangerously-bypass-approvals-and-sandbox",
            id
        ]
    );
    apply(&mut screen, Msg::NewTab);
    let fresh = key(&screen, 1);
    apply(&mut screen, Msg::Choose(fresh, Choice::NewChat("codex".to_owned())));
    assert_eq!(words(&screen, fresh)[4..], ["codex", "--dangerously-bypass-approvals-and-sandbox"], "a new chat");
}

/// A session of `firefly` with two new-chat tabs of `claude-sub`, opened at 1000 and 2000
/// seconds, and a tab that resumes `c-taken`.
fn new_chats() -> Session {
    let tab = |conversation: Option<&str>, opened: u64| SessionTab {
        kind: SessionTabKind::Profile("claude-sub".to_owned()),
        conversation: conversation.map(str::to_owned),
        opened,
    };
    Session {
        active: Some(ProjectId::parse("firefly").expect("an id")),
        projects: vec![SessionProject {
            id: ProjectId::parse("firefly").expect("an id"),
            active_tab: 0,
            tabs: vec![tab(None, 1_000), tab(None, 2_000), tab(Some("c-taken"), 500)],
        }],
    }
}

/// Brings `session` back into a screen of `firefly` and shows its open tab, without the work
/// the screen asks for being run.
fn restore(session: &Session, scratch: &Scratch) -> ProjectScreen {
    let mut screen = one_project(scratch);
    screen.restore_tabs(0, &session.projects[0]);
    drop(super::super::opened(&mut screen));
    screen
}

fn conversation_of(screen: &ProjectScreen, index: usize) -> Option<String> {
    screen.project().expect("a project").tabs()[index].conversation().map(str::to_owned)
}

#[test]
fn a_restored_new_chat_resumes_the_newest_conversation_it_could_have_had() {
    let scratch = Scratch::new("history-restore");
    let mut screen = restore(&new_chats(), &scratch);
    assert!(screen.project().expect("a project").tabs()[0].state().is_starting(), "the open tab woke");
    let found = vec![
        conversation("c-taken", None, 9_000_000),
        conversation("c-new", None, 8_000_000),
        conversation("c-second", None, 3_000_000),
        conversation("c-before", None, 900_000),
    ];
    let first = key(&screen, 0);
    apply(&mut screen, Msg::Woken(first, 0, Ok(()), Some(found.clone())));
    assert_eq!(conversation_of(&screen, 0).as_deref(), Some("c-new"), "the newest one no other tab shows");

    apply(&mut screen, Msg::OpenTab(1));
    let second = key(&screen, 1);
    apply(&mut screen, Msg::Woken(second, 0, Ok(()), Some(found)));
    assert_eq!(conversation_of(&screen, 1).as_deref(), Some("c-second"), "the next one, used after it opened");

    let recorded = screen.session();
    let kept: Vec<Option<&str>> = recorded.projects[0].tabs.iter().map(|tab| tab.conversation.as_deref()).collect();
    assert_eq!(kept, [Some("c-new"), Some("c-second"), Some("c-taken")], "the session keeps them from now on");
}

#[test]
fn a_restored_new_chat_with_nothing_to_resume_starts_a_new_one() {
    let scratch = Scratch::new("history-restore-none");
    let mut screen = restore(&new_chats(), &scratch);
    let first = key(&screen, 0);
    let older = vec![conversation("c-before", None, 900_000), conversation("c-taken", None, 9_000_000)];
    apply(&mut screen, Msg::Woken(first, 0, Ok(()), Some(older)));
    assert_eq!(conversation_of(&screen, 0), None, "nothing used since it opened and not taken");

    let scratch = Scratch::new("history-restore-unread");
    let mut screen = restore(&new_chats(), &scratch);
    let first = key(&screen, 0);
    apply(&mut screen, Msg::Woken(first, 0, Ok(()), None));
    assert_eq!(conversation_of(&screen, 0), None, "a reading that failed starts a new conversation, quietly");
    let state = screen.project().expect("a project").tabs()[0].state().clone();
    assert!(matches!(state, TabState::Failed(_)), "the tab went on to spawn its session: {state:?}");
}

#[test]
fn a_tab_that_resumes_a_conversation_already_is_not_given_another() {
    let scratch = Scratch::new("history-restore-kept");
    let mut session = new_chats();
    session.projects[0].active_tab = 2;
    let mut screen = restore(&session, &scratch);
    let tab = key(&screen, 2);
    apply(&mut screen, Msg::Woken(tab, 0, Ok(()), Some(vec![conversation("c-new", None, 9_000_000)])));
    assert_eq!(conversation_of(&screen, 2).as_deref(), Some("c-taken"));
    assert_eq!(words(&screen, tab)[4..], ["claude", "--dangerously-skip-permissions", "--resume", "c-taken"]);
}

#[test]
fn the_history_has_no_brackets_in_any_glyph_mode_and_reads_as_turkish() {
    let scratch = Scratch::new("history-look");
    let open = OpenProject::new(
        &file("firefly", "Firefly", &["claude-sub", "opencode"]),
        scratch.paths(),
        vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("opencode", HarnessKind::OpenCode)],
    );
    let screen = ProjectScreen::new(Some(engine()), HostUser::ImageDefault, vec![open]);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.send(Msg::NewTab);
    let mut found = chats(4);
    found[1].title = None;
    answer(&mut harness, "claude-sub", Ok(found));
    answer(&mut harness, "opencode", Err("Error: refused".to_owned()));
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        let text = harness.screen();
        for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│'] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {mode:?}:\n{text}");
        }
        assert!(text.contains("Chat 1") && text.contains("Show all"), "{text}");
    }
    let text = harness.screen();
    assert!(text.is_ascii(), "ASCII draws nothing but ASCII:\n{text}");
    assert!(text.contains("Show all - 1 more"), "{text}");

    harness.set_glyph_mode(GlyphMode::Unicode).set_locale("tr").render();
    let text = harness.screen();
    for label in ["Adsız sohbet", "bugün ", "Tümünü göster • 1 sohbet daha", "Sohbetler okunamadı"] {
        assert!(text.contains(label), "`{label}` is missing:\n{text}");
    }
    let scratch = Scratch::new("history-look-none");
    let mut harness = page(&scratch);
    harness.set_locale("tr").render();
    assert!(harness.screen().contains("Henüz sohbet yok"), "{}", harness.screen());
}
