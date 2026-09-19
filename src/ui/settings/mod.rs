//! The settings screen: what the person can change about QCode, and what the shell has already
//! decided for them.
//!
//! Everything the framework keeps for every Quvyta application — language, theme, icons and
//! reduced motion — is read straight from the environment the screen draws in, which is the one
//! place that knows what is actually in force. QCode's own settings, the container engine and
//! the workspace folder, come from [`Config`], which repairs the file it reads; the repairs are
//! shown here rather than swallowed.
//!
//! The screen changes nothing outside itself. A choice is applied at once, so it can be seen,
//! and handed to the application as a [`Request`] to be stored or carried out. That keeps the
//! settings file, the setup wizard and the container engine in the hands that own them.

pub mod bar;
pub mod engine;
pub mod identity;

use std::path::PathBuf;

use qframe::diagnostics::{Diagnostic, Severity};
use qframe::icons::IconMode;
use qframe::prelude::*;
use qframe::widgets::{ScrollView, Segmented, Select, SettingRow, SettingsList, SettingsRows, Spinner, Switch, Toast};

use crate::backup::BackupEvery;
use crate::base::apps::{Editor, Sound};
use crate::engine::EngineKind;
use crate::profile::SafeName;
use crate::workspace::{Config, OnClose, Platform};

use engine::{EngineState, Gate, Health};
use identity::ProfileIdentity;

/// Width of the drop-downs in the settings list. Wide enough for the longest theme and language
/// name, narrow enough to leave the labels their column.
const CONTROL_WIDTH: u16 = 20;

/// Rows between the blocks of the screen.
const BLOCK_GAP: u16 = 1;

/// Widest the settings column grows, in cells. A label, its description and a drop-down read as
/// one line at this width; on a wide terminal a list stretched edge to edge leaves the label and
/// its control too far apart to read together, so the column stays this wide in the middle.
const PAGE_WIDTH: u16 = 84;

/// The left margin a settings list keeps for its pillar. Everything drawn under the list keeps
/// it too, so nothing stands further left than the settings themselves.
const LEAD: u16 = 2;

/// The engines offered, in the order the setup wizard offers them.
const ENGINES: [EngineKind; 2] = [EngineKind::Podman, EngineKind::Docker];

/// What the settings screen asks the application to do, because it reaches past the screen.
///
/// The first five, the editor and the sound choice are choices already applied to the running application and only waiting to be
/// written down. The rest are work only the layers that own it can do: the setup wizard's steps
/// and the engine's volumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Store the language. The application already speaks it.
    Language(String),
    /// Store the theme. The application already wears it.
    Theme(String),
    /// Store the icon mode. The application already draws in it.
    Icons(IconMode),
    /// Store reduced motion. The application already moves that way.
    ReducedMotion(bool),
    /// Store the container engine and look for it again.
    Engine(EngineKind),
    /// Store the editor a file opens in, and open files in it from now on.
    Editor(Editor),
    /// Store what opening a sound does, and do that from now on.
    Sound(Sound),
    /// Run the setup wizard's engine step on its own. This is what the repair strip asks for.
    OpenEngineStep,
    /// Run the setup wizard's location step on its own, to move the workspace somewhere else.
    OpenLocationStep,
    /// Write the profile's stored login into every project that uses it. Confirmed.
    RefreshIdentity(SafeName),
    /// Delete the profile's credentials volume. Confirmed.
    SignOut(SafeName),
    /// Store what happens to the containers once no QCode is open.
    OnClose(OnClose),
    /// Store how often the open projects are backed up, and back them up that often from now on.
    BackupEvery(BackupEvery),
    /// Install the background service.
    InstallService,
    /// Remove the background service.
    RemoveService,
}

/// What the settings screen shows about the background service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRow {
    /// This system gets no service, and the row says why: Windows gives QCode no lock to tell
    /// when the last QCode closes, so there containers are not stopped at all.
    Unsupported,
    /// The service can be installed or removed.
    Ready {
        /// Whether its files are in place.
        installed: bool,
        /// Whether installing or removing it is under way.
        busy: bool,
    },
}

impl ServiceRow {
    /// The row for a machine of `platform`: `installed` is whether the service's files are
    /// there, or `None` where there is nowhere to put them — a machine with no home, which gets
    /// no row at all.
    #[must_use]
    pub fn of(platform: Platform, installed: Option<bool>) -> Option<Self> {
        match (platform, installed) {
            (Platform::Windows, _) => Some(Self::Unsupported),
            (_, None) => None,
            (_, Some(installed)) => Some(Self::Ready { installed, busy: false }),
        }
    }
}

/// What can happen on the settings screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// The engine check answered.
    Checked(Health),
    /// The profiles and their logins were read.
    Profiles(Vec<ProfileIdentity>),
    /// A language was chosen.
    Language(String),
    /// A theme was chosen.
    Theme(String),
    /// An icon mode was chosen.
    Icons(IconMode),
    /// The reduce-motion switch was moved.
    ReduceMotion(bool),
    /// An engine was chosen.
    Engine(EngineKind),
    /// An editor was chosen.
    Editor(Editor),
    /// What opening a sound does was chosen.
    Sound(Sound),
    /// A profile was chosen.
    Profile(usize),
    /// Refreshing the chosen profile's login was asked for; the question follows.
    AskRefresh,
    /// Signing the chosen profile out was asked for; the question follows.
    AskSignOut,
    /// Something the application does, past every question it needed.
    Request(Request),
    /// The repair report was read and can go.
    ReadRepairs,
    /// The report of the old settings files that were not moved was read and can go.
    ReadLeftBehind,
    /// The application stored the last change, or could not.
    Stored(Result<(), String>),
    /// What happens to the containers once no QCode is open was chosen.
    OnClose(OnClose),
    /// How often the open projects are backed up was chosen.
    BackupEvery(BackupEvery),
    /// Installing or removing the background service finished: which of the two it was, whether
    /// the service's files are there now, and the words of what failed.
    ServiceDone {
        /// Whether it was installing rather than removing.
        installing: bool,
        /// Whether the service's files are in place now.
        installed: bool,
        /// What failed, in the words of the command that failed.
        result: Result<(), String>,
    },
}

