//! Templates: the ready-made sets of parts a profile's image is built with, and the parts
//! themselves.
//!
//! A template is a set of [`Extra`]s it switches on for a harness ([`Template::parts`]), so the
//! wizard can offer every part with a switch of its own and a file can say which ones are off.
//! [`Extra::Settings`] is a part like the others: what every ready-made set writes into the image,
//! so that it can be switched off too.

use crate::profile::{ConfigFile, HarnessKind};

/// A ready-made set of the parts a profile's image is built with, or the person's own set of them.
///
/// No set decides whether the harness asks for permission: it never does, under any of them,
/// because the container is what keeps the work apart from the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// The harness as it comes: the image holds no part of ours. A file on disk that names no
    /// template, or names this one, is read as this.
    Base,
    /// The harness set up the way its makers and QCode recommend, with graphify beside it and
    /// the harness's own recommended plugins: "QCode recommended". The id is older than the name,
    /// and stays, because profiles on disk are written with it.
    Recommended,
    /// QCode recommended, and every other tool the owner of QCode works with installed beside the
    /// harness, with instructions written into the workspace so its agents can reach each other:
    /// "QCode extra". The id `high` is what the template was called before, and stays, because
    /// profiles on disk are written with it. See [`Addition`].
    High,
    /// QCode extra, and what building the Quvyta ecosystem's Rust terminal apps inside the
    /// container needs: "Quvyta development". See [`Addition::Rust`] and [`Addition::Chromium`].
    QuvytaDev,
    /// opencode with oh-my-opencode-slim, the lighter team of agents, and graphify beside it, on
    /// the settings, first-start answers and update switches every QCode set writes: "oh my
    /// opencode slim". It never carries oh-my-openagent. Offered to opencode alone
    /// ([`Template::offered`]); a definition written by hand for another harness gets graphify.
    Slim,
    /// The parts the person chose one by one: "Custom". Its file names what it goes without out of
    /// the whole list its harness can have ([`Template::available`]).
    ///
    /// Only a set no older QCode could build is written under it ([`Template::written_as`]), and
    /// in a folder of its own that an older QCode does not read.
    Custom,
}

impl Template {
    /// Every template a definition file can name, in the order the picker offers them: the four
    /// ready-made sets, then Custom. [`Template::Base`] is in the list because profiles on disk
    /// are written with it, though the picker never offers it.
    pub const ALL: [Self; 6] = [Self::Base, Self::Recommended, Self::High, Self::QuvytaDev, Self::Slim, Self::Custom];

