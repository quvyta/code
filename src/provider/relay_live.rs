//! The whole road, in a real container: a harness of a provider profile asks a model service on
//! this machine's network through the relay, and answers — while its container has no network at
//! all and the provider's key never leaves this process.
//!
//! This is the one thing no test with a stand-in can say. [`super::relay`]'s own tests drive the
//! socket and the framing; what they cannot prove is that Claude Code, started the way QCode
//! starts it, really speaks to the relay and really gets a model's answer back. That is what is
//! proven here, with the product's own builders: the container is [`ContainerPlan::profile`]'s,
//! the program is [`relay::wrapping`] over the harness's own command line, and the environment is
//! [`ProviderChoice::environment`]. Nothing here writes its own idea of any of them, because a
//! copy that drifts from the product proves nothing about the product.
//!
//! Every test is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`. They need a
//! container engine, the network once to build each image, and a model service reachable from
//! this machine; `QCODE_CONTAINER_ENGINE=podman` (or `docker`) runs them on one engine alone:
//!
//! ```text
//! QCODE_PROVIDER_URL=http://192.168.122.1:11434 QCODE_PROVIDER_MODEL=qwen3.5-256k \
//!   QCODE_CONTAINER_TESTS=1 cargo test provider::relay_live -- --ignored --test-threads=1
//! ```
//!
//! The OpenRouter test reads its key from `~/.config/quvyta/openrouter-key` when it runs, and
//! says it was skipped when there is no such file. The key is never written anywhere by it,
//! never put on a command line, and never formatted into a message: every assertion that looks
//! for it names where it looked, not what it looked for. `QCODE_OPENROUTER_MODEL` names the model;
//! without it, a free one that was seen to finish a turn with a tool in it.
//!
//! A local model answers slowly: a prompt of a few thousand tokens took half a minute on the
//! machine this was written on, and a larger model four times that. The waits here are therefore
//! generous rather than tight — a short limit would report a working provider as broken.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::relay::{self, Listener, Upstream};
use super::{Key, ProviderEntry, ProviderKind, Tag};
use crate::engine::run::capture;
use crate::engine::{Engine, EngineKind, Exec, HostUser, detect};
use crate::profile::harness_live::{build, clear};
use crate::profile::identity::Home;
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, ProviderChoice, SafeName, Template};
use crate::store::{WorkspaceId, WorkspacePaths};
use crate::ui::workspace::{ContainerPlan, ensure_running};

/// The workspace these containers belong to, which nobody has.
const WORKSPACE: &str = "relaytest";

/// The tag the provider is written down under here.
const TAG: &str = "olcum";

/// Where the owner keeps the OpenRouter key these tests may use, outside every repository.
const OPENROUTER_KEY_FILE: &str = ".config/quvyta/openrouter-key";

/// The free OpenRouter model asked when `QCODE_OPENROUTER_MODEL` names none: of the free models
/// tried on 2026-09-21 it answered plainly and called a tool, through Claude Code, in seconds.
const OPENROUTER_MODEL: &str = "nex-agi/nex-n2.5-mini:free";

/// The engines installed on this machine, or nothing at all when the tests are switched off.
/// `QCODE_CONTAINER_ENGINE` keeps one of them, because every image here is built once per engine
/// and a local model answers slowly enough that one engine is often all a run can afford.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let only = std::env::var("QCODE_CONTAINER_ENGINE").ok();
    let found: Vec<Engine> = [EngineKind::Podman, EngineKind::Docker]
        .into_iter()
        .filter(|kind| only.as_deref().is_none_or(|only| format!("{kind:?}").eq_ignore_ascii_case(only)))
        .filter_map(|kind| detect(kind).ok())
        .collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// The provider this run asks, and the model it asks for.
