//! Backing the open projects up: when the screen asks for a backup, what it says about the last
//! one, and what it leaves to be backed up once QCode has closed.
//!
//! The engine of these tests cannot be run, so every backup that is really asked for fails at
//! once with the machine's own words; that failure is how a test sees that a backup was asked
//! for, and when.

use super::*;

use qframe::date::{DateTime, TimeOfDay};

use crate::ui::project::FileMsg;

use crate::backup::conversations::Brought;
use crate::backup::{BackupEvery, Entry, Reason, Snapshot, SnapshotId};
use crate::ui::project::{BackupOf, BackupTrouble, Part, backups, leaving};

/// What a failed backup of `Firefly` says.
const FAILED: &str = "Firefly could not be backed up";

/// Fifteen minutes, the interval until the person chooses another.
const FIFTEEN: Duration = Duration::from_secs(15 * 60);

/// A harness of `one_project` with motion off, so a toast is on screen as soon as it is sent.
fn backing_up(scratch: &Scratch, every: BackupEvery) -> Harness<Screen> {
    let mut harness = harness(one_project(scratch).backing_up(every), SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    harness
}

/// What a backup of `Firefly` answers when it came to `made`.
fn backed(made: Result<Snapshot, BackupTrouble>) -> Msg {
    Msg::BackedUp { project: "firefly".to_owned(), name: "Firefly".to_owned(), made, missed: Vec::new() }
}

/// Lets `time` pass, and the work it set off run.
fn wait(harness: &mut Harness<Screen>, time: Duration) {
    harness.advance(time).render();
}

/// A made snapshot, as a backup that worked answers.
fn made(at: i64) -> Snapshot {
    Snapshot::Made { id: SnapshotId::parse(&"a".repeat(40)).expect("a whole commit id"), at }
}

#[test]
fn an_open_project_is_backed_up_one_interval_after_it_opens_and_not_at_once() {
    let scratch = Scratch::new("backup-timer");
    let mut harness = backing_up(&scratch, BackupEvery::Fifteen);
    assert!(!harness.screen().contains(FAILED), "nothing is backed up as the project opens");
    wait(&mut harness, FIFTEEN - Duration::from_secs(1));
    assert!(!harness.screen().contains(FAILED), "nor before the interval is up");
    wait(&mut harness, Duration::from_secs(1));
    assert!(harness.screen().contains(FAILED), "the timer asked for a backup:\n{}", harness.screen());
}

#[test]
fn a_failure_is_said_once_and_the_timer_keeps_going() {
    let scratch = Scratch::new("backup-once");
    let mut harness = backing_up(&scratch, BackupEvery::Five);
    wait(&mut harness, Duration::from_secs(5 * 60));
    assert!(harness.screen().contains(FAILED), "the first failure is said:\n{}", harness.screen());
    // Long enough for the toast to go, and for two more rounds to fail.
    wait(&mut harness, Duration::from_secs(5 * 60));
    wait(&mut harness, Duration::from_secs(5 * 60));
    assert!(!harness.screen().contains(FAILED), "the next ones are not:\n{}", harness.screen());
    assert!(scratch.paths().backup().is_dir(), "each round did reach the backup, which made its folder");
}

#[test]
fn a_backup_that_worked_lets_the_next_failure_be_said_again() {
    let scratch = Scratch::new("backup-again");
    let mut harness = backing_up(&scratch, BackupEvery::Fifteen);
    let trouble = || backed(Err(BackupTrouble::Said("gone".to_owned())));
    harness.send(trouble());
    assert!(harness.screen().contains(FAILED), "the first failure is said:\n{}", harness.screen());
    wait(&mut harness, Duration::from_secs(60));
    harness.send(trouble());
    assert!(!harness.screen().contains(FAILED), "the second is not:\n{}", harness.screen());
    harness.send(backed(Ok(made(1_000))));
    harness.send(trouble());
    assert!(harness.screen().contains(FAILED), "after one that worked it is:\n{}", harness.screen());
}

#[test]
fn a_changed_interval_replaces_the_timers_of_the_open_projects() {
    let scratch = Scratch::new("backup-change");
    let mut harness = backing_up(&scratch, BackupEvery::Hour);
    wait(&mut harness, Duration::from_secs(60));
    harness.send(Msg::BackupEvery(BackupEvery::Five));
    assert_eq!(harness.app().0.backup_every(), BackupEvery::Five);
    wait(&mut harness, Duration::from_secs(5 * 60));
    assert!(harness.screen().contains(FAILED), "the new timer went off at five minutes:\n{}", harness.screen());
}

#[test]
fn off_sets_no_timer_and_cancels_the_ones_set() {
    let scratch = Scratch::new("backup-off");
    let mut harness = backing_up(&scratch, BackupEvery::Five);
    harness.send(Msg::BackupEvery(BackupEvery::Off));
    wait(&mut harness, Duration::from_secs(2 * 60 * 60));
    assert!(!harness.screen().contains(FAILED), "no backup was asked for:\n{}", harness.screen());
    assert!(!scratch.paths().backup().exists(), "and nothing was made for one");
}

#[test]
fn without_an_engine_nothing_is_backed_up_and_nothing_is_said() {
    let scratch = Scratch::new("backup-no-engine");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    let screen = ProjectScreen::new(None, HostUser::ImageDefault, projects).backing_up(BackupEvery::Five);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    wait(&mut harness, Duration::from_secs(60 * 60));
    assert!(!harness.screen().contains(FAILED), "{}", harness.screen());
    assert!(!scratch.paths().backup().exists());
}

#[test]
fn a_project_closed_from_the_rail_is_backed_up_as_it_goes() {
    let scratch = Scratch::new("backup-close");
    let mut harness = backing_up(&scratch, BackupEvery::Fifteen);
    harness.send(Msg::CloseProject(0)).advance(Duration::from_millis(10));
    assert!(harness.app().0.project().is_none(), "the project left the rail");
    assert!(harness.screen().contains(FAILED), "and its backup was asked for:\n{}", harness.screen());

    let scratch = Scratch::new("backup-close-off");
    let mut harness = backing_up(&scratch, BackupEvery::Off);
    harness.send(Msg::CloseProject(0)).advance(Duration::from_millis(10));
    assert!(!harness.screen().contains(FAILED), "backups are off, so none is made:\n{}", harness.screen());
}

/// The row of the panel's project widget that reads one of `labels`, read with only that widget
/// unfolded; the panel is left as it was.
fn info_row(harness: &mut Harness<Screen>, labels: &[&str]) -> String {
    let order = harness.app().0.panel().order().to_vec();
    let unfolded: Vec<bool> = (0..order.len()).map(|index| harness.app().0.panel().is_unfolded(index)).collect();
    for (index, widget) in order.iter().enumerate() {
        harness.send(Msg::ToggleWidget(index, *widget == PanelWidget::Info));
    }
    let screen = harness.screen();
    let row = screen
        .lines()
        .find(|line| labels.iter().any(|label| line.contains(label)))
        .unwrap_or_else(|| panic!("the row is there:\n{screen}"))
        .to_owned();
    for (index, open) in unfolded.into_iter().enumerate() {
        harness.send(Msg::ToggleWidget(index, open));
    }
    row
}

/// The row of the project widget that names the last backup.
fn backup_row(harness: &mut Harness<Screen>) -> String {
    info_row(harness, &["Last backup", "Son yedek"])
}

/// The row of the project widget that names what the backup leaves out.
fn left_out_row(harness: &mut Harness<Screen>) -> String {
    info_row(harness, &["Left out", "Yedek dışı"])
}

/// Makes `key` the one selected entry, so the row a menu was on is drawn as any other again: a
/// selected row is drawn brighter.
fn select(harness: &mut Harness<Screen>, key: &str) {
    harness.send(Msg::SelectFile(key.to_owned()));
    harness.send(Msg::Files(FileMsg::Choose(vec![key.to_owned()])));
}

/// The colour the icon of the row showing `name` is drawn in.
fn icon_colour(harness: &Harness<Screen>, name: &str) -> Option<qframe::color::Rgb> {
    let (x, y) = harness.find(name).unwrap_or_else(|| panic!("`{name}` is on screen:\n{}", harness.screen()));
    harness.fg(u16::try_from(x - 2).unwrap_or(0), u16::try_from(y).unwrap_or(0))
}

#[test]
fn the_panel_says_when_the_last_backup_was_made() {
    let scratch = Scratch::new("backup-row");
    let mut harness = backing_up(&scratch, BackupEvery::Fifteen);
    assert!(backup_row(&mut harness).contains("not yet"), "a project never backed up");

    let now = DateTime::now_local();
    let at = DateTime { time: TimeOfDay::new(9, 41, 0), ..now }.to_unix();
    harness.send(Msg::NewestBackup("firefly".to_owned(), Some(at)));
    assert!(backup_row(&mut harness).contains("09:41"), "today's is its time of day");

    // A newer one made meanwhile is not taken back by an older answer.
    harness.send(backed(Ok(made(at + 60))));
    harness.send(Msg::NewestBackup("firefly".to_owned(), Some(at)));
    assert!(backup_row(&mut harness).contains("09:42"), "{}", harness.screen());

    harness.send(Msg::BackupEvery(BackupEvery::Off));
    assert!(backup_row(&mut harness).contains("off"));
    harness.set_locale("tr").render();
    assert!(backup_row(&mut harness).contains("kapalı"));
    harness.send(Msg::BackupEvery(BackupEvery::Five));
    assert!(backup_row(&mut harness).contains("09:42"));
}

#[test]
fn without_an_engine_the_panel_says_why_there_is_no_backup() {
    let scratch = Scratch::new("backup-row-engine");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    let mut harness = harness(ProjectScreen::new(None, HostUser::ImageDefault, projects), SIZE.0, SIZE.1);
    harness.set_reduced_motion(true);
    assert!(backup_row(&mut harness).contains("no engine"));
}

#[test]
fn an_older_backup_names_its_day_and_todays_only_its_time() {
    let now =
        DateTime { date: Date::new(2026, 9, 19).expect("a day"), time: TimeOfDay::new(15, 0, 0), offset_minutes: 180 };
    let today = DateTime { time: TimeOfDay::new(14, 32, 0), ..now }.to_unix();
    assert_eq!(backups::stamp(today, now), "14:32");
    let before = DateTime { date: Date::new(2026, 9, 18).expect("a day"), ..now }.to_unix();
    assert_eq!(backups::stamp(before, now), "2026-09-18 15:00");
    // 23:30 UTC on the 18th is already the 19th three hours east of it.
    let late =
        DateTime { date: Date::new(2026, 9, 18).expect("a day"), time: TimeOfDay::new(23, 30, 0), offset_minutes: 0 };
    assert_eq!(backups::stamp(late.to_unix(), now), "02:30");
}

#[test]
fn what_is_left_to_back_up_as_qcode_closes_is_every_project_of_the_rail() {
    let (screen, _scratches) = several(&[("firefly", "Firefly"), ("moth", "Moth")]);
    let left = leaving(&screen).expect("backups are on and there is an engine");
    assert_eq!(left.names(), ["Firefly", "Moth"]);
    let lines = crate::ui::settings::testing::translated("en", || left.back_up());
    assert_eq!(lines.len(), 2, "the engine is not there, so each one failed and says so: {lines:?}");
    assert!(lines[0].starts_with("Firefly could not be backed up: "), "{lines:?}");

    assert_eq!(leaving(&screen.backing_up(BackupEvery::Off)), None, "off backs nothing up");
    let scratch = Scratch::new("backup-leaving-engine");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    assert_eq!(leaving(&ProjectScreen::new(None, HostUser::ImageDefault, projects)), None, "nor does no engine");
    assert_eq!(leaving(&ProjectScreen::new(Some(engine()), HostUser::ImageDefault, Vec::new())), None);
}

/// A moment for a menu to settle.
const MOMENT: Duration = Duration::from_millis(400);

/// `one_project` with its `project.qcode` on disk, the way the workspace leaves a project.
fn with_file(name: &str) -> (Scratch, Harness<Screen>) {
    let scratch = Scratch::new(name);
    fs::write(scratch.paths().file, file("firefly", "Firefly", &["claude-sub"]).to_toml()).expect("a project file");
    let harness = backing_up(&scratch, BackupEvery::Fifteen);
    (scratch, harness)
}

/// Right-clicks the row showing `text`.
fn right_click(harness: &mut Harness<Screen>, text: &str) {
    use qframe::event::{MouseButton, MouseKind};
    let (x, y) = harness.find(text).unwrap_or_else(|| panic!("`{text}` is on screen:\n{}", harness.screen()));
    harness.mouse(MouseKind::Down(MouseButton::Right), x, y);
    harness.mouse(MouseKind::Up(MouseButton::Right), x, y);
    harness.advance(MOMENT);
}

/// What the open project's backup leaves out.
fn skipped(harness: &Harness<Screen>) -> Vec<String> {
    harness.app().0.project().expect("a project is open").backup_skip().to_vec()
}

/// What the project's file on disk leaves out.
fn on_disk(scratch: &Scratch) -> Vec<String> {
    let text = fs::read_to_string(scratch.paths().file).expect("the project file");
    ProjectFile::parse("project.qcode", &text).value.expect("a project").backup_skip
}

#[test]
fn a_folder_is_left_out_from_its_menu_and_taken_in_again_the_same_way() {
    let (scratch, mut harness) = with_file("skip-folder");
    assert!(left_out_row(&mut harness).contains("nothing"));
    let plain = icon_colour(&harness, "src");
    right_click(&mut harness, "src");
    harness.click_text("Don't back up").advance(MOMENT);
    assert_eq!(skipped(&harness), ["src"]);
    assert_eq!(on_disk(&scratch), ["src"], "the project file keeps it");
    select(&mut harness, "README.md");
    assert_ne!(icon_colour(&harness, "src"), plain, "the row looks left out");
    assert!(left_out_row(&mut harness).contains("src"), "and the project widget names it");

    right_click(&mut harness, "src");
    let text = harness.screen();
    assert!(!text.contains("Don't back up"), "{text}");
    harness.click_text("Back up again").advance(MOMENT);
    assert!(skipped(&harness).is_empty());
    assert!(on_disk(&scratch).is_empty());
    select(&mut harness, "README.md");
    assert_eq!(icon_colour(&harness, "src"), plain);
}

#[test]
fn a_left_out_entry_is_told_apart_from_a_cut_one() {
    let (_scratch, mut harness) = with_file("skip-cut");
    harness.send(Msg::Files(FileMsg::Cut("README.md".to_owned())));
    let cut = icon_colour(&harness, "README.md");
    harness.send(Msg::Files(FileMsg::DropCut));
    right_click(&mut harness, "README.md");
    harness.click_text("Don't back up").advance(MOMENT);
    select(&mut harness, "src");
    assert_ne!(icon_colour(&harness, "README.md"), cut);
}

#[test]
fn a_left_out_folder_takes_what_is_in_it_along_and_offers_nothing_for_it() {
    let (_scratch, mut harness) = with_file("skip-inside");
    harness.send(Msg::ExpandFile("src".to_owned(), true));
    let plain = icon_colour(&harness, "main.rs");
    right_click(&mut harness, "src");
    harness.click_text("Don't back up").advance(MOMENT);
    select(&mut harness, "README.md");
    assert_ne!(icon_colour(&harness, "main.rs"), plain, "the file inside looks left out with its folder");

    right_click(&mut harness, "main.rs");
    let text = harness.screen();
    assert!(!text.contains("Don't back up") && !text.contains("Back up again"), "it goes with its folder:\n{text}");
    assert!(text.contains("Rename"), "the rest of its menu is there:\n{text}");
}

#[test]
fn the_menu_of_a_selection_leaves_all_of_it_out() {
    let (scratch, mut harness) = with_file("skip-many");
    harness.send(Msg::SelectFile("src".to_owned()));
    harness.send(Msg::Files(FileMsg::Choose(vec!["src".to_owned(), "README.md".to_owned()])));
    right_click(&mut harness, "README.md");
    harness.click_text("Don't back up").advance(MOMENT);
    assert_eq!(on_disk(&scratch), ["src", "README.md"]);
    right_click(&mut harness, "README.md");
    harness.click_text("Back up again").advance(MOMENT);
    assert!(on_disk(&scratch).is_empty());
}

#[test]
fn a_project_file_that_cannot_be_written_says_so_and_nothing_changes() {
    let scratch = Scratch::new("skip-no-file");
    let mut harness = backing_up(&scratch, BackupEvery::Fifteen);
    right_click(&mut harness, "src");
    harness.click_text("Don't back up").advance(MOMENT);
    assert!(harness.screen().contains("could not be written"), "{}", harness.screen());
    assert!(skipped(&harness).is_empty());
}

#[test]
fn what_the_file_leaves_out_is_what_the_project_opens_with_and_it_speaks_turkish() {
    let scratch = Scratch::new("skip-open");
    let mut carried = file("firefly", "Firefly", &[]);
    carried.backup_skip = vec!["README.md".to_owned()];
    let projects = vec![OpenProject::new(&carried, scratch.paths(), Vec::new())];
    let screen = ProjectScreen::new(Some(engine()), HostUser::ImageDefault, projects);
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).set_locale("tr").render();
    assert_eq!(skipped(&harness), ["README.md"]);
    assert!(left_out_row(&mut harness).contains("README.md"), "{}", harness.screen());
    right_click(&mut harness, "README.md");
    assert!(harness.screen().contains("Yedeğe al"), "{}", harness.screen());
    harness.press("esc").advance(MOMENT);
    right_click(&mut harness, "src");
    assert!(harness.screen().contains("Yedeğe alma"), "{}", harness.screen());
}

