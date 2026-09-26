//! What a profile's image is built from, and the shell the login step runs inside a container.
//!
//! Both are text made from the harness record and the template, and nothing here runs anything:
//! a recipe is checked in tests on a machine with no container engine at all.
//!
//! The recipe belongs to the profile layer rather than to a screen; it lives here until that
//! layer grows an image description of its own.

use std::path::PathBuf;

use crate::base::paths::{HOME_DIR, OPEN_HOME, USER};
use crate::desktop::signin;
use crate::profile::guidance;
use crate::profile::{
    AccountKind, Addition, CARGO_HOME, CLAUDE_MARKETPLACES, Desktop, GRAPHIFY_HOME, GRAPHIFY_PACKAGE, HarnessKind,
    OH_MY_OPENAGENT, OH_MY_OPENCODE_SLIM, Profile, RUSTUP_HOME, Template,
};

/// Where the build context puts the files a template writes, so the `COPY` never has to know
/// the home directory of the image.
const STAGING: &str = "/qcode-template";

/// Where the login container can see a directory of the host, which is how a fresh login leaves
/// the container without anyone having to know where the home directory is.
pub const CAPTURE_DIR: &str = "/qcode-capture";

/// Where, under the home, the person's own installs go: the prefix `pipx` and `uv tool` use by
/// themselves, and the one `npm -g` is given.
pub const USER_PREFIX: &str = ".local";

/// The npm prefix the base image installs the harnesses into.
const IMAGE_NPM: &str = "/usr/local/npm";

/// A build context: the file that describes the image and everything the build copies in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// The Containerfile, ready to be written beside the files below.
    pub containerfile: String,
    /// Files the build copies in, by their path inside the context directory.
    pub files: Vec<(PathBuf, String)>,
}

/// The label a profile's image carries the [`Recipe::revision`] it was built from in.
pub const REVISION_LABEL: &str = "qcode.profile.revision";

impl Recipe {
    /// Which recipe this is: a digest of everything the build reads, the Containerfile and every
    /// file beside it with its path. It answers "was the image built from this text?", the way
    /// the base image's revision does, so an image an earlier QCode built can be told apart
    /// without anybody remembering to raise a number.
    #[must_use]
    pub fn revision(&self) -> String {
        let mut spelled = self.containerfile.clone();
        for (path, contents) in &self.files {
            spelled.push('\0');
            spelled.push_str(&path.to_string_lossy());
            spelled.push('\0');
            spelled.push_str(contents);
        }
        format!("{:016x}", crate::base::digest(&spelled))
    }

