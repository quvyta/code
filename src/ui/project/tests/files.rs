//! The project's files in the panel's file tree: the file manager, run end to end on real
//! temporary folders through the screen's own messages the way a click or a key would send them,
//! and the files opened in tabs: a picture and a text file in the project's container, a Markdown
//! document by QCode itself, and nothing at all for what no built-in app opens yet.
//!
//! The engine of these tests cannot be run, so a picture or an editor never really starts; what
//! is checked is the command the tab would spawn, which is where "nothing runs on this machine"
//! and "the path is one word" are decided.

use super::*;

use qframe::event::{Event, MouseButton, MouseEvent, MouseKind};
use qframe::keymap::Modifiers;
use qframe::widgets::TreeDrop;

use crate::base::apps::Editor;
use crate::ui::project::{FileMsg, FileTree, NameFor, escape, files};

/// Ctrl held, as a Ctrl+click holds it.
const CTRL: Modifiers = Modifiers { ctrl: true, alt: false, shift: false };

/// A moment for a menu or a dialog to settle.
const MOMENT: Duration = Duration::from_millis(400);

/// The screen with the project folder read, motion off so menus and dialogs are there at once.
fn files_harness(scratch: &Scratch) -> Harness<Screen> {
    let mut harness = harness(one_project(scratch), SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    harness
}

fn project_dir(scratch: &Scratch) -> std::path::PathBuf {
    scratch.0.join("Project")
}

fn tree(harness: &Harness<Screen>) -> &FileTree {
    harness.app().0.project().expect("a project is open").files()
}

/// Right-clicks the row showing `text`.
fn right_click(harness: &mut Harness<Screen>, text: &str) {
    let (x, y) = harness.find(text).unwrap_or_else(|| panic!("`{text}` is on screen:\n{}", harness.screen()));
    harness.mouse(MouseKind::Down(MouseButton::Right), x, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x, y);
    harness.advance(MOMENT);
}

/// Where the tree's top row, the project folder itself, is: the row above its first entry.
fn root_row(harness: &Harness<Screen>) -> (i32, i32) {
    let (x, y) = harness.find("src").expect("the first entry of the tree");
    (x, y - 1)
}

/// Right-clicks the project folder's own row.
fn right_click_root(harness: &mut Harness<Screen>) {
    let (x, y) = root_row(harness);
    harness.mouse(MouseKind::Down(MouseButton::Right), x, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x, y);
    harness.advance(MOMENT);
}

#[test]
fn a_new_file_is_made_in_a_folder_from_its_menu() {
    let scratch = Scratch::new("new-file");
    let mut harness = files_harness(&scratch);
    right_click(&mut harness, "src");
    harness.click_text("New file").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Name"), "a dialog asks for the name:\n{text}");
    assert!(harness.is_focused("project-files-name"), "and its field has the keyboard");

    harness.type_text("lib.rs").press("enter").advance(MOMENT);
    assert!(project_dir(&scratch).join("src/lib.rs").is_file(), "the file is on disk:\n{}", harness.screen());
    let text = harness.screen();
    assert!(text.contains("lib.rs") && text.contains("main.rs"), "the folder opened and shows it:\n{text}");
    assert_eq!(tree(&harness).selected(), Some("src/lib.rs"), "the selection follows the new file");
    assert!(tree(&harness).naming().is_none(), "the dialog closed");
}

#[test]
fn a_new_folder_is_made_at_the_root_from_the_project_folders_row() {
    let scratch = Scratch::new("new-folder");
    let mut harness = files_harness(&scratch);
    let (_, y) = root_row(&harness);
    let line = harness.screen().lines().nth(usize::try_from(y).expect("a row")).unwrap_or_default().to_owned();
    assert!(line.contains("Firefly"), "the project folder is the top row, by the project's name:\n{line}");
    right_click_root(&mut harness);
    let text = harness.screen();
    assert!(text.contains("New folder") && text.contains("Refresh"), "the project folder's menu:\n{text}");
    assert!(!text.contains("Rename"), "nothing to rename at the root:\n{text}");
    harness.click_text("New folder").advance(MOMENT);
    harness.type_text("docs").press("enter").advance(MOMENT);
    assert!(project_dir(&scratch).join("docs").is_dir(), "{}", harness.screen());
    assert!(harness.screen().contains("docs"), "{}", harness.screen());
    assert_eq!(tree(&harness).selected(), Some("docs"));
}

#[test]
fn names_are_checked_as_they_are_typed() {
    let scratch = Scratch::new("names");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::Files(FileMsg::NewFile(String::new()))).advance(MOMENT);
    assert!(!harness.screen().contains("cannot be empty"), "an empty field is not scolded at once");
    harness.press("enter").advance(MOMENT);
    assert!(harness.screen().contains("A name cannot be empty."), "{}", harness.screen());
    assert!(tree(&harness).naming().is_some(), "and the dialog stays");

    for (typed, said) in [
        ("README.md", "This folder already has an entry with"),
        ("a/b", "A name cannot contain “/”."),
        ("..", "“.” and “..” already mean this folder"),
    ] {
        harness.send(Msg::Files(FileMsg::Name(typed.to_owned()))).advance(MOMENT);
        assert!(harness.screen().contains(said), "`{typed}`:\n{}", harness.screen());
    }
    harness.press("enter").advance(MOMENT);
    assert!(!scratch.0.join("..").join("x").exists());
    harness.press("esc").advance(MOMENT);
    assert!(tree(&harness).naming().is_none(), "Esc closes the dialog:\n{}", harness.screen());
    assert!(fs::read_dir(project_dir(&scratch)).map(Iterator::count).unwrap_or_default() == 2, "nothing was made");
}

