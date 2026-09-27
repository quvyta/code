//! Closing a tab from the strip: a harness whose agent is still running is ended only once the
//! person said so, whichever way the tab was closed, and a tab with no work to lose closes at once.
//!
//! The harness is a program on this machine standing in for the one in the container, and the
//! engine is a script that writes down every call, so what closing asks of the container is read
//! from that list.

use super::*;

/// A workspace with a Claude Code tab whose program is running and a shell tab beside it, on an
/// engine that writes every call into the file returned beside the harness. The harness tab is
/// the open one.
fn working(scratch: &Scratch) -> (Harness<Screen>, PathBuf, TerminalSession) {
    let (engine, calls) = recording_engine(scratch);
    let workspaces =
        vec![workspace("firefly", "Firefly", scratch.paths(), vec![profile("claude-sub", HarnessKind::ClaudeCode)])];
    let mut screen = WorkspaceScreen::new(Some(engine), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces);
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, claude());
    open(&mut screen, Choice::Shell);
    let session = TerminalSession::spawn("/bin/sh".as_ref(), &["-c", "sleep 60"], Path::new("/"))
        .expect("a program for the harness tab");
    screen.attach(key(&screen, 0), session.clone());
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    click_tab(&mut harness, "claude-sub");
    harness.advance(Duration::from_secs(1)).render();
    (harness, calls, session)
}

/// Clicks the label of the tab titled `title` on the strip.
fn click_tab(harness: &mut Harness<Screen>, title: &str) {
    let at = strip(harness).find(title).expect("the tab is on the strip");
    let column = strip(harness)[..at].chars().count();
    harness.click(i32::try_from(column).expect("a column"), 0).render();
}

/// Clicks the close mark of the tab titled `title` on the strip, where the person's hand is.
fn click_close(harness: &mut Harness<Screen>, title: &str) {
    let line: Vec<char> = strip(harness).chars().collect();
    let label: Vec<char> = title.chars().collect();
    let at = line.windows(label.len()).position(|cells| cells == label.as_slice()).expect("the tab is on the strip");
    let mark = (at + label.len()..line.len()).find(|&x| line[x] == '×').expect("the tab has a close mark");
    harness.click(i32::try_from(mark).expect("a column"), 0).render();
}

/// Moves the keyboard onto the tab strip the way the person does: out of a harness with the key
/// that leaves it, since a click on a tab leaves the keyboard in its terminal, which takes Tab as
/// its own; and elsewhere with Tab.
fn focus_strip(harness: &mut Harness<Screen>) {
    if harness.is_focused("workspace-terminal") {
        harness.press("ctrl+alt+space");
    }
    for _ in 0..16 {
        if harness.is_focused("workspace-tabs") {
            return;
        }
        harness.press("tab");
    }
    panic!("Tab never reaches the tab strip:\n{}", harness.screen());
}

