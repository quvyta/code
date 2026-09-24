//! The workspaces screen: the workspaces of the store, and the dialog that makes a new one.
//!
//! The screen is a state, an [`update`] and a [`view`], and it speaks its own [`Msg`], which
//! the application maps to its own. Send it [`Msg::Refresh`] when it opens;
//! everything else follows from what the person does. Opening a workspace is the one thing it
//! cannot do itself, so [`update`] hands that workspace's identifier back to the application.
//!
//! Three things it never does quietly. A name that is already taken is told to the person and
//! not turned into `name-2`. A folder chosen to start from is copied, never moved, so their own
//! copy stays where it is. A copy or a clone that fails or is stopped takes the workspace folder
//! it was filling with it, so a half-made workspace is never left on the list.

mod draft;
mod fill;
mod remove;

pub(crate) use remove::Names;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use qframe::date::Date;
use qframe::prelude::*;
use qframe::runtime::{Confirm, Task, TaskEvent, TaskId, TaskOutcome, Tasks};
use qframe::widgets::{
    EmptyState, Field, FilePicker, FilePickerMsg, Form, FormErrors, LogBuffer, LogLevel, LogLine, LogView, Modal,
    RadioGroup, ScrollView, Skeleton, TaskList, TextInput, Toast,
};

use crate::engine::Engine;
use crate::store::{Store, WorkspaceEntry, WorkspaceId};

pub use draft::Source;

use draft::{Draft, FOLDER_FIELD, NAME_FIELD, Problem, URL_FIELD};
use fill::{Fill, Job};
use remove::{Outcome, Reach, Survey};

/// Width of the dialogs, in cells. Wide enough for a path and a git address to read as one line
/// on a normal terminal; a narrower screen shrinks them.
const DIALOG_WIDTH: u16 = 68;

/// Rows the folder browser gets inside the dialog.
const PICKER_ROWS: u16 = 12;

/// Rows the live log gets while a workspace is being filled.
const LOG_ROWS: u16 = 10;

/// Rows the skeleton stands in for while the first listing is on its way.
const SKELETON_ROWS: u16 = 6;

/// How many lines of the log are kept. A clone of a large repository says a great deal, and only
/// the end of it is ever read.
const LOG_LINES: usize = 2000;

/// What can happen on the workspaces screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Read the store again. This is also the message that opens the screen.
    Refresh,
    /// The store answered.
    Listed {
        /// Every workspace folder that was found, broken ones included.
        entries: Vec<WorkspaceEntry>,
        /// Why the store itself could not be read, when that is what happened.
        trouble: Option<String>,
    },
    /// The selection moved to the workspace at this place in the list.
    Select(usize),
    /// The selection moved to the row that makes a new workspace.
    SelectNew,
    /// A row was opened with Enter or a click.
    Activate(usize),
    /// Open the new-workspace dialog.
    Start,
    /// Close whatever is open over the screen.
    Dismiss,
    /// The name is being typed.
    Name(String),
    /// Another source was chosen.
    Source(usize),
    /// The git address is being written.
    Url(String),
    /// Something happened in the folder browser.
    Picker(FilePickerMsg),
    /// Make the workspace.
    Submit,
    /// One line of the copy or the clone.
    Line(String),
    /// The filling task reported.
    Progress(TaskEvent),
    /// Stop the filling task.
    Stop(TaskId),
    /// The filling finished.
    Filled,
    /// Open the dialog that asks which workspace to delete.
    Delete,
    /// The selection of that dialog moved.
    DeleteSelect(usize),
    /// A workspace was chosen in that dialog: the engine is asked what it holds of it first.
    DeletePick(usize),
    /// The engine answered what it holds of the workspace to be deleted.
    Surveyed(Box<Survey>),
    /// The question was answered with yes.
    DeleteConfirmed,
    /// The question was answered with no.
    DeleteKept,
    /// The deletion is over.
    Deleted(Box<Survey>, Outcome),
}

