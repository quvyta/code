//! What the application's own tests build an application out of, so that none of them needs
//! the person's settings file, a container engine or a store of their own.

use std::path::{Path, PathBuf};

use qframe::env::{AssetDirs, Env};
use qframe::icons::GlyphMode;
use qframe::runtime::Harness;

use super::{Config, Engine, EngineKind, Gates, HostDirs, InstallHost, QCode, SetupStep};
use crate::store::Platform;
use crate::ui::setup::gates::{EngineCheck, LocationCheck};

/// A folder of this machine's temporary directory, named after `what` and this process.
pub fn scratch(what: &str) -> PathBuf {
    std::env::temp_dir().join(format!("qcode-app-{what}-{}", std::process::id()))
}

/// A machine whose Documents folder is in a temporary home folder.
pub fn dirs() -> HostDirs {
    let documents = scratch("home").join("Documents");
    HostDirs { store: Some(documents.join("Quvyta").join("Code")), documents: Some(documents) }
}

/// An Arch machine with `paru`, so every install command a test sees is the same one.
pub fn host() -> InstallHost {
    InstallHost::read(Platform::Linux, Some("ID=arch\n"), |tool| tool == "paru")
}

/// Gates that all hold, which is a machine with everything in place.
pub fn settled() -> Gates {
    Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Usable }
}

/// A settings file of a finished setup, with `recent` as the workspaces opened before.
pub fn config(store: &Path, recent: &[&str]) -> Config {
    let list = recent.iter().map(|id| format!("\"{id}\"")).collect::<Vec<_>>().join(", ");
    let text = format!(
        "language = \"en\"\n\n[setup]\ncompleted = true\nstep = \"location\"\n\n[engine]\nkind = \"podman\"\n\n\
         [folder]\npath = \"{}\"\n\n[workspaces]\nrecent = [{list}]\n",
        store.display()
    );
    Config::parse_str("code.conf", &text)
}

/// An application over `config`, opening on the wizard's `entry` step or on the home screen.
///
/// Its providers screen reads a file of this test's own and asks through a web that reaches
/// nothing, so no test can read the person's data folder or leave the machine.
pub fn app(config: Config, gates: &Gates, entry: Option<SetupStep>) -> QCode {
    QCode::new(config, dirs(), host(), gates, None, None, entry).with_providers(Some(providers_file()), no_web())
}

/// A providers file of this process's own, never the person's.
pub fn providers_file() -> PathBuf {
    scratch("providers").join("providers.toml")
}

/// A web that reaches nothing at all: a test that sends a request by accident is told so
/// rather than quietly going out over the network.
pub fn no_web() -> crate::provider::Web {
    crate::provider::Web::new(|ask| {
        Err(crate::provider::AskError::Unreachable {
            url: ask.url.clone(),
            reason: "no test reaches the network".to_owned(),
        })
    })
}

/// An application on its home screen holding an engine whose binary is not there: every
/// piece of engine work is really run and really fails, with the machine's own words, so a
/// test sees that the work was reached without a container runtime on the machine.
pub fn app_with_absent_engine(config: Config) -> QCode {
    let engine = Engine::new(EngineKind::Podman, "/qcode/no/such/engine");
    QCode::new(config, dirs(), host(), &settled(), Some(engine), None, None)
        .with_providers(Some(providers_file()), no_web())
}

/// An application on its home screen holding an engine the test wrote, which is how a test
/// watches what QCode asks of one: the binary is a script of the test's own and every call it is
/// given is written down where the test can read it.
pub fn app_with_engine(config: Config, engine: Engine) -> QCode {
    QCode::new(config, dirs(), host(), &settled(), Some(engine), None, None)
        .with_providers(Some(providers_file()), no_web())
}

/// Runs the test `name` of this crate again in a process of its own, with none of the
/// person's XDG folders and with `vars` set, and answers what it printed.
///
/// The framework reads where things live from the environment, and a test process shares
/// one environment between all its threads; a child with its own is how a test sees where
/// QCode puts things on a machine it describes, without touching the person's folders.
pub fn in_child(name: &str, vars: &[(&str, &std::ffi::OsStr)]) -> String {
    let output = child_command(name, vars).output().expect("the test binary runs");
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{printed}{}", String::from_utf8_lossy(&output.stderr));
    printed
}

