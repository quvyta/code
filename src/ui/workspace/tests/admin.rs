//! "As administrator" in the containers panel: it opens a tab that is root in the profile's own
//! container, through the engine, recording its commands in the workspace's home; and a tab like
//! that is never brought back by opening QCode again.

use super::*;

/// The screen with the profile's container listed in `state`.
fn listed(scratch: &Scratch, state: ContainerState) -> Harness<Screen> {
    let mut screen = one_workspace(scratch);
    let container = Container { name: "qcode-firefly-claude-sub".to_owned(), state };
    apply(&mut screen, Msg::ContainersRead("firefly".to_owned(), Ok(vec![container])));
    harness(screen, 120, 36)
}

#[test]
fn as_administrator_opens_a_root_shell_in_the_profiles_container_that_records_its_commands() {
    let scratch = Scratch::new("admin");
    let mut harness = listed(&scratch, ContainerState::Running);
    harness.click_text("As administrator").render();
    let tab = harness.app().0.workspace().and_then(OpenWorkspace::active_tab).expect("a tab is open");
    assert_eq!(tab.kind(), &TabKind::Admin("claude-sub".to_owned()));
    let command = harness.app().0.launch_command(tab.key()).expect("the tab has a command");
    let args: Vec<String> = command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert_eq!(
        args,
        [
            "exec",
            "--user",
            "0",
            "--interactive",
            "--tty",
            "--env",
            "HISTFILE=/home/qcode/.qcode-admin-history",
            "--env",
            "PROMPT_COMMAND=history -a",
            "qcode-firefly-claude-sub",
            "bash"
        ]
    );
    assert!(harness.screen().contains("claude-sub · administrator"), "{}", harness.screen());
    let open = harness.app().0.workspace().map_or(0, |workspace| workspace.tabs().len());
    let session = harness.app().0.session();
    let kept: usize = session.workspaces.iter().map(|workspace| workspace.tabs.len()).sum();
    assert_eq!(kept, open - 1, "root never comes back with the session: {session:?}");
}

#[test]
fn a_stopped_container_offers_no_administrator() {
    let scratch = Scratch::new("admin-stopped");
    let mut harness = listed(&scratch, ContainerState::Exited);
    let before = harness.app().0.workspace().map_or(0, |workspace| workspace.tabs().len());
    harness.click_text("As administrator").render();
    let after = harness.app().0.workspace().map_or(0, |workspace| workspace.tabs().len());
    assert_eq!(before, after, "{}", harness.screen());
}

/// A stand-in podman holding a stopped container of `claude-sub` in `firefly` made from an
/// earlier plan, so opening a tab makes it again. The container was made from the image
/// `made_from`, the profile's image is now `profile`, and `diff` answers `diff`. A workspace's own
/// image exists once a file `own` holds the profile's image it was made on, which is what a
/// `commit` writes there; a run of the recorded commands fails while a file `fails` exists.
struct Kept {
    scratch: Scratch,
    binary: PathBuf,
}

