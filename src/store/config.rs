//! `code.conf`: the application's own settings, on top of the framework's
//! [`Settings`](qframe::storage::Settings).
//!
//! The [`Schema`] describes every key of the design document, and self-healing is on: a key
//! nobody knows is removed, a value nothing can read falls back to its default, and each repair
//! is a located diagnostic. A default is never written to the file, so an untouched
//! installation keeps an empty config and a changed default reaches every existing one.
//!
//! The file sits in the Quvyta family's folder, next to the other applications of the family,
//! and QCode's other configuration files would go in the `code/` folder beside it:
//!
//! ```text
//! ~/.config/quvyta/
//!     code.conf       QCode's settings
//!     code/           its other configuration files
//! ```
//!
//! Earlier versions kept `settings.toml` inside `code/`. That file is moved once, at start,
//! before the settings are read; a file that cannot be moved without overwriting something
//! stays where it is and is shown on the settings screen. The session is not a setting: it stays
//! in the data folder and does not move.

use std::fs;
use std::path::{Path, PathBuf};

use qframe::diagnostics::Diagnostic;
use qframe::icons::IconMode;
use qframe::storage::{Family, Schema, SettingKind, Settings, config_dir};

use super::{Loaded, WorkspaceId};
use crate::backup::BackupEvery;
use crate::base::apps::{Editor, Sound};

/// QCode's id in the family: its settings file is `code.conf` and its other files are under
/// `code/`.
pub const APP: &str = "code";

/// The folder under the platform's config directory that held `settings.toml` before. On Linux
/// it is the family's `code/` folder itself, so only the settings file moves.
const LEGACY: &str = "quvyta/code";

/// The settings file's name in [`LEGACY`].
const LEGACY_FILE: &str = "settings.toml";

/// The step of the setup wizard the application stopped at.
///
/// Stored so the wizard can open where the user left it. It is the wizard that decides whether
/// the step is still reachable; this is only how the step is written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    /// Choosing the language.
    Language,
    /// Choosing and finding the container engine.
    Engine,
    /// Choosing where the store lives.
    Location,
}

impl SetupStep {
    /// Every step, in the order the wizard walks them.
    pub const ALL: [Self; 3] = [Self::Language, Self::Engine, Self::Location];

    /// How the step is written in the config file.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Language => "language",
            Self::Engine => "engine",
            Self::Location => "location",
        }
    }

    /// The step a config file's `setup.step` names, if it names one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.key() == key)
    }
}

/// What happens to QCode's containers once no QCode is open any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnClose {
    /// They are stopped, not removed: the next QCode starts the same containers again.
    Stop,
    /// They are left running.
    Keep,
}

impl OnClose {
    /// Both choices, in the order the settings screen offers them.
    pub const ALL: [Self; 2] = [Self::Stop, Self::Keep];

    /// How the choice is written in the config file.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Keep => "keep",
        }
    }

    /// The choice a config file's `containers.on-close` names, if it names one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.key() == key)
    }
}

/// QCode's settings file.
#[derive(Debug, Clone)]
pub struct Config {
    settings: Settings,
}

/// The engines the application can drive. The engine layer gives them meaning; the config only
/// has to know which words it may store.
const ENGINE_KINDS: [&str; 2] = ["podman", "docker"];

impl Config {
    /// The key of the flag that says the wizard finished.
    const COMPLETED: &'static str = "setup.completed";
    /// The key of the step the wizard stopped at.
    const STEP: &'static str = "setup.step";
    /// The key of the chosen engine.
    const ENGINE: &'static str = "engine.kind";
    /// The key of the store directory.
    const FOLDER: &'static str = "folder.path";
    /// The key of the recently opened workspaces.
    const RECENT: &'static str = "workspaces.recent";
    /// What those two keys were called before workspaces had their name. A file written by an
    /// older QCode still spells them this way, and [`adopt_keys`] moves each value over before
    /// the schema is applied — which would otherwise drop a key it does not know, and with it the
    /// place the person chose for their store.
    const LEGACY_FOLDER: &'static str = "workspace.path";
    /// See [`LEGACY_FOLDER`](Self::LEGACY_FOLDER).
    const LEGACY_RECENT: &'static str = "projects.recent";
    /// The key of what happens to the containers once no QCode is open.
    const ON_CLOSE: &'static str = "containers.on-close";
    /// The key of how often an open workspace is backed up.
    const BACKUP_EVERY: &'static str = "backup.every";
    /// The key of the editor a text file opens in.
    const EDITOR: &'static str = "apps.editor";
    /// The key of what opening a sound does.
    const SOUND: &'static str = "apps.sound";

