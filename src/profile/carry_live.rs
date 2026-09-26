//! That a login made once, in the profile wizard, signs the harness in in a workspace made
//! afterwards, shown on the harness itself: the profile's image built by the product's own recipe,
//! the wizard's login container, its take-out into the credentials volume, and a new workspace's
//! first tab started by the line a tab starts it with, in a real terminal.
//!
//! A real sign-in needs an account, so the files a sign-in leaves are written into the login
//! container by hand, the way the harness writes them: Claude Code 2.1.282's `.credentials.json`
//! under `claudeAiOauth`, and in `~/.claude.json` the account and the end of its first-start
//! questions (`oauthAccount`, `hasCompletedOnboarding`), which it writes there in the same step;
//! opencode's `auth.json` keyed by provider. Everything after that is the product's code.
//!
//! The workspace's container has no network: the harness then shows what it holds without asking
//! anyone, so what comes up is what the copy gave it and nothing a server said.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test carry_live -- --ignored --test-threads=1
//! ```
//!
//! `QCODE_CONTAINER_ENGINE=podman` keeps one engine. Everything made is named after `carrytest`
//! and removed again.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use qframe::runtime::Harness as Screen;
use qframe::widgets::TerminalSession;

use super::harness_live::{build, clear};
use super::identity::Home;
use super::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect, names};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::profiles::work;
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// The workspace made after the login, which nobody has.
const WORKSPACE: &str = "carrytest";

/// The account the stand-in sign-in is for. Nothing but the copy can put it where the harness
/// reads it in the workspace.
const EMAIL: &str = "carry@qcode.test";

/// Claude Code's login as 2.1.282 writes it at the end of a sign-in with a subscription: the
/// tokens under `claudeAiOauth`, with the plan the account is on, which is what its header shows.
const CLAUDE_CREDENTIALS: &str = r#"{"claudeAiOauth":{"accessToken":"carrytest-access-not-a-token","refreshToken":"carrytest-refresh-not-a-token","expiresAt":4102444800000,"scopes":["user:inference","user:profile","user:sessions:claude_code"],"subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}"#;

/// What the same sign-in merges into `~/.claude.json`, as a node program run in the login
/// container: the account, and the flag that closes the first-start questions the sign-in was one
/// of. Every key already in the file stays, as the harness keeps them.
const CLAUDE_ACCOUNT: &str =
    "const fs = require('fs'); const p = require('path').join(process.env.HOME, '.claude.json');
let d = {}; try { d = JSON.parse(fs.readFileSync(p, 'utf8')); } catch (e) {}
d.hasCompletedOnboarding = true;
d.lastOnboardingVersion = '2.1.282';
d.userID = 'c0ffee';
d.oauthAccount = { accountUuid: '11111111-2222-3333-4444-555555555555', emailAddress: process.argv[1],
  organizationUuid: '66666666-7777-8888-9999-000000000000', organizationName: 'Carry', displayName: 'Carry' };
fs.writeFileSync(p, JSON.stringify(d, null, 2), { mode: 0o600 });";

/// The words of Claude Code's first-start questions and of its sign-in, any of which on the tab
/// means the workspace was not given the login: the text style opens the questions, and without
/// the network it stops at the check that it can reach its server, which only comes with them.
const ASKED_TO_SIGN_IN: [&str; 4] = [
    "Select login method",
    "Choose the text style",
    "Unable to connect to Anthropic services",
    "Welcome to Claude Code",
];

/// Words Claude Code draws once its prompt is up.
const PROMPT: &str = "shift+tab to cycle";

/// The answer that trusts the folder, on the question Claude Code asks about a folder it has not
/// been told about. A home made from an image with no settings of QCode's has not been told.
const TRUST: &str = "Yes, I trust this folder";

/// The engines installed on this machine, or nothing at all when the tests are switched off.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let only = std::env::var("QCODE_CONTAINER_ENGINE").ok();
    let found: Vec<Engine> = [EngineKind::Podman, EngineKind::Docker]
        .into_iter()
        .filter(|kind| only.as_deref().is_none_or(|only| format!("{kind:?}").eq_ignore_ascii_case(only)))
        .filter_map(|kind| detect(kind).ok())
        .collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// A profile of `harness` under `template` on a subscription, whose containers have no network.
