//! A desktop harness's tab: the window it opens, the states it goes through, and what it says in
//! each of them.
//!
//! The engine of these tests cannot be run, so no window ever opens; what is checked is the
//! command that would open it — where the decisions live — and the tab's own behaviour, which is
//! driven by handing it the messages the engine and the compositor would send.

use super::*;

use std::ffi::OsStr;

use qframe::runtime::OpenOutcome;

use crate::base::paths::MCP_DIR;
use crate::desktop::{Display, NoDisplay, RUNTIME_DIR};
use crate::ui::workspace::{Opening, entry};

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

    /// A screen with one workspace that carries one desktop profile, on this stand-in session.
    fn screen(&self) -> WorkspaceScreen {
        self.on(Ok(self.display()))
    }

    /// The same, on whatever this machine is said to offer.
    fn on(&self, display: Result<Display, NoDisplay>) -> WorkspaceScreen {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile(WINDOW, HarnessKind::AntigravityIde)],
        )];
        WorkspaceScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces).showing_on(display)
    }
}

/// A stand-in engine: a script that writes down every call it is given, answers the reading of a
/// settings file with "there is none", keeps what is written into one, and refuses to run the
/// window itself so that nothing of this test waits on a container that will never be there.
struct Recording {
    scratch: Scratch,
    binary: PathBuf,
    calls: PathBuf,
    written: PathBuf,
    /// The first line of what the carrier of a sign-in's way back was given, the way the
    /// application inside would have read it.
    carried: PathBuf,
}

impl Recording {
    fn new(name: &str) -> Self {
        Self::answering(name, "")
    }

    /// The same stand-in, answering an exec that starts the sign-in window the way an image built
    /// before there was one does.
    fn without_browser(name: &str) -> Self {
        let answer = format!(
            "for word in \"$@\"; do [ \"$word\" = {program} ] && {{ echo {said}; exit 0; }}; done\n",
            program = crate::desktop::signin::BROWSER_PROGRAM,
            said = crate::desktop::signin::NO_BROWSER,
        );
        Self::answering(name, &answer)
    }

    fn answering(name: &str, first: &str) -> Self {
        let scratch = Scratch::new(name);
        let (binary, calls, written) = (scratch.0.join("engine"), scratch.0.join("calls"), scratch.0.join("written"));
        let carried = scratch.0.join("carried");
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {calls}\n\
             [ \"$1\" = run ] && exit 1\n\
             [ \"$1 $2 $4\" = 'exec --interactive node' ] && {{ head -n 1 > {carried}; printf '{ANSWER}'; exit 0; }}\n\
             {first}\
             for word in \"$@\"; do\n\
             case \"$word\" in\n\
             *'exit 3'*) exit 3 ;;\n\
             *'qcode-new'*) cat > {written}; exit 0 ;;\n\
             esac\n\
             done\n\
             exit 0\n",
            calls = calls.display(),
            written = written.display(),
            carried = carried.display(),
        );
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        Self { scratch, binary, calls, written, carried }
    }

    /// A screen on this engine with the one desktop profile, and a display to open a window on.
    fn screen(&self) -> WorkspaceScreen {
        let runtime = self.scratch.0.join("runtime");
        fs::create_dir_all(&runtime).expect("a runtime folder");
        let socket = runtime.join("wayland-1");
        fs::write(&socket, "").expect("a stand-in compositor socket");
        let display = Display { socket, name: "wayland-1".to_owned(), device: None };
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile(WINDOW, HarnessKind::AntigravityIde)],
        )];
        WorkspaceScreen::new(
            Some(Engine::new(EngineKind::Podman, &self.binary)),
            HostUser::Ids { uid: 1000, gid: 1000 },
            workspaces,
        )
        .showing_on(Ok(display))
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }
}

/// What the application inside answers the sign-in's way back with, as `printf` is given it.
const ANSWER: &str = "HTTP/1.1 200 OK\\r\\nContent-Length: 13\\r\\n\\r\\nsigned-in-417";

/// A port nobody on this machine listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|socket| socket.local_addr()).expect("a free port").port()
}

/// Whether this machine could listen on `port` right now: nobody holds it.
fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Whether `port` can be listened on again. Asked a few times over a generous while, because a
/// test beside this one may hold the same port for an instant while it looks for a free one.
fn given_up(port: u16) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !port_is_free(port) {
        if std::time::Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Google's sign-in page for an application that waits on `port` of its own `localhost`.
fn google(port: u16) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Foauth-callback&state=s"
    )
}

