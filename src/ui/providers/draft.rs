//! The add-a-provider form's own state, and everything that can stop a provider being added.
//!
//! Nothing here touches the disk or the network, and nothing here is a sentence: every problem
//! is a value the view turns into words from the language files, so the same check reads the
//! same way in every language and is tested without a screen.
//!
//! The key is the one field that is never read back. It is typed into a password field, it goes
//! straight into a [`Key`](crate::provider::Key) when the provider is added, and the draft holds
//! it as text only for as long as the person is still typing it.

use crate::provider::{Key, ProviderEntry, ProviderKind, Tag, TagError, ask};

/// What stopped a provider being added, apart from its wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The tag cannot be a tag, and why.
    Tag(TagError),
    /// A provider of this tag is already there.
    Taken(String),
    /// No address was written.
    NoBase,
    /// An address QCode would not know how to reach.
    BaseNotAnAddress(String),
    /// An address that starts like one but that no request could be sent to, such as two run
    /// together when a person typed theirs after the one that was offered.
    BaseUnusable(String),
    /// This kind is reached with a key and none was pasted.
    NoKey(ProviderKind),
}

impl Problem {
    /// The field the problem belongs beside.
    #[must_use]
    pub fn field(&self) -> &'static str {
        match self {
            Self::Tag(_) | Self::Taken(_) => TAG_FIELD,
            Self::NoBase | Self::BaseNotAnAddress(_) | Self::BaseUnusable(_) => BASE_FIELD,
            Self::NoKey(_) => KEY_FIELD,
        }
    }
}

/// The name of the tag field, which the focus and the error summary use.
pub const TAG_FIELD: &str = "provider-tag";

/// The name of the address field.
pub const BASE_FIELD: &str = "provider-base";

/// The name of the key field.
pub const KEY_FIELD: &str = "provider-key";

/// A provider being written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The tag as it has been typed.
    pub tag: String,
    /// Which kind it is.
    pub kind: ProviderKind,
    /// The address as it has been typed.
    pub base: String,
    /// The key as it is being typed. Never shown back: the field it lives in is a password
    /// field, and once the provider is added this is gone.
    pub key: String,
    /// What stopped it last time the person asked for it to be added, or nothing while they
    /// have not asked yet. A form does not complain about a field nobody has finished.
    pub problem: Option<Problem>,
}

impl Draft {
    /// A new form, on the kind the page offers first, with that kind's usual address already in
    /// the field so the common case is one word of typing.
    #[must_use]
    pub fn new() -> Self {
        let kind = ProviderKind::ALL[0];
        Self { tag: String::new(), kind, base: kind.suggested_base().to_owned(), key: String::new(), problem: None }
    }

    /// Chooses a kind, and moves the address along with it when the person has not changed the
    /// one that was offered. A person who typed their own address keeps it. A ready-made kind
    /// brings its own tag too, on the same terms: only into a field the person has not written.
    pub fn pick_kind(&mut self, kind: ProviderKind) {
        if self.offered_base() {
            self.base = kind.suggested_base().to_owned();
        }
        let tag = self.tag.trim();
        if tag.is_empty() || self.kind.suggested_tag() == Some(tag) {
            self.tag = kind.suggested_tag().unwrap_or_default().to_owned();
        }
        self.kind = kind;
        self.problem = None;
    }

    /// Chooses where a ready-made provider answers, the `index`th of its kind's regions. The
    /// address is the region's whatever was in the field: picking a region is saying which
    /// address, and nothing else does that.
    pub fn pick_region(&mut self, index: usize) {
        if let Some(region) = self.kind.regions().get(index) {
            self.base = region.base.to_owned();
            self.problem = None;
        }
    }

    /// Which of the kind's regions the address is, or `None` when the person wrote one of their
    /// own.
    #[must_use]
    pub fn region(&self) -> Option<usize> {
        let base = crate::provider::trim_base(&self.base);
        self.kind.regions().iter().position(|region| region.base == base)
    }

    /// Whether the address is one QCode offered rather than one the person wrote.
    fn offered_base(&self) -> bool {
        self.base == self.kind.suggested_base() || self.region().is_some()
    }

    /// The provider this form describes, or the first reason it is not one.
    ///
    /// `taken` says whether a tag is already in the list, which only the list knows.
    ///
    /// # Errors
    ///
    /// [`Problem`] says what stopped it and which field it belongs beside.
    pub fn build(&self, taken: impl Fn(&str) -> bool) -> Result<ProviderEntry, Problem> {
        let tag = Tag::parse(self.tag.trim()).map_err(Problem::Tag)?;
        if taken(tag.as_str()) {
            return Err(Problem::Taken(tag.as_str().to_owned()));
        }
        let base = self.base.trim();
        if base.is_empty() {
            return Err(Problem::NoBase);
        }
        // Only an address with a scheme can be reached, and a person pasting one from a browser
        // has it. Saying so now is better than a connection that fails for a reason nobody can
        // see in the address.
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(Problem::BaseNotAnAddress(base.to_owned()));
        }
        let key = Key::new(&self.key);
        if self.kind.needs_key() && key.is_none() {
            return Err(Problem::NoKey(self.kind));
        }
        let mut entry = ProviderEntry::new(tag, self.kind, base);
        // Judged on an address a request really goes to, so what is refused here is exactly what
        // would have failed there.
        if !ask::can_be_sent_to(&entry.messages_address()) {
            return Err(Problem::BaseUnusable(base.to_owned()));
        }
        entry.key = key;
        Ok(entry)
    }
}

