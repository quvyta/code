//! The workspaces screen, driven through the framework's harness: full screens, a narrow terminal,
//! ASCII, reduced motion, the keyboard and the mouse.
//!
//! Nothing here needs a container runtime. The one test that does is marked `#[ignore]` and runs
//! only with `QCODE_CONTAINER_TESTS=1`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use qframe::env::{AssetDirs, Env};
use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::runtime::Harness;

use super::{Msg, Overlay, Workspaces, update, view};
use crate::engine::{Engine, EngineKind, detect};
use crate::store::Store;

/// A terminal big enough for the list and the dialog.
const SIZE: (u16, u16) = (96, 34);

/// A folder of this test's own, removed when it goes out of scope.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-workspaces-{name}-{stamp}"));
        fs::create_dir_all(&path).expect("a folder in the temporary folder");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The screen on its own, which is all a screen needs to be driven.
struct Screen {
    state: Workspaces,
    /// Every workspace the screen handed on to be opened, in order.
    opened: Vec<String>,
    /// Every workspace the screen said it deleted, in order.
    deleted: Vec<String>,
}

impl App for Screen {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        let (command, opened) = update(&mut self.state, message);
        self.opened.extend(opened.map(|id| id.as_str().to_owned()));
        self.deleted.extend(self.state.just_deleted().map(|id| id.as_str().to_owned()));
        command
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        AppShell::new().body(|ui| view(&self.state, ui)).show(ui);
    }
}

fn env() -> Env {
    Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() }).expect("the built-in files load")
}

/// A harness over the store at `root`, with the first listing already answered.
fn screen(root: &Path, engine: Option<Engine>) -> Harness<Screen> {
    screen_over(Workspaces::new(Store::new(root), engine, Vec::new()))
}

/// A harness over `state`, with the first listing already answered.
fn screen_over(state: Workspaces) -> Harness<Screen> {
    let mut harness =
        Harness::with_env(Screen { state, opened: Vec::new(), deleted: Vec::new() }, env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.send(Msg::Refresh).render();
    harness
}

/// An engine that is certainly not installed, for the states that only ask whether one is there.
fn absent_engine() -> Engine {
    Engine::new(EngineKind::Podman, "/qcode/no/such/engine")
}

/// Types `text` into the focused field, one message at a time, the way a field reports typing.
fn typed(harness: &mut Harness<Screen>, text: &str) {
    harness.send(Msg::Name(text.to_owned())).render();
}

#[test]
fn an_empty_store_says_what_a_workspace_is_and_offers_one() {
    let scratch = Scratch::new("empty");
    let harness = screen(scratch.path(), None);
    let text = harness.screen();
    assert!(text.contains("No workspaces yet"), "{text}");
    assert!(text.contains("QCode keeps your code"), "{text}");
    assert!(text.contains("New workspace"), "{text}");
}

#[test]
fn the_first_reading_stands_in_for_the_list_before_it_answers() {
    let scratch = Scratch::new("skeleton");
    let state = Workspaces::new(Store::new(scratch.path()), None, Vec::new());
    let harness = Harness::with_env(Screen { state, opened: Vec::new(), deleted: Vec::new() }, env(), SIZE.0, SIZE.1);
    // Nothing has been read yet, so neither the list nor the empty state may claim the screen.
    let text = harness.screen();
    assert!(!text.contains("No workspaces yet"), "{text}");
}

#[test]
fn workspaces_are_listed_with_the_day_they_were_made() {
    let scratch = Scratch::new("list");
    let store = Store::new(scratch.path());
    store.create_workspace("Firefly", qframe::date::Date::new(2026, 9, 17).expect("a real day")).expect("made");
    let harness = screen(scratch.path(), None);
    let text = harness.screen();
    assert!(text.contains("Firefly"), "{text}");
    assert!(text.contains("2026-09-17"), "{text}");
}

#[test]
fn the_workspace_opened_last_says_so_instead_of_its_day() {
    let scratch = Scratch::new("recent");
    let store = Store::new(scratch.path());
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("made");
    let state = Workspaces::new(Store::new(scratch.path()), None, vec!["firefly".to_owned()]);
    let mut harness =
        Harness::with_env(Screen { state, opened: Vec::new(), deleted: Vec::new() }, env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).send(Msg::Refresh).render();
    let text = harness.screen();
    assert!(text.contains("Opened last"), "{text}");
}

#[test]
fn a_broken_workspace_is_listed_and_says_why_when_it_is_opened() {
    let scratch = Scratch::new("broken");
    let store = Store::new(scratch.path());
    let file = store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("made");
    // A workspace whose own folder is gone is broken, and the reason names the folder.
    fs::remove_dir_all(store.workspace_paths(&file.id).code).expect("the folder goes");

    let mut harness = screen(scratch.path(), None);
    let text = harness.screen();
    assert!(text.contains("Firefly"), "{text}");
    assert!(text.contains("Broken"), "{text}");
    assert!(text.contains("1 problem"), "{text}");

    harness.click_text("Firefly");
    let text = harness.screen();
    assert!(text.contains("What is wrong with Firefly"), "{text}");
    assert!(text.contains("this folder of the workspace is missing"), "{text}");
}

#[test]
fn a_name_that_yields_no_folder_name_is_explained_where_it_was_typed() {
    let scratch = Scratch::new("name");
    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "...");
    let text = harness.screen();
    assert!(text.contains("leaves nothing a folder can be called"), "{text}");
}