/// Plays the browser Google sends back: one request to `port` of this machine, and the answer.
fn come_back(port: u16, target: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("QCode listens there");
    stream.set_read_timeout(Some(Duration::from_secs(60))).expect("a finite wait");
    write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").expect("the request goes");
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer
}

/// The choice a blank tab's page offers for the window.
fn window() -> Choice {
    Choice::Window(WINDOW.to_owned())
}

/// The tab of `key` in the open workspace.
fn tab(screen: &WorkspaceScreen, key: TabKey) -> &Tab {
    screen.workspace().expect("a workspace").tabs().iter().find(|tab| tab.key() == key).expect("the tab")
}

/// The words of the command that would open the window of the open workspace's desktop profile.
fn opening_words(screen: &WorkspaceScreen) -> Vec<String> {
    let open = screen.workspace().expect("a workspace");
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
    let open = screen.workspace().expect("a workspace");
    let window = open.plan(&TabKind::Desktop(WINDOW.to_owned())).expect("the window is planned");
    let harness = open.plan(&TabKind::Profile(WINDOW.to_owned())).expect("the harness container is planned");
    assert_eq!(window.name, "qcode-firefly-anti.desk");
    assert_ne!(window.name, harness.name, "the window has a container of its own");
    // The same image and the same home volume, so the window and the profile's terminal work share
    // their settings, their history and their sign-in.
    assert_eq!(window.image, harness.image);
    assert_eq!(window.home, harness.home);
    assert!(window.window.is_some() && harness.window.is_none());
    // Both are given the bridge between tabs, because the agent inside the window asks over the
    // same socket as any other; the same folder, so they reach the same QCode.
    assert_eq!(window.bridge, harness.bridge, "the window's agent reaches the other tabs too");
    assert!(window.bridge.is_some(), "a window's agent has nothing to ask over");
}

#[test]
fn the_window_sees_the_folder_the_bridges_socket_lives_in_read_only() {
    let session = Session::new("window-bridge");
    let words = opening_words(&session.screen());
    let folder = session.scratch.paths().mcp();
    // Read-only, as for a harness tab: a container may speak over the socket, never replace the
    // server every tab of the workspace starts.
    assert!(words.contains(&format!("{}:{}:ro,z", folder.display(), MCP_DIR)), "{words:?}");
}

#[test]
fn the_window_sees_the_workspace_the_home_volume_and_one_socket_of_this_machine() {
    let session = Session::new("window-command");
    let words = opening_words(&session.screen());
    assert_eq!(words[..6], ["run", "--detach", "--init", "--shm-size=1g", "--name", "qcode-firefly-anti.desk"]);
    assert!(words.contains(&format!("{RUNTIME_DIR}:rw,mode=0700,U")), "{words:?}");
    assert!(words.contains(&format!("/run/user/1000/wayland-1:{RUNTIME_DIR}/wayland-1:rw")), "{words:?}");
    assert!(words.contains(&format!("XDG_RUNTIME_DIR={RUNTIME_DIR}")), "{words:?}");
    assert!(words.contains(&"WAYLAND_DISPLAY=wayland-1".to_owned()), "{words:?}");
    assert!(words.contains(&"qcode-home-firefly-anti:/home/qcode:rw,z".to_owned()), "{words:?}");
    // The program, its one flag and the workspace folder, last as always.
    assert_eq!(
        &words[words.len() - 3..],
        ["/opt/antigravity-ide/antigravity-ide", "--ozone-platform=wayland", CODE_DIR]
    );
    // What the machine keeps to itself: the runtime folder, the engine's socket, the session bus.
    assert!(!words.iter().any(|word| word.ends_with("/run/user/1000") || word.contains("podman.sock")), "{words:?}");
}

#[test]
fn the_window_is_told_where_to_leave_an_address_it_wants_opened() {
    let session = Session::new("window-browser");
    let words = opening_words(&session.screen());
    // The folder the address is written into is the workspace's own and is writable, because
    // writing the address is the whole point of it.
    let folder = session.scratch.paths().browser();
    assert!(words.contains(&format!("{}:{}:rw,z", folder.display(), crate::desktop::signin::OPEN_DIR)), "{words:?}");
    // And the application is told to run QCode's little program rather than look for a browser
    // it has not got.
    assert!(words.contains(&format!("BROWSER={}", crate::desktop::signin::OPEN_PROGRAM)), "{words:?}");
}

