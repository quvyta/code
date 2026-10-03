//! The providers screen: the model services a person added, what each one offers, and the two
//! context figures that are never the same number.
//!
//! Three things make this screen what it is.
//!
//! **Nothing goes out unasked.** Opening the page reads a file and touches no network. Every
//! request leaves only because the person pressed the thing that sends it, and the address it
//! will go to stands on the page above that button, before it goes.
//!
//! **The key file is named out loud.** Not in a footnote: a line of its own says where the key
//! is written and that a backup of the home folder carries it in plain text, so that nobody
//! shares a backup and their key together without knowing.
//!
//! **Both windows are shown.** What the model's own record claims and what this server was
//! measured giving, side by side, because they are often an order of magnitude apart and a
//! coding agent whose instructions are silently thrown away looks stupid for no visible reason.
//! When the measured window is small the page says so plainly and says what the person can do
//! about it on their own machine — guidance QCode prints and never something QCode does to them.

mod draft;
mod lineup;
#[cfg(test)]
mod tests;
mod tried;

use std::path::PathBuf;

use qframe::diagnostics::Diagnostic;
use qframe::prelude::*;
use qframe::widgets::{EmptyState, Field, Modal, RadioGroup, TextInput};

use crate::provider::{
    Ask, AskError, CRAMPED, Key, Measured, Model, PermissionProblem, Price, ProviderEntry, ProviderKind,
    Providers as ProviderFile, Reached, Web, Wire, ask, permission_problems,
};
use crate::store::Loaded;
use crate::ui::page;

pub use draft::{BASE_FIELD, Draft, KEY_FIELD, Problem, TAG_FIELD};
pub use lineup::{Editor, Lineups, Refusal};

use lineup::{FILTER_FIELD, LIST as LINEUPS, MODELS as LINEUP_MODELS, NAME_FIELD, STEPS as LINEUP_STEPS};
use tried::{Reason, TRIED, Verdict};

/// Width of the add dialog, and of the lineups dialog beside it. Wide enough for an address to
/// read as one line, the longest being a ready-made service's API root with the name of its shape
/// in front of it, and for a model's own name in a list of them.
const DIALOG_WIDTH: u16 = 72;

/// The name of the list of providers, for the focus and the tests.
const LIST: &str = "providers";

/// The name of the list of models.
const MODELS: &str = "provider-models";

/// The rows the lineups dialog spends on its own frame: the title and the blank row under it, the
/// row of buttons, the row of hints under them, the air above and below, and the row the
/// framework leaves either side of the body. Inside a layer the view reports the size of the whole
/// screen, so this and the rows below are measured against what the terminal is, not against what
/// the dialog is given of it.
const DIALOG_FRAME: u16 = 8;

/// Rows the editor spends on everything that is not the list of the provider's models: the name
/// with its label and the line of help under it, the filter with its label, the lineup's own
/// steps, the two lines a warning about a price or a refusal takes, and the blank row between
/// each pair. A dialog is measured to what is inside it, so a list left to itself is as tall as
/// the one or two models in it and there is nothing to scroll in. The steps are counted here at
/// the two rows they take on the shortest terminal, and the rows a taller screen has over and
/// above those two are the steps' to grow into, the models having stopped at their own most.
const ROWS_AROUND_MODELS: u16 = 13;

/// Fewest rows the list of the provider's models takes: a list of one row is no list to move
/// through, and on the shortest terminal there is room for nothing more.
const MODEL_ROWS_MIN: u16 = 2;

/// Most: a list taller than this is further from the steps under it than the eye follows in one
/// go, and the rows a tall terminal has spare are better spent on the order than on more of the
/// hundred models the filter is there to find.
const MODEL_ROWS_MAX: u16 = 12;

/// Fewest rows the lineup's own steps take, and what they take wherever the models list has been
/// given every row there was. An order is a handful of models: the list grows and scrolls rather
/// than taking the row of buttons away.
const STEP_ROWS: u16 = 2;

/// Most: an order is a handful of models, so a list of steps taller than this is taller than any
/// lineup there is, and the rows beyond it are better left to the page behind. The steps are
/// given their rows only out of what the models list left over, so nothing above them is cut
/// short to reach this many.
const STEP_ROWS_MAX: u16 = 8;

/// The rows of the screen the editor's two lists share between them once the dialog's own rows
/// are off it, before either list has been given what it may take of them. Both lists are
/// measured against this, so their rows together come to no more than the screen holds.
fn rows_for_the_two_lists(ui: &View<'_, Msg>) -> u16 {
    ui.size().height.saturating_sub(DIALOG_FRAME + ROWS_AROUND_MODELS)
}

/// The rows the list of the provider's models takes. The dialog's own rows come off the screen
/// first, so however many models a provider offers, and however short the terminal is, the
/// buttons under them stay where a hand can reach them.
fn model_rows(ui: &View<'_, Msg>) -> u16 {
    rows_for_the_two_lists(ui).clamp(MODEL_ROWS_MIN, MODEL_ROWS_MAX)
}

/// The rows the lineup's own steps take. They begin at the two the shortest terminal can spare,
/// where an order is read two steps at a time, and grow into what the models list left over: a
/// lineup of six steps is not read at all through two rows of it, and the rows a tall terminal
/// has spare say more of the order rather than more of the hundred models above it.
fn step_rows(ui: &View<'_, Msg>) -> u16 {
    let spare = rows_for_the_two_lists(ui);
    (STEP_ROWS + spare.saturating_sub(model_rows(ui))).clamp(STEP_ROWS, STEP_ROWS_MAX)
}

/// What the screen is waiting for, so a button that has been pressed shows the work and takes no
/// second press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// The connection is being tried.
    Trying,
    /// The provider is being asked what it offers.
    Listing,
    /// A model's real window is being measured.
    Measuring,
}

/// The last thing that happened, kept as a value so that the view is the only place that turns
/// it into a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The connection was tried and something answered.
    Reached(Reached),
    /// A request did not get an answer it could use.
    Trouble(AskError),
    /// The providers file could not be written; the text is what the file system said.
    NotWritten(String),
}

