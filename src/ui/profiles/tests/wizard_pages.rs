//! The wizard's pages in order: the name, the account and its provider, the permissions, the
//! image and its build, and the sign-in, finished or not.

use qframe::event::MouseKind;

use super::*;

#[test]
fn the_wizard_walks_through_its_seven_pages_in_order() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New).render();
    let screen = harness.screen();
    assert!(screen.contains("Harness"), "the first step names itself:\n{screen}");
    assert!(screen.contains("Claude Code") && screen.contains("Codex"), "{screen}");
    assert!(screen.contains("Name"), "{screen}");
    for expected in [
        "The system this profile",
        "The parts the image carries",
        "What this profile signs in with",
        "may see and reach",
        "Build the image",
    ] {
        harness.send(Msg::Next).render();
        assert!(harness.screen().contains(expected), "`{expected}` is missing:\n{}", harness.screen());
    }
    harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next).render();
    assert!(harness.screen().contains("Open the sign-in terminal"), "{}", harness.screen());
    assert_eq!(harness.app().state.draft().expect("the wizard is open").stage, Stage::Login);
}

/// A provider entry with `models` already known, the way the Providers page leaves it once
/// someone has listed them.
fn provider_entry(tag: &str, models: &[&str]) -> ProviderEntry {
    let mut entry = ProviderEntry::new(
        crate::provider::Tag::parse(tag).expect("a tag"),
        crate::provider::ProviderKind::Ollama,
        "http://127.0.0.1:11434",
    );
    entry.models = models.iter().map(|id| crate::provider::Model::new(*id)).collect();
    entry
}

#[test]
fn pressing_the_providers_own_row_and_its_models_row_makes_a_profile_that_runs_on_it() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::ProvidersLoaded(vec![provider_entry("ev1", &["qwen3.8", "qwen3.8-32k"])]));
    harness.send(Msg::New).render();
    // Claude Code is the harness already offered first; System, Template and Account are next.
    harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
    assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
    // Pressed exactly where the person would: the row that names the account, then the rows
    // that name the provider and its model, never the message sent by hand.
    harness.click_text("a provider of your own").render();
    assert!(harness.screen().contains("ev1"), "the person's own provider is offered:\n{}", harness.screen());
    harness.click_text("ev1").render();
    assert!(harness.screen().contains("qwen3.8-32k"), "its models are offered:\n{}", harness.screen());
    harness.click_text("qwen3.8-32k").render();

    let draft = harness.app().state.draft().expect("the wizard is open");
    assert_eq!(draft.provider_tag.as_deref(), Some("ev1"));
    assert_eq!(draft.chosen_model().map(String::as_str), Some("qwen3.8-32k"));
    let profile = draft.profile().expect("a provider and a model are both chosen");
    let provider = profile.provider.expect("a provider profile carries one");
    assert_eq!(provider.tag, "ev1");
    assert_eq!(provider.asked(), "qwen3.8-32k");
}

/// A provider entry with `count` models, the way a service that lists hundreds leaves it:
/// `vendor/model-000` … `vendor/model-299`.
fn many_models(tag: &str, count: usize) -> ProviderEntry {
    let mut entry = ProviderEntry::new(
        crate::provider::Tag::parse(tag).expect("a tag"),
        crate::provider::ProviderKind::OpenRouter,
        "https://openrouter.ai/api/v1",
    );
    entry.models = (0..count).map(|n| crate::provider::Model::new(format!("vendor/model-{n:03}"))).collect();
    entry
}

/// The model the wizard has chosen, read off its draft.
fn chosen_model(harness: &Harness<Host>) -> Option<String> {
    harness.app().state.draft()?.chosen_model().cloned()
}

/// The wizard walked to the account page with the account row for a provider of one's own
/// pressed, the way a person walks to it: three pages on, then the row.
fn at_the_account_page(harness: &mut Harness<Host>, count: usize) {
    harness.send(Msg::ProvidersLoaded(vec![many_models("openrouter", count)]));
    harness.send(Msg::New).render();
    // Claude Code is the harness offered first; System, Template and Account are next.
    for _ in 0..3 {
        next(harness);
    }
    assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
    harness.click_text("a provider of your own").render();
}