    /// Every language QCode speaks, in the order the setup wizard offers them.
    ///
    /// The list lives with the settings file because this is where a chosen language stops
    /// being a choice on a screen and becomes text a schema has to accept. A screen that offers
    /// a language the file will not hold writes it once and loses it at the next start, so the
    /// screens read their list from here rather than keeping one of their own.
    pub const LANGUAGES: [&'static str; 9] = ["en", "tr", "de", "es", "fr", "pt-BR", "ru", "zh-Hans", "ja"];

    /// How many workspaces the recent list keeps. Beyond a screenful the list stops being a
    /// shortcut, and the full list of workspaces is one screen away.
    pub const RECENT_LIMIT: usize = 10;

    /// Brings the settings over from where earlier versions kept them and reads them from the
    /// family's folder. The diagnostics are the old files that stayed where they were, each
    /// with the reason; nothing was overwritten to move them. Without a home folder the settings
    /// stay in memory and say why.
    #[must_use]
    pub fn load() -> Loaded<Self> {
        match Family::QUVYTA.config_dir() {
            Some(folder) => {
                let legacy = config_dir(LEGACY).unwrap_or_else(|| folder.join(APP));
                Self::load_in(&folder, &legacy)
            }
            None => Loaded { value: Self::check(Settings::load_member(&Family::QUVYTA, APP)), diagnostics: Vec::new() },
        }
    }

    /// [`load`](Self::load) with `folder` as the family's folder and `legacy` as the folder the
    /// old `settings.toml` may be in, so a test never touches the person's own settings.
    #[must_use]
    pub fn load_in(folder: &Path, legacy: &Path) -> Loaded<Self> {
        let diagnostics = adopt(folder, legacy);
        Loaded { value: Self::check(Settings::open(folder.join(format!("{APP}.conf")))), diagnostics }
    }

    /// Reads the settings from TOML `text`, reporting problems against `file`. Saving does
    /// nothing.
    #[must_use]
    pub fn parse_str(file: &str, text: &str) -> Self {
        Self::check(Settings::parse_str(file, text))
    }

    fn check(settings: Settings) -> Self {
        let mut settings = settings;
        let moved = Self::adopt_keys(&mut settings);
        let mut config = Self { settings: settings.schema(Self::schema()).self_heal(true) };
        if moved {
            // A save that cannot be made changes nothing: the keys were moved in memory, so this
            // QCode reads them, and the next start finds the old names again and moves them again.
            let _ = config.settings.save();
        }
        config
    }

    /// Gives the keys an older QCode wrote the names they have today. Answers whether anything
    /// moved, which is what has to be written back.
    ///
    /// A value is moved only into a key that is not there already: settings that carry both names
    /// keep the newer one, and the old one goes, because it is the one nothing reads any more.
    fn adopt_keys(settings: &mut Settings) -> bool {
        let mut moved = false;
        if let Some(path) = settings.get::<String>(Self::LEGACY_FOLDER) {
            moved |= settings.get::<String>(Self::FOLDER).is_none() && settings.set(Self::FOLDER, path);
            moved |= settings.remove(Self::LEGACY_FOLDER);
        }
        if let Some(recent) = settings.get::<Vec<String>>(Self::LEGACY_RECENT) {
            moved |= settings.get::<Vec<String>>(Self::RECENT).is_none() && settings.set(Self::RECENT, recent);
            moved |= settings.remove(Self::LEGACY_RECENT);
        }
        moved
    }

    /// The shape of `code.conf`: the framework's own keys plus QCode's.
    fn schema() -> Schema {
        Schema::builtin()
            // No default: what QCode speaks when nobody has chosen is the machine's own
            // locale, which the framework works out, and a fixed default here would answer
            // that question a second time and differently. Without one, a choice is always
            // written down — even the one that happens to match the default that used to be
            // here — and a word QCode does not speak is dropped rather than quietly turned
            // into English on a machine that reads Turkish.
            .optional(Settings::LANGUAGE, SettingKind::choice(Self::LANGUAGES))
            .flag(Self::COMPLETED, false)
            .choice(Self::STEP, SetupStep::ALL.map(SetupStep::key), SetupStep::Language.key())
            .choice(Self::ENGINE, ENGINE_KINDS, ENGINE_KINDS[0])
            // Both of these are written only once there is something to write, so neither has a
            // default: an absent store means "the wizard has not run", not "the default one".
            .optional(Self::FOLDER, SettingKind::check(|path: &String| Path::new(path).is_absolute()))
            .optional(Self::RECENT, SettingKind::check(is_id_list))
            // Stopping is the default because a container left running holds memory the person
            // never sees again; keeping them running is the choice of someone who wants that.
            .choice(Self::ON_CLOSE, OnClose::ALL.map(OnClose::key), OnClose::Stop.key())
            .choice(Self::BACKUP_EVERY, BackupEvery::ALL.map(BackupEvery::key), BackupEvery::default().key())
            .choice(Self::EDITOR, Editor::ALL.map(Editor::key), Editor::default().key())
            .choice(Self::SOUND, Sound::ALL.map(Sound::key), Sound::default().key())
    }

    /// Every problem found while reading the file, including every repair that was made.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        self.settings.diagnostics()
    }

