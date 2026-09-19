//! The built-in apps: the programs the base image carries for opening a project's files, which
//! file each of them opens, and the exact command line each is started with.
//!
//! A file opened from the file tree runs in a tab like every other program: inside the project's
//! own container, entered through the engine. This module only decides which program and which
//! words; the tab and the container belong to the project screen.

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
    [
        "chafa",
        "--format=symbols",
        "--colors=full",
        // Wide symbols would put two cells' worth of picture where the grid expects one.
        "--symbols=block+border+space-wide",
        "--animate=off",
        "--polite=on",
        "--scale=max",
        path,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
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
    /// Nothing built in opens it yet: sound, PDF, office documents, archives and anything unknown.
    Unknown,
}

/// The extensions chafa is given; the image decoders the base image carries read all of them.
///
/// BMP is not among them: the chafa the image carries has no decoder for it and answers
/// "Unknown file format", so a bitmap is told that nothing opens it yet rather than opened into
/// an error.
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

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
];

#[cfg(test)]
mod tests {
    use super::{Editor, FileKind, PROGRAMS, classify, picture};
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
        for name in
            ["old.bmp", "song.mp3", "paper.pdf", "report.docx", "bundle.zip", "binary", "archive.tar.gz", ".hidden", ""]
        {
            assert_eq!(classify(name), FileKind::Unknown, "{name}");
        }
    }

    #[test]
    fn the_editor_opens_the_path_as_one_word() {
        let path = "/work/Project/a file with 'quotes' and $HOME.txt";
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
        let path = "/work/Project/-looks like an option.png";
        let command = picture(path);
        assert_eq!(command.first().map(String::as_str), Some("chafa"));
        assert_eq!(command.last().map(String::as_str), Some(path));
        assert_eq!(command.iter().filter(|word| word.as_str() == path).count(), 1);
        for option in ["--format=symbols", "--animate=off", "--colors=full"] {
            assert!(command.iter().any(|word| word == option), "{option} in {command:?}");
        }
    }

    #[test]
    fn the_image_installs_every_program_the_built_in_apps_run() {
        // The package of each program, as the Containerfile's install line names it.
        for package in ["nano", "vim", "chafa", "unzip", "zip", "xz-utils", "bzip2", "7zip"] {
            assert!(
                CONTAINERFILE
                    .lines()
                    .any(|line| line.contains("apt-get install") && line.contains(&format!(" {package}"))),
                "the image installs no {package}"
            );
        }
        assert_eq!(PROGRAMS.len(), 8, "every package has one program that answers for it");
        // vim-tiny has no syntax colours, which is what a person choosing vim expects.
        let installs = CONTAINERFILE.lines().filter(|line| line.contains("apt-get install"));
        assert!(installs.clone().all(|line| !line.contains("vim-tiny")), "the image installs the small vim");
        assert!(installs.clone().any(|line| line.contains(" vim ")), "the image installs no full vim");
    }
}
