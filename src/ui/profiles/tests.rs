//! The profiles screen driven on its own, without a container runtime: the list, the wizard's
//! pages, a build and a sign-in, and editing and rebuilding a profile.

use super::*;
use qframe::env::{AssetDirs, Env};
use qframe::icons::GlyphMode;
use qframe::runtime::{Harness, Task};

use crate::profile::{HarnessKind, NetworkMode};

mod display;
mod parts;
mod placement;
mod rebuild;
mod systems;
mod wizard_pages;

/// A terminal wide enough for the list beside its detail and for a wizard page.
const SIZE: (u16, u16) = (96, 34);

/// The screen on its own, so that a test drives exactly this module and nothing else. The
/// application's message is this module's message, which is what wiring the screen into
/// QCode does with one more layer around it.
struct Host {
    state: Profiles,
}

impl App for Host {
    type Msg = Msg;

    fn update(&mut self, message: Msg) -> Command<Msg> {
        update(&mut self.state, message)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        AppShell::new().body(|ui| view(&self.state, ui)).show(ui);
    }
}

fn env() -> Env {
    Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() }).expect("the built-in files load")
}

fn profile(name: &str, harness: HarnessKind) -> Profile {
    Profile {
        name: SafeName::parse(name).expect("the name is safe"),
        harness,
        template: Template::Recommended,
        account: AccountKind::Subscription,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// A store path that is never read: the tests deliver what the disk would have given,
/// so nothing here touches a file system or an engine.
fn root() -> PathBuf {
    std::env::temp_dir().join(format!("qcode-profiles-screen-{}", std::process::id()))
}

/// A screen with a store and no engine, on a terminal of `width` by `height`.
fn screen(width: u16, height: u16) -> Harness<Host> {
    let state = Profiles::new(Some(root()), None).with_providers_file(None);
    let mut harness = Harness::with_env(Host { state }, env(), width, height);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.render();
    harness
}

/// A screen with a store and an engine that is never actually run: every answer the
/// engine would give is delivered as a message instead.
fn with_engine() -> Harness<Host> {
    // A binary that is not there: an engine QCode holds but never gets an answer out of, so
    // the test says what the answers are instead of a machine happening to have podman.
    let engine = Engine::new(crate::engine::EngineKind::Podman, "/qcode/no/such/engine");
    let state = Profiles::new(Some(root()), Some(engine)).with_providers_file(None);
    let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    harness.render();
    harness
}

/// A screen holding `profiles`, delivered the way the store would deliver them.
fn loaded(profiles: Vec<Profile>) -> Harness<Host> {
    let mut harness = screen(SIZE.0, SIZE.1);
    harness.send(Msg::Loaded(Listing { profiles, diagnostics: Vec::new() })).render();
    harness
}

/// The engine's answer about every profile on screen.
fn answer(harness: &mut Harness<Host>, image: Readiness, identity: Readiness) {
    let answers: Vec<Status> = harness
        .app()
        .state
        .rows()
        .iter()
        .map(|row| Status { name: row.profile.name.clone(), image, revision: Revision::Unknown, identity })
        .collect();
    harness.send(Msg::Probed(answers)).render();
}

#[test]
fn a_store_without_profiles_starts_empty() {
    let state = Profiles::new(None, None);
    assert!(state.rows().is_empty());
    assert!(!state.is_loading(), "without a store there is nothing to wait for");
}

#[test]
fn an_empty_store_says_that_nothing_runs_without_a_profile() {
    let harness = loaded(Vec::new());
    let screen = harness.screen();
    assert!(screen.contains("No profiles yet"), "{screen}");
    assert!(screen.contains("harness through a profile"), "{screen}");
    assert!(screen.contains("New profile"), "the way out is offered:\n{screen}");
}

#[test]
fn a_store_being_read_says_so_instead_of_looking_empty() {
    let root = std::env::temp_dir().join(format!("qcode-profiles-loading-{}", std::process::id()));
    let state = Profiles::new(Some(root), None);
    assert!(state.is_loading());
    let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, SIZE.1);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).render();
    let screen = harness.screen();
    assert!(screen.contains("Reading the profiles"), "{screen}");
    assert!(!screen.contains("No profiles yet"), "a store being read is not an empty one:\n{screen}");
}

