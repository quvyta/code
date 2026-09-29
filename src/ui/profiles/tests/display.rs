//! The screen under every way it is used and read: the keyboard and the pointer, a narrow
//! terminal, ASCII, reduced motion and Turkish.

use super::*;

#[test]
fn the_keyboard_reaches_the_list_and_moves_the_selection() {
    let mut harness =
        loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("codex-key", HarnessKind::Codex)]);
    harness.press("tab");
    assert!(harness.is_focused("profiles"), "the list takes the first focus:\n{}", harness.screen());
    harness.press("down").render();
    let selected = harness.app().state.selected().expect("a profile is chosen");
    assert_eq!(selected.profile.name.as_str(), "codex-key");
    assert!(harness.screen().contains("Codex"), "{}", harness.screen());
}

#[test]
fn the_pointer_chooses_a_profile_and_hovering_changes_nothing_else() {
    let mut harness =
        loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("codex-key", HarnessKind::Codex)]);
    let (x, y) = harness.find("codex-key").expect("the second profile is on screen");
    harness.hover(x, y).render();
    assert_eq!(
        harness.app().state.selected().expect("a profile is chosen").profile.name.as_str(),
        "claude-sub",
        "hovering shows, it does not choose"
    );
    harness.click(x, y).render();
    assert_eq!(harness.app().state.selected().expect("a profile is chosen").profile.name.as_str(), "codex-key");
}

#[test]
fn a_narrow_screen_keeps_the_profiles_and_their_state() {
    let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    harness.resize(34, 18).render();
    let screen = harness.screen();
    assert!(screen.contains("claude-sub"), "{screen}");
    assert!(screen.contains("Sign out"), "{screen}");
}

#[test]
fn the_wizard_still_works_in_a_narrow_terminal() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New).render();
    harness.resize(40, 20).render();
    let screen = harness.screen();
    assert!(screen.contains("Claude Code"), "{screen}");
    harness.send(Msg::Next).render();
    assert!(harness.screen().contains("Debian 13"), "{}", harness.screen());
    harness.send(Msg::Next).render();
    assert!(harness.screen().contains("recommended"), "{}", harness.screen());
}

#[test]
fn ascii_mode_draws_nothing_but_ascii() {
    let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    harness.set_glyph_mode(GlyphMode::Ascii).render();
    let screen = harness.screen();
    assert!(screen.is_ascii(), "{screen}");
    assert!(screen.contains("claude-sub"), "{screen}");
}

#[test]
fn nothing_is_bracketed_lined_or_framed() {
    let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        // The row that makes a new profile carries the icon set's own mark for adding, which
        // is a plus in two of the modes; it is an icon, not a drawn corner.
        let icons = harness.env().icons();
        let add = icons.glyph("add").into_owned();
        let screen = harness.screen().replace(&format!("{add} New profile"), "New profile");
        for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│', '+'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` in {mode:?}:\n{screen}");
        }
        assert!(!screen.contains("=="), "{screen}");
        assert!(!screen.contains("->"), "{screen}");
    }
}

#[test]
fn reduced_motion_keeps_the_wizard_working() {
    let mut harness = loaded(Vec::new());
    harness.set_reduced_motion(true).send(Msg::New).render();
    harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
    assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
}

#[test]
fn turkish_reads_as_turkish() {
    let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    harness.set_locale("tr").render();
    let screen = harness.screen();
    for text in ["Profiller", "QCode önerilen", "Giriş yap", "Çıkış yap", "abonelik"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    harness.send(Msg::New).render();
    let screen = harness.screen();
    assert!(screen.contains("Düzenek"), "{screen}");
    assert!(screen.contains("kodlama ajanı"), "{screen}");
}
