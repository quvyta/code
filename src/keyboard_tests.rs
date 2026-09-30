//! Walking QCode with the keyboard alone: each screen from where it opens to what it is for,
//! with nothing but keys, the way a person who never reaches for the mouse uses it.

use std::path::{Path, PathBuf};

use qframe::runtime::Harness;

use crate::QCode;
use crate::engine::{Engine, EngineKind};
use crate::profile::Extra;
use crate::testing::{config, dirs, harness, highlighted, host, no_web, providers_file, settled};

/// A folder of this test's own, empty.
fn scratch(what: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-keyboard-{what}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("a scratch folder");
    folder
}

/// A stand-in engine in `folder` that answers every call with success, and keeps the
/// Containerfile of every image it is asked to build beside itself.
fn building_engine(folder: &Path) -> Engine {
    let script = format!(
        "#!/bin/sh\n\
         [ \"$1\" = build ] || exit 0\n\
         while [ \"$#\" -gt 0 ]; do\n\
         case \"$1\" in --tag) tag=\"$2\" ;; --file) file=\"$2\" ;; esac\n\
         shift\n\
         done\n\
         cp \"$file\" \"{folder}/built-$(echo \"$tag\" | tr / -)\"\n",
        folder = folder.display()
    );
    let binary = folder.join("engine");
    std::fs::write(&binary, script).expect("the stand-in engine is written");
    std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
    Engine::new(EngineKind::Podman, &binary)
}

/// QCode on its home screen over the store `store`, with `engine`.
fn qcode(store: &Path, engine: Engine) -> Harness<QCode> {
    let app = QCode::new(config(store, &[]), dirs(), host(), &settled(), Some(engine), None, None)
        .with_providers(Some(providers_file()), no_web());
    harness(app, 110, 44)
}

/// Waits, with a generous bound, until `done` holds, drawing the screen in between so the work
/// the screen started can answer.
fn wait(harness: &mut Harness<QCode>, what: &str, done: impl Fn(&Harness<QCode>) -> bool) {
    let started = std::time::Instant::now();
    while !done(harness) {
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "{what}:\n{}", harness.screen());
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.render();
    }
}