#[test]
fn a_rename_starts_from_the_old_name_and_follows_the_entry() {
    let scratch = Scratch::new("rename");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::ExpandFile("src".to_owned(), true));
    right_click(&mut harness, "src");
    harness.click_text("Rename").advance(MOMENT);
    let naming = tree(&harness).naming().expect("the dialog is open").clone();
    assert_eq!(naming.purpose, NameFor::Rename("src".to_owned()));
    assert_eq!(naming.value, "src", "the field starts with the old name");
    assert!(harness.screen().contains("New name for src"), "{}", harness.screen());
    // An unchanged name is not a clash with itself.
    assert!(!harness.screen().contains("already has"), "{}", harness.screen());

    harness.send(Msg::Files(FileMsg::Name("code".to_owned())));
    harness.press("enter").advance(MOMENT);
    let root = project_dir(&scratch);
    assert!(root.join("code/main.rs").is_file() && !root.join("src").exists(), "{}", harness.screen());
    let tree = tree(&harness);
    assert_eq!(tree.selected(), Some("code"));
    assert!(tree.is_open("code"), "an open folder stays open under its new name");
    assert!(harness.screen().contains("main.rs"), "{}", harness.screen());
}

#[test]
fn a_rename_starts_with_the_name_before_its_extension_selected() {
    let scratch = Scratch::new("rename-stem");
    let root = project_dir(&scratch);
    fs::write(root.join("report.final.md"), "").expect("a file with two dots");
    fs::write(root.join(".gitignore"), "").expect("a dotfile");
    let mut harness = files_harness(&scratch);

    // Typing replaces what is selected, so what survives shows what was.
    harness.send(Msg::Files(FileMsg::Rename("report.final.md".to_owned()))).advance(MOMENT);
    harness.type_text("summary").press("enter").advance(MOMENT);
    assert!(root.join("summary.md").is_file(), "only the part before the last dot:\n{}", harness.screen());

    harness.send(Msg::Files(FileMsg::Rename(".gitignore".to_owned()))).advance(MOMENT);
    harness.type_text("ignored").press("enter").advance(MOMENT);
    assert!(root.join("ignored").is_file(), "a dotfile is all name:\n{}", harness.screen());

    fs::create_dir_all(root.join("v1.2")).expect("a folder with a dot");
    harness.send(Msg::Files(FileMsg::Refresh));
    harness.send(Msg::Files(FileMsg::Rename("v1.2".to_owned()))).advance(MOMENT);
    harness.type_text("old").press("enter").advance(MOMENT);
    assert!(root.join("old").is_dir(), "a folder has no extension:\n{}", harness.screen());
}

#[test]
fn cut_and_paste_moves_an_entry_into_a_folder() {
    let scratch = Scratch::new("move");
    let mut harness = files_harness(&scratch);
    right_click(&mut harness, "README.md");
    harness.click_text("Cut").advance(MOMENT);
    assert_eq!(tree(&harness).cut(), ["README.md"]);

    right_click(&mut harness, "src");
    harness.click_text("Paste here").advance(MOMENT);
    let root = project_dir(&scratch);
    assert!(root.join("src/README.md").is_file() && !root.join("README.md").exists(), "{}", harness.screen());
    assert_eq!(fs::read_to_string(root.join("src/README.md")).ok().as_deref(), Some("hello\n"), "moved, not made anew");
    let tree = tree(&harness);
    assert!(tree.cut().is_empty(), "the cut is used up");
    assert_eq!(tree.selected(), Some("src/README.md"), "the selection follows it");
    assert!(tree.is_open("src"), "into a folder that opens to show it");
}

/// Selects the entries `keys` together, the cursor on the first, the way Ctrl+clicks leave them.
fn choose(harness: &mut Harness<Screen>, keys: &[&str]) {
    harness.send(Msg::SelectFile(keys[0].to_owned()));
    harness.send(Msg::Files(FileMsg::Choose(keys.iter().map(|key| (*key).to_owned()).collect())));
}

#[test]
fn ctrl_click_selects_several_entries() {
    let scratch = Scratch::new("choose");
    let mut harness = files_harness(&scratch);
    harness.click_text("README.md");
    let (x, y) = harness.find("src").expect("the folder's row");
    harness.events(&[
        Event::Mouse(MouseEvent { kind: MouseKind::Down(MouseButton::Left), x, y, mods: CTRL }),
        Event::Mouse(MouseEvent { kind: MouseKind::Up(MouseButton::Left), x, y, mods: CTRL }),
    ]);
    assert_eq!(tree(&harness).chosen(), ["README.md", "src"], "{}", harness.screen());
}

#[test]
fn the_menu_of_a_selected_row_cuts_the_whole_selection_and_paste_moves_it_all() {
    let scratch = Scratch::new("cut-many");
    let root = project_dir(&scratch);
    fs::create_dir_all(root.join("docs")).expect("a folder to move into");
    fs::write(root.join("plan.txt"), "plan\n").expect("a second file");
    let mut harness = files_harness(&scratch);
    choose(&mut harness, &["README.md", "plan.txt", "src"]);
    right_click(&mut harness, "plan.txt");
    let text = harness.screen();
    assert!(text.contains("Cut 3 entries") && text.contains("Delete 3 entries"), "{text}");
    assert!(!text.contains("Rename"), "a name is for one entry:\n{text}");
    harness.click_text("Cut 3 entries").advance(MOMENT);
    assert_eq!(tree(&harness).cut(), ["README.md", "plan.txt", "src"]);

    right_click(&mut harness, "docs");
    harness.click_text("Paste here").advance(MOMENT);
    for name in ["README.md", "plan.txt", "src/main.rs"] {
        assert!(root.join("docs").join(name).exists(), "{name} moved:\n{}", harness.screen());
        assert!(!root.join(name).exists(), "{name} left its place");
    }
    let tree = tree(&harness);
    assert!(tree.cut().is_empty(), "the cut is used up");
    assert_eq!(tree.chosen(), ["docs/README.md", "docs/plan.txt", "docs/src"], "the selection follows them");
}