/// The provider's own row pressed, which is what brings up the models it offers.
fn pick_the_provider(harness: &mut Harness<Host>) {
    harness.click_text("openrouter").render();
    assert!(harness.screen().contains("vendor/model-000"), "its models are offered:\n{}", harness.screen());
}

/// Puts the keyboard on the control named `id`, the way a person walks to it with Tab.
fn focus(harness: &mut Harness<Host>, id: &str) {
    for _ in 0..10 {
        if harness.is_focused(id) {
            return;
        }
        harness.press("tab");
    }
    assert!(harness.is_focused(id), "the keyboard never reached `{id}`:\n{}", harness.screen());
}

#[test]
fn a_provider_with_more_models_than_a_page_holds_scrolls_and_filters_and_the_wizard_goes_on() {
    let mut harness = loaded(Vec::new());
    at_the_account_page(&mut harness, 300);
    // A terminal of a laptop, on a service that offers more models than any page could hold.
    harness.resize(80, 24).render();
    pick_the_provider(&mut harness);
    assert!(harness.find("Next").is_some(), "the way on is on the screen:\n{}", harness.screen());
    let screen = harness.screen();
    assert!(!screen.contains("vendor/model-030"), "so the list is nowhere near its end:\n{screen}");

    // The wheel over the list moves it, as the wheel moves every list in QCode. How many rows a
    // page holds is the page's business: what is checked is that the list goes down and comes back.
    let (x, y) = harness.find("vendor/model-000").expect("the first model is drawn");
    let top = harness.screen();
    for _ in 0..8 {
        harness.mouse(MouseKind::ScrollDown, x, y);
    }
    let down = harness.screen();
    assert_ne!(down, top, "the wheel takes the list down:\n{down}");
    for _ in 0..8 {
        harness.mouse(MouseKind::ScrollUp, x, y);
    }
    assert_eq!(harness.screen(), top, "and the wheel takes it back:\n{}", harness.screen());

    // And the arrow keys move it too, from the top of the list, with the keyboard on it.
    focus(&mut harness, "profile-provider-model");
    for _ in 0..30 {
        harness.press("down");
    }
    let screen = harness.screen();
    assert!(screen.contains("vendor/model-030"), "the down key takes it there too:\n{screen}");
    assert_eq!(chosen_model(&harness).as_deref(), Some("vendor/model-030"), "and the row it passed chooses");

    // The filter is how a list of hundreds becomes a list of one.
    harness.click_text("Filter").render();
    harness.type_text("model-287");
    let screen = harness.screen();
    assert!(screen.contains("vendor/model-287"), "the model asked for is the one shown:\n{screen}");
    harness.click_text("vendor/model-287").render();
    assert_eq!(chosen_model(&harness).as_deref(), Some("vendor/model-287"), "its row chooses it");

    harness.click_text("Next").render();
    let screen = harness.screen();
    assert!(screen.contains("may see and reach"), "the wizard goes on:\n{screen}");
    let profile = harness.app().state.draft().expect("the wizard is open").profile().expect("a profile");
    let provider = profile.provider.expect("a provider profile carries one");
    assert_eq!((provider.tag.as_str(), provider.asked()), ("openrouter", "vendor/model-287"));
}