#[test]
fn a_profile_is_made_through_every_page_of_the_wizard_with_the_keyboard_alone() {
    let folder = scratch("profile");
    let store = folder.join("store");
    let mut harness = qcode(&store, building_engine(&folder));

    // Home: Profiles is the third row of the menu.
    harness.press("down").press("down").press("enter");
    wait(&mut harness, "the profiles are read", |harness| harness.screen().contains("No profiles yet"));
    // The empty screen's one button has the keyboard.
    harness.press("enter");
    assert!(harness.is_focused("profile-harness"), "the wizard opens on its first question:\n{}", harness.screen());

    // Harness: opencode, the second, which can run without an account; its name follows it.
    // Up on the first goes round to the last and Down from there round to the first again.
    harness.press("up").press("down").press("down");
    harness.press("tab");
    assert!(harness.is_focused("profile-name"), "{}", harness.screen());
    harness.press("ctrl+a").press("backspace");
    harness.type_text("keys");
    // Enter after the name goes on.
    harness.press("enter");

    // System: the keyboard is on the question; Debian stays.
    assert!(harness.is_focused("profile-system"), "{}", harness.screen());
    harness.press("tab").press("tab").press("tab");
    assert!(harness.is_focused("wizard-next"), "Cancel, Back, then Next:\n{}", harness.screen());
    harness.press("enter");

    // Template: Left on the picker's first row goes round to Custom, which switches everything
    // off; Right goes round to QCode recommended again and on to QCode extra. In the list below,
    // Space on oh-my-opencode-slim switches it on and oh-my-openagent off, and Space on the Rust
    // toolchain adds what no ready-made set pairs with that team.
    assert!(harness.is_focused("profile-template"), "{}", harness.screen());
    harness.press("left").press("right").press("right");
    harness.press("tab");
    assert!(harness.is_focused("profile-extras"), "{}", harness.screen());
    harness.press("down").press("down").press("space");
    harness.press("down").press("space");
    for _ in 0..16 {
        if harness.is_focused("wizard-next") {
            break;
        }
        harness.press("tab");
    }
    harness.press("enter");

    // Account: free, the first of opencode's.
    assert!(harness.is_focused("profile-account"), "{}", harness.screen());
    assert!(harness.screen().contains("free, no account"), "{}", harness.screen());
    harness.press("tab").press("tab").press("tab").press("enter");

    // Permissions: the assets folder, writable at first, read-only with Right; the network cut off
    // with End.
    assert!(harness.is_focused("profile-assets"), "{}", harness.screen());
    harness.press("right");
    harness.press("tab");
    assert!(harness.is_focused("profile-network"), "{}", harness.screen());
    harness.press("end");
    harness.press("tab").press("tab").press("tab").press("enter");

    // Image: arriving starts the build, and a free profile finishes there.
    wait(&mut harness, "the image is built", |harness| harness.screen().contains("Finish"));
    assert!(harness.is_focused("wizard-next"), "{}", harness.screen());
    harness.press("enter");

    // No ready-made set is that team with the toolchain, so the profile is a custom one, in the
    // folder a QCode from before custom profiles does not read.
    let saved = std::fs::read_to_string(store.join("Profiles").join("custom").join("keys.toml"))
        .unwrap_or_else(|trouble| panic!("the profile is saved: {trouble}\n{}", harness.screen()));
    let profile = crate::profile::Profile::parse("keys.toml", &saved).profile.expect("it reads back");
    assert_eq!(profile.harness, crate::profile::HarnessKind::OpenCode, "{saved}");
    assert_eq!(profile.account, crate::profile::AccountKind::Free, "{saved}");
    assert_eq!(profile.assets, crate::profile::MountAccess::ReadOnly, "{saved}");
    assert_eq!(profile.network, crate::profile::NetworkMode::ALL[crate::profile::NetworkMode::ALL.len() - 1]);
    assert_eq!(profile.template, crate::profile::Template::Custom, "{saved}");
    let parts = [Extra::Graphify, Extra::OhMyOpenCodeSlim, Extra::Rust, Extra::Settings];
    assert_eq!(profile.parts(), parts, "{saved}");
    let _ = std::fs::remove_dir_all(&folder);
}

/// The rows of the home menu without a workspace to continue, top to bottom.
const HOME_ROWS: [&str; 6] = ["New workspace", "Workspaces", "Profiles", "Providers", "Settings", "Quit"];

