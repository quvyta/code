//! Editing a profile and rebuilding its image with today's recipe, over an engine that records
//! what it is asked, and signing in again from the list.

use super::*;

/// A stand-in podman for building an image that is already there again. It writes down every
/// call; it has the profile's image under the identity `sha-old` with no revision label, the
/// way an image an earlier QCode built has, until a build finishes, and then `sha-new` with
/// the label the built Containerfile carried. The base image is always the current one. A build
/// fails when `fail` exists in the folder.
struct Rebuilt {
    folder: PathBuf,
    engine: Engine,
}

impl Rebuilt {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profiles-{name}-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a scratch folder");
        let script = r#"#!/bin/sh
printf '%s\n' "$*" >> FOLDER/calls
case "$1 $2 $4" in
'image inspect {{.Id}}') [ -e FOLDER/label ] && echo sha-new || echo sha-old; exit 0 ;;
esac
[ "$1 $2 $5" = "image inspect qcode/base" ] && { echo BASE; exit 0; }
[ "$1 $2" = "image inspect" ] && { cat FOLDER/label 2>/dev/null; echo; exit 0; }
[ "$1" = build ] || exit 0
echo 'STEP 1/9: FROM qcode/base'
[ -e FOLDER/fail ] && { echo 'Error: building at STEP 4/9: exit status 1' >&2; exit 1; }
while [ "$#" -gt 0 ]; do [ "$1" = --file ] && file="$2"; shift; done
cp "$file" FOLDER/built
sed -n 's/^LABEL qcode.profile.revision="\(.*\)"$/\1/p' "$file" > FOLDER/label
echo 'COMMIT qcode/profile/claude-sub'
"#
        .replace("FOLDER", &folder.display().to_string())
        .replace("BASE", &crate::base::revision());
        let binary = folder.join("engine");
        std::fs::write(&binary, script).expect("the stand-in engine is written");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        let engine = Engine::new(crate::engine::EngineKind::Podman, &binary);
        Self { folder, engine }
    }

    /// The profiles screen on this engine, holding `profile` and with the engine's answers
    /// about it in, as the screen asks for them itself.
    fn screen(&self, profile: Profile) -> Harness<Host> {
        let state = Profiles::new(Some(self.folder.clone()), Some(self.engine.clone())).with_providers_file(None);
        let mut harness = Harness::with_env(Host { state }, env(), SIZE.0, 40);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Loaded(Listing { profiles: vec![profile], diagnostics: Vec::new() })).render();
        harness
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.folder.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn builds(&self) -> Vec<String> {
        self.calls().into_iter().filter(|call| call.starts_with("build")).collect()
    }
}

