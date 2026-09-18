//! The widget panel on the right: which widgets it carries, in what order, and what they show.

use qframe::prelude::*;
use qframe::widgets::{Popover, Section, Spinner, Tree, TreeNode, WidgetDock};

use crate::engine::ContainerState;

use super::files::{self, FileTree};
use super::{Msg, OpenProject, ProjectScreen};

/// A widget the panel can carry.
///
/// There are three; a fourth is added here and in the dock's body, not by a trait, because a widget
/// is a shape on screen rather than a thing with behaviour of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelWidget {
    /// The project's own files.
    Files,
    /// What the project is and where it lives.
    Info,
    /// The project's containers, with the way to stop and restart them.
    Containers,
}

impl PanelWidget {
    /// Every widget, in the order the chooser offers them.
    pub const ALL: [Self; 3] = [Self::Files, Self::Info, Self::Containers];

    /// The key the widget's text and icon are found by.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Info => "info",
            Self::Containers => "containers",
        }
    }

    /// The icon of the widget's title.
    #[must_use]
    pub fn icon(self) -> &'static str {
        match self {
            Self::Files => "folder",
            Self::Info => "info",
            Self::Containers => "inbox",
        }
    }

    /// The widget's title, in the person's language.
    #[must_use]
    pub fn title(self) -> String {
        t!(&format!("project.widget.{}", self.key()))
    }
}

/// What the panel carries and how it stands. The application owns all of it, so it could be
/// saved and given back exactly as it was left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    open: bool,
    width: u16,
    order: Vec<PanelWidget>,
    opened: Vec<PanelWidget>,
    chooser: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            open: true,
            width: super::PANEL_WIDTH,
            order: PanelWidget::ALL.to_vec(),
            opened: vec![PanelWidget::Files, PanelWidget::Containers],
            chooser: false,
        }
    }
}

impl Panel {
    /// Whether the panel is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// How wide the panel is when open.
    #[must_use]
    pub fn width(&self) -> u16 {
        self.width
    }

    /// The widgets the panel carries, in the order they are shown.
    #[must_use]
    pub fn order(&self) -> &[PanelWidget] {
        &self.order
    }

    /// Whether the widget at `index` is unfolded.
    #[must_use]
    pub fn is_unfolded(&self, index: usize) -> bool {
        self.order.get(index).is_some_and(|widget| self.opened.contains(widget))
    }

    /// Opens or closes the panel.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Sets the panel's width; the framework keeps it within the screen's room.
    pub fn set_width(&mut self, width: u16) {
        self.width = width;
    }

    /// Unfolds or folds the widget at `index`.
    pub fn toggle(&mut self, index: usize, open: bool) {
        let Some(widget) = self.order.get(index).copied() else { return };
        self.opened.retain(|shown| *shown != widget);
        if open {
            self.opened.push(widget);
        }
    }

    /// Moves the widget at `from` so that its position becomes `to`.
    pub fn move_widget(&mut self, from: usize, to: usize) {
        if from >= self.order.len() || to >= self.order.len() {
            return;
        }
        let widget = self.order.remove(from);
        self.order.insert(to, widget);
    }

    /// Shows or dismisses the chooser of which widgets the panel carries.
    pub fn show_chooser(&mut self, show: bool) {
        self.chooser = show;
    }

    /// Whether the chooser is open.
    #[must_use]
    pub fn chooser_open(&self) -> bool {
        self.chooser
    }

    /// Adds the widget at `index` of [`PanelWidget::ALL`] to the panel, or takes it off.
    ///
    /// A widget taken off keeps nothing; putting it back gives it the end of the panel, which is
    /// where the person is looking when they add it.
    pub fn toggle_shown(&mut self, index: usize) {
        let Some(widget) = PanelWidget::ALL.get(index).copied() else { return };
        if self.order.contains(&widget) {
            self.order.retain(|shown| *shown != widget);
        } else {
            self.order.push(widget);
        }
    }

    /// Whether the panel carries `widget`.
    #[must_use]
    pub fn carries(&self, widget: PanelWidget) -> bool {
        self.order.contains(&widget)
    }
}

