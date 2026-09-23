//! The profile definition file: `Profiles/<profile>.toml`.

use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, ValueKind};
use qframe::storage::Settings;

use crate::base::Os;
use crate::engine::names;
use crate::profile::{AccountKind, Addition, ConfigFile, Extra, HarnessKind, SafeName, Template};
use crate::provider::Wire;

/// Whether a directory is mounted into the container writable or read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAccess {
    /// The harness may change what it finds.
    ReadWrite,
    /// The harness may read, and nothing it does can change the directory.
    ReadOnly,
}

/// Whether the container can reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// The container reaches the network as the machine does.
    Full,
    /// The container reaches nothing.
    None,
}

/// The provider and model a profile of [`AccountKind::Provider`] runs on: the person's own tag
/// from the Providers page, and one of that provider's models.
///
/// The tag is kept as the text it was written with rather than a checked
/// [`crate::provider::Tag`]: a profile is read long after the provider it names may have been
/// removed, and a tag the file cannot make sense of any more is still worth showing back to the
/// person as the reason nothing starts, not a reason to lose the rest of the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChoice {
    /// The provider's tag, as the Providers page names it.
    pub tag: String,
    /// The model asked for, as the provider itself names it.
    pub model: String,
}

/// The environment variable Claude Code reads for another endpoint's address in place of the
/// real one, checked against its own documentation.
pub const ANTHROPIC_BASE_URL: &str = "ANTHROPIC_BASE_URL";
/// The environment variable Claude Code reads for the token it sends that endpoint. The relay
/// strips this header before it ever asks the provider, so the value only has to be non-empty:
/// it is never the provider's key.
pub const ANTHROPIC_AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
/// The environment variable Claude Code reads for which model to ask for.
pub const ANTHROPIC_MODEL: &str = "ANTHROPIC_MODEL";
/// The environment variable Claude Code reads for how much room the model really has.
pub const MAX_CONTEXT_TOKENS: &str = "CLAUDE_CODE_MAX_CONTEXT_TOKENS";
/// The room Claude Code assumes a model it has never heard of has, when `CLAUDE_CODE_MAX_CONTEXT_TOKENS`
/// does not say otherwise. The profile wizard names it beside a model whose window nobody has
/// measured yet, because it is the figure Claude Code will then work to.
pub const ASSUMED_CONTEXT_TOKENS: u64 = 200_000;

/// The environment variable opencode reads a whole configuration from, merged over the file in
/// its home, checked against 1.18.31: the program's text holds
/// `process.env.OPENCODE_CONFIG_CONTENT` and logs `loaded custom config from
/// OPENCODE_CONFIG_CONTENT` when it is set.
pub const OPENCODE_CONFIG_CONTENT: &str = "OPENCODE_CONFIG_CONTENT";

impl ProviderChoice {
    /// The environment a tab of `harness` on this provider is started with, so the harness
    /// speaks to the workspace's relay at `http://127.0.0.1:<`[`crate::provider::relay::PORT`]`>`
    /// instead of the provider's own address, asking for [`ProviderChoice::model`]. Empty for a
    /// harness that is not offered a provider of one's own.
    ///
    /// `window` is what QCode measured this server really gives for that model, when it has been
    /// measured: the harness is told that rather than left to assume its own models' room.
    ///
    /// `token` is the tab's own bridge token, carried again rather than a second one minted for
    /// it: the relay resolves a tab's provider entry by the very token
    /// [`crate::bridge::TOKEN_VARIABLE`] already carries, and the harness needs some non-empty
    /// value here to send a request at all, never the provider's real key — that never leaves
    /// this machine's own process.
    #[must_use]
    pub fn environment(&self, harness: HarnessKind, token: &str, window: Option<u64>) -> Vec<(String, String)> {
        match harness {
            HarnessKind::ClaudeCode => self.claude_code(token, window),
            HarnessKind::OpenCode => vec![(OPENCODE_CONFIG_CONTENT.to_owned(), self.opencode(token, window))],
            HarnessKind::GeminiCli | HarnessKind::Codex | HarnessKind::AntigravityIde => Vec::new(),
        }
    }

    /// The address a harness inside the container is given for this provider in shape `wire`:
    /// always the workspace's relay, written the way a client of that shape expects its base —
    /// the bare address for the Anthropic shape, which clients follow with `/v1/messages`, and
    /// `/v1` for the OpenAI one, which clients follow with `/chat/completions`. Where the
    /// provider really answers that shape, and which header its key goes in, is the relay's to
    /// know; a harness is only ever told this address.
    #[must_use]
    pub fn endpoint(wire: Wire) -> String {
        match wire {
            Wire::Anthropic => relay_address(),
            Wire::OpenAi => format!("{}/v1", relay_address()),
        }
    }

