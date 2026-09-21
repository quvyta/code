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
    /// The harness set up the way QCode runs it: "QCode basic". The id is older than the name,
    /// and stays, because profiles on disk are written with it.
    Recommended,
    /// QCode basic, and the tools the owner of QCode works with installed beside the harness:
    /// "QCode high". See [`Addition`].
    High,
}

impl Template {
    /// Every template, in the order the profile wizard offers them.
    pub const ALL: [Self; 3] = [Self::Base, Self::Recommended, Self::High];

    /// How the template is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Recommended => "recommended",
            Self::High => "high",
        }
    }

    /// The template written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|template| template.id() == id)
    }

    /// The configuration files the image build writes for `harness`, in the order they are
    /// written.
    ///
    /// Both QCode templates write the harness's settings and its answers to its first-start
    /// questions. QCode high writes opencode's settings with oh-my-openagent in them instead of
    /// the plain ones: a plugin opencode is not told about is a plugin it never loads.
    #[must_use]
    pub fn files(self, harness: HarnessKind) -> Vec<ConfigFile> {
        let record = harness.record();
        let settings = match (self, harness) {
            (Self::Base, _) => return Vec::new(),
            (Self::High, HarnessKind::OpenCode) => Some(OPENCODE_HIGH_SETTINGS),
            _ => record.settings,
        };
        settings.into_iter().chain(record.first_start).collect()
    }

    /// The parts of [`Template::additions`] a profile of `harness` can go without, in the order
    /// the wizard lists them.
    #[must_use]
    pub fn extras(self, harness: HarnessKind) -> Vec<Extra> {
        self.additions(harness)
            .iter()
            .flat_map(|addition| match addition {
                Addition::Graphify => vec![Extra::Graphify],
                Addition::ClaudePlugins => CLAUDE_PLUGINS.map(Extra::Plugin).to_vec(),
                Addition::OhMyOpenAgent => vec![Extra::OhMyOpenAgent],
            })
            .collect()
    }

    /// What the image gets beside the harness under this template, in the order it is installed.
    #[must_use]
    pub fn additions(self, harness: HarnessKind) -> &'static [Addition] {
        match (self, harness) {
            (Self::Base | Self::Recommended, _) => &[],
            (Self::High, HarnessKind::ClaudeCode) => &[Addition::Graphify, Addition::ClaudePlugins],
            (Self::High, HarnessKind::OpenCode) => &[Addition::Graphify, Addition::OhMyOpenAgent],
            (Self::High, _) => &[Addition::Graphify],
        }
    }
}

/// Something QCode high installs into an image beside the harness.
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
    /// The Claude Code plugins in [`CLAUDE_PLUGINS`], from the marketplaces in
    /// [`CLAUDE_MARKETPLACES`]. Claude Code only; the others have their own counterparts.
    ClaudePlugins,
    /// oh-my-openagent (npm), opencode's plugin, loaded from the image by the path in
    /// `OPENCODE_HIGH_SETTINGS`. opencode only.
    OhMyOpenAgent,
}

/// One part of what QCode high adds that a profile can go without: graphify, one of the Claude
/// Code plugins, or oh-my-openagent.
///
/// The owner approved QCode high's additions as on unless switched off, so a profile names only
/// what it goes without, and a profile that names nothing has everything — which is also how every
/// profile written before the choice existed reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    /// graphify; without it, nothing of graphify is installed and nothing of it is set up in the
    /// workspace.
    Graphify,
    /// One Claude Code plugin, as `claude plugin install` takes it: one of [`CLAUDE_PLUGINS`].
    Plugin(&'static str),
    /// oh-my-openagent; without it, opencode is set up as under QCode basic.
    OhMyOpenAgent,
}

impl Extra {
    /// Every part, in the order the wizard lists them.
    #[must_use]
    pub fn all() -> Vec<Self> {
        let mut all = vec![Self::Graphify];
        all.extend(CLAUDE_PLUGINS.map(Self::Plugin));
        all.push(Self::OhMyOpenAgent);
        all
    }

