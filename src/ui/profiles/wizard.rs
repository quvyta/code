//! The profile wizard's state: the six steps (five for a profile that signs in to nothing), what
//! has been chosen on each of them, and how far the image build and the login have got.
//!
//! The state changes by message only. Nothing here starts work or waits for it, so every state
//! the screen can be in — including a failed build and an interrupted login — is reached in a
//! test without a container engine anywhere near it.

use std::sync::Arc;

use qframe::runtime::TaskId;
use qframe::widgets::{LogBuffer, TerminalSession};

use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};

use super::work::{LoginContainer, Problem};

/// How many lines of a build are kept. A build talks for minutes; this is more than any of them
/// says and still a fixed amount of memory.
const LOG_LINES: usize = 4000;

/// One page of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Which harness, and what the profile is called.
    Harness,
    /// The harness as it comes, or set up the way QCode runs it.
    Template,
    /// What the profile signs in with.
    Account,
    /// What the container may see and reach.
    Permissions,
    /// Building the image the profile's containers start from.
    Image,
    /// Signing in, so that projects that use the profile have a login to copy.
    Login,
}

impl Stage {
    /// Every page, in order.
    pub const ALL: [Self; 6] =
        [Self::Harness, Self::Template, Self::Account, Self::Permissions, Self::Image, Self::Login];

    /// The pages of a profile that has no login: everything up to the image. The sign-in page is
    /// last so that leaving it out moves no other page.
    pub const WITHOUT_LOGIN: [Self; 5] = [Self::Harness, Self::Template, Self::Account, Self::Permissions, Self::Image];

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
    /// How `Assets/` is mounted.
    pub assets: MountAccess,
    /// What the container may reach.
    pub network: NetworkMode,
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
}

impl Draft {
    /// A new draft for a workspace whose profiles are called `taken`.
    #[must_use]
    pub fn new(taken: impl IntoIterator<Item = String>) -> Self {
        let harness = HarnessKind::ALL[0];
        let mut draft = Self {
            stage: Stage::Harness,
            name: String::new(),
            renamed: false,
            harness,
            template: Template::Recommended,
            account: first_account(harness),
            // The assets folder is where the person keeps what the harness is meant to use and
            // add to, so a new profile may write there unless the person narrows it.
            assets: MountAccess::ReadWrite,
            network: NetworkMode::Full,
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
            assets: profile.assets,
            network: profile.network,
            build: Build::Done,
            log: LogBuffer::new(LOG_LINES),
            login: Login::Waiting,
            taken: Vec::new(),
            only_login: true,
        }
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

    /// The pages this draft goes through. A profile that signs in to nothing has no sign-in
    /// page: a step that could only say "nothing to do" would still have to be walked through.
    #[must_use]
    pub fn stages(&self) -> &'static [Stage] {
        if self.account.needs_login() { &Stage::ALL } else { &Stage::WITHOUT_LOGIN }
    }

    /// The profile the draft describes, if the name is usable.
    #[must_use]
    pub fn profile(&self) -> Option<Profile> {
        Some(Profile {
            name: SafeName::from_display(&self.name)?,
            harness: self.harness,
            template: self.template,
            account: self.account,
            assets: self.assets,
            network: self.network,
        })
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
            Stage::Image if self.build != Build::Done => Some(Blocked::NoImage),
            _ => None,
        }
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
        let mut draft = Draft::new([]);
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
        let mut draft = Draft::new(["claude-code".to_owned()]);
        assert_eq!(draft.blocked(), Some(Blocked::NameTaken));
        draft.name = "  ".to_owned();
        assert_eq!(draft.blocked(), Some(Blocked::NameEmpty));
        draft.name = "Günlük Çalışma".to_owned();
        assert_eq!(draft.blocked(), None);
        assert_eq!(draft.safe_name().expect("the name folds").as_str(), "gunluk-calisma");
    }

    #[test]
    fn the_image_page_cannot_be_left_before_there_is_an_image() {
        let mut draft = Draft::new([]);
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
        let mut draft = Draft::new([]);
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
        let mut draft = Draft::new([]);
        draft.stage = Stage::Permissions;
        draft.go_to(Stage::Image.index());
        assert_eq!(draft.stage, Stage::Permissions, "a page that has not been reached is not chosen");
        draft.go_to(Stage::Template.index());
        assert_eq!(draft.stage, Stage::Template);
    }

    #[test]
    fn choosing_a_harness_keeps_the_account_type_one_that_harness_can_use() {
        let mut draft = Draft::new([]);
        draft.account = AccountKind::ApiKey;
        draft.choose_harness(HarnessKind::GeminiCli);
        assert!(draft.harness.supports(draft.account));
    }

    #[test]
    fn opencode_starts_free_and_the_others_never_offer_it() {
        let mut draft = Draft::new([]);
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
        let mut draft = Draft::new([]);
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
        assert_eq!(Draft::new([]).assets, MountAccess::ReadWrite);
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
        let mut source = Draft::new([]);
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
        let mut draft = Draft::new([]);
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

    #[test]
    fn every_page_has_a_place_and_the_last_one_leads_nowhere() {
        for (index, stage) in Stage::ALL.iter().enumerate() {
            assert_eq!(stage.index(), index);
            assert_eq!(Stage::at(index), Some(*stage));
        }
        assert_eq!(Stage::at(Stage::ALL.len()), None);
    }
}
