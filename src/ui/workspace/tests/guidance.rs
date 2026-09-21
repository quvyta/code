//! A QCode high profile's tab, opened the way the person opens it, leaves its instruction files in
//! the workspace: graphify's own installer run inside the profile's container, and QCode's section
//! written into the file the harness reads. A profile on another template writes nothing.
//!
//! The engine is a script that writes down every call it is given, so what reaches a container is
//! read back from its list, and what reaches the workspace folder from the disk.

use super::*;

use crate::profile::Extra;
use crate::profile::guidance::{BEGIN, SECTION, block};
use crate::ui::workspace::plan::{self, MAP};

/// A stand-in engine that writes down every call, answers the reading of a settings file with
/// "there is none", keeps what is written into one, refuses to run a window unless `window_runs`
/// exists, and makes graphify's installer fail with graphify's own kind of words while `failing`
/// exists.
struct Recording {
    scratch: Scratch,
    binary: PathBuf,
    calls: PathBuf,
    failing: PathBuf,
    window_runs: PathBuf,
}

impl Recording {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let (binary, calls, written, failing, window_runs) = (
            scratch.0.join("engine"),
            scratch.0.join("calls"),
            scratch.0.join("written"),
            scratch.0.join("graphify-fails"),
            scratch.0.join("window-runs"),
        );
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {calls}\n\
             [ \"$1\" = run ] && [ ! -e {window_runs} ] && exit 1\n\
             for word in \"$@\"; do\n\
             case \"$word\" in\n\
             *'exit 3'*) exit 3 ;;\n\
             *'qcode-new'*) cat > {written}; exit 0 ;;\n\
             *'exec graphify'*) [ -e {failing} ] && {{ echo 'graphify: cannot write .claude/settings.json' >&2; exit 1; }} ;;\n\
             esac\n\
             done\n\
             exit 0\n",
            calls = calls.display(),
            written = written.display(),
            failing = failing.display(),
            window_runs = window_runs.display(),
        );
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        Self { scratch, binary, calls, failing, window_runs }
    }

    /// A screen on this engine with one workspace carrying `profile`.
    fn screen(&self, profile: Profile) -> WorkspaceScreen {
        let workspaces = vec![workspace("firefly", "Firefly", self.scratch.paths(), vec![profile])];
        WorkspaceScreen::new(
            Some(Engine::new(EngineKind::Podman, &self.binary)),
            HostUser::Ids { uid: 1000, gid: 1000 },
            workspaces,
        )
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn work(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.scratch.paths().code.join(file)).ok()
    }
}

fn on(harness: HarnessKind, name: &str, template: Template) -> Profile {
    Profile { template, ..profile(name, harness) }
}

/// Opens a tab of the workspace's one terminal profile the way the person does: a new tab from
/// its button, then the profile's "New chat" row on the blank tab's page.
/// Motion is off, so a toast the start sends is on screen as soon as it is sent.
fn open_a_chat(harness: &mut Harness<Screen>) {
    harness.set_reduced_motion(true);
    harness.click_text("New tab").render();
    harness.click_text("New chat").advance(Duration::from_millis(400)).render();
}

/// The calls that ran graphify's installer, and in which container.
fn graphify_calls(calls: &[String]) -> Vec<&String> {
    calls.iter().filter(|call| call.starts_with("exec ") && call.contains("exec graphify")).collect()
}

/// The calls that started graphify building its map, and in which container.
fn map_calls(calls: &[String]) -> Vec<&String> {
    calls.iter().filter(|call| call.starts_with("exec ") && call.contains("graphify update .")).collect()
}

