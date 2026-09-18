//! The setup wizard: the three questions QCode asks before it can do anything.
//!
//! Language, container engine and workspace, in that order, as the design document has them.
//! The screen is the usual three pieces — [`Setup`], [`update`] and [`view`] — and it is
//! complete on its own, so it is driven end to end by a test without the rest of the
//! application.
//!
//! The screen speaks its own [`Msg`] and nothing here names the application's messages; the
//! application maps them on the way out. The wizard also runs one step on its own, which is
//! what the repair strip on the settings screen asks for: [`Setup::for_step`].
//!
//! Where the wizard opens is not the step the config file remembers. Every gate is asked from
//! the first step on and the first one that does not hold is the way in; [`gates`] holds that
//! rule as a pure function of plain data. What a missing engine costs the person is a command
//! they can copy, never an installation QCode performs itself: [`install`] works out the one
//! line their own system installs it with.

pub mod gates;
pub mod install;

use std::path::{Path, PathBuf};

use qframe::prelude::*;
use qframe::widgets::{
    Button, CopyValue, Field, FileBrowser, FilePicker, FilePickerMsg, Form, FormErrors, LogBuffer, LogLevel, LogLine,
    LogView, PickMode, RadioGroup, RadioStyle, ScrollView, Spinner, Toast, Wizard,
};

use crate::engine::EngineKind;
use crate::workspace::{Config, HostDirs, SetupStep};

use gates::{EngineCheck, EngineProblem, Gates, LocationCheck, LocationProblem};
use install::{InstallHost, Remedy};

/// The marks of every choice in the wizard: the small square, as on every other screen of
/// QCode. It is set here by name because the framework's constructor starts from another style.
const MARK: RadioStyle = RadioStyle::Square;

/// The languages QCode speaks, in the order they are offered.
const LANGUAGES: [&str; 2] = ["en", "tr"];

/// The engines, in the order they are offered. Podman is first because it is the recommended
/// one, and first is also what a person who presses on through takes.
const ENGINES: [EngineKind; 2] = [EngineKind::Podman, EngineKind::Docker];

/// Where the workspace goes: the place QCode works out, or one the person browses to.
const DEFAULT_PLACE: usize = 0;
/// See [`DEFAULT_PLACE`].
const CUSTOM_PLACE: usize = 1;

/// Rows every page of the wizard gets, so the buttons stay where they were between steps.
///
/// It is the height of the engine step with a problem, its command and its way out, which is
/// the tallest page anyone meets, and it is also small enough that the buttons below it stay on
/// screen even in a terminal of twenty rows. A page that wants more rows than that, such as an
/// engine whose refusal QCode can only quote, scrolls.
const PAGE_ROWS: u16 = 14;

/// Rows the engine's own words get when QCode cannot read its refusal. Enough to recognise the
/// message; the lines scroll and can be copied for the rest.
const OUTPUT_ROWS: u16 = 4;

/// How wide the labels of the wizard's fields are, so the controls of all three steps line up.
const LABEL_WIDTH: u16 = 18;

/// What can happen on the setup screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// A language was chosen. It is spoken from that moment on.
    Language(usize),
    /// An engine was chosen, which is also a reason to ask it whether it works.
    Engine(usize),
    /// Ask the chosen engine again.
    Recheck,
    /// An engine answered. The kind rides along so a late answer about an engine the person has
    /// moved on from changes nothing.
    EngineChecked(EngineKind, EngineCheck),
    /// The default place for the workspace, or one of the person's own.
    Place(usize),
    /// Something happened in the folder picker.
    Picker(FilePickerMsg),
    /// A folder answered, with the folder it was about.
    LocationChecked(PathBuf, LocationCheck),
    /// The wizard's Back button.
    Back,
    /// The wizard's Next button.
    Next,
    /// The wizard's Finish button.
    Finish,
    /// A finished step was chosen in the step bar.
    Step(usize),
}

/// The setup wizard's state.
///
/// It owns the [`Config`] while it runs, because settling these three questions is writing that
/// file: the engine, the workspace and the flag that keeps the wizard from opening again are
/// written when the last step is finished. [`Setup::into_config`] hands it back.
#[derive(Debug)]
pub struct Setup {
    config: Config,
    host: InstallHost,
    step: usize,
    language: usize,
    engine: usize,
    engine_check: EngineCheck,
    output: LogBuffer,
    place: usize,
    default_path: Option<PathBuf>,
    custom_path: Option<PathBuf>,
    browser: FileBrowser,
    location_check: LocationCheck,
    errors: FormErrors,
    alone: bool,
    finished: bool,
}