/// The settings screen's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    engine: EngineState,
    editor: Editor,
    sound: Sound,
    workspace: Option<PathBuf>,
    repairs: Vec<Diagnostic>,
    repairs_read: bool,
    left_behind: Vec<Diagnostic>,
    profiles: Option<Vec<ProfileIdentity>>,
    chosen: usize,
    failure: Option<String>,
    on_close: OnClose,
    backup_every: BackupEvery,
    service: Option<ServiceRow>,
}

impl Settings {
    /// A settings screen showing what `config` holds, including every repair its file needed,
    /// with `engine` as the last check left it. The profiles are not read yet.
    #[must_use]
    pub fn new(config: &Config, engine: EngineState) -> Self {
        Self {
            engine,
            editor: config.editor(),
            sound: config.sound(),
            workspace: config.workspace_path(),
            repairs: config.diagnostics().to_vec(),
            repairs_read: false,
            left_behind: Vec::new(),
            profiles: None,
            chosen: 0,
            failure: None,
            on_close: config.on_close(),
            backup_every: config.backup_every(),
            service: None,
        }
    }

    /// The same screen, showing the background service as `row` says; `None` shows no row.
    #[must_use]
    pub fn with_service(mut self, row: Option<ServiceRow>) -> Self {
        self.service = row;
        self
    }

    /// What the screen shows about the background service.
    #[must_use]
    pub fn service(&self) -> Option<ServiceRow> {
        self.service
    }

    /// The same screen, also reporting `files`: the settings files of an earlier version that
    /// stayed where they were when the settings moved to the family's folder, each with the
    /// reason.
    #[must_use]
    pub fn with_left_behind(mut self, files: Vec<Diagnostic>) -> Self {
        self.left_behind = files;
        self
    }

    /// The old settings files the screen still has to report, so a screen made again from new
    /// settings keeps the news the person has not read yet.
    #[must_use]
    pub fn left_behind(&self) -> &[Diagnostic] {
        &self.left_behind
    }

    /// The engine as the last check left it, for the repair strip.
    #[must_use]
    pub fn engine(&self) -> &EngineState {
        &self.engine
    }

    /// Whether the actions that reach into a container may be used, and why not when they may
    /// not. Screens outside settings use the same gate, so a dead action means the same thing
    /// everywhere.
    #[must_use]
    pub fn gate(&self) -> Gate {
        Gate::of(&self.engine)
    }

    /// The profile the identity actions work on.
    #[must_use]
    pub fn chosen_profile(&self) -> Option<&ProfileIdentity> {
        self.profiles.as_ref()?.get(self.chosen)
    }
}

/// The line under the reduce-motion row while the shell decides it.
///
/// `QUVYTA_REDUCED_MOTION` wins over the saved setting and over anything the application asks
/// for, so the row says which value is in force and where it comes from. Without that the switch
/// would simply refuse to move, and refusing without a reason is how settings screens lose
/// people's trust.
#[must_use]
pub fn motion_note(forced: bool, reduced: bool) -> Option<String> {
    forced.then(|| if reduced { t!("settings.forced-on") } else { t!("settings.forced-off") })
}

/// Applies a settings message, answering with the command to run and what the application has
/// to do about it.
pub fn update(screen: &mut Settings, message: Msg) -> (Command<Msg>, Option<Request>) {
    match message {
        Msg::Checked(health) => {
            screen.engine = EngineState::new(screen.engine.kind(), health);
            (Command::none(), None)
        }
        Msg::Profiles(profiles) => {
            screen.chosen = screen.chosen.min(profiles.len().saturating_sub(1));
            screen.profiles = Some(profiles);
            (Command::none(), None)
        }
        Msg::Language(code) => (Command::set_locale(code.clone()), Some(Request::Language(code))),
        Msg::Theme(id) => (Command::set_theme(id.clone()), Some(Request::Theme(id))),
        Msg::Icons(mode) => (Command::set_icon_mode(mode), Some(Request::Icons(mode))),
        Msg::ReduceMotion(reduced) => (Command::set_reduced_motion(reduced), Some(Request::ReducedMotion(reduced))),
        Msg::Engine(kind) => {
            // The engine is unknown again until the application has looked for the new one.
            screen.engine = EngineState::new(kind, Health::Checking);
            (Command::none(), Some(Request::Engine(kind)))
        }
        Msg::Editor(editor) => {
            screen.editor = editor;
            (Command::none(), Some(Request::Editor(editor)))
        }
        Msg::Sound(sound) => {
            screen.sound = sound;
            (Command::none(), Some(Request::Sound(sound)))
        }
        Msg::Profile(index) => {
            if index < screen.profiles.as_ref().map_or(0, Vec::len) {
                screen.chosen = index;
            }
            (Command::none(), None)
        }
        Msg::AskRefresh => (ask(screen, false), None),
        Msg::AskSignOut => (ask(screen, true), None),
        Msg::Request(request) => {
            if matches!(request, Request::InstallService | Request::RemoveService) {
                match &mut screen.service {
                    // A second press while the first is under way asks for nothing.
                    Some(ServiceRow::Ready { busy: busy @ false, .. }) => *busy = true,
                    _ => return (Command::none(), None),
                }
            }
            (Command::none(), Some(request))
        }
        Msg::OnClose(choice) => {
            screen.on_close = choice;
            (Command::none(), Some(Request::OnClose(choice)))
        }
        Msg::BackupEvery(choice) => {
            screen.backup_every = choice;
            (Command::none(), Some(Request::BackupEvery(choice)))
        }
        Msg::ServiceDone { installing, installed, result } => {
            if let Some(ServiceRow::Ready { .. }) = screen.service {
                screen.service = Some(ServiceRow::Ready { installed, busy: false });
            }
            let told = match result {
                Ok(()) => Command::none(),
                Err(reason) => {
                    let title = if installing {
                        t!("settings.service-install-failed")
                    } else {
                        t!("settings.service-remove-failed")
                    };
                    Command::toast(Toast::danger(title).body(reason))
                }
            };
            (told, None)
        }
        Msg::ReadRepairs => {
            screen.repairs_read = true;
            (Command::focus(entry(screen)), None)
        }
        Msg::ReadLeftBehind => {
            screen.left_behind.clear();
            (Command::focus(entry(screen)), None)
        }
        Msg::Stored(stored) => {
            screen.failure = stored.err();
            (Command::none(), None)
        }
    }
}

