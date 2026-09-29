//! The sets of parts a profile is made with, the switches that pick them one by one, and what
//! each choice saves and downloads.

use super::*;

#[test]
fn opencode_is_offered_free_first_and_finishes_without_a_sign_in() {
    let mut harness = wizard_on(0);
    let opencode = HarnessKind::ALL.iter().position(|harness| *harness == HarnessKind::OpenCode);
    harness.send(Msg::PickHarness(opencode.expect("opencode is offered")));
    harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
    let screen = harness.screen();
    assert!(screen.contains("free, no account"), "{screen}");
    assert!(screen.contains("so there is no sign-in step"), "{screen}");
    assert!(!screen.contains("Sign in"), "the steps leave the sign-in out:\n{screen}");
    let (free, subscription) = (screen.find("free, no account"), screen.find("subscription"));
    assert!(free < subscription, "free use is listed first:\n{screen}");
    assert_eq!(harness.app().state.draft().expect("open").account, AccountKind::Free);

    harness.send(Msg::Next).send(Msg::Next).render();
    assert!(harness.screen().contains("Build the image"), "{}", harness.screen());
    harness.send(Msg::Finish).render();
    assert!(harness.app().state.draft().is_some(), "finishing without an image leaves nothing behind");
    harness.send(Msg::BuildEnded(Ok(()))).render();
    let screen = harness.screen();
    assert!(screen.contains("Finish"), "the image page is the last one:\n{screen}");
    assert!(screen.contains("opencode needs no sign-in"), "{screen}");
    harness.send(Msg::Next).render();
    assert_eq!(harness.app().state.draft().expect("open").stage, Stage::Image, "there is no page after it");
    harness.send(Msg::Finish).render();
    assert!(harness.app().state.draft().is_none(), "the wizard closes with the profile made");
}

#[test]
fn a_profile_finished_from_the_list_is_the_one_chosen_when_the_list_comes_back() {
    // No engine: the build's answer is given below instead of a missing binary failing it.
    let before = vec![profile("aaa-first", HarnessKind::ClaudeCode), profile("bbb-second", HarnessKind::Codex)];
    let mut harness = loaded(before.clone());
    harness.click_text("New profile").render();
    harness.click_text("opencode").render();
    let name = harness.app().state.draft().expect("the wizard is open").name.clone();
    for _ in 0..5 {
        next(&mut harness);
    }
    assert!(harness.screen().contains("Build the image"), "{}", harness.screen());
    // The engine's answer: the image is built. Nothing is built for real in a test.
    harness.send(Msg::BuildEnded(Ok(()))).render();
    harness.click_text("Finish").render();
    assert!(harness.app().state.draft().is_none(), "the wizard closed:\n{}", harness.screen());
    // What the store holds now, read back the way the reload reads it: the new profile last.
    // The test's store is empty, so its own reading comes back first without the profile,
    // which is the race a real reading can lose too.
    let mut after = before;
    let mut made = profile(&name, HarnessKind::OpenCode);
    made.account = AccountKind::Free;
    after.push(made);
    harness.send(Msg::Loaded(Listing { profiles: after, diagnostics: Vec::new() })).render();
    let chosen = harness.app().state.selected().expect("a profile is chosen");
    assert_eq!(chosen.profile.name.as_str(), name, "the new profile is the chosen one:\n{}", harness.screen());
    // And a later reading, with nothing just made, leaves the person's own choice alone.
    harness.click_text("aaa-first").render();
    let listing = harness.app().state.rows().iter().map(|row| row.profile.clone()).collect();
    harness.send(Msg::Loaded(Listing { profiles: listing, diagnostics: Vec::new() })).render();
    assert_eq!(harness.app().state.selected().expect("chosen").profile.name.as_str(), "aaa-first");
}

