//! The text QCode carries: nine languages, and every one of them whole.
//!
//! English is the file every other is measured against. A key that is added to it and forgotten
//! elsewhere would leave that screen half English for everyone else, so the gate asks for all
//! nine files at once rather than trusting anyone to remember.

use std::collections::BTreeMap;

use qcode::locales;
use qcode::store::Config;
use qframe::i18n::{Arg, I18n};
use toml::de::{DeTable, DeValue};

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

/// English values that every language writes the same way, because they are names or units
/// rather than words: a language that kept them is not a language left in English.
const SAME_EVERYWHERE: [&str; 9] = [
    // The container engines and the icon sets are products, called by their names everywhere.
    "Podman",
    "Docker",
    "Nerd Font",
    "Unicode",
    "ASCII",
    // A template named after the package it installs, as its maker spells the package's name.
    "oh my opencode slim",
    // `Work` is the name of the folder on disk, in every language.
    "Work",
    // An address, shown as an example of what to type.
    "https://example.org/team/app.git",
    // A system's name and its size in megabytes.
    "{name}, {mb} MB",
];

/// English words that are also the word a language uses, in that language: keeping them is a
/// translation. Checked against the value, so the same word elsewhere in English still counts.
const SAME_WORD: [(&str, &str); 32] = [
    // German writes these nouns as English does, and "in" is a German word.
    ("de", "Name"),
    ("de", "Engine"),
    ("de", "System"),
    ("de", "Image"),
    ("de", "Agent"),
    ("de", "Shell"),
    ("de", "Text"),
    ("de", "Editor"),
    ("de", "China"),
    ("de", "{profile} in {workspace}"),
    ("de", "{what} in {name}: {reason}"),
    // Spanish.
    ("es", "Panel"),
    ("es", "Editor"),
    ("es", "China"),
    ("es", "5 min"),
    ("es", "15 min"),
    // French.
    ("fr", "Image"),
    ("fr", "Agent"),
    ("fr", "Shell"),
    ("fr", "Europe"),
    ("fr", "5 min"),
    ("fr", "15 min"),
    // Brazilian Portuguese takes these words as they are.
    ("pt-BR", "Shell"),
    ("pt-BR", "Editor"),
    ("pt-BR", "China"),
    ("pt-BR", "Backups"),
    ("pt-BR", "BACKUP"),
    ("pt-BR", "LOGINS"),
    ("pt-BR", "5 min"),
    ("pt-BR", "15 min"),
    // Chinese keeps the English word for a shell; Turkish says "Panel".
    ("zh-Hans", "Shell"),
    ("tr", "Panel"),
];

/// The forms a plural value can have.
const FORMS: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

/// Every value of the locale file `text`, keyed as `section.key`, and a plural's forms as
/// `section.key.form`. The `[meta]` table names the language and is not text of the application.
fn values(text: &str) -> BTreeMap<String, String> {
    fn walk(table: &DeTable<'_>, prefix: &str, out: &mut BTreeMap<String, String>) {
        for (name, value) in table {
            let name = name.get_ref();
            if prefix.is_empty() && name == "meta" {
                continue;
            }
            let key = if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
            match value.get_ref() {
                DeValue::String(text) => {
                    out.insert(key, text.to_string());
                }
                DeValue::Table(inner) if inner.keys().all(|form| FORMS.contains(&form.get_ref().as_ref())) => {
                    for (form, text) in inner {
                        let text =
                            if let DeValue::String(text) = text.get_ref() { text.to_string() } else { String::new() };
                        out.insert(format!("{key}.{}", form.get_ref()), text);
                    }
                }
                DeValue::Table(inner) => walk(inner, &key, out),
                _ => {}
            }
        }
    }
    let table = DeTable::parse(text).expect("a locale file is TOML").into_inner();
    let mut out = BTreeMap::new();
    walk(&table, "", &mut out);
    out
}

/// Whether `text` is nothing but placeholders, spaces and punctuation: `{first} +{more}` says
/// nothing any language could say differently.
fn only_placeholders(text: &str) -> bool {
    let mut inside = false;
    text.chars().all(|character| match character {
        '{' => {
            inside = true;
            true
        }
        '}' => {
            inside = false;
            true
        }
        _ => inside || !character.is_alphanumeric(),
    })
}

/// The key a value of [`values`] belongs to: a plural's form is part of the key it counts for.
fn key_of(path: &str) -> &str {
    FORMS.iter().find_map(|form| path.strip_suffix(&format!(".{form}"))).unwrap_or(path)
}