fn provider() -> (String, String) {
    let base = std::env::var("QCODE_PROVIDER_URL").unwrap_or_else(|_| "http://192.168.122.1:11434".to_owned());
    let model = std::env::var("QCODE_PROVIDER_MODEL").unwrap_or_else(|_| "qwen3.5-256k".to_owned());
    (base, model)
}

/// The OpenRouter key, read from the owner's file into this process alone, or `None` when there
/// is no such file. Held as a [`Key`], whose `Debug` is its last four characters, so no message
/// here can print it by accident.
fn openrouter_key() -> Option<Key> {
    let home = std::env::var_os("HOME")?;
    let text = std::fs::read_to_string(PathBuf::from(home).join(OPENROUTER_KEY_FILE)).ok()?;
    Key::new(&text)
}

/// A workspace folder of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-relaylive-{name}-{stamp}"));
        std::fs::create_dir_all(path.join("Work")).expect("a workspace folder");
        std::fs::create_dir_all(path.join("Assets")).expect("an assets folder");
        Self(path)
    }

    fn paths(&self) -> WorkspacePaths {
        WorkspacePaths {
            root: self.0.clone(),
            file: self.0.join("workspace.qcode"),
            code: self.0.join("Work"),
            assets: self.0.join("Assets"),
            harness: self.0.join("Containers").join("Harness"),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A profile of `harness`, signed in with a provider of the person's own, in a container with no
/// network. The network is off on purpose — everything the harness reaches, it reaches through
/// the relay, so a container that could have gone around it could not have.
fn profile(harness: HarnessKind, model: &str) -> Profile {
    on(harness, model, crate::base::Os::Debian)
}

/// The same profile with its image built on `os`, named after the system too, so the images of
/// two systems never stand in for each other.
fn on(harness: HarnessKind, model: &str, os: crate::base::Os) -> Profile {
    let name = match os {
        crate::base::Os::Debian => format!("relaytest-{}", harness.record().id),
        _ => format!("relaytest-{}-{}", harness.record().id, os.id()),
    };
    Profile {
        name: SafeName::parse(&name).expect("the name is safe"),
        harness,
        template: Template::Recommended,
        account: AccountKind::Provider,
        provider: Some(ProviderChoice { tag: TAG.to_owned(), model: model.to_owned() }),
        assets: MountAccess::ReadOnly,
        network: NetworkMode::None,
        without: Vec::new(),
        os,
    }
}

/// Runs `script` with a shell in `container` and answers what it printed, or everything the
/// shell said when it failed.
fn run(engine: &Engine, container: &str, script: &str) -> Result<String, String> {
    capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] }))
        .map_err(|error| format!("{error:?}"))
}

/// The one shell word for `text`, so a prompt with quotes in it is still one argument.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// One provider profile's container, up and pointed at a relay of this process, with everything
/// it made taken away again when it goes — also when an assertion stops the test halfway, so a
/// failing run leaves no container, volume or image behind.
struct Road {
    engine: Engine,
    profile: Profile,
    plan: ContainerPlan,
    home: Home,
    token: String,
    events: Arc<Mutex<Vec<super::RelayEvent>>>,
    listener: Option<Listener>,
    scratch: Scratch,
}

impl Road {
    /// Builds `profile`'s image on `engine`, opens a relay that answers the tab's token with
    /// `entry` and carries requests through `upstream`, and brings the container up.
    fn open(engine: Engine, profile: &Profile, entry: ProviderEntry, upstream: Upstream) -> Self {
        let kind = engine.kind();
        let scratch = Scratch::new(profile.harness.record().id);
        let paths = scratch.paths();
        let workspace = WorkspaceId::parse(WORKSPACE).expect("a workspace id");
        let plan = ContainerPlan::profile(&workspace, &paths, profile);
        let home = Home::new(profile.name.clone(), workspace);
        let _ = capture(&engine.remove_container(&plan.name));
        let _ = capture(&engine.remove_volume(&home.volume()));
        build(&engine, profile);

        // QCode's own side: the socket the container reaches, the entry the token resolves to,
        // and the key added on the way out.
        let token = crate::bridge::token();
        let mine = token.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let listener = Listener::open(
            &paths.mcp(),
            move |asked| (asked == mine).then(|| entry.clone()),
            upstream,
            move |event| seen.lock().expect("the events are not poisoned").push(event),
        )
        .expect("the workspace's relay socket opens");
        assert!(listener.socket().exists(), "{kind:?}: the socket is there for the container to reach");
        let road =
            Self { engine, profile: profile.clone(), plan, home, token, events, listener: Some(listener), scratch };
        let user = HostUser::current().expect("the current user");
        ensure_running(&road.engine, &road.plan, user).unwrap_or_else(|failure| panic!("{kind:?}: {failure:?}"));
        road
    }

