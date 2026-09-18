//! What a profile's image is built from, and the shell the login step runs inside a container.
//!
//! Both are text made from the harness record and the template, and nothing here runs anything:
//! a recipe is checked in tests on a machine with no container engine at all.
//!
//! The recipe belongs to the profile layer rather than to a screen; it lives here until that
//! layer grows an image description of its own.

use std::path::PathBuf;

use crate::base::paths::OPEN_HOME;
use crate::engine::names::BASE_IMAGE;
use crate::profile::Profile;

/// Where the build context puts the files a template writes, so the `COPY` never has to know
/// the home directory of the image.
const STAGING: &str = "/qcode-template";

/// Where the login container can see a directory of the host, which is how a fresh login leaves
/// the container without anyone having to know where the home directory is.
pub const CAPTURE_DIR: &str = "/qcode-capture";

/// A build context: the file that describes the image and everything the build copies in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// The Containerfile, ready to be written beside the files below.
    pub containerfile: String,
    /// Files the build copies in, by their path inside the context directory.
    pub files: Vec<(PathBuf, String)>,
}

/// The recipe that builds `profile`'s image.
///
/// The harness is installed on top of the base image, the variables the harness needs are set in
/// the image itself (a container is created without any), and a template's configuration is
/// copied to a staging path and moved into the home directory by the image's own shell, so the
/// recipe never has to name that directory. The last step opens everything the build wrote into
/// the home directory: the build runs as the image's user and the container as the person's,
/// and where those differ a login could otherwise not be stored beside the settings.
#[must_use]
pub fn image(profile: &Profile) -> Recipe {
    let harness = profile.harness.record();
    let mut lines = vec![format!("FROM {BASE_IMAGE}")];
    for step in harness.install {
        lines.push(format!("RUN {step}"));
    }
    for (key, value) in harness.environment {
        lines.push(format!("ENV {key}=\"{value}\""));
    }
    let mut files = Vec::new();
    if let Some(file) = profile.template.settings(profile.harness) {
        files.push((PathBuf::from("template").join(file.path), file.contents.to_owned()));
        lines.push(format!("COPY template {STAGING}"));
        lines.push(format!(
            "RUN mkdir -p \"$HOME/$(dirname '{path}')\" && cp '{STAGING}/{path}' \"$HOME/{path}\"",
            path = file.path
        ));
    }
    lines.push(format!("RUN {OPEN_HOME}"));
    // The name is on the image so that a stray image can be traced back to its profile.
    lines.push(format!("LABEL qcode.profile=\"{}\"", profile.name));
    lines.push(String::new());
    Recipe { containerfile: lines.join("\n"), files }
}

