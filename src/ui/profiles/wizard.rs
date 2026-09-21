//! The profile wizard's state: the seven steps (six for a profile that signs in to nothing), what
//! has been chosen on each of them, and how far the image build and the login have got.
//!
//! The state changes by message only. Nothing here starts work or waits for it, so every state
//! the screen can be in — including a failed build and an interrupted login — is reached in a
//! test without a container engine anywhere near it.

use std::sync::Arc;

use qframe::runtime::TaskId;
use qframe::widgets::{LogBuffer, TerminalSession};

use crate::base::{Os, Refusal};
use crate::profile::{
    AccountKind, Extra, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template,
};
use crate::provider::{Model, ProviderEntry};

use super::recipe;
use super::work::{LoginContainer, Problem};

/// How many lines of a build are kept. A build talks for minutes; this is more than any of them
/// says and still a fixed amount of memory.
const LOG_LINES: usize = 4000;

/// One page of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Which harness, and what the profile is called.
    Harness,
    /// Which operating system the image is built on. Right after the harness, because a system
    /// can refuse a harness, and the person should learn that before choosing anything else.
    System,
    /// The harness as it comes, or set up the way QCode runs it.
    Template,
    /// What the profile signs in with.
    Account,
    /// What the container may see and reach.
    Permissions,
    /// Building the image the profile's containers start from.
    Image,
    /// Signing in, so that workspaces that use the profile have a login to copy.
    Login,
}

impl Stage {
    /// Every page, in order.
    pub const ALL: [Self; 7] =
        [Self::Harness, Self::System, Self::Template, Self::Account, Self::Permissions, Self::Image, Self::Login];

    /// The pages of a profile that has no login: everything up to the image. The sign-in page is
    /// last so that leaving it out moves no other page.
    pub const WITHOUT_LOGIN: [Self; 6] =
        [Self::Harness, Self::System, Self::Template, Self::Account, Self::Permissions, Self::Image];

    /// Which page this is, counting from the first.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|stage| *stage == self).unwrap_or(0)
    }

    /// The page at `index`, if there is one.
    #[must_use]
    pub fn at(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// How far the image build has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Build {
    /// It has not been started.
    Waiting,
    /// It is running; the task is kept so it can be stopped.
    Running(TaskId),
    /// The image is built.
    Done,
    /// The engine refused or could not be reached; nothing was left behind.
    Failed(Problem),
    /// The person stopped it; the half-made image was removed.
    Stopped,
}

/// Why a login ended without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unfinished {
    /// The person stopped it, or left the step.
    Stopped,
    /// The harness closed, or the person said they were done, and no login file was there.
    Missing,
}

/// How far the login has got.
#[derive(Debug)]
pub enum Login {
    /// It has not been started: the page explains what is about to happen.
    Waiting,
    /// The container is being made.
    Opening,
    /// The harness is running on a terminal and the person is signing in.
    Running {
        /// The terminal the harness draws on.
        session: TerminalSession,
        /// The container it runs in, and where its login will be picked up. It is shared
        /// because whoever clears it away — the page, or the wizard closing — may not be the
        /// one holding it.
        container: Arc<LoginContainer>,
    },
    /// The harness has stopped and the login is being looked for and stored.
    Storing,
    /// A login was found and stored; the number is how many files it took.
    Stored(usize),
    /// No login was stored, and the profile says so rather than pretending.
    Unfinished(Unfinished),
    /// The engine or the machine refused.
    Failed(Problem),
}

impl Login {
    /// Whether a login was stored. Only a stored login counts; everything else, including a
    /// terminal that closed by itself, does not.
    #[must_use]
    pub fn is_stored(&self) -> bool {
        matches!(self, Self::Stored(_))
    }

    /// Whether something is going on that the person should not be interrupted in the middle of.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Opening | Self::Storing)
    }
}

