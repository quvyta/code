//! The files whose text is taken out in the workspace's container and shown by QCode: a PDF, with
//! its pages drawn as pictures, and a word processor's document.
//!
//! The engine of these tests cannot be run, so nothing is really read or drawn; the text arrives
//! the way the background work would deliver it, and what is checked is the command each step
//! would run, which is where "nothing runs on this machine" and "the path is one word" are
//! decided.

use super::*;

use crate::engine::EngineCommand;
use crate::store::{Session, SessionTab, SessionTabKind, SessionWorkspace};
use crate::ui::workspace::{LaunchFailure, Pages, Taken, entry};

/// The PDF of these tests, with a space in its folder and its name.
const PDF: &str = "papers/tide tables.pdf";

/// What pdftotext prints for a two-page PDF.
const TWO_PAGES: &str = "Harbour notes\n\nThe tide turns at six.\n\u{c}Second page\n\u{c}";

/// A workspace folder with a PDF in it.
fn with_pdf(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    let papers = scratch.paths().code.join("papers");
    fs::create_dir_all(&papers).expect("a folder for papers");
    fs::write(papers.join("tide tables.pdf"), b"%PDF-1.4\n").expect("a PDF");
    scratch
}

/// The words of an engine command, after checking it is the engine that runs.
fn spelled(command: &EngineCommand) -> Vec<String> {
    assert_eq!(command.program, Path::new(NO_ENGINE), "only the engine binary is ever started");
    command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

/// The PDF opened in the open workspace, and the key of its tab.
fn open_pdf(screen: &mut WorkspaceScreen) -> TabKey {
    apply(screen, Msg::OpenFile(PDF.to_owned()));
    key(screen, 0)
}

/// The tab of `key` in the open workspace.
fn tab(screen: &WorkspaceScreen, key: TabKey) -> &Tab {
    screen.workspace().expect("a workspace").tabs().iter().find(|tab| tab.key() == key).expect("the tab")
}

#[test]
fn a_pdf_opens_as_its_text_taken_out_in_the_workspaces_own_container() {
    let scratch = with_pdf("pdf-open");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    assert_eq!(kinds(&screen), [TabKind::Pdf(PDF.to_owned())]);
    assert_eq!(state(&screen, 0, 0), TabState::Starting, "the text is being taken out");
    assert!(screen.launch_command(key).is_none(), "the text is not a program in a terminal");
    let read = spelled(&screen.read_command(key).expect("a PDF's text is read"));
    assert_eq!(
        read,
        ["exec", "qcode-firefly-base", "pdftotext", "-layout", &format!("{CODE_DIR}/{PDF}"), "-"],
        "the base container, no terminal, and the path as a single argument"
    );
}

#[test]
fn the_text_is_shown_by_qcode_with_the_way_to_draw_a_page() {
    let scratch = with_pdf("pdf-text");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    apply(&mut screen, Msg::TextRead(key, 0, Ok(Taken::new(TWO_PAGES.to_owned()))));
    assert_eq!(state(&screen, 0, 0), TabState::Running);
    assert_eq!(tab(&screen, key).pages(), Pages { count: 2, page: 1, drawn: false });
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    for words in ["tide tables.pdf", "Harbour notes", "The tide turns at six.", "Second page", "Page picture"] {
        assert!(text.contains(words), "`{words}`:\n{text}");
    }
    assert!(!text.contains('\u{c}'), "no page break is drawn as a character:\n{text}");
    assert_eq!(entry(&harness.app().0), "workspace-document", "the text takes the keyboard, like a document");
}

#[test]
fn a_page_is_drawn_as_a_picture_and_the_pages_are_walked_through() {
    let scratch = with_pdf("pdf-pages");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    apply(&mut screen, Msg::TextRead(key, 0, Ok(Taken::new(TWO_PAGES.to_owned()))));
    apply(&mut screen, Msg::Page(key, Some(1)));
    assert_eq!(state(&screen, 0, 0), TabState::Starting, "the page is drawn from the container up");
    let words = spelled(&screen.launch_command(key).expect("a page is drawn in a terminal"));
    assert_eq!(words[..4], ["exec", "--interactive", "--tty", "qcode-firefly-base"]);
    assert_eq!(words[4..6], ["sh", "-c"]);
    assert_eq!(words[7..], ["sh".to_owned(), "1".to_owned(), format!("{CODE_DIR}/{PDF}")]);

    apply(&mut screen, Msg::Page(key, Some(2)));
    assert_eq!(spelled(&screen.launch_command(key).expect("the next page"))[8], "2");
    apply(&mut screen, Msg::Page(key, Some(9)));
    assert_eq!(tab(&screen, key).pages().page, 2, "there is no page past the last");
    apply(&mut screen, Msg::Page(key, Some(0)));
    assert_eq!(tab(&screen, key).pages().page, 1, "nor before the first");

    let run = tab(&screen, key).run();
    apply(&mut screen, Msg::Output(key, run, TerminalEvent::Exited(Some(0))));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    for words in ["Previous", "Page 1 of 2", "Next", "Text", "Redraw"] {
        assert!(text.contains(words), "`{words}`:\n{text}");
    }
    assert!(!text.contains("could not be drawn"), "{text}");

    harness.click_text("Text");
    let screen = &harness.app().0;
    assert_eq!(state(screen, 0, 0), TabState::Running);
    assert!(!tab(screen, key).pages().drawn);
    assert!(harness.screen().contains("Harbour notes"), "{}", harness.screen());
    assert!(harness.is_focused("workspace-document"));
}

#[test]
fn a_pdf_without_text_opens_straight_on_its_first_page_and_has_no_text_to_go_back_to() {
    let scratch = with_pdf("pdf-scanned");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    apply(&mut screen, Msg::TextRead(key, 0, Ok(Taken::new("\u{c}\u{c}\u{c}".to_owned()))));
    assert_eq!(tab(&screen, key).pages(), Pages { count: 3, page: 1, drawn: true });
    let words = spelled(&screen.launch_command(key).expect("the first page is drawn"));
    assert_eq!(words[8], "1");

    apply(&mut screen, Msg::Page(key, None));
    assert!(tab(&screen, key).pages().drawn, "a scanned PDF has no text to show instead");
    let run = tab(&screen, key).run();
    apply(&mut screen, Msg::Output(key, run, TerminalEvent::Exited(Some(1))));
    let harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("The page could not be drawn"), "{text}");
    let top = text.lines().find(|line| line.contains("papers/tide tables.pdf")).unwrap_or_default();
    assert!(!top.contains("Text"), "no way back to a text there is not:\n{text}");
}