#[test]
fn a_name_shows_the_folder_name_it_would_get() {
    let scratch = Scratch::new("preview");
    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Ilık Çay");
    let text = harness.screen();
    assert!(text.contains("On disk: ilik-cay"), "{text}");
}

#[test]
fn a_name_typed_faster_than_a_frame_arrives_whole() {
    // A terminal hands over every key waiting since the last frame in one read: a quick typist,
    // a multiplexer or a slow connection does it. Each key must meet what the one before it did,
    // or "demo" becomes "o" and the workspace is made under a name nobody typed.
    let scratch = Scratch::new("burst");
    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    let keys: Vec<Event> = "demo".chars().map(|c| Event::Key(KeyEvent::press(&c.to_string()))).collect();
    harness.events(&keys);
    let text = harness.screen();
    assert!(text.contains("On disk: demo"), "{text}");
    harness.events(&[Event::Key(KeyEvent::press("enter"))]).render();
    assert!(Store::new(scratch.path()).workspaces_dir().join("demo").is_dir(), "{}", harness.screen());
}

#[test]
fn a_name_that_is_taken_is_said_out_loud_and_never_numbered() {
    let scratch = Scratch::new("taken");
    let store = Store::new(scratch.path());
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("made");

    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness.send(Msg::Submit).render();
    let text = harness.screen();
    assert!(text.contains("already a workspace called `firefly`"), "{text}");
    // Nothing was made behind the person's back.
    let names: Vec<String> = fs::read_dir(store.workspaces_dir())
        .expect("the workspaces folder is there")
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect();
    assert_eq!(names, ["firefly"], "{names:?}");
}

#[test]
fn an_empty_workspace_is_made_at_once_and_appears_on_the_list() {
    let scratch = Scratch::new("create");
    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness.send(Msg::Submit).render().render();
    let text = harness.screen();
    assert!(text.contains("Firefly"), "{text}");
    assert!(!text.contains("Start from"), "the dialog is gone:\n{text}");
    assert!(scratch.path().join("Workspaces").join("firefly").join("workspace.qcode").is_file());
}

