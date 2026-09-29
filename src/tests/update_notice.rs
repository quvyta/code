//! The notice that a newer version is out: said once for the ecosystem, never over the setup
//! wizard, and turned off and on again from the settings.

use super::*;

/// The family's folders of a test named `what`, empty, so the update notice starts on as it
/// does on a machine that never chose.
fn family(what: &str) -> crate::UpdateFolders {
    let root = scratch(&format!("family-{what}"));
    let _ = std::fs::remove_dir_all(&root);
    crate::UpdateFolders { config: root.join("config"), state: root.join("state") }
}

/// QCode on its home screen over the family's `folders`, as a person starts it.
fn started(folders: &crate::UpdateFolders) -> qframe::runtime::Harness<crate::QCode> {
    let store = scratch("updates-store");
    crate::testing::harness(
        app(config(&store, &[]), &settled(), None).with_update_notice(Some(folders.clone())),
        SIZE.0,
        SIZE.1,
    )
}

/// Moves the switch of the family's update notice in the settings, where it is drawn: a switch
/// is colour alone and stands at the column's right edge, which the language drop-down's arrow
/// marks.
fn click_update_notice(harness: &mut qframe::runtime::Harness<crate::QCode>) {
    harness.click_text("Settings").advance(MOMENT);
    assert_eq!(harness.app().page(), Page::Settings, "{}", harness.screen());
    harness.resize(SIZE.0, 70).render();
    let (_, row) = harness.find("Say when an update is out").expect("the switch is on screen");
    let (edge, _) = harness.find("▾").expect("the language drop-down");
    harness.click(edge - 1, row).advance(MOMENT);
}

#[test]
fn a_newer_version_is_said_in_the_familys_notice_and_the_same_one_is_not() {
    let folders = family("newer");
    let mut harness = started(&folders);
    let asked = harness.update_checks().to_vec();
    assert_eq!(asked.len(), 1, "QCode asks once at start");
    assert_eq!((asked[0].package(), asked[0].current()), ("quvyta-code", env!("CARGO_PKG_VERSION")));
    assert!(!harness.screen().contains("is out"), "nothing is said before the answer");

    harness.set_latest_version(Some("9.9.9")).advance(MOMENT);
    let screen = harness.screen();
    assert!(screen.contains("quvyta-code 9.9.9 is out"), "the notice names the new version:\n{screen}");
    assert!(screen.contains(env!("CARGO_PKG_VERSION")), "and the one running:\n{screen}");

    for latest in [env!("CARGO_PKG_VERSION"), "0.0.1"] {
        let mut same = started(&family("same"));
        same.set_latest_version(Some(latest)).advance(MOMENT);
        assert!(!same.screen().contains("is out"), "{latest} is not newer:\n{}", same.screen());
    }
    let _ = std::fs::remove_dir_all(folders.config.parent().expect("the family's root"));
}

#[test]
fn the_setup_wizard_is_not_interrupted_by_the_question() {
    let folders = family("wizard");
    let store = scratch("updates-wizard-store");
    let fresh = app(config(&store, &[]), &settled(), Some(SetupStep::Language)).with_update_notice(Some(folders));
    let harness = crate::testing::harness(fresh, SIZE.0, SIZE.1);
    assert_eq!(harness.app().page(), Page::Setup);
    assert!(harness.update_checks().is_empty(), "nothing is asked while the wizard is open");
}

#[test]
fn the_switch_in_the_settings_turns_the_question_off_for_the_family_and_back_on() {
    use qframe::storage::Family;

    let folders = family("switch");
    let mut harness = started(&folders);
    assert_eq!(harness.update_checks().len(), 1, "on until someone turns it off");
    click_update_notice(&mut harness);
    assert!(!Family::QUVYTA.update_notice_in(&folders.config), "the family's file says off:\n{}", harness.screen());
    let shared = std::fs::read_to_string(folders.config.join("quvyta.conf")).expect("the family's file");
    assert!(shared.contains("update-notice = false"), "{shared}");

    let mut off = started(&folders);
    assert!(off.update_checks().is_empty(), "a QCode started with it off asks nothing at all");
    off.set_latest_version(Some("9.9.9")).advance(MOMENT);
    assert!(!off.screen().contains("is out"), "{}", off.screen());

    click_update_notice(&mut off);
    assert!(Family::QUVYTA.update_notice_in(&folders.config), "turned back on:\n{}", off.screen());
    let on = started(&folders);
    assert_eq!(on.update_checks().len(), 1, "and the next start asks again");
    let _ = std::fs::remove_dir_all(folders.config.parent().expect("the family's root"));
}