/// A backup of the list, made at `at` for `reason`, that changed `changed` files.
fn entry(at: i64, reason: Reason, changed: usize) -> Entry {
    let id = SnapshotId::parse(&format!("{at:040x}")).expect("a whole commit id");
    Entry { id, at, reason, changed }
}

/// Today at `hour`:`minute`, where the machine stands.
fn today_at(hour: u8, minute: u8) -> i64 {
    DateTime { time: TimeOfDay::new(hour, minute, 0), ..DateTime::now_local() }.to_unix()
}

/// The number of the reading of the list of backups started last.
fn reading(harness: &Harness<Screen>) -> u64 {
    harness.app().0.last_listing
}

/// Presses the confirming button of the question on screen, the one beside its Cancel.
fn confirm(harness: &mut Harness<Screen>) {
    press_beside_cancel(harness, "Bring back");
}

/// Presses the button reading `label` to the right of the Cancel of the question on screen.
fn press_beside_cancel(harness: &mut Harness<Screen>, label: &str) {
    let screen = harness.screen();
    let (y, line) = screen.lines().enumerate().find(|(_, line)| line.contains("Cancel")).expect("a question");
    let at = line.find("Cancel").and_then(|cancel| line[cancel..].find(label).map(|x| cancel + x));
    let x = line[..at.expect("its button")].chars().count();
    harness.click(i32::try_from(x).unwrap_or(0), i32::try_from(y).unwrap_or(0)).advance(MOMENT);
}