#[test]
fn a_file_inside_a_selected_folder_travels_with_it() {
    let scratch = Scratch::new("nested");
    let root = project_dir(&scratch);
    fs::create_dir_all(root.join("docs")).expect("a folder to move into");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::ExpandFile("src".to_owned(), true));
    choose(&mut harness, &["src", "src/main.rs"]);
    harness.send(Msg::Files(FileMsg::Cut("src".to_owned())));
    assert_eq!(tree(&harness).cut(), ["src"], "only the folder is cut; its file goes inside it");
    harness.send(Msg::Files(FileMsg::Paste("docs".to_owned()))).advance(MOMENT);
    assert!(root.join("docs/src/main.rs").is_file(), "{}", harness.screen());
    assert!(!harness.screen().contains("could not be handled"), "nothing failed:\n{}", harness.screen());
}

#[test]
fn deleting_the_selection_asks_once_for_all_of_it() {
    let scratch = Scratch::new("delete-many");
    let root = project_dir(&scratch);
    let mut harness = files_harness(&scratch);
    choose(&mut harness, &["README.md", "src"]);
    harness.send(Msg::Files(FileMsg::Delete("src".to_owned()))).advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Delete 2 entries?"), "{text}");
    assert!(text.contains("README.md, src") && text.contains("everything in them"), "{text}");
    harness.press("tab").press("enter").advance(MOMENT);
    assert!(!root.join("README.md").exists() && !root.join("src").exists(), "{}", harness.screen());
    assert!(tree(&harness).chosen().is_empty(), "nothing gone stays selected");
}

#[test]
fn a_row_outside_the_selection_acts_on_itself_alone() {
    let scratch = Scratch::new("outside-selection");
    let mut harness = files_harness(&scratch);
    fs::write(project_dir(&scratch).join("plan.txt"), "").expect("another file");
    harness.send(Msg::Files(FileMsg::Refresh));
    choose(&mut harness, &["README.md", "plan.txt"]);
    right_click(&mut harness, "src");
    let text = harness.screen();
    assert!(text.contains("Rename") && !text.contains("entries"), "{text}");
    harness.click_text("Cut").advance(MOMENT);
    assert_eq!(tree(&harness).cut(), ["src"]);
}

#[test]
fn dragging_the_selection_onto_a_folder_moves_it_there() {
    let scratch = Scratch::new("drag");
    let root = project_dir(&scratch);
    fs::create_dir_all(root.join("docs")).expect("a folder to drop into");
    fs::write(root.join("plan.txt"), "plan\n").expect("a second file");
    let mut harness = files_harness(&scratch);
    choose(&mut harness, &["README.md", "plan.txt"]);
    let from = harness.find("plan.txt").expect("a selected row");
    let to = harness.find("docs").expect("the folder");
    harness.drag(from, to).advance(MOMENT);
    assert!(root.join("docs/README.md").is_file() && root.join("docs/plan.txt").is_file(), "{}", harness.screen());
    assert!(!root.join("README.md").exists() && !root.join("plan.txt").exists());
    assert!(tree(&harness).is_open("docs"), "the folder opens to show what came in");
}

#[test]
fn a_drop_follows_the_same_rules_as_a_paste() {
    let scratch = Scratch::new("drop-rules");
    let root = project_dir(&scratch);
    fs::create_dir_all(root.join("src/deep")).expect("a folder below");
    let mut harness = files_harness(&scratch);
    let drop = |keys: &[&str], into: Option<&str>| {
        Msg::Files(FileMsg::Drop(TreeDrop {
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
            into: into.map(str::to_owned),
        }))
    };
    harness.send(drop(&["src"], Some("src/deep"))).advance(MOMENT);
    assert!(harness.screen().contains("A folder cannot go into itself"), "{}", harness.screen());
    assert!(root.join("src/deep").is_dir());

    harness.send(drop(&["src/main.rs"], None)).advance(MOMENT);
    assert!(root.join("main.rs").is_file(), "the free space below the rows is the project folder");
}

#[test]
fn a_move_that_partly_fails_says_which_entries_stayed_and_why() {
    let scratch = Scratch::new("partial");
    let root = project_dir(&scratch);
    fs::write(root.join("src/README.md"), "other\n").expect("a clashing file");
    fs::write(root.join("plan.txt"), "plan\n").expect("a file that can move");
    let mut harness = files_harness(&scratch);
    choose(&mut harness, &["README.md", "plan.txt"]);
    harness.send(Msg::Files(FileMsg::Cut("README.md".to_owned())));
    harness.send(Msg::Files(FileMsg::Paste("src".to_owned()))).advance(MOMENT);
    assert!(root.join("src/plan.txt").is_file(), "what could move moved:\n{}", harness.screen());
    assert_eq!(fs::read_to_string(root.join("src/README.md")).ok().as_deref(), Some("other\n"), "nothing overwritten");
    assert_eq!(fs::read_to_string(root.join("README.md")).ok().as_deref(), Some("hello\n"));
    let text = harness.screen();
    assert!(text.contains("1 of 2 entries could not be handled"), "{text}");
    // The toast wraps its body, so the reason is looked for in parts.
    assert!(text.contains("README.md: The target folder") && text.contains("already has"), "{text}");
    assert_eq!(tree(&harness).cut(), ["README.md"], "what stayed stays cut, to try elsewhere");
}

#[test]
fn the_cut_row_is_faint_and_esc_lets_it_stay() {
    let scratch = Scratch::new("faint");
    let mut harness = files_harness(&scratch);
    let (x, y) = harness.find("README.md").expect("the row");
    let (x, y) = (u16::try_from(x).expect("a column"), u16::try_from(y).expect("a row"));
    let before = harness.fg(x, y);
    harness.send(Msg::Files(FileMsg::Cut("README.md".to_owned())));
    harness.hover(0, 0).render();
    assert_ne!(harness.fg(x, y), before, "the cut row is drawn in another tone:\n{}", harness.screen());

    let esc = escape(&harness.app().0).expect("Esc lets the cut go before it leaves the screen");
    harness.send(esc);
    assert!(tree(&harness).cut().is_empty());
    assert_eq!(harness.fg(x, y), before, "and the row is itself again");
    assert!(escape(&harness.app().0).is_none(), "with nothing cut Esc is the screen's own again");
}

