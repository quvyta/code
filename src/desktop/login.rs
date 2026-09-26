//! Signing a window in once, in the profile wizard, and giving that login to every workspace.
//!
//! What the login is made of, read in Antigravity IDE 2.5.5's own code (the archive the record
//! names, unpacked outside any installation):
//!
//! - The Google sign-in is the IDE's main process, `antigravityAuthMainService` in
//!   `resources/app/out/main.js`. The extension's authentication provider
//!   (`resources/app/extensions/antigravity/dist/extension.js`, `ExternalAuthProvider.getSessions`)
//!   answers a session only when two things are both there: the OAuth token
//!   (`OAuthPreferences.getOAuthTokenInfo`) and the user status with a non-empty e-mail
//!   (`UserStatus.getUserStatus`). With either missing it answers none, and the window asks to
//!   sign in.
//! - Both are kept by the application's storage service at application scope, under the keys
//!   `antigravityUnifiedStateSync.oauthToken` (`oauthLifecycle.js`: access token, refresh token,
//!   expiry, token type) and `antigravityUnifiedStateSync.userStatus` (`userStatusLifecycle.js`:
//!   name, e-mail, tier). Each value is a protocol buffer written as base64 text
//!   (`out-build/vs/base/common/proto.js`, `Qo` and `qo`).
//! - Application-scope storage is the `ItemTable` of `state.vscdb` in the default profile's
//!   `globalStorage` (`aAt.STORAGE_NAME = "state.vscdb"`, under `globalStorageHome`), which is
//!   `~/.config/Antigravity IDE/User/globalStorage/state.vscdb` for the product name
//!   `Antigravity IDE` in `product.json`. The table is
//!   `ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)`.
//! - Nothing of the sign-in goes through Electron's `safeStorage`: its one use in `main.js` is the
//!   encryption service behind extensions' secret storage, and the token is written as plain
//!   base64 into the table above. So a login taken from one container is read in another with no
//!   keyring and no `--password-store=basic`; the window's flags stay as they were.
//! - `~/.gemini/` holds no part of it: the language server's own paths there are artifacts,
//!   transcripts, its settings and the MCP servers' own tokens (`language_server_linux_x64`).
//!
//! So the smallest thing that carries the login is those two rows, not the database. The database
//! holds everything else the application remembers about a home — recent folders, panels, what
//! each extension stored — and copying it would carry the sign-in window's state into every
//! workspace, and would overwrite a workspace's own on the way. The two rows are taken out into a
//! file of QCode's ([`STORED`]) and put into a workspace's database, created when it is not there,
//! by a program of QCode's run inside a container ([`PROGRAM`]): the image carries Node, and Node 24
//! reads and writes SQLite by itself, so nothing is added to the image for it.
//!
//! The copy is made only where the workspace has no login of its own ([`Fill::Keep`]): a
//! workspace whose window was signed in to another account, or signed in again, keeps it. A person
//! who asks for the profile's login to be given to every workspace again gets [`Fill::Replace`].

use std::path::{Path, PathBuf};

use crate::base::paths::{HOME_DIR, KEEP_ALIVE};
use crate::engine::names;
use crate::engine::run::capture;
use crate::engine::scratch::Scratch;
use crate::engine::{
    Access, ContainerCreate, ContainerState, Engine, Exec, HostUser, Mount, MountSource, Network, RunOnce,
};
use crate::profile::identity::STORE_DIR;
use crate::profile::{Profile, SafeName};
use crate::ui::profiles::Problem;
use crate::ui::profiles::recipe::CAPTURE_DIR;
use crate::ui::profiles::work::{self, LoginContainer};

use super::{Display, signin};

/// The database the login lives in, relative to the home directory.
pub const DATABASE: &str = ".config/Antigravity IDE/User/globalStorage/state.vscdb";

/// The rows of [`DATABASE`] that are the login: the token first, then the account it belongs to.
pub const KEYS: [&str; 2] = ["antigravityUnifiedStateSync.oauthToken", "antigravityUnifiedStateSync.userStatus"];

/// The file the two rows are kept in, in the profile's credentials volume and in the folder they
/// are taken out into. Its presence in a credentials volume is what says the login is a window's.
pub const STORED: &str = "antigravity-login.json";

/// How long the sign-in window is given to quit, writing what it holds, before it is ended.
pub const QUIT_WITHIN: u32 = 30;

