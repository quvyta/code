//! A build whose log has stopped moving on the image page: the page says the build may be stuck
//! under its own words about the build, and says nothing about stopping it. A build that keeps
//! saying something is never warned about, and the warning is said in the language chosen.
//!
//! The stand-in engine is really asked for the image and really holds the build open, but the
//! build runs in a thread of the test's own, which is why [`Screen`] drops the task the button
//! asks for: the harness calls a background task stuck after ten seconds of it working, and a
//! build is allowed to take as long as the engine takes. What the test watches is the screen and
//! the stand-in's own marker, never the machine's process table.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::*;

use crate::ui::profiles::work;
use crate::ui::stalling::Lengths;

/// The lengths a test judges a build by: a silence a moment of real time reaches, and a look the
/// harness's clock brings round at once.
const BRIEF: Lengths = Lengths { quiet: Duration::from_millis(50), look: Duration::from_millis(10) };

/// The word of the warning that does not carry the minutes, so that a test which shortens the
/// quiet can still say what the person is told.
const WARNING: &str = "It may be stuck; you can stop it.";

/// What the same warning says in Turkish, which is what a person who chose that language is waited
/// for instead.
const TURKISH: &str = "Takılmış olabilir";

/// The row the image page puts above its lead sentence while it is being built, which is where the
/// warning stands below: a shimmer that is one line and is never wrapped.
const SHIMMER: &str = "Building the image";

/// The profiles screen in an application that runs everything as QCode does, except the build the
/// rebuild button starts: that task is asked for and left unstarted, because the build it would run
/// is the test's own thread. Everything the person presses still is pressed, so the page the test
/// reads is the one the rebuild button opens.
struct Screen(Profiles);

impl App for Screen {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        let asked_for_a_build = matches!(message, Msg::RebuildConfirmed);
        let command = update(&mut self.0, message);
        // The task is asked for and left unstarted: the page opens with the rebuild under way,
        // and the build itself is the test's own thread rather than one the harness waits on.
        if asked_for_a_build { Command::none() } else { command }
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        AppShell::new().body(|ui| view(&self.0, ui)).show(ui);
    }
}

/// A stand-in podman that has the base image and the profile's image without the label of today's
/// recipe, the way an image an earlier QCode built has it, so the list offers the rebuild. Its
/// `build` says one line and then holds still, writing `ended` when it lets go. With `talk` in the
/// folder it keeps a line coming every twentieth of a second instead, which is a build going on.
struct Standing {
    folder: PathBuf,
    engine: Engine,
}

impl Standing {
    fn new(name: &str, talk: bool) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profiles-{name}-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a scratch folder");
        let (ended, chatter) = (folder.join("ended"), folder.join("talk"));
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
            talk = chatter.display(),
            ended = ended.display(),
        );
        let binary = folder.join("engine");
        std::fs::write(&binary, script).expect("the stand-in engine is written");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        if talk {
            std::fs::write(&chatter, "").expect("the build keeps talking");
        }
        let engine = Engine::new(crate::engine::EngineKind::Podman, &binary);
        Self { folder, engine }
    }

    /// The file the stand-in's build writes when it lets go of it.
    fn ended(&self) -> PathBuf {
        self.folder.join("ended")
    }

    /// A harness of that screen on a terminal wide enough for the page at its own width, so that
    /// the sentences it draws stand on one line each, with the silence judged by `lengths`.
    ///
    /// The listing is delivered as it comes off the store, so the engine is asked about the
    /// profile the way the screen asks for itself.
    fn harness(&self, profile: Profile, lengths: Lengths) -> Harness<Screen> {
        let state = Profiles::new(Some(self.folder.clone()), Some(self.engine.clone()))
            .with_providers_file(None)
            .stalling(lengths);
        let mut harness = Harness::with_env(Screen(state), env(), 120, 60);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Loaded(Listing { profiles: vec![profile], diagnostics: Vec::new() }));
        harness
    }
}

