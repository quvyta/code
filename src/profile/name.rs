//! Names that are safe as a file name and as a container object name at the same time.

use std::fmt;

/// A name that is safe everywhere QCode puts it: as a file name in the workspace, as part of an
/// image name, and as a container or volume name.
///
/// It holds lowercase ASCII letters, digits and single hyphens, and begins and ends with a
/// letter or digit. Folding is written out character by character rather than left to
/// [`str::to_lowercase`], so that the same text gives the same name on every machine: in Turkish
/// `I` and `İ` do not fold the way the rest of the world folds them, and a name is a file name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SafeName(String);

impl SafeName {
    /// The longest a name may be, in characters. Container engines take far longer names; the
    /// limit keeps a name readable in a list and leaves room for the prefixes QCode adds.
    pub const MAX_LENGTH: usize = 64;

    /// Makes a name out of text a person typed, such as a project title. Letters fold to ASCII,
    /// everything else becomes a single hyphen, and the result is cut to [`Self::MAX_LENGTH`].
    /// Text without a single usable character has no name.
    #[must_use]
    pub fn from_display(text: &str) -> Option<Self> {
        let mut folded = String::new();
        for character in text.chars() {
            if character.is_ascii_alphanumeric() {
                folded.push(character.to_ascii_lowercase());
            } else if let Some(letter) = fold(character) {
                folded.push_str(letter);
            } else if !folded.is_empty() && !folded.ends_with('-') {
                folded.push('-');
            }
        }
        folded.truncate(Self::MAX_LENGTH);
        while folded.ends_with('-') {
            folded.pop();
        }
        (!folded.is_empty()).then_some(Self(folded))
    }

    /// Reads a name that is already safe, such as one from a definition file. Text that is not
    /// exactly what [`Self::from_display`] would have produced is refused rather than repaired,
    /// so that a file and the name in it can never disagree.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let safe = !text.is_empty()
            && text.len() <= Self::MAX_LENGTH
            && text.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && text.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !text.contains("--");
        safe.then(|| Self(text.to_owned()))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SafeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The ASCII a letter outside ASCII folds to. Letters QCode has no folding for become a
/// separator, so a name never carries a character an engine or a file system might refuse.
fn fold(character: char) -> Option<&'static str> {
    match character {
        'ç' | 'Ç' => Some("c"),
        'ğ' | 'Ğ' => Some("g"),
        'ı' | 'İ' => Some("i"),
        'î' | 'Î' => Some("i"),
        'ö' | 'Ö' => Some("o"),
        'ş' | 'Ş' => Some("s"),
        'ü' | 'Ü' => Some("u"),
        'â' | 'Â' => Some("a"),
        'û' | 'Û' => Some("u"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_text_becomes_a_lowercase_ascii_name() {
        assert_eq!(SafeName::from_display("Claude Sub").expect("has letters").as_str(), "claude-sub");
        assert_eq!(SafeName::from_display("  My   Project  ").expect("has letters").as_str(), "my-project");
        assert_eq!(SafeName::from_display("v2.0/build").expect("has letters").as_str(), "v2-0-build");
    }

    #[test]
    fn turkish_letters_fold_the_same_way_in_every_locale() {
        // The locale must not decide: `I` and `İ` and `ı` all end up as the ASCII `i`.
        assert_eq!(SafeName::from_display("İstanbul Projesi").expect("has letters").as_str(), "istanbul-projesi");
        assert_eq!(SafeName::from_display("IŞIK").expect("has letters").as_str(), "isik");
        assert_eq!(SafeName::from_display("ışık").expect("has letters").as_str(), "isik");
        assert_eq!(SafeName::from_display("Iı İi").expect("has letters").as_str(), "ii-ii");
        assert_eq!(SafeName::from_display("Öğün Çöp Şüphe").expect("has letters").as_str(), "ogun-cop-suphe");
    }

    #[test]
    fn a_name_never_starts_or_ends_with_a_separator() {
        assert_eq!(SafeName::from_display("--x--").expect("has letters").as_str(), "x");
        assert_eq!(SafeName::from_display(".hidden.").expect("has letters").as_str(), "hidden");
        assert_eq!(SafeName::from_display("_ _ a _ _").expect("has letters").as_str(), "a");
    }

    #[test]
    fn text_without_a_usable_character_has_no_name() {
        assert_eq!(SafeName::from_display(""), None);
        assert_eq!(SafeName::from_display("   "), None);
        assert_eq!(SafeName::from_display("///"), None);
        assert_eq!(SafeName::from_display("日本語"), None);
    }

    #[test]
    fn long_text_is_cut_to_the_limit_and_still_ends_in_a_letter() {
        let name = SafeName::from_display(&"ab ".repeat(50)).expect("has letters");
        assert!(name.as_str().len() <= SafeName::MAX_LENGTH, "{}", name.as_str());
        assert!(name.as_str().ends_with(|c: char| c.is_ascii_alphanumeric()), "{}", name.as_str());
    }

    #[test]
    fn parsing_accepts_only_an_already_safe_name() {
        assert_eq!(SafeName::parse("claude-sub").expect("safe").as_str(), "claude-sub");
        assert_eq!(SafeName::parse("a1"), SafeName::from_display("a1"));
        for text in ["Claude", "claude sub", "-claude", "claude-", "cl--aude", "claude_sub", "", &"a".repeat(65)] {
            assert_eq!(SafeName::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_name_displays_as_its_text() {
        let name = SafeName::from_display("Claude Sub").expect("has letters");
        assert_eq!(name.to_string(), "claude-sub");
    }
}