/// The tab of a window that is open, on the stand-in engine `engine`, in a harness.
fn open_window_on(engine: &Recording) -> (Harness<Screen>, TabKey) {
    let mut screen = engine.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    assert_eq!(state(&screen, 0, 0), TabState::Running);
    (harness(screen, SIZE.0, SIZE.1), key)
}

#[test]
fn a_sign_in_opens_in_the_persons_browser_after_its_way_back_is_listened_for() {
    let engine = Recording::new("window-signin-browser");
    let (mut harness, key) = open_window_on(&engine);
    let port = free_port();
    let address = google(port);
    harness.set_open_outcome(OpenOutcome::Opened);
    // The address arrives the way the window's own program leaves it.
    harness.send(Msg::SignInWanted(key, 0, vec![address.clone()]));

    // The page goes to this machine's browser, through the framework's one door, which the
    // harness records instead of carrying out.
    let asked = harness.opens().last().expect("the address was handed over to be opened");
    assert_eq!(asked.target.as_deref(), Some(OsStr::new(&address)), "the address is what is opened: {asked:?}");
    // Not to the window inside the container, which Google turns away.
    let calls = engine.calls();
    assert!(!calls.iter().any(|call| call.contains(crate::desktop::signin::BROWSER_PROGRAM)), "{calls:?}");
    // And the port the sign-in comes back to is this machine's to answer now.
    assert!(!port_is_free(port), "QCode listens on {port}");

    harness.render();
    let text = harness.screen();
    assert!(text.contains("accounts.google.com"), "the address is on the tab:\n{text}");
    assert!(text.contains("opened in your browser"), "{text}");
    assert!(text.contains("QCode passes the answer"), "{text}");
    assert!(text.contains(&format!("port {port}")) && text.contains("10 minutes"), "{text}");
}