#[test]
fn the_list_stands_in_the_middle_at_a_readable_width() {
    let mut harness =
        loaded(vec![profile("claude-main", HarnessKind::ClaudeCode), profile("gemini-work", HarnessKind::GeminiCli)]);
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    harness.resize(200, 34).render();
    let screen = harness.screen();
    // The heading at the page's left edge, the first row just inside it, and nothing further
    // right than the page, on a terminal twice as wide as the page.
    let left = (200 - i32::from(LIST_WIDTH)) / 2;
    let right = usize::try_from(left + i32::from(LIST_WIDTH)).expect("a column");
    let (heading, _) = harness.find("Profiles").expect("the heading is drawn");
    assert!(heading.abs_diff(left) <= 1, "the heading is not at the page's edge ({heading}):\n{screen}");
    let (first, _) = harness.find("New profile").expect("the first row is drawn");
    assert!((left..=left + 4).contains(&first), "the first row is not at the page's edge ({first}):\n{screen}");
    let lines: Vec<&str> = screen.lines().collect();
    let past = lines[..lines.len() - 1].iter().any(|line| line.trim_end().chars().count() > right);
    assert!(!past, "text reaches past the page:\n{screen}");

    // Narrower than the page, the page takes the terminal and the rows keep their names.
    harness.resize(70, 34).render();
    let screen = harness.screen();
    assert!(screen.contains("claude-main") && screen.contains("gemini-work"), "{screen}");
    assert!(screen.contains("Claude Code") && screen.contains("Gemini CLI"), "{screen}");
    let (heading, _) = harness.find("Profiles").expect("the heading is drawn");
    assert!(heading < 4, "a narrow terminal keeps the page at its edge:\n{screen}");
}

#[test]
fn the_list_and_the_wizard_say_each_thing_in_few_words() {
    let mut harness = screen(120, 40);
    harness
        .send(Msg::Loaded(Listing {
            profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
            diagnostics: Vec::new(),
        }))
        .render();
    let flat = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    let gone = |page: &str, old: &[&str]| {
        for sentence in old {
            assert!(!page.contains(sentence), "`{sentence}` is still said:\n{page}");
        }
    };
    let page = flat(&harness);
    assert!(page.contains("No engine: nothing can be built or signed in."), "{page}");
    gone(&page, &["No container engine was found"]);

    harness.click_text("New profile").render();
    let page = flat(&harness);
    assert!(page.contains("Which coding agent this profile runs."), "{page}");
    gone(&page, &["The image is built for it and nothing else"]);
    harness.click_text("Next").render();
    let page = flat(&harness);
    assert!(page.contains("Your files keep opening in Debian."), "{page}");
    gone(&page, &["whatever is chosen here", "What every profile has been built on so far"]);
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let page = flat(&harness);
    assert!(page.contains("What this profile signs in with."), "{page}");
    gone(&page, &["The harness decides which", "The last step of this wizard"]);
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let page = flat(&harness);
    assert!(page.contains("the profiles are untouched"), "{page}");
    gone(&page, &["Images and logins live in podman or docker"]);
}

#[test]
fn a_broken_definition_file_is_counted_on_the_screen() {
    let mut harness = screen(SIZE.0, SIZE.1);
    let diagnostics = vec![qframe::diagnostics::Diagnostic::error(
        Some(qframe::diagnostics::Location { file: "half.toml".to_owned(), line: 1, column: 1 }),
        "`name` is missing",
    )];
    harness.send(Msg::Loaded(Listing { profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)], diagnostics }));
    harness.render();
    let screen = harness.screen();
    assert!(screen.contains("1 profile file could not be read"), "{screen}");
    assert!(screen.contains("claude-sub"), "the readable profile is still listed:\n{screen}");
}

#[test]
fn before_the_engine_answers_nothing_claims_to_be_ready() {
    let harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    let screen = harness.screen();
    assert!(screen.contains("Image not checked"), "{screen}");
    assert!(screen.contains("Login not checked"), "{screen}");
    assert!(!screen.contains("Signed in"), "nothing is signed in until the engine says so:\n{screen}");
}