    /// Whether the file was read without a single problem.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.settings.diagnostics().is_empty()
    }

    /// The settings as TOML text, which is what saving writes.
    #[must_use]
    pub fn to_toml(&self) -> String {
        self.settings.to_toml()
    }

    /// Writes the settings to their file atomically.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory or the file cannot be written.
    pub fn save(&mut self) -> std::io::Result<()> {
        self.settings.save()
    }

    /// The framework settings underneath, for the screens that read the theme, the language or
    /// the icon mode and for applying them.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Records the language the application speaks. Answers whether anything changed.
    ///
    /// The four settings the framework keeps for every Quvyta application are already in force
    /// by the time they reach here: the settings screen applies a choice to the running
    /// application and asks for it to be stored, and storing is this file's part of that.
    pub fn set_language(&mut self, code: &str) -> bool {
        self.settings.set(Settings::LANGUAGE, code.to_owned())
    }

    /// Records the theme the application wears. Answers whether anything changed.
    pub fn set_theme(&mut self, id: &str) -> bool {
        self.settings.set(Settings::THEME, id.to_owned())
    }

    /// Records which glyphs the application draws with. Answers whether anything changed.
    pub fn set_icons(&mut self, mode: IconMode) -> bool {
        self.settings.set(Settings::ICONS, mode.name().to_owned())
    }

    /// Records whether the application moves as little as it can. Answers whether anything
    /// changed.
    pub fn set_reduced_motion(&mut self, reduced: bool) -> bool {
        self.settings.set(Settings::REDUCED_MOTION, reduced)
    }

    /// Whether the setup wizard has finished. Until it has, the wizard runs.
    #[must_use]
    pub fn setup_completed(&self) -> bool {
        self.settings.get_or(Self::COMPLETED, false)
    }

    /// Records whether the setup wizard has finished. Answers whether anything changed.
    pub fn set_setup_completed(&mut self, completed: bool) -> bool {
        self.settings.set(Self::COMPLETED, completed)
    }

    /// The step the wizard stopped at, the first one until it says otherwise.
    #[must_use]
    pub fn setup_step(&self) -> SetupStep {
        self.settings.get::<String>(Self::STEP).and_then(|key| SetupStep::from_key(&key)).unwrap_or(SetupStep::Language)
    }

    /// Records the step the wizard is on. Answers whether anything changed.
    pub fn set_setup_step(&mut self, step: SetupStep) -> bool {
        self.settings.set(Self::STEP, step.key().to_owned())
    }

    /// The chosen container engine, as the engine layer names it, or `None` while the user has
    /// not chosen one.
    #[must_use]
    pub fn engine_kind(&self) -> Option<&str> {
        match self.settings.value(Self::ENGINE) {
            Some(qframe::storage::SettingValue::Text(kind)) => Some(kind),
            _ => None,
        }
    }

    /// Records the chosen engine. A word no engine answers to is refused rather than stored.
    pub fn set_engine_kind(&mut self, kind: &str) -> bool {
        ENGINE_KINDS.contains(&kind) && self.settings.set(Self::ENGINE, kind.to_owned())
    }

    /// Where the store lives, once the wizard has placed it.
    #[must_use]
    pub fn folder_path(&self) -> Option<PathBuf> {
        self.settings.get::<String>(Self::FOLDER).map(PathBuf::from)
    }

    /// Records where the store lives. A relative path is refused: it would mean something
    /// different in every working directory the application is started from.
    pub fn set_folder_path(&mut self, path: &Path) -> bool {
        path.is_absolute() && self.settings.set(Self::FOLDER, path.display().to_string())
    }

    /// What happens to QCode's containers once no QCode is open, stopping them until the person
    /// says otherwise.
    #[must_use]
    pub fn on_close(&self) -> OnClose {
        self.settings.get::<String>(Self::ON_CLOSE).and_then(|key| OnClose::from_key(&key)).unwrap_or(OnClose::Stop)
    }

    /// Records what happens to the containers once no QCode is open. Answers whether anything
    /// changed.
    ///
    /// Choosing the default takes the key out of the file rather than writing the default in,
    /// so a later change of the default reaches this installation too.
    pub fn set_on_close(&mut self, choice: OnClose) -> bool {
        match choice {
            OnClose::Stop => self.settings.remove(Self::ON_CLOSE),
            OnClose::Keep => self.settings.set(Self::ON_CLOSE, choice.key().to_owned()),
        }
    }

    /// How often an open workspace is backed up, every fifteen minutes until the person says
    /// otherwise.
    #[must_use]
    pub fn backup_every(&self) -> BackupEvery {
        self.settings.get::<String>(Self::BACKUP_EVERY).and_then(|key| BackupEvery::from_key(&key)).unwrap_or_default()
    }

    /// Records how often an open workspace is backed up. Answers whether anything changed.
    ///
    /// Like [`set_on_close`](Self::set_on_close), choosing the default takes the key out of the
    /// file.
    pub fn set_backup_every(&mut self, choice: BackupEvery) -> bool {
        if choice == BackupEvery::default() {
            self.settings.remove(Self::BACKUP_EVERY)
        } else {
            self.settings.set(Self::BACKUP_EVERY, choice.key().to_owned())
        }
    }

    /// The workspaces that were opened last, newest first.
    #[must_use]
    pub fn recent_workspaces(&self) -> Vec<WorkspaceId> {
        self.settings
            .get::<Vec<String>>(Self::RECENT)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| WorkspaceId::parse(id).ok())
            .collect()
    }

    /// Moves a workspace to the front of the recent list, dropping the oldest beyond
    /// [`RECENT_LIMIT`](Self::RECENT_LIMIT).
    pub fn remember_workspace(&mut self, id: &WorkspaceId) {
        let mut recent = self.recent_ids();
        recent.retain(|stored| stored != id.as_str());
        recent.insert(0, id.as_str().to_owned());
        recent.truncate(Self::RECENT_LIMIT);
        self.settings.set(Self::RECENT, recent);
    }

    /// Takes a workspace out of the recent list, for one that is gone or was deleted.
    pub fn forget_workspace(&mut self, id: &WorkspaceId) {
        let mut recent = self.recent_ids();
        recent.retain(|stored| stored != id.as_str());
        if recent.is_empty() {
            self.settings.remove(Self::RECENT);
        } else {
            self.settings.set(Self::RECENT, recent);
        }
    }

    /// The editor a text file opens in; nano until the person chooses otherwise.
    #[must_use]
    pub fn editor(&self) -> Editor {
        self.settings.get::<String>(Self::EDITOR).and_then(|key| Editor::from_key(&key)).unwrap_or_default()
    }

    /// Records the editor a text file opens in. Answers whether anything changed.
    ///
    /// Choosing nano again takes the key out rather than writing the default down, so the file
    /// of a person who went back to the default is the file of one who never left it.
    pub fn set_editor(&mut self, editor: Editor) -> bool {
        if editor != Editor::default() {
            return self.settings.set(Self::EDITOR, editor.key().to_owned());
        }
        self.settings.remove(Self::EDITOR)
    }

    /// What opening a sound does; it plays until the person chooses otherwise.
    #[must_use]
    pub fn sound(&self) -> Sound {
        self.settings.get::<String>(Self::SOUND).and_then(|key| Sound::from_key(&key)).unwrap_or_default()
    }

    /// Records what opening a sound does. Answers whether anything changed.
    ///
    /// Playing is the default and is taken out rather than written down, as with the editor.
    pub fn set_sound(&mut self, sound: Sound) -> bool {
        if sound != Sound::default() {
            return self.settings.set(Self::SOUND, sound.key().to_owned());
        }
        self.settings.remove(Self::SOUND)
    }

    fn recent_ids(&self) -> Vec<String> {
        self.settings.get::<Vec<String>>(Self::RECENT).unwrap_or_default()
    }
}

