//! The pages an open workspace draws in its middle when there is no terminal to show: the screen
//! without a workspace or a tab, a blank tab's list, a tab waiting for or refused its container, a
//! missing image and its build, a file that is gone, and a desktop harness's window.
//!
//! On a wide terminal these stand in the middle at the width Settings keeps, as the list pages do,
//! and on a small one they keep every column; a harness's terminal is never one of them and keeps
//! every cell of the middle it is given.

use super::*;

use crate::profile::history::Conversation;
use crate::ui::page;
use crate::ui::workspace::HistoryKey;
use crate::ui::workspace::plan::LaunchFailure;

/// The three sizes a workspace is read at: a small terminal, an ordinary one, and one wide enough
/// for a page to spread across all of it.
const SIZES: [(u16, u16); 3] = [(80, 24), (120, 40), (200, 50)];

/// The desktop profile these pages carry beside the terminal one.
const WINDOW: &str = "anti";

/// A screen with one workspace carrying a terminal harness and a desktop one.
fn two_kinds(scratch: &Scratch, engine: Option<Engine>) -> WorkspaceScreen {
    let workspaces = vec![workspace(
        "firefly",
        "Firefly",
        scratch.paths(),
        vec![profile("claude-sub", HarnessKind::ClaudeCode), profile(WINDOW, HarnessKind::AntigravityIde)],
    )];
    WorkspaceScreen::new(engine, HostUser::Ids { uid: 1000, gid: 1000 }, workspaces)
}

/// The key and run of the open tab.
fn open_tab(screen: &WorkspaceScreen) -> (TabKey, u64) {
    let tab = screen.workspace().and_then(OpenWorkspace::active_tab).expect("a tab is open");
    (tab.key(), tab.run())
}

/// A refusal the engine gave, in its own words.
fn refusal(image_missing: bool) -> LaunchFailure {
    LaunchFailure {
        command: format!("{NO_ENGINE} run --name qcode-firefly-claude-sub qcode/profile/claude-sub"),
        output: "Error: short-name resolution enforced but cannot prompt without a TTY".to_owned(),
        image_missing,
    }
}

/// The conversations the page offers for the terminal profile, newest first.
fn chats() -> Vec<Conversation> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let now = i64::try_from(now.as_millis()).unwrap_or(i64::MAX);
    ["Sensor readings drift after midnight", "Draw the week as a chart", "Tidy the README"]
        .iter()
        .enumerate()
        .map(|(n, title)| Conversation {
            id: format!("c-{n}"),
            title: Some((*title).to_owned()),
            used_ms: now - i64::try_from(n).unwrap_or(0) * 3_600_000,
        })
        .collect()
}

/// Answers the reading of the conversations of `profile` under way on the open workspace.
fn answer(harness: &mut Harness<Screen>, profile: &str, found: Vec<Conversation>) {
    let generation = harness
        .app()
        .0
        .workspace()
        .and_then(|workspace| workspace.history.get(profile))
        .map_or(0, crate::ui::workspace::history::Shelf::generation);
    let key = HistoryKey { workspace: "firefly".to_owned(), profile: profile.to_owned() };
    harness.send(Msg::HistoryRead(key, generation, Ok(found)));
}