#[test]
fn a_model_at_the_end_of_a_long_list_is_chosen_with_the_keys_once_the_filter_finds_it() {
    let mut harness = loaded(Vec::new());
    at_the_account_page(&mut harness, 300);
    pick_the_provider(&mut harness);
    assert_eq!(chosen_model(&harness).as_deref(), Some("vendor/model-000"), "the first model comes with the provider");

    harness.click_text("Filter").render();
    harness.type_text("model-287");
    assert!(harness.screen().contains("vendor/model-287"), "{}", harness.screen());
    focus(&mut harness, "profile-provider-model");
    harness.press("down");
    harness.press("enter");
    assert_eq!(chosen_model(&harness).as_deref(), Some("vendor/model-287"), "the keys choose the row they are on");

    // The filter lets go of the model and the choice is still there, on the last row of the list.
    harness.press("shift+tab");
    for _ in 0..12 {
        harness.press("backspace");
    }
    let chosen = harness.screen().lines().find(|line| line.contains("vendor/model-287")).unwrap_or_default().to_owned();
    assert!(chosen.starts_with('▌'), "the chosen model is the row the list stands on:\n{}", harness.screen());
    assert_eq!(chosen_model(&harness).as_deref(), Some("vendor/model-287"), "and the draft has it");

    // A filter that leaves nothing says so, and takes the choice with it.
    harness.type_text("zzz");
    let screen = harness.screen();
    assert!(screen.contains("Nothing of this provider matches that"), "{screen}");
    assert_eq!(chosen_model(&harness).as_deref(), Some("vendor/model-287"), "a filter does not unchoose");
}

#[test]
fn a_long_model_list_and_the_way_on_both_fit_a_terminal_twice_as_wide_as_the_page() {
    let mut harness = loaded(Vec::new());
    at_the_account_page(&mut harness, 300);
    harness.resize(200, 50).render();
    pick_the_provider(&mut harness);
    let screen = harness.screen();
    assert!(screen.contains("vendor/model-000") && screen.contains("vendor/model-010"), "the list is drawn:\n{screen}");
    assert!(screen.contains("What this profile signs in with"), "the page keeps its own head:\n{screen}");
    assert!(harness.find("Next").is_some(), "the way on is on the screen:\n{screen}");
}

#[test]
fn a_model_whose_window_was_never_measured_says_what_claude_code_will_assume() {
    let mut entry = provider_entry("ev1", &["qwen3.8", "qwen3.8-32k"]);
    entry.models[1].measured = Some(crate::provider::Measured::About(31_512));
    let mut harness = loaded(Vec::new());
    // What the providers file holds, delivered the way the screen reads it.
    harness.send(Msg::ProvidersLoaded(vec![entry])).render();
    harness.click_text("New profile").render();
    next(&mut harness);
    next(&mut harness);
    next(&mut harness);
    harness.click_text("a provider of your own").render();
    harness.click_text("ev1").render();
    // Read across the rows the sentence wraps onto, the way a person reads it.
    let read = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    let said = "Claude Code assumes 200000 tokens for qwen3.8 until it is measured on the Providers page.";
    assert!(read(&harness).contains(said), "the first model was never measured:\n{}", harness.screen());
    harness.click_text("qwen3.8-32k").render();
    let screen = harness.screen();
    assert!(screen.contains("31512"), "the row of a measured model carries its window:\n{screen}");
    assert!(!screen.contains("will assume"), "a measured window is handed over, so nothing is said:\n{screen}");
    harness.click_text("qwen3.8").render();
    harness.set_locale("tr").render();
    assert!(read(&harness).contains("Claude Code, qwen3.8 için 200000 belirteç varsayar"), "{}", harness.screen());
}

#[test]
fn with_no_provider_added_the_account_page_says_so_and_offers_no_empty_list() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New).render();
    harness.send(Msg::Next).send(Msg::Next).send(Msg::Next).render();
    harness.click_text("a provider of your own").render();
    let screen = harness.screen();
    assert!(screen.contains("No provider yet. Add one on the Providers page"), "{screen}");
    assert!(!screen.contains("qwen"), "nothing borrowed from nowhere is offered:\n{screen}");
    harness.click_text("Go to Providers").render();
    // The application, not this screen, opens the Providers page; this screen only asked.
    assert!(harness.app().state.draft().is_some(), "the wizard itself is not closed by asking");
}

