//! The projects screen: the projects of the workspace, and the dialog that makes a new one.
//!
//! The screen is a state, an [`update`] and a [`view`], and it speaks its own [`Msg`], which
//! the application maps to its own. Send it [`Msg::Refresh`] when it opens;
//! everything else follows from what the person does. Opening a project is the one thing it
//! cannot do itself, so [`update`] hands that project's identifier back to the application.
//!
//! Three things it never does quietly. A name that is already taken is told to the person and
//! not turned into `name-2`. A folder chosen to start from is copied, never moved, so their own
//! copy stays where it is. A copy or a clone that fails or is stopped takes the project folder
//! it was filling with it, so a half-made project is never left on the list.

mod draft;
mod fill;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use qframe::date::Date;
use qframe::prelude::*;
use qframe::runtime::{Task, TaskEvent, TaskId, TaskOutcome, Tasks};
use qframe::widgets::{
    EmptyState, Field, FilePicker, FilePickerMsg, Form, FormErrors, LogBuffer, LogLevel, LogLine, LogView, Modal,
    RadioGroup, ScrollView, Skeleton, TaskList, TextInput, Toast,
};

use crate::engine::Engine;
use crate::workspace::{ProjectEntry, ProjectId, Workspace};

pub use draft::Source;

use draft::{Draft, FOLDER_FIELD, NAME_FIELD, Problem, URL_FIELD};
use fill::{Fill, Job};

/// Width of the dialogs, in cells. Wide enough for a path and a git address to read as one line
/// on a normal terminal; a narrower screen shrinks them.
const DIALOG_WIDTH: u16 = 68;

/// Rows the folder browser gets inside the dialog.
const PICKER_ROWS: u16 = 12;

/// Rows the live log gets while a project is being filled.
const LOG_ROWS: u16 = 10;

/// Rows the skeleton stands in for while the first listing is on its way.
const SKELETON_ROWS: u16 = 6;

/// How many lines of the log are kept. A clone of a large repository says a great deal, and only
/// the end of it is ever read.
const LOG_LINES: usize = 2000;

/// What can happen on the projects screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Read the workspace again. This is also the message that opens the screen.
    Refresh,
    /// The workspace answered.
    Listed {
        /// Every project folder that was found, broken ones included.
        entries: Vec<ProjectEntry>,
        /// Why the workspace itself could not be read, when that is what happened.
        trouble: Option<String>,
    },
    /// The selection moved to a row.
    Select(usize),
    /// A row was opened with Enter or a click.
    Activate(usize),
    /// Open the new-project dialog.
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
    /// Make the project.
    Submit,
    /// One line of the copy or the clone.
    Line(String),
    /// The filling task reported.
    Progress(TaskEvent),
    /// Stop the filling task.
    Stop(TaskId),
    /// The filling finished.
    Filled,
}

/// What is open over the list.
#[derive(Debug)]
enum Overlay {
    /// The new-project dialog.
    New(Box<Draft>),
    /// The problems of the broken project at this place in the list.
    Problems(usize),
}

/// The projects screen.
#[derive(Debug)]
pub struct Projects {
    workspace: Workspace,
    engine: Option<Engine>,
    recent: Vec<String>,
    entries: Vec<ProjectEntry>,
    trouble: Option<String>,
    listed: bool,
    selected: usize,
    overlay: Option<Overlay>,
    tasks: Tasks,
    log: LogBuffer,
    job: Option<TaskId>,
}

impl Projects {
    /// A screen over `workspace`, cloning inside `engine` when there is one, with `recent`
    /// holding the identifiers of the projects opened before, the most recent first.
    #[must_use]
    pub fn new(workspace: Workspace, engine: Option<Engine>, recent: Vec<String>) -> Self {
        Self {
            workspace,
            engine,
            recent,
            entries: Vec::new(),
            trouble: None,
            listed: false,
            selected: 0,
            overlay: None,
            tasks: Tasks::new(),
            log: LogBuffer::new(LOG_LINES),
            job: None,
        }
    }

