//! The one thing about the workspace screen no argument list can prove: that what a tab opens
//! really is inside a container.
//!
//! The tests here are `#[ignore]`d and do nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They pull `alpine` the first time, so
//! the first run needs the network. They never touch a real QCode image, workspace or profile:
//! the container they make is named after a workspace nobody has.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```

use std::path::{Path, PathBuf};

use qframe::widgets::{TerminalEvent, TerminalSession};

use crate::engine::names::HOSTNAME;
use crate::engine::run::capture;
use crate::engine::{Access, Engine, EngineKind, Exec, HostUser, Network, detect};

use super::plan::{ContainerPlan, SHELL};

/// The image the test container is made from, named in full: podman refuses a short name without
/// a terminal to ask at.
const ALPINE: &str = "docker.io/library/alpine:3";

/// The container this test lives in, named after a workspace nobody has.
const CONTAINER: &str = "qcode-uiworkspacelive-base";

/// A second container of the same kind, for the check that two of them share one machine name.
const TWIN: &str = "qcode-uiworkspacelive-twin";

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

/// A workspace folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-uilive-{stamp}"));
        std::fs::create_dir_all(path.join("Work")).expect("a workspace folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    fn plan(&self, name: &str) -> ContainerPlan {
        ContainerPlan {
            name: name.to_owned(),
            image: ALPINE.to_owned(),
            home: None,
            browser: None,
            code: self.0.join("Work"),
            assets: self.0.join("Assets"),
            assets_access: Access::ReadWrite,
            network: Network::Full,
            window: None,
            bridge: None,
            guidance: None,
            graphify: false,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn what_a_tab_opens_runs_in_the_container_and_not_on_this_machine() {
    for engine in engines() {
        let scratch = Scratch::new();
        let plan = scratch.plan(CONTAINER);
        let _ = capture(&engine.remove_container(CONTAINER));

        let user = HostUser::current().expect("the current user");
        capture(&plan.create(&engine, user)).expect("the container is made");
        capture(&engine.start_container(CONTAINER)).expect("the container starts");

        // Exactly the command a tab spawns, in exactly the way a tab spawns it.
        let command = plan.enter(&engine, SHELL);
        let session =
            TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0).expect("a pseudo-terminal");
        session.write(b"hostname > /work/where\nexit\n").expect("the shell reads its input");
        let watch = session.watch();
        while !matches!(watch.next(), TerminalEvent::Exited(_)) {}

        let written = std::fs::read_to_string(scratch.0.join("Work").join("where"))
            .expect("the shell wrote through the workspace mount");
        let inside = written.trim().to_owned();
        assert!(!inside.is_empty(), "the shell answered from somewhere");
        assert_ne!(inside, this_machine(), "the shell was not this machine's: {inside}");

        capture(&engine.remove_container(CONTAINER)).expect("the container is removed");
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn every_container_of_the_plan_answers_to_the_same_machine_name() {
    for engine in engines() {
        let scratch = Scratch::new();
        let user = HostUser::current().expect("the current user");
        let mut answered = Vec::new();
        for name in [CONTAINER, TWIN] {
            let _ = capture(&engine.remove_container(name));
            capture(&scratch.plan(name).create(&engine, user)).expect("the container is made");
            capture(&engine.start_container(name)).expect("the container starts");
            let inside = capture(&engine.exec_without_terminal(&Exec { container: name, command: &["hostname"] }))
                .expect("the container says its machine name");
            answered.push(inside.trim().to_owned());
            capture(&engine.remove_container(name)).expect("the container is removed");
        }
        // Not the engine's random name, and not this machine's either: the one QCode fixed, so a
        // login keyed to the machine name decrypts in whichever container it is copied into.
        assert_eq!(answered, [HOSTNAME, HOSTNAME], "{:?}", engine.kind());
    }
}

/// The container and home of the check that a container made from an older plan is made again.
const REMADE: &str = "qcode-uiworkspacelive-remade";

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_stopped_container_of_an_older_plan_is_made_again_with_the_bridge_and_keeps_its_home() {
    use crate::base::paths::{HOME_DIR, MCP_DIR};
    use crate::engine::names::BASE_IMAGE;
    use crate::profile::identity::Home;
    use crate::profile::{HarnessKind, SafeName};
    use crate::store::WorkspaceId;

    use super::plan::{Bridge, PLAN_LABEL, ensure_running};

    for engine in engines() {
        crate::base::ensure(&engine, &|| false, &mut |_| {}).expect("the base image is there");
        let scratch = Scratch::new();
        let user = HostUser::current().expect("the current user");
        let home = Home::new(
            SafeName::parse("remade").expect("a safe name"),
            WorkspaceId::parse("uiworkspacelive").expect("a workspace id"),
        );
        let _ = capture(&engine.remove_container(REMADE));
        let _ = capture(&engine.remove_volume(&home.volume()));
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: REMADE, command: &["sh", "-c", script] }))
        };

        // A container as QCode made it before the bridge: no mount of Containers/MCP.
        let older = ContainerPlan { image: BASE_IMAGE.to_owned(), home: Some(home.clone()), ..scratch.plan(REMADE) };
        ensure_running(&engine, &older, user).expect("the older container comes up");
        run(&format!("echo kept > {HOME_DIR}/marker")).expect("the home is written");
        capture(&engine.stop_container(REMADE)).expect("the container stops");

        let folder = scratch.0.join("Containers").join("MCP");
        let newer = ContainerPlan {
            bridge: Some(Bridge { folder: folder.clone(), harness: HarnessKind::ClaudeCode }),
            ..older.clone()
        };
        assert_ne!(older.digest(&engine, user), newer.digest(&engine, user), "the bridge changes the plan");
        ensure_running(&engine, &newer, user).expect("the newer container comes up");
        let label = capture(&engine.container_label(REMADE, PLAN_LABEL)).expect("the label is read");
        assert_eq!(label.trim(), newer.digest(&engine, user), "{:?}: the container was made again", engine.kind());
        assert_eq!(run(&format!("cat {HOME_DIR}/marker")).expect("the home is read").trim(), "kept");
        std::fs::write(folder.join("probe"), "from the host").expect("the folder was made as the person");
        assert_eq!(run(&format!("cat {MCP_DIR}/probe")).expect("the bridge is mounted").trim(), "from the host");
        assert!(run(&format!("touch {MCP_DIR}/written")).is_err(), "{:?}: the bridge is read-only", engine.kind());

        // Running, a container is never made again: tabs work in it.
        ensure_running(&engine, &older, user).expect("the running container is left as it is");
        let label = capture(&engine.container_label(REMADE, PLAN_LABEL)).expect("the label is read");
        assert_eq!(label.trim(), newer.digest(&engine, user));

        // Stopped and asked for with the same plan, it is only started: what was written into the
        // container itself is still there.
        run("echo same > /tmp/layer").expect("the container's own files are written");
        capture(&engine.stop_container(REMADE)).expect("the container stops");
        ensure_running(&engine, &newer, user).expect("the container starts again");
        assert_eq!(run("cat /tmp/layer").expect("the same container").trim(), "same");

        capture(&engine.remove_container(REMADE)).expect("the container is removed");
        capture(&engine.remove_volume(&home.volume())).expect("the home is removed");
    }
}