/// Draws the panel: its title bar, the chooser of widgets and the dock itself.
pub(super) fn view(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let panel = screen.panel();
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Text::new(t!("project.panel.title")).role("title").no_wrap());
            ui.spacer();
            chooser(panel, ui);
        })
        .fill_width();
        dock(screen, ui);
    })
    .gap(1)
    .padding(Padding::symmetric(1, 1))
    .fill();
}

/// The control that adds widgets to the panel and takes them off again.
fn chooser(panel: &Panel, ui: &mut View<'_, Msg>) {
    let open = panel.chooser_open();
    Popover::new(open)
        .on_dismiss(Msg::ShowWidgets(false))
        .anchor(|ui| {
            ui.add(Button::new(t!("project.panel.widgets")).icon("add").on_press(Msg::ShowWidgets(!open)))
                .id("project-panel-widgets");
        })
        .content(|ui| {
            let items = PanelWidget::ALL.map(|widget| ListItem::new(widget.title()).icon(widget.icon(), None));
            let checked: Vec<bool> = PanelWidget::ALL.iter().map(|widget| panel.carries(*widget)).collect();
            ui.add(List::new(items).checked(checked).on_toggle(Msg::ToggleShown).on_activate(Msg::ToggleShown))
                .id("project-panel-chooser")
                .width(Length::Cells(26));
        })
        .show(ui);
}

/// The stack of widgets.
fn dock(screen: &ProjectScreen, ui: &mut View<'_, Msg>) {
    let panel = screen.panel();
    let sections = panel.order().iter().map(|widget| Section::new(widget.title()).icon(widget.icon()));
    let open: Vec<bool> = (0..panel.order().len()).map(|index| panel.is_unfolded(index)).collect();
    let dock = WidgetDock::new(sections)
        .open(open)
        .empty_text(t!("project.panel.none"))
        .on_toggle(Msg::ToggleWidget)
        .on_move(move |from, to| Msg::MoveWidget { from, to });
    ui.add_with(dock, |ui| {
        for widget in panel.order() {
            ui.column(|ui| body(screen, *widget, ui)).fill().id(widget.key());
        }
    })
    .fill()
    .id("project-panel-dock");
}

/// The content of one widget.
fn body(screen: &ProjectScreen, widget: PanelWidget, ui: &mut View<'_, Msg>) {
    let Some(project) = screen.project() else { return };
    match widget {
        PanelWidget::Files => files_widget(project, ui),
        PanelWidget::Info => info_widget(screen, project, ui),
        PanelWidget::Containers => containers_widget(screen, project, ui),
    }
}

/// The project's own files.
fn files_widget(project: &OpenProject, ui: &mut View<'_, Msg>) {
    let tree = project.files();
    if tree.children(files::ROOT).is_none() && tree.error().is_none() {
        // An unread folder is not an empty one, so it never says "empty" before it is known.
        ui.add(Spinner::new().label(t!("project.files.reading")));
        return;
    }
    if let Some(problem) = tree.error() {
        ui.add(Text::new(t!("project.files.unreadable")).role("secondary"));
        ui.add(Text::new(problem.to_owned()).role("faint")).selectable(true);
        return;
    }
    ui.add(
        Tree::new(nodes(tree, files::ROOT))
            .selected(tree.selected())
            .empty_text(t!("project.files.empty"))
            .on_select(move |key| Msg::SelectFile(key.to_owned()))
            .on_expand(move |key, open| Msg::ExpandFile(key.to_owned(), open)),
    )
    .fill()
    .id("project-files");
}

/// The nodes below the folder `key`, as far as the tree has been read.
fn nodes(tree: &FileTree, key: &str) -> Vec<TreeNode> {
    let Some(entries) = tree.children(key) else { return Vec::new() };
    entries
        .iter()
        .map(|entry| {
            let child = files::child_key(key, &entry.name);
            let icon = if entry.folder { "folder" } else { "file" };
            let mut node = TreeNode::new(child.clone(), entry.name.clone()).icon(icon, None);
            if entry.folder {
                let open = tree.is_open(&child);
                node = node.expandable(true).expanded(open).loading(tree.is_loading(&child));
                if open {
                    node = node.children(nodes(tree, &child));
                }
            }
            node
        })
        .collect()
}