/// The shell that takes a fresh login out of the container it was made in.
///
/// It checks before it copies: the first file of the harness record is the login itself, and
/// without it the script fails, so a login that never happened can never be mistaken for one
/// that did. The files beside it, which some harnesses write and some do not, are copied when
/// they are there.
#[must_use]
pub fn capture_script(profile: &Profile) -> String {
    let identity = profile.harness.record().identity;
    let mut lines = vec!["set -e".to_owned()];
    if let Some(login) = identity.first() {
        lines.push(format!("test -f \"$HOME/{login}\""));
    }
    for path in identity {
        lines.push(format!(
            "if [ -f \"$HOME/{path}\" ]; then mkdir -p \"{CAPTURE_DIR}/$(dirname '{path}')\" && \
             cp \"$HOME/{path}\" \"{CAPTURE_DIR}/{path}\"; fi"
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, SafeName, Template};

    fn profile(harness: HarnessKind, template: Template) -> Profile {
        Profile {
            name: SafeName::parse("claude-sub").expect("the name is safe"),
            harness,
            template,
            account: AccountKind::Subscription,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
        }
    }

    #[test]
    fn an_image_is_built_on_the_base_image_and_installs_the_harness() {
        let recipe = image(&profile(HarnessKind::ClaudeCode, Template::Base));
        let first = recipe.containerfile.lines().next().expect("the file has a line");
        assert_eq!(first, format!("FROM {BASE_IMAGE}"));
        assert!(recipe.containerfile.contains("RUN npm install -g @anthropic-ai/claude-code"), "{recipe:?}");
    }

    #[test]
    fn the_base_template_copies_nothing_in() {
        let recipe = image(&profile(HarnessKind::ClaudeCode, Template::Base));
        assert!(recipe.files.is_empty(), "{recipe:?}");
        assert!(!recipe.containerfile.contains("COPY"), "{recipe:?}");
    }

    #[test]
    fn the_recommended_template_ships_the_file_the_harness_reads() {
        let recipe = image(&profile(HarnessKind::ClaudeCode, Template::Recommended));
        let (path, contents) = recipe.files.first().expect("the template writes a file");
        assert_eq!(path, &PathBuf::from("template/.claude/settings.json"));
        assert!(contents.contains("bypassPermissions"), "{contents}");
        assert!(recipe.containerfile.contains("COPY template /qcode-template"), "{recipe:?}");
        // The home directory belongs to the base image; the recipe asks the shell for it.
        assert!(recipe.containerfile.contains("\"$HOME/.claude/settings.json\""), "{recipe:?}");
    }

    #[test]
    fn a_harness_that_needs_variables_gets_them_from_the_image() {
        // A container is created without any variables, so the image is the only place they fit.
        let recipe = image(&profile(HarnessKind::GeminiCli, Template::Recommended));
        assert!(recipe.containerfile.contains("ENV GEMINI_FORCE_FILE_STORAGE=\"true\""), "{recipe:?}");
    }

    #[test]
    fn every_harness_gives_a_recipe_that_names_its_profile() {
        for harness in HarnessKind::ALL {
            for template in Template::ALL {
                let recipe = image(&profile(harness, template));
                assert!(recipe.containerfile.contains("LABEL qcode.profile=\"claude-sub\""), "{harness:?}");
                assert!(recipe.containerfile.ends_with('\n'), "{harness:?}");
            }
        }
    }

    #[test]
    fn the_last_run_step_opens_the_home_directory_to_whoever_runs_the_container() {
        // The install and the template both write under `$HOME` as the image's user, and the
        // container is run as the person's own uid. When that is not 1000 the login has nowhere
        // to go unless the build ends by opening what it wrote, so the check is on the last
        // `RUN`, whatever came before it.
        for harness in HarnessKind::ALL {
            for template in Template::ALL {
                let recipe = image(&profile(harness, template));
                let last_run = recipe.containerfile.lines().rfind(|line| line.starts_with("RUN "));
                assert_eq!(last_run, Some(format!("RUN {OPEN_HOME}").as_str()), "{harness:?} {template:?}");
                let opened = recipe.containerfile.find(OPEN_HOME).expect("the step is there");
                let label = recipe.containerfile.find("LABEL ").expect("the label is there");
                assert!(opened < label, "{harness:?} {template:?}: the label may only follow the last step");
            }
        }
    }

    #[test]
    fn capturing_fails_when_the_login_file_is_not_there() {
        let script = capture_script(&profile(HarnessKind::ClaudeCode, Template::Base));
        assert!(script.starts_with("set -e\n"), "{script}");
        assert!(script.contains("test -f \"$HOME/.claude/.credentials.json\""), "{script}");
    }

    #[test]
    fn a_file_the_harness_only_sometimes_writes_is_copied_only_when_it_is_there() {
        let script = capture_script(&profile(HarnessKind::GeminiCli, Template::Base));
        // The login itself is required; the account file beside it is not.
        assert!(script.contains("test -f \"$HOME/.gemini/gemini-credentials.json\""), "{script}");
        assert_eq!(script.matches("test -f").count(), 1, "{script}");
        assert!(script.contains("if [ -f \"$HOME/.gemini/google_accounts.json\" ]"), "{script}");
    }

    #[test]
    fn every_harness_can_be_captured_into_the_shared_directory() {
        for harness in HarnessKind::ALL {
            let script = capture_script(&profile(harness, Template::Base));
            for path in harness.record().identity {
                assert!(script.contains(&format!("{CAPTURE_DIR}/{path}")), "{harness:?}: {path}");
            }
        }
    }
}