    /// Claude Code speaks the Anthropic message shape and is pointed at another endpoint by
    /// three variables it reads once, as it starts.
    fn claude_code(&self, token: &str, window: Option<u64>) -> Vec<(String, String)> {
        let mut environment = vec![
            (ANTHROPIC_BASE_URL.to_owned(), Self::endpoint(Wire::Anthropic)),
            (ANTHROPIC_AUTH_TOKEN.to_owned(), token.to_owned()),
            (ANTHROPIC_MODEL.to_owned(), self.model.clone()),
        ];
        // A model a harness has never heard of is assumed to have the room the harness's own
        // models have, which for Claude Code is 200 000 tokens. A server that really gives
        // thirty thousand then loses the front of every larger prompt without a word, and the
        // agent behaves as though it never read what it was shown. So the number QCode measured
        // is handed over: it is the one number here nobody else can know.
        if let Some(window) = window {
            environment.push((MAX_CONTEXT_TOKENS.to_owned(), window.to_string()));
        }
        environment
    }

    /// opencode speaks the OpenAI completion shape to a provider it is told of in its own
    /// configuration: an OpenAI-compatible one named after the person's tag, at the relay's
    /// `/v1`, with the tab's token as its key and the chosen model as the one it uses — for the
    /// conversation and for the short titles it writes beside it, which would otherwise go to a
    /// provider of opencode's own choosing that a container with no network cannot reach.
    ///
    /// Handed over in [`OPENCODE_CONFIG_CONTENT`] rather than written into the image, so that
    /// the token, which is the tab's own and new for every tab, never lands in a file; opencode
    /// merges it over the settings the image and the bridge wrote, which keep their permission
    /// and their MCP server.
    fn opencode(&self, token: &str, window: Option<u64>) -> String {
        let mut model = serde_json::json!({ "name": self.model });
        // opencode compacts a conversation before it outgrows the window it was told of; told of
        // none, it never does, and a server that cuts the front of a long prompt cuts it in
        // silence. Only a measured window is handed over. Its configuration will not take a
        // window without the room for an answer beside it (1.18.31: `Missing key
        // ...limit.output`), and 0 there is opencode's own ceiling rather than a number QCode
        // would have to invent: it reads the room as `Math.min(limit.output, max) || max`.
        if let Some(window) = window {
            model["limit"] = serde_json::json!({ "context": window, "output": 0 });
        }
        let chosen = format!("{}/{}", self.tag, self.model);
        serde_json::json!({
            "provider": {
                self.tag.as_str(): {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": self.tag,
                    "options": { "baseURL": Self::endpoint(Wire::OpenAi), "apiKey": token },
                    "models": { self.model.as_str(): model },
                },
            },
            "model": chosen,
            "small_model": chosen,
        })
        .to_string()
    }
}

/// Where a harness inside a profile container reaches the relay: its loopback interface, on the
/// relay's own port.
fn relay_address() -> String {
    format!("http://127.0.0.1:{}", crate::provider::relay::PORT)
}

/// A profile as its definition file describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// The profile's name, which is also its file name and part of every name it is built into.
    pub name: SafeName,
    /// The harness the profile runs.
    pub harness: HarnessKind,
    /// How much of the harness's own configuration the image carries.
    pub template: Template,
    /// What the profile signs in with.
    pub account: AccountKind,
    /// The provider and model this profile runs on, when [`Profile::account`] is
    /// [`AccountKind::Provider`]; `None` otherwise.
    pub provider: Option<ProviderChoice>,
    /// How `Assets/` is mounted.
    pub assets: MountAccess,
    /// What the container may reach.
    pub network: NetworkMode,
    /// The parts of QCode high's additions the person switched off for this profile. Empty means
    /// everything its template adds, which is what a file without the choice reads as.
    pub without: Vec<Extra>,
    /// The system the profile's image is built on. Debian, which every profile was built on before
    /// there was a choice, is what a file that names none reads as.
    pub os: Os,
}

/// What reading a definition file gave: the profile when the file holds one, and everything
/// worth telling the user either way.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// The profile, unless something the file could not do without was missing or unusable.
    pub profile: Option<Profile>,
    /// What was wrong with the file, each with its place in it where there is one.
    pub diagnostics: Vec<Diagnostic>,
}

impl MountAccess {
    /// Every access, in the order the profile wizard offers them.
    pub const ALL: [Self; 2] = [Self::ReadWrite, Self::ReadOnly];

    /// How the access is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::ReadWrite => "rw",
            Self::ReadOnly => "ro",
        }
    }

    /// The access written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|access| access.id() == id)
    }
}

impl NetworkMode {
    /// Every mode, in the order the profile wizard offers them.
    pub const ALL: [Self; 2] = [Self::Full, Self::None];

    /// How the mode is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::None => "none",
        }
    }

    /// The mode written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

impl Profile {
    /// How `Work/` is mounted. The workspace's own directory is what the harness is there to
    /// work on, so it is writable in every profile and the file carries the choice only to be
    /// readable.
    pub const CODE_MOUNT: MountAccess = MountAccess::ReadWrite;

    /// The image the profile's containers are started from, which follows from the name and is
    /// never stored twice.
    #[must_use]
    pub fn image(&self) -> String {
        names::profile_image(self.name.as_str())
    }

    /// Reads a definition file. Nothing here panics and nothing stops at the first mistake: a
    /// value the file cannot mean is reported with its line and column and replaced by the
    /// default, and only a name, a harness or an account that cannot be made sense of leaves the
    /// file without a profile.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> Loaded {
        let document = Document::parse(file, text, &shape());
        let mut diagnostics = document.diagnostics().to_vec();
        let root = document.root();

