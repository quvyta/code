//! The part of a login that lives inside a harness's shared settings file rather than in a file
//! of its own.
//!
//! Claude Code keeps its tokens in `~/.claude/.credentials.json`, but the other half of a
//! sign-in in `~/.claude.json`, beside the folders it trusts and the servers it starts: the
//! account (`oauthAccount`) and the end of its first-start questions (`hasCompletedOnboarding`),
//! which a sign-in is one of. Read in its 2.1.282 bundle: the first start is shown whenever that
//! flag is missing, and it always holds the step that signs in, whatever tokens are on disk. A
//! home given the tokens alone was therefore asked to sign in again on a profile whose image
//! writes no `~/.claude.json`, and everywhere else showed no account.
//!
//! The file cannot be copied whole: in a workspace it holds what the profile's template wrote
//! and what QCode's bridge registered, and the login container's copy holds neither. So the keys
//! of the login are taken out on their own, into [`MERGE_DIR`] of the profile's credentials
//! volume, and merged into the home's file when a home is given the login: every key the home
//! has stays, the login's keys are the profile's.

use crate::base::paths::HOME_DIR;
use crate::profile::HarnessKind;
use crate::profile::identity::STORE_DIR;

/// The folder of a credentials volume, and of a capture, that holds the keys to be merged into a
/// home's files rather than copied over them: the same path under it as the file under the home.
pub const MERGE_DIR: &str = ".qcode-merge";

/// A settings file of a harness that a login writes some of its keys into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shared {
    /// Where the file is, relative to the harness's home directory.
    pub path: &'static str,
    /// The keys that are the login's.
    pub keys: &'static [&'static str],
}

/// Claude Code's: the account, and the flag with the version that closed the first start.
const CLAUDE_CODE: Shared =
    Shared { path: ".claude.json", keys: &["oauthAccount", "hasCompletedOnboarding", "lastOnboardingVersion"] };

/// The file `harness`'s login shares with its other settings, if it has one.
#[must_use]
pub fn shared(harness: HarnessKind) -> Option<Shared> {
    match harness {
        HarnessKind::ClaudeCode => Some(CLAUDE_CODE),
        _ => None,
    }
}

/// Takes the named keys out of a JSON file into a file of their own. Nothing is written when the
/// file is not there, cannot be read, or holds none of them: a login that wrote none leaves
/// nothing to merge, and that is not a failure.
const TAKE: &str = "const fs = require('fs'), path = require('path');
const [from, to, ...keys] = process.argv.slice(1);
let all; try { all = JSON.parse(fs.readFileSync(from, 'utf8')); } catch (e) { process.exit(0); }
const taken = {};
for (const key of keys) if (all && all[key] !== undefined) taken[key] = all[key];
if (Object.keys(taken).length === 0) process.exit(0);
fs.mkdirSync(path.dirname(to), { recursive: true });
fs.writeFileSync(to, JSON.stringify(taken), { mode: 0o600 });";

/// Merges every file under a store's merge folder into the file at the same path of a home,
/// keeping every key the home's file has and taking the store's for the rest, then removes the
/// merge folder the copy of the store brought into the home. A home file that is there and is
/// not JSON is left as it is rather than replaced by the login's keys alone.
const MERGE: &str = "const fs = require('fs'), path = require('path');
const [store, home, folder] = process.argv.slice(1);
const root = path.join(store, folder);
const walk = (dir) => fs.readdirSync(dir, { withFileTypes: true })
  .flatMap((entry) => entry.isDirectory() ? walk(path.join(dir, entry.name)) : [path.join(dir, entry.name)]);
let files = []; try { files = walk(root); } catch (e) {}
for (const file of files) {
  const target = path.join(home, path.relative(root, file));
  let into = {};
  if (fs.existsSync(target)) { try { into = JSON.parse(fs.readFileSync(target, 'utf8')); } catch (e) { continue; } }
  const given = JSON.parse(fs.readFileSync(file, 'utf8'));
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, JSON.stringify({ ...into, ...given }, null, 2), { mode: 0o600 });
}
fs.rmSync(path.join(home, folder), { recursive: true, force: true });";