/// Two backups, the newer one taken just before a restore.
fn two() -> Vec<Entry> {
    vec![entry(today_at(14, 32), Reason::BeforeRestore, 1), entry(today_at(9, 5), Reason::Scheduled, 3)]
}

#[test]
fn the_project_widget_opens_the_list_of_backups_and_a_project_never_backed_up_has_none() {
    let (_scratch, mut harness) = with_file("list-empty");
    let order = harness.app().0.panel().order().to_vec();
    for (index, widget) in order.iter().enumerate() {
        harness.send(Msg::ToggleWidget(index, *widget == PanelWidget::Info));
    }
    harness.click_text("Backups").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("No backup yet"), "{text}");
    harness.press("esc").advance(MOMENT);
    assert!(!harness.screen().contains("No backup yet"), "Esc closes it:\n{}", harness.screen());
}

#[test]
fn each_backup_says_when_it_was_made_and_how_many_files_it_changed() {
    let (_scratch, mut harness) = with_file("list-rows");
    harness.send(Msg::ShowBackups(None));
    harness.send(Msg::BackupsRead(reading(&harness), Ok(two())));
    assert!(harness.is_focused("project-backups"), "the list has the keyboard");
    let text = harness.screen();
    for words in ["Backups", "14:32", "before restore, 1 file changed", "09:05", "3 files changed", "Bring back"] {
        assert!(text.contains(words), "`{words}` is missing:\n{text}");
    }
    assert!(!text.contains(|cell| "[]┌┐└┘─│".contains(cell)), "no brackets or boxes:\n{text}");
    harness.set_locale("tr").render();
    let text = harness.screen();
    for words in ["Yedekler", "geri getirmeden önce, 1 dosya değişti", "3 dosya değişti", "Geri getir"] {
        assert!(text.contains(words), "`{words}` is missing:\n{text}");
    }
}

