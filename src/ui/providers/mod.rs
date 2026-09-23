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
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use qframe::diagnostics::Diagnostic;
use qframe::prelude::*;
use qframe::widgets::{EmptyState, Field, Modal, RadioGroup, TextInput};

use crate::provider::{
    Ask, AskError, CRAMPED, Key, Measured, Model, PermissionProblem, ProviderEntry, ProviderKind,
    Providers as ProviderFile, Reached, Web, Wire, ask, permission_problems,
};
use crate::store::Loaded;

pub use draft::{BASE_FIELD, Draft, KEY_FIELD, Problem, TAG_FIELD};

/// Width of the add dialog. Wide enough for an address to read as one line, the longest being a
/// ready-made service's API root with the name of its shape in front of it.
const DIALOG_WIDTH: u16 = 72;

/// The name of the list of providers, for the focus and the tests.
const LIST: &str = "providers";

/// The name of the list of models.
const MODELS: &str = "provider-models";

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

/// The control that takes the keyboard when the screen opens, once there is one.
#[must_use]
pub fn entry(state: &Providers) -> Option<&'static str> {
    (!state.entries().is_empty()).then_some(LIST)
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

/// Draws the screen: the providers, or the add dialog over them.
pub fn view(state: &Providers, ui: &mut View<'_, Msg>) {
    draw_list(state, ui);
    if let Some(draft) = state.draft() {
        draw_dialog(draft, ui);
    }
}

/// The providers, the key-file line and everything about the chosen one.
fn draw_list(state: &Providers, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.add(Text::new(t!("provider.title")).bold());
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
            ListItem::new(entry.tag.as_str().to_owned()).detail(format!("{}  {}", kind_word(entry.kind), entry.base))
        });
        ui.add(List::new(items).selected(Some(state.selected)).on_select(Msg::Select)).id(LIST).fill_width();

        if let Some(entry) = state.selected() {
            draw_chosen(state, entry, ui);
        }
        ui.row(|ui| {
            ui.add(Button::new(t!("provider.new")).icon("add").on_press(Msg::New)).id("provider-new");
            ui.spacer();
        })
        .fill_width();
    })
    .fill()
    .gap(1);
}

/// Everything about the provider the person is looking at.
fn draw_chosen(state: &Providers, entry: &ProviderEntry, ui: &mut View<'_, Msg>) {
    let busy = state.busy();
    ui.add(Text::new(key_line(entry)).role("secondary")).fill_width();
    if !entry.kind.regions().is_empty() {
        ui.column(|ui| draw_speaks(entry.kind, &entry.base, ui)).fill_width();
    }

    if entry.models.is_empty() {
        ui.add(Text::new(t!("provider.no-models")).role("secondary")).fill_width();
    } else {
        let items = entry.models.iter().map(|model| ListItem::new(model.id.clone()).detail(windows(model, entry.kind)));
        // A server can offer more models than the screen has rows. The list takes the rows the
        // rest of the page leaves and scrolls inside them, so the answer to Try and every button
        // under it stay where a hand can reach them.
        ui.add(List::new(items).selected(Some(state.model)).on_select(Msg::PickModel)).id(MODELS).fill();
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
        ui.spacer();
    })
    .fill_width()
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
            ui.add(RadioGroup::new(ProviderKind::ALL.map(kind_word)).selected(chosen).on_select(Msg::PickKind))
                .id("provider-kind");
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

/// What a ready-made kind fills in, said before the person types their key: where its
/// subscription answers, where each shape is asked, the header the key goes in, and the models
/// it offers.
fn draw_ready_made(draft: &Draft, ui: &mut View<'_, Msg>) {
    let regions = draft.kind.regions().iter().map(|region| region_word(region.id));
    ui.add(RadioGroup::new(regions).horizontal(true).selected(draft.region()).on_select(Msg::PickRegion))
        .id("provider-region");
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