        // A missing key or a value outside the choice is already reported by the document; the
        // name is the one value only qcode can judge, so an unsafe one is reported here.
        let name = root.text(NAME).and_then(|name| {
            let parsed = SafeName::parse(name);
            if parsed.is_none() {
                let message = format!("`{NAME}` is `{name}`, which no file system holds; the profile cannot be used");
                diagnostics.push(Diagnostic::error(root.value_location(NAME).cloned(), message));
            }
            parsed
        });
        let harness = root.text(HARNESS).and_then(HarnessKind::parse);
        let account = root.text(ACCOUNT).and_then(AccountKind::parse);
        let template = root.text(TEMPLATE).and_then(Template::parse).unwrap_or(Template::Base);
        let os = root.text(OS).and_then(Os::parse).unwrap_or_default();
        let mounts = root.table(MOUNTS);
        let assets = mounts.and_then(|mounts| mounts.text(ASSETS)).and_then(MountAccess::parse);
        let network = root.table(NETWORK).and_then(|network| network.text(MODE)).and_then(NetworkMode::parse);
        // Only a part switched off is worth anything here: everything else is on.
        let without: Vec<Extra> = root
            .table(ADDITIONS)
            .map(|table| Extra::all().into_iter().filter(|extra| table.flag(extra.id()) == Some(false)).collect())
            .unwrap_or_default();
        let provider_table = root.table(PROVIDER);
        let provider_tag = provider_table.and_then(|table| table.text(PROVIDER_TAG)).map(str::to_owned);
        let provider_model = provider_table.and_then(|table| table.text(PROVIDER_MODEL)).map(str::to_owned);

        let (Some(name), Some(harness), Some(account)) = (name, harness, account) else {
            return Loaded { profile: None, diagnostics };
        };
        if !harness.supports(account) {
            let message =
                format!("`{ACCOUNT}` is `{}`, which {} does not offer", account.id(), harness.record().display_name);
            diagnostics.push(Diagnostic::error(root.value_location(ACCOUNT).cloned(), message));
            return Loaded { profile: None, diagnostics };
        }
        // A profile without a provider and model to run on cannot be told which container to
        // point where; that is not a value to fall back on, it is the whole of what the profile
        // is for.
        let provider = if account == AccountKind::Provider {
            let (Some(tag), Some(model)) = (provider_tag, provider_model) else {
                let message = format!(
                    "`{ACCOUNT}` is `provider`, but no `{PROVIDER}.{PROVIDER_TAG}` and \
                                        `{PROVIDER}.{PROVIDER_MODEL}` are named"
                );
                diagnostics.push(Diagnostic::error(root.value_location(ACCOUNT).cloned(), message));
                return Loaded { profile: None, diagnostics };
            };
            Some(ProviderChoice { tag, model })
        } else {
            None
        };
        // A harness the system cannot run was never offered with it, so a file that pairs them was
        // written by hand; the profile cannot be built, which is not something to fall back from
        // quietly onto another system the person did not choose.
        if os.refuses(harness).is_some() {
            let message = format!(
                "`{OS}` is `{}`, on which {} does not run; the profile cannot be built",
                os.id(),
                harness.record().display_name
            );
            diagnostics.push(Diagnostic::error(root.value_location(OS).cloned(), message));
            return Loaded { profile: None, diagnostics };
        }
        if harness.withdrawn(account) {
            let message = format!(
                "`{ACCOUNT}` is `{}`: {} stopped this sign-in for personal accounts on 2026-06-18; \
                 it still works for Gemini Code Assist Standard and Enterprise, and a new profile \
                 with an API key works for everyone",
                account.id(),
                harness.record().display_name
            );
            diagnostics.push(Diagnostic::warning(root.value_location(ACCOUNT).cloned(), message));
        }