/// What is open over the list.
#[derive(Debug)]
enum Overlay {
    /// The new-workspace dialog.
    New(Box<Draft>),
    /// The problems of the broken workspace at this place in the list.
    Problems(usize),
    /// The choice of the workspace to delete, with the one at this place in the list chosen.
    Delete(usize),
}

/// The workspaces screen.
#[derive(Debug)]
pub struct Workspaces {
    store: Store,
    engine: Option<Engine>,
    recent: Vec<String>,
    entries: Vec<WorkspaceEntry>,
    trouble: Option<String>,
    listed: bool,
    selected: usize,
    /// Whether the selection is on the row that makes a new workspace rather than on `selected`.
    on_new: bool,
    overlay: Option<Overlay>,
    tasks: Tasks,
    log: LogBuffer,
    job: Option<TaskId>,
    /// The workspaces open in this QCode, which are not deleted from under their tabs.
    open: Vec<WorkspaceId>,
    /// A workspace is being looked at or deleted, so the button takes no second press.
    deleting: bool,
    /// What the question being asked is about.
    doomed: Option<Box<Survey>>,
    /// The workspace deleted last, for the application to forget, taken once.
    deleted: Option<WorkspaceId>,
}

impl Workspaces {
    /// A screen over `store`, cloning inside `engine` when there is one, with `recent`
    /// holding the identifiers of the workspaces opened before, the most recent first.
    #[must_use]
    pub fn new(store: Store, engine: Option<Engine>, recent: Vec<String>) -> Self {
        Self {
            store,
            engine,
            recent,
            entries: Vec::new(),
            trouble: None,
            listed: false,
            selected: 0,
            on_new: false,
            overlay: None,
            tasks: Tasks::new(),
            log: LogBuffer::new(LOG_LINES),
            job: None,
            open: Vec::new(),
            deleting: false,
            doomed: None,
            deleted: None,
        }
    }

    /// Tells the screen which workspaces are open in this QCode right now: those are not deleted,
    /// because their tabs are working in them.
    pub fn set_open(&mut self, open: Vec<WorkspaceId>) {
        self.open = open;
    }

    /// The workspace deleted last, taken once, so the application can forget it wherever it
    /// remembers it.
    pub fn just_deleted(&mut self) -> Option<WorkspaceId> {
        self.deleted.take()
    }

    /// Whether a workspace is being filled right now, which is what makes leaving the screen
    /// something to ask about.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }
}