/// Every page of an open workspace at `size`, by name, each with the text it is recognised by
/// and the harness it stands in.
fn pages(scratch: &Scratch, size: (u16, u16)) -> Vec<(&'static str, &'static str, Harness<Screen>)> {
    let mut out = Vec::new();

    let none = WorkspaceScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, Vec::new());
    out.push(("none", "No workspace is open", harness(none, size.0, size.1)));

    out.push(("empty", "No tab is open", harness(two_kinds(scratch, Some(engine())), size.0, size.1)));

    let mut blank = harness(two_kinds(scratch, Some(engine())), size.0, size.1);
    blank.click_text("New tab").render();
    answer(&mut blank, "claude-sub", chats());
    blank.render();
    out.push(("blank", "What should this tab open?", blank));

    let mut offline = harness(two_kinds(scratch, None), size.0, size.1);
    offline.click_text("New tab").render();
    out.push(("blank-no-engine", "What should this tab open?", offline));

    let mut refused = harness(two_kinds(scratch, Some(engine())), size.0, size.1);
    open_in(&mut refused, Choice::Shell);
    refused.render();
    out.push(("refused", "The engine refused", refused));

    let mut screen = two_kinds(scratch, Some(engine()));
    open(&mut screen, claude());
    let (key, run) = open_tab(&screen);
    apply(&mut screen, Msg::Ready(key, run, Err(refusal(true))));
    out.push(("no-image", "Build it now", harness(screen, size.0, size.1)));

    let mut screen = two_kinds(scratch, Some(engine()));
    open(&mut screen, claude());
    let (key, run) = open_tab(&screen);
    apply(&mut screen, Msg::Ready(key, run, Err(refusal(true))));
    apply(&mut screen, Msg::BuildImage(key));
    for line in ["STEP 1/4: FROM qcode/base", "STEP 2/4: RUN npm install -g @anthropic-ai/claude-code"] {
        apply(&mut screen, Msg::ImageLine(key, line.to_owned()));
    }
    out.push(("building", "STEP 2/4", harness(screen, size.0, size.1)));

    let mut screen = two_kinds(scratch, Some(engine()));
    open(&mut screen, Choice::Shell);
    let (key, run) = open_tab(&screen);
    apply(&mut screen, Msg::Output(key, run, qframe::widgets::TerminalEvent::Exited(Some(0))));
    apply(&mut screen, Msg::Checked(key, run, false));
    out.push(("stopped", "The container stopped", harness(screen, size.0, size.1)));

    let mut screen = two_kinds(scratch, Some(engine()));
    open(&mut screen, Choice::Window(WINDOW.to_owned()));
    let (key, run) = open_tab(&screen);
    apply(&mut screen, Msg::WindowOpened(key, run, Ok(crate::ui::workspace::Opening::Up)));
    out.push(("desktop", "Bring to front", harness(screen, size.0, size.1)));

    let mut missing = harness(two_kinds(scratch, Some(engine())), size.0, size.1);
    missing.send(Msg::OpenFile("gone.md".to_owned()));
    let (key, run) = open_tab(&missing.app().0);
    missing.send(Msg::Missing(key, run));
    missing.render();
    out.push(("missing-file", "Look again", missing));

    out
}

/// A shell tab whose terminal runs `script`, drawn in `size`.
///
/// A shell of this machine stands in for the container's: what is looked at is the room the
/// terminal is given, which does not depend on what runs in it.
fn running_shell(scratch: &Scratch, size: (u16, u16), script: &str) -> (Harness<Screen>, TerminalSession) {
    let mut screen = two_kinds(scratch, Some(engine()));
    open(&mut screen, Choice::Shell);
    let session = TerminalSession::spawn(std::ffi::OsStr::new("/bin/sh"), &["-c", script], &scratch.0)
        .expect("a pseudo-terminal for the tab");
    let active = screen.active;
    if let Some(tab) = screen.workspaces.get_mut(active).and_then(|workspace| workspace.tabs.last_mut()) {
        tab.attached(session.clone());
    }
    (harness(screen, size.0, size.1), session)
}

/// The leftmost and rightmost columns of `harness`'s screen that carry anything but the rail,
/// the strip and the panel: the middle below the strip, left of the panel.
fn middle_extent(harness: &Harness<Screen>, panel_left: usize) -> (usize, usize) {
    let text = harness.screen();
    let mut left = usize::MAX;
    let mut right = 0;
    // The strip is the first row; the rail is the first few columns.
    for line in text.lines().skip(1) {
        let cells: Vec<char> = line.chars().collect();
        for (column, cell) in cells.iter().enumerate().take(panel_left).skip(super::super::RAIL_WIDTH.into()) {
            if !cell.is_whitespace() {
                left = left.min(column);
                right = right.max(column);
            }
        }
    }
    (left, right)
}

#[test]
fn every_page_stands_in_the_middle_of_a_wide_terminal_at_the_width_settings_keeps() {
    let scratch = Scratch::new("pages-wide");
    let rail = usize::from(super::super::RAIL_WIDTH);
    let mut spread = Vec::new();
    for (name, said, harness) in pages(&scratch, (200, 50)) {
        let text = harness.screen();
        assert!(text.contains(said), "{name}: `{said}` is not on the page:\n{text}");
        let panel = harness.find("Panel").map_or(200, |(x, _)| usize::try_from(x).unwrap_or(200));
        let (left, right) = middle_extent(&harness, panel);
        let page_left = rail + (panel - rail - usize::from(page::WIDTH)) / 2;
        let page_right = page_left + usize::from(page::WIDTH);
        // Standing in the middle means the empty room is on both sides, not only on the right.
        if left < page_left || right >= page_right {
            spread.push(format!("{name}: columns {left}..={right}, outside {page_left}..{page_right}"));
        }
    }
    assert!(spread.is_empty(), "these pages spread beyond the page in the middle:\n{}", spread.join("\n"));
}