/// How long the wizard waits between two looks for the login while the sign-in window is open.
/// Each look runs a program in the window's container, so it is not made more often than a person
/// would notice.
pub const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// How long the wizard waits between two looks for a page the window wants opened. That look only
/// reads a folder of this machine, and the person has just pressed a button in the window.
pub const ASK_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// The program that reads and writes the two rows, run by Node inside a container.
///
/// A row being there is not a login: the application writes the token's row as soon as it has
/// looked whether it is signed in, holding only "signed out" when it is not (measured: a window
/// that never signed in had the row, with `authStateWithContextSentinelKey` in it and no token).
/// So the rows are read the way the application reads them. Each is a `Topic` of the application's
/// state sync, `data` (1) a map of `key` (1) to a `Row` whose `value` (1) is the entry, all written
/// as base64 of the protocol buffer (`exa.unified_state_sync_pb`, read out of the descriptors in
/// `main.js`). A login is the token row's `oauthTokenInfoSentinelKey` entry, an `OAuthTokenInfo`
/// with an access (1) or refresh (3) token, beside the account row's `userStatusSentinelKey`
/// entry, a `UserStatus` with an e-mail (7) — the two things the extension asks for.
///
/// With a fourth word `approve`, `keep` and `replace` also write what makes the agent act without
/// asking, in the same breath as the login and only where the login is written: see [`APPROVALS`].
/// `approvals <database>` prints those three settings as the application would read them, one
/// `name=value` line each, empty for one that is not there.
///
/// `seen <database>` answers 0 when there is a login; `take <database> <file>` writes the two rows
/// into the file and fails when there is none; `keep <file> <database>` puts them into the database
/// unless it holds a token of its own, and `replace <file> <database>` puts them in whatever it
/// holds. A database that is not there is made, with the table the application makes. Written with
/// double quotes only, so that a shell can carry it inside single ones.
pub const PROGRAM: &str = r#""use strict";
const fs = require("node:fs");
const path = require("node:path");
const { DatabaseSync } = require("node:sqlite");
const KEYS = ["antigravityUnifiedStateSync.oauthToken", "antigravityUnifiedStateSync.userStatus"];
const [mode, from, to, approve] = process.argv.slice(1);
const text = (value) => (typeof value === "string" ? value : Buffer.from(value).toString("utf8"));
const fields = (bytes) => {
  const out = [];
  let at = 0;
  const varint = () => {
    let value = 0n;
    let shift = 0n;
    for (;;) {
      const byte = bytes[at++];
      if (byte === undefined) throw new Error("short");
      value |= BigInt(byte & 127) << shift;
      shift += 7n;
      if (byte < 128) return value;
    }
  };
  while (at < bytes.length) {
    const tag = Number(varint());
    const kind = tag & 7;
    if (kind === 0) out.push([tag >> 3, varint()]);
    else if (kind === 2) {
      const length = Number(varint());
      if (at + length > bytes.length) throw new Error("short");
      out.push([tag >> 3, bytes.subarray(at, at + length)]);
      at += length;
    } else if (kind === 1) at += 8;
    else if (kind === 5) at += 4;
    else throw new Error("wire type");
  }
  return out;
};
const field = (message, wanted) => {
  try {
    for (const [number, value] of fields(Buffer.from(message, "base64"))) {
      if (number === wanted && typeof value !== "bigint") return text(value);
    }
  } catch (error) {
    return "";
  }
  return "";
};
const entry = (topic, wanted) => {
  try {
    for (const [number, data] of fields(Buffer.from(topic, "base64"))) {
      if (number !== 1 || typeof data === "bigint") continue;
      let key = "";
      let row = null;
      for (const [part, value] of fields(data)) {
        if (part === 1 && typeof value !== "bigint") key = text(value);
        if (part === 2 && typeof value !== "bigint") row = value;
      }
      if (key === wanted && row) {
        for (const [part, value] of fields(row)) if (part === 1 && typeof value !== "bigint") return text(value);
      }
    }
  } catch (error) {
    return "";
  }
  return "";
};
const token = (login) => entry(login[KEYS[0]] || "", "oauthTokenInfoSentinelKey");
const signed = (login) => field(token(login), 1).length > 0 || field(token(login), 3).length > 0;
const account = (login) => field(entry(login[KEYS[1]] || "", "userStatusSentinelKey"), 7);
const whole = (login) => signed(login) && account(login).length > 0;
const read = (file) => {
  const found = {};
  if (!fs.existsSync(file)) return found;
  const db = new DatabaseSync(file, { readOnly: true });
  try {
    const row = db.prepare("SELECT value FROM ItemTable WHERE key = ?");
    for (const key of KEYS) {
      const hit = row.get(key);
      if (hit && hit.value !== null && hit.value !== undefined) found[key] = text(hit.value);
    }
  } catch (error) {
    return {};
  } finally {
    db.close();
  }
  return found;
};
const varint = (number) => {
  const out = [];
  while (number > 127) {
    out.push((number & 127) | 128);
    number = Math.floor(number / 128);
  }
  out.push(number);
  return out;
};
const bytes = (number, value) => [...varint((number << 3) | 2), ...varint(value.length), ...value];
const utf8 = (value) => [...Buffer.from(value, "utf8")];
const base64 = (value) => Buffer.from(value).toString("base64");
const topic = (entries) => base64(entries.flatMap(([key, value]) => bytes(1, [...bytes(1, utf8(key)), ...bytes(2, bytes(1, utf8(value)))])));
const whole_number = (number, value) => base64([...varint(number << 3), ...varint(value)]);
const APPROVALS = [
  ["antigravityUnifiedStateSync.agentPreferences", topic([
    ["terminalAutoExecutionPolicySentinelKey", whole_number(2, 3)],
    ["artifactReviewPolicySentinelKey", whole_number(2, 2)],
  ])],
  ["antigravityUnifiedStateSync.browserPreferences", topic([["browser_js_execution_config_sentinel_key", whole_number(1, 4)]])],
  ["antigravityOnboarding", "true"],
];
const number = (message, wanted) => {
  try {
    for (const [at, value] of fields(Buffer.from(message, "base64"))) if (at === wanted && typeof value === "bigint") return String(value);
  } catch (error) {
    return "";
  }
  return "";
};
const row = (file, key) => {
  if (!fs.existsSync(file)) return "";
  const db = new DatabaseSync(file, { readOnly: true });
  try {
    const hit = db.prepare("SELECT value FROM ItemTable WHERE key = ?").get(key);
    return hit && hit.value !== null && hit.value !== undefined ? text(hit.value) : "";
  } catch (error) {
    return "";
  } finally {
    db.close();
  }
};
if (mode === "approvals") {
  const agent = row(from, APPROVALS[0][0]);
  const browser = row(from, APPROVALS[1][0]);
  console.log("terminal=" + number(entry(agent, "terminalAutoExecutionPolicySentinelKey"), 2));
  console.log("review=" + number(entry(agent, "artifactReviewPolicySentinelKey"), 2));
  console.log("javascript=" + number(entry(browser, "browser_js_execution_config_sentinel_key"), 1));
  console.log("onboarding=" + row(from, APPROVALS[2][0]));
  process.exit(0);
}
if (mode === "seen") process.exit(whole(read(from)) ? 0 : 1);
if (mode === "take") {
  const login = read(from);
  if (!whole(login)) process.exit(1);
  fs.mkdirSync(path.dirname(to), { recursive: true });
  fs.writeFileSync(to + ".part", JSON.stringify(login), { mode: 0o600 });
  fs.renameSync(to + ".part", to);
  process.exit(0);
}
if (mode === "keep" || mode === "replace") {
  const login = JSON.parse(fs.readFileSync(from, "utf8"));
  if (!whole(login)) process.exit(1);
  if (mode === "keep" && signed(read(to))) process.exit(0);
  fs.mkdirSync(path.dirname(to), { recursive: true });
  const db = new DatabaseSync(to);
  db.exec("CREATE TABLE IF NOT EXISTS ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)");
  const put = db.prepare("INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)");
  for (const key of KEYS) put.run(key, login[key]);
  if (approve === "approve") for (const [key, value] of APPROVALS) put.run(key, value);
  db.close();
  process.exit(0);
}
process.exit(2);
"#;

