//! Where the application opens, and the ways between its screens: the menu's rows, a workspace of
//! the list, and a profile or a provider made on the way and offered on the way back.

use super::*;

#[test]
fn a_machine_that_was_never_set_up_opens_the_wizard_on_the_gate_that_fell() {
    let gates = Gates { language: false, engine: EngineCheck::Unknown, location: LocationCheck::Unknown };
    let harness = harness(app(Config::parse_str("code.conf", ""), &gates, Some(SetupStep::Language)), SIZE.0, SIZE.1);
    assert_eq!(harness.app().page(), Page::Setup);
    assert!(harness.screen().contains("Chinese (Simplified)"), "{}", harness.screen());
}

#[test]
fn a_finished_setup_opens_the_home_screen() {
    let harness = harness(app(config(&scratch("open"), &[]), &settled(), None), SIZE.0, SIZE.1);
    assert_eq!(harness.app().page(), Page::Home);
    assert!(harness.screen().contains("New workspace"), "{}", harness.screen());
}

#[test]
fn settings_moved_from_the_old_folder_open_the_home_screen_they_describe() {
    let folder = scratch("moved-config");
    let _ = std::fs::remove_dir_all(&folder);
    let legacy = folder.join("code");
    std::fs::create_dir_all(&legacy).expect("folder");
    std::fs::write(legacy.join("settings.toml"), config(&scratch("moved"), &[]).to_toml()).expect("file");

    let loaded = Config::load_in(&folder, &legacy);

    assert!(loaded.is_clean(), "{:?}", loaded.diagnostics);
    let harness = harness(app(loaded.value, &settled(), None), SIZE.0, SIZE.1);
    assert_eq!(harness.app().page(), Page::Home, "the finished setup came along");
    assert!(harness.screen().contains("New workspace"), "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn an_old_settings_file_that_stayed_behind_is_said_on_the_settings_screen() {
    let folder = scratch("left-config");
    let _ = std::fs::remove_dir_all(&folder);
    let legacy = folder.join("code");
    std::fs::create_dir_all(&legacy).expect("folder");
    std::fs::write(legacy.join("settings.toml"), "[engine]\nkind = \"docker\"\n").expect("file");
    std::fs::write(folder.join("code.conf"), config(&scratch("left"), &[]).to_toml()).expect("file");

    let loaded = Config::load_in(&folder, &legacy);
    let app = app(loaded.value, &settled(), None).with_left_behind(loaded.diagnostics);
    // Tall enough for the report and every row of the page under it, so its title is not
    // scrolled away to keep the settings list in view.
    let mut harness = harness(app, SIZE.0, 40);
    assert_eq!(harness.app().page(), Page::Home);
    harness.click_text("Settings");

    let screen = harness.screen();
    assert!(screen.contains("Some old settings files stayed where they were"), "{screen}");
    assert!(screen.contains("settings.toml"), "{screen}");
    // The report has the keyboard, so the list below it does not scroll it out of sight.
    assert!(harness.is_focused("left-behind-read"), "{screen}");

    harness.press("enter").advance(MOMENT);
    let screen = harness.screen();
    assert!(!screen.contains("Some old settings files stayed where they were"), "{screen}");
    assert!(harness.is_focused("settings"), "the list takes the keyboard once the report is read");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_gate_that_falls_after_the_setup_leaves_the_wizard_shut_and_puts_the_strip_up() {
    // The design's rule: once the wizard has been through it never opens by itself again.
    // A missing engine is said in the strip over the screen, not asked for all over again.
    let harness = harness(app(config(&scratch("gone"), &[]), &without_engine(), None), SIZE.0, SIZE.1);
    assert_eq!(harness.app().page(), Page::Home);
    let screen = harness.screen();
    assert!(screen.contains("No container engine"), "{screen}");
    assert!(screen.contains("Repair"), "{screen}");
}

#[test]
fn every_row_of_the_menu_opens_its_screen_and_the_way_back_leads_home() {
    let mut harness = harness(app(config(&scratch("menu"), &[]), &settled(), None), SIZE.0, SIZE.1);
    for (row, page, word) in [
        ("Workspaces", Page::Workspaces, "New workspace"),
        ("Profiles", Page::Profiles, "Profiles"),
        ("Providers", Page::Providers, "plain text"),
        ("Settings", Page::Settings, "Language"),
    ] {
        harness.click_text(row).advance(MOMENT);
        assert_eq!(harness.app().page(), page, "`{row}` opens its screen:\n{}", harness.screen());
        assert!(harness.screen().contains(word), "`{word}` is on the screen `{row}` opened:\n{}", harness.screen());
        click_back(&mut harness);
        harness.advance(MOMENT);
        assert_eq!(harness.app().page(), Page::Home, "the way back leads home:\n{}", harness.screen());
    }
}

/// Walks the whole wizard, choosing the language whose name on the first step is `language`,
/// and answers what the settings file says afterwards when it is read back from its own text.
fn language_after_the_wizard(name: &str, language: &str, next: &str, finish: &str) -> Option<String> {
    let gates = Gates { language: false, engine: EngineCheck::Working, location: LocationCheck::Usable };
    let text = format!("[folder]\npath = \"{}\"\n", scratch(name).display());
    let app = app(Config::parse_str("code.conf", &text), &gates, Some(SetupStep::Language));
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.click_text(language).advance(MOMENT);
    for _ in 0..2 {
        harness.click_text(next).advance(MOMENT);
    }
    harness.click_text(finish).advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Home, "{}", harness.screen());
    // Read back from the text that would be written, not from the settings in memory: a
    // choice that is not in the file is a choice the next start does not have.
    Config::parse_str("code.conf", &harness.app().config.to_toml()).settings().language()
}

#[test]
fn a_wizard_finished_in_turkish_opens_in_turkish_next_time() {
    assert_eq!(language_after_the_wizard("language-tr", "Turkish", "İleri", "Bitir").as_deref(), Some("tr"));
}

#[test]
fn a_wizard_finished_in_english_opens_in_english_next_time() {
    // English is the answer that used to be the schema's default, and a default is the one
    // answer a settings file can lose: without it written down, a machine whose own locale
    // is Turkish would open in Turkish however plainly the person said English.
    assert_eq!(language_after_the_wizard("language-en", "English", "Next", "Finish").as_deref(), Some("en"));
}

#[test]
fn a_workspace_of_the_list_opens_its_own_screen_and_the_way_back_leads_to_the_list() {
    let root = scratch("open-workspace");
    let _ = std::fs::remove_dir_all(&root);
    let store = crate::store::Store::new(&root);
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Firefly").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    assert!(harness.screen().contains("No tab is open"), "{}", harness.screen());
    rail_back(&mut harness);
    assert_eq!(harness.app().page(), Page::Workspaces, "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_way_to_a_profile_from_a_workspace_comes_back_with_the_new_profile_offered() {
    let root = scratch("workspace-profiles");
    let _ = std::fs::remove_dir_all(&root);
    let store = crate::store::Store::new(&root);
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Firefly").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace);

    harness.send(Msg::Workspace(crate::ui::workspace::Msg::ManageProfiles)).advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Profiles, "the row of the chooser leads to the profiles screen");

    // A profile made there, while the workspace screen waits behind it.
    let profile = crate::profile::Profile {
        name: SafeName::parse("claude-sub").expect("the name is safe"),
        harness: crate::profile::HarnessKind::ClaudeCode,
        template: crate::profile::Template::Recommended,
        account: crate::profile::AccountKind::Subscription,
        provider: None,
        assets: crate::profile::MountAccess::ReadOnly,
        network: crate::profile::NetworkMode::Full,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    };
    store.write_profile(&profile).expect("the store takes a profile");
    click_back(&mut harness);
    harness.advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Workspace, "{}", harness.screen());
    let offered: Vec<String> = harness
        .app()
        .workspace
        .as_ref()
        .and_then(WorkspaceScreen::workspace)
        .map(|workspace| workspace.profiles().iter().map(|profile| profile.name.as_str().to_owned()).collect())
        .unwrap_or_default();
    assert_eq!(offered, ["claude-sub"], "the next new tab offers it without opening the workspace again");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_profile_made_for_a_workspace_takes_the_person_straight_back_to_it() {
    use crate::ui::profiles::Msg as Profiles;
    let root = scratch("workspace-profile-back");
    let _ = std::fs::remove_dir_all(&root);
    let store = crate::store::Store::new(&root);
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("the store takes a workspace");
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Workspaces").advance(MOMENT);
    harness.click_text("Firefly").advance(MOMENT);
    // The row a workspace's new tab offers when it has no profile to open a tab with. One
    // press for one wish: it lands in the wizard, not on a list with another such button.
    harness.click_text("New tab").advance(MOMENT);
    harness.click_text("New profile").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
    assert!(
        harness.app().profiles.as_ref().and_then(|screen| screen.draft()).is_some(),
        "the wizard is open:\n{}",
        harness.screen()
    );
    assert!(!harness.screen().contains("No profiles yet"), "{}", harness.screen());

    harness.type_text("scout");
    // A harness with no sign-in, so the image page is the wizard's last one.
    harness.click_text("opencode").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("free, no account").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    // The engine's answer, which is the one thing here that a machine has to give.
    harness.send(Msg::Profiles(Profiles::BuildEnded(Ok(())))).advance(MOMENT);

    assert_eq!(harness.app().page(), Page::Profiles, "still on the wizard until Finish is pressed");
    harness.click_text("Finish").advance(MOMENT);
    assert_eq!(
        harness.app().page(),
        Page::Workspace,
        "the profile was made for the workspace, so the workspace is where it goes:\n{}",
        harness.screen()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_provider_added_on_the_way_out_of_the_wizard_is_offered_when_the_person_comes_back() {
    let root = scratch("wizard-provider");
    let _ = std::fs::remove_dir_all(&root);
    let _store = crate::store::Store::new(&root);
    let providers = scratch("wizard-provider-file").join("providers.toml");
    let _ = std::fs::remove_dir_all(providers.parent().expect("its folder"));
    let app =
        app(config(&root, &[]), &settled(), None).with_providers(Some(providers.clone()), crate::testing::no_web());
    let mut harness = harness(app, SIZE.0, SIZE.1);
    harness.click_text("Profiles").advance(MOMENT);
    harness.click_text("New profile").advance(MOMENT);
    // Claude Code is the harness offered first; the system, the template and then the account
    // follow.
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("a provider of your own").advance(MOMENT);
    assert!(harness.screen().contains("No provider yet"), "{}", harness.screen());

    // Exactly what the page says to do: go to Providers, add one, come back.
    harness.click_text("Go to Providers").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Providers, "{}", harness.screen());
    harness.click_text("Add a provider").advance(MOMENT);
    harness.click_text("ollama").advance(MOMENT);
    harness.press("tab");
    harness.type_text("ev");
    harness.press("tab");
    for _ in 0..80 {
        harness.press("backspace");
    }
    harness.type_text("http://192.168.122.1:11434");
    harness.click_text("Add").advance(MOMENT);
    assert!(std::fs::read_to_string(&providers).is_ok_and(|text| text.contains("\"ev\"")), "it was added");
    click_back(&mut harness);
    harness.advance(MOMENT);

    assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
    let screen = harness.screen();
    assert!(!screen.contains("No provider yet"), "the wizard no longer says so:\n{screen}");
    assert!(screen.contains("ev"), "the provider just added is offered:\n{screen}");
    harness.click_text("ev").advance(MOMENT);
    let draft = harness.app().profiles.as_ref().and_then(|screen| screen.draft()).expect("the wizard is open");
    assert_eq!(draft.provider_tag.as_deref(), Some("ev"), "and it can be chosen");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(providers.parent().expect("its folder"));
}

#[test]
fn a_profile_made_from_the_home_screen_leaves_the_list_of_profiles_open() {
    use crate::ui::profiles::Msg as Profiles;
    // Nobody was waiting for this one. Somebody who came to the profiles to make profiles is
    // most likely making another, so the list they came for stays in front of them.
    let root = scratch("home-profile-stays");
    let _ = std::fs::remove_dir_all(&root);
    let _store = crate::store::Store::new(&root);
    let mut harness = harness(app(config(&root, &[]), &settled(), None), SIZE.0, SIZE.1);
    harness.click_text("Profiles").advance(MOMENT);
    harness.click_text("New profile").advance(MOMENT);
    harness.type_text("scout");
    harness.click_text("opencode").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("free, no account").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.click_text("Next").advance(MOMENT);
    harness.send(Msg::Profiles(Profiles::BuildEnded(Ok(())))).advance(MOMENT);
    harness.click_text("Finish").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Profiles, "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&root);
}
