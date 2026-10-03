//! A build whose log has stopped moving: the tab says the build may be stuck, leaves it running,
//! and takes the word away as soon as the build says something again. A build that keeps saying
//! something is never warned about.
//!
//! The stand-in engine really is asked for the image, and it really holds the build open, but the
//! build runs in a thread of the test's own: the harness calls a background task stuck after ten
//! seconds of it working, and a build is allowed to take as long as the engine takes. What the test
//! watches is the screen and the stand-in's own marker, never the machine's process table.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::*;

use crate::ui::profiles::work;
use crate::ui::stalling::Lengths;
use crate::ui::workspace::plan::LaunchFailure;

/// The lengths a test judges a build by: a silence a moment of real time reaches, and a look the
/// harness's clock brings round at once.
const BRIEF: Lengths = Lengths { quiet: Duration::from_millis(50), look: Duration::from_millis(10) };

/// The word of the warning that does not carry the minutes, so that a test which shortens the
/// quiet can still say what the person is told.
const WARNING: &str = "It may be stuck; you can stop it.";

/// What the same warning says in Turkish, which is what a person who chose that language is waited
/// for instead.
const TURKISH: &str = "Takılmış olabilir";

/// A stand-in podman that has the base image and the profile's image, and whose `build` says one
/// line and then holds still, writing `ended` when it lets go. With `talk` in the folder it keeps
/// a line coming every twentieth of a second instead, which is a build that is going.
struct Standing {
    scratch: Scratch,
    engine: Engine,
}

impl Standing {
    fn new(name: &str, talk: bool) -> Self {
        let scratch = Scratch::new(name);
        let ended = scratch.0.join("ended");
        let script = format!(
            "#!/bin/sh\n\
             case \"$1 $2 $5\" in\n\
             'image inspect qcode/base') echo {base}; exit 0 ;;\n\
             esac\n\
             case \"$1 $2 $4\" in\n\
             'image inspect {{{{.Id}}}}') echo sha-old; exit 0 ;;\n\
             esac\n\
             [ \"$1\" = build ] || exit 0\n\
             echo 'STEP 1/9: FROM qcode/base'\n\
             if [ -e {talk} ]; then\n\
             while :; do echo 'STEP 2/9: RUN npm install -g @anthropic-ai/claude-code'; sleep 0.02; done\n\
             fi\n\
             sleep 30\n\
             touch {ended}\n",
            base = crate::base::revision(),
            talk = scratch.0.join("talk").display(),
            ended = ended.display(),
        );
        let binary = scratch.0.join("engine");
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        if talk {
            fs::write(scratch.0.join("talk"), "").expect("the build keeps talking");
        }
        let engine = Engine::new(EngineKind::Podman, &binary);
        Self { scratch, engine }
    }

    /// The file the stand-in's build writes when it lets go of it.
    fn ended(&self) -> PathBuf {
        self.scratch.0.join("ended")
    }

    /// A screen whose one workspace carries the profile whose image the engine already has, so a tab
    /// of it can be told the image is missing and asked to build it the way a workspace whose engine
    /// lost the image is.
    fn screen(&self) -> WorkspaceScreen {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        )];
        WorkspaceScreen::new(Some(self.engine.clone()), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces)
    }
}

/// The profile of the stand-in's workspace, as the tab builds it.
fn standing() -> Profile {
    profile("claude-sub", HarnessKind::ClaudeCode)
}