impl Setup {
    /// A wizard for `config` on the machine `dirs` and `host` describe, opened on the first gate
    /// of `gates` that does not hold.
    ///
    /// The gates are asked before the screen appears, not by the screen: their answers decide
    /// whether the wizard is needed at all, and the one who asks that question already has them.
    #[must_use]
    pub fn new(config: Config, dirs: &HostDirs, gates: &Gates, host: InstallHost) -> Self {
        let language = config
            .settings()
            .language()
            .and_then(|code| LANGUAGES.iter().position(|known| *known == code))
            .unwrap_or(0);
        let engine = config
            .engine_kind()
            .and_then(EngineKind::from_name)
            .and_then(|kind| ENGINES.iter().position(|known| *known == kind))
            .unwrap_or(0);
        let default_path = dirs.default_workspace().value;
        // A workspace already written down is the person's own choice, unless it is the very
        // place QCode would have picked anyway.
        let stored = config.workspace_path().filter(|path| Some(path) != default_path.as_ref());
        let start = dirs
            .documents()
            .value
            .or_else(|| dirs.home.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut setup = Self {
            config,
            host,
            step: 0,
            language,
            engine,
            engine_check: gates.engine.clone(),
            output: LogBuffer::new(usize::from(OUTPUT_ROWS) * 8),
            place: if stored.is_some() || default_path.is_none() { CUSTOM_PLACE } else { DEFAULT_PLACE },
            default_path,
            custom_path: stored,
            browser: FileBrowser::new(start, PickMode::Folders),
            location_check: gates.location.clone(),
            errors: FormErrors::new(),
            alone: false,
            finished: false,
        };
        setup.remember(&gates.engine);
        setup.step = gates.entry().map_or(SetupStep::ALL.len() - 1, step_index);
        setup
    }

    /// A wizard that asks `step` alone, for a setting that has to be put right after the setup
    /// was finished: the engine that has gone missing, or a workspace that has to move.
    ///
    /// It shows that one step and finishes on it, so the person is not walked through questions
    /// they answered long ago. What it writes is what that step settles; the flag that keeps the
    /// wizard from opening at every start is already written and stays written.
    #[must_use]
    pub fn for_step(config: Config, dirs: &HostDirs, gates: &Gates, host: InstallHost, step: SetupStep) -> Self {
        let mut setup = Self::new(config, dirs, gates, host);
        setup.step = step_index(step);
        setup.alone = true;
        setup
    }

    /// Whether the wizard is asking one step on its own rather than walking all three.
    #[must_use]
    pub fn is_alone(&self) -> bool {
        self.alone
    }

    /// Which step the wizard is on.
    #[must_use]
    pub fn step(&self) -> SetupStep {
        SetupStep::ALL[self.step.min(SetupStep::ALL.len() - 1)]
    }

    /// The language the person chose, as the config file writes it.
    ///
    /// The wizard applies it to the running application itself; storing it is the caller's,
    /// because the language belongs to the whole application and not to this screen.
    #[must_use]
    pub fn language(&self) -> &'static str {
        LANGUAGES[self.language]
    }

    /// The engine the person chose, as the config file writes it.
    #[must_use]
    pub fn engine_name(&self) -> &'static str {
        ENGINES[self.engine].name()
    }

    /// What the chosen engine last answered.
    #[must_use]
    pub fn engine_check(&self) -> &EngineCheck {
        &self.engine_check
    }

    /// Where the workspace will be, once there is an answer to that at all.
    #[must_use]
    pub fn workspace_path(&self) -> Option<&Path> {
        match self.place {
            DEFAULT_PLACE => self.default_path.as_deref(),
            _ => self.custom_path.as_deref(),
        }
    }

    /// Whether the wizard is through: everything is settled and written down.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The settings, to read what the wizard has written.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Hands the settings back when the wizard is done with them.
    #[must_use]
    pub fn into_config(self) -> Config {
        self.config
    }

    /// The engine the person is looking at.
    fn kind(&self) -> EngineKind {
        ENGINES[self.engine]
    }

    /// Keeps the engine's own words, so a refusal QCode cannot read is still readable.
    fn remember(&mut self, check: &EngineCheck) {
        self.output.clear();
        let EngineCheck::Broken(problem) = check else { return };
        let Some(output) = problem.output() else { return };
        for line in output.lines().filter(|line| !line.trim().is_empty()) {
            self.output.push(LogLine::new(LogLevel::Error, line.trim_end()));
        }
    }

    /// Whether the wizard is waiting on an answer. Next waits with it rather than acting on a
    /// state that is about to change.
    fn is_busy(&self) -> bool {
        self.engine_check == EngineCheck::Running || self.location_check == LocationCheck::Running
    }

    /// What is wrong with the step the wizard is on. The names are the ids of the controls, so
    /// a step that cannot be left puts the focus on what is holding it.
    fn validate(&self) -> FormErrors {
        let mut errors = FormErrors::new();
        match self.step() {
            // Some language is always chosen, and the wizard opens in one.
            SetupStep::Language => {}
            SetupStep::Engine => {
                errors.check("setup-engine", self.engine_check == EngineCheck::Working, t!("setup.engine-not-ready"));
            }
            SetupStep::Location => {
                errors.check(
                    "setup-location",
                    self.location_check == LocationCheck::Usable,
                    t!("setup.location-not-ready"),
                );
            }
        }
        errors
    }
}

/// Which of the three steps this is.
fn step_index(step: SetupStep) -> usize {
    SetupStep::ALL.iter().position(|known| *known == step).unwrap_or(0)
}

/// Asks the chosen engine whether it works, off the render path.
fn ask_engine(kind: EngineKind) -> Command<Msg> {
    Command::perform(move || Msg::EngineChecked(kind, gates::check_engine(kind)))
}

/// Makes sure the workspace folder is there and writable, off the render path.
fn ask_folder(path: PathBuf) -> Command<Msg> {
    Command::perform(move || {
        let check = gates::check_location(&path);
        Msg::LocationChecked(path, check)
    })
}

/// The control a step opens with, so the keyboard lands on the question and not beside it.
fn first_control(step: usize) -> &'static str {
    ["setup-language", "setup-engine", "setup-location"][step.min(SetupStep::ALL.len() - 1)]
}