#[test]
fn a_folder_is_copied_and_the_persons_own_copy_stays_where_it_is() {
    let scratch = Scratch::new("copy");
    let source = scratch.path().join("source");
    fs::create_dir_all(source.join("src")).expect("a source tree");
    fs::write(source.join("src").join("main.rs"), "fn main() {}").expect("a file");
    let store = scratch.path().join("store");

    let mut harness = screen(&store, None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness
        .send(Msg::Source(super::Source::Folder.index()))
        .send(Msg::Picker(qframe::widgets::FilePickerMsg::Chosen(source.clone())))
        .send(Msg::Submit)
        .advance(Duration::from_millis(50))
        .render();

    let copied = store.join("Workspaces").join("firefly").join("Work").join("src").join("main.rs");
    assert!(copied.is_file(), "the copy landed in the workspace");
    assert!(source.join("src").join("main.rs").is_file(), "their own folder is untouched");
}

#[test]
fn choose_folder_takes_the_folder_that_is_open_and_not_the_first_one_inside_it() {
    // The browser rests its cursor on the first entry of a folder it opens. That resting place is
    // not a choice: Choose folder takes the folder on screen, and the copy holds all of it.
    let scratch = Scratch::new("pick");
    let source = scratch.path().join("source");
    fs::create_dir_all(source.join("src")).expect("a source tree");
    fs::write(source.join("src").join("main.rs"), "fn main() {}").expect("a file");
    let store = scratch.path().join("store");

    let mut harness = screen(&store, None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness
        .send(Msg::Source(super::Source::Folder.index()))
        .send(Msg::Picker(qframe::widgets::FilePickerMsg::Open(source.clone())))
        .advance(Duration::from_millis(50));
    harness.click_text("Choose folder");
    harness.send(Msg::Submit).advance(Duration::from_millis(50)).render();

    let work = store.join("Workspaces").join("firefly").join("Work");
    assert!(work.join("src").join("main.rs").is_file(), "the open folder was copied whole:\n{}", harness.screen());
}

#[test]
fn a_folder_that_holds_the_store_is_refused_before_anything_is_made() {
    let scratch = Scratch::new("inside");
    let store = scratch.path().join("store");
    let mut harness = screen(&store, None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness
        .send(Msg::Source(super::Source::Folder.index()))
        .send(Msg::Picker(qframe::widgets::FilePickerMsg::Chosen(scratch.path().to_path_buf())))
        .send(Msg::Submit)
        .render();
    let text = harness.screen();
    assert!(text.contains("so copying it would"), "{text}");
    assert!(!store.join("Workspaces").join("firefly").exists(), "nothing was made");
}

#[test]
fn a_copy_that_cannot_finish_leaves_no_half_made_workspace() {
    let scratch = Scratch::new("rollback");
    let store = scratch.path().join("store");
    let missing = scratch.path().join("gone");

    let mut harness = screen(&store, None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness
        .send(Msg::Source(super::Source::Folder.index()))
        .send(Msg::Picker(qframe::widgets::FilePickerMsg::Chosen(missing)))
        .send(Msg::Submit)
        .advance(Duration::from_millis(50))
        .render();

    assert!(!store.join("Workspaces").join("firefly").exists(), "the workspace folder went with the failure");
    let text = harness.screen();
    assert!(text.contains("could not be filled"), "{text}");
}

#[test]
fn cloning_without_an_engine_is_passive_and_says_why() {
    let scratch = Scratch::new("no-engine");
    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness.send(Msg::Source(super::Source::Git.index())).render();
    let text = harness.screen();
    assert!(text.contains("No container engine was"), "{text}");
    harness.send(Msg::Submit).render();
    assert!(!scratch.path().join("Workspaces").join("firefly").exists(), "nothing was made");
}

#[test]
fn a_git_address_that_looks_like_an_option_is_refused() {
    let scratch = Scratch::new("flag");
    let mut harness = screen(scratch.path(), Some(absent_engine()));
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness
        .send(Msg::Source(super::Source::Git.index()))
        .send(Msg::Url("--upload-pack=whatever".to_owned()))
        .send(Msg::Submit)
        .render();
    let text = harness.screen();
    assert!(text.contains("cannot start with a dash"), "{text}");
    assert!(!scratch.path().join("Workspaces").join("firefly").exists(), "nothing was made");
}

#[test]
fn a_clone_that_cannot_reach_an_engine_leaves_no_half_made_workspace() {
    let scratch = Scratch::new("clone-fails");
    let mut harness = screen(scratch.path(), Some(absent_engine()));
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness
        .send(Msg::Source(super::Source::Git.index()))
        .send(Msg::Url("https://example.org/team/app.git".to_owned()))
        .send(Msg::Submit)
        .advance(Duration::from_millis(50))
        .render();
    assert!(!scratch.path().join("Workspaces").join("firefly").exists(), "the workspace folder went with the failure");
    let text = harness.screen();
    assert!(text.contains("could not be filled"), "{text}");
}

#[test]
fn a_store_that_cannot_be_read_opens_the_screen_and_says_so() {
    let scratch = Scratch::new("blocked");
    // A file where the store folder should be: it can be neither made nor read.
    let root = scratch.path().join("store");
    fs::write(&root, "not a folder").expect("a file");
    let harness = screen(&root, None);
    let text = harness.screen();
    assert!(text.contains("The QCode folder cannot be read"), "{text}");
    assert!(text.contains("Refresh"), "{text}");
}

#[test]
fn nothing_is_bracketed_lined_or_framed_in_any_glyph_mode() {
    let scratch = Scratch::new("aesthetic");
    let store = Store::new(scratch.path());
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("made");
    let mut harness = screen(scratch.path(), None);
    // The list, the dialog on its plainest source and the dialog on the one with a field that
    // can be passive, each in all three glyph modes.
    for step in 0..3 {
        match step {
            1 => harness.send(Msg::Start),
            2 => harness.send(Msg::Source(super::Source::Git.index())),
            _ => &mut harness,
        };
        for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
            harness.set_glyph_mode(mode).render();
            let text = harness.screen();
            for forbidden in ['[', ']', '(', ')', '{', '}', '|', '┌', '─', '│'] {
                assert!(!text.contains(forbidden), "`{forbidden}` in {mode:?}:\n{text}");
            }
            assert!(!text.contains("=="), "{text}");
            assert!(!text.contains("->"), "{text}");
        }
    }
}

#[test]
fn a_hovered_row_and_a_keyboard_focused_list_both_answer() {
    let scratch = Scratch::new("hover");
    let store = Store::new(scratch.path());
    for name in ["Alpha", "Beta"] {
        store.create_workspace(name, qframe::date::Date::today_utc()).expect("made");
    }
    let mut harness = screen(scratch.path(), None);
    let (x, y) = harness.find("Beta").expect("the second row is on screen");
    let resting = harness.bg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0));
    harness.hover(x, y).render();
    let hovered = harness.bg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0));
    assert_ne!(resting, hovered, "a hovered row lifts:\n{}", harness.screen());

    while !harness.is_focused(super::LIST) {
        harness.press("tab");
    }
    assert!(harness.is_focused(super::LIST), "the keyboard reaches the list:\n{}", harness.screen());
}

