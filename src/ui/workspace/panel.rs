//! The widget panel on the right: which widgets it carries, in what order, and what they show.

use qframe::date::DateTime;
use qframe::prelude::*;
use qframe::widgets::{
    Click, ContextItem, ContextMenu, FileManager, FileManagerState, MenuTarget, Popover, RowMark, Section, ShimmerText,
    Spinner, Switch, Tooltip, WidgetDock,
};

use crate::engine::ContainerState;

use super::files;
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
            let list = List::new(items)
                .selected(Some(row))
                .on_select(Msg::HighlightWidget)
                .on_activate(move |index| Msg::AddWidget(offered[index]));
            ui.add(list.wrap(true)).id(CHOOSER_ID).width(Length::Cells(26));
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

/// The workspace's own files, in the framework's file manager; with `engine` their earlier
/// versions can be looked for too.
fn files_widget(workspace: &OpenWorkspace, engine: bool, ui: &mut View<'_, Msg>) {
    let tree = workspace.files();
    if tree.children(FileManagerState::ROOT).is_none() && tree.error().is_none() {
        // An unread folder is not an empty one, so it never says "empty" before it is known.
        ui.add(Spinner::new().label(t!("workspace.files.reading")));
        return;
    }
    if let Some(problem) = tree.error() {
        // Said here rather than by the manager, so the panel names the folder it is about.
        ui.add(Text::new(t!("workspace.files.unreadable")).role("secondary"));
        ui.add(Text::new(problem.to_owned()).role("faint")).selectable(true);
        return;
    }
    let skip = workspace.backup_skip().to_vec();
    let marked = skip.clone();
    let folders = tree.folder_keys();
    let root = tree.root().to_path_buf();
    FileManager::new(tree, files::wrap(workspace.id()))
        .root_label(workspace.name().to_owned())
        // One click opens, as it always has in qcode: a file in its tab, a folder where it stands.
        // Ctrl and Shift with a click still only select, and a drag still carries the selection.
        .open_on(Click::Single)
        .on_open(move |path| Msg::OpenFile(files::key_of(&root, path)))
        .menu_for(move |target| own_items(target, &skip, engine))
        .row_mark(move |key| mark(key, folders.contains(key), &marked))
        .id(FILES_ID)
        .show(ui)
        .fill();
}

/// The name the file tree is focused by.
pub(super) const FILES_ID: &str = "workspace-files";

/// The theme colour of the icon of an entry the backup leaves out.
const LEFT_OUT_TONE: &str = "warning";

/// What qcode says about the look of the row `key`, a folder when `folder`, in a workspace whose
/// backup leaves `skip` out.
///
/// The workspace folder itself carries the workspace's icon, so the top row reads as the workspace
/// rather than as one more folder. What the backup leaves out is faint, with everything in it, and
/// its icon takes the warning tone so it is told apart from a cut entry, which is faint in its own
/// colour; the workspace widget names it in words. A word beside the row would not do: the panel is
/// narrow, and the tree gives a detail its room before the name.
fn mark(key: &str, folder: bool, skip: &[String]) -> RowMark {
    if key == FileManagerState::ROOT {
        return RowMark::new().plain_sign("workspace");
    }
    if super::backups::is_left_out(skip, key) {
        let icon = if folder { "folder" } else { "file" };
        return RowMark::new().sign(icon, LEFT_OUT_TONE).faint(true);
    }
    RowMark::new()
}

/// qcode's own items on the menu of a row, in a workspace whose backup leaves `skip` out: a file's
/// earlier versions, read out of the backup in a container and so only with `engine`, and leaving
/// entries out of the backup or taking them in again. The manager puts them in a group of their
/// own before its last, destructive item.
fn own_items(target: &MenuTarget<'_>, skip: &[String], engine: bool) -> Vec<ContextItem<Msg>> {
    let backup = backup_item(target.key, target.selection, skip);
    if target.selection.len() > 1 || target.folder {
        return backup.into_iter().collect();
    }
    let versions = ContextItem::new(t!("workspace.files.versions"), Msg::ShowBackups(Some(target.key.to_owned())))
        .disabled(!engine);
    std::iter::once(versions).chain(backup).collect()
}

