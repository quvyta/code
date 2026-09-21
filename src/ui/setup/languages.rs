//! Which language stands where on the wizard's first page, and which one is already chosen.
//!
//! Two rules, both the person's: the machine's own language is the first row and the one that is
//! chosen when nothing has been chosen before, and the rest of the list reads alphabetically.
//! A machine that names no language QCode speaks gets English in that place, which is also the
//! language the framework falls back to, so the row that looks chosen is the language the screen
//! is really being drawn in.
//!
//! The order is worked out once, when the wizard is built, and then held. Sorting the rows again
//! at every draw would shuffle the list under the hand of someone who has just picked a language,
//! which is the one moment they are looking straight at it.

use qframe::i18n::I18n;

use crate::store::Config;

/// The language QCode speaks when the machine names none, which is also the framework's own
/// fallback.
const FALLBACK: &str = "en";

/// The order the languages are offered in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Languages {
    order: Vec<&'static str>,
}

impl Languages {
    /// The order for a machine whose own language is `system`, with `name` giving a language the
    /// name it is shown by.
    ///
    /// `system` may be anything; only a language QCode speaks takes the first row, and anything
    /// else leaves it to English.
    #[must_use]
    pub fn new(system: Option<&str>, name: impl Fn(&str) -> String) -> Self {
        let first =
            system.and_then(|wanted| Config::LANGUAGES.into_iter().find(|known| *known == wanted)).unwrap_or(FALLBACK);
        let mut rest: Vec<&'static str> = Config::LANGUAGES.into_iter().filter(|code| *code != first).collect();
        rest.sort_by_cached_key(|code| sort_key(&name(code)));
        let mut order = Vec::with_capacity(Config::LANGUAGES.len());
        order.push(first);
        order.extend(rest);
        Self { order }
    }

    /// The order for a machine whose own language is `system`, with the names as they read in
    /// `speaking`, the language the wizard is about to be drawn in.
    #[must_use]
    pub fn shown_in(system: Option<&str>, speaking: &str) -> Self {
        let mut catalog = I18n::builtin();
        for (file, text) in crate::locales() {
            catalog.add_source(&file, &text);
        }
        catalog.set_active(speaking);
        Self::new(system, |code| catalog.translate(&format!("setup.language-{code}"), &[]))
    }

    /// The languages, first row first.
    #[must_use]
    pub fn codes(&self) -> &[&'static str] {
        &self.order
    }

    /// The row `code` stands on, or the first row for a language that is not offered.
    #[must_use]
    pub fn row_of(&self, code: &str) -> usize {
        self.order.iter().position(|known| *known == code).unwrap_or(0)
    }

    /// The language on row `row`, or the first one when there is no such row.
    #[must_use]
    pub fn on_row(&self, row: usize) -> &'static str {
        self.order.get(row).copied().unwrap_or(self.order[0])
    }
}

/// The language this machine is set to, among the ones QCode speaks, read through `lookup`.
///
/// The reading itself is the framework's: the same locale names, the same order of environment
/// variables and the same matching that every Quvyta application follows.
#[must_use]
pub fn system(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    let found = I18n::builtin().detect(lookup)?;
    Config::LANGUAGES.into_iter().find(|known| *known == found).map(str::to_owned)
}

/// The language this machine is set to, read from the environment it was started in.
#[must_use]
pub fn system_here() -> Option<String> {
    system(|name| std::env::var(name).ok())
}

/// What a name is sorted by: its letters without case and without the marks above and below
/// them, so a Turkish list does not send `Çince` past `Rusça` for the sake of a cedilla.
///
/// Writing systems of their own sort by their own code points, which keeps them together and in
/// the same order every time.
fn sort_key(name: &str) -> String {
    name.to_lowercase().chars().map(fold).collect()
}

