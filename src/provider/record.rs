//! What a provider is: the tag that names it, the kind of service it is, where it answers, the
//! shape it speaks, what it can run and the key it is reached with.

use super::{Key, Tag};

/// A kind of provider QCode knows how to ask.
///
/// Every one of them speaks the Anthropic message shape at `/v1/messages` and the OpenAI one at
/// `/v1/chat/completions`, under [`ProviderKind::api_root`], which is why a harness can be
/// pointed straight at them in its own shape and no translating endpoint is built.
///
/// The last two are ready-made: picking one fills in where it answers, where its API sits for
/// each shape, which header its key goes in and the models it offers, so the person types their
/// key and nothing else. Each of those facts was asked of the real service with a real key
/// (2026-09-23) and the addresses are the ones the maker's own pages name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// An ollama server, the person's own or one on their network.
    Ollama,
    /// OpenRouter.
    OpenRouter,
    /// Xiaomi MiMo's Token Plan, the subscription. Its keys (`tp-…`) are not the pay-as-you-go
    /// account's (`sk-…` at `api.xiaomimimo.com`) and neither is accepted by the other's address.
    MimoTokenPlan,
    /// Kimi Code, Moonshot's coding subscription.
    KimiCode,
}

/// The shape of the request a provider is sent, and of the answer that comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// The Anthropic message shape: `/v1/messages`, and `usage.input_tokens` in the answer.
    Anthropic,
    /// The OpenAI completion shape: `/v1/chat/completions`, and `usage.prompt_tokens`.
    OpenAi,
}

/// One of the addresses a ready-made provider answers at, which is a choice of where in the world
/// the person's subscription lives rather than of anything they could type better themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// How the region is named in the language files, under `provider.region-`.
    pub id: &'static str,
    /// The address, as the maker's own page writes it without the path of either shape.
    pub base: &'static str,
}

/// A model a ready-made provider offers, as its maker publishes it: the window it gives and the
/// page that says so, so that a figure QCode did not ask the service for always says where it
/// was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Published {
    /// The model's name, as the service takes it.
    pub id: &'static str,
    /// The window the maker publishes for it.
    pub window: u64,
    /// Where that was read.
    pub source: &'static str,
}

/// Xiaomi's own model page gives every one of these "Context Window: 1M", and its own opencode
/// instructions write that as 1 048 576. The service's `/v1/models` names the models and no
/// window, so this is the only honest figure there is.
const MIMO_MODELS: &str = "https://mimo.mi.com/static/docs/quick-start/summary/model.md";

/// Kimi's own Claude Code page, whose table gives each model's window by plan. `k3` is 262 144 on
/// the plans below Pro and 1 048 576 above; the smaller is written, because it is the one every
/// plan that has the model gets. The service's `/v1/models` gives the window itself for the
/// models it lists, and that answer is taken over this one whenever it is there.
const KIMI_MODELS: &str = "https://www.kimi.com/code/docs/en/third-party-tools/claude-code";

const MIMO_PUBLISHED: [Published; 4] = [
    Published { id: "mimo-v2.6-pro", window: 1_048_576, source: MIMO_MODELS },
    Published { id: "mimo-v2.6-flash", window: 1_048_576, source: MIMO_MODELS },
    Published { id: "mimo-v2.5-pro", window: 1_048_576, source: MIMO_MODELS },
    Published { id: "mimo-v2.5", window: 1_048_576, source: MIMO_MODELS },
];

const KIMI_PUBLISHED: [Published; 4] = [
    Published { id: "kimi-for-coding", window: 1_048_576, source: KIMI_MODELS },
    Published { id: "k3", window: 262_144, source: KIMI_MODELS },
    Published { id: "k3-256k", window: 262_144, source: KIMI_MODELS },
    Published { id: "kimi-for-coding-highspeed", window: 262_144, source: KIMI_MODELS },
];

