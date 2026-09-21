//! The tabs that show a file's text taken out in the workspace's own container: a PDF, with the
//! way to draw any of its pages as a picture, and a word processor's document.
//!
//! The text is read by a program in the container, never on this machine, and shown by QCode in
//! its own scrolling view, the way a Markdown document is: it is selected, copied and scrolled
//! like every other text of the application, not like a terminal's screen.

use qframe::prelude::*;
use qframe::widgets::{ScrollView, Spinner};

use crate::base::apps;
use crate::engine::run::capture;

use super::plan::{self, LaunchFailure};
use super::{DOCUMENT_ID, Msg, Pages, Tab, TabKey, TabKind, TabState, WorkspaceScreen, inside_container};

/// The most text a tab keeps and shows, in bytes. A long PDF gives megabytes of text, and all of
/// it would be laid out again on every frame; past this much the person reads on in the page
/// pictures, which reach every page.
pub const MOST_TEXT: usize = 256 * 1024;

/// Text taken out of a file in the container, ready for a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// The text, no longer than [`MOST_TEXT`].
    pub text: String,
    /// How many pages the whole document has.
    pub pages: usize,
    /// Whether the text was cut short at [`MOST_TEXT`].
    pub cut: bool,
}

impl Taken {
    /// What a program printed, kept to [`MOST_TEXT`] and counted in pages.
    #[must_use]
    pub fn new(mut text: String) -> Self {
        let pages = apps::pages(&text);
        let cut = text.len() > MOST_TEXT;
        if cut {
            let mut end = MOST_TEXT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        Self { text, pages, cut }
    }
}

/// What the tab of `kind` runs to take its file's text out, when it is a tab that does.
pub(super) fn text_command(kind: &TabKind) -> Option<Vec<String>> {
    match kind {
        TabKind::Pdf(file) => Some(apps::pdf_text(&inside_container(file))),
        TabKind::Office(file) => apps::office_text(&inside_container(file)),
        _ => None,
    }
}

/// Takes the text of the tab `key`'s file out in the workspace's own container, on a background
/// thread: the file is looked for on this machine first, as for every file tab, then the
/// container is brought up and [`WorkspaceScreen::read_command`] run in it.
pub(super) fn take_text(screen: &WorkspaceScreen, key: TabKey, run: u64) -> Command<Msg> {
    let Some(workspace) = screen.workspaces.iter().find(|workspace| workspace.tabs.iter().any(|tab| tab.key() == key))
    else {
        return Command::none();
    };
    let Some(kind) = workspace.tabs.iter().find(|tab| tab.key() == key).map(Tab::kind) else {
        return Command::none();
    };
    let (Some(engine), Some(plan), Some(command), Some(file)) =
        (screen.engine.clone(), workspace.plan(kind), screen.read_command(key), kind.file())
    else {
        return Command::none();
    };
    let file = workspace.files.path(file);
    let (user, registry) = (screen.user, screen.registry.clone());
    Command::perform(move || {
        if !file.exists() {
            return Msg::Missing(key, run);
        }
        if let Err(failure) = plan::ensure_running(&engine, &plan, user) {
            return Msg::TextRead(key, run, Err(failure));
        }
        let read = capture(&command).map(Taken::new).map_err(|error| LaunchFailure::from(&error));
        super::noted(registry.as_deref(), engine.kind(), &plan.name, Msg::TextRead(key, run, read))
    })
}

/// Takes what reading the text of the tab `key` answered. A tab that was restarted or closed
/// meanwhile is left alone. A PDF with no text at all is pictures of pages, so it goes straight
/// to drawing its first page.
pub(super) fn text_read(
    screen: &mut WorkspaceScreen,
    key: TabKey,
    run: u64,
    answer: Result<Taken, LaunchFailure>,
) -> Command<Msg> {
    let Some((_, tab)) = screen.owner_mut(key).and_then(|workspace| workspace.find(key)) else {
        return Command::none();
    };
    if tab.run() != run || !tab.state().is_starting() {
        return Command::none();
    }
    let taken = match answer {
        Ok(taken) => taken,
        Err(failure) => {
            tab.settled(TabState::Failed(failure));
            return Command::none();
        }
    };
    let blank = apps::is_blank(&taken.text);
    let pages = Pages { count: taken.pages, ..tab.pages() };
    tab.turn(pages);
    tab.read(taken.text, taken.cut);
    if blank && matches!(tab.kind(), TabKind::Pdf(_)) {
        return show_page(screen, key, Some(1));
    }
    Command::none()
}

/// Draws page `page` of the PDF tab `key`, or with `None` goes back to its text.
///
/// Drawing a page is drawing a picture: the tab starts again from its container, and the page is
/// drawn at the tab's size of now. Going back to the text ends the drawing and shows the text
/// the tab kept.
pub(super) fn show_page(screen: &mut WorkspaceScreen, key: TabKey, page: Option<usize>) -> Command<Msg> {
    let engine = screen.engine.clone();
    let (user, registry) = (screen.user, screen.registry.clone());
    let Some(workspace) = screen.owner_mut(key) else { return Command::none() };
    let Some((_, tab)) = workspace.find(key) else { return Command::none() };
    if !matches!(tab.kind(), TabKind::Pdf(_)) {
        return Command::none();
    }
    let pages = tab.pages();
    let Some(page) = page else {
        // A scanned PDF has no text to go back to.
        if tab.document().is_none_or(apps::is_blank) {
            return Command::none();
        }
        tab.close_session();
        tab.turn(Pages { drawn: false, ..pages });
        tab.settled(TabState::Running);
        return Command::focus(DOCUMENT_ID);
    };
    let Some(engine) = engine else { return Command::none() };
    tab.turn(Pages { page: page.clamp(1, pages.count), drawn: true, ..pages });
    tab.restarting();
    let token = tab.token().to_owned();
    let run = tab.run();
    let kind = tab.kind().clone();
    let Some(plan) = workspace.plan(&kind) else { return Command::none() };
    let file = kind.file().map(|file| workspace.files.path(file));
    // A PDF tab never runs a harness, so it never carries a provider to check.
    let launch = super::Launch::new(key, run, token, None, None);
    super::bring_up(engine, plan, launch, user, file, registry)
}

/// Whether the tab `tab` shows a page drawn, rather than text.
pub(super) fn draws(tab: &Tab) -> bool {
    matches!(tab.kind(), TabKind::Pdf(_)) && tab.pages().drawn
}

/// Whether this module draws the tab `tab`: a tab of taken-out text once there is text or it is
/// being taken out, or a page once it is being drawn. Waiting and trouble are said the way every
/// tab says them.
pub(super) fn shows(tab: &Tab) -> bool {
    text_command(tab.kind()).is_some()
        && (matches!(tab.state(), TabState::Running | TabState::Ended { .. })
            || tab.state().is_starting() && !draws(tab))
}

/// Draws the tab `tab`, one [`shows`] said is this module's.
pub(super) fn view(tab: &Tab, ui: &mut View<'_, Msg>) {
    match tab.kind() {
        TabKind::Pdf(file) => pdf(tab, file, ui),
        TabKind::Office(file) => text_page(tab, file, None, None, ui),
        _ => {}
    }
}

/// A PDF tab: its text with the way to draw its pages above it, or a page drawn with the way
/// through the pages and back to the text.
fn pdf(tab: &Tab, file: &str, ui: &mut View<'_, Msg>) {
    let key = tab.key();
    let pages = tab.pages();
    if pages.drawn {
        let failed = matches!(tab.state(), TabState::Ended { code: Some(code) } if *code != 0);
        let has_text = tab.document().is_some_and(|text| !apps::is_blank(text));
        ui.column(|ui| {
            ui.row(|ui| {
                ui.add(Text::new(file.to_owned()).role("faint").no_wrap());
                ui.spacer();
                // Where "Page picture" stood, so the same place goes back and forth.
                if has_text {
                    ui.add(Button::new(t!("workspace.pdf.text")).on_press(Msg::Page(key, None)))
                        .id("workspace-text-action");
                }
            })
            .gap(2)
            .padding(Padding { left: 2, right: 1, ..Padding::default() })
            .fill_width();
            match tab.session() {
                Some(session) => super::terminal(ui, session),
                None => {
                    ui.column(|ui| {
                        ui.add(Spinner::new().label(t!("workspace.starting")));
                    })
                    .fill()
                    .align(Align::Center)
                    .justify(Align::Center);
                }
            }
            // On a line of its own: beside the way through the pages it would crowd them off a
            // narrow tab.
            if failed {
                ui.row(|ui| {
                    ui.add(Text::new(t!("workspace.pdf.not-drawn")).role("secondary"));
                })
                .padding(Padding { left: 2, right: 1, ..Padding::default() })
                .fill_width();
            }
            ui.row(|ui| {
                let previous = Button::new(t!("workspace.pdf.previous"))
                    .icon("arrow-left")
                    .disabled(pages.page <= 1)
                    .on_press(Msg::Page(key, Some(pages.page.saturating_sub(1))));
                ui.add(previous).id("workspace-page-previous");
                ui.add(Text::new(t!("workspace.pdf.page", page = pages.page, count = pages.count)).no_wrap());
                let next = Button::new(t!("workspace.pdf.next"))
                    .icon("arrow-right")
                    .disabled(pages.page >= pages.count)
                    .on_press(Msg::Page(key, Some(pages.page + 1)));
                ui.add(next).id("workspace-page-next");
                ui.spacer();
                ui.add(Button::new(t!("workspace.file.redraw")).on_press(Msg::Restart(key))).id("workspace-redraw");
            })
            .gap(2)
            .padding(Padding { left: 1, right: 1, ..Padding::default() })
            .fill_width();
        })
        .fill()
        .id("workspace-pdf-page");
        return;
    }
    let draw = Button::new(t!("workspace.pdf.picture")).on_press(Msg::Page(key, Some(pages.page)));
    text_page(tab, file, Some(draw), None, ui);
}

/// The text of a tab, scrolling under a row with the file's name and `action` at its right, and
/// `note` under that when there is something to say about the text.
pub(super) fn text_page(
    tab: &Tab,
    file: &str,
    action: Option<Button<Msg>>,
    note: Option<String>,
    ui: &mut View<'_, Msg>,
) {
    let Some(text) = tab.document() else {
        ui.column(|ui| {
            ui.add(Spinner::new().label(t!("workspace.file.reading")));
        })
        .fill()
        .align(Align::Center)
        .justify(Align::Center)
        .id("workspace-reading");
        return;
    };
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Text::new(file.to_owned()).role("faint").no_wrap());
            ui.spacer();
            if let Some(action) = action {
                ui.add(action).id("workspace-text-action");
            }
        })
        .gap(2)
        .padding(Padding { left: 2, right: 1, ..Padding::default() })
        .fill_width();
        // A long text says it is cut before it starts, not at an end few scroll to.
        let cut = tab.is_partial().then(|| t!("workspace.file.cut"));
        for note in [note, cut].into_iter().flatten() {
            ui.row(|ui| {
                ui.add(Text::new(note).role("secondary"));
            })
            .padding(Padding::symmetric(0, 2))
            .fill_width();
        }
        ui.add_with(ScrollView::new(), |ui| {
            ui.column(|ui| {
                if apps::is_blank(text) {
                    ui.add(Text::new(t!("workspace.file.no-text")).role("faint"));
                } else {
                    ui.add(Text::new(text.trim_end().to_owned())).selectable(true).fill_width();
                }
            })
            .padding(Padding::symmetric(0, 2))
            .fill_width();
        })
        .fill()
        .id(DOCUMENT_ID);
    })
    .gap(1)
    .fill()
    .id("workspace-text-page");
}
