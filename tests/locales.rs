//! The text QCode carries: nine languages, and every one of them whole.
//!
//! English is the file every other is measured against. A key that is added to it and forgotten
//! elsewhere would leave that screen half English for everyone else, so the gate asks for all
//! nine files at once rather than trusting anyone to remember.

use qcode::locales;
use qcode::store::Config;
use qframe::i18n::{Arg, I18n};

/// The languages QCode speaks, and the order the wizard offers them in. It is the list the
/// settings file will hold: a language with a file that the file refuses is one the person can
/// choose and loses at the next start.
const CODES: [&str; 9] = Config::LANGUAGES;

/// QCode's own files on top of the framework's, the way the running application loads them.
fn catalog() -> I18n {
    let mut catalog = I18n::builtin();
    for (file, text) in locales() {
        assert!(catalog.add_source(&file, &text), "{file} could not be read");
    }
    catalog
}

#[test]
fn every_locale_file_is_read_without_a_complaint() {
    let catalog = catalog();
    let diagnostics: Vec<String> = catalog.diagnostics().iter().map(ToString::to_string).collect();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let mut codes: Vec<String> = catalog.list().into_iter().map(|(code, _)| code).collect();
    codes.sort();
    let mut expected: Vec<String> = CODES.iter().map(|code| (*code).to_owned()).collect();
    expected.sort();
    assert_eq!(codes, expected, "the languages offered are the ones with a file");
}

#[test]
fn every_locale_carries_every_key_of_english() {
    let catalog = catalog();
    for (code, _) in catalog.list() {
        assert_eq!(catalog.missing_keys(&code, "en"), Vec::<String>::new(), "locale {code}");
    }
}

#[test]
fn every_language_names_every_language_the_wizard_offers() {
    let mut catalog = catalog();
    for code in CODES {
        catalog.set_active(code);
        for offered in CODES {
            let key = format!("setup.language-{offered}");
            let name = catalog.translate(&key, &[]);
            assert!(!name.starts_with('⟦'), "{code} has no name for {offered}");
            assert!(!name.is_empty(), "{code} leaves {offered} nameless");
        }
    }
}

#[test]
fn the_engine_step_speaks_each_language_in_its_own_words() {
    // Nine files with every key is not nine languages: a file whose values are all English
    // passes the completeness gate above and still leaves the person reading English. What the
    // engine step says about installing is new, so it is checked word by word.
    let mut catalog = catalog();
    let keys = ["install-here", "install-myself", "installing", "install-failed", "never-installs"];
    catalog.set_active("en");
    let english: Vec<String> = keys.iter().map(|key| catalog.translate(&format!("setup.{key}"), &[])).collect();
    for code in CODES.iter().filter(|code| **code != "en") {
        catalog.set_active(code);
        for (key, english) in keys.iter().zip(&english) {
            let said = catalog.translate(&format!("setup.{key}"), &[]);
            assert!(!said.starts_with('⟦') && !said.is_empty(), "{code} has no words for setup.{key}");
            assert_ne!(&said, english, "{code} still says setup.{key} in English");
        }
    }
}

/// Checks that every language other than English says each of `keys` in words of its own: a
/// file whose values were left in English passes the completeness gate and still leaves the
/// person reading English.
fn said_in_each_language(keys: &[&str]) {
    let mut catalog = catalog();
    catalog.set_active("en");
    let english: Vec<String> = keys.iter().map(|key| catalog.translate(key, &[])).collect();
    for code in CODES.iter().filter(|code| **code != "en") {
        catalog.set_active(code);
        for (key, english) in keys.iter().zip(&english) {
            let said = catalog.translate(key, &[]);
            assert!(!said.starts_with('⟦') && !said.is_empty(), "{code} has no words for {key}");
            assert_ne!(&said, english, "{code} still says {key} in English");
        }
    }
}

#[test]
fn what_an_engine_refusal_means_is_said_in_each_language() {
    said_in_each_language(&[
        "known.image-missing",
        "known.not-running",
        "known.no-permission",
        "known.no-id-ranges",
        "known.run",
        "known.said",
    ]);
}

#[test]
fn a_tab_whose_image_is_missing_speaks_each_language() {
    said_in_each_language(&[
        "workspace.image.missing",
        "workspace.image.missing-why",
        "workspace.image.build",
        "workspace.image.cancel",
        "workspace.image.building",
        "workspace.image.stop",
    ]);
}