/// The file a profile image on a QCode template carries so that the courier giving a home its login
/// also writes the agent's approvals into it. Its presence is the whole of the decision: the image
/// is built from the profile's template, and built again when the template changes, so the courier
/// never has to be told which template the profile is on. An image on `base` has no such file, and
/// the application's first start asks the person as it comes.
///
/// What is written, read in Antigravity IDE 2.5.5's `main.js` and `workbench.desktop.main.js`:
///
/// - `antigravityUnifiedStateSync.agentPreferences`, a state-sync `Topic` whose
///   `terminalAutoExecutionPolicySentinelKey` is a `Primitive` with `int32_value` (2) 3, `EAGER`
///   ("Always Proceed" for terminal commands), and `artifactReviewPolicySentinelKey` one with 2,
///   `TURBO` ("Always Proceed" for artifacts).
/// - `antigravityUnifiedStateSync.browserPreferences`, whose `browser_js_execution_config_sentinel_key`
///   is a `BrowserJavascriptExecutionConfig` with `browser_js_execution_policy` (1) 4, `TURBO`.
/// - `antigravityOnboarding` = `"true"`. The first-start onboarding ends by writing all three
///   policies from the mode the person picks, "Review-driven" unless they change it, over whatever
///   was there; `maybeStart()` skips it when this key holds anything. It is stored with
///   `store(key, "true", 0, 0)`, the profile scope, which for the default profile is this same
///   database. Nothing else is gated on it: the agent panel's barrier opens on it, and the login-only
///   flow comes up only when the person presses "Log in" or "Sign In" (`showLoginFlow`), which with a
///   login present the title bar does not even offer (`updateLoginNudgeVisibility`).
///
/// A person who changes a policy inside a workspace afterwards keeps it: a home that has a login is
/// never written again by [`Fill::Keep`].
pub const APPROVALS: &str = "/usr/share/qcode/antigravity-approvals";

