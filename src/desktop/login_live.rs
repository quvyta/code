//! The window's login taken whole, on a real engine: the sign-in window of a profile image built by
//! QCode's own builder, a login written into its database the way the application writes one, the
//! wizard's take-out, and workspaces whose homes are given it, with the agent's approvals the
//! profile's QCode template brings, before their windows open.
//!
//! A real Google sign-in cannot be made by a test, so the two rows are written by hand, with the
//! application running on them; everything around them is the product's own code. The window
//! opens on a compositor of the test's own, named by its socket in `QCODE_TEST_WAYLAND` (a headless
//! one, such as `kwin_wayland --virtual` started with a runtime folder of its own), never on the
//! person's screen: a socket that is the session's own is refused.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 QCODE_TEST_WAYLAND=<socket> cargo test login_live -- --ignored --test-threads=1
//! ```
//!
//! Everything it makes is named after the profile `logintest` and removed again: the image, the
//! containers, the credentials volume and the workspaces' homes.

use std::path::Path;
use std::time::{Duration, Instant};

use super::Display;
use super::login::{self, DATABASE, KEYS, STORED, Seen};
use crate::base::paths::{HOME_DIR, KEEP_ALIVE};
use crate::engine::run::capture;
use crate::engine::{
    Access, ContainerCreate, Engine, EngineKind, Exec, HostUser, Mount, MountSource, Network, detect, names,
};
use crate::profile::identity::{self, Home};
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::profiles::work;
use crate::ui::workspace::{ContainerPlan, give_window_login};

/// The profile these tests sign in.
const PROFILE: &str = "logintest";
/// The workspaces that are given its login: one new, one with a login of its own, and one whose
/// window ran before the profile was signed in and left the application's "signed out" behind.
const WORKSPACES: [&str; 3] = ["logintest-a", "logintest-b", "logintest-c"];

/// Writes a login into the database at `argv[1]` the way the application does when a sign-in
/// finishes, from an access token and an e-mail: the token row a `Topic` holding an
/// `OAuthTokenInfo` under `oauthTokenInfoSentinelKey`, the account row one holding a `UserStatus`
/// under `userStatusSentinelKey`, each as base64 of the protocol buffer. With no e-mail it writes
/// what a window that never signed in leaves: the token row holding only the "signed out" state.
const WRITER: &str = "const { DatabaseSync } = require('node:sqlite');
const [file, access, email] = process.argv.slice(1);
const v = (n) => { const o = []; while (n > 127) { o.push((n & 127) | 128); n = Math.floor(n / 128); } o.push(n); return o; };
const f = (no, bytes) => [...v((no << 3) | 2), ...v(bytes.length), ...bytes];
const s = (t) => [...Buffer.from(t, 'utf8')];
const b64 = (a) => Buffer.from(a).toString('base64');
const topic = (key, value) => b64(f(1, [...f(1, s(key)), ...f(2, f(1, s(value)))]));
const db = new DatabaseSync(file);
db.exec('CREATE TABLE IF NOT EXISTS ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)');
const put = db.prepare('INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)');
if (email) {
  const token = b64([...f(1, s(access)), ...f(2, s('Bearer')), ...f(3, s('refresh-' + access)), ...f(4, [8, ...v(1790000000)])]);
  put.run('antigravityUnifiedStateSync.oauthToken', topic('oauthTokenInfoSentinelKey', token));
  put.run('antigravityUnifiedStateSync.userStatus', topic('userStatusSentinelKey', b64([...f(3, s('Owner')), ...f(7, s(email))])));
} else {
  put.run('antigravityUnifiedStateSync.oauthToken', topic('authStateWithContextSentinelKey', JSON.stringify({ state: 'signedOut' })));
}
db.close();";

fn profile() -> Profile {
    Profile {
        name: SafeName::parse(PROFILE).expect("safe"),
        harness: HarnessKind::AntigravityIde,
        template: Template::Recommended,
        account: AccountKind::InApp,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os: crate::base::Os::Debian,
    }
}

/// Podman, when the tests are asked for.
fn engine() -> Option<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return None;
    }
    Some(detect(EngineKind::Podman).expect("these tests were asked for and podman did not answer"))
}

