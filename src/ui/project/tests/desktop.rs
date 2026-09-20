//! A desktop harness's tab: the window it opens, the states it goes through, and what it says in
//! each of them.
//!
//! The engine of these tests cannot be run, so no window ever opens; what is checked is the
//! command that would open it — where the decisions live — and the tab's own behaviour, which is
//! driven by handing it the messages the engine and the compositor would send.

use super::*;

use crate::desktop::{Display, NoDisplay, RUNTIME_DIR};
use crate::ui::project::{Opening, entry};

/// The desktop profile of these tests.
const WINDOW: &str = "anti";

/// A stand-in compositor: a folder with a socket file in it, and the display that names it.
struct Session {
    scratch: Scratch,
    socket: PathBuf,
}

impl Session {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let runtime = scratch.0.join("runtime");
        fs::create_dir_all(&runtime).expect("a runtime folder");
        let socket = runtime.join("wayland-1");
        fs::write(&socket, "").expect("a stand-in compositor socket");
        Self { scratch, socket }
    }

    fn display(&self) -> Display {
        Display { socket: self.socket.clone(), name: "wayland-1".to_owned(), device: None }
    }

    /// A screen with one project that carries one desktop profile, on this stand-in session.
    fn screen(&self) -> ProjectScreen {
        self.on(Ok(self.display()))
    }

    /// The same, on whatever this machine is said to offer.
    fn on(&self, display: Result<Display, NoDisplay>) -> ProjectScreen {
        let projects = vec![project(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile(WINDOW, HarnessKind::AntigravityIde)],
        )];
        ProjectScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, projects).showing_on(display)
    }
}

/// The choice a blank tab's page offers for the window.
fn window() -> Choice {
    Choice::Window(WINDOW.to_owned())
}

/// The tab of `key` in the open project.
fn tab(screen: &ProjectScreen, key: TabKey) -> &Tab {
    screen.project().expect("a project").tabs().iter().find(|tab| tab.key() == key).expect("the tab")
}