/// The item that leaves the entries `targets`, asked for on the row `key`, out of the backup of a
/// workspace that leaves `skip` out, or takes them in again when every one of them is left out by
/// name. An entry inside a folder that is left out goes with its folder, so it offers neither;
/// the workspace folder itself is not among `targets` and offers neither either.
///
/// Unlike cutting and deleting, the item does not count what it acts on: it is undone as easily
/// as it is done, and the menu stays narrow enough to leave the names below it readable.
fn backup_item(key: &str, targets: &[String], skip: &[String]) -> Option<ContextItem<Msg>> {
    let message = |out: bool| Msg::LeaveOut(key.to_owned(), out);
    if !targets.is_empty() && targets.iter().all(|target| skip.contains(target)) {
        return Some(ContextItem::new(t!("workspace.files.back-up"), message(false)));
    }
    if targets.iter().all(|target| super::backups::is_left_out(skip, target)) {
        return None;
    }
    Some(ContextItem::new(t!("workspace.files.leave-out"), message(true)))
}

/// Cells of the panel a widget's own content never has: the dock's indent on the left, the
/// panel's padding on the right. What is left is what a row of buttons has to fit in.
const WIDGET_INSET: u16 = 7;

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
    let backing_up = super::backups::is_running(screen, workspace);
    let backup_label = t!("workspace.info.backup");
    ui.column(|ui| {
        for (label, value) in rows {
            // A backup under way shines in its row, whatever the last one was; it is the panel's
            // only moving thing, and it stops the moment the round is over.
            if backing_up && label == backup_label {
                ui.row(|ui| {
                    ui.add(Text::new(format!("{label}  ")).role("faint"));
                    ui.add(ShimmerText::new(t!("workspace.backup.running"))).id("workspace-backing-up");
                });
                continue;
            }
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
                // A container QCode froze is shown as frozen rather than as the engine's `paused`:
                // the person did not pause it and cannot unpause it, and QCode wakes it by itself
                // the moment it is needed.
                .detail(state_text(&container.state, screen.freezing.frozen.contains(&container.name)))
        });
        let list = List::new(rows).selected(Some(workspace.container_row)).on_select(Msg::SelectContainer);
        ui.add(list.wrap(true)).fill().id("workspace-containers");
    }

    let selected = workspace.containers().get(workspace.container_row);
    let name = selected.map(|container| container.name.clone());
    // What the buttons act on is a container that is up, whether or not it is frozen: a tab cannot
    // be entered while it is frozen, but stopping it, starting it again and asking for a root shell
    // in it all wake it on the way, which is what those commands do.
    let up = selected.is_some_and(|container| container.state.is_up());
    let busy = workspace.busy;
    let labels =
        [t!("workspace.containers.stop"), t!("workspace.containers.restart"), t!("workspace.containers.refresh")];
    let buttons = |ui: &mut View<'_, Msg>| {
        let stop = name.clone().map(Msg::StopContainer);
        let mut button = Button::new(labels[0].clone()).disabled(!up || busy);
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
    // Root in a profile's container, for what its system lacks; what it installs stays in this
    // workspace. Offered for a profile's own container only, and only while it is up.
    let profile = workspace.profiles.iter().find(|profile| {
        name.as_deref() == Some(crate::engine::names::profile_container(workspace.id(), profile.name.as_str()).as_str())
    });
    if let Some(profile) = profile {
        let mut admin = Button::new(t!("workspace.admin.open")).disabled(!up);
        if up {
            admin = admin.on_press(Msg::OpenAdmin(profile.name.to_string()));
        }
        ui.add(admin).id("workspace-container-admin");
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
///
/// A paused container QCode froze reads as frozen: it is asleep because nothing of it is on screen
/// and it has done nothing for a while, and the person neither paused it nor can unpause it, since
/// QCode wakes it by itself the moment one of its tabs is needed. A paused container QCode did not
/// freeze — one a crash left behind, or one the person paused by hand — reads as paused, which is
/// what the engine says of it and the only word that tells the person how to undo it.
fn state_text(state: &ContainerState, frozen: bool) -> String {
    if frozen && matches!(state, ContainerState::Paused) {
        return t!("workspace.state.frozen");
    }
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