impl Kept {
    fn new(name: &str, made_from: &str, profile: &str, diff: &str) -> Self {
        let scratch = Scratch::new(name);
        let binary = scratch.0.join("engine");
        fs::write(scratch.0.join("diff"), diff).expect("the diff");
        let volume = crate::profile::identity::Home::new(
            SafeName::parse("claude-sub").expect("safe"),
            crate::store::WorkspaceId::parse("firefly").expect("a workspace id"),
        )
        .volume();
        let script = r#"#!/bin/sh
printf '%s\n' "$*" >> FOLDER/calls
case "$1 $2 $4" in
'container inspect {{.State.Status}}') [ -e FOLDER/started ] && echo running || echo exited; exit 0 ;;
'container inspect {{.Image}}') echo MADE; exit 0 ;;
'container inspect '*) echo an-earlier-plan; exit 0 ;;
'image inspect {{.Id}}')
  case "$5" in
  qcode/workspace/*) [ -e FOLDER/own ] || { echo 'Error: no such image' >&2; exit 1; }; echo sha-own ;;
  *) echo PROFILE ;;
  esac
  exit 0 ;;
'image inspect '*) cat FOLDER/own 2>/dev/null; exit 0 ;;
esac
case "$1" in
volume) echo VOLUME ;;
diff) cat FOLDER/diff ;;
commit) for word do case "$word" in 'LABEL qcode.workspace.base='*) printf '%s' "${word#*=}" > FOLDER/own ;; esac; done ;;
exec) case "$*" in *.replay*) [ -e FOLDER/fails ] && { printf 'apt-get install -y gone'; exit 1; } ;; esac ;;
start) case "$2" in *.replay) ;; *) touch FOLDER/started ;; esac ;;
esac
exit 0
"#
        .replace("FOLDER", &scratch.0.display().to_string())
        .replace("MADE", made_from)
        .replace("PROFILE", profile)
        .replace("VOLUME", &volume);
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        Self { scratch, binary }
    }

    /// The workspace already has an image of its own, made on the profile's image `base`.
    fn owning(self, base: &str) -> Self {
        fs::write(self.scratch.0.join("own"), base).expect("the workspace's image");
        self
    }

    fn harness(&self) -> Harness<Screen> {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        )];
        let screen = WorkspaceScreen::new(
            Some(Engine::new(EngineKind::Podman, &self.binary)),
            HostUser::Ids { uid: 1000, gid: 1000 },
            workspaces,
        );
        let mut harness = harness(screen, 120, 36);
        // Motion off, so a toast the start sends is on screen as soon as it is sent.
        harness.set_reduced_motion(true);
        harness
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.scratch.0.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }
}

/// Opens a new chat of the workspace's profile the way the person does: the `+`, then its row.
fn open_chat(harness: &mut Harness<Screen>) {
    harness.click_text("New tab").render();
    harness.click_text("New chat").advance(Duration::from_millis(400)).render();
}

/// Where the first call starting with `start` is among `calls`.
fn at(calls: &[String], start: &str) -> Option<usize> {
    calls.iter().position(|call| call.starts_with(start))
}

/// What a person who installed jq as the administrator changed, with the home and the scratch
/// folders, which are not the system's.
const INSTALLED: &str = "C /usr\nC /usr/bin\nA /usr/bin/jq\nC /home/qcode\nA /home/qcode/notes.txt\nC /tmp\nA /tmp/x\n";

#[test]
fn what_the_administrator_installed_is_kept_in_the_workspaces_image_before_the_container_is_made_again() {
    let engine = Kept::new("admin-keep", "sha-profile", "sha-profile", INSTALLED);
    let mut harness = engine.harness();
    open_chat(&mut harness);
    let calls = engine.calls();
    let committed = at(&calls, "commit ");
    let removed = at(&calls, "rm --force qcode-firefly-claude-sub");
    let created = at(&calls, "create --name qcode-firefly-claude-sub");
    let (Some(committed), Some(removed), Some(created)) = (committed, removed, created) else {
        panic!("kept, removed and made again: {calls:#?}")
    };
    assert!(committed < removed && removed < created, "{calls:#?}");
    assert_eq!(
        calls[committed],
        "commit --change USER qcode --change LABEL qcode.workspace.base=sha-profile qcode-firefly-claude-sub \
         qcode/workspace/firefly/claude-sub"
    );
    assert!(
        calls[created].contains(" qcode/workspace/firefly/claude-sub "),
        "made from the workspace's image: {calls:#?}"
    );
    assert!(!calls.iter().any(|call| call.contains(".replay")), "nothing to run again: {calls:#?}");
}

#[test]
fn a_container_that_changed_only_its_home_and_scratch_is_made_again_from_the_profiles_image() {
    let engine = Kept::new(
        "admin-home",
        "sha-profile",
        "sha-profile",
        "C /home/qcode\nA /home/qcode/notes.txt\nC /tmp\nA /tmp/x\n",
    );
    let mut harness = engine.harness();
    open_chat(&mut harness);
    let calls = engine.calls();
    assert!(at(&calls, "commit ").is_none(), "nothing of the system to keep: {calls:#?}");
    let created = at(&calls, "create --name qcode-firefly-claude-sub").map(|index| calls[index].clone());
    let created = created.unwrap_or_else(|| panic!("made again: {calls:#?}"));
    assert!(!created.contains("qcode/workspace/"), "{created}");
}

#[test]
fn after_a_rebuild_the_recorded_commands_run_again_on_the_new_image_and_the_workspace_moves_onto_it() {
    let engine = Kept::new("admin-replay", "sha-own", "sha-new", "").owning("sha-old");
    let mut harness = engine.harness();
    open_chat(&mut harness);
    let calls = engine.calls();
    let replayed = calls.iter().position(|call| {
        call.starts_with("exec --user 0 qcode-firefly-claude-sub.replay") && call.contains(".qcode-admin-history")
    });
    let committed = calls.iter().position(|call| {
        call == "commit --change USER qcode --change LABEL qcode.workspace.base=sha-new \
                 qcode-firefly-claude-sub.replay qcode/workspace/firefly/claude-sub"
    });
    let created = at(&calls, "create --name qcode-firefly-claude-sub ");
    let (Some(replayed), Some(committed), Some(created)) = (replayed, committed, created) else {
        panic!("run again, kept and made from it: {calls:#?}")
    };
    assert!(replayed < committed && committed < created, "{calls:#?}");
    assert!(calls[created].contains(" qcode/workspace/firefly/claude-sub "), "{calls:#?}");
    assert!(calls.contains(&"image rm sha-old".to_owned()), "the profile's old image is let go: {calls:#?}");
    assert!(!harness.screen().contains("earlier image"), "{}", harness.screen());
}

#[test]
fn a_command_that_cannot_be_run_again_on_a_rebuilt_image_is_named_to_the_person() {
    let engine = Kept::new("admin-behind", "sha-own", "sha-new", "").owning("sha-old");
    fs::write(engine.scratch.0.join("fails"), "").expect("the command is told to fail");
    let mut harness = engine.harness();
    open_chat(&mut harness);
    let calls = engine.calls();
    assert!(!calls.iter().any(|call| call.starts_with("commit ")), "nothing is kept of the failed run: {calls:#?}");
    let created = at(&calls, "create --name qcode-firefly-claude-sub ").map(|index| calls[index].clone());
    let created = created.unwrap_or_else(|| panic!("made all the same: {calls:#?}"));
    assert!(created.contains(" qcode/workspace/firefly/claude-sub "), "on the image it had: {created}");
    let screen = harness.screen().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(screen.contains("claude-sub stays on its earlier image"), "{screen}");
    assert!(screen.contains("apt-get install -y gone"), "{screen}");
}