/// The question that stands before an identity action, saying what it does rather than asking
/// whether the person is sure.
fn ask(screen: &Settings, sign_out: bool) -> Command<Msg> {
    let Some(profile) = screen.chosen_profile() else {
        return Command::none();
    };
    let name = profile.name().clone();
    let shown = name.as_str().to_owned();
    let question = if sign_out {
        Confirm::new(t!("settings.sign-out-title", profile = shown.clone()), Msg::Request(Request::SignOut(name)))
            .message(t!("settings.sign-out-text", profile = shown))
            .confirm_label(t!("settings.sign-out-do"))
            .danger()
    } else {
        let request = Request::RefreshIdentity(name);
        Confirm::new(t!("settings.refresh-title", profile = shown.clone()), Msg::Request(request))
            .message(t!("settings.refresh-text", profile = shown))
            .confirm_label(t!("settings.refresh-do"))
    };
    Command::confirm(question)
}

/// Draws the settings screen.
pub fn view(screen: &Settings, ui: &mut View<'_, Msg>) {
    // What is in force right now, not what was saved: the environment has the last word on some
    // of it, and a settings screen that shows the saved value would show a value nobody has.
    let languages = ui.env().i18n().list();
    let language = ui.env().i18n().active().to_owned();
    let themes = ui.env().themes();
    let theme = ui.env().theme().id().to_owned();
    let icons = ui.env().icon_mode();
    let reduced = ui.env().reduced_motion();
    let forced = ui.env().reduced_motion_forced();
    let gate = screen.gate();

    let width = ui.size().width.min(PAGE_WIDTH);
    ui.add_with(ScrollView::new(), |ui| {
        ui.row(|ui| {
            ui.column(|ui| {
                report(&Report::LEFT_BEHIND, &screen.left_behind, ui);
                if !screen.repairs_read {
                    report(&Report::REPAIRS, &screen.repairs, ui);
                }
                if let Some(reason) = &screen.failure {
                    ui.add(Text::new(t!("settings.store-failed", reason = reason.clone())).color("danger"))
                        .fill_width();
                }

                let list = SettingsList::show(ui, |list| {
                    list.heading(t!("settings.appearance"));

                    let codes: Vec<String> = languages.iter().map(|(code, _)| code.clone()).collect();
                    let names: Vec<String> = languages.iter().map(|(_, name)| name.clone()).collect();
                    let chosen = codes.iter().position(|code| *code == language);
                    list.row(SettingRow::new(t!("settings.language")), |ui| {
                        ui.add(
                            Select::new(names)
                                .selected(chosen)
                                .on_select(move |index| Msg::Language(codes[index].clone())),
                        )
                        .id("language")
                        .width(Length::Cells(CONTROL_WIDTH));
                    });

                    let ids: Vec<String> = themes.iter().map(|(id, _)| id.clone()).collect();
                    let titles: Vec<String> = themes.iter().map(|(_, name)| name.clone()).collect();
                    let chosen = ids.iter().position(|id| *id == theme);
                    list.row(SettingRow::new(t!("settings.theme")), |ui| {
                        ui.add(
                            Select::new(titles).selected(chosen).on_select(move |index| Msg::Theme(ids[index].clone())),
                        )
                        .id("theme")
                        .width(Length::Cells(CONTROL_WIDTH));
                    });

                    let modes = IconMode::ALL.map(|mode| t!(&format!("settings.icons-{}", mode.name())));
                    let chosen = IconMode::ALL.iter().position(|mode| *mode == icons);
                    list.row(SettingRow::new(t!("settings.icons")), |ui| {
                        ui.add(
                            Select::new(modes)
                                .selected(chosen)
                                .on_select(move |index| Msg::Icons(IconMode::ALL[index])),
                        )
                        .id("icons")
                        .width(Length::Cells(CONTROL_WIDTH));
                    });

                    let note = motion_note(forced, reduced).unwrap_or_else(|| t!("settings.reduce-motion-text"));
                    list.row(SettingRow::new(t!("settings.reduce-motion")).description(note).disabled(forced), |ui| {
                        ui.add(Switch::new(reduced).disabled(forced).on_toggle(Msg::ReduceMotion));
                    });

                    list.heading(t!("settings.containers"));
                    let row = SettingRow::new(t!("settings.engine")).description(screen.engine.summary());
                    list.row(row, |ui| match screen.engine.health() {
                        Health::Checking => {
                            ui.add(Spinner::new());
                        }
                        Health::Working | Health::Missing(_) => {
                            let names = ENGINES.map(engine::name);
                            let chosen = ENGINES.iter().position(|kind| *kind == screen.engine.kind()).unwrap_or(0);
                            ui.add(
                                Segmented::new(names)
                                    .selected(chosen)
                                    .on_select(move |index| Msg::Engine(ENGINES[index])),
                            )
                            .id("engine");
                        }
                    });

                    let choices = OnClose::ALL.map(|choice| t!(&format!("settings.on-close-{}", choice.key())));
                    let chosen = OnClose::ALL.iter().position(|choice| *choice == screen.on_close).unwrap_or(0);
                    // Where nothing can tell that the last QCode closed, the choice is kept for
                    // the file's sake but the row says plainly that it changes nothing here.
                    let about = if screen.service == Some(ServiceRow::Unsupported) {
                        t!("settings.on-close-windows")
                    } else {
                        t!("settings.on-close-text")
                    };
                    list.row(SettingRow::new(t!("settings.on-close")).description(about), |ui| {
                        ui.add(
                            Segmented::new(choices)
                                .selected(chosen)
                                .on_select(|index| Msg::OnClose(OnClose::ALL[index])),
                        )
                        .id("on-close");
                    });
                    if let Some(row) = screen.service {
                        service_row(list, row);
                    }

                    list.heading(t!("settings.backup"));
                    let choices = BackupEvery::ALL.map(|choice| t!(&format!("settings.backup-every-{}", choice.key())));
                    let chosen = BackupEvery::ALL.iter().position(|choice| *choice == screen.backup_every);
                    let row =
                        SettingRow::new(t!("settings.backup-every")).description(t!("settings.backup-every-text"));
                    list.row(row, |ui| {
                        ui.add(
                            Segmented::new(choices)
                                .selected(chosen.unwrap_or(0))
                                .on_select(|index| Msg::BackupEvery(BackupEvery::ALL[index])),
                        )
                        .id("backup-every");
                    });

                    list.heading(t!("settings.apps"));
                    // The program's own name is the label: it is what the person knows it by, in
                    // every language.
                    let names = Editor::ALL.map(Editor::key);
                    let chosen = Editor::ALL.iter().position(|editor| *editor == screen.editor).unwrap_or(0);
                    list.row(SettingRow::new(t!("settings.editor")).description(t!("settings.editor-text")), |ui| {
                        ui.add(
                            Segmented::new(names)
                                .selected(chosen)
                                .on_select(move |index| Msg::Editor(Editor::ALL[index])),
                        )
                        .id("editor");
                    });
                    let choices = Sound::ALL.map(|choice| t!(&format!("settings.sound-{}", choice.key())));
                    let chosen = Sound::ALL.iter().position(|sound| *sound == screen.sound).unwrap_or(0);
                    list.row(SettingRow::new(t!("settings.sound")).description(t!("settings.sound-text")), |ui| {
                        ui.add(
                            Segmented::new(choices).selected(chosen).on_select(|index| Msg::Sound(Sound::ALL[index])),
                        )
                        .id("sound");
                    });

                    list.heading(t!("settings.workspace"));
                    let folder = screen
                        .workspace
                        .as_ref()
                        .map_or_else(|| t!("settings.workspace-unset"), |path| path.display().to_string());
                    let row = SettingRow::new(t!("settings.workspace-folder"))
                        .description(folder)
                        .on_activate(Msg::Request(Request::OpenLocationStep));
                    list.row(row, |ui| {
                        ui.add(Text::new(t!("settings.workspace-change")).role("secondary"));
                    });
                });
                list.id("settings");
                // Everything under the list keeps the list's own left margin, so the screen reads as
                // one column rather than as a list with loose text beside it.
                ui.column(|ui| {
                    // What to do about a missing engine, and whatever the engine said, follow the
                    // list rather than the strip: here there is a screen to read them on.
                    if let Health::Missing(trouble) = screen.engine.health() {
                        let kind = screen.engine.kind();
                        ui.add(Text::new(trouble.remedy(kind)).role("secondary")).fill_width();
                        if let Some(said) = trouble.said() {
                            let words = t!("settings.engine-said", engine = engine::name(kind), output = said);
                            ui.add(Text::new(words).role("faint")).fill_width();
                        }
                    }
                    ui.spacer().height(Length::Cells(BLOCK_GAP));
                    ui.add(Text::new(t!("settings.profiles")).role("secondary").bold());
                    identity::view(screen.profiles.as_deref(), screen.chosen, &gate, ui);
                })
                .fill_width()
                .padding(Padding { left: LEAD, ..Padding::default() });
            })
            .width(Length::Cells(width))
            .gap(BLOCK_GAP)
            .id("settings-page");
        })
        .fill_width()
        .justify(Align::Center);
    })
    .fill();
}