#[test]
fn opening_a_qcode_high_claude_code_tab_runs_graphify_in_its_container_and_writes_qcodes_section() {
    let engine = Recording::new("guided-claude");
    let workspace_file = engine.scratch.paths().code.join("CLAUDE.md");
    fs::write(&workspace_file, "# Firefly\n\nThe person's own rules.\n").expect("the person's file");
    let mut harness =
        harness(engine.screen(on(HarnessKind::ClaudeCode, "claude-high", Template::High)), SIZE.0, SIZE.1);
    open_a_chat(&mut harness);

    let kinds = kinds(&harness.app().0);
    assert_eq!(kinds, vec![TabKind::Profile("claude-high".to_owned())], "the tab opened");

    let calls = engine.calls();
    let graphify = graphify_calls(&calls);
    assert_eq!(graphify.len(), 1, "graphify's installer ran once: {calls:?}");
    assert!(graphify[0].starts_with("exec qcode-firefly-claude-high "), "inside the profile's container: {graphify:?}");
    assert!(graphify[0].ends_with(&format!("{CODE_DIR} claude")), "in the workspace, for Claude Code: {graphify:?}");
    // After the container is up, and before the harness is started in it.
    let started = calls.iter().position(|call| call.starts_with("start ")).expect("the container is started");
    let installed = calls.iter().position(|call| call.contains("exec graphify")).expect("graphify ran");
    assert!(started < installed, "{calls:?}");

    let written = engine.work("CLAUDE.md").expect("the file is there");
    assert!(written.starts_with("# Firefly\n\nThe person's own rules.\n"), "the person's text is kept: {written}");
    assert!(written.ends_with(&format!("\n{}\n", block())), "{written}");
    assert!(!harness.screen().contains("could not be brought up to date"), "{}", harness.screen());
}

#[test]
fn a_qcode_high_tab_in_a_workspace_without_a_map_starts_graphify_building_it_and_does_not_wait() {
    let engine = Recording::new("map-absent");
    let mut harness =
        harness(engine.screen(on(HarnessKind::ClaudeCode, "claude-high", Template::High)), SIZE.0, SIZE.1);
    open_a_chat(&mut harness);
    let calls = engine.calls();
    let map = map_calls(&calls);
    assert_eq!(map.len(), 1, "the map is started once: {calls:?}");
    assert!(map[0].starts_with("exec qcode-firefly-claude-high "), "inside the profile's container: {map:?}");
    assert!(map[0].ends_with(&format!(" sh {CODE_DIR}")), "in the workspace: {map:?}");
    // Left running on its own, with nothing tied to the call that started it, so the call returns
    // at once and the harness is not held up.
    assert!(map[0].contains("nohup flock -n /tmp/qcode-map graphify update . </dev/null >/dev/null 2>&1 &"), "{map:?}");
    // After graphify's installer, whose section and hooks are what send the agent to the map.
    let installed = calls.iter().position(|call| call.contains("exec graphify")).expect("graphify ran");
    let started = calls.iter().position(|call| call.contains("graphify update .")).expect("the map was started");
    assert!(installed < started, "{calls:?}");
}

#[test]
fn a_qcode_high_tab_in_a_workspace_with_a_map_leaves_it_to_graphifys_own_hooks() {
    let engine = Recording::new("map-present");
    let map = engine.scratch.paths().code.join(MAP);
    fs::create_dir_all(map.parent().expect("graphify's folder")).expect("graphify's folder");
    fs::write(&map, "{}").expect("a map is there");
    let mut harness =
        harness(engine.screen(on(HarnessKind::ClaudeCode, "claude-high", Template::High)), SIZE.0, SIZE.1);
    open_a_chat(&mut harness);
    let calls = engine.calls();
    assert_eq!(graphify_calls(&calls).len(), 1, "graphify's installer still runs: {calls:?}");
    assert!(map_calls(&calls).is_empty(), "the map is not built again: {calls:?}");
    assert_eq!(fs::read_to_string(&map).expect("the map"), "{}", "nor touched");
}

#[test]
fn a_qcode_high_profile_that_went_without_graphify_neither_installs_it_nor_builds_its_map() {
    let engine = Recording::new("without-graphify");
    let profile =
        Profile { without: vec![Extra::Graphify], ..on(HarnessKind::ClaudeCode, "claude-high", Template::High) };
    let mut harness = harness(engine.screen(profile), SIZE.0, SIZE.1);
    open_a_chat(&mut harness);
    let calls = engine.calls();
    assert!(calls.iter().any(|call| call.starts_with("start ")), "the container came up: {calls:?}");
    assert!(graphify_calls(&calls).is_empty(), "{calls:?}");
    assert!(map_calls(&calls).is_empty(), "{calls:?}");
    // QCode's own section is about the other tabs, not graphify, so it is written all the same.
    let written = engine.work("CLAUDE.md").expect("QCode's section");
    assert!(written.contains(SECTION), "{written}");
    assert!(!harness.screen().contains("could not be brought up to date"), "{}", harness.screen());
}