/// Everything that can happen on the providers screen.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Read the providers file again. This is also how the screen is started.
    Reload,
    /// The file was read, and how wide its permissions and its folder's are was looked at.
    Loaded(Box<Loaded<ProviderFile>>, Vec<PermissionProblem>),
    /// The selection moved to a provider.
    Select(usize),
    /// The selection moved to a model.
    PickModel(usize),
    /// Open the add dialog.
    New,
    /// Leave the add dialog, keeping nothing.
    Cancel,
    /// The tag was typed.
    Tag(String),
    /// The address was typed.
    Base(String),
    /// The key was pasted.
    PasteKey(String),
    /// A kind was chosen.
    PickKind(usize),
    /// Where a ready-made provider answers was chosen, by its place among its kind's regions.
    PickRegion(usize),
    /// Add the provider the dialog describes.
    Add,
    /// Deleting the chosen provider was asked for.
    DeleteAsked,
    /// The question was answered with yes.
    DeleteConfirmed,
    /// Forget the chosen provider's key, keeping the provider.
    ForgetKey,
    /// Try the connection to the chosen provider. Nothing else on this screen sends a request.
    Try,
    /// The trial answered.
    Tried(Box<Result<Reached, AskError>>),
    /// Ask the chosen provider what it offers.
    Refresh,
    /// The provider answered with what it offers.
    Listed(Box<Result<Vec<Model>, AskError>>),
    /// Measure what the chosen model really gets on this server.
    MeasureAsked,
    /// The measurement finished, for the model of that name.
    Measured(String, Box<Result<Measured, AskError>>),
    /// Open the lineups of the chosen provider.
    LineupsAsked,
    /// The lineups dialog was closed, keeping whatever was already saved.
    LineupsClosed,
    /// A lineup of the list was chosen.
    LineupChosen(usize),
    /// Start a lineup that is not there yet.
    LineupNew,
    /// Start the chosen lineup for changing.
    LineupEditAsked,
    /// Deleting the chosen lineup was asked for.
    LineupDeleteAsked,
    /// The question was answered with yes.
    LineupDeleteConfirmed,
    /// The name of the lineup being written was typed.
    LineupName(String),
    /// The provider's models were filtered.
    LineupFilter(String),
    /// A model of the list was pointed at.
    LineupLooked(String),
    /// The model at that place of the filtered list was put at the end of the lineup.
    LineupAdded(usize),
    /// A step of the lineup was chosen.
    LineupStep(usize),
    /// The chosen step goes one place earlier.
    LineupUp,
    /// The chosen step goes one place later.
    LineupDown,
    /// The chosen step comes out of the lineup.
    LineupRemove,
    /// The lineup being written is kept.
    LineupSave,
    /// Leave the lineup being written, keeping nothing of it.
    LineupCancel,
}

/// The providers screen's state.
#[derive(Debug, Clone)]
pub struct Providers {
    /// Where the providers file is, or `None` on a machine that gives QCode no data folder.
    path: Option<PathBuf>,
    file: ProviderFile,
    problems: Vec<Diagnostic>,
    permissions: Vec<PermissionProblem>,
    selected: usize,
    model: usize,
    draft: Option<Draft>,
    lineups: Option<Lineups>,
    web: Web,
    busy: Option<Busy>,
    notice: Option<Notice>,
    loading: bool,
}

impl Providers {
    /// A screen over the providers file at `path`, asking questions through `web`.
    ///
    /// Both are parameters: a test gives a file of its own and a web that answers from a string,
    /// so the whole screen is driven without a data folder or a server anywhere.
    #[must_use]
    pub fn new(path: Option<PathBuf>, web: Web) -> Self {
        Self {
            path,
            file: ProviderFile::in_memory(),
            problems: Vec::new(),
            permissions: Vec::new(),
            selected: 0,
            model: 0,
            draft: None,
            lineups: None,
            web,
            busy: None,
            notice: None,
            loading: false,
        }
    }

    /// The providers file this screen reads and writes, or `None` on a machine that gives QCode
    /// no data folder.
    #[must_use]
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// The providers, in the order they were added.
    #[must_use]
    pub fn entries(&self) -> &[ProviderEntry] {
        self.file.entries()
    }

    /// The provider the person is looking at.
    #[must_use]
    pub fn selected(&self) -> Option<&ProviderEntry> {
        self.entries().get(self.selected)
    }

    /// The model of that provider the person is looking at.
    #[must_use]
    pub fn chosen_model(&self) -> Option<&Model> {
        self.selected()?.models.get(self.model)
    }

    /// What was wrong with the providers file.
    #[must_use]
    pub fn problems(&self) -> &[Diagnostic] {
        &self.problems
    }

    /// The places, the file or its folder, whose permissions let others on this machine read the
    /// key, as they were when the file was last read or written.
    #[must_use]
    pub fn permissions(&self) -> &[PermissionProblem] {
        &self.permissions
    }

    /// The add dialog, while one is open.
    #[must_use]
    pub fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }

    /// The lineups dialog of the chosen provider, while one is open.
    #[must_use]
    pub fn lineups(&self) -> Option<&Lineups> {
        self.lineups.as_ref()
    }

    /// What the screen is waiting for.
    #[must_use]
    pub fn busy(&self) -> Option<Busy> {
        self.busy
    }

    /// The last thing that happened.
    #[must_use]
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// Writes the file and remembers a refusal, which the page says out loud rather than losing.
    fn save(&mut self) {
        if let Err(reason) = self.file.save() {
            self.notice = Some(Notice::NotWritten(reason));
        }
        // Saving narrows the file and its folder, so a warning read before it may no longer hold;
        // one that still does, because the folder could not be narrowed, stays.
        if let Some(path) = &self.path {
            self.permissions = permission_problems(path);
        }
    }
}

/// The control that takes the keyboard when the screen opens, once there is one: the list, or on
/// an empty screen its one button.
#[must_use]
pub fn entry(state: &Providers) -> Option<&'static str> {
    if state.loading {
        None
    } else if state.entries().is_empty() {
        Some("providers-empty")
    } else {
        Some(LIST)
    }
}

/// The keys of the providers screen that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("hints.move")), (icons.glyph("enter").into_owned(), t!("hints.open"))]
}

