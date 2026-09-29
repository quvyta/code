//! The packages opencode fetches for its configuration folders, fetched while the image is built
//! rather than when a tab first starts.
//!
//! opencode (measured in 1.18.33) installs its own plugin package, `@opencode-ai/plugin` at its own
//! version, with npm into every configuration folder it reads that it may write: `~/.config/opencode`
//! and a `.opencode` of the workspace. A folder is left alone once it has a `node_modules` and its
//! `package-lock.json` names every package its `package.json` does. Before it loads any plugin it
//! waits for those installs, and QCode's shared server always loads one, the bridge's. Without the
//! network npm tries, waits and tries again: a profile without the network drew its first tab's
//! prompt only after 70–90 seconds.
//!
//! So the image runs opencode once, with the network, the way the server runs it: with a plugin in
//! `OPENCODE_CONFIG_CONTENT`, which is what makes it wait for the install (without one `debug
//! config` ends first and the install with it), in a folder of its own with its database, logs and
//! caches beside it, all removed again. The version is whichever opencode the image holds, so a
//! newer opencode fetches its own. What stays is the configuration folder's packages, about 62 MB.
//!
//! Two folders the image cannot fill are left, and npm's cache of the same packages is kept in the
//! image for them (about 94 MB, outside the home), where npm falls back on it without the network,
//! so their install takes seconds instead of the wait: the workspace's `.opencode`, where QCode
//! extra and Quvyta development have graphify's plugin written when a container comes up; and the
//! home of a workspace made before this step, since a home volume is filled from the image only
//! when it is first made.

use crate::profile::{HarnessKind, Profile, Template};

/// The package opencode installs into each of its configuration folders, and what the step asks
/// for afterwards so that an opencode that stopped doing it fails the build instead.
const PLUGIN_PACKAGE: &str = "@opencode-ai/plugin";

/// Whether `profile`'s tabs run on opencode's shared server, which loads the bridge's plugin and
/// so waits for the configuration folders' packages. The harness as it comes runs opencode on its
/// own, which loads no plugin QCode gave it.
fn loads_a_plugin(profile: &Profile) -> bool {
    profile.harness == HarnessKind::OpenCode && profile.template != Template::Base
}