/// Asks about whatever the step now open needs, and nothing it has already been told.
fn enter(setup: &mut Setup) -> Command<Msg> {
    let focus = Command::focus(first_control(setup.step));
    let ask = match setup.step() {
        SetupStep::Language => Command::none(),
        SetupStep::Engine if setup.engine_check == EngineCheck::Unknown => {
            setup.engine_check = EngineCheck::Running;
            ask_engine(setup.kind())
        }
        SetupStep::Location if setup.location_check == LocationCheck::Unknown => match setup.workspace_path() {
            Some(path) => {
                let path = path.to_path_buf();
                setup.location_check = LocationCheck::Running;
                ask_folder(path)
            }
            None => {
                setup.location_check = LocationCheck::Broken(LocationProblem::Unset);
                Command::none()
            }
        },
        SetupStep::Engine | SetupStep::Location => Command::none(),
    };
    Command::batch([focus, ask])
}

/// The work the wizard needs the moment it is shown: asking whatever the step it opens on
/// needs, and putting the keyboard on the question.
pub fn opened(setup: &mut Setup) -> Command<Msg> {
    enter(setup)
}

/// Applies a message to the wizard.
pub fn update(setup: &mut Setup, message: Msg) -> Command<Msg> {
    let command = match message {
        Msg::Language(index) => {
            setup.language = index.min(LANGUAGES.len() - 1);
            // The choice is the change: the screen is redrawn in the new language at once.
            Command::set_locale(setup.language())
        }
        Msg::Engine(index) => {
            setup.engine = index.min(ENGINES.len() - 1);
            setup.engine_check = EngineCheck::Running;
            setup.output.clear();
            ask_engine(setup.kind())
        }
        Msg::Recheck => {
            setup.engine_check = EngineCheck::Running;
            setup.output.clear();
            ask_engine(setup.kind())
        }
        Msg::EngineChecked(kind, check) => {
            // An answer about an engine the person has moved on from is not their answer.
            if kind == setup.kind() {
                setup.remember(&check);
                setup.engine_check = check;
            }
            Command::none()
        }
        Msg::Place(index) => {
            setup.place = index.min(CUSTOM_PLACE);
            let open = if setup.place == CUSTOM_PLACE && setup.browser.loading().is_none() {
                let folder = setup.browser.folder().to_path_buf();
                setup.browser.open(folder, Msg::Picker)
            } else {
                Command::none()
            };
            Command::batch([open, check_place(setup)])
        }
        Msg::Picker(FilePickerMsg::Chosen(path)) => {
            setup.custom_path = Some(path);
            setup.place = CUSTOM_PLACE;
            check_place(setup)
        }
        Msg::Picker(message) => setup.browser.update(message, Msg::Picker),
        Msg::LocationChecked(path, check) => {
            if Some(path.as_path()) == setup.workspace_path() {
                setup.location_check = check;
            }
            Command::none()
        }
        // Going back is a step opening like any other: a step whose question was never put
        // because the wizard opened past it is put now.
        Msg::Back => {
            setup.errors.clear();
            setup.step = setup.step.saturating_sub(1);
            setup.config.set_setup_step(setup.step());
            enter(setup)
        }
        Msg::Step(index) => {
            setup.errors.clear();
            setup.step = index.min(SetupStep::ALL.len() - 1);
            setup.config.set_setup_step(setup.step());
            enter(setup)
        }
        Msg::Next => {
            setup.errors = setup.validate();
            if !setup.errors.is_empty() {
                return setup.errors.focus_first();
            }
            setup.step = (setup.step + 1).min(SetupStep::ALL.len() - 1);
            setup.config.set_setup_step(setup.step());
            enter(setup)
        }
        Msg::Finish => {
            setup.errors = setup.validate();
            if !setup.errors.is_empty() {
                return setup.errors.focus_first();
            }
            finish(setup)
        }
    };
    // An error that has been put right stops being shown the moment it is put right, rather
    // than waiting for the person to press Next again.
    if !setup.errors.is_empty() {
        setup.errors = setup.validate();
    }
    command
}

/// Checks the folder the person is now pointing at, if they are pointing at one.
fn check_place(setup: &mut Setup) -> Command<Msg> {
    match setup.workspace_path() {
        Some(path) => {
            let path = path.to_path_buf();
            setup.location_check = LocationCheck::Running;
            ask_folder(path)
        }
        None => {
            setup.location_check = LocationCheck::Broken(LocationProblem::Unset);
            Command::none()
        }
    }
}

/// Writes down everything the wizard settled.
///
/// The flag that keeps the wizard from opening again is written last and only when the file was
/// really saved, so a workspace that cannot be written leaves the wizard where it is with the
/// reason on screen instead of locking the person out of it.
fn finish(setup: &mut Setup) -> Command<Msg> {
    let Some(path) = setup.workspace_path().map(Path::to_path_buf) else {
        return Command::none();
    };
    setup.config.set_engine_kind(setup.engine_name());
    setup.config.set_workspace_path(&path);
    setup.config.set_setup_step(SetupStep::Location);
    setup.config.set_setup_completed(true);
    if let Err(error) = setup.config.save() {
        setup.config.set_setup_completed(false);
        return Command::toast(Toast::danger(t!("setup.save-failed")).body(error.to_string()));
    }
    setup.finished = true;
    Command::none()
}