/// Moves the old settings into the family's layout and answers what stayed behind.
///
/// Only an old settings file is a reason to look. Where the old folder and `code/` are one
/// folder under two spellings (macOS, whose file system ignores case, names the family's folder
/// `Quvyta` and the old one `quvyta`) the framework could take them for two, and would then warn
/// at every start about files that are already in place; once the settings file has moved
/// there is nothing more to bring over.
fn adopt(folder: &Path, legacy: &Path) -> Vec<Diagnostic> {
    if fs::symlink_metadata(legacy.join(LEGACY_FILE)).is_err() {
        return Vec::new();
    }
    Family::QUVYTA.adopt_in(folder, APP, legacy).diagnostics().to_vec()
}

/// Whether every entry of a stored recent list is a workspace identifier.
///
/// The parameter is a `&Vec` rather than a slice because that is the shape
/// [`SettingKind::check`] hands a `Vec<String>` setting to its test.
#[allow(clippy::ptr_arg)]
fn is_id_list(ids: &Vec<String>) -> bool {
    ids.iter().all(|id| WorkspaceId::parse(id).is_ok())
}

#[cfg(test)]
mod tests {

    use super::*;

    const FILE: &str = "code.conf";

    #[test]
    fn a_fresh_config_writes_nothing_because_defaults_are_not_stored() {
        let config = Config::parse_str(FILE, "");
        assert!(config.is_clean(), "{:?}", config.diagnostics());
        assert!(!config.setup_completed());
        assert_eq!(config.setup_step(), SetupStep::Language);
        assert_eq!(config.engine_kind(), None);
        assert_eq!(config.folder_path(), None);
        assert!(config.recent_workspaces().is_empty());
        assert_eq!(config.to_toml(), "");
    }

