//! The home screen: the logo over the menu that opens everything else.

use qframe::prelude::*;

use crate::Msg as AppMsg;
use crate::ui::logo::Logo;

/// Width of the menu. Wide enough for the longest label with a project name beside it, narrow
/// enough to read as one column under the logo; a narrower terminal shrinks it.
const MENU_WIDTH: u16 = 34;

/// A row of the home menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// Goes back to the projects that were open, the way a browser brings back its windows.
    /// Takes the first row whenever there is something to go back to.
    Continue,
    /// Takes the first row while no project has been opened yet.
    NewProject,
    /// All projects.
    Projects,
    /// The harness profiles projects are opened with.
    Profiles,
    /// Language, theme and the container engine.
    Settings,
    /// Leaves the application.
    Quit,
}

impl Entry {
    /// The translated label.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Continue => t!("home.continue"),
            Self::NewProject => t!("home.new-project"),
            Self::Projects => t!("home.projects"),
            Self::Profiles => t!("home.profiles"),
            Self::Settings => t!("home.settings"),
            Self::Quit => t!("home.quit"),
        }
    }

    /// The icon drawn before the label. Both first rows are about a project, so they share one.
    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Continue | Self::NewProject | Self::Projects => "project",
            Self::Profiles => "profile",
            Self::Settings => "settings",
            Self::Quit => "power",
        }
    }
}

/// What can happen on the home screen.
///
/// Opening a row is not here: a row opens a screen, which is the application's to decide, so the
/// menu sends [`AppMsg::Open`] straight away rather than passing it through this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// The selection moved to a row.
    Select(usize),
}

/// The home screen's state: which projects "Continue" goes back to and where the selection
/// sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    open: Vec<String>,
    selected: usize,
}

impl Home {
    /// A home screen whose first row goes back to `open`, the names of the projects that were
    /// open in rail order, or starts a new project when there are none.
    #[must_use]
    pub fn new(open: Vec<String>) -> Self {
        Self { open, selected: 0 }
    }

    /// Takes `open` as the projects "Continue" goes back to, keeping the selection where it is.
    pub fn set_open(&mut self, open: Vec<String>) {
        self.open = open;
    }

    /// The rows, in order. The first one either continues where the person left off or starts
    /// their first project.
    #[must_use]
    pub fn entries(&self) -> [Entry; 5] {
        let first = if self.open.is_empty() { Entry::NewProject } else { Entry::Continue };
        [first, Entry::Projects, Entry::Profiles, Entry::Settings, Entry::Quit]
    }

    /// What the "Continue" row says beside its label: the first project, and how many more
    /// there are after it.
    fn detail(&self) -> Option<String> {
        let first = self.open.first()?;
        let n = self.open.len();
        Some(t!("home.continue-detail", n = n, first = first.as_str(), more = n - 1))
    }
}

/// The control that takes the keyboard when the screen opens: the menu, which is the whole
/// screen. Without it the hint under the screen would promise arrow keys that go nowhere until
/// something else is pressed first.
#[must_use]
pub fn entry() -> &'static str {
    MENU
}

/// The name of the menu, for the focus and the tests.
const MENU: &str = "menu";

/// Applies a home screen message.
pub fn update(home: &mut Home, message: Msg) -> Command<AppMsg> {
    match message {
        Msg::Select(index) => {
            if index < home.entries().len() {
                home.selected = index;
            }
        }
    }
    Command::none()
}

/// Draws the logo, the tagline and the menu, centred in the body.
pub fn view(home: &Home, ui: &mut View<'_, AppMsg>) {
    let entries = home.entries();
    let items = entries.into_iter().map(|entry| {
        let item = ListItem::new(entry.label()).icon(entry.icon(), None);
        match (entry, home.detail()) {
            (Entry::Continue, Some(detail)) => item.detail(detail),
            _ => item,
        }
    });
    let menu = List::new(items)
        .selected(Some(home.selected))
        .on_select(|index| AppMsg::Home(Msg::Select(index)))
        .on_activate(move |index| AppMsg::Open(entries[index.min(entries.len() - 1)]));

    ui.column(|ui| {
        ui.add(Logo::new()).width(Length::Cells(Logo::WIDTH));
        ui.add(Text::new(t!("app.tagline")).role("secondary"));
        ui.add(menu).id(MENU).width(Length::Cells(MENU_WIDTH));
    })
    .fill()
    .gap(1)
    .align(Align::Center)
    .justify(Align::Center);
}

/// The keys of the home screen that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("hints.move")), (icons.glyph("enter").into_owned(), t!("hints.open"))]
}