#[test]
fn the_browser_sent_back_reaches_the_application_and_the_tab_says_it_arrived() {
    let engine = Recording::new("window-signin-back");
    let (mut harness, key) = open_window_on(&engine);
    let port = free_port();
    harness.set_open_outcome(OpenOutcome::Opened);
    harness.send(Msg::SignInWanted(key, 0, vec![google(port)]));

    // Google sends the browser back with the code; QCode takes the connection at its next look.
    let browser = std::thread::spawn(move || come_back(port, "/oauth-callback?code=4%2F0Ab-417&state=s"));
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !browser.is_finished() && std::time::Instant::now() < deadline {
        harness.advance(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
    }
    let answer = browser.join().expect("the browser was answered");
    assert!(answer.ends_with("signed-in-417"), "the application's own answer reaches the browser: {answer}");
    // Carried to the application's container, with nothing typed at it through a terminal.
    let carried = fs::read_to_string(&engine.carried).expect("the request was carried in");
    assert_eq!(carried.trim_end(), "GET /oauth-callback?code=4%2F0Ab-417&state=s HTTP/1.1");
    let calls = engine.calls();
    let carrier = calls
        .iter()
        .find(|call| call.starts_with("exec --interactive qcode-firefly-anti.desk node -e "))
        .unwrap_or_else(|| panic!("carried inside the window's own container: {calls:?}"));
    assert!(!carrier.contains("--tty"), "{carrier}");
    // The program runs to more than one line; its two words follow the last of them.
    let written = fs::read_to_string(&engine.calls).expect("the calls were written down");
    assert!(written.contains(&format!("reach(0);\n {port} 127.0.0.1,::1\n")), "{written}");

    // The listening sees that the sign-in arrived and gives the port up.
    harness.advance(Duration::from_millis(200));
    assert!(given_up(port), "the port is given up once the sign-in has come back");
    let text = harness.screen();
    assert!(text.contains("The sign-in reached the application"), "{text}");
}

#[test]
fn a_sign_in_nobody_comes_back_from_gives_the_port_up_after_the_wait_and_says_so() {
    let engine = Recording::new("window-signin-wait");
    let (mut harness, key) = open_window_on(&engine);
    let port = free_port();
    harness.set_open_outcome(OpenOutcome::Opened);
    harness.send(Msg::SignInWanted(key, 0, vec![google(port)]));
    harness.advance(crate::desktop::callback::WAIT - Duration::from_secs(1));
    assert!(!port_is_free(port), "a second short of the wait, it still listens");
    harness.advance(Duration::from_secs(2));
    assert!(given_up(port), "the port is not held past the wait");
    let text = harness.screen();
    assert!(text.contains("Nothing came back to port"), "{text}");
    assert!(text.contains("QCode stopped"), "{text}");
}

#[test]
fn a_port_another_program_holds_keeps_the_page_closed_and_says_what_to_do() {
    let engine = Recording::new("window-signin-taken");
    let (mut harness, key) = open_window_on(&engine);
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("a program of someone else's");
    let port = holder.local_addr().expect("its address").port();
    harness.set_open_outcome(OpenOutcome::Opened);
    harness.send(Msg::SignInWanted(key, 0, vec![google(port)]));
    // Opening it would hand the sign-in to whoever holds the port.
    assert_eq!(harness.opens(), [], "nothing is opened");
    let text = harness.screen();
    assert!(text.contains(&format!("already listens on port {port}")), "{text}");
    assert!(text.contains("the page was not opened"), "{text}");
    assert!(text.contains("Use the sign-in window here"), "the way round it is offered: {text}");
    drop(holder);
}

#[test]
fn the_sign_in_window_here_is_a_button_away_and_gives_the_port_up() {
    let engine = Recording::new("window-signin-here");
    let (mut harness, key) = open_window_on(&engine);
    let port = free_port();
    let address = google(port);
    harness.set_open_outcome(OpenOutcome::Opened);
    harness.send(Msg::SignInWanted(key, 0, vec![address.clone()]));
    assert!(!port_is_free(port));

    harness.click_text("Use the sign-in window here");
    // Shown by the application's own Electron inside the window's container, on the same
    // compositor, with the address.
    let calls = engine.calls();
    let shown = calls
        .iter()
        .find(|call| call.starts_with("exec qcode-firefly-anti.desk sh -c "))
        .unwrap_or_else(|| panic!("the page is shown inside the window's container: {calls:?}"));
    assert!(
        shown.ends_with(&format!("{} --ozone-platform=wayland {address}", crate::desktop::signin::BROWSER_PROGRAM)),
        "{shown}"
    );
    // That window's `localhost` is the application's: nothing is carried, and the port goes.
    harness.advance(Duration::from_millis(200));
    assert!(given_up(port), "the port is given up");
    let text = harness.screen();
    assert!(text.contains("small sign-in window"), "{text}");
    assert!(text.contains("Google refuses"), "what Google does there is said: {text}");
    assert!(!text.contains("Use the sign-in window here"), "the button has done its work: {text}");
}

#[test]
fn the_sign_in_window_asked_for_on_an_old_image_says_how_to_get_one() {
    let engine = Recording::without_browser("window-signin-old-image");
    let (mut harness, key) = open_window_on(&engine);
    harness.set_open_outcome(OpenOutcome::Opened);
    harness.send(Msg::SignInWanted(key, 0, vec![google(free_port())]));
    harness.click_text("Use the sign-in window here");
    assert!(engine.calls().iter().any(|call| call.starts_with("exec qcode-firefly-anti.desk sh -c ")));
    let text = harness.screen();
    assert!(text.contains("has no sign-in window"), "{text}");
    assert!(text.contains("Rebuild image"), "{text}");
}

#[test]
fn a_page_with_no_way_back_to_this_machine_only_opens() {
    let engine = Recording::new("window-signin-plain");
    let (mut harness, key) = open_window_on(&engine);
    let address = "https://antigravity.google/docs/getting-started";
    harness.set_open_outcome(OpenOutcome::Opened);
    harness.send(Msg::SignInWanted(key, 0, vec![address.to_owned()]));
    let asked = harness.opens().last().expect("the address was handed over to be opened");
    assert_eq!(asked.target.as_deref(), Some(OsStr::new(address)));
    let text = harness.screen();
    assert!(text.contains("it opened in your browser"), "{text}");
    assert!(!text.contains("listens on port"), "nothing is listened for: {text}");
}

#[test]
fn a_window_that_closes_gives_its_sign_ins_port_up() {
    let engine = Recording::new("window-signin-closed");
    let (mut harness, key) = open_window_on(&engine);
    let port = free_port();
    harness.set_open_outcome(OpenOutcome::Opened);
    harness.send(Msg::SignInWanted(key, 0, vec![google(port)]));
    assert!(!port_is_free(port));
    // What the wait on the container answers when the person closes the window.
    harness.send(Msg::WindowEnded(key, 0, Some(0)));
    harness.advance(Duration::from_millis(200));
    assert!(given_up(port), "nobody is left to carry a sign-in to");
}

#[test]
fn a_page_that_can_be_shown_nowhere_leaves_the_address_to_be_read() {
    // An engine that cannot be run at all: the sign-in window cannot start, and this machine has
    // no browser to be had either.
    let session = Session::new("window-signin-nowhere");
    let mut screen = session.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let address = "https://accounts.google.com/o/oauth2/auth?client_id=y";
    harness.set_open_outcome(OpenOutcome::Failed("no opener on this machine".to_owned()));
    harness.send(Msg::SignInWanted(key, 0, vec![address.to_owned()]));
    harness.render();
    let text = harness.screen();
    assert!(text.contains(address), "the address is on the tab:\n{text}");
    assert!(text.contains("it could not be shown here"), "{text}");
}

#[test]
fn a_window_tab_has_no_terminal_to_spawn_and_no_program_to_be_entered_with() {
    let session = Session::new("window-no-terminal");
    let mut screen = session.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    assert_eq!(kinds(&screen), [TabKind::Desktop(WINDOW.to_owned())]);
    assert_eq!(screen.launch_command(key), None, "nothing is spawned in a pseudo-terminal for a window");
    let open = screen.workspace().expect("a workspace");
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
    // The one sign-in the window asks for is said before it comes: in the person's own browser,
    // carried back, and remembered after that.
    assert!(shown.contains("The page opens in your"), "{shown}");
    assert!(shown.contains("QCode carries Google's answer back"), "{shown}");
    assert!(shown.contains("profile remembers you"), "{shown}");
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
    assert!(shown.contains("stay in this workspace"), "nothing of the person's was lost: {shown}");
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
    let record = SessionWorkspace {
        id: WorkspaceId::parse("firefly").expect("a usable id"),
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
    apply(&mut screen, Msg::OpenWorkspace(0));
    let run = tab(&screen, key).run();
    apply(&mut screen, Msg::WindowOpened(key, run, Ok(Opening::Waiting)));
    assert_eq!(tab(&screen, key).state(), &TabState::Ended { code: None });

    // And the session file keeps it, so it comes back again next time.
    let saved = screen.session();
    assert_eq!(saved.workspaces[0].tabs[0].kind, SessionTabKind::Desktop(WINDOW.to_owned()));
}

#[test]
fn asking_for_the_window_lifts_the_hold_a_restored_tab_has() {
    let session = Session::new("window-asked");
    let mut screen = session.screen();
    let record = SessionWorkspace {
        id: WorkspaceId::parse("firefly").expect("a usable id"),
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
    assert_eq!(entry(&screen), "workspace-window");
}

#[test]
fn the_window_is_asked_to_come_forward_by_running_the_application_again_inside_its_container() {
    let session = Session::new("window-raise");
    let screen = session.screen();
    let open = screen.workspace().expect("a workspace");
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
            CODE_DIR
        ]
    );
    assert!(!words.iter().any(|word| word == "--tty"), "nothing is typed at it: {words:?}");
}

#[test]
fn a_profile_whose_harness_draws_in_a_terminal_has_no_window_to_plan() {
    let scratch = Scratch::new("window-none");
    let screen = one_workspace(&scratch);
    let open = screen.workspace().expect("a workspace");
    assert_eq!(open.plan(&TabKind::Desktop("claude-sub".to_owned())), None);
    for kind in [TabKind::Shell, TabKind::Profile("claude-sub".to_owned())] {
        let plan = open.plan(&kind).expect("a container is planned");
        assert!(plan.window.is_none(), "{kind:?}");
        assert_eq!(plan.open_window(screen.engine().expect("an engine"), screen_user(), &no_display(), None), None);
        assert_eq!(plan.raise_window(screen.engine().expect("an engine")), None);
        assert_eq!(plan.open_page(screen.engine().expect("an engine"), "https://example.com/"), None);
    }
}

fn screen_user() -> HostUser {
    HostUser::Ids { uid: 1000, gid: 1000 }
}

fn no_display() -> Display {
    Display { socket: PathBuf::from("/run/user/1000/wayland-1"), name: "wayland-1".to_owned(), device: None }
}

#[test]
fn choosing_the_window_writes_this_tabs_token_into_the_settings_before_the_window_is_started() {
    let engine = Recording::new("window-register");
    let mut harness = harness(engine.screen(), SIZE.0, SIZE.1);
    // Chosen on the blank tab's page, the way the person chooses it, with the background work
    // running as it does in the application.
    open_in(&mut harness, window());
    harness.render();

    let token = harness.app().0.workspace().expect("a workspace").tabs()[0].token().to_owned();
    let written = fs::read_to_string(&engine.written).expect("the settings were written");
    let settings: serde_json::Value = serde_json::from_str(&written).expect("they are JSON");
    assert_eq!(settings["mcpServers"]["qcode"]["env"]["QCODE_BRIDGE"], token, "{written}");
    assert_eq!(settings["mcpServers"]["qcode"]["command"], "node", "{written}");

    // Written from a container of its own on the window's home volume, and finished with before
    // the window is run: the application reads its settings as it starts.
    let calls = engine.calls();
    let scribe = calls
        .iter()
        .find(|call| call.starts_with("create --name qcode-firefly-anti.mcp"))
        .unwrap_or_else(|| panic!("a container of its own: {calls:?}"));
    assert!(scribe.contains("qcode-home-firefly-anti:/home/qcode"), "on the window's own home: {scribe}");
    assert!(scribe.contains("--network=none"), "it needs no network: {scribe}");
    let removed = calls
        .iter()
        .rposition(|call| call.starts_with("rm --force") && call.contains("qcode-firefly-anti.mcp"))
        .expect("the scribe is taken away again");
    let run = calls.iter().position(|call| call.starts_with("run ")).expect("the window is run");
    assert!(removed < run, "the settings are finished with before the window starts: {calls:?}");
}

/// QCode itself on the workspace screen `screen`, in a harness, so a key reaches the application's
/// own quit rather than the screen's.
fn qcode_on(screen: WorkspaceScreen) -> Harness<crate::QCode> {
    use crate::testing::{app_with_absent_engine, config, harness, scratch};
    let app = app_with_absent_engine(config(&scratch("quit-asks"), &[])).with_workspace(screen);
    harness(app, SIZE.0, SIZE.1)
}

#[test]
fn ctrl_q_asks_once_while_an_agents_window_is_open_and_staying_keeps_everything() {
    let engine = Recording::new("quit-stay");
    let mut screen = engine.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    let mut harness = qcode_on(screen);
    harness.press("ctrl+q").render();
    assert!(!harness.quit_requested(), "an agent is at work, so the key asks first");
    let asked = harness.screen();
    assert!(asked.contains("Quit while an agent is at work?"), "{asked}");
    assert!(asked.contains("An agent tab is still running"), "{asked}");
    harness.click_text("Stay").render();
    assert!(!harness.quit_requested(), "staying stays");
    assert!(!harness.screen().contains("Quit while an agent is at work?"), "{}", harness.screen());
    // Staying is forgotten: the next Ctrl+Q asks again rather than leaving on the spot.
    harness.press("ctrl+q").render();
    assert!(!harness.quit_requested());
    assert!(harness.screen().contains("Quit while an agent is at work?"), "{}", harness.screen());
    harness.click_text("Quit anyway").render();
    assert!(harness.quit_requested(), "the person said so");
}

#[test]
fn a_second_ctrl_q_while_the_question_stands_is_the_answer() {
    let engine = Recording::new("quit-twice");
    let mut screen = engine.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    let mut harness = qcode_on(screen);
    harness.press("ctrl+q").render();
    assert!(!harness.quit_requested());
    harness.press("ctrl+q").render();
    assert!(harness.quit_requested(), "pressing it again means it");
}

#[test]
fn ctrl_q_leaves_without_a_question_when_the_agents_window_has_closed() {
    let engine = Recording::new("quit-closed");
    let mut screen = engine.screen();
    open(&mut screen, window());
    let key = key(&screen, 0);
    apply(&mut screen, Msg::WindowOpened(key, 0, Ok(Opening::Up)));
    apply(&mut screen, Msg::WindowEnded(key, 0, Some(0)));
    let mut harness = qcode_on(screen);
    harness.press("ctrl+q").render();
    assert!(harness.quit_requested(), "nothing is at work, so nothing is asked:\n{}", harness.screen());
}
