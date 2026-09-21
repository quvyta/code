//! The widget panel on the right: which widgets it carries, in what order, and what they show.

use qframe::date::DateTime;
use qframe::prelude::*;
use qframe::widgets::{
    ContextItem, ContextMenu, Field, Form, FormErrors, Modal, Popover, Section, Spinner, Switch, TextInput, Tooltip,
    Tree, TreeNode, WidgetDock,
};

use crate::engine::ContainerState;

use super::file_ops::{is_within, stem};
use super::files::{self, FileMsg, FileTree, NameFor};
use super::{Msg, OpenWorkspace, WorkspaceScreen};

/// A widget the panel can carry.
///
/// There are three; a fourth is added here and in the dock's body, not by a trait, because a widget
/// is a shape on screen rather than a thing with behaviour of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelWidget {
    /// The workspace's own files.
    Files,
    /// What the workspace is and where it lives.
    Info,
    /// The workspace's containers, with the way to stop and restart them.
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
        t!(&format!("workspace.widget.{}", self.key()))
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
    /// The row of the chooser the keyboard is on.
    chooser_row: usize,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            open: true,
            width: super::PANEL_WIDTH,
            order: PanelWidget::ALL.to_vec(),
            opened: vec![PanelWidget::Files, PanelWidget::Containers],
            chooser: false,
            chooser_row: 0,
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

    /// Shows or dismisses the chooser of the widgets the panel does not carry; it always opens
    /// on its first row.
    pub fn show_chooser(&mut self, show: bool) {
        self.chooser = show;
        self.chooser_row = 0;
    }

    /// Puts the chooser's keyboard on `row`.
    pub fn highlight(&mut self, row: usize) {
        self.chooser_row = row;
    }

    /// Whether the chooser is open.
    #[must_use]
    pub fn chooser_open(&self) -> bool {
        self.chooser
    }

    /// Adds `widget` to the end of the panel, unfolded, which is where the person is looking when
    /// they add it, and closes the chooser that offered it. A widget the panel already carries
    /// stays where it is.
    pub fn add(&mut self, widget: PanelWidget) {
        if !self.order.contains(&widget) {
            self.order.push(widget);
            self.opened.push(widget);
        }
        self.chooser = false;
    }

    /// Takes `widget` off the panel; it keeps nothing, so adding it again starts it afresh.
    pub fn remove(&mut self, widget: PanelWidget) {
        self.order.retain(|shown| *shown != widget);
        self.opened.retain(|shown| *shown != widget);
    }

    /// The widgets the panel does not carry, in the order [`PanelWidget::ALL`] lists them.
    #[must_use]
    pub fn missing(&self) -> Vec<PanelWidget> {
        PanelWidget::ALL.into_iter().filter(|widget| !self.carries(*widget)).collect()
    }

    /// Whether the panel carries `widget`.
    #[must_use]
    pub fn carries(&self, widget: PanelWidget) -> bool {
        self.order.contains(&widget)
    }
}

/// The name the list of widgets to add is focused by.
pub(super) const CHOOSER_ID: &str = "workspace-panel-chooser";

/// Draws the panel: its title bar, the chooser of widgets and the dock itself.
pub(super) fn view(screen: &WorkspaceScreen, ui: &mut View<'_, Msg>) {
    let panel = screen.panel();
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Text::new(t!("workspace.panel.title")).role("title").no_wrap());
            ui.spacer();
            chooser(panel, ui);
        })
        .fill_width();
        dock(screen, ui);
        if let Some(workspace) = screen.workspace() {
            naming(workspace.files(), ui);
        }
        super::backups::view(screen, ui);
    })
    .gap(1)
    .padding(Padding::symmetric(1, 1))
    .fill();
}