        let profile = Profile {
            name,
            harness,
            template,
            account,
            provider,
            assets: assets.unwrap_or(MountAccess::ReadOnly),
            network: network.unwrap_or(NetworkMode::Full),
            without,
            os,
        };
        let derived = profile.image();
        if let Some(stored) = root.text(IMAGE)
            && stored != derived
        {
            let message = format!("`{IMAGE}` is `{stored}`; the profile's image is `{derived}`");
            diagnostics.push(Diagnostic::error(root.value_location(IMAGE).cloned(), message));
        }
        Loaded { profile: Some(profile), diagnostics }
    }

    /// The definition file for this profile, as it is written to the store.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut settings = Settings::in_memory();
        settings.set(NAME, self.name.as_str().to_owned());
        settings.set(HARNESS, self.harness.record().id.to_owned());
        // Debian is not written: a profile on it is the file it was before there was a choice,
        // byte for byte, so no file on anybody's disk changes by being saved again.
        if self.os != Os::Debian {
            settings.set(OS, self.os.id().to_owned());
        }
        settings.set(TEMPLATE, self.template.id().to_owned());
        settings.set(ACCOUNT, self.account.id().to_owned());
        settings.set(IMAGE, self.image());
        settings.set(&format!("{MOUNTS}.{CODE}"), Self::CODE_MOUNT.id().to_owned());
        settings.set(&format!("{MOUNTS}.{ASSETS}"), self.assets.id().to_owned());
        settings.set(&format!("{NETWORK}.{MODE}"), self.network.id().to_owned());
        if let Some(provider) = &self.provider {
            settings.set(&format!("{PROVIDER}.{PROVIDER_TAG}"), provider.tag.clone());
            settings.set(&format!("{PROVIDER}.{PROVIDER_MODEL}"), provider.model.clone());
        }
        for extra in &self.without {
            settings.set(&format!("{ADDITIONS}.{}", extra.id()), false);
        }
        settings.to_toml()
    }

    /// Whether this profile's image gets `extra`: its template adds it for this harness, and the
    /// person did not switch it off.
    #[must_use]
    pub fn has(&self, extra: Extra) -> bool {
        self.template.extras(self.harness).contains(&extra) && !self.without.contains(&extra)
    }

    /// The Claude Code plugins this profile's image gets, as `claude plugin install` takes them.
    #[must_use]
    pub fn claude_plugins(&self) -> Vec<&'static str> {
        self.template
            .extras(self.harness)
            .into_iter()
            .filter_map(|extra| match extra {
                Extra::Plugin(plugin) if self.has(extra) => Some(plugin),
                _ => None,
            })
            .collect()
    }

    /// What this profile's image gets beside the harness, in the order it is installed: its
    /// template's additions, less what the person switched off. The Claude Code plugins are one
    /// addition, there while any one of them is on.
    #[must_use]
    pub fn additions(&self) -> Vec<Addition> {
        self.template
            .additions(self.harness)
            .iter()
            .copied()
            .filter(|addition| match addition {
                Addition::Graphify => self.has(Extra::Graphify),
                Addition::ClaudePlugins => !self.claude_plugins().is_empty(),
                Addition::OhMyOpenAgent => self.has(Extra::OhMyOpenAgent),
            })
            .collect()
    }

    /// The configuration files this profile's image build writes, in the order they are written:
    /// its template's, except that opencode without oh-my-openagent gets QCode basic's settings,
    /// because QCode high's name the plugin by a path that would then lead nowhere.
    #[must_use]
    pub fn files(&self) -> Vec<ConfigFile> {
        if self.template == Template::High && self.harness == HarnessKind::OpenCode && !self.has(Extra::OhMyOpenAgent) {
            return Template::Recommended.files(self.harness);
        }
        self.template.files(self.harness)
    }
}

/// The key of the profile name.
const NAME: &str = "name";
/// The key of the harness.
const HARNESS: &str = "harness";
/// The key of the template.
const TEMPLATE: &str = "template";
/// The key of the system the image is built on; missing means Debian.
const OS: &str = "os";
/// The key of the account type.
const ACCOUNT: &str = "account";
/// The key of the profile image.
const IMAGE: &str = "image";
/// The table of mount accesses.
const MOUNTS: &str = "mounts";
/// The key, below `[mounts]`, of the access `Work/` is mounted with. The key keeps the name it
/// was written under; only the folder was renamed.
const CODE: &str = "code";
/// What that key was called while a workspace was still called a project. A profile written by
/// an older QCode still spells it this way, and it is accepted so that such a file is not
/// reported as broken; the next write puts [`CODE`] there instead.
const LEGACY_CODE: &str = "project";
/// The key, below `[mounts]`, of the access `Assets/` is mounted with.
const ASSETS: &str = "assets";
/// The table of network settings.
const NETWORK: &str = "network";
/// The key, below `[network]`, of the network mode.
const MODE: &str = "mode";
/// The table of QCode high's additions: each part by its id, `false` where the person switched it
/// off. A part not named is on.
const ADDITIONS: &str = "additions";
/// The table naming the provider and model a profile of [`AccountKind::Provider`] runs on.
const PROVIDER: &str = "provider";
/// The key, below `[provider]`, of the provider's tag.
const PROVIDER_TAG: &str = "tag";
/// The key, below `[provider]`, of the model asked for.
const PROVIDER_MODEL: &str = "model";

/// What a definition file may hold. The keys a profile cannot be guessed for are required, so
/// a missing or wrong one is an error; the rest are choices that fall back to the safer of the
/// two when they are missing or wrong.
fn shape() -> Shape {
    let mounts = Shape::new()
        .optional(CODE, ValueKind::choice([Profile::CODE_MOUNT.id()]))
        .optional(LEGACY_CODE, ValueKind::choice([Profile::CODE_MOUNT.id()]))
        .optional(ASSETS, ValueKind::choice(MountAccess::ALL.map(MountAccess::id)));
    let network = Shape::new().optional(MODE, ValueKind::choice(NetworkMode::ALL.map(NetworkMode::id)));
    let provider = Shape::new().optional(PROVIDER_TAG, ValueKind::text()).optional(PROVIDER_MODEL, ValueKind::text());
    let additions =
        Extra::all().into_iter().fold(Shape::new(), |shape, extra| shape.optional(extra.id(), ValueKind::flag()));
    Shape::new()
        .required(NAME, ValueKind::text())
        .required(HARNESS, ValueKind::choice(HarnessKind::ALL.map(|harness| harness.record().id)))
        .optional(TEMPLATE, ValueKind::choice(Template::ALL.map(Template::id)))
        .optional(OS, ValueKind::choice(Os::ALL.map(Os::id)))
        .required(ACCOUNT, ValueKind::choice(AccountKind::ALL.map(AccountKind::id)))
        .optional(IMAGE, ValueKind::text())
        .table(MOUNTS, mounts)
        .table(NETWORK, network)
        .table(PROVIDER, provider)
        .table(ADDITIONS, additions)
}