/// Applies a message of the workspaces screen, answering with the work to do and the workspace the
/// person opened, when they opened one.
pub fn update(workspaces: &mut Workspaces, message: Msg) -> (Command<Msg>, Option<WorkspaceId>) {
    let mut opened = None;
    let command = match message {
        Msg::Refresh => list(&workspaces.store),
        Msg::Listed { entries, trouble } => {
            let first = !workspaces.listed;
            workspaces.entries = entries;
            workspaces.trouble = trouble;
            workspaces.listed = true;
            workspaces.selected = workspaces.selected.min(workspaces.entries.len().saturating_sub(1));
            // The list is not on screen until the store has answered, so the keyboard is put
            // on it here rather than when the screen opened. Only the first answer does it: a
            // refresh must never take the keyboard off what the person is in the middle of.
            if first && !workspaces.entries.is_empty() && workspaces.overlay.is_none() {
                Command::focus(LIST)
            } else {
                Command::none()
            }
        }
        Msg::Select(index) => {
            if index < workspaces.entries.len() {
                workspaces.selected = index;
                workspaces.on_new = false;
            }
            Command::none()
        }
        Msg::SelectNew => {
            workspaces.on_new = true;
            Command::none()
        }
        Msg::Activate(index) => activate(workspaces, index, &mut opened),
        Msg::Start => {
            let mut draft = Draft::new(start_folder(&workspaces.store));
            let reading = draft.browser.open(draft.browser.folder().to_path_buf(), Msg::Picker);
            workspaces.overlay = Some(Overlay::New(Box::new(draft)));
            Command::batch([reading, Command::focus(NAME_INPUT)])
        }
        Msg::Dismiss => {
            workspaces.overlay = None;
            Command::none()
        }
        Msg::Name(text) => edit(workspaces, |draft| draft.name = text),
        Msg::Source(index) => edit(workspaces, |draft| draft.source = Source::from_index(index)),
        Msg::Url(text) => edit(workspaces, |draft| draft.url = text),
        Msg::Picker(FilePickerMsg::Chosen(path)) => edit(workspaces, |draft| draft.folder = Some(path)),
        Msg::Picker(message) => match workspaces.overlay {
            Some(Overlay::New(ref mut draft)) => draft.browser.update(message, Msg::Picker),
            _ => Command::none(),
        },
        Msg::Submit => submit(workspaces),
        Msg::Line(text) => {
            workspaces.log.push(LogLine::new(LogLevel::Info, text));
            Command::none()
        }
        Msg::Progress(event) => {
            workspaces.tasks.apply(&event);
            match event {
                TaskEvent::Finished { id, outcome } if workspaces.job == Some(id) => finished(workspaces, &outcome),
                _ => Command::none(),
            }
        }
        Msg::Stop(id) => Command::cancel_task(id),
        // The task's own success message; the screen acts on the outcome that follows it, so
        // there is only one place where a finished job is tidied away.
        Msg::Filled => Command::none(),
        Msg::Delete => {
            if workspaces.deleting || workspaces.entries.is_empty() {
                Command::none()
            } else {
                workspaces.overlay = Some(Overlay::Delete(workspaces.selected));
                Command::focus(DELETE_LIST)
            }
        }
        Msg::DeleteSelect(index) => {
            if let Some(Overlay::Delete(chosen)) = workspaces.overlay.as_mut()
                && index < workspaces.entries.len()
            {
                *chosen = index;
            }
            Command::none()
        }
        Msg::DeletePick(index) => ask_delete(workspaces, index),
        Msg::Surveyed(survey) => surveyed(workspaces, survey),
        Msg::DeleteConfirmed => delete(workspaces),
        Msg::DeleteKept => {
            workspaces.deleting = false;
            workspaces.doomed = None;
            Command::none()
        }
        Msg::Deleted(survey, outcome) => deleted(workspaces, &survey, outcome),
    };
    (command, opened)
}

/// The name of the name input, so the dialog can put the cursor in it as it opens.
const NAME_INPUT: &str = "workspace-name";

/// The name of the list, for the focus and the tests.
const LIST: &str = "workspaces";

/// The name of the list the workspace to delete is chosen from.
const DELETE_LIST: &str = "workspace-delete-list";

/// Reads the store on a background thread.
///
/// The folders are made first: a store that has never been used has nothing to list, and a
/// store that cannot be made is the same problem as one that cannot be read.
fn list(store: &Store) -> Command<Msg> {
    let store = store.clone();
    Command::perform(move || {
        if let Err(problem) = store.prepare() {
            return Msg::Listed { entries: Vec::new(), trouble: Some(problem.to_string()) };
        }
        let loaded = store.workspaces();
        // A store that cannot be read at all answers with no workspaces and one diagnostic;
        // anything else belongs to a workspace and is shown on its row.
        let trouble = match (loaded.value.is_empty(), loaded.diagnostics.first()) {
            (true, Some(problem)) => Some(problem.to_string()),
            _ => None,
        };
        Msg::Listed { entries: loaded.value, trouble }
    })
}

/// Opening a row: a broken workspace shows what is wrong with it, a whole one is handed to the
/// application to open.
fn activate(workspaces: &mut Workspaces, index: usize, opened: &mut Option<WorkspaceId>) -> Command<Msg> {
    match workspaces.entries.get(index) {
        None => Command::none(),
        Some(entry) if entry.is_broken() => {
            workspaces.selected = index;
            workspaces.overlay = Some(Overlay::Problems(index));
            Command::none()
        }
        Some(entry) => {
            workspaces.selected = index;
            // A workspace whose file could not be read has no identifier to open it by, and it is
            // already listed as broken, so there is nothing to hand on.
            *opened = entry.file.as_ref().map(|file| file.id.clone());
            Command::none()
        }
    }
}