#[test]
fn the_home_menu_goes_round_at_both_ends() {
    let folder = scratch("home");
    let mut harness = qcode(&folder.join("store"), building_engine(&folder));
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("New workspace"), "{}", harness.screen());
    harness.press("up");
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("Quit"), "Up on the first row:\n{}", harness.screen());
    harness.press("down");
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("New workspace"), "Down on the last row:\n{}", harness.screen());
    harness.press("down");
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("Workspaces"), "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn workspaces_are_made_and_walked_round_with_the_keyboard_alone() {
    let folder = scratch("workspaces");
    let store = folder.join("store");
    let mut harness = qcode(&store, building_engine(&folder));

    harness.press("down").press("enter");
    wait(&mut harness, "the workspaces are read", |harness| harness.screen().contains("No workspaces yet"));
    for name in ["Alder", "Birch"] {
        // The empty page's one button, and after that the list's first row, make a new one.
        harness.press("enter");
        assert!(harness.is_focused("workspace-name"), "the form opens on its name:\n{}", harness.screen());
        harness.type_text(name);
        // The name, where it starts from, Cancel, then Create.
        harness.press("tab").press("tab").press("tab");
        harness.press("enter");
        wait(&mut harness, "the workspace is made", |harness| !harness.screen().contains("Start from"));
        // The button the keyboard was on went with the empty page; the list takes it.
        wait(&mut harness, "the keyboard is back on the list", |harness| harness.is_focused("workspaces"));
        harness.press("home");
    }
    assert!(store.join("Workspaces").read_dir().is_ok_and(|mut entries| entries.nth(1).is_some()), "two were made");

    let rows = ["New workspace", "Alder", "Birch"];
    assert_eq!(highlighted(&harness, &rows), Some("New workspace"), "{}", harness.screen());
    harness.press("up");
    assert_eq!(highlighted(&harness, &rows), Some("Birch"), "Up on the first row:\n{}", harness.screen());
    harness.press("down");
    assert_eq!(highlighted(&harness, &rows), Some("New workspace"), "Down on the last row:\n{}", harness.screen());

    // Esc goes back one level: from the screen to the home menu.
    harness.press("esc");
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("Workspaces"), "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn providers_are_added_and_walked_round_with_the_keyboard_alone() {
    let folder = scratch("providers");
    let file = folder.join("providers.toml");
    let app = QCode::new(config(&folder.join("store"), &[]), dirs(), host(), &settled(), None, None, None)
        .with_providers(Some(file.clone()), no_web());
    let mut harness = harness(app, 110, 44);

    harness.press("down").press("down").press("down").press("enter");
    wait(&mut harness, "the providers are read", |harness| harness.screen().contains("Add a provider"));
    // The empty page's one button has the keyboard.
    harness.press("enter");
    for tag in ["box", "cup", "dew"] {
        // The dialog opens on the tag; an ollama server on the address it offers is kept.
        assert!(harness.is_focused("provider-tag"), "{}", harness.screen());
        harness.type_text(tag);
        // The address, Cancel, then Add.
        harness.press("tab").press("tab").press("tab").press("enter");
        wait(&mut harness, "the dialog closes", |harness| !harness.screen().contains("New provider"));
        wait(&mut harness, "the keyboard is on the list it was added to", |harness| harness.is_focused("providers"));
        // Below the list: the buttons that ask this provider something and delete it, and the way
        // to another one, which stands beside the page's name. Walked to by name, since how many
        // buttons stand between is the page's own business.
        if tag != "dew" {
            for _ in 0..8 {
                if harness.is_focused("provider-new") {
                    break;
                }
                harness.press("tab");
            }
            assert!(harness.is_focused("provider-new"), "the way to another provider:\n{}", harness.screen());
            harness.press("enter");
        }
    }
    let saved = std::fs::read_to_string(&file).expect("the providers are saved");
    assert!(["box", "cup", "dew"].iter().all(|tag| saved.contains(tag)), "{saved}");

    let rows = ["box", "cup", "dew"];
    assert_eq!(highlighted(&harness, &rows), Some("dew"), "the one just added is chosen:\n{}", harness.screen());
    harness.press("down");
    assert_eq!(highlighted(&harness, &rows), Some("box"), "Down on the last row:\n{}", harness.screen());
    harness.press("up");
    assert_eq!(highlighted(&harness, &rows), Some("dew"), "Up on the first row:\n{}", harness.screen());

    harness.press("esc");
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("Providers"), "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_setting_is_changed_and_its_choices_go_round_with_the_keyboard_alone() {
    let folder = scratch("settings");
    let mut harness = qcode(&folder.join("store"), building_engine(&folder));
    harness.press("up").press("up").press("enter");
    let rows = [
        "Language",
        "Theme",
        "Icons",
        "Reduce motion",
        "Container engine",
        "When QCode closes",
        "Back up open workspaces",
        "Editor",
        "Sounds",
        "Ask before the first message",
    ];
    assert_eq!(highlighted(&harness, &rows), Some("Language"), "the list has the keyboard:\n{}", harness.screen());
    for _ in 0..rows.len() {
        if highlighted(&harness, &rows) == Some("Editor") {
            break;
        }
        harness.press("down");
    }
    assert_eq!(highlighted(&harness, &rows), Some("Editor"), "{}", harness.screen());
    assert_eq!(harness.app().config.editor(), crate::base::apps::Editor::Nano);
    harness.press("right");
    assert_eq!(harness.app().config.editor(), crate::base::apps::Editor::Vim, "{}", harness.screen());
    // Right on the last choice goes round to the first.
    harness.press("right");
    assert_eq!(harness.app().config.editor(), crate::base::apps::Editor::Nano, "{}", harness.screen());
    harness.press("left");
    assert_eq!(
        harness.app().config.editor(),
        crate::base::apps::Editor::Vim,
        "Left on the first:\n{}",
        harness.screen()
    );

    harness.press("esc");
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("Settings"), "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_first_start_is_finished_with_the_keyboard_alone() {
    let folder = scratch("setup");
    let gates = crate::ui::setup::gates::Gates {
        language: false,
        engine: crate::ui::setup::gates::EngineCheck::Working,
        location: crate::ui::setup::gates::LocationCheck::Usable,
    };
    let app = crate::testing::app(
        crate::store::Config::parse_str("code.conf", ""),
        &gates,
        Some(crate::store::SetupStep::Language),
    );
    let mut harness = harness(app, 110, 44);
    let _ = &folder;

    assert!(harness.is_focused("setup-language"), "the page opens on its question:\n{}", harness.screen());
    // Up on the first language goes round to the last, and the screen speaks it at once.
    harness.press("up");
    assert!(harness.screen().contains("Türkçe"), "{}", harness.screen());
    harness.press("down");
    assert!(harness.screen().contains("Chinese (Simplified)"), "and back round:\n{}", harness.screen());
    harness.press("tab");
    assert!(harness.is_focused("wizard-next"), "{}", harness.screen());
    harness.press("enter");

    assert!(harness.is_focused("setup-engine"), "the engine page opens on its question:\n{}", harness.screen());
    for _ in 0..8 {
        if harness.is_focused("wizard-next") {
            break;
        }
        harness.press("tab");
    }
    harness.press("enter");

    assert!(harness.is_focused("setup-location"), "the place opens on its question:\n{}", harness.screen());
    for _ in 0..8 {
        if harness.is_focused("wizard-next") {
            break;
        }
        harness.press("tab");
    }
    assert!(harness.screen().contains("Finish"), "{}", harness.screen());
    harness.press("enter");
    wait(&mut harness, "the wizard finishes", |harness| harness.app().config.setup_completed());
    assert_eq!(harness.app().config.engine_kind(), Some("podman"));
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_profiles_list_goes_round_at_both_ends() {
    let folder = scratch("profiles-round");
    let store = folder.join("store");
    let profiles = store.join("Profiles");
    std::fs::create_dir_all(&profiles).expect("a profiles folder");
    for name in ["alpha", "beta"] {
        let text = format!(
            "name = \"{name}\"\nharness = \"opencode\"\ntemplate = \"base\"\naccount = \"free\"\n\
             image = \"qcode/profile/{name}\"\n\n[mounts]\ncode = \"rw\"\nassets = \"ro\"\n\n[network]\nmode = \"all\"\n"
        );
        std::fs::write(profiles.join(format!("{name}.toml")), text).expect("a profile");
    }
    let mut harness = qcode(&store, building_engine(&folder));
    harness.press("down").press("down").press("enter");
    wait(&mut harness, "the profiles are read", |harness| harness.is_focused("profiles"));

    let rows = ["New profile", "alpha", "beta"];
    harness.press("home");
    assert_eq!(highlighted(&harness, &rows), Some("New profile"), "{}", harness.screen());
    harness.press("up");
    assert_eq!(highlighted(&harness, &rows), Some("beta"), "Up on the first row:\n{}", harness.screen());
    harness.press("down");
    assert_eq!(highlighted(&harness, &rows), Some("New profile"), "Down on the last row:\n{}", harness.screen());

    harness.press("esc");
    assert_eq!(highlighted(&harness, &HOME_ROWS), Some("Profiles"), "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}
