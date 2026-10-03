//! What the screen does with a profile's container when nothing of it is on screen and it has done
//! nothing for a while: a minute that comes round, a reading per profile, and a container that is
//! paused and written out to swap — and every reason one is not.
//!
//! There is no container runtime anywhere near this file. The engine is a stand-in script that
//! answers the state a container is in, writes down every call it is given, and hands over the
//! control group a container has and the command lines of the processes in it. The screen's own
//! lengths are shortened to a second or two, since a test cannot wait ten minutes, and the machine's
//! control groups are a folder of the test's own with a `cpu.stat` and a writable `memory.reclaim`
//! in it.
//!
//! The test application carries the settings screen over the workspace screen the way QCode opens
//! it from the rail, so that turning freezing off is a click on the switch and nothing else.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;

use crate::bridge::protocol::Kind;
use crate::ui::settings::Request;
use crate::ui::workspace::bridge::{QUIET, RETRY_EVERY};
use crate::ui::workspace::{Letter, freeze};

/// The lengths every test here judges by: long enough for a moment of work to show and short enough
/// that a test reaches them.
const BRIEF: freeze::Lengths = freeze::Lengths { quiet: Duration::from_secs(1), window: Duration::from_secs(1) };

/// The look's period, which a test advances by rather than waits out.
const SOON: Duration = Duration::from_millis(40);

/// How far the harness's clock moves in one settle: a fifth of the period, so a look is four
/// settles away and the timer armed after one is four more. A helper that moves the clock a fixed
/// number of steps therefore brings up a fixed number of looks, which is what makes the number of
/// readings in a test mean something.
const STEP: Duration = Duration::from_millis(8);

/// The settles one look takes: four for the timer to come round, one for the reading it set off,
/// one for what the reading came to, and one for the freeze a verdict may have asked for.
const A_LOOK: usize = 8;

/// The stand-in engine's own words for the processes a quiet profile's container holds.
const IDLE: &str = "sleep 3600\nclaude --dangerously-skip-permissions\n";

/// The workspace screen, with the settings standing over it and a way in for what the person does
/// on either.
struct Frozen(WorkspaceScreen, Option<crate::ui::settings::Settings>);