/// Applies a providers screen message.
pub fn update(state: &mut Providers, message: Msg) -> Command<Msg> {
    match message {
        Msg::Reload => {
            state.loading = true;
            let path = state.path.clone();
            Command::perform(move || {
                let (loaded, permissions) = match &path {
                    Some(path) => (ProviderFile::open(path), permission_problems(path)),
                    None => (Loaded { value: ProviderFile::in_memory(), diagnostics: Vec::new() }, Vec::new()),
                };
                Msg::Loaded(Box::new(loaded), permissions)
            })
        }
        Msg::Loaded(loaded, permissions) => {
            let Loaded { mut value, diagnostics } = *loaded;
            state.permissions = permissions;
            if let Some(path) = &state.path {
                value = value.at(path);
            }
            state.file = value;
            // A file written before the record of tried models, or before a model in it was
            // tried, is shown in the same order a fresh answer would be.
            for mut entry in state.file.entries().to_vec() {
                ordered(&mut entry);
                let tag = entry.tag.as_str().to_owned();
                state.file.replace(&tag, entry);
            }
            state.problems = diagnostics;
            state.loading = false;
            state.selected = state.selected.min(state.entries().len().saturating_sub(1));
            state.model = 0;
            match entry(state) {
                Some(control) => Command::focus(control),
                None => Command::none(),
            }
        }
        Msg::Select(index) => {
            if index < state.entries().len() && index != state.selected {
                state.selected = index;
                // The models, the notice and anything being waited for all belong to the
                // provider that was chosen, not to this one.
                state.model = 0;
                state.notice = None;
            }
            Command::none()
        }
        Msg::PickModel(index) => {
            if index < state.selected().map_or(0, |entry| entry.models.len()) {
                state.model = index;
            }
            Command::none()
        }
        Msg::New => {
            state.draft = Some(Draft::new());
            Command::focus(TAG_FIELD)
        }
        Msg::Cancel => {
            // The key the person was typing goes with the dialog: nothing keeps it.
            state.draft = None;
            match entry(state) {
                Some(control) => Command::focus(control),
                None => Command::none(),
            }
        }
        Msg::Tag(text) => {
            if let Some(draft) = &mut state.draft {
                draft.tag = text;
                draft.problem = None;
            }
            Command::none()
        }
        Msg::Base(text) => {
            if let Some(draft) = &mut state.draft {
                draft.base = text;
                draft.problem = None;
            }
            Command::none()
        }
        Msg::PasteKey(text) => {
            if let Some(draft) = &mut state.draft {
                draft.key = text;
                draft.problem = None;
            }
            Command::none()
        }
        Msg::PickKind(index) => {
            if let (Some(draft), Some(kind)) = (&mut state.draft, ProviderKind::ALL.get(index).copied()) {
                draft.pick_kind(kind);
            }
            Command::none()
        }
        Msg::PickRegion(index) => {
            if let Some(draft) = &mut state.draft {
                draft.pick_region(index);
            }
            Command::none()
        }
        Msg::Add => add(state),
        Msg::DeleteAsked => match state.selected() {
            Some(entry) => Command::confirm(
                Confirm::new(t!("provider.delete-title", tag = entry.tag.as_str()), Msg::DeleteConfirmed)
                    .message(t!("provider.delete-message"))
                    .confirm_label(t!("provider.delete"))
                    .danger(),
            ),
            None => Command::none(),
        },
        Msg::DeleteConfirmed => {
            let Some(tag) = state.selected().map(|entry| entry.tag.as_str().to_owned()) else {
                return Command::none();
            };
            state.file.remove(&tag);
            state.selected = state.selected.min(state.entries().len().saturating_sub(1));
            state.model = 0;
            state.notice = None;
            state.save();
            Command::none()
        }
        Msg::ForgetKey => {
            let Some(tag) = state.selected().map(|entry| entry.tag.as_str().to_owned()) else {
                return Command::none();
            };
            state.file.forget_key(&tag);
            state.save();
            Command::none()
        }
        Msg::Try => start(state, Busy::Trying, |web, entry| Msg::Tried(Box::new(ask::try_connection(&web, &entry)))),
        Msg::Tried(answer) => {
            state.busy = None;
            state.notice = Some(match *answer {
                Ok(reached) => Notice::Reached(reached),
                Err(trouble) => Notice::Trouble(trouble),
            });
            Command::none()
        }
        Msg::Refresh => start(state, Busy::Listing, |web, entry| Msg::Listed(Box::new(ask::list_models(&web, &entry)))),
        Msg::Listed(answer) => {
            state.busy = None;
            match *answer {
                Ok(models) => {
                    let Some(mut entry) = state.selected().cloned() else { return Command::none() };
                    // A measurement is work the person asked for and waited through; asking a
                    // provider what it offers again must not quietly throw it away.
                    entry.models = models
                        .into_iter()
                        .map(|mut model| {
                            model.measured = entry.model(&model.id).and_then(|before| before.measured);
                            model
                        })
                        .collect();
                    ordered(&mut entry);
                    let tag = entry.tag.as_str().to_owned();
                    state.file.replace(&tag, entry);
                    state.model = 0;
                    state.notice = None;
                    state.save();
                }
                Err(trouble) => state.notice = Some(Notice::Trouble(trouble)),
            }
            Command::none()
        }
        Msg::MeasureAsked => {
            let (Some(entry), Some(model)) = (state.selected().cloned(), state.chosen_model().cloned()) else {
                return Command::none();
            };
            if state.busy.is_some() {
                return Command::none();
            }
            state.busy = Some(Busy::Measuring);
            state.notice = None;
            let web = state.web.clone();
            let id = model.id.clone();
            Command::perform(move || {
                let measured = ask::measure(&web, &entry, &id);
                Msg::Measured(id, Box::new(measured))
            })
        }
        Msg::Measured(id, answer) => {
            state.busy = None;
            match *answer {
                Ok(measured) => {
                    let Some(mut entry) = state.selected().cloned() else { return Command::none() };
                    if let Some(model) = entry.models.iter_mut().find(|model| model.id == id) {
                        model.measured = Some(measured);
                    }
                    let tag = entry.tag.as_str().to_owned();
                    state.file.replace(&tag, entry);
                    state.save();
                }
                Err(trouble) => state.notice = Some(Notice::Trouble(trouble)),
            }
            Command::none()
        }
        Msg::LineupsAsked => {
            let Some(entry) = state.selected() else { return Command::none() };
            let mut dialog = Lineups::new();
            dialog.open(entry);
            state.lineups = Some(dialog);
            Command::focus(LINEUPS)
        }
        Msg::LineupsClosed => {
            // A lineup left half-written is not kept: it reaches the file when it is saved, as
            // every other thing on this page.
            state.lineups = None;
            back(state)
        }
        Msg::LineupChosen(index) => {
            let chosen =
                state.selected().and_then(|entry| entry.lineups.get(index)).map(|l| l.name.as_str().to_owned());
            if let (Some(name), Some(dialog)) = (chosen, state.lineups.as_mut()) {
                dialog.choose(&name);
            }
            Command::none()
        }
        Msg::LineupNew => {
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.new_lineup();
            }
            Command::focus(NAME_FIELD)
        }
        Msg::LineupEditAsked => {
            // The lineup is read out of the file before the dialog is touched: what is edited is
            // what the file holds now, not what this screen last drew of it.
            let editing = state
                .lineups
                .as_ref()
                .and_then(|dialog| dialog.chosen.clone())
                .and_then(|name| state.selected().and_then(|entry| entry.lineup(&name)).cloned());
            if let (Some(lineup), Some(dialog)) = (editing, state.lineups.as_mut()) {
                dialog.edit(&lineup);
            }
            Command::focus(NAME_FIELD)
        }
        Msg::LineupDeleteAsked => {
            let chosen = state.lineups.as_ref().and_then(|dialog| dialog.chosen.clone());
            let Some(name) = chosen else { return Command::none() };
            Command::confirm(
                Confirm::new(t!("provider.lineup-delete-title", name = name.as_str()), Msg::LineupDeleteConfirmed)
                    .message(t!("provider.lineup-delete-message"))
                    .confirm_label(t!("provider.lineup-delete"))
                    .danger(),
            )
        }
        Msg::LineupDeleteConfirmed => {
            let chosen = state.lineups.as_ref().and_then(|dialog| dialog.chosen.clone());
            let Some(name) = chosen else { return Command::none() };
            let Some(tag) = state.selected().map(|entry| entry.tag.as_str().to_owned()) else {
                return Command::none();
            };
            state.file.remove_lineup(&tag, &name);
            state.save();
            // The list under the hand has one row fewer: the lineup that was there keeps the
            // choice if it is still there, and the first of what is left takes it when it is not.
            let left = lineup_names(&state.file, &tag);
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.kept(&left);
            }
            Command::none()
        }
        Msg::LineupName(text) => {
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.name(text);
            }
            Command::none()
        }
        Msg::LineupFilter(text) => {
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.filter_for(text);
            }
            Command::none()
        }
        Msg::LineupLooked(id) => {
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.look_at(&id);
            }
            Command::none()
        }
        Msg::LineupAdded(index) => {
            // The index is where the model stood in the list the filter leaves. The filter is
            // written as it is typed, so the model is read out of that list by its place in it and
            // the person is credited with the row they pointed at, not with a number.
            let id = state
                .lineups
                .as_ref()
                .zip(state.selected())
                .and_then(|(dialog, entry)| dialog.shown(entry).get(index).map(|model| model.id.clone()));
            if let (Some(id), Some(dialog)) = (id, state.lineups.as_mut()) {
                dialog.add(&id);
            }
            Command::none()
        }
        Msg::LineupStep(index) => {
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.pick_step(index);
            }
            Command::none()
        }
        Msg::LineupUp | Msg::LineupDown | Msg::LineupRemove => {
            if let Some(dialog) = state.lineups.as_mut() {
                match message {
                    Msg::LineupUp => dialog.up(),
                    Msg::LineupDown => dialog.down(),
                    _ => dialog.remove(),
                }
            }
            Command::none()
        }
        Msg::LineupSave => save_lineup(state),
        Msg::LineupCancel => {
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.cancelled();
            }
            Command::none()
        }
    }
}