#[test]
fn a_pdf_that_cannot_be_read_says_so_in_the_engines_words_and_reads_again() {
    let scratch = with_pdf("pdf-failed");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    let failure = LaunchFailure {
        command: "podman exec".to_owned(),
        output: "Syntax Error: Couldn't find trailer".to_owned(),
        image_missing: false,
    };
    apply(&mut screen, Msg::TextRead(key, 0, Err(failure)));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    let text = harness.screen();
    assert!(text.contains("Couldn't find trailer"), "{text}");
    harness.click_text("Start again");
    let screen = &harness.app().0;
    let tab = tab(screen, key);
    assert_eq!(tab.run(), 1, "the text is taken out again");
    assert!(!tab.pages().drawn, "and it is the text again, not a page");
}

#[test]
fn an_answer_for_a_tab_that_moved_on_is_ignored() {
    let scratch = with_pdf("pdf-stale");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    apply(&mut screen, Msg::Restart(key));
    apply(&mut screen, Msg::TextRead(key, 0, Ok(Taken::new("old".to_owned()))));
    assert_eq!(tab(&screen, key).document(), None, "an answer of the run before is not the text of this one");
}

#[test]
fn a_very_long_text_is_cut_at_a_character_and_says_so() {
    let long = "ş".repeat(super::super::viewer::MOST_TEXT);
    let taken = Taken::new(format!("{long}\u{c}second\u{c}"));
    assert!(taken.cut);
    assert!(taken.text.len() <= super::super::viewer::MOST_TEXT);
    assert_eq!(taken.pages, 2, "the pages are counted before the text is cut");
    assert!(!Taken::new(TWO_PAGES.to_owned()).cut);

    let scratch = with_pdf("pdf-long");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    apply(&mut screen, Msg::TextRead(key, 0, Ok(taken)));
    assert!(tab(&screen, key).is_partial());
    let harness = harness(screen, SIZE.0, SIZE.1);
    assert!(harness.screen().contains("only its beginning is shown"), "{}", harness.screen());
}

