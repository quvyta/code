//! opencode's shared server in a real container, from a real QCode recommended image: two tabs
//! share one server, each shows a conversation of its own, the bridge's plugin speaks for the tab
//! whose conversation a call comes from (a helper's too), a new line stays a new line, a server
//! that goes away comes back under both tabs, and closing tabs the way QCode closes them leaves
//! the server for the tab still open and takes it with the last one. Across two workspaces, each
//! tab's agent learns through the plugin which tab it is, and sees only its own workspace.
//!
//! `#[ignore]`d and doing nothing unless `QCODE_CONTAINER_TESTS=1`. The image is built with the
//! network once; the container the tabs run in has none, as a profile without the network runs,
//! so nothing here asks a model anything.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 QCODE_CONTAINER_ENGINE=podman \
//!   cargo test shared_live -- --ignored --test-threads=1
//! ```

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use qframe::widgets::TerminalSession;
use serde_json::Value;

use super::*;
use crate::bridge::protocol::{Answer, Question, You};
use crate::bridge::socket::Listener;
use crate::engine::{EngineKind, HostUser, detect};
use crate::profile::harness_live::{build, clear};
use crate::profile::identity::Home;
use crate::profile::{AccountKind, MountAccess, NetworkMode, SafeName};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// Words opencode's interface draws under the prompt of a conversation's page (1.18.32).
const ATTACHED: &str = "ctrl+p commands";

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
    fn new() -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-sharedlive-{stamp}"));
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

/// Runs `script` with a shell in `container`, with `env` set, and answers what it printed.
fn run(engine: &Engine, container: &str, env: &[(&str, &str)], script: &str) -> String {
    let mut command = vec!["env".to_owned()];
    command.extend(env.iter().map(|(name, value)| format!("{name}={value}")));
    command.extend(["sh".to_owned(), "-c".to_owned(), script.to_owned()]);
    let parts: Vec<&str> = command.iter().map(String::as_str).collect();
    capture(&engine.exec_without_terminal(&Exec { container, command: &parts }))
        .unwrap_or_else(|error| panic!("{:?}: {script}: {error:?}", engine.kind()))
}

/// How many processes of the container run a command line holding `words`, read from `/proc`:
/// the images promise no `ps`.
fn running(engine: &Engine, container: &str, words: &str) -> usize {
    // The words are this shell's own argument, so this shell and its subshells are passed over.
    let script = "n=0; for p in /proc/[0-9]*; do c=$(tr '\\000' ' ' < \"$p/cmdline\" 2>/dev/null); \
                  case \"$c\" in 'sh -c n=0;'*) continue;; *\"$1\"*) n=$((n+1));; esac; done; echo $n";
    let parts = ["sh", "-c", script, "sh", words];
    capture(&engine.exec_without_terminal(&Exec { container, command: &parts }))
        .ok()
        .and_then(|said| said.trim().parse().ok())
        .unwrap_or(0)
}

/// Asks the server in `container` from inside it: the status of the answer and what it said, or
/// no status when it did not answer within ten seconds (a server loading its plugins).
fn server(engine: &Engine, container: &str, method: &str, path: &str) -> (u16, Value) {
    let script = format!(
        "node -e 'fetch(\"http://127.0.0.1:{PORT}{path}\",{{method:\"{method}\",signal:AbortSignal.timeout(10000),headers:{{\"x-opencode-directory\":\"/work\",\"content-type\":\"application/json\"}},body:{body}}}).then(async r=>process.stdout.write(r.status+\" \"+await r.text())).catch(()=>process.stdout.write(\"0 null\"))'",
        body = if method == "POST" { "process.argv[1]" } else { "undefined" },
    );
    let said = run(engine, container, &[], &format!("{script} '{{}}'"));
    let (status, text) = said.split_once(' ').unwrap_or(("0", "null"));
    (status.parse().unwrap_or(0), serde_json::from_str(text).unwrap_or(Value::Null))
}

