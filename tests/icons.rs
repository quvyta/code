//! Every icon QCode names is one the framework's icon set has.
//!
//! An icon is named by a string, so a name the set does not have is not a build error: the screen
//! draws the name itself, `⟦plus⟧`, where a glyph was meant to be. The names are read out of the
//! source the way a person would find them, and each is looked up in the set QCode draws with.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use qframe::icons::{GlyphMode, IconSetRegistry};

/// The calls that take an icon's name as their first argument.
const CALLS: [&str; 3] = ["icon(\"", "glyph(\"", "IconButton::new(\""];

/// Every `.rs` file under `dir`, test files left out: a test may name an icon that is not there
/// on purpose.
fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, found);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") && !name.contains("tests") {
            found.push(path);
        }
    }
}

/// The names given as a string literal to one of [`CALLS`] in `text`, with the line each is on.
fn named(text: &str) -> Vec<(usize, String)> {
    let mut names = Vec::new();
    for (number, line) in text.lines().enumerate() {
        for call in CALLS {
            let mut rest = line;
            while let Some(at) = rest.find(call) {
                let after = &rest[at + call.len()..];
                if let Some(end) = after.find('"') {
                    names.push((number + 1, after[..end].to_owned()));
                }
                rest = after;
            }
        }
    }
    names
}

#[test]
fn every_icon_named_in_the_source_is_in_the_framework_set() {
    let icons = IconSetRegistry::builtin().icons("default", &BTreeMap::new(), GlyphMode::Unicode);
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut seen = 0;
    let mut missing = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("a source file reads");
        for (line, name) in named(&text) {
            seen += 1;
            if !icons.contains(&name) {
                missing.push(format!("{}:{line} `{name}`", file.display()));
            }
        }
    }
    // A pattern that stopped matching would pass with nothing checked.
    assert!(seen > 20, "only {seen} icon names were found in the source");
    assert!(missing.is_empty(), "the icon set has no glyph for:\n{}", missing.join("\n"));
}
