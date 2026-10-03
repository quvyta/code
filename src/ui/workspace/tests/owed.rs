//! The keyboard a tab is owed: a tab opened from the New tab page takes it when its terminal comes
//! up, not when the page is answered, since there is no terminal on the screen to take it then.
//!
//! A harness runs the screen's background work in line, so a start the screen begins itself is up
//! inside the very update that asked for the keyboard, and the focus lands on the terminal of the
//! frame painted after it — the defect cannot be seen that way. The real runtime is not like that:
//! the start comes back from a task, some updates later, and the focus asked for in the meantime has
//! nowhere to land. So the answer of a start is handed to the screen here as a task's is: in an
//! update of its own, after a frame has been drawn with the tab still starting.
//!
//! One stand-in container runtime stands in for the engine. It brings a container up by running a
//! shell of this machine in place of the container's — what the terminal does with keys does not
//! depend on what runs in it — which shows, in hex, the first three bytes that program receives.
//! While it is set to refuse, it brings nothing up and says what it says instead, so a tab can fail
//! to start the way one does when the engine will not do the work.

use std::time::{Duration, Instant};

use super::*;

use crate::ui::workspace::plan::LaunchFailure;

/// What the refusing engine says, in its own words.
const WORDS: &str = "Error: no container runtime answers here";

/// What a running tab's program runs: raw input, a line that says it is ready, then the first three
/// bytes it receives in hex, a dot every tenth of a second while it waits for them and a while
/// longer after it has them.
///
/// The dots are what keeps a test from hanging instead of failing: the screen watches the session
/// for what the program says, and a program that waits in silence for keys that never arrive would
/// hold the test at that watch rather than let it say the keys went nowhere.
fn echoing() -> String {
    "stty raw -echo; printf 'ready '; (while :; do sleep 0.1; printf '.'; done) & head -c 3 | od -An -tx1; sleep 30"
        .to_owned()
}

/// A stand-in container runtime that answers every question a start asks, so the answer a tab waits
/// for finds the command to spawn, and whose `exec` of that command is a shell of this machine.
struct Standing {
    scratch: Scratch,
    engine: Engine,
    /// The file that says whether the engine brings containers up or refuses to.
    marker: PathBuf,
}

impl Standing {
    /// An engine of its own at `name`, bringing containers up.
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let program = scratch.0.join("shell");
        fs::write(&program, echoing()).expect("the program a tab runs");
        let marker = scratch.0.join("refusing");
        let script = format!(
            "#!/bin/sh\n\
             if [ -e {marker} ]; then echo '{words}' >&2; exit 125; fi\n\
             case \"$1 $2\" in\n\
             'exec --interactive') exec /bin/sh {program} ;;\n\
             esac\n\
             exit 0\n",
            marker = marker.display(),
            words = WORDS,
            program = program.display(),
        );
        let binary = scratch.0.join("engine");
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        let engine = Engine::new(EngineKind::Podman, &binary);
        Self { scratch, engine, marker }
    }

    /// The same engine, refusing to bring any container up from now on, in its own words.
    fn refuses(&self) {
        fs::write(&self.marker, "").expect("the engine will refuse from now on");
    }

    /// The same engine, doing the work from now on.
    fn serves(&self) {
        fs::remove_file(&self.marker).expect("nothing stands in its way any more");
    }

    /// A screen with one workspace entered through this engine, carrying a profile so that the
    /// engine is asked for more than the workspace's own container, and `count` shell tabs open in
    /// it the way the person opens one — the `+` on the strip, then the Shell row on its page —
    /// every one of them still waiting for its container.
    fn waiting(&self, count: usize) -> WorkspaceScreen {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        )];
        let mut screen =
            WorkspaceScreen::new(Some(self.engine.clone()), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces);
        for _ in 0..count {
            open(&mut screen, Choice::Shell);
        }
        assert_eq!(kinds(&screen), vec![TabKind::Shell; count]);
        screen
    }
}

/// Renders until the screen shows `text`, waiting generously but not for ever: what a terminal says
/// comes from the program in it, so the screen is looked at until the program has said it.
fn wait_for(harness: &mut Harness<Screen>, text: &str) {
    let started = Instant::now();
    while !harness.render().screen().contains(text) {
        assert!(started.elapsed() < Duration::from_secs(20), "`{text}` never came:\n{}", harness.screen());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The key and run of the tab at `index` of the open workspace, which is what the answer of a start
/// is named for.
fn tab_of(screen: &WorkspaceScreen, index: usize) -> (TabKey, u64) {
    let tab = &screen.workspace().expect("a workspace is open").tabs()[index];
    (tab.key(), tab.run())
}

/// The session of the tab `key`, which is where that tab's keys land.
fn session(harness: &Harness<Screen>, key: TabKey) -> &TerminalSession {
    harness
        .app()
        .0
        .workspace()
        .and_then(|workspace| workspace.tabs().iter().find(|tab| tab.key() == key))
        .and_then(Tab::session)
        .expect("the tab has a session")
}

/// What an engine that brought no container up said.
fn refusal() -> LaunchFailure {
    LaunchFailure {
        command: format!("{} run --name qcode-firefly-base {} {}", NO_ENGINE, SHELL[0], SHELL[1]),
        output: WORDS.to_owned(),
        image_missing: false,
    }
}

#[test]
fn a_tab_opened_from_the_new_tab_page_takes_the_keyboard_when_its_terminal_comes_up() {
    let standing = Standing::new("owed-landing");
    let screen = standing.waiting(1);
    let (key, run) = tab_of(&screen, 0);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    assert!(harness.screen().contains("Starting the container"), "{}\n", harness.screen());

    // The container comes up, and the tab's terminal is on the screen at last.
    harness.send(Msg::Ready(key, run, Ok(())));
    wait_for(&mut harness, "ready");

    // What the person types now, with no click in it, is meant for the program in that terminal.
    harness.type_text("xyz");
    wait_for(&mut harness, "78 79 7a");
    session(&harness, key).kill();
}

#[test]
fn a_tab_still_starting_does_not_take_the_keyboard_from_where_the_person_put_it() {
    let standing = Standing::new("owed-leaving");
    let screen = standing.waiting(1);
    let (key, run) = tab_of(&screen, 0);
    let mut harness = harness(screen, SIZE.0, SIZE.1);

    // While the tab is still starting the person goes to the files and opens a folder there.
    harness.click_text("src").render();

    // The terminal comes up beside them, and the keyboard stays in the files.
    harness.send(Msg::Ready(key, run, Ok(())));
    wait_for(&mut harness, "ready");
    let since = Instant::now();
    harness.type_text("xyz").render();
    std::thread::sleep(Duration::from_millis(300));
    harness.render();
    assert!(session(&harness, key).last_input() <= since, "the terminal took the keys:\n{}", harness.screen());
    session(&harness, key).kill();
}

#[test]
fn a_tab_started_again_after_its_container_never_came_up_takes_the_keyboard_when_it_does() {
    let standing = Standing::new("owed-refused");
    standing.refuses();
    let screen = standing.waiting(1);
    let (key, run) = tab_of(&screen, 0);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.send(Msg::Ready(key, run, Err(refusal())));
    assert!(harness.render().screen().contains("The engine refused"), "{}\n", harness.screen());

    // The engine does the work now, and the person presses the button that starts the tab again;
    // the button goes with the page it is on, and the terminal that comes up in its place is where
    // the keyboard goes.
    standing.serves();
    harness.click_text("Start again");
    wait_for(&mut harness, "ready");
    harness.type_text("xyz");
    wait_for(&mut harness, "78 79 7a");
    session(&harness, key).kill();
}
