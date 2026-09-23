//! A tab of a profile whose sign-in its harness's makers closed still opens, since the sign-in
//! goes on working for some, and the person is told plainly, as they open it, why it may not.

use super::*;

/// A workspace carrying `profile`, with motion off so a toast is on screen as soon as it is sent.
fn carrying(scratch: &Scratch, profile: Profile) -> Harness<Screen> {
    let workspaces = vec![workspace("firefly", "Firefly", scratch.paths(), vec![profile])];
    let screen = WorkspaceScreen::new(Some(engine()), HostUser::Ids { uid: 1000, gid: 1000 }, workspaces);
    let mut harness = harness(screen, 120, 40);
    harness.set_reduced_motion(true).render();
    harness
}

#[test]
fn opening_a_gemini_cli_tab_made_with_the_closed_sign_in_says_why_it_may_not_sign_in() {
    let scratch = Scratch::new("closed");
    let mut harness = carrying(&scratch, profile("gemini-sub", HarnessKind::GeminiCli));
    harness.click_text("New tab").render();
    harness.click_text("New chat").render();
    // The toast wraps its words, so they are read as one line.
    let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(screen.contains("Google closed the sign-in of gemini-sub"), "{screen}");
    assert!(screen.contains("no longer supported for Gemini Code Assist for individuals"), "{screen}");
    assert!(screen.contains("Antigravity IDE"), "{screen}");
    let tabs = harness.app().0.workspace().expect("a workspace").tabs().len();
    assert_eq!(tabs, 1, "the tab opens all the same: {screen}");
}

#[test]
fn a_gemini_cli_tab_with_a_key_opens_without_a_word_about_it() {
    let scratch = Scratch::new("keyed");
    let keyed = Profile { account: AccountKind::ApiKey, ..profile("gemini-key", HarnessKind::GeminiCli) };
    let mut harness = carrying(&scratch, keyed);
    harness.click_text("New tab").render();
    harness.click_text("New chat").render();
    assert!(!harness.screen().contains("Google closed"), "{}", harness.screen());
}