/// The plain letter a marked one is sorted as.
fn fold(letter: char) -> char {
    match letter {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'é' | 'è' | 'ê' | 'ë' | 'ē' => 'e',
        'ğ' | 'ģ' => 'g',
        'í' | 'ì' | 'î' | 'ï' | 'ī' | 'ı' => 'i',
        'ñ' | 'ń' => 'n',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' => 'o',
        'ş' | 'ś' | 'š' => 's',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ž' | 'ź' | 'ż' => 'z',
        // The combining dot Turkish `İ` leaves behind once the case is gone.
        '\u{0307}' => '\0',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{Languages, system};

    fn english(code: &str) -> String {
        match code {
            "en" => "English",
            "tr" => "Turkish",
            "de" => "German",
            "es" => "Spanish",
            "fr" => "French",
            "pt-BR" => "Portuguese (Brazil)",
            "ru" => "Russian",
            "zh-Hans" => "Chinese (Simplified)",
            _ => "Japanese",
        }
        .to_owned()
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn the_machines_own_language_takes_the_first_row() {
        let order = Languages::new(Some("tr"), english);
        assert_eq!(order.codes()[0], "tr");
        assert_eq!(order.row_of("tr"), 0);
    }

    #[test]
    fn a_machine_that_names_no_language_qcode_speaks_gets_english_first() {
        // Finnish is a language QCode has no words for, and a machine set to `C` names none at
        // all: both leave the first row to English rather than to whatever happens to be first.
        for system in [None, Some("fi"), Some("")] {
            let order = Languages::new(system, english);
            assert_eq!(order.codes()[0], "en", "{system:?}");
        }
    }

    #[test]
    fn the_rest_of_the_list_reads_alphabetically() {
        let order = Languages::new(Some("tr"), english);
        assert_eq!(
            order.codes(),
            ["tr", "zh-Hans", "en", "fr", "de", "ja", "pt-BR", "ru", "es"],
            "Chinese, English, French, German, Japanese, Portuguese, Russian, Spanish"
        );
    }

    #[test]
    fn the_first_row_is_not_repeated_further_down() {
        let order = Languages::new(Some("de"), english);
        assert_eq!(order.codes().len(), 9);
        assert_eq!(order.codes().iter().filter(|code| **code == "de").count(), 1);
    }

    #[test]
    fn a_marked_letter_sorts_as_the_plain_one() {
        // The Turkish names of the nine, where a cedilla on `Çince` would otherwise throw it
        // past every unmarked name to the very end of the list.
        let turkish = |code: &str| {
            match code {
                "en" => "İngilizce",
                "tr" => "Türkçe",
                "de" => "Almanca",
                "es" => "İspanyolca",
                "fr" => "Fransızca",
                "pt-BR" => "Portekizce (Brezilya)",
                "ru" => "Rusça",
                "zh-Hans" => "Çince (Basitleştirilmiş)",
                _ => "Japonca",
            }
            .to_owned()
        };
        let order = Languages::new(Some("tr"), turkish);
        assert_eq!(order.codes(), ["tr", "de", "zh-Hans", "fr", "en", "es", "ja", "pt-BR", "ru"]);
    }

    #[test]
    fn the_language_of_the_machine_is_read_the_way_every_quvyta_application_reads_it() {
        assert_eq!(system(env(&[("LANG", "tr_TR.UTF-8")])), Some("tr".to_owned()));
        assert_eq!(system(env(&[("LC_ALL", "pt_BR.UTF-8"), ("LANG", "tr_TR.UTF-8")])), Some("pt-BR".to_owned()));
        assert_eq!(system(env(&[("LANG", "C")])), None);
        // An empty lookup is not an empty answer: the framework falls through to the operating
        // system's own setting, which is the machine the test is running on.
    }

    #[test]
    fn the_names_the_order_is_built_from_are_the_ones_the_wizard_shows() {
        // Built from QCode's own files rather than a test's copy of them: a name that changes in
        // `tr.toml` moves its row here too.
        let order = Languages::shown_in(Some("tr"), "tr");
        assert_eq!(order.codes(), ["tr", "de", "zh-Hans", "fr", "en", "es", "ja", "pt-BR", "ru"]);
    }
}
