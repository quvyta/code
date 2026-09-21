//! The part of asking a provider that no canned answer can stand for: that a real server really
//! answers in the shape [`ask`](super::ask) reads, and that the window it really gives is the one
//! [`measure`](super::ask::measure) finds.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_PROVIDER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no server. They go out over the network, to the
//! address in `QCODE_OLLAMA` or to the one below.
//!
//! ```text
//! QCODE_PROVIDER_TESTS=1 cargo test -- --ignored --nocapture provider::live
//! ```
//!
//! Nothing here needs a key and nothing here writes a file. An ollama server is reached with
//! nothing secret, which is the whole reason a profile on one is complete today: no secret
//! enters a container because there is none.
//!
//! A server that does not answer is said out loud and the test ends without failing — the suite
//! stays green on a machine without one. It is never a silent pass: the reason is printed, and a
//! run with `--nocapture` shows which tests really ran.

use super::ask::{self, Reached, Web};
use super::{ProviderEntry, ProviderKind, Tag};

/// The owner's own ollama server, on the machine that hosts this one.
const OLLAMA: &str = "http://192.168.122.1:11434";

/// The variable that names another server, for a machine whose ollama is somewhere else.
const ADDRESS: &str = "QCODE_OLLAMA";

/// The variable that names which model to measure. Without it the first the server lists is
/// taken, which is enough to show the measurement works; naming one is how a particular model's
/// real window is looked at.
const MODEL: &str = "QCODE_OLLAMA_MODEL";

/// The provider these tests ask, or nothing at all when they are switched off.
fn server() -> Option<ProviderEntry> {
    if std::env::var("QCODE_PROVIDER_TESTS").as_deref() != Ok("1") {
        return None;
    }
    let base = std::env::var(ADDRESS).unwrap_or_else(|_| OLLAMA.to_owned());
    let tag = Tag::parse("live").expect("the tag is a tag");
    Some(ProviderEntry::new(tag, ProviderKind::Ollama, &base))
}

/// Says why nothing ran, so that a skip is never mistaken for a pass.
fn skipped(why: &str) {
    println!("skipped: {why}");
}

#[test]
#[ignore = "needs an ollama server; QCODE_PROVIDER_TESTS=1"]
fn a_real_ollama_server_names_its_version_when_the_connection_is_tried() {
    let Some(entry) = server() else { return skipped("QCODE_PROVIDER_TESTS is not 1") };
    let web = Web::network();
    match ask::try_connection(&web, &entry) {
        Ok(Reached::Version(version)) => {
            assert!(!version.is_empty(), "a server that answers names a version");
            println!("{} answered version {version}", entry.base);
        }
        Ok(other) => panic!("an ollama server answers with its version, not {other:?}"),
        Err(trouble) => skipped(&format!("{} did not answer: {trouble:?}", trouble.url())),
    }
}

#[test]
#[ignore = "needs an ollama server; QCODE_PROVIDER_TESTS=1"]
fn a_real_ollama_server_lists_its_models_and_says_what_each_one_claims() {
    let Some(entry) = server() else { return skipped("QCODE_PROVIDER_TESTS is not 1") };
    let web = Web::network();
    let models = match ask::list_models(&web, &entry) {
        Ok(models) => models,
        Err(trouble) => return skipped(&format!("{} did not answer: {trouble:?}", trouble.url())),
    };
    if models.is_empty() {
        return skipped(&format!("{} has no models pulled", entry.base));
    }
    for model in &models {
        println!("{} claims {:?}", model.id, model.claimed);
    }
    assert!(models.iter().any(|model| model.claimed.is_some()), "a real server answers `/api/show` for its models");
}

#[test]
#[ignore = "needs an ollama server; QCODE_PROVIDER_TESTS=1"]
fn what_a_real_server_gives_is_measured_against_what_its_model_claims() {
    let Some(entry) = server() else { return skipped("QCODE_PROVIDER_TESTS is not 1") };
    let web = Web::network();
    let models = match ask::list_models(&web, &entry) {
        Ok(models) => models,
        Err(trouble) => return skipped(&format!("{} did not answer: {trouble:?}", trouble.url())),
    };
    let wanted = std::env::var(MODEL).ok();
    let chosen = match &wanted {
        Some(name) => models.iter().find(|model| &model.id == name),
        None => models.first(),
    };
    let Some(model) = chosen else {
        return skipped(&format!("{} has no {}", entry.base, wanted.as_deref().unwrap_or("models pulled")));
    };
    match ask::measure(&web, &entry, &model.id) {
        Ok(measured) => {
            // This is the whole point of the feature, printed: the two numbers side by side.
            println!("{}: claims {:?}, this server gives {measured:?}", model.id, model.claimed);
            assert!(measured.tokens() > 0, "a server that answers read something");
        }
        Err(trouble) => skipped(&format!("{} would not be probed: {trouble:?}", trouble.url())),
    }
}
