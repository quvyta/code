//! A sound opened from the file tree: played in a container made for it alone when this machine
//! has a sound server and the settings allow it, described otherwise.
//!
//! The engine of these tests cannot be run, so nothing plays; what is checked is the command the
//! tab would spawn and the way it would take, which is where "only this one container ever
//! reaches the sound server" is decided.

use std::os::unix::net::UnixListener;

use super::*;

use crate::base::apps::{Quiet, Sound};
use crate::store::{Session, SessionTabKind};
use crate::ui::workspace::{Taken, entry};

/// The sound of these tests, with a space in its folder's name and its own.
const SONG: &str = "sea sounds/fog horn.mp3";

/// What soxi prints for a short sound.
const DETAILS: &str = "Input File     : '/work/sea sounds/fog horn.mp3'\nChannels       : 2\n\
                       Sample Rate    : 44100\nDuration       : 00:00:03.00 = 132300 samples\n";

/// A workspace folder with a sound in it, and a runtime folder beside it with a sound server's
/// socket in it, served for as long as the listener lives.
struct Setting {
    scratch: Scratch,
    runtime: PathBuf,
    _server: UnixListener,
}

impl Setting {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let sounds = scratch.paths().code.join("sea sounds");
        fs::create_dir_all(&sounds).expect("a folder for sounds");
        fs::write(sounds.join("fog horn.mp3"), [0xff, 0xfb]).expect("a sound");
        let runtime = scratch.0.join("runtime");
        fs::create_dir_all(runtime.join("pulse")).expect("a runtime folder");
        let server = UnixListener::bind(runtime.join("pulse").join("native")).expect("a sound server's socket");
        Self { scratch, runtime, _server: server }
    }

    fn socket(&self) -> PathBuf {
        self.runtime.join("pulse").join("native")
    }

    /// The screen of `one_workspace`, looking for the sound server in `runtime`.
    fn screen(&self, runtime: Option<&Path>) -> WorkspaceScreen {
        one_workspace(&self.scratch).hearing_in(runtime.map(Path::to_path_buf))
    }
}

/// The tab of `key` in the open workspace.
fn tab(screen: &WorkspaceScreen, key: TabKey) -> &Tab {
    screen.workspace().expect("a workspace").tabs().iter().find(|tab| tab.key() == key).expect("the tab")
}