/// Draws the wizard.
pub fn view(setup: &Setup, ui: &mut View<'_, Msg>) {
    let all = [t!("setup.step-language"), t!("setup.step-engine"), t!("setup.step-location")];
    // One step on its own is one step in the step bar as well: a bar of three with two of them
    // unreachable would promise questions this wizard is not going to ask.
    let labels: Vec<String> = if setup.alone { vec![all[setup.step.min(all.len() - 1)].clone()] } else { all.to_vec() };
    let mut wizard = Wizard::new(labels)
        .current(if setup.alone { 0 } else { setup.step })
        .on_finish(Msg::Finish)
        .busy(setup.is_busy())
        .page_height(PAGE_ROWS);
    if !setup.alone {
        wizard = wizard.on_back(Msg::Back).on_next(Msg::Next).on_step(Msg::Step);
    }
    wizard
        .show(ui, |ui| match setup.step() {
            SetupStep::Language => language_page(setup, ui),
            SetupStep::Engine => engine_page(setup, ui),
            SetupStep::Location => location_page(setup, ui),
        })
        .fill();
}

/// The first step: the language, which changes as it is chosen.
fn language_page(setup: &Setup, ui: &mut View<'_, Msg>) {
    ui.add(Text::new(t!("setup.language-intro")).role("secondary")).fill_width();
    ui.spacer().height(Length::Cells(1));
    Form::new().label_width(LABEL_WIDTH).show(ui, |form| {
        form.field(Field::new(t!("setup.language")), |ui| {
            let options = LANGUAGES.map(|code| t!(&format!("setup.language-{code}")));
            ui.add(RadioGroup::new(options).selected(Some(setup.language)).style(MARK).on_select(Msg::Language))
                .id("setup-language");
        });
    });
}

/// The second step: the engine, and what to do when it is not there.
fn engine_page(setup: &Setup, ui: &mut View<'_, Msg>) {
    // A broken engine has more to say than a short terminal has rows, and none of it may be
    // out of reach: the page scrolls, and keyboard focus brings its own row into view.
    ui.add_with(ScrollView::new(), |ui| {
        ui.add(Text::new(t!("setup.engine-intro")).role("secondary")).fill_width();
        ui.spacer().height(Length::Cells(1));
        Form::new().label_width(LABEL_WIDTH).show(ui, |form| {
            // Nothing is required of the person here: one of the two is always chosen.
            let field = Field::new(t!("setup.engine")).error(setup.errors.get("setup-engine"));
            form.field(field, |ui| {
                let options = ENGINES.map(|kind| t!(&format!("setup.engine-{}", kind.name())));
                ui.add(RadioGroup::new(options).selected(Some(setup.engine)).style(MARK).on_select(Msg::Engine))
                    .id("setup-engine");
            });
        });
        ui.add(Text::new(t!(&format!("setup.{}-note", setup.kind().name()))).role("faint")).fill_width();
        ui.spacer().height(Length::Cells(1));
        engine_state(setup, ui);
    })
    .fill();
}

/// What the engine last answered, and what the person can do about it.
fn engine_state(setup: &Setup, ui: &mut View<'_, Msg>) {
    let name = t!(&format!("setup.engine-{}", setup.kind().name()));
    match &setup.engine_check {
        // Nothing has been asked yet only while the step is opening; the question goes out in
        // the same update, so both say the same thing rather than flashing an empty page.
        EngineCheck::Unknown | EngineCheck::Running => {
            ui.add(Spinner::new().label(t!("setup.checking", engine = name)));
        }
        EngineCheck::Working => {
            let mark = ui.env().icons().glyph("success").into_owned();
            ui.add(Text::rich([
                Span::new(format!("{mark} ")).color("success"),
                Span::new(t!("setup.ready", engine = name)),
            ]));
        }
        EngineCheck::Broken(problem) => engine_problem(setup, problem, &name, ui),
    }
}

/// A broken engine: what is wrong, the one line that puts it right, and a way to ask again.
fn engine_problem(setup: &Setup, problem: &EngineProblem, name: &str, ui: &mut View<'_, Msg>) {
    let mark = ui.env().icons().glyph("warning").into_owned();
    let said = match problem {
        EngineProblem::NotInstalled => t!("setup.not-installed", engine = name.to_owned()),
        EngineProblem::DaemonStopped { .. } => t!("setup.daemon-stopped"),
        EngineProblem::MachineStopped { .. } => t!("setup.machine-stopped"),
        EngineProblem::Refused { .. } => t!("setup.refused", engine = name.to_owned()),
        EngineProblem::NotRunnable { bin, .. } => {
            t!("setup.unreachable", engine = name.to_owned(), path = bin.display().to_string())
        }
    };
    ui.add(Text::rich([Span::new(format!("{mark} ")).color("warning"), Span::new(said)])).fill_width();
    ui.spacer().height(Length::Cells(1));

    match Remedy::for_problem(problem, setup.kind(), &setup.host) {
        Remedy::Install { command: Some(command), .. } => line(ui, &t!("setup.install-with"), &command),
        Remedy::Install { command: None, docs } => {
            ui.add(Text::new(t!("setup.no-manager", url = docs.to_owned())).role("secondary")).fill_width();
        }
        Remedy::Start { command: Some(command) } => line(ui, &t!("setup.start-with"), &command),
        Remedy::Start { command: None } => {
            ui.add(Text::new(t!("setup.start-desktop")).role("secondary")).fill_width();
        }
        // Nothing is offered for a refusal QCode cannot read: the engine's own words below are
        // the only honest help there is.
        Remedy::Unknown => {}
    }

    ui.spacer().height(Length::Cells(1));
    ui.add(Button::new(t!("setup.recheck")).on_press(Msg::Recheck)).id("setup-recheck");
    // On its own line, where a narrow terminal wraps the promise instead of cutting it in half.
    ui.add(Text::new(t!("setup.never-installs")).role("faint")).fill_width();

    // Last, because it is the only part that is read rather than acted on.
    if !setup.output.is_empty() {
        ui.spacer().height(Length::Cells(1));
        ui.add(Text::new(t!("setup.engine-said")).role("faint"));
        ui.add(LogView::new(&setup.output)).fill_width().height(Length::Cells(OUTPUT_ROWS));
    }
}