/// Changes the draft and forgets what stopped the last attempt: the person has just answered it.
fn edit(workspaces: &mut Workspaces, change: impl FnOnce(&mut Draft)) -> Command<Msg> {
    if let Some(Overlay::New(draft)) = workspaces.overlay.as_mut() {
        change(draft);
        draft.problem = None;
    }
    Command::none()
}

/// Makes the workspace, and starts filling it when the source asks for it.
///
/// The folder is made here rather than on the task's thread, so a name that is taken is answered
/// while the person is still looking at the field they typed it in.
fn submit(workspaces: &mut Workspaces) -> Command<Msg> {
    let Some(Overlay::New(draft)) = workspaces.overlay.as_mut() else { return Command::none() };
    if draft.busy {
        return Command::none();
    }
    if let Some(problem) = draft.check(workspaces.store.root(), workspaces.engine.is_some()) {
        let field = problem.field();
        draft.problem = Some(problem);
        return Command::focus(field_input(field));
    }

    let file = match workspaces.store.create_workspace(&draft.name, Date::today_utc()) {
        Ok(file) => file,
        Err(error) => {
            let problem = Problem::from(error);
            let field = problem.field();
            draft.problem = Some(problem);
            return Command::focus(field_input(field));
        }
    };

    let paths = workspaces.store.workspace_paths(&file.id);
    let fill = match (draft.source, draft.folder.clone(), workspaces.engine.clone()) {
        (Source::Folder, Some(from), _) => Fill::Copy { from },
        (Source::Git, _, Some(engine)) => Fill::Clone { engine, url: draft.url.trim().to_owned() },
        // An empty workspace is whole the moment its folders are there.
        _ => {
            workspaces.overlay = None;
            return Command::batch([
                Command::toast(Toast::success(t!("workspaces.created-toast", name = file.name))),
                list(&workspaces.store),
            ]);
        }
    };

    let label = match draft.source {
        Source::Git => t!("workspaces.cloning", name = file.name.clone()),
        _ => t!("workspaces.copying", name = file.name.clone()),
    };
    let job = Job { fill, root: paths.root, into: paths.code, id: file.id.as_str().to_owned() };
    draft.busy = true;
    draft.name = file.name;
    draft.problem = None;
    workspaces.log.clear();
    workspaces.tasks.clear_finished();
    let task = Task::new(label, move |cx| fill::run(&job, cx)).on_event(Msg::Progress);
    workspaces.job = Some(task.id());
    Command::task(task)
}

/// Tidies up after the filling task, whichever way it ended.
///
/// A run that failed or was stopped has already removed the workspace folder, so all three ways
/// out end with the list being read again.
fn finished(workspaces: &mut Workspaces, outcome: &TaskOutcome) -> Command<Msg> {
    workspaces.job = None;
    let name = match workspaces.overlay.as_mut() {
        Some(Overlay::New(draft)) => {
            draft.busy = false;
            draft.name.clone()
        }
        _ => String::new(),
    };
    let told = match outcome {
        TaskOutcome::Done => {
            workspaces.overlay = None;
            Command::toast(Toast::success(t!("workspaces.created-toast", name = name)))
        }
        TaskOutcome::Cancelled => {
            workspaces.overlay = None;
            Command::toast(
                Toast::warning(t!("workspaces.cancelled-toast", name = name)).body(t!("workspaces.cancelled-body")),
            )
        }
        // A failure keeps the dialog open and is told inside it, above the form: it is about the
        // dialog's own action, and a toast would only wait behind the dialog until it closed.
        // The name is still there to try again with.
        TaskOutcome::Failed(detail) => {
            if let Some(Overlay::New(draft)) = workspaces.overlay.as_mut() {
                draft.problem = Some(Problem::Failed(detail.clone()));
            }
            Command::none()
        }
    };
    Command::batch([told, list(&workspaces.store)])
}