#[derive(Debug, Clone)]
enum Test {
    Screen(Msg),
    /// The screen has been opened, which is what the application does when a workspace is entered:
    /// the open workspace is opened and the screen's own work for it — the relay, the bridge, the
    /// backups and the question of what this machine can do about freezing — is run.
    Entered,
    /// The workspace is closed from the rail and opened again from the list, with a harness tab of
    /// each profile in it and the last one shown: what a person who closed a workspace and opened
    /// it again has. Each tab runs a program that says nothing, standing in for the harness its
    /// container would be running.
    Reopened(Vec<(&'static str, HarnessKind)>, WorkspacePaths),
    /// A message waits in the tab of this key, as an agent of another tab left it.
    Letter(TabKey),
    /// The screen's own timer looks at the tabs holding messages as of this moment.
    Deliver(Instant),
    /// The screen's own timer comes round twice before the engine has answered anything the first
    /// time asked of it, which is what a slow engine does to a timer that keeps its own pace.
    DeliverTwice(Instant, Instant),
    /// The settings are opened over the screen, or closed.
    ShowSettings(bool),
    /// Something happened on the settings screen.
    Settings(crate::ui::settings::Msg),
}

impl App for Frozen {
    type Msg = Test;

    fn update(&mut self, message: Test) -> Command<Test> {
        match message {
            Test::Screen(message) => super::super::update(&mut self.0, message).map(Test::Screen),
            Test::Entered => {
                let opened = super::super::update(&mut self.0, Msg::OpenWorkspace(0));
                let entering = super::super::opened(&mut self.0);
                Command::batch([opened.map(Test::Screen), entering.map(Test::Screen)])
            }
            Test::Reopened(names, paths) => {
                let closed = super::super::update(&mut self.0, Msg::CloseWorkspace(0));
                let profiles: Vec<Profile> = names.iter().map(|(name, kind)| profile(name, *kind)).collect();
                let opened = super::super::add(&mut self.0, workspace("firefly", "Firefly", paths, profiles));
                for (name, _) in &names {
                    // The tab is opened as a person opens it, and the work that brings its
                    // container up is left undone, since a program that says nothing is attached
                    // instead: what a harness in a container doing nothing looks like from here.
                    apply(&mut self.0, Msg::NewTab);
                    let key =
                        self.0.workspace().and_then(OpenWorkspace::active_tab).map(Tab::key).expect("a blank tab");
                    apply(&mut self.0, Msg::Choose(key, Choice::NewChat((*name).to_owned())));
                    self.0.attach(key, quiet_program());
                }
                apply(&mut self.0, Msg::OpenTab(names.len().saturating_sub(1)));
                Command::batch([closed.map(Test::Screen), opened.map(Test::Screen)])
            }
            Test::Letter(key) => {
                if let Some((_, tab)) = self.0.owner_mut(key).and_then(|workspace| workspace.find(key)) {
                    tab.receive(letter());
                }
                Command::none()
            }
            Test::Deliver(now) => super::super::bridge::deliver(&mut self.0, now).map(Test::Screen),
            Test::DeliverTwice(first, second) => {
                let first = super::super::bridge::deliver(&mut self.0, first);
                let second = super::super::bridge::deliver(&mut self.0, second);
                Command::batch([first, second]).map(Test::Screen)
            }
            Test::ShowSettings(open) => {
                // The screen shows what the settings file holds, which is what the workspace screen
                // was last given.
                let file = if self.0.freezes_idle() { "" } else { "freeze_idle = false\n" };
                self.1 = open.then(|| {
                    let health = crate::ui::settings::engine::Health::Working;
                    crate::ui::settings::testing::from_config(file, EngineKind::Podman, health)
                });
                Command::none()
            }
            Test::Settings(message) => {
                let Some(settings) = self.1.as_mut() else { return Command::none() };
                let (command, request) = crate::ui::settings::update(settings, message);
                // What QCode does with the request: the switch reaches the open workspace screen.
                if let Some(Request::FreezeIdle(freeze)) = request {
                    self.0.set_freeze_idle(freeze);
                }
                command.map(Test::Settings)
            }
        }
    }

    fn view(&self, ui: &mut View<'_, Test>) {
        match &self.1 {
            Some(settings) => {
                ui.map(Test::Settings, |ui| crate::ui::settings::view(settings, ui)).fill();
            }
            None => super::super::view(&self.0, ui, Test::Screen, |_| {}, |_| {}, |_| {}),
        }
    }
}

/// A stand-in engine that writes every call down, says it runs as the person who started it — which
/// is half of what this machine can do about freezing — answers the state of a container it is asked
/// about, hands over the control group a container has and the command lines of the processes in
/// it, and lets a freeze write into its `memory.reclaim`.
///
/// The processes of a container are whatever `processes-<name>` holds in the test's own folder, and
/// whatever `processes` holds for a container nobody named one for: so a test makes a profile's
/// container idle or working by writing that file before the engine is made and nothing else, and
/// two profiles of different harnesses are told apart by each having a file of its own. A file that
/// is not there yet is a quiet container: the harness and the sleep it waits on, and nothing else.
///
/// It lists the containers it has a control group for, each as paused or running by the same file,
/// and a pause it takes writes that file and a wake removes it.
///
/// It says a container is paused while a file named after it is in the same folder, which is how a
/// container QCode froze is stood in for as far as the engine's own answers go: the engine refuses
/// to stop or start a paused one, so anything asked of it in that state has to wake it first.
///
/// It refuses to wake a container while a file called `refuse-unpause` is there, which is what an
/// engine answers for a container that something else woke first.
///
/// It refuses to pause a container while a file called `refuse` is in the same folder, which is how
/// an engine that will not pause is stood in for: the freeze fails, which is kept quiet, and the
/// container is read again by every look that follows.
fn stand_in(folder: &Path) -> (Engine, PathBuf) {
    let (binary, calls, processes) = (folder.join("engine"), folder.join("calls"), folder.join("processes"));
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$*\" >> {calls}\n\
         [ \"$1\" = 'info' ] && {{ printf 'true\\n'; exit 0; }}\n\
         [ \"$1\" = 'pause' ] && [ -e {folder}/refuse ] && exit 1\n\
         [ \"$1\" = 'pause' ] && {{ : > {folder}/paused-$2; exit 0; }}\n\
         [ \"$1\" = 'unpause' ] && [ -e {folder}/refuse-unpause ] && exit 1\n\
         [ \"$1\" = 'unpause' ] && {{ rm -f {folder}/paused-$2; exit 0; }}\n\
         if [ \"$1\" = 'ps' ]; then for g in {folder}/cgroup/libpod-*.scope; do n=${{g##*/libpod-}}; n=${{n%.scope}}; \
         if [ -e {folder}/paused-$n ]; then printf '%s\\tpaused\\n' \"$n\"; else printf '%s\\trunning\\n' \"$n\"; fi; done; exit 0; fi\n\
         if [ \"$1 $2\" = 'container inspect' ]; then case \"$4\" in\n\
         '{{{{.State.Status}}}}') if [ -e {folder}/paused-$5 ]; then printf 'paused\\n'; else printf 'running\\n'; fi;;\n\
         '{{{{.State.CgroupPath}}}}') printf '/libpod-%s.scope\\n' \"$5\";;\n\
         esac; exit 0; fi\n\
         [ \"$1\" = 'exec' ] && {{ cat {folder}/processes-$2 2>/dev/null || cat {processes} 2>/dev/null; exit 0; }}\n\
         exit 0\n",
        calls = calls.display(),
        processes = processes.display(),
        folder = folder.display(),
    );
    fs::write(&binary, script).expect("the stand-in engine is written");
    fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
    if !processes.exists() {
        fs::write(&processes, IDLE).expect("the processes of a quiet container");
    }
    (Engine::new(EngineKind::Podman, &binary), calls)
}

/// The processes the stand-in engine reports for the container of `profile`: the sleep every
/// container waits on and the harness of that profile, which is what a container doing nothing at
/// all holds. A test that makes a container work writes that file over this one, which the engine
/// reads afresh on every look.
fn held(scratch: &Scratch, profile: &str, harness: HarnessKind) {
    fs::write(
        scratch.0.join(format!("processes-{}", container_of(profile))),
        format!("sleep 3600\n{}\n", harness.record().command),
    )
    .expect("the processes a quiet container of that harness holds");
}

/// A scratch cgroup root holding a folder for `container` with the `cpu.stat` a look reads and the
/// `memory.reclaim` a freeze writes into, and the root the engine named those under.
///
/// The `usage_usec` is the CPU the container has used so far; a test raises it between two looks to
/// say that the container has been working.
fn cgroup(folder: &Path, container: &str, used: u64) {
    let group = folder.join("cgroup").join(format!("libpod-{container}.scope"));
    fs::create_dir_all(&group).expect("a control group of the test's own");
    fs::write(group.join("cpu.stat"), format!("usage_usec {used}\nuser_usec 0\nnr_periods 0\n"))
        .expect("the CPU the container has used");
    fs::write(group.join("memory.reclaim"), "").expect("the pages are reclaimed through this");
}

/// The CPU written into `container`'s `cpu.stat`, raised by `grew` and written back.
fn spent(folder: &Path, container: &str, grew: u64) {
    let stat = folder.join("cgroup").join(format!("libpod-{container}.scope")).join("cpu.stat");
    let used: u64 =
        fs::read_to_string(&stat).unwrap_or_default().split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
    fs::write(stat, format!("usage_usec {}\n", used + grew)).expect("the container's CPU");
}

/// Every call the stand-in engine was given, in order.
fn asked(calls: &Path) -> Vec<String> {
    fs::read_to_string(calls).unwrap_or_default().lines().map(str::to_owned).collect()
}

/// Every container the stand-in engine was asked to pause, by name.
fn paused(calls: &Path) -> Vec<String> {
    asked(calls).into_iter().filter_map(|call| call.strip_prefix("pause ").map(str::to_owned)).collect()
}

/// What was written into `container`'s `memory.reclaim`, which a freeze writes `4G` into.
fn reclaimed(folder: &Path, container: &str) -> String {
    fs::read_to_string(folder.join("cgroup").join(format!("libpod-{container}.scope")).join("memory.reclaim"))
        .unwrap_or_default()
}

/// The name of the container a profile's terminal tabs of this workspace run in.
fn container_of(profile: &str) -> String {
    format!("qcode-firefly-{profile}")
}

/// A message another tab of the workspace left for a harness, as the bridge hands it over.
fn letter() -> Letter {
    Letter {
        from: "Codex · codex-main".to_owned(),
        tab: 2,
        title: "codex-main".to_owned(),
        harness: "Codex".to_owned(),
        kind: Kind::Info,
        text: "Please write the test for parse().".to_owned(),
    }
}

/// Clicks the tab `title` on the strip, where the person's hand is: the strip is the first row, and
/// the panel's own list of containers is not.
fn click_tab(harness: &mut Harness<Frozen>, title: &str) {
    let line: Vec<char> = harness.screen().lines().next().unwrap_or_default().chars().collect();
    let title: Vec<char> = title.chars().collect();
    let at = line.windows(title.len()).position(|cells| cells == title.as_slice()).expect("the tab is on the strip");
    harness.click(i32::try_from(at).expect("a column"), 0).advance(STEP).render();
}

/// Clicks the close mark of the tab `title` on the strip, which is the person's other way of
/// closing a tab.
fn click_close(harness: &mut Harness<Frozen>, title: &str) {
    let line: Vec<char> = harness.screen().lines().next().unwrap_or_default().chars().collect();
    let title: Vec<char> = title.chars().collect();
    let at = line.windows(title.len()).position(|cells| cells == title.as_slice()).expect("the tab is on the strip");
    let mark = (at + title.len()..line.len()).find(|&x| line[x] == '×').expect("the tab has a close mark");
    harness.click(i32::try_from(mark).expect("a column"), 0).advance(STEP).render();
}

/// Looks until the container of the first profile is frozen, which is what the whole of this file
/// needs to go on from, and says what the engine was asked while it happened.
fn frozen(harness: &mut Harness<Frozen>, scratch: &Scratch) {
    look_again(harness);
    after_quiet();
    look_after(harness, scratch, 0);
    look_again(harness);
    assert_eq!(
        paused(&scratch.0.join("calls")),
        [container_of("claude-sub")],
        "the first profile's container is frozen:\n{:?}",
        asked(&scratch.0.join("calls"))
    );
}

/// Lets the screen's own work come round, so that what it asked the engine in the background has
/// been asked and answered.
fn settled(harness: &mut Harness<Frozen>) {
    for _ in 0..4 {
        harness.advance(STEP).render();
    }
}

/// Every call the stand-in engine was given that is about `container`, in order.
fn asked_of(calls: &Path, container: &str) -> Vec<String> {
    asked(calls).into_iter().filter(|call| call.contains(container)).collect()
}

/// Every call the stand-in engine was given that acts on `container` itself, in order. What was
/// only read out of it is not one of them: a look reads a container's control group and its
/// processes, which says nothing about what was done to the container.
fn acted_on(calls: &Path, container: &str) -> Vec<String> {
    asked(calls)
        .into_iter()
        .filter(|call| ["unpause", "stop", "start"].contains(&call.split(' ').next().unwrap_or_default()))
        .filter(|call| call.ends_with(container))
        .collect()
}

/// The cell of `text` in the panel's own column, which is where a row of the panel's list stands
/// and where nothing of the tab strip stands.
fn in_panel(harness: &Harness<Frozen>, text: &str) -> (i32, i32) {
    let width = usize::from(harness.app().0.panel().width());
    for (row, line) in harness.screen().lines().enumerate() {
        let cells: Vec<char> = line.chars().collect();
        let from = cells.len().saturating_sub(width);
        let panel: String = cells[from..].iter().collect();
        if let Some(at) = panel.find(text) {
            let column = from + panel[..at].chars().count();
            return (i32::try_from(column).expect("a column"), i32::try_from(row).expect("a row"));
        }
    }
    panic!("`{text}` is in the panel:\n{}", harness.screen());
}

/// The panel's own list of the workspace's containers, with the first profile's container frozen as
/// far as the engine's answers go, and the row of that container selected by a click on it.
fn panel_with_a_frozen_container(harness: &mut Harness<Frozen>, scratch: &Scratch) -> String {
    let container = container_of("claude-sub");
    fs::write(scratch.0.join(format!("paused-{container}")), "").expect("a frozen container");
    let listed = vec![
        Container { name: container.clone(), state: ContainerState::Paused },
        Container { name: container_of("codex-main"), state: ContainerState::Running },
    ];
    harness.send(Test::Screen(Msg::ContainersRead("firefly".to_owned(), Ok(listed)))).render();
    // The panel shows the part of the name that differs, which is the profile's own name.
    let (column, row) = in_panel(harness, "claude-sub");
    harness.click(column, row).advance(STEP).render();
    container
}

/// The two profiles of a workspace, a harness tab of each, and the second tab the one shown.
///
/// Two profiles because a look judges each on its own: the tab that is on screen may never be
/// frozen, and a test with one profile could not tell a rule from the screen's own state. The
/// screen judges by `lengths`: [`BRIEF`] for most tests, a window longer than the look for some.
fn screen_judging(scratch: &Scratch, engine: Engine, lengths: freeze::Lengths) -> Frozen {
    let profiles = vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("codex-main", HarnessKind::Codex)];
    let mut screen = WorkspaceScreen::new(
        Some(engine),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .looking(lengths, SOON, scratch.0.join("cgroup"));
    open(&mut screen, Choice::NewChat("claude-sub".to_owned()));
    open(&mut screen, Choice::NewChat("codex-main".to_owned()));
    Frozen(screen, None)
}

/// A screen whose two harness tabs are both running programs, with the second one shown, and whose
/// engine is the stand-in: what a look is answered against is a real screen, not a message.
///
/// Each container holds what its own harness holds, so nothing keeps either profile awake except the
/// rules themselves: the one that is not on screen is the only one that may be frozen.
fn quiet_screen(scratch: &Scratch) -> (Frozen, PathBuf) {
    quiet_screen_judging(scratch, BRIEF)
}

/// [`quiet_screen`], judging by `lengths` rather than [`BRIEF`].
fn quiet_screen_judging(scratch: &Scratch, lengths: freeze::Lengths) -> (Frozen, PathBuf) {
    held(scratch, "claude-sub", HarnessKind::ClaudeCode);
    held(scratch, "codex-main", HarnessKind::Codex);
    let (engine, calls) = stand_in(&scratch.0);
    let mut screen = screen_judging(scratch, engine, lengths);
    for index in 0..2 {
        let session = quiet_program();
        screen.0.attach(key(&screen.0, index), session);
    }
    apply(&mut screen.0, Msg::OpenTab(1));
    (screen, calls)
}

/// A program in a tab, standing in for the harness a container would run: it says nothing, so how
/// long a tab has been quiet is the time since it was attached.
fn quiet_program() -> qframe::widgets::TerminalSession {
    TerminalSession::spawn("/bin/sh".as_ref(), &["-c", "sleep 3600"], Path::new("/")).expect("a terminal for the tab")
}

/// A program that writes a line every fifth of the quiet time, as a harness at work draws its
/// spinner: however long a test waits, the tab has written something inside the quiet time.
fn speaking_program() -> qframe::widgets::TerminalSession {
    TerminalSession::spawn(
        "/bin/sh".as_ref(),
        &["-c", "while :; do printf 'working\\n'; sleep 0.2; done"],
        Path::new("/"),
    )
    .expect("a terminal for the tab")
}

/// A harness over a screen, with the workspace opened and drawn, and the control groups of both
/// containers in the test's own folder.
///
/// The screen is entered the way the application enters it, through [`opened`], which is where the
/// question of what this machine can do about freezing is asked: a screen that is only updated is
/// not a screen anyone has looked at, and the look belongs to entering it.
fn harness(screen: Frozen, scratch: &Scratch) -> Harness<Frozen> {
    for profile in ["claude-sub", "codex-main"] {
        cgroup(&scratch.0, &container_of(profile), 0);
    }
    let mut harness = Harness::with_env(screen, env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.send(Test::Entered).advance(SOON).render();
    harness
}

/// Lets the screen look once more, and the work that look set off run to its end.
fn look_again(harness: &mut Harness<Frozen>) {
    for _ in 0..A_LOOK {
        harness.advance(STEP).render();
    }
}

/// Waits for the quiet time the screen is judging by, so that a tab which has said nothing since it
/// was attached really has said nothing for it.
fn after_quiet() {
    std::thread::sleep(BRIEF.quiet + Duration::from_millis(50));
}

/// A look after the containers have used `grew` microseconds of CPU since the reading before it.
fn look_after(harness: &mut Harness<Frozen>, scratch: &Scratch, grew: u64) {
    for profile in ["claude-sub", "codex-main"] {
        spent(&scratch.0, &container_of(profile), grew);
    }
    look_again(harness);
}

#[test]
fn a_profile_nobody_is_looking_at_is_frozen_once_it_has_been_quiet_and_done_nothing() {
    let scratch = Scratch::new("freezing-idle");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 0);

    if paused(&calls).is_empty() {
        panic!("ASKED:\n{:#?}", asked(&calls));
    }
    assert_eq!(paused(&calls), [container_of("claude-sub")], "the profile whose tab is not on screen");
    assert_eq!(reclaimed(&scratch.0, &container_of("claude-sub")), "4G", "and its pages were handed back");
    assert_eq!(reclaimed(&scratch.0, &container_of("codex-main")), "", "the shown one was not touched at all");
}

#[test]
fn a_container_seen_for_the_first_time_is_only_read_and_the_look_after_it_may_freeze_it() {
    // The CPU is a difference against the reading before it, so a container with no earlier reading
    // has no CPU to be judged on. The first look takes that reading; the next one finds the
    // container quiet and idle, and the one after that freezes it.
    let scratch = Scratch::new("freezing-first");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    look_again(&mut harness);
    assert!(paused(&calls).is_empty(), "the first reading is not a verdict:\n{:?}", asked(&calls));
    after_quiet();
    look_after(&mut harness, &scratch, 0);
    assert_eq!(paused(&calls), [container_of("claude-sub")], "the look after it freezes the profile");
}

#[test]
fn a_workspace_that_leaves_the_rail_and_comes_back_is_read_from_this_moment() {
    // The CPU is a difference against the reading before it, and a reading of a container whose
    // workspace is not on the screen says nothing about what the container did while it was gone.
    // So the first look after the workspace comes back takes the reading and judges nothing, the
    // way the first look of any container does.
    //
    // The engine here refuses to pause, so the container is read by every look: a container QCode
    // could not freeze is not one it leaves alone, and a refused freeze is said nothing to the
    // person.
    let scratch = Scratch::new("freezing-left");
    fs::write(scratch.0.join("refuse"), "").expect("an engine that will not pause");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 0);
    assert_eq!(paused(&calls), [container_of("claude-sub")], "the look asks the engine to pause");
    assert!(
        !harness.app().0.freezing.is_frozen(&container_of("claude-sub")),
        "which refused, so the container is not called frozen and nothing is said to the person:\n{}",
        harness.screen()
    );