    /// The rows the template page's picker offers a profile of `harness`: the ready-made sets, in
    /// order, and Custom last. oh my opencode slim is opencode's alone, since the team of agents
    /// it is for is opencode's.
    #[must_use]
    pub fn offered(harness: HarnessKind) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|template| *template != Self::Base)
            .filter(|template| *template != Self::Slim || harness == HarnessKind::OpenCode)
            .collect()
    }

    /// How the template is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Recommended => "recommended",
            Self::Slim => "slim",
            Self::High => "high",
            Self::QuvytaDev => "quvyta-dev",
            Self::Custom => "custom",
        }
    }

    /// Whether the template carries everything QCode extra gives: QCode extra itself, and Quvyta
    /// development, which is QCode extra and more. What QCode extra does beyond the image — its
    /// instructions written into the workspace — follows from this, not from one name.
    ///
    /// A set of the person's own does not carry them, even when it holds every part QCode extra
    /// holds: opencode's QCode extra and QCode recommended install the same parts, so nothing in
    /// a set of parts can say which of the two was meant. It gets what QCode recommended gives.
    #[must_use]
    pub fn carries_high(self) -> bool {
        matches!(self, Self::High | Self::QuvytaDev)
    }

    /// The template written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|template| template.id() == id)
    }

    /// The harness's settings and first-start answers from its record, naming no plugin: what the
    /// "recommended settings" part writes for a harness whose plugin is off.
    #[must_use]
    pub fn plain_files(harness: HarnessKind) -> Vec<ConfigFile> {
        let record = harness.record();
        record.settings.into_iter().chain(record.first_start).collect()
    }

    /// The variables the "recommended settings" part sets in the image for `harness`: the maker's
    /// switches for its own update check and usage reports, where the harness offers them as
    /// variables rather than as keys of its settings file (those are in the file itself).
    ///
    /// An update the harness installs by itself would change the image's program under the
    /// person's feet, inside a container that is remade from the image anyway; a rebuild is how
    /// an update arrives. Every name was read in the harness's own package: Claude Code 2.1.281
    /// lists all three among the variables it reads; opencode 1.18.32 returns from its update
    /// check when `OPENCODE_DISABLE_AUTOUPDATE` is set; Kimi Code CLI 2.1.1 reads
    /// `KIMI_DISABLE_TELEMETRY` for its usage reports and `KIMI_CODE_NO_AUTO_UPDATE` for "no
    /// check, no background install, no prompt".
    #[must_use]
    pub fn maker_switches(harness: HarnessKind) -> &'static [(&'static str, &'static str)] {
        match harness {
            HarnessKind::ClaudeCode => {
                &[("DISABLE_AUTOUPDATER", "1"), ("DISABLE_TELEMETRY", "1"), ("DISABLE_ERROR_REPORTING", "1")]
            }
            HarnessKind::OpenCode => &[("OPENCODE_DISABLE_AUTOUPDATE", "1")],
            HarnessKind::KimiCode => &[("KIMI_DISABLE_TELEMETRY", "1"), ("KIMI_CODE_NO_AUTO_UPDATE", "1")],
            HarnessKind::GeminiCli | HarnessKind::Codex | HarnessKind::QwenCode | HarnessKind::AntigravityIde => &[],
        }
    }

    /// The Claude Code plugins the template installs, as `claude plugin install` takes them:
    /// the five of [`CLAUDE_STARTER_PLUGINS`] under QCode recommended, [`CLAUDE_EXTRA_PLUGINS`]
    /// under QCode extra, every one of [`CLAUDE_PLUGINS`] under Quvyta development, none under
    /// base.
    #[must_use]
    pub fn claude_plugins(self) -> &'static [&'static str] {
        match self {
            Self::Base | Self::Slim | Self::Custom => &[],
            Self::Recommended => &CLAUDE_STARTER_PLUGINS,
            Self::High => &CLAUDE_EXTRA_PLUGINS,
            Self::QuvytaDev => &CLAUDE_PLUGINS,
        }
    }

    /// Every part a profile of `harness` can have switched on, in the order the wizard lists
    /// them: graphify, the plugins of the harness that has them, the teams of agents opencode
    /// has, the Rust toolchain and the browser, and the recommended settings.
    ///
    /// This is what the page lists, whatever the picker reads: choosing a ready-made set is a
    /// way of switching these on, not a shorter list of them. Which of them an image can really
    /// carry is the harness's business; the system refuses Chromium on its own ([`crate::base::Os`]).
    #[must_use]
    pub fn available(harness: HarnessKind) -> Vec<Extra> {
        let mut all = vec![Extra::Graphify];
        if harness == HarnessKind::ClaudeCode {
            all.extend(CLAUDE_PLUGINS.iter().copied().map(Extra::Plugin));
        }
        if harness == HarnessKind::OpenCode {
            all.extend([Extra::OhMyOpenAgent, Extra::OhMyOpenCodeSlim]);
        }
        all.extend([Extra::Rust, Extra::Chromium, Extra::Settings]);
        Extra::ordered(all)
    }

    /// The parts this template switches on for a profile of `harness`, in the order the wizard
    /// lists them: the ready-made set the template stands for, or, for [`Template::Custom`], the
    /// whole list its harness can have.
    ///
    /// A custom profile is a list of its own rather than a set, so what it carries is this minus
    /// what its file goes without ([`crate::profile::Profile::parts`] is the whole of it). The
    /// empty set of "the harness as it comes" is a list of everything switched off, so
    /// [`Template::Base`] — the id older files were written with — has no parts at all.
    #[must_use]
    pub fn parts(self, harness: HarnessKind) -> Vec<Extra> {
        if self == Self::Custom {
            return Self::available(harness);
        }
        let parts = match self {
            Self::Base => Vec::new(),
            Self::Slim if harness != HarnessKind::OpenCode => vec![Extra::Settings, Extra::Graphify],
            template => {
                let mut parts = vec![Extra::Settings, Extra::Graphify];
                match harness {
                    HarnessKind::ClaudeCode => {
                        parts.extend(template.claude_plugins().iter().copied().map(Extra::Plugin));
                    }
                    HarnessKind::OpenCode => {
                        parts.push(if template == Self::Slim { Extra::OhMyOpenCodeSlim } else { Extra::OhMyOpenAgent });
                    }
                    _ => {}
                }
                if template == Self::QuvytaDev {
                    parts.extend([Extra::Rust, Extra::Chromium]);
                }
                parts
            }
        };
        Extra::ordered(parts)
    }

    /// The template a definition file names for a profile of `harness` whose parts are exactly
    /// `parts`, which the file's off-list then narrows down to them.
    ///
    /// The harness as it comes, every part off, is [`Template::Base`]. Otherwise the first
    /// ready-made set, in the picker's order, that holds every one of `parts` and differs from
    /// them only by parts an older QCode can leave out too ([`Extra::older_can_leave_out`]): such
    /// a file builds the same image under every QCode that reads it. Only a set that is no ready-
    /// made set narrowed down that way is [`Template::Custom`].
    #[must_use]
    pub fn written_as(parts: &[Extra], harness: HarnessKind) -> Self {
        if parts.is_empty() {
            return Self::Base;
        }
        Self::offered(harness)
            .into_iter()
            .filter(|template| *template != Self::Custom)
            .find(|template| {
                let carried = template.parts(harness);
                parts.iter().all(|part| carried.contains(part))
                    && carried.iter().filter(|part| !parts.contains(part)).all(|part| part.older_can_leave_out())
            })
            .unwrap_or(Self::Custom)
    }
}

/// Something an image is built with beside the harness, in the order a build installs it.
///
/// These are what the recipe's steps are: the parts of a profile that are downloaded while the image
/// is built, unpinned, like the harnesses themselves, so a rebuild is how an update arrives. A part
/// that is written into the image rather than downloaded — [`Extra::Settings`] — is not one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addition {
    /// graphify (PyPI `graphifyy`), the code map the owner's own agents ask before they read a
    /// file. Every harness gets it. It needs Python 3.10 or later, which the base image does not
    /// carry, so Debian's own `python3` and `pipx` come with it, and it is installed outside the
    /// home directory: everything under the home is copied into each workspace's home volume on
    /// its first start, and a 200 MB tool does not belong there.
    Graphify,
    /// The Claude Code plugins of [`Template::claude_plugins`], from the marketplaces in
    /// [`CLAUDE_MARKETPLACES`]. Claude Code only; the others have their own counterparts.
    ClaudePlugins,
    /// oh-my-openagent (npm), opencode's plugin, loaded from the image by the path in
    /// `OPENCODE_PLUGIN_SETTINGS`. opencode only.
    OhMyOpenAgent,
    /// oh-my-opencode-slim (npm), the lighter fork of oh-my-openagent, loaded from the image by
    /// the path in `OPENCODE_SLIM_SETTINGS`. opencode under oh my opencode slim only; a profile
    /// never has both, since each brings a whole team of agents meant to run alone.
    OhMyOpenCodeSlim,
    /// What building the Quvyta apps needs, Quvyta development only: a C compiler, pkg-config
    /// and OpenSSL's headers from the system's own packages, with git, ssh, curl, jq, procps and
    /// bash; Rust's stable toolchain with clippy and rustfmt from rustup, for whatever machine
    /// the image is built on; and uv. Rust is installed outside the home directory, in
    /// [`RUSTUP_HOME`] and [`CARGO_HOME`], for the same reason graphify is, and left writable by
    /// whoever the container runs as: a project that names its own toolchain has rustup fetch it
    /// there at run time.
    Rust,
    /// Chromium from the system's own packages, Quvyta development only: the browser project's
    /// tests drive a real one. Ubuntu 24.04 packages it only as a snap, which does not run in a
    /// container, so there it cannot be had ([`crate::base::Os::chromium`]).
    Chromium,
}

