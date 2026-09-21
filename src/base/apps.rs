//! The built-in apps: the programs the base image carries for opening a workspace's files, which
//! file each of them opens, and the exact command line each is started with.
//!
//! A file opened from the file tree runs in a tab like every other program: inside the workspace's
//! own container, entered through the engine. This module only decides which program and which
//! words; the tab and the container belong to the workspace screen.

use std::path::{Path, PathBuf};

/// The editor a text file opens in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Editor {
    /// GNU nano: every key it needs is written at the bottom of its screen, so it is the one that
    /// asks nothing of a person who has never used a terminal editor.
    #[default]
    Nano,
    /// Vim, for the person who asked for it.
    Vim,
}

impl Editor {
    /// Every editor, in the order the settings offer them.
    pub const ALL: [Self; 2] = [Self::Nano, Self::Vim];

    /// How the editor is written in the settings file, which is also the program's name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Nano => "nano",
            Self::Vim => "vim",
        }
    }

    /// The editor a settings file's word names, if it names one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|editor| editor.key() == key)
    }

    /// What the tab runs to edit the file at `path`, a path inside the container.
    ///
    /// The path is one word of its own and never goes through a shell, so a file name with
    /// spaces, quotes or a `$` in it is the file's name and nothing more. It is always absolute,
    /// so a name that starts with a dash is never read as an option.
    #[must_use]
    pub fn command(self, path: &str) -> Vec<String> {
        vec![self.key().to_owned(), path.to_owned()]
    }
}

/// What opening a sound file does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sound {
    /// It plays: opening a sound file is asking to hear it.
    #[default]
    Play,
    /// Its details are shown and nothing plays, for the person who does not want the machine's
    /// sound server reachable from a container at all.
    Details,
}

impl Sound {
    /// Every choice, in the order the settings offer them.
    pub const ALL: [Self; 2] = [Self::Play, Self::Details];

    /// How the choice is written in the settings file.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Details => "details",
        }
    }

    /// The choice a settings file's word names, if it names one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sound| sound.key() == key)
    }
}

/// Where the sound server's socket appears inside the container that plays a sound.
pub const SOUND_SOCKET: &str = "/run/qcode/pulse/native";

/// The sound server's socket on this machine, when there is one: PulseAudio's, which PipeWire
/// serves too, in the runtime folder `runtime` (`$XDG_RUNTIME_DIR`).
///
/// Only a socket counts. On macOS and Windows there is no such folder, and on a Linux without a
/// sound server there is no such socket; either way nothing could carry the sound out of a
/// container, and the tab says so instead of starting a player that cannot be heard.
#[must_use]
pub fn sound_socket(runtime: Option<&Path>) -> Option<PathBuf> {
    let socket = runtime?.join("pulse").join("native");
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(&socket).ok().filter(|meta| meta.file_type().is_socket()).map(|_| socket)
    }
    #[cfg(not(unix))]
    {
        let _ = socket;
        None
    }
}

/// How an opened sound is met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hearing {
    /// It plays, through the socket at this path on this machine.
    Play(PathBuf),
    /// Its details are shown, for this reason.
    Details(Quiet),
}

/// Why a sound is not played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    /// The settings say to show details only.
    Chosen,
    /// This machine has no sound server a container could reach.
    NoServer,
}

/// How a sound is met, given the person's `choice` and the `socket` this machine has.
#[must_use]
pub fn hearing(choice: Sound, socket: Option<PathBuf>) -> Hearing {
    match (choice, socket) {
        (Sound::Details, _) => Hearing::Details(Quiet::Chosen),
        (Sound::Play, None) => Hearing::Details(Quiet::NoServer),
        (Sound::Play, Some(socket)) => Hearing::Play(socket),
    }
}

/// What the tab runs to play the sound at `path`, a path inside the container: sox's `play`,
/// which prints the file's name, its length and a line of progress while it plays, and stops at
/// `ctrl+c`.
#[must_use]
pub fn play(path: &str) -> Vec<String> {
    vec!["play".to_owned(), path.to_owned()]
}

/// What the tab runs to say what the sound at `path` is, when it does not play it: sox's
/// `soxi`, which reads the file's header and prints its length, rate, channels and format.
#[must_use]
pub fn sound_details(path: &str) -> Vec<String> {
    vec!["soxi".to_owned(), path.to_owned()]
}