/// A provider entry with the lineup `lineup` of `steps` in the given order, the second of which
/// costs money, and `models` as its own models: what the Providers page leaves a provider it has
/// been asked about.
fn lineup_entry(tag: &str, lineup: &str, steps: &[(&str, bool)], models: &[&str]) -> ProviderEntry {
    let mut entry = ProviderEntry::new(
        crate::provider::Tag::parse(tag).expect("a tag"),
        crate::provider::ProviderKind::OpenRouter,
        "https://openrouter.ai/api/v1",
    );
    entry.models = steps
        .iter()
        .map(|(id, paid)| {
            let mut model = crate::provider::Model::new(*id);
            model.price = Some(if *paid { crate::provider::Price::Paid } else { crate::provider::Price::Free });
            model
        })
        .chain(models.iter().map(|id| {
            let mut model = crate::provider::Model::new(*id);
            model.price = Some(crate::provider::Price::Free);
            model
        }))
        .collect();
    entry.lineups = vec![crate::provider::Lineup {
        name: crate::provider::Tag::parse(lineup).expect("a name"),
        models: steps.iter().map(|(id, _)| (*id).to_owned()).collect(),
    }];
    entry
}

/// The pick the wizard has chosen, read off its draft, as the pair the profile will be written as.
fn chosen_pick(harness: &Harness<Host>) -> Option<crate::profile::Pick> {
    harness.app().state.draft()?.provider_pick.clone()
}

/// The wizard walked to the account page of a provider `entry`, with the account row pressed and
/// the provider's own row after it: three pages on, then the two rows, the way a person walks
/// there.
fn at_the_lineup(harness: &mut Harness<Host>, entry: ProviderEntry) {
    harness.send(Msg::ProvidersLoaded(vec![entry]));
    harness.send(Msg::New).render();
    for _ in 0..3 {
        next(harness);
    }
    assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
    harness.click_text("a provider of your own").render();
    harness.click_text("yol").render();
}

