//! The page a screen of a list is drawn on: a column of its own width, in the middle of a wide
//! terminal.
//!
//! QCode's screens are read at a glance and often left open beside a harness. A page that spread
//! from one edge of a wide terminal to the other would put a row's name and its right-hand detail
//! as far apart as the terminal happened to be wide; a page of its own width in the middle keeps
//! them one glance apart and gives the eye one edge to start at and one to finish at. Settings and
//! the lists of workspaces, profiles and providers stand on such a page, and so does whatever an
//! open workspace shows in its middle in place of a terminal: a blank tab's choices, a tab waiting
//! for its container or refused one, a missing image and its build, a desktop harness's window.
//! A terminal never does: every column of it is a column the program in it can use.
//!
//! The page takes the whole width of a terminal narrower than itself, so a narrow window loses
//! nothing. The dialogs drawn over these pages keep their own widths: a layer is centred on the
//! screen, and so is the page, so a dialog stands over the page it belongs to.

use qframe::prelude::*;

/// The width a page keeps, in cells. A label, the detail on the other end of its row and the
/// control under it read as one line at this width; the widest sentence QCode puts on a page of
/// its own is about as long as this, so nothing wraps.
pub const WIDTH: u16 = 84;

/// The width of a page that keeps `own` cells, on a screen `screen` cells across: the page's own
/// width, or every cell of the screen when the screen is narrower than the page.
#[must_use]
pub fn width(own: u16, screen: u16) -> u16 {
    own.min(screen)
}

/// Draws `body` as a page of `own` cells at the most, in the middle of the screen and from its
/// top row to its bottom one.
///
/// The page keeps the room it is given from top to bottom: only its two sides are bounded.
pub fn column<Msg: 'static>(ui: &mut View<'_, Msg>, own: u16, body: impl FnOnce(&mut View<'_, Msg>)) {
    let cells = width(own, ui.size().width);
    ui.column(|ui| {
        ui.column(body).width(Length::Cells(cells)).fill_height();
    })
    .fill()
    .align(Align::Center);
}

#[cfg(test)]
mod tests {
    use qframe::prelude::*;
    use qframe::runtime::Harness;

    use super::{WIDTH, column, width};

    /// A heading over one row with a detail at its other end, which is the shape of the three
    /// lists the page was made for.
    struct Page;

    impl App for Page {
        type Msg = ();

        fn update(&mut self, _: ()) -> Command<()> {
            Command::none()
        }

        fn view(&self, ui: &mut View<'_, ()>) {
            column(ui, WIDTH, |ui| {
                ui.add(Text::new("head"));
                ui.add(List::new([ListItem::new("row").detail("end")])).fill_width();
            });
        }
    }

    /// The page in a terminal `wide` cells across: where its heading, its row and the row's
    /// detail start.
    fn drawn(wide: u16) -> (i32, i32, i32) {
        let mut harness = Harness::new(Page, wide, 6);
        harness.render();
        let head = harness.find("head").expect("the heading is drawn");
        let row = harness.find("row").expect("the row is drawn");
        let end = harness.find("end").expect("the row's detail is drawn");
        (head.0, row.0, end.0)
    }

    #[test]
    fn a_page_stands_in_the_middle_at_its_own_width() {
        let (head, row, end) = drawn(200);
        let left = i32::from((200 - WIDTH) / 2);
        assert_eq!(head, left, "the heading is not at the page's left edge");
        // A list keeps two cells in front of a row for the mark of the chosen one, and one cell
        // after its detail, so the last letter stands one in from the page's edge.
        assert_eq!(row, left + 2, "the row does not begin at the page's left edge");
        assert_eq!(end + 3, left + i32::from(WIDTH) - 1, "the detail does not end at the page's right edge");
    }

    #[test]
    fn a_terminal_narrower_than_the_page_gives_it_every_column() {
        assert_eq!(width(WIDTH, 70), 70);
        let (head, _, end) = drawn(70);
        assert_eq!(head, 0, "the page is not at the screen's edge");
        assert_eq!(end + 3, 69, "the page does not take every column");
    }
}
