//! What a profile's image is built from, and the shell the login step runs inside a container.
//!
//! Both are text made from the harness record and the template, and nothing here runs anything:
//! a recipe is checked in tests on a machine with no container engine at all.
//!
//! The recipe belongs to the profile layer rather than to a screen; it lives here until that
//! layer grows an image description of its own.

use std::path::PathBuf;

use crate::base::paths::{OPEN_HOME, USER};
use crate::desktop::signin;
use crate::engine::names::BASE_IMAGE;
use crate::profile::{Desktop, Profile};

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
///
/// A harness that opens a window is installed by [`desktop_steps`] instead of from a registry: it
/// needs system packages and an archive unpacked into `/opt`, neither of which the base image's
/// own user may do, so those steps run as root and the image ends as its user again.
#[must_use]
pub fn image(profile: &Profile) -> Recipe {
    let harness = profile.harness.record();
    let mut lines = vec![format!("FROM {BASE_IMAGE}")];
    let desktop = profile.harness.desktop();
    if let Some(desktop) = desktop {
        // Root, because system packages and /opt are not the image user's to write. The image
        // goes back to its own user at the end, below.
        lines.push("USER root".to_owned());
        lines.extend(desktop_steps(desktop));
    }
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
    if desktop.is_some() {
        lines.push(format!("USER {USER}"));
    }
    lines.push(String::new());
    Recipe { containerfile: lines.join("\n"), files }
}