/// The control that adds a widget the panel does not carry yet: an icon-only `+` whose popover
/// lists just the missing widgets. With every widget on the panel there is nothing to add, so the
/// control is not drawn at all rather than opening an empty list. Taking a widget off is the
/// dock's context menu, see [`dock`].
fn chooser(panel: &Panel, ui: &mut View<'_, Msg>) {
    let missing = panel.missing();
    if missing.is_empty() {
        return;
    }
    let open = panel.chooser_open();
    Popover::new(open)
        .on_dismiss(Msg::ShowWidgets(false))
        .anchor(|ui| {
            // The glyph is the label rather than the icon, so the button stays symmetric: an icon
            // is always followed by a gap that only makes sense before a word.
            let plus = ui.env().icons().glyph("add").into_owned();
            ui.add_with(Tooltip::new(t!("workspace.panel.add")).on_focus(true), |ui| {
                ui.add(Button::new(plus).on_press(Msg::ShowWidgets(!open))).id("workspace-panel-add");
            });
        })
        .content(|ui| {
            let items = missing.iter().map(|widget| ListItem::new(widget.title()).icon(widget.icon(), None));
            let offered = missing.clone();
            let row = panel.chooser_row.min(missing.len() - 1);
            ui.add(
                List::new(items)
                    .selected(Some(row))
                    .on_select(Msg::HighlightWidget)
                    .on_activate(move |index| Msg::AddWidget(offered[index])),
            )
            .id(CHOOSER_ID)
            .width(Length::Cells(26));
        })
        .show(ui);
}

/// The stack of widgets.
fn dock(screen: &WorkspaceScreen, ui: &mut View<'_, Msg>) {
    let panel = screen.panel();
    let sections = panel.order().iter().map(|widget| Section::new(widget.title()).icon(widget.icon()));
    let open: Vec<bool> = (0..panel.order().len()).map(|index| panel.is_unfolded(index)).collect();
    let dock = WidgetDock::new(sections)
        .open(open)
        .empty_text(t!("workspace.panel.none"))
        .on_toggle(Msg::ToggleWidget)
        .on_move(move |from, to| Msg::MoveWidget { from, to });
    // A right click anywhere on the dock, or Shift+F10 from a widget in it, offers to take each
    // carried widget off. The dock is one area for the menu rather than one per widget, so a
    // folded widget, which shows only its title, can be taken off as well.
    let removals = panel.order().iter().map(|widget| {
        ContextItem::new(t!("workspace.panel.remove", widget = widget.title()), Msg::RemoveWidget(*widget))
            .icon(widget.icon())
    });
    ui.add_with(ContextMenu::new(removals), |ui| {
        ui.add_with(dock, |ui| {
            for widget in panel.order() {
                ui.column(|ui| body(screen, *widget, ui)).fill().id(widget.key());
            }
        })
        .fill()
        .id("workspace-panel-dock");
    })
    .fill();
}

/// The content of one widget.
fn body(screen: &WorkspaceScreen, widget: PanelWidget, ui: &mut View<'_, Msg>) {
    let Some(workspace) = screen.workspace() else { return };
    match widget {
        PanelWidget::Files => files_widget(workspace, screen.engine().is_some(), ui),
        PanelWidget::Info => info_widget(screen, workspace, ui),
        PanelWidget::Containers => containers_widget(screen, workspace, ui),
    }
}