/// Where the keyboard goes when a dialog over this page closes. The list is what both ways out of
/// it mean to leave it, the chosen provider named or nothing changed.
fn back(state: &Providers) -> Command<Msg> {
    match entry(state) {
        Some(control) => Command::focus(control),
        None => Command::none(),
    }
}

/// The names of the lineups the provider tagged `tag` has, in the order it has them.
fn lineup_names(file: &ProviderFile, tag: &str) -> Vec<String> {
    file.get(tag)
        .map_or_else(Vec::new, |entry| entry.lineups.iter().map(|lineup| lineup.name.as_str().to_owned()).collect())
}

/// Keeps the lineup the editor describes, or leaves the editor open with the reason it cannot be
/// kept. A lineup is written into the file as it is on any other change here, and the page shows
/// it the moment it is saved.
fn save_lineup(state: &mut Providers) -> Command<Msg> {
    let (Some(tag), Some(dialog)) =
        (state.selected().map(|entry| entry.tag.as_str().to_owned()), state.lineups.as_ref())
    else {
        return Command::none();
    };
    let file = &state.file;
    let taken = |name: &str| file.get(&tag).and_then(|entry| entry.lineup(name)).is_some();
    match dialog.build(taken) {
        Ok(lineup) => {
            let name = lineup.name.as_str().to_owned();
            state.file.set_lineup(&tag, lineup);
            state.save();
            if let Some(dialog) = state.lineups.as_mut() {
                dialog.saved(&name);
            }
            Command::focus(LINEUPS)
        }
        Err(refusal) => {
            // The name is what a refusal about the name belongs beside, as in the add dialog; the
            // steps are the only thing wrong otherwise, and the line about it stands under them.
            let beside_name = matches!(refusal, Refusal::Name(_) | Refusal::Taken(_));
            if let Some(editor) = state.lineups.as_mut().and_then(|dialog| dialog.editor.as_mut()) {
                editor.refusal = Some(refusal);
            }
            if beside_name { Command::focus(NAME_FIELD) } else { Command::none() }
        }
    }
}

/// Adds the provider the dialog describes, or leaves the dialog open with the reason it cannot.
fn add(state: &mut Providers) -> Command<Msg> {
    let Some(draft) = state.draft.clone() else { return Command::none() };
    let taken = |tag: &str| state.file.get(tag).is_some();
    match draft.build(taken) {
        Ok(entry) => {
            let tag = entry.tag.as_str().to_owned();
            // The list already refused a tag it holds, so this cannot fail; if it ever did, the
            // dialog stays open rather than the provider quietly disappearing.
            if state.file.add(entry).is_err() {
                return Command::none();
            }
            state.draft = None;
            state.selected = state.entries().iter().position(|entry| entry.tag.as_str() == tag).unwrap_or(0);
            state.model = 0;
            state.notice = None;
            state.save();
            Command::focus(LIST)
        }
        Err(problem) => {
            let field = problem.field();
            if let Some(draft) = &mut state.draft {
                draft.problem = Some(problem);
            }
            Command::focus(field)
        }
    }
}

/// Starts a request against the chosen provider, if one is not already out.
fn start(
    state: &mut Providers,
    busy: Busy,
    work: impl FnOnce(Web, ProviderEntry) -> Msg + Send + 'static,
) -> Command<Msg> {
    let Some(entry) = state.selected().cloned() else { return Command::none() };
    if state.busy.is_some() {
        return Command::none();
    }
    state.busy = Some(busy);
    state.notice = None;
    let web = state.web.clone();
    Command::perform(move || work(web, entry))
}

/// Draws the screen: the providers, with the add dialog or the lineups dialog over them.
pub fn view(state: &Providers, ui: &mut View<'_, Msg>) {
    draw_list(state, ui);
    if let Some(draft) = state.draft() {
        draw_dialog(draft, ui);
    }
    if let (Some(dialog), Some(entry)) = (state.lineups(), state.selected()) {
        draw_lineups(dialog, entry, ui);
    }
}