/// The row of the background service: what it does and whether it is there, with the button
/// that installs or removes it.
fn service_row(list: &mut SettingsRows<'_, Msg>, row: ServiceRow) {
    let description = match row {
        ServiceRow::Unsupported => t!("settings.service-windows"),
        ServiceRow::Ready { installed: true, .. } => t!("settings.service-installed"),
        ServiceRow::Ready { installed: false, .. } => t!("settings.service-not-installed"),
    };
    list.row(SettingRow::new(t!("settings.service")).description(description), |ui| match row {
        ServiceRow::Unsupported => {}
        ServiceRow::Ready { busy: true, .. } => {
            ui.add(Spinner::new());
        }
        ServiceRow::Ready { installed, busy: false } => {
            let (label, request) = if installed {
                (t!("settings.service-remove"), Request::RemoveService)
            } else {
                (t!("settings.service-install"), Request::InstallService)
            };
            ui.add(Button::new(label).on_press(Msg::Request(request))).id("service");
        }
    });
}

/// A report the settings screen stands at its top, one line per diagnostic.
struct Report {
    title: &'static str,
    text: &'static str,
    dismiss: Msg,
    id: &'static str,
}

impl Report {
    /// What reading the settings file had to put right.
    const REPAIRS: Self = Self {
        title: "settings.repaired",
        text: "settings.repaired-text",
        dismiss: Msg::ReadRepairs,
        id: "repairs-read",
    };