#[test]
fn opening_a_qcode_high_opencode_tab_writes_agents_md() {
    let engine = Recording::new("guided-opencode");
    let mut harness = harness(engine.screen(on(HarnessKind::OpenCode, "open-high", Template::High)), SIZE.0, SIZE.1);
    open_a_chat(&mut harness);
    let calls = engine.calls();
    let graphify = graphify_calls(&calls);
    assert_eq!(graphify.len(), 1, "{calls:?}");
    assert!(graphify[0].ends_with(&format!("{CODE_DIR} opencode")), "{graphify:?}");
    let written = engine.work("AGENTS.md").expect("AGENTS.md is made");
    assert_eq!(written, format!("{}\n", block()));
    assert_eq!(engine.work("CLAUDE.md"), None, "only the file opencode reads");
}

#[test]
fn a_profile_on_qcode_basic_leaves_the_workspaces_files_alone() {
    for template in [Template::Recommended, Template::Base] {
        let engine = Recording::new("unguided-basic");
        let mut harness = harness(engine.screen(on(HarnessKind::ClaudeCode, "claude-basic", template)), SIZE.0, SIZE.1);
        open_a_chat(&mut harness);
        let calls = engine.calls();
        assert!(calls.iter().any(|call| call.starts_with("start ")), "the container came up: {calls:?}");
        assert!(graphify_calls(&calls).is_empty(), "{template:?}: {calls:?}");
        assert!(map_calls(&calls).is_empty(), "{template:?}: {calls:?}");
        assert_eq!(engine.work("CLAUDE.md"), None, "{template:?}");
        assert_eq!(engine.work("AGENTS.md"), None, "{template:?}");
    }
}

#[test]
fn graphify_failing_is_said_on_screen_and_the_tab_still_starts_with_qcodes_section_written() {
    let engine = Recording::new("guided-fails");
    fs::write(&engine.failing, "").expect("graphify is told to fail");
    let mut harness =
        harness(engine.screen(on(HarnessKind::ClaudeCode, "claude-high", Template::High)), SIZE.0, SIZE.1);
    open_a_chat(&mut harness);
    let shown = harness.screen();
    // The toast wraps its words, so they are looked for in parts.
    assert!(shown.contains("The instructions of Claude Code"), "{shown}");
    assert!(shown.contains("graphify could not set itself up"), "{shown}");
    assert!(shown.contains("graphify: cannot write"), "graphify's own words: {shown}");
    let tab = &harness.app().0.workspace().expect("a workspace").tabs()[0];
    assert!(!matches!(tab.state(), TabState::Failed(_)), "the tab starts all the same: {:?}", tab.state());
    let written = engine.work("CLAUDE.md").expect("QCode's part is written all the same");
    assert!(written.contains(SECTION), "{written}");
}

#[test]
fn a_file_whose_section_has_lost_a_marker_line_is_left_alone_and_the_person_is_told() {
    let engine = Recording::new("guided-broken");
    let text = format!("# Firefly\n\n{BEGIN}\nhalf a section\n\nThe person's text after it.\n");
    fs::write(engine.scratch.paths().code.join("CLAUDE.md"), &text).expect("the person's file");
    let mut harness =
        harness(engine.screen(on(HarnessKind::ClaudeCode, "claude-high", Template::High)), SIZE.0, SIZE.1);
    open_a_chat(&mut harness);
    assert_eq!(engine.work("CLAUDE.md").as_deref(), Some(text.as_str()), "not a byte changed");
    let shown = harness.screen();
    assert!(shown.contains("The instructions of Claude Code"), "{shown}");
    // The toast wraps its words, so they are looked for in parts.
    assert!(shown.contains("CLAUDE.md has only one of the two lines"), "{shown}");
}

