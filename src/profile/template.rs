//! Templates: what a profile writes into the harness's own configuration, and what it installs
//! beside the harness.

use crate::profile::{ConfigFile, HarnessKind};

/// How much of the harness's own configuration a profile brings with it.
///
/// No template decides whether the harness asks for permission: it never does, under any
/// template, because the container is what keeps the work apart from the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// The harness as it comes: the image holds no configuration of ours.
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
    /// the settings, first-start answers and update switches every QCode template writes: "oh my
    /// opencode slim". A template of its own, not a variant of another: what it installs is read
    /// from its own arms below. It never carries oh-my-openagent. Offered to opencode alone
    /// ([`Template::offered`]); a definition written by hand for another harness gets graphify.
    Slim,
}

impl Template {
    /// Every template, in the order the profile wizard offers them.
    pub const ALL: [Self; 5] = [Self::Base, Self::Recommended, Self::High, Self::QuvytaDev, Self::Slim];

    /// How the template is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Recommended => "recommended",
            Self::Slim => "slim",
            Self::High => "high",
            Self::QuvytaDev => "quvyta-dev",
        }
    }

    /// Whether the template carries everything QCode extra gives: QCode extra itself, and Quvyta
    /// development, which is QCode extra and more. What QCode extra does beyond the image — its
    /// instructions written into the workspace — follows from this, not from one name.
    #[must_use]
    pub fn carries_high(self) -> bool {
        matches!(self, Self::High | Self::QuvytaDev)
    }

    /// The templates the wizard offers a profile of `harness`, in order: oh my opencode slim only
    /// to opencode, since the team of agents it is for is opencode's.
    #[must_use]
    pub fn offered(harness: HarnessKind) -> Vec<Self> {
        Self::ALL.into_iter().filter(|template| *template != Self::Slim || harness == HarnessKind::OpenCode).collect()
    }

    /// The template written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|template| template.id() == id)
    }

    /// The configuration files the image build writes for `harness`, in the order they are
    /// written.
    ///
    /// Every QCode template writes the harness's settings and its answers to its first-start
    /// questions. For opencode the settings name oh-my-openagent, which every QCode template
    /// installs: a plugin opencode is not told about is a plugin it never loads.
    #[must_use]
    pub fn files(self, harness: HarnessKind) -> Vec<ConfigFile> {
        match (self, harness) {
            (Self::Base, _) => Vec::new(),
            (Self::Slim, HarnessKind::OpenCode) => {
                [OPENCODE_SLIM_SETTINGS, SLIM_OWN_SETTINGS].into_iter().chain(harness.record().first_start).collect()
            }
            (_, HarnessKind::OpenCode) => {
                std::iter::once(OPENCODE_PLUGIN_SETTINGS).chain(harness.record().first_start).collect()
            }
            _ => Self::plain_files(harness),
        }
    }

    /// The harness's settings and first-start answers from its record, naming no plugin: what a
    /// QCode template writes for a harness whose plugin was switched off.
    #[must_use]
    pub fn plain_files(harness: HarnessKind) -> Vec<ConfigFile> {
        let record = harness.record();
        record.settings.into_iter().chain(record.first_start).collect()
    }

    /// The variables every QCode template sets in the image for `harness`: the maker's switches
    /// for its own update check and usage reports, where the harness offers them as variables
    /// rather than as keys of its settings file (those are in the file itself).
    ///
    /// An update the harness installs by itself would change the image's program under the
    /// person's feet, inside a container that is remade from the image anyway; a rebuild is how
    /// an update arrives. Every name was read in the harness's own package: Claude Code 2.1.281
    /// lists all three among the variables it reads; opencode 1.18.32 returns from its update
    /// check when `OPENCODE_DISABLE_AUTOUPDATE` is set; Kimi Code CLI 2.1.1 reads
    /// `KIMI_DISABLE_TELEMETRY` for its usage reports and `KIMI_CODE_NO_AUTO_UPDATE` for "no
    /// check, no background install, no prompt".
    #[must_use]
    pub fn environment(self, harness: HarnessKind) -> &'static [(&'static str, &'static str)] {
        match (self, harness) {
            (Self::Base, _) => &[],
            (_, HarnessKind::ClaudeCode) => {
                &[("DISABLE_AUTOUPDATER", "1"), ("DISABLE_TELEMETRY", "1"), ("DISABLE_ERROR_REPORTING", "1")]
            }
            (_, HarnessKind::OpenCode) => &[("OPENCODE_DISABLE_AUTOUPDATE", "1")],
            (_, HarnessKind::KimiCode) => &[("KIMI_DISABLE_TELEMETRY", "1"), ("KIMI_CODE_NO_AUTO_UPDATE", "1")],
            (_, HarnessKind::GeminiCli | HarnessKind::Codex | HarnessKind::QwenCode | HarnessKind::AntigravityIde) => {
                &[]
            }
        }
    }

    /// The Claude Code plugins the template installs, as `claude plugin install` takes them:
    /// the five of [`CLAUDE_STARTER_PLUGINS`] under QCode recommended, every one of
    /// [`CLAUDE_PLUGINS`] under QCode extra and Quvyta development, none under base.
    #[must_use]
    pub fn claude_plugins(self) -> &'static [&'static str] {
        match self {
            Self::Base => &[],
            Self::Recommended => &CLAUDE_STARTER_PLUGINS,
            Self::Slim => &[],
            Self::High | Self::QuvytaDev => &CLAUDE_PLUGINS,
        }
    }

    /// The parts of [`Template::additions`] a profile of `harness` can go without, in the order
    /// the wizard lists them.
    #[must_use]
    pub fn extras(self, harness: HarnessKind) -> Vec<Extra> {
        self.additions(harness)
            .iter()
            .flat_map(|addition| match addition {
                Addition::Graphify => vec![Extra::Graphify],
                Addition::ClaudePlugins => self.claude_plugins().iter().copied().map(Extra::Plugin).collect(),
                Addition::OhMyOpenAgent => vec![Extra::OhMyOpenAgent],
                Addition::OhMyOpenCodeSlim => vec![Extra::OhMyOpenCodeSlim],
                // The toolchain is what the template is for; without it, it is QCode extra.
                Addition::Rust => Vec::new(),
                Addition::Chromium => vec![Extra::Chromium],
            })
            .collect()
    }

    /// What the image gets beside the harness under this template, in the order it is installed.
    ///
    /// Every QCode template gives every harness graphify, Claude Code its plugins
    /// ([`Template::claude_plugins`] says which) and opencode oh-my-openagent; Quvyta development
    /// adds Rust and Chromium to QCode extra's.
    #[must_use]
    pub fn additions(self, harness: HarnessKind) -> &'static [Addition] {
        match (self, harness) {
            (Self::Base, _) => &[],
            (Self::Slim, HarnessKind::OpenCode) => &[Addition::Graphify, Addition::OhMyOpenCodeSlim],
            (Self::Slim, _) => &[Addition::Graphify],
            (Self::Recommended | Self::High, HarnessKind::ClaudeCode) => &[Addition::Graphify, Addition::ClaudePlugins],
            (Self::Recommended | Self::High, HarnessKind::OpenCode) => &[Addition::Graphify, Addition::OhMyOpenAgent],
            (Self::Recommended | Self::High, _) => &[Addition::Graphify],
            (Self::QuvytaDev, HarnessKind::ClaudeCode) => {
                &[Addition::Graphify, Addition::ClaudePlugins, Addition::Rust, Addition::Chromium]
            }
            (Self::QuvytaDev, HarnessKind::OpenCode) => {
                &[Addition::Graphify, Addition::OhMyOpenAgent, Addition::Rust, Addition::Chromium]
            }
            (Self::QuvytaDev, _) => &[Addition::Graphify, Addition::Rust, Addition::Chromium],
        }
    }
}