#[test]
fn ascii_mode_draws_nothing_but_ascii() {
    let scratch = Scratch::new("ascii");
    let store = Store::new(scratch.path());
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("made");
    let mut harness = screen(scratch.path(), None);
    harness.set_glyph_mode(GlyphMode::Ascii).render();
    let text = harness.screen();
    assert!(text.is_ascii(), "{text}");
    assert!(text.contains("Firefly"), "{text}");
}

#[test]
fn a_narrow_terminal_keeps_the_list_and_the_dialog_readable() {
    let scratch = Scratch::new("narrow");
    let store = Store::new(scratch.path());
    store.create_workspace("Firefly", qframe::date::Date::today_utc()).expect("made");
    let mut harness = screen(scratch.path(), None);
    harness.resize(40, 20).render();
    assert!(harness.screen().contains("Firefly"), "{}", harness.screen());
    harness.send(Msg::Start).render();
    let text = harness.screen();
    assert!(text.contains("New workspace"), "{text}");
    assert!(text.contains("Name"), "{text}");
}

#[test]
fn the_keyboard_moves_the_selection_and_opens_a_workspace() {
    let scratch = Scratch::new("keyboard");
    let store = Store::new(scratch.path());
    for name in ["Alpha", "Beta"] {
        store.create_workspace(name, qframe::date::Date::today_utc()).expect("made");
    }
    let mut harness = screen(scratch.path(), None);
    harness.press("tab");
    while !harness.is_focused(super::LIST) {
        harness.press("tab");
    }
    harness.press("down").press("enter").advance(Duration::from_millis(10));
    assert_eq!(harness.app().opened, ["beta"], "the second row is the workspace that is handed on");
}