/// What the tab runs to draw the image at `path`, a path inside the container.
///
/// The tab's terminal shows no kitty or sixel pictures, so the image is drawn in character cells
/// with full colour. The size is the terminal's own: the tab gives its terminal exactly its own
/// area, both engines hand that size to the program as it starts, and chafa fits the picture
/// into it and keeps a row free at the bottom so its last line does not scroll the top away.
/// Animation is off, because a picture that keeps drawing never ends and a tab only settles on
/// what is there once it has; polite mode leaves out the terminal queries a drawn picture does
/// not need.
#[must_use]
pub fn picture(path: &str) -> Vec<String> {
    let mut command = vec!["chafa".to_owned()];
    command.extend(CHAFA.iter().map(|option| (*option).to_owned()));
    command.push(path.to_owned());
    command
}

/// How chafa is asked to draw, for a picture and for a page of a PDF alike.
const CHAFA: [&str; 6] = [
    "--format=symbols",
    "--colors=full",
    // Wide symbols would put two cells' worth of picture where the grid expects one.
    "--symbols=block+border+space-wide",
    "--animate=off",
    "--polite=on",
    "--scale=max",
];

/// What the tab runs to take the text out of the PDF at `path`, a path inside the container.
///
/// `-layout` keeps the columns and the indentation of the page, so a table or a two-column page
/// reads the way it looks rather than in the order the file happens to store its words. The text
/// goes to standard output, where the tab reads it; every page ends in a form feed, which is how
/// the pages are counted.
#[must_use]
pub fn pdf_text(path: &str) -> Vec<String> {
    ["pdftotext", "-layout", path, "-"].into_iter().map(str::to_owned).collect()
}

/// What the tab runs to draw page `page` (counted from 1) of the PDF at `path`, a path inside the
/// container.
///
/// poppler renders the page to a PNG on standard output and chafa draws that the way [`picture`]
/// draws a picture. Two programs piped into each other need a shell to join them, and that is
/// all the shell does: its script is the same for every file and every page, and the page (`$1`)
/// and the path (`$2`) reach it as positional arguments, so no shell ever reads the file's name
/// as part of a command.
#[must_use]
pub fn pdf_page(path: &str, page: usize) -> Vec<String> {
    let script = format!("pdftoppm -f \"$1\" -l \"$1\" -r 110 -png \"$2\" | chafa {} -", CHAFA.join(" "));
    vec!["sh".to_owned(), "-c".to_owned(), script, "sh".to_owned(), page.to_string(), path.to_owned()]
}

/// How many pages the text [`pdf_text`] printed has: one form feed ends each page.
#[must_use]
pub fn pages(text: &str) -> usize {
    text.matches('\u{c}').count().max(1)
}

/// Whether the text taken out of a document has nothing in it to read: a scanned PDF is pictures
/// of pages, and its text is nothing but the page breaks, which count as white space.
#[must_use]
pub fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// What the tab runs to take the text out of the office document at `path`, a path inside the
/// container: docx2txt for Word's `.docx`, odt2txt for OpenDocument's `.odt`. `None` for any
/// other file.
///
/// Both print the text to standard output, the paragraphs one after another; the formatting is
/// left behind, which is what showing a document as text means.
#[must_use]
pub fn office_text(path: &str) -> Option<Vec<String>> {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase())?;
    let command: &[&str] = match extension.as_str() {
        "docx" => &["docx2txt", path, "-"],
        "odt" => &["odt2txt", path],
        _ => return None,
    };
    Some(command.iter().map(|word| (*word).to_owned()).collect())
}

/// Which built-in app opens a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A picture, drawn by chafa in the container.
    Image,
    /// A Markdown document, shown by QCode itself.
    Markdown,
    /// Text, opened in the chosen editor in the container.
    Text,
    /// A PDF: its text shown by QCode, its pages drawn by poppler and chafa in the container.
    Pdf,
    /// A word processor's document, `.docx` or `.odt`: its text shown by QCode.
    Office,
    /// A sound, played by sox in a container of its own, or described by it.
    Sound,
    /// Nothing built in opens it yet: spreadsheets, slides, archives and anything unknown.
    Unknown,
}

/// The extensions chafa is given; the image decoders the base image carries read all of them.
///
/// BMP is not among them: the chafa the image carries has no decoder for it and answers
/// "Unknown file format", so a bitmap is told that nothing opens it yet rather than opened into
/// an error.
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// The extensions of the office documents whose text is shown: a spreadsheet or a slide show
/// read as plain text is a heap of cells and captions, so those are not among them.
const OFFICE: &[&str] = &["docx", "odt"];