/// The rows the program writes for [`APPROVALS`], as the application stores them, so a test can hold
/// the program to them without running it.
pub const APPROVAL_ROWS: [(&str, &str); 3] = [
    (
        "antigravityUnifiedStateSync.agentPreferences",
        "CjAKJnRlcm1pbmFsQXV0b0V4ZWN1dGlvblBvbGljeVNlbnRpbmVsS2V5EgYKBEVBTT0KKQofYXJ0aWZhY3RSZXZpZXdQb2xpY3lTZW50aW5lbEtleRIGCgRFQUk9",
    ),
    (
        "antigravityUnifiedStateSync.browserPreferences",
        "CjIKKGJyb3dzZXJfanNfZXhlY3V0aW9uX2NvbmZpZ19zZW50aW5lbF9rZXkSBgoEQ0FRPQ==",
    ),
    ("antigravityOnboarding", "true"),
];

/// Whether a login given to a home may replace one the home already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// Only a home with no login of its own is given one: a workspace's first window, or one that
    /// was never signed in.
    Keep,
    /// Every home is given it, whatever it had: the person asked for the profile's login again.
    Replace,
}

impl Fill {
    fn word(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Replace => "replace",
        }
    }
}

/// The command that gives a home its profile's stored login, run in a container that mounts the
/// credentials volume at [`STORE_DIR`] and the home at [`HOME_DIR`].
///
/// What the store holds decides how: a window's login is the two rows, put into the home's
/// database; every other harness's is files, copied over. So the one courier serves both, and a
/// home never needs to be told which harness it belongs to. The courier runs from the profile's
/// image, so an image on a QCode template ([`APPROVALS`]) has the agent's approvals written with the
/// login, and one on `base` has the login alone.
#[must_use]
pub fn give(fill: Fill) -> Vec<String> {
    let script = format!(
        "if [ -f '{STORE_DIR}/{STORED}' ]; then approve=; [ -f '{APPROVALS}' ] && approve=approve; \
         exec node -e \"$1\" \"$2\" '{STORE_DIR}/{STORED}' '{HOME_DIR}/{DATABASE}' \"$approve\"; \
         else cp -R {STORE_DIR}/. {HOME_DIR}/; fi"
    );
    ["sh", "-c", &script, "sh", PROGRAM, fill.word()].map(str::to_owned).to_vec()
}

/// The shell that takes a window's login out of the home it was made in, into the capture folder;
/// it fails when the login is not whole, so a sign-in that never happened is never stored.
#[must_use]
pub fn capture_script() -> String {
    format!("set -e\nnode -e '{PROGRAM}' take '{HOME_DIR}/{DATABASE}' '{CAPTURE_DIR}/{STORED}'")
}

/// The command that answers, inside a running container, whether its home holds a whole login.
#[must_use]
pub fn seen_command() -> Vec<String> {
    ["node", "-e", PROGRAM, "seen", &format!("{HOME_DIR}/{DATABASE}")].map(str::to_owned).to_vec()
}

/// The window a profile is signed in from, and everything made for it.
#[derive(Debug, PartialEq, Eq)]
pub struct SignIn {
    /// The profile being signed in.
    pub profile: SafeName,
    /// The container the window is open in.
    pub window: String,
    /// The volume that is the window's home while it is open, and nothing afterwards.
    pub volume: String,
    /// The short-lived container the login is taken out of the home by.
    pub courier: String,
    /// The folder of this machine the login is taken out into.
    pub capture: PathBuf,
    /// The folder of this machine the window leaves the addresses it wants opened in.
    pub browser: PathBuf,
    /// The private folder both of those are in. The login passes through it, so it is the
    /// person's alone, and it goes with the sign-in however that ends.
    folder: Scratch,
}

impl SignIn {
    /// What signing `profile` in is called, and the private folder of this machine it uses; the
    /// containers and the volume are not made yet.
    ///
    /// # Errors
    ///
    /// When no private folder can be made.
    pub fn named(profile: &SafeName) -> std::io::Result<Self> {
        let folder = Scratch::new(&format!("signin-{profile}"))?;
        Ok(Self {
            profile: profile.clone(),
            window: format!("qcode-signin-{profile}"),
            volume: format!("qcode-signin-{profile}"),
            courier: format!("qcode-signin-take-{profile}"),
            capture: folder.path().join("capture"),
            browser: folder.path().join("browser"),
            folder,
        })
    }