/// Starts deleting the workspace at `index` by asking the engine what it holds of it, so that the
/// question can name everything that goes. A workspace open in this QCode is refused at once.
fn ask_delete(workspaces: &mut Workspaces, index: usize) -> Command<Msg> {
    if workspaces.deleting {
        return Command::none();
    }
    let Some(entry) = workspaces.entries.get(index).cloned() else { return Command::none() };
    workspaces.overlay = None;
    let name = display_name(&entry);
    let open = entry.file.as_ref().is_some_and(|file| workspaces.open.contains(&file.id));
    if open {
        return Command::toast(
            Toast::warning(t!("workspaces.removal.open", name = name.as_str()))
                .body(t!("workspaces.removal.open-body")),
        );
    }
    workspaces.deleting = true;
    let store = workspaces.store.clone();
    let engine = workspaces.engine.clone();
    Command::perform(move || Msg::Surveyed(Box::new(remove::survey(&store, engine.as_ref(), &entry, name))))
}

/// Asks the question, once, naming what goes and what stays; or says why there is nothing to
/// ask, when a container of the workspace is running.
fn surveyed(workspaces: &mut Workspaces, survey: Box<Survey>) -> Command<Msg> {
    let running = survey.running();
    if !running.is_empty() {
        workspaces.deleting = false;
        return Command::toast(running_toast(&survey.name, running));
    }
    let message = format!(
        "{} {} {}",
        t!("workspaces.removal.folder", path = survey.dir.display().to_string()),
        engine_words(&survey.engine),
        t!("workspaces.removal.kept")
    );
    let question = Confirm::new(t!("workspaces.removal.title", name = survey.name.as_str()), Msg::DeleteConfirmed)
        .message(message)
        .confirm_label(t!("workspaces.removal.confirm"))
        .on_cancel(Msg::DeleteKept)
        .danger();
    workspaces.doomed = Some(survey);
    Command::confirm(question)
}

/// What the question says about the engine's part of the workspace.
fn engine_words(reach: &Reach) -> String {
    match reach {
        Reach::Absent => t!("workspaces.removal.no-engine"),
        Reach::Unreachable(said) => t!("workspaces.removal.unreachable", said = said.as_str()),
        Reach::Listed { containers, volumes, .. } if containers.is_empty() && volumes.is_empty() => {
            t!("workspaces.removal.nothing-in-engine")
        }
        Reach::Listed { containers, volumes, .. } => {
            let all: Vec<&str> = containers.iter().chain(volumes).map(String::as_str).collect();
            t!("workspaces.removal.engine", names = all.join(", "))
        }
    }
}

/// The toast that says a running container is what keeps a workspace from being deleted.
fn running_toast(name: &str, running: &[String]) -> Toast<Msg> {
    Toast::warning(t!("workspaces.removal.running", name = name))
        .body(t!("workspaces.removal.running-body", containers = running.join(", ")))
}

/// Deletes what the question was about, on a background thread.
fn delete(workspaces: &mut Workspaces) -> Command<Msg> {
    let Some(survey) = workspaces.doomed.take() else { return Command::none() };
    let store = workspaces.store.clone();
    let engine = workspaces.engine.clone();
    Command::perform(move || {
        let outcome = remove::remove(&store, engine.as_ref(), &survey);
        Msg::Deleted(survey, outcome)
    })
}

/// Says how the deletion went and reads the list again, which is the only honest account of what
/// is left.
fn deleted(workspaces: &mut Workspaces, survey: &Survey, outcome: Outcome) -> Command<Msg> {
    workspaces.deleting = false;
    let told = match outcome {
        Outcome::Deleted => {
            workspaces.deleted.clone_from(&survey.id);
            Toast::success(t!("workspaces.removal.done", name = survey.name.as_str()))
        }
        Outcome::Running(running) => running_toast(&survey.name, &running),
        Outcome::Partly(left) => {
            // The folder may be gone even so; whatever is left is named, and the list says the rest.
            if !survey.dir.exists() {
                workspaces.deleted.clone_from(&survey.id);
            }
            Toast::danger(t!("workspaces.removal.partly", name = survey.name.as_str())).body(left.join("\n"))
        }
    };
    Command::batch([Command::toast(told), list(&workspaces.store)])
}