#[test]
fn a_folder_cannot_go_into_itself_and_a_clash_is_refused_with_a_reason() {
    let scratch = Scratch::new("refuse");
    let root = project_dir(&scratch);
    fs::create_dir_all(root.join("src/deep")).expect("a folder below");
    fs::write(root.join("src/README.md"), "other\n").expect("a clashing file");
    let mut harness = files_harness(&scratch);

    harness.send(Msg::Files(FileMsg::Cut("src".to_owned())));
    harness.send(Msg::Files(FileMsg::Paste("src/deep".to_owned()))).advance(MOMENT);
    assert!(harness.screen().contains("A folder cannot go into itself"), "{}", harness.screen());
    assert!(root.join("src/deep").is_dir());

    harness.send(Msg::Files(FileMsg::Cut("README.md".to_owned())));
    harness.send(Msg::Files(FileMsg::Paste("src".to_owned()))).advance(MOMENT);
    assert!(harness.screen().contains("already has something called"), "{}", harness.screen());
    assert_eq!(fs::read_to_string(root.join("README.md")).ok().as_deref(), Some("hello\n"));
    assert_eq!(fs::read_to_string(root.join("src/README.md")).ok().as_deref(), Some("other\n"));
    assert_eq!(tree(&harness).cut(), ["README.md"], "a refused paste keeps the cut to try elsewhere");
}

#[test]
fn a_folder_inside_the_cut_one_cannot_be_chosen_to_paste_into() {
    let scratch = Scratch::new("paste-disabled");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::Files(FileMsg::Cut("src".to_owned())));
    right_click(&mut harness, "src");
    harness.click_text("Paste here").advance(MOMENT);
    assert!(project_dir(&scratch).join("src/main.rs").is_file(), "nothing moved:\n{}", harness.screen());
    assert_eq!(tree(&harness).cut(), ["src"]);
}

#[test]
fn deleting_asks_first_and_a_folder_says_everything_in_it_goes() {
    let scratch = Scratch::new("delete");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::Files(FileMsg::Delete("src".to_owned()))).advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Delete src?"), "{text}");
    // The dialog wraps its message, so the words are looked for on their own.
    assert!(text.contains("together") && text.contains("everything in it"), "{text}");
    let root = project_dir(&scratch);
    assert!(root.join("src").is_dir(), "nothing goes before the answer");

    harness.press("esc").advance(MOMENT);
    assert!(root.join("src").is_dir(), "Esc keeps it:\n{}", harness.screen());

    harness.send(Msg::Files(FileMsg::Delete("README.md".to_owned()))).advance(MOMENT);
    assert!(!harness.screen().contains("everything in it"), "a file is only itself");
    harness.press("tab").press("enter").advance(MOMENT);
    assert!(!root.join("README.md").exists(), "confirming deletes it:\n{}", harness.screen());
    assert!(!harness.screen().contains("README.md"), "and the tree follows:\n{}", harness.screen());
}

#[test]
fn paths_that_escape_the_project_are_refused() {
    let scratch = Scratch::new("escape");
    fs::write(scratch.0.join("keep.txt"), "keep\n").expect("a file beside the project");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::Files(FileMsg::DeleteConfirmed(vec!["../keep.txt".to_owned()]))).advance(MOMENT);
    assert!(scratch.0.join("keep.txt").exists());
    assert!(harness.screen().contains("outside the project folder"), "{}", harness.screen());
    harness.send(Msg::Files(FileMsg::Cut("README.md".to_owned())));
    harness.send(Msg::Files(FileMsg::Paste("..".to_owned()))).advance(MOMENT);
    assert!(project_dir(&scratch).join("README.md").exists() && !scratch.0.join("README.md").exists());
}

#[test]
fn a_link_out_of_the_project_is_an_entry_and_deleting_it_leaves_its_target() {
    let scratch = Scratch::new("link");
    let away = scratch.0.join("away");
    fs::create_dir_all(&away).expect("a folder outside");
    fs::write(away.join("secret.txt"), "keep\n").expect("a file outside");
    std::os::unix::fs::symlink(&away, project_dir(&scratch).join("door")).expect("a link");
    let mut harness = files_harness(&scratch);
    assert!(!tree(&harness).is_folder("door"), "a link is not opened like a folder");
    harness.send(Msg::Files(FileMsg::NewFile("door".to_owned())));
    harness.send(Msg::Files(FileMsg::Name("x".to_owned())));
    harness.send(Msg::Files(FileMsg::Submit)).advance(MOMENT);
    assert!(!away.join("x").exists(), "nothing is made through it");
    harness.send(Msg::Files(FileMsg::DeleteConfirmed(vec!["door".to_owned()])));
    assert!(!project_dir(&scratch).join("door").exists());
    assert!(away.join("secret.txt").exists(), "what it pointed at stays");
}

#[test]
fn each_row_offers_what_can_be_done_to_it() {
    let scratch = Scratch::new("menus");
    let mut harness = files_harness(&scratch);
    right_click(&mut harness, "README.md");
    let text = harness.screen();
    for item in ["Rename", "Cut", "Delete"] {
        assert!(text.contains(item), "`{item}` on a file:\n{text}");
    }
    for item in ["New file", "New folder", "Paste here"] {
        assert!(!text.contains(item), "no `{item}` on a file:\n{text}");
    }
    harness.press("esc").advance(MOMENT);

    right_click(&mut harness, "src");
    let text = harness.screen();
    for item in ["New file", "New folder", "Rename", "Cut", "Delete"] {
        assert!(text.contains(item), "`{item}` on a folder:\n{text}");
    }
    assert!(!text.contains("Paste here"), "nothing to paste yet:\n{text}");
    harness.press("esc").advance(MOMENT);

    harness.send(Msg::Files(FileMsg::Cut("README.md".to_owned())));
    right_click(&mut harness, "src");
    let text = harness.screen();
    assert!(text.contains("Paste here") && text.contains("Cancel the move"), "{text}");
    harness.click_text("Cancel the move").advance(MOMENT);
    assert!(tree(&harness).cut().is_empty(), "the mouse lets a cut go too");
}