#[test]
fn a_qcode_high_window_is_prepared_from_a_container_that_sees_the_workspace_before_it_opens() {
    let engine = Recording::new("guided-window");
    let mut screen = engine.screen(on(HarnessKind::AntigravityIde, "anti", Template::High));
    let display = qframe_display(&engine);
    screen = screen.showing_on(Ok(display));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.click_text("New tab").render();
    harness.click_text("Open its window").render();

    let calls = engine.calls();
    let scribe = calls
        .iter()
        .find(|call| call.starts_with("create --name qcode-firefly-anti.mcp"))
        .unwrap_or_else(|| panic!("a container of its own: {calls:?}"));
    let work = engine.scratch.paths().code;
    assert!(scribe.contains(&format!("{}:{CODE_DIR}", work.display())), "it sees the workspace: {scribe}");
    let graphify = graphify_calls(&calls);
    assert_eq!(graphify.len(), 1, "{calls:?}");
    assert!(graphify[0].starts_with("exec qcode-firefly-anti.mcp "), "{graphify:?}");
    assert!(graphify[0].ends_with(&format!("{CODE_DIR} antigravity")), "{graphify:?}");
    let installed = calls.iter().position(|call| call.contains("exec graphify")).expect("graphify ran");
    let run = calls.iter().position(|call| call.starts_with("run ")).expect("the window is run");
    assert!(installed < run, "ready before the window opens: {calls:?}");
    let rule = engine.work(".agents/rules/qcode.md").expect("Antigravity's rule of QCode's own");
    assert!(rule.starts_with("---\ntrigger: always_on\n"), "{rule}");
    assert!(rule.contains(SECTION), "{rule}");
}

#[test]
fn a_qcode_high_window_starts_the_map_in_its_own_container_once_it_is_open() {
    // The window is opened by the call the tab's button runs, not through the screen: a window
    // that opens starts watches that last as long as it does, and a screen test runs them on the
    // spot.
    let engine = Recording::new("map-window");
    fs::write(&engine.window_runs, "").expect("the window is let run");
    let screen = engine.screen(on(HarnessKind::AntigravityIde, "anti", Template::High));
    let open = screen.workspace().expect("a workspace");
    let plan = open.plan(&TabKind::Desktop("anti".to_owned())).expect("the window is planned");
    let display = qframe_display(&engine);
    let running = Engine::new(EngineKind::Podman, &engine.binary);
    plan::open_window(&running, &plan, HostUser::Ids { uid: 1000, gid: 1000 }, &display).expect("the window opens");
    let calls = engine.calls();
    let map = map_calls(&calls);
    assert_eq!(map.len(), 1, "{calls:?}");
    // Not the container that prepared the window: that one is taken away at once, and the map
    // with it.
    assert!(map[0].starts_with("exec qcode-firefly-anti.desk "), "{map:?}");
    let run = calls.iter().position(|call| call.starts_with("run ")).expect("the window is run");
    let started = calls.iter().position(|call| call.contains("graphify update .")).expect("the map was started");
    assert!(run < started, "{calls:?}");
}

#[test]
fn a_basic_window_is_prepared_without_the_workspace_and_writes_nothing_there() {
    let engine = Recording::new("unguided-window");
    let screen = engine.screen(on(HarnessKind::AntigravityIde, "anti", Template::Recommended));
    let display = qframe_display(&engine);
    let mut harness = harness(screen.showing_on(Ok(display)), SIZE.0, SIZE.1);
    harness.click_text("New tab").render();
    harness.click_text("Open its window").render();
    let calls = engine.calls();
    let scribe =
        calls.iter().find(|call| call.starts_with("create --name qcode-firefly-anti.mcp")).expect("the scribe");
    assert!(!scribe.contains(&format!(":{CODE_DIR}")), "{scribe}");
    assert!(graphify_calls(&calls).is_empty(), "{calls:?}");
    assert!(map_calls(&calls).is_empty(), "{calls:?}");
    assert_eq!(engine.work(".agents/rules/qcode.md"), None);
}

/// A stand-in compositor socket in the engine's scratch folder, and the display naming it.
fn qframe_display(engine: &Recording) -> crate::desktop::Display {
    let runtime = engine.scratch.0.join("runtime");
    fs::create_dir_all(&runtime).expect("a runtime folder");
    let socket = runtime.join("wayland-1");
    fs::write(&socket, "").expect("a stand-in compositor socket");
    crate::desktop::Display { socket, name: "wayland-1".to_owned(), device: None }
}