/// One part of what an image can carry beside the harness: graphify, one of the Claude Code
/// plugins, one of the teams of agents opencode has, the Rust toolchain, the browser, or the
/// recommended settings.
///
/// A part is a row with a switch of its own. A ready-made set switches a set of them on
/// ([`Template::parts`]) and a custom profile names the ones it goes without, so a file never has to
/// spell out what it wants: what it names nothing of is on, which is also how every profile written
/// before the choice existed reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    /// graphify; without it, nothing of graphify is installed and nothing of it is set up in the
    /// workspace.
    Graphify,
    /// One Claude Code plugin, as `claude plugin install` takes it: one of [`CLAUDE_PLUGINS`].
    Plugin(&'static str),
    /// oh-my-openagent; without it, opencode's settings do not name it.
    OhMyOpenAgent,
    /// oh-my-opencode-slim; without it, opencode's settings do not name it.
    OhMyOpenCodeSlim,
    /// The Rust toolchain of Quvyta development: rustup's stable toolchain with clippy and
    /// rustfmt, the C toolchain and uv beside it. Without it no build of a Rust project can run in
    /// the image.
    Rust,
    /// Chromium, under Quvyta development; without it, no browser is installed.
    Chromium,
    /// The settings, first-start answers and update switches every ready-made set writes into the
    /// image of its own. Without it the harness runs as it comes and asks its first questions in
    /// every workspace.
    Settings,
}

impl Extra {
    /// Every part, in the order the wizard lists them.
    #[must_use]
    pub fn all() -> Vec<Self> {
        let mut all = vec![Self::Graphify];
        all.extend(CLAUDE_PLUGINS.map(Self::Plugin));
        all.push(Self::OhMyOpenAgent);
        all.push(Self::OhMyOpenCodeSlim);
        all.push(Self::Rust);
        all.push(Self::Chromium);
        all.push(Self::Settings);
        all
    }

    /// How the part is written in definition files, which is also the name its maker gives it.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Graphify => "graphify",
            Self::Plugin(plugin) => plugin.split_once('@').map_or(plugin, |(name, _)| name),
            Self::OhMyOpenAgent => OH_MY_OPENAGENT,
            Self::OhMyOpenCodeSlim => OH_MY_OPENCODE_SLIM,
            Self::Rust => "rust",
            Self::Chromium => "chromium",
            Self::Settings => "settings",
        }
    }

    /// The part written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::all().into_iter().find(|extra| extra.id() == id)
    }

    /// Whether a QCode from before [`Template::Custom`] also builds an image without this part
    /// when a file's off-list names it. The Rust toolchain and the recommended settings came
    /// with their sets, not as switches, and such a QCode would build them regardless.
    #[must_use]
    pub fn older_can_leave_out(self) -> bool {
        !matches!(self, Self::Rust | Self::Settings)
    }

    /// The parts in the order the wizard lists them, so that a set of them reads the same however
    /// it was put together.
    #[must_use]
    pub fn ordered(parts: impl IntoIterator<Item = Self>) -> Vec<Self> {
        let all = Self::all();
        let mut parts: Vec<Self> = parts.into_iter().collect();
        parts.sort_by_key(|part| all.iter().position(|one| *one == *part).unwrap_or(usize::MAX));
        parts
    }
}

/// The PyPI package graphify is published as; its command is `graphify`.
pub const GRAPHIFY_PACKAGE: &str = "graphifyy";

/// Where pipx keeps graphify's environment, outside the home directory.
pub const GRAPHIFY_HOME: &str = "/opt/pipx";

/// Where rustup keeps the toolchains under Quvyta development, outside the home directory.
pub const RUSTUP_HOME: &str = "/opt/rustup";

/// Where rustup puts `cargo`, `rustc` and its other commands under Quvyta development. Only the
/// commands are kept here: at run time cargo keeps what it downloads in the home, as it does
/// anywhere, so a workspace's home volume keeps its registry between starts.
pub const CARGO_HOME: &str = "/opt/cargo";

/// The npm package of oh-my-openagent.
pub const OH_MY_OPENAGENT: &str = "oh-my-openagent";

/// The npm package of oh-my-opencode-slim, github.com/alvinunreal/oh-my-opencode-slim (MIT).
pub const OH_MY_OPENCODE_SLIM: &str = "oh-my-opencode-slim";

/// The accesslint plugin, whose MCP server its own `.mcp.json` starts with `npx -y
/// @accesslint/mcp@latest` (plugin 0.10.3): npm is asked for the newest version at every tab's
/// start, the package is downloaded into npm's cache the first time, and `npx` stays running beside
/// the server for as long as the tab does (measured: about 45 MB a tab, and about 200 MB while the
/// first tab downloads). Without the network the server does not start at all.
pub const ACCESSLINT_PLUGIN: &str = "accesslint@accesslint";

/// The npm package of accesslint's MCP server, installed into the image beside the harness.
pub const ACCESSLINT_SERVER: &str = "@accesslint/mcp";

/// The program [`ACCESSLINT_SERVER`] installs, which the plugin's server entry is pointed at in
/// the image instead of `npx`.
pub const ACCESSLINT_PROGRAM: &str = "accesslint-mcp";

/// The security-guidance plugin, whose SessionStart hook builds a Python environment of its own and
/// pip-installs the agent SDK into it the first time a tab starts (plugin 2.0.8, read 2026-09-29):
/// about 110 MB and a few seconds on the first tab of a profile, and without a network the install
/// fails, which leaves the plugin's cross-file commit review with nothing to run on and the person
/// none the wiser.
pub const SECURITY_GUIDANCE_PLUGIN: &str = "security-guidance@claude-plugins-official";

/// The Python distribution that hook installs, and the module its commit reviewer imports: the same
/// name with the hyphen written as an underscore.
pub const AGENT_SDK: &str = "claude-agent-sdk";

