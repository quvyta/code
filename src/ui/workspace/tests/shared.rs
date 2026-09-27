//! opencode's shared server on the screen: what an opencode tab of a QCode template runs and
//! carries, how its conversation reaches it before it starts, and how the bridge tells the tabs of
//! one server apart.

use std::sync::mpsc::Receiver;

use super::*;

use crate::bridge::protocol::{Answer, Question, Request};
use crate::bridge::socket::Call;
use crate::ui::workspace::shared;

/// A screen with two opencode profiles under QCode recommended and one under base, with two tabs of
/// the first, one of the second and one of the third open, in that order.
fn opencode_tabs(scratch: &Scratch) -> WorkspaceScreen {
    let profiles = vec![
        profile("opencode", HarnessKind::OpenCode),
        profile("opencode-two", HarnessKind::OpenCode),
        Profile { template: Template::Base, ..profile("opencode-plain", HarnessKind::OpenCode) },
    ];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    );
    apply(&mut screen, Msg::OpenWorkspace(0));
    for name in ["opencode", "opencode", "opencode-two", "opencode-plain"] {
        open(&mut screen, Choice::NewChat(name.to_owned()));
    }
    screen
}

fn tab_at(screen: &WorkspaceScreen, index: usize) -> &Tab {
    &screen.workspace().expect("a workspace is open").tabs()[index]
}

/// Hands the tab at `index` the conversation its server chose, the way the tab's start does.
fn shown(screen: &mut WorkspaceScreen, index: usize, conversation: &str) {
    let key = tab_at(screen, index).key();
    let run = tab_at(screen, index).run();
    apply(screen, Msg::Woken(key, run, Ok(()), None, Some(conversation.to_owned())));
}

#[test]
fn an_opencode_tab_of_a_qcode_template_attaches_to_its_profiles_server_and_one_of_base_runs_its_own() {
    let scratch = Scratch::new("shared-program");
    let mut screen = opencode_tabs(&scratch);
    shown(&mut screen, 0, "ses_first");

    let words = spelled(&screen.launch_command(tab_at(&screen, 0).key()).expect("the tab has a command"));
    let tail = &words[words.len() - 6..];
    assert_eq!(tail[..2], ["sh", "-c"], "{words:?}");
    assert!(tail[2].contains("opencode attach"), "{words:?}");
    assert_eq!(tail[3..], ["sh", "/run/qcode-mcp/qcode-opencode.mjs", "ses_first"], "{words:?}");

    let words = spelled(&screen.launch_command(tab_at(&screen, 3).key()).expect("the tab has a command"));
    assert_eq!(&words[words.len() - 2..], ["opencode", "--auto"], "base is opencode as it comes: {words:?}");
    let env = envs(&screen.launch_command(tab_at(&screen, 3).key()).expect("the tab has a command"));
    assert!(!env.contains_key(shared::SERVER_VARIABLE), "{env:?}");
}

#[test]
fn the_tabs_of_one_profile_carry_one_server_token_which_is_no_tabs_own() {
    let scratch = Scratch::new("shared-token");
    let screen = opencode_tabs(&scratch);
    let env_of = |index: usize| envs(&screen.launch_command(tab_at(&screen, index).key()).expect("a command"));
    let (first, second, other) = (env_of(0), env_of(1), env_of(2));

    let server = first.get(shared::SERVER_VARIABLE).expect("the server's token").clone();
    assert_eq!(second.get(shared::SERVER_VARIABLE), Some(&server), "one server for the profile's tabs");
    assert_ne!(other.get(shared::SERVER_VARIABLE), Some(&server), "another profile has a server of its own");
    // A closed tab's processes are ended by the tab's own token, which the server never carries.
    for (index, env) in [(0, &first), (1, &second)] {
        assert_eq!(env.get("QCODE_BRIDGE").map(String::as_str), Some(tab_at(&screen, index).token()));
        assert_ne!(tab_at(&screen, index).token(), server);
    }
    assert_eq!(server.len(), 32, "{server}");

    let config: serde_json::Value =
        serde_json::from_str(first.get("OPENCODE_CONFIG_CONTENT").expect("opencode's configuration")).expect("JSON");
    assert_eq!(config["plugin"][0], "file:///run/qcode-mcp/qcode-opencode-plugin.mjs", "{config}");
    assert_eq!(config["mcp"]["qcode"]["enabled"], false, "the server's MCP bridge could not tell tabs apart");
    assert_eq!(first.get(shared::PORT_VARIABLE).map(String::as_str), Some("41418"));
    assert_eq!(first.get(shared::DIR_VARIABLE).map(String::as_str), Some("/work"));
}

