//! A profile's shell, driven from where the person's hand is: the list's button, the page's
//! "As administrator" and "Close the shell", and the question's two answers.
//!
//! The engine is a script that writes down every call and answers the way an engine would: the
//! shell's terminals wait like a shell, `diff` lists what a person who installed a program as
//! root and wrote a note in the home would have changed, the administrator's record holds the
//! command they ran, and a copy out of the container leaves an archive. What is checked is what
//! QCode asks of the engine and what it keeps.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use qframe::env::{AssetDirs, Env};
use qframe::icons::GlyphMode;
use qframe::runtime::Harness;

use super::*;
use crate::profile::{HarnessKind, MountAccess, NetworkMode, Template};

/// The screen on its own.
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

/// What the stand-in `diff` answers: a program installed as root, and a note in the home.
const DIFF: &str = "C /etc\nC /home\nC /home/qcode\nA /home/qcode/notes.txt\nC /usr\nC /usr/bin\nA /usr/bin/jq\n\
                    C /usr/lib\nA /usr/lib/libjq.so.1\nC /tmp\nA /tmp/qcode-shell-root-history\n";

/// A stand-in engine and the folder it answers from, which is also the store.
struct Stand {
    folder: PathBuf,
    engine: Engine,
    profile: Profile,
}

impl Stand {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-profile-shell-{name}-{stamp}"));
        std::fs::create_dir_all(folder.join("Profiles")).expect("a store");
        std::fs::write(folder.join("diff"), DIFF).expect("the diff");
        std::fs::write(folder.join("history"), "apt-get update\napt-get install -y jq\nexit\n").expect("the record");
        let script = r#"#!/bin/sh
printf '%s\n' "$*" >> FOLDER/calls
case "$1" in
container) [ -e FOLDER/leftover ] && { echo exited; exit 0; }; echo 'Error: no such container' >&2; exit 1 ;;
rm) rm -f FOLDER/leftover; exit 0 ;;
diff) cat FOLDER/diff; exit 0 ;;
cp) printf 'an archive' > "$3"; exit 0 ;;
build) while [ "$#" -gt 0 ]; do [ "$1" = --file ] && file="$2"; shift; done; cp "$file" FOLDER/built
       cp "$(dirname "$file")/own-home.tar" FOLDER/built-archive 2>/dev/null; exit 0 ;;
image) case "$*" in *'{{.Id}}'*) echo sha-old ;; esac; exit 0 ;;
exec)
  case "$*" in
  *--tty*) while :; do echo 'qcode@qcode:~$'; sleep 0.2; done ;;
  *qcode-shell-root-history*) case "$*" in *'cat '*) cat FOLDER/history ;; esac ;;
  esac
  exit 0 ;;
esac
exit 0
"#
        .replace("FOLDER", &folder.display().to_string());
        let binary = folder.join("engine");
        std::fs::write(&binary, script).expect("the stand-in engine is written");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        let profile = Profile {
            name: SafeName::parse(&format!("shell-{name}")).expect("safe"),
            harness: HarnessKind::ClaudeCode,
            template: Template::Base,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::None,
            without: Vec::new(),
            os: Os::Debian,
        };
        Store::new(&folder).write_profile(&profile).expect("the profile's file");
        Self { engine: Engine::new(crate::engine::EngineKind::Podman, &binary), folder, profile }
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.folder.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn screen(&self) -> Harness<Host> {
        let state = Profiles::new(Some(self.folder.clone()), Some(self.engine.clone())).with_providers_file(None);
        let env = Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() })
            .expect("the built-in files load");
        let mut harness = Harness::with_env(Host { state }, env, 120, 44);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness.send(Msg::Loaded(Listing { profiles: vec![self.profile.clone()], diagnostics: Vec::new() }));
        let present = Status {
            name: self.profile.name.clone(),
            image: Readiness::Present,
            revision: Revision::Current,
            identity: Readiness::Missing,
        };
        harness.send(Msg::Probed(vec![present])).render();
        harness
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The screen's words with every run of blanks and frame made one.
fn words(harness: &Harness<Host>) -> String {
    harness.screen().split_whitespace().filter(|word| *word != "▌" && *word != "│").collect::<Vec<_>>().join(" ")
}