/// Brings up a real QCode extra container of `harness` in a scratch workspace through the path a
/// tab takes, on every engine, and hands `check` a shell into it; then brings it up a second time
/// and checks that nothing in the workspace changed. Everything made is removed again.
fn verify_guided(harness: crate::profile::HarnessKind, check: impl Fn(&dyn Fn(&str) -> String, &Path)) {
    use crate::profile::guidance::{self, BEGIN};
    use crate::profile::{AccountKind, MountAccess, NetworkMode, Profile, SafeName, Template};
    use crate::store::{WorkspaceId, WorkspacePaths};

    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new();
        let profile = Profile {
            name: SafeName::parse(&format!("guidetest-{}", harness.record().id)).expect("a safe name"),
            harness,
            template: Template::High,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::None,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        };
        let workspace = WorkspaceId::parse("uiguidelive").expect("a workspace id");
        let paths = WorkspacePaths {
            root: scratch.0.clone(),
            file: scratch.0.join("workspace.qcode"),
            code: scratch.0.join("Work"),
            assets: scratch.0.join("Assets"),
            harness: scratch.0.join("Containers").join("Harness"),
        };
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        assert_eq!(plan.guidance, Some(harness), "a QCode extra profile's plan brings its files up to date");
        let home = plan.home.clone().expect("a profile has a home");
        // Everything this run makes goes again, also when a check below fails.
        let _made = Made {
            engine: engine.clone(),
            profile: profile.clone(),
            container: plan.name.clone(),
            volume: home.volume(),
        };
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));

        let built = std::time::Instant::now();
        crate::profile::harness_live::build(&engine, &profile);
        eprintln!("{kind:?} {}: QCode extra image built in {} s", harness.record().id, built.elapsed().as_secs());

        // The person's own file is there first; nothing of it may be lost.
        let file = paths.code.join(guidance::file(harness));
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("the folder");
        std::fs::write(&file, "# uiguidelive\n\nThe person's own line.\n").expect("the person's file");
        // A little code of the person's, for graphify's map to be built of.
        std::fs::write(paths.code.join("m.py"), "def a():\n    return b()\n\ndef b():\n    return 1\n")
            .expect("the person's code");

        let user = HostUser::current().expect("the current user");
        let started = std::time::Instant::now();
        let answer = super::start(None, &engine, &plan, user, None, "tok-guidelive", |result| {
            result.unwrap_or_else(|failure| panic!("{kind:?}: the container did not come up: {failure:?}"));
            super::Msg::Deliver
        });
        eprintln!(
            "{kind:?} {}: first bring-up, with graphify and QCode's section, {} ms",
            harness.record().id,
            started.elapsed().as_millis()
        );
        assert!(matches!(answer, super::Msg::Deliver), "{kind:?}: nothing went wrong on the way: {answer:?}");

        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
                .unwrap_or_else(|error| panic!("{kind:?}: `{script}` failed: {error:?}"))
        };
        let text = std::fs::read_to_string(&file).expect("the file is there");
        assert!(text.starts_with("# uiguidelive\n\nThe person's own line.\n"), "{kind:?}: {text}");
        assert!(text.contains("## graphify"), "{kind:?}: graphify wrote its section: {text}");
        assert!(text.contains(&guidance::block()), "{kind:?}: {text}");
        assert_eq!(text.matches(BEGIN).count(), 1, "{kind:?}");
        // The agent in the container reads the same file.
        assert_eq!(run(&format!("cat /work/{}", guidance::file(harness))), text, "{kind:?}");
        // graphify's skill came with the image into this workspace's home.
        run(&format!("test -f \"$HOME/{}\"", guidance::skill(harness)));
        // The map is built by the first bring-up, in the background, without the network: waited
        // for by the lock its build holds, for a generous but finite time.
        run("timeout 300 flock /tmp/qcode-map true && test -s /work/graphify-out/graph.json");
        let map = std::fs::read_to_string(paths.code.join("graphify-out/graph.json")).expect("the map is there");
        assert!(map.contains("m.py"), "{kind:?}: the map is of the workspace's code");
        check(&run, &paths.code);

        // Twice is harmless: the container is running, graphify writes nothing new and QCode's
        // section is already as it would be written.
        let before = run("cd /work && find . -type f -exec sha256sum {} + | sort");
        let started = std::time::Instant::now();
        let again = super::start(None, &engine, &plan, user, None, "tok-guidelive", |result| {
            result.expect("up again");
            super::Msg::Deliver
        });
        eprintln!("{kind:?} {}: second bring-up {} ms", harness.record().id, started.elapsed().as_millis());
        assert!(matches!(again, super::Msg::Deliver), "{kind:?}: {again:?}");
        assert_eq!(run("cd /work && find . -type f -exec sha256sum {} + | sort"), before, "{kind:?}: nothing changed");
    }
}