/// Where that hook builds the environment it installs into, under the folder it keeps its own state
/// in: `~/.claude/security`, which is also where its reviewer looks for the package when the
/// interpreter that ran the hook does not have it.
pub const AGENT_SDK_VENV: &str = ".claude/security/agent-sdk-venv";

/// The marketplaces the Claude Code plugins come from: the repository `claude plugin
/// marketplace add` is given, and the name the marketplace calls itself, which is what a plugin
/// is installed by. Each name was read from `claude plugin marketplace list` after adding it.
pub const CLAUDE_MARKETPLACES: [(&str, &str); 5] = [
    ("anthropics/claude-plugins-official", "claude-plugins-official"),
    ("wshobson/agents", "claude-code-workflows"),
    ("anthropics/skills", "anthropic-agent-skills"),
    ("gfargo/tui-design-skill", "tui-design-marketplace"),
    ("accesslint/claude-marketplace", "accesslint"),
];

/// The Claude Code plugins QCode recommended installs, as `claude plugin install` takes them:
/// the owner's starter set, chosen because none of them does what graphify already does.
pub const CLAUDE_STARTER_PLUGINS: [&str; 5] = [
    "superpowers@claude-plugins-official",
    "context7@claude-plugins-official",
    "code-review@claude-plugins-official",
    "security-guidance@claude-plugins-official",
    "block-no-verify@claude-code-workflows",
];

/// Every Claude Code plugin a profile can have: the ones the owner of QCode works with, the
/// starter set among them. Quvyta development installs all of them, QCode extra all but
/// [`RUST_ANALYZER_PLUGIN`] ([`CLAUDE_EXTRA_PLUGINS`]).
pub const CLAUDE_PLUGINS: [&str; 16] = [
    "superpowers@claude-plugins-official",
    "context7@claude-plugins-official",
    "code-review@claude-plugins-official",
    "security-guidance@claude-plugins-official",
    "rust-analyzer-lsp@claude-plugins-official",
    "code-simplifier@claude-plugins-official",
    "feature-dev@claude-plugins-official",
    "hookify@claude-plugins-official",
    "claude-md-management@claude-plugins-official",
    "block-no-verify@claude-code-workflows",
    "systems-programming@claude-code-workflows",
    "agent-teams@claude-code-workflows",
    "skill-forge-essentials@claude-code-workflows",
    "example-skills@anthropic-agent-skills",
    "tui-design@tui-design-marketplace",
    "accesslint@accesslint",
];

/// The plugin that gives Claude Code rust-analyzer's view of a Rust project. It only names the
/// program, `rust-analyzer`, which it expects to find: without it the plugin does nothing at all,
/// silently. So only a set that installs Rust with rust-analyzer carries it (Quvyta
/// development, [`Addition::Rust`]); it costs about 560 MB a tab once the tab touches a Rust file
/// (measured 2026-09-29, nothing shared between tabs).
pub const RUST_ANALYZER_PLUGIN: &str = "rust-analyzer-lsp@claude-plugins-official";

/// The Claude Code plugins QCode extra installs: [`CLAUDE_PLUGINS`] without
/// [`RUST_ANALYZER_PLUGIN`], since QCode extra has no Rust.
pub const CLAUDE_EXTRA_PLUGINS: [&str; 15] = [
    "superpowers@claude-plugins-official",
    "context7@claude-plugins-official",
    "code-review@claude-plugins-official",
    "security-guidance@claude-plugins-official",
    "code-simplifier@claude-plugins-official",
    "feature-dev@claude-plugins-official",
    "hookify@claude-plugins-official",
    "claude-md-management@claude-plugins-official",
    "block-no-verify@claude-code-workflows",
    "systems-programming@claude-code-workflows",
    "agent-teams@claude-code-workflows",
    "skill-forge-essentials@claude-code-workflows",
    "example-skills@anthropic-agent-skills",
    "tui-design@tui-design-marketplace",
    "accesslint@accesslint",
];

/// opencode's settings under every QCode template: the permission of the harness record, and
/// oh-my-openagent by the path npm installed it to in the image.
///
/// The path, not the package name, on purpose. `omo install` writes `oh-my-openagent@latest`,
/// which opencode resolves itself on every start: it downloads the package again, 469 MB, into
/// `~/.cache/opencode` — the workspace's home volume — and cannot at all in a container without
/// the network. By path, opencode loads the copy the build installed, with or without one.
pub const OPENCODE_PLUGIN_SETTINGS: ConfigFile = ConfigFile {
    path: ".config/opencode/opencode.json",
    contents: "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"permission\": {\n    \"*\": \"allow\"\n  },\n  \"plugin\": [\n    \"file:///usr/local/npm/lib/node_modules/oh-my-openagent\"\n  ]\n}\n",
};

/// opencode's settings under oh my opencode slim: the permission of the harness record, and
/// oh-my-opencode-slim by the path npm installed it to in the image, for the reason
/// [`OPENCODE_PLUGIN_SETTINGS`] gives.
pub const OPENCODE_SLIM_SETTINGS: ConfigFile = ConfigFile {
    path: ".config/opencode/opencode.json",
    contents: "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"permission\": {\n    \"*\": \"allow\"\n  },\n  \"plugin\": [\n    \"file:///usr/local/npm/lib/node_modules/oh-my-opencode-slim\"\n  ]\n}\n",
};

