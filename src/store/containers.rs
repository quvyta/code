//! `containers.toml`: the containers QCode itself started, so that once no QCode is open the ones
//! still running can be stopped.
//!
//! The file is state, not a setting: it lives in the application's data folder beside the
//! session, and QCode writes a name into it the moment it starts that container. Only QCode's own
//! containers are ever written or acted on — a name without QCode's prefix is refused on the way
//! in and reported on the way out — so a container the person or another program runs is never
//! touched. A file that cannot be read cleanly means "stop nothing": guessing which containers a
//! damaged list meant is how the wrong one gets stopped.
//!
//! ```toml
//! [[container]]
//! name = "qcode-firefly-base"
//! engine = "podman"
//!
//! [[container]]
//! name = "qcode-firefly-claude-sub"
//! engine = "docker"
//! ```

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, Table, ValueKind};
use qframe::storage::{atomic_write, data_dir};

use super::Loaded;
use super::workspace::quoted;
use crate::engine::EngineKind;

/// The folder of the application's own data, the same one the session is kept in.
const APP: &str = "quvyta/code";

/// The name of the file in that folder.
const FILE: &str = "containers.toml";

/// What every container QCode creates is called by, and the one thing that lets a name into the
/// list: [`crate::engine::names`] builds every container name on it.
pub const PREFIX: &str = "qcode-";

/// One process may start several containers at once, each on its own thread; each of them reads
/// the list, adds its name and writes it back. Taking turns inside the process keeps one of those
/// writes from undoing another.
static WRITING: Mutex<()> = Mutex::new(());

/// A container QCode started, and the engine it started it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    /// The container's name, always with [`PREFIX`].
    pub name: String,
    /// The engine it lives in.
    pub engine: EngineKind,
}

/// The containers QCode started, in the order it first started them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    /// Every container, each name once.
    pub containers: Vec<Registered>,
}

impl Registry {
    /// Where the list of this user is kept: `containers.toml` in QCode's data folder, or `None`
    /// on a machine that names no home, where nothing is recorded and so nothing is stopped.
    #[must_use]
    pub fn file() -> Option<PathBuf> {
        data_dir(APP).map(|dir| dir.join(FILE))
    }

    /// Whether the list holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.containers.is_empty()
    }

    /// Adds `name` in `engine`, unless it is there already or is not one of QCode's names.
    /// Answers whether the list changed.
    pub fn add(&mut self, name: &str, engine: EngineKind) -> bool {
        if !name.starts_with(PREFIX) || self.containers.iter().any(|known| known.name == name && known.engine == engine)
        {
            return false;
        }
        self.containers.push(Registered { name: name.to_owned(), engine });
        true
    }

    /// Reads the list at `path`.
    ///
    /// A file that is not there is an empty list: nothing was started yet, or everything was
    /// stopped. A file that cannot be read is an empty list with a diagnostic that says why.
    #[must_use]
    pub fn load(path: &Path) -> Loaded<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or(FILE);
                Self::parse(name, &text)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Loaded { value: Self::default(), diagnostics: Vec::new() }
            }
            Err(error) => Loaded {
                value: Self::default(),
                diagnostics: vec![Diagnostic::error(None, format!("{} could not be read: {error}", path.display()))],
            },
        }
    }

    /// Reads a list from `text`, reporting problems against `file`.
    ///
    /// An entry that is not one of QCode's containers, or names no engine QCode drives, is left
    /// out with a diagnostic; a name listed twice is kept once.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> Loaded<Self> {
        let document = Document::parse(file, text, &shape());
        let mut diagnostics = document.diagnostics().to_vec();
        let mut registry = Self::default();
        for entry in document.root().entries("container") {
            if let Some((name, engine)) = registered(entry, &mut diagnostics) {
                registry.add(&name, engine);
            }
        }
        Loaded { value: registry, diagnostics }
    }

    /// The file as it is written to disk.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        for (index, container) in self.containers.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let _ = write!(
                out,
                "[[container]]\nname = {}\nengine = {}\n",
                quoted(&container.name),
                quoted(container.engine.name())
            );
        }
        out
    }

    /// Writes the list to `path` atomically, making its folder first when it is not there.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the folder or the file cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        atomic_write(path, self.to_toml().as_bytes())
    }

    /// Notes in the list at `path` that QCode started the container `name` in `engine`, and
    /// answers what was wrong with the file it found there.
    ///
    /// The file is only written when the name is new, so the tab opened for the hundredth time
    /// costs a read and nothing more, and a watcher of the file is not woken for nothing. A file
    /// that was damaged is written again with what could be read of it and the new name: the
    /// container that just started would otherwise never be stopped, and the problems are
    /// answered so the person hears of them. A name that is not QCode's is not written at all.
    ///
    /// This reads and writes the disk, so it belongs on a background thread.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the file cannot be written.
    pub fn record(path: &Path, name: &str, engine: EngineKind) -> io::Result<Vec<Diagnostic>> {
        let _turn = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Loaded { value: mut registry, diagnostics } = Self::load(path);
        if registry.add(name, engine) || !diagnostics.is_empty() {
            registry.save(path)?;
        }
        Ok(diagnostics)
    }

    /// Writes `remaining` to the list at `path` in place of what it held, which is what stopping
    /// the containers does once it is through: only what could not be stopped stays listed.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the file cannot be written.
    pub fn replace(path: &Path, remaining: &Self) -> io::Result<()> {
        let _turn = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        remaining.save(path)
    }
}