/// The workspace's own files; with `engine` their earlier versions can be looked for too.
fn files_widget(workspace: &OpenWorkspace, engine: bool, ui: &mut View<'_, Msg>) {
    let tree = workspace.files();
    if tree.children(files::ROOT).is_none() && tree.error().is_none() {
        // An unread folder is not an empty one, so it never says "empty" before it is known.
        ui.add(Spinner::new().label(t!("workspace.files.reading")));
        return;
    }
    if let Some(problem) = tree.error() {
        ui.add(Text::new(t!("workspace.files.unreadable")).role("secondary"));
        ui.add(Text::new(problem.to_owned()).role("faint")).selectable(true);
        return;
    }
    let cut = tree.cut().to_vec();
    let folders = tree.folder_keys();
    let chosen = tree.chosen().to_vec();
    let skip = workspace.backup_skip().to_vec();
    let accepts = folders.clone();
    ui.add(
        Tree::new([root_node(workspace, tree)])
            .selected(tree.selected())
            .on_select(move |key| Msg::SelectFile(key.to_owned()))
            // Enter or a click on a file opens it in a tab; on a folder they open the folder,
            // which the tree does itself. Space and the modified clicks choose instead.
            .on_activate(move |key| Msg::OpenFile(key.to_owned()))
            .multi_select(tree.chosen(), |keys| Msg::Files(FileMsg::Choose(keys)))
            .droppable(|drop| Msg::Files(FileMsg::Drop(drop)), move |key| key == files::ROOT || accepts.contains(key))
            .on_expand(move |key, open| Msg::ExpandFile(key.to_owned(), open))
            .context_menu(move |key| {
                // The tree keeps the selection when the click is on one of its rows and makes the
                // row the selection otherwise, so the menu acts on what the click was on.
                let targets = files::targets(&chosen, key);
                let backup = backup_item(key, &targets, &skip);
                if targets.len() > 1 {
                    many_menu(key, targets.len(), !cut.is_empty(), backup.into_iter().collect())
                } else if key == files::ROOT || folders.contains(key) {
                    folder_menu(key, &cut, backup.into_iter().collect())
                } else {
                    // Earlier versions are read out of the backup in a container.
                    let versions =
                        ContextItem::new(t!("workspace.files.versions"), Msg::ShowBackups(Some(key.to_owned())))
                            .disabled(!engine);
                    file_menu(key, !cut.is_empty(), std::iter::once(versions).chain(backup).collect())
                }
            }),
    )
    .fill()
    .id(FILES_ID);
}

/// The workspace folder itself, as the one row at the top of the tree.
///
/// It is a row rather than nothing so the folder has a place of its own: its menu makes entries
/// and pastes at the top, reached by a right click or by selecting it and pressing the menu key
/// like any other row. A tree that is only as tall as its rows has no empty part below them to
/// click, so the row is the one way the mouse and the keyboard reach the folder alike.
fn root_node(workspace: &OpenWorkspace, tree: &FileTree) -> TreeNode {
    let entries = nodes(tree, files::ROOT, workspace.backup_skip());
    let mut root = TreeNode::new(files::ROOT, workspace.name().to_owned()).icon("workspace", None);
    if entries.is_empty() {
        root = root.detail(t!("workspace.files.empty"));
    }
    root.expandable(true).expanded(tree.is_open(files::ROOT)).children(entries)
}

/// The name the file tree is focused by.
pub(super) const FILES_ID: &str = "workspace-files";

/// The name the field of the naming dialog is focused by.
pub(super) const NAME_ID: &str = "workspace-files-name";

/// The item that leaves the entries `targets`, asked for on the row `key`, out of the backup of a
/// workspace that leaves `skip` out, or takes them in again when every one of them is left out by
/// name. An entry inside a folder that is left out goes with its folder, so it offers neither;
/// the workspace folder itself is not among `targets` and offers neither either.
///
/// Unlike cutting and deleting, the item does not count what it acts on: it is undone as easily
/// as it is done, and the menu stays narrow enough to leave the names below it readable.
fn backup_item(key: &str, targets: &[String], skip: &[String]) -> Option<ContextItem<Msg>> {
    let message = |out: bool| Msg::Files(FileMsg::LeaveOut(key.to_owned(), out));
    if !targets.is_empty() && targets.iter().all(|target| skip.contains(target)) {
        return Some(ContextItem::new(t!("workspace.files.back-up"), message(false)));
    }
    if targets.iter().all(|target| super::backups::is_left_out(skip, target)) {
        return None;
    }
    Some(ContextItem::new(t!("workspace.files.leave-out"), message(true)))
}

/// Puts the items of `backup`, when there are any, into a menu as a group of their own.
fn add_backup(items: &mut Vec<ContextItem<Msg>>, backup: Vec<ContextItem<Msg>>) {
    if !backup.is_empty() {
        items.push(ContextItem::gap());
        items.extend(backup);
    }
}