/// The build step that has opencode fetch its configuration folder's packages into the image's
/// home, for a profile whose tabs wait for them; `None` for every other profile.
///
/// It comes after every file the profile writes into the home and before the step that opens the
/// home to whoever runs the container.
#[must_use]
pub fn step(profile: &Profile) -> Option<String> {
    if !loads_a_plugin(profile) {
        return None;
    }
    // npm's cache is the image's own, where the installs at run time look: what npm wrote into it
    // is opened to everyone, since the container may run as another user than the build; the
    // folder itself is the base image's, and open already.
    Some(format!(
        "RUN {{ scratch=\"$(mktemp -d)\" \\\n \
         && cd \"$scratch\" \\\n \
         && echo 'export const Wait = async () => ({{}});' > wait.js \\\n \
         && XDG_DATA_HOME=\"$scratch/data\" XDG_CACHE_HOME=\"$scratch/cache\" XDG_STATE_HOME=\"$scratch/state\" \
         OPENCODE_CONFIG_CONTENT=\"{{\\\"plugin\\\":[\\\"file://$scratch/wait.js\\\"]}}\" opencode debug config > /dev/null \\\n \
         && cd / && rm -rf \"$scratch\" \\\n \
         && chmod -R a+rwX \"${{NPM_CONFIG_CACHE:-/var/cache/npm}}/_cacache\" \\\n \
         && test -f \"$HOME/.config/opencode/node_modules/{PLUGIN_PACKAGE}/package.json\"; }} \\\n \
         || {{ echo 'opencode could not fetch {PLUGIN_PACKAGE} into its settings folder; the build needs the network.' >&2; exit 1; }}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AccountKind, MountAccess, NetworkMode, SafeName};

    fn profile(harness: HarnessKind, template: Template) -> Profile {
        Profile {
            name: SafeName::parse("p").expect("a safe name"),
            harness,
            template,
            account: AccountKind::Free,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::None,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        }
    }

    #[test]
    fn only_an_opencode_profile_on_the_shared_server_fetches_its_packages_in_the_image() {
        for harness in HarnessKind::ALL {
            for template in Template::ALL {
                let wanted = harness == HarnessKind::OpenCode && template != Template::Base;
                let recipe = crate::ui::profiles::recipe::image(&profile(harness, template)).containerfile;
                assert_eq!(recipe.contains("opencode debug config"), wanted, "{harness:?} {template:?}: {recipe}");
            }
        }
    }

    #[test]
    fn the_packages_are_fetched_after_the_homes_files_and_before_the_home_is_opened() {
        for template in [Template::Recommended, Template::High, Template::QuvytaDev, Template::Slim] {
            let recipe = crate::ui::profiles::recipe::image(&profile(HarnessKind::OpenCode, template)).containerfile;
            let fetched = recipe.find("opencode debug config").expect("the step is there");
            let opened = recipe.rfind(crate::base::paths::OPEN_HOME).expect("the home is opened");
            assert!(fetched < opened, "{template:?}: {recipe}");
            for written in recipe.match_indices("cp '/qcode-template/").map(|(at, _)| at) {
                assert!(written < fetched, "{template:?}: a file after the step: {recipe}");
            }
            if let Some(graphify) = recipe.find("graphify opencode install") {
                assert!(graphify < fetched, "{template:?}: graphify's plugin after the step: {recipe}");
            }
        }
    }

    #[test]
    fn npms_cache_stays_in_the_image_where_the_installs_at_run_time_look_and_is_opened() {
        for template in [Template::Recommended, Template::Slim, Template::Custom, Template::High, Template::QuvytaDev] {
            let step = step(&profile(HarnessKind::OpenCode, template)).expect("a step");
            assert!(!step.contains("NPM_CONFIG_CACHE="), "{template:?}: a cache of the step's own: {step}");
            assert!(step.contains("chmod -R a+rwX \"${NPM_CONFIG_CACHE:-/var/cache/npm}/_cacache\""), "{step}");
        }
    }

    /// Runs the step's shell with a stand-in `opencode` that writes what `write` says, and answers
    /// whether it passed, what it said, and what the stand-in was given.
    fn run_with(write: &str) -> (bool, String, String) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let root = std::env::temp_dir().join(format!("qcode-opencode-packages-{}-{stamp}", std::process::id()));
        let bin = root.join("bin");
        let home = root.join("home");
        std::fs::create_dir_all(&bin).expect("a folder");
        std::fs::create_dir_all(&home).expect("a home");
        let stand_in = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" \"$OPENCODE_CONFIG_CONTENT\" \"$PWD\" > '{seen}'\n{write}\n",
            seen = root.join("seen").display()
        );
        std::fs::write(bin.join("opencode"), stand_in).expect("the stand-in");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("opencode"), std::fs::Permissions::from_mode(0o755)).expect("runnable");
        }
        let step = step(&profile(HarnessKind::OpenCode, Template::Recommended)).expect("a step");
        let script = step.strip_prefix("RUN ").expect("a RUN step");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        std::fs::create_dir_all(root.join("npm").join("_cacache")).expect("npm's cache");
        let ran = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", &home)
            .env("NPM_CONFIG_CACHE", root.join("npm"))
            .env("PATH", path)
            .output()
            .expect("a shell runs the step");
        let seen = std::fs::read_to_string(root.join("seen")).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&root);
        (ran.status.success(), String::from_utf8_lossy(&ran.stderr).into_owned(), seen)
    }

    #[test]
    fn the_step_passes_only_when_opencode_left_its_package_and_says_why_when_not() {
        let installed = "mkdir -p \"$HOME/.config/opencode/node_modules/@opencode-ai/plugin\" \
                         && echo '{}' > \"$HOME/.config/opencode/node_modules/@opencode-ai/plugin/package.json\"";
        let (passed, said, seen) = run_with(installed);
        assert!(passed, "{said}");
        let lines: Vec<&str> = seen.lines().collect();
        assert_eq!(lines.first().copied(), Some("debug config"), "{seen}");
        // The plugin that makes opencode wait, given the way the server is given the bridge's.
        let config: serde_json::Value = serde_json::from_str(lines.get(1).copied().unwrap_or_default()).expect("JSON");
        let plugin = config["plugin"][0].as_str().unwrap_or_default().to_owned();
        let folder = lines.get(2).copied().unwrap_or_default();
        assert_eq!(plugin, format!("file://{folder}/wait.js"), "{seen}");

        // An opencode that fetched nothing, as without the network: the build stops and says why.
        let (passed, said, _) = run_with("true");
        assert!(!passed);
        assert!(said.contains("could not fetch @opencode-ai/plugin") && said.contains("needs the network"), "{said}");
    }
}