/// The compositor of the test's own.
fn display() -> Option<Display> {
    let socket = std::env::var_os("QCODE_TEST_WAYLAND").map(std::path::PathBuf::from)?;
    if let Ok(own) = super::current() {
        assert_ne!(own.socket, socket, "this test never opens anything on the person's own screen");
    }
    let name = socket.file_name().and_then(|name| name.to_str()).expect("a socket name").to_owned();
    Some(Display { socket, name, device: None })
}

/// A workspace's folders, under `root`.
fn paths(root: &Path, id: &str) -> WorkspacePaths {
    let folder = root.join(id);
    for part in ["Work", "Assets", "Harness", "Containers/MCP", "Containers/Browser"] {
        std::fs::create_dir_all(folder.join(part)).expect("a workspace folder");
    }
    WorkspacePaths {
        root: folder.clone(),
        file: folder.join("workspace.qcode"),
        code: folder.join("Work"),
        assets: folder.join("Assets"),
        harness: folder.join("Harness"),
    }
}

/// The plan of `id`'s window.
fn plan(root: &Path, id: &str) -> ContainerPlan {
    ContainerPlan::window(&WorkspaceId::parse(id).expect("legal"), &paths(root, id), &profile()).expect("a window")
}

/// Runs `command` in a container of its own that has nothing but `volume` as its home, and answers
/// what it printed.
fn in_home(engine: &Engine, volume: &str, command: &[&str]) -> Result<String, crate::engine::run::EngineError> {
    let name = format!("qcode-{PROFILE}-reader");
    let _ = capture(&engine.remove_container(&name));
    let mounts =
        [Mount { source: MountSource::Volume(volume), target: Path::new(HOME_DIR), access: Access::ReadWrite }];
    capture(&engine.create_container(&ContainerCreate {
        name: &name,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &profile().image(),
        mounts: &mounts,
        network: Network::None,
        user: HostUser::current().expect("the current user"),
        workdir: None,
        command: KEEP_ALIVE,
    }))
    .expect("a reader is made");
    capture(&engine.start_container(&name)).expect("the reader starts");
    let answer = capture(&engine.exec_without_terminal(&Exec { container: &name, command }));
    let _ = capture(&engine.remove_container(&name));
    answer
}

/// The two rows of `volume`'s database, as `key=value` lines in the order of [`KEYS`]; an empty
/// value for a row that is not there.
fn rows(engine: &Engine, volume: &str) -> String {
    let program = "const { DatabaseSync } = require('node:sqlite'); \
                   const db = new DatabaseSync(process.argv[1], { readOnly: true }); \
                   for (const key of process.argv.slice(2)) { const row = db.prepare('SELECT value FROM ItemTable WHERE key = ?').get(key); \
                   console.log(key + '=' + (row ? String(row.value) : '')); }";
    let database = format!("{HOME_DIR}/{DATABASE}");
    let mut command = vec!["node", "-e", program, &database];
    command.extend(KEYS);
    in_home(engine, volume, &command).unwrap_or_default().trim().to_owned()
}

/// Whether `volume`'s database holds a login, asked the way the wizard asks a window.
fn signed_in(engine: &Engine, volume: &str) -> bool {
    let database = format!("{HOME_DIR}/{DATABASE}");
    in_home(engine, volume, &["node", "-e", login::PROGRAM, "seen", &database]).is_ok()
}

/// The agent's approvals in `volume`'s database, read the way the application reads them.
fn approvals(engine: &Engine, volume: &str) -> String {
    let database = format!("{HOME_DIR}/{DATABASE}");
    in_home(engine, volume, &["node", "-e", login::PROGRAM, "approvals", &database]).unwrap_or_default()
}

/// What a home on a QCode template is given beside the login: nothing asks before the agent acts,
/// and the first-start onboarding that would write "Review-driven" over it is done.
const APPROVED: &str = "terminal=3\nreview=2\njavascript=4\nonboarding=true\n";

/// Writes a login, or with no e-mail the "signed out" state, into `volume`'s database.
fn write_into(engine: &Engine, volume: &str, access: &str, email: &str) {
    let database = format!("{HOME_DIR}/{DATABASE}");
    in_home(engine, volume, &["sh", "-c", "mkdir -p \"$(dirname \"$1\")\"", "sh", &database]).expect("a folder");
    in_home(engine, volume, &["node", "-e", WRITER, &database, access, email]).expect("the rows are written");
}