/// The menu of a folder, or of the workspace folder itself when `key` is the root: what can be made
/// in it and, while something is cut, pasting it here. A folder cannot take itself or a folder
/// that holds it, so pasting there is shown but cannot be chosen.
fn folder_menu(key: &str, cut: &[String], backup: Vec<ContextItem<Msg>>) -> Vec<ContextItem<Msg>> {
    let message = |message: FileMsg| Msg::Files(message);
    let mut items = vec![
        ContextItem::new(t!("workspace.files.new-file"), message(FileMsg::NewFile(key.to_owned()))),
        ContextItem::new(t!("workspace.files.new-folder"), message(FileMsg::NewFolder(key.to_owned()))),
    ];
    let root = key == files::ROOT;
    if !root {
        items.push(ContextItem::gap());
        items.push(ContextItem::new(t!("workspace.files.rename"), message(FileMsg::Rename(key.to_owned()))));
        items.push(ContextItem::new(t!("workspace.files.cut"), message(FileMsg::Cut(key.to_owned()))));
    }
    if !cut.is_empty() {
        let paste = ContextItem::new(t!("workspace.files.paste"), message(FileMsg::Paste(key.to_owned())));
        items.push(paste.disabled(cut.iter().any(|cut| is_within(key, cut))));
        items.push(ContextItem::new(t!("workspace.files.drop-cut"), message(FileMsg::DropCut)));
    }
    add_backup(&mut items, backup);
    items.push(ContextItem::gap());
    if root {
        items.push(ContextItem::new(t!("workspace.files.refresh"), message(FileMsg::Refresh)));
    } else {
        items.push(
            ContextItem::new(t!("workspace.files.delete"), message(FileMsg::Delete(key.to_owned()))).danger(true),
        );
    }
    items
}

/// The menu of a file.
fn file_menu(key: &str, cutting: bool, backup: Vec<ContextItem<Msg>>) -> Vec<ContextItem<Msg>> {
    let message = |message: FileMsg| Msg::Files(message);
    let mut items = vec![
        ContextItem::new(t!("workspace.files.rename"), message(FileMsg::Rename(key.to_owned()))),
        ContextItem::new(t!("workspace.files.cut"), message(FileMsg::Cut(key.to_owned()))),
    ];
    if cutting {
        items.push(ContextItem::new(t!("workspace.files.drop-cut"), message(FileMsg::DropCut)));
    }
    add_backup(&mut items, backup);
    items.push(ContextItem::gap());
    items.push(ContextItem::new(t!("workspace.files.delete"), message(FileMsg::Delete(key.to_owned()))).danger(true));
    items
}

/// The menu of a row that is one of `count` selected entries: what can be done to all of them at
/// once. A name is given to one entry at a time, so renaming is not offered.
fn many_menu(key: &str, count: usize, cutting: bool, backup: Vec<ContextItem<Msg>>) -> Vec<ContextItem<Msg>> {
    let message = |message: FileMsg| Msg::Files(message);
    let mut items =
        vec![ContextItem::new(t!("workspace.files.cut-many", n = count), message(FileMsg::Cut(key.to_owned())))];
    if cutting {
        items.push(ContextItem::new(t!("workspace.files.drop-cut"), message(FileMsg::DropCut)));
    }
    add_backup(&mut items, backup);
    items.push(ContextItem::gap());
    items.push(
        ContextItem::new(t!("workspace.files.delete-many", n = count), message(FileMsg::Delete(key.to_owned())))
            .danger(true),
    );
    items
}