/// Everything the wizard has been told so far.
#[derive(Debug)]
pub struct Draft {
    /// The page being shown.
    pub stage: Stage,
    /// The name as it has been typed.
    pub name: String,
    /// Whether the name has been edited by hand; until it has, choosing a harness renames it.
    pub renamed: bool,
    /// The chosen harness.
    pub harness: HarnessKind,
    /// The chosen template.
    pub template: Template,
    /// The chosen account type.
    pub account: AccountKind,
    /// The providers the person has added, as the Providers page last had them, for the account
    /// page to offer when the chosen harness can be pointed at one of its own.
    pub providers: Vec<ProviderEntry>,
    /// The tag of the chosen provider, while `account` is [`AccountKind::Provider`].
    pub provider_tag: Option<String>,
    /// The model asked for, while `account` is [`AccountKind::Provider`].
    pub provider_model: Option<String>,
    /// How `Assets/` is mounted.
    pub assets: MountAccess,
    /// What the container may reach.
    pub network: NetworkMode,
    /// The parts of QCode high's additions switched off. Kept while another template or harness
    /// is looked at, so going back finds them as they were left; only the ones the chosen
    /// template and harness add reach the profile.
    pub without: Vec<Extra>,
    /// The system the image is built on.
    pub os: Os,
    /// How far the build has got.
    pub build: Build,
    /// Every line the build has printed.
    pub log: LogBuffer,
    /// How far the login has got.
    pub login: Login,
    /// The names already taken, so a clash is caught before anything is built.
    taken: Vec<String>,
    /// Whether this draft exists only to sign an existing profile in.
    only_login: bool,
}

/// Why a step cannot be left yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// The name is empty, or holds nothing a name can be made of.
    NameEmpty,
    /// Another profile already has this name.
    NameTaken,
    /// The image has not been built.
    NoImage,
    /// The account is a provider of one's own, and no provider and model have both been chosen.
    NoProvider,
    /// The chosen system does not run the chosen harness, for this reason.
    Unsupported(Refusal),
}

impl Draft {
    /// A new draft for a store whose profiles are called `taken`, offering `providers` on the
    /// account page for a harness that can be pointed at one.
    #[must_use]
    pub fn new(taken: impl IntoIterator<Item = String>, providers: Vec<ProviderEntry>) -> Self {
        let harness = HarnessKind::ALL[0];
        let mut draft = Self {
            stage: Stage::Harness,
            name: String::new(),
            renamed: false,
            harness,
            template: Template::Recommended,
            account: first_account(harness),
            providers,
            provider_tag: None,
            provider_model: None,
            // The assets folder is where the person keeps what the harness is meant to use and
            // add to, so a new profile may write there unless the person narrows it.
            assets: MountAccess::ReadWrite,
            network: NetworkMode::Full,
            without: Vec::new(),
            os: Os::Debian,
            build: Build::Waiting,
            log: LogBuffer::new(LOG_LINES),
            login: Login::Waiting,
            taken: taken.into_iter().collect(),
            only_login: false,
        };
        draft.name = draft.suggested_name();
        draft
    }

    /// A draft that only signs `profile` in: the image is already there, so the login page is
    /// the whole of it. This is how a login that was interrupted is picked up again.
    #[must_use]
    pub fn for_login(profile: &Profile) -> Self {
        Self {
            stage: Stage::Login,
            name: profile.name.as_str().to_owned(),
            renamed: true,
            harness: profile.harness,
            template: profile.template,
            account: profile.account,
            providers: Vec::new(),
            provider_tag: profile.provider.as_ref().map(|provider| provider.tag.clone()),
            provider_model: profile.provider.as_ref().map(|provider| provider.model.clone()),
            assets: profile.assets,
            network: profile.network,
            without: profile.without.clone(),
            os: profile.os,
            build: Build::Done,
            log: LogBuffer::new(LOG_LINES),
            login: Login::Waiting,
            taken: Vec::new(),
            only_login: true,
        }
    }

    /// Whether the build failed because a step of QCode high could not download what it
    /// installs, which the step says in words of its own (`recipe::HIGH_FAILED`) in the log.
    #[must_use]
    pub fn high_download_failed(&self) -> bool {
        matches!(self.build, Build::Failed(_)) && self.log.iter().any(|line| line.text().contains(recipe::HIGH_FAILED))
    }

    /// Whether this draft is only a login for a profile that already exists.
    #[must_use]
    pub fn is_only_login(&self) -> bool {
        self.only_login
    }

    /// The name a profile of this harness gets while nobody has typed one.
    fn suggested_name(&self) -> String {
        self.harness.record().id.to_owned()
    }

    /// Chooses a harness. While the name has not been touched it follows the harness, and a
    /// different harness brings its own first account type: the account page comes after this
    /// one, and each harness offers its own list, headed by the one most people start with.
    pub fn choose_harness(&mut self, harness: HarnessKind) {
        let changed = harness != self.harness;
        self.harness = harness;
        if !self.renamed {
            self.name = self.suggested_name();
        }
        if changed || !harness.supports(self.account) {
            self.account = first_account(harness);
        }
    }