    /// The workspace's `Work` folder on this machine, which is `/work` in the container.
    fn work(&self) -> PathBuf {
        self.scratch.paths().code
    }

    /// Runs `harness_arguments` after the profile's harness inside the container, started
    /// exactly as QCode starts a tab: the tab's environment, then the relay with the harness
    /// after it. `line` is the harness's own command line, as the product builds it.
    fn ask(&self, line: &[String]) -> Result<String, String> {
        let mut words = vec!["env".to_owned(), format!("{}={}", crate::bridge::TOKEN_VARIABLE, self.token)];
        let choice = self.profile.provider.as_ref().expect("a provider profile");
        // Nothing was measured here: this run is about the road, not the window, and a number
        // invented for it would be the very thing the measurement exists to avoid.
        for (name, value) in choice.environment(self.profile.harness, &self.token, None) {
            words.push(format!("{name}={}", quoted(&value)));
        }
        words.extend(relay::wrapping(line).into_iter().map(|word| quoted(&word)));
        run(&self.engine, &self.plan.name, &words.join(" "))
    }

    /// Claude Code asked `prompt` once, the way `claude -p` answers without a terminal.
    fn ask_claude(&self, prompt: &str) -> Result<String, String> {
        let mut line = HarnessKind::ClaudeCode.command_line(None);
        line.extend(["-p".to_owned(), prompt.to_owned()]);
        self.ask(&line)
    }

    /// opencode asked `prompt` once, by `opencode run`, the form of the same program that answers
    /// without a terminal and ends; the tab runs its interface instead, with the very same
    /// environment and relay in front of it.
    fn ask_opencode(&self, prompt: &str) -> Result<String, String> {
        let command = HarnessKind::OpenCode.record().command.to_owned();
        self.ask(&[command, "run".to_owned(), prompt.to_owned()])
    }

    /// The harness of this road asked `prompt` once.
    fn ask_once(&self, prompt: &str) -> Result<String, String> {
        match self.profile.harness {
            HarnessKind::OpenCode => self.ask_opencode(prompt),
            _ => self.ask_claude(prompt),
        }
    }

    /// Gathers everything inside the container, after leaving a word in the home folder and in
    /// `/tmp` to show the search really reaches the places it names — a search that read nothing
    /// would find no key either — and asserts `key` is in none of it.
    fn assert_no_trace_of(&self, key: &Key) {
        let kind = self.engine.kind();
        let canary = format!("qcode-canary-{}", self.token);
        run(&self.engine, &self.plan.name, &format!("echo {canary} > /tmp/canary; echo {canary} > \"$HOME/.canary\""))
            .unwrap_or_else(|trouble| panic!("{kind:?}: the canary could not be left: {trouble}"));
        let inside = self.everything_inside();
        let files = &inside.last().expect("the files were read").1;
        assert_eq!(files.matches(&canary).count(), 2, "{kind:?}: the search reads the home folder and /tmp");
        for (what, found) in inside {
            assert!(!found.is_empty(), "{kind:?}: {what} was read and is not empty");
            assert!(!found.contains(key.expose()), "{kind:?}: the key was found in {what}");
            println!("{kind:?}: no trace of the key in {what} ({} bytes read)", found.len());
        }
    }