    /// Whether a project is being filled right now, which is what makes leaving the screen
    /// something to ask about.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }
}

/// Applies a message of the projects screen, answering with the work to do and the project the
/// person opened, when they opened one.
pub fn update(projects: &mut Projects, message: Msg) -> (Command<Msg>, Option<ProjectId>) {
    let mut opened = None;
    let command = match message {
        Msg::Refresh => list(&projects.workspace),
        Msg::Listed { entries, trouble } => {
            let first = !projects.listed;
            projects.entries = entries;
            projects.trouble = trouble;
            projects.listed = true;
            projects.selected = projects.selected.min(projects.entries.len().saturating_sub(1));
            // The list is not on screen until the workspace has answered, so the keyboard is put
            // on it here rather than when the screen opened. Only the first answer does it: a
            // refresh must never take the keyboard off what the person is in the middle of.
            if first && !projects.entries.is_empty() && projects.overlay.is_none() {
                Command::focus(LIST)
            } else {
                Command::none()
            }
        }
        Msg::Select(index) => {
            if index < projects.entries.len() {
                projects.selected = index;
            }
            Command::none()
        }
        Msg::Activate(index) => activate(projects, index, &mut opened),
        Msg::Start => {
            let mut draft = Draft::new(start_folder(&projects.workspace));
            let reading = draft.browser.open(draft.browser.folder().to_path_buf(), Msg::Picker);
            projects.overlay = Some(Overlay::New(Box::new(draft)));
            Command::batch([reading, Command::focus(NAME_INPUT)])
        }
        Msg::Dismiss => {
            projects.overlay = None;
            Command::none()
        }
        Msg::Name(text) => edit(projects, |draft| draft.name = text),
        Msg::Source(index) => edit(projects, |draft| draft.source = Source::from_index(index)),
        Msg::Url(text) => edit(projects, |draft| draft.url = text),
        Msg::Picker(FilePickerMsg::Chosen(path)) => edit(projects, |draft| draft.folder = Some(path)),
        Msg::Picker(message) => match projects.overlay {
            Some(Overlay::New(ref mut draft)) => draft.browser.update(message, Msg::Picker),
            _ => Command::none(),
        },
        Msg::Submit => submit(projects),
        Msg::Line(text) => {
            projects.log.push(LogLine::new(LogLevel::Info, text));
            Command::none()
        }
        Msg::Progress(event) => {
            projects.tasks.apply(&event);
            match event {
                TaskEvent::Finished { id, outcome } if projects.job == Some(id) => finished(projects, &outcome),
                _ => Command::none(),
            }
        }
        Msg::Stop(id) => Command::cancel_task(id),
        // The task's own success message; the screen acts on the outcome that follows it, so
        // there is only one place where a finished job is tidied away.
        Msg::Filled => Command::none(),
    };
    (command, opened)
}

/// The name of the name input, so the dialog can put the cursor in it as it opens.
const NAME_INPUT: &str = "project-name";

/// The name of the list, for the focus and the tests.
const LIST: &str = "projects";

/// Reads the workspace on a background thread.
///
/// The folders are made first: a workspace that has never been used has nothing to list, and a
/// workspace that cannot be made is the same problem as one that cannot be read.
fn list(workspace: &Workspace) -> Command<Msg> {
    let workspace = workspace.clone();
    Command::perform(move || {
        if let Err(problem) = workspace.prepare() {
            return Msg::Listed { entries: Vec::new(), trouble: Some(problem.to_string()) };
        }
        let loaded = workspace.projects();
        // A workspace that cannot be read at all answers with no projects and one diagnostic;
        // anything else belongs to a project and is shown on its row.
        let trouble = match (loaded.value.is_empty(), loaded.diagnostics.first()) {
            (true, Some(problem)) => Some(problem.to_string()),
            _ => None,
        };
        Msg::Listed { entries: loaded.value, trouble }
    })
}