#[test]
fn a_provider_profiles_server_is_started_by_the_relay_and_asks_it_with_the_servers_token() {
    let scratch = Scratch::new("shared-provider");
    let path = measured_providers_file("opencode-shared", "ev1", "qwen3.8", 27_648);
    let profiles = vec![Profile { harness: HarnessKind::OpenCode, ..provider_profile("oc-prov", "ev1", "qwen3.8") }];
    let mut screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .with_providers_path(Some(path.clone()));
    apply(&mut screen, Msg::OpenWorkspace(0));
    open(&mut screen, Choice::NewChat("oc-prov".to_owned()));
    shown(&mut screen, 0, "ses_prov");

    let command = screen.launch_command(tab_at(&screen, 0).key()).expect("the tab has a command");
    let words = spelled(&command);
    assert_eq!(words[words.len() - 3..], ["sh", "/run/qcode-mcp/qcode-opencode.mjs", "ses_prov"], "{words:?}");
    let env = envs(&command);
    assert_eq!(env.get(shared::RELAY_VARIABLE).map(String::as_str), Some("/run/qcode-mcp/qcode-relay.mjs"));
    let server = env.get(shared::SERVER_VARIABLE).expect("the server's token");
    let config: serde_json::Value =
        serde_json::from_str(env.get("OPENCODE_CONFIG_CONTENT").expect("the configuration")).expect("JSON");
    // The relay knows the server by the token its requests carry, and the server outlives the tab.
    assert_eq!(&config["provider"]["ev1"]["options"]["apiKey"], server.as_str(), "{config}");
    assert_eq!(config["provider"]["ev1"]["options"]["baseURL"], "http://127.0.0.1:41417/v1");
    assert_eq!(config["model"], "ev1/qwen3.8");
    assert_eq!(config["plugin"][0], "file:///run/qcode-mcp/qcode-opencode-plugin.mjs");
    // A value only the measurement on the Providers page could have put there.
    assert_eq!(config["provider"]["ev1"]["models"]["qwen3.8"]["limit"]["context"], 27_648);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_tab_opens_the_conversation_its_server_chose_and_keeps_it() {
    let scratch = Scratch::new("shared-shown");
    let mut screen = opencode_tabs(&scratch);
    assert_eq!(tab_at(&screen, 1).conversation(), None);
    shown(&mut screen, 1, "ses_second");
    assert_eq!(tab_at(&screen, 1).conversation(), Some("ses_second"));

    // An answer to an earlier start is not this start's.
    let key = tab_at(&screen, 0).key();
    let stale = tab_at(&screen, 0).run() + 1;
    apply(&mut screen, Msg::Woken(key, stale, Ok(()), None, Some("ses_stale".to_owned())));
    assert_eq!(tab_at(&screen, 0).conversation(), None);
}

/// Asks the screen `request` with `token` and `session`, as the workspace's socket would bring it.
fn ask(screen: &mut WorkspaceScreen, token: String, session: Option<&str>, request: Request) -> Receiver<Answer> {
    let (call, answers) = Call::new(Ok(Question { token, session: session.map(str::to_owned), request }));
    drop(super::super::bridge::answer(screen, "firefly", call, std::time::Instant::now()));
    answers
}

fn server_token(screen: &WorkspaceScreen, index: usize) -> String {
    envs(&screen.launch_command(tab_at(screen, index).key()).expect("a command"))
        .remove(shared::SERVER_VARIABLE)
        .expect("the server's token")
}

#[test]
fn the_bridge_knows_a_tab_of_a_shared_server_by_the_servers_token_and_the_conversation_it_shows() {
    let scratch = Scratch::new("shared-bridge");
    let mut screen = opencode_tabs(&scratch);
    shown(&mut screen, 0, "ses_first");
    shown(&mut screen, 1, "ses_second");
    shown(&mut screen, 2, "ses_other");
    let server = server_token(&screen, 0);
    let second = tab_at(&screen, 1).number().to_string();
    let first = tab_at(&screen, 0).number().to_string();

    // The list leaves out the tab that asks, so it names which tab QCode took the question for.
    let answer = ask(&mut screen, server.clone(), Some("ses_second"), Request::List).recv().expect("an answer");
    assert!(answer.ok, "{answer:?}");
    let listed: Vec<String> = answer.tabs.expect("tabs").into_iter().map(|tab| tab.tab).collect();
    assert!(!listed.contains(&second) && listed.contains(&first), "{listed:?}");

    let answer = ask(&mut screen, server.clone(), Some("ses_first"), Request::List).recv().expect("an answer");
    let listed: Vec<String> = answer.tabs.expect("tabs").into_iter().map(|tab| tab.tab).collect();
    assert!(listed.contains(&second) && !listed.contains(&first), "{listed:?}");

    // The server's token alone, a conversation no tab of its profile shows, and a conversation of
    // another profile's tab each name nobody.
    let stranger = t!("bridge.answer.stranger");
    for session in [None, Some("ses_nobody"), Some("ses_other")] {
        let answer = ask(&mut screen, server.clone(), session, Request::List).recv().expect("an answer");
        assert_eq!((answer.ok, answer.text.as_str()), (false, stranger.as_str()), "{session:?}");
    }
    // Another profile's server cannot speak for this profile's tab by naming its conversation.
    let other = server_token(&screen, 2);
    let answer = ask(&mut screen, other, Some("ses_first"), Request::List).recv().expect("an answer");
    assert!(!answer.ok, "{answer:?}");
}

/// A stand-in podman for an opencode profile: no container or volume until one is made, the
/// image there once `built` exists, every call written down, and the shared server's `ready`
/// answered with a conversation of its own.
struct Serving {
    scratch: Scratch,
    binary: PathBuf,
    calls: PathBuf,
}

impl Serving {
    fn new(name: &str, image: bool) -> Self {
        let scratch = Scratch::new(name);
        let (binary, calls, built) = (scratch.0.join("engine"), scratch.0.join("calls"), scratch.0.join("built"));
        if image {
            fs::write(&built, "").expect("the image is there");
        }
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {calls}\n\
             for last; do :; done\n\
             case \"$1 $2\" in\n\
             'container inspect') echo \"Error: no such container $last\" >&2; exit 125 ;;\n\
             'image inspect') [ -e {built} ] && {{ echo sha256:0123; exit 0; }}; echo \"Error: $last: image not known\" >&2; exit 125 ;;\n\
             'volume inspect') echo \"Error: no such volume $last\" >&2; exit 125 ;;\n\
             esac\n\
             [ \"$1\" = build ] && {{ echo \"COMMIT $3\"; touch {built}; exit 0; }}\n\
             case \"$*\" in *qcode-opencode.mjs\\ ready*) echo ses_fromready; exit 0 ;; esac\n\
             exit 0\n",
            calls = calls.display(),
            built = built.display(),
        );
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        Self { scratch, binary, calls }
    }

    fn screen(&self) -> WorkspaceScreen {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile("opencode", HarnessKind::OpenCode)],
        )];
        WorkspaceScreen::new(
            Some(Engine::new(EngineKind::Podman, &self.binary)),
            HostUser::Ids { uid: 1000, gid: 1000 },
            workspaces,
        )
    }

    fn readies(&self) -> usize {
        fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .filter(|call| call.contains("qcode-opencode.mjs ready"))
            .count()
    }
}

