//! A provider's key, which is written once and never shown again.
//!
//! The key is a value of its own rather than a `String` so that nothing can print it by
//! accident: it has no `Display`, its `Debug` writes only the marker and the last four
//! characters, and the one method that hands the secret out
//! ([`expose`](Key::expose)) is visible only inside this crate, where the single caller is the
//! transport that puts it in a request header. A key never travels in a URL, so nothing that
//! records or reports an address can record it either.

/// How many characters of a key are ever shown.
const SHOWN: usize = 4;

/// The shortest key whose last four characters can be shown without showing most of it. A key
/// of six characters would be two thirds revealed by its last four.
const SHOWABLE: usize = 12;

/// A provider's key.
#[derive(Clone, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    /// The key the person pasted, with the spaces around it dropped, or `None` when they pasted
    /// nothing. A pasted key often carries a trailing newline from the clipboard, and a key with
    /// a newline in it makes a header no server accepts.
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        // A key that is not one line is not a key: a header holds no control characters, and
        // something pasted from a file may bring a whole paragraph with it.
        if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    /// The last four characters, which is all of a key that is ever shown, or `None` for a key
    /// so short that four characters would give most of it away.
    #[must_use]
    pub fn last_four(&self) -> Option<String> {
        let characters: Vec<char> = self.0.chars().collect();
        (characters.len() >= SHOWABLE).then(|| characters[characters.len() - SHOWN..].iter().collect())
    }

    /// The secret itself, for the one place that has to send it.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Key {
    /// Writes what the page writes and nothing more, so a key in a structure that is printed
    /// while something is being looked into does not end up in a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.last_four() {
            Some(tail) => write!(formatter, "Key(…{tail})"),
            None => formatter.write_str("Key(…)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a key of anyone's: the characters spell what it is.
    const MADE_UP: &str = "not-a-real-key-0000-wxyz";

    #[test]
    fn only_the_last_four_characters_are_ever_shown() {
        let key = Key::new(MADE_UP).expect("a key");
        assert_eq!(key.last_four().as_deref(), Some("wxyz"));
        assert_eq!(format!("{key:?}"), "Key(…wxyz)");
        assert!(!format!("{key:?}").contains("not-a-real"), "the front of the key is not printed");
    }

    #[test]
    fn a_short_key_shows_nothing_at_all_rather_than_most_of_itself() {
        let key = Key::new("abcdefgh").expect("a key");
        assert_eq!(key.last_four(), None, "four of eight characters is most of the key");
        assert_eq!(format!("{key:?}"), "Key(…)");
        assert!(!format!("{key:?}").contains("efgh"));
    }

    #[test]
    fn what_the_clipboard_brings_along_is_dropped_and_a_paragraph_is_refused() {
        let key = Key::new(&format!("  {MADE_UP}\n")).expect("a key");
        assert_eq!(key.expose(), MADE_UP, "the key itself is what a header gets");
        assert_eq!(Key::new(""), None);
        assert_eq!(Key::new("   \n  "), None);
        assert_eq!(Key::new("first line\nsecond line"), None, "no header holds two lines");
        assert_eq!(Key::new("has\ttab"), None);
    }
}
