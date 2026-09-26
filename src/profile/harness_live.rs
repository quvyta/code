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
use crate::engine::names::HOSTNAME;
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
        os: crate::base::Os::Debian,
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
        // Generous and finite: a harness answering for the first time, offline, can take minutes.
        let command = self
            .engine
            .exec_without_terminal(&Exec { container: &self.container, command: &command })
            .within(std::time::Duration::from_secs(600));
        capture(&command).map_err(|error| format!("{error:?}"))
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

/// Builds the image of `profile` from its recipe, on a copy of the base image of the profile's
/// system named [`TEST_BASE`], so the machine's own base image is never replaced. [`clear`] takes
/// both away.
pub(crate) fn build(engine: &Engine, profile: &Profile) {
    let record = profile.harness.record();
    let base = profile.os.image();
    let _ = capture(&engine.remove_image(&profile.image()));
    let _ = capture(&engine.remove_image(TEST_BASE));

    crate::base::ensure_os(engine, profile.os, &|| false, &mut |_| {}).unwrap_or_else(|error| {
        panic!("the base image of {:?} does not build on {:?}: {error:?}", profile.os, engine.kind())
    });

    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-harnesstest-{}-{stamp}", record.id));
    std::fs::create_dir_all(&folder).expect("a folder in the temporary folder");

    let copy = folder.join("base.Containerfile");
    std::fs::write(&copy, format!("FROM {base}\n")).expect("a Containerfile");
    build_image(
        engine,
        &ImageBuild { image: TEST_BASE, containerfile: &copy, context: &folder },
        &|| false,
        &mut |_| {},
    )
    .expect("the copy of the base image builds");

    let recipe = recipe::image(profile);
    let from = format!("FROM {base}");
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
        // Each argument quoted for the shell: Claude Code's include a JSON object.
        let arguments = record.auto_run.iter().map(|word| format!("'{word}'")).collect::<Vec<_>>().join(" ");
        let behind = lab.ok(&format!("{} {arguments} --version", record.command));
        assert!(names_a_version(&behind), "{kind:?}: `{} {arguments} --version` printed `{behind}`", record.command);

        // 4: the package's own text names every file the record says the login is in. Kimi Code
        // CLI joins its token file's name from the slot it signs in to, so no text holds it
        // whole; its own check below makes the harness read the file instead.
        for path in record.identity.iter().filter(|_| harness != HarnessKind::KimiCode) {
            let file = path.rsplit('/').next().expect("a path has a last part");
            lab.expect_in_package(file, "the login file the record names");
        }

        // 5: the template's file is in the home directory and the package knows its keys. A JSON
        // file may have graphify's hooks merged into it since; every key of the template's is
        // still there with its value.
        if let Some(file) = record.settings {
            let written = lab.ok(&format!("cat \"$HOME/{}\"", file.path));
            match serde_json::from_str::<serde_json::Value>(file.contents) {
                Ok(wanted) => {
                    let found: serde_json::Value = serde_json::from_str(&written).expect("the file is still JSON");
                    assert!(holds(&found, &wanted), "{kind:?}: the template's keys reached the image: {written}");
                }
                Err(_) => assert_eq!(written, file.contents, "{kind:?}: the template's file reached the image"),
            }
        }

        // 6: every variable the record and the template set is set in the container and read by
        // the package.
        for (key, value) in record.environment.iter().chain(profile.template.environment(harness)) {
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

/// A Gemini CLI profile that signs in with a key, under the `base` template that writes nothing
/// into the home: the image carries the system settings, readable by the person the container runs
/// as, and the harness reads them. Headless, with no key stored, it names the enforced type before
/// anything else, which it can only know from that file; the dialog itself, which offers the key
/// alone, was watched in a terminal by hand and is described on the record.
#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn gemini_cli_with_a_key_reads_the_system_settings_that_keep_it_to_the_key() {
    let profile = Profile {
        account: AccountKind::ApiKey,
        template: Template::Base,
        name: SafeName::parse("harnesstest-gemini-key").expect("the name is safe"),
        ..profile(HarnessKind::GeminiCli)
    };
    let file = HarnessKind::GeminiCli.record().key_only.expect("Gemini CLI is kept to a key");
    for engine in engines() {
        let lab = open(engine, &profile);
        let kind = lab.engine.kind();
        assert_eq!(lab.ok(&format!("cat {}", file.path)), file.contents, "{kind:?}: the person reads the file");
        assert_eq!(lab.ok(&format!("stat -c '%U %a' {}", file.path)).trim(), "root 644", "{kind:?}");
        let said = lab.ok("gemini -p hi 2>&1 || true");
        assert!(said.contains("'gemini-api-key' is enforced"), "{kind:?}: {said}");
        clear(&lab.engine, &profile, &lab.container);
    }
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

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn kimi_code_installs_answers_and_keeps_its_login_where_the_record_says() {
    verify(HarnessKind::KimiCode, |lab, record| {
        let kind = lab.engine.kind();
        let help = lab.ok("kimi --help");
        assert!(help.contains(record.auto_run[0]) && help.contains("Never Ask"), "{kind:?}: {help}");
        // Its parser refuses what it does not know, and says so before anything else runs.
        let unknown = lab.run("kimi --no-such-argument -p hi 2>&1").unwrap_or_else(|said| said);
        assert!(unknown.contains("unknown option"), "{kind:?}: {unknown}");

        // The login is two files, and each is read: the configuration names the provider and the
        // slot its token is in, the token is in that slot. With only one of them the harness
        // stops before it asks anybody, each time for its own reason; with both it goes on to
        // Kimi's server, which a container without a network cannot reach.
        let [token, config] = record.identity else { panic!("{kind:?}: the login is a token and a configuration") };
        lab.ok(&format!(
            "mkdir -p \"$HOME/$(dirname '{config}')\" && cat > \"$HOME/{config}\" <<'EOF'
default_model = \"kimi-code/kimi-for-coding\"

[providers.\"managed:kimi-code\"]
type = \"kimi\"
base_url = \"https://api.kimi.com/coding/v1\"
api_key = \"\"
oauth = {{ storage = \"file\", key = \"oauth/kimi-code\" }}

[models.\"kimi-code/kimi-for-coding\"]
provider = \"managed:kimi-code\"
model = \"kimi-for-coding\"
max_context_size = 262144
EOF"
        ));
        let untokened = lab.run("kimi -p hi 2>&1").unwrap_or_else(|said| said);
        assert!(untokened.contains("has no credential configured"), "{kind:?}: {untokened}");
        lab.ok(&format!(
            "mkdir -p \"$HOME/$(dirname '{token}')\" && printf '%s' \
             '{{\"access_token\":\"x\",\"refresh_token\":\"x\",\"expires_at\":9999999999,\"token_type\":\"Bearer\"}}' \
             > \"$HOME/{token}\""
        ));
        let signed = lab.run("kimi -p hi 2>&1").unwrap_or_else(|said| said);
        assert!(!signed.contains("no credential configured"), "{kind:?}: the file at {token} is not read: {signed}");
        assert!(!signed.contains("No model configured"), "{kind:?}: {signed}");
        lab.ok(&format!("rm \"$HOME/{config}\""));
        let unconfigured = lab.run("kimi -p hi 2>&1").unwrap_or_else(|said| said);
        assert!(unconfigured.contains("No model configured"), "{kind:?}: {config} is not read: {unconfigured}");
        lab.ok(&format!("rm \"$HOME/{token}\""));

        // The first start: the harness answers its own trust question into a file of that name.
        // The template's answer is taken away, the question is answered by Return on a terminal,
        // and the file the harness writes is the one the record writes.
        let trust = record.first_start.expect("the trust question is answered");
        assert_eq!(lab.ok(&format!("cat \"$HOME/{}\"", trust.path)), trust.contents, "{kind:?}: the template wrote it");
        lab.ok(&format!("rm \"$HOME/{}\"", trust.path));
        lab.ok("cd /work && (sleep 12; printf '\\r'; sleep 6) | TERM=xterm-256color timeout 40 script -q -c 'kimi --auto' /dev/null > /dev/null 2>&1; true");
        let answered = lab.ok(&format!("ls \"$HOME/$(dirname '{}')\"", trust.path));
        let name = trust.path.rsplit('/').next().expect("a name");
        assert_eq!(answered.trim(), name, "{kind:?}: the harness keeps its answer for /work under another name");

        // A provider of one's own: with the variables set, it asks the address they name rather
        // than asking anybody to sign in.
        let provider = lab.run(
            "KIMI_MODEL_NAME=m KIMI_MODEL_API_KEY=t KIMI_MODEL_PROVIDER_TYPE=openai \
             KIMI_MODEL_BASE_URL=http://127.0.0.1:9/v1 kimi -p hi 2>&1",
        );
        let provider = provider.unwrap_or_else(|said| said);
        assert!(!provider.contains("No model configured") && !provider.contains("/login"), "{kind:?}: {provider}");
        lab.expect_in_package("KIMI_MODEL_BASE_URL", "the variable the provider's address is handed over in");
    });
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qwen_code_installs_answers_and_reads_the_settings_the_template_writes() {
    verify(HarnessKind::QwenCode, |lab, record| {
        let kind = lab.engine.kind();
        let help = lab.ok("qwen --help");
        assert!(help.contains("--approval-mode") && help.contains("yolo"), "{kind:?}: {help}");
        let refused = lab.fails("qwen --approval-mode=bogus hi 2>&1");
        assert!(refused.contains("Invalid values"), "{kind:?}: {refused}");

        // A provider of one's own, from the three variables: it goes to the address they name
        // and says it could not connect, rather than asking how to sign in.
        let provider = "OPENAI_BASE_URL=http://127.0.0.1:9/v1 OPENAI_API_KEY=t OPENAI_MODEL=m";
        let asked = lab.ok(&format!("cd /work && {provider} qwen {} hi 2>&1 || true", record.auto_run.join(" ")));
        assert!(asked.contains("Connection error"), "{kind:?}: {asked}");
        assert!(!asked.contains("overridden"), "{kind:?}: the unattended mode was put back: {asked}");

        // The settings are read on every start, and the template's are taken: a broken file in
        // their place stops the same start with the file's name, the template's file does not.
        let settings = record.settings.expect("Qwen Code has a settings file");
        assert!(!asked.contains(settings.path), "{kind:?}: the template's settings are refused: {asked}");
        lab.ok(&format!("cp \"$HOME/{path}\" \"$HOME/{path}.aside\"", path = settings.path));
        lab.ok(&format!("printf '%s' '{{ broken' > \"$HOME/{}\"", settings.path));
        let broken = lab.ok(&format!("cd /work && {provider} qwen {} hi 2>&1 || true", record.auto_run.join(" ")));
        assert!(broken.contains(settings.path), "{kind:?}: the settings file is not read: {broken}");
        lab.ok(&format!("mv \"$HOME/{path}.aside\" \"$HOME/{path}\"", path = settings.path));
        lab.expect_in_package("folderTrust", "the setting that keeps the unattended mode");
        lab.expect_in_package("usageStatisticsEnabled", "the setting that stops its usage statistics");
    });
}

/// Whether `found` has every key of `wanted` with the same value, at every depth.
fn holds(found: &serde_json::Value, wanted: &serde_json::Value) -> bool {
    match (found, wanted) {
        (serde_json::Value::Object(found), serde_json::Value::Object(wanted)) => {
            wanted.iter().all(|(key, value)| found.get(key).is_some_and(|there| holds(there, value)))
        }
        _ => found == wanted,
    }
}

/// A server on the container's own loopback that writes down every request it gets and answers
/// each with an error: the provider a harness is pointed at, so what the harness would have sent a
/// model can be read without any model and without the network.
const CAPTURE: &str = "import http.server\nclass H(http.server.BaseHTTPRequestHandler):\n    def do_POST(self):\n        body = self.rfile.read(int(self.headers.get('content-length') or 0))\n        open('/tmp/captured', 'ab').write(body + b'\\n')\n        self.send_response(500)\n        self.end_headers()\n        self.wfile.write(b'{\"error\":{\"message\":\"capture\"}}')\n    do_GET = do_POST\n    def log_message(self, *a):\n        pass\nhttp.server.HTTPServer(('127.0.0.1', 8765), H).serve_forever()\n";

/// graphify's heading, which only its section carries: the skill the harnesses list is named
/// `graphify`, but none of them lists it under a Markdown heading. The sentences of the section
/// differ between harnesses (Codex's is shorter), so the heading is what is looked for.
const GRAPHIFY_LINE: &str = "## graphify";

/// How `harness` is made to send one prompt to the server at [`CAPTURE`], in `/work`, without a
/// terminal: the variables each already takes a provider of one's own by, and a command that
/// gives up after a generous minute. Codex prints the prompt it would send instead.
fn prompt(harness: HarnessKind) -> String {
    let at = "http://127.0.0.1:8765";
    match harness {
        HarnessKind::ClaudeCode => format!("ANTHROPIC_BASE_URL={at} ANTHROPIC_AUTH_TOKEN=x timeout 90 claude -p hi"),
        HarnessKind::OpenCode => format!(
            "OPENCODE_CONFIG_CONTENT='{{\"provider\":{{\"cap\":{{\"npm\":\"@ai-sdk/openai-compatible\",\"name\":\"cap\",\"options\":{{\"baseURL\":\"{at}/v1\",\"apiKey\":\"x\"}},\"models\":{{\"m\":{{\"name\":\"m\"}}}}}}}},\"model\":\"cap/m\",\"small_model\":\"cap/m\"}}' timeout 90 opencode run hi"
        ),
        // Without a key chosen in its settings it refuses before it sends anything, whatever the
        // variables say; the dialog of a real profile writes the same key.
        HarnessKind::GeminiCli => format!(
            "python3 -c \"import json, os; p = os.path.expanduser('~/.gemini/settings.json'); d = json.load(open(p)); \
             d.setdefault('security', {{}}).setdefault('auth', {{}})['selectedType'] = 'gemini-api-key'; \
             json.dump(d, open(p, 'w'))\" && GEMINI_API_KEY=x GOOGLE_GEMINI_BASE_URL={at} timeout 90 gemini -p hi"
        ),
        // In a shell of its own, so its output goes to the file the others' requests go to.
        HarnessKind::Codex => "sh -c 'codex debug prompt-input > /tmp/captured'".to_owned(),
        HarnessKind::KimiCode => format!(
            "KIMI_MODEL_NAME=m KIMI_MODEL_PROVIDER_TYPE=openai KIMI_MODEL_BASE_URL={at}/v1 KIMI_MODEL_API_KEY=x \
             KIMI_MODEL_MAX_CONTEXT_SIZE=100000 timeout 90 kimi -p hi"
        ),
        HarnessKind::QwenCode => {
            format!("OPENAI_BASE_URL={at}/v1 OPENAI_API_KEY=x OPENAI_MODEL=m timeout 90 qwen -p hi")
        }
        HarnessKind::AntigravityIde => String::new(),
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qcode_recommended_tells_each_harness_of_graphify_from_its_home_and_the_harness_reads_it() {
    // `QCODE_LIVE_HARNESS=<id>` runs one harness alone, for a second look at one that failed.
    let only = std::env::var("QCODE_LIVE_HARNESS").ok();
    for harness in
        HarnessKind::TERMINAL.into_iter().filter(|harness| only.as_deref().is_none_or(|id| id == harness.record().id))
    {
        let profile = profile(harness);
        for engine in engines() {
            let started = std::time::Instant::now();
            let lab = open(engine, &profile);
            // Taken away however the checks end.
            let _cleared = Cleared(&lab, &profile);
            let kind = lab.engine.kind();
            eprintln!(
                "{kind:?} {}: QCode recommended image built in {} s",
                harness.record().id,
                started.elapsed().as_secs()
            );
            assert!(lab.ok("graphify --version").starts_with("graphify "), "{kind:?}");
            // The section is in the file the harness reads from the home, and the file graphify
            // wrote in the home is gone.
            let user = super::guidance::user_file(harness);
            let text = lab.ok(&format!("cat \"$HOME/{user}\""));
            assert!(text.contains(GRAPHIFY_LINE), "{kind:?} {harness:?}: {text}");
            let left = super::guidance::graphify_file(harness);
            lab.fails(&format!("test -e \"$HOME/{left}\""));
            // Nothing is in the workspace folder of the image.
            assert_eq!(lab.ok("ls -A /work 2>/dev/null || true").trim(), "", "{kind:?} {harness:?}");
            // The hooks, where the harness has them, are in its own settings in the home.
            match harness {
                HarnessKind::ClaudeCode => {
                    let settings = lab.ok("cat \"$HOME/.claude/settings.json\"");
                    assert!(settings.contains("graphify hook-guard search"), "{kind:?}: {settings}");
                }
                HarnessKind::GeminiCli => {
                    let settings = lab.ok("cat \"$HOME/.gemini/settings.json\"");
                    assert!(settings.contains("graphify hook-guard gemini"), "{kind:?}: {settings}");
                }
                HarnessKind::OpenCode => {
                    let config = lab.ok("cd /work && opencode debug config 2>&1");
                    assert!(config.contains("/.config/opencode/plugins/graphify.js"), "{kind:?}: {config}");
                    lab.fails("test -e \"$HOME/.opencode\"");
                }
                _ => {}
            }
            // And the harness reads it: the line is in what it sends its provider.
            lab.ok(&format!("cat > /tmp/capture.py <<'PYTHON'\n{CAPTURE}PYTHON"));
            lab.ok("cd /tmp && (nohup python3 capture.py > /dev/null 2>&1 &) && sleep 2");
            let _ = lab.run(&format!("cd /work && {} > /tmp/said 2>&1", prompt(harness)));
            let sent = lab.ok("cat /tmp/captured 2>/dev/null || true");
            assert!(
                sent.contains(GRAPHIFY_LINE),
                "{kind:?} {harness:?}: the harness did not read {user}:\n{}",
                lab.ok("tail -c 2000 /tmp/said")
            );
        }
    }
}

/// Clears a lab's container and images when it goes, also when a check failed on the way.
struct Cleared<'a>(&'a Lab, &'a Profile);

impl Drop for Cleared<'_> {
    fn drop(&mut self) {
        clear(&self.0.engine, self.1, &self.0.container);
    }
}

/// A profile of `harness` under `template`, whose checks run without the network like every
/// profile here: what the build installed has to work from the image alone.
fn on(template: Template, harness: HarnessKind) -> Profile {
    Profile {
        name: SafeName::parse(&format!("harnesstest-{}-{}", template.id(), harness.record().id))
            .expect("the name is safe"),
        template,
        ..profile(harness)
    }
}

/// The size of `image` as the engine reports it, in whole megabytes, for the record of what a
/// template costs.
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

/// The engines a run is asked for: every one found, or only the one `QCODE_LIVE_ENGINE` names
/// (`podman` or `docker`), for a run that should not take twice as long.
fn chosen_engines() -> Vec<Engine> {
    let only = std::env::var("QCODE_LIVE_ENGINE").ok();
    engines()
        .into_iter()
        .filter(|engine| {
            only.as_deref().is_none_or(|name| match engine.kind() {
                EngineKind::Podman => name == "podman",
                EngineKind::Docker => name == "docker",
            })
        })
        .collect()
}

/// Builds the image of `profile` through the product's own recipe on every engine asked for,
/// checks what every QCode template gives — graphify answering from outside the home and
/// mapping code offline, its section where the harness reads it and the harness reading it —
/// then `more` for what only that harness gets, and says how long the build took and what it
/// weighs.
fn verify_template(profile: &Profile, more: impl Fn(&Lab)) {
    let harness = profile.harness;
    let template = profile.template;
    for engine in chosen_engines() {
        let started = std::time::Instant::now();
        let lab = open(engine, profile);
        // Taken away however the checks end.
        let _cleared = Cleared(&lab, profile);
        let kind = lab.engine.kind();
        eprintln!(
            "{kind:?} {} {}: image built in {} s, {} MB",
            template.id(),
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
            "{kind:?} {} {}: home directory of the image: {} MB",
            template.id(),
            harness.record().id,
            home.lines().last().unwrap_or("")
        );
        let sizes = lab.ok("du -sm /opt/pipx /usr/local/npm/lib/node_modules/* 2>/dev/null || true");
        eprintln!("{kind:?} {} {}: sizes in MB:\n{sizes}", template.id(), harness.record().id);
        // It maps code without asking anyone: the part of it that needs no model.
        lab.ok("mkdir -p /tmp/g && cd /tmp/g && printf 'def a():\\n    return b()\\n\\ndef b():\\n    return 1\\n' > m.py && graphify update . > /dev/null 2>&1 && test -s graphify-out/graph.json");

        // graphify's section: under QCode recommended the image carries it in the file the
        // harness reads from the home; under QCode extra it is written into the workspace when a
        // container comes up, by graphify's own installer run in `/work`, which is done here the
        // way the workspace does it (`plan::guidance`).
        let user = super::guidance::user_file(harness);
        let in_home = lab.ok(&format!("cat \"$HOME/{user}\" 2>/dev/null || true"));
        if template.carries_high() {
            assert!(!in_home.contains(GRAPHIFY_LINE), "{kind:?}: extra keeps it out of the home: {in_home}");
            lab.ok(&format!("cd /work && graphify {} install", super::guidance::platform(harness)));
        } else {
            assert!(in_home.contains(GRAPHIFY_LINE), "{kind:?}: {user}: {in_home}");
            assert_eq!(lab.ok("ls -A /work 2>/dev/null || true").trim(), "", "{kind:?}: nothing in the workspace");
        }
        // Its hooks, where the harness runs them.
        match harness {
            HarnessKind::ClaudeCode => {
                let settings = if template.carries_high() {
                    lab.ok("cat /work/.claude/settings.json")
                } else {
                    lab.ok("cat \"$HOME/.claude/settings.json\"")
                };
                assert!(settings.contains("graphify hook-guard search"), "{kind:?}: {settings}");
            }
            HarnessKind::OpenCode => {
                let config = lab.ok("cd /work && opencode debug config 2>&1");
                assert!(config.contains("plugins/graphify.js"), "{kind:?}: {config}");
            }
            _ => {}
        }
        // And the harness reads graphify's guidance: its heading is in what it sends its provider.
        lab.ok(&format!("cat > /tmp/capture.py <<'PYTHON'\n{CAPTURE}PYTHON"));
        lab.ok("cd /tmp && (nohup python3 capture.py > /dev/null 2>&1 &) && sleep 2");
        let _ = lab.run(&format!("cd /work && {} > /tmp/said 2>&1", prompt(harness)));
        let sent = lab.ok("cat /tmp/captured 2>/dev/null || true");
        assert!(
            sent.contains(GRAPHIFY_LINE),
            "{kind:?} {template:?} {harness:?}: the harness did not get graphify's guidance:\n{}",
            lab.ok("tail -c 2000 /tmp/said")
        );

        more(&lab);
    }
}

/// The plugins `claude plugin list` names, as `name@marketplace`, in the order it lists them.
fn listed_plugins(listed: &str) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("❯ "))
        .map(|plugin| plugin.trim().to_owned())
        .collect()
}

/// Checks that Claude Code of `profile` has exactly the plugins its template and switches give,
/// every one enabled, on top of the settings and first-start answers the template wrote.
fn claude_code_has_its_plugins(lab: &Lab, profile: &Profile) {
    let kind = lab.engine.kind();
    let listed = lab.ok("claude plugin list 2>&1");
    let mut found = listed_plugins(&listed);
    found.sort();
    let mut wanted: Vec<String> = profile.claude_plugins().into_iter().map(str::to_owned).collect();
    wanted.sort();
    assert_eq!(found, wanted, "{kind:?}: exactly the chosen plugins:\n{listed}");
    for plugin in &wanted {
        let at = listed.find(plugin.as_str()).unwrap_or_default();
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
    eprintln!(
        "{kind:?} {}: plugins in the home: {} MB",
        profile.template.id(),
        lab.ok("du -sm \"$HOME/.claude/plugins\" | cut -f1").trim()
    );
}

/// Checks that opencode loads oh-my-openagent from the image: its agents are opencode's own
/// agents, with no network and nothing downloaded into the home at start.
fn opencode_loads_oh_my_openagent(lab: &Lab) {
    let kind = lab.engine.kind();
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
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qcode_recommended_gives_claude_code_graphify_and_the_starter_plugins() {
    let profile = on(Template::Recommended, HarnessKind::ClaudeCode);
    verify_template(&profile, |lab| claude_code_has_its_plugins(lab, &profile));
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qcode_extra_gives_claude_code_graphify_and_every_plugin_with_one_switched_off_left_out() {
    // One plugin switched off, as the wizard writes it: the list must lack exactly that one.
    let profile = Profile {
        without: vec![super::Extra::Plugin("hookify@claude-plugins-official")],
        ..on(Template::High, HarnessKind::ClaudeCode)
    };
    assert_eq!(profile.claude_plugins().len(), super::CLAUDE_PLUGINS.len() - 1);
    verify_template(&profile, |lab| claude_code_has_its_plugins(lab, &profile));
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qcode_recommended_gives_opencode_graphify_and_loads_oh_my_openagent_from_the_image() {
    verify_template(&on(Template::Recommended, HarnessKind::OpenCode), opencode_loads_oh_my_openagent);
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn qcode_extra_gives_opencode_graphify_and_loads_oh_my_openagent_from_the_image() {
    verify_template(&on(Template::High, HarnessKind::OpenCode), opencode_loads_oh_my_openagent);
}

/// Checks that opencode loads oh-my-opencode-slim from the image, and nothing of
/// oh-my-openagent: the slim team's agents are opencode's own, the settings name the image's copy,
/// and nothing is downloaded into the home at start.
fn opencode_loads_oh_my_opencode_slim(lab: &Lab) {
    let kind = lab.engine.kind();
    let agents = lab.ok("cd /work && opencode agent list 2>&1");
    for agent in ["orchestrator", "explorer", "oracle", "librarian", "designer", "fixer"] {
        assert!(agents.contains(agent), "{kind:?}: opencode does not load oh-my-opencode-slim ({agent}):\n{agents}");
    }
    assert!(!agents.contains("Sisyphus"), "{kind:?}: oh-my-openagent's team is here too:\n{agents}");
    let config = lab.ok("cat \"$HOME/.config/opencode/opencode.json\"");
    assert!(config.contains("file:///usr/local/npm/lib/node_modules/oh-my-opencode-slim"), "{kind:?}: {config}");
    assert!(!config.contains("oh-my-openagent"), "{kind:?}: {config}");
    assert!(!lab.ok("ls /usr/local/npm/lib/node_modules").contains("oh-my-openagent"), "{kind:?}");
    let fetched = lab.ok("ls \"$HOME/.cache/opencode/packages\" 2>/dev/null || true");
    assert!(!fetched.contains("oh-my-opencode-slim"), "{kind:?}: a second copy was fetched: {fetched}");
    eprintln!(
        "{kind:?} slim: {} MB in the image",
        lab.ok("du -sm /usr/local/npm/lib/node_modules/oh-my-opencode-slim | cut -f1").trim()
    );
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn oh_my_opencode_slim_gives_opencode_graphify_and_loads_its_team_from_the_image() {
    verify_template(&on(Template::Slim, HarnessKind::OpenCode), opencode_loads_oh_my_opencode_slim);
}
