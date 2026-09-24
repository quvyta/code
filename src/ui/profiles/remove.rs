//! Deleting a profile: finding everything it has in the store and in the engine, and taking that
//! away, the workspaces' homes of it only when the person asks for them too.
//!
//! A profile is its definition file, its image, the volume its login is kept in, and in every
//! workspace that used it a container or two and a home volume. The home is the workspace's own
//! record of the harness — its history, memory and settings — so it stays unless the person
//! chooses otherwise. A workspace that carried the profile stops naming it in its
//! `workspace.qcode`, so nothing there offers a profile that is gone.
//!
//! Names are matched the way deleting a workspace matches them: a container or volume name that a
//! different workspace and profile could also have produced is left where it is.

use std::fs;

use crate::engine::names;
use crate::engine::run::{EngineError, capture};
use crate::engine::{Container, Engine};
use crate::profile::SafeName;
use crate::store::{Store, WorkspaceId, remove_profile};
use crate::ui::workspaces::Names;

/// Everything of one profile, as it was found when the person asked to delete it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// The profile.
    pub name: SafeName,
    /// The workspaces whose file names the profile.
    pub carried_by: Vec<WorkspaceId>,
    /// What the engine answered.
    pub engine: Reach,
}

/// What the engine holds of a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// There is no engine, so whatever it holds of the profile stays there.
    Absent,
    /// The engine could not be asked; its own words.
    Unreachable(String),
    /// The engine was asked.
    Listed(Held),
}

/// What the engine was found to hold of a profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    /// The profile's image, when it is there.
    pub image: Option<String>,
    /// The volume its login is kept in, when there is one.
    pub login: Option<String>,
    /// Its containers in the workspaces.
    pub containers: Vec<String>,
    /// The ones among them that are running.
    pub running: Vec<String>,
    /// The homes the workspaces keep of it.
    pub homes: Vec<String>,
}

impl Survey {
    /// The profile's containers that are running, which is what stops the deletion.
    #[must_use]
    pub fn running(&self) -> &[String] {
        match &self.engine {
            Reach::Listed(held) => &held.running,
            Reach::Absent | Reach::Unreachable(_) => &[],
        }
    }

    /// The homes the workspaces keep of the profile.
    #[must_use]
    pub fn homes(&self) -> &[String] {
        match &self.engine {
            Reach::Listed(held) => &held.homes,
            Reach::Absent | Reach::Unreachable(_) => &[],
        }
    }
}

/// What came of a deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Everything asked for is gone.
    Deleted,
    /// A container of the profile was started after the question was asked, so nothing was
    /// removed.
    Running(Vec<String>),
    /// Some of it could not be removed: each line names what is left and why.
    Partly(Vec<String>),
}

/// Finds everything of the profile `name` in `store` and `engine`.
///
/// Runs the engine and reads the disk, so it belongs on a background thread.
#[must_use]
pub fn survey(store: &Store, engine: Option<&Engine>, name: &SafeName) -> Survey {
    let carried_by = store
        .workspaces()
        .value
        .into_iter()
        .filter_map(|entry| entry.file)
        .filter(|file| file.profiles.iter().any(|profile| profile.name == name.as_str()))
        .map(|file| file.id)
        .collect();
    let engine = match engine {
        None => Reach::Absent,
        Some(engine) => held(store, engine, name),
    };
    Survey { name: name.clone(), carried_by, engine }
}