/// Which input a problem asks the person to go back to.
fn field_input(field: &str) -> &'static str {
    match field {
        URL_FIELD => URL_INPUT,
        FOLDER_FIELD => LIST,
        _ => NAME_INPUT,
    }
}

/// The name of the address input.
const URL_INPUT: &str = "workspace-url";

/// The name the choice of where a new workspace starts from is focused by.
const SOURCE_GROUP: &str = "workspace-source";

/// Where the folder browser starts: the person's home folder, or the store when there is no
/// home folder to be had.
fn start_folder(store: &Store) -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| store.root().to_path_buf())
}

/// Draws the workspaces screen.
pub fn view(workspaces: &Workspaces, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.add(Text::new(t!("workspaces.title")).bold().no_wrap());
        body(workspaces, ui);
    })
    .fill()
    .gap(1);
    overlay(workspaces, ui);
}

/// The list, or whatever stands in its place.
fn body(workspaces: &Workspaces, ui: &mut View<'_, Msg>) {
    if !workspaces.listed {
        // Only the first reading shows a skeleton. Every later one keeps the list that is
        // already there until the new one arrives, so nothing blinks while it is refreshed.
        ui.add(Skeleton::lines(SKELETON_ROWS)).fill_width();
        return;
    }
    if let Some(trouble) = &workspaces.trouble {
        ui.add(
            EmptyState::new(t!("workspaces.unreadable-title"))
                .icon("error")
                .message(trouble.clone())
                .action(Button::new(t!("workspaces.refresh")).on_press(Msg::Refresh)),
        )
        .fill();
        return;
    }
    if workspaces.entries.is_empty() {
        ui.add(
            EmptyState::new(t!("workspaces.empty-title"))
                .icon("inbox")
                .message(t!("workspaces.empty-message"))
                .action(Button::new(t!("workspaces.new")).variant("primary").on_press(Msg::Start)),
        )
        .fill();
        return;
    }
    // The way to a new workspace is the list's first row, so the keyboard reaches it the way it
    // reaches every workspace, and the list and its one action start on the same edge.
    let new = ListItem::new(t!("workspaces.new")).icon("add", Some("accent"));
    let items = std::iter::once(new).chain(workspaces.entries.iter().map(|entry| row(workspaces, entry)));
    let selected = if workspaces.on_new { 0 } else { workspaces.selected + 1 };
    let list = List::new(items)
        .selected(Some(selected))
        .on_select(|row| row.checked_sub(1).map_or(Msg::SelectNew, Msg::Select))
        .on_activate(|row| row.checked_sub(1).map_or(Msg::Start, Msg::Activate));
    ui.add(list).id(LIST).fill();
    ui.row(|ui| {
        ui.spacer();
        let mut delete = Button::new(t!("workspaces.removal.button")).variant("danger").loading(workspaces.deleting);
        if !workspaces.deleting {
            delete = delete.on_press(Msg::Delete);
        }
        ui.add(delete).id("workspace-delete");
    })
    .fill_width();
}

/// One workspace's row: what it is called, and either why it is broken or when it was last seen.
fn row(workspaces: &Workspaces, entry: &WorkspaceEntry) -> ListItem {
    let item = ListItem::new(display_name(entry));
    if entry.is_broken() {
        let count = i64::try_from(entry.problems.len()).unwrap_or(i64::MAX);
        return item.icon("error", Some("danger")).detail(format!(
            "{} · {}",
            t!("workspaces.broken"),
            t!("workspaces.broken-detail", n = count)
        ));
    }
    item.icon("folder", None).detail(when(workspaces, entry))
}

