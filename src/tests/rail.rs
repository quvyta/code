//! The rail and the foot every screen carries: the way back, the settings and the key that opens
//! the list of keys, and where each of them stands.

use super::*;

#[test]
fn the_foot_of_the_rail_is_a_gap_the_keys_the_settings_a_gap_and_the_way_back() {
    let root = scratch("rail-foot");
    let mut harness = open_workspace(&root);
    let screen = harness.screen();
    let rows: Vec<&str> = screen.lines().collect();
    let icons = harness.env().icons();
    let (settings, arrow) = (icons.glyph("settings").into_owned(), icons.glyph("arrow-left").into_owned());
    // The rail is four cells wide; the rest of each row is the tab and the panel.
    let rail = |row: &str| row.chars().take(4).collect::<String>();
    let foot: Vec<String> = rows[rows.len() - 5..].iter().map(|row| rail(row).trim().to_owned()).collect();
    assert_eq!(foot, ["", "?", settings.as_str(), "", arrow.as_str()], "the foot of the rail:\n{screen}");

    // Each one does what it stands for, pressed where it stands.
    rail_keys(&mut harness);
    assert!(harness.screen().contains("close tab"), "the keys opened:\n{}", harness.screen());
    harness.press("esc").advance(MOMENT);
    rail_settings(&mut harness);
    assert_eq!(harness.app().page(), Page::Settings, "the settings opened:\n{}", harness.screen());
    click_back(&mut harness);
    harness.advance(MOMENT);
    rail_back(&mut harness);
    assert_eq!(harness.app().page(), Page::Workspaces, "the way back led back:\n{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}

/// Opens `count` workspaces of a fresh store at `root`, one after the other, so the rail holds
/// all of them; the first by name is opened last and is the one on screen.
fn open_workspaces(root: &std::path::Path, count: usize, size: (u16, u16)) -> qframe::runtime::Harness<crate::QCode> {
    let _ = std::fs::remove_dir_all(root);
    let store = crate::store::Store::new(root);
    let names: Vec<String> = (0..count).map(|index| format!("Place{index:02}")).collect();
    for name in &names {
        store.create_workspace(name, qframe::date::Date::today_utc()).expect("the store takes a workspace");
    }
    let mut harness = harness(app(config(root, &[]), &settled(), None), size.0, size.1);
    harness.click_text("Workspaces").advance(MOMENT);
    for (index, name) in names.iter().rev().enumerate() {
        harness.click_text(name).advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Workspace, "{name} opened:\n{}", harness.screen());
        if index + 1 < names.len() {
            // The way back, on the last row, leads to the list of workspaces.
            harness.click(1, i32::from(size.1) - 1).advance(MOMENT);
        }
    }
    harness
}

#[test]
fn the_workspaces_take_every_row_above_the_foot_and_the_gaps_are_nobodys() {
    // Heights of every remainder a workspace's four rows can leave, so at one of them a
    // rail that had the gap's row would draw one more workspace there, and a wide one.
    let sizes = (20..28).map(|height| (80, height)).chain([(160, 50)]);
    for size in sizes {
        let root = scratch(&format!("rail-fill-{}x{}", size.0, size.1));
        // More workspaces than the rail has rows for, so every row the rail owns holds one.
        let count = usize::from(size.1 / 3) + 2;
        let mut harness = open_workspaces(&root, count, size);
        assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
        let height = i32::from(size.1);
        let screen = harness.screen();
        let rows: Vec<&str> = screen.lines().collect();
        let rail = |row: &str| row.chars().take(4).collect::<String>();
        // The gap over the foot is empty, and the rail reaches down to it: what is left between
        // its last workspace and the gap is less than one more workspace would need, which is
        // a workspace's three rows and the row between two of them.
        let gap = rows.len() - 5;
        assert_eq!(rail(rows[gap]).trim(), "", "the gap over the foot at {size:?}:\n{screen}");
        let last = (0..gap).rev().find(|&row| !rail(rows[row]).trim().is_empty()).expect("the rail draws workspaces");
        assert!(gap - last - 1 < 4, "the rail leaves {} rows unused at {size:?}:\n{screen}", gap - last - 1);

        // Pressing either empty row does nothing: it opens no other workspace and no control,
        // so the screen stays exactly as it was.
        for up in [4, 1] {
            harness.click(1, height - 1 - up).advance(MOMENT);
            assert_eq!(harness.screen(), screen, "row {up} up changed something at {size:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn every_screen_carries_one_key_hint_and_no_hint_bar() {
    let wizard = harness(
        app(
            Config::parse_str("code.conf", ""),
            &Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown },
            Some(SetupStep::Language),
        ),
        SIZE.0,
        SIZE.1,
    );
    let mut home = harness(app(config(&scratch("hint-bar"), &[]), &settled(), None), SIZE.0, SIZE.1);
    let mut screens = vec![(Page::Setup, last_row(&wizard), wizard.screen())];
    screens.push((Page::Home, last_row(&home), home.screen()));
    for row in ["Workspaces", "Profiles", "Settings"] {
        home.click_text(row).advance(MOMENT);
        screens.push((home.app().page(), last_row(&home), home.screen()));
        click_back(&mut home);
        home.advance(MOMENT);
    }
    let root = scratch("hint-bar-workspace");
    let workspace = open_workspace(&root);
    screens.push((Page::Workspace, last_row(&workspace), workspace.screen()));
    let _ = std::fs::remove_dir_all(&root);

    for (page, last, screen) in screens {
        // The workspace screen keeps its way to the keys at the foot of its rail, where it
        // costs the terminal no row; every other screen has it on the last row.
        match page {
            Page::Workspace => {
                let rows: Vec<&str> = screen.lines().collect();
                let keys = rows[rows.len() - 4].trim();
                assert_eq!(keys, "?", "the way to the keys is in the rail:\n{screen}");
                assert!(!screen.contains("Keys"), "and nowhere else:\n{screen}");
            }
            // The home screen is where every way back leads, and the setup is left by
            // finishing it, so neither has a way back of its own.
            Page::Home | Page::Setup => {
                assert_eq!(last.split_whitespace().collect::<Vec<_>>(), ["?", "Keys"], "{page:?}:\n{screen}");
            }
            // Every other screen has the way back in the other corner of the same row, drawn
            // the same way: the key, then what it does.
            _ => assert_eq!(
                last.split_whitespace().collect::<Vec<_>>(),
                ["esc", "Back", "?", "Keys"],
                "{page:?}:\n{screen}"
            ),
        }
        // The words of the old bars are gone: moving, choosing, quitting and the tab keys.
        for word in ["move", "choose", "quit", "close tab"] {
            assert!(!screen.contains(word), "{page:?} still says `{word}`:\n{screen}");
        }
    }
}

#[test]
fn the_way_back_sits_in_the_bottom_left_corner_and_leads_back_on_a_click_and_on_esc() {
    let mut harness = harness(app(config(&scratch("back-corner"), &[]), &settled(), None), SIZE.0, SIZE.1);
    assert!(harness.find("Back").is_none(), "the home screen has nowhere to go back to:\n{}", harness.screen());
    for leave in ["click", "esc"] {
        harness.click_text("Profiles").advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
        let (x, y) = harness.find("esc").expect("the way back is on screen");
        assert_eq!(y, i32::from(SIZE.1) - 1, "on the last row:\n{}", harness.screen());
        assert!(x <= 3, "at the left edge:\n{}", harness.screen());
        assert_eq!(harness.find("Back").map(|(_, row)| row), Some(y), "with its word beside its key");
        if leave == "click" {
            harness.click(x, y).advance(MOMENT);
        } else {
            harness.press("esc").advance(MOMENT);
        }
        assert_eq!(harness.app().page(), Page::Home, "{leave} led back:\n{}", harness.screen());
    }
}

#[test]
fn the_key_hint_sits_in_the_bottom_right_corner() {
    let harness = harness(app(config(&scratch("hint-corner"), &[]), &settled(), None), SIZE.0, SIZE.1);
    let (x, y) = harness.find("Keys").expect("the hint is on screen");
    assert_eq!(y, i32::from(SIZE.1) - 1, "{}", harness.screen());
    assert!(x + 4 >= i32::from(SIZE.0) - 3, "at the right edge:\n{}", harness.screen());
}

#[test]
fn the_help_key_opens_the_list_with_the_screens_own_keys_and_esc_closes_it() {
    let mut harness = harness(app(config(&scratch("help-key"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Settings").advance(MOMENT);
    harness.press("?");
    let screen = harness.screen();
    assert!(screen.contains("This screen"), "the key list is open:\n{screen}");
    for word in ["move", "open", "leave this screen"] {
        assert!(screen.contains(word), "`{word}` is in the list:\n{screen}");
    }
    harness.press("esc").advance(MOMENT);
    assert!(!harness.screen().contains("This screen"), "Esc closes it:\n{}", harness.screen());
    assert_eq!(harness.app().page(), Page::Settings, "and only it; the screen stays");

    harness.press("f1");
    assert!(harness.screen().contains("This screen"), "F1 opens it too:\n{}", harness.screen());
}

#[test]
fn a_click_on_the_hint_opens_the_list() {
    let mut harness = harness(app(config(&scratch("help-click"), &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Keys");
    let screen = harness.screen();
    assert!(screen.contains("This screen"), "{screen}");
    assert!(screen.contains("move"), "the home screen's keys are listed:\n{screen}");
}

#[test]
fn the_workspace_screens_key_list_says_how_to_leave_the_terminal() {
    let root = scratch("help-workspace");
    let mut harness = open_workspace(&root);
    rail_keys(&mut harness);
    let screen = harness.screen();
    for word in ["tabs", "ctrl w", "close tab", "shift tab", "leave the terminal"] {
        assert!(screen.contains(word), "`{word}` is in the list:\n{screen}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_key_list_names_the_way_from_the_harness_to_the_tabs_in_both_languages() {
    let root = scratch("help-leave");
    let mut harness = open_workspace(&root);
    rail_keys(&mut harness);
    let screen = harness.screen();
    assert!(screen.contains("ctrl alt space"), "{screen}");
    assert!(screen.contains("between the harness and the tabs"), "{screen}");
    harness.set_locale("tr").render();
    assert!(harness.screen().contains("düzenek ile sekmeler arasında geç"), "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn on_the_workspace_screen_the_panel_runs_from_the_first_row_to_the_last() {
    let root = scratch("full-height-panel");
    let harness = open_workspace(&root);
    let screen = harness.screen();
    let (panel_x, _) = harness.find("Panel").expect("the panel is open");
    let arrow = harness.env().icons().glyph("arrow-left").into_owned();
    let (back_x, back_y) = harness.find(&arrow).expect("the way back");
    let (keys_x, keys_y) = harness.find("?").expect("the way to the keys");
    assert_eq!(back_y, i32::from(SIZE.1) - 1, "the way back is on the last row:\n{screen}");
    assert_eq!(keys_y, i32::from(SIZE.1) - 4, "and the keys three rows above it:\n{screen}");
    assert!(back_x < panel_x && keys_x < panel_x, "both in the rail, not over the panel:\n{screen}");
    // The rail is where they are, so the tabs have the first row and the terminal the last.
    assert!(screen.lines().next().is_some_and(|row| row.contains('+')), "the tabs are first:\n{screen}");
    let right = SIZE.0 - 1;
    let panel = harness.bg(right, SIZE.1 / 2);
    assert_eq!(harness.bg(right, 0), panel, "the panel's surface on the first row:\n{screen}");
    assert_eq!(harness.bg(right, SIZE.1 - 1), panel, "and on the last:\n{screen}");
    let _ = std::fs::remove_dir_all(&root);
}