#[test]
fn a_free_profile_reads_as_ready_and_offers_no_sign_in() {
    let mut free = profile("oc-free", HarnessKind::OpenCode);
    free.account = AccountKind::Free;
    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing { profiles: vec![free], diagnostics: Vec::new() }));
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    let screen = harness.screen();
    assert!(screen.contains("No sign-in needed"), "{screen}");
    assert!(!screen.contains("Not signed in"), "{screen}");
    assert!(!screen.contains("Sign in") && !screen.contains("Sign out"), "{screen}");
    assert!(screen.contains("opencode, QCode recommended, free, no account"), "{screen}");
    assert!(harness.app().state.selected().expect("chosen").is_runnable());
    harness.send(Msg::SignInAsked).render();
    assert!(harness.app().state.draft().is_none(), "there is nothing to sign in to");
    harness.set_locale("tr").render();
    assert!(harness.screen().contains("Giriş gerekmez"), "{}", harness.screen());
    assert!(harness.screen().contains("ücretsiz, hesapsız"), "{}", harness.screen());
}

#[test]
fn every_set_the_picker_offers_sets_every_switch_on_the_page_to_what_it_carries() {
    // From where the person's hand is: the button that opens the wizard, the button that moves
    // on and the row that names the set. What each switch reads is read off the screen.
    for (row, set) in [
        ("QCode recommended", Template::Recommended),
        ("QCode extra", Template::High),
        ("Quvyta development", Template::QuvytaDev),
    ] {
        let mut harness = wizard_on(2);
        harness.click_text(row).render();
        let screen = harness.screen();
        assert_eq!(harness.app().state.draft().expect("open").preset(), set, "{row}:\n{screen}");
        for part in Template::available(HarnessKind::ClaudeCode) {
            let on = set.parts(HarnessKind::ClaudeCode).contains(&part);
            let label = row_of(&harness, part);
            assert_eq!(switch_on(&mut harness, &label), on, "{row}: `{}`\n{screen}", part.id());
        }
    }
    // opencode has a set of its own: the lighter team of agents, never beside the other.
    let mut harness = wizard_on(0);
    harness.click_text("opencode").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    for (row, set) in [("QCode recommended", Template::Recommended), ("QCode extra", Template::High)] {
        harness.click_text(row).render();
        let screen = harness.screen();
        assert_eq!(harness.app().state.draft().expect("open").preset(), set, "{row}:\n{screen}");
        for part in Template::available(HarnessKind::OpenCode) {
            let on = set.parts(HarnessKind::OpenCode).contains(&part);
            let label = row_of(&harness, part);
            assert_eq!(switch_on(&mut harness, &label), on, "{row}: `{}`\n{screen}", part.id());
        }
    }
    harness.click_text("oh my opencode slim").render();
    let screen = harness.screen();
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Slim, "{screen}");
    assert!(switch_on(&mut harness, "oh-my-opencode-slim"), "slim's own team is on:\n{screen}");
    assert!(!switch_on(&mut harness, "oh-my-openagent"), "and oh-my-openagent is not:\n{screen}");
    assert!(!screen.contains("superpowers"), "Claude Code's plugins are not opencode's:\n{screen}");
}