/// The words of the command that would open the window of the open project's desktop profile.
fn opening_words(screen: &ProjectScreen) -> Vec<String> {
    let open = screen.project().expect("a project");
    let plan = open.plan(&TabKind::Desktop(WINDOW.to_owned())).expect("the window is planned");
    let engine = screen.engine().expect("an engine");
    let display =
        Display { socket: PathBuf::from("/run/user/1000/wayland-1"), name: "wayland-1".to_owned(), device: None };
    let command = plan
        .open_window(engine, HostUser::Ids { uid: 1000, gid: 1000 }, &display, None)
        .expect("the plan opens a window");
    assert_eq!(command.program, Path::new(NO_ENGINE), "a window only ever starts the engine binary");
    command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

#[test]
fn the_window_runs_in_a_container_of_its_own_beside_the_profiles_harness_container() {
    let session = Session::new("window-plan");
    let screen = session.screen();
    let open = screen.project().expect("a project");
    let window = open.plan(&TabKind::Desktop(WINDOW.to_owned())).expect("the window is planned");
    let harness = open.plan(&TabKind::Profile(WINDOW.to_owned())).expect("the harness container is planned");
    assert_eq!(window.name, "qcode-firefly-anti.desk");
    assert_ne!(window.name, harness.name, "the window has a container of its own");
    // The same image and the same home volume, so the window and the profile's terminal work share
    // their settings, their history and their sign-in.
    assert_eq!(window.image, harness.image);
    assert_eq!(window.home, harness.home);
    assert!(window.window.is_some() && harness.window.is_none());
    // The harness container answers to an agent and is given the bridge between tabs; the window
    // runs the application itself, which is nobody's agent, so it never sees that socket.
    assert!(harness.bridge.is_some(), "a harness tab can be talked to");
    assert!(window.bridge.is_none(), "a window has no agent to talk to");
}

#[test]
fn the_window_sees_the_project_the_home_volume_and_one_socket_of_this_machine() {
    let session = Session::new("window-command");
    let words = opening_words(&session.screen());
    assert_eq!(words[..6], ["run", "--detach", "--init", "--shm-size=1g", "--name", "qcode-firefly-anti.desk"]);
    assert!(words.contains(&format!("{RUNTIME_DIR}:rw,mode=0700,U")), "{words:?}");
    assert!(words.contains(&format!("/run/user/1000/wayland-1:{RUNTIME_DIR}/wayland-1:rw")), "{words:?}");
    assert!(words.contains(&format!("XDG_RUNTIME_DIR={RUNTIME_DIR}")), "{words:?}");
    assert!(words.contains(&"WAYLAND_DISPLAY=wayland-1".to_owned()), "{words:?}");
    assert!(words.contains(&"qcode-home-firefly-anti:/home/qcode:rw,z".to_owned()), "{words:?}");
    // The program, its one flag and the project folder, last as always.
    assert_eq!(
        &words[words.len() - 3..],
        ["/opt/antigravity-ide/antigravity-ide", "--ozone-platform=wayland", PROJECT_DIR]
    );
    // What the machine keeps to itself: the runtime folder, the engine's socket, the session bus.
    assert!(!words.iter().any(|word| word.ends_with("/run/user/1000") || word.contains("podman.sock")), "{words:?}");
}

#[test]
fn a_window_tab_has_no_terminal_to_spawn_and_no_program_to_be_entered_with() {
    let session = Session::new("window-no-terminal");
    let mut screen = session.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    assert_eq!(kinds(&screen), [TabKind::Desktop(WINDOW.to_owned())]);
    assert_eq!(screen.launch_command(key), None, "nothing is spawned in a pseudo-terminal for a window");
    let open = screen.project().expect("a project");
    assert_eq!(open.program(tab(&screen, key), screen.editor()), None);
    assert!(tab(&screen, key).session().is_none());
}

#[test]
fn the_tab_says_it_is_opening_while_the_container_starts() {
    let session = Session::new("window-opening");
    // Opened without letting the background work run, so the tab stands where it stands while the
    // container is starting; the engine of these tests could never open a window anyway.
    let mut screen = session.screen();
    open(&mut screen, window());
    assert_eq!(tab(&screen, key(&screen, 0)).state(), &TabState::Starting);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.render();
    assert!(harness.screen().contains("Opening the window"), "{}", harness.screen());
}

#[test]
fn the_open_window_is_said_plainly_with_the_two_things_that_can_be_done_to_it() {
    let session = Session::new("window-open");
    let mut screen = session.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    // What the engine would have said once the container was up. The wait that follows is not let
    // run: through it the engine of these tests would answer at once that the window had closed.
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    assert_eq!(tab(&screen, key).state(), &TabState::Running);

    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.render();
    let shown = harness.screen();
    assert!(shown.contains("Window open"), "{shown}");
    assert!(shown.contains("Bring to front") && shown.contains("Close the window"), "{shown}");
    // The one thing QCode cannot do for the person is said on the tab rather than left to be found
    // out in the window.
    assert!(shown.contains("sign in"), "{shown}");
    // No box, no bracket: the state is said in words and colour.
    assert!(!shown.contains('[') && !shown.contains('\u{250c}'), "{shown}");
}

#[test]
fn the_person_closing_the_window_closes_the_tabs_window_and_leaves_the_way_back() {
    let session = Session::new("window-closed");
    let mut screen = session.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    // What the wait on the container answers when the person closes the window.
    apply(&mut screen, Msg::WindowEnded(key, 0, Some(0)));
    assert_eq!(tab(&screen, key).state(), &TabState::Ended { code: Some(0) });

    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.render();
    let shown = harness.screen();
    assert!(shown.contains("Window closed"), "{shown}");
    assert!(shown.contains("Open the window"), "the way back is offered: {shown}");
    assert!(shown.contains("stay in this project"), "nothing of the person's was lost: {shown}");
}

#[test]
fn a_window_that_ended_badly_says_the_code_and_still_offers_the_way_back() {
    let session = Session::new("window-code");
    let mut screen = session.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    apply(&mut screen, Msg::WindowEnded(key, 0, Some(133)));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.render();
    assert!(harness.screen().contains("133"), "{}", harness.screen());
    assert!(harness.screen().contains("Open the window"), "{}", harness.screen());
}

#[test]
fn an_answer_from_a_run_the_tab_has_left_behind_changes_nothing() {
    let session = Session::new("window-stale");
    let mut screen = session.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    assert_eq!(tab(&screen, key).state(), &TabState::Running);
    // A wait left over from an earlier run of the same tab must not close the window now open.
    apply(&mut screen, Msg::WindowEnded(key, 0, Some(0)));
    let now = tab(&screen, key).state().clone();
    apply(&mut screen, Msg::WindowEnded(key, 99, Some(0)));
    assert_eq!(tab(&screen, key).state(), &now, "an answer of another run is ignored");
}

#[test]
fn a_machine_with_no_compositor_says_so_instead_of_opening_anything() {
    let session = Session::new("window-no-wayland");
    // Through the harness, so the work really runs: there is no display, so the tab is told why
    // before any engine command is built.
    let mut harness = harness(session.on(Err(NoDisplay::NoWayland)), SIZE.0, SIZE.1);
    open_in(&mut harness, window());
    harness.render();
    let shown = harness.screen();
    assert!(shown.contains("The window did not open"), "{shown}");
    assert!(shown.contains("Wayland"), "{shown}");
    // X11 is not offered as a way round it: there every program on the screen could read this
    // one's windows and keys.
    assert!(shown.contains("X11"), "the refusal says why, so nobody looks for the option: {shown}");
}

#[test]
fn a_compositor_that_restarted_says_which_socket_is_gone() {
    let session = Session::new("window-no-socket");
    let gone = PathBuf::from("/run/user/1000/wayland-42");
    let mut harness = harness(session.on(Err(NoDisplay::NoSocket(gone))), SIZE.0, SIZE.1);
    open_in(&mut harness, window());
    harness.render();
    assert!(harness.screen().contains("wayland-42"), "{}", harness.screen());
}

#[test]
fn a_window_tab_brought_back_from_the_last_session_waits_to_be_asked() {
    let session = Session::new("window-restore");
    let mut screen = session.screen();
    let record = SessionProject {
        id: ProjectId::parse("firefly").expect("a usable id"),
        active_tab: 0,
        tabs: vec![SessionTab {
            kind: SessionTabKind::Desktop(WINDOW.to_owned()),
            conversation: None,
            opened: 1_789_700_000,
        }],
    };
    screen.restore_tabs(0, &record);
    assert_eq!(kinds(&screen), [TabKind::Desktop(WINDOW.to_owned())]);
    let key = key(&screen, 0);
    assert!(tab(&screen, key).is_held(), "opening QCode again is not asking for a window on the screen");

    // Showing the tab wakes it, and waking a held tab settles it as closed rather than opening.
    apply(&mut screen, Msg::OpenProject(0));
    let run = tab(&screen, key).run();
    apply(&mut screen, Msg::WindowOpened(key, run, Ok(Opening::Waiting)));
    assert_eq!(tab(&screen, key).state(), &TabState::Ended { code: None });

    // And the session file keeps it, so it comes back again next time.
    let saved = screen.session();
    assert_eq!(saved.projects[0].tabs[0].kind, SessionTabKind::Desktop(WINDOW.to_owned()));
}

#[test]
fn asking_for_the_window_lifts_the_hold_a_restored_tab_has() {
    let session = Session::new("window-asked");
    let mut screen = session.screen();
    let record = SessionProject {
        id: ProjectId::parse("firefly").expect("a usable id"),
        active_tab: 0,
        tabs: vec![SessionTab {
            kind: SessionTabKind::Desktop(WINDOW.to_owned()),
            conversation: None,
            opened: 1_789_700_000,
        }],
    };
    screen.restore_tabs(0, &record);
    let key = key(&screen, 0);
    apply(&mut screen, Msg::Restart(key));
    assert!(!tab(&screen, key).is_held(), "the person asked for it");
    assert_eq!(tab(&screen, key).state(), &TabState::Starting);
}

#[test]
fn the_blank_pages_row_for_a_desktop_profile_opens_its_window_and_offers_no_conversations() {
    let session = Session::new("window-page");
    let mut harness = harness(session.screen(), SIZE.0, SIZE.1);
    harness.send(Msg::NewTab);
    harness.render();
    let shown = harness.screen();
    assert!(shown.contains("Open its window"), "{shown}");
    assert!(shown.contains("on your own screen"), "{shown}");
    // No conversation rows and no new chat: the shape of what the agent in the window writes was
    // never read, so nothing is offered that might not open.
    assert!(!shown.contains("New chat"), "{shown}");
}

#[test]
fn the_keyboard_lands_on_the_tabs_one_action_rather_than_on_a_terminal_that_is_not_there() {
    let session = Session::new("window-focus");
    let mut screen = session.screen();
    open(&mut screen, window());
    assert_eq!(entry(&screen), "project-window");
}

#[test]
fn the_window_is_asked_to_come_forward_by_running_the_application_again_inside_its_container() {
    let session = Session::new("window-raise");
    let screen = session.screen();
    let open = screen.project().expect("a project");
    let plan = open.plan(&TabKind::Desktop(WINDOW.to_owned())).expect("the window is planned");
    let command = plan.raise_window(screen.engine().expect("an engine")).expect("the plan can raise it");
    let words: Vec<String> = command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    // An editor of this family, started a second time on the same data folder, tells the instance
    // already running and exits. QCode has no activation token of the compositor's to hand it, so
    // this is as far as a terminal application can go.
    assert_eq!(
        words,
        [
            "exec",
            "qcode-firefly-anti.desk",
            "/opt/antigravity-ide/antigravity-ide",
            "--ozone-platform=wayland",
            PROJECT_DIR
        ]
    );
    assert!(!words.iter().any(|word| word == "--tty"), "nothing is typed at it: {words:?}");
}

#[test]
fn a_profile_whose_harness_draws_in_a_terminal_has_no_window_to_plan() {
    let scratch = Scratch::new("window-none");
    let screen = one_project(&scratch);
    let open = screen.project().expect("a project");
    assert_eq!(open.plan(&TabKind::Desktop("claude-sub".to_owned())), None);
    for kind in [TabKind::Shell, TabKind::Profile("claude-sub".to_owned())] {
        let plan = open.plan(&kind).expect("a container is planned");
        assert!(plan.window.is_none(), "{kind:?}");
        assert_eq!(plan.open_window(screen.engine().expect("an engine"), screen_user(), &no_display(), None), None);
        assert_eq!(plan.raise_window(screen.engine().expect("an engine")), None);
    }
}

fn screen_user() -> HostUser {
    HostUser::Ids { uid: 1000, gid: 1000 }
}

fn no_display() -> Display {
    Display { socket: PathBuf::from("/run/user/1000/wayland-1"), name: "wayland-1".to_owned(), device: None }
}