/// Opening a row: a broken project shows what is wrong with it, a whole one is handed to the
/// application to open.
fn activate(projects: &mut Projects, index: usize, opened: &mut Option<ProjectId>) -> Command<Msg> {
    match projects.entries.get(index) {
        None => Command::none(),
        Some(entry) if entry.is_broken() => {
            projects.selected = index;
            projects.overlay = Some(Overlay::Problems(index));
            Command::none()
        }
        Some(entry) => {
            projects.selected = index;
            // A project whose file could not be read has no identifier to open it by, and it is
            // already listed as broken, so there is nothing to hand on.
            *opened = entry.file.as_ref().map(|file| file.id.clone());
            Command::none()
        }
    }
}

/// Changes the draft and forgets what stopped the last attempt: the person has just answered it.
fn edit(projects: &mut Projects, change: impl FnOnce(&mut Draft)) -> Command<Msg> {
    if let Some(Overlay::New(draft)) = projects.overlay.as_mut() {
        change(draft);
        draft.problem = None;
    }
    Command::none()
}

/// Makes the project, and starts filling it when the source asks for it.
///
/// The folder is made here rather than on the task's thread, so a name that is taken is answered
/// while the person is still looking at the field they typed it in.
fn submit(projects: &mut Projects) -> Command<Msg> {
    let Some(Overlay::New(draft)) = projects.overlay.as_mut() else { return Command::none() };
    if draft.busy {
        return Command::none();
    }
    if let Some(problem) = draft.check(projects.workspace.root(), projects.engine.is_some()) {
        let field = problem.field();
        draft.problem = Some(problem);
        return Command::focus(field_input(field));
    }

    let file = match projects.workspace.create_project(&draft.name, Date::today_utc()) {
        Ok(file) => file,
        Err(error) => {
            let problem = Problem::from(error);
            let field = problem.field();
            draft.problem = Some(problem);
            return Command::focus(field_input(field));
        }
    };

    let paths = projects.workspace.project_paths(&file.id);
    let fill = match (draft.source, draft.folder.clone(), projects.engine.clone()) {
        (Source::Folder, Some(from), _) => Fill::Copy { from },
        (Source::Git, _, Some(engine)) => Fill::Clone { engine, url: draft.url.trim().to_owned() },
        // An empty project is whole the moment its folders are there.
        _ => {
            projects.overlay = None;
            return Command::batch([
                Command::toast(Toast::success(t!("projects.created-toast", name = file.name))),
                list(&projects.workspace),
            ]);
        }
    };

    let label = match draft.source {
        Source::Git => t!("projects.cloning", name = file.name.clone()),
        _ => t!("projects.copying", name = file.name.clone()),
    };
    let job = Job { fill, root: paths.root, into: paths.project, id: file.id.as_str().to_owned() };
    draft.busy = true;
    draft.name = file.name;
    draft.problem = None;
    projects.log.clear();
    projects.tasks.clear_finished();
    let task = Task::new(label, move |cx| fill::run(&job, cx)).on_event(Msg::Progress);
    projects.job = Some(task.id());
    Command::task(task)
}

/// Tidies up after the filling task, whichever way it ended.
///
/// A run that failed or was stopped has already removed the project folder, so all three ways
/// out end with the list being read again.
fn finished(projects: &mut Projects, outcome: &TaskOutcome) -> Command<Msg> {
    projects.job = None;
    let name = match projects.overlay.as_mut() {
        Some(Overlay::New(draft)) => {
            draft.busy = false;
            draft.name.clone()
        }
        _ => String::new(),
    };
    let told = match outcome {
        TaskOutcome::Done => {
            projects.overlay = None;
            Command::toast(Toast::success(t!("projects.created-toast", name = name)))
        }
        TaskOutcome::Cancelled => {
            projects.overlay = None;
            Command::toast(
                Toast::warning(t!("projects.cancelled-toast", name = name)).body(t!("projects.cancelled-body")),
            )
        }
        // A failure keeps the dialog open and is told inside it, above the form: it is about the
        // dialog's own action, and a toast would only wait behind the dialog until it closed.
        // The name is still there to try again with.
        TaskOutcome::Failed(detail) => {
            if let Some(Overlay::New(draft)) = projects.overlay.as_mut() {
                draft.problem = Some(Problem::Failed(detail.clone()));
            }
            Command::none()
        }
    };
    Command::batch([told, list(&projects.workspace)])
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
const URL_INPUT: &str = "project-url";

/// The name the choice of where a new project starts from is focused by.
const SOURCE_GROUP: &str = "project-source";

/// Where the folder browser starts: the person's home folder, or the workspace when there is no
/// home folder to be had.
fn start_folder(workspace: &Workspace) -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| workspace.root().to_path_buf())
}