/// The Token Plan's three clusters, from Xiaomi's Token Plan quick-access page
/// (`mimo.mi.com/static/docs/tokenplan/Token Plan/quick-access.md`). A key works only at the
/// cluster the subscription page names, so the person picks the one they were shown.
const MIMO_REGIONS: [Region; 3] = [
    Region { id: "europe", base: "https://token-plan-ams.xiaomimimo.com" },
    Region { id: "singapore", base: "https://token-plan-sgp.xiaomimimo.com" },
    Region { id: "china", base: "https://token-plan-cn.xiaomimimo.com" },
];

/// Kimi Code's two addresses, from its own overview page (`kimi.com/code/docs/en/`), which calls
/// the first China's and the second the one for everywhere else. The same key was accepted at
/// both.
const KIMI_REGIONS: [Region; 2] =
    [Region { id: "china", base: "https://api.kimi.com" }, Region { id: "overseas", base: "https://api.kimi.ai" }];

impl ProviderKind {
    /// Every kind, in the order the page offers them.
    pub const ALL: [Self; 4] = [Self::Ollama, Self::OpenRouter, Self::MimoTokenPlan, Self::KimiCode];

    /// How the kind is written in the providers file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenRouter => "openrouter",
            Self::MimoTokenPlan => "mimo-token-plan",
            Self::KimiCode => "kimi-code",
        }
    }

    /// The kind written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// The address the page offers when this kind is picked. An ollama server is usually the
    /// machine QCode runs on; the others are the service's own, and for a ready-made one the
    /// first of its [`regions`](Self::regions).
    #[must_use]
    pub fn suggested_base(self) -> &'static str {
        match self {
            Self::Ollama => "http://127.0.0.1:11434",
            Self::OpenRouter => "https://openrouter.ai",
            Self::MimoTokenPlan => MIMO_REGIONS[0].base,
            Self::KimiCode => KIMI_REGIONS[0].base,
        }
    }

    /// The tag the page offers for a ready-made kind, so that a person who only has a key to
    /// give is not stopped by a name they had no reason to think about. Kinds whose address is
    /// the person's own offer none: two ollama servers are what a tag tells apart.
    #[must_use]
    pub fn suggested_tag(self) -> Option<&'static str> {
        match self {
            Self::Ollama | Self::OpenRouter => None,
            Self::MimoTokenPlan => Some("mimo"),
            Self::KimiCode => Some("kimi"),
        }
    }

    /// The addresses a ready-made kind can be picked at; empty for a kind whose address is
    /// written by hand.
    #[must_use]
    pub fn regions(self) -> &'static [Region] {
        match self {
            Self::Ollama | Self::OpenRouter => &[],
            Self::MimoTokenPlan => &MIMO_REGIONS,
            Self::KimiCode => &KIMI_REGIONS,
        }
    }

    /// Whether this kind is reached with a key. An ollama server on the person's own network
    /// asks for none, which is why a profile on one carries no secret into a container at all.
    #[must_use]
    pub fn needs_key(self) -> bool {
        match self {
            Self::Ollama => false,
            Self::OpenRouter | Self::MimoTokenPlan | Self::KimiCode => true,
        }
    }

    /// The shape this kind speaks by default.
    #[must_use]
    pub fn wire(self) -> Wire {
        Wire::Anthropic
    }

    /// The header a key goes in, and what stands in front of it there.
    ///
    /// Each is the header the maker's own instructions name: Xiaomi's show `api-key`, Kimi's
    /// Claude Code page sets `ANTHROPIC_API_KEY`, which Claude Code sends as `x-api-key`. Both
    /// services were seen to accept `Authorization: Bearer` too, in both shapes, but the header a
    /// service documents is the one it will keep accepting.
    #[must_use]
    pub fn key_header(self) -> (&'static str, &'static str) {
        match self {
            Self::Ollama | Self::OpenRouter => ("Authorization", "Bearer "),
            Self::MimoTokenPlan => ("api-key", ""),
            Self::KimiCode => ("x-api-key", ""),
        }
    }

    /// Where this kind's API sits under the address a person knows it by, for a request of
    /// shape `wire` — which is not always the address itself, and not always the same place for
    /// both shapes.
    ///
    /// OpenRouter is known as `https://openrouter.ai`, and that is the address its page names,
    /// but its API answers under `/api`: `https://openrouter.ai/api/v1/messages`. The same path
    /// without it is the website, which answers a message with `200` and a page of HTML — a
    /// harness reading that as a model's answer is told nothing true about what went wrong. An
    /// ollama server answers at its own root. Xiaomi's Token Plan answers the Anthropic shape
    /// under `/anthropic` and the OpenAI one at its root, and its `/anthropic/v1/models` is a
    /// `404`; Kimi Code answers both under `/coding`.
    #[must_use]
    pub fn api_root(self, wire: Wire) -> &'static str {
        match (self, wire) {
            (Self::Ollama, _) | (Self::MimoTokenPlan, Wire::OpenAi) => "",
            (Self::OpenRouter, _) => "/api",
            (Self::MimoTokenPlan, Wire::Anthropic) => "/anthropic",
            (Self::KimiCode, _) => "/coding",
        }
    }

    /// The address `path` of this kind's API sits at under `base`, for a request of shape
    /// `wire`: under [`api_root`](Self::api_root), whichever of the kind's roots `base` was
    /// written with. OpenRouter's own instructions for Claude Code name
    /// `https://openrouter.ai/api` as the base, and Xiaomi's name `…/anthropic` for one shape and
    /// `…/v1` for the other, so a person who copied any of those must be sent neither to
    /// `/api/api` nor, asking in the other shape, to `/anthropic/v1/chat/completions`.
    #[must_use]
    pub fn api_address(self, base: &str, wire: Wire, path: &str) -> String {
        let base = trim_base(base);
        let mut endings: Vec<String> = Wire::ALL
            .iter()
            .map(|wire| self.api_root(*wire))
            .flat_map(|root| [format!("{root}/v1"), root.to_owned()])
            .filter(|ending| !ending.is_empty())
            .collect();
        // The longest first, so `/anthropic/v1` is taken off whole rather than leaving
        // `/anthropic` behind; `/v1` is taken off too, because the path brings it again.
        endings.sort_by_key(|ending| std::cmp::Reverse(ending.len()));
        let host = endings.iter().find_map(|ending| base.strip_suffix(ending.as_str())).unwrap_or(&base);
        format!("{host}{}{path}", self.api_root(wire))
    }

    /// The models a ready-made kind is known to offer, each with the window its maker publishes
    /// and where that was read. Empty for a kind whose models are only known by asking.
    #[must_use]
    pub fn published(self) -> &'static [Published] {
        match self {
            Self::Ollama | Self::OpenRouter => &[],
            Self::MimoTokenPlan => &MIMO_PUBLISHED,
            Self::KimiCode => &KIMI_PUBLISHED,
        }
    }

    /// Whether the window this kind claims for a model is the window a harness gets.
    ///
    /// Not for an ollama server: its record said 262 144 while its endpoints cut prompts at
    /// about three thousand, which is why only a measured window is handed to a harness there.
    /// A ready-made service is the maker's own endpoint, and what it or its maker says it gives
    /// is what it was built to give; a harness told nothing would assume a window of its own
    /// instead, which is a guess about somebody else's model.
    #[must_use]
    pub fn claims_are_served(self) -> bool {
        matches!(self, Self::MimoTokenPlan | Self::KimiCode)
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

    /// The shape a request for `path` is in, judged by the path a harness asked for. A model
    /// listing is asked of the OpenAI root: both shapes name it `/v1/models`, and the one
    /// service whose roots differ serves it only there.
    #[must_use]
    pub fn of_path(path: &str) -> Self {
        let path = path.split('?').next().unwrap_or(path);
        if path == Self::Anthropic.messages_path() { Self::Anthropic } else { Self::OpenAi }
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

    /// The window a harness on this model is told of: what this server was measured giving, or,
    /// for a kind whose claims are what it serves ([`ProviderKind::claims_are_served`]), what it
    /// claims. `None` when neither is known, and the harness is left to its own assumption.
    #[must_use]
    pub fn window(&self, kind: ProviderKind) -> Option<u64> {
        self.measured.map(Measured::tokens).or_else(|| self.claimed.filter(|_| kind.claims_are_served()))
    }

    /// Where the claimed window was read, when it is the maker's published figure rather than
    /// something the service itself answered.
    #[must_use]
    pub fn published_at(&self, kind: ProviderKind) -> Option<&'static str> {
        kind.published()
            .iter()
            .find(|published| published.id == self.id && Some(published.window) == self.claimed)
            .map(|published| published.source)
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
    ///
    /// A ready-made kind starts with the models its maker publishes, so the page and the profile
    /// wizard have something to offer before the service has been asked anything.
    pub fn new(tag: Tag, kind: ProviderKind, base: &str) -> Self {
        let models = kind
            .published()
            .iter()
            .map(|published| Model { id: published.id.to_owned(), claimed: Some(published.window), measured: None })
            .collect();
        Self { tag, kind, base: trim_base(base), wire: kind.wire(), models, key: None }
    }

    /// The address `path` sits at under this provider's base, which is what the page shows
    /// before a request goes out.
    #[must_use]
    pub fn address(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The address `path` of this provider's API sits at for a request of shape `wire`; see
    /// [`ProviderKind::api_address`].
    #[must_use]
    pub fn api_address(&self, wire: Wire, path: &str) -> String {
        self.kind.api_address(&self.base, wire, path)
    }

    /// Where a message to `model` would go. The page prints this before the person presses
    /// anything, so that nothing leaves the machine towards an address they have not read.
    #[must_use]
    pub fn messages_address(&self) -> String {
        self.api_address(self.wire, self.wire.messages_path())
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
            assert_eq!(entry.api_address(Wire::OpenAi, "/v1/models"), "https://openrouter.ai/api/v1/models", "{base}");
        }
        let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
        entry.wire = Wire::OpenAi;
        assert_eq!(entry.messages_address(), "https://openrouter.ai/api/v1/chat/completions");
        let ollama = ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://h:1");
        assert_eq!(
            ollama.api_address(Wire::OpenAi, "/v1/models"),
            "http://h:1/v1/models",
            "an ollama server answers at its root"
        );
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
    fn only_a_local_server_is_reached_without_a_key() {
        assert!(!ProviderKind::Ollama.needs_key(), "a local server carries no secret into a container");
        assert!(ProviderKind::OpenRouter.needs_key());
        assert!(ProviderKind::MimoTokenPlan.needs_key());
        assert!(ProviderKind::KimiCode.needs_key());
    }

    #[test]
    fn xiaomi_is_asked_under_anthropic_in_one_shape_and_at_its_root_in_the_other() {
        // Measured with a real key on 2026-09-23: `/anthropic/v1/messages` and
        // `/v1/chat/completions` answer, `/v1/models` lists, `/anthropic/v1/models` is a 404.
        for base in [
            "https://token-plan-ams.xiaomimimo.com",
            "https://token-plan-ams.xiaomimimo.com/",
            "https://token-plan-ams.xiaomimimo.com/anthropic",
            "https://token-plan-ams.xiaomimimo.com/v1",
            "https://token-plan-ams.xiaomimimo.com/anthropic/v1/",
        ] {
            let entry = ProviderEntry::new(tag("mimo"), ProviderKind::MimoTokenPlan, base);
            assert_eq!(
                entry.messages_address(),
                "https://token-plan-ams.xiaomimimo.com/anthropic/v1/messages",
                "{base}"
            );
            assert_eq!(
                entry.api_address(Wire::OpenAi, "/v1/chat/completions"),
                "https://token-plan-ams.xiaomimimo.com/v1/chat/completions",
                "{base}"
            );
            assert_eq!(
                entry.api_address(Wire::of_path("/v1/models"), "/v1/models"),
                "https://token-plan-ams.xiaomimimo.com/v1/models",
                "{base}: the listing is only at the root"
            );
        }
    }

    #[test]
    fn kimi_is_asked_under_coding_in_both_shapes() {
        for base in ["https://api.kimi.com", "https://api.kimi.com/coding", "https://api.kimi.com/coding/v1"] {
            let entry = ProviderEntry::new(tag("kimi"), ProviderKind::KimiCode, base);
            assert_eq!(entry.messages_address(), "https://api.kimi.com/coding/v1/messages", "{base}");
            assert_eq!(
                entry.api_address(Wire::OpenAi, "/v1/chat/completions"),
                "https://api.kimi.com/coding/v1/chat/completions",
                "{base}"
            );
        }
    }

    #[test]
    fn a_harness_says_its_shape_by_the_path_it_asks_for() {
        assert_eq!(Wire::of_path("/v1/messages"), Wire::Anthropic);
        assert_eq!(Wire::of_path("/v1/messages?beta=true"), Wire::Anthropic, "what Claude Code really sends");
        assert_eq!(Wire::of_path("/v1/chat/completions"), Wire::OpenAi);
        assert_eq!(Wire::of_path("/v1/models"), Wire::OpenAi, "the one root every kind lists at");
    }

    #[test]
    fn each_kind_puts_its_key_in_the_header_its_maker_names() {
        assert_eq!(ProviderKind::Ollama.key_header(), ("Authorization", "Bearer "));
        assert_eq!(ProviderKind::OpenRouter.key_header(), ("Authorization", "Bearer "));
        assert_eq!(ProviderKind::MimoTokenPlan.key_header(), ("api-key", ""));
        assert_eq!(ProviderKind::KimiCode.key_header(), ("x-api-key", ""));
    }

    #[test]
    fn a_ready_made_kind_starts_with_its_makers_models_and_a_kind_of_ones_own_with_none() {
        let mimo =
            ProviderEntry::new(tag("mimo"), ProviderKind::MimoTokenPlan, ProviderKind::MimoTokenPlan.suggested_base());
        let ids: Vec<&str> = mimo.models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["mimo-v2.6-pro", "mimo-v2.6-flash", "mimo-v2.5-pro", "mimo-v2.5"]);
        let flash = mimo.model("mimo-v2.6-flash").expect("the model");
        assert_eq!(flash.claimed, Some(1_048_576));
        assert!(
            flash
                .published_at(ProviderKind::MimoTokenPlan)
                .is_some_and(|source| source.starts_with("https://mimo.mi.com/"))
        );
        let kimi = ProviderEntry::new(tag("kimi"), ProviderKind::KimiCode, ProviderKind::KimiCode.suggested_base());
        assert_eq!(kimi.model("kimi-for-coding").and_then(|model| model.claimed), Some(1_048_576));
        assert!(ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://h:1").models.is_empty());
    }

    #[test]
    fn a_harness_is_told_a_claimed_window_only_where_the_claim_is_what_is_served() {
        let claimed = Model { id: "m".to_owned(), claimed: Some(262_144), measured: None };
        // An ollama server's record said 262 144 while it cut prompts at three thousand.
        assert_eq!(claimed.window(ProviderKind::Ollama), None);
        assert_eq!(claimed.window(ProviderKind::OpenRouter), None);
        assert_eq!(claimed.window(ProviderKind::MimoTokenPlan), Some(262_144));
        assert_eq!(claimed.window(ProviderKind::KimiCode), Some(262_144));
        let measured = Model { measured: Some(Measured::About(31_512)), ..claimed };
        for kind in ProviderKind::ALL {
            assert_eq!(measured.window(kind), Some(31_512), "{kind:?}: what was measured comes first");
        }
    }

    #[test]
    fn every_region_offered_is_an_address_of_its_own_kind() {
        for kind in ProviderKind::ALL {
            for region in kind.regions() {
                assert!(region.base.starts_with("https://"), "{kind:?} {region:?}");
                assert_eq!(trim_base(region.base), region.base, "{kind:?}: no trailing slash");
            }
            if let Some(first) = kind.regions().first() {
                assert_eq!(first.base, kind.suggested_base(), "{kind:?}: the offered address is the first region");
            }
        }
        assert_eq!(ProviderKind::MimoTokenPlan.regions().len(), 3);
        assert_eq!(ProviderKind::KimiCode.suggested_base(), "https://api.kimi.com");
    }
}