/// Waits a generous but finite while for `done`, looking every half second.
fn wait(what: &str, seconds: u64, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(seconds), "{what} did not happen in {seconds} s");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Takes away everything the test makes.
fn clear(engine: &Engine) {
    let sign_in = login::SignIn::named(&profile().name).expect("a private folder");
    login::close(engine, &sign_in);
    for workspace in WORKSPACES {
        let home = Home::new(profile().name, WorkspaceId::parse(workspace).expect("legal"));
        let _ = capture(&engine.remove_container(&names::desktop_container(workspace, PROFILE)));
        let _ = capture(&engine.remove_container(&home.settings_container()));
        let _ = capture(&engine.remove_container(&format!("qcode-refresh-{workspace}-{PROFILE}")));
        let _ = capture(&engine.remove_volume(&home.volume()));
    }
    let _ = capture(&engine.remove_container(&format!("qcode-{PROFILE}-reader")));
    let _ = capture(&engine.remove_container(&format!("qcode-store-{PROFILE}")));
    let _ = capture(&engine.remove_volume(&names::credential_volume(PROFILE)));
}

#[test]
#[ignore = "builds the window image and opens it on a compositor of the test's own; run with QCODE_CONTAINER_TESTS=1 QCODE_TEST_WAYLAND=<socket>"]
fn a_login_taken_from_the_sign_in_window_reaches_every_workspace_whole_and_the_application_starts_with_it() {
    let (Some(engine), Some(display)) = (engine(), display()) else { return };
    let user = HostUser::current().expect("the current user");
    let root = std::env::temp_dir().join(format!("qcode-loginlive-{}", std::process::id()));
    clear(&engine);
    let built = work::build_whole(&engine, &profile(), &|| false, &mut || {}, &mut |_| {});
    assert!(built.is_ok(), "the window image does not build: {built:?}");

    // The wizard's window, as the wizard opens it, with the application running in it.
    let sign_in = login::open(&engine, &profile(), &display).expect("the sign-in window opens");
    let database = format!("{HOME_DIR}/{DATABASE}");
    let has_database = || {
        capture(
            &engine.exec_without_terminal(&Exec { container: &sign_in.window, command: &["test", "-f", &database] }),
        )
        .is_ok()
    };
    wait("the application making its database", 180, has_database);
    assert_eq!(login::seen(&engine, &sign_in), Seen::NotYet, "a fresh window is not signed in");

    // A window left alone writes that it is signed out; that is no login.
    std::thread::sleep(Duration::from_secs(20));
    let fresh = rows(&engine, &sign_in.volume);
    println!("a window never signed in holds:\n{fresh}");
    assert_eq!(login::seen(&engine, &sign_in), Seen::NotYet, "a window that says it is signed out is not signed in");

    // The application signs in; the wizard's look finds it and the take-out stores it.
    let exec = ["node", "-e", WRITER, &database, "access-417", "owner@example.com"];
    capture(&engine.exec_without_terminal(&Exec { container: &sign_in.window, command: &exec })).expect("signed in");
    assert_eq!(login::seen(&engine, &sign_in), Seen::SignedIn);
    let expected = rows(&engine, &sign_in.volume);
    assert!(
        expected.lines().all(|line| line.split_once('=').is_some_and(|(_, value)| !value.is_empty())),
        "{expected}"
    );
    let stored = login::take(&engine, &profile(), &sign_in);
    login::close(&engine, &sign_in);
    assert_eq!(stored, Ok(1), "the two rows are stored as one file, {STORED}");
    let volumes = capture(&engine.list_volumes()).expect("volumes");
    assert!(identity::is_stored(&volumes, &profile().name), "the profile reads as signed in");
    assert!(!volumes.lines().any(|line| line.trim() == sign_in.volume), "the sign-in's home is gone");

    // Workspaces made afterwards, or opened before: one with a login of its own, one whose window
    // said it was signed out.
    let plans: Vec<ContainerPlan> = WORKSPACES.iter().map(|id| plan(&root, id)).collect();
    let home = |at: usize| plans[at].home.as_ref().expect("a home").volume();
    write_into(&engine, &home(1), "own-access", "someone@example.com");
    let own = rows(&engine, &home(1));
    write_into(&engine, &home(2), "", "");
    assert!(!signed_in(&engine, &home(2)), "a signed-out home holds no login");
    for _ in 0..2 {
        for plan in &plans {
            give_window_login(&engine, plan, user).expect("the home is given the login");
        }
        assert_eq!(rows(&engine, &home(0)), expected, "a new home gets the rows byte for byte");
        assert_eq!(rows(&engine, &home(2)), expected, "a signed-out home gets them too");
        assert_eq!(rows(&engine, &home(1)), own, "a workspace's own login is never replaced");
        // The profile is on QCode basic, so the approvals come with the login, and a home that
        // kept its own login keeps whatever was chosen in it.
        assert_eq!(approvals(&engine, &home(0)), APPROVED, "a new home");
        assert_eq!(approvals(&engine, &home(2)), APPROVED, "a signed-out home");
        assert_eq!(approvals(&engine, &home(1)), "terminal=\nreview=\njavascript=\nonboarding=\n");
    }

    // The application starts on a home given the login, and the login is still there after it has.
    let window = &plans[0];
    crate::ui::workspace::plan_open_window(&engine, window, user, &display).expect("the window opens");
    let started = || {
        capture(
            &engine
                .exec_without_terminal(&Exec { container: &window.name, command: &["pgrep", "-f", "antigravity-ide"] }),
        )
        .is_ok()
    };
    wait("the application starting", 120, started);
    std::thread::sleep(Duration::from_secs(20));
    let running = capture(&engine.container_state(&window.name)).unwrap_or_default();
    assert_eq!(running.trim(), "running", "the application is still up on the given login");
    let _ = capture(&engine.stop_container_within(&window.name, super::WINDOW_GRACE));
    let after = rows(&engine, &home(0));
    println!("after the application ran on it:\n{after}\nsame as given: {}", after == expected);
    assert!(signed_in(&engine, &home(0)), "the application kept the login it was given: {after}");
    // Started offline on them, the application did not put its onboarding's review-driven values
    // back over the approvals.
    assert_eq!(approvals(&engine, &home(0)), APPROVED, "after the application ran");

    clear(&engine);
    let _ = capture(&engine.remove_image(&profile().image()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "builds the window image and opens it on a compositor of the test's own; run with QCODE_CONTAINER_TESTS=1 QCODE_TEST_WAYLAND=<socket>"]
fn the_sign_in_window_asked_to_quit_writes_what_it_holds_before_the_login_is_taken() {
    // A real Google login stays in the application's memory until it quits: signed in at
    // 20:14:13, on disk at 20:20:49 when the window closed. So the wizard asks the window to quit
    // before it takes the login, and this proves the premise: asked the way the wizard asks, the
    // application quits by itself in time and closes its storage, which is what leaves
    // `state.vscdb.backup` beside the database. That the asking comes before the taking is proven
    // without an engine in `window_tests`.
    let (Some(engine), Some(display)) = (engine(), display()) else { return };
    clear(&engine);
    let built = work::build_whole(&engine, &profile(), &|| false, &mut || {}, &mut |_| {});
    assert!(built.is_ok(), "the window image does not build: {built:?}");
    let sign_in = login::open(&engine, &profile(), &display).expect("the sign-in window opens");
    let database = format!("{HOME_DIR}/{DATABASE}");
    let backup = format!("{database}.backup");
    wait("the application writing its database", 180, || {
        in_home(&engine, &sign_in.volume, &["test", "-f", &database]).is_ok()
    });
    assert!(in_home(&engine, &sign_in.volume, &["test", "-f", &backup]).is_err(), "no quit has happened yet");

    let asked = Instant::now();
    let stopped = capture(&engine.stop_container_within(&sign_in.window, login::QUIT_WITHIN));
    let took = asked.elapsed();
    let quit = in_home(&engine, &sign_in.volume, &["test", "-f", &backup]).is_ok();
    login::close(&engine, &sign_in);
    clear(&engine);
    let _ = capture(&engine.remove_image(&profile().image()));
    assert!(stopped.is_ok(), "{stopped:?}");
    assert!(
        took < Duration::from_secs(u64::from(login::QUIT_WITHIN)),
        "the application did not quit by itself: {took:?}"
    );
    assert!(quit, "the application quit without closing its storage");
}
