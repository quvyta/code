//! The tag a person gives a provider: the short name that stands in front of a model.
//!
//! A tag is checked rather than repaired. It is shown beside a model name, written into a
//! profile and used to find a provider again, so a tag with a slash or a space in it would
//! either read as something else or match nothing; the person is told which character stopped it
//! and where, and changes it themselves.

/// The longest a tag may be. A tag stands in front of a model name on one row, and a model name
/// is already long; past this the row is the tag and nothing else.
const LONGEST: usize = 24;

/// Why a name cannot be a tag. Positions are counted from one, the way a person counts the
/// letters of the name they just typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagError {
    /// Nothing was written.
    Empty,
    /// The name is longer than a tag may be.
    TooLong {
        /// How many characters were written.
        length: usize,
    },
    /// The first character is not a letter, so the tag would not read as a name.
    BadStart {
        /// The character that was written first.
        character: char,
    },
    /// A character a tag cannot hold.
    Illegal {
        /// Which character it is, counted from one.
        position: usize,
        /// The character itself.
        character: char,
    },
}

/// A provider's tag, checked.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tag(String);

impl Tag {
    /// The tag `name` spells, or the first reason it cannot be one.
    ///
    /// # Errors
    ///
    /// [`TagError`] says what stopped it and where.
    pub fn parse(name: &str) -> Result<Self, TagError> {
        let characters: Vec<char> = name.chars().collect();
        let Some(first) = characters.first().copied() else { return Err(TagError::Empty) };
        if characters.len() > LONGEST {
            return Err(TagError::TooLong { length: characters.len() });
        }
        // Digits and dashes are allowed inside a tag but not in front of it: a name that starts
        // with one reads as a number or an option rather than as the person's own word for a
        // machine.
        if !first.is_ascii_alphabetic() {
            return Err(TagError::BadStart { character: first });
        }
        for (index, character) in characters.iter().copied().enumerate() {
            if !(character.is_ascii_alphanumeric() || character == '-' || character == '_') {
                return Err(TagError::Illegal { position: index + 1, character });
            }
        }
        Ok(Self(name.to_owned()))
    }

    /// The tag as it is written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_a_tag_and_reads_back_as_itself() {
        for name in ["ev", "ev-makinesi", "openrouter_2", "a"] {
            let tag = Tag::parse(name).expect(name);
            assert_eq!(tag.as_str(), name);
            assert_eq!(tag.to_string(), name);
        }
    }

    #[test]
    fn a_character_that_would_read_as_something_else_is_refused_where_it_stands() {
        // A slash is the one that matters most: the tag stands in front of a model name.
        assert_eq!(Tag::parse("ev/makine"), Err(TagError::Illegal { position: 3, character: '/' }));
        assert_eq!(Tag::parse("ev makine"), Err(TagError::Illegal { position: 3, character: ' ' }));
        assert_eq!(Tag::parse("ev.makine"), Err(TagError::Illegal { position: 3, character: '.' }));
        assert_eq!(Tag::parse("evÇ"), Err(TagError::Illegal { position: 3, character: 'Ç' }));
    }

    #[test]
    fn a_tag_starts_with_a_letter_and_is_never_empty() {
        assert_eq!(Tag::parse(""), Err(TagError::Empty));
        assert_eq!(Tag::parse("2ev"), Err(TagError::BadStart { character: '2' }));
        assert_eq!(Tag::parse("-ev"), Err(TagError::BadStart { character: '-' }));
        assert_eq!(Tag::parse("_ev"), Err(TagError::BadStart { character: '_' }));
    }

    #[test]
    fn a_tag_long_enough_to_take_the_whole_row_is_refused_with_its_length() {
        let long = "e".repeat(LONGEST + 1);
        assert_eq!(Tag::parse(&long), Err(TagError::TooLong { length: LONGEST + 1 }));
        assert!(Tag::parse(&"e".repeat(LONGEST)).is_ok(), "the longest allowed tag still passes");
    }
}
