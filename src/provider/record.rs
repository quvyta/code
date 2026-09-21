//! What a provider is: the tag that names it, the kind of service it is, where it answers, the
//! shape it speaks, what it can run and the key it is reached with.

use super::{Key, Tag};

/// A kind of provider QCode knows how to ask.
///
/// Both of them speak the Anthropic message shape at `/v1/messages` and the OpenAI one at
/// `/v1/chat/completions`, under [`ProviderKind::api_root`], which is why a harness can be
/// pointed straight at them in its own shape and no translating endpoint is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// An ollama server, the person's own or one on their network.
    Ollama,
    /// OpenRouter.
    OpenRouter,
}

/// The shape of the request a provider is sent, and of the answer that comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// The Anthropic message shape: `/v1/messages`, and `usage.input_tokens` in the answer.
    Anthropic,
    /// The OpenAI completion shape: `/v1/chat/completions`, and `usage.prompt_tokens`.
    OpenAi,
}

impl ProviderKind {
    /// Every kind, in the order the page offers them.
    pub const ALL: [Self; 2] = [Self::Ollama, Self::OpenRouter];

    /// How the kind is written in the providers file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenRouter => "openrouter",
        }
    }

    /// The kind written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// The address the page offers when this kind is picked. An ollama server is usually the
    /// machine QCode runs on; OpenRouter is always at the one address.
    #[must_use]
    pub fn suggested_base(self) -> &'static str {
        match self {
            Self::Ollama => "http://127.0.0.1:11434",
            Self::OpenRouter => "https://openrouter.ai",
        }
    }

    /// Whether this kind is reached with a key. An ollama server on the person's own network
    /// asks for none, which is why a profile on one carries no secret into a container at all.
    #[must_use]
    pub fn needs_key(self) -> bool {
        match self {
            Self::Ollama => false,
            Self::OpenRouter => true,
        }
    }

    /// The shape this kind speaks by default.
    #[must_use]
    pub fn wire(self) -> Wire {
        Wire::Anthropic
    }

    /// Where this kind's API sits under the address a person knows it by, which is not always
    /// the address itself.
    ///
    /// OpenRouter is known as `https://openrouter.ai`, and that is the address its page names,
    /// but its API answers under `/api`: `https://openrouter.ai/api/v1/messages`. The same path
    /// without it is the website, which answers a message with `200` and a page of HTML — a
    /// harness reading that as a model's answer is told nothing true about what went wrong. An
    /// ollama server answers at its own root.
    #[must_use]
    pub fn api_root(self) -> &'static str {
        match self {
            Self::Ollama => "",
            Self::OpenRouter => "/api",
        }
    }
}

impl Wire {
    /// Every shape, in the order the page offers them.
    pub const ALL: [Self; 2] = [Self::Anthropic, Self::OpenAi];

    /// How the shape is written in the providers file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }

    /// The shape written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|wire| wire.id() == id)
    }

    /// Where a message is sent, under the provider's base address.
    #[must_use]
    pub fn messages_path(self) -> &'static str {
        match self {
            Self::Anthropic => "/v1/messages",
            Self::OpenAi => "/v1/chat/completions",
        }
    }
}

/// What a server was seen to accept, as opposed to what its model claims.
///
/// The two are not the same thing and the difference is the point: an ollama server answered for
/// a model whose own record says 262 144 tokens and then silently threw away everything past
/// about three thousand. A window is only ever reported as [`About`](Measured::About) when
/// growth was seen to stop; when the largest probe still grew, all that is known is that the
/// window is at least that big, and saying more would be inventing a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measured {
    /// Growth stopped here: prompts larger than this are cut, whatever the model claims.
    About(u64),
    /// Every probe was taken whole, so the window is at least this and its edge was not found.
    AtLeast(u64),
}

impl Measured {
    /// The number itself.
    #[must_use]
    pub fn tokens(self) -> u64 {
        match self {
            Self::About(tokens) | Self::AtLeast(tokens) => tokens,
        }
    }

    /// How it is written in the providers file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::About(_) => "about",
            Self::AtLeast(_) => "at-least",
        }
    }

    /// The measurement of `tokens` written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str, tokens: u64) -> Option<Self> {
        match id {
            "about" => Some(Self::About(tokens)),
            "at-least" => Some(Self::AtLeast(tokens)),
            _ => None,
        }
    }

    /// Whether a window this size leaves a coding agent enough room to work.
    ///
    /// A harness sends its own instructions, the files that are open and the conversation so
    /// far. Below this the instructions alone no longer fit, the rest is thrown away without a
    /// word, and the agent behaves as though it never saw the files.
    #[must_use]
    pub fn is_cramped(self) -> bool {
        matches!(self, Self::About(tokens) if tokens < CRAMPED)
    }
}

/// The window below which a coding agent loses its own instructions. Measured against the
/// owner's server, whose compatibility endpoints pinned the window to 4096 and cut prompts at
/// about 2 050 and 3 012 input tokens.
pub const CRAMPED: u64 = 16_384;

/// One model of a provider, with both of its context figures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// The model's name as the provider writes it, which is what a harness is given.
    pub id: String,
    /// The window the model's own record claims, as the provider's API reports it.
    pub claimed: Option<u64>,
    /// The window this server was seen to accept, when it has been measured.
    pub measured: Option<Measured>,
}

impl Model {
    /// A model nothing has been asked about yet.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), claimed: None, measured: None }
    }

    /// Whether the server gives this model far less room than the model claims, which is the one
    /// thing a person has to be told before they point a coding agent at it.
    #[must_use]
    pub fn is_short_changed(&self) -> bool {
        match (self.claimed, self.measured) {
            (Some(claimed), Some(measured)) => measured.tokens() * 2 < claimed,
            _ => false,
        }
    }
}

