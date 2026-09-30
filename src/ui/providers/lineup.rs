//! The dialog that makes, changes and deletes a provider's lineups: its own state, and
//! everything that can stop a lineup being saved.
//!
//! A lineup is a named order of one provider's own models, and the name is the person's own word
//! for it, so it follows the same rules as a provider's tag: a harness is told `coder`, and no
//! name that could be read as a model name or a key of the file is taken.
//!
//! Nothing here touches the disk or the network, and nothing here is a sentence: a refusal is a
//! value the view turns into words from the language files, the same way the add-a-provider form
//! does. The steps are held in the dialog while it is open and reach the file only when they are
//! saved, so an order left half-made is not left behind in the file either.

use qframe::text::fuzzy;

use crate::provider::{Lineup, Model, Price, ProviderEntry, Tag, TagError};

/// The name of the list of a provider's lineups, for the focus.
pub const LIST: &str = "lineups";

/// The name of the field the lineup is named in.
pub const NAME_FIELD: &str = "lineup-name";

/// The name of the field the provider's models are filtered in.
pub const FILTER_FIELD: &str = "lineup-filter";

/// The name of the list the provider's models are chosen from.
pub const MODELS: &str = "lineup-models";

/// The name of the list the lineup's own steps stand in.
pub const STEPS: &str = "lineup-steps";

/// What stopped a lineup being saved, apart from its wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The name cannot be a name, and why.
    Name(TagError),
    /// This provider already has a lineup of that name.
    Taken(String),
    /// Nothing was put in it. An order with no step is not one: a request that failed on the
    /// first model would have nothing to fall back to, and the relay would be told so at the
    /// worst moment rather than here.
    Empty,
}

/// The lineups dialog, while it is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lineups {
    /// The lineup being written, or nothing while the list of them stands in its place.
    pub editor: Option<Editor>,
    /// The lineup the person is looking at, named rather than numbered: a lineup saved or deleted
    /// under the person must not move the list out from under their hand.
    pub chosen: Option<String>,
}

/// A lineup as it is being written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Editor {
    /// The name as it has been typed.
    pub name: String,
    /// The lineup this one replaces, or nothing for a lineup that is new. A person editing `coder`
    /// and saving it under the same name has taken nothing, and this is what says so.
    pub editing: Option<String>,
    /// What has been written above the list of the provider's models.
    pub filter: String,
    /// The models in the order they are tried, as the person's own list.
    pub models: Vec<String>,
    /// The model the person is looking at in the list of the provider's models, named rather than
    /// numbered, because the filter is what numbers them and it is written as it is typed.
    pub model: Option<String>,
    /// The step the person is looking at in the lineup.
    pub step: usize,
    /// What stopped it last time it was asked to be saved, or nothing while they have not asked.
    /// A form does not complain about a field nobody has finished.
    pub refusal: Option<Refusal>,
}

impl Lineups {
    /// The dialog as it opens: the list, with nothing chosen and nothing being written.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a lineup called `name` is the one being written, which is what decides whether the
    /// name is free.
    #[must_use]
    pub fn editing(&self, name: &str) -> bool {
        self.editor.as_ref().is_some_and(|editor| editor.editing.as_deref() == Some(name))
    }

    /// What has been written above the list of the provider's models.
    #[must_use]
    pub fn filter(&self) -> &str {
        self.editor.as_ref().map_or("", |editor| editor.filter.as_str())
    }

    /// The steps of the lineup being written, in the order they are tried.
    #[must_use]
    pub fn steps(&self) -> &[String] {
        self.editor.as_ref().map_or(&[], |editor| editor.models.as_slice())
    }

    /// The step the person is looking at, while a lineup is being written.
    #[must_use]
    pub fn step(&self) -> usize {
        self.editor.as_ref().map_or(0, |editor| editor.step)
    }

    /// What stopped the lineup being saved last time, as the view turns it into words.
    #[must_use]
    pub fn refusal(&self) -> Option<&Refusal> {
        self.editor.as_ref().and_then(|editor| editor.refusal.as_ref())
    }

