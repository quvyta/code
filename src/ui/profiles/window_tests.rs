//! Signing a window harness in from the Profiles page, driven from where the person's hand is: the
//! list's Sign in, the page's button, and the window's own Done and Stop.
//!
//! No window opens. The engine is a script that writes down every call and answers the way the
//! engine would: the window's container runs, a look inside it finds the login once the test says
//! the application has written it, and the courier that takes it out drops the file the program
//! inside would have written. What is checked is what QCode does with those answers.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use qframe::env::{AssetDirs, Env};
use qframe::icons::GlyphMode;
use qframe::runtime::{Harness, OpenOutcome};

use super::*;
use crate::desktop::login::{LOOK_EVERY, STORED};
use crate::profile::{HarnessKind, MountAccess, NetworkMode, Template};

/// The screen on its own.
struct Host {
    state: Profiles,
}

impl App for Host {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        update(&mut self.state, message)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        AppShell::new().body(|ui| view(&self.state, ui)).show(ui);
    }
}

/// The Antigravity profile these tests sign in, as a definition file made before QCode stored
/// this login would describe it.
fn antigravity(name: &str) -> Profile {
    Profile {
        name: SafeName::parse(name).expect("the name is safe"),
        harness: HarnessKind::AntigravityIde,
        template: Template::Recommended,
        account: AccountKind::InApp,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
        os: Os::Debian,
    }
}

/// A stand-in engine and the files it answers from.
struct Stand {
    /// The profile signed in; each test has its own, so tests running side by side never share a
    /// sign-in's names or folders.
    profile: String,
    folder: PathBuf,
    binary: PathBuf,
    calls: PathBuf,
    /// Present once the application inside has written its login.
    signed: PathBuf,
    /// Present while the application holds a login it has not written yet: it writes it, as
    /// Antigravity IDE does, only when it is asked to quit.
    held: PathBuf,
    /// What the engine answers when asked for the window's state; `running` when absent.
    state: PathBuf,
    /// Where the engine writes down the folder of this machine the courier was handed, which it
    /// writes the login into.
    capture: PathBuf,
    /// Where the engine writes down the folder the window was handed to leave the addresses it
    /// wants opened in.
    browser: PathBuf,
}

