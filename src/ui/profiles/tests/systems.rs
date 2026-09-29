//! The work that shines while it runs, the systems an image is built on, the parts left out of
//! an image, the lines each language really has, and Antigravity's network.

use super::*;

/// The wizard on its image page with the build under way. A build in the harness would run to
/// its end before the first frame, so the state it stands in while it runs is set here, the way
/// the task leaves it the moment it starts.
fn building(reduced: bool) -> Harness<Host> {
    let mut state = Profiles::new(Some(root()), None).with_providers_file(None);
    drop(update(&mut state, Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })));
    drop(update(&mut state, Msg::New));
    let task = Task::new("build", |_| Ok(Msg::LoginDiscarded));
    if let Some(draft) = state.draft.as_mut() {
        draft.stage = Stage::Image;
        draft.build = Build::Running(task.id());
    }
    let mut harness = Harness::with_env(Host { state }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(reduced).render();
    harness
}

/// Whether the letters of `text` change colour while time passes.
fn shines(harness: &mut Harness<Host>, text: &str) -> bool {
    let colours = |harness: &Harness<Host>| {
        let (x, y) = harness.find(text).unwrap_or_else(|| panic!("`{text}`:\n{}", harness.screen()));
        let (x, y) = (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"));
        let width = u16::try_from(text.chars().count()).expect("short");
        (x..x + width).map(|column| harness.fg(column, y)).collect::<Vec<_>>()
    };
    let mut seen = vec![colours(harness)];
    for _ in 0..8 {
        harness.advance(std::time::Duration::from_millis(120)).render();
        seen.push(colours(harness));
    }
    seen.windows(2).any(|pair| pair[0] != pair[1])
}

/// The wizard on its sign-in page with the login at `login`, a state the work leaves it in
/// only while it runs.
fn signing_in(login: Login) -> Harness<Host> {
    let mut state = Profiles::new(Some(root()), None).with_providers_file(None);
    drop(update(&mut state, Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })));
    drop(update(&mut state, Msg::New));
    if let Some(draft) = state.draft.as_mut() {
        draft.stage = Stage::Login;
        draft.build = Build::Done;
        draft.login = login;
    }
    let mut harness = Harness::with_env(Host { state }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).render();
    harness
}

#[test]
fn the_state_of_every_profile_stands_in_columns_and_the_actions_once_under_the_rows() {
    let free = Profile { account: AccountKind::Free, ..profile("open-free", HarnessKind::OpenCode) };
    let mut harness =
        loaded(vec![profile("claude-main", HarnessKind::ClaudeCode), free, profile("codex-work", HarnessKind::Codex)]);
    let answers = vec![
        Status {
            name: SafeName::parse("claude-main").expect("safe"),
            image: Readiness::Present,
            revision: Revision::Current,
            identity: Readiness::Present,
        },
        Status {
            name: SafeName::parse("open-free").expect("safe"),
            image: Readiness::Present,
            revision: Revision::Current,
            identity: Readiness::Missing,
        },
        Status {
            name: SafeName::parse("codex-work").expect("safe"),
            image: Readiness::Missing,
            revision: Revision::Unknown,
            identity: Readiness::Missing,
        },
    ];
    harness.send(Msg::Probed(answers)).render();
    harness.click_text("claude-main").render();
    let screen = harness.screen();
    let lines: Vec<&str> = screen.lines().collect();
    let row = |name: &str| lines.iter().position(|line| line.contains(name)).expect("the profile's row");
    let column = |line: usize, words: &str| {
        let at = lines[line].find(words).unwrap_or_else(|| panic!("`{words}`:\n{screen}"));
        lines[line][..at].chars().count()
    };
    let (main, free, codex) = (row("claude-main"), row("open-free"), row("codex-work"));
    // Each answer on its profile's own row, and the answers of one kind under each other.
    assert_eq!(column(main, "Image ready"), column(codex, "No image"), "{screen}");
    assert_eq!(column(main, "Image ready"), column(free, "Image ready"), "{screen}");
    assert_eq!(column(main, "Signed in"), column(free, "No sign-in needed"), "{screen}");
    assert_eq!(column(main, "Signed in"), column(codex, "Not signed in"), "{screen}");
    // The actions stand once, one empty row under the last profile.
    assert_eq!(screen.matches("Rebuild image").count(), 1, "{screen}");
    assert_eq!(row("Rebuild image"), codex + 2, "{screen}");
    assert!(lines[row("Rebuild image")].contains("Delete"), "{screen}");
}