/// What a guided live run made: its container, its home volume and its images, taken away when
/// the run ends, however it ends.
struct Made {
    engine: Engine,
    profile: crate::profile::Profile,
    container: String,
    volume: String,
}

impl Drop for Made {
    fn drop(&mut self) {
        let _ = capture(&self.engine.remove_container(&self.container));
        let _ = capture(&self.engine.remove_volume(&self.volume));
        crate::profile::harness_live::clear(&self.engine, &self.profile, &self.container);
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn a_qcode_extra_claude_code_container_comes_up_with_graphifys_part_and_qcodes_in_claude_md() {
    verify_guided(crate::profile::HarnessKind::ClaudeCode, |run, code| {
        // The hooks are in the project settings Claude Code reads from the folder it runs in,
        // and graphify's command answers inside the container, where the hook runs it.
        let settings = std::fs::read_to_string(code.join(".claude/settings.json")).expect("the project settings");
        assert!(settings.contains("\"PreToolUse\""), "{settings}");
        assert!(settings.contains("/usr/local/bin/graphify hook-guard search"), "{settings}");
        assert!(settings.contains("/usr/local/bin/graphify hook-guard read"), "{settings}");
        run("cd /work && echo '{}' | /usr/local/bin/graphify hook-guard search");
        // Claude Code's own package names the project settings file graphify wrote into.
        let found =
            run("grep -rlaF -- '.claude/settings.json' /usr/local/npm/lib/node_modules/@anthropic-ai | head -1");
        assert!(!found.trim().is_empty(), "Claude Code reads .claude/settings.json");
    });
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn a_qcode_extra_opencode_container_comes_up_with_graphifys_part_and_qcodes_in_agents_md() {
    verify_guided(crate::profile::HarnessKind::OpenCode, |run, code| {
        let settings = std::fs::read_to_string(code.join(".opencode/opencode.json")).expect("the project settings");
        assert!(settings.contains(".opencode/plugins/graphify.js"), "{settings}");
        assert!(code.join(".opencode/plugins/graphify.js").is_file(), "graphify's plugin");
        // opencode itself, in the workspace, loads the plugin graphify registered, beside the
        // one QCode high put in the image.
        let config = run("cd /work && opencode debug config 2>&1");
        assert!(config.contains("graphify.js"), "{config}");
        assert!(config.contains("oh-my-openagent"), "{config}");
    });
}

/// Everything the own-copy run makes, taken away when it ends, however it ends: the containers
/// and homes of its workspaces, the stored login, the images.
struct Copies {
    engine: Engine,
    profile: crate::profile::Profile,
    containers: Vec<String>,
    volumes: Vec<String>,
    images: std::cell::RefCell<Vec<String>>,
}

impl Drop for Copies {
    fn drop(&mut self) {
        for container in &self.containers {
            let _ = capture(&self.engine.remove_container(container));
        }
        for volume in &self.volumes {
            let _ = capture(&self.engine.remove_volume(volume));
        }
        crate::profile::harness_live::clear(&self.engine, &self.profile, "");
        for image in self.images.borrow().iter() {
            let _ = capture(&self.engine.remove_image(image));
        }
    }
}

#[test]
#[ignore = "needs a container engine and the network; run with QCODE_CONTAINER_TESTS=1"]
fn each_workspace_changes_its_own_copy_of_the_profiles_home_and_never_the_profile() {
    use crate::base::paths::{HOME_DIR, KEEP_ALIVE};
    use crate::engine::names;
    use crate::engine::{ContainerCreate, Mount, MountSource};
    use crate::profile::identity::STORE_DIR;
    use crate::profile::{AccountKind, Extra, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
    use crate::store::{Store, WorkspaceId, WorkspacePaths};

    use super::plan::{PLAN_LABEL, ensure_running};

    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new();
        let user = HostUser::current().expect("the current user");
        let profile = Profile {
            name: SafeName::parse("owncopytest").expect("a safe name"),
            harness: HarnessKind::ClaudeCode,
            template: Template::Recommended,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::None,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        };
        let workspaces = ["owncopy-a", "owncopy-b", "owncopy-c", "owncopy-d"];
        let login = names::credential_volume(profile.name.as_str());
        let made = Copies {
            engine: engine.clone(),
            profile: profile.clone(),
            containers: workspaces
                .iter()
                .map(|workspace| names::profile_container(workspace, profile.name.as_str()))
                .chain(["qcode-owncopytest-courier".to_owned()])
                .collect(),
            volumes: workspaces
                .iter()
                .map(|workspace| names::home_volume(workspace, profile.name.as_str()))
                .chain([login.clone()])
                .collect(),
            images: std::cell::RefCell::new(Vec::new()),
        };
        // What an interrupted run may have left is cleared first.
        for container in &made.containers {
            let _ = capture(&engine.remove_container(container));
        }
        for volume in &made.volumes {
            let _ = capture(&engine.remove_volume(volume));
        }
        crate::profile::harness_live::build(&engine, &profile);

        // The profile's definition file, in a store of this run's own, and its stored login.
        let store = Store::new(scratch.0.join("store"));
        store.write_profile(&profile).expect("the definition is written");
        let definition = scratch.0.join("store").join("Profiles").join("owncopytest.toml");
        let written = std::fs::read(&definition).expect("the definition is there");
        let courier_up = |access: Access| {
            let name = "qcode-owncopytest-courier";
            let _ = capture(&engine.remove_container(name));
            let mounts = [Mount { source: MountSource::Volume(&login), target: STORE_DIR.as_ref(), access }];
            capture(&engine.create_container(&ContainerCreate {
                name,
                hostname: HOSTNAME,
                labels: &[],
                image: &profile.image(),
                mounts: &mounts,
                network: Network::None,
                user,
                workdir: None,
                command: KEEP_ALIVE,
            }))
            .expect("the courier is made");
            capture(&engine.start_container(name)).expect("the courier starts");
        };
        let courier = |script: &str| {
            let name = "qcode-owncopytest-courier";
            courier_up(Access::ReadOnly);
            let said =
                capture(&engine.exec_without_terminal(&Exec { container: name, command: &["sh", "-c", script] }))
                    .expect("the courier's script");
            let _ = capture(&engine.remove_container(name));
            said
        };
        // Stored the way the product stores a login: copied in by the engine, which a volume that
        // belongs to root under Docker takes as well.
        let fake = scratch.0.join("login");
        std::fs::create_dir_all(fake.join(".claude")).expect("a login folder");
        std::fs::write(fake.join(".claude").join(".credentials.json"), "stored-login\n").expect("a login");
        courier_up(Access::ReadWrite);
        capture(&engine.copy_in(&crate::engine::CopyIn {
            host: &fake.join("."),
            container: "qcode-owncopytest-courier",
            target: Path::new(STORE_DIR),
        }))
        .expect("the login is stored");
        let _ = capture(&engine.remove_container("qcode-owncopytest-courier"));
        let stored = || courier(&format!("cd {STORE_DIR} && find . -type f | sort | xargs sha256sum"));
        let login_before = stored();

        let plan_of = |workspace: &str, profile: &Profile| {
            let root = scratch.0.join(workspace);
            for folder in ["Work", "Assets"] {
                std::fs::create_dir_all(root.join(folder)).expect("a workspace folder");
            }
            let paths = WorkspacePaths {
                root: root.clone(),
                file: root.join("workspace.qcode"),
                code: root.join("Work"),
                assets: root.join("Assets"),
                harness: root.join("Containers").join("Harness"),
            };
            super::plan::ContainerPlan::profile(&WorkspaceId::parse(workspace).expect("an id"), &paths, profile)
        };
        let run = |plan: &super::plan::ContainerPlan, script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
                .unwrap_or_else(|error| panic!("{kind:?}: `{script}` in {} failed: {error:?}", plan.name))
        };
        // Node, which every image of a command-line harness carries; Python comes with graphify
        // only, and the rebuild below goes without it.
        let theme = format!(
            "node -e \"const d = require('{HOME_DIR}/.claude/settings.json'); console.log(d.theme || 'none')\""
        );

        // A and B come up from the one profile, each with the stored login copied in.
        let a = plan_of("owncopy-a", &profile);
        let b = plan_of("owncopy-b", &profile);
        ensure_running(&engine, &a, user).expect("A comes up");
        ensure_running(&engine, &b, user).expect("B comes up");
        for plan in [&a, &b] {
            assert_eq!(
                run(plan, &format!("cat {HOME_DIR}/.claude/.credentials.json")).trim(),
                "stored-login",
                "{kind:?}"
            );
        }

        // In A the person writes a file and changes a setting of the harness.
        run(&a, &format!("echo mine > {HOME_DIR}/only-in-a"));
        run(
            &a,
            &format!(
                "node -e \"const f = require('fs'); const p = '{HOME_DIR}/.claude/settings.json'; const d = JSON.parse(f.readFileSync(p)); d.theme = 'light'; f.writeFileSync(p, JSON.stringify(d))\""
            ),
        );
        run(&a, &format!("echo changed-in-a > {HOME_DIR}/.claude/.credentials.json"));
        assert_eq!(run(&a, &theme).trim(), "light", "{kind:?}");

        // B has none of it, and neither has C, made after it.
        let c = plan_of("owncopy-c", &profile);
        ensure_running(&engine, &c, user).expect("C comes up");
        for plan in [&b, &c] {
            assert!(
                run(plan, &format!("test -e {HOME_DIR}/only-in-a && echo there || echo absent")).contains("absent")
            );
            assert_eq!(run(plan, &theme).trim(), "none", "{kind:?}: {}", plan.name);
            assert_eq!(run(plan, &format!("cat {HOME_DIR}/.claude/.credentials.json")).trim(), "stored-login");
        }
        // The profile itself is as it was: its file byte for byte, its login file for file.
        assert_eq!(std::fs::read(&definition).expect("the definition"), written, "{kind:?}");
        assert_eq!(stored(), login_before, "{kind:?}: the stored login was not written");

        // A change of the profile that the containers take (the network) remakes A's stopped
        // container from the new plan, and A's own copy of the home stays as A left it.
        capture(&engine.stop_container(&a.name)).expect("A stops");
        let opened = Profile { network: NetworkMode::Full, ..profile.clone() };
        let a_open = plan_of("owncopy-a", &opened);
        ensure_running(&engine, &a_open, user).expect("A comes up again");
        let label = capture(&engine.container_label(&a.name, PLAN_LABEL)).expect("the label");
        assert_eq!(label.trim(), a_open.digest(&engine, user), "{kind:?}: A's container was made again");
        assert_eq!(run(&a_open, &format!("cat {HOME_DIR}/only-in-a")).trim(), "mine", "{kind:?}");
        assert_eq!(run(&a_open, &theme).trim(), "light", "{kind:?}");

        // A change the image is built from: the template's new state reaches a new workspace,
        // and A keeps its own copy, the person's edits in it and what the old image gave it.
        let old = capture(&engine.image_exists(&profile.image())).expect("the image").trim().to_owned();
        made.images.borrow_mut().push(old);
        let rebuilt = Profile { without: vec![Extra::Graphify], ..opened.clone() };
        capture(&engine.stop_container(&a.name)).expect("A stops");
        crate::profile::harness_live::build(&engine, &rebuilt);
        let a_new = plan_of("owncopy-a", &rebuilt);
        ensure_running(&engine, &a_new, user).expect("A comes up on the new image");
        assert_eq!(run(&a_new, "command -v graphify || echo gone").trim(), "gone", "{kind:?}: the new image");
        assert_eq!(run(&a_new, &format!("cat {HOME_DIR}/only-in-a")).trim(), "mine", "{kind:?}");
        assert_eq!(run(&a_new, &theme).trim(), "light", "{kind:?}");
        assert!(run(&a_new, &format!("cat {HOME_DIR}/.claude/CLAUDE.md")).contains("## graphify"), "{kind:?}: kept");
        let d = plan_of("owncopy-d", &rebuilt);
        ensure_running(&engine, &d, user).expect("D comes up");
        let fresh = run(&d, &format!("cat {HOME_DIR}/.claude/CLAUDE.md 2>/dev/null || echo none"));
        assert!(!fresh.contains("## graphify"), "{kind:?}: a new workspace gets the new home: {fresh}");
        assert_eq!(std::fs::read(&definition).expect("the definition"), written, "{kind:?}");
        assert_eq!(stored(), login_before, "{kind:?}");
        drop(made);
    }
}

/// The name of the machine QCode is running on, read without asking a shell for it.
fn this_machine() -> String {
    std::fs::read_to_string(Path::new("/etc/hostname")).unwrap_or_default().trim().to_owned()
}

/// The container of the check that closing a tab ends what the tab started inside it.
const ENDED: &str = "qcode-uiworkspacelive-ended";

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_closed_tabs_processes_end_inside_the_container_and_no_other_tabs_do() {
    for engine in engines() {
        let scratch = Scratch::new();
        let plan = scratch.plan(ENDED);
        let _ = capture(&engine.remove_container(ENDED));
        let user = HostUser::current().expect("the current user");
        capture(&plan.create(&engine, user)).expect("the container is made");
        capture(&engine.start_container(ENDED)).expect("the container starts");
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: ENDED, command: &["sh", "-c", script] }))
                .expect("the shell runs")
        };
        // Two tabs' harnesses, each with a child, left behind the way a closed tab's are: the
        // command that started them has ended and they go on running.
        run("QCODE_BRIDGE=tok-closed nohup sh -c 'sleep 611 & sleep 612' >/dev/null 2>&1 & \
             QCODE_BRIDGE=tok-open nohup sleep 613 >/dev/null 2>&1 &");
        let running = || run("ps -o args | grep -c '^sleep 61[123]$'; true");
        assert_eq!(running().trim(), "3", "{:?}: the three are running", engine.kind());

        capture(&plan.end_tab(&engine, "tok-closed")).expect("the ending runs");
        std::thread::sleep(std::time::Duration::from_secs(2));
        let left = run("ps -o args | grep '^sleep 61[123]$'; true");
        assert_eq!(left.trim(), "sleep 613", "{:?}: only the open tab's is left", engine.kind());

        capture(&engine.remove_container(ENDED)).expect("the container is removed");
    }
}
