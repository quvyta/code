//! Every screen of QCode as text, at the two widths a wide terminal is read at, so that the
//! words standing on them can be counted and argued about.
//!
//! Nothing here starts a container, reaches the network, opens anything on a desktop or
//! reads a file of the person running it: the engine the screens ask questions of is a stand-in
//! script under a temporary folder that answers what a dump needs and fails everything else, the
//! providers screen is handed a web that answers from a string and a file of the dump's own, and
//! every store is under that same folder. Every name is invented, and the folders are named after
//! the screen rather than after the clock, so a dump written twice says the same thing both times.
//!
//! Where a screen can be reached by clicking or pressing, it is: the walk to it is the one a
//! person makes. Where a state cannot be made from on screen — a tab that has chosen a harness but
//! whose container was never started — it is built directly, and the comment at that place says so.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use qcode::base::Os;
use qcode::engine::{Container, ContainerState, Engine, EngineKind, HostUser};
use qcode::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};
use qcode::provider::{Answer, Ask, AskError, Key, ProviderEntry, ProviderKind, Providers, Tag, Web};
use qcode::service::units::ServiceHost;
use qcode::store::{
    Config, HostDirs, Platform, SetupStep, Store, WorkspaceFile, WorkspaceId, WorkspacePaths, WorkspaceProfile,
};
use qcode::ui::profiles::Msg as ProfilesMsg;
use qcode::ui::setup::gates::{EngineCheck, EngineProblem, Gates, LocationCheck};
use qcode::ui::setup::install::InstallHost;
use qcode::ui::workspace::{Choice, Msg as WorkspaceMsg, OpenWorkspace, Tab, WorkspaceScreen};
use qcode::{Msg, QCode};
use qframe::date::Date;
use qframe::env::{AssetDirs, Env};
use qframe::icons::GlyphMode;
use qframe::runtime::Harness;

/// The sizes the dumps are taken at: an ordinary wide terminal, and one wide enough for a screen to
/// spread across all of it.
const SIZES: [(u16, u16); 2] = [(120, 40), (200, 50)];

/// A theme of the framework's own, so a dump carries no colour decision of this machine's.
const THEME: &str = "monochrome";

/// A binary that is not there, for the one screen whose engine is never asked anything.
const NO_ENGINE: &str = "/nonexistent/qcode-screen-dump-engine";

/// The store every path on screen lives in: a made-up person's documents.
const FOLDER: &str = "/home/demo/Documents/Quvyta/Code";

/// The workspaces of the demo store, by display name.
const WORKSPACES: [&str; 3] = ["Weather Station", "Recipe Book", "Pixel Editor"];

/// The folder name the first of them has on disk, which is what the session and the store name it.
const WEATHER: &str = "weather-station";

/// Not anyone's key: the characters say so.
const MADE_UP_KEY: &str = "not-a-real-key-0000-wxyz";

