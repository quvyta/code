//! `providers.toml`: the one file a key is written into, and the checks made every time it is
//! read.
//!
//! It is not the settings file and it never will be. Settings are copied between machines, sent
//! to someone who is helping, and pasted into a report; a key that lived there would travel with
//! them. This file lives in QCode's own data folder, it is written with mode 600 in a folder of
//! mode 700, and how wide its permissions really are is checked on every read and said out loud
//! when they are wider than that.
//!
//! Nothing here repairs the file and nothing panics over it. A provider that cannot be read is
//! reported with the line and column where it went wrong and left out of the list; the providers
//! around it are still returned.

use std::path::{Path, PathBuf};

use qframe::diagnostics::{Diagnostic, Location};
use qframe::document::{Document, Shape, ValueKind};
use qframe::storage::data_dir;

use crate::store::Loaded;

use super::{Key, Measured, Model, ProviderEntry, ProviderKind, Tag, Wire, record::trim_base};

/// QCode's folder under the platform's data directory: `<XDG_DATA_HOME>/quvyta/code` on Linux.
const APP: &str = "quvyta/code";

/// The file itself, under that folder.
const FILE_NAME: &str = "providers.toml";

/// The mode the folder is created with and checked against.
#[cfg(unix)]
const FOLDER_MODE: u32 = 0o700;

/// The mode the file is created with and checked against.
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// The key of the array of providers.
const PROVIDER: &str = "provider";
/// The key of the array of models, which is flat and names its provider, so that a model of a
/// provider that could not be read is simply left where it is.
const MODEL: &str = "model";
/// The key of a provider's tag, and of the tag a model belongs to.
const TAG: &str = "tag";
/// The key of a provider's kind.
const KIND: &str = "kind";
/// The key of a provider's base address.
const BASE: &str = "base";
/// The key of a provider's wire format.
const WIRE: &str = "wire";
/// The key of a provider's key.
const KEY: &str = "key";
/// The key of a model's name.
const ID: &str = "id";
/// The key of the window a model's own record claims.
const CLAIMED: &str = "claimed-context";
/// The key of the window the server was seen to accept.
const MEASURED: &str = "measured-context";
/// The key of how that measurement ended: at the window's edge, or still growing.
const MEASURED_KIND: &str = "measured";

/// The providers a person has added, and where they are kept.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Providers {
    path: Option<PathBuf>,
    entries: Vec<ProviderEntry>,
}

/// Why a provider could not be added or renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    /// A provider of this tag is already there. Two providers of one tag would make the tag in
    /// front of a model name mean two different machines.
    Taken(String),
}

impl Providers {
    /// Providers that live only in memory; saving does nothing. For tests, and for a machine
    /// with no data folder at all.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Where the providers file goes on this machine, or `None` on one whose data folder cannot
    /// be worked out.
    #[must_use]
    pub fn file() -> Option<PathBuf> {
        data_dir(APP).map(|dir| dir.join(FILE_NAME))
    }

    /// Reads the providers of this machine, with everything that was wrong with the file. How
    /// wide its permissions are is asked separately, of [`permission_problems`].
    #[must_use]
    pub fn load() -> Loaded<Self> {
        match Self::file() {
            Some(path) => Self::open(&path),
            None => Loaded { value: Self::in_memory(), diagnostics: Vec::new() },
        }
    }