#[test]
fn an_answer_for_a_list_closed_meanwhile_is_dropped() {
    let (_scratch, mut harness) = with_file("list-stale");
    harness.send(Msg::ShowBackups(None));
    let first = reading(&harness);
    harness.send(Msg::CloseBackups);
    harness.send(Msg::BackupsRead(first, Ok(two())));
    assert!(!harness.screen().contains("14:32"), "{}", harness.screen());
}

#[test]
fn choosing_a_backup_asks_first_and_says_nothing_is_deleted() {
    let (_scratch, mut harness) = with_file("list-confirm");
    harness.send(Msg::ShowBackups(None));
    harness.send(Msg::BackupsRead(reading(&harness), Ok(two())));
    harness.press("down").press("enter").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Bring back the files of 09:05?"), "{text}");
    // The dialog wraps its message, so the words are looked for on their own.
    assert!(text.contains("Nothing is") && text.contains("deleted"), "{text}");
    assert!(text.contains("backed up") && text.contains("first"), "{text}");

    // The engine is not there, so the restore fails and the list stays to try again.
    confirm(&mut harness);
    let text = harness.screen();
    assert!(text.contains("Nothing was brought back"), "{text}");
    assert!(text.contains("09:05"), "the list is still open:\n{text}");
}

#[test]
fn a_backup_brought_back_closes_the_list_and_says_so() {
    let (_scratch, mut harness) = with_file("list-done");
    harness.send(Msg::ShowBackups(None));
    harness.send(Msg::BackupsRead(reading(&harness), Ok(two())));
    let before = made(today_at(15, 0));
    let done = Ok(Brought::Done(crate::backup::Restored::Done { before }));
    let back = |done| Msg::Restored {
        project: "firefly".to_owned(),
        name: "Firefly".to_owned(),
        of: BackupOf::Project,
        file: None,
        done,
    };
    harness.send(back(done));
    let text = harness.screen();
    assert!(text.contains("Firefly is back as it was"), "{text}");
    assert!(!text.contains("09:05"), "the list closed:\n{text}");
    assert!(backup_row(&mut harness).contains("15:00"), "the backup taken first is the newest one now");

    harness.send(Msg::ShowBackups(None));
    harness.send(back(Ok(Brought::Done(crate::backup::Restored::Busy))));
    let text = harness.screen();
    assert!(text.contains("Another QCode is backing this project up"), "{text}");
    assert!(text.contains("Backups"), "the list stays open:\n{text}");
}