    let profiles = vec![("claude-sub", HarnessKind::ClaudeCode), ("codex-main", HarnessKind::Codex)];
    // What was asked before the workspace came back is of no account: the container is read again
    // from this moment, and the first reading is not a verdict.
    fs::write(&calls, "").expect("the calls before the workspace came back");
    harness.send(Test::Reopened(profiles, scratch.paths())).render();
    after_quiet();
    look_again(&mut harness);
    after_quiet();
    look_again(&mut harness);

    // The freeze is asked for only between two readings taken since the workspace came back: a
    // reading of a container that was not on the screen says nothing about what it did while it
    // was gone.
    let container = container_of("claude-sub");
    let asked = asked(&calls);
    let pause = asked
        .iter()
        .position(|call| call == &format!("pause {container}"))
        .unwrap_or_else(|| panic!("a quiet profile is frozen: {asked:#?}"));
    let reads = asked[..pause].iter().filter(|call| call.starts_with(&format!("exec {container}"))).count();
    assert!(reads >= 2, "the container is read afresh before it is judged:\n{asked:#?}");
}

#[test]
fn a_tab_that_wrote_something_within_the_quiet_time_is_not_frozen() {
    // The rule is the quiet, not the container: a profile whose tab has written nothing for the
    // quiet time is idle, and one whose tab has written inside it has a harness still going.
    let scratch = Scratch::new("freezing-speaking");
    let (mut screen, calls) = quiet_screen(&scratch);
    let session = speaking_program();
    screen.0.attach(key(&screen.0, 0), session);
    let mut harness = harness(screen, &scratch);
    // Every other rule is met: the container is idle, holds only its harness, and the window the CPU
    // is read over is as long as the rules ask. The tab's own output is the one thing left.
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 0);
    look_again(&mut harness);
    assert!(paused(&calls).is_empty(), "a tab that has just written is not quiet:\n{:?}", asked(&calls));
}