impl Stand {
    fn new(name: &str) -> Self {
        let profile = format!("anti-{name}");
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-window-login-{name}-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a scratch folder");
        let stand = Self {
            binary: folder.join("engine"),
            calls: folder.join("calls"),
            signed: folder.join("signed"),
            held: folder.join("held"),
            state: folder.join("state"),
            capture: folder.join("capture-at"),
            browser: folder.join("browser-at"),
            folder,
            profile,
        };
        // The folders are QCode's own private ones with names nobody can guess, so the engine
        // learns them the way a real one does: from the mounts it is handed.
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> '{calls}'\n\
             for word in \"$@\"; do\n\
             case \"$word\" in\n\
             *:{CAPTURE_DIR}:rw*) printf '%s' \"${{word%:{CAPTURE_DIR}:rw*}}\" > '{capture}' ;;\n\
             *:{OPEN_DIR}:rw*) printf '%s' \"${{word%:{OPEN_DIR}:rw*}}\" > '{browser}' ;;\n\
             esac\n\
             done\n\
             case \"$1\" in\n\
             run) exit 0 ;;\n\
             stop) [ -f '{held}' ] && mv '{held}' '{signed}'; exit 0 ;;\n\
             container) cat '{state}' 2>/dev/null || echo running; exit 0 ;;\n\
             exec)\n\
             for word in \"$@\"; do\n\
             [ \"$word\" = seen ] && {{ [ -f '{signed}' ] && exit 0; exit 1; }}\n\
             case \"$word\" in *\"' take '\"*) [ -f '{signed}' ] || exit 1; printf '{{}}' > \"$(cat '{capture}')/{STORED}\"; exit 0 ;; esac\n\
             done\n\
             exit 0 ;;\n\
             esac\n\
             exit 0\n",
            calls = stand.calls.display(),
            state = stand.state.display(),
            signed = stand.signed.display(),
            held = stand.held.display(),
            capture = stand.capture.display(),
            browser = stand.browser.display(),
            CAPTURE_DIR = crate::ui::profiles::recipe::CAPTURE_DIR,
            OPEN_DIR = crate::desktop::signin::OPEN_DIR,
        );
        std::fs::write(&stand.binary, script).expect("the stand-in engine is written");
        std::fs::set_permissions(&stand.binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("it can be run");
        stand
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// The application inside writes its login.
    fn sign_in(&self) {
        std::fs::write(&self.signed, "").expect("the stand-in login");
    }

    /// The application inside is signed in and keeps the login in memory, writing it to its home
    /// only when it is asked to quit, which is what Antigravity IDE does with a real Google login.
    fn sign_in_held(&self) {
        std::fs::write(&self.held, "").expect("the stand-in login held in memory");
    }

    /// A screen on this engine holding `profiles`, with a stand-in compositor to open windows on.
    fn screen(&self, profiles: Vec<Profile>) -> Harness<Host> {
        let socket = self.folder.join("wayland-1");
        std::fs::write(&socket, "").expect("a stand-in compositor socket");
        self.screen_on(profiles, Ok(Display { socket, name: "wayland-1".to_owned(), device: None }))
    }

    /// The same, on whatever this machine is said to offer.
    fn screen_on(&self, profiles: Vec<Profile>, display: Result<Display, NoDisplay>) -> Harness<Host> {
        let engine = Engine::new(crate::engine::EngineKind::Podman, &self.binary);
        let state =
            Profiles::new(Some(self.folder.join("store")), Some(engine)).with_providers_file(None).showing_on(display);
        let env = Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() })
            .expect("the built-in files load");
        let mut harness = Harness::with_env(Host { state }, env, 110, 40);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        harness.send(Msg::Loaded(Listing { profiles, diagnostics: Vec::new() }));
        let answers: Vec<Status> = harness
            .app()
            .state
            .rows()
            .iter()
            .map(|row| Status {
                name: row.profile.name.clone(),
                image: Readiness::Present,
                revision: Revision::Current,
                identity: Readiness::Missing,
            })
            .collect();
        harness.send(Msg::Probed(answers)).render();
        harness
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The screen's words with every run of blanks made one, so a sentence the page wraps still reads
/// as one.
fn words(harness: &Harness<Host>) -> String {
    harness.screen().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Moves the clock and draws until the screen says `text`, over a generous but finite while.
fn until(harness: &mut Harness<Host>, text: &str) {
    let started = Instant::now();
    loop {
        harness.advance(Duration::from_millis(100));
        if words(harness).contains(text) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "never said {text:?}:\n{}", harness.screen());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Opens the sign-in window of the one profile on screen, from the list's button onwards.
fn open_window(harness: &mut Harness<Host>) {
    harness.click_text("Sign in").render();
    let page = words(harness);
    assert!(page.contains("Antigravity IDE signs in with Google in its own window"), "{page}");
    assert!(page.contains("Every workspace then opens Antigravity IDE signed in"), "{page}");
    harness.click_text("Open the sign-in window").render();
    until(harness, "The Antigravity IDE window is open on your screen");
}

#[test]
fn a_profile_made_before_the_login_was_stored_is_listed_as_not_signed_in_and_offers_the_sign_in() {
    let stand = Stand::new("listed");
    let harness = stand.screen(vec![antigravity(&stand.profile)]);
    let screen = words(&harness);
    assert!(screen.contains("Not signed in"), "{screen}");
    assert!(!screen.contains("No sign-in needed"), "{screen}");
    // The list's own button is there to press, and no window has been opened for it.
    assert!(screen.contains("Sign in"), "{screen}");
    assert!(!stand.calls().iter().any(|call| call.starts_with("run ")), "{:?}", stand.calls());
}

#[test]
fn the_login_is_noticed_in_the_window_stored_with_the_profile_and_the_window_taken_away() {
    let stand = Stand::new("noticed");
    let mut harness = stand.screen(vec![antigravity(&stand.profile)]);
    open_window(&mut harness);

    // The window opens as a window of its own, with its own home, never a workspace's.
    let calls = stand.calls();
    let run = calls.iter().find(|call| call.starts_with("run ")).expect("the window was opened");
    let p = &stand.profile;
    assert!(run.contains(&format!("--name qcode-signin-{p} ")), "{run}");
    assert!(run.contains(&format!("--volume qcode-signin-{p}:/home/qcode:rw")), "{run}");
    assert!(run.contains("WAYLAND_DISPLAY=wayland-1"), "{run}");
    assert!(!run.contains("qcode-home-"), "{run}");

    // Not signed in yet: the page stays, looking again at every pause.
    harness.advance(LOOK_EVERY);
    harness.advance(LOOK_EVERY);
    assert!(words(&harness).contains("The Antigravity IDE window is open on your screen"), "{}", harness.screen());
    assert!(!stand.calls().iter().any(|call| call.starts_with("cp ")), "nothing is stored before the login is there");

    // The application writes its login; the next look finds it without anyone pressing anything.
    stand.sign_in();
    until(&mut harness, "Signed in. The Antigravity IDE sign-in is stored with this profile.");

    let calls = stand.calls();
    let courier = calls
        .iter()
        .find(|call| call.starts_with(&format!("create --name qcode-signin-take-{p} ")))
        .expect("a courier takes the login out");
    assert!(courier.contains(&format!("qcode-signin-{p}:/home/qcode:rw")), "{courier}");
    assert!(courier.contains(":/qcode-capture:rw"), "{courier}");
    assert!(courier.contains("--network=none"), "{courier}");
    assert!(calls.iter().any(|call| call.starts_with("cp ") && call.ends_with(":/qcode-credentials")), "{calls:?}");
    // And everything made for the sign-in is gone again: the window, its home and the courier.
    for gone in [
        format!("rm --force qcode-signin-{p}"),
        format!("rm --force qcode-signin-take-{p}"),
        format!("volume rm qcode-signin-{p}"),
    ] {
        let at = calls.iter().rposition(|call| *call == gone).unwrap_or_else(|| panic!("{gone}: {calls:?}"));
        let stored = calls.iter().position(|call| call.starts_with("cp ")).expect("stored");
        assert!(at > stored, "{gone} after the login was stored: {calls:?}");
    }
    // The folder the login passed through on this machine goes with them.
    let captured = std::fs::read_to_string(&stand.capture).expect("the courier was handed a folder");
    let private = Path::new(&captured).parent().expect("the sign-in's own folder");
    let started = Instant::now();
    while private.exists() {
        assert!(started.elapsed() < Duration::from_secs(30), "{} is still there", private.display());
        harness.advance(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn done_takes_the_login_at_once_without_waiting_for_the_next_look() {
    let stand = Stand::new("done");
    let mut harness = stand.screen(vec![antigravity(&stand.profile)]);
    open_window(&mut harness);
    stand.sign_in();
    // The clock is not moved: only the button can have started the taking.
    harness.click_text("I have signed in").render();
    let started = Instant::now();
    while !words(&harness).contains("sign-in is stored with this profile") {
        assert!(started.elapsed() < Duration::from_secs(30), "{}", harness.screen());
        std::thread::sleep(Duration::from_millis(20));
        harness.render();
    }
    let courier = format!("create --name qcode-signin-take-{} ", stand.profile);
    assert!(stand.calls().iter().any(|call| call.starts_with(&courier)));
}

#[test]
fn done_asks_the_window_to_quit_so_a_login_the_application_holds_in_memory_is_written_and_stored() {
    let stand = Stand::new("held");
    let mut harness = stand.screen(vec![antigravity(&stand.profile)]);
    open_window(&mut harness);
    stand.sign_in_held();
    harness.click_text("I have signed in").render();
    let started = Instant::now();
    while !words(&harness).contains("sign-in is stored with this profile") {
        assert!(started.elapsed() < Duration::from_secs(30), "{}", harness.screen());
        std::thread::sleep(Duration::from_millis(20));
        harness.render();
    }
    let calls = stand.calls();
    let p = &stand.profile;
    // The window is asked to quit, and given time for it, before anything is taken out: a kill
    // would lose the login the application had not written yet.
    let stop = calls
        .iter()
        .position(|call| call.starts_with("stop --time ") && call.ends_with(&format!(" qcode-signin-{p}")))
        .unwrap_or_else(|| panic!("the window is asked to quit: {calls:?}"));
    let courier = calls
        .iter()
        .position(|call| call.starts_with(&format!("create --name qcode-signin-take-{p} ")))
        .expect("a courier takes the login out");
    assert!(stop < courier, "{calls:?}");
    let removed = calls.iter().rposition(|call| *call == format!("rm --force qcode-signin-{p}")).expect("removed");
    assert!(stop < removed, "the window quits before it is removed: {calls:?}");
}

#[test]
fn done_without_a_login_stores_nothing_and_says_so() {
    let stand = Stand::new("nothing");
    let mut harness = stand.screen(vec![antigravity(&stand.profile)]);
    open_window(&mut harness);
    harness.click_text("I have signed in").render();
    until(&mut harness, "No sign-in was found in the window, so nothing was stored.");
    let calls = stand.calls();
    assert!(!calls.iter().any(|call| call.starts_with("cp ")), "nothing reached the profile's store: {calls:?}");
    assert!(calls.iter().any(|call| *call == format!("volume rm qcode-signin-{}", stand.profile)), "{calls:?}");
}

#[test]
fn a_window_closed_before_signing_in_ends_the_look_and_stores_nothing() {
    let stand = Stand::new("closed");
    let mut harness = stand.screen(vec![antigravity(&stand.profile)]);
    open_window(&mut harness);
    std::fs::write(&stand.state, "exited\n").expect("the window closes");
    harness.advance(LOOK_EVERY);
    until(&mut harness, "No sign-in was found in the window");
    assert!(!stand.calls().iter().any(|call| call.starts_with("cp ")));
}

#[test]
fn stop_takes_the_window_away_and_leaves_the_profile_as_it_was() {
    let stand = Stand::new("stop");
    let mut harness = stand.screen(vec![antigravity(&stand.profile)]);
    open_window(&mut harness);
    harness.click_text("Stop").render();
    until(&mut harness, "The sign-in was stopped");
    let started = Instant::now();
    let volume = format!("volume rm qcode-signin-{}", stand.profile);
    while !stand.calls().contains(&volume) {
        assert!(started.elapsed() < Duration::from_secs(30), "{:?}", stand.calls());
        harness.advance(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
    }
    let window = format!("rm --force qcode-signin-{}", stand.profile);
    assert!(stand.calls().contains(&window), "{:?}", stand.calls());
    assert!(!stand.calls().iter().any(|call| call.starts_with("cp ")));
}

#[test]
fn the_page_the_window_asks_for_opens_in_the_persons_browser_with_its_way_back_listened_for() {
    let stand = Stand::new("browser");
    let mut harness = stand.screen(vec![antigravity(&stand.profile)]);
    harness.set_open_outcome(OpenOutcome::Opened);
    open_window(&mut harness);
    let port =
        std::net::TcpListener::bind("127.0.0.1:0").and_then(|socket| socket.local_addr()).expect("a port").port();
    let address = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Foauth-callback"
    );
    // Written the way the window's own opener leaves it.
    let asked_in = std::fs::read_to_string(&stand.browser).expect("the window was handed a folder");
    std::fs::write(Path::new(&asked_in).join("1.url"), format!("{address}\n")).expect("the window asks");
    let started = Instant::now();
    while harness.opens().is_empty() {
        assert!(started.elapsed() < Duration::from_secs(30), "nothing was opened:\n{}", harness.screen());
        harness.advance(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
    }
    let asked = harness.opens().last().expect("opened");
    assert_eq!(asked.target.as_deref(), Some(std::ffi::OsStr::new(&address)));
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err(), "QCode listens on {port}");
    until(&mut harness, "the page opened in your browser");
    assert!(words(&harness).contains(&format!("QCode listens on port {port}")), "{}", harness.screen());
    // Leaving the page gives the port up and takes the window away.
    harness.click_text("Stop").render();
    let started = Instant::now();
    while std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
        assert!(started.elapsed() < Duration::from_secs(30), "the port was kept");
        harness.advance(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_machine_without_a_screen_is_told_so_and_nothing_is_started() {
    let stand = Stand::new("noscreen");
    let mut harness = stand.screen_on(vec![antigravity(&stand.profile)], Err(NoDisplay::NoWayland));
    harness.click_text("Sign in").render();
    harness.click_text("Open the sign-in window").render();
    let screen = words(&harness);
    assert!(screen.contains("This session has no Wayland compositor"), "{screen}");
    // Only the list's own questions reached the engine: no window, no container, no volume.
    let calls = stand.calls();
    assert!(calls.iter().all(|call| call.starts_with("volume ls") || call.starts_with("image inspect")), "{calls:?}");
}