#[cfg(test)]
mod tests {
    use qframe::diagnostics::Severity;

    use super::*;
    use crate::profile::{AccountKind, HarnessKind, Template};

    const COMPLETE: &str = "\
name = \"claude-sub\"
harness = \"claude-code\"
template = \"recommended\"
account = \"subscription\"
image = \"qcode/profile/claude-sub\"

[mounts]
code = \"rw\"
assets = \"ro\"

[network]
mode = \"full\"
";

    fn parse(text: &str) -> Loaded {
        Profile::parse("claude-sub.toml", text)
    }

    #[test]
    fn a_complete_file_reads_without_a_word_of_complaint() {
        let loaded = parse(COMPLETE);
        assert_eq!(loaded.diagnostics, []);
        let profile = loaded.profile.expect("the file is complete");
        assert_eq!(profile.name.as_str(), "claude-sub");
        assert_eq!(profile.harness, HarnessKind::ClaudeCode);
        assert_eq!(profile.template, Template::Recommended);
        assert_eq!(profile.account, AccountKind::Subscription);
        assert_eq!(profile.assets, MountAccess::ReadOnly);
        assert_eq!(profile.network, NetworkMode::Full);
        assert_eq!(profile.image(), "qcode/profile/claude-sub");
    }

    #[test]
    fn what_is_written_reads_back_as_the_same_profile() {
        let profile = parse(COMPLETE).profile.expect("the file is complete");
        assert_eq!(profile.to_toml(), COMPLETE);
        assert_eq!(parse(&profile.to_toml()).profile, Some(profile));
    }

    /// A profile written before workspaces had their name says `project` where the file now says
    /// `code`. Both name the same mount, so the older file is read without a complaint.
    #[test]
    fn a_profile_that_still_calls_the_mount_a_project_reads_without_a_complaint() {
        let older = COMPLETE.replace("code = ", "project = ");
        let loaded = parse(&older);
        assert_eq!(loaded.diagnostics, []);
        let profile = loaded.profile.expect("the file is complete");
        assert_eq!(profile.assets, MountAccess::ReadOnly);
        assert_eq!(profile.to_toml(), COMPLETE, "writing it again puts the new name there");
    }

    #[test]
    fn a_missing_key_that_has_no_sensible_default_stops_the_profile() {
        let loaded = parse("harness = \"claude-code\"\naccount = \"subscription\"\n");
        assert_eq!(loaded.profile, None);
        assert!(loaded.diagnostics.iter().any(|d| d.message.contains("name")), "{:?}", loaded.diagnostics);
        let loaded = parse("name = \"claude-sub\"\naccount = \"subscription\"\n");
        assert_eq!(loaded.profile, None);
        assert!(loaded.diagnostics.iter().any(|d| d.message.contains("harness")), "{:?}", loaded.diagnostics);
    }

    #[test]
    fn a_missing_key_with_a_default_falls_back_quietly() {
        let loaded = parse("name = \"claude-sub\"\nharness = \"claude-code\"\naccount = \"api-key\"\n");
        assert_eq!(loaded.diagnostics, []);
        let profile = loaded.profile.expect("the required keys are there");
        assert_eq!(profile.template, Template::Base);
        assert_eq!(profile.assets, MountAccess::ReadOnly);
        assert_eq!(profile.network, NetworkMode::Full);
    }

    #[test]
    fn a_broken_value_points_at_its_line_and_column_instead_of_panicking() {
        let text = "name = \"claude-sub\"\nharness = \"claude-code\"\naccount = \"subscription\"\n\n[network]\nmode = \"halb\"\n";
        let loaded = parse(text);
        let at = loaded
            .diagnostics
            .iter()
            .find_map(|d| d.location.clone())
            .expect("the broken value has a place in the file");
        assert_eq!(at.to_string(), "claude-sub.toml:6:8", "the line and column of the value that went wrong");
        assert_eq!(loaded.profile.expect("the rest of the file is usable").network, NetworkMode::Full);
    }

    #[test]
    fn a_file_that_is_not_toml_is_reported_not_panicked_over() {
        let loaded = parse("name = \n");
        assert_eq!(loaded.profile, None);
        assert!(loaded.diagnostics.iter().any(|d| d.severity == Severity::Error), "{:?}", loaded.diagnostics);
    }

    #[test]
    fn a_name_the_file_system_could_not_hold_is_refused() {
        let loaded = parse("name = \"Claude Sub\"\nharness = \"claude-code\"\naccount = \"subscription\"\n");
        assert_eq!(loaded.profile, None);
        assert!(loaded.diagnostics.iter().any(|d| d.message.contains("name")), "{:?}", loaded.diagnostics);
    }