/// [`in_child`] started and left running, for a test that acts on the child while it lives:
/// a lock it holds is released only when it exits.
pub fn spawn_child(name: &str, vars: &[(&str, &std::ffi::OsStr)]) -> std::process::Child {
    child_command(name, vars)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the test binary runs")
}

/// Which of `rows` is the one lit as the keyboard's row: the only one whose first letter
/// stands on a background none of the others has. A row off the screen is never it. Two rows
/// alone cannot say which of them is the odd one, so a test names at least three.
///
/// Read from the painted cells rather than from a selection kept in the screen's state, so a
/// test says what a person sees, not what it set.
pub fn highlighted<'a, A: qframe::runtime::App>(harness: &Harness<A>, rows: &[&'a str]) -> Option<&'a str> {
    assert!(rows.len() >= 3, "two rows cannot tell which is lit: {rows:?}");
    let ground: Vec<_> = rows
        .iter()
        .map(|row| {
            harness.find(row).and_then(|(x, y)| {
                let (x, y) = (u16::try_from(x).ok()?, u16::try_from(y).ok()?);
                harness.bg(x, y)
            })
        })
        .collect();
    let lone: Vec<usize> = (0..rows.len())
        .filter(|&at| {
            ground[at].is_some() && ground.iter().enumerate().all(|(other, bg)| other == at || *bg != ground[at])
        })
        .collect();
    match lone.as_slice() {
        [at] => Some(rows[*at]),
        _ => None,
    }
}

/// The test binary, set to run only the test `name`, as [`in_child`] describes.
fn child_command(name: &str, vars: &[(&str, &std::ffi::OsStr)]) -> std::process::Command {
    let exe = std::env::current_exe().expect("the test binary");
    let mut command = std::process::Command::new(exe);
    command.args([name, "--exact", "--nocapture", "--test-threads=1"]);
    for var in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_DOCUMENTS_DIR"] {
        command.env_remove(var);
    }
    command.envs(vars.iter().copied());
    command
}

/// QCode's own text and keys, and nothing of the person's.
pub fn env() -> Env {
    let dirs =
        AssetDirs { locale_sources: crate::locales(), keymap_source: Some(crate::keymap()), ..AssetDirs::default() };
    Env::load(&dirs).expect("the built-in files load")
}

/// A harness over `app`, in English, with Unicode glyphs and no motion.
pub fn harness(app: QCode, width: u16, height: u16) -> Harness<QCode> {
    let mut harness = Harness::with_env(app, env(), width, height);
    harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.render();
    harness
}

/// What a machine that knowingly has no Node sets to `1`, so the tests that run a program under
/// Node say they were skipped instead of failing.
pub const SKIP_NODE: &str = "QCODE_SKIP_NODE";

/// The Node on this machine's path, for a test that runs `what` under it.
///
/// A test without Node checks nothing, and passing it would read as checked: the gate would stay
/// green on a machine where a broken script goes unseen. So a missing Node fails the test and
/// says how to go on, and only [`SKIP_NODE`] set to `1` turns that into a skip, which the test
/// prints so the log still shows what was not run.
pub fn node(what: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = std::env::split_paths(&path).map(|dir| dir.join("node")).find(|candidate| {
        candidate.is_file()
            && std::process::Command::new(candidate).arg("--version").output().is_ok_and(|out| out.status.success())
    });
    if found.is_some() {
        return found;
    }
    assert!(
        std::env::var_os(SKIP_NODE).is_some_and(|value| value == "1"),
        "this machine has no Node on its path, and {what} needs one: install Node, or set {SKIP_NODE}=1 to skip \
         these tests knowingly"
    );
    eprintln!("skipped: this machine has no Node, and {what} needs one ({SKIP_NODE}=1)");
    None
}