    /// The provider the tag `tag` names among the ones the person has added, if there is one.
    #[must_use]
    fn provider_entry(&self, tag: &str) -> Option<&ProviderEntry> {
        self.providers.iter().find(|provider| provider.tag.as_str() == tag)
    }

    /// The models of the chosen provider, or none while no provider is chosen or it offers none
    /// yet: a provider the person has not asked to list is added with an empty list, and its own
    /// page is where that is fixed.
    #[must_use]
    pub fn provider_models(&self) -> &[Model] {
        self.provider_tag.as_deref().and_then(|tag| self.provider_entry(tag)).map_or(&[], |entry| &entry.models)
    }

    /// Chooses the provider tagged `tag`, and with it the first model it is known to offer, so
    /// that picking a provider that already has one model chosen it for the person rather than
    /// leaving a second, empty choice behind it.
    pub fn choose_provider(&mut self, tag: &str) {
        self.provider_tag = Some(tag.to_owned());
        self.provider_model =
            self.provider_entry(tag).and_then(|entry| entry.models.first()).map(|model| model.id.clone());
    }

    /// Offers `providers` from now on, as the file has them after the person was away on the
    /// Providers page. A provider account keeps the provider and model it had where they are
    /// still there, and otherwise takes the first of what is offered now, the way picking the
    /// account does: the page told the person to add one and come back, so the one they added is
    /// the one they meant.
    pub fn offer_providers(&mut self, providers: Vec<ProviderEntry>) {
        self.providers = providers;
        if self.account != AccountKind::Provider {
            return;
        }
        let kept = self.provider_tag.clone().filter(|tag| self.provider_entry(tag).is_some());
        match kept.or_else(|| self.providers.first().map(|entry| entry.tag.to_string())) {
            Some(tag) => {
                let model = self.provider_model.clone();
                self.choose_provider(&tag);
                let offered = self.provider_models();
                if let Some(model) = model.filter(|id| offered.iter().any(|offered| offered.id == *id)) {
                    self.provider_model = Some(model);
                }
            }
            None => {
                self.provider_tag = None;
                self.provider_model = None;
            }
        }
    }

    /// Chooses the model `id` of the provider already chosen.
    pub fn choose_provider_model(&mut self, id: &str) {
        self.provider_model = Some(id.to_owned());
    }

    /// The pages this draft goes through. A profile that signs in to nothing has no sign-in
    /// page: a step that could only say "nothing to do" would still have to be walked through.
    #[must_use]
    pub fn stages(&self) -> &'static [Stage] {
        if self.account.needs_login() { &Stage::ALL } else { &Stage::WITHOUT_LOGIN }
    }

    /// The profile the draft describes, if the name is usable and, for a provider account, both
    /// a provider and a model have been chosen.
    #[must_use]
    pub fn profile(&self) -> Option<Profile> {
        let provider = match self.account {
            AccountKind::Provider => {
                Some(ProviderChoice { tag: self.provider_tag.clone()?, model: self.provider_model.clone()? })
            }
            _ => None,
        };
        Some(Profile {
            name: SafeName::from_display(&self.name)?,
            harness: self.harness,
            template: self.template,
            account: self.account,
            provider,
            assets: self.assets,
            network: self.network,
            without: self.without.iter().copied().filter(|extra| self.offers(*extra)).collect(),
            os: self.os,
        })
    }

    /// Whether the chosen template adds `extra` for the chosen harness, so that it is offered.
    #[must_use]
    pub fn offers(&self, extra: Extra) -> bool {
        self.template.extras(self.harness).contains(&extra)
    }

    /// Whether `extra` is offered and switched on.
    #[must_use]
    pub fn has(&self, extra: Extra) -> bool {
        self.offers(extra) && !self.without.contains(&extra)
    }

    /// Whether anything at all is added to the image beside what QCode basic writes: false for
    /// QCode basic and base, and for QCode high with every part switched off.
    #[must_use]
    pub fn adds_anything(&self) -> bool {
        self.template.extras(self.harness).into_iter().any(|extra| self.has(extra))
    }

    /// Switches `extra` on or off.
    pub fn switch(&mut self, extra: Extra, on: bool) {
        self.without.retain(|left| *left != extra);
        if !on {
            self.without.push(extra);
        }
    }

    /// The name as it will be written, which is not always the name as it was typed.
    #[must_use]
    pub fn safe_name(&self) -> Option<SafeName> {
        SafeName::from_display(&self.name)
    }

    /// Why the current page cannot be left, if it cannot.
    #[must_use]
    pub fn blocked(&self) -> Option<Blocked> {
        match self.stage {
            Stage::Harness => match self.safe_name() {
                None => Some(Blocked::NameEmpty),
                Some(name) if self.taken.iter().any(|taken| taken == name.as_str()) => Some(Blocked::NameTaken),
                Some(_) => None,
            },
            // A harness the system cannot run is never built there: the page says why and stays
            // until the person picks another system or goes back for another harness.
            Stage::System => self.os.refuses(self.harness).map(Blocked::Unsupported),
            Stage::Account if self.account == AccountKind::Provider && self.profile_provider().is_none() => {
                Some(Blocked::NoProvider)
            }
            Stage::Image if self.build != Build::Done => Some(Blocked::NoImage),
            _ => None,
        }
    }

    /// The provider and model the account page has chosen so far, when both are there.
    #[must_use]
    fn profile_provider(&self) -> Option<ProviderChoice> {
        Some(ProviderChoice { tag: self.provider_tag.clone()?, model: self.provider_model.clone()? })
    }

    /// Whether the wizard should show its buttons as working and ignore them.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(self.build, Build::Running(_)) || self.login.is_busy()
    }

    /// Moves to the next page, unless the current one is blocked or something is running.
    pub fn advance(&mut self) -> Option<Blocked> {
        if self.is_busy() {
            return None;
        }
        if let Some(blocked) = self.blocked() {
            return Some(blocked);
        }
        if let Some(next) = self.stages().get(self.stage.index() + 1).copied() {
            self.stage = next;
        }
        None
    }

    /// Moves back a page.
    pub fn back(&mut self) {
        if self.is_busy() {
            return;
        }
        if let Some(previous) = self.stage.index().checked_sub(1).and_then(|index| self.stages().get(index).copied()) {
            self.stage = previous;
        }
    }

    /// Goes back to a page that has been finished. Pages ahead of the current one are not
    /// reachable this way, and neither is any page while work is running.
    pub fn go_to(&mut self, index: usize) {
        if self.is_busy() {
            return;
        }
        if let Some(stage) = self.stages().get(index).copied()
            && index < self.stage.index()
        {
            self.stage = stage;
        }
    }
}