/// What the project is and where it lives.
fn info_widget(screen: &ProjectScreen, project: &OpenProject, ui: &mut View<'_, Msg>) {
    let engine = screen
        .engine()
        .map_or_else(|| t!("project.info.no-engine"), |engine| format!("{:?}", engine.kind()).to_lowercase());
    let rows = [
        (t!("project.info.name"), project.name().to_owned()),
        (t!("project.info.id"), project.id().to_owned()),
        (t!("project.info.folder"), project.paths().root.display().to_string()),
        (t!("project.info.profiles"), project.profiles().len().to_string()),
        (t!("project.info.engine"), engine),
    ];
    ui.column(|ui| {
        for (label, value) in rows {
            ui.add(Text::rich([Span::new(format!("{label}  ")).role("faint"), Span::new(value)]));
        }
    })
    .fill_width()
    .selectable(true);
}

/// The project's containers, and the way to stop and restart them.
fn containers_widget(screen: &ProjectScreen, project: &OpenProject, ui: &mut View<'_, Msg>) {
    if screen.engine().is_none() {
        ui.add(Text::new(t!("project.no-engine")).role("secondary"));
        return;
    }
    if let Some(failure) = &project.container_error {
        ui.add(Text::new(t!("project.containers.unreadable")).role("secondary"));
        ui.add(Text::new(failure.output.clone()).role("faint")).selectable(true);
    }
    // The panel is narrow and every container of a project starts with the same `qcode-<id>-`,
    // so the part that differs is what is shown; the whole name is in the engine's own listing.
    let prefix = format!("qcode-{}-", project.id());
    let rows = project.containers().iter().map(|container| {
        let short = container.name.strip_prefix(&prefix).unwrap_or(&container.name);
        ListItem::new(short.to_owned()).icon("dot", Some(tone(&container.state))).detail(state_text(&container.state))
    });
    ui.add(
        List::new(rows)
            .selected(Some(project.container_row))
            .empty_text(t!("project.containers.empty"))
            .on_select(Msg::SelectContainer),
    )
    .fill()
    .id("project-containers");

    let selected = project.containers().get(project.container_row);
    let name = selected.map(|container| container.name.clone());
    let running = selected.is_some_and(|container| container.state.is_running());
    let busy = project.busy;
    ui.row(|ui| {
        let stop = name.clone().map(Msg::StopContainer);
        let mut button = Button::new(t!("project.containers.stop")).disabled(!running || busy);
        if let Some(message) = stop {
            button = button.on_press(message);
        }
        ui.add(button).id("project-container-stop");

        let restart = name.map(Msg::RestartContainer);
        let mut button = Button::new(t!("project.containers.restart")).disabled(selected.is_none() || busy);
        if let Some(message) = restart {
            button = button.on_press(message);
        }
        ui.add(button).id("project-container-restart");

        // The engine is the only one who knows; the list is what it said last, and this asks again.
        ui.add(
            Button::new(t!("project.containers.refresh")).loading(busy).disabled(busy).on_press(Msg::RefreshContainers),
        )
        .id("project-container-refresh");
    })
    .gap(1)
    .fill_width();
}

/// The theme colour of a container's state. Colour never carries the meaning alone: the state is
/// written out beside the dot.
fn tone(state: &ContainerState) -> &'static str {
    match state {
        ContainerState::Running => "success",
        ContainerState::Created | ContainerState::Restarting => "info",
        ContainerState::Paused => "warning",
        ContainerState::Dead => "danger",
        ContainerState::Exited | ContainerState::Removing | ContainerState::Unknown(_) => "muted",
    }
}

/// A container's state in the person's language; a word this version does not know is shown as
/// the engine wrote it rather than hidden.
fn state_text(state: &ContainerState) -> String {
    let key = match state {
        ContainerState::Created => "created",
        ContainerState::Running => "running",
        ContainerState::Paused => "paused",
        ContainerState::Restarting => "restarting",
        ContainerState::Removing => "removing",
        ContainerState::Exited => "exited",
        ContainerState::Dead => "dead",
        ContainerState::Unknown(word) => return word.clone(),
    };
    t!(&format!("project.state.{key}"))
}