    /// The settings files of an earlier version that were not moved into the family's folder.
    const LEFT_BEHIND: Self = Self {
        title: "settings.left-behind",
        text: "settings.left-behind-text",
        dismiss: Msg::ReadLeftBehind,
        id: "left-behind-read",
    };
}

/// A report over `lines`. It stands until it is read: a repair or a file left behind is news,
/// and news that disappears on its own is news nobody got.
fn report(which: &Report, lines: &[Diagnostic], ui: &mut View<'_, Msg>) {
    if lines.is_empty() {
        return;
    }
    ui.add_with(Panel::new().title(t!(which.title)), |ui| {
        ui.add(Text::new(t!(which.text)).role("secondary"));
        for repair in lines {
            let place = repair.location.as_ref().map(ToString::to_string);
            let line = match place {
                Some(place) => format!("{place}  {}", repair.message),
                None => repair.message.clone(),
            };
            let colour = if repair.severity == Severity::Error { "danger" } else { "warning" };
            ui.add(Text::new(line).color(colour));
        }
        ui.add(Button::new(t!("settings.repaired-dismiss")).on_press(which.dismiss.clone())).id(which.id);
    })
    .fill_width();
}

/// The control that takes the keyboard when the screen opens: the list of settings, so the
/// arrow keys walk it from the first key the person presses.
///
/// A report that stands above the list comes first: it is news the person has not read, and the
/// list is taller than a small terminal, so focusing the list would scroll the report out of
/// sight. Its dismiss button takes the keyboard instead, and dismissing it hands the keyboard on.
#[must_use]
pub fn entry(screen: &Settings) -> &'static str {
    if !screen.left_behind.is_empty() {
        Report::LEFT_BEHIND.id
    } else if !screen.repairs_read && !screen.repairs.is_empty() {
        Report::REPAIRS.id
    } else {
        "settings"
    }
}

/// The keys of the settings screen that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("hints.move")), (icons.glyph("enter").into_owned(), t!("hints.open"))]
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;

    use qframe::env::{AssetDirs, Env};
    use qframe::i18n::I18n;
    use qframe::icons::GlyphMode;
    use qframe::prelude::*;
    use qframe::runtime::Harness;

    use super::engine::{EngineState, Health};
    use super::identity::ProfileIdentity;
    use super::{Msg, Request, Settings};
    use crate::engine::EngineKind;
    use crate::profile::SafeName;
    use crate::workspace::Config;

    /// Runs `body` with QCode's own text loaded, for the functions that translate outside a
    /// running application.
    pub fn translated<R>(locale: &str, body: impl FnOnce() -> R) -> R {
        let mut catalog = I18n::builtin();
        for (file, text) in crate::locales() {
            catalog.add_source(&file, &text);
        }
        catalog.set_active(locale);
        qframe::i18n::scope(Arc::new(catalog), body)
    }

    /// A settings screen over a clean settings file.
    pub fn screen(kind: EngineKind, health: Health) -> Settings {
        from_config("", kind, health)
    }

    /// A settings screen over the settings file `text`, repairs and all.
    pub fn from_config(text: &str, kind: EngineKind, health: Health) -> Settings {
        Settings::new(&Config::parse_str("code.conf", text), EngineState::new(kind, health))
    }

    /// A profile with or without a stored login.
    pub fn profile(name: &str, stored: bool) -> ProfileIdentity {
        ProfileIdentity::new(SafeName::parse(name).expect("the test names are already safe"), stored)
    }

    /// The application message a settings message becomes in the tests.
    #[must_use]
    pub fn wrap(message: Msg) -> HostMsg {
        HostMsg::Screen(message)
    }

    /// What reaches the test application.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum HostMsg {
        /// A settings message.
        Screen(Msg),
    }

    /// The application the screen tests drive: the settings screen in a shell, with the repair
    /// strip in the header where the real application puts it.
    pub struct Host {
        /// The screen under test.
        pub screen: Settings,
        /// Everything the screen asked the application to do, in order.
        pub asked: Vec<Request>,
    }

    impl App for Host {
        type Msg = HostMsg;

        fn update(&mut self, message: HostMsg) -> Command<HostMsg> {
            let HostMsg::Screen(message) = message;
            let (command, request) = super::update(&mut self.screen, message);
            if let Some(request) = request {
                self.asked.push(request);
            }
            command.map(HostMsg::Screen)
        }

        fn view(&self, ui: &mut View<'_, HostMsg>) {
            AppShell::new()
                .header(|ui| {
                    ui.map(HostMsg::Screen, |ui| super::bar::view(self.screen.engine(), ui)).fill_width();
                })
                .body(|ui| {
                    ui.map(HostMsg::Screen, |ui| super::view(&self.screen, ui)).fill();
                })
                .show(ui);
        }
    }

    fn env() -> Env {
        Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() })
            .expect("the built-in files load")
    }

    /// A harness showing `screen` in English with Unicode glyphs.
    pub fn host(screen: Settings, width: u16, height: u16) -> Harness<Host> {
        let mut harness = Harness::with_env(Host { screen, asked: Vec::new() }, env(), width, height);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        harness.render();
        harness
    }
}

#[cfg(test)]
mod tests {
    use qframe::icons::GlyphMode;

    use super::testing;
    use crate::backup::BackupEvery;
    use crate::base::apps::{Editor, Sound};
    use crate::engine::EngineKind;
    use crate::ui::settings::Request;
    use crate::ui::settings::engine::{Health, Trouble};
    use crate::workspace::{OnClose, Platform};

    const SIZE: (u16, u16) = (100, 34);

    #[test]
    fn the_common_settings_are_the_frameworks_own() {
        let harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        let screen = harness.screen();
        for label in ["Language", "Theme", "Icons", "Reduce motion", "Container engine", "Folder"] {
            assert!(screen.contains(label), "`{label}` is missing:\n{screen}");
        }
        assert!(screen.contains("English"), "the language in force is the one shown:\n{screen}");
    }