#[test]
fn a_switch_pressed_by_hand_makes_the_picker_read_custom_and_the_image_what_is_left_on() {
    // The set of nobody's: every switch off, then graphify and the Rust toolchain and nothing
    // else — no settings, no plugins, no team of agents.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-profiles-own-{stamp}"));
    let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let screen = harness.screen();
    assert!(screen.contains("The parts the image carries"), "{screen}");
    // QCode recommended: graphify, Claude Code's five starter plugins and the settings.
    assert!(switch_on(&mut harness, "graphify") && switch_on(&mut harness, "context7"), "{screen}");
    assert!(!switch_on(&mut harness, "hookify"), "a plugin only QCode extra carries is off:\n{screen}");
    assert!(part_on(&mut harness, Extra::Settings), "{screen}");

    // Custom starts from the harness as it comes: every switch off, and the page says so.
    harness.click_text("Custom").render();
    let screen = harness.screen();
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Custom, "{screen}");
    for part in Template::available(HarnessKind::ClaudeCode) {
        assert!(!part_on(&mut harness, part), "`{}` is off: {screen}", part.id());
    }
    assert!(screen.contains("Nothing is installed beside the harness"), "{screen}");

    // graphify and the toolchain on, and the rest left off.
    let (x, y) = switch_of(&mut harness, "graphify");
    harness.click(x, y).render();
    let (x, y) = switch_of(&mut harness, "Rust toolchain");
    harness.click(x, y).render();
    let screen = harness.screen();
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Custom, "{screen}");
    assert!(switch_on(&mut harness, "graphify") && switch_on(&mut harness, "Rust toolchain"), "{screen}");
    assert!(!part_on(&mut harness, Extra::Settings), "{screen}");

    let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
    assert_eq!(profile.template, Template::Custom);
    let recipe = super::recipe::image(&profile);
    let file = &recipe.containerfile;
    assert!(file.contains("graphifyy"), "graphify is on:\n{file}");
    assert!(file.contains("https://sh.rustup.rs"), "and so is the toolchain:\n{file}");
    for gone in ["context7", "claude plugin", "DISABLE_AUTOUPDATER", "COPY template", "chromium"] {
        assert!(!file.contains(gone), "`{gone}` is switched off and is not in the image:\n{file}");
    }
    assert!(recipe.files.is_empty(), "no settings of ours are written: {recipe:?}");

    // Written where a QCode from before custom profiles does not look, and read back as the
    // same profile.
    harness.app().state.store().expect("a store").write_profile(&profile).expect("the file is written");
    let text = std::fs::read_to_string(folder.join("Profiles").join("custom").join("claude-code.toml"))
        .expect("the file is there");
    assert!(text.contains("template = \"custom\""), "{text}");
    // A custom file names the parts it goes without, and every part of the list is named but
    // the two that are on — so a file cannot be read as a set that carries more than it does.
    assert!(text.contains("settings = false") && text.contains("context7 = false"), "{text}");
    assert!(!text.contains("graphify = false") && !text.contains("rust = false"), "{text}");
    let read = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
    assert_eq!(read, profile, "{text}");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_recommended_settings_switch_decides_whether_the_image_carries_our_own_settings() {
    // On: the harness's settings, its first questions answered and its maker's update checks
    // off. Off: none of them, and the image is the harness as it comes.
    for kind in [HarnessKind::ClaudeCode, HarnessKind::Codex, HarnessKind::OpenCode] {
        let mut harness = wizard_on(0);
        if kind != HarnessKind::ClaudeCode {
            harness.click_text(kind.record().display_name).render();
        }
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        let (x, y) = switch_of(&mut harness, "Recommended settings");
        harness.click(x, y).render();
        let draft = harness.app().state.draft().expect("open");
        assert!(!draft.has(crate::profile::Extra::Settings), "{kind:?}");
        let profile = draft.profile().expect("a profile");
        assert!(profile.files().is_empty(), "{kind:?}: {:?}", profile.files());
        assert!(profile.environment().is_empty(), "{kind:?}: {:?}", profile.environment());
        let recipe = super::recipe::image(&profile);
        assert!(recipe.files.is_empty(), "{kind:?}: {recipe:?}");
        assert!(!recipe.containerfile.contains("COPY template"), "{kind:?}");
        assert!(!recipe.containerfile.contains("ENV DISABLE_"), "{kind:?}");

        // Back on: everything the harness's own makers and QCode recommend is written again,
        // and the page reads as the set of ours that writes it.
        let (x, y) = switch_of(&mut harness, "Recommended settings");
        harness.click(x, y).render();
        let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
        assert!(!profile.files().is_empty(), "{kind:?}: the settings are written again");
        assert_eq!(profile.template, Template::Recommended, "{kind:?}: which is QCode recommended again");
    }
}

