//! The identifier a workspace is known by on disk, in container names and in the config.
//!
//! A display name is whatever the user typed; an identifier is what every file system, every
//! container engine and a case-insensitive comparison all accept. Turning one into the other
//! must not depend on the machine's locale, which is why the folding here is written out by
//! hand: in a Turkish locale `I` and `i` are not each other's pair, and `İ` lowercases to two
//! characters in Unicode.

use std::fmt;

/// Why a display name or a stored identifier cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceIdError {
    /// Nothing in the name can be part of an identifier.
    Empty,
    /// A character an identifier may not contain, and where it is.
    Illegal {
        /// The character's position in characters, starting at 0.
        position: usize,
        /// The character itself.
        character: char,
    },
    /// Longer than [`WorkspaceId::MAX_LEN`].
    TooLong {
        /// The length that was found, in characters.
        length: usize,
    },
    /// A name Windows reserves for a device and never gives to a file.
    Reserved,
}

/// The name a workspace has on disk: lower-case ASCII letters and digits, separated by `-`.
///
/// Generated from a display name with [`from_display_name`](Self::from_display_name) and read
/// back from disk with [`parse`](Self::parse). Every identifier is also a legal folder name, a
/// legal container name and a legal volume name, so the same string names the workspace
/// everywhere.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId(String);

/// The names Windows keeps for devices. A file cannot carry one of these, with or without an
/// extension, whatever its case.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// What is appended to a reserved name to make it usable. It cannot itself be reserved, and it
/// is not a number, so it never looks like the automatic `-2` this layer refuses to invent.
const ESCAPE: &str = "-qcode";

impl WorkspaceId {
    /// The longest an identifier may be, in characters.
    ///
    /// The identifier is part of `qcode-home-<workspace>-<profile>`, of a folder name below the
    /// store and of a volume name; 64 leaves room for all of them inside the 255 characters
    /// the strictest of them allows.
    pub const MAX_LEN: usize = 64;

    /// Builds the identifier of a workspace the user named `name`.
    ///
    /// Letters fold to ASCII, everything else becomes a single `-`, and leading and trailing
    /// separators — which is where a trailing dot or space ends up — are removed. A name a
    /// device is called by is moved out of the way. The result never depends on the locale.
    ///
    /// # Errors
    ///
    /// [`WorkspaceIdError::Empty`] when nothing in `name` can be part of an identifier.
    pub fn from_display_name(name: &str) -> Result<Self, WorkspaceIdError> {
        let mut out = String::with_capacity(name.len());
        for character in name.chars() {
            match fold(character) {
                Some(folded) => out.push(folded),
                None if out.ends_with('-') => {}
                None => out.push('-'),
            }
        }
        out.truncate(out.char_indices().nth(Self::MAX_LEN).map_or(out.len(), |(at, _)| at));
        let trimmed = out.trim_matches('-');
        if trimmed.is_empty() {
            return Err(WorkspaceIdError::Empty);
        }
        let mut id = trimmed.to_owned();
        if RESERVED.contains(&id.as_str()) {
            id.push_str(ESCAPE);
        }
        Ok(Self(id))
    }

    /// Reads an identifier that is already stored: a folder name, a key in the config or the
    /// `id` of a `workspace.qcode`.
    ///
    /// # Errors
    ///
    /// Says which rule the text breaks, so the caller can point at it.
    pub fn parse(text: &str) -> Result<Self, WorkspaceIdError> {
        let length = text.chars().count();
        if length == 0 {
            return Err(WorkspaceIdError::Empty);
        }
        if length > Self::MAX_LEN {
            return Err(WorkspaceIdError::TooLong { length });
        }
        for (position, character) in text.chars().enumerate() {
            let edge = position == 0 || position + 1 == length;
            let legal = character.is_ascii_lowercase() || character.is_ascii_digit() || (character == '-' && !edge);
            if !legal {
                return Err(WorkspaceIdError::Illegal { position, character });
            }
        }
        if RESERVED.contains(&text) {
            return Err(WorkspaceIdError::Reserved);
        }
        Ok(Self(text.to_owned()))
    }

