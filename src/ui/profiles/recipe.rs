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
use crate::profile::guidance;
use crate::profile::{
    Addition, CLAUDE_MARKETPLACES, Desktop, GRAPHIFY_HOME, GRAPHIFY_PACKAGE, HarnessKind, OH_MY_OPENAGENT, Profile,
};

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
///
/// What QCode high adds is installed by [`addition_step`], in an order that matters: graphify's
/// system packages as root and nothing else as root; the configuration files before the Claude
/// Code plugins, because installing a plugin adds it to the settings file and copying the file
/// afterwards would forget every one of them.
#[must_use]
pub fn image(profile: &Profile) -> Recipe {
    let harness = profile.harness.record();
    // Less whatever the person switched off: a part they went without is not downloaded at all.
    let additions = profile.additions();
    // The base image of the system the profile chose: Debian's is `qcode/base`, the name every
    // profile image was built from before there was a choice.
    let mut lines = vec![format!("FROM {}", profile.os.image())];
    let desktop = profile.harness.desktop();
    if let Some(desktop) = desktop {
        // Root, because system packages and /opt are not the image user's to write. The image
        // goes back to its own user at the end, below.
        lines.push("USER root".to_owned());
        lines.extend(desktop_steps(desktop));
    }
    lines.extend(harness.image_steps());
    for (key, value) in harness.environment {
        lines.push(format!("ENV {key}=\"{value}\""));
    }
    if additions.contains(&Addition::Graphify) {
        // A window's image is root at this point already, and goes back at the end.
        if desktop.is_none() {
            lines.push("USER root".to_owned());
        }
        lines.push(addition_step(Addition::Graphify, profile));
        if desktop.is_none() {
            lines.push(format!("USER {USER}"));
        }
        lines.push(graphify_skill(profile.harness));
    }
    if additions.contains(&Addition::OhMyOpenAgent) {
        lines.push(addition_step(Addition::OhMyOpenAgent, profile));
        // Its own words after installing: "Anonymous telemetry is enabled by default. Disable it
        // with OMO_SEND_ANONYMOUS_TELEMETRY=0 or OMO_DISABLE_POSTHOG=1." QCode's templates turn
        // a maker's telemetry off wherever there is a switch for it.
        lines.push("ENV OMO_SEND_ANONYMOUS_TELEMETRY=\"0\"".to_owned());
        lines.push("ENV OMO_DISABLE_POSTHOG=\"1\"".to_owned());
    }
    let mut files = Vec::new();
    let written = profile.files();
    if !written.is_empty() {
        lines.push(format!("COPY template {STAGING}"));
    }
    for file in written {
        files.push((PathBuf::from("template").join(file.path), file.contents.to_owned()));
        lines.push(format!(
            "RUN mkdir -p \"$HOME/$(dirname '{path}')\" && cp '{STAGING}/{path}' \"$HOME/{path}\"",
            path = file.path
        ));
    }
    if additions.contains(&Addition::ClaudePlugins) {
        lines.push(addition_step(Addition::ClaudePlugins, profile));
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

/// The step that puts graphify's skill for `harness` into the image's home: what the agent turns
/// to when the person asks it to build the map (`/graphify .`).
///
/// It is written here rather than when a workspace's container comes up, because it lands in the
/// home directory and never depends on the workspace: measured, `graphify install --platform
/// <harness>` writes only under `$HOME` (for Claude Code also a line in `~/.claude/CLAUDE.md` that
/// names the skill), needs no network and writes the same bytes a second time. The one exception
/// is opencode's, which also leaves a plugin in the folder it runs in; it runs in a folder of its
/// own that is removed again, because opencode's plugin belongs to the workspace and is written
/// there when the container comes up. The step ends by asking for the skill, so a graphify that
/// moved it fails the build instead of leaving an image without it.
fn graphify_skill(harness: HarnessKind) -> String {
    format!(
        "RUN scratch=\"$(mktemp -d)\" \\\n && cd \"$scratch\" \\\n && graphify install --platform {platform} \\\n \
         && cd / && rm -rf \"$scratch\" \\\n && test -f \"$HOME/{skill}\"",
        platform = guidance::platform(harness),
        skill = guidance::skill(harness),
    )
}

/// The words a build step of QCode high starts its complaint with when it fails, so the screen
/// can tell that failure from any other and say what it means.
pub const HIGH_FAILED: &str = "QCode high:";

/// The step that installs `addition` for `profile`, checked before it counts: each one ends by
/// asking what it installed, so a step that seemed to work and left something out still fails the
/// build. Of the Claude Code plugins, only the ones `profile` has switched on are installed, and
/// only the marketplaces they come from are added.
///
/// Everything here is downloaded — from Debian and PyPI, npm, GitHub — and a build without the
/// network would otherwise stop on some package manager's own words, or, where a tool shrugs an
/// error off, not stop at all. So a failing step says, after whatever the tool said, what it was
/// installing and that the build needs the network; the image is then not made, and never made
/// with a part missing.
fn addition_step(addition: Addition, profile: &Profile) -> String {
    let (what, commands) = match addition {
        // Python and pipx from the system's own repositories, under its own names, then graphify
        // into the same place on every system.
        Addition::Graphify => (
            "graphify",
            vec![
                profile.os.install(profile.os.python()),
                profile.os.pipx(GRAPHIFY_HOME, GRAPHIFY_PACKAGE),
                "graphify --version".to_owned(),
            ],
        ),
        Addition::ClaudePlugins => {
            let plugins = profile.claude_plugins();
            let mut commands: Vec<String> = CLAUDE_MARKETPLACES
                .iter()
                .filter(|(_, name)| plugins.iter().any(|plugin| plugin.ends_with(&format!("@{name}"))))
                .map(|(repository, _)| format!("claude plugin marketplace add {repository}"))
                .collect();
            commands.extend(plugins.iter().map(|plugin| format!("claude plugin install {plugin}")));
            commands.push("claude plugin list > /tmp/qcode-plugins".to_owned());
            commands.extend(plugins.iter().map(|plugin| format!("grep -qF '{plugin}' /tmp/qcode-plugins")));
            commands.push("rm /tmp/qcode-plugins".to_owned());
            ("the Claude Code plugins", commands)
        }
        Addition::OhMyOpenAgent => (
            "oh-my-openagent",
            vec![
                // A cache of its own, removed in the same step: the image's shared npm cache would
                // otherwise keep another 214 MB of this one package's downloads in a layer.
                format!("npm install -g --cache /tmp/qcode-npm-cache {OH_MY_OPENAGENT}"),
                "rm -rf /tmp/qcode-npm-cache".to_owned(),
                format!("test -f /usr/local/npm/lib/node_modules/{OH_MY_OPENAGENT}/package.json"),
            ],
        ),
    };
    format!(
        "RUN {{ {steps}; }} \\\n || {{ echo '{HIGH_FAILED} could not install {what}. What QCode high adds is downloaded while the image is built, so the build needs the network.' >&2; exit 1; }}",
        steps = commands.join(" \\\n && "),
    )
}

/// The steps that put a desktop application into the image: the packages it needs, then the
/// archive from its maker's address.
///
/// The packages are Debian's, installed with apt: a window is offered on Debian only
/// ([`crate::base::Os::refuses`]), because that is the one system it was measured on.
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
        "RUN {write} \\\n && chmod 0755 '{program}' \\\n && mkdir -p '{folder}'",
        write = signin::written(&signin::script(), signin::OPEN_PROGRAM),
        program = signin::OPEN_PROGRAM,
        folder = signin::OPEN_DIR,
    );
    // The sign-in window's browser is linked in the step that unpacks the application, and only
    // there: in a step of its own, the layer would copy the 200 MB executable it links to.
    let browser: String =
        signin::browser_install(dir, desktop.program).iter().map(|command| format!(" \\\n && {command}")).collect();
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
             && test -x '{program}'{browser}",
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
    use crate::profile::{
        AccountKind, CLAUDE_PLUGINS, Extra, HarnessKind, MountAccess, NetworkMode, SafeName, Template,
    };

    use crate::base::Os;
    use crate::engine::names::BASE_IMAGE;

    fn profile(harness: HarnessKind, template: Template) -> Profile {
        Profile {
            name: SafeName::parse("claude-sub").expect("the name is safe"),
            harness,
            template,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        }
    }

    #[test]
    fn an_image_is_built_on_the_base_image_and_installs_the_harness() {
        let recipe = image(&profile(HarnessKind::ClaudeCode, Template::Base));
        let first = recipe.containerfile.lines().next().expect("the file has a line");
        assert_eq!(first, format!("FROM {BASE_IMAGE}"));
        assert!(recipe.containerfile.contains("npm install -g @anthropic-ai/claude-code"), "{recipe:?}");
    }

    #[test]
    fn a_profile_on_debian_is_built_exactly_as_before_there_was_a_choice() {
        // Every recipe of a Debian profile starts from the image it always started from, and its
        // graphify step is the very text it was: the choice of a system changes nothing for anyone
        // who does not make it.
        let file = image(&profile(HarnessKind::ClaudeCode, Template::High)).containerfile;
        assert!(file.starts_with("FROM qcode/base\n"), "{file}");
        assert!(
            file.contains(
                "RUN { apt-get update \\\n && apt-get install --yes --no-install-recommends python3 pipx \\\n \
                 && rm -rf /var/lib/apt/lists/* \\\n && PIPX_GLOBAL_HOME=/opt/pipx PIPX_GLOBAL_BIN_DIR=/usr/local/bin \
                 pipx install --global graphifyy \\\n && graphify --version; }"
            ),
            "{file}"
        );
    }

    #[test]
    fn a_profile_on_another_system_is_built_on_its_base_and_installs_with_its_package_manager() {
        let on =
            |os: Os, harness: HarnessKind| image(&Profile { os, ..profile(harness, Template::High) }).containerfile;
        let arch = on(Os::Arch, HarnessKind::ClaudeCode);
        assert!(arch.starts_with("FROM qcode/base-arch\n"), "{arch}");
        assert!(arch.contains("pacman -Syu --noconfirm --needed python python-pipx"), "{arch}");
        assert!(!arch.contains("apt-get"), "Arch has no apt: {arch}");
        let ubuntu = on(Os::Ubuntu, HarnessKind::Codex);
        assert!(ubuntu.starts_with("FROM qcode/base-ubuntu\n"), "{ubuntu}");
        assert!(ubuntu.contains("apt-get install --yes --no-install-recommends python3 pipx"), "{ubuntu}");
        assert!(ubuntu.contains("PIPX_HOME=/opt/pipx PIPX_BIN_DIR=/usr/local/bin pipx install graphifyy"), "{ubuntu}");
        let alpine = on(Os::Alpine, HarnessKind::OpenCode);
        assert!(alpine.starts_with("FROM qcode/base-alpine\n"), "{alpine}");
        assert!(alpine.contains("apk add --no-cache python3 pipx"), "{alpine}");
        for os in Os::ALL {
            let file = on(os, HarnessKind::ClaudeCode);
            assert_eq!(file.lines().filter(|line| line.starts_with("FROM ")).count(), 1, "{os:?}");
            assert!(
                file.contains("npm install -g @anthropic-ai/claude-code"),
                "{os:?}: the harness installs the same way"
            );
            assert!(file.contains("graphify --version"), "{os:?}");
        }
    }

    #[test]
    fn every_harness_install_leaves_no_npm_cache_in_its_layer() {
        // npm keeps every tarball it downloads in its cache; in the base image's shared cache
        // that was 323 MB of opencode's image. Removing it in a later step would not help — the
        // layer that wrote it keeps it — so the cache is a private one, named before the install
        // and removed after it, all in the one `RUN` step.
        for harness in HarnessKind::TERMINAL {
            for template in Template::ALL {
                let file = image(&profile(harness, template)).containerfile;
                for install in harness.record().install {
                    let step = steps(&file)
                        .into_iter()
                        .find(|step| step.contains(install))
                        .unwrap_or_else(|| panic!("{harness:?} {template:?}: `{install}` is not in {file}"));
                    let cache = step.find("export NPM_CONFIG_CACHE=/tmp/qcode-npm-cache ");
                    let installed = step.find(install).expect("found above");
                    let removed = step.rfind("&& rm -rf /tmp/qcode-npm-cache");
                    assert!(cache.is_some_and(|cache| cache < installed), "{harness:?}: no cache of its own: {step}");
                    assert!(removed.is_some_and(|removed| removed > installed), "{harness:?}: cache kept: {step}");
                    assert!(!step.contains("/var/cache/npm"), "{harness:?}: {step}");
                }
            }
        }
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
            for template in [Template::Base, Template::Recommended] {
                let file = image(&profile(harness, template)).containerfile;
                assert!(!file.contains("USER root"), "{harness:?} {template:?}: {file}");
                assert!(!file.contains("apt-get"), "{harness:?} {template:?}: {file}");
            }
        }
    }

    #[test]
    fn a_command_line_harness_image_never_ends_as_root() {
        // QCode high needs root for graphify's system packages, and for nothing after them: the
        // plugins and the configuration belong to the image's user, and a container must never
        // default to root.
        for harness in HarnessKind::ALL {
            for template in Template::ALL {
                let file = image(&profile(harness, template)).containerfile;
                let last_user = file.lines().rfind(|line| line.starts_with("USER "));
                if file.contains("USER root") {
                    assert_eq!(last_user, Some(format!("USER {USER}").as_str()), "{harness:?} {template:?}: {file}");
                }
                if harness.desktop().is_none() && template == Template::High {
                    let root = file.find("USER root").expect("graphify's packages need root");
                    let back = file[root..].find(&format!("USER {USER}")).expect("and it goes back") + root;
                    let root_part = &file[root..back];
                    assert!(root_part.contains("pipx install"), "{harness:?}: {root_part}");
                    assert_eq!(root_part.matches("\nRUN ").count(), 1, "one step as root and no more: {root_part}");
                }
            }
        }
    }

    /// The `RUN` steps of `file`, each with its continuation lines and nothing after them.
    fn steps(file: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut lines = file.lines();
        while let Some(line) = lines.next() {
            let Some(first) = line.strip_prefix("RUN ") else { continue };
            let mut step = vec![first];
            let mut last = first;
            while last.trim_end().ends_with('\\') {
                let Some(next) = lines.next() else { break };
                step.push(next);
                last = next;
            }
            found.push(step.join("\n"));
        }
        found
    }

    #[test]
    fn qcode_high_installs_graphify_for_every_harness_outside_the_home() {
        for harness in HarnessKind::ALL {
            let file = image(&profile(harness, Template::High)).containerfile;
            let step = steps(&file).into_iter().find(|step| step.contains("pipx")).expect("graphify is installed");
            assert!(step.contains("apt-get install --yes --no-install-recommends python3 pipx"), "{step}");
            assert!(step.contains("pipx install --global graphifyy"), "{step}");
            assert!(step.contains("PIPX_GLOBAL_HOME=/opt/pipx"), "not under the home: {step}");
            assert!(step.contains("PIPX_GLOBAL_BIN_DIR=/usr/local/bin"), "on the path: {step}");
            assert!(step.contains("graphify --version"), "the step checks what it installed: {step}");
            assert!(!step.contains("$HOME"), "{step}");
            for other in [Template::Base, Template::Recommended] {
                let file = image(&profile(harness, other)).containerfile;
                assert!(!file.contains("graphify"), "{harness:?} {other:?}: {file}");
            }
        }
    }

    #[test]
    fn qcode_high_puts_graphifys_skill_for_its_own_harness_into_the_home() {
        for harness in HarnessKind::ALL {
            let file = image(&profile(harness, Template::High)).containerfile;
            let all = steps(&file);
            let graphify = all.iter().position(|step| step.contains("pipx install")).expect("graphify is installed");
            let skill = all
                .iter()
                .position(|step| step.contains("graphify install --platform"))
                .unwrap_or_else(|| panic!("{harness:?}: no skill: {file}"));
            assert!(graphify < skill, "the skill comes after graphify: {file}");
            let step = &all[skill];
            let word = guidance::platform(harness);
            assert!(step.contains(&format!("graphify install --platform {word} ")), "{step}");
            assert!(step.contains(&format!("test -f \"$HOME/{}\"", guidance::skill(harness))), "{step}");
            assert!(step.contains("rm -rf \"$scratch\""), "the folder it ran in is removed: {step}");
            if harness.desktop().is_none() {
                let back = file.find(&format!("USER {USER}")).expect("back to the user");
                let at = file.find("graphify install --platform").expect("the skill");
                assert!(back < at, "written as the image's user, not root: {file}");
            }
            for other in [Template::Base, Template::Recommended] {
                let file = image(&profile(harness, other)).containerfile;
                assert!(!file.contains("--platform"), "{harness:?} {other:?}: {file}");
            }
        }
    }

    #[test]
    fn the_plugins_go_into_claude_codes_image_and_no_other() {
        for harness in HarnessKind::ALL {
            let file = image(&profile(harness, Template::High)).containerfile;
            assert_eq!(file.contains("claude plugin"), harness == HarnessKind::ClaudeCode, "{harness:?}: {file}");
            assert_eq!(file.contains("oh-my-openagent"), harness == HarnessKind::OpenCode, "{harness:?}: {file}");
        }
        let file = image(&profile(HarnessKind::ClaudeCode, Template::High)).containerfile;
        for (repository, _) in CLAUDE_MARKETPLACES {
            assert!(file.contains(&format!("claude plugin marketplace add {repository}")), "{file}");
        }
        for plugin in CLAUDE_PLUGINS {
            assert!(file.contains(&format!("claude plugin install {plugin}")), "{plugin}: {file}");
            assert!(file.contains(&format!("grep -qF '{plugin}'")), "{plugin} is checked after: {file}");
        }
        // Installing a plugin writes it into the settings file; the file copied in after would
        // undo every one of them.
        let settings = file.find("\"$HOME/.claude/settings.json\"").expect("the settings are written");
        let plugins = file.find("claude plugin install").expect("the plugins are installed");
        assert!(settings < plugins, "{file}");
    }

    #[test]
    fn a_part_switched_off_is_not_installed_and_the_rest_still_are() {
        let without = |without: Vec<Extra>| Profile { without, ..profile(HarnessKind::ClaudeCode, Template::High) };
        let file = image(&without(vec![Extra::Plugin("context7@claude-plugins-official")])).containerfile;
        assert!(!file.contains("context7"), "{file}");
        for plugin in CLAUDE_PLUGINS.iter().filter(|plugin| !plugin.starts_with("context7@")) {
            assert!(file.contains(&format!("claude plugin install {plugin}")), "{plugin}: {file}");
        }
        assert!(file.contains("pipx install --global graphifyy"), "{file}");

        // The only plugin of a marketplace switched off, and that marketplace is not added either.
        let file = image(&without(vec![Extra::Plugin("block-no-verify@claude-code-workflows")])).containerfile;
        assert!(!file.contains("wshobson/agents"), "{file}");
        assert!(file.contains("anthropics/claude-plugins-official"), "{file}");

        let file = image(&without(vec![Extra::Graphify])).containerfile;
        assert!(!file.contains("graphify") && !file.contains("pipx"), "{file}");
        assert!(file.contains("claude plugin install superpowers@claude-plugins-official"), "{file}");

        // Everything off is QCode basic's image.
        let file = image(&without(Template::High.extras(HarnessKind::ClaudeCode))).containerfile;
        assert_eq!(file, image(&profile(HarnessKind::ClaudeCode, Template::Recommended)).containerfile);
    }

    #[test]
    fn opencode_without_oh_my_openagent_is_not_told_of_it() {
        let recipe =
            image(&Profile { without: vec![Extra::OhMyOpenAgent], ..profile(HarnessKind::OpenCode, Template::High) });
        assert!(!recipe.containerfile.contains("oh-my-openagent"), "{recipe:?}");
        assert!(!recipe.files.iter().any(|(_, contents)| contents.contains("oh-my-openagent")), "{recipe:?}");
        assert!(recipe.containerfile.contains("graphify"), "{recipe:?}");
    }

    #[test]
    fn opencode_gets_oh_my_openagent_and_is_told_where_it_is() {
        let recipe = image(&profile(HarnessKind::OpenCode, Template::High));
        let file = &recipe.containerfile;
        assert!(file.contains("npm install -g --cache /tmp/qcode-npm-cache oh-my-openagent"), "{file}");
        assert!(file.contains("ENV OMO_SEND_ANONYMOUS_TELEMETRY=\"0\""), "{file}");
        let (_, settings) = recipe
            .files
            .iter()
            .find(|(path, _)| path == &PathBuf::from("template/.config/opencode/opencode.json"))
            .expect("opencode's settings are written");
        assert!(settings.contains("file:///usr/local/npm/lib/node_modules/oh-my-openagent"), "{settings}");
        let basic = image(&profile(HarnessKind::OpenCode, Template::Recommended));
        assert!(!basic.files.iter().any(|(_, contents)| contents.contains("oh-my-openagent")), "{basic:?}");
    }

    #[test]
    fn claude_codes_first_start_is_answered_under_both_qcode_templates() {
        for template in [Template::Recommended, Template::High] {
            let recipe = image(&profile(HarnessKind::ClaudeCode, template));
            let paths: Vec<&PathBuf> = recipe.files.iter().map(|(path, _)| path).collect();
            assert!(paths.contains(&&PathBuf::from("template/.claude.json")), "{template:?}: {paths:?}");
            assert!(recipe.containerfile.contains("\"$HOME/.claude.json\""), "{template:?}: {recipe:?}");
            assert_eq!(recipe.containerfile.matches("COPY template").count(), 1, "one staging copy: {recipe:?}");
        }
    }

    #[test]
    fn every_step_of_qcode_high_says_the_build_needs_the_network_when_it_fails() {
        for harness in HarnessKind::ALL {
            let file = image(&profile(harness, Template::High)).containerfile;
            let high: Vec<String> = steps(&file)
                .into_iter()
                .filter(|step| ["pipx", "claude plugin", "oh-my-openagent"].iter().any(|word| step.contains(word)))
                .collect();
            assert_eq!(high.len(), Template::High.additions(harness).len(), "{harness:?}: {file}");
            for step in high {
                assert!(step.starts_with("{ "), "the whole step is one group: {step}");
                assert!(step.contains(&format!("|| {{ echo '{HIGH_FAILED} ")), "{step}");
                assert!(step.contains("the build needs the network"), "{step}");
                assert!(step.trim_end().ends_with("exit 1; }"), "the build stops: {step}");
            }
        }
    }

    #[test]
    fn a_failing_step_of_qcode_high_stops_and_says_why_in_a_real_shell() {
        // The same step, with its first command made to fail the way it fails without the network,
        // run by a shell: it must fail, and say what the screen looks for.
        let file = image(&profile(HarnessKind::OpenCode, Template::High)).containerfile;
        let step = steps(&file)
            .into_iter()
            .find(|step| step.contains("npm install -g --cache /tmp/qcode-npm-cache oh-my-openagent"))
            .expect("step");
        let script = step.replace("npm install -g --cache /tmp/qcode-npm-cache oh-my-openagent", "false");
        let ran = std::process::Command::new("sh").arg("-c").arg(&script).output().expect("a shell runs the step");
        assert!(!ran.status.success(), "{script}");
        let said = String::from_utf8_lossy(&ran.stderr);
        assert!(said.contains(HIGH_FAILED) && said.contains("needs the network"), "{said}");
        // And a step whose commands all succeed says nothing and succeeds.
        let fine = step
            .replace("npm install -g --cache /tmp/qcode-npm-cache oh-my-openagent", "true")
            .replace("test -f", "test -n");
        let ran = std::process::Command::new("sh").arg("-c").arg(&fine).output().expect("a shell runs the step");
        assert!(ran.status.success(), "{fine}: {}", String::from_utf8_lossy(&ran.stderr));
    }

    #[test]
    fn a_window_gets_its_settings_into_the_home_the_same_way_every_harness_does() {
        // The file lands in the image's home directory, which is copied into the workspace's home
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
        // The person signs in inside the window, where the application stores it in the workspace's
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
    fn the_sign_in_window_is_linked_in_the_very_step_that_unpacks_the_application() {
        let recipe = image(&profile(HarnessKind::AntigravityIde, Template::Recommended));
        let unpack = recipe
            .containerfile
            .split("\nRUN ")
            .find(|step| step.contains("tar --extract"))
            .expect("the image unpacks the application")
            .to_owned();
        // In a step of its own the layer would copy the 200 MB executable instead of linking it.
        assert!(unpack.contains("cp -al"), "{unpack}");
        assert!(unpack.contains(crate::desktop::signin::BROWSER_PROGRAM), "{unpack}");
        assert!(unpack.find("tar --extract") < unpack.find("cp -al"), "linked after it is unpacked: {unpack}");
        assert!(unpack.contains(&format!("&& test -x '{}'", crate::desktop::signin::BROWSER_PROGRAM)), "{unpack}");
        let steps = recipe.containerfile.matches(crate::desktop::signin::BROWSER_DIR).count();
        assert_eq!(steps, unpack.matches(crate::desktop::signin::BROWSER_DIR).count(), "no other step touches it");
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