/// The extensions of the sounds sox reads with the formats the image carries. AAC and M4A are
/// not among them: sox reads them only through ffmpeg, which is several hundred megabytes.
const SOUNDS: &[&str] = &["mp3", "ogg", "oga", "opus", "flac", "wav"];

/// The extensions of Markdown.
const MARKDOWN: &[&str] = &["md", "markdown"];

/// The extensions of files that are text to be read and changed in an editor: prose, data,
/// configuration and the source code of the common languages.
const TEXT: &[&str] = &[
    "txt",
    "text",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonc",
    "json5",
    "toml",
    "yaml",
    "yml",
    "xml",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "lock",
    "rs",
    "py",
    "js",
    "mjs",
    "cjs",
    "ts",
    "mts",
    "tsx",
    "jsx",
    "go",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "java",
    "kt",
    "kts",
    "swift",
    "rb",
    "php",
    "pl",
    "lua",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "sql",
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "vue",
    "svelte",
    "zig",
    "nix",
    "dart",
    "ex",
    "exs",
    "erl",
    "hs",
    "ml",
    "scala",
    "clj",
    "el",
    "vim",
    "r",
    "jl",
    "tex",
    "rst",
    "adoc",
    "org",
    "diff",
    "patch",
    "gradle",
    "cmake",
    "mk",
    "gitignore",
    "gitattributes",
    "editorconfig",
    "dockerignore",
];

/// Names of text files that carry no extension, or whose extension says nothing about them.
const TEXT_NAMES: &[&str] = &[
    "README",
    "LICENSE",
    "LICENCE",
    "COPYING",
    "NOTICE",
    "AUTHORS",
    "CONTRIBUTORS",
    "CHANGELOG",
    "CHANGES",
    "TODO",
    "Makefile",
    "GNUmakefile",
    "Dockerfile",
    "Containerfile",
    "Justfile",
    "justfile",
    "Procfile",
    "Gemfile",
    "Rakefile",
    "Vagrantfile",
    "CODEOWNERS",
    ".env",
];

/// Which built-in app opens the file called `name`.
///
/// The name alone decides, so nothing is read from the disk to decide it. Extensions are
/// compared without regard to case, because `PHOTO.JPG` is as much a picture as `photo.jpg`; the
/// names without an extension are compared as they are written, the way they are conventionally
/// spelled.
#[must_use]
pub fn classify(name: &str) -> FileKind {
    if TEXT_NAMES.contains(&name) {
        return FileKind::Text;
    }
    // A leading dot starts a hidden file's name rather than an extension: `.gitignore` is a file
    // called gitignore, hidden.
    let bare = name.strip_prefix('.').unwrap_or(name);
    let Some((_, extension)) = bare.rsplit_once('.').or_else(|| (bare != name).then_some(("", bare))) else {
        return FileKind::Unknown;
    };
    let extension = extension.to_ascii_lowercase();
    if IMAGES.contains(&extension.as_str()) {
        FileKind::Image
    } else if MARKDOWN.contains(&extension.as_str()) {
        FileKind::Markdown
    } else if TEXT.contains(&extension.as_str()) {
        FileKind::Text
    } else if extension == "pdf" {
        FileKind::Pdf
    } else if OFFICE.contains(&extension.as_str()) {
        FileKind::Office
    } else if SOUNDS.contains(&extension.as_str()) {
        FileKind::Sound
    } else {
        FileKind::Unknown
    }
}

/// Every program the built-in apps run, and the archive tools beside them, each with the
/// arguments that make it say its version and nothing else. The base image has to answer every
/// one of them, which its live test asks.
pub const PROGRAMS: &[&[&str]] = &[
    &["nano", "--version"],
    &["vim", "--version"],
    &["chafa", "--version"],
    &["unzip", "-v"],
    &["zip", "-v"],
    &["xz", "--version"],
    &["bzip2", "--help"],
    &["7z", "i"],
    &["pdftotext", "-v"],
    &["pdftoppm", "-v"],
    // docx2txt has no way to say its version and nothing else, so it is only looked for.
    &["sh", "-c", "command -v docx2txt"],
    &["odt2txt", "--version"],
    &["play", "--version"],
    // soxi says its version but ends with a failure, taking `--version` for a file option it does
    // not know, so it is only looked for, like docx2txt.
    &["sh", "-c", "command -v soxi"],
];