/// A command the person runs themselves, with its one-line explanation above it.
fn line(ui: &mut View<'_, Msg>, label: &str, command: &str) {
    ui.add(Text::new(label.to_owned()).role("faint"));
    ui.add(CopyValue::new(command.to_owned())).id("setup-command").fill_width();
}

/// The third step: where the workspace goes.
fn location_page(setup: &Setup, ui: &mut View<'_, Msg>) {
    ui.add(Text::new(t!("setup.location-intro")).role("secondary")).fill_width();
    ui.spacer().height(Length::Cells(1));
    Form::new().label_width(LABEL_WIDTH).show(ui, |form| {
        // Nothing is required of the person here either: a place is always chosen.
        let field = Field::new(t!("setup.location")).error(setup.errors.get("setup-location"));
        form.field(field, |ui| {
            let default = match &setup.default_path {
                Some(path) => format!("{}  {}", t!("setup.location-default"), path.display()),
                None => t!("setup.location-default"),
            };
            let options = [default, t!("setup.location-custom")];
            ui.add(
                RadioGroup::new(options)
                    .selected(Some(setup.place))
                    .style(MARK)
                    .disabled(setup.default_path.is_none() && setup.place == DEFAULT_PLACE)
                    .on_select(Msg::Place),
            )
            .id("setup-location");
        });
    });
    if setup.default_path.is_none() {
        ui.add(Text::new(t!("setup.no-default")).role("secondary")).fill_width();
    }
    ui.spacer().height(Length::Cells(1));
    // What the folder answered stands above the picker, which takes every row that is left:
    // the answer is about the choice, and it must not be what a short terminal drops.
    location_state(setup, ui);
    if setup.place == CUSTOM_PLACE {
        FilePicker::new(&setup.browser, Msg::Picker).show(ui).fill();
    }
}

/// What the folder last answered.
fn location_state(setup: &Setup, ui: &mut View<'_, Msg>) {
    match &setup.location_check {
        // Before anything has been asked the step is still opening and the question is already
        // on its way, so nothing is said rather than something that is about to be replaced.
        LocationCheck::Unknown => {}
        LocationCheck::Running => {
            ui.add(Spinner::new().label(t!("setup.location-checking")));
        }
        LocationCheck::Usable => {
            let mark = ui.env().icons().glyph("success").into_owned();
            ui.add(Text::rich([
                Span::new(format!("{mark} ")).color("success"),
                Span::new(t!("setup.location-usable")),
            ]));
        }
        LocationCheck::Broken(problem) => {
            let mark = ui.env().icons().glyph("warning").into_owned();
            let said = match problem {
                LocationProblem::Unset => t!("setup.location-unset"),
                LocationProblem::NotAFolder => t!("setup.not-a-folder"),
                LocationProblem::Blocked(reason) => t!("setup.blocked", reason = reason.clone()),
            };
            ui.add(Text::rich([Span::new(format!("{mark} ")).color("warning"), Span::new(said)])).fill_width();
        }
    }
}