/// The dialog that asks for a name, while one is asked for. It waits for an answer rather than
/// sitting beside the tree: the name is all there is to do until it is given or dropped.
fn naming(tree: &FileTree, ui: &mut View<'_, Msg>) {
    let Some(naming) = tree.naming() else { return };
    let (title, confirm) = match &naming.purpose {
        NameFor::File => (t!("workspace.files.new-file-title"), t!("workspace.files.create")),
        NameFor::Folder => (t!("workspace.files.new-folder-title"), t!("workspace.files.create")),
        NameFor::Rename(key) => {
            (t!("workspace.files.rename-title", name = super::file_ops::name_of(key)), t!("workspace.files.rename-do"))
        }
    };
    let close = Msg::Files(FileMsg::CloseNaming);
    let dialog = Modal::new()
        .title(title)
        .width(NAMING_WIDTH)
        .on_close(close.clone())
        .action(Button::new(t!("workspace.files.cancel")).on_press(close))
        .action(Button::new(confirm).variant("primary").on_press(Msg::Files(FileMsg::Submit)));
    let mut errors = FormErrors::new();
    if let Some(problem) = tree.naming_problem() {
        errors.set(NAME_ID, problem.message());
    }
    let value = naming.value.clone();
    // A rename selects the name without its extension, so typing gives a new name and keeps the
    // kind of file; a new entry starts empty and has nothing to select.
    let selection = match &naming.purpose {
        NameFor::Rename(key) => Some(stem(&value, tree.is_folder(key))),
        NameFor::File | NameFor::Folder => None,
    };
    ui.add_with(dialog, |ui| {
        Form::new().show(ui, |fields| {
            fields.field(Field::new(t!("workspace.files.name-label")).error(errors.get(NAME_ID)), |ui| {
                let mut input = TextInput::new(value)
                    .invalid(errors.has(NAME_ID))
                    .on_change(|value| Msg::Files(FileMsg::Name(value)))
                    .on_submit(|_| Msg::Files(FileMsg::Submit));
                if let Some(range) = selection {
                    input = input.select_on_focus(range);
                }
                ui.add(input).id(NAME_ID).fill_width();
            });
        });
    });
}

/// Width of the naming dialog, in cells: room for a long file name without covering the screen.
const NAMING_WIDTH: u16 = 48;

/// Cells of the panel a widget's own content never has: the dock's indent on the left, the
/// panel's padding on the right. What is left is what a row of buttons has to fit in.
const WIDGET_INSET: u16 = 7;

/// The theme colour of the icon of an entry the backup leaves out.
const LEFT_OUT_TONE: &str = "warning";

/// The nodes below the folder `key`, as far as the tree has been read, in a workspace whose backup
/// leaves `skip` out.
fn nodes(tree: &FileTree, key: &str, skip: &[String]) -> Vec<TreeNode> {
    let Some(entries) = tree.children(key) else { return Vec::new() };
    entries
        .iter()
        .map(|entry| {
            let child = files::child_key(key, &entry.name);
            let icon = if entry.folder { "folder" } else { "file" };
            // What was cut is drawn faint until it is pasted or let go, with everything in it.
            // What the backup leaves out is faint too, with everything in it, and its icon takes
            // the warning tone so the two are told apart; the workspace widget names it in words.
            // A word beside the row would not do: the panel is narrow, and the tree gives a
            // detail its room before the name.
            let cut = tree.is_cut(&child);
            let out = super::backups::is_left_out(skip, &child);
            let tone = out.then_some(LEFT_OUT_TONE);
            let mut node = TreeNode::new(child.clone(), entry.name.clone()).icon(icon, tone).faint(cut || out);
            if entry.folder {
                let open = tree.is_open(&child);
                node = node.expandable(true).expanded(open).loading(tree.is_loading(&child));
                if open {
                    node = node.children(nodes(tree, &child, skip));
                }
            }
            node
        })
        .collect()
}

/// What the workspace is and where it lives.
fn info_widget(screen: &WorkspaceScreen, workspace: &OpenWorkspace, ui: &mut View<'_, Msg>) {
    let engine = screen
        .engine()
        .map_or_else(|| t!("workspace.info.no-engine"), |engine| format!("{:?}", engine.kind()).to_lowercase());
    let rows = [
        (t!("workspace.info.name"), workspace.name().to_owned()),
        (t!("workspace.info.id"), workspace.id().to_owned()),
        (t!("workspace.info.folder"), workspace.paths().root.display().to_string()),
        (
            t!("workspace.info.profiles"),
            workspace.profiles().iter().filter(|profile| workspace.carries(profile.name.as_str())).count().to_string(),
        ),
        (t!("workspace.info.engine"), engine),
        (t!("workspace.info.backup"), super::backups::last_text(screen, workspace, DateTime::now_local())),
        (t!("workspace.info.left-out"), super::backups::left_out_text(workspace)),
        (t!("workspace.info.backup-size"), super::backups::size_text(workspace)),
    ];
    ui.column(|ui| {
        for (label, value) in rows {
            ui.add(Text::rich([Span::new(format!("{label}  ")).role("faint"), Span::new(value)]));
        }
    })
    .fill_width()
    .selectable(true);
    ui.add(Switch::new(workspace.backs_up_assets()).label(t!("workspace.backup.assets")).on_toggle(Msg::BackupAssets))
        .id("workspace-backup-assets");
    // The backups are read in a container, so without an engine there is no list to open.
    let backups = Button::new(t!("workspace.backup.list")).disabled(screen.engine().is_none());
    ui.add(backups.on_press(Msg::ShowBackups(None))).id("workspace-backups-open");
}

