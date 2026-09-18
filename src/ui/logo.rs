//! The QCode wordmark, drawn from block elements.
//!
//! The drawing belongs to this application rather than to the framework: it is one picture of one
//! name, not a component another application could reuse.

use qframe::geometry::{Rect, Size};
use qframe::icons::GlyphMode;
use qframe::style::CellStyle;
use qframe::text;
use qframe::widget::{MeasureCx, PaintCx, Widget};

/// The wordmark, one string per row. Every character is one cell wide, so a column of the drawing
/// is a column of the screen.
const ART: [&str; ROWS as usize] = [
    "   ▄████████",
    " ▄███▀▀▀▀███",
    "███▀     ███  ██████  ██████  █████▄  ██████",
    "███      ███  ██      ██  ██  ██  ██  ██▄▄▄▄",
    "███▄▄▄▄▄  ▀█  ██      ██  ██  ██  ██  ██▀▀▀▀",
    "█████████▄    ██████  ██████  █████▀  ██████",
];

/// Rows the drawing takes.
const ROWS: u16 = 6;

/// The form the drawing gives way to. The product name is a name rather than a phrase: it reads
/// the same in every language and so is not part of the language files.
const PLAIN: &str = "QCode";

/// Rows the logo keeps free beneath itself: the tagline and the five menu rows. The menu is the
/// screen and the logo is its decoration, so on a short terminal the logo shrinks to the name and
/// then goes altogether rather than pushing a row of the menu off the bottom.
const ROWS_BELOW: u16 = 6;

/// Theme colour the wordmark is painted in. A logo is one accent, not a palette.
const COLOR: &str = "accent";

/// The QCode wordmark.
///
/// Drawn from block elements where the terminal has them. ASCII has no block elements and a shape
/// spelled out of `#` or `=` would be a text ornament, so in that mode the lit cells are painted
/// in the accent colour instead, which rounds every half block up to a whole cell.
///
/// The drawing gives way before the menu does: without the room for it the name is written
/// plainly, and without the room for that either nothing is drawn.
///
/// Give the logo [`Logo::WIDTH`] cells of width. It centres whichever form it draws inside the
/// area it is given, so a narrower area still holds the name in the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Logo;

impl Logo {
    /// Cells the drawing takes across, and the width the logo is laid out with.
    pub const WIDTH: u16 = 44;

    /// The wordmark.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl<Msg: 'static> Widget<Msg> for Logo {
    fn measure(&self, _cx: &mut MeasureCx<'_>, available: Size) -> Size {
        let width = available.width.min(Self::WIDTH);
        // What is left once the tagline and the menu have their rows.
        let spare = available.height.saturating_sub(ROWS_BELOW);
        if width >= Self::WIDTH && spare >= ROWS {
            Size::new(width, ROWS)
        } else if width >= text::width(PLAIN) && spare >= 1 {
            Size::new(width, 1)
        } else {
            Size::new(0, 0)
        }
    }