/// The steps that put a desktop application into the image: the packages it needs, then the
/// archive from its maker's address.
///
/// The download and the check are one step, so a build never keeps an archive that is not the one
/// this version of QCode was written against: the digest is verified before anything is unpacked,
/// and the archive is removed in the same step so it does not become a layer of its own. The
/// archive holds a single directory, which is stripped, so the program lands directly under the
/// install directory whatever the maker renames that directory to.
///
/// The unpacking is done as root on purpose: the archive carries a set-user-id helper the
/// application's own sandbox uses when it cannot open an unprivileged user namespace, and only
/// root can restore that bit. Nothing else in the image needs root.
fn desktop_steps(desktop: &Desktop) -> Vec<String> {
    let packages = desktop.packages.join(" ");
    let dir = desktop.install_dir;
    // The program the application runs to open a web address, and the folder it writes into. The
    // folder is made in the image so that a container given nothing there still has it.
    //
    // The script is written with `printf '%b'` from a single line: a `RUN` step is one line, so
    // the script's own line breaks travel as `\n` and are turned back into breaks by printf.
    let opener = format!(
        "RUN printf '%b' '{script}' > '{program}' \\\n && chmod 0755 '{program}' \\\n && mkdir -p '{folder}'",
        script = signin::script().replace('\\', "\\\\").replace('\'', "'\\''").replace('\n', "\\n"),
        program = signin::OPEN_PROGRAM,
        folder = signin::OPEN_DIR,
    );
    vec![
        opener,
        format!(
            "RUN apt-get update \\\n && apt-get install --yes --no-install-recommends {packages} \\\n \
             && rm -rf /var/lib/apt/lists/*"
        ),
        format!(
            "RUN curl --fail --silent --show-error --location --output /tmp/desktop.tar.gz '{archive}' \\\n \
             && echo '{sha}  /tmp/desktop.tar.gz' | sha256sum --check --strict - \\\n \
             && mkdir -p '{dir}' \\\n \
             && tar --extract --gzip --file /tmp/desktop.tar.gz --directory '{dir}' --strip-components=1 \\\n \
             && rm /tmp/desktop.tar.gz \\\n \
             && test -x '{program}'",
            archive = desktop.archive,
            sha = desktop.sha256,
            program = desktop.command(),
        ),
    ]
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
    fn a_window_is_installed_from_its_maker_and_never_carried_inside_qcode() {
        let recipe = image(&profile(HarnessKind::AntigravityIde, Template::Recommended));
        let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
        assert!(recipe.containerfile.contains("RUN curl "), "{recipe:?}");
        assert!(recipe.containerfile.contains(desktop.archive), "the archive comes from its maker");
        assert!(recipe.containerfile.contains(desktop.sha256), "nothing is unpacked unchecked");
        assert!(recipe.containerfile.contains("sha256sum --check --strict"), "{recipe:?}");
        // The digest is checked before the archive is opened, and the archive does not stay.
        let unpack = recipe.containerfile.find("tar --extract").expect("the archive is unpacked");
        assert!(recipe.containerfile.find("sha256sum").expect("checked") < unpack, "{recipe:?}");
        assert!(recipe.containerfile.contains("rm /tmp/desktop.tar.gz"), "{recipe:?}");
        // The one directory inside the archive is stripped, so the program is where the record says.
        assert!(recipe.containerfile.contains("--strip-components=1"), "{recipe:?}");
        assert!(recipe.containerfile.contains(&format!("test -x '{}'", desktop.command())), "{recipe:?}");
        // Nothing of the application is in QCode's own files: only its address.
        assert!(recipe.files.iter().all(|(path, _)| path.starts_with("template")), "{recipe:?}");
    }

    #[test]
    fn a_window_installs_its_packages_as_root_and_the_image_ends_as_its_own_user() {
        let recipe = image(&profile(HarnessKind::AntigravityIde, Template::Recommended));
        let file = &recipe.containerfile;
        // Root only for the two steps that need it: system packages and /opt.
        let root = file.find("USER root").expect("the packages need root");
        let apt = file.find("apt-get install").expect("the packages are installed");
        let back = file.rfind(&format!("USER {USER}")).expect("the image ends as its own user");
        assert!(root < apt && apt < back, "{file}");
        assert!(file.trim_end().ends_with(&format!("USER {USER}")), "a container must not default to root: {file}");
        for package in HarnessKind::AntigravityIde.desktop().expect("a window").packages {
            assert!(file.contains(package), "{package} is not installed");
        }
        assert!(file.contains("rm -rf /var/lib/apt/lists/*"), "the package lists stay in the image: {file}");
    }

    #[test]
    fn a_command_line_harness_never_becomes_root_in_its_image() {
        for harness in HarnessKind::TERMINAL {
            for template in Template::ALL {
                let file = image(&profile(harness, template)).containerfile;
                assert!(!file.contains("USER root"), "{harness:?} {template:?}: {file}");
                assert!(!file.contains("apt-get"), "{harness:?} {template:?}: {file}");
            }
        }
    }

    #[test]
    fn a_window_gets_its_settings_into_the_home_the_same_way_every_harness_does() {
        // The file lands in the image's home directory, which is copied into the project's home
        // volume the first time a container of the profile starts and never again, so what the
        // person changes afterwards stays theirs.
        let recipe = image(&profile(HarnessKind::AntigravityIde, Template::Recommended));
        let (path, contents) = recipe.files.first().expect("the template writes a file");
        assert_eq!(path, &PathBuf::from("template/.config/Antigravity IDE/User/settings.json"));
        assert!(contents.contains("\"telemetry.telemetryLevel\": \"off\""), "{contents}");
        assert!(recipe.containerfile.contains("\"$HOME/.config/Antigravity IDE/User/settings.json\""), "{recipe:?}");
        assert!(image(&profile(HarnessKind::AntigravityIde, Template::Base)).files.is_empty(), "base writes nothing");
    }

    #[test]
    fn a_window_has_no_login_for_the_sign_in_container_to_take() {
        // The person signs in inside the window, where the application stores it in the project's
        // home volume; there is no file to copy out and the script would have nothing to do.
        let script = capture_script(&profile(HarnessKind::AntigravityIde, Template::Base));
        assert_eq!(script, "set -e");
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
    #[test]
    fn the_image_writes_the_opener_and_a_shell_turns_it_back_into_the_script() {
        let recipe = image(&profile(HarnessKind::AntigravityIde, Template::Recommended));
        let program = crate::desktop::signin::OPEN_PROGRAM;
        let step = recipe
            .containerfile
            .split("\nRUN ")
            .find(|step| step.contains(program))
            .expect("the image installs the opener")
            .to_owned();
        assert!(recipe.containerfile.contains("xdg-utils"), "and the call that runs it is installed");
        // A build step is one line; what looks like several is a line continuation.
        assert!(step.lines().count() > 1, "the step is written over continuations");
        assert!(step.lines().rev().skip(1).all(|line| line.trim_end().ends_with('\\')), "{step}");

        // The proof that the escaping is right: a shell runs the step and the file it writes is
        // the script, with only the paths changed to this test's own.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = std::env::temp_dir().join(format!("qcode-opener-{stamp}"));
        let open_dir = dir.join("open");
        std::fs::create_dir_all(&dir).expect("a folder of this test's own");
        let here = dir.join("qcode-open");
        let script = step
            .replace(program, &here.display().to_string())
            .replace(crate::desktop::signin::OPEN_DIR, &open_dir.display().to_string());
        let ran = std::process::Command::new("sh").arg("-c").arg(&script).status().expect("a shell runs the step");
        assert!(ran.success(), "{script}");
        let written = std::fs::read_to_string(&here).expect("the opener is written");
        let wanted =
            crate::desktop::signin::script().replace(crate::desktop::signin::OPEN_DIR, &open_dir.display().to_string());
        assert_eq!(written, wanted);

        // And the script it wrote really hands an address over.
        let address = "https://accounts.google.com/o/oauth2/auth?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A1";
        let ran = std::process::Command::new("sh").arg(&here).arg(address).status().expect("the opener runs");
        assert!(ran.success());
        assert_eq!(crate::desktop::signin::taken(&open_dir), [address]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
