//! The text QCode carries: nine languages, and every one of them whole.
//!
//! English is the file every other is measured against. A key that is added to it and forgotten
//! elsewhere would leave that screen half English for everyone else, so the gate asks for all
//! nine files at once rather than trusting anyone to remember.

use qcode::locales;
use qcode::store::Config;
use qframe::i18n::I18n;

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