impl Drop for Rebuilt {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

#[test]
fn an_image_an_earlier_qcode_built_says_so_and_rebuilding_it_is_asked_first() {
    let engine = Rebuilt::new("rebuild-asked");
    let mut harness = engine.screen(profile("claude-sub", HarnessKind::ClaudeCode));
    let screen = harness.screen();
    assert!(screen.contains("Image ready"), "{screen}");
    assert!(screen.contains("Built by an earlier QCode; rebuild for today's image."), "{screen}");
    harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
    // The question wraps inside its frame, so it is read as one line of words.
    let screen = harness.screen().split_whitespace().filter(|word| *word != "▌").collect::<Vec<_>>().join(" ");
    assert!(screen.contains("Rebuild the image of claude-sub?"), "{screen}");
    assert!(screen.contains("the harness is downloaded again"), "{screen}");
    assert!(screen.contains("the login and the conversations are kept"), "{screen}");
    assert!(screen.contains("open tabs stay on the old image until opened again"), "{screen}");
    assert!(screen.contains("the old image stays"), "{screen}");
    assert!(engine.builds().is_empty(), "nothing is built before the answer: {:#?}", engine.calls());
    harness.press("esc").advance(std::time::Duration::from_millis(400));
    assert!(engine.builds().is_empty(), "a question put away builds nothing: {:#?}", engine.calls());
    assert!(harness.app().state.draft().is_none(), "{}", harness.screen());
}

#[test]
fn rebuilding_builds_the_image_from_its_first_step_with_todays_recipe_and_lets_the_old_one_go() {
    let engine = Rebuilt::new("rebuild-done");
    let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
    let mut harness = engine.screen(chosen.clone());
    harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
    harness.click_text("Build it again").render();
    let builds = engine.builds();
    assert_eq!(builds.len(), 1, "{:#?}", engine.calls());
    assert!(builds[0].starts_with("build --no-cache --tag qcode/profile/claude-sub --file "), "{builds:#?}");
    let built = std::fs::read_to_string(engine.folder.join("built")).expect("a Containerfile was built");
    let recipe = recipe::image(&chosen);
    assert_eq!(built, recipe.labelled(), "today's recipe, labelled with its revision");
    let screen = harness.screen();
    assert!(screen.contains("Rebuilding the image of claude-sub"), "{screen}");
    assert!(screen.contains("STEP 1/9: FROM qcode/base"), "the build's own lines are shown:\n{screen}");
    assert!(screen.contains("The image of claude-sub is built again."), "{screen}");
    let calls = engine.calls();
    assert!(calls.contains(&"image rm sha-old".to_owned()), "the image it replaced is let go: {calls:#?}");
    assert!(!calls.iter().any(|call| call.contains("--force")), "never by force: {calls:#?}");
    assert!(
        !std::fs::read_dir(&engine.folder).expect("the folder").flatten().any(|entry| entry.file_name() == "Profiles"),
        "the definition is not written again"
    );
    harness.click_text("Close").render();
    let screen = harness.screen();
    assert!(harness.app().state.draft().is_none(), "{screen}");
}

#[test]
fn a_rebuilt_image_no_longer_says_an_earlier_qcode_built_it() {
    let engine = Rebuilt::new("rebuild-current");
    let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
    std::fs::write(engine.folder.join("label"), recipe::image(&chosen).revision()).expect("the label is set");
    let harness = engine.screen(chosen);
    let screen = harness.screen();
    assert!(screen.contains("Image ready"), "{screen}");
    assert!(!screen.contains("built by an earlier QCode"), "{screen}");
}

/// The definition file of `name` in the store of `engine`, as the disk has it.
fn definition(engine: &Rebuilt, name: &str) -> String {
    std::fs::read_to_string(engine.folder.join("Profiles").join(format!("{name}.toml"))).unwrap_or_default()
}

/// Opens the wizard on the chosen profile the way the person does, from its button under the
/// rows, and walks `pages` pages on with the wizard's own Next.
fn edit(harness: &mut Harness<Host>, pages: usize) {
    harness.click_text("Edit").render();
    assert!(harness.app().state.draft().is_some_and(Draft::is_editing), "{}", harness.screen());
    for _ in 0..pages {
        harness.click_text("Next").render();
    }
}

#[test]
fn editing_a_profile_opens_the_wizard_on_it_with_its_name_kept() {
    let engine = Rebuilt::new("edit-open");
    let mut chosen = profile("claude-sub", HarnessKind::ClaudeCode);
    chosen.network = NetworkMode::None;
    let mut harness = engine.screen(chosen);
    edit(&mut harness, 0);
    let screen = harness.screen();
    assert!(screen.contains("The name stays as it is"), "{screen}");
    assert!(screen.contains("claude-sub"), "{screen}");
    assert!(!screen.contains("The name of the definition file"), "no field that would not take: {screen}");
    // Where the name stands, a click and typing change nothing: there is no field to take it.
    let (x, y) = harness.find("claude-sub").expect("the name is shown");
    harness.click(x + 2, y).render();
    harness.type_text("-new").render();
    assert_eq!(harness.app().state.draft().expect("open").name, "claude-sub", "{}", harness.screen());
    let draft = harness.app().state.draft().expect("open");
    assert_eq!(draft.network, NetworkMode::None, "everything else comes as the profile has it");
    // Leaving before the image page writes nothing.
    harness.click_text("Cancel").render();
    assert_eq!(definition(&engine, "claude-sub"), "", "nothing was saved");
    assert!(engine.builds().is_empty(), "{:#?}", engine.calls());
}

#[test]
fn a_change_the_image_is_built_from_is_saved_through_a_rebuild() {
    let engine = Rebuilt::new("edit-image");
    let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
    let mut harness = engine.screen(chosen.clone());
    edit(&mut harness, 2);
    harness.click_text("QCode extra").render();
    // Account, permissions, then the image page, which saves by building again.
    edit_on(&mut harness, 3);
    let builds = engine.builds();
    assert_eq!(builds.len(), 1, "one rebuild: {:#?}", engine.calls());
    assert!(builds[0].starts_with("build --no-cache --tag qcode/profile/claude-sub "), "{builds:#?}");
    let changed = Profile { template: Template::High, ..chosen };
    let built = std::fs::read_to_string(engine.folder.join("built")).expect("a Containerfile was built");
    assert_eq!(built, recipe::image(&changed).labelled(), "the changed profile's recipe");
    let text = definition(&engine, "claude-sub");
    assert!(text.contains("template = \"high\""), "{text}");
    let read = Profile::parse("claude-sub.toml", &text).profile.expect("the file reads");
    assert_eq!(read, changed);
    let screen = harness.screen();
    assert!(screen.contains("is built again with the changes"), "{screen}");
    // The login it has stays: the account and the harness did not change.
    assert_eq!(harness.app().state.draft().expect("open").stages(), Stage::WITHOUT_LOGIN);
    harness.click_text("Finish").render();
    assert!(harness.app().state.draft().is_none(), "{}", harness.screen());
    assert_eq!(harness.app().state.made, None, "a changed profile is not a new one");
}

#[test]
fn a_change_of_account_brings_the_sign_in_back_and_builds_nothing() {
    let engine = Rebuilt::new("edit-account");
    let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
    let mut harness = engine.screen(chosen.clone());
    edit(&mut harness, 3);
    let (x, y) = radio_mark(&harness, "API key");
    harness.click(i32::from(x) + 1, i32::from(y)).render();
    assert_eq!(harness.app().state.draft().expect("open").account, AccountKind::ApiKey, "{}", harness.screen());
    edit_on(&mut harness, 2);
    assert!(engine.builds().is_empty(), "nothing the image is built from changed: {:#?}", engine.calls());
    let read = Profile::parse("claude-sub.toml", &definition(&engine, "claude-sub")).profile.expect("saved");
    assert_eq!(read, Profile { account: AccountKind::ApiKey, ..chosen });
    // The login it has was made for a subscription; the sign-in page is the next one.
    harness.click_text("Next").render();
    let draft = harness.app().state.draft().expect("open");
    assert_eq!(draft.stage, Stage::Login, "{}", harness.screen());
    assert!(harness.screen().contains("Open the sign-in"), "{}", harness.screen());
}

#[test]
fn a_change_of_the_network_is_saved_without_a_rebuild_and_remakes_a_stopped_container() {
    let engine = Rebuilt::new("edit-network");
    let chosen = profile("claude-sub", HarnessKind::ClaudeCode);
    let mut harness = engine.screen(chosen.clone());
    edit(&mut harness, 4);
    let (x, y) = radio_mark(&harness, "none");
    harness.click(i32::from(x) + 1, i32::from(y)).render();
    edit_on(&mut harness, 1);
    assert!(engine.builds().is_empty(), "the network is the containers', not the image's: {:#?}", engine.calls());
    let screen = harness.screen();
    assert!(screen.contains("the image is kept as it is"), "{screen}");
    assert!(screen.contains("made again with the changes the next time it starts"), "{screen}");
    let read = Profile::parse("claude-sub.toml", &definition(&engine, "claude-sub")).profile.expect("saved");
    assert_eq!(read, Profile { network: NetworkMode::None, ..chosen.clone() });

    // A workspace's stopped container of the profile, made from the plan as it was, is made
    // again from the one the saved profile gives, with the same home volume.
    let workspace = crate::store::WorkspaceId::parse("firefly").expect("an id");
    let paths = crate::store::WorkspacePaths {
        root: engine.folder.join("w"),
        file: engine.folder.join("w").join("workspace.qcode"),
        code: engine.folder.join("w").join("Work"),
        assets: engine.folder.join("w").join("Assets"),
        harness: engine.folder.join("w").join("Containers").join("Harness"),
    };
    let user = crate::engine::HostUser::Ids { uid: 1000, gid: 1000 };
    let stand_in = engine.folder.join("containers");
    let before = crate::ui::workspace::ContainerPlan::profile(&workspace, &paths, &chosen);
    let after = crate::ui::workspace::ContainerPlan::profile(&workspace, &paths, &read);
    let podman = Engine::new(crate::engine::EngineKind::Podman, &stand_in);
    assert_ne!(before.digest(&podman, user), after.digest(&podman, user));
    let script = r#"#!/bin/sh
printf '%s\n' "$*" >> FOLDER/container-calls
case "$1 $2 $4" in
'container inspect {{.State.Status}}') echo exited; exit 0 ;;
'container inspect {{.Image}}') echo sha-same; exit 0 ;;
'container inspect '*) echo DIGEST; exit 0 ;;
'image inspect {{.Id}}') case "$5" in qcode/workspace/*) exit 1 ;; esac; echo sha-same; exit 0 ;;
esac
exit 0
"#
    .replace("FOLDER", &engine.folder.display().to_string())
    .replace("DIGEST", &before.digest(&podman, user));
    std::fs::write(&stand_in, script).expect("the stand-in engine");
    std::fs::set_permissions(&stand_in, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
    crate::ui::workspace::ensure_running(&podman, &after, user).expect("it comes up");
    let calls = std::fs::read_to_string(engine.folder.join("container-calls")).expect("calls");
    let calls: Vec<&str> = calls.lines().collect();
    let removed = calls.iter().position(|call| call.starts_with("rm ") && call.contains("qcode-firefly-claude-sub"));
    let created = calls.iter().position(|call| call.starts_with("create ") && call.contains("--network=none"));
    assert!(removed.is_some() && created.is_some() && removed < created, "made again: {calls:#?}");
    let label = format!("qcode.plan={}", after.digest(&podman, user));
    assert!(calls.iter().any(|call| call.contains(&label)), "with the new plan's label: {calls:#?}");
    assert!(!calls.iter().any(|call| call.starts_with("volume rm")), "the home stays: {calls:#?}");
}

/// Walks `pages` more pages on with the wizard's own Next.
fn edit_on(harness: &mut Harness<Host>, pages: usize) {
    for _ in 0..pages {
        harness.click_text("Next").render();
    }
}

#[test]
fn a_rebuild_that_fails_says_the_old_image_is_kept_and_removes_nothing() {
    let engine = Rebuilt::new("rebuild-failed");
    std::fs::write(engine.folder.join("fail"), "").expect("the build will fail");
    let mut harness = engine.screen(profile("claude-sub", HarnessKind::ClaudeCode));
    harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
    harness.click_text("Build it again").render();
    let screen = harness.screen();
    assert!(screen.contains("The rebuild failed."), "{screen}");
    assert!(screen.contains("the image that was there before is kept"), "{screen}");
    assert!(screen.contains("exit status 1"), "the engine's own words:\n{screen}");
    let calls = engine.calls();
    assert!(!calls.iter().any(|call| call.starts_with("image rm")), "the old image is kept: {calls:#?}");
}

#[test]
fn a_profile_without_an_image_offers_no_rebuild() {
    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing {
        profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        diagnostics: Vec::new(),
    }));
    answer(&mut harness, Readiness::Missing, Readiness::Missing);
    harness.click_text("Rebuild image").advance(std::time::Duration::from_millis(400));
    assert!(!harness.screen().contains("Rebuild the image of"), "{}", harness.screen());
}

#[test]
fn the_wizard_says_from_the_account_on_that_it_signs_in_once_for_every_workspace_and_what_skipping_costs() {
    let (folder, mut harness) = recording("sign-in-step");
    harness.click_text("New profile").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    harness.click_text("Next").render();
    let words = |harness: &Harness<Host>| harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    let account = words(&harness);
    assert!(account.contains("The last step signs Claude Code in once;"), "{account}");
    assert!(account.contains("the login is kept with the profile"), "{account}");
    harness.click_text("Next").render();
    // Arriving on the image page is what starts the build; the stand-in engine finishes it.
    harness.click_text("Next").render();
    assert!(matches!(harness.app().state.draft().expect("open").build, Build::Done), "{}", harness.screen());
    harness.click_text("Next").render();
    let page = words(&harness);
    assert!(page.contains("Sign in"), "the step is named:\n{page}");
    assert!(page.contains("every workspace gets this login from here"), "{page}");
    assert!(page.contains("You can finish without signing in"), "{page}");
    assert!(page.contains("each workspace then asks inside Claude Code"), "{page}");

    // The engine's answer that the login was found and stored, which only a container gives.
    harness.send(Msg::LoginStored(Ok(3))).render();
    let page = words(&harness);
    assert!(page.contains("Signed in. 3 files were stored"), "{page}");
    assert!(page.contains("Kept. Every workspace that opens this profile from now on gets this login."), "{page}");
    assert!(!page.contains("You can finish without signing in"), "said only before a login:\n{page}");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn signing_in_again_from_the_list_does_not_speak_of_finishing_without_it() {
    let mut harness = with_engine();
    harness.send(Msg::Loaded(Listing {
        profiles: vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        diagnostics: Vec::new(),
    }));
    answer(&mut harness, Readiness::Present, Readiness::Missing);
    harness.click_text("claude-sub").render();
    harness.click_text("Sign in").render();
    let page = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(page.contains("every workspace gets this login from here"), "{page}");
    assert!(!page.contains("You can finish without signing in"), "{page}");
}