/// The providers, the key-file line and everything about the chosen one.
fn draw_list(state: &Providers, ui: &mut View<'_, Msg>) {
    page::column(ui, page::WIDTH, |ui| {
        ui.column(|ui| {
            // The page's own name, and the one button that acts on the page rather than on a
            // provider in it. They share a row because a page a provider is on has a row of facts,
            // two lists and three rows of buttons under its name, and a terminal with few rows
            // gives them away to the button that adds a provider: nothing else on the page adds
            // one. An empty page has no providers to leave room for, and its own empty state says
            // so with a button of its own.
            ui.row(|ui| {
                ui.add(Text::new(t!("provider.title")).bold());
                if !state.entries().is_empty() {
                    ui.spacer();
                    ui.add(Button::new(t!("provider.new")).icon("add").on_press(Msg::New)).id("provider-new");
                }
            })
            .fill_width();
            // Where the key is kept and what carries it, in a line of its own. This is not a
            // footnote and it is not behind anything: a person who shares a backup should know what
            // they are sharing before they do it.
            ui.add(Text::new(key_file_line(state)).color("warning")).fill_width();
            for problem in state.permissions() {
                let (place, mode, wanted) = (
                    problem.place.display().to_string(),
                    format!("{:04o}", problem.mode),
                    format!("{:04o}", problem.wanted),
                );
                let said =
                    t!("provider.permissions", place = place.as_str(), mode = mode.as_str(), wanted = wanted.as_str());
                ui.add(Text::new(said).color("warning")).fill_width();
            }
            for problem in state.problems() {
                ui.add(Text::new(problem.to_string()).color("warning")).fill_width();
            }
            if state.loading {
                ui.add(Text::new(t!("provider.reading")).role("secondary"));
                return;
            }
            if state.entries().is_empty() {
                ui.add(
                    EmptyState::new(t!("provider.empty-title"))
                        .icon("inbox")
                        .message(t!("provider.empty-message"))
                        .action(Button::new(t!("provider.new")).variant("primary").on_press(Msg::New)),
                )
                .id("providers-empty")
                .fill();
                return;
            }
            let items = state.entries().iter().map(|entry| {
                ListItem::new(entry.tag.as_str().to_owned()).detail(format!(
                    "{}  {}",
                    kind_word(entry.kind),
                    entry.base
                ))
            });
            let list = List::new(items).label_first(true).selected(Some(state.selected)).on_select(Msg::Select);
            ui.add(list.wrap(true)).id(LIST).fill_width();

            if let Some(entry) = state.selected() {
                draw_chosen(state, entry, ui);
            }
        })
        .fill()
        .gap(1);
    });
}

/// Everything about the provider the person is looking at.
fn draw_chosen(state: &Providers, entry: &ProviderEntry, ui: &mut View<'_, Msg>) {
    let busy = state.busy();
    // What this provider is reached with, and what orders of its models it has: two lines of
    // facts about this one provider, with no blank row between them, since a blank row of the
    // page is a row of a twenty-four row terminal that one of the lists could have. A long order
    // is cut to the width rather than wrapped over the rows below, since the dialog behind the
    // button beside Try is where a long order is read.
    ui.column(|ui| {
        ui.add(Text::new(key_line(entry)).role("secondary")).fill_width();
        ui.add(Text::new(lineups_line(entry)).role("secondary").no_wrap()).fill_width();
    })
    .fill_width()
    .gap(0);
    if !entry.kind.regions().is_empty() {
        ui.column(|ui| draw_speaks(entry.kind, &entry.base, ui)).fill_width();
    }

    if entry.models.is_empty() {
        ui.add(Text::new(t!("provider.no-models")).role("secondary")).fill_width();
    } else {
        let items = entry.models.iter().map(|model| model_row(model, entry.kind));
        // A server can offer more models than the screen has rows. The list takes the rows the
        // rest of the page leaves and scrolls inside them, so the answer to Try and every button
        // under it stay where a hand can reach them.
        let list = List::new(items).selected(Some(state.model)).on_select(Msg::PickModel);
        ui.add(list.wrap(true)).id(MODELS).fill();
    }
    // What QCode saw of the chosen model in a harness, in words, beside the mark on its row.
    if let Some(model) = state.chosen_model()
        && let Some((text, tone)) = verdict_line(model, entry.kind)
    {
        ui.add(Text::new(text).color(tone)).fill_width();
    }
    // A figure QCode did not ask the service for says where it was read.
    if let Some(model) = state.chosen_model()
        && let Some(source) = model.published_at(entry.kind)
    {
        ui.add(Text::new(t!("provider.published-at", model = model.id.as_str(), source = source)).role("secondary"))
            .fill_width();
    }
    // A window that is really this small is the one thing a person has to be told before they
    // point a coding agent at it, with what they can do about it on their own machine.
    if let Some(model) = state.chosen_model()
        && model.measured.is_some_and(Measured::is_cramped)
    {
        ui.add(Text::new(t!("provider.cramped", tokens = tokens(CRAMPED))).color("warning")).fill_width();
        ui.add(Text::new(t!("provider.cramped-remedy", model = model.id.as_str())).role("secondary")).fill_width();
    }
    if let Some(notice) = state.notice() {
        let (text, tone) = notice_line(notice, entry);
        ui.add(Text::new(text).color(tone)).fill_width();
    }

    // Where the request would go, before anything sends one.
    ui.add(Text::new(t!("provider.going-to", line = ask::trial(entry).line().as_str())).role("secondary")).fill_width();

    ui.row(|ui| {
        ui.add(pressable(t!("provider.try"), t!("provider.trying"), busy == Some(Busy::Trying), busy, Msg::Try))
            .id("provider-try");
        let refresh = pressable(
            t!("provider.refresh"),
            t!("provider.refreshing"),
            busy == Some(Busy::Listing),
            busy,
            Msg::Refresh,
        );
        ui.add(refresh).id("provider-refresh");
        if state.chosen_model().is_some() {
            let measure = pressable(
                t!("provider.measure"),
                t!("provider.measuring"),
                busy == Some(Busy::Measuring),
                busy,
                Msg::MeasureAsked,
            );
            ui.add(measure).id("provider-measure");
        }
        // A lineup is a decision about which of the models above to lean on, so its button stands
        // with the other two that ask this provider something. The row wraps rather than cutting
        // its last button off the edge: four labels do not fit a narrow terminal, and a button
        // that is not on the screen is a button nobody can press.
        ui.add(Button::new(t!("provider.lineup-button")).on_press(Msg::LineupsAsked)).id("provider-lineups");
        ui.spacer();
    })
    .fill_width()
    .wrap(true)
    .gap(1);
    ui.row(|ui| {
        if entry.key.is_some() {
            ui.add(Button::new(t!("provider.forget-key")).on_press(Msg::ForgetKey)).id("provider-forget");
        }
        ui.add(Button::new(t!("provider.delete")).variant("danger").on_press(Msg::DeleteAsked)).id("provider-delete");
        ui.spacer();
    })
    .fill_width()
    .gap(1);
}

/// A button that shows the work while it is out and takes no second press, and that is not
/// pressable at all while something else is out.
fn pressable(label: String, working: String, mine: bool, busy: Option<Busy>, message: Msg) -> Button<Msg> {
    let button = Button::new(if mine { working } else { label });
    if busy.is_some() { button } else { button.on_press(message) }
}