#[test]
fn a_files_earlier_versions_are_the_same_list_for_that_file() {
    let (_scratch, mut harness) = with_file("list-file");
    right_click(&mut harness, "README.md");
    harness.click_text("Earlier versions").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Earlier versions of README.md"), "{text}");
    assert!(text.contains("No earlier version yet"), "{text}");
    harness.send(Msg::BackupsRead(reading(&harness), Ok(two())));
    let text = harness.screen();
    assert!(text.contains("before restore") && !text.contains("files changed"), "{text}");
    harness.press("down").press("enter").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Bring back README.md of 09:05?"), "{text}");
    assert!(text.contains("README.md comes back as it was"), "{text}");
}

#[test]
fn a_slow_list_shows_its_indicator_and_keeps_it_long_enough_to_be_read() {
    let scratch = Scratch::new("list-slow");
    let mut screen = one_project(&scratch);
    apply(&mut screen, Msg::ShowBackups(None));
    let number = screen.last_listing;
    let mut harness = harness(screen, SIZE.0, SIZE.1);
    harness.set_reduced_motion(true).render();
    assert!(!harness.screen().contains("Reading the backups"), "not in its first moment");
    harness.send(Msg::BackupsSlow(number));
    assert!(harness.screen().contains("Reading the backups"), "{}", harness.screen());
    harness.send(Msg::BackupsRead(number, Ok(two())));
    assert!(!harness.screen().contains("09:05"), "the indicator stays a moment longer");
    harness.send(Msg::BackupsSettled(number));
    assert!(harness.screen().contains("09:05"), "{}", harness.screen());
}

#[test]
fn a_list_that_could_not_be_read_says_why() {
    let (_scratch, mut harness) = with_file("list-unread");
    harness.send(Msg::ShowBackups(None));
    harness.send(Msg::BackupsRead(reading(&harness), Err(BackupTrouble::Said("git refused".to_owned()))));
    let text = harness.screen();
    assert!(text.contains("The backups could not be read.") && text.contains("git refused"), "{text}");
}

#[test]
fn without_an_engine_the_backups_cannot_be_opened() {
    let scratch = Scratch::new("list-no-engine");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    let mut harness = harness(ProjectScreen::new(None, HostUser::ImageDefault, projects), SIZE.0, SIZE.1);
    harness.set_reduced_motion(true);
    harness.send(Msg::ShowBackups(None));
    assert!(harness.app().0.listing.is_none());
    right_click(&mut harness, "README.md");
    harness.click_text("Earlier versions").advance(MOMENT);
    assert!(harness.app().0.listing.is_none(), "the item is there but cannot be chosen");
}

/// The profiles whose conversations the next round of the open project takes.
fn used(screen: &ProjectScreen) -> Vec<String> {
    backups::used(screen.project().expect("a project is open")).into_iter().collect()
}

/// A screen of `Firefly` carrying a Claude Code and an opencode profile, reaching `engine`.
fn two_profiles(scratch: &Scratch, engine: Engine) -> ProjectScreen {
    let profiles = vec![profile("claude-sub", HarnessKind::ClaudeCode), profile("oc", HarnessKind::OpenCode)];
    let projects = vec![project("firefly", "Firefly", scratch.paths(), profiles)];
    ProjectScreen::new(Some(engine), HostUser::Ids { uid: 1000, gid: 1000 }, projects)
}

#[test]
fn a_round_takes_the_conversations_of_every_profile_whose_tab_was_open_since_the_last_one() {
    let scratch = Scratch::new("round-profiles");
    let mut screen = two_profiles(&scratch, engine());
    apply(&mut screen, Msg::OpenProject(0));
    assert!(used(&screen).is_empty(), "no harness ran yet");
    open(&mut screen, Choice::NewChat("oc".to_owned()));
    open(&mut screen, claude());
    assert_eq!(used(&screen), ["claude-sub", "oc"]);

    // Closed before the round, a tab still counts for it: its harness wrote in the meantime.
    apply(&mut screen, Msg::CloseTab(0));
    assert_eq!(used(&screen), ["claude-sub", "oc"]);
    apply(&mut screen, Msg::CloseProject(0));
    assert!(screen.project().is_none());

    // A round takes them off the list; the next one starts from the tabs still open.
    let mut screen = two_profiles(&scratch, engine());
    apply(&mut screen, Msg::OpenProject(0));
    open(&mut screen, Choice::NewChat("oc".to_owned()));
    open(&mut screen, claude());
    apply(&mut screen, Msg::CloseTab(0));
    apply(&mut screen, Msg::BackupDue("firefly".to_owned(), 1));
    assert_eq!(used(&screen), ["claude-sub"], "only the tab still open counts for the next round");
}