/// Removes the profile `survey` found, and the workspaces' homes of it when `homes` is true.
///
/// The containers go first, so no volume or image is in use when its turn comes; the definition
/// file goes last, so a profile whose engine part could not be removed stays on the list to be
/// deleted again. The engine is asked once more first: a container started since the question was
/// asked is not stopped behind the person's back.
///
/// Runs the engine and the disk, so it belongs on a background thread.
#[must_use]
pub fn remove(store: &Store, engine: Option<&Engine>, survey: &Survey, homes: bool) -> Outcome {
    let mut left = Vec::new();
    if let (Some(engine), Reach::Listed(found)) = (engine, &survey.engine) {
        if let Reach::Listed(now) = held(store, engine, &survey.name)
            && !now.running.is_empty()
        {
            return Outcome::Running(now.running);
        }
        let mut gone = |what: &str, command| {
            if let Err(error) = capture(&command) {
                left.push(format!("{what}: {}", said(&error)));
            }
        };
        for container in &found.containers {
            gone(container, engine.remove_container(container));
        }
        if let Some(image) = &found.image {
            gone(image, engine.remove_image(image));
        }
        if let Some(login) = &found.login {
            gone(login, engine.remove_volume(login));
        }
        if homes {
            for home in &found.homes {
                gone(home, engine.remove_volume(home));
            }
        }
    }
    for id in &survey.carried_by {
        if let Err(problem) = remove_profile(&store.workspace_paths(id), survey.name.as_str()) {
            left.push(problem.to_string());
        }
    }
    let file = store.profiles_dir().join(format!("{}.toml", survey.name));
    if left.is_empty()
        && let Err(error) = fs::remove_file(&file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        left.push(format!("{}: {error}", file.display()));
    }
    if left.is_empty() { Outcome::Deleted } else { Outcome::Partly(left) }
}

/// Asks the engine what it holds of the profile `name`.
fn held(store: &Store, engine: &Engine, name: &SafeName) -> Reach {
    let containers = match capture(&engine.list_containers()) {
        Ok(output) => Container::parse_list(&output),
        Err(error) => return Reach::Unreachable(said(&error)),
    };
    let volumes: Vec<String> = match capture(&engine.list_volumes()) {
        Ok(output) => output.lines().map(str::trim).map(str::to_owned).collect(),
        Err(error) => return Reach::Unreachable(said(&error)),
    };
    let image = names::profile_image(name.as_str());
    let image = capture(&engine.image_exists(&image)).is_ok().then_some(image);
    let login = names::credential_volume(name.as_str());
    let login = volumes.contains(&login).then_some(login);
    let names = Names::read(store);
    let profile = name.as_str();
    let mut held = Held { image, login, ..Held::default() };
    for container in containers {
        if names.profile_owns_container(profile, &container.name) {
            if container.state.is_running() {
                held.running.push(container.name.clone());
            }
            held.containers.push(container.name);
        }
    }
    held.homes = volumes.into_iter().filter(|volume| names.profile_owns_volume(profile, volume)).collect();
    Reach::Listed(held)
}