#[test]
fn a_click_or_enter_on_a_file_opens_it_while_ctrl_click_only_chooses_it() {
    let scratch = Scratch::new("open-or-choose");
    let mut harness = files_harness(&scratch);
    harness.click_text("src").advance(MOMENT);
    let (x, y) = harness.find("README.md").expect("the file's row");
    harness.events(&[
        Event::Mouse(MouseEvent { kind: MouseKind::Down(MouseButton::Left), x, y, mods: CTRL }),
        Event::Mouse(MouseEvent { kind: MouseKind::Up(MouseButton::Left), x, y, mods: CTRL }),
    ]);
    assert_eq!(tree(&harness).chosen(), ["src", "README.md"], "{}", harness.screen());
    assert!(labels(&harness).is_empty(), "a Ctrl+click chooses without opening");
    harness.press("space").advance(MOMENT);
    assert!(labels(&harness).is_empty(), "Space chooses too, it does not open");

    harness.click_text("main.rs").advance(MOMENT);
    assert_eq!(labels(&harness), [TabKind::Editor("src/main.rs".to_owned())], "a plain click opens the file");
    assert_eq!(tree(&harness).chosen(), ["src/main.rs"], "and makes it the one selected entry");

    harness.click_text("src").advance(MOMENT);
    harness.press("end").press("enter").advance(MOMENT);
    assert_eq!(
        labels(&harness),
        [TabKind::Editor("src/main.rs".to_owned()), TabKind::Markdown("README.md".to_owned())],
        "Enter opens the file under the cursor"
    );
}

#[test]
fn the_keyboard_reaches_the_same_menus() {
    let scratch = Scratch::new("keys");
    let mut harness = files_harness(&scratch);
    // A click on the file would open it and hand the keyboard to its tab, so the tree is reached
    // through the folder above it.
    harness.click_text("src").advance(MOMENT);
    harness.press("end").advance(MOMENT);
    assert_eq!(tree(&harness).selected(), Some("README.md"), "{}", harness.screen());
    assert!(harness.is_focused("project-files"), "{}", harness.screen());
    harness.press("shift+f10").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Rename") && text.contains("Delete"), "the selected row's menu:\n{text}");
    harness.press("enter").advance(MOMENT);
    assert!(
        matches!(tree(&harness).naming().map(|naming| &naming.purpose), Some(NameFor::Rename(key)) if key == "README.md"),
        "{}",
        harness.screen()
    );
}

#[test]
fn the_keyboard_reaches_the_project_folders_menu_from_its_row() {
    let scratch = Scratch::new("root-keys");
    let mut harness = files_harness(&scratch);
    harness.click_text("src").advance(MOMENT);
    harness.press("home").advance(MOMENT);
    assert_eq!(tree(&harness).selected(), Some(""), "the top row is the project folder");
    harness.press("shift+f10").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("New folder") && text.contains("Refresh"), "{text}");
    assert!(!text.contains("Rename") && !text.contains("Delete"), "the folder itself stays:\n{text}");
}

#[test]
fn an_empty_project_folder_still_has_its_row_to_make_the_first_entry() {
    let scratch = Scratch::new("empty");
    let root = project_dir(&scratch);
    fs::remove_dir_all(&root).expect("clear the project");
    fs::create_dir_all(&root).expect("an empty project folder");
    let mut harness = files_harness(&scratch);
    let text = harness.screen();
    assert!(text.contains("empty"), "the row says the folder is empty:\n{text}");
    let (x, y) = harness.find("empty").expect("the empty row");
    harness.mouse(MouseKind::Down(MouseButton::Right), x, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x, y);
    harness.advance(MOMENT);
    harness.click_text("New file").advance(MOMENT);
    harness.type_text("first.txt").press("enter").advance(MOMENT);
    assert!(root.join("first.txt").is_file(), "{}", harness.screen());
    assert!(harness.screen().contains("first.txt"), "{}", harness.screen());
}

#[test]
fn refresh_and_coming_back_read_the_open_folders_again() {
    let scratch = Scratch::new("refresh");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::ExpandFile("src".to_owned(), true));
    fs::write(project_dir(&scratch).join("src/late.rs"), "").expect("a file made by someone else");
    fs::write(project_dir(&scratch).join("GUIDE.md"), "").expect("another");
    assert!(!harness.screen().contains("late.rs"), "nothing is watched or polled");
    harness.send(Msg::Files(FileMsg::Refresh));
    let text = harness.screen();
    assert!(text.contains("late.rs") && text.contains("GUIDE.md"), "{text}");

    fs::remove_file(project_dir(&scratch).join("GUIDE.md")).expect("gone again");
    harness.send(Msg::OpenProject(0));
    assert!(!harness.screen().contains("GUIDE.md"), "coming back reads again:\n{}", harness.screen());
}

#[test]
fn a_folder_removed_by_another_program_leaves_nothing_behind() {
    let scratch = Scratch::new("removed");
    let mut harness = files_harness(&scratch);
    harness.send(Msg::ExpandFile("src".to_owned(), true));
    harness.send(Msg::Files(FileMsg::Cut("src/main.rs".to_owned())));
    fs::remove_dir_all(project_dir(&scratch).join("src")).expect("removed by someone else");
    harness.send(Msg::Files(FileMsg::Refresh));
    let files = tree(&harness);
    assert!(!files.is_open("src"), "a folder that is gone is not remembered open");
    assert!(files.cut().is_empty(), "and nothing in it waits to be pasted");
    fs::create_dir_all(project_dir(&scratch).join("src")).expect("a new folder of the same name");
    harness.send(Msg::Files(FileMsg::Refresh));
    assert!(!tree(&harness).is_open("src"), "a new folder of the old name starts closed");
}