#[test]
fn a_lineup_is_chosen_from_the_account_page_and_the_profile_file_names_it_and_not_a_model() {
    // The whole of it, from the person's own account row to the file on the disk: the lineup comes
    // chosen with its provider, the warning about the step that costs is said while it is being
    // chosen, and a model chosen and then a lineup again reaches the file as `lineup`.
    let (folder, mut harness) = recording("lineup-profile");
    at_the_lineup(
        &mut harness,
        lineup_entry(
            "yol",
            "coder",
            &[("a/one:free", false), ("b/two", true)],
            &["c/three", "d/four", "e/five", "f/six", "g/seven"],
        ),
    );

    let screen = harness.screen();
    assert!(screen.contains("Lineups") && screen.contains("Models"), "the two sections are named:\n{screen}");
    assert!(screen.contains("a/one:free → b/two"), "the row carries the steps in order:\n{screen}");
    assert_eq!(
        chosen_pick(&harness),
        Some(crate::profile::Pick::Lineup("coder".to_owned())),
        "the first lineup comes with the provider, before any row is pressed"
    );
    // Read across the rows the sentences wrap onto, the way a person reads them. Nothing about
    // money is said without being asked for it, and this is where it is asked for.
    let read = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(read.contains("This costs money: b/two."), "the step that costs is named:\n{screen}");
    assert!(read.contains("paid from your OpenRouter balance."), "and what landing on it spends:\n{screen}");

    // A model is chosen instead: it is one step and nothing to fall back to, so there is nothing
    // left that could cost money.
    harness.click_text("c/three").render();
    assert_eq!(chosen_pick(&harness), Some(crate::profile::Pick::Model("c/three".to_owned())));
    assert!(!harness.screen().contains("This costs money"), "a free model spends nothing:\n{}", harness.screen());

    // And the lineup again, which is what the profile is finished with.
    harness.click_text("coder").render();
    assert_eq!(chosen_pick(&harness), Some(crate::profile::Pick::Lineup("coder".to_owned())));
    assert!(harness.screen().contains("This costs money"), "the warning comes back with it:\n{}", harness.screen());

    // Walked to the end and finished, the file on the disk names the lineup and no model.
    for _ in 0..2 {
        next(&mut harness);
    }
    harness.click_text("Finish").render();
    let written =
        std::fs::read_to_string(folder.join("Profiles").join("claude-code.toml")).expect("the profile is written");
    assert!(written.contains("lineup = \"coder\""), "{written}");
    assert!(!written.contains("model ="), "and no model beside it: {written}");
    let read = crate::profile::Profile::parse("claude-code.toml", &written).profile.expect("the file reads");
    assert_eq!(read.provider, Some(crate::profile::ProviderChoice::lineup("yol", "coder")), "{written}");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_lineup_whose_steps_nobody_measured_says_what_claude_code_will_assume_for_each_of_them() {
    let mut harness = loaded(Vec::new());
    at_the_lineup(&mut harness, lineup_entry("yol", "coder", &[("a/one", false), ("b/two", false)], &["c/three"]));
    let read = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    let said = "Claude Code assumes 200000 tokens for a/one, b/two of coder until they are measured";
    assert!(read(&harness).contains(said), "the steps of the lineup nobody measured:\n{}", harness.screen());
    harness.set_locale("tr").render();
    assert!(
        read(&harness).contains("Claude Code, coder sırasındaki a/one, b/two için 200000 belirteç varsayar"),
        "{}",
        harness.screen()
    );
}

#[test]
fn one_filter_leaves_both_a_lineup_and_the_models_around_it() {
    let mut harness = loaded(Vec::new());
    at_the_lineup(
        &mut harness,
        lineup_entry("yol", "coder", &[("a/one", false), ("b/two", false)], &["vendor/model-287", "vendor/model-288"]),
    );
    // Nothing typed: the whole list, both sections of it.
    assert!(
        harness.screen().contains("coder") && harness.screen().contains("vendor/model-287"),
        "{}",
        harness.screen()
    );

    harness.click_text("Filter").render();
    harness.type_text("model-28").render();
    let screen = harness.screen();
    assert!(
        screen.contains("vendor/model-287") && screen.contains("vendor/model-288"),
        "the models it matches:\n{screen}"
    );
    // The arrow is on a lineup's row and nowhere else, so it is what says whether one is drawn.
    assert!(!screen.contains('→'), "and only they, for a lineup is a name of its own: {screen}");

    // The same box finds a lineup by its own name, which is a word of the person's rather than of
    // any model: without it, a lineup they heard of would read as one that is not there.
    for _ in 0..9 {
        harness.press("backspace");
    }
    harness.type_text("cod").render();
    let screen = harness.screen();
    assert!(screen.contains('→'), "the lineup is found by its name:\n{screen}");
    assert!(!screen.contains("vendor/model-287"), "and the models that do not match it are not drawn: {screen}");
    harness.click_text("coder").render();
    assert_eq!(chosen_pick(&harness), Some(crate::profile::Pick::Lineup("coder".to_owned())));

    // A filter that matches nothing says so, and takes the standing choice with it.
    focus(&mut harness, "profile-provider-model-filter");
    for _ in 0..9 {
        harness.press("backspace");
    }
    harness.type_text("zzz").render();
    let screen = harness.screen();
    assert!(screen.contains("Nothing of this provider matches that"), "{screen}");
    assert_eq!(
        chosen_pick(&harness),
        Some(crate::profile::Pick::Lineup("coder".to_owned())),
        "a filter does not unchoose"
    );
}

#[test]
fn opencode_offers_a_provider_of_ones_own_from_the_rows_a_person_presses() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::ProvidersLoaded(vec![provider_entry("ev1", &["qwen3.8", "qwen3-coder:30b"])])).render();
    harness.click_text("New profile").render();
    harness.click_text("opencode").render();
    assert_eq!(harness.app().state.draft().expect("open").harness, HarnessKind::OpenCode);
    next(&mut harness);
    next(&mut harness);
    next(&mut harness);
    assert!(harness.screen().contains("What this profile signs in with"), "{}", harness.screen());
    harness.click_text("a provider of your own").render();
    harness.click_text("ev1").render();
    harness.click_text("qwen3-coder:30b").render();
    // What Claude Code would assume is Claude Code's alone and is not said of opencode.
    assert!(!harness.screen().contains("will assume"), "{}", harness.screen());
    let profile = harness.app().state.draft().expect("open").profile().expect("a provider and a model");
    assert_eq!(profile.harness, HarnessKind::OpenCode);
    assert_eq!(profile.account, AccountKind::Provider);
    let provider = profile.provider.expect("a provider profile carries one");
    assert_eq!((provider.tag.as_str(), provider.asked()), ("ev1", "qwen3-coder:30b"));
}