    #[test]
    fn a_chosen_language_is_written_down_whichever_one_it_is() {
        // The language has no default in the schema, because what QCode speaks without one is
        // the machine's own locale. So every choice is a choice, including the one that used to
        // be the default: a person on a Turkish machine who says English must be heard.
        for code in ["en", "tr"] {
            let mut config = Config::parse_str(FILE, "");
            assert!(config.set_language(code));
            let stored = Config::parse_str(FILE, &config.to_toml());
            assert!(stored.is_clean(), "{:?}", stored.diagnostics());
            assert_eq!(stored.settings().language().as_deref(), Some(code));
        }
    }

    #[test]
    fn every_language_qcode_speaks_survives_being_written_down() {
        // The wizard and the settings screen offer every language QCode carries a file for. One
        // the settings file will not hold is chosen, written, and gone the next time QCode
        // opens: the person picks Japanese and comes back to a machine speaking English.
        for code in Config::LANGUAGES {
            let mut config = Config::parse_str(FILE, "");
            assert!(config.set_language(code), "{code} is written down");
            let stored = Config::parse_str(FILE, &config.to_toml());
            assert!(stored.is_clean(), "{code}: {:?}", stored.diagnostics());
            assert_eq!(stored.settings().language().as_deref(), Some(code));
        }
    }

    #[test]
    fn a_language_qcode_does_not_speak_is_dropped_rather_than_swapped_for_another() {
        // Turning `it` into `en` would put an application the person never asked for in front
        // of them. Dropping it hands the question back to the machine's own locale.
        let config = Config::parse_str(FILE, "language = \"it\"\n");
        assert_eq!(config.settings().language(), None);
        assert!(!config.is_clean(), "the repair is reported");
        assert!(!config.to_toml().contains("language"), "{}", config.to_toml());
    }

    #[test]
    fn the_keys_of_the_design_round_trip() {
        let text = "language = \"tr\"\n\n[setup]\ncompleted = true\nstep = \"location\"\n\n\
                    [engine]\nkind = \"docker\"\n\n[folder]\npath = \"/home/ada/Belgeler/QCode\"\n\n\
                    [workspaces]\nrecent = [\"proje\", \"oteki-proje\"]\n";
        let config = Config::parse_str(FILE, text);
        assert!(config.is_clean(), "{:?}", config.diagnostics());
        assert!(config.setup_completed());
        assert_eq!(config.setup_step(), SetupStep::Location);
        assert_eq!(config.engine_kind(), Some("docker"));
        assert_eq!(config.folder_path(), Some(PathBuf::from("/home/ada/Belgeler/QCode")));
        let recent: Vec<String> = config.recent_workspaces().iter().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(recent, ["proje", "oteki-proje"]);
    }

    #[test]
    fn an_unknown_key_is_removed_and_reported_with_its_place() {
        let config = Config::parse_str(FILE, "[engine]\nkind = \"podman\"\nspeed = \"fast\"\n");
        assert_eq!(config.engine_kind(), Some("podman"));
        assert_eq!(config.to_toml(), "[engine]\nkind = \"podman\"\n");
        let located = config.diagnostics()[0].location.as_ref().map(ToString::to_string);
        assert_eq!(located, Some("code.conf:3:1".to_owned()));
    }

    #[test]
    fn an_engine_that_qcode_cannot_drive_falls_back_to_the_default() {
        let config = Config::parse_str(FILE, "[engine]\nkind = \"lxc\"\n");
        assert_eq!(config.engine_kind(), Some("podman"));
        assert_eq!(config.diagnostics().len(), 1);
    }