    /// The identifier as it is written on disk.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether a file system that ignores case would see `name` as this workspace's folder.
    ///
    /// Folding is ASCII only on purpose: an identifier holds nothing else, and a locale-aware
    /// fold would answer differently in a Turkish locale.
    #[must_use]
    pub fn clashes_with(&self, name: &str) -> bool {
        self.0.eq_ignore_ascii_case(name)
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The ASCII letter or digit an identifier keeps `character` as, or `None` when it is a
/// separator.
///
/// The Turkish letters are listed by hand: `İ` has no single-character lower case in Unicode,
/// and `I` and `ı` must land on the same letter here however the machine is set up.
fn fold(character: char) -> Option<char> {
    Some(match character {
        'i' | 'ı' | 'İ' | 'I' => 'i',
        'ç' | 'Ç' => 'c',
        'ğ' | 'Ğ' => 'g',
        'ö' | 'Ö' => 'o',
        'ş' | 'Ş' => 's',
        'ü' | 'Ü' => 'u',
        letter if letter.is_ascii_alphanumeric() => letter.to_ascii_lowercase(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> String {
        WorkspaceId::from_display_name(name).expect("a usable name").as_str().to_owned()
    }

    #[test]
    fn turkish_letters_fold_the_same_way_in_every_locale() {
        assert_eq!(id("İstanbul Şehri"), "istanbul-sehri");
        assert_eq!(id("Iğdır ılık"), "igdir-ilik");
        assert_eq!(id("ÇÖĞÜŞİI"), "cogusii");
        assert_eq!(id("çöğüşıi"), "cogusii");
        // The dotted capital I lowercases to two characters in Unicode; it must still be one.
        assert_eq!(id("İ"), "i");
    }

    #[test]
    fn spaces_dots_and_case_become_one_separator() {
        assert_eq!(id("  My.Workspace   v2  "), "my-workspace-v2");
        assert_eq!(id("a---b"), "a-b");
        assert_eq!(id("snake_case"), "snake-case");
    }

    #[test]
    fn trailing_dots_and_spaces_never_survive() {
        // Windows silently strips them from a folder name, which would break the id on disk.
        assert_eq!(id("Proje."), "proje");
        assert_eq!(id("Proje ..  "), "proje");
        assert_eq!(id("...Proje..."), "proje");
    }

    #[test]
    fn windows_device_names_are_moved_out_of_the_way() {
        for reserved in ["con", "CON", "Prn", "aux", "NUL", "com1", "COM9", "lpt1", "LPT9"] {
            let made = id(reserved);
            assert!(made.ends_with("-qcode"), "{reserved} became {made}");
            assert!(WorkspaceId::parse(&made).is_ok(), "{made} is a legal id");
        }
        for fine in ["con3", "com", "com10", "console", "lpt"] {
            assert!(!id(fine).ends_with("-qcode"), "{fine} is not a device name");
        }
    }

    #[test]
    fn a_name_with_nothing_usable_in_it_is_an_error() {
        for empty in ["", "   ", "...", "///", "。。"] {
            assert_eq!(WorkspaceId::from_display_name(empty), Err(WorkspaceIdError::Empty), "{empty:?}");
        }
    }

    #[test]
    fn a_long_name_is_cut_without_leaving_a_dangling_separator() {
        let made = id(&format!("{} {}", "a".repeat(WorkspaceId::MAX_LEN - 1), "bbbb"));
        assert_eq!(made.len(), WorkspaceId::MAX_LEN - 1, "cut at the separator, which is then trimmed");
        let long = id(&"ş".repeat(WorkspaceId::MAX_LEN * 2));
        assert_eq!(long.len(), WorkspaceId::MAX_LEN);
        assert!(WorkspaceId::parse(&long).is_ok());
    }

    #[test]
    fn parsing_accepts_only_what_generating_produces() {
        assert_eq!(WorkspaceId::parse("my-workspace").map(|id| id.as_str().to_owned()), Ok("my-workspace".to_owned()));
        assert_eq!(WorkspaceId::parse(""), Err(WorkspaceIdError::Empty));
        assert_eq!(WorkspaceId::parse("My-Workspace"), Err(WorkspaceIdError::Illegal { position: 0, character: 'M' }));
        assert_eq!(WorkspaceId::parse("a b"), Err(WorkspaceIdError::Illegal { position: 1, character: ' ' }));
        assert_eq!(WorkspaceId::parse("-a"), Err(WorkspaceIdError::Illegal { position: 0, character: '-' }));
        assert_eq!(WorkspaceId::parse("a-"), Err(WorkspaceIdError::Illegal { position: 1, character: '-' }));
        assert_eq!(WorkspaceId::parse("con"), Err(WorkspaceIdError::Reserved));
        let long = "a".repeat(WorkspaceId::MAX_LEN + 1);
        assert_eq!(WorkspaceId::parse(&long), Err(WorkspaceIdError::TooLong { length: WorkspaceId::MAX_LEN + 1 }));
    }

    #[test]
    fn a_clash_ignores_case_because_some_file_systems_do() {
        let made = WorkspaceId::from_display_name("Belgeler").expect("usable");
        assert!(made.clashes_with("BELGELER"));
        assert!(made.clashes_with("Belgeler"));
        assert!(!made.clashes_with("belgelerim"));
        // Folding is ASCII only: a Turkish locale must not turn `I` into `ı` here.
        assert!(!WorkspaceId::from_display_name("ilik").expect("usable").clashes_with("ILİK"));
    }
}