#[test]
fn the_two_teams_of_agents_switch_each_other_off() {
    let mut harness = wizard_on(0);
    harness.click_text("opencode").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let screen = harness.screen();
    assert!(switch_on(&mut harness, "oh-my-openagent") && !switch_on(&mut harness, "oh-my-opencode-slim"), "{screen}");

    let (x, y) = switch_of(&mut harness, "oh-my-opencode-slim");
    harness.click(x, y).render();
    let screen = harness.screen();
    assert!(
        switch_on(&mut harness, "oh-my-opencode-slim") && !switch_on(&mut harness, "oh-my-openagent"),
        "one team at a time, and it is the one just switched on:\n{screen}"
    );
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Slim, "slim's own set:\n{screen}");

    let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
    let recipe = super::recipe::image(&profile);
    let file = &recipe.containerfile;
    assert!(file.contains("oh-my-opencode-slim") && !file.contains("oh-my-openagent"), "{file}");
    // opencode is told to load the team that is there, and only that one.
    let settings: String = recipe.files.iter().map(|(_, contents)| contents.as_str()).collect();
    assert!(settings.contains("file:///usr/local/npm/lib/node_modules/oh-my-opencode-slim"), "{recipe:?}");
    assert!(!settings.contains("oh-my-openagent"), "{recipe:?}");
}