#[cfg(test)]
mod tests {
    use qframe::icons::GlyphMode;
    use qframe::runtime::Harness;

    use super::{Entry, Home};
    use crate::{QCode, testing};

    /// A terminal wide enough for the logo and the menu.
    const SIZE: (u16, u16) = (90, 30);

    fn home(recent: Option<&str>, width: u16, height: u16) -> Harness<QCode> {
        let workspace = testing::scratch("home-workspace");
        let recent: Vec<&str> = recent.into_iter().collect();
        let app = testing::app(testing::config(&workspace, &recent), &testing::settled(), None);
        testing::harness(app, width, height)
    }

    #[test]
    fn the_logo_stands_over_the_menu() {
        let harness = home(None, SIZE.0, SIZE.1);
        let screen = harness.screen();
        assert!(screen.contains("███▀     ███  ██████"), "the wordmark is drawn:\n{screen}");
        for label in ["New project", "Projects", "Profiles", "Settings", "Quit"] {
            assert!(screen.contains(label), "`{label}` is missing:\n{screen}");
        }
        let (_, logo_row) = harness.find("Coding harnesses").expect("the tagline is on screen");
        let (_, menu_row) = harness.find("Projects").expect("the menu is on screen");
        assert!(logo_row < menu_row, "the tagline sits above the menu:\n{screen}");
    }

    #[test]
    fn a_recent_project_takes_the_first_row_and_names_itself() {
        let harness = home(Some("firefly"), SIZE.0, SIZE.1);
        let screen = harness.screen();
        assert!(screen.contains("Continue"), "{screen}");
        assert!(screen.contains("firefly"), "{screen}");
        assert!(!screen.contains("New project"), "{screen}");
    }

    #[test]
    fn without_a_recent_project_the_menu_offers_a_new_one() {
        let screen = home(None, SIZE.0, SIZE.1).screen();
        assert!(screen.contains("New project"), "{screen}");
        assert!(!screen.contains("Continue"), "{screen}");
    }

    #[test]
    fn continue_stands_while_there_is_something_to_go_back_to() {
        let mut one = Home::new(vec!["Firefly".to_owned()]);
        let mut three = Home::new(vec!["Firefly".to_owned(), "Moth".to_owned(), "Lantern".to_owned()]);
        assert_eq!(one.entries()[0], Entry::Continue);
        one.set_open(Vec::new());
        three.selected = 3;
        three.set_open(vec!["Moth".to_owned()]);
        assert_eq!(one.entries()[0], Entry::NewProject, "nothing to go back to is a new project again");
        assert_eq!(three.selected, 3, "a new list of projects leaves the selection where it was");
    }

    #[test]
    fn continue_reads_in_turkish_with_its_count() {
        let session = testing::scratch("home-session");
        let _ = std::fs::remove_dir_all(&session);
        let file = session.join("session.toml");
        std::fs::create_dir_all(&session).expect("a folder");
        std::fs::write(
            &file,
            "[[project]]\nid = \"firefly\"\n\n[[project]]\nid = \"moth\"\n\n[[project]]\nid = \"lantern\"\n",
        )
        .expect("a session file");
        let workspace = testing::scratch("home-workspace");
        let app = testing::app(testing::config(&workspace, &[]), &testing::settled(), None).with_session(Some(file));
        let mut harness = testing::harness(app, SIZE.0, SIZE.1);
        assert!(
            harness.screen().contains("Continue") && harness.screen().contains("firefly +2"),
            "{}",
            harness.screen()
        );
        harness.set_locale("tr").render();
        let screen = harness.screen();
        assert!(screen.contains("Devam et") && screen.contains("firefly +2"), "{screen}");
        let _ = std::fs::remove_dir_all(&session);
    }

    #[test]
    fn a_narrow_screen_keeps_the_menu_and_writes_the_logo_small() {
        let harness = home(None, 16, 14);
        let screen = harness.screen();
        assert!(screen.contains("QCode"), "the logo falls back to the plain name:\n{screen}");
        assert!(!screen.contains('▀'), "no room for block glyphs:\n{screen}");
        assert!(screen.contains("Quit"), "{screen}");
    }

    #[test]
    fn a_short_screen_drops_the_logo_before_it_drops_a_menu_row() {
        // Fifteen rows hold the drawing, the tagline and the whole menu; ten hold the name in
        // place of the drawing; nine hold the menu alone.
        let drawn = home(None, SIZE.0, 15).screen();
        assert!(drawn.contains("█████████▄"), "{drawn}");
        let named = home(None, SIZE.0, 10).screen();
        assert!(!named.contains('█'), "the drawing gives way first:\n{named}");
        assert!(named.contains("QCode"), "{named}");
        let bare = home(None, SIZE.0, 9).screen();
        assert!(!bare.contains("QCode"), "the logo goes rather than the menu:\n{bare}");
        for screen in [&drawn, &named, &bare] {
            assert!(screen.contains("New project") && screen.contains("Quit"), "the menu is whole:\n{screen}");
        }
    }