#[cfg(test)]
mod tests {
    use super::{
        Editor, FileKind, Hearing, PROGRAMS, Quiet, Sound, classify, hearing, is_blank, office_text, pages, pdf_page,
        pdf_text, picture, play, sound_details, sound_socket,
    };
    use crate::base::CONTAINERFILE;

    #[test]
    fn a_file_is_opened_by_what_its_name_says_it_is() {
        for name in ["photo.png", "PHOTO.JPG", "a.b.jpeg", "anim.gif", "x.webp"] {
            assert_eq!(classify(name), FileKind::Image, "{name}");
        }
        for name in ["README.md", "notes.Markdown"] {
            assert_eq!(classify(name), FileKind::Markdown, "{name}");
        }
        for name in [
            "notes.txt",
            "main.rs",
            "Cargo.toml",
            "Cargo.lock",
            "README",
            "LICENSE",
            "Makefile",
            "Dockerfile",
            ".gitignore",
            ".env",
            "script.SH",
        ] {
            assert_eq!(classify(name), FileKind::Text, "{name}");
        }
        for name in ["paper.pdf", "SCAN.PDF"] {
            assert_eq!(classify(name), FileKind::Pdf, "{name}");
        }
        for name in ["report.docx", "Letter.ODT"] {
            assert_eq!(classify(name), FileKind::Office, "{name}");
        }
        for name in ["song.mp3", "a.ogg", "b.OGA", "c.opus", "d.flac", "e.wav"] {
            assert_eq!(classify(name), FileKind::Sound, "{name}");
        }
        for name in [
            "old.bmp",
            "song.m4a",
            "voice.aac",
            "budget.xlsx",
            "slides.pptx",
            "sheet.ods",
            "talk.odp",
            "old.doc",
            "bundle.zip",
            "binary",
            "archive.tar.gz",
            ".hidden",
            "",
        ] {
            assert_eq!(classify(name), FileKind::Unknown, "{name}");
        }
    }

    #[test]
    fn the_editor_opens_the_path_as_one_word() {
        let path = "/work/a file with 'quotes' and $HOME.txt";
        assert_eq!(Editor::Nano.command(path), ["nano", path]);
        assert_eq!(Editor::Vim.command(path), ["vim", path]);
        assert_eq!(Editor::default(), Editor::Nano, "nano is the editor nobody has to choose");
        for editor in Editor::ALL {
            assert_eq!(Editor::from_key(editor.key()), Some(editor));
        }
        assert_eq!(Editor::from_key("emacs"), None);
    }

    #[test]
    fn a_picture_is_drawn_once_in_character_cells_and_the_path_is_the_last_word() {
        let path = "/work/-looks like an option.png";
        let command = picture(path);
        assert_eq!(command.first().map(String::as_str), Some("chafa"));
        assert_eq!(command.last().map(String::as_str), Some(path));
        assert_eq!(command.iter().filter(|word| word.as_str() == path).count(), 1);
        for option in ["--format=symbols", "--animate=off", "--colors=full"] {
            assert!(command.iter().any(|word| word == option), "{option} in {command:?}");
        }
    }

    #[test]
    fn a_pdf_gives_its_text_with_the_layout_to_standard_output() {
        let path = "/work/a paper; rm -rf $HOME.pdf";
        assert_eq!(pdf_text(path), ["pdftotext", "-layout", path, "-"]);
    }

    #[test]
    fn an_office_document_gives_its_text_to_standard_output_with_the_path_as_one_word() {
        let path = "/work/it's a $(report).docx";
        assert_eq!(office_text(path), Some(vec!["docx2txt".to_owned(), path.to_owned(), "-".to_owned()]));
        let path = "/work/Letter.ODT";
        assert_eq!(office_text(path), Some(vec!["odt2txt".to_owned(), path.to_owned()]));
        assert_eq!(office_text("/work/budget.xlsx"), None);
        assert_eq!(office_text("/work/docx"), None);
    }

    #[test]
    fn a_sound_plays_or_is_described_with_the_path_as_one_word() {
        let path = "/work/it's a $(song).mp3";
        assert_eq!(play(path), ["play", path]);
        assert_eq!(sound_details(path), ["soxi", path]);
    }