    /// How large the profile's image is, as the engine reports it, in bytes.
    fn image_bytes(&self) -> String {
        let command = crate::engine::EngineCommand {
            program: self.engine.bin().to_path_buf(),
            args: ["image", "inspect", "--format", "{{.Size}}", &self.profile.image()]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect(),
        };
        capture(&command).map(|size| size.trim().to_owned()).unwrap_or_default()
    }

    /// What the relay was told so far.
    fn events(&self) -> Vec<super::RelayEvent> {
        self.events.lock().expect("the events are not poisoned").clone()
    }

    /// Everything inside the container a key could have been left in, gathered into this
    /// process: the environment of every process running there and of a fresh shell, and every
    /// file of the home folder, the workspace, `/run` and `/tmp`. Nothing is searched inside the
    /// container, because searching there would mean carrying the key in.
    fn everything_inside(&self) -> Vec<(&'static str, String)> {
        let places: [(&'static str, &str); 3] = [
            ("the environment of a shell", "env"),
            ("the environment of every process", "cat /proc/[0-9]*/environ 2>/dev/null | tr '\\0' '\\n'; true"),
            (
                "the files of the home folder, /work, /run and /tmp",
                "find \"$HOME\" /work /run /tmp -xdev -type f -size -20M -print0 2>/dev/null \
                 | xargs -0 -r cat 2>/dev/null; true",
            ),
        ];
        places
            .into_iter()
            .map(|(what, script)| {
                // What the shell said is not quoted when it fails: it is the very text being
                // searched for the key.
                let found =
                    run(&self.engine, &self.plan.name, script).unwrap_or_else(|_| panic!("{what} could not be read"));
                (what, found)
            })
            .collect()
    }
}

impl Drop for Road {
    fn drop(&mut self) {
        drop(self.listener.take());
        clear(&self.engine, &self.profile, &self.plan.name);
        let _ = capture(&self.engine.remove_volume(&self.home.volume()));
    }
}

/// `text` with `key` taken out, for a message that quotes what a harness said: nothing a model
/// answered is trusted not to hold it.
fn without(text: &str, key: &Key) -> String {
    text.replace(key.expose(), "(the key)")
}

#[test]
#[ignore = "needs a container engine, the network once for the image, and a model service; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_answers_and_uses_a_tool_through_a_provider_of_this_machine() {
    let (base, model) = provider();
    let profile = profile(HarnessKind::ClaudeCode, &model);
    let entry = ProviderEntry::new(Tag::parse(TAG).expect("a tag"), ProviderKind::Ollama, &base);
    for engine in engines() {
        let kind = engine.kind();
        // There is no key for an ollama server, which is why this one can be run by anybody with
        // a model on their own network.
        let road = Road::open(engine, &profile, entry.clone(), Upstream::network());
        std::fs::write(road.work().join("parola.txt"), "kirlangic\n").expect("a file for the harness to read");

        // What a person types first: a question with one right answer no container could have
        // made up on its own.
        let said = road
            .ask_claude("Reply with just the digits: what is 2+2?")
            .unwrap_or_else(|trouble| panic!("{kind:?}: the harness said nothing: {trouble}"));
        assert!(said.contains('4'), "{kind:?}: the model answered through the relay: {said}");

        // And the thing a coding agent is for: reading a file of the workspace and saying what is
        // in it. Only a model that can call a tool and read its result can answer this.
        let said = road
            .ask_claude("Read the file parola.txt in this folder and reply with only the word inside it.")
            .unwrap_or_else(|trouble| panic!("{kind:?}: the harness said nothing: {trouble}"));
        assert!(said.contains("kirlangic"), "{kind:?}: the agent read the file through its tools: {said}");

        let events = road.events();
        assert!(
            events.iter().any(|event| matches!(event, super::RelayEvent::Forwarded { .. })),
            "{kind:?}: QCode carried the requests itself: {events:?}"
        );
        assert!(
            !events.iter().any(|event| matches!(event, super::RelayEvent::UnknownTab | super::RelayEvent::Malformed)),
            "{kind:?}: nothing knocked that QCode did not know: {events:?}"
        );
    }
}