    #[test]
    fn a_folder_path_that_is_not_absolute_is_dropped() {
        // A relative path would mean something different in every working directory.
        let config = Config::parse_str(FILE, "[folder]\npath = \"Belgeler/QCode\"\n");
        assert_eq!(config.folder_path(), None);
        assert_eq!(config.diagnostics().len(), 1);
        assert_eq!(config.to_toml(), "");
    }

    #[test]
    fn a_recent_list_that_is_not_a_list_of_ids_is_dropped() {
        let config = Config::parse_str(FILE, "[workspaces]\nrecent = [\"Proje\"]\n");
        assert!(config.recent_workspaces().is_empty());
        assert_eq!(config.diagnostics().len(), 1);
    }

    #[test]
    fn an_unknown_setup_step_falls_back_to_the_first_one() {
        let config = Config::parse_str(FILE, "[setup]\nstep = \"login\"\n");
        assert_eq!(config.setup_step(), SetupStep::Language);
        assert_eq!(config.diagnostics().len(), 1);
    }

    #[test]
    fn writing_the_setup_state_stores_only_what_differs_from_the_default() {
        let mut config = Config::parse_str(FILE, "");
        config.set_setup_step(SetupStep::Engine);
        config.set_engine_kind("docker");
        config.set_folder_path(Path::new("/home/ada/Belgeler/QCode"));
        config.set_setup_completed(true);
        assert_eq!(
            config.to_toml(),
            "[setup]\nstep = \"engine\"\ncompleted = true\n\n[engine]\nkind = \"docker\"\n\n\
             [folder]\npath = \"/home/ada/Belgeler/QCode\"\n"
        );
    }

    #[test]
    fn a_used_workspace_moves_to_the_front_of_the_recent_list_without_repeating() {
        let mut config = Config::parse_str(FILE, "");
        for name in ["bir", "iki", "uc"] {
            config.remember_workspace(&WorkspaceId::from_display_name(name).expect("usable"));
        }
        config.remember_workspace(&WorkspaceId::from_display_name("bir").expect("usable"));
        let recent: Vec<String> = config.recent_workspaces().iter().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(recent, ["bir", "uc", "iki"]);

        config.forget_workspace(&WorkspaceId::from_display_name("uc").expect("usable"));
        let recent: Vec<String> = config.recent_workspaces().iter().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(recent, ["bir", "iki"]);
    }

    #[test]
    fn the_recent_list_stops_growing() {
        let mut config = Config::parse_str(FILE, "");
        for number in 0..Config::RECENT_LIMIT + 5 {
            config.remember_workspace(&WorkspaceId::from_display_name(&format!("proje{number}")).expect("usable"));
        }
        assert_eq!(config.recent_workspaces().len(), Config::RECENT_LIMIT);
        assert_eq!(config.recent_workspaces()[0].as_str(), format!("proje{}", Config::RECENT_LIMIT + 4));
    }