/// Draws the projects screen.
pub fn view(projects: &Projects, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Text::new(t!("projects.title")).bold().no_wrap());
            ui.spacer();
            ui.add(Button::new(t!("projects.new")).icon("add").variant("primary").on_press(Msg::Start));
        })
        .fill_width()
        .gap(2);
        body(projects, ui);
    })
    .fill()
    .gap(1);
    overlay(projects, ui);
}

/// The list, or whatever stands in its place.
fn body(projects: &Projects, ui: &mut View<'_, Msg>) {
    if !projects.listed {
        // Only the first reading shows a skeleton. Every later one keeps the list that is
        // already there until the new one arrives, so nothing blinks while it is refreshed.
        ui.add(Skeleton::lines(SKELETON_ROWS)).fill_width();
        return;
    }
    if let Some(trouble) = &projects.trouble {
        ui.add(
            EmptyState::new(t!("projects.unreadable-title"))
                .icon("error")
                .message(trouble.clone())
                .action(Button::new(t!("projects.refresh")).on_press(Msg::Refresh)),
        )
        .fill();
        return;
    }
    if projects.entries.is_empty() {
        ui.add(
            EmptyState::new(t!("projects.empty-title"))
                .icon("inbox")
                .message(t!("projects.empty-message"))
                .action(Button::new(t!("projects.new")).variant("primary").on_press(Msg::Start)),
        )
        .fill();
        return;
    }
    let items = projects.entries.iter().map(|entry| row(projects, entry));
    ui.add(List::new(items).selected(Some(projects.selected)).on_select(Msg::Select).on_activate(Msg::Activate))
        .id(LIST)
        .fill();
}

/// One project's row: what it is called, and either why it is broken or when it was last seen.
fn row(projects: &Projects, entry: &ProjectEntry) -> ListItem {
    let item = ListItem::new(display_name(entry));
    if entry.is_broken() {
        let count = i64::try_from(entry.problems.len()).unwrap_or(i64::MAX);
        return item.icon("error", Some("danger")).detail(format!(
            "{} · {}",
            t!("projects.broken"),
            t!("projects.broken-detail", n = count)
        ));
    }
    item.icon("folder", None).detail(when(projects, entry))
}

/// What a row says on its right: that this is the project opened last, or the day it was made.
fn when(projects: &Projects, entry: &ProjectEntry) -> String {
    let id = entry.file.as_ref().map(|file| file.id.as_str());
    if let (Some(id), Some(recent)) = (id, projects.recent.first())
        && id == recent
    {
        return t!("projects.last-opened");
    }
    match entry.file.as_ref().and_then(|file| file.created) {
        Some(date) => t!("projects.created", date = date.to_string()),
        None => t!("projects.undated"),
    }
}

/// The name a project is shown by: what its file calls it, or its folder when the file is gone.
fn display_name(entry: &ProjectEntry) -> String {
    match &entry.file {
        Some(file) => file.name.clone(),
        None => entry.dir.file_name().unwrap_or(entry.dir.as_os_str()).to_string_lossy().into_owned(),
    }
}

/// Whatever is open over the list.
fn overlay(projects: &Projects, ui: &mut View<'_, Msg>) {
    match &projects.overlay {
        None => {}
        Some(Overlay::New(draft)) => new_project(projects, draft, ui),
        Some(Overlay::Problems(index)) => {
            if let Some(entry) = projects.entries.get(*index) {
                problems(entry, ui);
            }
        }
    }
}