#[test]
fn a_login_being_opened_or_stored_shines() {
    let mut opening = signing_in(Login::Opening);
    assert!(shines(&mut opening, "Opening the container"), "{}", opening.screen());
    let mut storing = signing_in(Login::Storing);
    assert!(shines(&mut storing, "Looking for the login"), "{}", storing.screen());
}

#[test]
fn a_build_under_way_shines_and_stands_still_under_reduced_motion() {
    let mut harness = building(false);
    assert!(shines(&mut harness, "Building the image"), "{}", harness.screen());
    let mut still = building(true);
    assert!(!shines(&mut still, "Building the image"), "{}", still.screen());
}

#[test]
fn choosing_arch_where_the_hand_is_saves_it_and_builds_the_image_on_arch_with_pacman() {
    let (folder, mut harness) = recording("arch");
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    let screen = harness.screen();
    assert!(screen.contains("Debian 13, 517 MB"), "every system is offered with its size:\n{screen}");
    assert!(screen.contains("Alpine 3.24, 312 MB, not recommended"), "{screen}");
    assert_eq!(harness.app().state.draft().expect("open").os, Os::Debian, "Debian is chosen until changed");
    harness.click_text("Arch Linux, 809 MB").render();
    assert_eq!(harness.app().state.draft().expect("open").os, Os::Arch, "{}", harness.screen());
    assert!(harness.screen().contains("No sox here"), "what Arch leaves out is said:\n{}", harness.screen());
    harness.click_text("Next").render();
    harness.click_text("QCode extra").render();
    assert!(harness.screen().contains("downloaded from Arch Linux, PyPI"), "{}", harness.screen());
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    // Arriving on the image page is what starts the build.
    harness.click_text("Next").render();
    let build = &harness.app().state.draft().expect("open").build;
    assert!(matches!(build, Build::Done), "the build ended well: {build:?}\n{}", harness.screen());

    let text = std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the profile is saved");
    assert!(text.contains("\nos = \"arch\"\n"), "{text}");
    let saved = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
    assert_eq!(saved.os, Os::Arch);

    let base = std::fs::read_to_string(folder.join("built-qcode-base-arch")).expect("Arch's base image was built");
    assert!(base.starts_with(Os::Arch.containerfile()), "from Arch's own description");
    assert!(!folder.join("built-qcode-base").exists(), "Debian's base is not what this profile needed");
    let image = std::fs::read_to_string(folder.join("built-qcode-profile-claude-code"))
        .expect("the profile's image was built from a Containerfile");
    assert!(image.starts_with("FROM qcode/base-arch\n"), "{image}");
    assert!(image.contains("pacman -Syu --noconfirm --needed python python-pipx"), "{image}");
    assert!(!image.contains("apt-get"), "{image}");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn on_alpine_a_harness_that_does_not_run_there_is_explained_and_never_built() {
    let (folder, mut harness) = recording("alpine");
    harness.click_text("New profile").render();
    harness.click_text("Gemini CLI").render();
    harness.click_text("Next").render();
    harness.click_text("Alpine 3.24, 312 MB, not recommended").render();
    let read = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        read(&harness).contains("Gemini CLI does not run on Alpine 3.24: it would crash on every command."),
        "the reason is said where the system is chosen:\n{}",
        harness.screen()
    );
    assert!(read(&harness).contains("Kept for other work; not recommended."), "{}", harness.screen());
    harness.click_text("Next").render();
    assert_eq!(harness.app().state.draft().expect("open").stage, Stage::System, "it cannot be built there");
    assert!(harness.is_focused("profile-system"), "the focus goes to the choice:\n{}", harness.screen());

    // Back on the first page, the harness that cannot run there says so right under the list.
    harness.click_text("Back").render();
    assert!(read(&harness).contains("Gemini CLI does not run on Alpine 3.24"), "{}", harness.screen());
    harness.click_text("Antigravity IDE").render();
    assert!(read(&harness).contains("its program is built for glibc"), "{}", harness.screen());
    harness.click_text("Codex").render();
    assert!(!read(&harness).contains("does not run on"), "Codex runs on Alpine:\n{}", harness.screen());
    harness.click_text("Next").render();
    assert!(read(&harness).contains("No docx2txt here"), "what Alpine lacks is said:\n{}", harness.screen());
    harness.click_text("Next").render();
    assert_eq!(harness.app().state.draft().expect("open").stage, Stage::Template, "Codex goes on");
    assert!(!folder.join("Profiles").exists(), "nothing was saved on the way");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_plugin_switched_off_in_the_wizard_is_saved_with_the_profile_and_left_out_of_its_image() {
    // Driven from where the person's hand is: the buttons, the set's row, the switch
    // itself, the key on it, and the build button.
    let (folder, mut harness) = recording("switch");
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    harness.click_text("QCode extra").render();

    // Every part of QCode extra is on until it is switched off, and the picker says so.
    let hookify = Extra::parse("hookify").expect("a part of QCode extra");
    let has = |harness: &Harness<Host>, extra| harness.app().state.draft().expect("open").has(extra);
    for part in Template::High.parts(HarnessKind::ClaudeCode) {
        assert!(has(&harness, part), "{part:?} is on until switched off:\n{}", harness.screen());
        assert!(part_on(&mut harness, part), "{part:?} reads on:\n{}", harness.screen());
    }
    let (x, y) = switch_of(&mut harness, "hookify");
    harness.click(x, y).render();
    assert!(!has(&harness, hookify), "a click switches it off:\n{}", harness.screen());
    assert!(!switch_on(&mut harness, "hookify"), "and the switch reads off:\n{}", harness.screen());
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Custom, "one part off is nobody's set");
    // The keys reach the same switch: the list has the row the click was on.
    harness.press("space").render();
    assert!(has(&harness, hookify), "space switches it back on:\n{}", harness.screen());
    harness.press("space").render();
    assert!(!has(&harness, hookify), "and off again:\n{}", harness.screen());
    for part in Template::High.parts(HarnessKind::ClaudeCode).into_iter().filter(|part| *part != hookify) {
        assert!(has(&harness, part), "{part:?} stays on");
    }

    harness.click_text("Next").render();
    harness.click_text("Next").render();
    // Arriving on the image page is what starts the build.
    harness.click_text("Next").render();
    let build = &harness.app().state.draft().expect("open").build;
    assert!(matches!(build, Build::Done), "the build ended well: {build:?}\n{}", harness.screen());

    // QCode extra with one plugin off is written as that, which every QCode builds the same.
    let text = std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the profile is saved");
    assert!(text.contains("template = \"high\""), "{text}");
    assert!(text.contains("[additions]\nhookify = false\n"), "{text}");
    let saved = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
    assert_eq!(saved.without, [hookify], "{text}");

    let image = std::fs::read_to_string(folder.join("built-qcode-profile-claude-code"))
        .expect("the profile's image was built from a Containerfile");
    assert!(!image.contains("hookify"), "{image}");
    for plugin in crate::profile::CLAUDE_EXTRA_PLUGINS.iter().filter(|plugin| !plugin.starts_with("hookify@")) {
        assert!(image.contains(&format!("claude plugin install {plugin}")), "{plugin}: {image}");
    }
    assert!(image.contains("pipx install --global graphifyy"), "graphify stays: {image}");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn quvyta_development_is_offered_and_saved_with_chromium_switched_off_and_built_with_rust() {
    // From where the person's hand is: the buttons, the set's row, the switch.
    let (folder, mut harness) = recording("quvyta-dev");
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    harness.click_text("Quvyta development").render();
    let screen = harness.screen();
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::QuvytaDev, "{screen}");
    let (rust_x, rust_y) = switch_of(&mut harness, "Rust toolchain");
    harness.hover(rust_x, rust_y).advance(std::time::Duration::from_secs(1)).render();
    assert!(
        harness.screen().contains("rustup's stable Rust"),
        "what it adds is said on the switch, under the hand:\n{}",
        harness.screen()
    );
    assert!(screen.contains("GitHub, rustup.rs and"), "and where it comes from:\n{screen}");
    for part in ["graphify", "superpowers", "chromium"] {
        assert!(screen.contains(part), "`{part}` is listed:\n{screen}");
    }
    let has = |harness: &Harness<Host>| harness.app().state.draft().expect("open").has(Extra::Chromium);
    assert!(has(&harness), "on until switched off");
    assert!(switch_on(&mut harness, "chromium"), "and the switch reads on:\n{screen}");
    let (x, y) = switch_of(&mut harness, "chromium");
    harness.click(x, y).render();
    assert!(!has(&harness), "a click switches it off:\n{}", harness.screen());
    assert!(!switch_on(&mut harness, "chromium"), "and the switch reads off:\n{}", harness.screen());
    assert_eq!(
        harness.app().state.draft().expect("open").preset(),
        Template::Custom,
        "Quvyta development without its browser is nobody's set:\n{}",
        harness.screen()
    );

    harness.click_text("Next").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let build = &harness.app().state.draft().expect("open").build;
    assert!(matches!(build, Build::Done), "the build ended well: {build:?}\n{}", harness.screen());

    // Quvyta development without its browser is written as that, as every QCode reads it.
    let text = std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the profile is saved");
    assert!(text.contains("template = \"quvyta-dev\""), "{text}");
    assert!(text.contains("[additions]\nchromium = false\n"), "{text}");
    let saved = crate::profile::Profile::parse("claude-code.toml", &text).profile.expect("it reads back");
    assert_eq!((saved.template, saved.without), (Template::QuvytaDev, vec![Extra::Chromium]), "{text}");
    let image = std::fs::read_to_string(folder.join("built-qcode-profile-claude-code"))
        .expect("the profile's image was built from a Containerfile");
    assert!(image.contains("https://sh.rustup.rs"), "{image}");
    assert!(!image.contains("chromium"), "{image}");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn on_ubuntu_the_chromium_switch_is_there_and_cannot_move_and_the_page_says_why_on_it() {
    let (folder, mut harness) = recording("quvyta-dev-ubuntu");
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Ubuntu 24.04 LTS").render();
    harness.click_text("Next").render();
    let stage = |harness: &Harness<Host>| harness.app().state.draft().expect("open").stage;
    assert_eq!(stage(&harness), Stage::Template, "{}", harness.screen());
    harness.click_text("Quvyta development").render();
    let screen = harness.screen();
    // Quvyta development's browser cannot be carried there, so its switch is off and refuses
    // to move; the rest of the set is on, and the picker still reads the set.
    assert!(!switch_on(&mut harness, "chromium"), "{screen}");
    assert!(switch_on(&mut harness, "Rust toolchain"), "the rest of the set is on:\n{screen}");
    assert_eq!(harness.app().state.draft().expect("open").preset(), Template::QuvytaDev, "{screen}");
    let (x, y) = switch_of(&mut harness, "chromium");
    harness.click(x, y).render();
    assert!(!switch_on(&mut harness, "chromium"), "a click cannot move a refused switch:\n{}", harness.screen());

    // The reason is on the switch itself, said where the hand is, not in a paragraph.
    harness.hover(x, y).advance(std::time::Duration::from_secs(1)).render();
    assert!(harness.screen().contains("only as a snap"), "the reason is under the pointer:\n{}", harness.screen());

    harness.click_text("Next").render();
    assert_eq!(
        stage(&harness),
        Stage::Account,
        "the page is left for a profile that can be built:\n{}",
        harness.screen()
    );
    let profile = harness.app().state.draft().and_then(Draft::profile).expect("a profile");
    assert!(!profile.has(Extra::Chromium), "the file asks for no browser");
    let read = crate::profile::Profile::parse("claude-code.toml", &profile.to_toml());
    assert_eq!(read.diagnostics, [], "which the loader reads without a word of complaint");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn what_a_part_is_shows_under_the_keyboards_row_and_the_space_bar_switches_it() {
    let (folder, mut harness) = recording("hint-keys");
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Ubuntu 24.04 LTS").render();
    harness.click_text("Next").render();
    harness.hover(0, 0).render();
    for _ in 0..8 {
        if harness.is_focused("profile-extras") {
            break;
        }
        harness.press("tab").render();
    }
    assert!(harness.is_focused("profile-extras"), "{}", harness.screen());
    let graphify = "A map of the code";
    let rust = "rustup's stable Rust";
    let screen = harness.screen();
    assert!(screen.contains(graphify), "the first row says what it is at once:\n{screen}");
    assert!(!screen.contains(rust), "and only that row:\n{screen}");

    harness.press("down").render();
    let screen = harness.screen();
    assert!(screen.contains(rust), "the next row says what it is:\n{screen}");
    assert!(!screen.contains(graphify), "the row left says nothing more:\n{screen}");
    let has = |harness: &Harness<Host>, part| harness.app().state.draft().expect("open").has(part);
    let was = has(&harness, Extra::Rust);
    harness.press("space").render();
    assert_ne!(has(&harness, Extra::Rust), was, "the space bar switches the row's part:\n{}", harness.screen());
    assert_ne!(part_on(&mut harness, Extra::Rust), was, "and its switch reads so:\n{}", harness.screen());

    // Chromium cannot be had on Ubuntu: the keys still reach its row, which says why, and the
    // space bar moves nothing there.
    harness.press("down").render();
    assert!(harness.screen().contains("only as a snap"), "the reason is said to the keyboard:\n{}", harness.screen());
    harness.press("space").render();
    assert!(!has(&harness, Extra::Chromium), "a refused part stays off:\n{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_whole_list_of_a_profile_fits_a_screen_of_forty_rows_each_with_its_switch() {
    let mut harness = wizard_on(2);
    harness.resize(100, 40).render();
    harness.click_text("QCode extra").render();
    let screen = harness.screen();
    // Every part Claude Code can have, all sixteen plugins among them, is on the page at once.
    for part in Template::available(HarnessKind::ClaudeCode) {
        let label = row_of(&harness, part);
        assert!(screen.contains(&label), "`{label}` is not listed:\n{screen}");
        let has = |harness: &Harness<Host>| harness.app().state.draft().expect("open").has(part);
        let was = has(&harness);
        let (x, y) = part_switch(&mut harness, part);
        harness.click(x, y).render();
        assert_ne!(has(&harness), was, "{label}: a click moves it:\n{}", harness.screen());
        assert_eq!(harness.app().state.draft().expect("open").preset(), Template::Custom, "{label}");
        harness.click(x, y).render();
        assert_eq!(has(&harness), was, "{label}: and a second moves it back");
        let preset = harness.app().state.draft().expect("open").preset();
        assert_eq!(preset, Template::High, "{label}: and the set reads as itself again");
    }
    assert!(screen.contains("needs the network"), "{screen}");
    assert!(screen.contains("Next"), "the buttons are still on screen:\n{screen}");
}

#[test]
fn opencode_under_every_set_has_its_own_team_of_agents_and_neither_of_claude_codes_plugins() {
    for row in ["QCode recommended", "QCode extra"] {
        let mut harness = wizard_on(0);
        harness.click_text("opencode").render();
        harness.click_text("Next").render();
        harness.click_text("Next").render();
        harness.click_text(row).render();
        let screen = harness.screen();
        assert!(screen.contains("oh-my-openagent"), "{row}: {screen}");
        assert!(screen.contains("graphify"), "{row}: {screen}");
        assert!(!screen.contains("superpowers"), "{row}: {screen}");
        assert!(switch_on(&mut harness, "oh-my-openagent"), "{row}: the team is on:\n{screen}");
        assert!(!switch_on(&mut harness, "oh-my-opencode-slim"), "{row}: and never both:\n{screen}");
        assert!(screen.contains("npm and GitHub"), "{row}: the download is said: {screen}");
    }
}

#[test]
fn a_build_of_a_qcode_template_that_could_not_download_says_whose_it_was() {
    for (row, failed, name) in [
        ("QCode extra", recipe::EXTRA_FAILED, "QCode extra"),
        ("QCode recommended", recipe::RECOMMENDED_FAILED, "QCode recommended"),
    ] {
        let mut harness = wizard_on(2);
        harness.click_text(row).render();
        for _ in 0..3 {
            harness.send(Msg::Next);
        }
        let said = format!(
            "{failed} could not install graphify. What {name} adds is downloaded while the image is built, so the build needs the network."
        );
        harness
            .send(Msg::BuildLine(said))
            .send(Msg::BuildEnded(Err(Problem::Refused("exit status 1".to_owned()))))
            .render();
        let screen = harness.screen();
        assert!(screen.contains(&format!("{name} could not download what it adds")), "{screen}");
    }
    // Any other failure keeps the plain words.
    let mut harness = wizard_on(5);
    harness.send(Msg::BuildEnded(Err(Problem::Refused("no space left".to_owned())))).render();
    assert!(!harness.screen().contains("could not download what it adds"), "{}", harness.screen());
}

#[test]
fn the_lines_of_the_system_page_are_really_in_every_language() {
    // As for QCode high's lines: every key being there says nothing about the language it is
    // in. With the names of the systems and the tools taken out, most of what each language
    // says must be words of its own.
    let keys = [
        "profiles.wizard.step-system",
        "profiles.wizard.system-lead",
        "profiles.wizard.system-option-not-recommended",
        "profiles.wizard.system-debian-detail",
        "profiles.wizard.system-arch-detail",
        "profiles.wizard.system-ubuntu-detail",
        "profiles.wizard.system-alpine-detail",
        "profiles.wizard.system-gap-docx2txt",
        "profiles.wizard.system-gap-sox",
        "profiles.wizard.system-gap-sox-opus",
        "profiles.wizard.system-refused-terminal",
        "profiles.wizard.system-refused-glibc",
        "profiles.wizard.system-refused-window",
        "profiles.summary-system",
    ];
    let names = [
        "Debian",
        "Arch",
        "Ubuntu",
        "Alpine",
        "Linux",
        "glibc",
        "musl",
        "docx2txt",
        "sox",
        "ffmpeg",
        "opus",
        "Word",
        "Node",
        "Node-Projekt",
        "Gemini",
        "CLI",
        "Antigravity",
        "IDE",
        "MB",
        "Mo",
        "МБ",
        "System",
        "{harness}",
        "{system}",
        "{name}",
        "{mb}",
        "{summary}",
        "24.04",
        "24",
    ];
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.' && c != '{' && c != '}')
            .map(|word| word.trim_matches('.').to_lowercase())
            .filter(|word| !word.is_empty() && word.chars().any(char::is_alphabetic))
            .filter(|word| !names.iter().any(|name| name.to_lowercase() == *word))
            .collect()
    };
    let mut catalog = qframe::i18n::I18n::builtin();
    for (file, text) in crate::locales() {
        catalog.add_source(&file, &text);
    }
    catalog.set_active("en");
    let english: Vec<(String, Vec<String>)> =
        keys.iter().map(|key| (catalog.translate(key, &[]), words(&catalog.translate(key, &[])))).collect();
    for code in crate::store::Config::LANGUAGES.into_iter().filter(|code| *code != "en") {
        catalog.set_active(code);
        for (key, (english_text, english_words)) in keys.iter().zip(&english) {
            let text = catalog.translate(key, &[]);
            assert!(!text.starts_with('⟦') && !text.is_empty(), "{code} has no words for `{key}`");
            // The step's name is one word, and German calls it what English does.
            if *key == "profiles.wizard.step-system" {
                continue;
            }
            assert_ne!(&text, english_text, "{code}: `{key}` is the English line");
            let own = words(&text);
            let borrowed = own.iter().filter(|word| english_words.contains(word)).count();
            assert!(borrowed * 2 < own.len().max(1), "{code}: `{key}` reads as English: {text}");
        }
    }
}

#[test]
fn the_lines_of_the_template_parts_are_really_in_every_language() {
    // A file whose values are English still has every key. What each language is checked for
    // is its own words: with the names of the tools taken out, most of what is left must not
    // be words of the English line.
    let keys = [
        "profiles.wizard.high-graphify",
        "profiles.wizard.high-omo",
        "profiles.wizard.high-offline-claude",
        "profiles.wizard.high-offline-opencode",
        "profiles.wizard.high-offline",
    ];
    let names = [
        "QCode",
        "basic",
        "high",
        "graphify",
        "Claude",
        "Code",
        "opencode",
        "oh-my-openagent",
        "context7",
        "grep.app",
        "Python",
        "Debian",
        "PyPI",
        "npm",
        "GitHub",
        "MB",
        "{plugins}",
        "{system}",
    ];
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.' && c != '{' && c != '}')
            .map(|word| word.trim_matches('.').to_lowercase())
            .filter(|word| !word.is_empty() && word.chars().any(char::is_alphabetic))
            .filter(|word| !names.iter().any(|name| name.to_lowercase() == *word))
            .collect()
    };
    let mut catalog = qframe::i18n::I18n::builtin();
    for (file, text) in crate::locales() {
        catalog.add_source(&file, &text);
    }
    catalog.set_active("en");
    let english: Vec<(String, Vec<String>)> =
        keys.iter().map(|key| (catalog.translate(key, &[]), words(&catalog.translate(key, &[])))).collect();
    for code in crate::store::Config::LANGUAGES.into_iter().filter(|code| *code != "en") {
        catalog.set_active(code);
        for (key, (english_text, english_words)) in keys.iter().zip(&english) {
            let text = catalog.translate(key, &[]);
            assert_ne!(&text, english_text, "{code}: `{key}` is the English line");
            let own = words(&text);
            let borrowed = own.iter().filter(|word| english_words.contains(word)).count();
            assert!(borrowed * 2 < own.len().max(1), "{code}: `{key}` reads as English: {text}");
        }
    }
}

#[test]
fn every_english_key_this_screen_uses_has_a_turkish_one() {
    let missing = env().i18n().missing_keys("tr", "en");
    assert_eq!(missing, Vec::<String>::new());
}

#[test]
fn antigravity_is_not_offered_with_the_network_off_and_the_network_stays_on_for_it() {
    // Every step where the person takes it: the buttons, the rows and the keys.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-profiles-offline-{stamp}"));
    let mut harness = Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), None) }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
    harness.click_text("New profile").render();
    assert!(harness.screen().contains("Antigravity IDE"), "offered while the network is on:\n{}", harness.screen());
    for _ in 0..4 {
        harness.click_text("Next").render();
    }
    assert!(harness.screen().contains("may see and reach"), "{}", harness.screen());
    let (x, y) = radio_mark(&harness, "none");
    harness.click(i32::from(x), i32::from(y)).render();
    assert_eq!(harness.app().state.draft().expect("open").network, NetworkMode::None);

    // Back on the first page, the harness that means nothing offline is gone, and why is said.
    for _ in 0..4 {
        harness.click_text("Back").render();
    }
    let screen = harness.screen();
    assert!(screen.contains("Claude Code") && screen.contains("Qwen Code"), "{screen}");
    assert!(
        screen.contains("Antigravity IDE is not offered: it needs the network"),
        "only the reason names it:\n{screen}"
    );
    assert!(
        !screen.contains(&format!("{}  Antigravity IDE", harness.env().icons().glyph("radio-mark-small"))),
        "{screen}"
    );
    // A person on the keyboard goes to the end of the list and finds the last harness offered.
    let (x, y) = radio_mark(&harness, "Claude Code");
    harness.click(i32::from(x), i32::from(y)).render();
    harness.press("end");
    harness.render();
    assert_eq!(harness.app().state.draft().expect("open").harness, HarnessKind::QwenCode, "{}", harness.screen());
    let (x, y) = radio_mark(&harness, "Claude Code");
    harness.click(i32::from(x), i32::from(y)).render();

    // With the network on again it is offered, and once chosen the network cannot be turned off.
    for _ in 0..4 {
        harness.click_text("Next").render();
    }
    let (x, y) = radio_mark(&harness, "full");
    harness.click(i32::from(x), i32::from(y)).render();
    for _ in 0..4 {
        harness.click_text("Back").render();
    }
    let (x, y) = radio_mark(&harness, "Antigravity IDE");
    harness.click(i32::from(x), i32::from(y)).render();
    assert_eq!(harness.app().state.draft().expect("open").harness, HarnessKind::AntigravityIde);
    for _ in 0..4 {
        harness.click_text("Next").render();
    }
    let screen = harness.screen();
    assert!(screen.contains("may see and reach"), "{screen}");
    assert!(screen.contains("Antigravity IDE signs in and works over the network"), "{screen}");
    let square = harness.env().icons().glyph("radio-mark-small").into_owned();
    assert!(!screen.contains(&format!("{square}  none")), "no network is not offered:\n{screen}");
    let (x, y) = radio_mark(&harness, "full");
    harness.click(i32::from(x), i32::from(y)).render();
    harness.press("right");
    harness.press("end");
    harness.render();
    assert_eq!(harness.app().state.draft().expect("open").network, NetworkMode::Full, "{}", harness.screen());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_profile_file_that_pairs_antigravity_with_no_network_is_held_on_the_permissions_page() {
    let mut profile = profile("anti", HarnessKind::AntigravityIde);
    profile.network = NetworkMode::None;
    let mut draft = Draft::new(Vec::<String>::new(), Vec::new());
    draft.choose_harness(profile.harness);
    draft.network = profile.network;
    draft.stage = Stage::Permissions;
    assert_eq!(draft.blocked(), Some(Blocked::NeedsNetwork));
    draft.network = NetworkMode::Full;
    assert_eq!(draft.blocked(), None);
}