    /// A fresh folder under the system's temporary folder; never the person's own config folder.
    fn temp(name: &str) -> PathBuf {
        let dir = crate::testing::scratch(&format!("config-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("folder");
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("folder");
        fs::write(path, text).expect("file");
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("readable")
    }

    const SETTLED: &str = "[setup]\ncompleted = true\n\n[engine]\nkind = \"docker\"\n\n\
                           [folder]\npath = \"/home/ada/Belgeler/QCode\"\n";

    #[test]
    fn the_old_settings_file_becomes_code_conf_with_every_value() {
        let folder = temp("same-folder");
        let legacy = folder.join("code");
        write(&legacy.join("settings.toml"), SETTLED);
        write(&legacy.join("settings.toml.bak"), "old backup\n");

        let Loaded { value: config, diagnostics } = Config::load_in(&folder, &legacy);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(read(&folder.join("code.conf")), SETTLED, "moved as it was");
        assert!(!legacy.join("settings.toml").exists());
        assert_eq!(read(&legacy.join("settings.toml.bak")), "old backup\n", "the other files stay in code/");
        assert!(config.is_clean(), "{:?}", config.diagnostics());
        assert!(config.setup_completed());
        assert_eq!(config.engine_kind(), Some("docker"));
        assert_eq!(config.folder_path(), Some(PathBuf::from("/home/ada/Belgeler/QCode")), "a chosen place stays");
        assert_eq!(config.settings().path(), Some(folder.join("code.conf").as_path()));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_second_start_moves_nothing_and_reads_the_same() {
        let folder = temp("again");
        let legacy = folder.join("code");
        write(&legacy.join("settings.toml"), SETTLED);
        let _ = Config::load_in(&folder, &legacy);

        let Loaded { value: config, diagnostics } = Config::load_in(&folder, &legacy);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(read(&folder.join("code.conf")), SETTLED);
        assert_eq!(config.engine_kind(), Some("docker"));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn an_existing_code_conf_wins_and_the_old_file_stays_whole_and_is_reported() {
        let folder = temp("both");
        let legacy = folder.join("code");
        write(&legacy.join("settings.toml"), SETTLED);
        write(&folder.join("code.conf"), "[engine]\nkind = \"podman\"\n");

        let Loaded { value: config, diagnostics } = Config::load_in(&folder, &legacy);

        assert_eq!(read(&legacy.join("settings.toml")), SETTLED, "the old file is never touched");
        assert_eq!(read(&folder.join("code.conf")), "[engine]\nkind = \"podman\"\n");
        assert_eq!(config.engine_kind(), Some("podman"));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("settings.toml"), "{}", diagnostics[0]);
        let _ = fs::remove_dir_all(&folder);
    }

    #[cfg(unix)]
    #[test]
    fn an_old_settings_file_that_is_a_link_is_left_alone_and_reported() {
        let folder = temp("link");
        let legacy = folder.join("code");
        write(&folder.join("elsewhere.toml"), SETTLED);
        fs::create_dir_all(&legacy).expect("folder");
        std::os::unix::fs::symlink(folder.join("elsewhere.toml"), legacy.join("settings.toml")).expect("link");

        let Loaded { value: config, diagnostics } = Config::load_in(&folder, &legacy);

        assert!(fs::symlink_metadata(legacy.join("settings.toml")).expect("still there").file_type().is_symlink());
        assert!(!folder.join("code.conf").exists(), "nothing is made up in its place");
        assert_eq!(config.engine_kind(), None);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_fresh_start_writes_nothing_until_a_setting_changes() {
        let folder = temp("fresh");
        let legacy = folder.join("code");

        let Loaded { value: mut config, diagnostics } = Config::load_in(&folder, &legacy);

        assert!(diagnostics.is_empty());
        assert!(config.is_clean(), "{:?}", config.diagnostics());
        assert!(!folder.join("code.conf").exists());
        assert!(!legacy.exists());
        config.set_engine_kind("docker");
        config.save().expect("saved");
        assert_eq!(read(&folder.join("code.conf")), "[engine]\nkind = \"docker\"\n");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_damaged_old_file_is_moved_as_it_was_then_healed_with_a_backup() {
        let folder = temp("damaged");
        let legacy = folder.join("code");
        let text = "[engine]\nkind = \"lxc\"\n";
        write(&legacy.join("settings.toml"), text);

        let Loaded { value: config, diagnostics } = Config::load_in(&folder, &legacy);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(read(&folder.join("code.conf.bak")), text, "what the person wrote is kept");
        assert!(!config.is_clean());
        assert_eq!(config.engine_kind(), Some("podman"));
        let _ = fs::remove_dir_all(&folder);
    }

    /// Run with a config folder of its own, so the move happens where the platform puts the
    /// family's folder and never in the developer's.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn loading_moves_the_old_file_from_where_earlier_versions_kept_it() {
        const PRINT: &str = "QCODE_TEST_PRINT_CONFIG";
        if std::env::var_os(PRINT).is_some() {
            let Loaded { value: config, diagnostics } = Config::load();
            let path = config.settings().path().map(|path| path.display().to_string()).unwrap_or_default();
            println!("file={path}\nengine={:?}\nleft={}", config.engine_kind(), diagnostics.len());
            return;
        }
        let home = temp("xdg");
        let xdg = home.join("config");
        write(&xdg.join("quvyta").join("code").join("settings.toml"), SETTLED);

        let printed = crate::testing::in_child(
            "store::config::tests::loading_moves_the_old_file_from_where_earlier_versions_kept_it",
            &[(PRINT, "1".as_ref()), ("HOME", home.as_os_str()), ("XDG_CONFIG_HOME", xdg.as_os_str())],
        );

        let file = xdg.join("quvyta").join("code.conf");
        assert!(printed.contains(&format!("file={}\n", file.display())), "{printed}");
        assert!(printed.contains("engine=Some(\"docker\")\nleft=0\n"), "{printed}");
        assert_eq!(read(&file), SETTLED);
        assert!(!xdg.join("quvyta").join("code").join("settings.toml").exists());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn containers_are_stopped_on_close_until_the_person_says_otherwise() {
        let mut config = Config::parse_str(FILE, "");
        assert_eq!(config.on_close(), OnClose::Stop);
        config.set_on_close(OnClose::Stop);
        assert_eq!(config.to_toml(), "", "the default is not written down");
        assert!(config.set_on_close(OnClose::Keep));
        assert_eq!(config.to_toml(), "[containers]\non-close = \"keep\"\n");
        let stored = Config::parse_str(FILE, &config.to_toml());
        assert!(stored.is_clean(), "{:?}", stored.diagnostics());
        assert_eq!(stored.on_close(), OnClose::Keep);
        config.set_on_close(OnClose::Stop);
        assert_eq!(config.to_toml(), "", "going back to the default takes the key out again");
    }

    #[test]
    fn a_workspace_is_backed_up_every_fifteen_minutes_until_the_person_says_otherwise() {
        let mut config = Config::parse_str(FILE, "");
        assert_eq!(config.backup_every(), BackupEvery::Fifteen);
        config.set_backup_every(BackupEvery::Fifteen);
        assert_eq!(config.to_toml(), "", "the default is not written down");
        assert!(config.set_backup_every(BackupEvery::Five));
        assert_eq!(config.to_toml(), "[backup]\nevery = \"5m\"\n");
        let stored = Config::parse_str(FILE, &config.to_toml());
        assert!(stored.is_clean(), "{:?}", stored.diagnostics());
        assert_eq!(stored.backup_every(), BackupEvery::Five);
        assert!(config.set_backup_every(BackupEvery::Off));
        assert_eq!(Config::parse_str(FILE, &config.to_toml()).backup_every(), BackupEvery::Off);
        config.set_backup_every(BackupEvery::Fifteen);
        assert_eq!(config.to_toml(), "", "going back to the default takes the key out again");
        let unknown = Config::parse_str(FILE, "[backup]\nevery = \"1m\"\n");
        assert_eq!(unknown.backup_every(), BackupEvery::Fifteen);
        assert_eq!(unknown.diagnostics().len(), 1);
    }

    #[test]
    fn an_on_close_choice_nobody_knows_falls_back_to_stopping() {
        let config = Config::parse_str(FILE, "[containers]\non-close = \"pause\"\n");
        assert_eq!(config.on_close(), OnClose::Stop);
        assert_eq!(config.diagnostics().len(), 1);
    }

    #[test]
    fn the_editor_is_nano_until_another_is_chosen_and_nano_is_never_written() {
        let mut config = Config::parse_str(FILE, "");
        assert_eq!(config.editor(), Editor::Nano);
        assert!(config.set_editor(Editor::Vim));
        assert_eq!(config.to_toml(), "[apps]\neditor = \"vim\"\n");
        let stored = Config::parse_str(FILE, &config.to_toml());
        assert!(stored.is_clean(), "{:?}", stored.diagnostics());
        assert_eq!(stored.editor(), Editor::Vim);

        let mut config = stored;
        config.set_editor(Editor::Nano);
        assert_eq!(config.editor(), Editor::Nano);
        assert_eq!(config.to_toml(), "", "the default is not written down");
    }

    #[test]
    fn an_editor_the_image_does_not_carry_falls_back_to_nano() {
        let config = Config::parse_str(FILE, "[apps]\neditor = \"emacs\"\n");
        assert_eq!(config.editor(), Editor::Nano);
        assert_eq!(config.diagnostics().len(), 1);
    }

    #[test]
    fn sounds_play_until_details_are_chosen_and_playing_is_never_written() {
        let mut config = Config::parse_str(FILE, "");
        assert_eq!(config.sound(), Sound::Play);
        assert!(config.set_sound(Sound::Details));
        assert_eq!(config.to_toml(), "[apps]\nsound = \"details\"\n");
        let stored = Config::parse_str(FILE, &config.to_toml());
        assert!(stored.is_clean(), "{:?}", stored.diagnostics());
        assert_eq!(stored.sound(), Sound::Details);

        let mut config = stored;
        config.set_sound(Sound::Play);
        assert_eq!(config.sound(), Sound::Play);
        assert_eq!(config.to_toml(), "", "the default is not written down");
        let unknown = Config::parse_str(FILE, "[apps]\nsound = \"loud\"\n");
        assert_eq!(unknown.sound(), Sound::Play);
        assert_eq!(unknown.diagnostics().len(), 1);
    }

    #[test]
    fn the_setup_steps_are_the_three_of_the_design_in_order() {
        assert_eq!(SetupStep::ALL, [SetupStep::Language, SetupStep::Engine, SetupStep::Location]);
        for step in SetupStep::ALL {
            assert_eq!(SetupStep::from_key(step.key()), Some(step));
        }
        assert_eq!(SetupStep::from_key("login"), None);
    }
}