/// The product's proportion: the CPU window is longer than the time between two looks, as two
/// minutes are longer than the minute the screen looks every. Quiet as [`BRIEF`]; the window three
/// times that, so that even a machine slowed down by a build leaves two looks inside it.
const LONG_WINDOW: freeze::Lengths = freeze::Lengths { quiet: Duration::from_secs(1), window: Duration::from_secs(3) };

/// Waits until [`LONG_WINDOW`]'s window has passed since `first`, the moment of the first reading,
/// whatever the machine's load did to the steps before.
fn after_the_window(first: Instant) {
    let until = first + LONG_WINDOW.window + Duration::from_millis(100);
    std::thread::sleep(until.saturating_duration_since(Instant::now()));
}

#[test]
fn a_window_longer_than_the_look_is_measured_across_looks_and_a_quiet_profile_is_frozen() {
    // Looks come round more often than the window is long, as in the product, so no single gap
    // between two looks is a window long enough to be judged on. The reading the window is measured
    // from is kept across looks until it is, and then the quiet profile is frozen. Measured from the
    // look just before, as it once was, the last gap is about two seconds of a three-second window
    // and nothing is ever frozen.
    let scratch = Scratch::new("freezing-window");
    let (screen, calls) = quiet_screen_judging(&scratch, LONG_WINDOW);
    let mut harness = harness(screen, &scratch);
    let first = Instant::now();
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 0);
    // Only a look that really came inside the window says anything here: a machine under load may
    // bring it late, and then a verdict is what it should give.
    if first.elapsed() < LONG_WINDOW.window {
        assert!(paused(&calls).is_empty(), "a second of quiet is not yet the window:\n{:?}", asked(&calls));
    }
    after_the_window(first);
    look_after(&mut harness, &scratch, 0);
    assert_eq!(paused(&calls), [container_of("claude-sub")], "once the window is long enough, it freezes");
}

