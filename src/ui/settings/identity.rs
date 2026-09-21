//! Logins, profile by profile: whether one is stored, refreshing it into the workspaces that use
//! it, and signing out.

use qframe::prelude::*;
use qframe::widgets::{EmptyState, Skeleton};

use super::Msg;
use super::engine::Gate;
use crate::profile::SafeName;

/// How many faint lines stand in for the profiles while the store is being read. Three is
/// what a first store tends to hold; the list replaces them in one frame.
const PLACEHOLDER_LINES: u16 = 3;

/// Cells between the two actions.
const GAP: u16 = 2;

/// A profile and whether QCode holds a login for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileIdentity {
    name: SafeName,
    stored: bool,
}

impl ProfileIdentity {
    /// A profile named `name`, whose credentials volume holds a login when `stored`.
    #[must_use]
    pub fn new(name: SafeName, stored: bool) -> Self {
        Self { name, stored }
    }

    /// The profile's name.
    #[must_use]
    pub fn name(&self) -> &SafeName {
        &self.name
    }

    /// Whether a login is stored for it.
    #[must_use]
    pub fn stored(&self) -> bool {
        self.stored
    }
}

/// Draws the logins: one row per profile with its state, and the two actions on the chosen one.
///
/// `profiles` is `None` while the store has not been read yet, which is drawn as faint lines
/// rather than as an empty state: an empty state there would claim there are no profiles before
/// anyone has looked.
pub fn view(profiles: Option<&[ProfileIdentity]>, chosen: usize, engine: &Gate, ui: &mut View<'_, Msg>) {
    let Some(profiles) = profiles else {
        ui.add(Skeleton::lines(PLACEHOLDER_LINES)).fill_width();
        return;
    };
    if profiles.is_empty() {
        ui.add(
            EmptyState::new(t!("settings.profiles-empty")).icon("inbox").message(t!("settings.profiles-empty-text")),
        )
        .fill_width();
        return;
    }

    let items = profiles.iter().map(|profile| {
        let state = if profile.stored() { t!("settings.identity-saved") } else { t!("settings.identity-none") };
        ListItem::new(profile.name().as_str().to_owned()).detail(state)
    });
    ui.add(List::new(items).selected(Some(chosen)).on_select(Msg::Profile)).id("profiles").fill_width();

    // The nearer reason wins: without a profile or without a login there is nothing to do even
    // with an engine running, and saying "needs an engine" there would send the person after the
    // wrong thing.
    let gate = match profiles.get(chosen) {
        None => Gate::shut(t!("settings.no-profile")),
        Some(profile) if !profile.stored() => Gate::shut(t!("settings.no-identity", profile = profile.name().as_str())),
        Some(_) => engine.clone(),
    };
    gate.show(ui, |ui, enabled| {
        ui.row(|ui| {
            ui.add(Button::new(t!("settings.refresh")).on_press(Msg::AskRefresh).disabled(!enabled))
                .id("refresh-identity");
            ui.add(Button::new(t!("settings.sign-out")).variant("danger").on_press(Msg::AskSignOut).disabled(!enabled))
                .id("sign-out");
        })
        .gap(GAP);
    });
    // The same reason in writing, because a dead button takes no focus and a tooltip alone would
    // never reach someone working from the keyboard.
    if let Some(reason) = gate.reason() {
        ui.add(Text::new(reason.to_owned()).role("secondary")).fill_width();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use qframe::runtime::Harness;

    use crate::engine::EngineKind;
    use crate::profile::SafeName;
    use crate::ui::settings::engine::{Health, Trouble};
    use crate::ui::settings::testing::{self, Host};
    use crate::ui::settings::{Msg, Request};

    const SIZE: (u16, u16) = (100, 42);

    fn named(name: &str) -> SafeName {
        SafeName::parse(name).expect("the test names are already safe")
    }

    fn with_profiles(health: Health, profiles: &[(&str, bool)]) -> Harness<Host> {
        let mut harness = testing::host(testing::screen(EngineKind::Podman, health), SIZE.0, SIZE.1);
        let read = profiles.iter().map(|(name, signed_in)| testing::profile(name, *signed_in)).collect();
        harness.send(testing::wrap(Msg::Profiles(read))).render();
        harness
    }

    #[test]
    fn profiles_that_have_not_been_read_yet_do_not_claim_to_be_none() {
        // The screen opens before the store has been read; an empty state there would be a lie.
        let harness = testing::host(testing::screen(EngineKind::Podman, Health::Working), SIZE.0, SIZE.1);
        let screen = harness.screen();
        assert!(!screen.contains("No profiles yet"), "{screen}");
    }

    #[test]
    fn a_store_without_profiles_says_where_they_are_made() {
        let harness = with_profiles(Health::Working, &[]);
        let screen = harness.screen();
        assert!(screen.contains("No profiles yet"), "{screen}");
        assert!(screen.contains("profiles screen"), "{screen}");
    }

    #[test]
    fn every_profile_says_whether_a_login_is_stored_for_it() {
        let harness = with_profiles(Health::Working, &[("claude-sub", true), ("codex-key", false)]);
        let screen = harness.screen();
        assert!(screen.contains("claude-sub"), "{screen}");
        assert!(screen.contains("identity: saved"), "{screen}");
        assert!(screen.contains("codex-key"), "{screen}");
        assert!(screen.contains("identity: none"), "{screen}");
    }

    #[test]
    fn signing_out_says_what_it_deletes_and_what_keeps_working() {
        let mut harness = with_profiles(Health::Working, &[("claude-sub", true)]);
        harness.click_text("Sign out").advance(Duration::from_millis(400));
        let screen = harness.screen();
        assert!(screen.contains("Sign claude-sub out?"), "{screen}");
        assert!(screen.contains("is deleted"), "{screen}");
        assert!(screen.contains("with their own copy"), "{screen}");
        assert!(harness.app().asked.is_empty(), "nothing happens before the answer");
    }

    #[test]
    fn the_answer_to_signing_out_is_what_reaches_the_application() {
        let mut harness = with_profiles(Health::Working, &[("claude-sub", true)]);
        harness.click_text("Sign out").advance(Duration::from_millis(400));
        harness.click_text("Delete the login");
        assert_eq!(harness.app().asked, [Request::SignOut(named("claude-sub"))]);
    }

    #[test]
    fn cancelling_the_sign_out_leaves_the_login_alone() {
        let mut harness = with_profiles(Health::Working, &[("claude-sub", true)]);
        harness.click_text("Sign out").advance(Duration::from_millis(400));
        harness.press("esc").advance(Duration::from_millis(400));
        assert!(harness.app().asked.is_empty(), "{}", harness.screen());
    }

    #[test]
    fn refreshing_says_which_copies_are_rewritten_and_what_stays() {
        let mut harness = with_profiles(Health::Working, &[("claude-sub", true)]);
        harness.click_text("Refresh identity").advance(Duration::from_millis(400));
        let screen = harness.screen();
        assert!(screen.contains("Every workspace that uses claude-sub"), "{screen}");
        assert!(screen.contains("memory and settings stay."), "{screen}");
        harness.click_text("Refresh now");
        assert_eq!(harness.app().asked, [Request::RefreshIdentity(named("claude-sub"))]);
    }

    #[test]
    fn a_profile_without_a_login_has_nothing_to_refresh_or_sign_out_of() {
        let harness = with_profiles(Health::Working, &[("codex-key", false)]);
        let screen = harness.screen();
        assert!(screen.contains("codex-key has no stored login."), "{screen}");
    }

    #[test]
    fn without_an_engine_the_actions_that_touch_a_volume_are_dead_and_say_why() {
        let health = Health::Missing(Trouble::NotInstalled);
        let mut harness = with_profiles(health, &[("claude-sub", true)]);
        let screen = harness.screen();
        assert!(screen.contains("Needs a container engine."), "{screen}");
        harness.click_text("Sign out").advance(Duration::from_millis(400));
        assert!(!harness.screen().contains("Sign claude-sub out?"), "a dead action does nothing");
        assert!(harness.app().asked.is_empty());
    }

    #[test]
    fn the_pointer_learns_why_an_action_is_dead_by_resting_on_it() {
        let health = Health::Missing(Trouble::NotInstalled);
        let mut harness = with_profiles(health, &[("claude-sub", true)]);
        let reason = "Needs a container engine.";
        let printed = harness.screen().matches(reason).count();
        let (x, y) = harness.find("Sign out").expect("the action is on screen");
        harness.hover(x, y).advance(Duration::from_secs(1));
        let hovered = harness.screen().matches(reason).count();
        assert!(hovered > printed, "resting on the action says why as well:\n{}", harness.screen());
    }

    #[test]
    fn the_keyboard_moves_the_choice_between_profiles() {
        let mut harness = with_profiles(Health::Working, &[("claude-sub", true), ("codex-key", false)]);
        harness.click_text("claude-sub");
        harness.press("down").render();
        let screen = harness.screen();
        assert!(screen.contains("codex-key has no stored login."), "the second profile is the chosen one:\n{screen}");
    }
}
