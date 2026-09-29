//! What QCode knows of free OpenRouter models from having run a harness on them, read from
//! `tried.toml` beside this file.
//!
//! Free models are offered by the hundred and change by the week, and a good share of them do not
//! work with a coding agent at all: some answer a harness with nothing, some refuse its tool
//! definitions outright. A person picking one at random finds that out only after building a
//! profile on it. So the page puts the ones QCode saw work first, and the ones it saw fail last
//! with the reason, and leaves everything else where the service listed it.
//!
//! The file never adds a model: the order is applied to what the service answered, so a model it
//! no longer offers is not shown, whatever the file says of it.

use std::sync::LazyLock;

use qframe::diagnostics::Diagnostic;
use qframe::document::{Document, Shape, ValueKind};

use crate::profile::HarnessKind;
use crate::provider::Model;

/// The record, as it ships.
const TEXT: &str = include_str!("tried.toml");

/// The harnesses a free model has been run on.
const HARNESSES: [HarnessKind; 2] = [HarnessKind::ClaudeCode, HarnessKind::OpenCode];

/// Why a model did not work with a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The harness got an empty answer.
    Empty,
    /// The service refused the harness's own tool definitions.
    Tools,
    /// It used its tools only part of the way.
    Half,
}

impl Reason {
    const ALL: [Self; 3] = [Self::Empty, Self::Tools, Self::Half];

    fn id(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Tools => "tools",
            Self::Half => "half",
        }
    }
}

/// What is known of one model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It answered and used its tools in each of these harnesses.
    Works(Vec<HarnessKind>),
    /// It did not work in this harness, for this reason.
    Fails(HarnessKind, Reason),
}

/// Every model the file speaks of, in its order.
#[derive(Debug, Default)]
pub struct Tried {
    works: Vec<(String, HarnessKind)>,
    fails: Vec<(String, HarnessKind, Reason)>,
}

impl Tried {
    /// Reads the record from TOML `text`, and what was wrong with it, reported against `file`.
    /// What could be read is kept; an entry that could not is left out.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> (Self, Vec<Diagnostic>) {
        let harness = || ValueKind::choice(HARNESSES.map(|harness| harness.record().id));
        let shape = Shape::new()
            .required("tried", ValueKind::text())
            .entries("works", Shape::new().required("model", ValueKind::text()).required("with", harness()))
            .entries(
                "fails",
                Shape::new()
                    .required("model", ValueKind::text())
                    .required("with", harness())
                    .required("reason", ValueKind::choice(Reason::ALL.map(Reason::id))),
            );
        let document = Document::parse(file, text, &shape);
        let root = document.root();
        let harness_of = |id: Option<&str>| id.and_then(HarnessKind::parse);
        let works = root
            .entries("works")
            .iter()
            .filter_map(|table| Some((table.text("model")?.to_owned(), harness_of(table.text("with"))?)))
            .collect();
        let fails = root
            .entries("fails")
            .iter()
            .filter_map(|table| {
                let reason = Reason::ALL.into_iter().find(|reason| Some(reason.id()) == table.text("reason"))?;
                Some((table.text("model")?.to_owned(), harness_of(table.text("with"))?, reason))
            })
            .collect();
        (Self { works, fails }, document.diagnostics().to_vec())
    }

    /// What is known of the model `id`. A model seen working anywhere is said to work: a failure
    /// in one harness is only worth a line when nothing is known to go.
    #[must_use]
    pub fn verdict(&self, id: &str) -> Option<Verdict> {
        let works: Vec<HarnessKind> =
            self.works.iter().filter(|(model, _)| model == id).map(|(_, harness)| *harness).collect();
        if !works.is_empty() {
            return Some(Verdict::Works(works));
        }
        self.fails
            .iter()
            .find(|(model, _, _)| model == id)
            .map(|(_, harness, reason)| Verdict::Fails(*harness, *reason))
    }

    /// Puts the models seen working first, in the file's order, and the ones seen failing last,
    /// leaving every other model where it was.
    pub fn order(&self, models: &mut [Model]) {
        let place = |model: &Model| {
            if let Some(at) = self.works.iter().position(|(id, _)| *id == model.id) {
                (0, at)
            } else if self.fails.iter().any(|(id, _, _)| *id == model.id) {
                (2, 0)
            } else {
                (1, 0)
            }
        };
        models.sort_by_key(place);
    }
}

/// The record QCode ships with.
///
/// It ships inside the program and a test reads it without a single problem, so there is no one to
/// tell of a problem here at run time; a broken entry would only be missing from the order.
pub static TRIED: LazyLock<Tried> = LazyLock::new(|| Tried::parse("tried.toml", TEXT).0);

#[cfg(test)]
mod tests {
    use super::{Reason, TEXT, TRIED, Tried, Verdict};
    use crate::profile::HarnessKind;
    use crate::provider::Model;

    #[test]
    fn the_record_that_ships_reads_without_a_problem_and_says_what_it_should() {
        let (_, diagnostics) = Tried::parse("tried.toml", TEXT);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            TRIED.verdict("nex-agi/nex-n2.5-mini:free"),
            Some(Verdict::Works(vec![HarnessKind::ClaudeCode, HarnessKind::OpenCode]))
        );
        assert_eq!(
            TRIED.verdict("qwen/qwen3.8-27b:free"),
            Some(Verdict::Fails(HarnessKind::ClaudeCode, Reason::Tools))
        );
        assert_eq!(TRIED.verdict("google/gemma-4-31b-it:free"), None, "only busy when it was tried");
    }

    #[test]
    fn a_mistake_in_the_record_is_a_diagnostic_with_its_place_and_not_a_panic() {
        let (tried, diagnostics) = Tried::parse(
            "tried.toml",
            "tried = \"x\"\n[[fails]]\nmodel = \"a:free\"\nwith = \"claude-code\"\nreason = \"slow\"\n",
        );
        assert_eq!(tried.verdict("a:free"), None, "a reason nobody wrote a sentence for is not shown");
        let said = diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        assert!(said.contains("tried.toml:5"), "{said}");
    }

    #[test]
    fn the_order_moves_only_what_the_record_knows_and_invents_nothing() {
        let (tried, _) = Tried::parse(
            "t.toml",
            "tried = \"x\"\n\
             [[works]]\nmodel = \"b\"\nwith = \"claude-code\"\n\
             [[works]]\nmodel = \"gone\"\nwith = \"opencode\"\n\
             [[works]]\nmodel = \"a\"\nwith = \"opencode\"\n\
             [[fails]]\nmodel = \"x\"\nwith = \"claude-code\"\nreason = \"empty\"\n",
        );
        let mut models: Vec<Model> = ["x", "m", "a", "n", "b"].into_iter().map(Model::new).collect();
        tried.order(&mut models);
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "m", "n", "x"]);
    }
}