/// The workspace's containers, and the way to stop and restart them.
fn containers_widget(screen: &WorkspaceScreen, workspace: &OpenWorkspace, ui: &mut View<'_, Msg>) {
    if screen.engine().is_none() {
        ui.add(Text::new(t!("workspace.no-engine")).role("secondary"));
        return;
    }
    if let Some(failure) = &workspace.container_error {
        ui.add(Text::new(t!("workspace.containers.unreadable")).role("secondary"));
        ui.add(Text::new(failure.output.clone()).role("faint")).selectable(true);
    }
    // The panel is narrow and every container of a workspace starts with the same `qcode-<id>-`,
    // so the part that differs is what is shown; the whole name is in the engine's own listing.
    let prefix = format!("qcode-{}-", workspace.id());
    if workspace.containers().is_empty() {
        // A list's empty text is one line cut at the panel's edge, and the panel is narrow enough
        // that the sentence loses its end; as text of its own it wraps and is read whole.
        ui.add(Text::new(t!("workspace.containers.empty")).role("secondary")).fill_width().id("workspace-containers");
    } else {
        let rows = workspace.containers().iter().map(|container| {
            let short = container.name.strip_prefix(&prefix).unwrap_or(&container.name);
            ListItem::new(short.to_owned())
                .icon("dot", Some(tone(&container.state)))
                .detail(state_text(&container.state))
        });
        ui.add(List::new(rows).selected(Some(workspace.container_row)).on_select(Msg::SelectContainer))
            .fill()
            .id("workspace-containers");
    }

    let selected = workspace.containers().get(workspace.container_row);
    let name = selected.map(|container| container.name.clone());
    let running = selected.is_some_and(|container| container.state.is_running());
    let busy = workspace.busy;
    let labels =
        [t!("workspace.containers.stop"), t!("workspace.containers.restart"), t!("workspace.containers.refresh")];
    let buttons = |ui: &mut View<'_, Msg>| {
        let stop = name.clone().map(Msg::StopContainer);
        let mut button = Button::new(labels[0].clone()).disabled(!running || busy);
        if let Some(message) = stop {
            button = button.on_press(message);
        }
        ui.add(button).id("workspace-container-stop");

        let restart = name.clone().map(Msg::RestartContainer);
        let mut button = Button::new(labels[1].clone()).disabled(selected.is_none() || busy);
        if let Some(message) = restart {
            button = button.on_press(message);
        }
        ui.add(button).id("workspace-container-restart");

        // The engine is the only one who knows; the list is what it said last, and this asks again.
        ui.add(Button::new(labels[2].clone()).loading(busy).disabled(busy).on_press(Msg::RefreshContainers))
            .id("workspace-container-refresh");
    };
    // Three words side by side fit an English panel and not a German or Russian one. Rather than
    // cut a word, the row becomes a column as soon as the three no longer fit the panel's width.
    if buttons_width(&labels) <= screen.panel().width().saturating_sub(WIDGET_INSET) {
        ui.row(buttons).gap(1).fill_width();
    } else {
        ui.column(buttons).fill_width();
    }
}

/// Cells a row of these buttons asks for: each label in its own button, which the theme pads by
/// two cells on either side, and one cell between neighbours.
fn buttons_width(labels: &[String]) -> u16 {
    let gaps = u16::try_from(labels.len().saturating_sub(1)).unwrap_or(0);
    labels.iter().fold(gaps, |total, label| total.saturating_add(qframe::text::width(label)).saturating_add(4))
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
    t!(&format!("workspace.state.{key}"))
}
