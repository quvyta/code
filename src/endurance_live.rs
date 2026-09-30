//! What the endurance trial of messages between tabs needs before the real `qcode` is started on
//! it: a home folder of its own with QCode's settings, a store with the trial's profiles and one
//! workspace carrying them, the ready-made providers with the owner's keys, a session that opens
//! the workspace with a tab of each profile, and each profile's image built with the product's
//! own recipe. Nothing is started here; the trial itself runs the program in a terminal, where
//! the agents of the five tabs give each other work for half an hour.
//!
//! It is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1` and `QCODE_ENDURANCE_HOME`
//! names the home folder to fill, which must not exist yet or be empty. The keys are read from the
//! owner's `~/.config/quvyta/*-key` into this process and written only where QCode itself keeps
//! them, `providers.toml` of that home, with the file's own mode 600, because the program the
//! trial runs reads them from nowhere else. So the copy lives exactly as long as the trial needs
//! it: a preparation that fails, however it fails, takes the whole home away again, and the trial
//! ends by clearing it with the second test here.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 QCODE_ENDURANCE_HOME=/tmp/…/home cargo test --lib endurance_home_is_prepared -- --ignored
//! QCODE_CONTAINER_TESTS=1 QCODE_ENDURANCE_HOME=/tmp/…/home cargo test --lib endurance_home_is_cleared -- --ignored
//! ```

use std::path::{Path, PathBuf};

use crate::engine::{EngineKind, detect};
use crate::profile::harness_live::build;
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template};
use crate::provider::{Key, ProviderEntry, ProviderKind, Providers, Tag};
use crate::store::{Session, SessionTab, SessionTabKind, SessionWorkspace, Store, add_profile};

/// The workspace the trial runs in.
const WORKSPACE: &str = "dayaniklilik";

/// The trial's profiles: a name, its harness, and the tag of the provider it runs on with the
/// model it asks for. Two opencode tabs, Claude Code on each service, and Codex.
const PROFILES: [(&str, HarnessKind, &str, &str); 5] = [
    ("oc-mimo", HarnessKind::OpenCode, "mimo", "mimo-v2.6-flash"),
    ("oc-kimi", HarnessKind::OpenCode, "kimi", "kimi-for-coding"),
    ("cc-mimo", HarnessKind::ClaudeCode, "mimo", "mimo-v2.6-flash"),
    ("cc-kimi", HarnessKind::ClaudeCode, "kimi", "kimi-for-coding"),
    ("cx-mimo", HarnessKind::Codex, "mimo", "mimo-v2.6-flash"),
];