/// Moves the clock and draws until the screen says `text`, over a generous but finite while.
fn until(harness: &mut Harness<Host>, text: &str) {
    let started = Instant::now();
    loop {
        harness.advance(Duration::from_millis(100));
        if words(harness).contains(text) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "never said {text:?}:\n{}", harness.screen());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Opens the shell from the list, becomes the administrator in it, and closes it.
fn work_in_the_shell(harness: &mut Harness<Host>) {
    harness.click_text("Open shell").render();
    until(harness, "Your shell");
    assert!(words(harness).contains("none of your files"), "{}", harness.screen());
    harness.click_text("As administrator").render();
    until(harness, "You are root in this container");
    harness.click_text("Close the shell").render();
    until(harness, "Add what you did to");
}

#[test]
fn what_is_done_in_a_profiles_shell_becomes_its_image_and_is_kept_for_rebuilds_when_added() {
    let stand = Stand::new("add");
    let container = format!("qcode-shell.{}", stand.profile.name);
    let mut harness = stand.screen();
    work_in_the_shell(&mut harness);

    let question = words(&harness);
    assert!(question.contains("In the home: 1 changed. In the system: 2 changed."), "{question}");
    assert!(question.contains("apt-get install -y jq"), "{question}");
    assert!(!question.contains("exit"), "leaving the shell is no step: {question}");
    let calls = stand.calls();
    let created = calls.iter().find(|call| call.starts_with("create")).expect("a container was made");
    assert!(created.contains(&format!("--name {container}")), "{created}");
    assert!(created.contains(&format!("qcode/profile/{}", stand.profile.name)), "from the profile's image: {created}");
    assert!(!created.contains("--volume") && !created.contains("--network=none"), "no mount, the network: {created}");
    assert!(
        calls.iter().any(|call| call.starts_with(&format!("exec --user 0 --interactive --tty --env HISTFILE=/tmp/qcode-shell-root-history --env PROMPT_COMMAND=history -a {container} bash"))),
        "the administrator is root through the engine: {calls:#?}"
    );
    assert!(!calls.iter().any(|call| call.starts_with("commit")), "nothing is added before the answer: {calls:#?}");

    harness.click_text("Add to the profile").render();
    until(&mut harness, &format!("Added to {}", stand.profile.name));
    let calls = stand.calls();
    let commit = calls.iter().position(|call| call.starts_with("commit")).expect("the shell was committed");
    assert_eq!(calls[commit], format!("commit --change USER qcode {container} qcode/profile/{}", stand.profile.name));
    // The login is out of the home before the image is made.
    let forgotten = calls.iter().position(|call| call.contains(".claude/.credentials.json")).expect("the login goes");
    assert!(forgotten < commit, "{calls:#?}");
    let profiles = stand.folder.join("Profiles");
    let (own, problems) = crate::profile::own::Own::load(&profiles, &stand.profile.name);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(own.steps, ["apt-get update", "apt-get install -y jq"]);
    assert_eq!(own.home, ["notes.txt"]);
    assert!(crate::profile::own::Own::archive(&profiles, &stand.profile.name).is_file(), "the home's archive is kept");
    assert!(harness.app().state.shell().is_none(), "the page is closed");

    // The list shows the steps, and a rebuild runs them after the recipe.
    until(&mut harness, "Your own steps (2)");
    harness.click_text("Rebuild image").advance(Duration::from_millis(400));
    assert!(
        // The question stands over the list, whose words run on beside its lines.
        words(&harness).contains("Your 2") && words(&harness).contains("steps from the profile's shell are run again"),
        "{}",
        harness.screen()
    );
    harness.click_text("Build it again").render();
    let started = Instant::now();
    while !stand.folder.join("built").exists() {
        harness.advance(Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(30), "no build: {:#?}", stand.calls());
        std::thread::sleep(Duration::from_millis(20));
    }
    let built = std::fs::read_to_string(stand.folder.join("built")).expect("the built file");
    let label = built.find("LABEL qcode.profile.revision").expect("the recipe's label");
    let step = built.find("RUN apt-get install -y jq").expect("the step is built");
    assert!(label < step, "after the recipe: {built}");
    assert!(built.contains("COPY own-home.tar"), "{built}");
    assert!(stand.folder.join("built-archive").is_file(), "the archive is in the build's context");
}

#[test]
fn a_shell_whose_work_is_discarded_leaves_the_image_and_the_store_as_they_were() {
    let stand = Stand::new("discard");
    let container = format!("qcode-shell.{}", stand.profile.name);
    let mut harness = stand.screen();
    work_in_the_shell(&mut harness);
    harness.click_text("Discard").render();
    until(&mut harness, "nothing was added");
    let calls = stand.calls();
    assert!(!calls.iter().any(|call| call.starts_with("commit")), "{calls:#?}");
    assert_eq!(calls.last(), Some(&format!("rm --force {container}")), "{calls:#?}");
    assert!(!stand.folder.join("Profiles").join(stand.profile.name.as_str()).exists(), "nothing is kept");
}

#[test]
fn a_removed_own_step_is_off_the_list_and_off_the_next_rebuild() {
    let stand = Stand::new("trim");
    let profiles = stand.folder.join("Profiles");
    let own = crate::profile::own::Own {
        steps: vec!["apt-get install -y jq".to_owned(), "apt-get install -y sl".to_owned()],
        home: Vec::new(),
    };
    own.save(&profiles, &stand.profile.name).expect("saved");
    let mut harness = stand.screen();
    until(&mut harness, "Your own steps (2)");
    assert!(words(&harness).contains("apt-get install -y jq"), "{}", harness.screen());
    // The first step's button.
    harness.click_text("Remove").render();
    until(&mut harness, "Your own step ");
    assert_eq!(crate::profile::own::Own::load(&profiles, &stand.profile.name).0.steps, ["apt-get install -y sl"]);
}

/// A stand-in with a shell of the profile left behind by a QCode that ended while it was open,
/// which did what `diff` and the record say; the list is on screen and "Open shell" is pressed.
fn left_open(stand: &Stand) -> Harness<Host> {
    std::fs::write(stand.folder.join("leftover"), "").expect("a shell was left open");
    let mut harness = stand.screen();
    harness.click_text("Open shell").render();
    harness
}

#[test]
fn a_shell_left_open_with_work_in_it_is_not_removed_and_the_person_continues_in_it() {
    let stand = Stand::new("left-continue");
    let container = format!("qcode-shell.{}", stand.profile.name);
    let mut harness = left_open(&stand);
    until(&mut harness, "was still open when QCode last closed");
    let question = words(&harness);
    assert!(question.contains("In the home: 1 changed. In the system: 2 changed."), "{question}");
    assert!(question.contains("apt-get install -y jq"), "{question}");
    let calls = stand.calls();
    assert!(calls.contains(&format!("start {container}")), "started to read its record: {calls:#?}");
    assert!(!calls.iter().any(|call| call.starts_with("rm ")), "nothing is removed before the answer: {calls:#?}");

    harness.click_text("Continue in it").render();
    until(&mut harness, "Your shell");
    let calls = stand.calls();
    assert!(
        calls.iter().any(|call| call.starts_with("exec --interactive --tty") && call.contains(&container)),
        "the person's terminal is in the container that was left: {calls:#?}"
    );
    assert!(!calls.iter().any(|call| call.starts_with("rm ") || call.starts_with("create")), "{calls:#?}");
}

#[test]
fn a_shell_left_open_with_work_in_it_is_added_to_the_profile_when_the_person_says_so() {
    let stand = Stand::new("left-add");
    let container = format!("qcode-shell.{}", stand.profile.name);
    let mut harness = left_open(&stand);
    until(&mut harness, "was still open when QCode last closed");
    harness.click_text("Add it to the profile").render();
    until(&mut harness, &format!("Added to {}", stand.profile.name));
    let calls = stand.calls();
    let commit = calls.iter().position(|call| call.starts_with("commit")).expect("the shell was committed");
    assert_eq!(calls[commit], format!("commit --change USER qcode {container} qcode/profile/{}", stand.profile.name));
    assert!(!calls[..commit].iter().any(|call| call.starts_with("rm ")), "{calls:#?}");
    assert!(!calls.iter().any(|call| call.starts_with("create")), "{calls:#?}");
    let profiles = stand.folder.join("Profiles");
    assert_eq!(
        crate::profile::own::Own::load(&profiles, &stand.profile.name).0.steps,
        ["apt-get update", "apt-get install -y jq"]
    );
}

#[test]
fn a_shell_left_open_with_work_in_it_goes_only_when_the_person_discards_it() {
    let stand = Stand::new("left-discard");
    let container = format!("qcode-shell.{}", stand.profile.name);
    let mut harness = left_open(&stand);
    until(&mut harness, "was still open when QCode last closed");
    harness.click_text("Discard it").render();
    until(&mut harness, "nothing was added");
    let calls = stand.calls();
    assert!(!calls.iter().any(|call| call.starts_with("commit") || call.starts_with("create")), "{calls:#?}");
    assert_eq!(calls.last(), Some(&format!("rm --force {container}")), "{calls:#?}");
    assert!(harness.app().state.shell().is_none(), "the page is closed");
}

#[test]
fn a_shell_left_open_that_changed_nothing_is_removed_without_a_word_and_a_new_one_opens() {
    let stand = Stand::new("left-empty");
    std::fs::write(stand.folder.join("diff"), "").expect("nothing changed");
    std::fs::write(stand.folder.join("history"), "").expect("nothing was run");
    let container = format!("qcode-shell.{}", stand.profile.name);
    let mut harness = left_open(&stand);
    until(&mut harness, "Your shell");
    assert!(!words(&harness).contains("was still open"), "{}", harness.screen());
    let calls = stand.calls();
    let removed = calls.iter().position(|call| *call == format!("rm --force {container}"));
    let created = calls.iter().position(|call| call.starts_with("create"));
    let (Some(removed), Some(created)) = (removed, created) else { panic!("removed, then made anew: {calls:#?}") };
    assert!(removed < created, "{calls:#?}");
}