    /// Reads the providers at `path`. A file that is not there yet is an empty list, not a
    /// problem: nobody has added a provider.
    #[must_use]
    pub fn open(path: &Path) -> Loaded<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                let diagnostics = vec![Diagnostic::error(None, format!("{}: {error}", path.display()))];
                return Loaded { value: Self { path: Some(path.to_path_buf()), entries: Vec::new() }, diagnostics };
            }
        };
        let name = path.file_name().map_or_else(|| FILE_NAME.to_owned(), |name| name.to_string_lossy().into_owned());
        let mut loaded = Self::parse(&name, &text);
        loaded.value.path = Some(path.to_path_buf());
        loaded
    }

    /// Reads providers from TOML `text`, reporting problems against `file`. Saving does nothing.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> Loaded<Self> {
        let document = Document::parse(file, text, &shape());
        let mut diagnostics = document.diagnostics().to_vec();
        let root = document.root();
        let mut entries: Vec<ProviderEntry> = Vec::new();

        for table in root.entries(PROVIDER) {
            let at = |key: &str| table.value_location(key).cloned();
            let Some(written) = table.text(TAG) else { continue };
            let tag = match Tag::parse(written) {
                Ok(tag) => tag,
                Err(problem) => {
                    let message = format!("`{TAG}` is `{written}`, which is not a tag: {problem:?}");
                    diagnostics.push(Diagnostic::error(at(TAG), message));
                    continue;
                }
            };
            if entries.iter().any(|entry| entry.tag == tag) {
                let message = format!("`{TAG}` is `{tag}`, and a provider of that tag was already read");
                diagnostics.push(Diagnostic::error(at(TAG), message));
                continue;
            }
            // A kind or a base the file does not hold is already reported by the document; a
            // provider without either cannot be asked anything, so it is left out.
            let (Some(kind), Some(base)) = (table.text(KIND).and_then(ProviderKind::parse), table.text(BASE)) else {
                continue;
            };
            let wire = match table.text(WIRE) {
                Some(written) => Wire::parse(written).unwrap_or_else(|| kind.wire()),
                None => kind.wire(),
            };
            let key = table.text(KEY).and_then(Key::new);
            if table.text(KEY).is_some() && key.is_none() {
                let message = format!("`{KEY}` of `{tag}` is not a key that fits a request header; it is not used");
                diagnostics.push(Diagnostic::warning(at(KEY), message));
            }
            entries.push(ProviderEntry { tag, kind, base: trim_base(base), wire, models: Vec::new(), key });
        }

        for table in root.entries(MODEL) {
            let at = |key: &str| table.value_location(key).cloned();
            let (Some(tag), Some(id)) = (table.text(TAG), table.text(ID)) else { continue };
            let Some(entry) = entries.iter_mut().find(|entry| entry.tag.as_str() == tag) else {
                let message = format!("`{TAG}` is `{tag}`, and no provider of that tag was read; the model is dropped");
                diagnostics.push(Diagnostic::warning(at(TAG), message));
                continue;
            };
            let claimed = positive(table.integer(CLAIMED), CLAIMED, at(CLAIMED), &mut diagnostics);
            let tokens = positive(table.integer(MEASURED), MEASURED, at(MEASURED), &mut diagnostics);
            // A measured window is two values that only mean anything together: the number, and
            // whether that number is the edge or only as far as anyone looked.
            let measured = match (tokens, table.text(MEASURED_KIND)) {
                (Some(tokens), Some(written)) => match Measured::parse(written, tokens) {
                    Some(measured) => Some(measured),
                    None => {
                        let message = format!("`{MEASURED_KIND}` is `{written}`; the measurement is dropped");
                        diagnostics.push(Diagnostic::warning(at(MEASURED_KIND), message));
                        None
                    }
                },
                (Some(_), None) => {
                    let message =
                        format!("`{MEASURED}` is there without `{MEASURED_KIND}`; the measurement is dropped");
                    diagnostics.push(Diagnostic::warning(at(MEASURED), message));
                    None
                }
                (None, _) => None,
            };
            entry.models.push(Model { id: id.to_owned(), claimed, measured });
        }

        Loaded { value: Self { path: None, entries }, diagnostics }
    }

    /// The providers, in the order they were added.
    #[must_use]
    pub fn entries(&self) -> &[ProviderEntry] {
        &self.entries
    }

    /// The provider tagged `tag`.
    #[must_use]
    pub fn get(&self, tag: &str) -> Option<&ProviderEntry> {
        self.entries.iter().find(|entry| entry.tag.as_str() == tag)
    }

    /// Adds `entry` at the end of the list.
    ///
    /// # Errors
    ///
    /// [`AddError::Taken`] when a provider of that tag is already there.
    pub fn add(&mut self, entry: ProviderEntry) -> Result<(), AddError> {
        if self.entries.iter().any(|already| already.tag == entry.tag) {
            return Err(AddError::Taken(entry.tag.as_str().to_owned()));
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Replaces what is known about the provider tagged `tag`, when it is still there.
    pub fn replace(&mut self, tag: &str, entry: ProviderEntry) {
        if let Some(slot) = self.entries.iter_mut().find(|already| already.tag.as_str() == tag) {
            *slot = entry;
        }
    }

    /// Removes the provider tagged `tag`, and with it the key it was reached with. Whether
    /// anything was there is the answer.
    pub fn remove(&mut self, tag: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.tag.as_str() != tag);
        before != self.entries.len()
    }

    /// Forgets the key of the provider tagged `tag`, leaving the provider itself where it is.
    /// This is a separate thing from removing the provider, because a person who has rolled
    /// their key wants the tag, the address and the measurements to stay.
    pub fn forget_key(&mut self, tag: &str) -> bool {
        match self.entries.iter_mut().find(|entry| entry.tag.as_str() == tag) {
            Some(entry) => entry.key.take().is_some(),
            None => false,
        }
    }

    /// The file as it is written.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut text = String::new();
        for entry in &self.entries {
            text.push_str(&format!("[[{PROVIDER}]]\n"));
            text.push_str(&format!("{TAG} = {}\n", quote(entry.tag.as_str())));
            text.push_str(&format!("{KIND} = {}\n", quote(entry.kind.id())));
            text.push_str(&format!("{BASE} = {}\n", quote(&entry.base)));
            text.push_str(&format!("{WIRE} = {}\n", quote(entry.wire.id())));
            if let Some(key) = &entry.key {
                text.push_str(&format!("{KEY} = {}\n", quote(key.expose())));
            }
            text.push('\n');
        }
        for entry in &self.entries {
            for model in &entry.models {
                text.push_str(&format!("[[{MODEL}]]\n"));
                text.push_str(&format!("{TAG} = {}\n", quote(entry.tag.as_str())));
                text.push_str(&format!("{ID} = {}\n", quote(&model.id)));
                if let Some(claimed) = model.claimed {
                    text.push_str(&format!("{CLAIMED} = {claimed}\n"));
                }
                if let Some(measured) = model.measured {
                    text.push_str(&format!("{MEASURED} = {}\n", measured.tokens()));
                    text.push_str(&format!("{MEASURED_KIND} = {}\n", quote(measured.id())));
                }
                text.push('\n');
            }
        }
        text
    }

    /// Writes the file, creating its folder with mode 700 and the file itself with mode 600. A
    /// folder that was already there, made wider by something else, is narrowed to 700 too: the
    /// key is about to be in it.
    ///
    /// The file is written beside its place and moved onto it, so a write that stops halfway
    /// leaves the providers that were there rather than half a file. The temporary file is made
    /// with the narrow mode too: a key must never exist, even for an instant, in a file anyone
    /// else on the machine can open.
    ///
    /// # Errors
    ///
    /// What the file system said, when the folder or the file could not be written.
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let folder = path.parent().ok_or_else(|| format!("{}: it has no folder", path.display()))?;
        create_folder(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        let temporary = path.with_extension("toml.writing");
        write_narrow(&temporary, &self.to_toml()).map_err(|error| format!("{}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, path).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("{}: {error}", path.display())
        })
    }

    /// Where these providers are kept, for the line the page shows.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The same providers, kept at `path`. For a test that writes a file of its own.
    #[must_use]
    pub fn at(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}