    #[test]
    fn a_sound_plays_only_when_the_setting_says_so_and_there_is_a_server() {
        let socket = std::path::PathBuf::from("/run/user/1000/pulse/native");
        assert_eq!(hearing(Sound::Play, Some(socket.clone())), Hearing::Play(socket.clone()));
        assert_eq!(hearing(Sound::Play, None), Hearing::Details(Quiet::NoServer));
        assert_eq!(hearing(Sound::Details, Some(socket)), Hearing::Details(Quiet::Chosen));
        assert_eq!(hearing(Sound::Details, None), Hearing::Details(Quiet::Chosen));
        assert_eq!(Sound::default(), Sound::Play, "opening a sound is asking to hear it");
        for sound in Sound::ALL {
            assert_eq!(Sound::from_key(sound.key()), Some(sound));
        }
        assert_eq!(Sound::from_key("loud"), None);
    }

    #[cfg(unix)]
    #[test]
    fn only_a_socket_in_the_runtime_folder_is_a_sound_server() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let runtime = std::env::temp_dir().join(format!("qcode-sound-{}-{stamp}", std::process::id()));
        let socket = runtime.join("pulse").join("native");
        std::fs::create_dir_all(runtime.join("pulse")).expect("a runtime folder");
        assert_eq!(sound_socket(None), None, "no runtime folder, no server");
        assert_eq!(sound_socket(Some(&runtime)), None, "an empty folder has no server");
        std::fs::write(&socket, "").expect("a plain file");
        assert_eq!(sound_socket(Some(&runtime)), None, "a plain file is no server");
        std::fs::remove_file(&socket).expect("the plain file goes");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("a socket");
        assert_eq!(sound_socket(Some(&runtime)), Some(socket));
        let _ = std::fs::remove_dir_all(&runtime);
    }

    #[test]
    fn a_page_is_drawn_by_a_script_that_never_changes_and_takes_the_page_and_path_as_arguments() {
        let path = "/work/it's \"$(quoted)\".pdf";
        let first = pdf_page(path, 3);
        assert_eq!(first[..2], ["sh", "-c"]);
        assert_eq!(first[3..], ["sh", "3", path], "the page and the path are arguments, each one word");
        let script = &first[2];
        assert!(!script.contains("quoted"), "the file's name is never part of the script: {script}");
        assert_eq!(pdf_page("/work/other.pdf", 9)[2], *script, "the script is the same for every file");
        assert!(script.starts_with("pdftoppm -f \"$1\" -l \"$1\" "), "{script}");
        // The page is drawn exactly the way a picture is.
        let drawn = picture("x");
        assert!(script.ends_with(&format!("| chafa {} -", drawn[1..drawn.len() - 1].join(" "))), "{script}");
    }

    #[test]
    fn a_pdf_counts_its_pages_by_their_breaks_and_a_scanned_one_has_no_text() {
        assert_eq!(pages("Harbour notes\n\u{c}Second page\n\u{c}"), 2);
        assert_eq!(pages("one page without a break"), 1, "a document always has a page");
        assert!(is_blank("\u{c}\u{c}\n  \u{c}"), "a scanned PDF is page breaks and nothing else");
        assert!(!is_blank("\u{c}words\u{c}"));
    }

    #[test]
    fn the_image_installs_every_program_the_built_in_apps_run() {
        // An instruction may go on over several lines, each ending in a backslash; read as one.
        let instructions = CONTAINERFILE.replace("\\\n", " ");
        let installs: Vec<&str> = instructions.lines().filter(|line| line.contains("apt-get install")).collect();
        // The package of each program, as the Containerfile's install line names it.
        for package in [
            "nano",
            "vim",
            "chafa",
            "unzip",
            "zip",
            "xz-utils",
            "bzip2",
            "7zip",
            "poppler-utils",
            "docx2txt",
            "odt2txt",
            "sox",
            "libsox-fmt-mp3",
            "libsox-fmt-opus",
            "libsox-fmt-pulse",
        ] {
            assert!(
                installs.iter().any(|line| line.split_whitespace().any(|word| word == package)),
                "the image installs no {package}"
            );
        }
        assert_eq!(PROGRAMS.len(), 14, "every program the built-in apps run answers for its package");
        // vim-tiny has no syntax colours, which is what a person choosing vim expects.
        assert!(installs.iter().all(|line| !line.contains("vim-tiny")), "the image installs the small vim");
    }
}