#[test]
fn a_tab_brought_back_and_never_shown_has_run_no_harness() {
    let scratch = Scratch::new("round-waiting");
    let mut screen = two_profiles(&scratch, engine());
    let tab = |kind| SessionTab { kind, conversation: None, opened: 0 };
    let record = SessionProject {
        id: ProjectId::parse("firefly").expect("an id"),
        active_tab: 0,
        tabs: vec![tab(SessionTabKind::Shell), tab(SessionTabKind::Profile("oc".to_owned()))],
    };
    screen.restore_tabs(0, &record);
    apply(&mut screen, Msg::OpenProject(0));
    assert!(used(&screen).is_empty(), "{:?}", used(&screen));
    apply(&mut screen, Msg::OpenTab(1));
    assert_eq!(used(&screen), ["oc"], "shown, it started");
}

#[test]
fn conversations_that_could_not_be_backed_up_are_said_once_like_the_project() {
    let scratch = Scratch::new("round-chat-once");
    let mut harness = backing_up(&scratch, BackupEvery::Fifteen);
    let words = "Conversations of claude-sub not backed up";
    let missed = || Msg::BackedUp {
        project: "firefly".to_owned(),
        name: "Firefly".to_owned(),
        made: Ok(Snapshot::Unchanged),
        missed: vec![(Part::Conversations("claude-sub".to_owned()), BackupTrouble::Said("gone".to_owned()))],
    };
    harness.send(missed());
    assert!(harness.screen().contains(words), "{}", harness.screen());
    wait(&mut harness, Duration::from_secs(60));
    harness.send(missed());
    assert!(!harness.screen().contains(words), "said once:\n{}", harness.screen());
    harness.send(backed(Err(BackupTrouble::Said("gone".to_owned()))));
    assert!(!harness.screen().contains(FAILED), "nor is the project's own failure after it:\n{}", harness.screen());
    harness.send(backed(Ok(Snapshot::Unchanged)));
    harness.send(missed());
    assert!(harness.screen().contains(words), "after a round that worked it is:\n{}", harness.screen());
}

/// An engine of this test's own that says every container runs and every command worked.
#[cfg(unix)]
fn always_running(scratch: &Scratch) -> Engine {
    use std::os::unix::fs::PermissionsExt;
    let bin = scratch.0.join("engine");
    fs::write(&bin, "#!/bin/sh\necho running\n").expect("the stand-in");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("runnable");
    Engine::new(EngineKind::Podman, bin)
}

#[test]
fn quitting_backs_up_the_conversations_used_and_a_database_only_once_its_container_stopped() {
    let scratch = Scratch::new("round-leaving");
    let mut screen = two_profiles(&scratch, engine());
    apply(&mut screen, Msg::OpenProject(0));
    open(&mut screen, Choice::NewChat("oc".to_owned()));
    open(&mut screen, claude());
    let left = leaving(&screen).expect("backups are on");
    // The engine is not there, so every part fails and says so.
    let lines = crate::ui::settings::testing::translated("en", || left.back_up());
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("Firefly could not be backed up: "), "{lines:?}");
    assert!(lines[1].starts_with("Conversations of claude-sub not backed up in Firefly: "), "{lines:?}");
    let lines = crate::ui::settings::testing::translated("en", || left.back_up_stopped());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("Conversations of oc not backed up in Firefly"), "{lines:?}");
}

#[cfg(unix)]
#[test]
fn a_database_whose_container_still_runs_is_passed_by_without_a_word() {
    let scratch = Scratch::new("round-running");
    let mut screen = two_profiles(&scratch, always_running(&scratch));
    apply(&mut screen, Msg::OpenProject(0));
    open(&mut screen, Choice::NewChat("oc".to_owned()));
    let left = leaving(&screen).expect("backups are on");
    let lines = crate::ui::settings::testing::translated("en", || left.back_up_stopped());
    assert!(lines.is_empty(), "{lines:?}");
    assert!(!scratch.paths().backup().join("Conversations").join("oc.git").exists(), "nothing was taken");
}

/// The list of backups of `with_file`'s project, made the list of `claude-sub`'s conversations
/// with `two` backups in it.
fn conversations_listed(name: &str) -> (Scratch, Harness<Screen>) {
    let (scratch, mut harness) = with_file(name);
    harness.send(Msg::ShowBackups(None));
    harness.send(Msg::BackupsOf(1));
    harness.send(Msg::BackupsRead(reading(&harness), Ok(two())));
    (scratch, harness)
}

/// What bringing `claude-sub`'s conversations back came to.
fn conversations_back(done: Result<Brought, BackupTrouble>) -> Msg {
    Msg::Restored {
        project: "firefly".to_owned(),
        name: "Firefly".to_owned(),
        of: BackupOf::Conversations("claude-sub".to_owned()),
        file: None,
        done,
    }
}