    /// The command that opens the window: the profile's image, a home of its own in
    /// [`SignIn::volume`], the folder it leaves addresses in, and the network, which a sign-in
    /// cannot do without. No workspace is mounted and none is opened: the window is only there to
    /// be signed in.
    #[must_use]
    pub fn open_command(
        &self,
        engine: &Engine,
        profile: &Profile,
        user: HostUser,
        display: &Display,
        seccomp: Option<&Path>,
    ) -> Option<crate::engine::EngineCommand> {
        let desktop = profile.harness.desktop()?;
        let program = desktop.command();
        let mut command = vec![program.as_str()];
        command.extend(desktop.flags);
        let image = profile.image();
        let mounts = [
            Mount { source: MountSource::Volume(&self.volume), target: Path::new(HOME_DIR), access: Access::ReadWrite },
            Mount {
                source: MountSource::Path(&self.browser),
                target: Path::new(signin::OPEN_DIR),
                access: Access::ReadWrite,
            },
        ];
        let once =
            RunOnce { image: &image, mounts: &mounts, network: Network::Full, user, workdir: None, command: &command };
        Some(super::run_command(engine, &self.window, once, display, seccomp))
    }
}

/// Whether the window's home holds a login, looked at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// Both rows are there.
    SignedIn,
    /// The window is open and not signed in yet.
    NotYet,
    /// The window is gone: the person closed it, or it never came up.
    Closed,
}

/// Opens the sign-in window of `profile` on `display`.
///
/// Whatever an interrupted sign-in left under the same names is taken away first: its window
/// would stand in the way of this one, and its home could hold a login that is not this one.
///
/// # Errors
///
/// When a folder cannot be made, the seccomp profile cannot be written, or the engine refuses.
pub fn open(engine: &Engine, profile: &Profile, display: &Display) -> Result<SignIn, Problem> {
    let sign_in = SignIn::named(&profile.name).map_err(|error| Problem::Machine(error.to_string()))?;
    clear(engine, &sign_in);
    let user = HostUser::current().map_err(|error| Problem::Machine(error.to_string()))?;
    for folder in [&sign_in.capture, &sign_in.browser] {
        std::fs::create_dir_all(folder).map_err(|error| Problem::Machine(error.to_string()))?;
    }
    let seccomp = if engine.needs_sandbox_profile() {
        Some(super::seccomp::file().map_err(|error| Problem::Machine(error.to_string()))?)
    } else {
        None
    };
    let Some(command) = sign_in.open_command(engine, profile, user, display, seccomp.as_deref()) else {
        close(engine, &sign_in);
        return Err(Problem::NoLogin);
    };
    match capture(&command) {
        Ok(_) => Ok(sign_in),
        Err(error) => {
            close(engine, &sign_in);
            Err(error.into())
        }
    }
}

/// Looks once whether the window's home holds a login.
///
/// The look runs inside the window's own container. When it cannot, the container is asked
/// whether it still runs: a window that is gone is said as such, so the wizard stops looking and
/// takes whatever the home has.
pub fn seen(engine: &Engine, sign_in: &SignIn) -> Seen {
    let command = seen_command();
    let words: Vec<&str> = command.iter().map(String::as_str).collect();
    match capture(&engine.exec_without_terminal(&Exec { container: &sign_in.window, command: &words })) {
        Ok(_) => Seen::SignedIn,
        Err(_) => {
            let state = capture(&engine.container_state(&sign_in.window)).map(|word| ContainerState::parse(&word));
            if state.is_ok_and(|state| state.is_running()) { Seen::NotYet } else { Seen::Closed }
        }
    }
}

/// Takes the login out of the window's home and stores it with the profile, and answers how many
/// files it stored.
///
/// It is taken by a short-lived container of its own that mounts the same home, rather than from
/// the window's: the window may be closed already, and a closed container cannot be run in. The
/// storing itself is the one every harness's login goes through.
///
/// # Errors
///
/// [`Problem::NoLogin`] when the home holds no whole login, or the engine's words when it refuses.
pub fn take(engine: &Engine, profile: &Profile, sign_in: &SignIn) -> Result<usize, Problem> {
    let user = HostUser::current().map_err(|error| Problem::Machine(error.to_string()))?;
    // The application keeps a new login in memory and writes it to its home only when it quits
    // (measured with a real Google sign-in: signed in at 20:14:13, the two rows on disk at
    // 20:20:49, the second the window closed). So the window is asked to quit, and given the time
    // to, before anything is read; removing it outright would lose the login it had not written.
    // A window that is gone already makes the engine refuse, which changes nothing here.
    let _ = capture(&engine.stop_container_within(&sign_in.window, QUIT_WITHIN));
    let _ = capture(&engine.remove_container(&sign_in.courier));
    let mounts = [
        Mount { source: MountSource::Volume(&sign_in.volume), target: Path::new(HOME_DIR), access: Access::ReadWrite },
        Mount {
            source: MountSource::Path(&sign_in.capture),
            target: Path::new(CAPTURE_DIR),
            access: Access::ReadWrite,
        },
    ];
    let image = profile.image();
    let create = engine.create_container(&ContainerCreate {
        name: &sign_in.courier,
        hostname: names::HOSTNAME,
        labels: &[],
        image: &image,
        mounts: &mounts,
        network: Network::None,
        user,
        workdir: None,
        command: KEEP_ALIVE,
    });
    let up = capture(&create).and_then(|_| capture(&engine.start_container(&sign_in.courier)));
    let stored = match up {
        Ok(_) => {
            let courier = LoginContainer::into_folder(sign_in.courier.clone(), sign_in.capture.clone());
            work::store_login(engine, profile, &courier)
        }
        Err(error) => Err(error.into()),
    };
    let _ = capture(&engine.remove_container(&sign_in.courier));
    stored
}