#[test]
fn the_dialog_closes_with_escape_while_nothing_is_running() {
    let scratch = Scratch::new("escape");
    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    assert!(harness.screen().contains("Start from"), "{}", harness.screen());
    harness.press("esc").render();
    assert!(!harness.screen().contains("Start from"), "{}", harness.screen());
}

#[test]
fn turkish_reads_as_turkish() {
    let scratch = Scratch::new("turkish");
    let mut harness = screen(scratch.path(), None);
    harness.set_locale("tr").render();
    let text = harness.screen();
    assert!(text.contains("Henüz çalışma alanı yok"), "{text}");
    harness.send(Msg::Start).render();
    let text = harness.screen();
    for word in ["Yeni çalışma alanı", "Nereden", "Boş", "Bir git adresinden", "Oluştur"] {
        assert!(text.contains(word), "`{word}` is missing:\n{text}");
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_git_address_is_cloned_inside_a_container() {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return;
    }
    let engine = [EngineKind::Podman, EngineKind::Docker]
        .into_iter()
        .find_map(|kind| detect(kind).ok())
        .expect("these tests were asked for and no engine answered");

    let scratch = Scratch::new("clone");
    let origin = scratch.path().join("origin");
    fs::create_dir_all(&origin).expect("a folder to clone from");
    for command in [vec!["init", "--bare"], vec!["--version"]] {
        let _ = std::process::Command::new("git").args(&command).current_dir(&origin).status();
    }

    let store = scratch.path().join("store");
    let mut harness = screen(&store, Some(engine));
    harness.send(Msg::Start).render();
    typed(&mut harness, "Firefly");
    harness
        .send(Msg::Source(super::Source::Git.index()))
        .send(Msg::Url(format!("file://{}", origin.display())))
        .send(Msg::Submit)
        .advance(Duration::from_secs(30))
        .render();
    // Either it cloned, or it failed and took the workspace with it. What it may never do is
    // leave a workspace folder that was never filled.
    let workspace = store.join("Workspaces").join("firefly");
    assert!(!workspace.exists() || workspace.join("Work").join(".git").exists(), "{}", harness.screen());
}

#[test]
fn a_new_workspace_starts_empty_and_says_so_with_the_mark_of_a_choice() {
    let scratch = Scratch::new("source-default");
    let mut harness = screen(scratch.path(), None);
    harness.send(Msg::Start).render();
    let Some(Overlay::New(draft)) = &harness.app().state.overlay else { panic!("the dialog is open") };
    assert_eq!(draft.source, super::Source::Empty, "the plain empty workspace is chosen");
    // Only the chosen option is lit: its label is bold and the others are not.
    assert!(bold(&harness, "Empty"), "{}", harness.screen());
    assert!(!bold(&harness, "A folder"), "{}", harness.screen());
    assert!(!bold(&harness, "A git address"), "{}", harness.screen());
    // A pointer resting on another option does not make it look chosen.
    let (x, y) = harness.find("A git address").expect("shown");
    harness.hover(x + 2, y).render();
    assert!(!bold(&harness, "A git address"), "{}", harness.screen());
    assert!(bold(&harness, "Empty"), "{}", harness.screen());
}

/// Whether the first cell of `word` on the screen is bold.
fn bold(harness: &Harness<Screen>, word: &str) -> bool {
    let (x, y) = harness.find(word).unwrap_or_else(|| panic!("`{word}` is shown:\n{}", harness.screen()));
    harness.is_bold(u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"))
}

/// A stand-in engine that lists `containers` (name, a tab, the state; a line each) and `volumes`
/// (a name a line), answers every other command with success and writes down every command it is
/// given, so a test reads exactly what the engine was asked to do.
#[cfg(unix)]
fn stand_in(scratch: &Scratch, containers: &str, volumes: &str) -> (Engine, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch.path().join("engine");
    fs::create_dir_all(&dir).expect("a folder for the stand-in");
    fs::write(dir.join("ps"), containers).expect("the containers");
    fs::write(dir.join("volumes"), volumes).expect("the volumes");
    let asked = dir.join("asked");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{asked}'\ncase \"$1 $2\" in\n  ps\\ *) cat '{ps}' ;;\n  'volume ls') cat '{volumes}' ;;\nesac\nexit 0\n",
        asked = asked.display(),
        ps = dir.join("ps").display(),
        volumes = dir.join("volumes").display(),
    );
    let bin = dir.join("podman");
    fs::write(&bin, script).expect("the stand-in");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("runnable");
    (Engine::new(EngineKind::Podman, bin), asked)
}

/// What the stand-in engine was asked to do, a command a line.
fn asked(path: &Path) -> Vec<String> {
    fs::read_to_string(path).unwrap_or_default().lines().map(str::to_owned).collect()
}

/// The removals among what the stand-in engine was asked to do.
fn removals(path: &Path) -> Vec<String> {
    asked(path).into_iter().filter(|line| line.starts_with("rm ") || line.starts_with("volume rm ")).collect()
}

/// Lets background work answer until `done` holds, for a generous but finite while.
fn settle(harness: &mut Harness<Screen>, done: impl Fn(&Harness<Screen>) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !done(harness) && std::time::Instant::now() < deadline {
        harness.advance(Duration::from_millis(20)).render();
    }
}

/// Clicks `text` on the first line below the line that holds `anchor`: a row of a dialog whose
/// words are also on the screen behind it.
fn click_below(harness: &mut Harness<Screen>, anchor: &str, text: &str) {
    let screen = harness.screen();
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.iter().position(|line| line.contains(anchor)).unwrap_or_else(|| panic!("`{anchor}`:\n{screen}"));
    let (y, x) = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(y, line)| line.find(text).map(|at| (y, line[..at].chars().count())))
        .unwrap_or_else(|| panic!("`{text}` under `{anchor}`:\n{screen}"));
    harness.click(i32::try_from(x).expect("on screen"), i32::try_from(y).expect("on screen"));
}