    fn paint(&self, cx: &mut PaintCx<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        let color = cx.color(COLOR);
        if area.width < Self::WIDTH || area.height < ROWS {
            let name = area.centered(Size::new(text::width(PLAIN), 1));
            cx.text(name.x, name.y, PLAIN, CellStyle::fg(color).with_bold(true), area.width);
            return;
        }
        let drawing = area.centered(Size::new(Self::WIDTH, ROWS));
        let painted = cx.env().glyph_mode() == GlyphMode::Ascii;
        for (row, line) in ART.iter().enumerate() {
            let y = drawing.y + i32::try_from(row).unwrap_or(0);
            for (column, glyph) in line.chars().enumerate() {
                if glyph == ' ' {
                    continue;
                }
                let x = drawing.x + i32::try_from(column).unwrap_or(0);
                if painted {
                    cx.clear(Rect::new(x, y, 1, 1), color);
                } else {
                    let mut buffer = [0u8; 4];
                    cx.text(x, y, glyph.encode_utf8(&mut buffer), CellStyle::fg(color), 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use qframe::prelude::*;
    use qframe::runtime::Harness;

    use super::*;

    /// The logo with the room it keeps free below it, laid out the way the home screen lays it
    /// out: a fixed width, so the drawing is never measured against a narrower column.
    struct Demo;

    impl App for Demo {
        type Msg = ();

        fn update(&mut self, (): ()) -> Command<()> {
            Command::none()
        }

        fn view(&self, ui: &mut View<'_, ()>) {
            ui.add(Logo::new()).width(Length::Cells(Logo::WIDTH));
        }
    }

    fn logo(width: u16, height: u16) -> Harness<Demo> {
        let mut harness = Harness::new(Demo, width, height);
        harness.set_glyph_mode(GlyphMode::Unicode).render();
        harness
    }

    /// The shortest screen the drawing appears on.
    const TALL: u16 = ROWS + ROWS_BELOW;

    #[test]
    fn every_glyph_takes_exactly_one_cell() {
        for line in ART {
            for glyph in line.chars() {
                let mut buffer = [0u8; 4];
                let glyph = glyph.encode_utf8(&mut buffer);
                assert_eq!(text::width(glyph), 1, "`{glyph}` is not one cell wide");
            }
        }
    }

    #[test]
    fn the_rows_keep_the_widths_they_were_drawn_with() {
        let widths: Vec<u16> = ART.iter().map(|line| text::width(line)).collect();
        assert_eq!(widths, vec![12, 12, Logo::WIDTH, Logo::WIDTH, Logo::WIDTH, Logo::WIDTH]);
    }

    #[test]
    fn unicode_draws_the_wordmark_in_the_accent_colour() {
        let harness = logo(Logo::WIDTH, TALL);
        let screen = harness.screen();
        let drawn: Vec<&str> = screen.lines().take(usize::from(ROWS)).map(str::trim_end).collect();
        assert_eq!(drawn, ART, "{screen}");
        assert_eq!(harness.fg(3, 0), harness.env().theme().color(COLOR));
    }

    #[test]
    fn nerd_font_draws_the_same_wordmark() {
        let mut harness = logo(Logo::WIDTH, TALL);
        let unicode = harness.screen();
        harness.set_glyph_mode(GlyphMode::Nerd).render();
        assert_eq!(harness.screen(), unicode);
    }

    #[test]
    fn ascii_paints_the_shape_in_cells_of_colour() {
        let mut harness = logo(Logo::WIDTH, TALL);
        harness.set_glyph_mode(GlyphMode::Ascii).render();
        let screen = harness.screen();
        assert!(screen.is_ascii(), "{screen}");
        assert!(screen.trim().is_empty(), "the shape is colour, not characters:\n{screen}");
        let accent = harness.env().theme().color(COLOR);
        assert_eq!(harness.bg(3, 0), accent, "the first lit cell of the top row");
        assert_ne!(harness.bg(0, 0), accent, "the indent of the top row stays empty");
        // `▀` is half a cell in the other modes and the whole of one here.
        assert_eq!(harness.bg(10, 4), accent, "a half block rounds up to a whole cell");
        assert_ne!(harness.bg(8, 4), accent, "the gap beside it stays empty");
    }

    #[test]
    fn a_narrow_screen_writes_the_name_plainly_and_centred() {
        let harness = logo(Logo::WIDTH - 1, TALL);
        assert_eq!(harness.screen().lines().next(), Some("                   QCode"));
    }

    #[test]
    fn a_short_screen_writes_the_name_plainly() {
        let harness = logo(Logo::WIDTH, TALL - 1);
        assert_eq!(harness.screen().lines().next().map(str::trim), Some(PLAIN));
    }

    #[test]
    fn a_screen_with_no_room_to_spare_drops_the_logo() {
        let harness = logo(Logo::WIDTH, ROWS_BELOW);
        assert!(harness.screen().trim().is_empty(), "{}", harness.screen());
    }

    #[test]
    fn a_screen_too_narrow_even_for_the_name_drops_the_logo() {
        let harness = logo(text::width(PLAIN) - 1, TALL);
        assert!(harness.screen().trim().is_empty(), "{}", harness.screen());
    }

    #[test]
    fn ascii_never_leaves_a_block_element_behind() {
        let mut harness = logo(Logo::WIDTH - 1, TALL);
        harness.set_glyph_mode(GlyphMode::Ascii).render();
        assert!(harness.screen().is_ascii(), "{}", harness.screen());
    }
}