impl Default for Draft {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(tag: &str, kind: ProviderKind, base: &str, key: &str) -> Draft {
        Draft { tag: tag.to_owned(), kind, base: base.to_owned(), key: key.to_owned(), problem: None }
    }

    #[test]
    fn a_filled_form_becomes_a_provider() {
        let draft = filled("ev", ProviderKind::Ollama, " http://192.168.122.1:11434/ ", "");
        let entry = draft.build(|_| false).expect("the form is filled");
        assert_eq!(entry.tag.as_str(), "ev");
        assert_eq!(entry.base, "http://192.168.122.1:11434", "the address is tidied as it is taken");
        assert_eq!(entry.key, None);
    }

    #[test]
    fn a_tag_that_is_already_there_is_refused_beside_the_tag_field() {
        let draft = filled("ev", ProviderKind::Ollama, "http://h:1", "");
        let problem = draft.build(|tag| tag == "ev").expect_err("the tag is taken");
        assert_eq!(problem, Problem::Taken("ev".to_owned()));
        assert_eq!(problem.field(), TAG_FIELD);
    }

    #[test]
    fn a_tag_that_could_not_stand_in_front_of_a_model_is_refused_with_its_reason() {
        let draft = filled("ev/makine", ProviderKind::Ollama, "http://h:1", "");
        let problem = draft.build(|_| false).expect_err("a slash is not a tag");
        assert_eq!(problem, Problem::Tag(TagError::Illegal { position: 3, character: '/' }));
        assert_eq!(problem.field(), TAG_FIELD);
    }

    #[test]
    fn an_address_with_no_scheme_is_named_now_rather_than_failing_for_no_visible_reason_later() {
        let draft = filled("ev", ProviderKind::Ollama, "192.168.122.1:11434", "");
        let problem = draft.build(|_| false).expect_err("that cannot be reached");
        assert_eq!(problem, Problem::BaseNotAnAddress("192.168.122.1:11434".to_owned()));
        assert_eq!(problem.field(), BASE_FIELD);
        assert_eq!(filled("ev", ProviderKind::Ollama, "  ", "").build(|_| false), Err(Problem::NoBase));
    }

    #[test]
    fn two_addresses_run_together_are_refused_by_the_parser_the_requests_use() {
        let joined = "http://127.0.0.1:11434http://192.168.122.1:11434";
        let problem =
            filled("ev", ProviderKind::Ollama, joined, "").build(|_| false).expect_err("no request goes there");
        assert_eq!(problem, Problem::BaseUnusable(joined.to_owned()));
        assert_eq!(problem.field(), BASE_FIELD);
        for broken in ["http://", "http://ev makine:11434", "https://[::1"] {
            assert!(
                matches!(
                    filled("ev", ProviderKind::Ollama, broken, "").build(|_| false),
                    Err(Problem::BaseUnusable(_))
                ),
                "{broken}"
            );
        }
        assert!(filled("ev", ProviderKind::Ollama, "http://[::1]:11434", "").build(|_| false).is_ok());
    }

    #[test]
    fn a_kind_that_is_reached_with_a_key_is_not_added_without_one() {
        let without = filled("yol", ProviderKind::OpenRouter, "https://openrouter.ai", "");
        assert_eq!(without.build(|_| false), Err(Problem::NoKey(ProviderKind::OpenRouter)));
        assert_eq!(Problem::NoKey(ProviderKind::OpenRouter).field(), KEY_FIELD);
        let with = filled("yol", ProviderKind::OpenRouter, "https://openrouter.ai", "not-a-real-key-0000-wxyz");
        let entry = with.build(|_| false).expect("the key is there");
        assert_eq!(entry.key.expect("a key").last_four().as_deref(), Some("wxyz"));
    }

    #[test]
    fn changing_the_kind_moves_an_address_nobody_typed_and_leaves_one_they_did() {
        let mut draft = Draft::new();
        assert_eq!(draft.kind, ProviderKind::Ollama);
        draft.pick_kind(ProviderKind::OpenRouter);
        assert_eq!(draft.base, ProviderKind::OpenRouter.suggested_base(), "the offer follows the kind");

        let mut own = Draft::new();
        own.base = "http://192.168.122.1:11434".to_owned();
        own.pick_kind(ProviderKind::OpenRouter);
        assert_eq!(own.base, "http://192.168.122.1:11434", "an address the person typed is theirs");
    }
}