/// A store with Firefly, which carries the profile claude, and Serenity beside it.
fn two_workspaces(scratch: &Scratch) -> Store {
    let store = Store::new(scratch.path());
    let today = qframe::date::Date::today_utc();
    let firefly = store.create_workspace("Firefly", today).expect("made");
    store.create_workspace("Serenity", today).expect("made");
    crate::store::add_profile(&store.workspace_paths(&firefly.id), "claude", today).expect("carried");
    fs::write(store.workspace_paths(&firefly.id).code.join("main.rs"), "fn main() {}").expect("some code");
    store
}

const CONTAINERS: &str =
    "qcode-firefly-base\texited\nqcode-firefly-claude\texited\nqcode-serenity-base\texited\nsomeone-else\trunning\n";
const VOLUMES: &str = "qcode-home-firefly-claude\nqcode-home-serenity-claude\nqcode-cred-claude\nsomeone-elses\n";

#[cfg(unix)]
#[test]
fn deleting_a_workspace_asks_once_naming_what_goes_and_takes_exactly_that() {
    let scratch = Scratch::new("delete");
    let store = two_workspaces(&scratch);
    let (engine, log) = stand_in(&scratch, CONTAINERS, VOLUMES);
    let mut harness = screen(scratch.path(), Some(engine));

    harness.click_text("Delete a workspace").render();
    let text = harness.screen();
    assert!(text.contains("Choose the workspace to delete"), "{text}");
    click_below(&mut harness, "Choose the workspace to delete", "Firefly");
    settle(&mut harness, |harness| harness.screen().contains("Delete Firefly?"));
    let text = harness.screen();
    assert!(text.contains("Delete Firefly?"), "{text}");
    for named in ["qcode-firefly-base", "qcode-firefly-claude", "qcode-home-firefly-claude"] {
        assert!(text.contains(named), "`{named}` is named in the question:\n{text}");
    }
    for other in ["qcode-serenity-base", "qcode-home-serenity-claude", "qcode-cred-claude", "someone-else"] {
        assert!(!text.contains(other), "`{other}` is not Firefly's:\n{text}");
    }
    assert_eq!(removals(&log), [] as [String; 0], "nothing is removed before the answer");

    // The question opens on Cancel; the answer is the button beside it.
    harness.press("tab").press("enter").render();
    settle(&mut harness, |harness| harness.screen().contains("Firefly was deleted"));
    assert!(harness.screen().contains("Firefly was deleted"), "{}", harness.screen());

    assert_eq!(
        removals(&log),
        ["rm --force qcode-firefly-base", "rm --force qcode-firefly-claude", "volume rm qcode-home-firefly-claude"],
        "exactly Firefly's containers and home, and nothing of Serenity's or of the profile's"
    );
    assert!(!store.workspaces_dir().join("firefly").exists(), "the folder is gone");
    assert!(store.workspaces_dir().join("serenity").is_dir(), "the other workspace stays");
    settle(&mut harness, |harness| !harness.screen().contains("Firefly"));
    assert!(!harness.screen().contains("Firefly"), "and the list says so:\n{}", harness.screen());
    assert_eq!(harness.app().deleted, ["firefly"], "the application is told, once");
}