/// A provider as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEntry {
    /// The person's own name for it, which stands in front of a model name.
    pub tag: Tag,
    /// Which service it is.
    pub kind: ProviderKind,
    /// Where it answers, without a trailing slash.
    pub base: String,
    /// The shape it speaks.
    pub wire: Wire,
    /// What it was last seen to offer. Empty until it has been asked.
    pub models: Vec<Model>,
    /// The key it is reached with, for a provider that needs one.
    pub key: Option<Key>,
}

impl ProviderEntry {
    /// A provider of `kind` at `base`, named `tag`, with nothing asked of it yet.
    #[must_use]
    pub fn new(tag: Tag, kind: ProviderKind, base: &str) -> Self {
        Self { tag, kind, base: trim_base(base), wire: kind.wire(), models: Vec::new(), key: None }
    }

    /// The address `path` sits at under this provider's base, which is what the page shows
    /// before a request goes out.
    #[must_use]
    pub fn address(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The address `path` of this provider's API sits at: under [`ProviderKind::api_root`],
    /// unless the base a person wrote already ends there. OpenRouter's own instructions for
    /// Claude Code name `https://openrouter.ai/api` as the base, so a person who copied that must
    /// not be sent to `/api/api`.
    #[must_use]
    pub fn api_address(&self, path: &str) -> String {
        let root = self.kind.api_root();
        if self.base.ends_with(root) { self.address(path) } else { format!("{}{root}{path}", self.base) }
    }

    /// Where a message to `model` would go. The page prints this before the person presses
    /// anything, so that nothing leaves the machine towards an address they have not read.
    #[must_use]
    pub fn messages_address(&self) -> String {
        self.api_address(self.wire.messages_path())
    }

    /// The model named `id`, when this provider was last seen to have one.
    #[must_use]
    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.id == id)
    }
}

/// An address with the trailing slashes taken off, so that joining a path to it never doubles
/// one. A person pasting an address from a browser brings the slash with them.
#[must_use]
pub fn trim_base(base: &str) -> String {
    base.trim().trim_end_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str) -> Tag {
        Tag::parse(name).expect("a tag")
    }

    #[test]
    fn a_pasted_address_never_doubles_its_slash() {
        let entry = ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "  http://192.168.122.1:11434/  ");
        assert_eq!(entry.base, "http://192.168.122.1:11434");
        assert_eq!(entry.messages_address(), "http://192.168.122.1:11434/v1/messages");
        assert_eq!(entry.address("/api/tags"), "http://192.168.122.1:11434/api/tags");
    }

    #[test]
    fn an_openai_shaped_provider_is_asked_at_the_other_path() {
        let mut entry = ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://h:1");
        assert_eq!(entry.messages_address(), "http://h:1/v1/messages", "both kinds speak Anthropic by default");
        entry.wire = Wire::OpenAi;
        assert_eq!(entry.messages_address(), "http://h:1/v1/chat/completions");
    }

    #[test]
    fn openrouter_is_asked_under_its_api_whichever_of_its_two_addresses_was_written() {
        // Measured against the real service: `https://openrouter.ai/v1/messages` is the website
        // and answers `200` with HTML; only the path under `/api` is the API.
        for base in [
            "https://openrouter.ai",
            "https://openrouter.ai/",
            "https://openrouter.ai/api",
            "https://openrouter.ai/api/",
        ] {
            let entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, base);
            assert_eq!(entry.messages_address(), "https://openrouter.ai/api/v1/messages", "{base}");
            assert_eq!(entry.api_address("/v1/models"), "https://openrouter.ai/api/v1/models", "{base}");
        }
        let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
        entry.wire = Wire::OpenAi;
        assert_eq!(entry.messages_address(), "https://openrouter.ai/api/v1/chat/completions");
        let ollama = ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://h:1");
        assert_eq!(ollama.api_address("/v1/models"), "http://h:1/v1/models", "an ollama server answers at its root");
    }

    #[test]
    fn every_written_value_reads_back_as_itself() {
        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::parse(kind.id()), Some(kind));
        }
        for wire in Wire::ALL {
            assert_eq!(Wire::parse(wire.id()), Some(wire));
        }
        for measured in [Measured::About(7), Measured::AtLeast(7)] {
            assert_eq!(Measured::parse(measured.id(), 7), Some(measured));
        }
        assert_eq!(ProviderKind::parse("ollamaa"), None);
        assert_eq!(Wire::parse("anthropics"), None);
        assert_eq!(Measured::parse("exactly", 7), None);
    }

    #[test]
    fn only_a_window_whose_edge_was_found_is_called_cramped() {
        assert!(Measured::About(3_012).is_cramped(), "the edge was found and it is small");
        assert!(!Measured::AtLeast(3_012).is_cramped(), "a probe that never stopped growing says nothing yet");
        assert!(!Measured::About(65_536).is_cramped());
        assert_eq!(Measured::About(3_012).tokens(), 3_012);
    }

    #[test]
    fn a_server_giving_far_less_than_the_model_claims_is_what_the_person_is_told() {
        // The numbers are the ones measured against the owner's own server.
        let short = Model { id: "qwen3.8".to_owned(), claimed: Some(262_144), measured: Some(Measured::About(3_012)) };
        assert!(short.is_short_changed());
        let honest = Model { id: "q".to_owned(), claimed: Some(32_768), measured: Some(Measured::AtLeast(32_768)) };
        assert!(!honest.is_short_changed());
        assert!(!Model::new("q").is_short_changed(), "nothing is claimed about a model nobody asked about");
    }

    #[test]
    fn only_openrouter_asks_for_a_key() {
        assert!(!ProviderKind::Ollama.needs_key(), "a local server carries no secret into a container");
        assert!(ProviderKind::OpenRouter.needs_key());
    }
}