fn first_tab(harness: &Harness<Screen>) -> &Tab {
    &harness.app().0.workspace().expect("a workspace").tabs()[0]
}

#[test]
fn a_new_opencode_tab_asks_its_server_for_its_conversation_before_it_starts() {
    let engine = Serving::new("shared-wake", true);
    let mut harness = harness(engine.screen(), 120, 36);
    harness.click_text("New tab").render();
    harness.click_text("New chat").render();
    assert_eq!(engine.readies(), 1, "{}", fs::read_to_string(&engine.calls).unwrap_or_default());
    assert_eq!(first_tab(&harness).conversation(), Some("ses_fromready"));
    assert!(engine.scratch.paths().mcp().join(shared::SCRIPT_NAME).is_file(), "the script is written first");
}

#[test]
fn an_opencode_tab_whose_image_was_just_built_asks_its_server_too() {
    let engine = Serving::new("shared-built", false);
    let mut harness = harness(engine.screen(), 120, 36);
    harness.click_text("New tab").render();
    harness.click_text("New chat").render();
    harness.click_text("Build it now").render();
    assert_eq!(engine.readies(), 1, "{}", harness.screen());
    assert_eq!(first_tab(&harness).conversation(), Some("ses_fromready"), "{}", harness.screen());
}

#[test]
fn a_tab_brought_back_from_the_last_session_asks_its_server_for_the_conversation_it_showed() {
    let engine = Serving::new("shared-restored", true);
    let mut screen = engine.screen();
    let record = crate::store::SessionWorkspace {
        id: WorkspaceId::parse("firefly").expect("a usable id"),
        active_tab: 0,
        tabs: vec![crate::store::SessionTab {
            kind: crate::store::SessionTabKind::Profile("opencode".to_owned()),
            conversation: Some("ses_lasttime".to_owned()),
            opened: 1,
            number: None,
            name: None,
        }],
    };
    screen.restore_tabs(0, &record);
    let harness = harness(screen, 120, 36);
    let calls = fs::read_to_string(&engine.calls).unwrap_or_default();
    assert!(calls.lines().any(|call| call.ends_with("qcode-opencode.mjs ready ses_lasttime")), "{calls}");
    assert_eq!(first_tab(&harness).conversation(), Some("ses_fromready"), "the server's answer is the one kept");
}