#[test]
fn the_menu_and_the_dialog_draw_no_brackets_or_frames() {
    let scratch = Scratch::new("look");
    let mut harness = files_harness(&scratch);
    for mode in [GlyphMode::Unicode, GlyphMode::Ascii] {
        harness.set_glyph_mode(mode).render();
        right_click(&mut harness, "src");
        let menu = harness.screen();
        harness.press("esc").advance(MOMENT);
        harness.send(Msg::Files(FileMsg::Rename("src".to_owned()))).advance(MOMENT);
        let dialog = harness.screen();
        harness.press("esc").advance(MOMENT);
        for text in [menu, dialog] {
            for mark in ['[', ']', '|', '│', '┌', '┐', '└', '┘', '─'] {
                assert!(!text.contains(mark), "{mode:?} draws `{mark}`:\n{text}");
            }
        }
    }
}

#[test]
fn the_file_manager_speaks_turkish() {
    let scratch = Scratch::new("turkish");
    let mut harness = files_harness(&scratch);
    harness.set_locale("tr").render();
    right_click(&mut harness, "src");
    let text = harness.screen();
    for item in ["Yeni dosya", "Yeni klasör", "Yeniden adlandır", "Kes", "Sil"] {
        assert!(text.contains(item), "`{item}`:\n{text}");
    }
    harness.press("esc").advance(MOMENT);
    harness.send(Msg::Files(FileMsg::Delete("src".to_owned()))).advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("src silinsin mi?") && text.contains("içindeki her şeyle"), "{text}");
}

#[test]
fn a_message_for_a_project_that_is_gone_changes_nothing() {
    let scratch = Scratch::new("gone");
    let mut screen: ProjectScreen = one_project(&scratch);
    apply(
        &mut screen,
        Msg::Files(FileMsg::Done(
            "elsewhere".to_owned(),
            vec![("x".to_owned(), Ok(super::super::Change::Created("x".into())))],
        )),
    );
    assert_eq!(screen.project().expect("a project").files().selected(), None);
}

/// A project folder with a picture, a Markdown document and a text file with an awkward name.
fn with_files(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    let project = scratch.paths().project;
    fs::create_dir_all(project.join("art")).expect("a folder for pictures");
    fs::create_dir_all(project.join("guide")).expect("a folder for documents");
    fs::write(project.join("art").join("the logo.png"), [0x89, b'P', b'N', b'G']).expect("a picture");
    fs::write(project.join("guide").join("harbour.md"), "# Harbour notes\n\nThe tide turns at *six*.\n")
        .expect("a document");
    fs::write(project.join("it's $HOME.txt"), "text\n").expect("a text file");
    fs::write(project.join("song.mp3"), [0, 1, 2]).expect("a sound");
    scratch
}

/// The words of the command the tab `key` would spawn.
fn words(screen: &ProjectScreen, key: TabKey) -> Vec<String> {
    let command = screen.launch_command(key).expect("the tab has a command");
    assert_eq!(command.program, Path::new(NO_ENGINE), "a tab only ever starts the engine binary");
    command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

/// The labels of the open project's tabs, as the strip shows them.
fn labels(harness: &Harness<Screen>) -> Vec<TabKind> {
    kinds(&harness.app().0)
}

#[test]
fn a_text_file_opens_in_nano_in_the_projects_own_container_as_one_word() {
    let scratch = with_files("text");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("it's $HOME.txt".to_owned()));
    assert_eq!(kinds(&screen), [TabKind::Editor("it's $HOME.txt".to_owned())]);
    let args = words(&screen, key(&screen, 0));
    assert_eq!(
        args,
        ["exec", "--interactive", "--tty", "qcode-firefly-base", "nano", &format!("{PROJECT_DIR}/it's $HOME.txt")],
        "the base container, the editor, and the path as a single argument that no shell reads"
    );
    assert_eq!(state(&screen, 0, 0), TabState::Starting, "a file tab starts as soon as it is shown");
}

#[test]
fn the_chosen_editor_is_the_one_a_file_opens_in() {
    let scratch = with_files("vim");
    let mut screen = one_project(&scratch);
    assert_eq!(screen.editor(), Editor::Nano, "nano until another is chosen");
    screen.set_editor(Editor::Vim);
    apply(&mut screen, Msg::OpenFile("src/main.rs".to_owned()));
    let args = words(&screen, key(&screen, 0));
    assert_eq!(args[args.len() - 2..], ["vim".to_owned(), format!("{PROJECT_DIR}/src/main.rs")]);

    screen.set_editor(Editor::Nano);
    let args = words(&screen, key(&screen, 0));
    assert_eq!(args[args.len() - 2], "nano", "the tab takes the editor of the moment it is opened again");
}

#[test]
fn a_picture_is_drawn_by_chafa_in_the_projects_own_container() {
    let scratch = with_files("picture");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("art/the logo.png".to_owned()));
    assert_eq!(kinds(&screen), [TabKind::Image("art/the logo.png".to_owned())]);
    let args = words(&screen, key(&screen, 0));
    assert_eq!(args[..4], ["exec", "--interactive", "--tty", "qcode-firefly-base"]);
    assert_eq!(args[4], "chafa");
    assert_eq!(args.last().map(String::as_str), Some(format!("{PROJECT_DIR}/art/the logo.png").as_str()));
    assert_eq!(args.iter().filter(|word| word.ends_with("the logo.png")).count(), 1, "{args:?}");
}

#[test]
fn a_drawn_picture_stays_with_a_quiet_way_to_draw_it_again_and_no_ending_line() {
    let scratch = with_files("drawn");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("art/the logo.png".to_owned()));
    let tab = key(&screen, 0);
    apply(&mut screen, Msg::Output(tab, 0, TerminalEvent::Exited(Some(0))));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("Redraw"), "{text}");
    assert!(text.contains("the logo.png"), "the tab is named after the file:\n{text}");
    assert!(!text.contains("ended"), "a picture that is there needs no note that its program ended:\n{text}");
    assert!(!text.contains("Start again"), "{text}");

    harness.click_text("Redraw");
    let app = &harness.app().0;
    let tab = &app.project().expect("a project").tabs()[0];
    assert_eq!(tab.run(), 1, "drawing again starts chafa again, at the tab's size of now");
}

