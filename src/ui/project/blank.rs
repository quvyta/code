//! The page of a blank tab: what the tab should open, as one list sectioned by profile.
//!
//! The list is drawn from a small model, [`Row`], and a row the list reports by its position is
//! found again in the same model, so a profile's section can grow rows of its own without the
//! list and what its rows do drifting apart.
//!
//! Under each profile's new chat come its newest conversations in the project, read out of the
//! profile's container while the page is shown (see [`super::history`]), and a row that shows
//! the rest of them.

use qframe::date::DateTime;
use qframe::prelude::*;

use crate::profile::history::Conversation;

use super::history::{self, HistoryKey, NEWEST, Shown};
use super::{Msg, OpenProject, ProjectScreen, TabKey, TabKind};

/// The name the list of the page is focused by.
pub(super) const CHOICES_ID: &str = "project-choices";

/// The widest the list grows, so a detail on the right stays within reach of its label on a
/// wide terminal.
const READABLE_WIDTH: u16 = 72;

/// What a blank tab can turn into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// A shell in the project's own container.
    Shell,
    /// A new conversation with the harness of the profile of this name.
    NewChat(String),
    /// A conversation the harness of the profile of this name had before, by the harness's id.
    Resume(String, String),
    /// The window of the desktop harness of the profile of this name, on the person's own screen.
    Window(String),
}

/// One row of the page.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    /// Opens a shell.
    Shell,
    /// The empty row before a section.
    Gap,
    /// The heading of the section of the profile at this position of the project's profiles.
    Profile(usize),
    /// Starts a new conversation with the profile of this name.
    NewChat(String),
    /// Opens the window of the desktop profile of this name.
    Window(String),
    /// Opens again a conversation the profile of this name had.
    Chat(String, Conversation),
    /// Shows every conversation of a profile in place, when this many more are hidden.
    ShowAll(HistoryKey, usize),
    /// Says the profile has had no conversation in the project yet.
    NoChats,
    /// Says the profile's conversations are being read.
    Reading,
    /// Says the profile's conversations could not be read, and why.
    Unread(String),
    /// Leads to the profiles screen, when there is no profile to choose from at all.
    MakeProfile,
}

impl Row {
    /// Whether the list lets the keyboard and the pointer rest on the row.
    fn selectable(&self) -> bool {
        !matches!(self, Self::Gap | Self::Profile(_))
    }

    /// What choosing the row in the blank tab `tab` asks for. A heading and a gap ask nothing,
    /// and neither does a row that only says how a section stands.
    fn message(&self, tab: TabKey) -> Option<Msg> {
        match self {
            Self::Shell => Some(Msg::Choose(tab, Choice::Shell)),
            Self::NewChat(name) => Some(Msg::Choose(tab, Choice::NewChat(name.clone()))),
            Self::Window(name) => Some(Msg::Choose(tab, Choice::Window(name.clone()))),
            Self::Chat(name, conversation) => {
                Some(Msg::Choose(tab, Choice::Resume(name.clone(), conversation.id.clone())))
            }
            Self::ShowAll(key, _) => Some(Msg::ShowAll(key.clone())),
            Self::MakeProfile => Some(Msg::ManageProfiles),
            Self::Gap | Self::Profile(_) | Self::NoChats | Self::Reading | Self::Unread(_) => None,
        }
    }

    /// Whether `other` is this row, even when what it shows was read again since: a
    /// conversation is the same one while its id is, whatever its title and time say now.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Chat(profile, conversation), Self::Chat(other_profile, other_conversation)) => {
                profile == other_profile && conversation.id == other_conversation.id
            }
            (Self::ShowAll(key, _), Self::ShowAll(other_key, _)) => key == other_key,
            _ => self == other,
        }
    }
}

/// The rows of the page for `project`: the shell first, then a section for every profile in the
/// order the project offers them, or the way to a first profile when there is none.
///
/// Without an engine nothing was read, so a section is only its new chat.
fn rows(screen: &ProjectScreen, project: &OpenProject) -> Vec<Row> {
    let mut rows = vec![Row::Shell];
    if project.profiles().is_empty() {
        rows.extend([Row::Gap, Row::MakeProfile]);
    }
    for (index, profile) in project.profiles().iter().enumerate() {
        let name = profile.name.as_str();
        // A desktop profile's section is one row: its window. It has no conversations QCode can
        // list — the shape of what the agent inside writes was never read — so none are offered.
        if profile.harness.desktop().is_some() {
            rows.extend([Row::Gap, Row::Profile(index), Row::Window(name.to_owned())]);
            continue;
        }
        rows.extend([Row::Gap, Row::Profile(index), Row::NewChat(name.to_owned())]);
        if screen.engine().is_none() {
            continue;
        }
        let Some(shelf) = project.history.get(name) else { continue };
        match shelf.shown() {
            Shown::Nothing => {}
            Shown::Reading => rows.push(Row::Reading),
            Shown::Failed(reason) => rows.push(Row::Unread(reason.to_owned())),
            Shown::Conversations([]) => rows.push(Row::NoChats),
            Shown::Conversations(found) => {
                let key = HistoryKey { project: project.id().to_owned(), profile: name.to_owned() };
                let count = if screen.expanded.contains(&key) { found.len() } else { found.len().min(NEWEST) };
                rows.extend(found[..count].iter().map(|conversation| Row::Chat(name.to_owned(), conversation.clone())));
                if count < found.len() {
                    rows.push(Row::ShowAll(key, found.len() - count));
                }
            }
        }
    }
    rows
}