fn profile(harness: HarnessKind, template: Template) -> Profile {
    Profile {
        name: SafeName::parse(&format!("carrytest-{}-{}", harness.record().id, template.id())).expect("safe"),
        harness,
        template,
        account: AccountKind::Subscription,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// A workspace folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-carrylive-{name}-{stamp}"));
        std::fs::create_dir_all(path.join("Work")).expect("a workspace folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    fn paths(&self) -> WorkspacePaths {
        WorkspacePaths {
            root: self.0.clone(),
            file: self.0.join("workspace.qcode"),
            code: self.0.join("Work"),
            assets: self.0.join("Assets"),
            harness: self.0.join("Containers").join("Harness"),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A screen that draws nothing but one tab's terminal, the widget a tab draws with.
struct Watched(TerminalSession);

impl qframe::prelude::App for Watched {
    type Msg = ();

    fn update(&mut self, (): ()) -> qframe::prelude::Command<()> {
        qframe::prelude::Command::none()
    }

    fn view(&self, ui: &mut qframe::prelude::View<'_, ()>) {
        ui.add(qframe::widgets::Terminal::new(&self.0).read_only()).fill_width().fill_height();
    }
}

/// Everything one run makes, taken away again when it ends, whatever happened.
struct Made {
    engine: Engine,
    profile: Profile,
    plan: ContainerPlan,
    home: Home,
    scratch: Scratch,
}

impl Made {
    fn run(&self, script: &str) -> String {
        let kind = self.engine.kind();
        capture(
            &self.engine.exec_without_terminal(&Exec { container: &self.plan.name, command: &["sh", "-c", script] }),
        )
        .unwrap_or_else(|error| panic!("{kind:?}: `{script}`: {error:?}"))
    }
}

impl Drop for Made {
    fn drop(&mut self) {
        let _ = capture(&self.engine.remove_container(&self.plan.name));
        let _ = capture(&self.engine.remove_volume(&self.home.volume()));
        let _ = capture(&self.engine.remove_volume(&names::credential_volume(self.profile.name.as_str())));
        clear(&self.engine, &self.profile, &self.plan.name);
    }
}

/// Builds `profile`'s image, signs it in through the wizard's own login container with `sign_in`
/// standing in for the harness (its positional words `given`), stores the login the way the wizard does, and then brings up the
/// container of a workspace made afterwards the way its first tab does.
fn signed_in_then_new_workspace(engine: Engine, profile: Profile, sign_in: &str, given: &[&str]) -> Made {
    let kind = engine.kind();
    let scratch = Scratch::new(&format!("{}-{}", profile.harness.record().id, profile.template.id()));
    let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
    let plan = ContainerPlan::profile(&workspace, &scratch.paths(), &profile);
    let home = Home::new(profile.name.clone(), workspace);
    let _ = capture(&engine.remove_container(&plan.name));
    let _ = capture(&engine.remove_volume(&home.volume()));
    let _ = capture(&engine.remove_volume(&names::credential_volume(profile.name.as_str())));
    build(&engine, &profile);
    let made = Made { engine, profile, plan, home, scratch };

    let login = work::open_login(&made.engine, &made.profile).unwrap_or_else(|problem| panic!("{kind:?}: {problem:?}"));
    let mut words = vec!["sh", "-c", sign_in, "sh"];
    words.extend_from_slice(given);
    let wrote = capture(&made.engine.exec_without_terminal(&Exec { container: &login.name, command: &words }));
    let stored = wrote
        .map_err(|error| format!("{error:?}"))
        .and_then(|_| work::store_login(&made.engine, &made.profile, &login).map_err(|problem| format!("{problem:?}")));
    work::close_login(&made.engine, &login);
    stored.unwrap_or_else(|problem| panic!("{kind:?}: the login is stored: {problem}"));

    let user = HostUser::current().expect("the current user");
    ensure_running(&made.engine, &made.plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
    made
}

/// The shell that stands in for Claude Code's sign-in in the login container, given
/// [`CLAUDE_ACCOUNT`] and the e-mail after it.
fn claude_sign_in() -> String {
    format!(
        "set -e; mkdir -p \"$HOME/.claude\"; umask 077; \
         printf '%s' '{CLAUDE_CREDENTIALS}' > \"$HOME/.claude/.credentials.json\"; node -e \"$1\" \"$2\""
    )
}

/// Claude Code's whole check under `template`, on every engine.
fn claude_code_is_signed_in_in_a_new_workspace(template: Template) {
    for engine in engines() {
        let kind = engine.kind();
        let made = signed_in_then_new_workspace(
            engine,
            profile(HarnessKind::ClaudeCode, template),
            &claude_sign_in(),
            &[CLAUDE_ACCOUNT, EMAIL],
        );

        // The tab: started the way a tab starts it, it comes up at its prompt on the plan the
        // login is on, and never on a question that belongs to signing in.
        let line = HarnessKind::ClaudeCode.command_line(None);
        let words: Vec<&str> = line.iter().map(String::as_str).collect();
        let command = made.plan.enter(&made.engine, &words);
        let session = TerminalSession::spawn(command.program.as_os_str(), &command.args, &made.scratch.0)
            .expect("a pseudo-terminal for the tab");
        let mut screen = Screen::new(Watched(session.clone()), 120, 36);
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut trusted = false;
        let drawn = loop {
            screen.render();
            let drawn = screen.screen();
            if let Some(words) = ASKED_TO_SIGN_IN.iter().find(|words| drawn.contains(*words)) {
                session.kill();
                panic!("{kind:?} {template:?}: the new workspace asked to sign in again (`{words}`):\n{drawn}");
            }
            if drawn.contains(TRUST) && !trusted {
                // A question about the folder, not the login: answered the way a person would.
                // The keys wait until the question reads them, and Return waits until the arrow
                // is seen on the answer, since the one highlighted at first leaves.
                trusted = true;
                std::thread::sleep(Duration::from_secs(2));
                session.write(b"\x1b[B").expect("the key reaches the tab");
                let chosen = Instant::now() + Duration::from_secs(20);
                loop {
                    screen.render();
                    if screen.screen().contains(&format!("\u{276f} {TRUST}")) || Instant::now() > chosen {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                session.write(b"\r").expect("the key reaches the tab");
            }
            if drawn.contains(PROMPT) || Instant::now() > deadline {
                break drawn;
            }
            std::thread::sleep(Duration::from_millis(250));
        };
        session.kill();
        assert!(drawn.contains(PROMPT), "{kind:?} {template:?}: no prompt came up:\n{drawn}");
        assert!(drawn.contains("Claude Max"), "{kind:?} {template:?}: the plan of the login is not shown:\n{drawn}");
        // And what the harness itself says about the login it finds in the workspace's home.
        let status = made.run("claude auth status 2>&1 || true");
        assert!(status.contains("\"loggedIn\": true"), "{kind:?} {template:?}: {status}");
        assert!(status.contains(EMAIL), "{kind:?} {template:?}: the account did not come along:\n{status}");
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn a_claude_code_login_made_in_the_wizard_signs_in_a_new_workspace_of_a_profile_on_base() {
    claude_code_is_signed_in_in_a_new_workspace(Template::Base);
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn a_claude_code_login_made_in_the_wizard_signs_in_a_new_workspace_of_a_profile_on_qcode_basic() {
    claude_code_is_signed_in_in_a_new_workspace(Template::Recommended);
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn an_opencode_login_made_in_the_wizard_signs_in_a_new_workspace() {
    let sign_in = "set -e; login=\"$HOME/.local/share/opencode/auth.json\"; mkdir -p \"$(dirname \"$login\")\"; \
                   umask 077; printf '%s' '{\"anthropic\":{\"type\":\"oauth\",\"refresh\":\"r-carry\",\"access\":\"a-carry\",\"expires\":4102444800000}}' > \"$login\"";
    for engine in engines() {
        let kind = engine.kind();
        let made = signed_in_then_new_workspace(engine, profile(HarnessKind::OpenCode, Template::Base), sign_in, &[]);
        let listed = made.run("opencode providers list 2>&1");
        assert!(listed.contains("1 credentials"), "{kind:?}: the login did not reach the workspace:\n{listed}");
        assert!(listed.to_lowercase().contains("anthropic"), "{kind:?}: {listed}");
    }
}
