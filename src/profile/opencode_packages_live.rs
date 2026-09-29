//! That a profile without the network draws its first opencode tab's prompt in seconds: the
//! profile's image built by the product's own recipe, a workspace container with no network, and
//! the first tab brought up the way the workspace brings it up, the server made ready first and
//! the tab's own loop attached to it in a real terminal. Under QCode extra graphify's installer
//! runs in the workspace first, as it does when the container comes up, and leaves its plugin in
//! the workspace's `.opencode`; under QCode recommended the plugin is in the image's home.
//!
//! Before the image fetched opencode's configuration packages this took 70–90 seconds, npm's
//! retries without the network. The bound is generous for a loaded machine and still far below
//! that. A third run takes the packages out of the home first, as in a workspace whose home volume
//! was made from an image before them: npm then installs them from the image's cache.
//!
//! `#[ignore]`d and doing nothing unless `QCODE_CONTAINER_TESTS=1`; the image is built with the
//! network once:
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 QCODE_CONTAINER_ENGINE=podman \
//!   cargo test opencode_packages_live -- --ignored --nocapture --test-threads=1
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use qframe::runtime::Harness as Screen;
use qframe::widgets::TerminalSession;

use super::harness_live::{build, clear};
use super::identity::Home;
use super::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::shared::{ServerTokens, ready_command, server_environment, tab_program, write_scripts};
use crate::ui::workspace::{CODE_DIR, ContainerPlan, ensure_running};

/// Words opencode's interface draws under the prompt of a conversation's page (1.18.33).
const ATTACHED: &str = "ctrl+p commands";

/// How long the first tab may take, from the server being asked for to the prompt on screen.
const BOUND: Duration = Duration::from_secs(60);

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

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-firsttab-{name}-{stamp}"));
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

/// Draws a tab's terminal until `wanted` is on it or `until` has passed.
fn until_shown(session: &TerminalSession, wanted: &str, until: Instant) -> String {
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
    let mut screen = Screen::new(Watched(session.clone()), 120, 36);
    loop {
        screen.render();
        let drawn = screen.screen();
        if drawn.contains(wanted) || Instant::now() > until {
            return drawn;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn first_tab(template: Template, old_home: bool) {
    let profile = Profile {
        name: SafeName::parse(&format!("firsttab-opencode-{}", template.id())).expect("a safe name"),
        harness: HarnessKind::OpenCode,
        template,
        account: AccountKind::Free,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    };
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new(template.id());
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse("firsttab").expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace);
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        let run = |script: &str| {
            capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &["sh", "-c", script] }))
        };
        assert!(
            run("node -e 'fetch(\"https://registry.npmjs.org\").then(()=>process.exit(0),()=>process.exit(1))'")
                .is_err(),
            "{kind:?}: the container reaches the network"
        );

        if old_home {
            run("cd \"$HOME/.config/opencode\" && rm -rf node_modules package.json package-lock.json")
                .expect("the packages are taken out of the home");
        }
        let started = Instant::now();
        if template.carries_high() {
            // What the workspace runs when the container comes up under QCode extra.
            run(&format!("cd {CODE_DIR} && graphify opencode install")).expect("graphify's installer runs");
            assert!(paths.code.join(".opencode/plugins/graphify.js").exists(), "{kind:?}: graphify's plugin");
        }
        let environment = server_environment(&ServerTokens::default().token(profile.name.as_str()), &[]);
        write_scripts(&paths.mcp()).expect("the scripts are written");
        let command = ready_command(&environment, None);
        let parts: Vec<&str> = command.iter().map(String::as_str).collect();
        let said = capture(&engine.exec_without_terminal(&Exec { container: &plan.name, command: &parts }).unbounded())
            .unwrap_or_else(|error| panic!("{kind:?}: the server did not come up: {error:?}"));
        let conversation =
            said.lines().rev().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned();
        let served = started.elapsed();

        let program = tab_program(Some(&conversation));
        let words: Vec<&str> = program.iter().map(String::as_str).collect();
        let mut env: Vec<(&str, &str)> = vec![(crate::bridge::TOKEN_VARIABLE, "tok-firsttab")];
        env.extend(environment.iter().map(|(name, value)| (name.as_str(), value.as_str())));
        let command = plan.enter_with(&engine, &words, &env);
        let session =
            TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0).expect("a pseudo-terminal");
        // Watched well past the bound, so a slow tab is measured and not only failed.
        let drawn = until_shown(&session, ATTACHED, started + Duration::from_secs(240));
        let shown = started.elapsed();
        eprintln!(
            "== {kind:?} {template:?}: server ready in {:.1} s, first tab's prompt in {:.1} s",
            served.as_secs_f64(),
            shown.as_secs_f64()
        );
        let installed = run(&format!(
            "ls -d \"$HOME/.config/opencode/node_modules/@opencode-ai/plugin\" {CODE_DIR}/.opencode/node_modules/@opencode-ai/plugin 2>&1; true"
        ))
        .unwrap_or_default();
        eprintln!("== packages:\n{installed}");

        session.kill();
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        clear(&engine, &profile, &plan.name);
        assert!(drawn.contains(ATTACHED), "{kind:?} {template:?}: no prompt:\n{drawn}");
        assert!(shown < BOUND, "{kind:?} {template:?}: the first tab took {} s", shown.as_secs());
    }
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn an_offline_qcode_recommended_opencode_tab_draws_its_prompt_in_seconds() {
    first_tab(Template::Recommended, false);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn an_offline_opencode_tab_of_a_home_made_before_the_packages_draws_its_prompt_in_seconds() {
    first_tab(Template::Recommended, true);
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn an_offline_qcode_extra_opencode_tab_draws_its_prompt_in_seconds() {
    first_tab(Template::High, false);
}