#[test]
fn a_picture_chafa_could_not_draw_says_so() {
    let scratch = with_files("not-drawn");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("art/the logo.png".to_owned()));
    let tab = key(&screen, 0);
    apply(&mut screen, Msg::Output(tab, 0, TerminalEvent::Exited(Some(2))));
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("The picture could not be drawn"), "{text}");
    assert!(text.contains("Redraw"), "{text}");
}

#[test]
fn leaving_the_editor_closes_the_file_and_offers_to_open_it_again() {
    let scratch = with_files("closed");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("src/main.rs".to_owned()));
    let tab = key(&screen, 0);
    apply(&mut screen, Msg::Output(tab, 0, TerminalEvent::Exited(Some(0))));
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("The file was closed"), "{text}");
    assert!(text.contains("Open again"), "{text}");
    assert!(!text.contains("The session in the container ended"), "{text}");
}

#[test]
fn a_markdown_document_is_shown_by_qcode_and_edited_in_a_tab_of_its_own() {
    let scratch = with_files("markdown");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.send(Msg::OpenFile("guide/harbour.md".to_owned())).render();
    let text = harness.screen();
    assert!(text.contains("Harbour notes"), "the heading is drawn:\n{text}");
    assert!(text.contains("The tide turns at six"), "{text}");
    assert!(!text.contains('#'), "the heading is drawn, not its markup:\n{text}");
    assert!(text.contains("Edit"), "{text}");
    assert!(harness.is_focused("project-document"), "the document takes the keyboard");
    let screen = &harness.app().0;
    assert!(screen.launch_command(key(screen, 0)).is_none(), "a document starts no container at all");

    harness.click_text("Edit");
    assert_eq!(
        labels(&harness),
        [TabKind::Markdown("guide/harbour.md".to_owned()), TabKind::Editor("guide/harbour.md".to_owned())],
        "the editor opens beside the document"
    );
    let screen = &harness.app().0;
    let args = words(screen, key(screen, 1));
    assert_eq!(args[args.len() - 2..], ["nano".to_owned(), format!("{PROJECT_DIR}/guide/harbour.md")]);
}

#[test]
fn a_document_shown_again_is_read_again() {
    let scratch = with_files("reread");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.send(Msg::OpenFile("guide/harbour.md".to_owned()));
    harness.send(Msg::OpenFile("src/main.rs".to_owned()));
    fs::write(scratch.paths().project.join("guide").join("harbour.md"), "# Changed in the editor\n").expect("a change");
    harness.send(Msg::OpenTab(0)).render();
    assert!(harness.screen().contains("Changed in the editor"), "{}", harness.screen());
}

#[test]
fn a_file_opened_twice_is_one_tab() {
    let scratch = with_files("twice");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("guide/harbour.md".to_owned()));
    apply(&mut screen, Msg::OpenFile("src/main.rs".to_owned()));
    apply(&mut screen, Msg::OpenFile("guide/harbour.md".to_owned()));
    assert_eq!(kinds(&screen).len(), 2, "a second click on a file shows its tab");
    assert_eq!(screen.project().and_then(OpenProject::active_tab).map(Tab::key), Some(key(&screen, 0)));
}

#[test]
fn a_file_no_built_in_app_opens_is_said_in_a_toast_and_opens_nothing() {
    let scratch = with_files("unknown");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.send(Msg::OpenFile("song.mp3".to_owned())).advance(Duration::from_millis(400));
    let text = harness.screen();
    assert!(text.contains("No built-in app opens this kind of file yet"), "{text}");
    assert!(text.contains("song.mp3"), "{text}");
    assert!(labels(&harness).is_empty(), "no tab is opened for it");

    harness.set_locale("tr").send(Msg::OpenFile("song.mp3".to_owned())).advance(Duration::from_millis(400));
    assert!(harness.screen().contains("Bu tür için henüz bir uygulama yok"), "{}", harness.screen());
}

#[test]
fn a_path_that_climbs_out_of_the_project_is_refused_everywhere() {
    for key in ["", "../secret.txt", "guide/../../secret.md", "/etc/passwd", "guide//harbour.md", "./harbour.md", "a/."]
    {
        assert!(!files::is_inside(key), "{key:?}");
    }
    for key in ["harbour.md", "guide/harbour.md", "..hidden.txt", "a/b/c.rs", "it's $HOME.txt"] {
        assert!(files::is_inside(key), "{key:?}");
    }

    let scratch = with_files("escape");
    let mut screen = one_project(&scratch);
    for key in ["../secret.txt", "/etc/passwd", "../../outside.md"] {
        apply(&mut screen, Msg::OpenFile(key.to_owned()));
        apply(&mut screen, Msg::EditFile(key.to_owned()));
    }
    assert!(kinds(&screen).is_empty(), "{:?}", kinds(&screen));

    let record = SessionProject {
        id: ProjectId::parse("firefly").expect("a usable id"),
        active_tab: 0,
        tabs: vec![
            SessionTab { kind: SessionTabKind::Markdown("../../outside.md".to_owned()), conversation: None, opened: 1 },
            SessionTab { kind: SessionTabKind::Editor("/etc/passwd".to_owned()), conversation: None, opened: 2 },
            SessionTab { kind: SessionTabKind::Image("art/the logo.png".to_owned()), conversation: None, opened: 3 },
        ],
    };
    screen.restore_tabs(0, &record);
    assert_eq!(
        kinds(&screen),
        [TabKind::Image("art/the logo.png".to_owned())],
        "a session file cannot name one either"
    );
}

#[cfg(unix)]
#[test]
fn a_document_that_links_out_of_the_project_is_not_read() {
    let scratch = with_files("link");
    let outside = scratch.0.join("outside.md");
    fs::write(&outside, "# Not the project's\n").expect("a file beside the project");
    std::os::unix::fs::symlink(&outside, scratch.paths().project.join("link.md")).expect("a link");
    let mut harness = harness(one_project(&scratch), SIZE.0, SIZE.1);
    harness.send(Msg::OpenFile("link.md".to_owned())).render();
    let text = harness.screen();
    assert!(!text.contains("Not the project's"), "{text}");
    assert!(text.contains("leads outside the project folder"), "{text}");
}