/// `harness` on OpenRouter, the provider that needs a key, in a container that has no network
/// and never holds that key: QCode adds it on the way out. This is the one thing the relay's own
/// tests can only say with a made-up key and a stand-in: that the real service accepts what
/// QCode sends it, at the address QCode sends it to, and that the harness gets a turn with tools
/// in it back. Then everything inside the container is gathered here and searched for the key.
fn works_on_openrouter_and_the_key_never_enters_its_container(harness: HarnessKind) {
    let Some(key) = openrouter_key() else {
        println!("skipped: there is no key at ~/{OPENROUTER_KEY_FILE}");
        return;
    };
    let model = std::env::var("QCODE_OPENROUTER_MODEL").unwrap_or_else(|_| OPENROUTER_MODEL.to_owned());
    let profile = profile(harness, &model);
    let mut entry = ProviderEntry::new(
        Tag::parse(TAG).expect("a tag"),
        ProviderKind::OpenRouter,
        ProviderKind::OpenRouter.suggested_base(),
    );
    entry.key = Some(key.clone());
    for engine in engines() {
        let kind = engine.kind();
        let road = Road::open(engine, &profile, entry.clone(), Upstream::network());
        std::fs::write(road.work().join("parola.txt"), "kirlangic\n").expect("a file for the harness to read");

        let started = std::time::Instant::now();
        let said = road
            .ask_once("Reply with just the digits: what is 2+2?")
            .unwrap_or_else(|trouble| panic!("{kind:?}: the harness said nothing: {}", without(&trouble, &key)));
        assert!(said.contains('4'), "{kind:?}: OpenRouter answered through the relay: {}", without(&said, &key));
        println!("{kind:?} {harness:?}: {model} answered 2+2 in {} s", started.elapsed().as_secs());

        // A turn with two tools in it: one reads a file of the workspace, one writes another. The
        // word read back is one no model could guess, and the file written is looked for on this
        // machine, where only a tool that really ran could have put it.
        let started = std::time::Instant::now();
        let said = road
            .ask_once(
                "Read the file parola.txt in this folder. Then create a file named cevap.txt in this \
                 folder whose only content is that same word. Reply with only the word.",
            )
            .unwrap_or_else(|trouble| panic!("{kind:?}: the harness said nothing: {}", without(&trouble, &key)));
        let written = std::fs::read_to_string(road.work().join("cevap.txt")).unwrap_or_default();
        assert!(
            written.contains("kirlangic"),
            "{kind:?}: the agent wrote the file with its tools; it said: {}",
            without(&said, &key)
        );
        println!("{kind:?} {harness:?}: {model} read one file and wrote another in {} s", started.elapsed().as_secs());

        let events = road.events();
        assert!(
            events.iter().any(|event| matches!(event, super::RelayEvent::Forwarded { status: 200, .. })),
            "{kind:?}: QCode carried the requests to OpenRouter and it answered: {events:?}"
        );
        // The one promise of the relay, checked where it would break: nowhere in the container.
        road.assert_no_trace_of(&key);
    }
}

#[test]
#[ignore = "needs a container engine, the network, and the owner's OpenRouter key; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_works_on_openrouter_and_the_key_never_enters_its_container() {
    works_on_openrouter_and_the_key_never_enters_its_container(HarnessKind::ClaudeCode);
}

#[test]
#[ignore = "needs a container engine, the network, and the owner's OpenRouter key; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_works_on_openrouter_and_the_key_never_enters_its_container() {
    works_on_openrouter_and_the_key_never_enters_its_container(HarnessKind::OpenCode);
}