/// Opens the question about Firefly the way a person does: the button, then Firefly in the list
/// it opens.
fn ask_about_firefly(harness: &mut Harness<Screen>) {
    harness.click_text("Delete a workspace").render();
    click_below(harness, "Choose the workspace to delete", "Firefly");
}

#[cfg(unix)]
#[test]
fn a_workspace_with_a_running_container_is_not_deleted_and_the_container_is_named() {
    let scratch = Scratch::new("delete-running");
    let store = two_workspaces(&scratch);
    let running = CONTAINERS.replace("qcode-firefly-claude\texited", "qcode-firefly-claude\trunning");
    let (engine, log) = stand_in(&scratch, &running, VOLUMES);
    let mut harness = screen(scratch.path(), Some(engine));
    ask_about_firefly(&mut harness);
    settle(&mut harness, |harness| harness.screen().contains("has a container running"));
    let text = harness.screen();
    assert!(text.contains("Firefly has a container running"), "{text}");
    assert!(text.contains("qcode-firefly-claude"), "the running one is named:\n{text}");
    assert!(!text.contains("Delete Firefly?"), "and nothing is asked:\n{text}");
    assert_eq!(removals(&log), [] as [String; 0]);
    assert!(store.workspaces_dir().join("firefly").is_dir());
    assert!(harness.app().deleted.is_empty());
}

#[cfg(unix)]
#[test]
fn a_workspace_open_in_this_qcode_is_not_deleted_and_the_engine_is_not_even_asked() {
    let scratch = Scratch::new("delete-open");
    let store = two_workspaces(&scratch);
    let (engine, log) = stand_in(&scratch, CONTAINERS, VOLUMES);
    let mut state = Workspaces::new(Store::new(scratch.path()), Some(engine), Vec::new());
    state.set_open(vec![crate::store::WorkspaceId::parse("firefly").expect("an identifier")]);
    let mut harness = screen_over(state);
    ask_about_firefly(&mut harness);
    settle(&mut harness, |harness| harness.screen().contains("Firefly is open"));
    let text = harness.screen();
    assert!(text.contains("Firefly is open"), "{text}");
    assert!(!text.contains("Delete Firefly?"), "{text}");
    assert_eq!(asked(&log), [] as [String; 0], "the engine was not asked anything");
    assert!(store.workspaces_dir().join("firefly").is_dir());
}