    /// The provider's models the filter keeps, in the provider's own order.
    ///
    /// A service such as OpenRouter offers hundreds of models and a dialog holds a handful, so the
    /// list shows what is asked for. What the filter hides is not thrown away: a step already in
    /// the lineup stays in it, and the line under the name says so.
    #[must_use]
    pub fn shown<'a>(&self, entry: &'a ProviderEntry) -> Vec<&'a Model> {
        let wanted = self.filter().trim();
        if wanted.is_empty() {
            return entry.models.iter().collect();
        }
        entry.models.iter().filter(|model| fuzzy(wanted, &model.id).is_some()).collect()
    }

    /// The steps that cost money, said as [`Lineup::paid`] would say it for a saved one: the
    /// person hears it while they are choosing the order rather than from a bill afterwards.
    /// There is no lineup yet to ask, so the steps are read off the editor.
    #[must_use]
    pub fn paid<'a>(&self, entry: &'a ProviderEntry) -> Vec<&'a str> {
        self.steps()
            .iter()
            .filter_map(|id| entry.model(id))
            .filter(|model| model.price == Some(Price::Paid))
            .map(|model| model.id.as_str())
            .collect()
    }

    /// The first lineup of `entry`, so that Edit and Delete act on something the moment the
    /// dialog opens rather than waiting for a row to be pointed at.
    pub fn open(&mut self, entry: &ProviderEntry) {
        self.chosen = entry.lineups.first().map(|lineup| lineup.name.as_str().to_owned());
    }

    /// Puts the choice on a lineup that is still there after the one under the hand is gone, and
    /// on the first of what is left when the one under the hand is the one that went. The names
    /// are the list as it is now, read after the deletion rather than before it.
    pub fn kept(&mut self, names: &[String]) {
        let kept: Option<String> = self.chosen.clone().filter(|name| names.iter().any(|other| other == name));
        self.chosen = kept.or_else(|| names.first().cloned());
    }

    /// The lineup called `name` is the one the person is looking at.
    pub fn choose(&mut self, name: &str) {
        self.chosen = Some(name.to_owned());
    }

    /// Starts a lineup that is not there yet. Nothing of another is carried into it: its steps are
    /// its own.
    pub fn new_lineup(&mut self) {
        self.editor = Some(Editor::default());
    }

    /// Starts the lineup `lineup` for changing, with its own name and steps in the fields.
    pub fn edit(&mut self, lineup: &Lineup) {
        self.editor = Some(Editor {
            name: lineup.name.as_str().to_owned(),
            editing: Some(lineup.name.as_str().to_owned()),
            models: lineup.models.clone(),
            ..Editor::default()
        });
    }

    /// The name as it has been typed. A form does not complain about a field nobody has finished,
    /// so what was said before is taken back as soon as it is written in.
    pub fn name(&mut self, text: String) {
        if let Some(editor) = &mut self.editor {
            editor.name = text;
            editor.refusal = None;
        }
    }

    /// What has been written above the list of the provider's models.
    pub fn filter_for(&mut self, text: String) {
        if let Some(editor) = &mut self.editor {
            editor.filter = text;
        }
    }

    /// The model the person is looking at in the list of the provider's models.
    pub fn look_at(&mut self, id: &str) {
        if let Some(editor) = &mut self.editor {
            editor.model = Some(id.to_owned());
        }
    }

    /// Puts the model `id` at the end of the lineup.
    ///
    /// The same model twice would send the same request to it twice and fall back to it twice, so
    /// a step already in the lineup is not put in again: the list of steps, right under the one
    /// that was pressed, is where that is visible.
    pub fn add(&mut self, id: &str) {
        let Some(editor) = &mut self.editor else { return };
        if !editor.models.iter().any(|step| step == id) {
            editor.models.push(id.to_owned());
        }
        editor.step = editor.models.len().saturating_sub(1);
    }

    /// The step the person is looking at in the lineup.
    pub fn pick_step(&mut self, index: usize) {
        if let Some(editor) = &mut self.editor {
            editor.step = index.min(editor.models.len().saturating_sub(1));
        }
    }

    /// Moves the chosen step one place earlier. The first step has nowhere earlier to go, and a
    /// request that failed on it is what the order is for.
    pub fn up(&mut self) {
        if let Some(editor) = &mut self.editor
            && editor.step > 0
        {
            editor.step -= 1;
            editor.models.swap(editor.step, editor.step + 1);
        }
    }

    /// Moves the chosen step one place later. The last step has nowhere later to go.
    pub fn down(&mut self) {
        if let Some(editor) = &mut self.editor
            && editor.step + 1 < editor.models.len()
        {
            editor.models.swap(editor.step, editor.step + 1);
            editor.step += 1;
        }
    }

    /// Takes the chosen step out of the lineup.
    pub fn remove(&mut self) {
        if let Some(editor) = &mut self.editor
            && editor.step < editor.models.len()
        {
            editor.models.remove(editor.step);
            editor.step = editor.step.min(editor.models.len().saturating_sub(1));
        }
    }

    /// The lineup the editor describes, or the first reason it is not one.
    ///
    /// `taken` says whether a lineup of this name is already there, which only the file knows.
    ///
    /// # Errors
    ///
    /// [`Refusal`] says what stopped it, in the words the view shows beside the name field.
    pub fn build(&self, taken: impl Fn(&str) -> bool) -> Result<Lineup, Refusal> {
        let Some(editor) = &self.editor else { return Err(Refusal::Empty) };
        let name = Tag::parse(editor.name.trim()).map_err(Refusal::Name)?;
        // The lineup being written is not in the way of its own name; another one is, since two
        // orders of one provider under one name would leave nothing to tell which was meant.
        if taken(name.as_str()) && editor.editing.as_deref() != Some(name.as_str()) {
            return Err(Refusal::Taken(name.as_str().to_owned()));
        }
        if editor.models.is_empty() {
            return Err(Refusal::Empty);
        }
        Ok(Lineup { name, models: editor.models.clone() })
    }

    /// Puts the saved lineup back on the list and leaves the editor, which is what a save means
    /// everywhere else on this page: what was asked to be kept is now what is shown.
    pub fn saved(&mut self, name: &str) {
        self.editor = None;
        self.chosen = Some(name.to_owned());
    }

    /// Leaves the lineup being written, keeping nothing of it. A lineup reaches the file when it
    /// is saved, so one left half-made is not left behind in the file either.
    pub fn cancelled(&mut self) {
        self.editor = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Model, ProviderEntry, ProviderKind, Wire};

    /// A provider of forty models, every third of them priced, as OpenRouter's listing leaves it.
    fn entry() -> ProviderEntry {
        let mut entry =
            ProviderEntry::new(Tag::parse("yol").expect("a tag"), ProviderKind::OpenRouter, "https://openrouter.ai");
        entry.wire = Wire::OpenAi;
        entry.models = (1..=40)
            .map(|n| Model {
                id: format!("acme/model-{n:02}"),
                claimed: Some(262_144),
                measured: None,
                price: match n % 3 {
                    0 => Some(Price::Free),
                    1 => Some(Price::Paid),
                    _ => None,
                },
            })
            .collect();
        entry
    }

    /// A lineup of the first `n` models, under the name `name`.
    fn lineup(entry: &ProviderEntry, name: &str, n: usize) -> Lineup {
        Lineup {
            name: Tag::parse(name).expect("a name"),
            models: entry.models.iter().take(n).map(|model| model.id.clone()).collect(),
        }
    }

    /// The names the dialog is given for the lineups the file holds now.
    fn names(entry: &ProviderEntry) -> Vec<String> {
        entry.lineups.iter().map(|lineup| lineup.name.as_str().to_owned()).collect()
    }

    /// Whether a lineup of this name is in the file, which is all the file knows and all the
    /// dialog needs to ask it. A fresh closure each time, so a test may write into the file
    /// between two of the dialog's questions.
    fn held(entry: &ProviderEntry) -> impl Fn(&str) -> bool {
        let names = names(entry);
        move |name: &str| names.iter().any(|other| other == name)
    }

    #[test]
    fn a_lineup_keeps_the_order_the_person_chose_and_the_buttons_move_it_whole() {
        let mut entry = entry();
        entry.lineups.push(lineup(&entry, "coder", 3));
        let mut dialog = Lineups::new();
        dialog.open(&entry);
        assert_eq!(dialog.chosen.as_deref(), Some("coder"), "the first one is under the hand as it opens");
        dialog.edit(entry.lineup("coder").expect("the lineup that is there"));
        assert_eq!(dialog.filter(), "", "editing starts with nothing written above the models");
        assert_eq!(dialog.shown(&entry).len(), 40, "an empty filter leaves every model");
        assert_eq!(dialog.steps(), ["acme/model-01", "acme/model-02", "acme/model-03"]);

        // A part of a name, matched over the whole of it: `model-3` keeps the three that carry a
        // three and the ten that end in one, in the order the provider listed them.
        dialog.filter_for("model-3".to_owned());
        let shown: Vec<&str> = dialog.shown(&entry).iter().map(|model| model.id.as_str()).collect();
        assert_eq!(shown.len(), 13, "{shown:?}");
        assert_eq!(shown.first(), Some(&"acme/model-03"));
        assert_eq!(shown.last(), Some(&"acme/model-39"));
        assert!(!shown.contains(&"acme/model-40"), "the filter is not a prefix of the name: {shown:?}");

        // What the filter hides is not thrown away: the steps already in the lineup are its own.
        assert_eq!(dialog.steps(), ["acme/model-01", "acme/model-02", "acme/model-03"]);
        dialog.add("acme/model-13");
        dialog.add("acme/model-23");
        assert_eq!(
            dialog.steps(),
            ["acme/model-01", "acme/model-02", "acme/model-03", "acme/model-13", "acme/model-23"]
        );

        // The first step has nowhere earlier to go, and the last none later: a request that failed
        // on the first model is what the order is for.
        dialog.pick_step(0);
        dialog.up();
        assert_eq!(
            dialog.steps(),
            ["acme/model-01", "acme/model-02", "acme/model-03", "acme/model-13", "acme/model-23"]
        );
        dialog.pick_step(1);
        dialog.up();
        assert_eq!(
            dialog.steps(),
            ["acme/model-02", "acme/model-01", "acme/model-03", "acme/model-13", "acme/model-23"]
        );
        dialog.down();
        assert_eq!(
            dialog.steps(),
            ["acme/model-01", "acme/model-02", "acme/model-03", "acme/model-13", "acme/model-23"]
        );
        dialog.down();
        assert_eq!(
            dialog.steps(),
            ["acme/model-01", "acme/model-03", "acme/model-02", "acme/model-13", "acme/model-23"]
        );
        dialog.down();
        assert_eq!(
            dialog.steps(),
            ["acme/model-01", "acme/model-03", "acme/model-13", "acme/model-02", "acme/model-23"]
        );
        dialog.pick_step(4);
        dialog.down();
        assert_eq!(
            dialog.steps(),
            ["acme/model-01", "acme/model-03", "acme/model-13", "acme/model-02", "acme/model-23"]
        );

        dialog.pick_step(1);
        dialog.remove();
        assert_eq!(dialog.steps(), ["acme/model-01", "acme/model-13", "acme/model-02", "acme/model-23"]);
        dialog.pick_step(99);
        dialog.remove();
        assert_eq!(dialog.steps(), ["acme/model-01", "acme/model-13", "acme/model-02"]);
    }

    #[test]
    fn the_same_model_twice_would_send_the_same_request_twice_so_it_is_not_put_in_twice() {
        let mut dialog = Lineups::new();
        dialog.new_lineup();
        dialog.add("acme/model-05");
        dialog.add("acme/model-05");
        dialog.add("acme/model-06");
        assert_eq!(dialog.steps(), ["acme/model-05", "acme/model-06"]);
    }

    #[test]
    fn what_a_step_costs_is_named_while_the_order_is_chosen_and_a_price_nobody_published_is_not() {
        let entry = entry();
        let mut dialog = Lineups::new();
        dialog.new_lineup();
        for id in ["acme/model-01", "acme/model-02", "acme/model-03"] {
            dialog.add(id);
        }
        // `model-01` is paid, `model-02` is not priced at all, `model-03` is free: only the first
        // is a step a request would spend money on, and nothing is said about the second.
        assert_eq!(dialog.paid(&entry), ["acme/model-01"]);
    }

    #[test]
    fn a_lineup_is_not_kept_without_a_name_it_can_hold_a_name_already_taken_or_a_step() {
        let mut entry = entry();
        entry.lineups.push(lineup(&entry, "coder", 2));
        let mut dialog = Lineups::new();

        // Nothing in it: a request that failed on the first model would have nothing to fall back to.
        dialog.new_lineup();
        dialog.name("yeni".to_owned());
        assert_eq!(dialog.build(held(&entry)), Err(Refusal::Empty));

        dialog.add("acme/model-01");
        let mut build = |name: &str, entry: &ProviderEntry| {
            dialog.name(name.to_owned());
            dialog.build(held(entry))
        };
        assert_eq!(build("yeni", &entry).map(|l| l.name.as_str().to_owned()), Ok("yeni".to_owned()));
        assert_eq!(build("yeni", &entry).map(|l| l.models), Ok(vec!["acme/model-01".to_owned()]));

        assert_eq!(build("2coder", &entry), Err(Refusal::Name(TagError::BadStart { character: '2' })));
        assert_eq!(build("coder/2", &entry), Err(Refusal::Name(TagError::Illegal { position: 6, character: '/' })));
        assert_eq!(build("  ", &entry), Err(Refusal::Name(TagError::Empty)));

        // Another lineup of this provider is in the way of the name, and that is said before
        // anything else about a lineup that is written.
        assert_eq!(build("coder", &entry), Err(Refusal::Taken("coder".to_owned())));
    }

    #[test]
    fn a_lineup_being_written_does_not_take_its_own_name_and_saving_keeps_it_on_the_list() {
        let mut entry = entry();
        let mut dialog = Lineups::new();
        dialog.open(&entry);
        dialog.new_lineup();
        dialog.name("coder".to_owned());
        dialog.add("acme/model-01");
        let saved = dialog.build(held(&entry)).expect("a lineup with a name and a step");
        entry.lineups.push(saved);
        assert_eq!(entry.lineup("coder").map(|lineup| lineup.models.clone()), Some(vec!["acme/model-01".to_owned()]));

        dialog.saved("coder");
        assert!(dialog.editor.is_none(), "the editor is left and the list stands in its place");
        assert_eq!(dialog.chosen.as_deref(), Some("coder"), "what was saved is the one under the hand");

        // Editing it under the same name takes nothing, since there is one lineup of that name.
        dialog.edit(entry.lineup("coder").expect("the lineup that is there"));
        assert!(dialog.editing("coder"));
        dialog.add("acme/model-02");
        let built = dialog.build(held(&entry)).expect("a lineup of the same name is not taken by itself");
        assert_eq!(built.name.as_str(), "coder");
        assert_eq!(built.models, ["acme/model-01", "acme/model-02"]);
    }

    #[test]
    fn a_lineup_deleted_from_under_the_hand_leaves_one_of_the_rest_under_it() {
        let mut entry = entry();
        for name in ["coder", "fast", "yavas"] {
            entry.lineups.push(lineup(&entry, name, 1));
        }
        let mut dialog = Lineups::new();
        dialog.open(&entry);
        assert_eq!(dialog.chosen.as_deref(), Some("coder"));

        dialog.choose("fast");
        entry.lineups.retain(|lineup| lineup.name.as_str() != "fast");
        dialog.kept(&names(&entry));
        assert_eq!(
            dialog.chosen.as_deref(),
            Some("coder"),
            "the one under the hand went, so the first of the rest takes it"
        );

        dialog.choose("coder");
        entry.lineups.retain(|lineup| lineup.name.as_str() != "coder");
        dialog.kept(&names(&entry));
        assert_eq!(dialog.chosen.as_deref(), Some("yavas"), "the one that is left is the one under the hand");

        entry.lineups.clear();
        dialog.kept(&names(&entry));
        assert_eq!(dialog.chosen, None, "a provider with no lineups has none to point at");
    }
}
