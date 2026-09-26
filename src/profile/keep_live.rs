//! What a person installs stays where they installed it, shown on a real engine: a profile's image
//! built by the product's own recipe, a workspace's container brought up by `ensure_running`, and
//! the container made again the way a changed plan makes it.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test keep_live -- --ignored --test-threads=1
//! ```
//!
//! `QCODE_CONTAINER_ENGINE=podman` keeps one engine. Everything made is named after `keeptest`
//! and removed again.

use std::path::PathBuf;

use super::identity::Home;
use super::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::profiles::work;
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// The engines installed on this machine, or nothing at all when the tests are switched off.
pub(crate) fn engines() -> Vec<Engine> {
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

/// A Claude Code profile on the base template, which reaches the network.
pub(crate) fn profile(name: &str) -> Profile {
    Profile {
        name: SafeName::parse(name).expect("safe"),
        harness: HarnessKind::ClaudeCode,
        template: Template::Base,
        account: AccountKind::Subscription,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// Builds `profile`'s image the way the wizard does.
pub(crate) fn build(engine: &Engine, profile: &Profile) {
    let _ = capture(&engine.remove_image(&profile.image()));
    let mut said = String::new();
    let built = work::build_whole(engine, profile, &|| false, &mut || {}, &mut |line| {
        said.push_str(line);
        said.push('\n');
    });
    assert!(built.is_ok(), "{:?}: the profile's image did not build: {built:?}\n{said}", engine.kind());
}

/// A workspace folder of a test's own, removed when the test ends.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-keeplive-{name}-{stamp}"));
        std::fs::create_dir_all(path.join("Work")).expect("a workspace folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    pub(crate) fn paths(&self) -> WorkspacePaths {
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

/// One workspace's container of a profile, and everything that goes with it when the test ends.
pub(crate) struct Workspace {
    pub(crate) engine: Engine,
    pub(crate) id: WorkspaceId,
    pub(crate) scratch: Scratch,
}

impl Workspace {
    pub(crate) fn new(engine: &Engine, id: &str) -> Self {
        Self { engine: engine.clone(), id: WorkspaceId::parse(id).expect("an id"), scratch: Scratch::new(id) }
    }

    pub(crate) fn plan(&self, profile: &Profile) -> ContainerPlan {
        ContainerPlan::profile(&self.id, &self.scratch.paths(), profile)
    }

    /// Brings the container of `profile` up the way a tab does.
    pub(crate) fn up(&self, profile: &Profile) -> ContainerPlan {
        let plan = self.plan(profile);
        let user = HostUser::current().expect("the current user");
        ensure_running(&self.engine, &plan, user)
            .unwrap_or_else(|failure| panic!("{:?}: {failure:?}", self.engine.kind()));
        plan
    }

    /// Runs `script` in the container of `plan` as the person, and says what it printed; a
    /// failure is `Err` with the engine's words.
    pub(crate) fn run(&self, plan: &ContainerPlan, script: &str) -> Result<String, String> {
        capture(&self.engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
            .map_err(|error| format!("{error:?}"))
    }

    pub(crate) fn clear(&self, profile: &Profile) {
        let plan = self.plan(profile);
        let _ = capture(&self.engine.remove_container(&plan.name));
        let _ = capture(&self.engine.remove_volume(&Home::new(profile.name.clone(), self.id.clone()).volume()));
    }
}

/// What `npm -g`, `pipx` and `uv tool` install for the person's user is still there once the
/// container has been made again for a changed plan.
#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn what_a_person_installs_for_their_user_stays_when_the_plan_changes() {
    for engine in engines() {
        let kind = engine.kind();
        let mut profile = profile("keeptest-user");
        let workspace = Workspace::new(&engine, "keeptest");
        workspace.clear(&profile);
        build(&engine, &profile);
        let plan = workspace.up(&profile);

        let installed =
            workspace.run(&plan, "npm install -g --no-fund --no-audit semver@7.7.2 >/dev/null 2>&1; command -v semver");
        assert_eq!(
            installed.as_deref().map(str::trim),
            Ok("/home/qcode/.local/bin/semver"),
            "{kind:?}: npm -g does not install into the home"
        );
        let pipx = workspace.run(&plan, "command -v pipx").is_ok();
        if pipx {
            let installed = workspace.run(&plan, "pipx install pycowsay==0.0.0.2 >/dev/null 2>&1; command -v pycowsay");
            assert_eq!(installed.as_deref().map(str::trim), Ok("/home/qcode/.local/bin/pycowsay"), "{kind:?}");
        }
        // Something only in the container's own layer, which tells a container made again from one
        // that was only restarted.
        workspace.run(&plan, "echo here > /var/tmp/keeptest-layer").expect("the layer is written");

        // The network is switched off: the plan changes, and the stopped container is made again.
        let _ = capture(&engine.stop_container(&plan.name));
        profile.network = NetworkMode::None;
        let plan = workspace.up(&profile);
        assert!(
            workspace.run(&plan, "test -e /var/tmp/keeptest-layer").is_err(),
            "{kind:?}: the container was not made again"
        );

        let version = workspace.run(&plan, "semver --help | head -1");
        let clear = || {
            workspace.clear(&profile);
            let _ = capture(&engine.remove_image(&profile.image()));
        };
        let kept = version.as_deref().is_ok_and(|said| said.contains("SemVer 7.7.2"));
        let cow = !pipx || workspace.run(&plan, "pycowsay moo").is_ok_and(|said| said.contains("moo"));
        clear();
        assert!(kept, "{kind:?}: npm -g went with the container: {version:?}");
        assert!(cow, "{kind:?}: pipx went with the container");
        println!("{kind:?}: npm -g kept; pipx {}", if pipx { "kept" } else { "not in the image" });
    }
}

/// Types `lines` into `session` as a person would, then `exit` until the shell ends, over a
/// generous but finite while. `exit` is typed again and again because a program still running,
/// such as `apt-get`, reads what is typed while it runs and would swallow a single one.
fn type_and_wait_for_the_end(session: &qframe::widgets::TerminalSession, lines: &[&str], within: std::time::Duration) {
    std::thread::sleep(std::time::Duration::from_secs(2));
    for line in lines {
        session.write(format!("{line}\r").as_bytes()).expect("the keys reach the shell");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    // The session learns of its program's end by being watched, as a tab watches it.
    let watch = session.watch();
    let deadline = std::time::Instant::now() + within;
    let mut asked = std::time::Instant::now();
    session.write(b"exit\r").expect("the keys reach the shell");
    while session.exit().is_none() {
        assert!(std::time::Instant::now() < deadline, "the shell did not end within {within:?}");
        let _ = watch.next_change_within(std::time::Duration::from_millis(500));
        if asked.elapsed() > std::time::Duration::from_secs(5) && session.exit().is_none() {
            let _ = session.write(b"exit\r");
            asked = std::time::Instant::now();
        }
    }
}

/// What is done in a profile's shell and added — a program installed as the administrator, a file
/// written in the home — is in every workspace made afterwards, and still there after a rebuild;
/// what is discarded never reaches the image.
#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn what_is_added_in_a_profiles_shell_reaches_new_workspaces_and_survives_a_rebuild() {
    use crate::ui::profiles::shell;
    use qframe::widgets::TerminalSession;
    use std::time::Duration;

    for engine in engines() {
        let kind = engine.kind();
        let profile = profile("keeptest-shell");
        let store = Scratch::new("store");
        let profiles = store.0.join("Profiles");
        let first = Workspace::new(&engine, "keeptest-one");
        let second = Workspace::new(&engine, "keeptest-two");
        let clear = || {
            first.clear(&profile);
            second.clear(&profile);
            shell::discard(&engine, &profile.name);
            let _ = capture(&engine.remove_image(&profile.image()));
            forget_unnamed(&engine, &profile);
        };
        clear();
        build(&engine, &profile);
        let user = HostUser::current().expect("the current user");

        // The shell, as its page opens it: the administrator installs a program, the person writes
        // a note in the home, each in a real terminal.
        let container = fresh(shell::open(&engine, &profile, user), kind);
        let admin = shell::enter(&engine, &container, true);
        let admin = TerminalSession::spawn(admin.program.as_os_str(), &admin.args, &store.0).expect("a terminal");
        type_and_wait_for_the_end(
            &admin,
            &["apt-get update -qq && apt-get install -y -qq jq >/dev/null"],
            Duration::from_secs(600),
        );
        let you = shell::enter(&engine, &container, false);
        let you = TerminalSession::spawn(you.program.as_os_str(), &you.args, &store.0).expect("a terminal");
        type_and_wait_for_the_end(&you, &["echo kept-by-the-profile > ~/notes.txt"], Duration::from_secs(60));

        let session = shell::survey(&engine, &profile.name).unwrap_or_else(|problem| panic!("{kind:?}: {problem:?}"));
        assert_eq!(session.steps, ["apt-get update -qq && apt-get install -y -qq jq >/dev/null"], "{kind:?}");
        assert!(session.changes.home.contains(&"notes.txt".to_owned()), "{kind:?}: {:?}", session.changes);
        assert!(session.changes.system.iter().any(|path| path == "/usr/bin/jq"), "{kind:?}: {:?}", session.changes);
        shell::add(&engine, &profile, &profiles, session.steps)
            .unwrap_or_else(|problem| panic!("{kind:?}: {problem:?}"));

        let plan = first.up(&profile);
        let jq = first.run(&plan, "command -v jq");
        let note = first.run(&plan, "cat ~/notes.txt");

        // A rebuild starts from the recipe and puts both back.
        let rebuilt =
            crate::ui::profiles::work::rebuild(&engine, &profile, &profiles, &|| false, &mut || {}, &mut |_| {});
        let plan_two = second.up(&profile);
        let jq_two = second.run(&plan_two, "command -v jq");
        let note_two = second.run(&plan_two, "cat ~/notes.txt");

        // Discarded work never reaches the image.
        let before = capture(&engine.image_exists(&profile.image())).unwrap_or_default();
        let container = fresh(shell::open(&engine, &profile, user), kind);
        let _ = capture(&engine.exec_as_root_without_terminal(&Exec {
            container: &container,
            command: &["sh", "-c", "touch /usr/local/bin/discarded"],
        }));
        // QCode ends with the shell open, and the shell is opened again: the work is still there,
        // and it is said rather than removed.
        let _ = capture(&engine.stop_container(&container));
        let left = shell::open(&engine, &profile, user);
        let still = capture(&engine.exec_as_root_without_terminal(&Exec {
            container: &container,
            command: &["sh", "-c", "test -e /usr/local/bin/discarded"],
        }));
        shell::discard(&engine, &profile.name);
        let after = capture(&engine.image_exists(&profile.image())).unwrap_or_default();
        clear();

        assert_eq!(
            jq.as_deref().map(str::trim),
            Ok("/usr/bin/jq"),
            "{kind:?}: the program did not reach a new workspace"
        );
        assert_eq!(note.as_deref().map(str::trim), Ok("kept-by-the-profile"), "{kind:?}: the home's file did not");
        assert!(rebuilt.is_ok(), "{kind:?}: {rebuilt:?}");
        assert_eq!(jq_two.as_deref().map(str::trim), Ok("/usr/bin/jq"), "{kind:?}: the rebuild lost the program");
        assert_eq!(
            note_two.as_deref().map(str::trim),
            Ok("kept-by-the-profile"),
            "{kind:?}: the rebuild lost the file"
        );
        match &left {
            Ok(shell::Opened::Leftover(name, session)) => {
                assert_eq!(name, &container, "{kind:?}");
                assert!(
                    session.changes.system.iter().any(|path| path == "/usr/local/bin/discarded"),
                    "{kind:?}: {session:?}"
                );
            }
            other => panic!("{kind:?}: the shell left open was not said: {other:?}"),
        }
        assert!(still.is_ok(), "{kind:?}: the work of the shell left open went without a word: {still:?}");
        assert_eq!(before, after, "{kind:?}: discarding changed the image");
        println!("{kind:?}: added, new workspace has both, rebuild has both, discard leaves the image");
    }
}

/// What the administrator installs in a workspace stays in that workspace when its container is
/// made again for a changed plan, and reaches neither another workspace of the same profile nor
/// the profile's image; a rebuild of the profile's image installs it again on the new one.
#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn what_the_administrator_installs_in_a_workspace_stays_there_and_nowhere_else() {
    use crate::ui::workspace::{WORKSPACE_BASE_LABEL, ensure_running_noting, enter_admin, workspace_image};
    use qframe::widgets::TerminalSession;
    use std::time::Duration;

    for engine in engines() {
        let kind = engine.kind();
        let mut profile = profile("keeptest-admin");
        let store = Scratch::new("admin-store");
        let one = Workspace::new(&engine, "keeptest-a");
        let two = Workspace::new(&engine, "keeptest-b");
        let own = workspace_image("keeptest-a", profile.name.as_str());
        let clear = |profile: &Profile| {
            one.clear(profile);
            two.clear(profile);
            let _ = capture(&engine.remove_image(&own));
            let _ = capture(&engine.remove_image(&workspace_image("keeptest-b", profile.name.as_str())));
            let _ = capture(&engine.remove_image(&profile.image()));
            forget_unnamed(&engine, profile);
        };
        clear(&profile);
        build(&engine, &profile);
        let plan = one.up(&profile);

        // As administrator, the way the tab starts it, in a real terminal.
        let admin = enter_admin(&engine, &plan);
        let admin = TerminalSession::spawn(admin.program.as_os_str(), &admin.args, &store.0).expect("a terminal");
        type_and_wait_for_the_end(
            &admin,
            &["apt-get update -qq && apt-get install -y -qq jq >/dev/null"],
            Duration::from_secs(600),
        );
        // The digest of the plan a container was made from, which tells a container made again
        // from one only restarted: what is in its layer is kept now, so the layer cannot tell.
        let made_from = |plan: &crate::ui::workspace::ContainerPlan| {
            capture(&engine.container_label(&plan.name, crate::ui::workspace::PLAN_LABEL)).unwrap_or_default()
        };
        let first_plan = made_from(&plan);

        // The plan changes: the container is made again, and keeps what was installed.
        let _ = capture(&engine.stop_container(&plan.name));
        profile.network = NetworkMode::None;
        let plan = one.up(&profile);
        let remade = made_from(&plan) != first_plan;
        let kept = one.run(&plan, "command -v jq");
        let other = two.up(&profile);
        let elsewhere = two.run(&other, "command -v jq");
        let workspace_image_of_two =
            capture(&engine.image_exists(&workspace_image("keeptest-b", profile.name.as_str())));

        // A rebuild of the profile's image: the recorded command runs again on the new one.
        profile.network = NetworkMode::Full;
        let profiles = store.0.join("Profiles");
        let rebuilt =
            crate::ui::profiles::work::rebuild(&engine, &profile, &profiles, &|| false, &mut || {}, &mut |_| {});
        let _ = capture(&engine.stop_container(&plan.name));
        let user = HostUser::current().expect("the current user");
        let again = ensure_running_noting(&engine, &one.plan(&profile), user);
        let after_rebuild = one.run(&one.plan(&profile), "command -v jq");
        let base = capture(&engine.image_label(&own, WORKSPACE_BASE_LABEL)).unwrap_or_default();
        let now = capture(&engine.image_exists(&profile.image())).unwrap_or_default();
        let in_profile = two.run(&other, "command -v jq");
        clear(&profile);

        assert!(remade, "{kind:?}: the container was not made again");
        assert_eq!(kept.as_deref().map(str::trim), Ok("/usr/bin/jq"), "{kind:?}: jq went with the container");
        assert!(elsewhere.is_err(), "{kind:?}: another workspace got jq: {elsewhere:?}");
        assert!(in_profile.is_err(), "{kind:?}: the profile's image got jq");
        assert!(workspace_image_of_two.is_err(), "{kind:?}: the other workspace has an image of its own");
        assert!(rebuilt.is_ok(), "{kind:?}: {rebuilt:?}");
        assert_eq!(again, Ok(None), "{kind:?}: the command did not run again on the rebuilt image");
        assert_eq!(after_rebuild.as_deref().map(str::trim), Ok("/usr/bin/jq"), "{kind:?}: jq went with the rebuild");
        assert_eq!(base.trim(), now.trim(), "{kind:?}: the workspace's image is not made on the rebuilt one");
        println!("{kind:?}: kept in the workspace, not in another or the profile, installed again after a rebuild");
    }
}

/// The name of a shell's container that `open` made anew; anything else fails the test.
fn fresh(
    opened: Result<crate::ui::profiles::shell::Opened, crate::ui::profiles::work::Problem>,
    kind: crate::engine::EngineKind,
) -> String {
    match opened {
        Ok(crate::ui::profiles::shell::Opened::Fresh(name)) => name,
        other => panic!("{kind:?}: a new shell was not opened: {other:?}"),
    }
}

/// Removes the images of `profile` that lost their name while a container still held them: an
/// Add or a rebuild replaced them while a workspace's container was made from them, and the test
/// removed that container itself rather than letting QCode make it again.
fn forget_unnamed(engine: &Engine, profile: &Profile) {
    let program = engine.remove_image(&profile.image()).program;
    let label = format!("label=qcode.profile={}", profile.name);
    let listed = std::process::Command::new(&program)
        .args(["images", "--quiet", "--filter", "dangling=true", "--filter", &label])
        .output();
    for id in listed.map(|out| String::from_utf8_lossy(&out.stdout).into_owned()).unwrap_or_default().split_whitespace()
    {
        let _ = capture(&engine.remove_image(id));
    }
}
