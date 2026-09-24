//! What shines while the workspace waits: a tab whose container is starting. It follows reduced
//! motion and stands still under it.

use super::*;

/// The colours of the letters of `text` where the screen shows it now.
fn colours(harness: &Harness<Screen>, text: &str) -> Vec<Option<qframe::color::Rgb>> {
    let (x, y) = harness.find(text).unwrap_or_else(|| panic!("`{text}` is shown:\n{}", harness.screen()));
    let (x, y) = (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"));
    let width = u16::try_from(text.chars().count()).expect("a short text");
    (x..x + width).map(|column| harness.fg(column, y)).collect()
}

/// Whether the letters of `text` change colour while time passes.
fn shines(harness: &mut Harness<Screen>, text: &str) -> bool {
    let mut seen = vec![colours(harness, text)];
    for _ in 0..8 {
        harness.advance(Duration::from_millis(120)).render();
        seen.push(colours(harness, text));
    }
    seen.windows(2).any(|pair| pair[0] != pair[1])
}

/// A workspace with a shell tab chosen on the new tab's page, looked at while its container is
/// starting: the choice is made without letting the background work run, since the engine of
/// these tests answers at once and the starting would be over before a frame was drawn.
fn starting(name: &str, reduced: bool) -> (Scratch, Harness<Screen>) {
    let scratch = Scratch::new(name);
    let mut screen = one_workspace(&scratch);
    open(&mut screen, Choice::Shell);
    assert_eq!(state(&screen, 0, 0), TabState::Starting);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(reduced).render();
    (scratch, harness)
}

#[test]
fn a_starting_container_shines_and_stands_still_under_reduced_motion() {
    let (_scratch, mut harness) = starting("shine-starting", false);
    assert!(harness.screen().contains("Starting the container"), "{}", harness.screen());
    assert!(shines(&mut harness, "Starting the container"), "the words shine while it starts");

    let (_scratch, mut still) = starting("shine-starting-still", true);
    assert!(still.screen().contains("Starting the container"), "{}", still.screen());
    assert!(!shines(&mut still, "Starting the container"), "reduced motion draws the words still");
}

/// A workspace whose backup has just started, looked at with only the panel's workspace widget
/// unfolded: the timer went off and the round it started is still under way.
fn backing_up(name: &str, reduced: bool) -> (Scratch, Harness<Screen>) {
    let scratch = Scratch::new(name);
    let mut screen = one_workspace(&scratch).backing_up(crate::backup::BackupEvery::Fifteen);
    apply(&mut screen, Msg::OpenWorkspace(0));
    let run = screen.last_timer;
    apply(&mut screen, Msg::BackupDue("firefly".to_owned(), run));
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(reduced);
    let order = harness.app().0.panel().order().to_vec();
    for (index, widget) in order.iter().enumerate() {
        harness.send(Msg::ToggleWidget(index, *widget == PanelWidget::Info));
    }
    // The widgets fold and unfold as they move, so the panel is looked at once they have.
    harness.advance(Duration::from_secs(1)).render();
    (scratch, harness)
}

#[test]
fn a_backup_under_way_shines_in_its_row_and_stands_still_under_reduced_motion() {
    let (_scratch, mut harness) = backing_up("shine-backup", false);
    let row = harness.screen().lines().find(|line| line.contains("Last backup")).map(str::to_owned);
    assert!(row.as_deref().is_some_and(|row| row.contains("backing up")), "{}", harness.screen());
    assert!(shines(&mut harness, "backing up"), "{}", harness.screen());

    let (_scratch, mut still) = backing_up("shine-backup-still", true);
    assert!(!shines(&mut still, "backing up"), "{}", still.screen());
}