/// A trial profile: QCode basic, a provider of one's own, and no network, so everything the
/// harness reaches it reaches through the relay.
fn profile(name: &str, harness: HarnessKind, tag: &str, model: &str) -> Profile {
    Profile {
        name: SafeName::parse(name).expect("the name is safe"),
        harness,
        template: Template::Recommended,
        account: AccountKind::Provider,
        provider: Some(ProviderChoice::model(tag, model)),
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// The owner's key in `~/.config/quvyta/<file>`, read into this process alone.
fn owners_key(file: &str) -> Key {
    let home = std::env::var_os("HOME").expect("a home folder");
    let text = std::fs::read_to_string(PathBuf::from(home).join(".config/quvyta").join(file))
        .unwrap_or_else(|_| panic!("the owner's {file} is there"));
    Key::new(&text).expect("the key reads as one")
}

/// Writes `text` into `path`, making its folder.
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("the folder is made");
    std::fs::write(path, text).expect("the file is written");
}

/// A trial home being filled: removed with everything in it, the owner's keys first of all, when
/// it goes out of scope, unless it was [`kept`](Self::keep) for the trial. A preparation that
/// stops halfway, by a failed step or a panic, so leaves nothing behind.
struct Filling {
    home: Option<PathBuf>,
}

impl Filling {
    /// Starts filling `home`, which must be a folder nobody uses: one that is not there yet, or
    /// is empty. Anything else is refused rather than filled, since a failure would remove it.
    fn start(home: &Path) -> Self {
        let used = std::fs::read_dir(home).is_ok_and(|mut entries| entries.next().is_some());
        assert!(!used, "{} is not empty; the trial fills a home of its own", home.display());
        std::fs::create_dir_all(home).expect("the home is made");
        Self { home: Some(home.to_owned()) }
    }

    /// The home is whole: it stays for the trial, which clears it when it is over.
    fn keep(mut self) {
        self.home = None;
    }
}

impl Drop for Filling {
    fn drop(&mut self) {
        if let Some(home) = &self.home {
            let _ = std::fs::remove_dir_all(home);
        }
    }
}

/// Whether `home` is a home this trial filled: its workspace and its providers are where the
/// preparation puts them. Only such a folder is cleared.
fn is_trial_home(home: &Path) -> bool {
    home.join("Documents/Quvyta/Code/Workspaces").join(WORKSPACE).is_dir()
        && home.join(".local/share/quvyta/code/providers.toml").is_file()
}

/// Takes a trial home away, keys and all.
///
/// # Panics
///
/// When `home` is not a trial home: a folder named by mistake is never removed.
fn clear(home: &Path) {
    assert!(is_trial_home(home), "{} is not an endurance trial home; nothing is removed", home.display());
    std::fs::remove_dir_all(home).expect("the trial home is removed");
}

#[test]
#[ignore = "fills a home folder for the endurance trial and builds its images; run with QCODE_CONTAINER_TESTS=1 and QCODE_ENDURANCE_HOME"]
fn endurance_home_is_prepared() {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return;
    }
    let home = PathBuf::from(std::env::var("QCODE_ENDURANCE_HOME").expect("QCODE_ENDURANCE_HOME names the home"));
    let filling = Filling::start(&home);
    let config = home.join(".config");
    let data = home.join(".local/share");
    let root = home.join("Documents/Quvyta/Code");

    // The settings a person has once the setup is done: the engine and the store, English words,
    // and no question to crates.io, which is not what is being measured.
    write(
        &config.join("quvyta/code.conf"),
        &format!(
            "language = \"en\"\n\n[setup]\ncompleted = true\n\n[engine]\nkind = \"podman\"\n\n[folder]\npath = \"{}\"\n",
            root.display()
        ),
    );
    write(&config.join("quvyta/quvyta.conf"), "update-notice = false\n");

    let store = Store::new(&root);
    store.prepare().expect("the store is made");
    let today = qframe::date::Date::today_utc();
    let file = store.create_workspace("Dayaniklilik", today).expect("the workspace is made");
    assert_eq!(file.id.as_str(), WORKSPACE);
    let paths = store.workspace_paths(&file.id);
    let mut tabs = Vec::new();
    for (name, harness, tag, model) in PROFILES {
        store.write_profile(&profile(name, harness, tag, model)).expect("the profile is written");
        add_profile(&paths, name, today).expect("the workspace carries the profile");
        tabs.push(SessionTab {
            kind: SessionTabKind::Profile(name.to_owned()),
            conversation: None,
            opened: 0,
            number: None,
            name: None,
        });
    }
    std::fs::create_dir_all(paths.code.join("ledger")).expect("the ledger folder");

    let mut providers = Providers::in_memory().at(data.join("quvyta/code/providers.toml"));
    for (tag, kind, file) in
        [("mimo", ProviderKind::MimoTokenPlan, "mimo-key"), ("kimi", ProviderKind::KimiCode, "kimi-key")]
    {
        let mut entry = ProviderEntry::new(Tag::parse(tag).expect("a tag"), kind, kind.suggested_base());
        entry.key = Some(owners_key(file));
        providers.add(entry).expect("the provider is added");
    }
    providers.save().expect("providers.toml is written");

    let session = Session {
        active: Some(file.id.clone()),
        workspaces: vec![SessionWorkspace { id: file.id, active_tab: 0, tabs }],
    };
    session.save(&data.join("quvyta/code/session.toml")).expect("the session is written");

    let engine = detect(EngineKind::Podman).expect("podman answers");
    for (name, harness, tag, model) in PROFILES {
        let started = std::time::Instant::now();
        build(&engine, &profile(name, harness, tag, model));
        println!("{name}: image built in {} s", started.elapsed().as_secs());
    }
    let _ = crate::engine::run::capture(&engine.remove_image("qcode/harnesstest-base"));
    filling.keep();
    println!("{} is ready; clear it with endurance_home_is_cleared when the trial is over", home.display());
}

#[test]
#[ignore = "removes the home of the endurance trial, keys and all; run with QCODE_CONTAINER_TESTS=1 and QCODE_ENDURANCE_HOME"]
fn endurance_home_is_cleared() {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return;
    }
    clear(Path::new(&std::env::var("QCODE_ENDURANCE_HOME").expect("QCODE_ENDURANCE_HOME names the home")));
}

mod tests {
    use super::{Filling, WORKSPACE, clear, is_trial_home};
    use std::path::{Path, PathBuf};

    /// A folder of the test's own to put a trial home in.
    fn parent(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = std::env::temp_dir().join(format!("qcode-endurance-{name}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        dir
    }

    /// What the preparation writes that matters here: a key where QCode keeps it, and the workspace.
    fn fill(home: &Path) {
        super::write(&home.join(".local/share/quvyta/code/providers.toml"), "key = \"stand-in\"\n");
        std::fs::create_dir_all(home.join("Documents/Quvyta/Code/Workspaces").join(WORKSPACE)).expect("a workspace");
    }

    #[test]
    fn a_preparation_that_stops_halfway_leaves_no_key_behind() {
        let dir = parent("halfway");
        let home = dir.join("home");
        let stopped = std::panic::catch_unwind(|| {
            let _filling = Filling::start(&home);
            fill(&home);
            panic!("a step of the preparation fails");
        });
        let gone = !home.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(stopped.is_err());
        assert!(gone, "the half-made home and the key in it are gone");
    }

    #[test]
    fn a_finished_home_stays_for_the_trial_and_is_cleared_after_it() {
        let dir = parent("finished");
        let home = dir.join("home");
        let filling = Filling::start(&home);
        fill(&home);
        filling.keep();
        let kept = is_trial_home(&home);
        clear(&home);
        let gone = !home.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kept, "the trial finds its home");
        assert!(gone, "and the end of the trial takes it away");
    }

    #[test]
    fn a_folder_in_use_is_neither_filled_nor_cleared() {
        let dir = parent("in-use");
        std::fs::write(dir.join("notes.txt"), "the person's own").expect("a file of theirs");
        let filled = std::panic::catch_unwind(|| Filling::start(&dir).keep());
        let cleared = std::panic::catch_unwind(|| clear(&dir));
        let kept = dir.join("notes.txt").is_file();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(filled.is_err() && cleared.is_err());
        assert!(kept, "the folder named by mistake is untouched");
    }
}