/// The new-project dialog: the form, or the work it started.
fn new_project(projects: &Projects, draft: &Draft, ui: &mut View<'_, Msg>) {
    let mut dialog = Modal::new()
        .title(t!("projects.new-title"))
        .width(DIALOG_WIDTH)
        .on_close(Msg::Dismiss)
        // While the work runs, Esc and the close mark would leave a project half made; the way
        // out is the button that stops the work and undoes it.
        .dismissable(!draft.busy);
    dialog = match (draft.busy, projects.job) {
        (true, Some(id)) => dialog.action(Button::new(t!("projects.stop")).variant("danger").on_press(Msg::Stop(id))),
        _ => dialog
            .action(Button::new(t!("projects.cancel")).on_press(Msg::Dismiss))
            .action(Button::new(t!("projects.create")).variant("primary").on_press(Msg::Submit)),
    };
    ui.add_with(dialog, |ui| {
        if draft.busy {
            working(projects, ui);
        } else {
            form(projects, draft, ui);
        }
    });
}

/// The form of the dialog.
fn form(projects: &Projects, draft: &Draft, ui: &mut View<'_, Msg>) {
    let mut errors = FormErrors::new();
    if let Some(problem) = draft.problem.as_ref().or(draft.typing_problem().as_ref()) {
        errors.set(problem.field(), problem.message());
    }
    let engine = projects.engine.is_some();
    let preview = draft.preview().map(|id| t!("projects.name-preview", id = id));
    Form::new().summary(&errors).show(ui, |fields| {
        let mut name = Field::new(t!("projects.name-label")).required(true).error(errors.get(NAME_FIELD));
        if let Some(preview) = preview {
            name = name.hint(preview);
        }
        fields.field(name, |ui| {
            ui.add(
                TextInput::new(&draft.name)
                    .placeholder(t!("projects.name-placeholder"))
                    .invalid(errors.has(NAME_FIELD))
                    .on_change(Msg::Name)
                    .on_submit(|_| Msg::Submit),
            )
            .id(NAME_INPUT)
            .fill_width();
        });
        fields.field(Field::new(t!("projects.source-label")), |ui| {
            // A radio group rather than segments: its chosen option carries a mark of its own, so
            // the plain empty project reads as the one chosen, where a lit segment under a resting
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
                    None => t!("projects.folder-none"),
                };
                let field = Field::new(t!("projects.folder-label"))
                    .hint(t!("projects.folder-hint"))
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
                let mut field = Field::new(t!("projects.url-label")).disabled(!engine).error(errors.get(URL_FIELD));
                if !engine {
                    field = field.hint(t!("projects.engine-missing"));
                }
                fields.field(field, |ui| {
                    ui.add(
                        TextInput::new(&draft.url)
                            .placeholder(t!("projects.url-placeholder"))
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
fn working(projects: &Projects, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        TaskList::new(&projects.tasks).show(ui).fill_width();
        ui.add(LogView::new(&projects.log).empty_text(t!("projects.log-empty")))
            .id("project-log")
            .height(Length::Cells(LOG_ROWS))
            .fill_width();
    })
    .fill_width()
    .gap(1);
}

/// What is wrong with a broken project, in the words of whoever found it.
fn problems(entry: &ProjectEntry, ui: &mut View<'_, Msg>) {
    let dialog = Modal::new()
        .title(t!("projects.broken-title", name = display_name(entry)))
        .width(DIALOG_WIDTH)
        .variant("danger")
        .on_close(Msg::Dismiss)
        .close_on_click_outside(true)
        .action(Button::new(t!("projects.close")).on_press(Msg::Dismiss));
    let lines: Vec<String> = entry.problems.iter().map(ToString::to_string).collect();
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(t!("projects.broken-message")).role("secondary")).fill_width();
        ui.add_with(ScrollView::new(), |ui| {
            for line in lines {
                ui.add(Text::new(line)).fill_width();
            }
        })
        .height(Length::Cells(LOG_ROWS))
        .fill_width();
    });
}

/// The keys of the projects screen that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("hints.move")), (icons.glyph("enter").into_owned(), t!("hints.open"))]
}