    #[test]
    fn an_account_the_harness_does_not_support_is_refused() {
        // An unknown account type is refused like any value outside the choice.
        let loaded = parse("name = \"x\"\nharness = \"codex\"\naccount = \"none\"\n");
        assert_eq!(loaded.profile, None);
        assert!(loaded.diagnostics.iter().any(|d| d.severity == Severity::Error), "{:?}", loaded.diagnostics);
    }

    #[test]
    fn a_gemini_profile_signed_in_with_google_still_loads_and_says_the_sign_in_closed() {
        let loaded = Profile::parse("g.toml", "name = \"g\"\nharness = \"gemini-cli\"\naccount = \"subscription\"\n");
        let profile = loaded.profile.expect("a Code Assist Standard or Enterprise login still works");
        assert_eq!(profile.account, AccountKind::Subscription);
        assert_eq!(loaded.diagnostics.len(), 1, "{:?}", loaded.diagnostics);
        assert_eq!(loaded.diagnostics[0].severity, Severity::Warning);
        assert!(loaded.diagnostics[0].message.contains("2026-06-18"), "{:?}", loaded.diagnostics);
    }

    #[test]
    fn free_use_loads_for_opencode_and_is_refused_where_it_is_not_offered() {
        let text = "name = \"oc\"\nharness = \"opencode\"\naccount = \"free\"\n";
        let loaded = Profile::parse("oc.toml", text);
        assert_eq!(loaded.diagnostics, []);
        let profile = loaded.profile.expect("opencode can be used for free");
        assert_eq!(profile.account, AccountKind::Free);
        assert!(profile.to_toml().contains("account = \"free\"\n"), "{}", profile.to_toml());
        assert_eq!(Profile::parse("oc.toml", &profile.to_toml()).profile, Some(profile));

        let loaded = Profile::parse("x.toml", "name = \"x\"\nharness = \"claude-code\"\naccount = \"free\"\n");
        assert_eq!(loaded.profile, None);
        let refused = loaded.diagnostics.iter().find(|d| d.severity == Severity::Error).expect("the reason is given");
        assert!(refused.message.contains("free") && refused.message.contains("Claude Code"), "{}", refused.message);
        assert_eq!(refused.location.as_ref().map(ToString::to_string).as_deref(), Some("x.toml:3:11"));
    }

    #[test]
    fn an_image_name_that_drifted_from_the_profile_name_is_reported_and_derived_again() {
        let text = "name = \"claude-sub\"\nharness = \"claude-code\"\naccount = \"subscription\"\nimage = \"qcode/profile/old\"\n";
        let loaded = parse(text);
        let profile = loaded.profile.expect("the name is still usable");
        assert_eq!(profile.image(), "qcode/profile/claude-sub");
        assert!(loaded.diagnostics.iter().any(|d| d.message.contains("image")), "{:?}", loaded.diagnostics);
    }

    #[test]
    fn an_unknown_key_is_reported_and_dropped() {
        let text = "name = \"claude-sub\"\nharness = \"claude-code\"\naccount = \"subscription\"\ncolour = \"red\"\n";
        let loaded = parse(text);
        assert!(loaded.profile.is_some());
        assert!(loaded.diagnostics.iter().any(|d| d.message.contains("colour")), "{:?}", loaded.diagnostics);
    }

    #[test]
    fn a_provider_profile_round_trips_its_tag_and_model() {
        let text = "name = \"ev\"\nharness = \"claude-code\"\naccount = \"provider\"\n\n\
                     [provider]\ntag = \"ev1\"\nmodel = \"qwen3.8\"\n";
        let loaded = Profile::parse("ev.toml", text);
        assert_eq!(loaded.diagnostics, []);
        let profile = loaded.profile.expect("tag and model are both named");
        let provider = profile.provider.as_ref().expect("a provider profile carries one");
        assert_eq!(provider.tag, "ev1");
        assert_eq!(provider.model, "qwen3.8");
        assert!(profile.to_toml().contains("[provider]"), "{}", profile.to_toml());
        assert_eq!(Profile::parse("ev.toml", &profile.to_toml()).profile, Some(profile));
    }

    /// A file written before this release names no provider at all, and still loads: the field
    /// is new, and every profile that never had a reason to hold it keeps working.
    #[test]
    fn a_file_written_before_providers_existed_still_loads_unchanged() {
        let loaded = parse(COMPLETE);
        let profile = loaded.profile.expect("the older file is still complete");
        assert_eq!(profile.provider, None);
        assert!(!profile.to_toml().contains("[provider]"), "{}", profile.to_toml());
    }