#[test]
fn the_engines_answer_becomes_the_two_badges() {
    let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    let screen = harness.screen();
    assert!(screen.contains("Image ready"), "{screen}");
    assert!(screen.contains("Not signed in"), "{screen}");
    answer(&mut harness, Readiness::Present, Readiness::Present);
    assert!(harness.screen().contains("Signed in"), "{}", harness.screen());
}

#[test]
fn a_profile_on_another_system_names_it_in_the_list_and_one_on_debian_does_not() {
    let arch = Profile { os: Os::Arch, ..profile("claude-arch", HarnessKind::ClaudeCode) };
    let harness = loaded(vec![arch, profile("claude-sub", HarnessKind::ClaudeCode)]);
    let screen = harness.screen();
    assert!(screen.contains("Claude Code, QCode recommended, subscription, on Arch Linux"), "{screen}");
    assert!(!screen.contains("Debian"), "a profile on Debian reads as it did before:\n{screen}");
}

#[test]
fn a_profile_summary_says_what_it_runs_and_what_it_may_reach() {
    let harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    let screen = harness.screen();
    assert!(screen.contains("Claude Code"), "{screen}");
    assert!(screen.contains("Claude Code, QCode recommended, subscription"), "{screen}");
    assert!(screen.contains("subscription"), "{screen}");
    assert!(screen.contains("assets read-only"), "{screen}");
    assert!(screen.contains("network full"), "{screen}");
}

#[test]
fn without_an_engine_the_screen_says_what_it_cannot_do_and_the_buttons_are_dead() {
    let mut harness = loaded(vec![profile("claude-sub", HarnessKind::ClaudeCode)]);
    answer(&mut harness, Readiness::Present, Readiness::Present);
    let screen = harness.screen();
    assert!(
        screen.contains("No engine: nothing can be built or signed in."),
        "the reason stands beside the dead buttons:\n{screen}"
    );
    assert!(!screen.contains("No container engine was found"), "said in fewer words:\n{screen}");
    harness.click_text("Sign out").render();
    assert!(!harness.screen().contains("Sign claude-sub out?"), "a dead button asks nothing:\n{screen}");
}

#[test]
fn signing_out_is_asked_first_and_says_exactly_what_goes() {
    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing {
        profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        diagnostics: Vec::new(),
    }));
    answer(&mut harness, Readiness::Present, Readiness::Present);
    harness.send(Msg::SignOutAsked).render();
    let screen = harness.screen();
    assert!(screen.contains("Sign claude-sub out?"), "{screen}");
    assert!(screen.contains("Workspaces that already have a copy keep working"), "{screen}");
    assert!(screen.contains("new workspaces get no login"), "{screen}");
}

#[test]
fn a_profile_that_is_not_signed_in_cannot_be_signed_out() {
    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing {
        profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        diagnostics: Vec::new(),
    }));
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    harness.send(Msg::SignOutAsked).render();
    assert!(!harness.screen().contains("Sign claude-sub out?"), "{}", harness.screen());
}

/// Clicks the wizard's `Next` button, the way a person moves on a page.
fn next(harness: &mut Harness<Host>) {
    harness.click_text("Next").render();
}