#[test]
fn the_list_of_backups_switches_to_a_profiles_conversations_and_back() {
    let (_scratch, mut harness) = with_file("of-switch");
    harness.send(Msg::ShowBackups(None));
    let first = reading(&harness);
    let text = harness.screen();
    assert!(text.contains("Project files"), "the choice shows what the list is of:\n{text}");
    harness.send(Msg::BackupsOf(1));
    assert_ne!(reading(&harness), first, "read afresh");
    harness.send(Msg::BackupsRead(first, Ok(two())));
    assert!(!harness.screen().contains("09:05"), "the answer for the project's list is dropped");
    let text = harness.screen();
    assert!(text.contains("Conversations of claude-sub"), "{text}");
    harness.send(Msg::BackupsRead(reading(&harness), Ok(Vec::new())));
    assert!(harness.screen().contains("No backup of these conversations yet"), "{}", harness.screen());
    harness.send(Msg::BackupsOf(0));
    harness.send(Msg::BackupsOf(1));
    harness.send(Msg::BackupsRead(reading(&harness), Ok(two())));
    let text = harness.screen();
    assert!(text.contains("09:05") && text.contains("3 files changed"), "{text}");
    assert!(!text.contains(|cell| "[]┌┐└┘─│".contains(cell)), "no brackets or boxes:\n{text}");

    harness.send(Msg::BackupsOf(0));
    assert!(harness.screen().contains("Project files"), "{}", harness.screen());
    harness.set_locale("tr").render();
    harness.send(Msg::BackupsOf(1));
    assert!(harness.screen().contains("claude-sub sohbetleri"), "{}", harness.screen());
}

#[test]
fn a_files_list_and_a_project_with_no_profile_offer_no_choice() {
    let (_scratch, mut file_list) = with_file("of-none");
    right_click(&mut file_list, "README.md");
    file_list.click_text("Earlier versions").advance(MOMENT);
    assert!(!file_list.screen().contains("Project files"), "{}", file_list.screen());
    file_list.send(Msg::BackupsOf(1));
    assert!(file_list.screen().contains("Earlier versions of README.md"), "nothing to switch to");

    let scratch = Scratch::new("of-no-profile");
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    let mut harness = harness(ProjectScreen::new(Some(engine()), HostUser::ImageDefault, projects), SIZE.0, SIZE.1);
    harness.send(Msg::ShowBackups(None));
    assert!(!harness.screen().contains("Project files"), "{}", harness.screen());
}

#[test]
fn bringing_conversations_back_asks_first_and_says_what_it_does() {
    let (_scratch, mut harness) = conversations_listed("of-confirm");
    harness.press("down").press("enter").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Bring back the conversations of claude-sub"), "{text}");
    assert!(!text.contains("Stop and"), "its container is not known to run:\n{text}");
    // The engine is not there, so the restore fails and the list stays to try again.
    confirm(&mut harness);
    let text = harness.screen();
    assert!(text.contains("Nothing was brought back"), "{text}");
    assert!(text.contains("09:05"), "the list is still open:\n{text}");
}

#[test]
fn conversations_under_a_running_container_are_brought_back_after_stopping_it() {
    let (_scratch, mut harness) = conversations_listed("of-running");
    let running = Container { name: "qcode-firefly-claude-sub".to_owned(), state: ContainerState::Running };
    harness.send(Msg::ContainersRead("firefly".to_owned(), Ok(vec![running])));
    harness.press("down").press("enter").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Stop and bring back"), "{text}");
    assert!(text.contains("is running"), "{text}");
    // Stopping needs the engine, which is not there: nothing is brought back, and it says so.
    press_beside_cancel(&mut harness, "Stop and bring back");
    assert!(harness.screen().contains("Nothing was brought back"), "{}", harness.screen());
}

#[test]
fn a_container_found_running_by_the_restore_is_offered_to_be_stopped() {
    let (_scratch, mut harness) = conversations_listed("of-found-running");
    harness.press("down").press("enter").advance(MOMENT);
    confirm(&mut harness);
    // The panel had not heard it was running; the restore found out.
    harness.send(conversations_back(Ok(Brought::Running)));
    let text = harness.screen();
    assert!(text.contains("Stop and bring back"), "asked again, offering to stop it:\n{text}");
}

#[test]
fn conversations_brought_back_close_the_list_and_a_missing_home_is_said_plainly() {
    let (_scratch, mut harness) = conversations_listed("of-done");
    harness.send(conversations_back(Ok(Brought::NoHome)));
    let text = harness.screen();
    assert!(text.contains("claude-sub has no home in this project"), "{text}");
    assert!(text.contains("09:05"), "the list stays:\n{text}");

    let before = crate::backup::Restored::Done { before: made(today_at(15, 0)) };
    harness.send(conversations_back(Ok(Brought::Done(before))));
    let text = harness.screen();
    assert!(text.contains("Conversations of claude-sub are back"), "{text}");
    assert!(!text.contains("09:05"), "the list closed:\n{text}");
    assert!(backup_row(&mut harness).contains("not yet"), "the project's own last backup is not the conversations'");
}

/// Unfolds only the panel's project widget, where the backup's switch and rows are.
fn only_info(harness: &mut Harness<Screen>) {
    let order = harness.app().0.panel().order().to_vec();
    for (index, widget) in order.iter().enumerate() {
        harness.send(Msg::ToggleWidget(index, *widget == PanelWidget::Info));
    }
}

/// Whether the project's file on disk backs its assets up.
fn assets_on_disk(scratch: &Scratch) -> bool {
    let text = fs::read_to_string(scratch.paths().file).expect("the project file");
    ProjectFile::parse("project.qcode", &text).value.expect("a project").backup_assets
}