/// What a row says on its right: that this is the workspace opened last, or the day it was made.
fn when(workspaces: &Workspaces, entry: &WorkspaceEntry) -> String {
    let id = entry.file.as_ref().map(|file| file.id.as_str());
    if let (Some(id), Some(recent)) = (id, workspaces.recent.first())
        && id == recent
    {
        return t!("workspaces.last-opened");
    }
    match entry.file.as_ref().and_then(|file| file.created) {
        Some(date) => t!("workspaces.created", date = date.to_string()),
        None => t!("workspaces.undated"),
    }
}

/// The name a workspace is shown by: what its file calls it, or its folder when the file is gone.
fn display_name(entry: &WorkspaceEntry) -> String {
    match &entry.file {
        Some(file) => file.name.clone(),
        None => entry.dir.file_name().unwrap_or(entry.dir.as_os_str()).to_string_lossy().into_owned(),
    }
}

/// Whatever is open over the list.
fn overlay(workspaces: &Workspaces, ui: &mut View<'_, Msg>) {
    match &workspaces.overlay {
        None => {}
        Some(Overlay::New(draft)) => new_workspace(workspaces, draft, ui),
        Some(Overlay::Problems(index)) => {
            if let Some(entry) = workspaces.entries.get(*index) {
                problems(entry, ui);
            }
        }
        Some(Overlay::Delete(chosen)) => choose_doomed(workspaces, *chosen, ui),
    }
}

/// The choice of the workspace to delete.
///
/// A click on a row of the list opens that workspace, so the list itself cannot also be where a
/// workspace is picked for deleting; this dialog is, and a click here only chooses. Nothing is
/// deleted from it: the question that names everything that goes comes after it.
fn choose_doomed(workspaces: &Workspaces, chosen: usize, ui: &mut View<'_, Msg>) {
    let dialog = Modal::new()
        .title(t!("workspaces.removal.choose-title"))
        .width(DIALOG_WIDTH)
        .on_close(Msg::Dismiss)
        .close_on_click_outside(true)
        .action(Button::new(t!("workspaces.cancel")).on_press(Msg::Dismiss));
    let items: Vec<ListItem> = workspaces.entries.iter().map(|entry| row(workspaces, entry)).collect();
    let rows = u16::try_from(items.len()).unwrap_or(u16::MAX).min(LOG_ROWS);
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(t!("workspaces.removal.choose-message")).role("secondary")).fill_width();
        ui.add(List::new(items).selected(Some(chosen)).on_select(Msg::DeleteSelect).on_activate(Msg::DeletePick))
            .id(DELETE_LIST)
            .height(Length::Cells(rows))
            .fill_width();
    });
}

/// The new-workspace dialog: the form, or the work it started.
fn new_workspace(workspaces: &Workspaces, draft: &Draft, ui: &mut View<'_, Msg>) {
    let mut dialog = Modal::new()
        .title(t!("workspaces.new-title"))
        .width(DIALOG_WIDTH)
        .on_close(Msg::Dismiss)
        // While the work runs, Esc and the close mark would leave a workspace half made; the way
        // out is the button that stops the work and undoes it.
        .dismissable(!draft.busy);
    dialog = match (draft.busy, workspaces.job) {
        (true, Some(id)) => dialog.action(Button::new(t!("workspaces.stop")).variant("danger").on_press(Msg::Stop(id))),
        _ => dialog
            .action(Button::new(t!("workspaces.cancel")).on_press(Msg::Dismiss))
            .action(Button::new(t!("workspaces.create")).variant("primary").on_press(Msg::Submit)),
    };
    ui.add_with(dialog, |ui| {
        if draft.busy {
            working(workspaces, ui);
        } else {
            form(workspaces, draft, ui);
        }
    });
}

