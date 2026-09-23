//! A tab opened after its profile's image was built again gets a container made from the new
//! image, not the stopped one made from the old: the plan names the image, and a rebuild keeps
//! the name while the image under it changes. The old image is then let go, never by force.
//!
//! The engine is a script that writes down every call and has a stopped container of the
//! profile made from the very plan the tab asks for, so the only thing that differs is the image.

use super::*;

use crate::store::WorkspaceId;
use crate::ui::workspace::plan::ContainerPlan;

/// A stand-in podman holding a stopped container of `claude-sub` in `firefly`, made from the
/// image `sha-old` and labelled with the plan a tab of it asks for; the profile's image is
/// whatever `image` says.
struct Stopped {
    scratch: Scratch,
    binary: PathBuf,
    calls: PathBuf,
}

impl Stopped {
    fn new(name: &str, image: &str) -> Self {
        let scratch = Scratch::new(name);
        let (binary, calls) = (scratch.0.join("engine"), scratch.0.join("calls"));
        let user = HostUser::Ids { uid: 1000, gid: 1000 };
        let plan = ContainerPlan::profile(
            &WorkspaceId::parse("firefly").expect("a workspace id"),
            &scratch.paths(),
            &profile("claude-sub", HarnessKind::ClaudeCode),
        );
        let digest = plan.digest(&Engine::new(EngineKind::Podman, &binary), user);
        let script = r#"#!/bin/sh
printf '%s\n' "$*" >> FOLDER/calls
case "$1 $2 $4" in
'container inspect {{.State.Status}}') [ -e FOLDER/started ] && echo running || echo exited; exit 0 ;;
'container inspect {{.Image}}') echo sha-old; exit 0 ;;
'container inspect '*) echo DIGEST; exit 0 ;;
'image inspect {{.Id}}') echo IMAGE; exit 0 ;;
esac
[ "$1" = start ] && touch FOLDER/started
exit 0
"#
        .replace("FOLDER", &scratch.0.display().to_string())
        .replace("DIGEST", &digest)
        .replace("IMAGE", image);
        fs::write(&binary, script).expect("the stand-in engine is written");
        fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
        Self { scratch, binary, calls }
    }

    fn screen(&self) -> WorkspaceScreen {
        let workspaces = vec![workspace(
            "firefly",
            "Firefly",
            self.scratch.paths(),
            vec![profile("claude-sub", HarnessKind::ClaudeCode)],
        )];
        WorkspaceScreen::new(
            Some(Engine::new(EngineKind::Podman, &self.binary)),
            HostUser::Ids { uid: 1000, gid: 1000 },
            workspaces,
        )
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }
}

/// Opens a new chat of the workspace's profile the way the person does: the `+`, then its row.
fn open_chat(harness: &mut Harness<Screen>) {
    harness.click_text("New tab").render();
    harness.click_text("New chat").render();
}

#[test]
fn a_tab_opened_after_a_rebuild_is_made_from_the_new_image_and_the_old_one_is_let_go() {
    let engine = Stopped::new("rebuilt", "sha-new");
    let mut harness = harness(engine.screen(), 120, 36);
    open_chat(&mut harness);
    let calls = engine.calls();
    let removed = calls.iter().position(|call| call == "rm --force qcode-firefly-claude-sub");
    let created = calls.iter().position(|call| call.starts_with("create --name qcode-firefly-claude-sub"));
    let (Some(removed), Some(created)) = (removed, created) else {
        panic!("the old container goes and a new one is made: {calls:#?}")
    };
    assert!(removed < created, "{calls:#?}");
    assert!(calls.contains(&"image rm sha-old".to_owned()), "the old image is let go: {calls:#?}");
}

#[test]
fn a_tab_whose_image_is_the_same_starts_the_container_it_had() {
    let engine = Stopped::new("unchanged", "sha-old");
    let mut harness = harness(engine.screen(), 120, 36);
    open_chat(&mut harness);
    let calls = engine.calls();
    assert!(calls.contains(&"start qcode-firefly-claude-sub".to_owned()), "{calls:#?}");
    assert!(!calls.iter().any(|call| call.starts_with("rm ")), "the container is kept: {calls:#?}");
    assert!(!calls.iter().any(|call| call.starts_with("create")), "{calls:#?}");
    assert!(!calls.iter().any(|call| call.starts_with("image rm")), "{calls:#?}");
}