/// The screen as one line of words, so a sentence a dialog wrapped reads whole.
fn said(harness: &Harness<Screen>) -> String {
    harness.screen().replace('▌', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The calls of the engine that ended something inside a container.
fn endings(calls: &Path) -> Vec<String> {
    let written = std::fs::read_to_string(calls).unwrap_or_default();
    written.lines().filter(|call| call.contains("kill -TERM")).map(str::to_owned).collect()
}

fn tabs(harness: &Harness<Screen>) -> Vec<TabKind> {
    kinds(&harness.app().0)
}

fn harness_state(harness: &Harness<Screen>) -> TabState {
    harness.app().0.workspace().expect("a workspace is open").tabs()[0].state().clone()
}

#[test]
fn the_close_mark_of_a_running_harness_asks_before_its_agent_is_ended() {
    let scratch = Scratch::new("close-ask-mark");
    let (mut harness, calls, session) = working(&scratch);

    click_close(&mut harness, "claude-sub");
    harness.advance(Duration::from_secs(2)).render();
    let screen = said(&harness);
    assert!(screen.contains("End Claude Code in this tab?"), "{screen}");
    assert!(screen.contains("The agent is still running"), "{screen}");
    assert_eq!(tabs(&harness).len(), 2, "nothing is closed while the question stands");
    assert!(endings(&calls).is_empty(), "and nothing is ended: {:?}", endings(&calls));

    harness.click_text("Keep it running").advance(Duration::from_secs(2)).render();
    assert_eq!(tabs(&harness).len(), 2, "kept, the tab stays");
    assert_eq!(harness_state(&harness), TabState::Running, "with its agent running");
    assert!(endings(&calls).is_empty(), "{:?}", endings(&calls));

    click_close(&mut harness, "claude-sub");
    harness.click_text("End and close").advance(Duration::from_secs(2)).render();
    assert_eq!(tabs(&harness), [TabKind::Shell], "ended, the tab is gone:\n{}", harness.screen());
    session.kill();
}

#[test]
fn ctrl_w_on_a_running_harness_asks_before_its_agent_is_ended() {
    let scratch = Scratch::new("close-ask-key");
    let (mut harness, calls, session) = working(&scratch);

    focus_strip(&mut harness);
    harness.press("ctrl+w").advance(Duration::from_secs(2)).render();
    let screen = said(&harness);
    assert!(screen.contains("End Claude Code in this tab?"), "{screen}");
    assert_eq!(tabs(&harness).len(), 2, "nothing is closed while the question stands");
    assert!(endings(&calls).is_empty(), "and nothing is ended: {:?}", endings(&calls));

    harness.click_text("Keep it running").advance(Duration::from_secs(2)).render();
    assert_eq!(tabs(&harness).len(), 2, "kept, the tab stays");
    assert_eq!(harness_state(&harness), TabState::Running, "with its agent running");

    focus_strip(&mut harness);
    harness.press("ctrl+w").render();
    harness.click_text("End and close").advance(Duration::from_secs(2)).render();
    assert_eq!(tabs(&harness), [TabKind::Shell], "ended, the tab is gone:\n{}", harness.screen());
    session.kill();
}

#[test]
fn a_shell_tab_and_a_harness_that_already_exited_close_without_a_question() {
    let scratch = Scratch::new("close-no-ask");
    let (mut harness, _calls, session) = working(&scratch);
    session.kill();
    // The program's end, as the tab's watch on its terminal brings it.
    let key = key(&harness.app().0, 0);
    let run = harness.app().0.workspace().expect("a workspace is open").tabs()[0].run();
    harness.send(Msg::Output(key, run, TerminalEvent::Exited(Some(0)))).advance(Duration::from_secs(1)).render();
    assert!(!matches!(harness_state(&harness), TabState::Running), "the harness is over");

    click_close(&mut harness, "Shell");
    assert!(!said(&harness).contains("End "), "{}", harness.screen());
    assert_eq!(tabs(&harness), [TabKind::Profile("claude-sub".to_owned())], "the shell closes at once");

    click_close(&mut harness, "claude-sub");
    assert!(!said(&harness).contains("End Claude Code"), "{}", harness.screen());
    assert!(tabs(&harness).is_empty(), "so does a harness with nothing left running");
}

#[test]
fn ending_a_running_harness_from_its_close_mark_ends_the_agent_inside_its_container() {
    // The engine's command attached to the tab's terminal is only the outside of the harness: what
    // it started inside the container goes on unless the tab's own ending reaches it.
    let scratch = Scratch::new("close-ends-agent");
    let (mut harness, calls, session) = working(&scratch);
    let expected = {
        let screen = &harness.app().0;
        let workspace = screen.workspace().expect("a workspace is open");
        let tab = &workspace.tabs()[0];
        let plan = workspace.plan(tab.kind()).expect("the harness tab has a container");
        let command = plan.end_tab(screen.engine().expect("an engine"), tab.token());
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    };
    assert!(expected.contains("qcode-firefly-claude-sub"), "the tab's own container: {expected}");

    click_close(&mut harness, "claude-sub");
    harness.click_text("End and close").advance(Duration::from_secs(2)).render();
    assert_eq!(endings(&calls), [expected], "one ending, the tab's, in the tab's container");
    session.kill();
}