/// The add dialog.
fn draw_dialog(draft: &Draft, ui: &mut View<'_, Msg>) {
    let problem = draft.problem.as_ref();
    let at = |field: &str| problem.filter(|problem| problem.field() == field).map(problem_message);
    let chosen = ProviderKind::ALL.iter().position(|kind| *kind == draft.kind);
    let dialog = Modal::new().title(t!("provider.new-title")).width(DIALOG_WIDTH).on_close(Msg::Cancel);
    ui.add_with(dialog, |ui| {
        ui.column(|ui| {
            let kinds = RadioGroup::new(ProviderKind::ALL.map(kind_word)).selected(chosen).on_select(Msg::PickKind);
            ui.add(kinds.wrap(true)).id("provider-kind");
            if !draft.kind.regions().is_empty() {
                draw_ready_made(draft, ui);
            }
            let tag_error = at(TAG_FIELD);
            ui.add_with(
                Field::new(t!("provider.tag")).hint(t!("provider.tag-hint")).error(tag_error.clone()).required(true),
                |ui| {
                    ui.add(TextInput::new(draft.tag.clone()).invalid(tag_error.is_some()).on_change(Msg::Tag))
                        .id(TAG_FIELD)
                        .fill_width();
                },
            )
            .fill_width();
            let base_error = at(BASE_FIELD);
            ui.add_with(
                Field::new(t!("provider.base")).hint(t!("provider.base-hint")).error(base_error.clone()).required(true),
                |ui| {
                    // The offered address is a suggestion, not a start: a person who tabs in and
                    // types their own replaces it instead of writing after it.
                    ui.add(
                        TextInput::new(draft.base.clone())
                            .select_all_on_focus()
                            .invalid(base_error.is_some())
                            .on_change(Msg::Base),
                    )
                    .id(BASE_FIELD)
                    .fill_width();
                },
            )
            .fill_width();
            if draft.kind.needs_key() {
                let key_error = at(KEY_FIELD);
                ui.add_with(
                    Field::new(t!("provider.key"))
                        .hint(t!("provider.key-hint"))
                        .error(key_error.clone())
                        .required(true),
                    |ui| {
                        // Never readable, not even by the person typing it: what is pasted here
                        // is shown again only as its last four characters, and only after it has
                        // been added.
                        ui.add(
                            TextInput::new(draft.key.clone())
                                .password(true)
                                .invalid(key_error.is_some())
                                .on_change(Msg::PasteKey),
                        )
                        .id(KEY_FIELD)
                        .fill_width();
                    },
                )
                .fill_width();
            }
            ui.row(|ui| {
                ui.spacer();
                ui.add(Button::new(t!("provider.cancel")).on_press(Msg::Cancel)).id("provider-cancel");
                ui.add(Button::new(t!("provider.add")).variant("primary").on_press(Msg::Add)).id("provider-add");
            })
            .fill_width()
            .gap(1);
        })
        .gap(1);
    });
}

/// The lineups of the chosen provider: their list, or the one being written in its place.
fn draw_lineups(dialog: &Lineups, entry: &ProviderEntry, ui: &mut View<'_, Msg>) {
    let title = t!("provider.lineup-title", tag = entry.tag.as_str());
    let mut modal = Modal::new().title(title).width(DIALOG_WIDTH).on_close(Msg::LineupsClosed);
    if dialog.editor.is_some() {
        modal = modal
            .action(Button::new(t!("provider.lineup-up")).on_press(Msg::LineupUp))
            .action(Button::new(t!("provider.lineup-down")).on_press(Msg::LineupDown))
            .action(Button::new(t!("provider.lineup-remove")).variant("danger").on_press(Msg::LineupRemove))
            .action(Button::new(t!("provider.lineup-cancel")).on_press(Msg::LineupCancel))
            .action(Button::new(t!("provider.lineup-save")).variant("primary").on_press(Msg::LineupSave));
    } else {
        modal = modal
            .action(Button::new(t!("provider.lineup-new")).on_press(Msg::LineupNew))
            .action(Button::new(t!("provider.lineup-edit")).on_press(Msg::LineupEditAsked))
            .action(Button::new(t!("provider.lineup-delete")).variant("danger").on_press(Msg::LineupDeleteAsked))
            .action(Button::new(t!("provider.lineup-close")).on_press(Msg::LineupsClosed));
    }
    ui.add_with(modal, |ui| {
        // The body takes the whole of what the title and the buttons leave, so the two lists
        // inside it have rows to scroll in rather than the height of the two or three models
        // that happen to be in them.
        ui.column(|ui| {
            if let Some(editor) = &dialog.editor {
                draw_editor(dialog, editor, entry, ui);
            } else {
                draw_list_of_lineups(dialog, entry, ui);
            }
        })
        .fill_height()
        .gap(1);
    });
}

/// What this provider has named: each lineup with the models it tries, in that order.
fn draw_list_of_lineups(dialog: &Lineups, entry: &ProviderEntry, ui: &mut View<'_, Msg>) {
    ui.add(Text::new(t!("provider.lineup-lead")).role("secondary")).fill_width();
    if entry.lineups.is_empty() {
        ui.add(Text::new(t!("provider.lineup-empty")).role("secondary")).fill_width();
        return;
    }
    let items = entry.lineups.iter().map(|lineup| {
        let steps: Vec<&str> = lineup.models.iter().map(String::as_str).collect();
        ListItem::new(lineup.name.as_str().to_owned()).detail(steps.join(" → "))
    });
    let chosen = dialog.chosen.as_deref().and_then(|name| entry.lineups.iter().position(|l| l.name.as_str() == name));
    let list = List::new(items).selected(chosen).on_select(Msg::LineupChosen);
    ui.add(list.wrap(true)).id(LINEUPS).fill();
}