    #[test]
    fn the_settings_stand_in_the_middle_at_a_readable_width() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), 160, SIZE.1);
        harness.render();
        let (label, _) = harness.find("Language").expect("the language row is shown");
        let (folder, _) = harness.find("Folder").expect("the folder row is shown");
        let left = (160 - i32::from(super::PAGE_WIDTH)) / 2;
        assert!(
            label >= left && folder >= left,
            "the column starts in the middle, not at the edge:\n{}",
            harness.screen()
        );
        // The control at the end of a row ends where the column does.
        let (change, _) = harness.find("Change").expect("the folder row offers a change");
        let right = left + i32::from(super::PAGE_WIDTH);
        assert!(change + 6 <= right, "nothing reaches past the column:\n{}", harness.screen());
        assert!(change + 6 >= right - 4, "the column is as wide as it may be:\n{}", harness.screen());

        // A terminal narrower than the column gives it all of its width.
        harness.resize(70, SIZE.1).render();
        assert!(harness.screen().contains("Language"), "{}", harness.screen());
        let (label, _) = harness.find("Language").expect("the language row is shown");
        assert!(label < 8, "a narrow terminal keeps the column at its edge:\n{}", harness.screen());
    }

    #[test]
    fn a_shell_that_decides_reduced_motion_says_so_in_place_of_the_description() {
        // The switch cannot move while the variable is set, so the row explains rather than
        // simply refusing. The environment is only readable through a real process environment,
        // which a test may not change, so the wording is checked where it is decided.
        testing::translated("en", || {
            let forced_on = super::motion_note(true, true).expect("a forced setting says so");
            assert!(forced_on.contains("QUVYTA_REDUCED_MOTION"), "{forced_on}");
            let forced_off = super::motion_note(true, false).expect("a forced setting says so");
            assert!(forced_off.contains("QUVYTA_REDUCED_MOTION"), "{forced_off}");
            assert_ne!(forced_on, forced_off, "the value in force is named, not just the variable");
            assert_eq!(super::motion_note(false, true), None, "a saved setting explains nothing");
        });
    }

    #[test]
    fn the_turkish_note_is_turkish() {
        testing::translated("tr", || {
            let note = super::motion_note(true, true).expect("a forced setting says so");
            assert!(note.contains("Kabuğunda"), "{note}");
            assert!(note.contains("değiştirilemez"), "{note}");
        });
    }

    #[test]
    fn choosing_a_language_changes_the_application_and_asks_for_it_to_be_stored() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        harness.send(testing::wrap(super::Msg::Language("tr".to_owned()))).render();
        assert_eq!(harness.app().asked, [Request::Language("tr".to_owned())]);
        let screen = harness.screen();
        assert!(screen.contains("Kapsayıcı motoru"), "the screen speaks it at once:\n{screen}");
    }

    #[test]
    fn choosing_an_engine_stops_claiming_the_old_one_works() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        harness.send(testing::wrap(super::Msg::Engine(EngineKind::Docker))).render();
        assert_eq!(harness.app().asked, [Request::Engine(EngineKind::Docker)]);
        let screen = harness.screen();
        assert!(screen.contains("Looking for the engine"), "{screen}");
        assert!(!screen.contains("Podman answers"), "{screen}");
    }

    #[test]
    fn the_workspace_row_shows_the_folder_and_opens_the_step_that_moves_it() {
        let text = "[workspace]\npath = \"/home/ada/Documents/QCode\"\n";
        let screen = testing::from_config(text, EngineKind::Podman, Health::Working);
        let mut harness = testing::host(screen, SIZE.0, SIZE.1);
        assert!(harness.screen().contains("/home/ada/Documents/QCode"), "{}", harness.screen());
        harness.click_text("Change");
        assert_eq!(harness.app().asked, [Request::OpenLocationStep]);
    }

    #[test]
    fn a_workspace_that_was_never_chosen_says_so_instead_of_showing_nothing() {
        let harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        assert!(harness.screen().contains("Not chosen yet"), "{}", harness.screen());
    }

    #[test]
    fn a_repaired_settings_file_is_reported_with_the_place_of_every_repair() {
        // The unknown key is removed by `Config`; the screen's work is to say that it was.
        let screen = testing::from_config(
            "[engine]\nkind = \"podman\"\nspeed = \"fast\"\n",
            EngineKind::Podman,
            Health::Working,
        );
        let mut harness = testing::host(screen, SIZE.0, SIZE.1);
        let shown = harness.screen();
        assert!(shown.contains("The settings file was repaired"), "{shown}");
        assert!(shown.contains("code.conf:3:1"), "the repair is pinned to its place:\n{shown}");
        harness.click_text("Got it");
        assert!(!harness.screen().contains("The settings file was repaired"), "{}", harness.screen());
    }

    #[test]
    fn old_settings_files_that_were_not_moved_are_reported_until_read() {
        let left = qframe::diagnostics::Diagnostic::warning(
            None,
            "/home/ada/.config/quvyta/code/settings.toml: code.conf is already there; it stays where it is",
        );
        let screen = testing::screen(EngineKind::Podman, Health::Working).with_left_behind(vec![left]);
        let mut harness = testing::host(screen, SIZE.0, SIZE.1);
        let shown = harness.screen();
        assert!(shown.contains("Some old settings files stayed where they were"), "{shown}");
        assert!(shown.contains("code/settings.toml"), "the file is named:\n{shown}");
        assert!(!shown.contains("was repaired"), "a clean file has nothing repaired:\n{shown}");
        harness.click_text("Got it");
        assert!(!harness.screen().contains("stayed where they were"), "{}", harness.screen());
    }

    #[test]
    fn a_clean_settings_file_reports_nothing() {
        let harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        assert!(!harness.screen().contains("was repaired"), "{}", harness.screen());
    }

    #[test]
    fn a_change_that_could_not_be_written_is_said_out_loud() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        let failed = super::Msg::Stored(Err("read-only file system".to_owned()));
        harness.send(testing::wrap(failed)).render();
        let screen = harness.screen();
        assert!(screen.contains("read-only file system"), "{screen}");
        harness.send(testing::wrap(super::Msg::Stored(Ok(())))).render();
        assert!(!harness.screen().contains("read-only file system"), "{}", harness.screen());
    }

    #[test]
    fn a_narrow_terminal_keeps_every_setting() {
        let harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), 46, 20);
        let screen = harness.screen();
        for label in ["Language", "Theme", "Reduce motion"] {
            assert!(screen.contains(label), "`{label}` is missing:\n{screen}");
        }
    }

    #[test]
    fn the_keyboard_reaches_the_settings_and_changes_one() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        // The scrolling body takes the first stop, the list of settings the second.
        harness.press("tab").press("tab");
        assert!(harness.is_focused("settings"), "{}", harness.screen());
        // Language, theme, icons, then the switch, which the space bar moves.
        harness.press("down").press("down").press("down").press("space");
        assert_eq!(harness.app().asked, [Request::ReducedMotion(true)]);
    }

    #[test]
    fn what_to_do_about_a_missing_engine_is_said_where_there_is_room_to_read_it() {
        let trouble = Trouble::Refused("short-name resolution enforced".to_owned());
        let harness = testing::host(testing::screen(EngineKind::Podman, Health::Missing(trouble)), SIZE.0, SIZE.1);
        let screen = harness.screen();
        assert!(screen.contains("Repair looks again"), "{screen}");
        assert!(screen.contains("short-name resolution enforced"), "the engine's own words are kept:\n{screen}");
    }

    #[test]
    fn the_editor_is_chosen_among_the_built_in_apps_and_nano_comes_first() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        let screen = harness.screen();
        for label in ["BUILT-IN APPS", "Editor", "nano", "vim"] {
            assert!(screen.contains(label), "`{label}` is missing:\n{screen}");
        }
        harness.send(testing::wrap(super::Msg::Editor(Editor::Vim))).render();
        assert_eq!(harness.app().asked, [Request::Editor(Editor::Vim)]);
        assert_eq!(harness.app().screen.editor, Editor::Vim);

        // A settings file that chose vim opens the screen on vim.
        let screen = testing::from_config("[apps]\neditor = \"vim\"\n", EngineKind::Podman, Health::Working);
        assert_eq!(screen.editor, Editor::Vim);
    }

    #[test]
    fn sounds_play_or_show_their_details_and_the_choice_sits_under_the_editor() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, 60);
        let screen = harness.screen();
        for label in ["Sounds", "Play", "Details only"] {
            assert!(screen.contains(label), "`{label}` is missing:\n{screen}");
        }
        let editor = harness.find("Editor").expect("the editor row is on screen");
        let sounds = harness.find("Sounds").expect("the sound row is on screen");
        assert!(sounds.1 > editor.1, "the sound row comes after the editor's:\n{screen}");
        harness.send(testing::wrap(super::Msg::Sound(Sound::Details))).render();
        assert_eq!(harness.app().asked, [Request::Sound(Sound::Details)]);
        assert_eq!(harness.app().screen.sound, Sound::Details);

        // A settings file that chose the details opens the screen on them.
        let screen = testing::from_config("[apps]\nsound = \"details\"\n", EngineKind::Podman, Health::Working);
        assert_eq!(screen.sound, Sound::Details);
    }

    #[test]
    fn the_built_in_apps_are_named_in_turkish() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, 40);
        harness.send(testing::wrap(super::Msg::Language("tr".to_owned()))).render();
        let screen = harness.screen();
        assert!(screen.contains("YERLEŞİK UYGULAMALAR"), "{screen}");
        assert!(screen.contains("Düzenleyici"), "{screen}");
        assert!(screen.contains("Sesler"), "{screen}");
        assert!(screen.contains("Yalnızca ayrıntılar"), "{screen}");
    }

    #[test]
    fn reduced_motion_leaves_the_screen_working() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        harness.set_reduced_motion(true).render();
        assert!(harness.screen().contains("Container engine"), "{}", harness.screen());
    }

    #[test]
    fn ascii_mode_draws_nothing_but_ascii() {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        harness.set_glyph_mode(GlyphMode::Ascii).render();
        let screen = harness.screen();
        assert!(screen.is_ascii(), "{screen}");
        assert!(screen.contains("Container engine"), "{screen}");
    }

    /// A settings screen showing the background service as `row` says, tall enough to show it.
    fn with_service(row: Option<super::ServiceRow>) -> qframe::runtime::Harness<testing::Host> {
        testing::host(testing::screen(EngineKind::Podman, Health::Working).with_service(row), SIZE.0, 44)
    }

    #[test]
    fn the_service_is_offered_where_it_can_be_installed_and_fits_in_both_languages() {
        for platform in [Platform::Linux, Platform::MacOs] {
            let row = super::ServiceRow::of(platform, Some(false));
            assert_eq!(row, Some(super::ServiceRow::Ready { installed: false, busy: false }));
        }
        let mut harness = with_service(super::ServiceRow::of(Platform::Linux, Some(false)));
        let screen = harness.screen();
        assert!(screen.contains("Background service"), "{screen}");
        assert!(screen.contains("Install"), "{screen}");
        harness.set_locale("tr").render();
        let screen = harness.screen();
        assert!(!screen.contains('…'), "every word fits:\n{screen}");
        assert!(screen.contains("Arka plan servisi"), "{screen}");
        assert!(screen.contains("Kur"), "{screen}");
    }

    #[test]
    fn windows_is_told_that_it_has_no_service_and_that_nothing_is_stopped_there() {
        assert_eq!(super::ServiceRow::of(Platform::Windows, None), Some(super::ServiceRow::Unsupported));
        let mut harness = with_service(Some(super::ServiceRow::Unsupported));
        let screen = harness.screen();
        assert!(screen.contains("Not available on Windows"), "{screen}");
        assert!(screen.contains("containers keep running"), "the choice says it has no effect:\n{screen}");
        assert!(!screen.contains("Install"), "{screen}");
        harness.set_locale("tr").render();
        let screen = harness.screen();
        assert!(!screen.contains('…'), "every word fits:\n{screen}");
        assert!(screen.contains("Windows'ta yok"), "{screen}");
        assert!(screen.contains("açık kalır"), "{screen}");
    }

    #[test]
    fn a_machine_with_nowhere_to_put_the_service_shows_no_row() {
        assert_eq!(super::ServiceRow::of(Platform::Linux, None), None);
        let screen = with_service(None).screen();
        assert!(!screen.contains("Background service"), "{screen}");
    }

    #[test]
    fn installing_asks_once_waits_and_says_what_failed_in_the_commands_words() {
        let row = super::ServiceRow::of(Platform::Linux, Some(false));
        let mut harness = with_service(row);
        assert!(harness.screen().contains("Not installed"), "{}", harness.screen());
        harness.click_text("Install");
        harness.send(testing::wrap(super::Msg::Request(Request::InstallService)));
        assert_eq!(harness.app().asked, [Request::InstallService], "a second press while it works asks for nothing");
        assert_eq!(harness.app().screen.service(), Some(super::ServiceRow::Ready { installed: false, busy: true }));

        let failed = super::Msg::ServiceDone {
            installing: true,
            installed: false,
            result: Err("systemctl --user daemon-reload: Failed to connect to bus".to_owned()),
        };
        harness.send(testing::wrap(failed));
        harness.advance(std::time::Duration::from_millis(300));
        let screen = harness.screen();
        assert!(screen.contains("could not be installed"), "{screen}");
        assert!(
            screen.contains("daemon-reload: Failed to") && screen.contains("connect to bus"),
            "the command's own words:\n{screen}"
        );
        assert!(screen.contains("Install"), "the button is back:\n{screen}");
    }

    #[test]
    fn an_installed_service_offers_to_be_removed() {
        let mut harness = with_service(super::ServiceRow::of(Platform::Linux, Some(true)));
        let screen = harness.screen();
        assert!(screen.contains("Installed;"), "{screen}");
        harness.click_text("Remove");
        assert_eq!(harness.app().asked, [Request::RemoveService]);
        let done = super::Msg::ServiceDone { installing: false, installed: false, result: Ok(()) };
        harness.send(testing::wrap(done)).render();
        assert!(harness.screen().contains("Not installed"), "{}", harness.screen());
    }

    #[test]
    fn what_happens_to_the_containers_on_close_is_a_choice_of_two() {
        let mut harness = with_service(None);
        let screen = harness.screen();
        for words in ["When QCode closes", "Stop", "Keep running"] {
            assert!(screen.contains(words), "`{words}` is missing:\n{screen}");
        }
        harness.click_text("Keep running");
        assert_eq!(harness.app().asked, [Request::OnClose(OnClose::Keep)]);
        harness.set_locale("tr").render();
        let screen = harness.screen();
        for words in ["QCode kapanınca", "Durdurulsun", "Açık kalsın"] {
            assert!(screen.contains(words), "`{words}` is missing:\n{screen}");
        }
    }

    #[test]
    fn how_often_open_projects_are_backed_up_is_a_choice_of_four() {
        let mut harness = with_service(None);
        let screen = harness.screen();
        for words in ["BACKUP", "Back up open projects", "Off", "5 min", "15 min", "1 hour"] {
            assert!(screen.contains(words), "`{words}` is missing:\n{screen}");
        }
        harness.click_text("5 min");
        harness.click_text("Off");
        assert_eq!(
            harness.app().asked,
            [Request::BackupEvery(BackupEvery::Five), Request::BackupEvery(BackupEvery::Off)]
        );
        harness.set_locale("tr").render();
        let screen = harness.screen();
        assert!(!screen.contains('…'), "every word fits:\n{screen}");
        for words in ["YEDEK", "Açık projeleri yedekle", "Kapalı", "5 dk", "15 dk", "1 saat"] {
            assert!(screen.contains(words), "`{words}` is missing:\n{screen}");
        }
    }

    #[test]
    fn a_stored_interval_is_the_one_shown() {
        let screen = testing::from_config("[backup]\nevery = \"1h\"\n", EngineKind::Podman, Health::Working);
        assert_eq!(screen.backup_every, BackupEvery::Hour);
        let screen = testing::from_config("", EngineKind::Podman, Health::Working);
        assert_eq!(screen.backup_every, BackupEvery::Fifteen, "fifteen minutes until the person chooses");
    }

    #[test]
    fn a_stored_choice_to_keep_them_running_is_the_one_shown() {
        let screen = testing::from_config("[containers]\non-close = \"keep\"\n", EngineKind::Podman, Health::Working);
        assert_eq!(screen.on_close, OnClose::Keep);
    }

    #[test]
    fn nothing_is_bracketed_lined_or_framed() {
        let profiles = vec![testing::profile("claude-sub", true)];
        let mut harness =
            testing::host(testing::screen(EngineKind::Podman, Health::Missing(Trouble::NotInstalled)), SIZE.0, SIZE.1);
        harness.send(testing::wrap(super::Msg::Profiles(profiles)));
        for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
            harness.set_glyph_mode(mode).render();
            let screen = harness.screen();
            for forbidden in ['[', ']', '(', ')', '{', '}', '|', '┌', '─', '│'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` in {mode:?}:\n{screen}");
            }
        }
    }
}