/// Something a QCode template installs into an image beside the harness.
///
/// Every one of them is downloaded while the image is built, unpinned, like the harnesses
/// themselves: a rebuild is how an update arrives.
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

/// One part of what a QCode template adds that a profile can go without: graphify, one of the
/// Claude Code plugins, oh-my-openagent, or Chromium.
///
/// The owner approved the additions as on unless switched off, so a profile names only
/// what it goes without, and a profile that names nothing has everything — which is also how every
/// profile written before the choice existed reads.
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
    /// Chromium, under Quvyta development; without it, no browser is installed.
    Chromium,
}

impl Extra {
    /// Every part, in the order the wizard lists them.
    #[must_use]
    pub fn all() -> Vec<Self> {
        let mut all = vec![Self::Graphify];
        all.extend(CLAUDE_PLUGINS.map(Self::Plugin));
        all.push(Self::OhMyOpenAgent);
        all.push(Self::OhMyOpenCodeSlim);
        all.push(Self::Chromium);
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
            Self::Chromium => "chromium",
        }
    }

    /// The part written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::all().into_iter().find(|extra| extra.id() == id)
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

/// Every Claude Code plugin QCode extra installs: the ones the owner of QCode works with, the
/// starter set among them.
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
    use crate::profile::HarnessKind;

    /// A plugin's own name, without its marketplace.
    fn name(plugin: &str) -> &str {
        plugin.split_once('@').map_or(plugin, |(name, _)| name)
    }

    #[test]
    fn the_base_template_writes_nothing_and_adds_nothing() {
        for harness in HarnessKind::ALL {
            assert_eq!(Template::Base.files(harness), Vec::new(), "{harness:?}");
            assert!(Template::Base.additions(harness).is_empty(), "{harness:?}");
            assert!(Template::Base.extras(harness).is_empty(), "{harness:?}");
        }
        assert!(Template::Base.claude_plugins().is_empty());
    }

    #[test]
    fn qcode_recommended_writes_what_the_harness_record_holds_and_adds_graphify_and_the_makers_plugins() {
        for harness in HarnessKind::ALL {
            let wanted = match harness {
                HarnessKind::OpenCode => vec![OPENCODE_PLUGIN_SETTINGS],
                _ => Template::plain_files(harness),
            };
            assert_eq!(Template::Recommended.files(harness), wanted, "{harness:?}");
            let additions = Template::Recommended.additions(harness);
            assert_eq!(additions.first(), Some(&Addition::Graphify), "{harness:?}: graphify for every harness");
            let wanted: &[Addition] = match harness {
                HarnessKind::ClaudeCode => &[Addition::Graphify, Addition::ClaudePlugins],
                HarnessKind::OpenCode => &[Addition::Graphify, Addition::OhMyOpenAgent],
                _ => &[Addition::Graphify],
            };
            assert_eq!(additions, wanted, "{harness:?}");
        }
        let claude = Template::Recommended.files(HarnessKind::ClaudeCode);
        assert_eq!(claude[0].path, ".claude/settings.json");
        assert!(claude[0].contents.contains("bypassPermissions"));
        let codex = Template::Recommended.files(HarnessKind::Codex);
        assert_eq!(codex[0].path, ".codex/config.toml");
        assert!(codex[0].contents.contains("approval_policy = \"never\""));
    }

    #[test]
    fn every_qcode_template_answers_claude_codes_first_questions() {
        // Each key was taken away once in a container and its question came back; see the
        // record of Claude Code.
        for template in [Template::Recommended, Template::High, Template::QuvytaDev] {
            let files = template.files(HarnessKind::ClaudeCode);
            let settings = files.iter().find(|file| file.path == ".claude/settings.json").expect("settings");
            let settings: serde_json::Value = serde_json::from_str(settings.contents).expect("JSON");
            assert_eq!(settings["skipDangerousModePermissionPrompt"], true, "{template:?}");
            let state = files.iter().find(|file| file.path == ".claude.json").expect("the first-start answers");
            let state: serde_json::Value = serde_json::from_str(state.contents).expect("JSON");
            assert_eq!(state["hasCompletedOnboarding"], true, "{template:?}");
            let trusted = &state["projects"][crate::base::paths::CODE_DIR]["hasTrustDialogAccepted"];
            assert_eq!(trusted, true, "{template:?}: the folder the workspace is mounted at");
        }
        assert!(Template::Base.files(HarnessKind::ClaudeCode).is_empty(), "base is the harness as it comes");
    }

    #[test]
    fn qcode_extra_is_qcode_recommended_with_every_plugin_the_owner_uses() {
        for harness in HarnessKind::ALL {
            assert_eq!(Template::High.additions(harness), Template::Recommended.additions(harness), "{harness:?}");
            assert_eq!(Template::High.files(harness), Template::Recommended.files(harness), "{harness:?}");
            assert_eq!(
                Template::High.environment(harness),
                Template::Recommended.environment(harness),
                "{harness:?}: the same switches"
            );
            let recommended = Template::Recommended.extras(harness);
            let extra = Template::High.extras(harness);
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
        assert_eq!(Template::High.claude_plugins(), CLAUDE_PLUGINS);
    }

    #[test]
    fn the_plugins_are_claude_codes_and_oh_my_openagent_opencodes_under_every_qcode_template() {
        for template in Template::ALL {
            for harness in HarnessKind::ALL {
                let additions = template.additions(harness);
                assert_eq!(
                    additions.contains(&Addition::ClaudePlugins),
                    !matches!(template, Template::Base | Template::Slim) && harness == HarnessKind::ClaudeCode,
                    "{template:?} {harness:?}: the plugins are Claude Code's alone, and not slim's"
                );
                assert_eq!(
                    additions.contains(&Addition::OhMyOpenAgent),
                    !matches!(template, Template::Base | Template::Slim) && harness == HarnessKind::OpenCode,
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
    fn quvyta_development_is_qcode_extra_with_rust_and_chromium() {
        for harness in HarnessKind::ALL {
            let dev = Template::QuvytaDev.additions(harness);
            assert!(dev.starts_with(Template::High.additions(harness)), "{harness:?}: QCode extra's come first");
            assert_eq!(&dev[Template::High.additions(harness).len()..], [Addition::Rust, Addition::Chromium]);
            assert_eq!(Template::QuvytaDev.files(harness), Template::High.files(harness), "{harness:?}");
            assert_eq!(Template::QuvytaDev.environment(harness), Template::High.environment(harness));
            let extras = Template::QuvytaDev.extras(harness);
            assert_eq!(extras.last(), Some(&Extra::Chromium), "{harness:?}: Chromium can be switched off");
            assert_eq!(&extras[..extras.len() - 1], Template::High.extras(harness), "{harness:?}");
        }
        assert!(Template::QuvytaDev.carries_high() && Template::High.carries_high());
        assert!(!Template::Recommended.carries_high() && !Template::Base.carries_high());
        for template in [Template::Base, Template::Recommended, Template::High] {
            for harness in HarnessKind::ALL {
                let additions = template.additions(harness);
                assert!(!additions.contains(&Addition::Rust), "{template:?} {harness:?}");
                assert!(!additions.contains(&Addition::Chromium), "{template:?} {harness:?}");
            }
        }
    }

    #[test]
    fn every_qcode_template_tells_opencode_to_load_the_plugin_from_the_image() {
        for template in [Template::Recommended, Template::High, Template::QuvytaDev] {
            assert_eq!(template.files(HarnessKind::OpenCode), [OPENCODE_PLUGIN_SETTINGS], "{template:?}");
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
    fn unattended_mode_does_not_depend_on_the_template() {
        // The container is the isolation. A template only writes settings; what makes the
        // harness stop asking are the arguments it is started with, under every template.
        for harness in HarnessKind::TERMINAL {
            assert!(!harness.record().auto_run.is_empty(), "{harness:?}");
        }
    }

    #[test]
    fn every_part_reads_back_as_itself_by_its_makers_name() {
        let ids: Vec<&str> = Extra::all().into_iter().map(Extra::id).collect();
        let mut wanted = vec!["graphify"];
        wanted.extend(CLAUDE_PLUGINS.iter().map(|plugin| name(plugin)));
        wanted.extend(["oh-my-openagent", "oh-my-opencode-slim", "chromium"]);
        assert_eq!(ids, wanted);
        for extra in Extra::all() {
            assert_eq!(Extra::parse(extra.id()), Some(extra));
        }
        assert_eq!(Extra::parse("context7@claude-plugins-official"), None, "the file names it by its own name");
        assert_eq!(Extra::parse("hookify"), Some(Extra::Plugin("hookify@claude-plugins-official")));
    }

    #[test]
    fn each_harness_offers_the_parts_it_gets() {
        let ids = |template: Template, harness| -> Vec<&str> {
            template.extras(harness).into_iter().map(Extra::id).collect()
        };
        assert_eq!(
            ids(Template::Recommended, HarnessKind::ClaudeCode),
            ["graphify", "superpowers", "context7", "code-review", "security-guidance", "block-no-verify"]
        );
        assert_eq!(ids(Template::High, HarnessKind::ClaudeCode).len(), 17, "graphify and the sixteen plugins");
        for template in [Template::Recommended, Template::High] {
            assert_eq!(ids(template, HarnessKind::OpenCode), ["graphify", "oh-my-openagent"], "{template:?}");
            assert_eq!(ids(template, HarnessKind::Codex), ["graphify"], "{template:?}");
        }
    }

    #[test]
    fn the_qcode_templates_turn_the_makers_update_checks_and_reports_off_and_base_does_not() {
        // Written out, so a switch dropped from the list is a failing test.
        let claude = Template::Recommended.environment(HarnessKind::ClaudeCode);
        assert_eq!(
            claude,
            [("DISABLE_AUTOUPDATER", "1"), ("DISABLE_TELEMETRY", "1"), ("DISABLE_ERROR_REPORTING", "1")]
        );
        assert_eq!(Template::Recommended.environment(HarnessKind::OpenCode), [("OPENCODE_DISABLE_AUTOUPDATE", "1")]);
        assert_eq!(
            Template::Recommended.environment(HarnessKind::KimiCode),
            [("KIMI_DISABLE_TELEMETRY", "1"), ("KIMI_CODE_NO_AUTO_UPDATE", "1")]
        );
        // The rest have keys of their settings file for it.
        let settings = |harness: HarnessKind| Template::Recommended.files(harness)[0].contents;
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
            assert!(Template::Base.environment(harness).is_empty(), "{harness:?}: base is the harness as it comes");
        }
    }

    #[test]
    fn every_qcode_template_opens_antigravity_without_asking_whether_the_folder_is_trusted() {
        // Workspace trust off: the window neither asks about the folder nor keeps it in
        // restricted mode. `permission_live.rs` checks the installed application reads the key.
        for template in [Template::Recommended, Template::High] {
            let files = template.files(HarnessKind::AntigravityIde);
            let settings: serde_json::Value = serde_json::from_str(files[0].contents).expect("JSON");
            assert_eq!(settings["security.workspace.trust.enabled"], false, "{template:?}");
        }
        assert!(Template::Base.files(HarnessKind::AntigravityIde).is_empty());
    }

    #[test]
    fn templates_read_back_as_the_same_template_and_old_files_still_read() {
        for template in Template::ALL {
            assert_eq!(Template::parse(template.id()), Some(template));
        }
        // What every profile on disk was written with before the names changed on screen.
        assert_eq!(Template::parse("recommended"), Some(Template::Recommended));
        assert_eq!(Template::parse("high"), Some(Template::High));
        assert_eq!(Template::parse("Recommended"), None);
        assert_eq!(Template::parse("basic"), None, "the name on screen is not the id on disk");
        assert_eq!(Template::parse("extra"), None, "the name on screen is not the id on disk");
        assert_eq!(Template::ALL.map(Template::id), ["base", "recommended", "high", "quvyta-dev", "slim"]);
    }
}