/// For every language but English, the keys whose value is still one of English's values for that
/// key, with the allowances above. `files` maps a language's code to its file.
fn left_in_english(files: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let english = values(&files["en"]);
    let mut left = BTreeMap::new();
    for code in CODES.iter().filter(|code| **code != "en") {
        let own = values(files.get(*code).unwrap_or_else(|| panic!("{code} has a file")));
        let mut same: Vec<String> = own
            .iter()
            .filter(|(path, text)| {
                let key = key_of(path);
                let english_forms: Vec<&String> =
                    english.iter().filter(|(other, _)| key_of(other) == key).map(|(_, text)| text).collect();
                english_forms.contains(text)
                    && !only_placeholders(text)
                    && !SAME_EVERYWHERE.contains(&text.as_str())
                    && !SAME_WORD.contains(&(*code, text.as_str()))
            })
            .map(|(path, _)| key_of(path).to_owned())
            .collect();
        same.dedup();
        left.insert((*code).to_owned(), same);
    }
    left
}

/// QCode's locale files by the code of their language.
fn files() -> BTreeMap<String, String> {
    locales().into_iter().map(|(file, text)| (file.trim_end_matches(".toml").to_owned(), text)).collect()
}

#[test]
fn no_language_leaves_a_value_in_english() {
    // Nine files with every key is not nine languages: a file whose values are all English passes
    // the completeness gate above and still leaves the person reading English. Every value of
    // every language is measured against English, plural forms included.
    let left = left_in_english(&files());
    let counts: Vec<String> = left.iter().map(|(code, keys)| format!("{code}: {}", keys.len())).collect();
    assert!(left.values().all(Vec::is_empty), "still in English, {}:\n{left:#?}", counts.join(", "));
}

#[test]
fn a_sentence_a_word_or_a_plural_form_left_in_english_is_found() {
    // The allowances are only what they must be: in a file that is otherwise translated, a
    // sentence, a single word and one plural form left in English each still count, and a value
    // of placeholders or a product's name does not.
    let english = "[a]\nsentence = \"{name} was deleted\"\nword = \"Edit\"\n\
                   count = { one = \"{n} file\", other = \"{n} files\" }\nboth = \"{first} +{more}\"\n\
                   engine = \"Podman\"\n";
    let german = "[a]\nsentence = \"{name} wurde gelöscht\"\nword = \"Bearbeiten\"\n\
                  count = { one = \"{n} Datei\", other = \"{n} Dateien\" }\nboth = \"{first} +{more}\"\n\
                  engine = \"Podman\"\n";
    let with = |german: &str| -> BTreeMap<String, String> {
        CODES.iter().map(|code| ((*code).to_owned(), if *code == "en" { english } else { german }.to_owned())).collect()
    };
    assert!(left_in_english(&with(german)).values().all(Vec::is_empty));
    for (translated, kept, key) in [
        ("{name} wurde gelöscht", "{name} was deleted", "a.sentence"),
        ("Bearbeiten", "Edit", "a.word"),
        ("\"{n} Dateien\"", "\"{n} files\"", "a.count"),
    ] {
        let left = left_in_english(&with(&german.replacen(translated, kept, 1)));
        for code in CODES.iter().filter(|code| **code != "en") {
            assert_eq!(left[*code], [key], "{code}");
        }
    }
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
fn what_a_lineup_would_cost_and_what_it_is_assumed_to_hold_is_said_in_each_language() {
    // A lineup's two lines name a lineup, its steps and a number. A file that kept the English
    // words around them would still pass the completeness gate.
    let mut catalog = catalog();
    let unmeasured: Vec<(&str, Arg)> =
        vec![("lineup", Arg::from("coder")), ("models", Arg::from("a/one, b/two")), ("tokens", Arg::from("200000"))];
    let paid: Vec<(&str, Arg)> = vec![("models", Arg::from("b/two"))];
    catalog.set_active("en");
    let sentences = [
        (
            catalog.translate("profiles.wizard.provider-lineup-unmeasured", &unmeasured),
            vec!["coder", "a/one", "b/two", "200000", "Claude Code"],
        ),
        (catalog.translate("profile.lineup-paid", &paid), vec!["b/two", "OpenRouter"]),
    ];
    for code in CODES.iter().filter(|code| **code != "en") {
        catalog.set_active(code);
        for (key, (english, values)) in
            ["profiles.wizard.provider-lineup-unmeasured", "profile.lineup-paid"].iter().zip(sentences.iter())
        {
            let said = catalog.translate(key, if *key == "profile.lineup-paid" { &paid } else { &unmeasured });
            assert!(!said.starts_with('⟦') && !said.is_empty(), "{code} has no words for {key}");
            assert_ne!(&said, english, "{code} still says {key} in English");
            for value in values {
                assert!(said.contains(value), "{code} loses {value} from {key}: {said}");
            }
        }
    }
}