    /// Every file written before a profile could choose its system names none, and nobody's
    /// profile on disk may change because of the choice: each older shape reads as Debian without
    /// a word and is written back byte for byte as it was.
    #[test]
    fn a_file_written_before_the_system_could_be_chosen_is_debian_and_written_back_unchanged() {
        let high =
            COMPLETE.replace("template = \"recommended\"", "template = \"high\"") + "\n[additions]\ncontext7 = false\n";
        let provider = "name = \"ev\"\nharness = \"claude-code\"\ntemplate = \"recommended\"\naccount = \"provider\"\n\
                        image = \"qcode/profile/ev\"\n\n[mounts]\ncode = \"rw\"\nassets = \"rw\"\n\n[network]\n\
                        mode = \"none\"\n\n[provider]\ntag = \"ev1\"\nmodel = \"qwen3.8\"\n";
        for older in [COMPLETE.to_owned(), high, provider.to_owned()] {
            let loaded = Profile::parse("older.toml", &older);
            assert_eq!(loaded.diagnostics, [], "{older}");
            let profile = loaded.profile.expect("the older file is complete");
            assert_eq!(profile.os, Os::Debian, "{older}");
            assert_eq!(profile.to_toml(), older, "the file is written back exactly as it was");
            assert_eq!(profile.image(), names::profile_image(profile.name.as_str()), "and names the same image");
        }
    }

    #[test]
    fn a_profile_on_another_system_says_so_and_reads_back_as_the_same_profile() {
        let text = COMPLETE.replace("harness = \"claude-code\"\n", "harness = \"claude-code\"\nos = \"arch\"\n");
        let loaded = parse(&text);
        assert_eq!(loaded.diagnostics, []);
        let profile = loaded.profile.expect("complete");
        assert_eq!(profile.os, Os::Arch);
        assert_eq!(profile.to_toml(), text, "the system is written where it was read");
        for os in Os::ALL {
            let mut again = profile.clone();
            again.os = os;
            assert_eq!(parse(&again.to_toml()).profile, Some(again), "{os:?}");
        }
    }

    #[test]
    fn a_system_that_is_not_one_is_reported_at_its_place_and_debian_is_used() {
        let text = COMPLETE.replace("harness = \"claude-code\"\n", "harness = \"claude-code\"\nos = \"gentoo\"\n");
        let loaded = parse(&text);
        let at = loaded.diagnostics.iter().find_map(|d| d.location.clone()).expect("the value has a place");
        assert_eq!(at.to_string(), "claude-sub.toml:3:6");
        assert_eq!(loaded.profile.expect("the rest is usable").os, Os::Debian);
    }

    #[test]
    fn a_harness_its_system_cannot_run_is_refused_with_the_reason() {
        let text = "name = \"g\"\nharness = \"gemini-cli\"\nos = \"alpine\"\naccount = \"api-key\"\n";
        let loaded = Profile::parse("g.toml", text);
        assert_eq!(loaded.profile, None, "a Gemini CLI image on Alpine would crash at its first command");
        let refused = loaded.diagnostics.iter().find(|d| d.severity == Severity::Error).expect("the reason is given");
        assert!(refused.message.contains("alpine") && refused.message.contains("Gemini CLI"), "{}", refused.message);
        assert_eq!(refused.location.as_ref().map(ToString::to_string).as_deref(), Some("g.toml:3:6"));
        let fine = text.replace("gemini-cli", "codex");
        assert_eq!(Profile::parse("c.toml", &fine).profile.map(|profile| profile.os), Some(Os::Alpine));
    }

    /// A profile written before QCode high's parts could be switched off holds no choice, and
    /// reads as having every one of them: the same file, the same profile, the same image.
    #[test]
    fn a_qcode_high_file_without_the_choice_has_everything_and_is_written_back_unchanged() {
        let text = COMPLETE.replace("template = \"recommended\"", "template = \"high\"");
        let loaded = parse(&text);
        assert_eq!(loaded.diagnostics, []);
        let profile = loaded.profile.expect("complete");
        assert_eq!(profile.without, []);
        assert_eq!(profile.additions(), Template::High.additions(HarnessKind::ClaudeCode));
        assert_eq!(profile.claude_plugins(), crate::profile::CLAUDE_PLUGINS);
        assert_eq!(profile.to_toml(), text, "nothing is added to it");
    }

    #[test]
    fn a_part_switched_off_is_written_and_read_back_and_the_rest_stay_on() {
        let text = COMPLETE.replace("template = \"recommended\"", "template = \"high\"");
        let mut profile = parse(&text).profile.expect("complete");
        profile.without = vec![Extra::Plugin("context7@claude-plugins-official"), Extra::Graphify];
        let written = profile.to_toml();
        assert!(written.contains("[additions]\ncontext7 = false\ngraphify = false\n"), "{written}");
        let read = parse(&written);
        assert_eq!(read.diagnostics, []);
        let read = read.profile.expect("complete");
        assert!(!read.has(Extra::Graphify) && !read.has(Extra::Plugin("context7@claude-plugins-official")));
        assert!(read.has(Extra::Plugin("superpowers@claude-plugins-official")));
        assert_eq!(read.additions(), [Addition::ClaudePlugins], "graphify is not installed");
        assert_eq!(read.claude_plugins().len(), 4);
        // A part named `true`, or not named, is on.
        let on = written.replace("graphify = false", "graphify = true");
        assert!(parse(&on).profile.expect("complete").has(Extra::Graphify));
    }

