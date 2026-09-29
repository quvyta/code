//! The wizard's pages in order: the name, the account and its provider, the permissions, the
//! image and its build, and the sign-in, finished or not.

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
    assert_eq!(draft.provider_model.as_deref(), Some("qwen3.8-32k"));
    let profile = draft.profile().expect("a provider and a model are both chosen");
    let provider = profile.provider.expect("a provider profile carries one");
    assert_eq!(provider.tag, "ev1");
    assert_eq!(provider.model, "qwen3.8-32k");
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
    assert_eq!((provider.tag.as_str(), provider.model.as_str()), ("ev1", "qwen3-coder:30b"));
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
