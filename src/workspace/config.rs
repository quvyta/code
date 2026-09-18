//! `code.toml`: the application's own settings, on top of the framework's
//! [`Settings`](qframe::storage::Settings).
//!
//! The [`Schema`] describes every key of the design document, and self-healing is on: a key
//! nobody knows is removed, a value nothing can read falls back to its default, and each repair
//! is a located diagnostic. A default is never written to the file, so an untouched
//! installation keeps an empty config and a changed default reaches every existing one.

use std::path::{Path, PathBuf};

use qframe::diagnostics::Diagnostic;
use qframe::icons::IconMode;
use qframe::storage::{Schema, SettingKind, Settings};

use super::ProjectId;

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
    /// Choosing where the workspace lives.
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
    /// The key of the workspace directory.
    const WORKSPACE: &'static str = "workspace.path";
    /// The key of the recently opened projects.
    const RECENT: &'static str = "projects.recent";

    /// How many projects the recent list keeps. Beyond a screenful the list stops being a
    /// shortcut, and the full list of projects is one screen away.
    pub const RECENT_LIMIT: usize = 10;

    /// Loads the settings of QCode from the platform config directory.
    #[must_use]
    pub fn load() -> Self {
        Self::check(Settings::load("quvyta/code"))
    }

    /// Reads the settings from TOML `text`, reporting problems against `file`. Saving does
    /// nothing.
    #[must_use]
    pub fn parse_str(file: &str, text: &str) -> Self {
        Self::check(Settings::parse_str(file, text))
    }

    fn check(settings: Settings) -> Self {
        Self { settings: settings.schema(Self::schema()).self_heal(true) }
    }

    /// The shape of `code.toml`: the framework's own keys plus QCode's.
    fn schema() -> Schema {
        Schema::builtin()
            // No default: what QCode speaks when nobody has chosen is the machine's own
            // locale, which the framework works out, and a fixed default here would answer
            // that question a second time and differently. Without one, a choice is always
            // written down — even the one that happens to match the default that used to be
            // here — and a word QCode does not speak is dropped rather than quietly turned
            // into English on a machine that reads Turkish.
            .optional(Settings::LANGUAGE, SettingKind::choice(["en", "tr"]))
            .flag(Self::COMPLETED, false)
            .choice(Self::STEP, SetupStep::ALL.map(SetupStep::key), SetupStep::Language.key())
            .choice(Self::ENGINE, ENGINE_KINDS, ENGINE_KINDS[0])
            // Both of these are written only once there is something to write, so neither has a
            // default: an absent workspace means "the wizard has not run", not "the default one".
            .optional(Self::WORKSPACE, SettingKind::check(|path: &String| Path::new(path).is_absolute()))
            .optional(Self::RECENT, SettingKind::check(is_id_list))
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

    /// Where the workspace lives, once the wizard has placed it.
    #[must_use]
    pub fn workspace_path(&self) -> Option<PathBuf> {
        self.settings.get::<String>(Self::WORKSPACE).map(PathBuf::from)
    }

    /// Records where the workspace lives. A relative path is refused: it would mean something
    /// different in every working directory the application is started from.
    pub fn set_workspace_path(&mut self, path: &Path) -> bool {
        path.is_absolute() && self.settings.set(Self::WORKSPACE, path.display().to_string())
    }

    /// The projects that were opened last, newest first.
    #[must_use]
    pub fn recent_projects(&self) -> Vec<ProjectId> {
        self.settings
            .get::<Vec<String>>(Self::RECENT)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| ProjectId::parse(id).ok())
            .collect()
    }

    /// Moves a project to the front of the recent list, dropping the oldest beyond
    /// [`RECENT_LIMIT`](Self::RECENT_LIMIT).
    pub fn remember_project(&mut self, id: &ProjectId) {
        let mut recent = self.recent_ids();
        recent.retain(|stored| stored != id.as_str());
        recent.insert(0, id.as_str().to_owned());
        recent.truncate(Self::RECENT_LIMIT);
        self.settings.set(Self::RECENT, recent);
    }

    /// Takes a project out of the recent list, for one that is gone or was deleted.
    pub fn forget_project(&mut self, id: &ProjectId) {
        let mut recent = self.recent_ids();
        recent.retain(|stored| stored != id.as_str());
        if recent.is_empty() {
            self.settings.remove(Self::RECENT);
        } else {
            self.settings.set(Self::RECENT, recent);
        }
    }

    fn recent_ids(&self) -> Vec<String> {
        self.settings.get::<Vec<String>>(Self::RECENT).unwrap_or_default()
    }
}

/// Whether every entry of a stored recent list is a project identifier.
///
/// The parameter is a `&Vec` rather than a slice because that is the shape
/// [`SettingKind::check`] hands a `Vec<String>` setting to its test.
#[allow(clippy::ptr_arg)]
fn is_id_list(ids: &Vec<String>) -> bool {
    ids.iter().all(|id| ProjectId::parse(id).is_ok())
}

#[cfg(test)]
mod tests {