/// The command, run in the container a login was made in, that takes the login's keys of
/// `harness`'s shared file out into the capture folder `capture`; `None` for a harness whose
/// login is only files of its own.
#[must_use]
pub fn take(harness: HarnessKind, capture: &str) -> Option<Vec<String>> {
    let shared = shared(harness)?;
    let mut words = vec![
        "node".to_owned(),
        "-e".to_owned(),
        TAKE.to_owned(),
        format!("{HOME_DIR}/{}", shared.path),
        format!("{capture}/{MERGE_DIR}/{}", shared.path),
    ];
    words.extend(shared.keys.iter().map(|key| (*key).to_owned()));
    Some(words)
}

/// Takes the named keys out of a JSON file where it is, keeping every other key. A file that is
/// not there, or not JSON, is left as it is.
const STRIP: &str = "const fs = require('fs');
const [file, ...keys] = process.argv.slice(1);
let all; try { all = JSON.parse(fs.readFileSync(file, 'utf8')); } catch (e) { process.exit(0); }
if (!all || typeof all !== 'object') process.exit(0);
for (const key of keys) delete all[key];
fs.writeFileSync(file, JSON.stringify(all, null, 2));";

/// The command, run in a profile's shell before what it holds becomes the profile's image, that
/// takes the login's keys out of `harness`'s shared file: a login never goes into an image, where
/// every workspace of the profile would carry it; `None` for a harness whose login is only files
/// of its own.
#[must_use]
pub fn strip(harness: HarnessKind) -> Option<Vec<String>> {
    let shared = shared(harness)?;
    let mut words = vec!["node".to_owned(), "-e".to_owned(), STRIP.to_owned(), format!("{HOME_DIR}/{}", shared.path)];
    words.extend(shared.keys.iter().map(|key| (*key).to_owned()));
    Some(words)
}