/// The column of `label` on the row where it follows the small square of a radio group, and
/// that row, so a test can look at the mark two cells before it.
fn radio_mark(harness: &Harness<Host>, label: &str) -> (u16, u16) {
    let square = harness.env().icons().glyph("radio-mark-small").into_owned();
    let screen = harness.screen();
    let (y, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(&format!("{square}  {label}")))
        .unwrap_or_else(|| panic!("`{label}` is offered with the small square:\n{screen}"));
    let at = line.find(&format!("{square}  {label}")).expect("the option is on the row");
    let x = line[..at].chars().count();
    (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"))
}

/// A wizard opened on a new profile and walked `pages` pages on.
fn wizard_on(pages: usize) -> Harness<Host> {
    let mut harness = loaded(Vec::new());
    harness.set_reduced_motion(true).send(Msg::New);
    for _ in 0..pages {
        harness.send(Msg::Next);
    }
    harness.render();
    harness
}

/// Where the switch of the row labelled `label` is drawn: the rightmost cell of that row whose
/// ground is not the row's own, a cell inside the switch's track.
fn switch_of(harness: &mut Harness<Host>, label: &str) -> (i32, i32) {
    // A tooltip under the pointer lies over the row below it, switch and all, so the pointer
    // is moved off the page first, as a person's eye would wait for the tip to go.
    harness.hover(0, 0).render();
    let screen = harness.screen();
    let (y, line) = screen
        .lines()
        .enumerate()
        // A row's label stands apart from everything else on its line by two blanks or more.
        .find(|(_, line)| line.split("  ").any(|chunk| chunk.trim_matches([' ', '▌']) == label))
        .unwrap_or_else(|| panic!("`{label}` has a row:\n{screen}"));
    let y = u16::try_from(y).expect("on screen");
    let start = line.find(label).map(|at| line[..at].chars().count()).expect("the label is on the row");
    let after = u16::try_from(start + label.chars().count() + 1).expect("on screen");
    let ground = harness.bg(after, y);
    // The whole width, not the line's: a switch is drawn in coloured blanks, which the text of
    // the screen trims away. The first such cell after the label is its own switch, also when
    // rows stand in columns side by side; one cell further in is inside the track.
    let width = harness.buffer().area.width;
    let cell = (after..width)
        .find(|x| harness.bg(*x, y) != ground)
        .unwrap_or_else(|| panic!("`{label}` has a switch on its row:\n{screen}"));
    (i32::from(cell) + 1, i32::from(y))
}

/// Whether the switch on the row that says `label` is on, read off the screen rather than off
/// the state. A switch that is on is its lit track across the three cells the knob is not in;
/// one that is off has the knob, which is painted in a colour of its own, over the first two.
fn switch_on(harness: &mut Harness<Host>, label: &str) -> bool {
    let (x, y) = switch_of(harness, label);
    let y = u16::try_from(y).expect("on screen");
    let left = u16::try_from(x).expect("on screen").saturating_sub(1);
    let cells: Vec<Option<qframe::color::Rgb>> = (left..left + 5).map(|at| harness.bg(at, y)).collect();
    cells[0] == cells[1] && cells[1] == cells[2]
}

/// Where the switch of `part`'s row is.
fn part_switch(harness: &mut Harness<Host>, part: Extra) -> (i32, i32) {
    let label = row_of(harness, part);
    switch_of(harness, &label)
}

/// Whether `part`'s switch reads on, off the screen.
fn part_on(harness: &mut Harness<Host>, part: Extra) -> bool {
    let label = row_of(harness, part);
    switch_on(harness, &label)
}

/// The label a part's row carries on the page: the page's own words for the parts it names in
/// words, and the part's own name for the rest.
fn row_of(harness: &Harness<Host>, part: Extra) -> String {
    let key = match part {
        Extra::Rust => "profiles.template.rust",
        Extra::Settings => "profiles.template.settings",
        _ => return part.id().to_owned(),
    };
    harness.env().i18n().translate(key, &[])
}

/// A profiles screen on a store in a scratch folder of its own, with an engine that is a script
/// keeping every Containerfile it is asked to build, under the image's tag with the slashes
/// turned into dashes, so what reaches an image is read from there. Nothing is built for real.
fn recording(name: &str) -> (PathBuf, Harness<Host>) {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let folder = std::env::temp_dir().join(format!("qcode-profiles-{name}-{stamp}"));
    std::fs::create_dir_all(&folder).expect("a scratch folder");
    // Each image's Containerfile is kept under its tag, with the slashes turned into dashes.
    let script = format!(
        "#!/bin/sh\n\
         [ \"$1\" = build ] || exit 0\n\
         while [ \"$#\" -gt 0 ]; do\n\
         case \"$1\" in --tag) tag=\"$2\" ;; --file) file=\"$2\" ;; esac\n\
         shift\n\
         done\n\
         cp \"$file\" \"{folder}/built-$(echo \"$tag\" | tr / -)\"\n",
        folder = folder.display()
    );
    let binary = folder.join("engine");
    std::fs::write(&binary, script).expect("the stand-in engine is written");
    std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
    let engine = Engine::new(crate::engine::EngineKind::Podman, &binary);
    let mut harness =
        Harness::with_env(Host { state: Profiles::new(Some(folder.clone()), Some(engine)) }, env(), 96, 60);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.send(Msg::Loaded(Listing { profiles: Vec::new(), diagnostics: Vec::new() })).render();
    (folder, harness)
}