#[test]
fn work_anywhere_in_a_window_longer_than_the_look_keeps_the_container_awake() {
    // The work happened between the first two looks; the look that finally has a window long enough
    // sees it, because the window is measured from the reading before the work, not from the look
    // just before.
    let scratch = Scratch::new("freezing-window-work");
    let (screen, calls) = quiet_screen_judging(&scratch, LONG_WINDOW);
    let mut harness = harness(screen, &scratch);
    let first = Instant::now();
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 2_000_000);
    after_the_window(first);
    look_after(&mut harness, &scratch, 0);
    assert!(paused(&calls).is_empty(), "two seconds of CPU in the window is work:\n{:?}", asked(&calls));
}

#[test]
fn a_container_that_has_been_working_is_not_frozen() {
    // Under the share is under it: a look reads the CPU the container used since the reading before
    // it, and a second of CPU over a second of window is a whole core.
    let scratch = Scratch::new("freezing-cpu");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 2_000_000);
    assert!(paused(&calls).is_empty(), "a whole core in a second is work:\n{:?}", asked(&calls));
}

#[test]
fn a_container_with_something_running_in_it_is_not_frozen() {
    // A build, a test or any long command breaks the share at once, whatever the CPU says of the
    // second it was read over.
    let scratch = Scratch::new("freezing-working");
    let (screen, calls) = quiet_screen(&scratch);
    fs::write(scratch.0.join(format!("processes-{}", container_of("claude-sub"))), format!("{IDLE}cargo build\n"))
        .expect("a build in the container");
    let mut harness = harness(screen, &scratch);
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 0);
    assert!(paused(&calls).is_empty(), "a build in the container is work whatever the CPU says:\n{:?}", asked(&calls));
}

