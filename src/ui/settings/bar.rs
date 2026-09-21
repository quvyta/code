//! The repair strip: the band that stands over the whole application while the engine is gone.

use qframe::prelude::*;

use super::engine::{EngineState, Health};
use super::{Msg, Request};

/// Cells between the strip's words and its action.
const GAP: u16 = 2;

/// Draws the strip that stands over the whole application while there is no engine: what is
/// wrong, and the action that puts it right.
///
/// It is one thought and no more. What to do about it, and whatever the engine itself said, are
/// a screen's worth of reading and belong on the settings screen; a band over every screen that
/// grows to four rows in a narrow terminal pushes the application out of its own window.
///
/// It draws nothing while the engine answers, and nothing while the check has not answered yet:
/// a band that flashes before QCode has looked would accuse a machine that is perfectly fine.
/// Put it in the application shell's header, so that it stands over every screen rather than
/// over one of them.
///
/// Repair sends [`Request::OpenEngineStep`], which the application answers by running the setup
/// wizard's engine step on its own. Nothing here runs that step; the strip only asks.
pub fn view(engine: &EngineState, ui: &mut View<'_, Msg>) {
    let Health::Missing(trouble) = engine.health() else {
        return;
    };
    // The mark carries the warning without leaning on colour alone.
    let mark = ui.env().icons().glyph("warning").into_owned();
    ui.add_with(Panel::new().gap(0), |ui| {
        ui.row(|ui| {
            ui.add(Text::rich([
                Span::new(format!("{mark} ")).color("warning"),
                Span::new(trouble.brief(engine.kind())),
            ]))
            .fill_width();
            ui.add(Button::new(t!("settings.repair")).on_press(Msg::Request(Request::OpenEngineStep)))
                .id("engine-repair");
        })
        .fill_width()
        .gap(GAP);
    })
    .fill_width();
}

#[cfg(test)]
mod tests {
    use qframe::icons::GlyphMode;
    use qframe::runtime::Harness;

    use crate::engine::EngineKind;
    use crate::ui::settings::Request;
    use crate::ui::settings::engine::{Health, Trouble};
    use crate::ui::settings::testing::{self, Host};

    /// A terminal wide enough for the strip and the settings underneath.
    const SIZE: (u16, u16) = (100, 38);

    fn host(kind: EngineKind, health: Health, width: u16, height: u16) -> Harness<Host> {
        testing::host(testing::screen(kind, health), width, height)
    }

    #[test]
    fn a_working_engine_leaves_the_top_of_the_application_alone() {
        let harness = host(EngineKind::Podman, Health::Working, SIZE.0, SIZE.1);
        let screen = harness.screen();
        assert!(!screen.contains("Repair"), "{screen}");
        assert!(!screen.contains("is not installed"), "{screen}");
    }

    #[test]
    fn a_check_that_has_not_answered_shouts_nothing() {
        // A strip that flashes before the check is done would accuse a working machine.
        let screen = host(EngineKind::Podman, Health::Checking, SIZE.0, SIZE.1).screen();
        assert!(!screen.contains("Repair"), "{screen}");
    }

    #[test]
    fn a_missing_engine_stands_a_strip_with_a_repair_action_over_the_application() {
        let harness = host(EngineKind::Podman, Health::Missing(Trouble::NotInstalled), SIZE.0, SIZE.1);
        let screen = harness.screen();
        assert!(screen.contains("No container engine"), "{screen}");
        assert!(screen.contains("Repair"), "{screen}");
        let (_, strip) = harness.find("Repair").expect("the repair action is on screen");
        let (_, settings) = harness.find("Container engine").expect("the settings are on screen");
        assert!(strip < settings, "the strip stands over the application:\n{screen}");
    }

    #[test]
    fn a_stopped_daemon_is_told_apart_from_a_missing_engine() {
        let harness = host(EngineKind::Docker, Health::Missing(Trouble::DaemonStopped), SIZE.0, SIZE.1);
        let screen = harness.screen();
        assert!(screen.contains("Docker is installed but not running."), "{screen}");
        assert!(!screen.contains("is not installed"), "{screen}");
    }

    #[test]
    fn the_strip_says_one_thing_and_leaves_the_reading_to_the_screen() {
        // A band over every screen stays one row; the remedy has a screen of its own to live on.
        let harness = host(EngineKind::Docker, Health::Missing(Trouble::DaemonStopped), SIZE.0, SIZE.1);
        let (_, brief) = harness.find("Docker is not running").expect("the strip says what is wrong");
        let (_, sentence) = harness.find("Docker is installed but not running.").expect("the screen says it in full");
        let (_, remedy) = harness.find("Start Docker,").expect("the screen says what to do");
        assert!(brief < sentence && sentence < remedy, "the reading is below the strip:\n{}", harness.screen());
    }

    #[test]
    fn repair_asks_the_application_to_run_the_engine_step() {
        let mut harness = host(EngineKind::Podman, Health::Missing(Trouble::NotInstalled), SIZE.0, SIZE.1);
        harness.click_text("Repair");
        assert_eq!(harness.app().asked, [Request::OpenEngineStep]);
    }

    #[test]
    fn the_keyboard_reaches_repair_first() {
        let mut harness = host(EngineKind::Podman, Health::Missing(Trouble::NotInstalled), SIZE.0, SIZE.1);
        harness.press("tab");
        assert!(harness.is_focused("engine-repair"), "{}", harness.screen());
        harness.press("enter");
        assert_eq!(harness.app().asked, [Request::OpenEngineStep]);
    }

    #[test]
    fn a_narrow_terminal_keeps_the_words_and_the_action() {
        // Nothing may be cut in half here: a strip that loses half its sentence is worse than
        // no strip, because the person cannot tell what it was going to say.
        let harness = host(EngineKind::Podman, Health::Missing(Trouble::NotInstalled), 40, 24);
        let screen = harness.screen();
        assert!(screen.contains("Repair"), "{screen}");
        assert!(screen.contains("No container engine"), "{screen}");
    }

    #[test]
    fn ascii_mode_draws_nothing_but_ascii() {
        let mut harness = host(EngineKind::Podman, Health::Missing(Trouble::NotInstalled), SIZE.0, SIZE.1);
        harness.set_glyph_mode(GlyphMode::Ascii).render();
        let screen = harness.screen();
        assert!(screen.is_ascii(), "{screen}");
        assert!(screen.contains("Repair"), "{screen}");
    }

    #[test]
    fn the_strip_is_neither_bracketed_nor_framed() {
        let mut harness = host(EngineKind::Podman, Health::Missing(Trouble::NotInstalled), SIZE.0, SIZE.1);
        for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
            harness.set_glyph_mode(mode).render();
            let screen = harness.screen();
            for forbidden in ['[', ']', '(', ')', '{', '}', '|', '┌', '─', '│'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` in {mode:?}:\n{screen}");
            }
        }
    }
}
