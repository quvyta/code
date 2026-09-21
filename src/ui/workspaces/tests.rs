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
}

impl App for Screen {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        let (command, opened) = update(&mut self.state, message);
        self.opened.extend(opened.map(|id| id.as_str().to_owned()));
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
    let state = Workspaces::new(Store::new(root), engine, Vec::new());
    let mut harness = Harness::with_env(Screen { state, opened: Vec::new() }, env(), SIZE.0, SIZE.1);
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
    let harness = Harness::with_env(Screen { state, opened: Vec::new() }, env(), SIZE.0, SIZE.1);
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
    let mut harness = Harness::with_env(Screen { state, opened: Vec::new() }, env(), SIZE.0, SIZE.1);
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
    assert!(text.contains("No container engine was found"), "{text}");
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
