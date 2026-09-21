//! A page is called by one name in each language: the name on its own title is the name every
//! other line uses when it sends the person there.
//!
//! The Chinese and the Russian files once called the providers page one thing on the page and
//! another in the profile wizard that points to it, so a person told to go to one page found a
//! menu that did not say that word anywhere.

use toml::de::{DeTable, DeValue};

/// The lines, outside the page itself, that send the person to the providers page by its name.
const POINTING_AT_PROVIDERS: [&str; 4] = [
    "profiles.wizard.provider-none",
    "profiles.wizard.provider-model-none",
    "profiles.wizard.provider-model-unmeasured",
    "workspace.provider-missing",
];

fn value<'a>(table: &'a DeTable<'a>, key: &str) -> Option<&'a str> {
    let mut parts = key.split('.').peekable();
    let mut current = table;
    while let Some(part) = parts.next() {
        let item = current.get(part)?.get_ref();
        if parts.peek().is_none() {
            return match item {
                DeValue::String(text) => Some(text.as_ref()),
                _ => None,
            };
        }
        let DeValue::Table(inner) = item else { return None };
        current = inner;
    }
    None
}

#[test]
fn every_language_calls_the_providers_page_by_the_name_on_its_title() {
    let locales = qcode::locales();
    for (file, text) in &locales {
        let parsed = DeTable::parse(text).expect("the locale parses");
        let table = parsed.get_ref();
        let title = value(table, "provider.title").expect("the page has a title");
        assert_eq!(value(table, "provider.menu"), Some(title), "{file}: the menu row and the title differ");
        for key in POINTING_AT_PROVIDERS {
            let line = value(table, key).unwrap_or_else(|| panic!("{file}: {key} is missing"));
            assert!(line.contains(title), "{file}: {key} does not call the page {title}: {line}");
        }
    }
    assert_eq!(locales.len(), 9, "every language qcode ships");
}