/// The account type a harness is offered with before anyone chooses: the first of its own list.
fn first_account(harness: HarnessKind) -> AccountKind {
    harness.record().accounts.first().copied().unwrap_or(AccountKind::Subscription)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_draft_is_named_after_its_harness_until_it_is_renamed() {
        let mut draft = Draft::new([], Vec::new());
        assert_eq!(draft.name, "claude-code");
        draft.choose_harness(HarnessKind::Codex);
        assert_eq!(draft.name, "codex");
        draft.renamed = true;
        draft.name = "work".to_owned();
        draft.choose_harness(HarnessKind::OpenCode);
        assert_eq!(draft.name, "work", "a name typed by hand is never overwritten");
    }

    #[test]
    fn a_name_that_is_already_taken_blocks_the_first_page() {
        let mut draft = Draft::new(["claude-code".to_owned()], Vec::new());
        assert_eq!(draft.blocked(), Some(Blocked::NameTaken));
        draft.name = "  ".to_owned();
        assert_eq!(draft.blocked(), Some(Blocked::NameEmpty));
        draft.name = "Günlük Çalışma".to_owned();
        assert_eq!(draft.blocked(), None);
        assert_eq!(draft.safe_name().expect("the name folds").as_str(), "gunluk-calisma");
    }

    #[test]
    fn the_image_page_cannot_be_left_before_there_is_an_image() {
        let mut draft = Draft::new([], Vec::new());
        draft.stage = Stage::Image;
        assert_eq!(draft.blocked(), Some(Blocked::NoImage));
        assert_eq!(draft.advance(), Some(Blocked::NoImage));
        assert_eq!(draft.stage, Stage::Image);
        draft.build = Build::Failed(Problem::NoLogin);
        assert_eq!(draft.blocked(), Some(Blocked::NoImage), "a failed build is not an image");
        draft.build = Build::Stopped;
        assert_eq!(draft.blocked(), Some(Blocked::NoImage), "a stopped build is not an image");
        draft.build = Build::Done;
        assert_eq!(draft.advance(), None);
        assert_eq!(draft.stage, Stage::Login);
    }

    #[test]
    fn nothing_moves_while_the_build_is_running() {
        let mut draft = Draft::new([], Vec::new());
        draft.stage = Stage::Image;
        draft.build = Build::Running(qframe::runtime::Task::<()>::new("build", |_| Ok(())).id());
        assert!(draft.is_busy());
        assert_eq!(draft.advance(), None);
        assert_eq!(draft.stage, Stage::Image, "a running build keeps the page");
        draft.back();
        assert_eq!(draft.stage, Stage::Image);
    }

    #[test]
    fn a_finished_page_can_be_gone_back_to_and_one_ahead_cannot() {
        let mut draft = Draft::new([], Vec::new());
        draft.stage = Stage::Permissions;
        draft.go_to(Stage::Image.index());
        assert_eq!(draft.stage, Stage::Permissions, "a page that has not been reached is not chosen");
        draft.go_to(Stage::Template.index());
        assert_eq!(draft.stage, Stage::Template);
    }

    #[test]
    fn choosing_a_harness_keeps_the_account_type_one_that_harness_can_use() {
        let mut draft = Draft::new([], Vec::new());
        draft.account = AccountKind::ApiKey;
        draft.choose_harness(HarnessKind::GeminiCli);
        assert!(draft.harness.supports(draft.account));
    }

    #[test]
    fn opencode_starts_free_and_the_others_never_offer_it() {
        let mut draft = Draft::new([], Vec::new());
        assert_eq!(draft.account, AccountKind::Subscription, "Claude Code starts with a subscription");
        draft.choose_harness(HarnessKind::OpenCode);
        assert_eq!(draft.account, AccountKind::Free, "opencode is chosen with its free use");
        draft.account = AccountKind::ApiKey;
        draft.choose_harness(HarnessKind::OpenCode);
        assert_eq!(draft.account, AccountKind::ApiKey, "choosing the same harness again keeps the account");
        for harness in [HarnessKind::ClaudeCode, HarnessKind::GeminiCli, HarnessKind::Codex] {
            draft.choose_harness(HarnessKind::OpenCode);
            draft.choose_harness(harness);
            assert_ne!(draft.account, AccountKind::Free, "{harness:?}");
        }
    }

    #[test]
    fn a_free_profile_has_no_sign_in_page_and_ends_with_its_image() {
        let mut draft = Draft::new([], Vec::new());
        draft.choose_harness(HarnessKind::OpenCode);
        assert_eq!(draft.stages(), Stage::WITHOUT_LOGIN);
        draft.stage = Stage::Image;
        draft.build = Build::Done;
        assert_eq!(draft.advance(), None);
        assert_eq!(draft.stage, Stage::Image, "there is no page after the image");
        draft.go_to(Stage::Login.index());
        assert_eq!(draft.stage, Stage::Image);
        draft.account = AccountKind::Subscription;
        assert_eq!(draft.stages(), Stage::ALL);
        draft.advance();
        assert_eq!(draft.stage, Stage::Login);
    }

    #[test]
    fn a_new_profile_may_write_to_its_assets_unless_narrowed() {
        assert_eq!(Draft::new([], Vec::new()).assets, MountAccess::ReadWrite);
    }

    #[test]
    fn a_login_counts_only_once_it_has_been_stored() {
        assert!(!Login::Waiting.is_stored());
        assert!(!Login::Unfinished(Unfinished::Missing).is_stored());
        assert!(!Login::Unfinished(Unfinished::Stopped).is_stored());
        assert!(!Login::Failed(Problem::NoLogin).is_stored());
        assert!(Login::Stored(1).is_stored());
    }

    #[test]
    fn a_login_only_draft_starts_on_the_login_page_of_a_profile_that_already_has_an_image() {
        let mut source = Draft::new([], Vec::new());
        source.name = "claude-sub".to_owned();
        let profile = source.profile().expect("the name folds");
        let draft = Draft::for_login(&profile);
        assert!(draft.is_only_login());
        assert_eq!(draft.stage, Stage::Login);
        assert_eq!(draft.build, Build::Done, "the image is already there");
        assert!(!draft.login.is_stored(), "opening the page signs nothing in");
        assert_eq!(draft.profile(), Some(profile));
    }

    #[test]
    fn a_draft_becomes_the_profile_it_describes() {
        let mut draft = Draft::new([], Vec::new());
        draft.name = "Claude Abonelik".to_owned();
        draft.template = Template::Recommended;
        draft.assets = MountAccess::ReadWrite;
        draft.network = NetworkMode::None;
        let profile = draft.profile().expect("the name folds");
        assert_eq!(profile.name.as_str(), "claude-abonelik");
        assert_eq!(profile.image(), "qcode/profile/claude-abonelik");
        assert_eq!(profile.assets, MountAccess::ReadWrite);
        assert_eq!(profile.network, NetworkMode::None);
    }

    /// A provider entry for the tests below, with `models` already known the way the Providers
    /// page leaves them once someone has listed them.
    fn entry(tag: &str, models: &[&str]) -> ProviderEntry {
        let mut entry = ProviderEntry::new(
            crate::provider::Tag::parse(tag).expect("a tag"),
            crate::provider::ProviderKind::Ollama,
            "http://127.0.0.1:11434",
        );
        entry.models = models.iter().map(|id| crate::provider::Model::new(*id)).collect();
        entry
    }

    #[test]
    fn choosing_a_provider_account_page_cannot_be_left_until_a_provider_and_model_are_chosen() {
        let mut draft = Draft::new([], vec![entry("ev1", &["qwen3.8"])]);
        draft.choose_harness(HarnessKind::ClaudeCode);
        draft.account = AccountKind::Provider;
        draft.stage = Stage::Account;
        assert_eq!(draft.blocked(), Some(Blocked::NoProvider));
        draft.choose_provider("ev1");
        assert_eq!(draft.provider_tag.as_deref(), Some("ev1"));
        assert_eq!(draft.provider_model.as_deref(), Some("qwen3.8"), "the first model is chosen along with it");
        assert_eq!(draft.blocked(), None);
    }

    #[test]
    fn a_provider_with_no_models_listed_yet_still_blocks_the_page() {
        let mut draft = Draft::new([], vec![entry("ev1", &[])]);
        draft.account = AccountKind::Provider;
        draft.stage = Stage::Account;
        draft.choose_provider("ev1");
        assert_eq!(draft.provider_model, None, "nothing to choose from yet");
        assert_eq!(draft.blocked(), Some(Blocked::NoProvider));
        assert!(draft.provider_models().is_empty());
    }

    #[test]
    fn a_second_model_can_be_chosen_of_the_same_provider() {
        let mut draft = Draft::new([], vec![entry("ev1", &["qwen3.8", "qwen3.8-32k"])]);
        draft.choose_provider("ev1");
        draft.choose_provider_model("qwen3.8-32k");
        assert_eq!(draft.provider_model.as_deref(), Some("qwen3.8-32k"));
        assert_eq!(draft.provider_models().len(), 2);
    }

    #[test]
    fn a_provider_profile_carries_its_tag_and_model_and_an_incomplete_one_makes_no_profile() {
        let mut draft = Draft::new([], vec![entry("ev1", &["qwen3.8"])]);
        draft.name = "ev-tab".to_owned();
        draft.account = AccountKind::Provider;
        assert_eq!(draft.profile(), None, "nothing chosen yet");
        draft.choose_provider("ev1");
        let profile = draft.profile().expect("a provider and a model are both chosen");
        let provider = profile.provider.expect("a provider profile carries one");
        assert_eq!(provider.tag, "ev1");
        assert_eq!(provider.model, "qwen3.8");
    }

    #[test]
    fn a_system_that_cannot_run_the_harness_keeps_its_page_and_says_why() {
        let mut draft = Draft::new([], Vec::new());
        draft.choose_harness(HarnessKind::GeminiCli);
        draft.advance();
        assert_eq!(draft.stage, Stage::System, "the system is chosen right after the harness");
        draft.os = Os::Alpine;
        assert_eq!(draft.advance(), Some(Blocked::Unsupported(Refusal::TerminalLibrary)));
        assert_eq!(draft.stage, Stage::System, "Gemini CLI is never built on Alpine");
        draft.os = Os::Arch;
        assert_eq!(draft.advance(), None);
        assert_eq!(draft.stage, Stage::Template);
        assert_eq!(draft.profile().map(|profile| profile.os), Some(Os::Arch));
    }

    #[test]
    fn a_new_draft_is_built_on_debian_and_a_login_draft_keeps_its_profiles_system() {
        let mut draft = Draft::new([], Vec::new());
        assert_eq!(draft.os, Os::Debian);
        draft.os = Os::Ubuntu;
        let profile = draft.profile().expect("a profile");
        assert_eq!(Draft::for_login(&profile).os, Os::Ubuntu);
    }

    #[test]
    fn every_page_has_a_place_and_the_last_one_leads_nowhere() {
        for (index, stage) in Stage::ALL.iter().enumerate() {
            assert_eq!(stage.index(), index);
            assert_eq!(Stage::at(index), Some(*stage));
        }
        assert_eq!(Stage::at(Stage::ALL.len()), None);
    }
}