/// Takes away everything a sign-in made: the window, the courier, the window's home and this
/// machine's folders. What cannot be removed is left rather than reported; the next sign-in of the
/// profile starts by clearing it.
pub fn close(engine: &Engine, sign_in: &SignIn) {
    clear(engine, sign_in);
    let _ = std::fs::remove_dir_all(sign_in.folder.path());
}

/// Removes the containers and the volume of a sign-in, the window first since it holds the volume.
fn clear(engine: &Engine, sign_in: &SignIn) {
    for container in [&sign_in.window, &sign_in.courier] {
        let _ = capture(&engine.remove_container(container));
    }
    let _ = capture(&engine.remove_volume(&sign_in.volume));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineKind;

    fn words(command: &crate::engine::EngineCommand) -> Vec<String> {
        command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_program_names_the_two_rows_and_travels_inside_single_quotes() {
        for key in KEYS {
            assert!(PROGRAM.contains(&format!("\"{key}\"")), "{key}");
        }
        assert!(!PROGRAM.contains('\''), "a shell carries the program inside single quotes");
        assert!(DATABASE.ends_with("/User/globalStorage/state.vscdb"), "{DATABASE}");
        // The same table the application makes, so a database QCode starts is one it reads.
        assert!(PROGRAM.contains("ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)"));
    }

    #[test]
    fn a_window_login_is_put_into_the_database_and_any_other_is_copied() {
        let keep = give(Fill::Keep);
        assert_eq!(&keep[..2], ["sh", "-c"]);
        assert_eq!(&keep[3..], ["sh", PROGRAM, "keep"]);
        let script = &keep[2];
        assert!(script.contains(&format!("[ -f '{STORE_DIR}/{STORED}' ]")), "{script}");
        assert!(script.contains(&format!("'{HOME_DIR}/{DATABASE}'")), "{script}");
        assert!(script.contains(&format!("else cp -R {STORE_DIR}/. {HOME_DIR}/; fi")), "{script}");
        assert_eq!(give(Fill::Replace).last().map(String::as_str), Some("replace"));
    }

    #[test]
    fn the_courier_asks_for_the_approvals_only_from_an_image_that_carries_the_mark() {
        let script = &give(Fill::Keep)[2];
        assert!(script.contains(&format!("approve=; [ -f '{APPROVALS}' ] && approve=approve;")), "{script}");
        assert!(script.contains(&format!("'{HOME_DIR}/{DATABASE}' \"$approve\";")), "{script}");
    }

    /// A state-sync `Topic` decoded down to each key's `Primitive` fields.
    type Decoded = Vec<(String, Vec<(u64, Vec<u8>)>)>;

    /// `text` decoded from standard base64.
    fn unbase64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let (mut bits, mut held) = (0_u32, 0_u32);
        for byte in text.bytes().filter(|&byte| byte != b'=') {
            let at = ALPHABET.iter().position(|&letter| letter == byte).expect("base64");
            bits = (bits << 6) | u32::try_from(at).expect("small");
            held += 6;
            if held >= 8 {
                held -= 8;
                out.push(u8::try_from((bits >> held) & 0xff).expect("a byte"));
            }
        }
        out
    }

    /// The fields of one protobuf message, by number: a length-delimited field as its bytes, a
    /// varint as its value's bytes spelled out in decimal.
    fn message(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
        let varint = |at: &mut usize| {
            let (mut value, mut shift) = (0_u64, 0);
            loop {
                let byte = bytes[*at];
                *at += 1;
                value |= u64::from(byte & 127) << shift;
                shift += 7;
                if byte < 128 {
                    return value;
                }
            }
        };
        let (mut out, mut at) = (Vec::new(), 0);
        while at < bytes.len() {
            let tag = varint(&mut at);
            let value = match tag & 7 {
                0 => varint(&mut at).to_string().into_bytes(),
                2 => {
                    let length = usize::try_from(varint(&mut at)).expect("a length");
                    at += length;
                    bytes[at - length..at].to_vec()
                }
                kind => panic!("wire type {kind}"),
            };
            out.push((tag >> 3, value));
        }
        out
    }

    /// A state-sync `Topic` row, as `(key, the Row's value decoded from base64)` pairs.
    fn topic(row: &str) -> Vec<(String, Vec<u8>)> {
        message(&unbase64(row))
            .into_iter()
            .map(|(number, entry)| {
                assert_eq!(number, 1, "Topic.data");
                let entry = message(&entry);
                assert_eq!(entry.iter().map(|(number, _)| *number).collect::<Vec<_>>(), [1, 2], "a map entry");
                let row = message(&entry[1].1);
                assert_eq!(row.len(), 1, "Row.value alone");
                assert_eq!(row[0].0, 1, "Row.value");
                (
                    String::from_utf8(entry[0].1.clone()).expect("a key"),
                    unbase64(std::str::from_utf8(&row[0].1).expect("text")),
                )
            })
            .collect()
    }

    #[test]
    fn the_approval_rows_say_always_proceed_as_the_application_reads_them() {
        // Decoded here without the program, so the rows are held to what the application's own
        // reading of them means: `Primitive.int32_value` (2) 3 is EAGER and 2 is TURBO, and
        // `browser_js_execution_policy` (1) 4 is TURBO.
        let [(agent_key, agent), (browser_key, browser), (onboarding_key, onboarding)] = APPROVAL_ROWS;
        assert_eq!(agent_key, "antigravityUnifiedStateSync.agentPreferences");
        let agent: Decoded = topic(agent).into_iter().map(|(key, value)| (key, message(&value))).collect();
        assert_eq!(
            agent,
            [
                ("terminalAutoExecutionPolicySentinelKey".to_owned(), vec![(2, b"3".to_vec())]),
                ("artifactReviewPolicySentinelKey".to_owned(), vec![(2, b"2".to_vec())]),
            ]
        );
        assert_eq!(browser_key, "antigravityUnifiedStateSync.browserPreferences");
        let browser: Decoded = topic(browser).into_iter().map(|(key, value)| (key, message(&value))).collect();
        assert_eq!(browser, [("browser_js_execution_config_sentinel_key".to_owned(), vec![(1, b"4".to_vec())])]);
        assert_eq!((onboarding_key, onboarding), ("antigravityOnboarding", "true"));
    }

    /// Node, when this machine has one: the program runs in the image, which always has it.
    fn node() -> Option<PathBuf> {
        let found = std::process::Command::new("sh").args(["-c", "command -v node"]).output().ok()?;
        found.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&found.stdout).trim()))
    }

    /// A login as the application writes one after a Google sign-in: the token for `access-417`
    /// and the account `owner@example.com`, in the shape the take-out stores.
    const LOGIN: &str = r#"{"antigravityUnifiedStateSync.oauthToken":"Cl8KGW9hdXRoVG9rZW5JbmZvU2VudGluZWxLZXkSQgpAQ2dwaFkyTmxjM010TkRFM0VnWkNaV0Z5WlhJYUVuSmxabkpsYzJndFlXTmpaWE56TFRReE55SUdDSUQzeE5VRw==","antigravityUnifiedStateSync.userStatus":"Cj8KFXVzZXJTdGF0dXNTZW50aW5lbEtleRImCiRHZ1ZQZDI1bGNqb1JiM2R1WlhKQVpYaGhiWEJzWlM1amIyMD0="}"#;

    /// Runs the program with `words` and answers what it printed.
    fn program(node: &Path, words: &[&str]) -> String {
        let out = std::process::Command::new(node).args(["-e", PROGRAM]).args(words).output().expect("node runs");
        assert!(out.status.success(), "{words:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// One row of `database`, as text, read without the program.
    fn raw(node: &Path, database: &Path, key: &str) -> String {
        let read = "const { DatabaseSync } = require('node:sqlite'); \
                    const row = new DatabaseSync(process.argv[1], { readOnly: true }).prepare('SELECT value FROM ItemTable WHERE key = ?').get(process.argv[2]); \
                    process.stdout.write(row ? String(row.value) : '');";
        let out =
            std::process::Command::new(node).args(["-e", read]).arg(database).arg(key).output().expect("node runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn the_approvals_are_written_as_the_application_stores_them_and_only_with_a_login_given() {
        let Some(node) = node() else { return };
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-approvals-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a folder");
        let login = folder.join(STORED);
        std::fs::write(&login, LOGIN).expect("a stored login");
        let spelled = |path: &Path| path.to_str().expect("a path").to_owned();

        // A home on a QCode template: the login and the three rows, byte for byte as the
        // application writes them, and read back as the application reads them.
        let approved = folder.join("approved/state.vscdb");
        program(&node, &["keep", &spelled(&login), &spelled(&approved), "approve"]);
        for (key, value) in APPROVAL_ROWS {
            assert_eq!(raw(&node, &approved, key), value, "{key}");
        }
        assert_eq!(
            program(&node, &["approvals", &spelled(&approved)]),
            "terminal=3\nreview=2\njavascript=4\nonboarding=true\n"
        );
        assert!(
            std::process::Command::new(&node)
                .args(["-e", PROGRAM, "seen"])
                .arg(&approved)
                .status()
                .expect("runs")
                .success()
        );

        // A home on base: the login alone, and the application asks as it comes.
        let plain = folder.join("plain/state.vscdb");
        program(&node, &["keep", &spelled(&login), &spelled(&plain), ""]);
        assert_eq!(program(&node, &["approvals", &spelled(&plain)]), "terminal=\nreview=\njavascript=\nonboarding=\n");

        // A home that has a login of its own keeps it and every choice made in it.
        let own = folder.join("own/state.vscdb");
        program(&node, &["keep", &spelled(&login), &spelled(&own), ""]);
        program(&node, &["keep", &spelled(&login), &spelled(&own), "approve"]);
        assert_eq!(program(&node, &["approvals", &spelled(&own)]), "terminal=\nreview=\njavascript=\nonboarding=\n");
        // Asked for again by the person, it is written whatever the home had.
        program(&node, &["replace", &spelled(&login), &spelled(&own), "approve"]);
        assert_eq!(
            program(&node, &["approvals", &spelled(&own)]),
            "terminal=3\nreview=2\njavascript=4\nonboarding=true\n"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(unix)]
    #[test]
    fn the_login_taken_out_of_a_window_is_readable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let Some(node) = node() else {
            eprintln!("skipped: this machine has no Node, and the program that takes the login out needs one");
            return;
        };
        let folder = Scratch::new("token-mode").expect("a folder");
        let spelled = |path: &Path| path.to_str().expect("a path").to_owned();
        let login = folder.path().join(STORED);
        std::fs::write(&login, LOGIN).expect("a stored login");
        let home = folder.path().join("home/state.vscdb");
        program(&node, &["keep", &spelled(&login), &spelled(&home), ""]);
        // Taken out the way the courier does it, into a folder the program makes itself.
        let taken = folder.path().join("capture/out").join(STORED);
        program(&node, &["take", &spelled(&home), &spelled(&taken)]);
        let mode = std::fs::metadata(&taken).expect("the login was taken out").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the token is the person's alone");
        assert_eq!(std::fs::read_to_string(&taken).expect("readable"), LOGIN);
    }

    #[test]
    fn taking_the_login_fails_when_it_is_not_there() {
        let script = capture_script();
        assert!(script.starts_with("set -e\nnode -e '"), "{script}");
        assert!(script.ends_with(&format!("' take '{HOME_DIR}/{DATABASE}' '{CAPTURE_DIR}/{STORED}'")), "{script}");
        assert_eq!(seen_command()[3..], ["seen".to_owned(), format!("{HOME_DIR}/{DATABASE}")]);
    }

    #[test]
    fn the_sign_in_window_has_a_home_of_its_own_and_opens_no_workspace() {
        let engine = Engine::new(EngineKind::Podman, "/usr/bin/podman");
        let profile = Profile {
            name: SafeName::parse("anti").expect("safe"),
            harness: crate::profile::HarnessKind::AntigravityIde,
            template: crate::profile::Template::Recommended,
            account: crate::profile::AccountKind::InApp,
            provider: None,
            assets: crate::profile::MountAccess::ReadOnly,
            network: crate::profile::NetworkMode::None,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        };
        let sign_in = SignIn::named(&profile.name).expect("a private folder");
        let display =
            Display { socket: PathBuf::from("/run/user/1000/wayland-1"), name: "wayland-1".to_owned(), device: None };
        let command = sign_in
            .open_command(&engine, &profile, HostUser::Ids { uid: 1000, gid: 1000 }, &display, None)
            .expect("a window harness opens a window");
        let words = words(&command);
        let spelled = words.join(" ");
        assert!(spelled.starts_with("run --detach --init --shm-size=1g --name qcode-signin-anti "), "{spelled}");
        assert!(spelled.contains("qcode-signin-anti:/home/qcode:rw"), "{spelled}");
        assert!(spelled.contains(&format!(":{}:rw", signin::OPEN_DIR)), "{spelled}");
        assert!(spelled.contains(&format!("BROWSER={}", signin::OPEN_PROGRAM)), "{spelled}");
        assert!(spelled.contains("WAYLAND_DISPLAY=wayland-1"), "{spelled}");
        // Signing in needs the network whatever the profile's own setting is.
        assert!(!spelled.contains("--network=none"), "{spelled}");
        assert!(!spelled.contains("/work"), "no workspace is mounted or opened: {spelled}");
        assert_eq!(
            words.iter().rev().take(2).collect::<Vec<_>>(),
            ["--ozone-platform=wayland", "/opt/antigravity-ide/antigravity-ide"]
        );
    }
}