#[test]
fn choosing_a_set_says_what_it_downloads_and_what_the_network_means_and_saves_it() {
    // Every step is taken where the person takes it: the button that opens the wizard, the
    // button that moves on, the row that names the set and the one that names the network,
    // never the message sent by hand.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-profiles-high-{stamp}"));
    let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let screen = harness.screen();
    assert!(screen.contains("The parts the image carries"), "{screen}");
    // The whole list stands on the page at once, whatever the picker reads: QCode recommended
    // brings graphify and Claude Code's starter plugins, and the rest of the list is there to
    // be switched on or refused.
    for listed in [
        "graphify",
        "superpowers",
        "context7",
        "code-review",
        "security-guidance",
        "block-no-verify",
        "hookify",
        "Rust toolchain",
        "chromium",
        "Recommended settings",
    ] {
        assert!(screen.contains(listed), "`{listed}` is listed:\n{screen}");
    }
    assert!(
        screen.contains("What QCode recommended adds is downloaded from Debian 13, PyPI, npm and GitHub"),
        "recommended's download is said:\n{screen}"
    );

    harness.click_text("QCode extra").render();
    let screen = harness.screen();
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::High, "{screen}");
    for part in Template::High.parts(HarnessKind::ClaudeCode) {
        assert!(part_on(&mut harness, part), "`{}` is on:\n{screen}", part.id());
    }
    assert!(screen.contains("What QCode extra adds is downloaded"), "{screen}");
    assert!(!screen.contains("oh-my-openagent"), "that is opencode's, not Claude Code's:\n{screen}");
    assert!(screen.contains("so the build needs the network"), "the download is said:\n{screen}");
    let at = |text: &str| {
        let (x, y) = harness.find(text).unwrap_or_else(|| panic!("`{text}` is drawn"));
        harness.fg(u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"))
    };
    assert_ne!(at("needs the network"), at("superpowers"), "the network line stands out:\n{screen}");
    assert!(!screen.contains("context7 cannot fetch"), "the network is still on:\n{screen}");

    harness.click_text("Next").render();
    harness.click_text("Next").render();
    assert!(harness.screen().contains("may see and reach"), "{}", harness.screen());
    let (x, y) = radio_mark(&harness, "none");
    harness.click(i32::from(x), i32::from(y)).render();
    assert_eq!(harness.app().state.draft().expect("open").network, NetworkMode::None, "{}", harness.screen());
    assert!(harness.screen().contains("context7 cannot fetch documentation"), "{}", harness.screen());

    harness.click_text("Next").render();
    let screen = harness.screen();
    assert!(screen.contains("Build the image"), "{screen}");
    assert!(screen.contains("so the build needs the network"), "said again where the build starts:\n{screen}");
    assert!(screen.contains("context7 cannot fetch documentation"), "{screen}");

    // What the build task writes once the image is there, from the profile the wizard hands it.
    let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
    harness.app().state.store().expect("a store").write_profile(&profile).expect("the file is written");
    let text = std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the file is there");
    assert!(text.contains("template = \"high\""), "{text}");
    let read = crate::profile::Profile::parse("claude-code.toml", &text);
    assert_eq!(read.profile.map(|profile| profile.template), Some(Template::High), "{:?}", read.diagnostics);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn opencode_offers_oh_my_opencode_slim_whose_image_carries_graphify_and_the_slim_team_and_not_oh_my_openagent() {
    // Chosen where the person chooses it: the button that opens the wizard, the harness's
    // row, the button that moves on and the set's own row.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-profiles-slim-{stamp}"));
    let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
    harness.click_text("New profile").render();
    harness.click_text("opencode").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    assert!(harness.screen().contains("The parts the image carries"), "{}", harness.screen());
    harness.click_text("oh my opencode slim").render();
    let screen = harness.screen();
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Slim, "{screen}");
    assert!(screen.contains("oh-my-opencode-slim"), "what it carries is named:\n{screen}");
    // Never both in one profile: the other team's switch is off where this set's is on.
    assert!(switch_on(&mut harness, "oh-my-opencode-slim"), "{screen}");
    assert!(!switch_on(&mut harness, "oh-my-openagent"), "never both:\n{screen}");

    let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
    let files = profile.files();
    let settings = files
        .iter()
        .find(|file| file.path == ".config/opencode/opencode.json")
        .expect("opencode's settings are written");
    assert!(
        settings.contents.contains("file:///usr/local/npm/lib/node_modules/oh-my-opencode-slim"),
        "opencode loads the slim plugin from the image: {}",
        settings.contents
    );
    assert!(!settings.contents.contains("oh-my-openagent"), "{}", settings.contents);
    let own = files
        .iter()
        .find(|file| file.path == ".config/opencode/oh-my-opencode-slim.json")
        .expect("the plugin's own settings are written");
    assert!(own.contents.contains("\"autoUpdate\": false"), "{}", own.contents);
    let recipe = super::recipe::image(&profile);
    assert!(
        recipe.containerfile.contains("npm install -g --cache /tmp/qcode-npm-cache oh-my-opencode-slim"),
        "{}",
        recipe.containerfile
    );
    assert!(!recipe.containerfile.contains("oh-my-openagent"), "{}", recipe.containerfile);
    assert!(recipe.containerfile.contains("graphifyy"), "graphify is installed too: {}", recipe.containerfile);
    assert!(screen.contains("graphify"), "and listed on the page:\n{screen}");

    // Written and read back as the slim set.
    harness.app().state.store().expect("a store").write_profile(&profile).expect("the file is written");
    let name = format!("{}.toml", profile.name);
    let text = std::fs::read_to_string(folder.join("Profiles").join(&name)).expect("the file is there");
    assert!(text.contains("template = \"slim\""), "{text}");
    let read = crate::profile::Profile::parse(&name, &text);
    assert_eq!(read.profile.map(|profile| profile.template), Some(Template::Slim), "{:?}", read.diagnostics);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn oh_my_opencode_slim_is_offered_to_opencode_alone() {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-profiles-slim-claude-{stamp}"));
    let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let screen = harness.screen();
    assert!(screen.contains("The parts the image carries"), "{screen}");
    assert!(screen.contains("QCode extra"), "{screen}");
    assert!(!screen.contains("oh my opencode slim"), "slim is opencode's:\n{screen}");
    let _ = std::fs::remove_dir_all(&folder);
}