/// The words of the command the tab `key` would spawn.
fn words(screen: &WorkspaceScreen, key: TabKey) -> Vec<String> {
    let command = screen.launch_command(key).expect("the tab has a command");
    assert_eq!(command.program, Path::new(NO_ENGINE), "a tab only ever starts the engine binary");
    command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

/// A session on this machine standing in for the player, printing what `play` prints.
fn player(script: &str) -> TerminalSession {
    TerminalSession::spawn("/bin/sh".as_ref(), &["-c", script], Path::new("/")).expect("a terminal for the shell")
}

#[test]
fn a_sound_plays_in_a_container_of_its_own_that_reaches_nothing_but_the_sound_server() {
    let setting = Setting::new("sound-play");
    let mut screen = setting.screen(Some(&setting.runtime));
    apply(&mut screen, Msg::OpenFile(SONG.to_owned()));
    let key = key(&screen, 0);
    assert_eq!(kinds(&screen), [TabKind::Sound(SONG.to_owned())]);
    assert_eq!(state(&screen, 0, 0), TabState::Starting);
    assert!(screen.launch_command(key).is_none(), "nothing plays before the way to the server is known");

    apply(&mut screen, Msg::Playable(key, 0, Ok(setting.socket())));
    let socket = setting.socket().display().to_string();
    let workspace = setting.scratch.paths().code.display().to_string();
    assert_eq!(
        words(&screen, key),
        [
            "run",
            "--rm",
            "--interactive",
            "--tty",
            "--name",
            "qcode-firefly.play-0",
            "--env",
            "PULSE_SERVER=unix:/run/qcode/pulse/native",
            "--env",
            "PULSE_COOKIE=/tmp/qcode-pulse-cookie",
            "--volume",
            &format!("{socket}:/run/qcode/pulse/native:rw"),
            "--userns=keep-id",
            "--network=none",
            "--volume",
            &format!("{workspace}:/work:ro,z"),
            "--workdir",
            "/work",
            "--pull=never",
            "qcode/base",
            "play",
            &format!("{CODE_DIR}/{SONG}"),
        ],
        "a one-off container, removed when it ends, with no network, the workspace read-only, the \
         socket and nothing else of this machine, and the path as one word"
    );
    // The engine of the test cannot be started, so the player fails the way a missing engine
    // would; the command above is what it would have been.
    assert!(matches!(state(&screen, 0, 0), TabState::Failed(_)), "{:?}", state(&screen, 0, 0));
}

#[test]
fn a_sound_is_played_only_with_a_server_and_the_setting_on_and_described_otherwise() {
    let setting = Setting::new("sound-way");
    let empty = setting.scratch.0.join("no-server");
    fs::create_dir_all(&empty).expect("a runtime folder with no server");
    // With the engine of the test every way ends in its failure; which way was taken shows in
    // whether the tab knows a reason to keep quiet.
    for (runtime, sound, quiet) in [
        (Some(setting.runtime.as_path()), Sound::Play, None),
        (Some(empty.as_path()), Sound::Play, Some(Quiet::NoServer)),
        (None, Sound::Play, Some(Quiet::NoServer)),
        (Some(setting.runtime.as_path()), Sound::Details, Some(Quiet::Chosen)),
    ] {
        let mut screen = setting.screen(runtime);
        screen.set_sound(sound);
        assert_eq!(screen.sound(), sound);
        let mut shown = harness(screen, SIZE.0, SIZE.1);
        shown.send(Msg::OpenFile(SONG.to_owned())).render();
        let screen = &shown.app().0;
        assert_eq!(tab(screen, key(screen, 0)).quiet(), quiet, "{runtime:?} {sound:?}");
        assert!(matches!(state(screen, 0, 0), TabState::Failed(_)), "{:?}", state(screen, 0, 0));
    }
}

#[test]
fn a_sound_that_does_not_play_shows_its_details_and_says_why() {
    let setting = Setting::new("sound-details");
    for (why, words) in [
        (Quiet::NoServer, "This machine has no sound server a container can reach"),
        (Quiet::Chosen, "Sounds are set to show their details only"),
    ] {
        let mut screen = setting.screen(None);
        apply(&mut screen, Msg::OpenFile(SONG.to_owned()));
        let key = key(&screen, 0);
        let read = spelled(&super::super::sound::details_command(&screen, key).expect("the details are asked"));
        assert_eq!(read, ["exec", "qcode-firefly-base", "soxi", &format!("{CODE_DIR}/{SONG}")]);
        apply(&mut screen, Msg::Described(key, 0, why, Ok(Taken::new(DETAILS.to_owned()))));
        assert_eq!(state(&screen, 0, 0), TabState::Running);
        assert_eq!(entry(&screen), "workspace-document", "the details take the keyboard, like a document");
        let mut shown = harness(screen, SIZE.0, SIZE.1);
        let text = shown.screen();
        assert!(text.contains(words), "{text}");
        assert!(text.contains("Duration       : 00:00:03.00"), "{text}");
        assert!(!text.contains("Play"), "nothing offers to play:\n{text}");
        shown.set_locale("tr").render();
        let turkish = if why == Quiet::Chosen { "yalnızca ayrıntılarıyla" } else { "ses sunucusu yok" };
        assert!(shown.screen().contains(turkish), "{}", shown.screen());
    }
}

/// The words of an engine command, after checking it is the engine that runs.
fn spelled(command: &crate::engine::EngineCommand) -> Vec<String> {
    assert_eq!(command.program, Path::new(NO_ENGINE), "only the engine binary is ever started");
    command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

#[test]
fn a_playing_sound_says_how_to_stop_it_and_offers_to_play_again_once_it_ended() {
    let setting = Setting::new("sound-playing");
    let mut screen = setting.screen(Some(&setting.runtime));
    apply(&mut screen, Msg::OpenFile(SONG.to_owned()));
    let key = key(&screen, 0);
    let session = player("printf 'fog horn.mp3:\\n\\n Duration: 00:00:03.00\\n\\nIn:33.3%% 00:00:01.00'; sleep 30");
    let watch = session.watch();
    screen.attach(key, session);
    assert_eq!(
        super::super::sound::playing(
            screen.workspace().expect("a workspace"),
            &screen.workspace().expect("a workspace").tabs().iter().collect::<Vec<_>>()
        ),
        ["qcode-firefly.play-0"],
        "the container a playing tab would take away when it closes"
    );
    let mut shown = harness(screen, SIZE.0, SIZE.1);
    let started = std::time::Instant::now();
    while !shown.render().screen().contains("In:33.3%") {
        assert!(started.elapsed() < Duration::from_secs(10), "{}", shown.screen());
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = shown.screen();
    assert!(text.contains("Duration: 00:00:03.00"), "{text}");
    assert!(text.contains("ctrl c stops it"), "written the way the key list writes keys:\n{text}");
    assert!(!text.contains("Play again"), "{text}");

    shown.send(Msg::Output(key, 0, TerminalEvent::Exited(Some(0)))).render();
    let text = shown.screen();
    assert!(text.contains("Play again"), "{text}");
    assert!(text.contains("In:33.3%"), "the player's last screen stays:\n{text}");
    assert!(!text.contains("ended"), "a sound that played needs no note that its player ended:\n{text}");
    assert!(
        super::super::sound::playing(
            shown.app().0.workspace().expect("a workspace"),
            &shown.app().0.workspace().expect("a workspace").tabs().iter().collect::<Vec<_>>()
        )
        .is_empty(),
        "a sound that ended has no container left to take away"
    );
    drop(watch);

    shown.click_text("Play again");
    assert_eq!(tab(&shown.app().0, key).run(), 1, "playing again starts from the beginning");
}

#[test]
fn a_sound_brought_back_from_the_last_session_waits_to_be_asked() {
    let setting = Setting::new("sound-session");
    let mut screen = setting.screen(Some(&setting.runtime));
    apply(&mut screen, Msg::OpenFile(SONG.to_owned()));
    let session = screen.session();
    assert_eq!(session.workspaces[0].tabs[0].kind, SessionTabKind::Sound(SONG.to_owned()));
    let read = Session::parse("session.toml", &session.to_toml());
    assert!(read.is_clean(), "{:?}", read.diagnostics);
    assert_eq!(read.value, session);

    let mut again = setting.screen(Some(&setting.runtime));
    again.restore_tabs(0, &read.value.workspaces[0]);
    let key = key(&again, 0);
    assert!(tab(&again, key).is_held(), "opening QCode again is not asking to hear the sound again");
    apply(&mut again, Msg::OpenTab(0));
    apply(&mut again, Msg::Playable(key, 0, Ok(setting.socket())));
    assert_eq!(state(&again, 0, 0), TabState::Ended { code: None }, "it is ready, and does not play");
    assert!(tab(&again, key).session().is_none());
    let mut shown = harness(again, SIZE.0, SIZE.1);
    let text = shown.screen();
    assert!(text.contains("fog horn.mp3") && text.contains("Play"), "{text}");
    assert!(!text.contains("Play again"), "{text}");

    shown.click_text("Play");
    let tab = tab(&shown.app().0, key);
    assert!(!tab.is_held(), "asked, it plays");
    assert_eq!(tab.run(), 1);
}

#[test]
fn a_sound_whose_file_is_gone_says_so_and_plays_nothing() {
    let setting = Setting::new("sound-gone");
    let mut shown = harness(setting.screen(Some(&setting.runtime)), SIZE.0, SIZE.1);
    shown.send(Msg::OpenFile(SONG.to_owned()));
    fs::remove_file(setting.scratch.paths().code.join(SONG)).expect("the sound goes");
    let key = key(&shown.app().0, 0);
    shown.send(Msg::Restart(key)).render();
    assert_eq!(state(&shown.app().0, 0, 0), TabState::Missing);
    assert!(shown.screen().contains("fog horn.mp3 is not in the workspace any more"), "{}", shown.screen());
}
