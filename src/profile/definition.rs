//! The profile definition file: `Profiles/<profile>.toml`.

use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, ValueKind};
use qframe::storage::Settings;

use crate::engine::names;
use crate::profile::{AccountKind, HarnessKind, SafeName, Template};

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
    /// How `Assets/` is mounted.
    pub assets: MountAccess,
    /// What the container may reach.
    pub network: NetworkMode,
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
        let mounts = root.table(MOUNTS);
        let assets = mounts.and_then(|mounts| mounts.text(ASSETS)).and_then(MountAccess::parse);
        let network = root.table(NETWORK).and_then(|network| network.text(MODE)).and_then(NetworkMode::parse);

        let (Some(name), Some(harness), Some(account)) = (name, harness, account) else {
            return Loaded { profile: None, diagnostics };
        };
        if !harness.supports(account) {
            let message =
                format!("`{ACCOUNT}` is `{}`, which {} does not offer", account.id(), harness.record().display_name);
            diagnostics.push(Diagnostic::error(root.value_location(ACCOUNT).cloned(), message));
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
            assets: assets.unwrap_or(MountAccess::ReadOnly),
            network: network.unwrap_or(NetworkMode::Full),
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
        settings.set(TEMPLATE, self.template.id().to_owned());
        settings.set(ACCOUNT, self.account.id().to_owned());
        settings.set(IMAGE, self.image());
        settings.set(&format!("{MOUNTS}.{CODE}"), Self::CODE_MOUNT.id().to_owned());
        settings.set(&format!("{MOUNTS}.{ASSETS}"), self.assets.id().to_owned());
        settings.set(&format!("{NETWORK}.{MODE}"), self.network.id().to_owned());
        settings.to_toml()
    }
}

/// The key of the profile name.
const NAME: &str = "name";
/// The key of the harness.
const HARNESS: &str = "harness";
/// The key of the template.
const TEMPLATE: &str = "template";
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

/// What a definition file may hold. The keys a profile cannot be guessed for are required, so
/// a missing or wrong one is an error; the rest are choices that fall back to the safer of the
/// two when they are missing or wrong.
fn shape() -> Shape {
    let mounts = Shape::new()
        .optional(CODE, ValueKind::choice([Profile::CODE_MOUNT.id()]))
        .optional(LEGACY_CODE, ValueKind::choice([Profile::CODE_MOUNT.id()]))
        .optional(ASSETS, ValueKind::choice(MountAccess::ALL.map(MountAccess::id)));
    let network = Shape::new().optional(MODE, ValueKind::choice(NetworkMode::ALL.map(NetworkMode::id)));
    Shape::new()
        .required(NAME, ValueKind::text())
        .required(HARNESS, ValueKind::choice(HarnessKind::ALL.map(|harness| harness.record().id)))
        .optional(TEMPLATE, ValueKind::choice(Template::ALL.map(Template::id)))
        .required(ACCOUNT, ValueKind::choice(AccountKind::ALL.map(AccountKind::id)))
        .optional(IMAGE, ValueKind::text())
        .table(MOUNTS, mounts)
        .table(NETWORK, network)
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