/// What the engine said when it would not do what was asked.
fn said(error: &EngineError) -> String {
    match error {
        EngineError::NotRunnable { error, .. } => error.to_string(),
        EngineError::Failed(failure) => failure.output.trim().to_owned(),
        EngineError::Cancelled { .. } => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use qframe::env::{AssetDirs, Env};
    use qframe::icons::GlyphMode;
    use qframe::prelude::*;
    use qframe::runtime::Harness;

    use super::super::{Msg, Profiles, update, view};
    use crate::engine::{Engine, EngineKind};
    use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
    use crate::store::{Store, add_profile};

    /// The profiles screen on its own.
    struct Host {
        state: Profiles,
    }

    impl App for Host {
        type Msg = Msg;

        fn update(&mut self, message: Msg) -> Command<Msg> {
            update(&mut self.state, message)
        }

        fn view(&self, ui: &mut View<'_, Msg>) {
            AppShell::new().body(|ui| view(&self.state, ui)).show(ui);
        }
    }

    /// A folder of this test's own, removed when it goes out of scope.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let stamp =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path = std::env::temp_dir().join(format!("qcode-profile-delete-{name}-{stamp}"));
            fs::create_dir_all(&path).expect("a folder in the temporary folder");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const CONTAINERS: &str = "qcode-firefly-claude\texited\nqcode-serenity-claude.desk\texited\nqcode-firefly-base\texited\nqcode-firefly-gemini\texited\n";
    const VOLUMES: &str = "qcode-cred-claude\nqcode-home-firefly-claude\nqcode-home-serenity-claude\nqcode-home-firefly-gemini\nqcode-cred-gemini\n";

    /// A stand-in engine that lists `containers` and `VOLUMES`, answers every other command with
    /// success — an image it is asked about is there — and writes down every command it is given.
    fn stand_in(scratch: &Scratch, containers: &str) -> (Engine, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch.0.join("engine");
        fs::create_dir_all(&dir).expect("a folder for the stand-in");
        fs::write(dir.join("ps"), containers).expect("the containers");
        fs::write(dir.join("volumes"), VOLUMES).expect("the volumes");
        let asked = dir.join("asked");
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{asked}'\ncase \"$1 $2\" in\n  ps\\ *) cat '{ps}' ;;\n  'volume ls') cat '{volumes}' ;;\nesac\nexit 0\n",
            asked = asked.display(),
            ps = dir.join("ps").display(),
            volumes = dir.join("volumes").display(),
        );
        let bin = dir.join("podman");
        fs::write(&bin, script).expect("the stand-in");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("runnable");
        (Engine::new(EngineKind::Podman, bin), asked)
    }

    /// The removals among what the stand-in engine was asked to do.
    fn removals(asked: &Path) -> Vec<String> {
        fs::read_to_string(asked)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("rm ") || line.contains(" rm "))
            .map(str::to_owned)
            .collect()
    }

    fn profile(name: &str, harness: HarnessKind) -> Profile {
        Profile {
            name: SafeName::parse(name).expect("the name is safe"),
            harness,
            template: Template::Recommended,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        }
    }

    /// A store with the profiles claude and gemini, and two workspaces that both carry claude.
    fn store(scratch: &Scratch) -> Store {
        let store = Store::new(scratch.0.join("store"));
        store.write_profile(&profile("claude", HarnessKind::ClaudeCode)).expect("written");
        store.write_profile(&profile("gemini", HarnessKind::GeminiCli)).expect("written");
        let today = qframe::date::Date::today_utc();
        for name in ["Firefly", "Serenity"] {
            let file = store.create_workspace(name, today).expect("made");
            add_profile(&store.workspace_paths(&file.id), "claude", today).expect("carried");
        }
        store
    }

    /// The screen over `store`, opened the way the application opens it, with its list read.
    fn screen(store: &Store, engine: Engine) -> Harness<Host> {
        let state = Profiles::new(Some(store.root().to_path_buf()), Some(engine)).with_providers_file(None);
        let env = Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() })
            .expect("the built-in files load");
        let mut harness = Harness::with_env(Host { state }, env, 110, 40);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Reload);
        settle(&mut harness, |harness| harness.screen().contains("Delete"));
        harness
    }

    /// Lets background work answer until `done` holds, for a generous but finite while.
    fn settle(harness: &mut Harness<Host>, done: impl Fn(&Harness<Host>) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !done(harness) && std::time::Instant::now() < deadline {
            harness.advance(Duration::from_millis(20)).render();
        }
    }

    /// Chooses claude in the list and presses its Delete, the way a person does.
    fn ask_about_claude(harness: &mut Harness<Host>) {
        harness.click_text("claude").render();
        harness.click_text("Delete").render();
        settle(harness, |harness| harness.screen().contains("Delete the profile claude?"));
    }

    fn carried(store: &Store, id: &str) -> Vec<String> {
        let id = crate::store::WorkspaceId::parse(id).expect("an identifier");
        store.read_workspace(&id).value.expect("readable").profiles.into_iter().map(|profile| profile.name).collect()
    }

    #[test]
    fn deleting_a_profile_takes_its_definition_image_login_and_containers_and_keeps_the_homes() {
        let scratch = Scratch::new("keep-homes");
        let store = store(&scratch);
        let (engine, asked) = stand_in(&scratch, CONTAINERS);
        let mut harness = screen(&store, engine);
        ask_about_claude(&mut harness);
        let text = harness.screen();
        for named in ["qcode/profile/claude", "qcode-cred-claude", "qcode-firefly-claude", "firefly, serenity"] {
            assert!(text.contains(named), "`{named}` is named in the question:\n{text}");
        }
        for other in ["qcode-firefly-gemini", "qcode-home-firefly-gemini", "qcode-cred-gemini", "qcode-firefly-base"] {
            assert!(!text.contains(other), "`{other}` is not claude's:\n{text}");
        }
        assert_eq!(removals(&asked), [] as [String; 0], "nothing is removed before the answer");

        harness.click_text("Delete, keep the homes").render();
        settle(&mut harness, |harness| harness.screen().contains("was deleted"));
        assert!(harness.screen().contains("The profile claude was deleted"), "{}", harness.screen());
        assert_eq!(
            removals(&asked),
            [
                "rm --force qcode-firefly-claude",
                "rm --force qcode-serenity-claude.desk",
                "image rm --force qcode/profile/claude",
                "volume rm qcode-cred-claude",
            ]
        );
        assert!(!store.profiles_dir().join("claude.toml").exists(), "the definition is gone");
        assert!(store.profiles_dir().join("gemini.toml").is_file(), "the other profile stays");
        assert_eq!(carried(&store, "firefly"), [] as [String; 0], "the workspace stops naming it");
        assert_eq!(carried(&store, "serenity"), [] as [String; 0]);
        let firefly = crate::store::WorkspaceId::parse("firefly").expect("an identifier");
        assert!(!store.workspace_paths(&firefly).harness_profile("claude").exists(), "with its empty folder");
        settle(&mut harness, |harness| !harness.screen().contains("claude"));
        assert!(!harness.screen().contains("claude"), "and the list says so:\n{}", harness.screen());
    }

    #[test]
    fn the_homes_go_too_only_when_the_person_says_so() {
        let scratch = Scratch::new("with-homes");
        let store = store(&scratch);
        let (engine, asked) = stand_in(&scratch, CONTAINERS);
        let mut harness = screen(&store, engine);
        ask_about_claude(&mut harness);
        harness.click_text("Delete with the homes").render();
        settle(&mut harness, |harness| harness.screen().contains("was deleted"));
        let removed = removals(&asked);
        for home in ["volume rm qcode-home-firefly-claude", "volume rm qcode-home-serenity-claude"] {
            assert!(removed.iter().any(|line| line == home), "{home} in {removed:?}");
        }
        assert!(!removed.iter().any(|line| line.contains("gemini")), "{removed:?}");
    }

    #[test]
    fn a_profile_with_a_running_container_is_not_deleted_and_the_container_is_named() {
        let scratch = Scratch::new("running");
        let store = store(&scratch);
        let running = CONTAINERS.replace("qcode-firefly-claude\texited", "qcode-firefly-claude\trunning");
        let (engine, asked) = stand_in(&scratch, &running);
        let mut harness = screen(&store, engine);
        harness.click_text("claude").render();
        harness.click_text("Delete").render();
        settle(&mut harness, |harness| harness.screen().contains("has a container running"));
        let text = harness.screen();
        assert!(text.contains("claude has a container running"), "{text}");
        assert!(!text.contains("Delete the profile claude?"), "{text}");
        assert_eq!(removals(&asked), [] as [String; 0]);
        assert!(store.profiles_dir().join("claude.toml").is_file());
    }

    #[test]
    fn a_new_profile_is_the_first_row_of_the_list_for_the_pointer_and_the_keyboard() {
        let scratch = Scratch::new("new-row");
        let store = store(&scratch);
        let (engine, _) = stand_in(&scratch, CONTAINERS);
        let mut harness = screen(&store, engine);
        let text = harness.screen();
        let lines: Vec<&str> = text.lines().collect();
        let new = lines.iter().position(|line| line.contains("New profile")).expect("the row is there");
        let first = lines.iter().position(|line| line.contains("claude")).expect("the profile is there");
        assert_eq!(first, new + 1, "the new row stands first, right over the profiles:\n{text}");
        assert_eq!(text.matches("New profile").count(), 1, "and it is the only way to one:\n{text}");

        harness.click_text("New profile").render();
        assert!(harness.app().state.draft().is_some(), "a click opens the wizard:\n{}", harness.screen());
        harness.click_text("Cancel").render();
        settle(&mut harness, |harness| harness.screen().contains("New profile"));

        while !harness.is_focused("profiles") {
            harness.press("tab");
        }
        harness.press("home").render();
        assert!(harness.app().state.draft().is_none(), "moving onto the row only chooses it");
        assert!(!harness.screen().contains("Rebuild image"), "and no profile's actions stand under it");
        harness.press("enter").render();
        assert!(harness.app().state.draft().is_some(), "Enter on it opens the wizard");
    }
}