/// The build of `profile`'s image over `engine`, in a thread of the test's own, whose lines are
/// taken and handed to the screen the way the build's task hands them over.
///
/// Dropped, the build is asked to stop and the thread is waited for, so no stand-in is left
/// running behind a test that has finished with it.
struct Building {
    lines: mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
    ended: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl Building {
    /// The next line of the build, or `None` when none has come within a generous wait.
    fn next_line(&self) -> Option<String> {
        self.lines.recv_timeout(Duration::from_secs(10)).ok()
    }

    /// Whether the stand-in has let go of the build.
    fn let_go(&self) -> bool {
        self.ended.exists()
    }
}

impl Drop for Building {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Builds `profile`'s image over `engine`, the whole build the button starts, and says through
/// `ended` — the stand-in's own marker — when the engine let go of it.
fn building(engine: &Standing, profile: &Profile) -> Building {
    let (sender, lines) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let asked = Arc::clone(&stop);
    let podman = engine.engine.clone();
    let profile = profile.clone();
    let thread = std::thread::spawn(move || {
        let cancel = || asked.load(Ordering::SeqCst);
        let _ = work::build_whole(&podman, &profile, &cancel, &mut || {}, &mut |line: &str| {
            let _ = sender.send(line.to_owned());
        });
    });
    Building { lines, stop, ended: engine.ended(), thread: Some(thread) }
}

/// A tab of `profile` whose image the engine does not have, told to build it the way the person
/// tells it: the button on the page of the missing image, applied without letting its task run.
fn building_tab(screen: &mut WorkspaceScreen) -> TabKey {
    open(screen, claude());
    let tab = key(screen, 0);
    let missing = LaunchFailure { command: String::new(), output: String::new(), image_missing: true };
    apply(screen, Msg::Ready(tab, 0, Err(missing)));
    apply(screen, Msg::BuildImage(tab));
    tab
}

/// A harness over a tab whose build has said one line, with the silence judged by [`BRIEF`].
fn harness_of(engine: &Standing) -> (Harness<Screen>, TabKey, Building) {
    let mut screen = engine.screen();
    let tab = building_tab(&mut screen);
    let build = building(engine, &standing());
    let mut harness = harness(screen.stalling(BRIEF), 120, 36);
    harness.send(Msg::ImageLine(tab, build.next_line().expect("the build says what it is doing")));
    (harness, tab, build)
}

/// The screen's own first look: everything after this one is its own timer, which the harness's
/// clock carries round while the silence it judges is real time.
fn looks(harness: &mut Harness<Screen>) {
    harness.send(Msg::BuildLooked(Instant::now()));
}

/// Waits, bounded and generous, until the page says the build may be stuck in `said`, which is the
/// warning as the language the person reads it in writes it. A round is the screen's own look
/// coming round and one render, which is all that waiting for it asks of a person.
///
/// Every round says again that the build of `tab` is under way, since a warning is only words: a
/// screen that stopped the build the moment it warned about it is caught here rather than after.
fn waits_for_the_warning(harness: &mut Harness<Screen>, tab: TabKey, said: &str) {
    let until = Instant::now() + Duration::from_secs(10);
    while !harness.screen().contains(said) {
        assert!(Instant::now() < until, "the build was never warned about:\n{}", harness.screen());
        assert!(is_building(harness, tab), "the screen that warned stopped the build:\n{}", harness.screen());
        std::thread::sleep(Duration::from_millis(10));
        harness.advance(BRIEF.look);
    }
}

/// Whether the tab of `key` is still being built, which is all a person is shown of a build that is
/// under way.
fn is_building(harness: &Harness<Screen>, key: TabKey) -> bool {
    let Some(tab) = harness.app().0.workspace().expect("a workspace").tabs().iter().find(|tab| tab.key() == key) else {
        return false;
    };
    matches!(tab.state(), TabState::Building(_))
}

/// A look taken while the silence is still shorter than the rule, which is what the page says about
/// the minutes that started again with the build's last line.
fn looks_within_the_quiet(harness: &mut Harness<Screen>) {
    harness.send(Msg::BuildLooked(Instant::now() - BRIEF.quiet / 2));
}

#[test]
fn a_build_that_has_said_nothing_is_warned_about_and_is_left_running() {
    let engine = Standing::new("stalled", false);
    let (mut harness, tab, build) = harness_of(&engine);

    looks(&mut harness);
    waits_for_the_warning(&mut harness, tab, WARNING);

    let screen = harness.screen();
    assert!(screen.contains(WARNING), "the build is warned about:\n{screen}");
    assert!(screen.contains("Building the image of claude-sub"), "it is still being built:\n{screen}");
    assert!(screen.contains("STEP 1/9"), "the log is still there:\n{screen}");
    assert!(!build.let_go(), "the stand-in is still on the build");
    assert!(is_building(&harness, tab), "and the tab is still building:\n{screen}");

    // Between the row that stops it and the log it is silent in, which is where a person looks.
    let (stop, warning, line) = (
        harness.find("Stop").expect("the Stop button").1,
        harness.find(WARNING).expect("the warning").1,
        harness.find("STEP 1/9").expect("the log").1,
    );
    assert!(stop < warning && warning < line, "the warning stands under the Stop row and over the log: {screen}");

    harness.click_text("Stop").render();
    let state = harness.app().0.workspace().expect("a workspace").tabs()[0].state().clone();
    assert_eq!(state, TabState::NoImage, "the person's own Stop ends it as it always has");
    assert!(harness.screen().contains("Build it now"), "and the build is offered again:\n{}", harness.screen());
    assert!(!build.let_go(), "and nothing of QCode's stopped it on its own");
}

#[test]
fn a_line_from_the_build_takes_the_warning_away_and_the_silence_starts_again() {
    let engine = Standing::new("stalled-again", false);
    let (mut harness, tab, _build) = harness_of(&engine);

    looks(&mut harness);
    waits_for_the_warning(&mut harness, tab, WARNING);

    harness.send(Msg::ImageLine(tab, "STEP 2/9: RUN npm install -g @anthropic-ai/claude-code".to_owned()));
    assert!(!harness.screen().contains(WARNING), "a line ends the warning at once:\n{}", harness.screen());

    // The five minutes of the rule start again with the line, so a silence shorter than them is
    // still a silence that has not reached it.
    looks_within_the_quiet(&mut harness);
    assert!(!harness.screen().contains(WARNING), "and it is not warned about again so soon:\n{}", harness.screen());
}

#[test]
fn a_build_that_keeps_saying_something_is_never_warned_about() {
    let engine = Standing::new("stalled-talking", true);
    let mut screen = engine.screen();
    let tab = building_tab(&mut screen);
    let build = building(&engine, &standing());
    let quiet = Lengths { quiet: Duration::from_millis(300), look: Duration::from_millis(10) };
    let mut harness = harness(screen.stalling(quiet), 120, 36);
    harness.send(Msg::BuildLooked(Instant::now()));

    // A second of a build that says something every twentieth of a second, which is fifteen times
    // over the quiet it is judged by, never read as quiet at all.
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        if let Some(line) = build.next_line() {
            harness.send(Msg::ImageLine(tab, line));
        }
        harness.advance(quiet.look);
        assert!(
            !harness.screen().contains(WARNING),
            "a build that is talking is not warned about:\n{}",
            harness.screen()
        );
    }
    assert!(!build.let_go(), "and it is still going");
}

#[test]
fn the_warning_is_in_the_persons_own_language() {
    let engine = Standing::new("stalled-turkish", false);
    let (mut harness, tab, _build) = harness_of(&engine);
    harness.set_locale("tr");

    looks(&mut harness);
    waits_for_the_warning(&mut harness, tab, TURKISH);

    let screen = harness.screen();
    assert!(screen.contains(TURKISH), "the warning is said in Turkish:\n{screen}");
    assert!(!screen.contains(WARNING), "and not in the language it started in:\n{screen}");
}