#[test]
fn the_assets_switch_is_written_into_the_project_file_and_back_out() {
    let (scratch, mut harness) = with_file("assets-switch");
    only_info(&mut harness);
    assert!(harness.screen().contains("Back up Assets too"), "{}", harness.screen());
    harness.click_text("Back up Assets too").advance(MOMENT);
    assert!(harness.app().0.project().expect("open").backs_up_assets());
    assert!(assets_on_disk(&scratch));
    harness.click_text("Back up Assets too").advance(MOMENT);
    assert!(!harness.app().0.project().expect("open").backs_up_assets());
    assert!(!fs::read_to_string(scratch.paths().file).expect("the file").contains("assets"), "off is not written");
    harness.set_locale("tr").render();
    assert!(harness.screen().contains("Varlıkları yedekle"), "{}", harness.screen());
}

#[test]
fn an_assets_switch_that_cannot_be_written_says_so_and_stays_off() {
    let scratch = Scratch::new("assets-no-file");
    let mut harness = backing_up(&scratch, BackupEvery::Fifteen);
    harness.send(Msg::BackupAssets(true));
    assert!(harness.screen().contains("could not be written"), "{}", harness.screen());
    assert!(!harness.app().0.project().expect("open").backs_up_assets());
}

#[test]
fn backed_up_assets_are_listed_and_brought_back_after_asking() {
    let (_scratch, mut harness) = with_file("assets-list");
    harness.send(Msg::ShowBackups(None));
    harness.send(Msg::BackupsOf(1));
    assert!(!harness.screen().contains("No backup of Assets"), "off, the list offers no assets:\n{}", harness.screen());
    harness.send(Msg::CloseBackups);

    harness.send(Msg::BackupAssets(true));
    harness.send(Msg::ShowBackups(None));
    harness.send(Msg::BackupsOf(1));
    harness.send(Msg::BackupsRead(reading(&harness), Ok(Vec::new())));
    assert!(harness.screen().contains("No backup of Assets yet"), "{}", harness.screen());
    harness.send(Msg::BackupsOf(0));
    harness.send(Msg::BackupsOf(1));
    harness.send(Msg::BackupsRead(reading(&harness), Ok(two())));
    harness.press("down").press("enter").advance(MOMENT);
    let text = harness.screen();
    assert!(text.contains("Bring back Assets of 09:05?"), "{text}");
    assert!(text.contains("Nothing is") && text.contains("deleted"), "{text}");

    let before = crate::backup::Restored::Done { before: made(today_at(15, 0)) };
    harness.send(Msg::Restored {
        project: "firefly".to_owned(),
        name: "Firefly".to_owned(),
        of: BackupOf::Assets,
        file: None,
        done: Ok(Brought::Done(before)),
    });
    assert!(harness.screen().contains("Assets are back as they were"), "{}", harness.screen());
    assert!(harness.app().0.listing.is_none(), "the list closed");
}

#[test]
fn a_round_takes_the_assets_when_the_project_asks_for_them() {
    let (_scratch, mut harness) = with_file("assets-round");
    harness.send(Msg::BackupAssets(true));
    let left = leaving(&harness.app().0).expect("backups are on");
    let lines = crate::ui::settings::testing::translated("en", || left.back_up());
    assert_eq!(lines.len(), 2, "the project and its assets each failed without an engine: {lines:?}");
    assert!(lines[1].starts_with("Assets not backed up in Firefly: "), "{lines:?}");

    harness.send(Msg::BackedUp {
        project: "firefly".to_owned(),
        name: "Firefly".to_owned(),
        made: Ok(Snapshot::Unchanged),
        missed: vec![(Part::Assets, BackupTrouble::Said("gone".to_owned()))],
    });
    assert!(harness.screen().contains("Assets not backed up"), "{}", harness.screen());

    harness.send(Msg::BackupAssets(false));
    let left = leaving(&harness.app().0).expect("backups are on");
    let lines = crate::ui::settings::testing::translated("en", || left.back_up());
    assert_eq!(lines.len(), 1, "off, only the project is backed up: {lines:?}");
}

#[test]
fn a_size_is_said_in_the_unit_that_keeps_it_short_and_in_the_persons_writing() {
    let said = |locale: &str, count| crate::ui::settings::testing::translated(locale, || backups::bytes(count));
    assert_eq!(said("en", 0), "0 B");
    assert_eq!(said("en", 999), "999 B");
    assert_eq!(said("en", 1_500), "1.5 KB");
    assert_eq!(said("en", 9_999), "9.9 KB");
    assert_eq!(said("en", 12_345_678), "12 MB");
    assert_eq!(said("en", 3_200_000_000), "3.2 GB");
    assert_eq!(said("tr", 1_500), "1,5 KB");
}

/// The row of the project widget that says how much the backup holds.
fn size_row(harness: &mut Harness<Screen>) -> String {
    info_row(harness, &["Backup size", "Yedek boyutu"])
}

#[test]
fn the_panel_says_how_much_the_backup_holds_and_reads_it_again_after_a_round() {
    let (scratch, mut harness) = with_file("size-row");
    assert!(size_row(&mut harness).contains("0 B"), "{}", harness.screen());
    let git = scratch.paths().backup().join("Project.git");
    fs::create_dir_all(&git).expect("a backup folder");
    fs::write(git.join("pack"), vec![0; 12_000]).expect("a file of the backup");
    assert!(size_row(&mut harness).contains("0 B"), "the disk is not read while drawing");
    harness.send(backed(Ok(made(1_000))));
    assert!(size_row(&mut harness).contains("12 KB"), "{}", harness.screen());
    // Without an engine it is still read: it is this machine's disk.
    let projects = vec![project("firefly", "Firefly", scratch.paths(), Vec::new())];
    let mut offline = super::harness(ProjectScreen::new(None, HostUser::ImageDefault, projects), SIZE.0, SIZE.1);
    offline.set_reduced_motion(true).set_locale("tr").render();
    assert!(size_row(&mut offline).contains("12 KB"), "{}", offline.screen());
}