#[test]
fn a_look_is_not_asked_at_all_while_the_person_has_turned_freezing_off() {
    // The switch in the settings, reached as the person reaches it. With it off the engine is not
    // even asked what this machine could do, so nothing is read and no timer is ever timed.
    let scratch = Scratch::new("freezing-off");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    harness.send(Test::ShowSettings(true));
    harness.resize(SIZE.0, 70).render();
    // A switch is drawn in colour alone; it stands at the column's right edge, which the language
    // drop-down's arrow marks.
    let (_, row) = harness.find("Freeze quiet profiles in the background").expect("the row is on screen");
    let (edge, _) = harness.find("▾").expect("the language drop-down");
    harness.click(edge - 1, row).advance(STEP).render();
    assert!(!harness.app().0.freezes_idle(), "the switch reached the screen:\n{}", harness.screen());
    harness.send(Test::ShowSettings(false));
    // What was asked before the switch moved is of no account: a container's own listing and the
    // question of what this machine can do are the workspace's, not the freeze's.
    fs::write(&calls, "").expect("the calls before the switch moved");
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 0);
    let asked = asked(&calls);
    assert!(asked.is_empty(), "nothing at all is asked of the engine from here on: {asked:?}");
}

#[test]
fn a_person_who_opens_a_tab_of_a_frozen_profile_gets_its_container_woken_first() {
    // A paused container refuses every command the screen runs in it and answers nothing to the
    // harness in it, so the wake is the first and only thing asked of that container when the tab
    // is shown: whatever else the screen does with the tab, it is done in an awake container.
    let scratch = Scratch::new("freezing-shown");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    fs::write(&calls, "").expect("the calls before the tab was opened");
    click_tab(&mut harness, "claude-sub");
    settled(&mut harness);
    let about = asked_of(&calls, &container);
    assert_eq!(
        about.first(),
        Some(&format!("unpause {container}")),
        "the wake is the first thing asked of it:\n{about:#?}"
    );
    assert!(!harness.app().0.freezing.is_frozen(&container), "and the screen does not call it frozen any more");
    assert_eq!(reclaimed(&scratch.0, &container), "4G", "its pages were handed back to the machine when it was frozen");
}