/// Draws a tab's terminal until `wanted` is on it, for a generous while.
fn until_shown(session: &TerminalSession, wanted: &str) -> String {
    use qframe::runtime::Harness as Screen;
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
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        screen.render();
        let drawn = screen.screen();
        if drawn.contains(wanted) || Instant::now() > deadline {
            return drawn;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(180);
    while !done() {
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn two_opencode_tabs_share_one_server_and_each_stays_itself() {
    let profile = Profile {
        name: SafeName::parse("sharedtest-opencode").expect("a safe name"),
        harness: HarnessKind::OpenCode,
        template: Template::Recommended,
        account: AccountKind::Free,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    };
    for engine in engines() {
        let kind = engine.kind();
        let scratch = Scratch::new();
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse("sharedtest").expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, &profile);
        let home = Home::new(profile.name.clone(), workspace.clone());
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, &profile);
        let user = HostUser::current().expect("the current user");
        ensure_running(&engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));

        // QCode's end of the bridge, noting every question.
        let listener = Listener::open(&paths.mcp()).expect("the bridge's socket");
        let (seen, questions) = mpsc::channel::<Question>();
        let inbox = listener.inbox();
        let answering = std::thread::spawn(move || {
            while let Some(call) = inbox.next() {
                if let Ok(question) = call.question.clone() {
                    call.answer(Answer::listed("nobody else".to_owned(), You::default(), Vec::new()));
                    let _ = seen.send(question);
                }
            }
        });

        // What the screen hands the server and each tab: the server's token, never a tab's.
        let tokens = ServerTokens::default();
        let token = tokens.token(profile.name.as_str());
        let environment = server_environment(&token, &[]);
        let first = ready(&engine, &plan.name, &paths.mcp(), &environment, None)
            .unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        let second = ready(&engine, &plan.name, &paths.mcp(), &environment, None).expect("a second conversation");
        assert_ne!(first, second, "{kind:?}: each tab shows a conversation of its own");
        assert_eq!(
            ready(&engine, &plan.name, &paths.mcp(), &environment, Some(&first)).expect("the first again"),
            first,
            "{kind:?}: a tab opens its conversation again"
        );

        let tab = |conversation: &str, tab_token: &str| {
            let program = tab_program(Some(conversation));
            let parts: Vec<&str> = program.iter().map(String::as_str).collect();
            let mut env: Vec<(&str, &str)> = vec![(crate::bridge::TOKEN_VARIABLE, tab_token)];
            env.extend(environment.iter().map(|(name, value)| (name.as_str(), value.as_str())));
            let command = plan.enter_with(&engine, &parts, &env);
            TerminalSession::spawn(command.program.as_os_str(), &command.args, &scratch.0).expect("a pseudo-terminal")
        };
        let one = tab(&first, "tok-one");
        let two = tab(&second, "tok-two");
        // An interface showing a conversation draws its page, not the empty home screen's prompt.
        for session in [&one, &two] {
            let drawn = until_shown(session, ATTACHED);
            assert!(drawn.contains(ATTACHED), "{kind:?}: no prompt:\n{drawn}");
        }
        assert_eq!(running(&engine, &plan.name, "opencode serve"), 1, "{kind:?}: one server");
        assert_eq!(running(&engine, &plan.name, "attach http://127.0.0.1:41418"), 2, "{kind:?}: two interfaces");
        assert_eq!(running(&engine, &plan.name, "opencode --auto"), 0, "{kind:?}: no server of a tab's own");
        assert_eq!(running(&engine, &plan.name, "qcode-opencode.mjs keep"), 1, "{kind:?}: one keeper");

        // opencode loaded the bridge's plugin, arguments and all, and left the MCP bridge off.
        let (_, tools) = server(&engine, &plan.name, "GET", "/experimental/tool/ids");
        for name in ["qcode_list_tabs", "qcode_send_message", "qcode_check_inbox"] {
            assert!(tools.as_array().is_some_and(|ids| ids.iter().any(|id| id == name)), "{kind:?}: {tools}");
        }
        let (_, servers) = server(&engine, &plan.name, "GET", "/mcp");
        assert_eq!(servers["qcode"]["status"], "disabled", "{kind:?}: {servers}");

        // The plugin's call from a helper of the second tab's agent reaches QCode as the second
        // tab: the server's token, and the conversation the tab shows.
        let child = run(
            &engine,
            &plan.name,
            &[],
            &format!(
                "node -e 'fetch(\"http://127.0.0.1:{PORT}/session\",{{method:\"POST\",headers:{{\"x-opencode-directory\":\"/work\",\"content-type\":\"application/json\"}},body:JSON.stringify({{parentID:\"{second}\"}})}}).then(r=>r.json()).then(j=>process.stdout.write(j.id))'"
            ),
        );
        let call = format!(
            "const u='http://127.0.0.1:{PORT}';const h={{'x-opencode-directory':'/work'}};\
             const {{QCodeBridge}}=await import('{plugin}');\
             const client={{session:{{get:async({{path:{{id}}}})=>({{data:await (await fetch(u+'/session/'+id,{{headers:h}})).json()}})}}}};\
             const hooks=await QCodeBridge({{client}});\
             process.stdout.write(await hooks.tool.qcode_list_tabs.execute({{}},{{sessionID:'{child}'}}));",
            plugin = plugin_in_container(),
        );
        let said = run(
            &engine,
            &plan.name,
            &[(crate::bridge::TOKEN_VARIABLE, &token)],
            &format!("node --input-type=module -e \"{call}\""),
        );
        assert!(said.contains("nobody else"), "{kind:?}: {said}");
        let asked = questions.recv_timeout(Duration::from_secs(30)).expect("QCode was asked");
        assert_eq!(asked.token, token, "{kind:?}");
        assert_eq!(asked.session.as_deref(), Some(second.as_str()), "{kind:?}: the helper's tab");

        // Shift+Enter under the kitty keyboard protocol, as the terminal sends it once a program
        // asks for it: a new line in the prompt, nothing sent.
        one.write(b"first line").expect("typed");
        one.write(b"\x1b[13;2u").expect("a new line");
        one.write(b"second line").expect("typed");
        let drawn = until_shown(&one, "second line");
        assert!(drawn.contains("first line") && drawn.contains("second line"), "{kind:?}:\n{drawn}");
        let (_, said) = server(&engine, &plan.name, "GET", &format!("/session/{first}/message"));
        assert_eq!(said.as_array().map(Vec::len), Some(0), "{kind:?}: nothing was sent: {said}");

        // The server goes away under both tabs: it comes back and both are attached again.
        run(
            &engine,
            &plan.name,
            &[],
            "for p in /proc/[0-9]*; do c=$(tr '\\000' ' ' < \"$p/cmdline\" 2>/dev/null); \
             case \"$c\" in 'opencode serve '*) kill -9 \"${p#/proc/}\";; esac; done; true",
        );
        until("a server again", || running(&engine, &plan.name, "opencode serve") == 1);
        until("both attached again", || running(&engine, &plan.name, "attach http://127.0.0.1:41418") == 2);
        for session in [&one, &two] {
            let drawn = until_shown(session, ATTACHED);
            assert!(drawn.contains(ATTACHED), "{kind:?}: the tab did not come back:\n{drawn}");
        }

        // The first tab closes the way QCode closes a tab: the other keeps the server, and the
        // conversation nothing was said in is gone.
        capture(&plan.end_tab(&engine, "tok-one")).expect("the tab is ended");
        until("the first interface ends", || running(&engine, &plan.name, "attach http://127.0.0.1:41418") == 1);
        assert_eq!(running(&engine, &plan.name, "opencode serve"), 1, "{kind:?}: the second tab keeps it");
        until("the empty conversation goes", || {
            server(&engine, &plan.name, "GET", &format!("/session/{first}")).0 == 404
        });

        // The last tab closes: the server goes with it.
        capture(&plan.end_tab(&engine, "tok-two")).expect("the tab is ended");
        until("the server ends", || running(&engine, &plan.name, "opencode serve") == 0);

        one.kill();
        two.kill();
        drop(listener);
        let _ = answering.join();
        capture(&engine.remove_container(&plan.name)).expect("the container is removed");
        capture(&engine.remove_volume(&home.volume())).expect("the home is removed");
        clear(&engine, &profile, &plan.name);
    }
}

