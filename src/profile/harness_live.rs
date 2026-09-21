//! The part of a harness record no documentation can answer for: that the install really
//! succeeds in the image, that the program really starts and takes the unattended-mode argument,
//! that the login lives where the record says, that the variables the record sets are really
//! read, and that the configuration the `recommended` template writes is really accepted.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. Each one installs a harness from npm
//! into a throwaway profile image, so every run needs the network once per harness; the checks
//! themselves run in a container without one.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```
//!
//! Nobody signs in here: that needs a person and an account. The login file is checked the way
//! the record was written, from the installed package's own text, and then the harness is made
//! to read a file placed where the record says, so that a path the harness no longer reads is
//! caught even when the text still mentions it.
//!
//! Everything a test makes is named `qcode/harnesstest-…`, `qcode/profile/harnesstest-…` or
//! `qcode-harnesstest-…` and is removed again. The machine's own `qcode/base` is built when it
//! is missing and left, as [`crate::base::ensure`] means it to be, and a copy of it stands in
//! for it under the profile image.

use std::path::Path;

use super::{AccountKind, Harness, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::base::paths::{CODE_DIR, KEEP_ALIVE};
use crate::engine::names::{BASE_IMAGE, HOSTNAME};
use crate::engine::run::{build_image, capture};
use crate::engine::{ContainerCreate, Engine, EngineKind, Exec, HostUser, ImageBuild, Network, detect};
use crate::ui::profiles::recipe;

/// The copy of the base image the profile images here are built on.
const TEST_BASE: &str = "qcode/harnesstest-base";

/// Where a global npm install lands in the base image, and so where the installed package is
/// read for the file names the record promises.
const PACKAGES: &str = "/usr/local/npm/lib/node_modules";

/// The engines installed on this machine, or nothing at all when the tests are switched off.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let found: Vec<Engine> =
        [EngineKind::Podman, EngineKind::Docker].into_iter().filter_map(|kind| detect(kind).ok()).collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// The profile a harness is verified as: the `recommended` template, so that the settings file
/// is in the image and can be checked along with everything else.
fn profile(harness: HarnessKind) -> Profile {
    Profile {
        name: SafeName::parse(&format!("harnesstest-{}", harness.record().id)).expect("the name is safe"),
        harness,
        template: Template::Recommended,
        account: AccountKind::Subscription,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
    }
}

/// A running container of the profile image, and the shell into it.
struct Lab {
    engine: Engine,
    container: String,
}

impl Lab {
    /// Runs `script` with a shell and answers what it printed, or everything the shell said
    /// when it failed.
    fn run(&self, script: &str) -> Result<String, String> {
        let command = ["sh", "-c", script];
        capture(&self.engine.exec_without_terminal(&Exec { container: &self.container, command: &command }))
            .map_err(|error| format!("{error:?}"))
    }

    /// Runs `script` and fails the test with the shell's own words when it refuses.
    fn ok(&self, script: &str) -> String {
        self.run(script).unwrap_or_else(|said| panic!("`{script}` on {:?} failed: {said}", self.engine.kind()))
    }

    /// Runs `script`, which must fail, and answers what it said.
    fn fails(&self, script: &str) -> String {
        match self.run(script) {
            Ok(printed) => panic!("`{script}` on {:?} was expected to fail; it printed: {printed}", self.engine.kind()),
            Err(said) => said,
        }
    }

    /// The files of the installed package that hold `text`, byte for byte, so that a name in a
    /// compiled binary is found as well as one in a script.
    fn package_mentions(&self, text: &str) -> Vec<String> {
        self.run(&format!("grep -rlaF -- '{text}' {PACKAGES}"))
            .map(|found| found.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Fails unless the installed package names `text` somewhere.
    fn expect_in_package(&self, text: &str, why: &str) {
        let found = self.package_mentions(text);
        assert!(!found.is_empty(), "{:?}: nothing in the package mentions `{text}` ({why})", self.engine.kind());
    }
}

/// Builds the profile image on a copy of the base image and starts a container of it, as the
/// person, without a network: the install already happened, and nothing checked afterwards
/// should need the outside.
fn open(engine: Engine, profile: &Profile) -> Lab {
    let container = format!("qcode-harnesstest-{}", profile.harness.record().id);
    let _ = capture(&engine.remove_container(&container));
    build(&engine, profile);
    capture(&engine.create_container(&ContainerCreate {
        name: &container,
        hostname: HOSTNAME,
        labels: &[],
        image: &profile.image(),
        mounts: &[],
        network: Network::None,
        user: HostUser::current().expect("the current user"),
        workdir: Some(Path::new(CODE_DIR)),
        command: KEEP_ALIVE,
    }))
    .expect("the container is made");
    capture(&engine.start_container(&container)).expect("the container starts");
    Lab { engine, container }
}

/// Builds the image of `profile` from its recipe, on a copy of the base image named
/// [`TEST_BASE`] so the machine's own `qcode/base` is never replaced. [`clear`] takes both away.
pub(crate) fn build(engine: &Engine, profile: &Profile) {
    let record = profile.harness.record();
    let _ = capture(&engine.remove_image(&profile.image()));
    let _ = capture(&engine.remove_image(TEST_BASE));

    crate::base::ensure(engine, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("the base image does not build on {:?}: {error:?}", engine.kind()));

    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-harnesstest-{}-{stamp}", record.id));
    std::fs::create_dir_all(&folder).expect("a folder in the temporary folder");

    let copy = folder.join("base.Containerfile");
    std::fs::write(&copy, format!("FROM {BASE_IMAGE}\n")).expect("a Containerfile");
    build_image(
        engine,
        &ImageBuild { image: TEST_BASE, containerfile: &copy, context: &folder },
        &|| false,
        &mut |_| {},
    )
    .expect("the copy of the base image builds");

    let recipe = recipe::image(profile);
    let from = format!("FROM {BASE_IMAGE}");
    assert!(recipe.containerfile.starts_with(&from), "the recipe builds on the base image: {}", recipe.containerfile);
    let containerfile = recipe.containerfile.replacen(&from, &format!("FROM {TEST_BASE}"), 1);
    std::fs::write(folder.join("Containerfile"), &containerfile).expect("a Containerfile");
    for (path, contents) in &recipe.files {
        let target = folder.join(path);
        std::fs::create_dir_all(target.parent().expect("the file has a folder")).expect("a folder");
        std::fs::write(&target, contents).expect("a template file");
    }
    let mut said = String::new();
    let built = build_image(
        engine,
        &ImageBuild { image: &profile.image(), containerfile: &folder.join("Containerfile"), context: &folder },
        &|| false,
        &mut |line| {
            said.push_str(line);
            said.push('\n');
        },
    );
    let _ = std::fs::remove_dir_all(&folder);
    assert!(built.is_ok(), "{} on {:?} did not build:\n{said}", record.id, engine.kind());
}

/// Takes away everything a run makes, except the machine's own base image.
pub(crate) fn clear(engine: &Engine, profile: &Profile, container: &str) {
    let _ = capture(&engine.remove_container(container));
    let _ = capture(&engine.remove_image(&profile.image()));
    let _ = capture(&engine.remove_image(TEST_BASE));
}

/// Whether `text` holds something shaped like a version: three or more dotted numbers.
fn names_a_version(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_digit() && c != '.').any(|word| {
        let parts: Vec<&str> = word.split('.').collect();
        parts.len() >= 3 && parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    })
}

/// The checks every record makes the same promises about, followed by `more` for the ones only
/// that harness can be asked.
fn verify(harness: HarnessKind, more: impl Fn(&Lab, &Harness)) {
    let profile = profile(harness);
    let record = harness.record();
    for engine in engines() {
        let lab = open(engine, &profile);
        let kind = lab.engine.kind();

        // 1 and 2: the install succeeded, because the image built, and the program answers.
        let version = lab.ok(&format!("{} --version", record.command));
        assert!(names_a_version(&version), "{kind:?}: `{} --version` printed `{version}`", record.command);

        // 3: the unattended-mode arguments are taken; a program that rejected them would refuse
        // to print its version behind them. Which of them is really parsed is `more`'s work.
        let arguments = record.auto_run.join(" ");
        let behind = lab.ok(&format!("{} {arguments} --version", record.command));
        assert!(names_a_version(&behind), "{kind:?}: `{} {arguments} --version` printed `{behind}`", record.command);

        // 4: the package's own text names every file the record says the login is in.
        for path in record.identity {
            let file = path.rsplit('/').next().expect("a path has a last part");
            lab.expect_in_package(file, "the login file the record names");
        }

        // 5: the template's file is in the home directory and the package knows its keys.
        if let Some(file) = record.settings {
            let written = lab.ok(&format!("cat \"$HOME/{}\"", file.path));
            assert_eq!(written, file.contents, "{kind:?}: the template's file reached the image");
        }

        // 6: every variable the record sets is set in the container and read by the package.
        for (key, value) in record.environment {
            assert_eq!(lab.ok(&format!("printf '%s' \"${key}\"")), *value, "{kind:?}: {key} is set in the image");
            lab.expect_in_package(key, "a variable the record sets");
        }

        more(&lab, record);
        clear(&lab.engine, &profile, &lab.container);
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_installs_answers_and_keeps_its_login_where_the_record_says() {
    verify(HarnessKind::ClaudeCode, |lab, record| {
        let kind = lab.engine.kind();
        let help = lab.ok("claude --help");
        assert!(help.contains(record.auto_run[0]), "{kind:?}: `--help` no longer lists {}", record.auto_run[0]);

        // The login: `auth status` reads the credentials file, so a file placed where the record
        // says turns "not logged in" into "logged in". One with the inference scope: without it
        // the file is read and still counts as nobody.
        let login = record.identity[0];
        // `auth status` prints its answer either way and leaves with 1 when nobody is signed in.
        let before = lab.ok("claude auth status 2>&1 || true");
        assert!(before.contains("\"loggedIn\": false"), "{kind:?}: {before}");
        lab.ok(&format!(
            "printf '%s' '{{\"claudeAiOauth\":{{\"accessToken\":\"x\",\"refreshToken\":\"x\",\"expiresAt\":1,\"scopes\":[\"user:inference\"]}}}}' \
             > \"$HOME/{login}\""
        ));
        let after = lab.ok("claude auth status 2>&1 || true");
        assert!(after.contains("\"loggedIn\": true"), "{kind:?}: the file at {login} is not read: {after}");
        lab.ok(&format!("rm \"$HOME/{login}\""));

        // The settings: `doctor` reads the settings files and names an invalid one.
        let settings = record.settings.expect("Claude Code has a settings file");
        lab.expect_in_package("permissions.defaultMode", "the settings key the template writes");
        lab.expect_in_package("bypassPermissions", "the value the template writes");
        let doctor = lab.ok("claude doctor 2>&1");
        assert!(!doctor.contains("Invalid settings"), "{kind:?}: the template's settings are refused: {doctor}");
        lab.ok(&format!("printf '%s' '{{ broken' > \"$HOME/{}\"", settings.path));
        let broken = lab.ok("claude doctor 2>&1");
        assert!(broken.contains("Invalid settings"), "{kind:?}: `doctor` does not read the settings file: {broken}");
    });
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_installs_answers_and_keeps_its_login_where_the_record_says() {
    verify(HarnessKind::OpenCode, |lab, record| {
        let kind = lab.engine.kind();
        // The argument belongs to the default command, the one that opens the interface, and
        // not only to `run`.
        let help = lab.ok("opencode --help 2>&1");
        assert!(help.contains(record.auto_run[0]), "{kind:?}: `--help` no longer lists {}", record.auto_run[0]);

        // The login: `providers list` prints where it reads credentials from and how many it
        // found, so a file placed where the record says is counted.
        let login = record.identity[0];
        let before = lab.ok("opencode providers list 2>&1");
        assert!(before.contains(login) && before.contains("0 credentials"), "{kind:?}: {before}");
        lab.ok(&format!(
            "mkdir -p \"$HOME/$(dirname '{login}')\" && \
             printf '%s' '{{\"anthropic\":{{\"type\":\"api\",\"key\":\"x\"}}}}' > \"$HOME/{login}\""
        ));
        let after = lab.ok("opencode providers list 2>&1");
        assert!(after.contains("1 credentials"), "{kind:?}: the file at {login} is not read: {after}");
        lab.ok(&format!("rm \"$HOME/{login}\""));

        // The settings: the resolved configuration carries the permission the template wrote.
        let config = lab.ok("opencode debug config");
        assert!(config.contains("\"permission\"") && config.contains("\"*\": \"allow\""), "{kind:?}: {config}");
    });
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn gemini_cli_installs_answers_and_keeps_its_login_where_the_record_says() {
    verify(HarnessKind::GeminiCli, |lab, record| {
        let kind = lab.engine.kind();
        let help = lab.ok("gemini --help");
        assert!(help.contains("--approval-mode") && help.contains("yolo"), "{kind:?}: {help}");

        // The argument is really parsed: a value outside its choices is refused before anything
        // else happens, and the right one is announced. Headless mode stops at the missing
        // account, before the network; it fails either way, so both streams are collected and
        // the exit is disregarded.
        let refused = lab.fails("gemini --approval-mode=bogus -p hi");
        assert!(refused.contains("Invalid values"), "{kind:?}: {refused}");
        let headless = format!("gemini {} -p hi 2>&1 || true", record.auto_run.join(" "));
        let trusted = lab.ok(&headless);
        assert!(trusted.contains("YOLO mode is enabled"), "{kind:?}: {trusted}");
        // Without the template's settings the folder is untrusted and the mode is put back to
        // "default", which is the reason the record writes them.
        assert!(!trusted.contains("overridden"), "{kind:?}: the unattended mode was overridden: {trusted}");
        let settings = record.settings.expect("Gemini CLI has a settings file");
        lab.ok(&format!("mv \"$HOME/{path}\" \"$HOME/{path}.aside\"", path = settings.path));
        let untrusted = lab.ok(&headless);
        assert!(untrusted.contains("overridden to \"default\""), "{kind:?}: {untrusted}");
        lab.ok(&format!("mv \"$HOME/{path}.aside\" \"$HOME/{path}\"", path = settings.path));
        lab.expect_in_package("folderTrust", "the settings key the template writes");

        // The login: the file keychain and the account file are named in the bundle, and the
        // variable that forces the file keychain is read there.
        lab.expect_in_package("FileKeychain", "the store that writes the login file");
        lab.expect_in_package("getGoogleAccountsPath", "the store that writes the account file");
        let dir = record.identity[0].split('/').next().expect("the login is in a directory");
        assert_eq!(dir, ".gemini");
        lab.expect_in_package("var GEMINI_DIR = \".gemini\"", "the directory both files are under");

        // The login file is encrypted with a key salted by the machine name and the user name,
        // which is the reason every container QCode creates is given one fixed machine name.
        // Only the salt can be checked here: the file itself is written by a sign-in through a
        // browser, and there is no way to make the harness write one unattended.
        // The bundle numbers its imports (`os15.hostname()`), so the salt is looked for by the
        // two parts that do not carry that number.
        lab.expect_in_package(".hostname()}-${", "the machine-name half of the salt the login key is made from");
        lab.expect_in_package(".userInfo().username}-gemini-cli", "the user-name half of that salt");
        assert_eq!(lab.ok("hostname").trim(), HOSTNAME, "{kind:?}: the container answers to the fixed machine name");
        assert_eq!(lab.ok("id -un").trim(), "qcode", "{kind:?}: the user half of the salt is the image's user");
    });
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn codex_installs_answers_and_keeps_its_login_where_the_record_says() {
    verify(HarnessKind::Codex, |lab, record| {
        let kind = lab.engine.kind();
        // The argument parser is strict, so `--version` behind an unknown argument would have
        // failed already; the help is checked so the reason is named when it goes.
        let help = lab.ok("codex --help");
        assert!(help.contains(record.auto_run[0]), "{kind:?}: `--help` no longer lists {}", record.auto_run[0]);
        let unknown = lab.fails("codex --no-such-argument --version");
        assert!(unknown.contains("unexpected argument"), "{kind:?}: {unknown}");

        // The login: `login status` reads the auth file, so a file placed where the record says
        // turns "not logged in" into "logged in".
        let login = record.identity[0];
        // `login status` says what it found on the error stream and leaves with 1 when nothing.
        let before = lab.fails("codex login status 2>&1");
        assert!(before.contains("Not logged in"), "{kind:?}: {before}");
        lab.ok(&format!(
            "mkdir -p \"$HOME/$(dirname '{login}')\" && printf '%s' '{{\"OPENAI_API_KEY\":\"x\"}}' > \"$HOME/{login}\""
        ));
        let after = lab.ok("codex login status 2>&1");
        assert!(after.contains("Logged in"), "{kind:?}: the file at {login} is not read: {after}");
        lab.ok(&format!("rm \"$HOME/{login}\""));

        // The settings: the configuration is loaded by every command and a wrong value is
        // named, so a clean `login status` means the template's file was taken.
        let settings = record.settings.expect("Codex has a settings file");
        lab.expect_in_package("approval_policy", "a settings key the template writes");
        lab.expect_in_package("sandbox_mode", "a settings key the template writes");
        assert!(!before.contains("Error loading configuration"), "{kind:?}: {before}");
        lab.ok(&format!("cp \"$HOME/{path}\" \"$HOME/{path}.aside\"", path = settings.path));
        lab.ok(&format!("printf '%s' 'sandbox_mode = \"nonsense\"' > \"$HOME/{}\"", settings.path));
        let broken = lab.run("codex login status 2>&1").unwrap_or_else(|said| said);
        assert!(broken.contains("Error loading configuration"), "{kind:?}: the settings file is not read: {broken}");
        lab.ok(&format!("mv \"$HOME/{path}.aside\" \"$HOME/{path}\"", path = settings.path));
    });
}

/// A profile of `harness` under QCode high, whose checks run without the network like every
/// profile here: what the build installed has to work from the image alone.
fn high(harness: HarnessKind) -> Profile {
    Profile {
        name: SafeName::parse(&format!("harnesstest-high-{}", harness.record().id)).expect("the name is safe"),
        template: Template::High,
        ..profile(harness)
    }
}

/// The size of `image` as the engine reports it, in whole megabytes, for the record of what QCode
/// high costs.
fn image_mb(engine: &Engine, image: &str) -> u64 {
    let program = match engine.kind() {
        EngineKind::Podman => "podman",
        EngineKind::Docker => "docker",
    };
    let out = std::process::Command::new(program)
        .args(["image", "inspect", "--format", "{{.Size}}", image])
        .output()
        .expect("the engine answers");
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) / 1_000_000
}

/// Builds the QCode high image of `harness` on every engine, checks what every harness gets, then
/// `more` for what only that harness gets, and says how long the build took and what it weighs.
fn verify_high(harness: HarnessKind, more: impl Fn(&Lab)) {
    let profile = high(harness);
    for engine in engines() {
        let started = std::time::Instant::now();
        let lab = open(engine, &profile);
        let kind = lab.engine.kind();
        eprintln!(
            "{kind:?} {}: QCode high image built in {} s, {} MB",
            harness.record().id,
            started.elapsed().as_secs(),
            image_mb(&lab.engine, &profile.image())
        );

        // graphify answers, from outside the home, for a user the image never saw, offline.
        let version = lab.ok("graphify --version");
        assert!(version.starts_with("graphify "), "{kind:?}: {version}");
        assert_eq!(lab.ok("command -v graphify").trim(), "/usr/local/bin/graphify", "{kind:?}");
        let home = lab.ok("ls -A \"$HOME\"; du -sm \"$HOME\" | cut -f1");
        assert!(!home.contains("pipx") && !home.contains(".local/share/pipx"), "{kind:?}: {home}");
        eprintln!(
            "{kind:?} {}: home directory of the image: {} MB",
            harness.record().id,
            home.lines().last().unwrap_or("")
        );
        let sizes = lab.ok("du -sm /opt/pipx /var/cache/npm /usr/local/npm/lib/node_modules/* 2>/dev/null || true");
        eprintln!("{kind:?} {}: sizes in MB:\n{sizes}", harness.record().id);
        // It maps code without asking anyone: the part of it that needs no model.
        lab.ok("mkdir -p /tmp/g && cd /tmp/g && printf 'def a():\\n    return b()\\n\\ndef b():\\n    return 1\\n' > m.py && graphify update . > /dev/null 2>&1 && test -s graphify-out/graph.json");

        more(&lab);
        clear(&lab.engine, &profile, &lab.container);
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qcode_high_gives_claude_code_graphify_and_the_five_plugins() {
    verify_high(HarnessKind::ClaudeCode, |lab| {
        let kind = lab.engine.kind();
        let listed = lab.ok("claude plugin list 2>&1");
        for plugin in super::CLAUDE_PLUGINS {
            let at = listed.find(plugin).unwrap_or_else(|| panic!("{kind:?}: {plugin} is not installed:\n{listed}"));
            let status = listed[at..].lines().find(|line| line.contains("Status:")).unwrap_or_default();
            assert!(status.contains("enabled"), "{kind:?}: {plugin}: {status}");
        }
        // The plugins were added to the settings the template wrote, not written over them, and
        // the first-start answers are still where Claude Code reads them.
        let settings = lab.ok("cat \"$HOME/.claude/settings.json\"");
        assert!(settings.contains("\"skipDangerousModePermissionPrompt\": true"), "{kind:?}: {settings}");
        assert!(settings.contains("bypassPermissions") && settings.contains("enabledPlugins"), "{kind:?}: {settings}");
        let state = lab.ok("cat \"$HOME/.claude.json\"");
        assert!(state.contains("\"hasTrustDialogAccepted\": true"), "{kind:?}: {state}");
        assert!(state.contains("\"hasCompletedOnboarding\": true"), "{kind:?}: {state}");
        assert!(!lab.ok("ls /usr/local/npm/lib/node_modules").contains("oh-my-openagent"), "{kind:?}");
        eprintln!("{kind:?}: plugins in the home: {} MB", lab.ok("du -sm \"$HOME/.claude/plugins\" | cut -f1").trim());
    });
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qcode_high_gives_opencode_graphify_and_loads_oh_my_openagent_from_the_image() {
    verify_high(HarnessKind::OpenCode, |lab| {
        let kind = lab.engine.kind();
        // Loaded, not only installed: its agents are opencode's own agents now, with no network
        // and nothing downloaded into the home at start.
        let agents = lab.ok("cd /work && opencode agent list 2>&1");
        // These come from the plugin alone; opencode without it lists build, plan, explore,
        // general and its own helpers.
        for agent in ["Sisyphus - ultraworker", "Prometheus - Plan Builder", "Metis - Plan Consultant"] {
            assert!(agents.contains(agent), "{kind:?}: opencode does not load oh-my-openagent:\n{agents}");
        }
        let config = lab.ok("cd /work && opencode debug config 2>&1");
        assert!(config.contains("file:///usr/local/npm/lib/node_modules/oh-my-openagent"), "{kind:?}: {config}");
        let fetched = lab.ok("ls \"$HOME/.cache/opencode/packages\" 2>/dev/null || true");
        assert!(!fetched.contains("oh-my-openagent"), "{kind:?}: a second copy was fetched: {fetched}");
        assert_eq!(lab.ok("printf '%s' \"$OMO_SEND_ANONYMOUS_TELEMETRY\""), "0", "{kind:?}");
        assert!(!lab.ok("claude --version 2>&1 || true").contains("Claude Code"), "{kind:?}: no plugins here");
    });
}