#[test]
fn a_message_waiting_in_a_frozen_tab_waits_for_the_container_and_goes_in_once_it_is_awake() {
    // A paste into a harness that is not running goes nowhere, so the message stays in the tab
    // where the person can read it until the container is awake, and the tab's own timer then hands
    // it over the way any other message is handed over.
    let scratch = Scratch::new("freezing-letter");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    let tab = harness.app().0.workspace().expect("a workspace").tabs()[0].key();
    let session = harness.app().0.workspace().expect("a workspace").tabs()[0].session().cloned().expect("a program");
    // The moment the screen's own timer would reach, once the tab has fallen quiet for as long as
    // the bridge waits before it writes into a harness.
    let at = session.last_output() + QUIET;
    harness.send(Test::Letter(tab));
    fs::write(&calls, "").expect("the calls before the message was looked at");
    harness.send(Test::Deliver(at)).advance(STEP).render();
    settled(&mut harness);
    let waiting = |harness: &Harness<Frozen>| {
        let tab = &harness.app().0.workspace().expect("a workspace").tabs()[0];
        (tab.letters().len(), tab.owed().is_some())
    };
    let about = asked_of(&calls, &container);
    assert_eq!(about.first(), Some(&format!("unpause {container}")), "the container is woken for it:\n{about:#?}");
    assert_eq!(waiting(&harness), (1, false), "and the message waits rather than going into nothing");

    // The next time round, which the tab's own timer brings, the message goes in as it always does.
    harness.send(Test::Deliver(at + RETRY_EVERY)).advance(STEP).render();
    assert_eq!(waiting(&harness), (0, true), "the message is in the prompt and its Return is on its way");
}

#[test]
fn closing_a_tab_of_a_frozen_profile_wakes_its_container_before_it_ends_the_work_in_it() {
    // What ends a harness's processes is a command run inside its container, and a paused container
    // refuses it: a tab closed without waking the container would leave the agent running in one
    // nobody is left to read, until the reaper stops it.
    let scratch = Scratch::new("freezing-close");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    fs::write(&calls, "").expect("the calls before the tab was closed");
    click_close(&mut harness, "claude-sub");
    harness.click_text("End and close").advance(STEP).render();
    settled(&mut harness);
    let about = asked_of(&calls, &container);
    assert_eq!(about.first(), Some(&format!("unpause {container}")), "the container is woken first:\n{about:#?}");
    assert!(
        about.iter().any(|call| call.starts_with("exec") && call.contains("kill -TERM")),
        "and the work in it is ended:\n{about:#?}"
    );
    assert!(!harness.app().0.freezing.is_frozen(&container), "and the screen does not call it frozen any more");
}

#[test]
fn the_panels_own_stop_and_restart_wake_a_frozen_container_before_they_act_on_it() {
    // The engine refuses to stop a paused container, so the panel's own Stop would leave it as it
    // is and say so, and the Restart after it would refuse to start it: both wake it first.
    let scratch = Scratch::new("freezing-buttons");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    // The panel runs the whole height of the screen, and the list of containers is under the tree of
    // files, so the screen is given the rows the list stands on.
    harness.resize(SIZE.0, 80).render();

    let container = panel_with_a_frozen_container(&mut harness, &scratch);
    fs::write(&calls, "").expect("the calls before the person pressed");
    harness.click_text("Stop").advance(STEP).render();
    settled(&mut harness);
    assert_eq!(acted_on(&calls, &container), [format!("unpause {container}"), format!("stop {container}")]);

    // The panel asks the engine again after each of them, and the engine still says the container
    // is there and frozen: the person presses the other button on the row they can see.
    let container = panel_with_a_frozen_container(&mut harness, &scratch);
    fs::write(&calls, "").expect("the calls before the person pressed again");
    harness.click_text("Restart").advance(STEP).render();
    settled(&mut harness);
    assert_eq!(
        acted_on(&calls, &container),
        [format!("unpause {container}"), format!("stop {container}"), format!("start {container}")]
    );
}