#[test]
fn the_account_page_offers_each_harness_its_own_accounts_and_a_provider_only_where_one_was_proven() {
    let offered = [
        ("Gemini CLI", &["API key"][..], false),
        ("Codex", &["subscription", "API key"][..], true),
        ("Kimi Code CLI", &["subscription"][..], true),
        ("Qwen Code", &[][..], true),
    ];
    for (name, accounts, provider) in offered {
        let mut harness = loaded(Vec::new());
        harness.click_text("New profile").render();
        harness.click_text(name).render();
        next(&mut harness);
        next(&mut harness);
        next(&mut harness);
        let screen = harness.screen();
        assert!(screen.contains("What this profile signs in with"), "{name}:\n{screen}");
        assert_eq!(screen.contains("a provider of your own"), provider, "{name}:\n{screen}");
        for account in accounts {
            assert!(screen.contains(account), "{name} offers {account}:\n{screen}");
        }
        // A key is only offered where the harness keeps it in a file of its own.
        if !accounts.contains(&"API key") {
            assert!(!screen.contains("API key"), "{name}:\n{screen}");
        }
    }
}

#[test]
fn a_name_that_is_taken_stops_the_wizard_on_the_first_page() {
    let mut harness = loaded(vec![profile("claude-code", HarnessKind::ClaudeCode)]);
    harness.send(Msg::New).send(Msg::Next).render();
    let screen = harness.screen();
    assert!(screen.contains("There is already a profile with this name"), "{screen}");
    assert!(harness.is_focused("profile-name"), "the focus goes to the problem:\n{screen}");
    harness.send(Msg::Name("Günlük İş".to_owned())).render();
    assert!(harness.screen().contains("It is written as gunluk-is"), "{}", harness.screen());
    harness.send(Msg::Next).render();
    assert!(harness.screen().contains("The system this profile"), "{}", harness.screen());
}

#[test]
fn the_permissions_page_says_the_workspace_is_always_writable() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..4 {
        harness.send(Msg::Next);
    }
    harness.render();
    let screen = harness.screen();
    assert!(screen.contains("always writable"), "{screen}");
    assert!(screen.contains("read-only") && screen.contains("writable"), "{screen}");
    assert!(screen.contains("full") && screen.contains("none"), "{screen}");
}

#[test]
fn the_workspace_folder_is_called_work_in_the_list_and_on_the_permissions_page() {
    // The folder is `Work/` on disk and `/work` in the container; a screen that still calls
    // it Code sends the person looking for a folder that is not there.
    let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    let screen = harness.screen();
    assert!(screen.contains("Work writable"), "the list names the folder:\n{screen}");
    assert!(!screen.contains("Code writable"), "{screen}");
    harness.click_text("New profile").render();
    for _ in 0..4 {
        next(&mut harness);
    }
    let screen = harness.screen();
    assert!(screen.contains("may see and reach"), "the permissions page is open:\n{screen}");
    assert!(screen.contains("The Work folder, /work in the container"), "{screen}");
    assert!(!screen.contains("code directory"), "{screen}");
    harness.set_locale("tr").render();
    let screen = harness.screen();
    assert!(screen.contains("Work klasörü, kapsayıcıda /work"), "{screen}");
    assert!(!screen.contains("kod klasörü"), "{screen}");
}