#[test]
fn what_is_left_after_an_engine_is_installed_is_said_in_each_language() {
    said_in_each_language(&[
        "setup.host.relogin-now",
        "setup.host.relogin-after",
        "setup.host.start-here",
        "setup.host.run-here",
        "setup.host.run-myself",
        "setup.host.run-with",
    ]);
}

/// `switch.home-of` is left out: "{profile} in {workspace}" is German as well as English.
#[test]
fn the_page_that_follows_a_change_of_engine_speaks_each_language() {
    said_in_each_language(&[
        "switch.title",
        "switch.offer",
        "switch.offer-nothing-yet",
        "switch.use",
        "switch.images-none",
        "switch.build",
        "switch.build-all",
        "switch.old-silent",
        "switch.homes-there",
        "switch.login-of",
        "switch.copy",
        "switch.copied",
        "switch.replace-text",
        "switch.leftover",
        "switch.removed",
        "switch.done",
    ]);
}

#[test]
fn the_warning_about_the_key_files_permissions_is_said_in_each_language() {
    // The warning names a place and two modes, so a file that kept the English sentence around
    // them would still pass the completeness gate.
    let mut catalog = catalog();
    let values = [("place", "/home/a/.local/share/quvyta/code"), ("mode", "0755"), ("wanted", "0700")];
    let args: Vec<(&str, Arg)> = values.iter().map(|(name, value)| (*name, Arg::from(*value))).collect();
    catalog.set_active("en");
    let english = catalog.translate("provider.permissions", &args);
    for code in CODES.iter().filter(|code| **code != "en") {
        catalog.set_active(code);
        let said = catalog.translate("provider.permissions", &args);
        assert!(!said.starts_with('⟦') && !said.is_empty(), "{code} has no words for it");
        assert_ne!(said, english, "{code} still says it in English");
        for (_, value) in values {
            assert!(said.contains(value), "{code} loses {value}: {said}");
        }
    }
}

#[test]
fn the_workspace_folder_keeps_its_name_inside_each_languages_own_words() {
    // The folder is called Work on disk in every language, so the name stays; what is around it
    // must be the language's own words, and the container's path must survive translation.
    let mut catalog = catalog();
    let args: Vec<(&str, Arg)> = vec![("assets", Arg::from("x")), ("network", Arg::from("y"))];
    catalog.set_active("en");
    let why = catalog.translate("profiles.wizard.code-why", &[]);
    let mounts = catalog.translate("profiles.mounts", &args);
    for code in CODES {
        catalog.set_active(code);
        let said_why = catalog.translate("profiles.wizard.code-why", &[]);
        let said_mounts = catalog.translate("profiles.mounts", &args);
        assert!(said_why.contains("Work") && said_why.contains("/work"), "{code}: {said_why}");
        assert!(said_mounts.contains("Work"), "{code}: {said_mounts}");
        assert_eq!(catalog.translate("profiles.wizard.permissions-code", &[]), "Work", "{code}");
        if code != "en" {
            assert_ne!(said_why, why, "{code} still says it in English");
            assert_ne!(said_mounts, mounts, "{code} still says it in English");
        }
    }
}

#[test]
fn what_claude_code_assumes_of_an_unmeasured_model_is_said_in_each_language() {
    // The line names a model and a number; a file that kept the English words around them would
    // still pass the completeness gate.
    let mut catalog = catalog();
    let args: Vec<(&str, Arg)> = vec![("model", Arg::from("qwen3.8")), ("tokens", Arg::from("200000"))];
    catalog.set_active("en");
    let english = catalog.translate("profiles.wizard.provider-model-unmeasured", &args);
    for code in CODES.iter().filter(|code| **code != "en") {
        catalog.set_active(code);
        let said = catalog.translate("profiles.wizard.provider-model-unmeasured", &args);
        assert!(!said.starts_with('⟦') && !said.is_empty(), "{code} has no words for it");
        assert_ne!(said, english, "{code} still says it in English");
        for value in ["qwen3.8", "200000", "Claude Code"] {
            assert!(said.contains(value), "{code} loses {value}: {said}");
        }
    }
}

#[test]
fn messages_between_tabs_speak_each_language() {
    said_in_each_language(&[
        "settings.bridge",
        "settings.bridge-ask",
        "settings.bridge-ask-text",
        "bridge.stopped.line",
        "bridge.stopped.seen",
        "bridge.stopped.title",
        "bridge.stopped.why",
    ]);
}