/// Asks, from inside `container`, the bridge's opencode plugin for `qcode_list_tabs` the way
/// opencode calls it for the conversation `session`, with the server's `token`, and answers the
/// words the agent is shown.
fn list_through_plugin(engine: &Engine, container: &str, token: &str, session: &str) -> String {
    let call = format!(
        "const u='http://127.0.0.1:{PORT}';const h={{'x-opencode-directory':'/work'}};\
         const {{QCodeBridge}}=await import('{plugin}');\
         const client={{session:{{get:async({{path:{{id}}}})=>({{data:await (await fetch(u+'/session/'+id,{{headers:h}})).json()}})}}}};\
         const hooks=await QCodeBridge({{client}});\
         process.stdout.write(await hooks.tool.qcode_list_tabs.execute({{}},{{sessionID:'{session}'}}));",
        plugin = plugin_in_container(),
    );
    run(
        engine,
        container,
        &[(crate::bridge::TOKEN_VARIABLE, token)],
        &format!("node --input-type=module -e \"{call}\""),
    )
}

/// A conversation started inside `container` as a helper of `parent`, the way an agent's subagent
/// starts one.
fn helper_of(engine: &Engine, container: &str, parent: &str) -> String {
    run(
        engine,
        container,
        &[],
        &format!(
            "node -e 'fetch(\"http://127.0.0.1:{PORT}/session\",{{method:\"POST\",headers:{{\"x-opencode-directory\":\"/work\",\"content-type\":\"application/json\"}},body:JSON.stringify({{parentID:\"{parent}\"}})}}).then(r=>r.json()).then(j=>process.stdout.write(j.id))'"
        ),
    )
}