#[test]
fn the_image_page_cannot_be_left_until_there_is_an_image() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    harness.render();
    assert!(harness.screen().contains("Build the image"), "{}", harness.screen());
    harness.send(Msg::Next).render();
    assert!(harness.screen().contains("Build the image"), "a page without an image keeps its place");
    harness.send(Msg::BuildEnded(Ok(()))).render();
    assert!(harness.screen().contains("written to the QCode folder"), "{}", harness.screen());
    harness.send(Msg::Next).render();
    assert!(harness.screen().contains("in a container terminal below"), "{}", harness.screen());
}

#[test]
fn a_build_refused_for_a_reason_qcode_recognises_says_it_plainly_above_the_engines_words() {
    let mut harness = with_engine();
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    let said = "Error: cannot find UID/GID for user ada: no subuid ranges found for user \"ada\" in /etc/subuid";
    harness.send(Msg::BuildEnded(Err(Problem::Refused(said.to_owned())))).render();
    let screen = harness.screen();
    assert!(screen.contains("Failed. What the engine said is below"), "{screen}");
    assert!(screen.contains("no user id ranges for your account"), "{screen}");
    assert!(screen.contains("--add-subuids"), "the line that adds them is there to copy:\n{screen}");
    assert!(screen.contains("no subuid ranges found"), "the engine is still quoted:\n{screen}");
}

#[test]
fn a_failed_build_shows_the_engines_own_words_and_leaves_no_image() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    harness.send(Msg::BuildLine("STEP 1/3: FROM qcode/base".to_owned()));
    harness.send(Msg::BuildEnded(Err(Problem::Refused("Error: qcode/base: image not known".to_owned()))));
    harness.render();
    let screen = harness.screen();
    assert!(screen.contains("Failed. What the engine said is below"), "{screen}");
    assert!(screen.contains("image not known"), "the engine is quoted, not summarised:\n{screen}");
    assert!(screen.contains("nothing was left behind"), "{screen}");
    assert!(screen.contains("Build again"), "{screen}");
    harness.send(Msg::Next).render();
    assert!(harness.screen().contains("Build again"), "a failed build is not an image");
}

#[test]
fn a_stopped_build_says_that_nothing_was_written() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    harness.send(Msg::BuildEnded(Err(Problem::Cancelled))).render();
    let screen = harness.screen();
    assert!(screen.contains("Stopped: the half-made image was removed"), "{screen}");
    assert!(screen.contains("nothing was written"), "{screen}");
}

#[test]
fn the_login_page_says_why_a_terminal_opens_what_to_do_and_how_it_is_checked() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next).render();
    let screen = harness.screen();
    assert!(screen.contains("QCode runs Claude Code in a container terminal below"), "why:\n{screen}");
    assert!(screen.contains("Nothing of your machine is in it"), "what it cannot see:\n{screen}");
    assert!(screen.contains("Sign in there as you normally would"), "what to do:\n{screen}");
    assert!(screen.contains("QCode checks inside the container"), "how it is checked:\n{screen}");
    assert!(screen.contains("Open the sign-in terminal"), "{screen}");
}

#[test]
fn a_harness_that_closed_without_a_login_is_never_counted_as_signed_in() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next);
    harness.send(Msg::LoginStored(Err(Problem::NoLogin))).render();
    let screen = harness.screen();
    assert!(screen.contains("The harness left no login"), "{screen}");
    assert!(screen.contains("nothing was stored"), "{screen}");
    assert!(screen.contains("Sign in from the list whenever you like"), "the way back is offered:\n{screen}");
    let draft = harness.app().state.draft().expect("the wizard is open");
    assert!(!draft.login.is_stored());
}

#[test]
fn an_interrupted_login_says_that_nothing_was_kept() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next);
    // The page reached by stopping a sign-in that was under way.
    harness.send(Msg::LoginFailed(Problem::Refused("Error: no such container".to_owned()))).render();
    let screen = harness.screen();
    assert!(screen.contains("The sign-in could not be opened"), "{screen}");
    assert!(screen.contains("no such container"), "{screen}");
    assert!(screen.contains("Try again"), "{screen}");
}