/// opencode on a model service of this machine's network, through the relay, in a container
/// with no network: it answers, and it reads one file of the workspace and writes another with
/// its own tools. It speaks the OpenAI shape, so this is also the proof that the relay carries
/// that shape as well as Claude Code's.
#[test]
#[ignore = "needs a container engine, the network once for the image, and a model service; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_answers_and_uses_a_tool_through_a_provider_of_this_machine() {
    let (base, model) = provider();
    let profile = profile(HarnessKind::OpenCode, &model);
    let entry = ProviderEntry::new(Tag::parse(TAG).expect("a tag"), ProviderKind::Ollama, &base);
    for engine in engines() {
        let kind = engine.kind();
        let road = Road::open(engine, &profile, entry.clone(), Upstream::network());
        std::fs::write(road.work().join("parola.txt"), "kirlangic\n").expect("a file for the harness to read");

        let started = std::time::Instant::now();
        let said = road
            .ask_opencode("Reply with just the digits: what is 2+2?")
            .unwrap_or_else(|trouble| panic!("{kind:?}: opencode said nothing: {trouble}"));
        assert!(said.contains('4'), "{kind:?}: the model answered through the relay: {said}");
        println!("{kind:?}: opencode on {model} answered 2+2 in {} s", started.elapsed().as_secs());

        let started = std::time::Instant::now();
        let said = road
            .ask_opencode(
                "Read the file parola.txt in this folder. Then create a file named cevap.txt in this \
                 folder whose only content is that same word. Reply with only the word.",
            )
            .unwrap_or_else(|trouble| panic!("{kind:?}: opencode said nothing: {trouble}"));
        let written = std::fs::read_to_string(road.work().join("cevap.txt")).unwrap_or_default();
        assert!(written.contains("kirlangic"), "{kind:?}: opencode wrote the file with its tools; it said: {said}");
        println!("{kind:?}: opencode on {model} read one file and wrote another in {} s", started.elapsed().as_secs());

        let events = road.events();
        assert!(
            events.iter().any(|event| matches!(
                event,
                super::RelayEvent::Forwarded { path, status: 200, .. } if path.starts_with("/v1/chat/completions")
            )),
            "{kind:?}: QCode carried opencode's requests in its own shape: {events:?}"
        );
        assert!(
            !events.iter().any(|event| matches!(event, super::RelayEvent::NotAllowed { .. })),
            "{kind:?}: nothing opencode asked for was refused: {events:?}"
        );
    }
}

/// What a person sees in a Claude Code tab when the provider says too many requests: the relay
/// carries the status and the provider's own words through rather than turning them into a
/// failure of its own, so Claude Code can say what happened. The provider here is a stand-in
/// answering exactly what OpenRouter answered for a busy free model, so no key and no network is
/// needed.
#[test]
#[ignore = "needs a container engine and the network once for the image; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_hears_that_a_busy_provider_asked_it_to_wait() {
    const BUSY: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Provider returned error"},"metadata":{"raw":"nex-agi/nex-n2.5-mini:free is temporarily rate-limited upstream. Please retry shortly."}}"#;
    let profile = profile(HarnessKind::ClaudeCode, OPENROUTER_MODEL);
    let entry = ProviderEntry::new(Tag::parse(TAG).expect("a tag"), ProviderKind::OpenRouter, "https://openrouter.ai");
    let busy = Upstream::new(|_| {
        Ok(super::relay::UpstreamAnswer {
            status: 429,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Box::new(std::io::Cursor::new(BUSY.as_bytes().to_vec())),
        })
    });
    for engine in engines() {
        let kind = engine.kind();
        let road = Road::open(engine, &profile, entry.clone(), busy.clone());
        let started = std::time::Instant::now();
        let said = road.ask_claude("Reply with just the digits: what is 2+2?").unwrap_or_else(|trouble| trouble);
        println!("{kind:?}: after {} s Claude Code printed: {said}", started.elapsed().as_secs());
        assert!(said.contains("429"), "{kind:?}: the status reached the tab: {said}");
        let events = road.events();
        assert!(
            events.iter().any(|event| matches!(event, super::RelayEvent::Forwarded { status: 429, .. })),
            "{kind:?}: {events:?}"
        );
    }
}