/// Reads an integer that only means anything above zero, reporting one that is not.
fn positive(value: Option<i64>, key: &str, at: Option<Location>, diagnostics: &mut Vec<Diagnostic>) -> Option<u64> {
    match value {
        None => None,
        Some(number) if number > 0 => u64::try_from(number).ok(),
        Some(number) => {
            diagnostics.push(Diagnostic::warning(at, format!("`{key}` is {number}; a window is a count of tokens")));
            None
        }
    }
}

/// What a providers file may hold.
fn shape() -> Shape {
    let provider = Shape::new()
        .required(TAG, ValueKind::text())
        .required(KIND, ValueKind::choice(ProviderKind::ALL.map(ProviderKind::id)))
        .required(BASE, ValueKind::text())
        .optional(WIRE, ValueKind::choice(Wire::ALL.map(Wire::id)))
        .optional(KEY, ValueKind::text());
    let model = Shape::new()
        .required(TAG, ValueKind::text())
        .required(ID, ValueKind::text())
        .optional(CLAIMED, ValueKind::integer())
        .optional(MEASURED, ValueKind::integer())
        .optional(MEASURED_KIND, ValueKind::choice(["about", "at-least"]));
    Shape::new().entries(PROVIDER, provider).entries(MODEL, model)
}

/// A TOML string, with what TOML cannot hold plain written the way TOML writes it.
fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other if (other as u32) < 0x20 => quoted.push_str(&format!("\\u{:04X}", other as u32)),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// A place that lets others on this machine read what is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionProblem {
    /// The providers file, or its folder.
    pub place: PathBuf,
    /// Its permissions as they are, such as `0o755`.
    pub mode: u32,
    /// What they should be.
    pub wanted: u32,
}

/// What is wrong with how wide the permissions of `path` and its folder are, read from the disk
/// as it is now.
///
/// This is a warning and never a refusal to read: a person whose backup program widened the
/// folder still wants their providers, and what they need is to be told. While there is no file
/// there is no key to protect, so the folder, which other parts of QCode make too, is not asked
/// about; the first save narrows it.
#[must_use]
pub fn permission_problems(path: &Path) -> Vec<PermissionProblem> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if !path.exists() {
            return Vec::new();
        }
        let mut problems = Vec::new();
        let mut check = |place: &Path, wanted: u32| {
            let Ok(metadata) = std::fs::metadata(place) else { return };
            let mode = metadata.permissions().mode() & 0o777;
            if mode & !wanted != 0 {
                problems.push(PermissionProblem { place: place.to_path_buf(), mode, wanted });
            }
        };
        if let Some(folder) = path.parent() {
            check(folder, FOLDER_MODE);
        }
        check(path, FILE_MODE);
        problems
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Vec::new()
    }
}

/// Creates `folder` and everything above it, with the folder itself narrow.
fn create_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;

        if let Some(above) = folder.parent() {
            std::fs::create_dir_all(above)?;
        }
        match std::fs::DirBuilder::new().mode(FOLDER_MODE).create(folder) {
            Ok(()) => Ok(()),
            // Made before, perhaps by another part of QCode with the usual 755; the key is about
            // to be written into it, so it is narrowed now rather than warned about afterwards.
            // A folder that is not the person's to narrow keeps its mode and the file is still
            // written: the page reads the permissions again after a save and says what is wrong.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = std::fs::metadata(folder)?.permissions().mode() & 0o777;
                if mode & !FOLDER_MODE != 0 {
                    let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(mode & FOLDER_MODE));
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(folder)
}

/// Writes `text` to `path` in a file only its owner can open.
fn write_narrow(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}