#[test]
fn a_pdf_tab_is_kept_in_the_session_and_its_file_looked_for_when_it_comes_back() {
    let scratch = with_pdf("pdf-session");
    let mut screen = one_workspace(&scratch);
    open_pdf(&mut screen);
    let session = screen.session();
    assert_eq!(session.workspaces[0].tabs[0].kind, SessionTabKind::Pdf(PDF.to_owned()));
    let read = Session::parse("session.toml", &session.to_toml());
    assert!(read.is_clean(), "{:?}", read.diagnostics);
    assert_eq!(read.value, session);

    let mut again = one_workspace(&scratch);
    again.restore_tabs(0, &read.value.workspaces[0]);
    assert_eq!(state(&again, 0, 0), TabState::Waiting, "it waits to be shown");
    apply(&mut again, Msg::OpenTab(0));
    assert_eq!(state(&again, 0, 0), TabState::Starting, "shown, it takes its text out");

    let record = SessionWorkspace {
        id: WorkspaceId::parse("firefly").expect("a usable id"),
        active_tab: 0,
        tabs: vec![SessionTab {
            kind: SessionTabKind::Pdf("../outside.pdf".to_owned()),
            conversation: None,
            opened: 1,
        }],
    };
    let mut escaping = one_workspace(&scratch);
    escaping.restore_tabs(0, &record);
    assert!(kinds(&escaping).is_empty(), "a session file cannot name a PDF outside the workspace");

    // Gone from the disk, it says so rather than asking the container.
    fs::remove_file(scratch.paths().code.join(PDF)).expect("the PDF goes");
    let mut gone = one_workspace(&scratch);
    gone.restore_tabs(0, &read.value.workspaces[0]);
    let harness = harness(gone, SIZE.0, SIZE.1);
    assert_eq!(state(&harness.app().0, 0, 0), TabState::Missing);
    assert!(harness.screen().contains("tide tables.pdf is not in the workspace any more"), "{}", harness.screen());
}

#[test]
fn the_pdf_tab_draws_nothing_bracketed_nothing_but_ascii_in_ascii_mode_and_turkish_in_turkish() {
    let scratch = with_pdf("pdf-aesthetic");
    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    apply(&mut screen, Msg::TextRead(key, 0, Ok(Taken::new(TWO_PAGES.to_owned()))));
    let mut shown = harness(screen, SIZE.0, SIZE.1);
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        shown.set_glyph_mode(mode).render();
        let text = shown.screen();
        for forbidden in ['[', ']', '{', '}', '|', '┌', '─', '│'] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {mode:?}:\n{text}");
        }
    }
    assert!(shown.screen().is_ascii(), "{}", shown.screen());
    shown.set_glyph_mode(GlyphMode::Unicode).set_locale("tr").render();
    assert!(shown.screen().contains("Sayfa resmi"), "{}", shown.screen());

    let mut screen = one_workspace(&scratch);
    let key = open_pdf(&mut screen);
    apply(&mut screen, Msg::TextRead(key, 0, Ok(Taken::new(TWO_PAGES.to_owned()))));
    apply(&mut screen, Msg::Page(key, Some(1)));
    apply(&mut screen, Msg::Output(key, 1, TerminalEvent::Exited(Some(0))));
    let mut drawn = harness(screen, SIZE.0, SIZE.1);
    drawn.set_locale("tr").render();
    let text = drawn.screen();
    for words in ["Önceki", "Sayfa 1 / 2", "Sonraki", "Metin"] {
        assert!(text.contains(words), "`{words}`:\n{text}");
    }
}

/// A workspace folder with a Word document, an OpenDocument text and the office files that are
/// not opened.
fn with_office(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    let letters = scratch.paths().code.join("letters");
    fs::create_dir_all(&letters).expect("a folder for letters");
    for file in ["to the harbour master.docx", "Tide.ODT", "budget.xlsx", "talk.pptx", "sheet.ods", "talk.odp"] {
        fs::write(letters.join(file), b"PK").expect("an office file");
    }
    scratch
}