    use super::*;

    const FILE: &str = "code.toml";

    #[test]
    fn a_fresh_config_writes_nothing_because_defaults_are_not_stored() {
        let config = Config::parse_str(FILE, "");
        assert!(config.is_clean(), "{:?}", config.diagnostics());
        assert!(!config.setup_completed());
        assert_eq!(config.setup_step(), SetupStep::Language);
        assert_eq!(config.engine_kind(), None);
        assert_eq!(config.workspace_path(), None);
        assert!(config.recent_projects().is_empty());
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
    fn a_language_qcode_does_not_speak_is_dropped_rather_than_swapped_for_another() {
        // Turning `fr` into `en` would put an application the person never asked for in front
        // of them. Dropping it hands the question back to the machine's own locale.
        let config = Config::parse_str(FILE, "language = \"fr\"\n");
        assert_eq!(config.settings().language(), None);
        assert!(!config.is_clean(), "the repair is reported");
        assert!(!config.to_toml().contains("language"), "{}", config.to_toml());
    }

    #[test]
    fn the_keys_of_the_design_round_trip() {
        let text = "language = \"tr\"\n\n[setup]\ncompleted = true\nstep = \"location\"\n\n\
                    [engine]\nkind = \"docker\"\n\n[workspace]\npath = \"/home/ada/Belgeler/QCode\"\n\n\
                    [projects]\nrecent = [\"proje\", \"oteki-proje\"]\n";
        let config = Config::parse_str(FILE, text);
        assert!(config.is_clean(), "{:?}", config.diagnostics());
        assert!(config.setup_completed());
        assert_eq!(config.setup_step(), SetupStep::Location);
        assert_eq!(config.engine_kind(), Some("docker"));
        assert_eq!(config.workspace_path(), Some(PathBuf::from("/home/ada/Belgeler/QCode")));
        let recent: Vec<String> = config.recent_projects().iter().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(recent, ["proje", "oteki-proje"]);
    }

    #[test]
    fn an_unknown_key_is_removed_and_reported_with_its_place() {
        let config = Config::parse_str(FILE, "[engine]\nkind = \"podman\"\nspeed = \"fast\"\n");
        assert_eq!(config.engine_kind(), Some("podman"));
        assert_eq!(config.to_toml(), "[engine]\nkind = \"podman\"\n");
        let located = config.diagnostics()[0].location.as_ref().map(ToString::to_string);
        assert_eq!(located, Some("code.toml:3:1".to_owned()));
    }

    #[test]
    fn an_engine_that_qcode_cannot_drive_falls_back_to_the_default() {
        let config = Config::parse_str(FILE, "[engine]\nkind = \"lxc\"\n");
        assert_eq!(config.engine_kind(), Some("podman"));
        assert_eq!(config.diagnostics().len(), 1);
    }

    #[test]
    fn a_workspace_path_that_is_not_absolute_is_dropped() {
        // A relative path would mean something different in every working directory.
        let config = Config::parse_str(FILE, "[workspace]\npath = \"Belgeler/QCode\"\n");
        assert_eq!(config.workspace_path(), None);
        assert_eq!(config.diagnostics().len(), 1);
        assert_eq!(config.to_toml(), "");
    }

    #[test]
    fn a_recent_list_that_is_not_a_list_of_ids_is_dropped() {
        let config = Config::parse_str(FILE, "[projects]\nrecent = [\"Proje\"]\n");
        assert!(config.recent_projects().is_empty());
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
        config.set_workspace_path(Path::new("/home/ada/Belgeler/QCode"));
        config.set_setup_completed(true);
        assert_eq!(
            config.to_toml(),
            "[setup]\nstep = \"engine\"\ncompleted = true\n\n[engine]\nkind = \"docker\"\n\n\
             [workspace]\npath = \"/home/ada/Belgeler/QCode\"\n"
        );
    }

    #[test]
    fn a_used_project_moves_to_the_front_of_the_recent_list_without_repeating() {
        let mut config = Config::parse_str(FILE, "");
        for name in ["bir", "iki", "uc"] {
            config.remember_project(&ProjectId::from_display_name(name).expect("usable"));
        }
        config.remember_project(&ProjectId::from_display_name("bir").expect("usable"));
        let recent: Vec<String> = config.recent_projects().iter().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(recent, ["bir", "uc", "iki"]);

        config.forget_project(&ProjectId::from_display_name("uc").expect("usable"));
        let recent: Vec<String> = config.recent_projects().iter().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(recent, ["bir", "iki"]);
    }

    #[test]
    fn the_recent_list_stops_growing() {
        let mut config = Config::parse_str(FILE, "");
        for number in 0..Config::RECENT_LIMIT + 5 {
            config.remember_project(&ProjectId::from_display_name(&format!("proje{number}")).expect("usable"));
        }
        assert_eq!(config.recent_projects().len(), Config::RECENT_LIMIT);
        assert_eq!(config.recent_projects()[0].as_str(), format!("proje{}", Config::RECENT_LIMIT + 4));
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