#[cfg(unix)]
#[test]
fn saying_no_keeps_everything_and_the_button_works_again() {
    let scratch = Scratch::new("delete-kept");
    let store = two_workspaces(&scratch);
    let (engine, log) = stand_in(&scratch, CONTAINERS, VOLUMES);
    let mut harness = screen(scratch.path(), Some(engine));
    ask_about_firefly(&mut harness);
    settle(&mut harness, |harness| harness.screen().contains("Delete Firefly?"));
    harness.press("enter").render();
    assert!(!harness.screen().contains("Delete Firefly?"), "{}", harness.screen());
    harness.advance(Duration::from_millis(200)).render();
    assert_eq!(removals(&log), [] as [String; 0]);
    assert!(store.workspaces_dir().join("firefly").join("Work").join("main.rs").is_file());
    // The same way in opens the question again: nothing was left waiting.
    ask_about_firefly(&mut harness);
    settle(&mut harness, |harness| harness.screen().contains("Delete Firefly?"));
    assert!(harness.screen().contains("Delete Firefly?"), "{}", harness.screen());
}

#[test]
fn without_an_engine_the_folder_goes_and_the_question_says_the_engine_keeps_its_part() {
    let scratch = Scratch::new("delete-engineless");
    let store = two_workspaces(&scratch);
    let outside = scratch.path().join("my-own-code");
    fs::create_dir_all(&outside).expect("a folder of the person's own");
    fs::write(outside.join("main.rs"), "fn main() {}").expect("their file");
    let mut harness = screen(scratch.path(), None);
    ask_about_firefly(&mut harness);
    settle(&mut harness, |harness| harness.screen().contains("Delete Firefly?"));
    let text = harness.screen();
    assert!(text.contains("No container engine was"), "{text}");
    harness.press("tab").press("enter").render();
    settle(&mut harness, |harness| harness.screen().contains("Firefly was deleted"));
    assert!(!store.workspaces_dir().join("firefly").exists());
    assert!(outside.join("main.rs").is_file(), "a folder outside the store is never touched");
}

#[test]
fn a_new_workspace_is_the_first_row_of_the_list_for_the_pointer_and_the_keyboard() {
    let scratch = Scratch::new("new-row");
    let store = Store::new(scratch.path());
    store.create_workspace("Alpha", qframe::date::Date::today_utc()).expect("made");
    let mut harness = screen(scratch.path(), None);
    let text = harness.screen();
    let lines: Vec<&str> = text.lines().collect();
    let new = lines.iter().position(|line| line.contains("New workspace")).expect("the row is there");
    let alpha = lines.iter().position(|line| line.contains("Alpha")).expect("the workspace is there");
    assert_eq!(alpha, new + 1, "the new row stands first, right over the workspaces:\n{text}");
    assert_eq!(text.matches("New workspace").count(), 1, "and it is the only way to one:\n{text}");

    harness.click_text("New workspace").render();
    assert!(matches!(harness.app().state.overlay, Some(Overlay::New(_))), "a click opens the dialog");
    harness.press("esc").render();

    while !harness.is_focused(super::LIST) {
        harness.press("tab");
    }
    // The keyboard starts on the first workspace, so Enter still opens it as it always did; one
    // row up is the way to a new one, and moving there opens nothing by itself.
    harness.press("up").render();
    assert!(harness.app().state.overlay.is_none(), "moving onto the row only chooses it");
    harness.press("enter").render();
    assert!(matches!(harness.app().state.overlay, Some(Overlay::New(_))), "Enter on it opens the dialog");
    assert!(harness.app().opened.is_empty(), "and no workspace was opened on the way");
}