/// What a run over two workspaces makes, taken away when the run ends, whether it passed or not.
struct Made<'a> {
    engine: &'a Engine,
    profile: &'a Profile,
    /// Each workspace's container and the home volume of its profile.
    homes: Vec<(String, String)>,
}

impl Drop for Made<'_> {
    fn drop(&mut self) {
        for (container, volume) in &self.homes {
            let _ = capture(&self.engine.remove_container(container));
            let _ = capture(&self.engine.remove_volume(volume));
        }
        if let Some((container, _)) = self.homes.first() {
            clear(self.engine, self.profile, container);
        }
    }
}

#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn every_opencode_tab_of_two_workspaces_learns_through_the_plugin_which_tab_it_is() {
    let profile = Profile {
        name: SafeName::parse("idtest-opencode").expect("a safe name"),
        harness: HarnessKind::OpenCode,
        template: Template::Recommended,
        account: AccountKind::Free,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    };
    for engine in engines() {
        // The words QCode answers in are English here, as they are for an agent of a QCode whose
        // person chose English.
        let english = std::sync::Arc::new(crate::service::translator(Some("en")));
        qframe::i18n::scope(english, || each_tab_names_itself(&engine, &profile));
    }
}

/// Two workspaces of two opencode tabs each on `engine`, every tab's agent and a helper of it
/// asking through the plugin which tab it is.
fn each_tab_names_itself(engine: &Engine, profile: &Profile) {
    use crate::bridge::socket::Call;
    use crate::store::{WorkspaceFile, WorkspaceProfile};
    use crate::ui::workspace::{Choice, Msg, OpenWorkspace, Tab, TabKind, WorkspaceScreen, bridge, update};

    let named = [("idtest-alder", "Alder"), ("idtest-birch", "Birch")];
    let kind = engine.kind();
    let scratches = [Scratch::new(), Scratch::new()];
    let day = qframe::date::Date::new(2026, 9, 27).expect("a day the calendar has");
    let workspaces: Vec<OpenWorkspace> = named
        .iter()
        .zip(&scratches)
        .map(|((id, name), scratch)| {
            let mut file = WorkspaceFile::new(WorkspaceId::parse(id).expect("a workspace id"), *name, day);
            file.profiles = vec![WorkspaceProfile { name: profile.name.as_str().to_owned(), added: Some(day) }];
            OpenWorkspace::new(&file, scratch.paths(), vec![profile.clone()])
        })
        .collect();
    let user = HostUser::current().expect("the current user");
    let mut screen = WorkspaceScreen::new(Some(engine.clone()), user, workspaces);
    let mut made = Made { engine, profile, homes: Vec::new() };
    build(engine, profile);

    // Two opencode tabs in each workspace, both on the workspace's one server, each showing a
    // conversation of its own that the server chose for it.
    let mut plans = Vec::new();
    for (index, scratch) in scratches.iter().enumerate() {
        drop(update(&mut screen, Msg::OpenWorkspace(index)));
        let plan = screen.workspaces()[index]
            .plan(&TabKind::Profile(profile.name.as_str().to_owned()))
            .expect("the profile's container");
        let workspace = WorkspaceId::parse(named[index].0).expect("a workspace id");
        let volume = Home::new(profile.name.clone(), workspace).volume();
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&volume));
        made.homes.push((plan.name.clone(), volume));
        ensure_running(engine, &plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        let token = screen.workspaces()[index].servers.token(profile.name.as_str());
        let environment = server_environment(&token, &[]);
        for _ in 0..2 {
            drop(update(&mut screen, Msg::NewTab));
            let key = screen.workspace().and_then(OpenWorkspace::active_tab).map(Tab::key).expect("a blank tab");
            drop(update(&mut screen, Msg::Choose(key, Choice::NewChat(profile.name.as_str().to_owned()))));
            let run = screen.workspaces()[index].tabs().iter().find(|tab| tab.key() == key).map(Tab::run);
            let conversation = ready(engine, &plan.name, &scratch.paths().mcp(), &environment, None)
                .unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
            drop(update(&mut screen, Msg::Woken(key, run.expect("the tab"), Ok(()), None, Some(conversation))));
        }
        plans.push((plan, token));
    }
    // The person names Birch's second tab, so that a list from Alder could not read it by chance.
    drop(update(&mut screen, Msg::OpenWorkspace(1)));
    drop(update(&mut screen, Msg::RenameTab(1)));
    drop(update(&mut screen, Msg::TabName("birch reviewer".to_owned())));
    drop(update(&mut screen, Msg::NameTab));

    // QCode's end of each workspace's socket, handing every call to the screen as its listener
    // does.
    let (calls, asked) = mpsc::channel::<(&'static str, Call)>();
    let listeners: Vec<(Listener, std::thread::JoinHandle<()>)> = scratches
        .iter()
        .zip(named)
        .map(|(scratch, (id, _))| {
            let listener = Listener::open(&scratch.paths().mcp()).expect("the bridge's socket");
            let inbox = listener.inbox();
            let calls = calls.clone();
            let handing = std::thread::spawn(move || {
                while let Some(call) = inbox.next() {
                    if calls.send((id, call)).is_err() {
                        break;
                    }
                }
            });
            (listener, handing)
        })
        .collect();

    for (index, (plan, token)) in plans.iter().enumerate() {
        let tabs: Vec<(u32, String, String)> = screen.workspaces()[index]
            .tabs()
            .iter()
            .enumerate()
            .map(|(place, tab)| {
                let conversation = tab.conversation().expect("a conversation").to_owned();
                (tab.number(), screen.workspaces()[index].tab_label(place), conversation)
            })
            .collect();
        assert_eq!(tabs.iter().map(|tab| tab.0).collect::<Vec<_>>(), [1, 2], "{kind:?}: ids count per workspace");
        for (number, title, conversation) in &tabs {
            // The tab's agent, and a helper its agent started.
            for session in [conversation.clone(), helper_of(engine, &plan.name, conversation)] {
                let (engine, container, token) = (engine.clone(), plan.name.clone(), token.clone());
                let asking = std::thread::spawn(move || list_through_plugin(&engine, &container, &token, &session));
                let (id, call) = asked.recv_timeout(Duration::from_secs(120)).expect("QCode was asked");
                drop(bridge::answer(&mut screen, id, call, Instant::now()));
                let said = asking.join().expect("the plugin answered");
                let workspace = named[index].1;
                let first = said.lines().next().unwrap_or_default();
                assert_eq!(
                    first,
                    format!("You are tab {number}, «{title}», in workspace «{workspace}»."),
                    "{kind:?}: {said}"
                );
                let twin = tabs.iter().find(|tab| tab.0 != *number).expect("a twin");
                assert!(said.contains("One other tab runs an agent:"), "{kind:?}: one other tab: {said}");
                assert!(said.contains(&format!("tab {}: {},", twin.0, twin.1)), "{kind:?}: its twin: {said}");
                let stranger = if index == 0 { "birch reviewer" } else { "Alder" };
                assert!(!said.contains(stranger), "{kind:?}: nothing of the other workspace: {said}");
            }
        }
    }

    for (listener, handing) in listeners {
        drop(listener);
        let _ = handing.join();
    }
}