/// `harness` on each system `QCODE_OS` names (all four when it names none), through the relay to
/// the model service of this machine, in a container with no network: the image of the profile is
/// built on that system's own base with the product's recipe, and the harness answers a question
/// and reads a file of the workspace with its own tools. Claude Code is asked only to read, since
/// that alone needs a tool; opencode, like its own test above, also writes one.
fn answers_on_every_system(harness: HarnessKind) {
    let (base, model) = provider();
    let entry = ProviderEntry::new(Tag::parse(TAG).expect("a tag"), ProviderKind::Ollama, &base);
    for engine in engines() {
        for os in crate::base::live::systems() {
            let kind = engine.kind();
            let profile = on(harness, &model, os);
            assert!(os.refuses(harness).is_none(), "{harness:?} is not offered on {os:?}");
            let built = std::time::Instant::now();
            let road = Road::open(engine.clone(), &profile, entry.clone(), Upstream::network());
            println!(
                "{kind:?} {os:?} {harness:?}: base and profile image ready in {} s, profile image {} bytes",
                built.elapsed().as_secs(),
                road.image_bytes()
            );
            std::fs::write(road.work().join("parola.txt"), "kirlangic\n").expect("a file for the harness to read");

            let started = std::time::Instant::now();
            let said = road
                .ask_once("Reply with just the digits: what is 2+2?")
                .unwrap_or_else(|trouble| panic!("{kind:?} {os:?}: the harness said nothing: {trouble}"));
            assert!(said.contains('4'), "{kind:?} {os:?}: the model answered through the relay: {said}");
            println!("{kind:?} {os:?} {harness:?}: {model} answered 2+2 in {} s", started.elapsed().as_secs());

            let started = std::time::Instant::now();
            let asked = match harness {
                HarnessKind::OpenCode => {
                    "Read the file parola.txt in this folder. Then create a file named cevap.txt in this folder \
                     whose only content is that same word. Reply with only the word."
                }
                _ => "Read the file parola.txt in this folder and reply with only the word inside it.",
            };
            let said = road
                .ask_once(asked)
                .unwrap_or_else(|trouble| panic!("{kind:?} {os:?}: the harness said nothing: {trouble}"));
            if harness == HarnessKind::OpenCode {
                let written = std::fs::read_to_string(road.work().join("cevap.txt")).unwrap_or_default();
                assert!(written.contains("kirlangic"), "{kind:?} {os:?}: opencode wrote with its tools: {said}");
            } else {
                assert!(said.contains("kirlangic"), "{kind:?} {os:?}: the agent read the file with its tools: {said}");
            }
            println!("{kind:?} {os:?} {harness:?}: {model} used a tool in {} s", started.elapsed().as_secs());
            assert!(
                road.events().iter().any(|event| matches!(event, super::RelayEvent::Forwarded { status: 200, .. })),
                "{kind:?} {os:?}: QCode carried the requests itself"
            );
        }
    }
}

#[test]
#[ignore = "needs a container engine, the network to build each system's images, and a model service; run with QCODE_CONTAINER_TESTS=1"]
fn claude_code_answers_on_every_system_through_a_provider_of_this_machine() {
    answers_on_every_system(HarnessKind::ClaudeCode);
}

#[test]
#[ignore = "needs a container engine, the network to build each system's images, and a model service; run with QCODE_CONTAINER_TESTS=1"]
fn opencode_answers_on_every_system_through_a_provider_of_this_machine() {
    answers_on_every_system(HarnessKind::OpenCode);
}