/// The keys of the setup wizard that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("setup.hint-move")), (icons.glyph("enter").into_owned(), t!("setup.hint-choose"))]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use qframe::env::{AssetDirs, Env};
    use qframe::event::MouseKind;
    use qframe::icons::GlyphMode;
    use qframe::prelude::*;
    use qframe::runtime::Harness;
    use qframe::widgets::AppShell;

    use super::gates::{EngineCheck, EngineProblem, Gates, LocationCheck, LocationProblem};
    use super::install::InstallHost;
    use super::{Msg, Setup, update, view};
    use crate::workspace::{Config, HostDirs, Platform, SetupStep};

    /// A terminal with room for the steps, a page and the buttons.
    const SIZE: (u16, u16) = (88, 28);

    /// A terminal tall enough for every row of the tallest page at once, which is what a person
    /// on a full-screen terminal has.
    const TALL: (u16, u16) = (88, 36);

    /// The wizard on its own, which is how the screen is meant to be usable.
    struct Wizard {
        setup: Setup,
    }

    impl App for Wizard {
        type Msg = Msg;

        fn update(&mut self, message: Msg) -> Command<Msg> {
            update(&mut self.setup, message)
        }

        fn view(&self, ui: &mut View<'_, Msg>) {
            AppShell::new().body(|ui| view(&self.setup, ui)).show(ui);
        }
    }

    fn env() -> Env {
        Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() })
            .expect("the built-in files load")
    }

    /// A folder of this machine's temporary directory, named after `what` and this process, so
    /// tests never meet each other and never touch the person running them.
    fn temporary(what: &str) -> PathBuf {
        std::env::temp_dir().join(format!("qcode-setup-{what}-{}", std::process::id()))
    }

    /// Host directories whose documents folder is below `home`.
    fn dirs(home: Option<PathBuf>) -> HostDirs {
        HostDirs { platform: Platform::Linux, home, documents_hint: None, user_dirs: None }
    }

    /// An Arch machine with `paru`, so the install command of every test is the same one.
    fn host() -> InstallHost {
        InstallHost::read(Platform::Linux, Some("ID=arch\n"), |tool| tool == "paru")
    }

    fn gates(engine: EngineCheck, location: LocationCheck) -> Gates {
        Gates { language: true, engine, location }
    }

    fn wizard(config: &str, gates: &Gates, home: Option<PathBuf>) -> Harness<Wizard> {
        sized(config, gates, home, SIZE)
    }

    fn sized(config: &str, gates: &Gates, home: Option<PathBuf>, size: (u16, u16)) -> Harness<Wizard> {
        let setup = Setup::new(Config::parse_str("code.toml", config), &dirs(home), gates, host());
        let mut harness = Harness::with_env(Wizard { setup }, env(), size.0, size.1);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        harness.render();
        harness
    }

    /// A wizard on the engine step, with the engine broken in the given way.
    fn on_engine(config: &str, problem: EngineProblem) -> Harness<Wizard> {
        wizard(config, &gates(EngineCheck::Broken(problem), LocationCheck::Unknown), Some(temporary("home")))
    }

    /// The same, on a terminal with room for every row of it.
    fn on_engine_tall(config: &str, problem: EngineProblem) -> Harness<Wizard> {
        sized(config, &gates(EngineCheck::Broken(problem), LocationCheck::Unknown), Some(temporary("home")), TALL)
    }

    #[test]
    fn a_fresh_machine_opens_on_the_first_step() {
        let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
        let harness = wizard("", &gates, Some(temporary("home")));
        let screen = harness.screen();
        assert_eq!(harness.app().setup.step(), SetupStep::Language);
        assert!(screen.contains("Pick the language"), "{screen}");
        assert!(screen.contains("English"), "{screen}");
    }

    #[test]
    fn a_gate_that_fell_later_is_where_the_wizard_opens() {
        // The language was settled before; the engine is gone, so that is the step that opens,
        // whatever the config file remembers about how far the person once got.
        let config = "language = \"en\"\n\n[setup]\nstep = \"location\"\n";
        let harness = on_engine(config, EngineProblem::NotInstalled);
        assert_eq!(harness.app().setup.step(), SetupStep::Engine);
        let screen = harness.screen();
        assert!(screen.contains("Container engine"), "{screen}");
    }

    #[test]
    fn the_language_is_spoken_the_moment_it_is_chosen() {
        let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
        let mut harness = wizard("", &gates, Some(temporary("home")));
        harness.click_text("Turkish").render();
        let screen = harness.screen();
        assert!(screen.contains("konuşacağı dili seç"), "the whole screen turns at once:\n{screen}");
        assert_eq!(harness.app().setup.language(), "tr");
    }

    #[test]
    fn podman_is_recommended_and_docker_is_fully_supported() {
        let harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        let screen = harness.screen();
        assert!(screen.contains("Podman") && screen.contains("Docker"), "{screen}");
        assert!(screen.contains("Recommended"), "the recommendation is said, not implied:\n{screen}");
        let (_, podman) = harness.find("Podman").expect("podman is offered");
        let (_, docker) = harness.find("Docker").expect("docker is offered");
        assert!(podman < docker, "podman is the first and default choice:\n{screen}");
        assert_eq!(harness.app().setup.engine_name(), "podman");
    }

    #[test]
    fn docker_says_what_it_is_when_it_is_chosen() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        harness.click_text("Docker").render();
        let screen = harness.screen();
        assert!(screen.contains("Fully supported"), "{screen}");
        assert_eq!(harness.app().setup.engine_name(), "docker");
    }

    #[test]
    fn a_missing_engine_shows_the_line_this_system_installs_it_with() {
        let harness = on_engine_tall("language = \"en\"\n", EngineProblem::NotInstalled);
        let screen = harness.screen();
        assert!(screen.contains("not installed"), "{screen}");
        assert!(screen.contains("paru -S podman"), "the command belongs to this machine:\n{screen}");
        assert!(screen.contains("never installs anything"), "QCode says it will not do it itself:\n{screen}");
        assert!(screen.contains("Check again"), "{screen}");
    }

    #[test]
    fn the_install_command_is_copied_with_one_click() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        harness.click_text("paru -S podman");
        assert_eq!(harness.copied(), ["paru -S podman"]);
    }

    #[test]
    fn a_stopped_docker_daemon_is_a_different_story_from_a_missing_docker() {
        let config = "language = \"en\"\n\n[engine]\nkind = \"docker\"\n";
        let problem = EngineProblem::DaemonStopped { output: "cannot connect to the docker daemon".to_owned() };
        let harness = on_engine(config, problem);
        let screen = harness.screen();
        assert!(screen.contains("Docker is installed"), "{screen}");
        assert!(!screen.contains("not installed"), "nothing is missing, so nothing says so:\n{screen}");
        assert!(screen.contains("sudo systemctl start docker"), "{screen}");
    }

    #[test]
    fn a_podman_machine_that_is_not_running_asks_for_the_machine_and_not_an_install() {
        let problem = EngineProblem::MachineStopped { output: "podman machine start".to_owned() };
        let harness = on_engine("language = \"en\"\n", problem);
        let screen = harness.screen();
        assert!(screen.contains("virtual machine"), "{screen}");
        assert!(screen.contains("podman machine start"), "{screen}");
    }

    #[test]
    fn a_refusal_qcode_cannot_read_shows_the_engines_own_words() {
        let problem = EngineProblem::Refused { code: Some(125), output: "short-name resolution enforced".to_owned() };
        let harness = on_engine_tall("language = \"en\"\n", problem);
        let screen = harness.screen();
        assert!(screen.contains("refused to answer"), "{screen}");
        assert!(screen.contains("short-name resolution enforced"), "the engine's own words are readable:\n{screen}");
    }

    #[test]
    fn while_the_engine_is_being_asked_the_screen_says_so_and_the_wizard_waits() {
        let harness = wizard(
            "language = \"en\"\n",
            &gates(EngineCheck::Running, LocationCheck::Unknown),
            Some(temporary("home")),
        );
        let screen = harness.screen();
        assert!(screen.contains("Asking Podman whether it works"), "{screen}");
    }

    #[test]
    fn asking_again_puts_the_question_out_again() {
        // Sent straight to update, so the test never starts a container engine of its own.
        let mut setup = Setup::new(
            Config::parse_str("code.toml", "language = \"en\"\n"),
            &dirs(Some(temporary("home"))),
            &gates(EngineCheck::Broken(EngineProblem::NotInstalled), LocationCheck::Unknown),
            host(),
        );
        let _: Command<Msg> = update(&mut setup, Msg::Recheck);
        assert_eq!(setup.engine_check(), &EngineCheck::Running);
    }

    #[test]
    fn the_wizard_does_not_go_on_while_the_engine_does_not_work() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        harness.click_text("Next").render();
        let screen = harness.screen();
        assert_eq!(harness.app().setup.step(), SetupStep::Engine, "{screen}");
        assert!(screen.contains("Pick an engine that works"), "{screen}");
    }

    #[test]
    fn the_location_step_offers_the_default_place_and_one_of_your_own() {
        let home = temporary("home");
        let harness =
            wizard("language = \"en\"\n", &gates(EngineCheck::Working, LocationCheck::Usable), Some(home.clone()));
        assert_eq!(harness.app().setup.step(), SetupStep::Location);
        let screen = harness.screen();
        assert!(screen.contains("Default place") && screen.contains("Somewhere else"), "{screen}");
        assert!(screen.contains("QCode"), "the default place is named, not implied:\n{screen}");
        assert_eq!(harness.app().setup.workspace_path(), Some(home.join("Documents").join("QCode")).as_deref());
    }

    #[test]
    fn a_machine_with_no_documents_folder_asks_for_a_place_instead_of_inventing_one() {
        let harness = wizard("language = \"en\"\n", &gates(EngineCheck::Working, LocationCheck::Usable), None);
        let screen = harness.screen();
        assert!(screen.contains("could not work out a documents folder"), "{screen}");
        assert_eq!(harness.app().setup.workspace_path(), None);
    }

    #[test]
    fn a_place_of_your_own_is_browsed_for() {
        let mut harness =
            wizard("language = \"en\"\n", &gates(EngineCheck::Working, LocationCheck::Usable), Some(temporary("home")));
        harness.click_text("Somewhere else").render();
        let screen = harness.screen();
        assert!(screen.contains("Choose"), "the folder picker is on screen:\n{screen}");
    }

    #[test]
    fn a_folder_that_cannot_be_used_says_why_and_holds_the_wizard() {
        let problem = LocationProblem::Blocked("permission denied".to_owned());
        let mut harness = wizard(
            "language = \"en\"\n",
            &gates(EngineCheck::Working, LocationCheck::Broken(problem)),
            Some(temporary("home")),
        );
        let screen = harness.screen();
        assert!(screen.contains("permission denied"), "the system's own reason is shown:\n{screen}");
        harness.click_text("Finish").render();
        assert!(!harness.app().setup.is_finished(), "a folder that cannot be used finishes nothing");
        assert!(harness.screen().contains("Pick a folder QCode can write in"), "{}", harness.screen());
    }

    #[test]
    fn finishing_writes_the_setup_down() {
        let home = temporary("finish-home");
        let mut harness =
            wizard("language = \"en\"\n", &gates(EngineCheck::Working, LocationCheck::Usable), Some(home.clone()));
        harness.click_text("Finish").render();
        let setup = &harness.app().setup;
        assert!(setup.is_finished(), "{}", harness.screen());
        let config = setup.config();
        assert!(config.setup_completed(), "the wizard never opens again");
        assert_eq!(config.engine_kind(), Some("podman"));
        assert_eq!(config.workspace_path(), Some(home.join("Documents").join("QCode")));
    }

    #[test]
    fn the_keyboard_walks_the_wizard_from_end_to_end() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        // The steps take the first stop, because a finished step can be gone back to from the
        // keyboard as well as the mouse, and the page that scrolls takes one of its own.
        harness.press("tab");
        assert!(harness.is_focused("wizard-steps"), "{}", harness.screen());
        let reached = (0..3).any(|_| {
            harness.press("tab");
            harness.is_focused("setup-engine")
        });
        assert!(reached, "the choice is a few stops away, never out of reach:\n{}", harness.screen());
        harness.press("down").render();
        assert_eq!(harness.app().setup.engine_name(), "docker", "{}", harness.screen());
        harness.press("up").render();
        assert_eq!(harness.app().setup.engine_name(), "podman");
    }

    #[test]
    fn while_an_answer_is_awaited_the_wizard_cannot_be_hurried() {
        let mut harness = wizard(
            "language = \"en\"\n",
            &gates(EngineCheck::Running, LocationCheck::Unknown),
            Some(temporary("home")),
        );
        harness.click_text("Next").render();
        assert_eq!(harness.app().setup.step(), SetupStep::Engine, "{}", harness.screen());
    }

    #[test]
    fn going_back_returns_to_a_step_with_its_answer_still_on_it() {
        let mut harness =
            wizard("language = \"en\"\n", &gates(EngineCheck::Working, LocationCheck::Usable), Some(temporary("home")));
        harness.click_text("Back").render();
        assert_eq!(harness.app().setup.step(), SetupStep::Engine);
        assert!(harness.screen().contains("It is ready"), "{}", harness.screen());
    }

    #[test]
    fn the_pointer_lights_the_option_it_is_over() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        let (x, y) = harness.find("Docker").expect("docker is offered");
        let (x, y) = (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"));
        let row = |harness: &Harness<Wizard>| {
            (0..SIZE.0).map(|cell| (harness.fg(cell, y), harness.bg(cell, y))).collect::<Vec<_>>()
        };
        let quiet = row(&harness);
        harness.hover(i32::from(x), i32::from(y)).render();
        assert_ne!(row(&harness), quiet, "the row under the pointer answers:\n{}", harness.screen());
    }

    #[test]
    fn a_narrow_screen_keeps_the_wizard_usable() {
        let setup = Setup::new(
            Config::parse_str("code.toml", "language = \"en\"\n"),
            &dirs(Some(temporary("home"))),
            &gates(EngineCheck::Broken(EngineProblem::NotInstalled), LocationCheck::Unknown),
            host(),
        );
        let mut harness = Harness::with_env(Wizard { setup }, env(), 34, 20);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).render();
        let screen = harness.screen();
        assert!(screen.contains("Podman"), "the choice survives a narrow terminal:\n{screen}");
        assert!(screen.contains("Next"), "the way on is never pushed off the screen:\n{screen}");
        assert!(!screen.contains("paru -S podman"), "there is no room for the command yet:\n{screen}");
        for _ in 0..4 {
            harness.mouse(MouseKind::ScrollDown, 10, 8);
        }
        let screen = harness.screen();
        assert!(screen.contains("paru -S podman"), "what is below the fold is scrolled to:\n{screen}");
        assert!(screen.contains("Next"), "the buttons stay where they were:\n{screen}");
    }

    #[test]
    fn ascii_mode_draws_nothing_but_ascii() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        harness.set_glyph_mode(GlyphMode::Ascii).render();
        let screen = harness.screen();
        assert!(screen.is_ascii(), "{screen}");
        assert!(screen.contains("paru -S podman"), "{screen}");
    }

    #[test]
    fn every_choice_carries_the_small_square_and_the_chosen_one_its_tone() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        harness.set_reduced_motion(true).render();
        let square = harness.env().icons().glyph("radio-mark-small").into_owned();
        let screen = harness.screen();
        // The row of an option is the one where its label follows the square; the mark is two
        // cells and stands two cells before the label.
        let mark = |label: &str| {
            let (y, line) = screen
                .lines()
                .enumerate()
                .find(|(_, line)| line.contains(&format!("{square}  {label}")))
                .unwrap_or_else(|| panic!("`{label}` is offered with the small square:\n{screen}"));
            let x = line[..line.find(label).expect("the label is on the row")].chars().count();
            let (x, y) = (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"));
            harness.fg(x - 4, y)
        };
        let (chosen, other) = (mark("Podman"), mark("Docker"));
        assert_ne!(chosen, other, "the chosen square is told apart by its tone:\n{screen}");
    }

    #[test]
    fn nothing_is_bracketed_lined_or_framed() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
            harness.set_glyph_mode(mode).render();
            let screen = harness.screen();
            for forbidden in ['[', ']', '(', ')', '{', '}', '|', '┌', '─', '│', '+'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` in {mode:?}:\n{screen}");
            }
            assert!(!screen.contains("=="), "{screen}");
            assert!(!screen.contains("->"), "{screen}");
        }
    }

    #[test]
    fn reduced_motion_keeps_the_wizard_working() {
        let mut harness = on_engine("language = \"en\"\n", EngineProblem::NotInstalled);
        harness.set_reduced_motion(true).click_text("Docker").render();
        assert_eq!(harness.app().setup.engine_name(), "docker", "{}", harness.screen());
    }

    #[test]
    fn turkish_reads_as_turkish() {
        let mut harness = on_engine("language = \"tr\"\n", EngineProblem::NotInstalled);
        harness.set_locale("tr").render();
        let screen = harness.screen();
        for text in ["Kapsayıcı motoru", "kurulu değil", "Yeniden denetle", "yetkisini yükseltmez"] {
            assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
        }
    }
}
