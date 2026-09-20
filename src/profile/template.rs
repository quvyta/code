//! Templates: what a profile writes into the harness's own configuration.

use crate::profile::{ConfigFile, HarnessKind};

/// How much of the harness's own configuration a profile brings with it.
///
/// Neither template decides whether the harness asks for permission: it never does, under either
/// template, because the container is what keeps the work apart from the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// The harness as it comes: the image holds no configuration of ours.
    Base,
    /// The harness set up the way QCode runs it.
    Recommended,
}

impl Template {
    /// Every template, in the order the profile wizard offers them.
    pub const ALL: [Self; 2] = [Self::Base, Self::Recommended];

    /// How the template is written in definition files.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Recommended => "recommended",
        }
    }

    /// The template written as `id`, if there is one.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|template| template.id() == id)
    }

    /// The configuration file the image build writes for `harness`, if this template writes one.
    #[must_use]
    pub fn settings(self, harness: HarnessKind) -> Option<ConfigFile> {
        match self {
            Self::Base => None,
            Self::Recommended => harness.record().settings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::HarnessKind;

    #[test]
    fn the_base_template_writes_nothing() {
        for harness in HarnessKind::ALL {
            assert_eq!(Template::Base.settings(harness), None, "{harness:?}");
        }
    }

    #[test]
    fn the_recommended_template_writes_what_the_harness_record_holds() {
        for harness in HarnessKind::ALL {
            assert_eq!(Template::Recommended.settings(harness), harness.record().settings, "{harness:?}");
        }
        let claude = Template::Recommended.settings(HarnessKind::ClaudeCode).expect("claude-code has settings");
        assert_eq!(claude.path, ".claude/settings.json");
        assert!(claude.contents.contains("bypassPermissions"));
        let codex = Template::Recommended.settings(HarnessKind::Codex).expect("codex has settings");
        assert_eq!(codex.path, ".codex/config.toml");
        assert!(codex.contents.contains("approval_policy = \"never\""));
    }

    #[test]
    fn unattended_mode_does_not_depend_on_the_template() {
        // The container is the isolation. A template only writes settings; what makes the
        // harness stop asking are the arguments it is started with, under both templates.
        for harness in HarnessKind::TERMINAL {
            assert!(!harness.record().auto_run.is_empty(), "{harness:?}");
        }
    }

    #[test]
    fn templates_read_back_as_the_same_template() {
        for template in Template::ALL {
            assert_eq!(Template::parse(template.id()), Some(template));
        }
        assert_eq!(Template::parse("Recommended"), None);
        assert_eq!(Template::ALL.map(Template::id), ["base", "recommended"]);
    }
}
