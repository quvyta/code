//! Naming a tab: the person gives a tab a name of their own, from its menu on the strip or with a
//! key on the open tab. The name stands in the strip, in what `list_tabs` tells the agents and in
//! the header of every message the tab sends, in place of the automatic one, until the person
//! clears it.

use qframe::prelude::*;
use qframe::widgets::{Field, Form, Modal, TextInput};

use super::{Msg, TabKey, WorkspaceScreen};

/// The name field of the dialog, which takes the keyboard when the dialog opens.
pub(super) const NAME_ID: &str = "workspace-tab-name";

/// Width of the dialog, in cells: room for a long name without covering the screen.
const WIDTH: u16 = 44;

/// A tab being named, while the dialog is open.
#[derive(Debug)]
pub(super) struct Renaming {
    /// The tab.
    key: TabKey,
    /// What the field holds.
    value: String,
    /// What the tab read when the dialog opened.
    was: String,
    /// Whether that was a name the person had given it.
    named: bool,
}

/// Opens the dialog for the tab at `index` of the open workspace, holding what the tab reads now,
/// all of it selected so that typing replaces it.
pub(super) fn open(screen: &mut WorkspaceScreen, index: usize) -> Command<Msg> {
    let Some(workspace) = screen.workspace() else { return Command::none() };
    let Some(tab) = workspace.tabs.get(index) else { return Command::none() };
    let was = workspace.tab_label(index);
    screen.renaming = Some(Renaming { key: tab.key(), value: was.clone(), was, named: tab.name().is_some() });
    // The keyboard is in the dialog, so a tab still waiting for its container is owed nothing.
    screen.owed_focus = None;
    Command::focus(NAME_ID)
}

/// The field now holds `value`.
pub(super) fn typed(screen: &mut WorkspaceScreen, value: String) {
    if let Some(renaming) = &mut screen.renaming {
        renaming.value = value;
    }
}

/// Keeps what the field holds as the tab's name and closes the dialog. An empty field gives the
/// tab its automatic name back. Enter on an automatic name the person did not change keeps it
/// automatic, so that it goes on following the conversation's title.
pub(super) fn submit(screen: &mut WorkspaceScreen) -> Command<Msg> {
    let Some(renaming) = screen.renaming.take() else { return Command::none() };
    let value = renaming.value.trim();
    let name = match value {
        "" => None,
        same if same == renaming.was && !renaming.named => None,
        value => Some(value.to_owned()),
    };
    if let Some((_, tab)) = screen.owner_mut(renaming.key).and_then(|workspace| workspace.find(renaming.key)) {
        tab.rename(name);
    }
    screen.owed_focus = None;
    Command::focus(super::TABS_ID)
}

/// Closes the dialog and leaves the tab's name as it was.
pub(super) fn cancel(screen: &mut WorkspaceScreen) -> Command<Msg> {
    screen.renaming = None;
    screen.owed_focus = None;
    Command::focus(super::TABS_ID)
}

/// Forgets the dialog of a tab that was closed while it was open.
pub(super) fn closed(screen: &mut WorkspaceScreen, keys: &[TabKey]) {
    if screen.renaming.as_ref().is_some_and(|renaming| keys.contains(&renaming.key)) {
        screen.renaming = None;
    }
}

/// The dialog, while a tab is being named.
pub(super) fn view(screen: &WorkspaceScreen, ui: &mut View<'_, Msg>) {
    let Some(renaming) = &screen.renaming else { return };
    let dialog = Modal::new()
        .title(t!("workspace.rename.title", tab = renaming.was.as_str()))
        .width(WIDTH)
        .on_close(Msg::CancelRename)
        .action(Button::new(t!("workspace.rename.cancel")).on_press(Msg::CancelRename))
        .action(Button::new(t!("workspace.rename.keep")).variant("primary").on_press(Msg::NameTab));
    ui.add_with(dialog, |ui| {
        Form::new().show(ui, |fields| {
            fields.field(Field::new(t!("workspace.rename.label")).hint(t!("workspace.rename.hint")), |ui| {
                let input = TextInput::new(renaming.value.clone())
                    .on_change(Msg::TabName)
                    .on_submit(|_| Msg::NameTab)
                    .select_all_on_focus();
                ui.add(input).id(NAME_ID).fill_width();
            });
        });
    });
}