/// One screen's text as a person sees it: the name the dump is written under, and the screen.
type Dump = (&'static str, String);

/// A folder of this machine's temporary directory holding one screen's store, removed when that
/// screen is done. Nothing in it is ever shown under a name that changes from one run to the next.
struct Demo(PathBuf);

impl Demo {
    fn new(screen: &str) -> Self {
        let path = std::env::temp_dir().join("qcode-screen-dump").join(screen);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a screen's folder");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A harness over `app` with QCode's own text and keys, in English, with Unicode glyphs, one theme
/// of the framework's own and no motion, so nothing of the machine's own settings reaches a dump.
fn harness(app: QCode, size: (u16, u16)) -> Harness<QCode> {
    let dirs =
        AssetDirs { locale_sources: qcode::locales(), keymap_source: Some(qcode::keymap()), ..AssetDirs::default() };
    let env = Env::load(&dirs).expect("the built-in files load");
    let mut harness = Harness::with_env(app, env, size.0, size.1);
    harness.set_locale("en").set_theme(THEME).set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness.render();
    harness
}

/// Renders until `text` stands on the screen, which a reading from the disk or the stand-in engine
/// puts there in its own time.
fn wait_for(harness: &mut Harness<QCode>, text: &str) {
    let started = Instant::now();
    while !harness.render().screen().contains(text) {
        assert!(started.elapsed() < Duration::from_secs(10), "`{text}` never came:\n{}", harness.screen());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A day of 2026, so every date a dump carries is one anybody could read.
fn day(month: u8, day: u8) -> Date {
    Date::new(2026, month, day).expect("a day the calendar has")
}

/// A settings file of a finished setup on Podman, whose store is `store` and whose workspaces
/// opened before are `recent`.
fn config(store: &Path, recent: &[&str]) -> Config {
    let list = recent.iter().map(|id| format!("\"{id}\"")).collect::<Vec<_>>().join(", ");
    let text = format!(
        "language = \"en\"\n\n[setup]\ncompleted = true\nstep = \"location\"\n\n[engine]\nkind = \"podman\"\n\n\
         [folder]\npath = \"{}\"\n\n[workspaces]\nrecent = [{list}]\n",
        store.display()
    );
    Config::parse_str("code.conf", &text)
}

/// The application over the demo's settings, on a Linux machine whose home is the demo's own folder
/// and whose every gate holds. `engine` is the container engine it holds, so a screen that asks it
/// something really asks.
///
/// The providers file is the demo's own and the web answers from a string, because a screen left to
/// find them would read the file and reach the address of the person running the test.
fn application(demo: &Demo, recent: &[&str], engine: Option<Engine>) -> QCode {
    let documents = demo.path().join("home").join("Documents");
    let dirs = HostDirs { store: Some(documents.join("Quvyta").join("Code")), documents: Some(documents) };
    let host = InstallHost::read(Platform::Linux, Some("ID=arch\n"), |tool| tool == "paru");
    let gates = Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Usable };
    // The providers file is the demo's own and the web answers from a string, because a screen left
    // to find them would read the file of the person running this and reach the address on it. The
    // background service's row is one the settings screen shows on a machine of this kind; it is
    // read from the disk and never installed, so asking for it changes nothing.
    QCode::new(config(&demo.path().join("store"), recent), dirs, host, &gates, engine, None, None)
        .with_providers(Some(providers_file(demo)), no_web())
        .with_service(ServiceHost::detect(), Platform::host())
}

/// The providers file of `demo`, which every screen reads instead of the person's own.
fn providers_file(demo: &Demo) -> PathBuf {
    demo.path().join("providers.toml")
}

/// A web that reaches nothing at all: a screen that sent a request by accident would be told so
/// rather than quietly going out over the network.
fn no_web() -> Web {
    Web::new(|ask: &Ask| {
        Err(AskError::Unreachable { url: ask.url.clone(), reason: "no dump reaches the network".to_owned() })
    })
}

/// A web that answers the three questions a model is asked, out of a string: what the server offers,
/// what one model claims for itself, and how much room this server really gives. Nothing leaves the
/// machine, and the room it gives is far smaller than the model claims — which is the one thing the
/// providers page exists to say.
fn canned_web() -> Web {
    Web::new(|ask: &Ask| {
        let body = if ask.url.ends_with("/api/tags") {
            r#"{"models":[{"name":"qwen3.8:latest"},{"name":"qwen3.8-32k:latest"}]}"#
        } else if ask.url.ends_with("/api/show") {
            r#"{"model_info":{"qwen35.context_length":262144}}"#
        } else if ask.url.ends_with("/v1/messages") {
            r#"{"usage":{"input_tokens":3010}}"#
        } else {
            return Err(AskError::Unreachable { url: ask.url.clone(), reason: "no route to host".to_owned() });
        };
        Ok(Answer { status: 200, body: body.to_owned() })
    })
}

/// The stand-in engine under `demo`: a script that answers the two questions the screens ask and
/// nothing else. `volume ls` names the logins of the profiles in `signed_in` and of no others, so
/// the sign-in column of a list is filled in; `image inspect` fails the way a removed image does,
/// so nothing claims an image that was never built. No command it is given starts, builds or
/// reaches anything.
fn stand_in_engine(demo: &Demo, signed_in: &[&str]) -> Engine {
    let binary = demo.path().join("engine");
    let volumes = signed_in.iter().map(|name| format!("qcode-cred-{name}")).collect::<Vec<_>>().join(" ");
    let script = format!(
        "#!/bin/sh\n\
         case \"$1 $2\" in\n\
         'volume ls') printf '%s\\n' {volumes} ;;\n\
         'image inspect') echo 'Error: no such image' >&2; exit 125 ;;\n\
         *) exit 0 ;;\n\
         esac\n"
    );
    fs::write(&binary, script).expect("the stand-in engine is written");
    fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("it can be run");
    Engine::new(EngineKind::Podman, &binary)
}

/// The demo store's profiles: Claude Code on a plan, Gemini CLI on a key, and opencode on its own
/// free models, which signs in to nothing.
fn profiles() -> Vec<Profile> {
    let one = |name: &str, harness, account| Profile {
        name: SafeName::parse(name).expect("a usable profile name"),
        harness,
        template: Template::Recommended,
        account,
        provider: None,
        assets: MountAccess::ReadOnly,
        network: NetworkMode::Full,
        without: Vec::new(),
        os: Os::Debian,
    };
    vec![
        one("claude-main", HarnessKind::ClaudeCode, AccountKind::Subscription),
        one("gemini-work", HarnessKind::GeminiCli, AccountKind::ApiKey),
        one("open-free", HarnessKind::OpenCode, AccountKind::Free),
    ]
}

/// The profiles written into the demo store, so the profiles screen has a store to read.
fn write_profiles(store: &Store) {
    for profile in profiles() {
        store.write_profile(&profile).expect("the store takes a profile");
    }
}

/// One workspace of the demo store as the rail opens it, working in `code` where the file tree is
/// read from a folder of the dump's own.
fn open_workspace(name: &str, carried: &[&str], code: PathBuf) -> OpenWorkspace {
    let id = WorkspaceId::from_display_name(name).expect("a usable workspace name");
    let root = Path::new(FOLDER).join(id.as_str());
    let mut file = WorkspaceFile::new(id, name, day(9, 2));
    file.profiles =
        carried.iter().map(|name| WorkspaceProfile { name: (*name).to_owned(), added: Some(day(9, 2)) }).collect();
    let paths = WorkspacePaths {
        file: root.join("workspace.qcode"),
        code,
        assets: root.join("Assets"),
        harness: root.join("Containers").join("Harness"),
        root,
    };
    OpenWorkspace::new(&file, paths, profiles())
}

/// The weather station's own code folder, with the few files its tree shows.
fn weather_code(demo: &Demo) -> PathBuf {
    let root = demo.path().join("code");
    for (name, text) in [
        ("Cargo.toml", "[package]\nname = \"weather-station\"\nversion = \"0.3.0\"\nedition = \"2024\"\n"),
        ("README.md", "# Weather Station\n\nReads the garden sensors and draws the week.\n"),
        ("src/main.rs", "fn main() {}\n"),
        ("src/sensor.rs", "fn start() {}\n"),
    ] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().expect("a folder above")).expect("a folder of the workspace's own");
        fs::write(path, text).expect("a file of the workspace's own");
    }
    root
}

/// The containers of the weather station as the engine would list them.
fn containers() -> Vec<Container> {
    let one = |name: &str, state| Container { name: format!("qcode-{WEATHER}-{name}"), state };
    vec![one("base", ContainerState::Running), one("claude-main", ContainerState::Exited)]
}

/// The person the containers run as: two numbers, and never this machine's own account.
fn host_user() -> HostUser {
    HostUser::Ids { uid: 1000, gid: 1000 }
}

/// The home screen: the logo over the menu, with the two workspaces that were open last time.
fn home(size: (u16, u16)) -> Vec<Dump> {
    let demo = Demo::new("home");
    let session = demo.path().join("session.toml");
    let text = format!("[[workspace]]\nid = \"{WEATHER}\"\n\n[[workspace]]\nid = \"recipe-book\"\n");
    fs::write(&session, text).expect("a session file");
    let app = application(&demo, &[], Some(stand_in_engine(&demo, &[]))).with_session(Some(session));
    vec![("home", harness(app, size).screen())]
}

/// The first-start wizard, one dump for each of its three steps.
///
/// The gates of a machine decide which step the wizard opens on, and the rule is that it never
/// opens on a step whose gate holds: so the engine step is only ever met on a machine whose engine
/// does not work, and the step after it on one whose folder has not answered yet. Every dump is
/// drawn settled rather than with a spinner on it.
fn setup(size: (u16, u16)) -> Vec<Dump> {
    let mut out = Vec::new();
    for (name, step, gates) in [
        // Nothing has been settled at all: this is the screen a person meets first.
        (
            "setup-language",
            SetupStep::Language,
            Gates { language: false, engine: EngineCheck::Working, location: LocationCheck::Usable },
        ),
        // A machine with neither engine, which is what the engine step is for.
        (
            "setup-engine",
            SetupStep::Engine,
            Gates {
                language: true,
                engine: EngineCheck::Broken(EngineProblem::NotInstalled),
                location: LocationCheck::Usable,
            },
        ),
        // Only the folder is unanswered, which is where a first start reaches the last step. The
        // check really runs, on the dump's own folder, and its own answer is what the page shows.
        (
            "setup-location",
            SetupStep::Location,
            Gates { language: true, engine: EngineCheck::Working, location: LocationCheck::Unknown },
        ),
    ] {
        let demo = Demo::new("setup");
        let mut harness = harness(wizard(&demo, step, &gates), size);
        if step == SetupStep::Location {
            wait_for(&mut harness, "The folder is ready");
        }
        out.push((name, harness.screen()));
    }
    out
}

/// The wizard of a machine whose gates open it on `step`.
///
/// `step` says which one that is meant to be; the gates decide in fact, which is the rule the
/// wizard is built on, and `Some` is only what tells the application there is a wizard to show.
fn wizard(demo: &Demo, step: SetupStep, gates: &Gates) -> QCode {
    let documents = demo.path().join("home").join("Documents");
    let dirs = HostDirs { store: Some(documents.join("Quvyta").join("Code")), documents: Some(documents) };
    let host = InstallHost::read(Platform::Linux, Some("ID=arch\n"), |tool| tool == "paru");
    let text = format!("language = \"en\"\n\n[setup]\nstep = \"{}\"\n", step.key());
    QCode::new(Config::parse_str("code.conf", &text), dirs, host, gates, None, None, Some(step))
}

/// The workspaces of the store, and the dialog that makes a new one, both reached the way they are
/// reached: the home screen's row, then the first row of the list.
fn workspaces(size: (u16, u16)) -> Vec<Dump> {
    let demo = Demo::new("workspaces");
    let store = Store::new(demo.path().join("store"));
    for (name, made) in WORKSPACES.into_iter().zip([day(9, 2), day(8, 21), day(7, 30)]) {
        store.create_workspace(name, made).expect("the store takes a workspace");
    }
    let mut harness = harness(application(&demo, &[WEATHER], Some(stand_in_engine(&demo, &[]))), size);
    harness.click_text("Workspaces");
    wait_for(&mut harness, "Weather Station");
    let list = harness.screen();
    // The form is drawn as it opens, over an empty workspace. No folder is chosen for it, which is
    // what keeps the file browser — and with it the home folder of whoever runs this — off the
    // screen; the words under the folder source are on the same page, one row away.
    harness.click_text("New workspace");
    vec![("workspaces", list), ("workspaces-new", harness.screen())]
}

/// The workspace screen: the weather station open with one Claude Code tab, whose harness was chosen
/// and whose container was never started.
///
/// The tab is built directly rather than clicked, because choosing a harness on a blank tab asks the
/// engine to bring a container up in the same step; the work that asks is dropped, so the tab stands
/// where a person would meet it before its container is there.
fn workspace(size: (u16, u16)) -> Vec<Dump> {
    let demo = Demo::new("workspace");
    let rail = vec![open_workspace(WORKSPACES[0], &["claude-main"], weather_code(&demo))];
    let mut screen = WorkspaceScreen::new(Some(Engine::new(EngineKind::Podman, NO_ENGINE)), host_user(), rail);
    drop(qcode::ui::workspace::update(&mut screen, WorkspaceMsg::NewTab));
    let tab = screen.workspace().and_then(OpenWorkspace::active_tab).map(Tab::key).expect("a blank tab");
    drop(qcode::ui::workspace::update(
        &mut screen,
        WorkspaceMsg::Choose(tab, Choice::NewChat("claude-main".to_owned())),
    ));
    let app = application(&demo, &[WEATHER], None).with_workspace(screen);
    let mut harness = harness(app, size);
    harness.send(Msg::Workspace(WorkspaceMsg::OpenWorkspace(0)));
    harness.send(Msg::Workspace(WorkspaceMsg::ContainersRead(WEATHER.to_owned(), Ok(containers()))));
    vec![("workspace", harness.screen())]
}

/// The profiles of the store, and every step of the wizard that makes a new one.
///
/// The wizard is walked with Next rather than opened page by page, so the steps are the seven a
/// person walks. The build the image step starts by itself is run by the stand-in engine, which
/// builds nothing; its answer is what carries the walk on to the sign-in step.
fn profiles_and_wizard(size: (u16, u16)) -> Vec<Dump> {
    let listed = {
        let demo = Demo::new("profiles");
        let store = Store::new(demo.path().join("store"));
        write_profiles(&store);
        let app = application(&demo, &[WEATHER], Some(stand_in_engine(&demo, &["claude-main"])));
        let mut harness = harness(app, size);
        harness.click_text("Profiles");
        wait_for(&mut harness, "No image");
        ("profiles", harness.screen())
    };
    let walked = {
        let demo = Demo::new("wizard");
        let app = application(&demo, &[], Some(stand_in_engine(&demo, &[])));
        let mut harness = harness(app, size);
        harness.click_text("Profiles");
        // A store without profiles: the way to a new one is the button on the empty page, which is
        // also the only "New profile" on it, so a click reaches the wizard rather than a row.
        wait_for(&mut harness, "New profile");
        harness.click_text("New profile");
        let mut out = Vec::new();
        for (name, said) in [
            ("wizard-harness", "Harness"),
            ("wizard-system", "The system this profile"),
            ("wizard-template", "The parts the image carries"),
            ("wizard-account", "What this profile signs in with"),
            ("wizard-permissions", "may see and reach"),
            // The image page builds by itself the moment it is reached, so what it is drawn with is
            // what the stand-in engine answered: an image that is there. The build never runs.
            ("wizard-image", "The image is built"),
        ] {
            wait_for(&mut harness, said);
            out.push((name, harness.screen()));
            harness.send(Msg::Profiles(ProfilesMsg::Next));
        }
        harness.send(Msg::Profiles(ProfilesMsg::Next));
        wait_for(&mut harness, "Open the sign-in terminal");
        out.push(("wizard-login", harness.screen()));
        out
    };
    let mut out = vec![listed];
    out.extend(walked);
    out
}

/// The providers of the demo file, the dialog that adds one, and what the page says about a model
/// once it has been asked about and measured. All three are reached the way they are.
fn providers(size: (u16, u16)) -> Vec<Dump> {
    let listed = {
        let demo = Demo::new("providers");
        write_demo_providers(&demo);
        let mut harness = harness(application(&demo, &[], Some(stand_in_engine(&demo, &[]))), size);
        harness.click_text("Providers");
        wait_for(&mut harness, "yol");
        ("providers", harness.screen())
    };
    let measured = {
        let demo = Demo::new("providers-window");
        write_demo_providers(&demo);
        let app = application(&demo, &[], Some(stand_in_engine(&demo, &[])));
        // The same page, with a web that answers from a string: what the provider offers, what a
        // model claims, and the room this server really gives. Nothing is reached.
        let app = app.with_providers(Some(providers_file(&demo)), canned_web());
        let mut harness = harness(app, size);
        harness.click_text("Providers");
        wait_for(&mut harness, "Nothing has been asked");
        harness.click_text("Ask what it offers");
        wait_for(&mut harness, "the model says");
        harness.click_text("Measure the real window");
        wait_for(&mut harness, "too small for a coding agent");
        ("providers-window", harness.screen())
    };
    let form = {
        let demo = Demo::new("provider-form");
        write_demo_providers(&demo);
        let mut harness = harness(application(&demo, &[], Some(stand_in_engine(&demo, &[]))), size);
        harness.click_text("Providers");
        wait_for(&mut harness, "yol");
        // The dialog is drawn as it opens: Ollama is the kind it offers first and needs nothing but
        // an address, so what stands on it is the kind, the tag, the address, two hints and the
        // two buttons.
        harness.click_text("Add a provider");
        ("provider-form", harness.screen())
    };
    vec![listed, measured, form]
}

/// The two providers of the demo file: an ollama of the person's own, and a ready-made service
/// reached with a made-up key.
fn write_demo_providers(demo: &Demo) {
    let path = providers_file(demo);
    let mut file = Providers::in_memory().at(&path);
    let ollama =
        ProviderEntry::new(Tag::parse("ev").expect("a tag"), ProviderKind::Ollama, "http://192.168.122.1:11434");
    let mut ready =
        ProviderEntry::new(Tag::parse("yol").expect("a tag"), ProviderKind::OpenRouter, "https://openrouter.ai");
    ready.key = Key::new(MADE_UP_KEY);
    file.add(ollama).expect("the first tag is free");
    file.add(ready).expect("the second tag is free");
    file.save().expect("the providers file is written");
}

/// The settings screen, reached by the menu row as a person reaches it.
fn settings(size: (u16, u16)) -> Vec<Dump> {
    let demo = Demo::new("settings");
    let store = Store::new(demo.path().join("store"));
    write_profiles(&store);
    let app = application(&demo, &[], Some(stand_in_engine(&demo, &["claude-main", "gemini-work"])));
    let mut harness = harness(app, size);
    harness.click_text("Settings");
    // The logins are read from the engine on a task of their own, so the section stands before the
    // rows in it; the rows are what says the engine answered.
    wait_for(&mut harness, "login kept");
    vec![("settings", harness.screen())]
}

/// The list of every key, opened from the foot of the home screen the way a person opens it.
fn keys(size: (u16, u16)) -> Vec<Dump> {
    let demo = Demo::new("keys");
    let mut harness = harness(application(&demo, &[], Some(stand_in_engine(&demo, &[]))), size);
    harness.click_text("Keys");
    vec![("keys", harness.screen())]
}

/// Every screen of the application, as the text a person sees at `size`.
fn screens(size: (u16, u16)) -> Vec<Dump> {
    [
        home(size),
        setup(size),
        workspaces(size),
        workspace(size),
        profiles_and_wizard(size),
        providers(size),
        settings(size),
        keys(size),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Writes the text of every screen at every size into the folder `QCODE_SCREEN_DUMP` names, one file
/// per screen and width.
///
/// Ignored in the gate and in a plain `cargo test`, so no dump changes unless it is asked for; the
/// files are read before they are committed.
#[test]
#[ignore = "writes the text of every screen: QCODE_SCREEN_DUMP=<folder> cargo test --test screen_dump -- --ignored"]
fn screen_dump() {
    let Ok(dir) = std::env::var("QCODE_SCREEN_DUMP") else {
        eprintln!("QCODE_SCREEN_DUMP names the folder the dumps go into; nothing was written");
        return;
    };
    let dir = PathBuf::from(dir);
    fs::create_dir_all(&dir).expect("the dumps' folder");
    for (width, height) in SIZES {
        for (name, text) in screens((width, height)) {
            let path = dir.join(format!("{name}-{width}.txt"));
            fs::write(&path, text).unwrap_or_else(|error| panic!("{} is written: {error}", path.display()));
        }
    }
}