#[test]
fn a_word_document_opens_as_its_text_taken_out_in_the_workspaces_own_container() {
    let scratch = with_office("office-open");
    let mut screen = one_workspace(&scratch);
    apply(&mut screen, Msg::OpenFile("letters/to the harbour master.docx".to_owned()));
    apply(&mut screen, Msg::OpenFile("letters/Tide.ODT".to_owned()));
    assert_eq!(
        kinds(&screen),
        [
            TabKind::Office("letters/to the harbour master.docx".to_owned()),
            TabKind::Office("letters/Tide.ODT".to_owned())
        ]
    );
    let docx = spelled(&screen.read_command(key(&screen, 0)).expect("a document's text is read"));
    assert_eq!(
        docx,
        ["exec", "qcode-firefly-base", "docx2txt", &format!("{CODE_DIR}/letters/to the harbour master.docx"), "-"]
    );
    let odt = spelled(&screen.read_command(key(&screen, 1)).expect("a document's text is read"));
    assert_eq!(odt, ["exec", "qcode-firefly-base", "odt2txt", &format!("{CODE_DIR}/letters/Tide.ODT")]);
    assert!(screen.launch_command(key(&screen, 0)).is_none(), "the text is not a program in a terminal");

    let first = key(&screen, 0);
    let letter = "Dear harbour master,\n\nThe tide turns at six.\n";
    apply(&mut screen, Msg::OpenTab(0));
    apply(&mut screen, Msg::TextRead(first, 0, Ok(Taken::new(letter.to_owned()))));
    assert_eq!(entry(&screen), "workspace-document");
    let mut shown = harness(screen, SIZE.0, SIZE.1);
    let text = shown.screen();
    assert!(text.contains("Dear harbour master,") && text.contains("The tide turns at six."), "{text}");
    assert!(!text.contains("Page picture"), "a document has no pages to draw:\n{text}");
    shown.send(Msg::Page(first, Some(1))).render();
    assert!(!tab(&shown.app().0, first).pages().drawn, "and asking for one does nothing");
}

#[test]
fn a_document_with_no_text_says_so() {
    let scratch = with_office("office-empty");
    let mut screen = one_workspace(&scratch);
    apply(&mut screen, Msg::OpenFile("letters/Tide.ODT".to_owned()));
    let key = key(&screen, 0);
    apply(&mut screen, Msg::TextRead(key, 0, Ok(Taken::new("\n\n".to_owned()))));
    let mut shown = harness(screen, SIZE.0, SIZE.1);
    assert!(shown.screen().contains("There is no text in this file."), "{}", shown.screen());
    shown.set_locale("tr").render();
    assert!(shown.screen().contains("Bu dosyada metin yok."), "{}", shown.screen());
}

#[test]
fn spreadsheets_and_slides_are_still_not_opened() {
    let scratch = with_office("office-other");
    let mut shown = harness(one_workspace(&scratch), SIZE.0, SIZE.1);
    for file in ["budget.xlsx", "talk.pptx", "sheet.ods", "talk.odp"] {
        shown.send(Msg::OpenFile(format!("letters/{file}"))).advance(Duration::from_millis(400));
        assert!(shown.screen().contains("No built-in app opens this kind of file yet"), "{}", shown.screen());
    }
    assert!(kinds(&shown.app().0).is_empty());
}

#[test]
fn an_office_tab_comes_back_from_the_session() {
    let scratch = with_office("office-session");
    let mut screen = one_workspace(&scratch);
    apply(&mut screen, Msg::OpenFile("letters/Tide.ODT".to_owned()));
    let session = screen.session();
    assert_eq!(session.workspaces[0].tabs[0].kind, SessionTabKind::Office("letters/Tide.ODT".to_owned()));
    let read = Session::parse("session.toml", &session.to_toml());
    assert!(read.is_clean(), "{:?}", read.diagnostics);
    let mut again = one_workspace(&scratch);
    again.restore_tabs(0, &read.value.workspaces[0]);
    assert_eq!(kinds(&again), [TabKind::Office("letters/Tide.ODT".to_owned())]);
    assert_eq!(state(&again, 0, 0), TabState::Waiting);
}