#[test]
fn a_stored_login_says_how_much_was_kept_and_what_happens_to_it() {
    let mut harness = loaded(Vec::new());
    harness.send(Msg::New);
    for _ in 0..5 {
        harness.send(Msg::Next);
    }
    harness.send(Msg::BuildEnded(Ok(()))).send(Msg::Next);
    harness.send(Msg::LoginStored(Ok(1))).render();
    let screen = harness.screen();
    assert!(screen.contains("Signed in. 1 file was stored"), "{screen}");
    assert!(screen.contains("opens this profile from now on gets this login"), "{screen}");
}

#[test]
fn a_profile_with_an_image_can_be_signed_in_again_from_the_list() {
    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing {
        profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        diagnostics: Vec::new(),
    }));
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    harness.send(Msg::SignInAsked).render();
    let screen = harness.screen();
    assert!(screen.contains("Sign claude-sub in"), "{screen}");
    assert!(screen.contains("Open the sign-in terminal"), "{screen}");
    assert!(!screen.contains("Permissions"), "signing in again is not the whole wizard:\n{screen}");
}

#[test]
fn a_gemini_cli_profile_with_a_key_is_told_where_the_key_comes_from_and_what_the_dialog_asks() {
    let mut harness = with_engine();
    let keyed = Profile { account: AccountKind::ApiKey, ..profile("gemini-key", HarnessKind::GeminiCli) };
    harness.send(Msg::Loaded(Listing { profiles: vec![keyed], diagnostics: Vec::new() }));
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    harness.click_text("Sign in").render();
    let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(screen.contains("Gemini CLI signs in with an API key from Google AI Studio"), "{screen}");
    assert!(screen.contains("aistudio.google.com/app/apikey"), "{screen}");
    assert!(screen.contains("Choose Use Gemini API Key, paste the key"), "{screen}");
    assert!(!screen.contains("in a container terminal below"), "{screen}");
}

#[test]
fn a_gemini_cli_profile_made_with_the_closed_sign_in_says_so_where_it_is_listed_and_still_loads() {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-profiles-closed-{stamp}"));
    let signed_in = profile("gemini-sub", HarnessKind::GeminiCli);
    Store::new(&folder).write_profile(&signed_in).expect("the definition is written");
    let state = Profiles::new(Some(folder.clone()), None).with_providers_file(None);
    let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    // The store is read the way the screen always reads it, from the file on the disk.
    harness.send(Msg::Reload).render();
    let _ = std::fs::remove_dir_all(&folder);
    let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(harness.app().state.rows().len(), 1, "the profile still loads:\n{screen}");
    assert!(
        screen.contains("Google closed Gemini CLI's sign-in with a Google account for personal accounts"),
        "{screen}"
    );
    assert!(screen.contains("no longer supported for Gemini Code Assist for individuals"), "{screen}");
    assert!(screen.contains("Standard and Enterprise"), "{screen}");
    assert!(screen.contains("Antigravity IDE"), "{screen}");
    assert!(screen.contains("API key from Google AI Studio"), "{screen}");
    assert!(!screen.contains("could not be read"), "a profile that loaded is not an unreadable file:\n{screen}");
}

#[test]
fn a_gemini_cli_profile_with_a_key_says_nothing_of_a_closed_sign_in() {
    let keyed = Profile { account: AccountKind::ApiKey, ..profile("gemini-key", HarnessKind::GeminiCli) };
    let harness = loaded(vec![keyed]);
    assert!(!harness.screen().contains("Google closed"), "{}", harness.screen());
}

#[test]
fn a_profile_without_an_image_cannot_be_signed_in() {
    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing {
        profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        diagnostics: Vec::new(),
    }));
    answer(&mut harness, Readiness::Missing, Readiness::Missing);
    harness.send(Msg::SignInAsked).render();
    assert!(!harness.screen().contains("Sign claude-sub in"), "{}", harness.screen());
}