/// The command, run in the container that gives a home its profile's login after the store was
/// copied in, that merges the login's keys into the home's shared files.
#[must_use]
pub fn merge() -> Vec<String> {
    ["node", "-e", MERGE, STORE_DIR, HOME_DIR, MERGE_DIR].map(str::to_owned).to_vec()
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::*;

    /// Runs `words` with this machine's Node, with the container's folders in them moved under
    /// `root`, and answers whether it succeeded.
    fn node(words: &[String], root: &Path) -> bool {
        let moved: Vec<String> = words
            .iter()
            .map(|word| if word.starts_with('/') { format!("{}{word}", root.display()) } else { word.clone() })
            .collect();
        Command::new(&moved[0]).args(&moved[1..]).status().is_ok_and(|status| status.success())
    }

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("the file is there")).expect("JSON")
    }

    #[test]
    fn only_claude_code_shares_its_login_with_its_settings() {
        assert_eq!(shared(HarnessKind::ClaudeCode).map(|shared| shared.path), Some(".claude.json"));
        for harness in HarnessKind::ALL.into_iter().filter(|harness| *harness != HarnessKind::ClaudeCode) {
            assert_eq!(shared(harness), None, "{harness:?}");
            assert_eq!(take(harness, "/capture"), None, "{harness:?}");
        }
    }

    #[test]
    fn a_login_s_keys_are_taken_out_and_merged_into_a_home_keeping_what_the_home_had() {
        let root = crate::testing::scratch("account-merge");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join(HOME_DIR.trim_start_matches('/'));
        let capture = "/capture";
        std::fs::create_dir_all(&home).expect("a home");
        // The login container's file, as Claude Code leaves it after a sign-in: the login's keys
        // among others that belong to that container alone.
        std::fs::write(
            home.join(".claude.json"),
            r#"{"hasCompletedOnboarding":true,"oauthAccount":{"emailAddress":"take@qcode.test"},"projects":{"/tmp":{}},"numStartups":3}"#,
        )
        .expect("the login container's file");
        let words = take(HarnessKind::ClaudeCode, capture).expect("Claude Code has keys to take");
        assert!(node(&words, &root), "the keys are taken");
        let taken = read(&root.join("capture").join(MERGE_DIR).join(".claude.json"));
        assert_eq!(taken["oauthAccount"]["emailAddress"], "take@qcode.test");
        assert_eq!(taken["hasCompletedOnboarding"], true);
        assert!(taken.get("projects").is_none() && taken.get("numStartups").is_none(), "{taken}");

        // The store holds what was taken; a workspace's home holds what its template wrote and
        // what the copy of the store brought in beside it.
        let store = root.join(STORE_DIR.trim_start_matches('/'));
        std::fs::create_dir_all(store.join(MERGE_DIR)).expect("a store");
        std::fs::rename(
            root.join("capture").join(MERGE_DIR).join(".claude.json"),
            store.join(MERGE_DIR).join(".claude.json"),
        )
        .expect("the capture is stored");
        std::fs::write(home.join(".claude.json"), r#"{"projects":{"/work":{"hasTrustDialogAccepted":true}}}"#)
            .expect("the template's file");
        std::fs::create_dir_all(home.join(MERGE_DIR)).expect("the copy's folder");
        std::fs::copy(store.join(MERGE_DIR).join(".claude.json"), home.join(MERGE_DIR).join(".claude.json"))
            .expect("the copy of the store");
        assert!(node(&merge(), &root), "the keys are merged");
        let merged = read(&home.join(".claude.json"));
        assert_eq!(merged["oauthAccount"]["emailAddress"], "take@qcode.test");
        assert_eq!(merged["hasCompletedOnboarding"], true);
        assert_eq!(merged["projects"]["/work"]["hasTrustDialogAccepted"], true, "the template's keys stay");
        assert!(!home.join(MERGE_DIR).exists(), "nothing of the copy is left in the home");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_home_with_no_file_gets_one_and_a_home_file_that_is_not_json_is_left_alone() {
        let root = crate::testing::scratch("account-merge-edges");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join(HOME_DIR.trim_start_matches('/'));
        let store = root.join(STORE_DIR.trim_start_matches('/'));
        std::fs::create_dir_all(store.join(MERGE_DIR)).expect("a store");
        std::fs::create_dir_all(&home).expect("a home");
        std::fs::write(store.join(MERGE_DIR).join(".claude.json"), r#"{"hasCompletedOnboarding":true}"#).expect("keys");
        assert!(node(&merge(), &root));
        assert_eq!(read(&home.join(".claude.json"))["hasCompletedOnboarding"], true);

        std::fs::write(home.join(".claude.json"), "not json").expect("a broken file");
        assert!(node(&merge(), &root));
        assert_eq!(std::fs::read_to_string(home.join(".claude.json")).expect("there"), "not json");

        // A login container whose harness wrote no such file leaves nothing to merge.
        std::fs::remove_file(home.join(".claude.json")).expect("removed");
        let words = take(HarnessKind::ClaudeCode, "/capture").expect("keys");
        assert!(node(&words, &root));
        assert!(!root.join("capture").exists(), "nothing was taken");
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn a_login_made_in_a_profiles_shell_is_taken_out_of_the_shared_file_and_the_rest_stays() {
        let root = crate::testing::scratch("account-strip");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join(HOME_DIR.trim_start_matches('/'));
        std::fs::create_dir_all(&home).expect("a home");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"hasCompletedOnboarding":true,"lastOnboardingVersion":"2.1.282","oauthAccount":{"emailAddress":"strip@qcode.test"},"theme":"dark"}"#,
        )
        .expect("the shell's file");
        let words = strip(HarnessKind::ClaudeCode).expect("Claude Code has keys to strip");
        assert!(node(&words, &root), "the keys are stripped");
        let left = read(&home.join(".claude.json"));
        assert_eq!(left, serde_json::json!({"theme": "dark"}));
        std::fs::write(home.join(".claude.json"), "not json").expect("a broken file");
        assert!(node(&words, &root));
        assert_eq!(std::fs::read_to_string(home.join(".claude.json")).expect("there"), "not json");
        assert_eq!(strip(HarnessKind::OpenCode), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