/// The row the keyboard rests on: `wanted` when a row can rest there, otherwise the shell, which
/// is always the first row.
fn resting_row(rows: &[Row], wanted: usize) -> usize {
    if rows.get(wanted).is_some_and(Row::selectable) { wanted } else { 0 }
}

/// Applies `change` to the screen, keeping the keyboard on the row it rested on when the change
/// adds or takes away rows above it: a section whose conversations arrive while the person is
/// further down the page must not move the selection onto another row under their hands.
pub(super) fn keep_row(screen: &mut ProjectScreen, change: impl FnOnce(&mut ProjectScreen)) {
    let page = |screen: &ProjectScreen| {
        screen
            .project()
            .filter(|project| project.active_tab().is_some_and(|tab| tab.kind() == &TabKind::New))
            .map(|project| rows(screen, project))
    };
    let before = page(screen).and_then(|rows| rows.get(screen.blank_row).cloned());
    change(screen);
    let (Some(before), Some(after)) = (before, page(screen)) else { return };
    if let Some(index) = after.iter().position(|row| row.same(&before)) {
        screen.blank_row = index;
    }
}

/// The cells a row without an icon is moved in by, so its label starts under the label of the
/// new chat above it: the width of the new chat's glyph and the gap after it.
fn indent(ui: &View<'_, Msg>) -> String {
    let glyph = ui.env().icons().glyph("add");
    " ".repeat(usize::from(qframe::text::width(&glyph)) + 1)
}

/// How `row` is drawn. Without an engine every row is faint: it can still be looked at and
/// moved through, and the line above the list says why choosing it does nothing. A row that
/// only says how a section stands is always faint, since there is nothing to choose in it.
fn item(project: &OpenProject, row: &Row, look: &Look, ready: bool) -> ListItem {
    let dot = look.dot.as_str();
    match row {
        Row::Shell => ListItem::new(t!("project.tab.shell"))
            .icon("prompt", None)
            .detail(t!("project.choose.shell-detail"))
            .faint(!ready),
        Row::Gap => ListItem::gap(),
        Row::Profile(index) => {
            let Some(profile) = project.profiles().get(*index) else { return ListItem::gap() };
            let name = profile.name.as_str();
            let harness = profile.harness.record().display_name;
            // A profile the project does not carry yet is offered all the same; its heading says
            // that choosing from it adds it, so the file changing is no surprise.
            ListItem::header(if project.carries(name) {
                t!("project.choose.section", profile = name, harness = harness, dot = dot)
            } else {
                t!("project.choose.section-adds", profile = name, harness = harness, dot = dot)
            })
        }
        Row::NewChat(_) => ListItem::new(t!("project.choose.new-chat")).icon("add", None).faint(!ready),
        Row::Window(_) => ListItem::new(t!("project.choose.window"))
            .icon("window-maximize", None)
            .detail(t!("project.choose.window-detail"))
            .faint(!ready),
        Row::Chat(_, conversation) => {
            let title = conversation.title.clone().unwrap_or_else(|| t!("project.history.untitled"));
            ListItem::new(format!("{}{title}", look.indent))
                .detail(history::when(conversation.used_ms, look.now))
                .faint(!ready)
        }
        Row::ShowAll(_, more) => {
            ListItem::new(format!("{}{}", look.indent, t!("project.history.show-all", more = *more, dot = dot)))
                .faint(!ready)
        }
        Row::NoChats => ListItem::new(format!("{}{}", look.indent, t!("project.history.none"))).faint(true),
        Row::Reading => ListItem::new(format!("{}{}", look.indent, t!("project.history.reading"))).faint(true),
        Row::Unread(reason) => {
            ListItem::new(format!("{}{}", look.indent, t!("project.history.unread"))).detail(reason.clone()).faint(true)
        }
        Row::MakeProfile => {
            ListItem::new(t!("project.make-profile")).icon("add", None).detail(t!("project.no-profiles")).faint(!ready)
        }
    }
}

/// What every row of one frame of the page is drawn with.
struct Look {
    /// The separator of a heading, in the glyph mode of the frame.
    dot: String,
    /// What moves a row in under the label of the new chat.
    indent: String,
    /// Now, so "today" and "yesterday" mean the same for every row.
    now: DateTime,
}

/// Draws the page of the blank tab `tab` of `project`: a question, and the list that answers it,
/// at the top left where reading starts.
pub(super) fn view(screen: &ProjectScreen, project: &OpenProject, tab: TabKey, ui: &mut View<'_, Msg>) {
    let ready = screen.engine().is_some();
    let rows = rows(screen, project);
    let look =
        Look { dot: ui.env().icons().glyph("bullet").into_owned(), indent: indent(ui), now: DateTime::now_local() };
    let items: Vec<ListItem> = rows.iter().map(|row| item(project, row, &look, ready)).collect();
    let selected = resting_row(&rows, screen.blank_row);
    ui.column(|ui| {
        ui.add(Text::new(t!("project.choose.title")).role("title"));
        if !ready {
            ui.add(Text::new(t!("project.no-engine")).role("secondary"));
        }
        ui.add(
            List::new(items)
                .selected(Some(selected))
                .on_select(Msg::HighlightChoice)
                // A row that only says how a section stands has nothing to choose; activating it
                // leaves the keyboard on it, where the pointer or the keys put it. The list never
                // activates a heading or a gap, since it does not let the keyboard rest there.
                .on_activate(move |index| {
                    rows.get(index).and_then(|row| row.message(tab)).unwrap_or(Msg::HighlightChoice(index))
                }),
        )
        .id(CHOICES_ID)
        .width(Length::Cells(READABLE_WIDTH))
        .fill_height();
    })
    .gap(1)
    .padding(Padding::symmetric(1, 2))
    .fill()
    .id("project-blank");
}