    #[test]
    fn opencode_without_oh_my_openagent_is_set_up_as_under_qcode_basic() {
        let text = COMPLETE
            .replace("template = \"recommended\"", "template = \"high\"")
            .replace("claude-code", "opencode")
            .replace("subscription", "api-key");
        let mut profile = parse(&text).profile.expect("complete");
        assert_ne!(profile.files(), Template::Recommended.files(HarnessKind::OpenCode), "with it, the plugin is named");
        profile.without = vec![Extra::OhMyOpenAgent];
        assert_eq!(profile.files(), Template::Recommended.files(HarnessKind::OpenCode));
        assert_eq!(profile.additions(), [Addition::Graphify]);
    }

    /// A profile that says it runs on a provider but names no tag or model cannot be pointed
    /// anywhere, so it is refused the way any other value it cannot make sense of would be,
    /// rather than starting a tab that could never work.
    #[test]
    fn a_provider_account_without_a_tag_and_model_is_refused() {
        let loaded = Profile::parse("ev.toml", "name = \"ev\"\nharness = \"claude-code\"\naccount = \"provider\"\n");
        assert_eq!(loaded.profile, None);
        let refused = loaded.diagnostics.iter().find(|d| d.severity == Severity::Error).expect("the reason is given");
        assert!(refused.message.contains("provider"), "{}", refused.message);
    }

    #[test]
    fn a_providers_environment_carries_the_relay_and_the_model_and_never_a_key() {
        let provider = ProviderChoice { tag: "ev1".to_owned(), model: "qwen3.8".to_owned() };
        let env = provider.environment(HarnessKind::ClaudeCode, "tab-token-abc", None);
        let get = |env: &Vec<(String, String)>, name: &str| {
            env.iter().find(|(key, _)| key == name).map(|(_, value)| value.to_owned())
        };
        assert_eq!(get(&env, ANTHROPIC_BASE_URL).as_deref(), Some("http://127.0.0.1:41417"));
        assert_eq!(get(&env, ANTHROPIC_MODEL).as_deref(), Some("qwen3.8"));
        assert_eq!(get(&env, ANTHROPIC_AUTH_TOKEN).as_deref(), Some("tab-token-abc"));
        assert_eq!(crate::provider::relay::PORT, 41417, "the address above must track the relay's real port");
        assert_eq!(env.len(), 3, "a window nobody measured is not invented: nothing else is set");

        // A window that was measured is handed over, because a harness that has never heard of
        // this model assumes its own models' room and loses the front of every larger prompt.
        let measured = provider.environment(HarnessKind::ClaudeCode, "tab-token-abc", Some(31_512));
        assert_eq!(get(&measured, MAX_CONTEXT_TOKENS).as_deref(), Some("31512"));
    }

    #[test]
    fn opencode_is_told_of_the_relay_as_an_openai_provider_named_after_the_tag() {
        let provider = ProviderChoice { tag: "ev1".to_owned(), model: "qwen3-coder:30b".to_owned() };
        let env = provider.environment(HarnessKind::OpenCode, "tab-token-abc", None);
        assert_eq!(env.len(), 1, "one variable, the whole of it: {env:?}");
        assert_eq!(env[0].0, OPENCODE_CONFIG_CONTENT);
        let config: serde_json::Value = serde_json::from_str(&env[0].1).expect("the configuration is JSON");
        let entry = &config["provider"]["ev1"];
        assert_eq!(entry["npm"], "@ai-sdk/openai-compatible", "opencode carries this one in its own program");
        assert_eq!(entry["options"]["baseURL"], "http://127.0.0.1:41417/v1");
        assert_eq!(entry["options"]["apiKey"], "tab-token-abc", "the tab's token, never a provider's key");
        assert!(entry["models"]["qwen3-coder:30b"].is_object());
        assert!(entry["models"]["qwen3-coder:30b"].get("limit").is_none(), "no window is invented");
        assert_eq!(config["model"], "ev1/qwen3-coder:30b");
        assert_eq!(config["small_model"], "ev1/qwen3-coder:30b", "titles go to the same model, not elsewhere");

        let measured = provider.environment(HarnessKind::OpenCode, "tab-token-abc", Some(31_512));
        let config: serde_json::Value = serde_json::from_str(&measured[0].1).expect("JSON");
        let limit = &config["provider"]["ev1"]["models"]["qwen3-coder:30b"]["limit"];
        assert_eq!(limit["context"], 31_512);
        assert_eq!(limit["output"], 0, "opencode's own ceiling for the answer; its configuration needs the key");
    }

    #[test]
    fn a_harness_offered_no_provider_is_given_no_environment_for_one() {
        let provider = ProviderChoice { tag: "ev1".to_owned(), model: "m".to_owned() };
        for harness in [HarnessKind::GeminiCli, HarnessKind::Codex, HarnessKind::AntigravityIde] {
            assert!(!harness.supports(AccountKind::Provider), "{harness:?}");
            assert!(provider.environment(harness, "t", None).is_empty(), "{harness:?}");
        }
    }

    #[test]
    fn mount_and_network_values_read_back_as_themselves() {
        for access in MountAccess::ALL {
            assert_eq!(MountAccess::parse(access.id()), Some(access));
        }
        for mode in NetworkMode::ALL {
            assert_eq!(NetworkMode::parse(mode.id()), Some(mode));
        }
        assert_eq!(MountAccess::parse("rwx"), None);
        assert_eq!(NetworkMode::parse("host"), None);
    }
}