/// What a `containers.toml` holds.
fn shape() -> Shape {
    let names: Vec<&str> = [EngineKind::Podman, EngineKind::Docker].map(EngineKind::name).to_vec();
    let container = Shape::new().required("name", ValueKind::text()).required("engine", ValueKind::choice(names));
    Shape::new().entries("container", container)
}

/// One `[[container]]` entry, or `None` when it is not a container QCode may act on.
fn registered(entry: &Table, diagnostics: &mut Vec<Diagnostic>) -> Option<(String, EngineKind)> {
    // A missing name or engine, or an engine word nobody knows, was reported by the document.
    let name = entry.text("name")?;
    let engine = EngineKind::from_name(entry.text("engine")?)?;
    if !name.starts_with(PREFIX) {
        let at = entry.value_location("name").cloned();
        diagnostics.push(Diagnostic::warning(at, format!("`{name}` is not a QCode container; it is left alone")));
        return None;
    }
    Some((name.to_owned(), engine))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "containers.toml";

    fn located(diagnostic: &Diagnostic) -> String {
        diagnostic.location.as_ref().map_or_else(|| "nowhere".to_owned(), ToString::to_string)
    }

    /// A folder of this test's own, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("qcode-registry-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("deeper").join(NAME)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample() -> Registry {
        let mut registry = Registry::default();
        registry.add("qcode-firefly-base", EngineKind::Podman);
        registry.add("qcode-firefly-claude \"sub\"", EngineKind::Docker);
        registry
    }

    #[test]
    fn a_written_list_reads_back_the_same() {
        let text = sample().to_toml();
        assert_eq!(
            text,
            "[[container]]\nname = \"qcode-firefly-base\"\nengine = \"podman\"\n\n\
             [[container]]\nname = \"qcode-firefly-claude \\\"sub\\\"\"\nengine = \"docker\"\n"
        );
        let read = Registry::parse(NAME, &text);
        assert!(read.is_clean(), "{:?}\n{text}", read.diagnostics);
        assert_eq!(read.value, sample());
        assert_eq!(Registry::default().to_toml(), "", "an empty list is an empty file");
    }

    #[test]
    fn recording_makes_the_folder_and_keeps_each_name_once() {
        let scratch = Scratch::new("record");
        let path = scratch.file();
        assert_eq!(Registry::load(&path), Loaded { value: Registry::default(), diagnostics: Vec::new() });
        for _ in 0..3 {
            let problems = Registry::record(&path, "qcode-moth-base", EngineKind::Podman).expect("written");
            assert!(problems.is_empty(), "{problems:?}");
        }
        Registry::record(&path, "qcode-moth-codex", EngineKind::Docker).expect("written");
        let read = Registry::load(&path);
        assert!(read.is_clean(), "{:?}", read.diagnostics);
        let names: Vec<&str> = read.value.containers.iter().map(|known| known.name.as_str()).collect();
        assert_eq!(names, ["qcode-moth-base", "qcode-moth-codex"]);
        assert_eq!(read.value.containers[1].engine, EngineKind::Docker);
    }

    #[test]
    fn a_name_already_listed_is_not_written_again() {
        // A watcher of the file wakes on every write; opening a tab again is not news.
        let scratch = Scratch::new("unchanged");
        let path = scratch.file();
        Registry::record(&path, "qcode-moth-base", EngineKind::Podman).expect("written");
        let before = std::fs::metadata(&path).and_then(|meta| meta.modified()).expect("a time");
        std::thread::sleep(std::time::Duration::from_millis(20));
        Registry::record(&path, "qcode-moth-base", EngineKind::Podman).expect("read");
        let after = std::fs::metadata(&path).and_then(|meta| meta.modified()).expect("a time");
        assert_eq!(before, after);
    }

    #[test]
    fn only_qcode_names_are_ever_written() {
        let scratch = Scratch::new("foreign");
        let path = scratch.file();
        Registry::record(&path, "postgres", EngineKind::Podman).expect("nothing to write");
        assert!(!path.exists(), "a name that is not QCode's does not even make the file");
        let mut registry = Registry::default();
        assert!(!registry.add("my-qcode-thing", EngineKind::Docker));
        assert!(registry.is_empty());
    }

    #[test]
    fn many_threads_recording_at_once_lose_no_name() {
        let scratch = Scratch::new("threads");
        let path = scratch.file();
        let handles: Vec<_> = (0..8)
            .map(|number| {
                let path = path.clone();
                std::thread::spawn(move || {
                    Registry::record(&path, &format!("qcode-p-{number}"), EngineKind::Podman).expect("written");
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("the thread finishes");
        }
        assert_eq!(Registry::load(&path).value.containers.len(), 8);
    }

    #[test]
    fn a_foreign_name_or_an_unknown_engine_in_the_file_is_reported_with_its_place() {
        let text = "[[container]]\nname = \"postgres\"\nengine = \"podman\"\n\n\
                    [[container]]\nname = \"qcode-a-base\"\nengine = \"lxc\"\n\n\
                    [[container]]\nname = \"qcode-b-base\"\nengine = \"docker\"\n\n\
                    [[container]]\nname = \"qcode-b-base\"\nengine = \"docker\"\n";
        let read = Registry::parse(NAME, text);
        let places: Vec<String> = read.diagnostics.iter().map(located).collect();
        for place in ["containers.toml:2:8", "containers.toml:7:10"] {
            assert!(places.iter().any(|at| at == place), "{place} in {places:?}");
        }
        assert_eq!(read.value.containers, [Registered { name: "qcode-b-base".to_owned(), engine: EngineKind::Docker }]);
    }

    #[test]
    fn a_broken_file_says_where_and_recording_writes_back_what_could_be_read() {
        let scratch = Scratch::new("broken");
        let path = scratch.file();
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the folder");
        std::fs::write(
            &path,
            "[[container]]\nname = \"qcode-a-base\"\nengine = \"podman\"\n\n[[container]]\nname = \n",
        )
        .expect("a broken file");
        let read = Registry::load(&path);
        assert!(
            read.diagnostics.iter().any(|d| located(d).starts_with("containers.toml:6:")),
            "{:?}",
            read.diagnostics
        );

        let problems = Registry::record(&path, "qcode-b-base", EngineKind::Podman).expect("written");
        assert!(!problems.is_empty(), "the damage is answered so it can be said");
        let read = Registry::load(&path);
        assert!(read.is_clean(), "the file is whole again: {:?}", read.diagnostics);
        assert_eq!(read.value.containers.len(), 2);
    }

    #[test]
    fn an_unreadable_file_is_an_empty_list_that_says_why() {
        let scratch = Scratch::new("folder");
        std::fs::create_dir_all(&scratch.0).expect("a folder");
        let read = Registry::load(&scratch.0);
        assert!(read.value.is_empty());
        assert_eq!(read.diagnostics.len(), 1, "{:?}", read.diagnostics);
    }

    #[test]
    fn garbage_never_panics() {
        for text in ["", "\u{0}", "[[container]]\n", "container = 3\n", "[[container]]]]\nname=\n", "name = 7"] {
            let read = Registry::parse(NAME, text);
            assert!(read.value.containers.iter().all(|known| known.name.starts_with(PREFIX)));
        }
    }
}
