//! What QCode knows about a profile on this machine: whether its image is built and whether a
//! login is kept for it.
//!
//! Both answers come from the engine and neither is ever guessed. Until the engine has been
//! asked, the answer is [`Readiness::Unknown`] and the screen says so rather than showing a
//! profile as ready to run.

use crate::profile::{Profile, SafeName};

/// Whether something the engine holds is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// The engine has not been asked yet, or could not answer.
    Unknown,
    /// The engine has it.
    Present,
    /// The engine was asked and does not have it.
    Missing,
}

impl Readiness {
    /// The answer to "is it there?", when there is one.
    #[must_use]
    pub fn of(present: bool) -> Self {
        if present { Self::Present } else { Self::Missing }
    }
}

/// What the engine answered about one profile, carried by name so an answer that arrives late
/// still finds the profile it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The profile the answer is about.
    pub name: SafeName,
    /// Whether the profile's image is built.
    pub image: Readiness,
    /// Whether a login is kept in the profile's credentials volume.
    pub identity: Readiness,
}

/// One profile as the list shows it: the definition, and what the engine says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The profile as its definition file describes it.
    pub profile: Profile,
    /// Whether the profile's image is built.
    pub image: Readiness,
    /// Whether a login is kept for the profile.
    pub identity: Readiness,
}

impl Row {
    /// A row nothing has been asked about yet.
    #[must_use]
    pub fn new(profile: Profile) -> Self {
        Self { profile, image: Readiness::Unknown, identity: Readiness::Unknown }
    }

    /// Whether a project could open this profile right now: the image is there and so is a
    /// login, unless the profile signs in to nothing. An unknown answer is not a yes.
    #[must_use]
    pub fn is_runnable(&self) -> bool {
        self.image == Readiness::Present && (self.is_signed_in() || !self.profile.account.needs_login())
    }

    /// Whether a login is known to be kept for the profile.
    #[must_use]
    pub fn is_signed_in(&self) -> bool {
        self.identity == Readiness::Present
    }

    /// Takes an answer that is about this profile; answers about another one are ignored.
    pub fn apply(&mut self, status: &Status) {
        if status.name == self.profile.name {
            self.image = status.image;
            self.identity = status.identity;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Template};

    fn profile(name: &str) -> Profile {
        Profile {
            name: SafeName::parse(name).expect("the name is safe"),
            harness: HarnessKind::ClaudeCode,
            template: Template::Recommended,
            account: AccountKind::Subscription,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
        }
    }

    #[test]
    fn nothing_is_ready_before_the_engine_has_answered() {
        let row = Row::new(profile("claude-sub"));
        assert_eq!(row.image, Readiness::Unknown);
        assert_eq!(row.identity, Readiness::Unknown);
        assert!(!row.is_runnable(), "an unanswered profile is never treated as ready");
    }

    #[test]
    fn a_profile_is_runnable_only_with_both_an_image_and_a_login() {
        let mut row = Row::new(profile("claude-sub"));
        row.apply(&Status {
            name: SafeName::parse("claude-sub").expect("safe"),
            image: Readiness::Present,
            identity: Readiness::Missing,
        });
        assert!(!row.is_runnable());
        row.apply(&Status {
            name: SafeName::parse("claude-sub").expect("safe"),
            image: Readiness::Present,
            identity: Readiness::Present,
        });
        assert!(row.is_runnable());
    }

    #[test]
    fn a_free_profile_is_runnable_with_its_image_alone() {
        let mut free = profile("oc");
        free.harness = HarnessKind::OpenCode;
        free.account = AccountKind::Free;
        let mut row = Row::new(free);
        assert!(!row.is_runnable(), "an image nobody has checked is still not a yes");
        row.apply(&Status {
            name: SafeName::parse("oc").expect("safe"),
            image: Readiness::Present,
            identity: Readiness::Missing,
        });
        assert!(row.is_runnable(), "no login is kept, and none is needed");
    }

    #[test]
    fn an_answer_about_another_profile_is_left_alone() {
        let mut row = Row::new(profile("claude-sub"));
        row.apply(&Status {
            name: SafeName::parse("codex-key").expect("safe"),
            image: Readiness::Present,
            identity: Readiness::Present,
        });
        assert_eq!(row.image, Readiness::Unknown);
        assert_eq!(row.identity, Readiness::Unknown);
    }
}