    /// The Containerfile as the engine is given it: the recipe's own, and last the label that
    /// says which recipe it was. Last, so that no step before it reads differently and every
    /// layer an unchanged recipe made before is still reused.
    #[must_use]
    pub fn labelled(&self) -> String {
        format!("{}LABEL {REVISION_LABEL}=\"{}\"\n", self.containerfile, self.revision())
    }
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
/// What a QCode template adds is installed by [`addition_step`], in an order that matters: graphify's
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
    let mut files = Vec::new();
    if let Some(desktop) = desktop {
        // Built into the application rather than installed into the home, so every workspace's
        // window has it whatever its home volume holds, and an update of the image brings the
        // new one.
        let staged = PathBuf::from("desktop").join(INBOX_EXTENSION);
        for (name, contents) in INBOX_FILES {
            files.push((staged.join(name), contents.to_owned()));
        }
        lines.push(format!(
            "COPY {staged} {dir}/resources/app/extensions/{INBOX_EXTENSION}",
            staged = staged.display(),
            dir = desktop.install_dir
        ));
    }
    for (key, value) in harness.environment {
        lines.push(format!("ENV {key}=\"{value}\""));
    }
    for (key, value) in profile.template.environment(profile.harness) {
        lines.push(format!("ENV {key}=\"{value}\""));
    }
    // Copied as root whoever the image's user is, and left readable by the person the container
    // runs as. Only a profile that signs in with a key is kept to one: a profile made with a
    // sign-in that still works for somebody must go on offering it.
    if profile.account == AccountKind::ApiKey
        && let Some(file) = harness.key_only
    {
        let staged = PathBuf::from("system").join(file.path.trim_start_matches('/'));
        lines.push(format!("COPY {} {}", staged.display(), file.path));
        files.push((staged, file.contents.to_owned()));
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
    // Quvyta development's system packages, toolchain and browser, as root like graphify's
    // packages, and nothing after them as root.
    let rust = additions.contains(&Addition::Rust);
    let chromium = additions.contains(&Addition::Chromium);
    if rust || chromium {
        if desktop.is_none() {
            lines.push("USER root".to_owned());
        }
        if rust {
            lines.push(addition_step(Addition::Rust, profile));
        }
        if chromium {
            lines.push(addition_step(Addition::Chromium, profile));
        }
        if desktop.is_none() {
            lines.push(format!("USER {USER}"));
        }
    }
    if rust {
        // Every shell of the container finds the toolchain, whoever it runs as. Builds start from
        // scratch, as the ecosystem builds everywhere: incremental builds keep a copy of every
        // crate's work in `target/`, which on a small machine is the difference that matters.
        lines.push(format!("ENV RUSTUP_HOME=\"{RUSTUP_HOME}\""));
        lines.push(format!("ENV PATH=\"{CARGO_HOME}/bin:$PATH\""));
        lines.push("ENV CARGO_INCREMENTAL=\"0\"".to_owned());
    }
    if additions.contains(&Addition::OhMyOpenCodeSlim) {
        // It sends no telemetry and needs no key (read in its 2.2.25 package); its own settings
        // file, written with the others, turns its self-update off.
        lines.push(addition_step(Addition::OhMyOpenCodeSlim, profile));
    }
    if additions.contains(&Addition::OhMyOpenAgent) {
        lines.push(addition_step(Addition::OhMyOpenAgent, profile));
        // Its own words after installing: "Anonymous telemetry is enabled by default. Disable it
        // with OMO_SEND_ANONYMOUS_TELEMETRY=0 or OMO_DISABLE_POSTHOG=1." QCode's templates turn
        // a maker's telemetry off wherever there is a switch for it.
        lines.push("ENV OMO_SEND_ANONYMOUS_TELEMETRY=\"0\"".to_owned());
        lines.push("ENV OMO_DISABLE_POSTHOG=\"1\"".to_owned());
    }
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
    // After the template's files, because the step merges graphify's hooks into the settings they
    // wrote. QCode extra has graphify's section written into the workspace instead, when a
    // container comes up, beside QCode's own.
    if additions.contains(&Addition::Graphify) && !profile.template.carries_high() {
        lines.push(graphify_guidance(profile.harness));
    }
    if additions.contains(&Addition::ClaudePlugins) {
        lines.push(addition_step(Addition::ClaudePlugins, profile));
    }
    // Only a mark, read by the courier that gives a workspace's home its login: the approvals are
    // written into the application's database there, beside the login (`desktop::login`).
    if desktop.is_some() && profile.template != Template::Base {
        lines.push(format!(
            "RUN mkdir -p \"$(dirname '{marker}')\" && echo agent-driven > '{marker}' && chmod 0644 '{marker}'",
            marker = crate::desktop::login::APPROVALS
        ));
    }
    // Last, after every step that installs with npm, so those still land in the image's own
    // prefix. From here on whatever the person installs for their user — `npm -g`, `pipx`,
    // `uv tool` — goes into the home, which is the workspace's own volume: it stays when the
    // container is made again for a changed plan, a rebuild or a new QCode, where the container's
    // own layer does not. The home's programs come first, the image's harness right behind.
    lines.push(format!("ENV NPM_CONFIG_PREFIX=\"{HOME_DIR}/{USER_PREFIX}\""));
    lines.push(format!("ENV PATH=\"{HOME_DIR}/{USER_PREFIX}/bin:{IMAGE_NPM}/bin:$PATH\""));
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

/// The step that tells the harness of graphify from the home directory, for every folder it
/// works in, under QCode recommended: its section, and its hooks or plugin where the harness has them.
///
/// graphify has no installer for a home directory; its workspace installer writes into the folder
/// it runs in. Run in the home, it merges its hooks into the harness's own settings there, and its
/// section lands in a file the harness does not read from the home; that file is moved to the one
/// it does read ([`guidance::user_file`]), after whatever is already in it, and what else it left
/// goes where [`guidance::graphify_leftovers`] says. Nothing of it is written into the person's
/// workspace: graphify's map is, when a container comes up, and kept out of git there. The step
/// ends by asking for graphify's heading in the file, so a graphify that wrote elsewhere fails the
/// build.
fn graphify_guidance(harness: HarnessKind) -> String {
    let written = guidance::graphify_file(harness);
    let user = guidance::user_file(harness);
    let mut commands = vec![
        "cd \"$HOME\"".to_owned(),
        format!("graphify {} install", guidance::platform(harness)),
        format!("mkdir -p \"$HOME/$(dirname '{user}')\""),
        format!("{{ if [ -s \"$HOME/{user}\" ]; then echo; fi; cat '{written}'; }} >> \"$HOME/{user}\""),
        format!("rm '{written}'"),
    ];
    for (from, to) in guidance::graphify_leftovers(harness) {
        if to.is_empty() {
            commands.push(format!("rm -rf '{from}'"));
        } else {
            commands.push(format!("mkdir -p \"$(dirname '{to}')\" && mv '{from}' '{to}'"));
        }
    }
    commands.push(format!("grep -qF '## graphify' \"$HOME/{user}\""));
    format!("RUN {}", commands.join(" \\\n && "))
}

/// The words a build step of QCode extra starts its complaint with when it fails, so the screen
/// can tell that failure from any other and say what it means.
pub const EXTRA_FAILED: &str = "QCode extra:";

/// The same for QCode recommended.
pub const RECOMMENDED_FAILED: &str = "QCode recommended:";

/// The same for oh my opencode slim.
pub const SLIM_FAILED: &str = "oh my opencode slim:";

/// The same for Quvyta development, whose steps are QCode extra's and its own.
pub const DEV_FAILED: &str = "Quvyta development:";

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
        // rustup's own installer, which picks the build for the machine the image is built on:
        // the same recipe makes an image for a laptop and for a Raspberry Pi. It is told where to
        // put everything and not to touch any shell's startup file; the image's `ENV` says where
        // it is instead. The toolchain is opened to everyone in the same step, so no second layer
        // holds a copy of it. uv comes from its maker's installer the same way, straight into
        // `/usr/local/bin`.
        Addition::Rust => (
            "Rust and the build tools",
            vec![
                profile.os.install(profile.os.build_tools()),
                format!(
                    "curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location https://sh.rustup.rs \\\n \
                     | RUSTUP_HOME={RUSTUP_HOME} CARGO_HOME={CARGO_HOME} sh -s -- -y --no-modify-path --profile minimal \
                     --default-toolchain stable --component clippy --component rustfmt"
                ),
                format!("chmod -R a+rwX {RUSTUP_HOME} {CARGO_HOME}"),
                format!(
                    "RUSTUP_HOME={RUSTUP_HOME} {CARGO_HOME}/bin/cargo --version \\\n \
                     && RUSTUP_HOME={RUSTUP_HOME} {CARGO_HOME}/bin/cargo clippy --version \\\n \
                     && RUSTUP_HOME={RUSTUP_HOME} {CARGO_HOME}/bin/cargo fmt --version"
                ),
                "curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location https://astral.sh/uv/install.sh \\\n \
                 | env UV_INSTALL_DIR=/usr/local/bin UV_NO_MODIFY_PATH=1 sh"
                    .to_owned(),
                "uv --version".to_owned(),
            ],
        ),
        Addition::Chromium => (
            "Chromium",
            vec![profile.os.install(profile.os.chromium().unwrap_or_default()), "chromium --version".to_owned()],
        ),
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
        Addition::OhMyOpenCodeSlim => (
            "oh-my-opencode-slim",
            vec![
                // A cache of its own, removed in the same step: the image's shared npm cache would
                // otherwise keep this one package's downloads in a layer.
                format!("npm install -g --cache /tmp/qcode-npm-cache {OH_MY_OPENCODE_SLIM}"),
                "rm -rf /tmp/qcode-npm-cache".to_owned(),
                format!("test -f /usr/local/npm/lib/node_modules/{OH_MY_OPENCODE_SLIM}/package.json"),
            ],
        ),
    };
    let (failed, template) = match profile.template {
        Template::Slim => (SLIM_FAILED, "oh my opencode slim"),
        Template::QuvytaDev => (DEV_FAILED, "Quvyta development"),
        Template::High => (EXTRA_FAILED, "QCode extra"),
        Template::Base | Template::Recommended => (RECOMMENDED_FAILED, "QCode recommended"),
    };
    format!(
        "RUN {{ {steps}; }} \\\n || {{ echo '{failed} could not install {what}. What {template} adds is downloaded while the image is built, so the build needs the network.' >&2; exit 1; }}",
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
/// Which archive is fetched is decided inside the build, by the processor `uname -m` names there,
/// because that is the processor the image will run on, whatever QCode itself was compiled for. A
/// processor the record has no archive for stops the build with a line saying so, before anything
/// is downloaded.
///
/// The unpacking is done as root on purpose: the archive carries a set-user-id helper the
/// application's own sandbox uses when it cannot open an unprivileged user namespace, and only
/// root can restore that bit. Nothing else in the image needs root.
fn desktop_steps(desktop: &Desktop) -> Vec<String> {
    let packages = desktop.packages.join(" ");
    let dir = desktop.install_dir;
    // The program the application runs to open a web address, and the folder it writes into.
    let opener = signin::opener_step();
    // The sign-in window's browser is linked in the step that unpacks the application, and only
    // there: in a step of its own, the layer would copy the 200 MB executable it links to.
    let browser: String =
        signin::browser_install(dir, desktop.program).iter().map(|command| format!(" \\\n && {command}")).collect();
    let known: Vec<&str> = desktop.archives.iter().map(|archive| archive.machine).collect();
    let choices: String = desktop
        .archives
        .iter()
        .map(|archive| {
            format!(
                " \\\n    {machine}) address='{address}'; sum='{sha}' ;;",
                machine = archive.machine,
                address = archive.address,
                sha = archive.sha256
            )
        })
        .collect();
    vec![
        opener,
        format!(
            "RUN apt-get update \\\n && apt-get install --yes --no-install-recommends {packages} \\\n \
             && rm -rf /var/lib/apt/lists/*"
        ),
        format!(
            "RUN case \"$(uname -m)\" in{choices} \\\n    \
             *) echo \"{name} {version} has no archive for $(uname -m); QCode knows {known}.\" >&2; exit 1 ;; \\\n \
             esac \\\n \
             && curl --fail --silent --show-error --location --output /tmp/desktop.tar.gz \"$address\" \\\n \
             && echo \"$sum  /tmp/desktop.tar.gz\" | sha256sum --check --strict - \\\n \
             && mkdir -p '{dir}' \\\n \
             && tar --extract --gzip --file /tmp/desktop.tar.gz --directory '{dir}' --strip-components=1 \\\n \
             && rm /tmp/desktop.tar.gz \\\n \
             && test -x '{program}'{browser}",
            name = desktop.program,
            version = desktop.version,
            known = known.join(" and "),
            program = desktop.command(),
        ),
    ]
}

/// The folder of the extension a window's image carries so that its agent is told when another
/// tab sent it a message: a window has no prompt QCode could type the message into, and the
/// extension, from inside the application, can send the agent panel one.
pub const INBOX_EXTENSION: &str = "qcode-inbox";

/// The extension's files, by name.
const INBOX_FILES: [(&str, &str); 2] = [
    ("package.json", include_str!("../../../assets/desktop/qcode-inbox/package.json")),
    ("extension.js", include_str!("../../../assets/desktop/qcode-inbox/extension.js")),
];

/// The shell that takes a fresh login out of the container it was made in.
///
/// It checks before it copies: the first file of the harness record is the login itself, and
/// without it the script fails, so a login that never happened can never be mistaken for one
/// that did. The files beside it, which some harnesses write and some do not, are copied when
/// they are there.
///
/// A window's login is not a file of its own but two rows of the application's database, so its
/// script takes those rows out instead, and fails the same way when they are not both there
/// ([`crate::desktop::login::capture_script`]).
#[must_use]
pub fn capture_script(profile: &Profile) -> String {
    if profile.harness.desktop().is_some() {
        return crate::desktop::login::capture_script();
    }
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
        AccountKind, CLAUDE_PLUGINS, CLAUDE_STARTER_PLUGINS, Extra, HarnessKind, MountAccess, NetworkMode, SafeName,
        Template,
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
    fn a_gemini_cli_profile_with_a_key_is_kept_to_the_key_whatever_its_template() {
        for template in Template::ALL {
            let keyed = Profile { account: AccountKind::ApiKey, ..profile(HarnessKind::GeminiCli, template) };
            let recipe = image(&keyed);
            assert!(
                recipe
                    .containerfile
                    .contains("\nCOPY system/etc/gemini-cli/settings.json /etc/gemini-cli/settings.json\n"),
                "{template:?}: {}",
                recipe.containerfile
            );
            let (_, contents) = recipe
                .files
                .iter()
                .find(|(path, _)| path == &PathBuf::from("system/etc/gemini-cli/settings.json"))
                .expect("the file is in the context");
            let settings: serde_json::Value = serde_json::from_str(contents).expect("the file is JSON");
            assert_eq!(settings["security"]["auth"]["enforcedType"], "gemini-api-key", "{contents}");
            assert!(
                settings["security"]["auth"].get("selectedType").is_none(),
                "the dialog must still open: {contents}"
            );
        }
    }

    #[test]
    fn a_profile_made_with_a_sign_in_or_of_another_harness_is_not_kept_to_a_key() {
        let signed_in = image(&profile(HarnessKind::GeminiCli, Template::Recommended));
        assert!(!signed_in.containerfile.contains("/etc/gemini-cli"), "{}", signed_in.containerfile);
        for harness in [HarnessKind::ClaudeCode, HarnessKind::Codex, HarnessKind::OpenCode] {
            let keyed = Profile { account: AccountKind::ApiKey, ..profile(harness, Template::Recommended) };
            assert!(!image(&keyed).containerfile.contains("COPY system/"), "{harness:?}");
        }
    }

    #[test]
    fn a_recipes_revision_changes_with_its_file_or_any_file_beside_it_and_ends_the_labelled_file() {
        let high = image(&profile(HarnessKind::ClaudeCode, Template::High));
        assert_eq!(high.revision(), image(&profile(HarnessKind::ClaudeCode, Template::High)).revision());
        assert_ne!(high.revision(), image(&profile(HarnessKind::ClaudeCode, Template::Base)).revision());
        let mut step = high.clone();
        step.containerfile.push_str("RUN true\n");
        assert_ne!(step.revision(), high.revision(), "a changed step is another recipe");
        let mut file = high.clone();
        let (_, contents) = file.files.first_mut().expect("QCode extra copies files in");
        contents.push(' ');
        assert_ne!(file.revision(), high.revision(), "a changed file beside it is another recipe");
        let labelled = high.labelled();
        assert!(labelled.starts_with(&high.containerfile), "every step before the label reads as it did");
        assert_eq!(
            labelled.lines().last(),
            Some(format!("LABEL qcode.profile.revision=\"{}\"", high.revision()).as_str())
        );
    }

    /// The Containerfile of a Claude Code profile under Quvyta development on `os`.
    fn quvyta_dev(os: Os) -> String {
        image(&Profile { os, ..profile(HarnessKind::ClaudeCode, Template::QuvytaDev) }).containerfile
    }

    #[test]
    fn quvyta_development_builds_rust_and_its_tools_into_the_image_for_the_machine_it_is_built_on() {
        for os in Os::ALL {
            let file = quvyta_dev(os);
            assert!(file.contains(&os.install(os.build_tools())), "{os:?}: the build tools: {file}");
            assert!(file.contains("https://sh.rustup.rs"), "{os:?}: {file}");
            assert!(
                file.contains(
                    "| RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo sh -s -- -y --no-modify-path --profile minimal \
                     --default-toolchain stable --component clippy --component rustfmt"
                ),
                "{os:?}: {file}"
            );
            // rustup picks the build for the machine; the recipe names none.
            for machine in ["x86_64", "aarch64", "amd64", "arm64", "unknown-linux", "--default-host"] {
                assert!(!file.contains(machine), "{os:?}: `{machine}` in {file}");
            }
            assert!(file.contains("chmod -R a+rwX /opt/rustup /opt/cargo"), "{os:?}: anyone may use it: {file}");
            for check in ["/opt/cargo/bin/cargo --version", "cargo clippy --version", "cargo fmt --version"] {
                assert!(file.contains(check), "{os:?}: `{check}` in {file}");
            }
            assert!(file.contains("https://astral.sh/uv/install.sh"), "{os:?}: {file}");
            assert!(file.contains("| env UV_INSTALL_DIR=/usr/local/bin UV_NO_MODIFY_PATH=1 sh"), "{os:?}: {file}");
            assert!(file.contains("\\\n && uv --version"), "{os:?}: {file}");
            for variable in [
                "\nENV RUSTUP_HOME=\"/opt/rustup\"\n",
                "\nENV PATH=\"/opt/cargo/bin:$PATH\"\n",
                "\nENV CARGO_INCREMENTAL=\"0\"\n",
            ] {
                assert!(file.contains(variable), "{os:?}: `{variable}` in {file}");
            }
            // Installed as root, and the image is its user's again afterwards.
            let rustup = file.find("sh.rustup.rs").expect("rustup's step");
            assert_eq!(file[..rustup].rfind("\nUSER "), file[..rustup].rfind("\nUSER root"), "{os:?}: {file}");
            assert!(file[rustup..].contains(&format!("\nUSER {USER}\n")), "{os:?}: {file}");
            // Everything QCode extra gives is there too.
            assert!(file.contains("graphify --version"), "{os:?}: {file}");
            for plugin in crate::profile::CLAUDE_PLUGINS {
                assert!(file.contains(&format!("claude plugin install {plugin}")), "{os:?}: {plugin}");
            }
        }
    }

    #[test]
    fn quvyta_development_installs_chromium_from_each_systems_packages_unless_switched_off() {
        for os in [Os::Debian, Os::Arch, Os::Alpine] {
            let file = quvyta_dev(os);
            assert!(file.contains(&os.install(&["chromium"])), "{os:?}: {file}");
            assert!(file.contains("\\\n && chromium --version"), "{os:?}: {file}");
        }
        assert!(!quvyta_dev(Os::Ubuntu).contains("chromium"), "Ubuntu has none that runs in a container");
        let without =
            Profile { without: vec![Extra::Chromium], ..profile(HarnessKind::ClaudeCode, Template::QuvytaDev) };
        let file = image(&without).containerfile;
        assert!(!file.contains("chromium"), "switched off, it is not downloaded at all: {file}");
        assert!(file.contains("https://sh.rustup.rs"), "and the rest stays: {file}");
    }

    #[test]
    fn no_other_template_gets_rust_or_chromium() {
        for template in [Template::Base, Template::Recommended, Template::High] {
            for harness in HarnessKind::ALL {
                let file = image(&profile(harness, template)).containerfile;
                for word in ["rustup", "chromium", "CARGO_INCREMENTAL", "astral.sh"] {
                    assert!(!file.contains(word), "{template:?} {harness:?}: `{word}` in {file}");
                }
            }
        }
    }

    #[test]
    fn a_failing_step_of_quvyta_development_says_it_is_its_own() {
        let file = quvyta_dev(Os::Debian);
        assert!(
            file.contains(
                "Quvyta development: could not install Rust and the build tools. What Quvyta development adds is \
                 downloaded while the image is built, so the build needs the network."
            ),
            "{file}"
        );
        assert!(file.contains("Quvyta development: could not install Chromium."), "{file}");
        assert!(file.contains("Quvyta development: could not install graphify."), "QCode extra's steps too: {file}");
        assert!(!file.contains(EXTRA_FAILED), "{file}");
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
    fn what_the_person_installs_for_their_user_goes_into_the_home_after_every_install_of_the_image() {
        for harness in HarnessKind::ALL {
            for template in Template::ALL {
                let file = image(&profile(harness, template)).containerfile;
                let prefix = file.find("ENV NPM_CONFIG_PREFIX=\"/home/qcode/.local\"");
                let prefix =
                    prefix.unwrap_or_else(|| panic!("{harness:?} {template:?}: no prefix in the home:\n{file}"));
                // Every npm step of the image still installs into the image's own prefix.
                let last_npm = file.rfind("npm ").unwrap_or(0);
                assert!(last_npm < prefix, "{harness:?} {template:?}: an npm step comes after the prefix:\n{file}");
                assert!(
                    file.contains("ENV PATH=\"/home/qcode/.local/bin:/usr/local/npm/bin:$PATH\""),
                    "{harness:?} {template:?}: the home's programs are not found first:\n{file}"
                );
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
        assert!(recipe.containerfile.contains("&& curl --fail "), "{recipe:?}");
        for archive in desktop.archives {
            assert!(recipe.containerfile.contains(archive.address), "the archive comes from its maker");
            assert!(recipe.containerfile.contains(archive.sha256), "nothing is unpacked unchecked");
        }
        assert!(recipe.containerfile.contains("sha256sum --check --strict"), "{recipe:?}");
        // The digest is checked before the archive is opened, and the archive does not stay.
        let unpack = recipe.containerfile.find("tar --extract").expect("the archive is unpacked");
        assert!(recipe.containerfile.find("sha256sum").expect("checked") < unpack, "{recipe:?}");
        assert!(recipe.containerfile.contains("rm /tmp/desktop.tar.gz"), "{recipe:?}");
        // The one directory inside the archive is stripped, so the program is where the record says.
        assert!(recipe.containerfile.contains("--strip-components=1"), "{recipe:?}");
        assert!(recipe.containerfile.contains(&format!("test -x '{}'", desktop.command())), "{recipe:?}");
        // Nothing of the application is in QCode's own files: only its address, and QCode's own
        // small extension that goes beside it.
        assert!(
            recipe
                .files
                .iter()
                .all(|(path, _)| path.starts_with("template") || path.starts_with("desktop/qcode-inbox")),
            "{recipe:?}"
        );
    }

    /// The part of the window's install step that picks the archive, run by `sh` on a machine
    /// whose `uname -m` answers `machine`, with what it chose printed after it.
    fn chosen_on(machine: &str) -> std::process::Output {
        let recipe = image(&profile(HarnessKind::AntigravityIde, Template::Recommended));
        let file = &recipe.containerfile;
        let start = file.find("case \"$(uname -m)\" in").expect("the archive is chosen in the build");
        let end = file[start..].find("esac").expect("the choice ends") + start + "esac".len();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let bin = std::env::temp_dir().join(format!("qcode-uname-{machine}-{stamp}"));
        std::fs::create_dir_all(&bin).expect("a folder");
        let uname = bin.join("uname");
        std::fs::write(&uname, format!("#!/bin/sh\necho {machine}\n")).expect("a uname");
        std::fs::set_permissions(&uname, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} && echo \"$address $sum\"", &file[start..end]))
            .env("PATH", path)
            .output()
            .expect("sh runs");
        let _ = std::fs::remove_dir_all(&bin);
        out
    }

    #[test]
    fn a_window_is_built_from_the_archive_of_the_processor_the_image_is_built_on() {
        // Chosen by the build's own `uname -m`, not by the processor QCode was compiled for: both
        // answers are asked of the same QCode here.
        let desktop = HarnessKind::AntigravityIde.desktop().expect("it opens a window");
        for archive in desktop.archives {
            let out = chosen_on(archive.machine);
            assert!(out.status.success(), "{}: {}", archive.machine, String::from_utf8_lossy(&out.stderr));
            assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{} {}\n", archive.address, archive.sha256));
        }
        // Any other processor stops the build, saying which it has and which QCode knows.
        let out = chosen_on("riscv64");
        assert!(!out.status.success());
        assert!(out.stdout.is_empty(), "nothing is chosen: {}", String::from_utf8_lossy(&out.stdout));
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "antigravity-ide 2.5.5 has no archive for riscv64; QCode knows x86_64 and aarch64.\n"
        );
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
    fn a_command_line_harness_never_becomes_root_in_its_image_without_graphify() {
        for harness in HarnessKind::TERMINAL {
            let without = Profile { without: vec![Extra::Graphify], ..profile(harness, Template::Recommended) };
            for profile in [profile(harness, Template::Base), without] {
                let file = image(&profile).containerfile;
                assert!(!file.contains("USER root"), "{harness:?} {:?}: {file}", profile.template);
                assert!(!file.contains("apt-get"), "{harness:?} {:?}: {file}", profile.template);
            }
        }
    }

    #[test]
    fn a_command_line_harness_image_never_ends_as_root() {
        // A QCode template needs root for graphify's system packages, and for nothing after them: the
        // plugins and the configuration belong to the image's user, and a container must never
        // default to root.
        for harness in HarnessKind::ALL {
            for template in Template::ALL {
                let file = image(&profile(harness, template)).containerfile;
                let last_user = file.lines().rfind(|line| line.starts_with("USER "));
                if file.contains("USER root") {
                    assert_eq!(last_user, Some(format!("USER {USER}").as_str()), "{harness:?} {template:?}: {file}");
                }
                if harness.desktop().is_none() && template != Template::Base {
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
    fn both_qcode_templates_install_graphify_for_every_harness_outside_the_home() {
        for (harness, template) in HarnessKind::ALL
            .into_iter()
            .flat_map(|harness| [Template::Recommended, Template::High].map(|template| (harness, template)))
        {
            let file = image(&profile(harness, template)).containerfile;
            let step = steps(&file).into_iter().find(|step| step.contains("pipx")).expect("graphify is installed");
            assert!(step.contains("apt-get install --yes --no-install-recommends python3 pipx"), "{step}");
            assert!(step.contains("pipx install --global graphifyy"), "{step}");
            assert!(step.contains("PIPX_GLOBAL_HOME=/opt/pipx"), "not under the home: {step}");
            assert!(step.contains("PIPX_GLOBAL_BIN_DIR=/usr/local/bin"), "on the path: {step}");
            assert!(step.contains("graphify --version"), "the step checks what it installed: {step}");
            assert!(!step.contains("$HOME"), "{step}");
            let base = image(&profile(harness, Template::Base)).containerfile;
            assert!(!base.contains("graphify"), "{harness:?}: {base}");
        }
    }

    #[test]
    fn both_qcode_templates_put_graphifys_skill_for_their_own_harness_into_the_home() {
        for (harness, template) in HarnessKind::ALL
            .into_iter()
            .flat_map(|harness| [Template::Recommended, Template::High].map(|template| (harness, template)))
        {
            let file = image(&profile(harness, template)).containerfile;
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
            let base = image(&profile(harness, Template::Base)).containerfile;
            assert!(!base.contains("--platform"), "{harness:?}: {base}");
        }
    }

    #[test]
    fn qcode_recommended_tells_every_harness_of_graphify_from_the_home_after_its_settings() {
        for harness in HarnessKind::ALL {
            let file = image(&profile(harness, Template::Recommended)).containerfile;
            let all = steps(&file);
            let word = guidance::platform(harness);
            let at = all
                .iter()
                .position(|step| step.contains(&format!("graphify {word} install")))
                .unwrap_or_else(|| panic!("{harness:?}: graphify's section is not set up: {file}"));
            let step = &all[at];
            assert!(step.starts_with("cd \"$HOME\""), "run in the home, never in the workspace: {step}");
            assert!(!step.contains("/work"), "{step}");
            let user = guidance::user_file(harness);
            let written = guidance::graphify_file(harness);
            assert!(step.contains(&format!("cat '{written}'; }} >> \"$HOME/{user}\"")), "{step}");
            assert!(step.contains(&format!("rm '{written}'")), "{step}");
            assert!(step.ends_with(&format!("grep -qF '## graphify' \"$HOME/{user}\"")), "it checks: {step}");
            // The settings of the template are copied first, so the hooks are merged into them
            // rather than copied over.
            if let Some(settings) = profile(harness, Template::Recommended).files().first() {
                let copied = file.find(&format!("\"$HOME/{}\"", settings.path)).expect("the settings are written");
                let merged = file.find(&format!("graphify {word} install")).expect("the step");
                assert!(copied < merged, "{harness:?}: {file}");
            }
            // QCode extra writes graphify's section into the workspace instead, and base has none.
            for other in [Template::Base, Template::High] {
                let file = image(&profile(harness, other)).containerfile;
                assert!(!file.contains(&format!("graphify {word} install")), "{harness:?} {other:?}: {file}");
            }
            let without = Profile { without: vec![Extra::Graphify], ..profile(harness, Template::Recommended) };
            assert!(!image(&without).containerfile.contains("graphify"), "{harness:?}: switched off");
        }
        // Where the one harness that has a plugin gets it.
        let opencode = image(&profile(HarnessKind::OpenCode, Template::Recommended)).containerfile;
        assert!(
            opencode.contains(
                "mkdir -p \"$(dirname '.config/opencode/plugins/graphify.js')\" && mv '.opencode/plugins/graphify.js' '.config/opencode/plugins/graphify.js'"
            ),
            "{opencode}"
        );
        assert!(opencode.contains("rm -rf '.opencode'"), "{opencode}");
    }

    #[test]
    fn the_qcode_templates_set_the_makers_switches_in_the_image_and_base_does_not() {
        for harness in HarnessKind::ALL {
            for template in [Template::Recommended, Template::High] {
                let file = image(&profile(harness, template)).containerfile;
                for (key, value) in template.environment(harness) {
                    assert!(file.contains(&format!("\nENV {key}=\"{value}\"\n")), "{harness:?} {template:?}: {file}");
                }
            }
            let base = image(&profile(harness, Template::Base)).containerfile;
            for (key, _) in Template::Recommended.environment(harness) {
                assert!(!base.contains(key), "{harness:?}: {base}");
            }
        }
        let claude = image(&profile(HarnessKind::ClaudeCode, Template::Recommended)).containerfile;
        assert!(claude.contains("\nENV DISABLE_AUTOUPDATER=\"1\"\n"), "{claude}");
    }

    #[test]
    fn the_home_step_puts_graphifys_section_where_each_harness_reads_it_in_a_real_shell() {
        // graphify's installer stood in for by a script that writes what the real one was
        // measured to write, and the step run by `sh` in a home of its own: the section lands in
        // the file the harness reads, after what the file held, and nothing is left behind.
        for harness in HarnessKind::ALL {
            let root = std::env::temp_dir().join(format!("qcode-home-step-{}-{harness:?}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let (home, bin) = (root.join("home"), root.join("bin"));
            std::fs::create_dir_all(&home).expect("home");
            std::fs::create_dir_all(&bin).expect("bin");
            let written = guidance::graphify_file(harness);
            let mut fake = format!(
                "#!/bin/sh\nmkdir -p \"$(dirname '{written}')\"\nprintf '## graphify\\n\\nMap rules.\\n' > '{written}'\n"
            );
            for (from, _) in guidance::graphify_leftovers(harness) {
                if from.ends_with(".js") {
                    fake.push_str(&format!("mkdir -p \"$(dirname '{from}')\" && echo plugin > '{from}'\n"));
                } else {
                    fake.push_str(&format!("mkdir -p '{from}/inner' && echo left > '{from}/inner/file'\n"));
                }
            }
            let graphify = bin.join("graphify");
            std::fs::write(&graphify, fake).expect("the stand-in");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&graphify, std::fs::Permissions::from_mode(0o755)).expect("executable");
            let user = guidance::user_file(harness);
            let before = home.join(user);
            std::fs::create_dir_all(before.parent().expect("a folder")).expect("folder");
            std::fs::write(&before, "# graphify\nthe skill's own line\n").expect("what the skill step wrote");
            let step = graphify_guidance(harness);
            let script = step.strip_prefix("RUN ").expect("a step");
            let status = std::process::Command::new("sh")
                .args(["-c", script])
                .env("HOME", &home)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .status()
                .expect("sh runs");
            assert!(status.success(), "{harness:?}: {script}");
            let text = std::fs::read_to_string(&before).expect("the file is there");
            assert_eq!(text, "# graphify\nthe skill's own line\n\n## graphify\n\nMap rules.\n", "{harness:?}");
            assert!(!home.join(written).exists(), "{harness:?}: the file graphify wrote is gone");
            for (from, to) in guidance::graphify_leftovers(harness) {
                assert!(!home.join(from).exists(), "{harness:?}: {from} is left");
                if !to.is_empty() {
                    assert!(home.join(to).exists(), "{harness:?}: {to} is not there");
                }
            }
            let _ = std::fs::remove_dir_all(&root);
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
    fn qcode_recommended_installs_the_starter_plugins_and_qcode_extra_every_one() {
        let installed = |template| -> Vec<String> {
            let file = image(&profile(HarnessKind::ClaudeCode, template)).containerfile;
            file.split("claude plugin install ")
                .skip(1)
                .filter_map(|after| after.split_whitespace().next())
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(installed(Template::Recommended), CLAUDE_STARTER_PLUGINS);
        assert_eq!(installed(Template::High), CLAUDE_PLUGINS);
        assert_eq!(installed(Template::QuvytaDev), CLAUDE_PLUGINS);
        let file = image(&profile(HarnessKind::ClaudeCode, Template::Recommended)).containerfile;
        let added: Vec<&str> = file
            .split("claude plugin marketplace add ")
            .skip(1)
            .filter_map(|after| after.split_whitespace().next())
            .collect();
        assert_eq!(added, ["anthropics/claude-plugins-official", "wshobson/agents"], "only the ones it installs from");
        let file = image(&profile(HarnessKind::ClaudeCode, Template::High)).containerfile;
        let added = file.matches("claude plugin marketplace add ").count();
        assert_eq!(added, CLAUDE_MARKETPLACES.len(), "{file}");
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

        // Every plugin of a marketplace switched off, and that marketplace is not added either.
        let file = image(&without(vec![
            Extra::Plugin("example-skills@anthropic-agent-skills"),
            Extra::Plugin("accesslint@accesslint"),
        ]))
        .containerfile;
        assert!(!file.contains("anthropics/skills") && !file.contains("accesslint"), "{file}");
        assert!(file.contains("anthropics/claude-plugins-official"), "{file}");
        assert!(file.contains("gfargo/tui-design-skill"), "{file}");
        let starter = Profile {
            without: vec![Extra::Plugin("block-no-verify@claude-code-workflows")],
            ..profile(HarnessKind::ClaudeCode, Template::Recommended)
        };
        let file = image(&starter).containerfile;
        assert!(!file.contains("wshobson/agents"), "the starter set's only plugin from it: {file}");

        let file = image(&without(vec![Extra::Graphify])).containerfile;
        assert!(!file.contains("graphify") && !file.contains("pipx"), "{file}");
        assert!(file.contains("claude plugin install superpowers@claude-plugins-official"), "{file}");

        // Everything off is QCode recommended's image with everything off.
        let file = image(&without(Template::High.extras(HarnessKind::ClaudeCode))).containerfile;
        let recommended = Profile {
            without: Template::Recommended.extras(HarnessKind::ClaudeCode),
            ..profile(HarnessKind::ClaudeCode, Template::Recommended)
        };
        assert_eq!(file, image(&recommended).containerfile);
        assert!(!file.contains("claude plugin") && !file.contains("graphify"), "{file}");
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
        // QCode recommended installs it and names it the same way.
        let recommended = image(&profile(HarnessKind::OpenCode, Template::Recommended));
        assert_eq!(recommended.files, recipe.files, "the same settings");
        assert!(
            recommended.containerfile.contains("npm install -g --cache /tmp/qcode-npm-cache oh-my-openagent"),
            "{}",
            recommended.containerfile
        );
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
    fn every_step_of_a_qcode_template_says_whose_it_is_and_that_the_build_needs_the_network_when_it_fails() {
        for (template, failed, name) in [
            (Template::Recommended, RECOMMENDED_FAILED, "QCode recommended"),
            (Template::High, EXTRA_FAILED, "QCode extra"),
        ] {
            for harness in HarnessKind::ALL {
                let file = image(&profile(harness, template)).containerfile;
                let added: Vec<String> = steps(&file)
                    .into_iter()
                    .filter(|step| ["pipx", "claude plugin", "oh-my-openagent"].iter().any(|word| step.contains(word)))
                    .collect();
                assert_eq!(added.len(), template.additions(harness).len(), "{template:?} {harness:?}: {file}");
                for step in added {
                    assert!(step.starts_with("{ "), "the whole step is one group: {step}");
                    assert!(step.contains(&format!("|| {{ echo '{failed} ")), "{step}");
                    assert!(step.contains(&format!("What {name} adds")), "{step}");
                    assert!(step.contains("the build needs the network"), "{step}");
                    assert!(step.trim_end().ends_with("exit 1; }"), "the build stops: {step}");
                }
            }
        }
    }

    #[test]
    fn a_failing_graphify_step_of_qcode_recommended_says_it_is_its_own_in_a_real_shell() {
        let file = image(&profile(HarnessKind::Codex, Template::Recommended)).containerfile;
        let step = steps(&file).into_iter().find(|step| step.contains("pipx install")).expect("step");
        let script = step.replace("graphify --version", "false");
        let ran = std::process::Command::new("sh").arg("-c").arg(&script).output().expect("a shell runs the step");
        let said = String::from_utf8_lossy(&ran.stderr);
        // The installs themselves fail too on a machine without apt; what counts is the words.
        assert!(!ran.status.success(), "{script}");
        assert!(said.contains(&format!("{RECOMMENDED_FAILED} could not install graphify")), "{said}");
        assert!(said.contains("What QCode recommended adds") && !said.contains(EXTRA_FAILED), "{said}");
    }

    #[test]
    fn a_failing_step_of_qcode_extra_stops_and_says_why_in_a_real_shell() {
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
        assert!(said.contains(EXTRA_FAILED) && said.contains("needs the network"), "{said}");
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
        let in_home = |recipe: &Recipe| -> Vec<(PathBuf, String)> {
            recipe.files.iter().filter(|(path, _)| path.starts_with("template")).cloned().collect()
        };
        let (path, contents) = in_home(&recipe).into_iter().next().expect("the template writes a file");
        assert_eq!(path, PathBuf::from("template/.config/Antigravity IDE/User/settings.json"));
        assert!(contents.contains("\"telemetry.telemetryLevel\": \"off\""), "{contents}");
        assert!(recipe.containerfile.contains("\"$HOME/.config/Antigravity IDE/User/settings.json\""), "{recipe:?}");
        assert!(
            in_home(&image(&profile(HarnessKind::AntigravityIde, Template::Base))).is_empty(),
            "base writes nothing"
        );
    }

    #[test]
    fn a_window_carries_the_inbox_extension_inside_the_application_whatever_its_template() {
        for template in Template::ALL {
            let recipe = image(&profile(HarnessKind::AntigravityIde, template));
            let staged: Vec<&PathBuf> =
                recipe.files.iter().map(|(path, _)| path).filter(|path| path.starts_with("desktop")).collect();
            assert_eq!(
                staged,
                [
                    &PathBuf::from("desktop/qcode-inbox/package.json"),
                    &PathBuf::from("desktop/qcode-inbox/extension.js")
                ],
                "{template:?}"
            );
            let copy = "COPY desktop/qcode-inbox /opt/antigravity-ide/resources/app/extensions/qcode-inbox";
            let at =
                recipe.containerfile.find(copy).unwrap_or_else(|| panic!("{template:?}: {}", recipe.containerfile));
            let unpacked = recipe.containerfile.find("tar --extract").expect("the application is unpacked");
            assert!(unpacked < at, "{template:?}: copied into the application after it is unpacked");
            let script = &recipe.files.iter().find(|(path, _)| path.ends_with("extension.js")).expect("the script").1;
            assert!(script.contains("antigravity.sendPromptToAgentPanel") && script.contains("\"peek\""), "{script}");
        }
        for harness in HarnessKind::TERMINAL {
            let recipe = image(&profile(harness, Template::Recommended));
            assert!(!recipe.containerfile.contains("qcode-inbox"), "{harness:?} has a prompt of its own");
        }
    }

    #[test]
    fn a_window_login_is_taken_as_the_two_rows_of_its_database() {
        // The database holds far more than the login; only the rows are taken, and the script
        // fails when they are not both there, as a missing login file fails for a terminal.
        let script = capture_script(&profile(HarnessKind::AntigravityIde, Template::Base));
        assert_eq!(script, crate::desktop::login::capture_script());
        assert!(script.contains(&format!("{CAPTURE_DIR}/{}", crate::desktop::login::STORED)), "{script}");
        assert!(!script.contains("cp "), "the database itself is never copied: {script}");
    }

    #[test]
    fn a_window_on_a_qcode_template_is_marked_for_its_approvals_and_one_on_base_is_not() {
        let marker = crate::desktop::login::APPROVALS;
        for template in [Template::Recommended, Template::High] {
            let recipe = image(&profile(HarnessKind::AntigravityIde, template));
            let step = recipe
                .containerfile
                .lines()
                .find(|line| line.contains(marker))
                .unwrap_or_else(|| panic!("{template:?}: {}", recipe.containerfile));
            assert!(step.starts_with("RUN "), "{step}");
            // Written while the image is still root, since /usr/share is not the image user's.
            let at = recipe.containerfile.find(step).expect("the step");
            let back = recipe.containerfile.rfind(&format!("USER {USER}")).expect("the image ends as its user");
            assert!(at < back, "{}", recipe.containerfile);
        }
        let base = image(&profile(HarnessKind::AntigravityIde, Template::Base));
        assert!(!base.containerfile.contains(marker), "base stays as it comes: {}", base.containerfile);
        for harness in HarnessKind::TERMINAL {
            assert!(!image(&profile(harness, Template::High)).containerfile.contains(marker), "{harness:?}");
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
        for harness in HarnessKind::TERMINAL {
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