impl Drop for Standing {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The build of `profile`'s image over `engine` on `folder`, in a thread of the test's own, whose
/// lines are taken and handed to the screen the way the build's own task hands them over.
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

/// Builds the image of `profile` over `engine` exactly as the button's own build does, and says
/// through `ended` — the stand-in's own marker — when the engine let go of it.
fn building(engine: &Standing, profile: &Profile) -> Building {
    let (sender, lines) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let asked = Arc::clone(&stop);
    let podman = engine.engine.clone();
    let profiles = engine.folder.join("Profiles");
    let profile = profile.clone();
    let thread = std::thread::spawn(move || {
        let cancel = || asked.load(Ordering::SeqCst);
        let _ = work::rebuild(&podman, &profile, &profiles, &cancel, &mut || {}, &mut |line: &str| {
            let _ = sender.send(line.to_owned());
        });
    });
    Building { lines, stop, ended: engine.ended(), thread: Some(thread) }
}

/// A harness on the image page with the rebuild under way, the build already having said one line:
/// the person opens the rebuild with the list's own button, answers the question, and the line the
/// build's first step prints is delivered as the build's task delivers it.
fn rebuilding(engine: &Standing, profile: &Profile) -> (Harness<Screen>, Building) {
    let mut harness = engine.harness(profile.clone(), BRIEF);
    harness.click_text("Rebuild image").advance(Duration::from_millis(400));
    harness.click_text("Build it again").render();
    let build = building(engine, profile);
    harness.send(Msg::BuildLine(build.next_line().expect("the build says what it is doing")));
    (harness, build)
}

/// The screen's own first look: everything after this one is its own timer, which the harness's
/// clock carries round while the silence it judges is real time.
fn looks(harness: &mut Harness<Screen>) {
    harness.send(Msg::BuildLooked(Instant::now()));
}

/// Waits, bounded and generous, until the page says the build may be stuck in `said`, which is the
/// warning as the language the person reads it in writes it. A round is the screen's own look
/// coming round and one render, which is all that waiting for it asks of a person.
fn waits_for_the_warning(harness: &mut Harness<Screen>, said: &str) {
    let until = Instant::now() + Duration::from_secs(10);
    while !words(harness).contains(said) {
        assert!(Instant::now() < until, "the build was never warned about:\n{}", harness.screen());
        std::thread::sleep(Duration::from_millis(10));
        harness.advance(BRIEF.look);
    }
}

/// A look taken while the silence is still shorter than the rule, which is what the page says about
/// the minutes that started again with the build's last line.
fn looks_within_the_quiet(harness: &mut Harness<Screen>) {
    harness.send(Msg::BuildLooked(Instant::now() - BRIEF.quiet / 2));
}

/// The screen as one line of words, so that a sentence the page wrapped inside its frame is read
/// as the one sentence it is.
fn words(harness: &Harness<Screen>) -> String {
    harness.screen().split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn a_build_that_has_said_nothing_is_warned_about_and_is_left_running() {
    let engine = Standing::new("stalled", false);
    let (mut harness, build) = rebuilding(&engine, &profile("claude-sub", HarnessKind::ClaudeCode));
    looks(&mut harness);
    waits_for_the_warning(&mut harness, WARNING);

    let screen = words(&harness);
    assert!(screen.contains(WARNING), "the build is warned about:\n{screen}");
    assert!(screen.contains("STEP 1/9"), "the log is still there:\n{screen}");
    assert!(!build.let_go(), "the stand-in is still on the build");
    assert!(
        matches!(harness.app().0.draft().expect("open").build, Build::Running(_)),
        "the build is still running:\n{screen}"
    );

    // Under the words the page says about the build being under way, and over the log it is
    // silent in, which is where a person looks.
    let (lead, warning, line) = (
        harness.find(SHIMMER).expect("the build's own words").1,
        harness.find(WARNING).expect("the warning").1,
        harness.find("STEP 1/9").expect("the log").1,
    );
    assert!(lead < warning && warning < line, "the warning stands under the lead and over the log: {screen}");

    harness.click_text("Stop").render();
    assert_eq!(
        harness.app().0.draft().expect("open").build,
        Build::Stopped,
        "the person's own Stop ends it as it always has:\n{}",
        harness.screen()
    );
    assert!(!build.let_go(), "and nothing of QCode's stopped it on its own");
}

#[test]
fn a_line_from_the_build_takes_the_warning_away_and_the_silence_starts_again() {
    let engine = Standing::new("stalled-again", false);
    let (mut harness, build) = rebuilding(&engine, &profile("claude-sub", HarnessKind::ClaudeCode));
    looks(&mut harness);
    waits_for_the_warning(&mut harness, WARNING);

    harness.send(Msg::BuildLine("STEP 2/9: RUN npm install -g @anthropic-ai/claude-code".to_owned()));
    assert!(!words(&harness).contains(WARNING), "a line ends the warning at once:\n{}", harness.screen());

    // The five minutes of the rule start again with the line, so a silence shorter than them is
    // still a silence that has not reached it.
    looks_within_the_quiet(&mut harness);
    assert!(!words(&harness).contains(WARNING), "and it is not warned about again so soon:\n{}", harness.screen());
    assert!(!build.let_go(), "and the build is still going");
}

#[test]
fn a_build_that_ends_is_not_left_warned_about() {
    let engine = Standing::new("stalled-ended", false);
    let (mut harness, build) = rebuilding(&engine, &profile("claude-sub", HarnessKind::ClaudeCode));
    looks(&mut harness);
    waits_for_the_warning(&mut harness, WARNING);

    // The build finishes of itself, and the page stays up to say so, so a warning left standing
    // would tell the person to stop a build that is already made.
    harness.send(Msg::BuildEnded(Ok(())));
    assert!(!words(&harness).contains(WARNING), "a build that ended is not warned about:\n{}", harness.screen());
    assert!(!build.let_go(), "and nothing stopped it to get there");
}

#[test]
fn a_build_that_keeps_saying_something_is_never_warned_about() {
    let engine = Standing::new("stalled-talking", true);
    let quiet = Lengths { quiet: Duration::from_millis(300), look: Duration::from_millis(10) };
    let mut harness = engine.harness(profile("claude-sub", HarnessKind::ClaudeCode), quiet);
    harness.click_text("Rebuild image").advance(Duration::from_millis(400));
    harness.click_text("Build it again").render();
    let build = building(&engine, &profile("claude-sub", HarnessKind::ClaudeCode));
    harness.send(Msg::BuildLine(build.next_line().expect("the build says what it is doing")));
    looks(&mut harness);

    // A second of a build that says something every twentieth of a second, which is fifteen times
    // over the quiet it is judged by, never read as quiet at all.
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        if let Some(line) = build.next_line() {
            harness.send(Msg::BuildLine(line));
        }
        harness.advance(quiet.look);
        assert!(
            !words(&harness).contains(WARNING),
            "a build that is talking is not warned about:\n{}",
            harness.screen()
        );
    }
    assert!(!build.let_go(), "and it is still going");
}

#[test]
fn the_warning_is_in_the_persons_own_language() {
    let engine = Standing::new("stalled-turkish", false);
    let (mut harness, _build) = rebuilding(&engine, &profile("claude-sub", HarnessKind::ClaudeCode));
    harness.set_locale("tr");
    looks(&mut harness);
    waits_for_the_warning(&mut harness, TURKISH);

    let screen = words(&harness);
    assert!(screen.contains(TURKISH), "the warning is said in Turkish:\n{screen}");
    assert!(!screen.contains(WARNING), "and not in the language it started in:\n{screen}");
}