#[test]
fn file_tabs_are_kept_in_the_session_and_come_back_one_at_a_time() {
    let scratch = with_files("session");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("art/the logo.png".to_owned()));
    apply(&mut screen, Msg::OpenFile("it's $HOME.txt".to_owned()));
    apply(&mut screen, Msg::OpenFile("guide/harbour.md".to_owned()));
    let session = screen.session();
    let saved: Vec<SessionTabKind> = session.projects[0].tabs.iter().map(|tab| tab.kind.clone()).collect();
    assert_eq!(
        saved,
        [
            SessionTabKind::Image("art/the logo.png".to_owned()),
            SessionTabKind::Editor("it's $HOME.txt".to_owned()),
            SessionTabKind::Markdown("guide/harbour.md".to_owned()),
        ]
    );
    let text = session.to_toml();
    let read = Session::parse("session.toml", &text);
    assert!(read.is_clean(), "{:?}\n{text}", read.diagnostics);
    assert_eq!(read.value, session, "the file reads back as it was written");

    // Brought back, only the tab that is open starts; the others wait to be shown.
    let mut again = one_project(&scratch);
    again.restore_tabs(0, &read.value.projects[0]);
    assert_eq!(again.session(), session);
    let mut harness = harness(again, SIZE.0, SIZE.1);
    let screen = &harness.app().0;
    assert_eq!(state(screen, 0, 0), TabState::Waiting, "the picture has not been drawn");
    assert_eq!(state(screen, 0, 1), TabState::Waiting, "the editor has not been opened");
    assert_eq!(state(screen, 0, 2), TabState::Running, "the open document was read");
    assert!(harness.screen().contains("Harbour notes"), "{}", harness.screen());

    harness.send(Msg::OpenTab(1));
    assert_ne!(state(&harness.app().0, 0, 1), TabState::Waiting, "shown, the editor tab starts");
}

#[test]
fn a_restored_tab_whose_file_is_gone_says_so() {
    let scratch = with_files("gone");
    let tab = |kind| SessionTab { kind, conversation: None, opened: 1 };
    let record = SessionProject {
        id: ProjectId::parse("firefly").expect("a usable id"),
        active_tab: 0,
        tabs: vec![
            tab(SessionTabKind::Markdown("guide/gone.md".to_owned())),
            tab(SessionTabKind::Editor("gone.txt".to_owned())),
        ],
    };
    let mut screen = one_project(&scratch);
    screen.restore_tabs(0, &record);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    assert_eq!(state(&harness.app().0, 0, 0), TabState::Missing);
    let text = harness.screen();
    assert!(text.contains("guide/gone.md is not in the project any more"), "{text}");
    assert!(text.contains("Look again"), "{text}");

    // The editor would open an empty file of that name, so it is not started either.
    harness.send(Msg::OpenTab(1)).render();
    assert_eq!(state(&harness.app().0, 0, 1), TabState::Missing);
    assert!(harness.screen().contains("gone.txt is not in the project any more"), "{}", harness.screen());

    // The file comes back and looking again finds it.
    fs::write(scratch.paths().project.join("guide").join("gone.md"), "# Back again\n").expect("the file returns");
    harness.send(Msg::OpenTab(0)).click_text("Look again").render();
    assert!(harness.screen().contains("Back again"), "{}", harness.screen());
}

#[test]
fn without_an_engine_a_document_still_opens_and_a_picture_waits() {
    let scratch = with_files("no-engine");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    let mut harness = harness(ProjectScreen::new(None, HostUser::ImageDefault, projects), SIZE.0, SIZE.1);
    harness.send(Msg::OpenFile("guide/harbour.md".to_owned())).render();
    assert!(harness.screen().contains("Harbour notes"), "{}", harness.screen());
    harness.send(Msg::OpenFile("art/the logo.png".to_owned())).render();
    assert_eq!(state(&harness.app().0, 0, 1), TabState::Waiting);
    assert!(harness.screen().contains("No container engine was found"), "{}", harness.screen());
}

#[test]
fn file_tabs_draw_nothing_bracketed_nothing_but_ascii_in_ascii_mode_and_turkish_in_turkish() {
    let scratch = with_files("aesthetic");
    let mut document = harness(one_project(&scratch), SIZE.0, SIZE.1);
    document.send(Msg::OpenFile("guide/harbour.md".to_owned())).render();
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        document.set_glyph_mode(mode).render();
        let text = document.screen();
        for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│'] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {mode:?}:\n{text}");
        }
    }
    document.set_glyph_mode(GlyphMode::Ascii).render();
    assert!(document.screen().is_ascii(), "{}", document.screen());

    document.set_glyph_mode(GlyphMode::Unicode).set_locale("tr").render();
    assert!(document.screen().contains("Düzenle"), "{}", document.screen());

    // The picture's own page, and the editor's closed one, in Turkish.
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::OpenFile("art/the logo.png".to_owned()));
    let tab = key(&screen, 0);
    apply(&mut screen, Msg::Output(tab, 0, TerminalEvent::Exited(Some(0))));
    apply(&mut screen, Msg::OpenFile("src/main.rs".to_owned()));
    let tab = key(&screen, 1);
    apply(&mut screen, Msg::Output(tab, 0, TerminalEvent::Exited(Some(0))));
    let mut shown = harness(screen, SIZE.0, SIZE.1);
    shown.set_locale("tr").render();
    let text = shown.screen();
    assert!(text.contains("Dosya kapandı"), "{text}");
    assert!(text.contains("Yeniden aç"), "{text}");
    shown.send(Msg::OpenTab(0)).render();
    assert!(shown.screen().contains("Yeniden çiz"), "{}", shown.screen());
    shown.set_locale("en").set_glyph_mode(GlyphMode::Ascii).render();
    assert!(shown.screen().is_ascii(), "{}", shown.screen());
}