/// oh-my-opencode-slim's own settings under oh my opencode slim, read in its 2.2.25 package.
///
/// No models and no presets: with none, every agent of its team runs on the model of the
/// opencode session, which is the one the profile's account gives it, so the team works with any
/// account QCode offers. Loaded by a `file://` path, it takes itself for a local copy and never
/// asks npm for a newer version (`getLocalDevVersion` returns before the check); `autoUpdate` off
/// keeps it from installing one into the home should that ever change, since a rebuild is how an
/// update arrives, as for every harness. Its companion window and its multiplexer panes are off
/// by its own defaults, and it carries no telemetry.
pub const SLIM_OWN_SETTINGS: ConfigFile = ConfigFile {
    path: ".config/opencode/oh-my-opencode-slim.json",
    contents: "{\n  \"$schema\": \"https://unpkg.com/oh-my-opencode-slim@latest/oh-my-opencode-slim.schema.json\",\n  \"autoUpdate\": false\n}\n",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::Os;
    use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName};

    /// The profile a ready-made set makes for `harness`, with nothing switched off: the set as it
    /// stands, which is what a file written before the switches existed reads as.
    ///
    /// A custom profile is a set of nothing but its own off-list, so the one built here is every
    /// part switched off — the harness as it comes — rather than every part on, which is a state
    /// no person can choose.
    fn under(template: Template, harness: HarnessKind) -> Profile {
        let without = if template == Template::Custom { Template::available(harness) } else { Vec::new() };
        Profile {
            name: SafeName::parse("claude-sub").expect("the name is safe"),
            harness,
            template,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
            without,
            os: Os::Debian,
        }
    }

    /// The parts of `profile`'s set, by the id a file writes them under.
    fn ids(parts: Vec<Extra>) -> Vec<&'static str> {
        parts.into_iter().map(Extra::id).collect()
    }

    /// A plugin's own name, without its marketplace.
    fn name(plugin: &str) -> &str {
        plugin.split_once('@').map_or(plugin, |(name, _)| name)
    }

    #[test]
    fn the_base_set_carries_nothing_and_the_harness_as_it_comes_writes_nothing() {
        for harness in HarnessKind::ALL {
            let profile = under(Template::Base, harness);
            assert!(profile.parts().is_empty(), "{harness:?}");
            assert!(profile.files().is_empty(), "{harness:?}");
            assert!(profile.environment().is_empty(), "{harness:?}");
            assert!(profile.additions().is_empty(), "{harness:?}");
        }
        assert!(Template::Base.claude_plugins().is_empty());
    }

    #[test]
    fn qcode_recommended_writes_what_the_harness_record_holds_and_adds_graphify_and_the_makers_plugins() {
        for harness in HarnessKind::ALL {
            let profile = under(Template::Recommended, harness);
            let wanted = match harness {
                HarnessKind::OpenCode => vec![OPENCODE_PLUGIN_SETTINGS],
                _ => Template::plain_files(harness),
            };
            assert_eq!(profile.files(), wanted, "{harness:?}");
            let additions = profile.additions();
            assert_eq!(additions.first(), Some(&Addition::Graphify), "{harness:?}: graphify for every harness");
            let wanted: &[Addition] = match harness {
                HarnessKind::ClaudeCode => &[Addition::Graphify, Addition::ClaudePlugins],
                HarnessKind::OpenCode => &[Addition::Graphify, Addition::OhMyOpenAgent],
                _ => &[Addition::Graphify],
            };
            assert_eq!(additions, wanted, "{harness:?}");
        }
        let claude = under(Template::Recommended, HarnessKind::ClaudeCode).files();
        assert_eq!(claude[0].path, ".claude/settings.json");
        assert!(claude[0].contents.contains("bypassPermissions"));
        let codex = under(Template::Recommended, HarnessKind::Codex).files();
        assert_eq!(codex[0].path, ".codex/config.toml");
        assert!(codex[0].contents.contains("approval_policy = \"never\""));
    }

    #[test]
    fn every_set_that_writes_settings_answers_claude_codes_first_questions() {
        // Each key was taken away once in a container and its question came back; see the
        // record of Claude Code.
        for template in [Template::Recommended, Template::High, Template::QuvytaDev] {
            let files = under(template, HarnessKind::ClaudeCode).files();
            let settings = files.iter().find(|file| file.path == ".claude/settings.json").expect("settings");
            let settings: serde_json::Value = serde_json::from_str(settings.contents).expect("JSON");
            assert_eq!(settings["skipDangerousModePermissionPrompt"], true, "{template:?}");
            let state = files.iter().find(|file| file.path == ".claude.json").expect("the first-start answers");
            let state: serde_json::Value = serde_json::from_str(state.contents).expect("JSON");
            assert_eq!(state["hasCompletedOnboarding"], true, "{template:?}");
            let trusted = &state["projects"][crate::base::paths::CODE_DIR]["hasTrustDialogAccepted"];
            assert_eq!(trusted, true, "{template:?}: the folder the workspace is mounted at");
        }
        assert!(under(Template::Base, HarnessKind::ClaudeCode).files().is_empty(), "the harness as it comes");
    }

    #[test]
    fn qcode_extra_is_qcode_recommended_with_every_plugin_the_owner_uses() {
        for harness in HarnessKind::ALL {
            let extra = under(Template::High, harness);
            let recommended = under(Template::Recommended, harness);
            assert_eq!(extra.additions(), recommended.additions(), "{harness:?}");
            assert_eq!(extra.files(), recommended.files(), "{harness:?}");
            assert_eq!(extra.environment(), recommended.environment(), "{harness:?}: the same switches");
            let recommended = Template::Recommended.parts(harness);
            let extra = Template::High.parts(harness);
            assert!(recommended.iter().all(|part| extra.contains(part)), "{harness:?}: everything of recommended");
            assert_eq!(
                extra.len() > recommended.len(),
                harness == HarnessKind::ClaudeCode,
                "{harness:?}: more only where more was proven"
            );
        }
        for plugin in CLAUDE_STARTER_PLUGINS {
            assert!(CLAUDE_PLUGINS.contains(&plugin), "{plugin}: the starter set is part of extra's");
        }
        assert_eq!(Template::Recommended.claude_plugins(), CLAUDE_STARTER_PLUGINS);
        assert_eq!(Template::QuvytaDev.claude_plugins(), CLAUDE_PLUGINS);
        let extra: Vec<&str> = CLAUDE_PLUGINS.into_iter().filter(|plugin| *plugin != RUST_ANALYZER_PLUGIN).collect();
        assert_eq!(Template::High.claude_plugins(), extra, "QCode extra has no Rust for rust-analyzer's plugin");
    }

    #[test]
    fn the_plugins_are_claude_codes_and_oh_my_openagent_opencodes_under_every_set() {
        for template in Template::ALL {
            for harness in HarnessKind::ALL {
                let additions = under(template, harness).additions();
                assert_eq!(
                    additions.contains(&Addition::ClaudePlugins),
                    !matches!(template, Template::Base | Template::Slim | Template::Custom)
                        && harness == HarnessKind::ClaudeCode,
                    "{template:?} {harness:?}: the plugins are Claude Code's alone, and not slim's"
                );
                assert_eq!(
                    additions.contains(&Addition::OhMyOpenAgent),
                    !matches!(template, Template::Base | Template::Slim | Template::Custom)
                        && harness == HarnessKind::OpenCode,
                    "{template:?} {harness:?}: oh-my-openagent is opencode's alone, and never slim's"
                );
                assert_eq!(
                    additions.contains(&Addition::OhMyOpenCodeSlim),
                    template == Template::Slim && harness == HarnessKind::OpenCode,
                    "{template:?} {harness:?}: oh-my-opencode-slim is QCode slim's, for opencode"
                );
            }
        }
    }

    #[test]
    fn quvyta_development_is_qcode_extra_with_rust_chromium_and_rust_analyzers_plugin() {
        for harness in HarnessKind::ALL {
            let dev = under(Template::QuvytaDev, harness);
            let extra = under(Template::High, harness);
            // The two build the same image but for the toolchain and the browser, which come
            // after everything of extra's, since an image is built one step at a time.
            let additions = dev.additions();
            assert!(additions.starts_with(&extra.additions()), "{harness:?}: QCode extra's come first");
            assert_eq!(&additions[extra.additions().len()..], [Addition::Rust, Addition::Chromium]);
            assert_eq!(dev.files(), extra.files(), "{harness:?}");
            assert_eq!(dev.environment(), extra.environment());
            let parts = Template::QuvytaDev.parts(harness);
            let rust_only = [Extra::Rust, Extra::Chromium, Extra::Plugin(RUST_ANALYZER_PLUGIN)];
            let without_dev = parts.iter().copied().filter(|part| !rust_only.contains(part));
            assert_eq!(Extra::ordered(without_dev), Template::High.parts(harness), "{harness:?}");
        }
        assert!(Template::QuvytaDev.carries_high() && Template::High.carries_high());
        assert!(!Template::Recommended.carries_high() && !Template::Base.carries_high());
        assert!(!Template::Custom.carries_high(), "a set of the person's own promises nothing of QCode extra's");
        for template in [Template::Base, Template::Recommended, Template::High] {
            for harness in HarnessKind::ALL {
                let additions = under(template, harness).additions();
                assert!(!additions.contains(&Addition::Rust), "{template:?} {harness:?}");
                assert!(!additions.contains(&Addition::Chromium), "{template:?} {harness:?}");
            }
        }
    }

    #[test]
    fn every_set_that_writes_settings_tells_opencode_to_load_the_plugin_from_the_image() {
        for template in [Template::Recommended, Template::High, Template::QuvytaDev] {
            assert_eq!(under(template, HarnessKind::OpenCode).files(), [OPENCODE_PLUGIN_SETTINGS], "{template:?}");
        }
        let plain = HarnessKind::OpenCode.record().settings.expect("opencode has settings");
        assert_eq!(OPENCODE_PLUGIN_SETTINGS.path, plain.path, "the same file, not a second one");
        assert_eq!(Template::plain_files(HarnessKind::OpenCode), [plain]);
        let with: serde_json::Value = serde_json::from_str(OPENCODE_PLUGIN_SETTINGS.contents).expect("JSON");
        let plain: serde_json::Value = serde_json::from_str(plain.contents).expect("JSON");
        assert_eq!(with["permission"], plain["permission"], "everything the record says still holds");
        let plugin = with["plugin"][0].as_str().expect("one plugin");
        // The path npm installs to in the base image: its prefix, then `lib/node_modules`.
        assert_eq!(plugin, format!("file:///usr/local/npm/lib/node_modules/{OH_MY_OPENAGENT}"));
        assert!(!plugin.contains('@'), "a version spec makes opencode download it again at every start");
    }

    #[test]
    fn the_plugins_are_the_ones_the_owner_chose() {
        // Written out rather than read from the lists, so that a plugin dropped from a list or
        // one slipped into it is a failing test, not a quieter image.
        let starter: Vec<&str> = CLAUDE_STARTER_PLUGINS.iter().map(|plugin| name(plugin)).collect();
        assert_eq!(starter, ["superpowers", "context7", "code-review", "security-guidance", "block-no-verify"]);
        assert_eq!(
            CLAUDE_PLUGINS,
            [
                "superpowers@claude-plugins-official",
                "context7@claude-plugins-official",
                "code-review@claude-plugins-official",
                "security-guidance@claude-plugins-official",
                "rust-analyzer-lsp@claude-plugins-official",
                "code-simplifier@claude-plugins-official",
                "feature-dev@claude-plugins-official",
                "hookify@claude-plugins-official",
                "claude-md-management@claude-plugins-official",
                "block-no-verify@claude-code-workflows",
                "systems-programming@claude-code-workflows",
                "agent-teams@claude-code-workflows",
                "skill-forge-essentials@claude-code-workflows",
                "example-skills@anthropic-agent-skills",
                "tui-design@tui-design-marketplace",
                "accesslint@accesslint",
            ]
        );
    }

    #[test]
    fn the_plugins_come_from_the_marketplaces_that_are_added() {
        for plugin in CLAUDE_PLUGINS {
            let (_, marketplace) = plugin.split_once('@').expect("a plugin is named with its marketplace");
            assert!(CLAUDE_MARKETPLACES.iter().any(|(_, name)| *name == marketplace), "{plugin}");
        }
        for (repository, marketplace) in CLAUDE_MARKETPLACES {
            let used = CLAUDE_PLUGINS.iter().any(|plugin| plugin.ends_with(&format!("@{marketplace}")));
            assert!(used, "{repository}: a marketplace no plugin comes from");
        }
    }

    #[test]
    fn unattended_mode_does_not_depend_on_the_set() {
        // The container is the isolation. A set only writes settings; what makes the
        // harness stop asking are the arguments it is started with, under every set.
        for harness in HarnessKind::TERMINAL {
            assert!(!harness.record().auto_run.is_empty(), "{harness:?}");
        }
    }

    #[test]
    fn every_part_reads_back_as_itself_by_its_makers_name() {
        let ids: Vec<&str> = Extra::all().into_iter().map(Extra::id).collect();
        let mut wanted = vec!["graphify"];
        wanted.extend(CLAUDE_PLUGINS.iter().map(|plugin| name(plugin)));
        wanted.extend(["oh-my-openagent", "oh-my-opencode-slim", "rust", "chromium", "settings"]);
        assert_eq!(ids, wanted);
        for extra in Extra::all() {
            assert_eq!(Extra::parse(extra.id()), Some(extra));
        }
        assert_eq!(Extra::parse("context7@claude-plugins-official"), None, "the file names it by its own name");
        assert_eq!(Extra::parse("hookify"), Some(Extra::Plugin("hookify@claude-plugins-official")));
    }

    #[test]
    fn the_list_is_the_whole_of_what_a_profile_can_have() {
        // Only what the harness has of its own: Claude Code's plugins are Claude Code's, and the
        // two teams of agents are opencode's.
        let ids = |harness| ids(Template::available(harness));
        assert_eq!(ids(HarnessKind::ClaudeCode), {
            let mut wanted = vec!["graphify"];
            wanted.extend(CLAUDE_PLUGINS.iter().map(|plugin| name(plugin)));
            wanted.extend(["rust", "chromium", "settings"]);
            wanted
        });
        assert_eq!(
            ids(HarnessKind::OpenCode),
            ["graphify", "oh-my-openagent", "oh-my-opencode-slim", "rust", "chromium", "settings"]
        );
        assert_eq!(ids(HarnessKind::Codex), ["graphify", "rust", "chromium", "settings"]);
        assert_eq!(ids(HarnessKind::GeminiCli), ["graphify", "rust", "chromium", "settings"]);
        for harness in HarnessKind::ALL {
            let available = Template::available(harness);
            for template in Template::ALL {
                for part in template.parts(harness) {
                    if template == Template::Custom {
                        continue;
                    }
                    assert!(available.contains(&part), "{template:?} {harness:?}: `{part:?}` is not in the list");
                }
            }
        }
    }

    #[test]
    fn a_set_is_written_in_the_terms_an_older_qcode_builds_the_same_and_custom_only_when_none_does() {
        for harness in HarnessKind::ALL {
            // Every ready-made set as itself. opencode's QCode extra holds what QCode recommended
            // holds, so a set of parts alone is written as the one that promises less.
            for template in [Template::Recommended, Template::High, Template::QuvytaDev] {
                let same = template.parts(harness) == Template::Recommended.parts(harness);
                let wanted = if same { Template::Recommended } else { template };
                assert_eq!(Template::written_as(&template.parts(harness), harness), wanted, "{template:?} {harness:?}");
            }
            // Every part off is the harness as it comes, under its old id.
            assert_eq!(Template::written_as(&[], harness), Template::Base, "{harness:?}");
            // A set narrowed by parts every QCode could switch off is still that set: graphify off.
            let narrowed: Vec<Extra> =
                Template::QuvytaDev.parts(harness).into_iter().filter(|part| *part != Extra::Graphify).collect();
            assert_eq!(Template::written_as(&narrowed, harness), Template::QuvytaDev, "{harness:?}");
            // The settings off, or the toolchain without the rest of Quvyta development's set, is
            // something no older QCode can build.
            let no_settings: Vec<Extra> =
                Template::Recommended.parts(harness).into_iter().filter(|part| *part != Extra::Settings).collect();
            assert_eq!(Template::written_as(&no_settings, harness), Template::Custom, "{harness:?}");
            assert_eq!(Template::written_as(&[Extra::Rust], harness), Template::Custom, "{harness:?}");
        }
        // opencode's lighter team with the toolchain is in no ready-made set.
        let opencode = HarnessKind::OpenCode;
        let slim_rust = [Extra::Graphify, Extra::OhMyOpenCodeSlim, Extra::Rust, Extra::Settings];
        assert_eq!(Template::written_as(&slim_rust, opencode), Template::Custom);
        assert_eq!(Template::written_as(&Template::Slim.parts(opencode), opencode), Template::Slim);
        // A plugin QCode extra alone carries, with a starter plugin off: QCode extra, narrowed.
        let claude = HarnessKind::ClaudeCode;
        let parts: Vec<Extra> = Template::High
            .parts(claude)
            .into_iter()
            .filter(|part| *part != Extra::Plugin("context7@claude-plugins-official"))
            .collect();
        assert_eq!(Template::written_as(&parts, claude), Template::High);
        assert!(Extra::Graphify.older_can_leave_out() && Extra::Chromium.older_can_leave_out());
        assert!(!Extra::Rust.older_can_leave_out() && !Extra::Settings.older_can_leave_out());
    }

    #[test]
    fn the_picker_offers_the_four_ready_made_sets_and_custom_and_slim_to_opencode_alone() {
        for harness in HarnessKind::ALL {
            let offered = Template::offered(harness);
            let wanted: Vec<Template> = if harness == HarnessKind::OpenCode {
                vec![Template::Recommended, Template::High, Template::QuvytaDev, Template::Slim, Template::Custom]
            } else {
                vec![Template::Recommended, Template::High, Template::QuvytaDev, Template::Custom]
            };
            assert_eq!(offered, wanted, "{harness:?}");
            assert!(!offered.contains(&Template::Base), "{harness:?}: the harness as it comes is not a row");
        }
    }

    #[test]
    fn the_two_teams_of_agents_exclude_each_other_wherever_the_set_is_written_down() {
        // A hand-written file may ask for both; a container runs one of them and its settings can
        // name only one, so the file is read as the list's own order decides: the lighter one.
        let harness = HarnessKind::OpenCode;
        let with = |kept: &[Extra]| Profile {
            without: Template::available(harness).into_iter().filter(|part| !kept.contains(part)).collect(),
            ..under(Template::Custom, harness)
        };
        let both = with(&[Extra::Graphify, Extra::Settings, Extra::OhMyOpenAgent, Extra::OhMyOpenCodeSlim]);
        assert!(both.has(Extra::OhMyOpenCodeSlim) && !both.has(Extra::OhMyOpenAgent), "{:?}", both.parts());
        assert_eq!(both.additions(), [Addition::Graphify, Addition::OhMyOpenCodeSlim]);
        assert_eq!(both.files(), [OPENCODE_SLIM_SETTINGS, SLIM_OWN_SETTINGS]);
        // And each one on its own is itself, as the page leaves it.
        let slim = with(&[Extra::Graphify, Extra::Settings, Extra::OhMyOpenCodeSlim]);
        assert_eq!(slim.files(), [OPENCODE_SLIM_SETTINGS, SLIM_OWN_SETTINGS]);
        let omo = with(&[Extra::Graphify, Extra::Settings, Extra::OhMyOpenAgent]);
        assert!(omo.has(Extra::OhMyOpenAgent) && !omo.has(Extra::OhMyOpenCodeSlim));
        assert_eq!(omo.additions(), [Addition::Graphify, Addition::OhMyOpenAgent]);
        assert_eq!(omo.files(), [OPENCODE_PLUGIN_SETTINGS]);
    }

    #[test]
    fn the_recommended_settings_part_is_what_a_set_that_has_no_settings_writes_with() {
        // The harness as it comes and a set with the part off write the same image, which is the
        // point of the part being a row of its own.
        for harness in HarnessKind::ALL {
            let off = under(Template::Custom, harness);
            let base = under(Template::Base, harness);
            assert!(off.files().is_empty() && off.environment().is_empty(), "{harness:?}");
            assert_eq!(off.parts(), base.parts(), "{harness:?}: the harness as it comes either way");
            for template in [Template::Recommended, Template::High, Template::QuvytaDev, Template::Slim] {
                let on = under(template, harness);
                assert!(on.has(Extra::Settings), "{template:?} {harness:?}: every set of ours writes them");
                assert_eq!(on.environment(), Template::maker_switches(harness), "{template:?} {harness:?}");
            }
        }
    }

    #[test]
    fn every_set_that_writes_settings_opens_antigravity_without_asking_whether_the_folder_is_trusted() {
        // Workspace trust off: the window neither asks about the folder nor keeps it in
        // restricted mode. `permission_live.rs` checks the installed application reads the key.
        for template in [Template::Recommended, Template::High] {
            let files = under(template, HarnessKind::AntigravityIde).files();
            let settings: serde_json::Value = serde_json::from_str(files[0].contents).expect("JSON");
            assert_eq!(settings["security.workspace.trust.enabled"], false, "{template:?}");
        }
        assert!(under(Template::Base, HarnessKind::AntigravityIde).files().is_empty());
    }

    #[test]
    fn the_makers_update_checks_and_reports_are_off_and_the_harness_as_it_comes_leaves_them_on() {
        // Written out, so a switch dropped from the list is a failing test.
        let claude = Template::maker_switches(HarnessKind::ClaudeCode);
        assert_eq!(
            claude,
            [("DISABLE_AUTOUPDATER", "1"), ("DISABLE_TELEMETRY", "1"), ("DISABLE_ERROR_REPORTING", "1")]
        );
        assert_eq!(Template::maker_switches(HarnessKind::OpenCode), [("OPENCODE_DISABLE_AUTOUPDATE", "1")]);
        assert_eq!(
            Template::maker_switches(HarnessKind::KimiCode),
            [("KIMI_DISABLE_TELEMETRY", "1"), ("KIMI_CODE_NO_AUTO_UPDATE", "1")]
        );
        // The rest have keys of their settings file for it.
        let settings = |harness: HarnessKind| under(Template::Recommended, harness).files()[0].contents;
        let gemini: serde_json::Value = serde_json::from_str(settings(HarnessKind::GeminiCli)).expect("JSON");
        assert_eq!(gemini["general"]["enableAutoUpdate"], false);
        assert_eq!(gemini["general"]["enableAutoUpdateNotification"], false);
        assert_eq!(gemini["privacy"]["usageStatisticsEnabled"], false);
        let qwen: serde_json::Value = serde_json::from_str(settings(HarnessKind::QwenCode)).expect("JSON");
        assert_eq!(qwen["general"]["enableAutoUpdate"], false);
        assert_eq!(qwen["privacy"]["usageStatisticsEnabled"], false);
        let codex = settings(HarnessKind::Codex);
        assert!(codex.contains("\ncheck_for_update_on_startup = false\n"), "{codex}");
        assert!(codex.contains("\n[analytics]\nenabled = false\n"), "{codex}");
        let antigravity = settings(HarnessKind::AntigravityIde);
        assert!(antigravity.contains("\"telemetry.telemetryLevel\": \"off\""), "{antigravity}");
        for harness in HarnessKind::ALL {
            assert!(under(Template::Base, harness).environment().is_empty(), "{harness:?}");
        }
    }

    #[test]
    fn sets_read_back_as_the_same_set_and_old_files_still_read() {
        for template in Template::ALL {
            assert_eq!(Template::parse(template.id()), Some(template));
        }
        // What every profile on disk was written with before the names changed on screen.
        assert_eq!(Template::parse("recommended"), Some(Template::Recommended));
        assert_eq!(Template::parse("high"), Some(Template::High));
        assert_eq!(Template::parse("quvyta-dev"), Some(Template::QuvytaDev));
        assert_eq!(Template::parse("slim"), Some(Template::Slim));
        assert_eq!(Template::parse("base"), Some(Template::Base));
        assert_eq!(Template::parse("custom"), Some(Template::Custom));
        assert_eq!(Template::parse("Recommended"), None);
        assert_eq!(Template::parse("basic"), None, "the name on screen is not the id on disk");
        assert_eq!(Template::parse("extra"), None, "the name on screen is not the id on disk");
        assert_eq!(Template::ALL.map(Template::id), ["base", "recommended", "high", "quvyta-dev", "slim", "custom"]);
    }
}