    #[test]
    fn ascii_mode_draws_nothing_but_ascii() {
        let mut harness = home(None, SIZE.0, SIZE.1);
        harness.set_glyph_mode(GlyphMode::Ascii).render();
        let screen = harness.screen();
        assert!(screen.is_ascii(), "{screen}");
        assert!(screen.contains("Quit"), "{screen}");
        // In ASCII mode the logo is painted in colour rather than written in characters, so the
        // text of the screen alone cannot tell whether it is there. Only the logo sits above the
        // tagline, so a coloured cell there is the logo.
        let (_, tagline) = harness.find("Coding harnesses").expect("the tagline is on screen");
        let rows = 0..u16::try_from(tagline).expect("the tagline row fits the screen");
        let background = harness.bg(0, 0);
        let painted = rows.flat_map(|y| (0..SIZE.0).map(move |x| (x, y))).any(|(x, y)| harness.bg(x, y) != background);
        assert!(painted, "the logo is painted in cells of colour:\n{screen}");
    }

    #[test]
    fn nothing_is_bracketed_lined_or_framed() {
        let mut harness = home(Some("firefly"), SIZE.0, SIZE.1);
        for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
            harness.set_glyph_mode(mode).render();
            let screen = harness.screen();
            for forbidden in ['[', ']', '(', ')', '{', '}', '|', '┌', '─', '│', '+'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` in {mode:?}:\n{screen}");
            }
            assert!(!screen.contains("=="), "{screen}");
            assert!(!screen.contains("->"), "{screen}");
        }
    }

    #[test]
    fn every_row_carries_its_icon_in_every_glyph_mode() {
        // The bar of the selected row is a glyph in Nerd Font and Unicode; ASCII paints it in
        // colour, so its first row starts at the icon.
        let mut harness = home(Some("firefly"), SIZE.0, SIZE.1);
        for (mode, expected) in [
            (
                GlyphMode::Nerd,
                [
                    "▌  \u{f1b2} Continue             firefly",
                    "\u{f1b2} Projects",
                    "\u{f007} Profiles",
                    "\u{f013} Settings",
                    "\u{f011} Quit",
                ],
            ),
            (
                GlyphMode::Unicode,
                ["▌  ◈ Continue             firefly", "◈ Projects", "◉ Profiles", "▤ Settings", "○ Quit"],
            ),
            (GlyphMode::Ascii, ["# Continue             firefly", "# Projects", "@ Profiles", "* Settings", "x Quit"]),
        ] {
            harness.set_glyph_mode(mode).render();
            let screen = harness.screen();
            let (_, first) = harness.find("Continue").expect("the menu is on screen");
            let first = usize::try_from(first).expect("a row on the screen");
            let rows: Vec<&str> = screen.lines().skip(first).take(5).map(str::trim).collect();
            assert_eq!(rows, expected, "{mode:?}:\n{screen}");
        }
    }

    #[test]
    fn the_keyboard_moves_the_selection_and_opens_a_row() {
        let mut harness = home(None, SIZE.0, SIZE.1);
        assert!(harness.is_focused("menu"), "the menu takes the first focus:\n{}", harness.screen());
        harness.press("end").press("enter");
        assert!(harness.quit_requested(), "the last row leaves the application");
    }

    #[test]
    fn reduced_motion_keeps_the_menu_working() {
        let mut harness = home(None, SIZE.0, SIZE.1);
        harness.set_reduced_motion(true).press("down").render();
        let screen = harness.screen();
        assert!(screen.contains("Projects"), "{screen}");
        harness.press("end").press("enter");
        assert!(harness.quit_requested(), "{screen}");
    }

    #[test]
    fn clicking_the_last_row_leaves() {
        let mut harness = home(None, SIZE.0, SIZE.1);
        harness.click_text("Quit");
        assert!(harness.quit_requested());
    }

    #[test]
    fn turkish_reads_as_turkish() {
        let mut harness = home(Some("firefly"), SIZE.0, SIZE.1);
        harness.set_locale("tr").render();
        let screen = harness.screen();
        for label in ["Devam et", "Projeler", "Profiller", "Ayarlar", "Çıkış", "kapsayıcının"] {
            assert!(screen.contains(label), "`{label}` is missing:\n{screen}");
        }
    }
}