/// The lineup being written: its name, the provider's models to pick from, and the order itself.
///
/// The two lists share whatever the dialog has left of the screen, so however many models the
/// provider offers, and however long the order grows, the buttons under them stay where a hand
/// can reach them.
fn draw_editor(dialog: &Lineups, editor: &Editor, entry: &ProviderEntry, ui: &mut View<'_, Msg>) {
    let refusal = editor.refusal.as_ref();
    let name_error = name_refusal(refusal);
    ui.add_with(
        Field::new(t!("provider.lineup-name"))
            .hint(t!("provider.lineup-name-hint"))
            .error(name_error.clone())
            .required(true),
        |ui| {
            ui.add(TextInput::new(editor.name.clone()).invalid(name_error.is_some()).on_change(Msg::LineupName))
                .id(NAME_FIELD)
                .fill_width();
        },
    )
    .fill_width();
    ui.add_with(Field::new(t!("provider.lineup-models")), |ui| {
        ui.add(
            TextInput::new(editor.filter.clone())
                .placeholder(t!("provider.lineup-filter"))
                .on_change(Msg::LineupFilter),
        )
        .id(FILTER_FIELD)
        .fill_width();
    })
    .fill_width();

    let shown = dialog.shown(entry);
    let rows = model_rows(ui);
    // A model is under the hand from the first moment: the first of the ones the filter leaves, or
    // the one the person pointed at while it was still there. A list with nothing chosen in it
    // would swallow every Return the person presses looking for the model they just typed.
    let chosen = editor
        .model
        .as_deref()
        .and_then(|id| shown.iter().position(|model| model.id == id))
        .or((!shown.is_empty()).then_some(0));
    // The model the person pointed at, by its place among the ones the filter leaves: the filter
    // is written as it is typed, so a number the person chose an hour ago no longer means the
    // same row, while a model's own name always does.
    let ids: Vec<String> = shown.iter().map(|model| model.id.clone()).collect();
    let items =
        shown.iter().map(|model| ListItem::new(model.id.clone()).detail(price_words(model.price).unwrap_or_default()));
    let list = List::new(items)
        .empty_text(t!("provider.lineup-none-match"))
        .selected(chosen)
        .on_select(move |index| Msg::LineupLooked(ids.get(index).cloned().unwrap_or_default()))
        .on_activate(Msg::LineupAdded);
    ui.add(list.wrap(true)).id(LINEUP_MODELS).height(Length::Cells(rows));

    let steps = dialog.steps();
    let step_height = step_rows(ui);
    let rows: Vec<ListItem> = steps
        .iter()
        .enumerate()
        .map(|(at, id)| {
            let price = entry.model(id).and_then(|model| price_words(model.price)).unwrap_or_default();
            ListItem::new(format!("{}. {id}", at + 1)).detail(price)
        })
        .collect();
    let step_list = List::new(rows)
        .empty_text(t!("provider.lineup-none-yet"))
        .selected((!steps.is_empty()).then(|| dialog.step().min(steps.len() - 1)))
        .on_select(Msg::LineupStep);
    ui.add(step_list.wrap(true)).id(LINEUP_STEPS).height(Length::Cells(step_height));

    // A step that costs money is said out loud where the order is chosen, not in a bill later.
    let paid = dialog.paid(entry);
    if !paid.is_empty() {
        ui.add(Text::new(t!("provider.lineup-paid", models = paid.join(", ").as_str())).color("warning")).fill_width();
    }
    if let Some(Refusal::Empty) = refusal {
        ui.add(Text::new(t!("provider.lineup-refused-empty")).color("warning")).fill_width();
    }
}

/// One line naming the provider's lineups compactly, or that it has none.
fn lineups_line(entry: &ProviderEntry) -> String {
    if entry.lineups.is_empty() {
        return t!("provider.lineup-none");
    }
    let named: Vec<String> = entry
        .lineups
        .iter()
        .map(|lineup| {
            let steps: Vec<&str> = lineup.models.iter().map(String::as_str).collect();
            format!("{} ({})", lineup.name, steps.join(" → "))
        })
        .collect();
    t!("provider.lineup-summary", lineups = named.join(", ").as_str())
}

/// What using a model costs, in one word, when the service says what it is.
fn price_words(price: Option<Price>) -> Option<String> {
    match price {
        Some(Price::Free) => Some(t!("provider.price-free")),
        Some(Price::Paid) => Some(t!("provider.price-paid")),
        None => None,
    }
}

/// What stopped the name being used, in the words the add dialog uses for a tag, since a lineup's
/// name is checked by the same rules.
fn name_refusal(refusal: Option<&Refusal>) -> Option<String> {
    use crate::provider::TagError;

    match refusal? {
        Refusal::Name(TagError::Empty) => Some(t!("provider.tag-empty")),
        Refusal::Name(TagError::TooLong { length }) => {
            Some(t!("provider.tag-long", length = i64::try_from(*length).unwrap_or(i64::MAX)))
        }
        Refusal::Name(TagError::BadStart { character }) => {
            Some(t!("provider.tag-start", character = character.to_string().as_str()))
        }
        Refusal::Name(TagError::Illegal { position, character }) => Some(t!(
            "provider.tag-character",
            character = character.to_string().as_str(),
            position = i64::try_from(*position).unwrap_or(i64::MAX)
        )),
        Refusal::Taken(name) => Some(t!("provider.lineup-name-taken", name = name.as_str())),
        Refusal::Empty => None,
    }
}

/// What a ready-made kind fills in, said before the person types their key: where its
/// subscription answers, where each shape is asked, the header the key goes in, and the models
/// it offers.
fn draw_ready_made(draft: &Draft, ui: &mut View<'_, Msg>) {
    let regions = draft.kind.regions().iter().map(|region| region_word(region.id));
    let group = RadioGroup::new(regions).horizontal(true).selected(draft.region()).on_select(Msg::PickRegion);
    ui.add(group.wrap(true)).id("provider-region");
    // One block, read together: it is what picking the kind filled in.
    ui.column(|ui| {
        draw_speaks(draft.kind, &draft.base, ui);
        let models: Vec<&str> = draft.kind.published().iter().map(|model| model.id).collect();
        ui.add(Text::new(t!("provider.ready-made-models", models = models.join(", ").as_str())).role("secondary"))
            .fill_width();
    })
    .fill_width();
}

/// Where a provider of `kind` at `base` is asked in each shape, written as the base a client of
/// that shape is given, and the header its key goes in: the same places the relay and the page's
/// own requests go.
fn draw_speaks(kind: ProviderKind, base: &str, ui: &mut View<'_, Msg>) {
    let (header, _) = kind.key_header();
    let anthropic = kind.api_address(base, Wire::Anthropic, "");
    let openai = kind.api_address(base, Wire::OpenAi, "/v1");
    ui.add(Text::new(t!("provider.speaks-anthropic", address = anthropic.as_str())).role("secondary")).fill_width();
    ui.add(Text::new(t!("provider.speaks-openai", address = openai.as_str())).role("secondary")).fill_width();
    ui.add(Text::new(t!("provider.key-header", header = header)).role("secondary")).fill_width();
}

/// How a region is named on the page.
fn region_word(id: &str) -> String {
    match id {
        "europe" => t!("provider.region-europe"),
        "singapore" => t!("provider.region-singapore"),
        "china" => t!("provider.region-china"),
        _ => t!("provider.region-overseas"),
    }
}

/// The line that says where the key is written and what carries it away.
fn key_file_line(state: &Providers) -> String {
    match &state.path {
        Some(path) => t!("provider.key-file", path = path.display().to_string().as_str()),
        None => t!("provider.key-file-nowhere"),
    }
}

/// What is shown of a provider's key: the last four characters, or that it has none.
fn key_line(entry: &ProviderEntry) -> String {
    match entry.key.as_ref().map(Key::last_four) {
        Some(Some(tail)) => t!("provider.key-shown", tail = tail.as_str()),
        Some(None) => t!("provider.key-hidden"),
        None => t!("provider.key-none"),
    }
}

