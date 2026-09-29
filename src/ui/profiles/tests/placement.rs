//! Where the wizard stands on the screen and how its choices are marked, and the choices it
//! starts with.

use super::*;

#[test]
fn the_wizard_stands_in_the_middle_of_a_wide_screen() {
    let mut harness = wizard_on(0);
    harness.resize(160, 50).render();
    let (x, y) = harness.find("Harness").expect("the first step is named");
    assert!(x >= 25, "the wizard is centred across, not glued to the left edge:\n{}", harness.screen());
    assert!(y >= 8, "the wizard is pushed down from the top:\n{}", harness.screen());
    let first = (x, y);
    harness.send(Msg::Next).render();
    assert_eq!(harness.find("Harness"), Some(first), "the steps stay where they were from page to page");

    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing {
        profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        diagnostics: Vec::new(),
    }));
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    harness.send(Msg::SignInAsked).resize(160, 50).render();
    let (x, y) = harness.find("Sign claude-sub in").expect("the sign-in page is open");
    assert!(x >= 25 && y >= 8, "the sign-in page is centred too:\n{}", harness.screen());
}

#[test]
fn a_short_terminal_keeps_the_wizard_at_the_top() {
    let mut harness = wizard_on(5);
    harness.resize(SIZE.0, 24).render();
    // Seven steps do not keep their names in a terminal this narrow, so the row of steps shows
    // their markers and the current step's name.
    let (_, y) = harness.find("Image").expect("the current step is named");
    assert!(y <= 1, "no room to spare means no rows given away:\n{}", harness.screen());
}

#[test]
fn the_tallest_page_fits_the_rows_the_wizard_is_placed_by() {
    // The image page without an engine is the tallest; if it grows past the constant the
    // wizard would be placed too low and its buttons pushed towards the bottom edge.
    let mut harness = wizard_on(5);
    harness.resize(PAGE_WIDTH, 60).render();
    let (_, top) = harness.find("Harness").expect("the steps are drawn");
    let (_, bottom) = harness.find("Cancel").expect("the buttons are drawn");
    let rows = u16::try_from(bottom - top + 1).expect("the buttons are under the steps");
    assert!(rows <= WIZARD_ROWS, "{rows} rows:\n{}", harness.screen());
}

#[test]
fn every_choice_in_the_wizard_is_marked_with_the_small_square() {
    let harness = wizard_on(0);
    let square = harness.env().icons().glyph("radio-mark-small").into_owned();
    // The square marks every option, the chosen one included; the style that grows the
    // chosen one into a full box would leave one short.
    assert_eq!(harness.screen().matches(square.as_str()).count(), HarnessKind::ALL.len(), "{}", harness.screen());
    let chosen = radio_mark(&harness, "Claude Code");
    let other = radio_mark(&harness, "Codex");
    assert_ne!(harness.fg(chosen.0, chosen.1), harness.fg(other.0, other.1), "{}", harness.screen());
    let harness = wizard_on(1);
    assert_eq!(harness.screen().matches(square.as_str()).count(), Os::ALL.len(), "{}", harness.screen());
    let harness = wizard_on(2);
    let offered = Template::offered(HarnessKind::ClaudeCode).len();
    assert_eq!(harness.screen().matches(square.as_str()).count(), offered, "{}", harness.screen());
    let harness = wizard_on(4);
    let options = MountAccess::ALL.len() + NetworkMode::ALL.len();
    assert_eq!(harness.screen().matches(square.as_str()).count(), options, "{}", harness.screen());
}

#[test]
fn the_assets_come_writable_and_chosen_from_the_first_frame() {
    let harness = wizard_on(4);
    assert_eq!(harness.app().state.draft().expect("the wizard is open").assets, MountAccess::ReadWrite);
    let writable = radio_mark(&harness, "writable");
    let read_only = radio_mark(&harness, "read-only");
    let full = radio_mark(&harness, "full");
    let tone = |(x, y): (u16, u16)| harness.fg(x, y);
    assert_eq!(tone(writable), tone(full), "writable wears the chosen tone:\n{}", harness.screen());
    assert_ne!(tone(writable), tone(read_only), "{}", harness.screen());
}

#[test]
fn the_templates_carry_the_names_the_owner_chose_in_each_language() {
    let harness = wizard_on(2);
    let screen = harness.screen();
    for name in ["QCode recommended", "QCode extra", "Quvyta development", "Custom"] {
        assert!(screen.contains(name), "`{name}`:\n{screen}");
    }
    assert!(
        !screen.contains("As it comes"),
        "the harness as it comes is every switch off, which the picker reads as Custom:\n{screen}"
    );
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Recommended, "recommended is chosen");
    let mut harness = wizard_on(2);
    harness.set_locale("tr").render();
    let screen = harness.screen();
    for name in ["QCode önerilen", "QCode ekstra", "Quvyta geliştirme", "Özel"] {
        assert!(screen.contains(name), "`{name}`:\n{screen}");
    }
    // The old names are gone from every screen, in every set's page.
    let names = [
        ("en", ["QCode recommended", "QCode extra", "Quvyta development"]),
        ("tr", ["QCode önerilen", "QCode ekstra", "Quvyta geliştirme"]),
    ];
    for (language, rows) in names {
        for row in rows {
            let mut harness = wizard_on(2);
            harness.set_locale(language).render();
            harness.click_text(row).render();
            let screen = harness.screen();
            for word in ["basic", "high", "yüksek"] {
                let found = screen.split(|c: char| !c.is_alphanumeric()).any(|each| each.eq_ignore_ascii_case(word));
                assert!(!found, "{language} {row}: `{word}`:\n{screen}");
            }
        }
    }
}