#[test]
fn on_a_small_terminal_every_page_keeps_all_it_says() {
    let scratch = Scratch::new("pages-small");
    let rail = usize::from(super::super::RAIL_WIDTH);
    for (name, said, harness) in pages(&scratch, (80, 24)) {
        let text = harness.screen();
        assert!(text.contains(said), "{name}: `{said}` is cut on a small terminal:\n{text}");
        // The page takes every column the middle has, so it starts where the middle does.
        let panel = harness.find("Panel").map_or(80, |(x, _)| usize::try_from(x).unwrap_or(80));
        let (left, _) = middle_extent(&harness, panel);
        assert!(left <= rail + 8 || name == "none" || name == "empty", "{name}: the page starts at {left}:\n{text}");
    }
}

#[test]
fn on_a_small_terminal_a_blank_tab_names_every_choice_beside_its_note() {
    // A row is chosen by its name; the note beside it only explains, so the note gives way first.
    let scratch = Scratch::new("pages-labels");
    let (_, _, blank) =
        pages(&scratch, (80, 24)).into_iter().find(|(name, _, _)| *name == "blank").expect("the blank page");
    let text = blank.screen();
    let shell = text.lines().find(|line| line.contains("in the workspace")).unwrap_or_default();
    assert!(shell.contains("Shell"), "the shell's row lost its name:\n{text}");
}

#[test]
fn a_running_terminal_takes_every_column_of_the_middle_on_a_wide_terminal() {
    let scratch = Scratch::new("pages-terminal");
    // The shell says how wide its terminal is, again and again, so the width it says last is the
    // one the screen gave it.
    let (mut harness, session) = running_shell(
        &scratch,
        (200, 50),
        "while :; do printf '\\033[H\\033[2Kcols=%s\\n' \"$(stty size | cut -d' ' -f2)\"; sleep 0.1; done",
    );
    let rail = i32::from(super::super::RAIL_WIDTH);
    // The pseudo-terminal takes the size the screen drew it at while the session is waited on,
    // which the screen's own watch does in the application.
    let watch = session.watch();
    let started = std::time::Instant::now();
    let columns = loop {
        harness.render();
        let _ = watch.next_change_within(Duration::from_millis(50));
        let text = harness.screen();
        let said = text.lines().find_map(|line| {
            let rest = line.split("cols=").nth(1)?;
            rest.split_whitespace().next()?.parse::<i32>().ok()
        });
        if let Some(columns) = said.filter(|columns| *columns > 80) {
            break columns;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "the shell never said a wide terminal:\n{text}");
    };
    // The rail and the panel keep their cells, and a line of the frame may keep one on either
    // side; every other column is the terminal's.
    let (panel, _) = harness.find("Panel").expect("the panel is open");
    assert!(
        columns >= panel - rail - 3 && columns > i32::from(page::WIDTH) + 40,
        "the terminal was given {columns} columns between the rail and the panel at {panel}:\n{}",
        harness.screen()
    );
}

/// Writes every page at every size into the folder `QCODE_SCREEN_DUMP` names.
#[test]
#[ignore = "writes the pages of a workspace: QCODE_SCREEN_DUMP=<folder> cargo test --lib pages::dump -- --ignored"]
fn dump() {
    let Ok(dir) = std::env::var("QCODE_SCREEN_DUMP") else { return };
    let dir = PathBuf::from(dir);
    fs::create_dir_all(&dir).expect("the dumps' folder");
    let scratch = Scratch::new("pages-dump");
    for size in SIZES {
        for (name, _, harness) in pages(&scratch, size) {
            fs::write(dir.join(format!("workspace-{name}-{}.txt", size.0)), harness.screen()).expect("written");
        }
        let (shell, _) = running_shell(&scratch, size, "printf 'a shell in the container\\n'; sleep 30");
        fs::write(dir.join(format!("workspace-terminal-{}.txt", size.0)), shell.screen()).expect("written");
    }
}