#[test]
fn the_containers_panel_shows_a_frozen_container_as_frozen() {
    let scratch = Scratch::new("freezing-panel");
    let (screen, _calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    look_again(&mut harness);
    after_quiet();
    look_after(&mut harness, &scratch, 0);
    let containers = vec![
        Container { name: container_of("claude-sub"), state: ContainerState::Paused },
        Container { name: container_of("codex-main"), state: ContainerState::Running },
    ];
    harness.send(Test::Screen(Msg::ContainersRead("firefly".to_owned(), Ok(containers)))).render();
    let panel = panel_of(&harness, harness.app().0.panel().width()).join(" ");
    assert!(panel.contains("claude-sub") && panel.contains("frozen"), "the frozen one reads as frozen:\n{panel}");
    assert!(panel.contains("codex-main") && panel.contains("running"), "and the other as the engine says:\n{panel}");
}

#[test]
fn a_message_for_one_tab_wakes_no_other_frozen_container() {
    // The message waits in the shown profile's tab, whose container is awake: the frozen one has
    // nothing waiting in it and stays as it is.
    let scratch = Scratch::new("freezing-other-letter");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    let tab = harness.app().0.workspace().expect("a workspace").tabs()[1].key();
    harness.send(Test::Letter(tab));
    fs::write(&calls, "").expect("the calls before the message was looked at");
    harness.send(Test::Deliver(Instant::now() + QUIET)).advance(STEP).render();
    settled(&mut harness);
    let about = asked_of(&calls, &container);
    assert!(!about.iter().any(|call| call.starts_with("unpause")), "the frozen container is left asleep:\n{about:#?}");
    assert!(harness.app().0.freezing.is_frozen(&container));
}

#[test]
fn a_wake_on_its_way_is_not_asked_for_twice() {
    // The engine refuses to unpause an awake container, and that refusal after the first wake's
    // success would have the screen call the container frozen again and hold its message for ever.
    let scratch = Scratch::new("freezing-twice");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    let tab = harness.app().0.workspace().expect("a workspace").tabs()[0].key();
    let session = harness.app().0.workspace().expect("a workspace").tabs()[0].session().cloned().expect("a program");
    let at = session.last_output() + QUIET;
    harness.send(Test::Letter(tab));
    fs::write(&calls, "").expect("the calls before the message was looked at");
    harness.send(Test::DeliverTwice(at, at + RETRY_EVERY));
    settled(&mut harness);
    let wakes = asked_of(&calls, &container).iter().filter(|call| call.starts_with("unpause")).count();
    assert_eq!(wakes, 1, "one wake is asked for:\n{:#?}", asked(&calls));
    assert!(!harness.app().0.freezing.is_frozen(&container), "and the container is awake");
}

#[test]
fn a_freeze_that_lands_after_the_person_came_back_to_the_tab_is_undone_at_once() {
    // The look judged the profile while nobody was looking at it, and the person clicked its tab
    // while the engine was pausing it: the tab on the screen must not stay asleep.
    let scratch = Scratch::new("freezing-late");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    let container = container_of("claude-sub");
    click_tab(&mut harness, "claude-sub");
    fs::write(&calls, "").expect("the calls before the freeze landed");
    harness.send(Test::Screen(Msg::Frozen(container.clone(), true)));
    settled(&mut harness);
    assert_eq!(asked_of(&calls, &container).first(), Some(&format!("unpause {container}")), "{:#?}", asked(&calls));
    assert!(!harness.app().0.freezing.is_frozen(&container));
}

#[test]
fn a_container_paused_whose_pages_the_kernel_kept_is_still_frozen_and_is_woken() {
    // The reclaim is refused here, but the engine did pause the container: its tabs answer nothing
    // until it is woken, and only a screen that calls it frozen wakes it.
    let scratch = Scratch::new("freezing-no-reclaim");
    let (screen, calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    let reclaim =
        scratch.0.join("cgroup").join(format!("libpod-{}.scope", container_of("claude-sub"))).join("memory.reclaim");
    fs::remove_file(&reclaim).expect("the reclaim file");
    fs::create_dir(&reclaim).expect("a reclaim the kernel refuses");
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    assert!(harness.app().0.freezing.is_frozen(&container), "paused is frozen");
    fs::write(&calls, "").expect("the calls before the tab was opened");
    click_tab(&mut harness, "claude-sub");
    settled(&mut harness);
    assert_eq!(asked_of(&calls, &container).first(), Some(&format!("unpause {container}")));
}

#[test]
fn a_container_the_engine_lists_as_awake_is_no_longer_called_frozen() {
    // Something the screen did not hear of woke it — the person's own `podman unpause`, or the
    // engine restarting — and a container still called frozen would never be looked at again.
    let scratch = Scratch::new("freezing-listed");
    let (screen, _calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    let listed = vec![
        Container { name: container.clone(), state: ContainerState::Running },
        Container { name: container_of("codex-main"), state: ContainerState::Running },
    ];
    harness.send(Test::Screen(Msg::ContainersRead("firefly".to_owned(), Ok(listed)))).render();
    assert!(!harness.app().0.freezing.is_frozen(&container));
}

#[test]
fn a_refused_wake_of_a_container_that_is_awake_counts_as_awake() {
    // Something else woke it first, and the engine refuses to wake an awake container. The screen
    // asks what it is: awake is awake, and a container still asleep stays called frozen.
    let scratch = Scratch::new("freezing-refused-wake");
    let (screen, _calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    frozen(&mut harness, &scratch);
    let container = container_of("claude-sub");
    fs::write(scratch.0.join("refuse-unpause"), "").expect("an engine that will not wake it");
    fs::write(scratch.0.join(format!("paused-{container}")), "").expect("a container still asleep");
    click_tab(&mut harness, "claude-sub");
    settled(&mut harness);
    assert!(harness.app().0.freezing.is_frozen(&container), "still asleep, still frozen");

    fs::remove_file(scratch.0.join(format!("paused-{container}"))).expect("woken by something else");
    click_tab(&mut harness, "codex-main");
    click_tab(&mut harness, "claude-sub");
    settled(&mut harness);
    assert!(!harness.app().0.freezing.is_frozen(&container), "awake is awake, whoever woke it");
}

#[test]
fn the_containers_panel_follows_a_freeze_and_a_wake_without_being_refreshed() {
    // Nobody presses Refresh: the panel says "frozen" once the look has frozen the container, and
    // what the engine says of it once the person's click on its tab has woken it.
    let scratch = Scratch::new("freezing-panel-follows");
    let (screen, _calls) = quiet_screen(&scratch);
    let mut harness = harness(screen, &scratch);
    harness.resize(SIZE.0, 80).render();
    frozen(&mut harness, &scratch);
    settled(&mut harness);
    let row = |harness: &Harness<Frozen>| {
        panel_of(harness, harness.app().0.panel().width())
            .into_iter()
            .find(|line| line.contains("claude-sub"))
            .unwrap_or_default()
    };
    assert!(row(&harness).contains("frozen"), "the panel says it is frozen:\n{}", harness.screen());
    click_tab(&mut harness, "claude-sub");
    settled(&mut harness);
    assert!(row(&harness).contains("running"), "and running once it is awake:\n{}", harness.screen());
}