    /// How the part is written in definition files, which is also the name its maker gives it.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Graphify => "graphify",
            Self::Plugin(plugin) => plugin.split_once('@').map_or(plugin, |(name, _)| name),
            Self::OhMyOpenAgent => OH_MY_OPENAGENT,
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

/// The npm package of oh-my-openagent.
pub const OH_MY_OPENAGENT: &str = "oh-my-openagent";

/// The marketplaces the Claude Code plugins come from: the repository `claude plugin
/// marketplace add` is given, and the name the marketplace calls itself, which is what a plugin
/// is installed by.
pub const CLAUDE_MARKETPLACES: [(&str, &str); 2] =
    [("anthropics/claude-plugins-official", "claude-plugins-official"), ("wshobson/agents", "claude-code-workflows")];

/// The Claude Code plugins QCode high installs, as `claude plugin install` takes them. The
/// owner chose these five because none of them does what graphify already does.
pub const CLAUDE_PLUGINS: [&str; 5] = [
    "superpowers@claude-plugins-official",
    "context7@claude-plugins-official",
    "code-review@claude-plugins-official",
    "security-guidance@claude-plugins-official",
    "block-no-verify@claude-code-workflows",
];

/// opencode's settings under QCode high: the permission of QCode basic, and oh-my-openagent by
/// the path npm installed it to in the image.
///
/// The path, not the package name, on purpose. `omo install` writes `oh-my-openagent@latest`,
/// which opencode resolves itself on every start: it downloads the package again, 469 MB, into
/// `~/.cache/opencode` — the workspace's home volume — and cannot at all in a container without
/// the network. By path, opencode loads the copy the build installed, with or without one.
pub const OPENCODE_HIGH_SETTINGS: ConfigFile = ConfigFile {
    path: ".config/opencode/opencode.json",
    contents: "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"permission\": {\n    \"*\": \"allow\"\n  },\n  \"plugin\": [\n    \"file:///usr/local/npm/lib/node_modules/oh-my-openagent\"\n  ]\n}\n",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::HarnessKind;

    #[test]
    fn the_base_template_writes_nothing_and_adds_nothing() {
        for harness in HarnessKind::ALL {
            assert_eq!(Template::Base.files(harness), Vec::new(), "{harness:?}");
            assert!(Template::Base.additions(harness).is_empty(), "{harness:?}");
        }
    }

    #[test]
    fn qcode_basic_writes_what_the_harness_record_holds() {
        for harness in HarnessKind::ALL {
            let record = harness.record();
            let wanted: Vec<ConfigFile> = record.settings.into_iter().chain(record.first_start).collect();
            assert_eq!(Template::Recommended.files(harness), wanted, "{harness:?}");
            assert!(Template::Recommended.additions(harness).is_empty(), "{harness:?}");
        }
        let claude = Template::Recommended.files(HarnessKind::ClaudeCode);
        assert_eq!(claude[0].path, ".claude/settings.json");
        assert!(claude[0].contents.contains("bypassPermissions"));
        let codex = Template::Recommended.files(HarnessKind::Codex);
        assert_eq!(codex[0].path, ".codex/config.toml");
        assert!(codex[0].contents.contains("approval_policy = \"never\""));
    }

    #[test]
    fn both_qcode_templates_answer_claude_codes_first_questions() {
        // Each key was taken away once in a container and its question came back; see the
        // record of Claude Code.
        for template in [Template::Recommended, Template::High] {
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
    fn qcode_high_is_qcode_basic_with_its_additions() {
        for harness in HarnessKind::ALL {
            let additions = Template::High.additions(harness);
            assert_eq!(additions.first(), Some(&Addition::Graphify), "{harness:?}: graphify is for everyone");
            assert_eq!(
                additions.contains(&Addition::ClaudePlugins),
                harness == HarnessKind::ClaudeCode,
                "{harness:?}: the plugins are Claude Code's alone"
            );
            assert_eq!(
                additions.contains(&Addition::OhMyOpenAgent),
                harness == HarnessKind::OpenCode,
                "{harness:?}: oh-my-openagent is opencode's alone"
            );
            if harness != HarnessKind::OpenCode {
                assert_eq!(Template::High.files(harness), Template::Recommended.files(harness), "{harness:?}");
            }
        }
    }

    #[test]
    fn qcode_high_tells_opencode_to_load_the_plugin_from_the_image() {
        let files = Template::High.files(HarnessKind::OpenCode);
        assert_eq!(files, [OPENCODE_HIGH_SETTINGS]);
        let basic = HarnessKind::OpenCode.record().settings.expect("opencode has settings");
        assert_eq!(OPENCODE_HIGH_SETTINGS.path, basic.path, "the same file, not a second one");
        let high: serde_json::Value = serde_json::from_str(OPENCODE_HIGH_SETTINGS.contents).expect("JSON");
        let basic: serde_json::Value = serde_json::from_str(basic.contents).expect("JSON");
        assert_eq!(high["permission"], basic["permission"], "everything QCode basic says still holds");
        let plugin = high["plugin"][0].as_str().expect("one plugin");
        // The path npm installs to in the base image: its prefix, then `lib/node_modules`.
        assert_eq!(plugin, format!("file:///usr/local/npm/lib/node_modules/{OH_MY_OPENAGENT}"));
        assert!(!plugin.contains('@'), "a version spec makes opencode download it again at every start");
    }

    #[test]
    fn the_plugins_are_the_five_the_owner_approved() {
        // Written out rather than read from the list, so that a plugin dropped from the list or
        // one slipped into it is a failing test, not a quieter image.
        let names: Vec<&str> =
            CLAUDE_PLUGINS.iter().map(|plugin| plugin.split_once('@').map_or(*plugin, |(name, _)| name)).collect();
        assert_eq!(names, ["superpowers", "context7", "code-review", "security-guidance", "block-no-verify"]);
    }

    #[test]
    fn the_plugins_come_from_the_marketplaces_that_are_added() {
        for plugin in CLAUDE_PLUGINS {
            let (_, marketplace) = plugin.split_once('@').expect("a plugin is named with its marketplace");
            assert!(CLAUDE_MARKETPLACES.iter().any(|(_, name)| *name == marketplace), "{plugin}");
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
    fn every_part_of_qcode_high_reads_back_as_itself_by_its_makers_name() {
        let ids: Vec<&str> = Extra::all().into_iter().map(Extra::id).collect();
        assert_eq!(
            ids,
            [
                "graphify",
                "superpowers",
                "context7",
                "code-review",
                "security-guidance",
                "block-no-verify",
                "oh-my-openagent"
            ]
        );
        for extra in Extra::all() {
            assert_eq!(Extra::parse(extra.id()), Some(extra));
        }
        assert_eq!(Extra::parse("context7@claude-plugins-official"), None, "the file names it by its own name");
    }

    #[test]
    fn each_harness_offers_the_parts_it_gets() {
        let ids = |harness| -> Vec<&str> { Template::High.extras(harness).into_iter().map(Extra::id).collect() };
        assert_eq!(ids(HarnessKind::ClaudeCode).len(), 6, "graphify and the five plugins");
        assert_eq!(ids(HarnessKind::OpenCode), ["graphify", "oh-my-openagent"]);
        assert_eq!(ids(HarnessKind::Codex), ["graphify"]);
        assert!(Template::Recommended.extras(HarnessKind::ClaudeCode).is_empty());
    }

    #[test]
    fn templates_read_back_as_the_same_template() {
        for template in Template::ALL {
            assert_eq!(Template::parse(template.id()), Some(template));
        }
        assert_eq!(Template::parse("high"), Some(Template::High));
        assert_eq!(Template::parse("Recommended"), None);
        assert_eq!(Template::parse("basic"), None, "the name on screen is not the id on disk");
        assert_eq!(Template::ALL.map(Template::id), ["base", "recommended", "high"]);
    }
}