/// The form of the dialog.
fn form(workspaces: &Workspaces, draft: &Draft, ui: &mut View<'_, Msg>) {
    let mut errors = FormErrors::new();
    if let Some(problem) = draft.problem.as_ref().or(draft.typing_problem().as_ref()) {
        errors.set(problem.field(), problem.message());
    }
    let engine = workspaces.engine.is_some();
    let preview = draft.preview().map(|id| t!("workspaces.name-preview", id = id));
    Form::new().summary(&errors).show(ui, |fields| {
        let mut name = Field::new(t!("workspaces.name-label")).required(true).error(errors.get(NAME_FIELD));
        if let Some(preview) = preview {
            name = name.hint(preview);
        }
        fields.field(name, |ui| {
            ui.add(
                TextInput::new(&draft.name)
                    .placeholder(t!("workspaces.name-placeholder"))
                    .invalid(errors.has(NAME_FIELD))
                    .on_change(Msg::Name)
                    .on_submit(|_| Msg::Submit),
            )
            .id(NAME_INPUT)
            .fill_width();
        });
        fields.field(Field::new(t!("workspaces.source-label")), |ui| {
            // A radio group rather than segments: its chosen option carries a mark of its own, so
            // the plain empty workspace reads as the one chosen, where a lit segment under a resting
            // pointer looked like a choice of its own.
            ui.add(
                RadioGroup::new(Source::ALL.map(Source::label))
                    .horizontal(true)
                    .selected(Some(draft.source.index()))
                    .on_select(Msg::Source),
            )
            .id(SOURCE_GROUP)
            .fill_width();
        });
        match draft.source {
            Source::Empty => {}
            Source::Folder => {
                let chosen = match &draft.folder {
                    Some(folder) => folder.display().to_string(),
                    None => t!("workspaces.folder-none"),
                };
                let field = Field::new(t!("workspaces.folder-label"))
                    .hint(t!("workspaces.folder-hint"))
                    .error(errors.get(FOLDER_FIELD));
                fields.field(field, |ui| {
                    ui.add(Text::new(chosen).role(if draft.folder.is_some() { "body" } else { "secondary" }))
                        .fill_width();
                });
                FilePicker::new(&draft.browser, Msg::Picker)
                    .show(fields.ui())
                    .height(Length::Cells(PICKER_ROWS))
                    .fill_width();
            }
            Source::Git => {
                let mut field = Field::new(t!("workspaces.url-label")).disabled(!engine).error(errors.get(URL_FIELD));
                if !engine {
                    field = field.hint(t!("workspaces.engine-missing"));
                }
                fields.field(field, |ui| {
                    ui.add(
                        TextInput::new(&draft.url)
                            .placeholder(t!("workspaces.url-placeholder"))
                            .disabled(!engine)
                            .invalid(errors.has(URL_FIELD))
                            .on_change(Msg::Url)
                            .on_submit(|_| Msg::Submit),
                    )
                    .id(URL_INPUT)
                    .fill_width();
                });
            }
        }
    });
}

/// The dialog while the copy or the clone runs: what it is doing, and everything it has said.
fn working(workspaces: &Workspaces, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        TaskList::new(&workspaces.tasks).show(ui).fill_width();
        ui.add(LogView::new(&workspaces.log).empty_text(t!("workspaces.log-empty")))
            .id("workspace-log")
            .height(Length::Cells(LOG_ROWS))
            .fill_width();
    })
    .fill_width()
    .gap(1);
}

/// What is wrong with a broken workspace, in the words of whoever found it.
fn problems(entry: &WorkspaceEntry, ui: &mut View<'_, Msg>) {
    let dialog = Modal::new()
        .title(t!("workspaces.broken-title", name = display_name(entry)))
        .width(DIALOG_WIDTH)
        .variant("danger")
        .on_close(Msg::Dismiss)
        .close_on_click_outside(true)
        .action(Button::new(t!("workspaces.close")).on_press(Msg::Dismiss));
    let lines: Vec<String> = entry.problems.iter().map(ToString::to_string).collect();
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(t!("workspaces.broken-message")).role("secondary")).fill_width();
        ui.add_with(ScrollView::new(), |ui| {
            for line in lines {
                ui.add(Text::new(line)).fill_width();
            }
        })
        .height(Length::Cells(LOG_ROWS))
        .fill_width();
    });
}

/// The keys of the workspaces screen that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("hints.move")), (icons.glyph("enter").into_owned(), t!("hints.open"))]
}