/// Puts the free models QCode saw work first and the ones it saw fail last. Only OpenRouter's
/// free models were tried; any other provider's list stays as the provider gave it.
fn ordered(entry: &mut ProviderEntry) {
    if entry.kind == ProviderKind::OpenRouter {
        TRIED.order(&mut entry.models);
    }
}

/// What QCode saw of `model` on a provider of `kind`, when it was tried.
fn verdict(model: &Model, kind: ProviderKind) -> Option<Verdict> {
    (kind == ProviderKind::OpenRouter).then(|| TRIED.verdict(&model.id)).flatten()
}

/// A model's row: a mark for one seen working, and for one seen failing a faint row whose detail
/// is the reason, since its windows do not matter to anyone who cannot use it.
///
/// What a model costs stands at the end of every row that knows it, next to the two windows: a
/// person choosing models to lean on is choosing what a request that falls through them will cost,
/// and a price nobody published is left off the row rather than guessed at.
fn model_row(model: &Model, kind: ProviderKind) -> ListItem {
    let row = ListItem::new(model.id.clone());
    match verdict(model, kind) {
        Some(Verdict::Works(_)) => row.icon("check", Some("success")).detail(about_model(model, kind)),
        Some(Verdict::Fails(harness, reason)) => {
            row.icon("warning", Some("warning")).faint(true).detail(fails(harness, reason))
        }
        None => row.detail(about_model(model, kind)),
    }
}

/// The two windows of a model and what it costs, in the order a person reads them.
fn about_model(model: &Model, kind: ProviderKind) -> String {
    match price_words(model.price) {
        Some(price) => format!("{} · {price}", windows(model, kind)),
        None => windows(model, kind),
    }
}

/// Why a model does not work, in the fewest words.
fn fails(harness: crate::profile::HarnessKind, reason: Reason) -> String {
    let harness = harness.record().display_name;
    match reason {
        Reason::Empty => t!("provider.fails-empty", harness = harness),
        Reason::Tools => t!("provider.fails-tools", harness = harness),
        Reason::Half => t!("provider.fails-half", harness = harness),
    }
}

/// The line under the list for a chosen model QCode tried, and its tone.
fn verdict_line(model: &Model, kind: ProviderKind) -> Option<(String, &'static str)> {
    match verdict(model, kind)? {
        Verdict::Works(harnesses) => {
            let names: Vec<&str> = harnesses.iter().map(|harness| harness.record().display_name).collect();
            Some((t!("provider.tried", harnesses = names.join(", ").as_str()), "success"))
        }
        Verdict::Fails(harness, reason) => Some((fails(harness, reason), "warning")),
    }
}

/// The two windows of a model, side by side, each one saying plainly when it is not known, and a
/// claim that is the maker's published figure rather than the service's answer saying so.
fn windows(model: &Model, kind: ProviderKind) -> String {
    let claimed = match model.claimed {
        Some(claimed) if model.published_at(kind).is_some() => {
            t!("provider.claims-published", claimed = tokens(claimed))
        }
        Some(claimed) => t!("provider.claims", claimed = tokens(claimed)),
        None => t!("provider.claims-unknown"),
    };
    let given = match model.measured {
        Some(Measured::About(number)) => t!("provider.gives-about", tokens = tokens(number)),
        Some(Measured::AtLeast(number)) => t!("provider.gives-at-least", tokens = tokens(number)),
        None => t!("provider.gives-unknown"),
    };
    format!("{claimed} · {given}")
}

/// A count of tokens, as a number the person reads.
fn tokens(count: u64) -> String {
    count.to_string()
}

/// The sentence for one thing that happened, and the tone it is said in.
fn notice_line(notice: &Notice, entry: &ProviderEntry) -> (String, &'static str) {
    match notice {
        Notice::Reached(Reached::Version(version)) => {
            (t!("provider.reached-version", tag = entry.tag.as_str(), version = version.as_str()), "success")
        }
        Notice::Reached(Reached::KeyAccepted) => (t!("provider.reached-key"), "success"),
        Notice::Trouble(AskError::Unreachable { url, reason }) => {
            (t!("provider.unreachable", url = url.as_str(), reason = reason.as_str()), "danger")
        }
        // Too many requests is not a refusal of the key or the address, and saying "refused"
        // sends a person to check both. Free models hit it within a minute of use; the person is
        // told it is the service asking them to wait, and what to do meanwhile.
        Notice::Trouble(AskError::Refused { url, status: 429, said }) => {
            (t!("provider.rate-limited", url = url.as_str(), said = said.as_str()), "warning")
        }
        Notice::Trouble(AskError::Refused { url, status, said }) => {
            (t!("provider.refused", url = url.as_str(), status = i64::from(*status), said = said.as_str()), "danger")
        }
        Notice::Trouble(AskError::Unreadable { url, wanted }) => {
            (t!("provider.unreadable", url = url.as_str(), wanted = wanted.as_str()), "danger")
        }
        Notice::NotWritten(reason) => (t!("provider.save-failed", reason = reason.as_str()), "danger"),
    }
}

/// The sentence for one reason a provider could not be added.
fn problem_message(problem: &Problem) -> String {
    use crate::provider::TagError;

    match problem {
        Problem::Tag(TagError::Empty) => t!("provider.tag-empty"),
        Problem::Tag(TagError::TooLong { length }) => {
            t!("provider.tag-long", length = i64::try_from(*length).unwrap_or(i64::MAX))
        }
        Problem::Tag(TagError::BadStart { character }) => {
            t!("provider.tag-start", character = character.to_string().as_str())
        }
        Problem::Tag(TagError::Illegal { position, character }) => t!(
            "provider.tag-character",
            character = character.to_string().as_str(),
            position = i64::try_from(*position).unwrap_or(i64::MAX)
        ),
        Problem::Taken(tag) => t!("provider.tag-taken", tag = tag.as_str()),
        Problem::NoBase => t!("provider.base-empty"),
        Problem::BaseNotAnAddress(written) => t!("provider.base-not-an-address", written = written.as_str()),
        Problem::BaseUnusable(written) => t!("provider.base-unusable", written = written.as_str()),
        Problem::NoKey(ProviderKind::OpenRouter) => t!("provider.key-needed"),
        Problem::NoKey(kind) => t!("provider.key-needed-for", kind = kind_word(*kind).as_str()),
    }
}

/// How a kind is named on the page. These are the services' own names, written as they write
/// them, so they are the same word in every language.
fn kind_word(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Ollama => "ollama".to_owned(),
        ProviderKind::OpenRouter => "OpenRouter".to_owned(),
        ProviderKind::MimoTokenPlan => "Xiaomi MiMo Token Plan".to_owned(),
        ProviderKind::KimiCode => "Kimi Code".to_owned(),
    }
}

/// The request the trial would send, for anything that needs to name it outside this module.
#[must_use]
pub fn trial_of(entry: &ProviderEntry) -> Ask {
    ask::trial(entry)
}
