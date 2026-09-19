//! The background service: stopping the containers QCode started once no QCode is open.
//!
//! Four parts, each usable on its own:
//!
//! - [`stop`] is the work itself: read the list of QCode's containers, stop the ones that run,
//!   keep listed what could not be stopped.
//! - [`Open`] is what every running QCode holds for its whole life: a shared lock on
//!   `instances.lock` beside the list. Closing it is where the last QCode to close does the work
//!   itself, so containers are stopped on a normal exit even with no service installed.
//! - [`reaper`] is what `qcode reaper` runs: it waits in the kernel for the exclusive lock on the
//!   same file, which it gets only once no QCode holds it, however the last one went.
//! - [`units`] makes the systemd and launchd files that run `qcode reaper` when the list changes,
//!   and the steps that install and remove them.
//!
//! The lock is the kernel's (`flock`), so a QCode that crashes or is killed holds nothing and
//! keeps nobody waiting. Windows has no such lock for QCode: there every call answers
//! [`io::ErrorKind::Unsupported`], nothing is held and nothing is stopped.

pub mod stop;
pub mod units;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use qframe::storage::InstanceLock;
use qframe::t;

use crate::engine::run::EngineError;
use crate::engine::{Engine, EngineCommand, EngineKind};
use crate::workspace::{OnClose, Registry};
use stop::Reaped;

/// The name of the file every open QCode holds a shared lock on, beside the list.
pub const LOCK_FILE: &str = "instances.lock";

/// Where the list of QCode's containers and the lock of the open QCodes are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// The list of the containers QCode started.
    pub registry: PathBuf,
    /// The file every open QCode holds a shared lock on.
    pub lock: PathBuf,
}

impl Places {
    /// This user's: both in QCode's data folder, or `None` on a machine that names no home, where
    /// nothing is recorded and so there is nothing to stop.
    #[must_use]
    pub fn detect() -> Option<Self> {
        Registry::file().map(Self::beside)
    }

    /// The list at `registry`, with the lock beside it.
    #[must_use]
    pub fn beside(registry: PathBuf) -> Self {
        let lock = registry.with_file_name(LOCK_FILE);
        Self { registry, lock }
    }

    /// Makes the folder the lock lives in, which a first run on a machine does not have yet.
    fn make_folder(&self) -> io::Result<()> {
        match self.lock.parent() {
            Some(folder) if !folder.as_os_str().is_empty() => std::fs::create_dir_all(folder),
            _ => Ok(()),
        }
    }
}

/// How the containers are reached: `engine_for` finds the engine of a kind, `run` runs one command
/// and answers what it printed. The real ones are [`crate::engine::detect`] and
/// [`crate::engine::run::capture`]; a test passes its own.
pub struct Engines<'a> {
    /// The engine of a kind, or `None` when it is not there.
    pub engine_for: &'a dyn Fn(EngineKind) -> Option<Engine>,
    /// Runs one command.
    pub run: &'a mut dyn FnMut(&EngineCommand) -> Result<String, EngineError>,
}

/// A running QCode, as the other QCodes and the service see it: a shared lock on the lock file,
/// held for as long as this value lives.
///
/// Any number of QCodes hold one at once. Taking it waits only while the service is stopping
/// containers, so a QCode starting then waits for that instead of starting a container the
/// service is about to stop.
#[derive(Debug)]
pub struct Open {
    places: Places,
    instance: InstanceLock,
}

impl Open {
    /// Takes the shared lock at `places`, making its folder first.
    ///
    /// # Errors
    ///
    /// When the folder or the file cannot be made or locked, and
    /// [`io::ErrorKind::Unsupported`] on Windows.
    pub fn hold(places: Places) -> io::Result<Self> {
        places.make_folder()?;
        let instance = InstanceLock::shared(&places.lock)?;
        Ok(Self { places, instance })
    }

    /// Closes this QCode: lets go of its lock and, when no other QCode holds one, does the
    /// service's work itself — stops the listed containers if `on_close` says to and empties
    /// the list. Answers what came of it, or `None` when another QCode is still open or the
    /// service is already at work.
    ///
    /// The setting is asked for only once this is the last QCode, so a choice made in another
    /// QCode a moment ago counts. The exclusive lock is asked for without waiting and held for
    /// the stopping only: a QCode starting meanwhile waits for it.
    ///
    /// # Errors
    ///
    /// When the lock file cannot be opened, or what stays listed cannot be written.
    pub fn close(self, on_close: &dyn Fn() -> OnClose, engines: &mut Engines<'_>) -> io::Result<Option<Reaped>> {
        let Self { places, instance } = self;
        drop(instance);
        let Some(last) = settled_exclusive(&places.lock)? else { return Ok(None) };
        let reaped = stop::reap(&places.registry, on_close(), engines.engine_for, engines.run)?;
        drop(last);
        Ok(Some(reaped))
    }
}

/// How long a closing QCode gives its own lock to be let go of everywhere before it concludes
/// that another QCode is open.
///
/// A program another thread of this QCode is starting holds a copy of every open file between
/// `fork` and `exec`, this lock included, for a few milliseconds; the lock is released only when
/// that copy closes too. Waiting this long costs a QCode that is not the last one no more than a
/// tenth of a second on its way out.
const SETTLING: Duration = Duration::from_millis(100);

/// The exclusive lock at `path` when nobody holds a lock on it once [`SETTLING`] has passed, or
/// `None` because somebody still does.
fn settled_exclusive(path: &Path) -> io::Result<Option<InstanceLock>> {
    let deadline = Instant::now() + SETTLING;
    loop {
        if let Some(lock) = InstanceLock::try_exclusive(path)? {
            return Ok(Some(lock));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What `qcode reaper` does: waits until no QCode is open, stops the listed containers if the
/// setting says to, empties the list, and writes to `out` what it did.
///
/// The wait is the kernel's: the process sleeps until the last shared lock is gone and costs
/// nothing meanwhile. It returns at once when no QCode is open, so a run started by a list that
/// a closing QCode wrote finds the work done and exits. The service manager starts the reaper
/// again on the next write to the list, which is how it learns that a new QCode is at work;
/// it never waits twice in one run. The setting is read after the wait, so a choice made while
/// the QCodes were open counts.
///
/// # Errors
///
/// When the lock file cannot be made or locked ([`io::ErrorKind::Unsupported`] on Windows),
/// when what stays listed cannot be written, or when `out` cannot be written to.
pub fn reaper(
    out: &mut dyn Write,
    places: &Places,
    on_close: &dyn Fn() -> OnClose,
    engines: &mut Engines<'_>,
) -> io::Result<()> {
    places.make_folder()?;
    let last = InstanceLock::wait_exclusive(&places.lock)?;
    let reaped = stop::reap(&places.registry, on_close(), engines.engine_for, engines.run)?;
    drop(last);
    for line in report(&reaped) {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// What a reaper run has to say about `reaped`, one line each, in the language in force.
#[must_use]
pub fn report(reaped: &Reaped) -> Vec<String> {
    match reaped {
        Reaped::Kept => vec![t!("reaper.kept")],
        Reaped::Broken(problems) => {
            problems.iter().map(|problem| t!("reaper.broken", problem = problem.to_string())).collect()
        }
        Reaped::Done(stopping) => {
            let mut lines: Vec<String> =
                stopping.stopped.iter().map(|name| t!("reaper.stopped", name = name.as_str())).collect();
            lines.extend(
                stopping
                    .failed
                    .iter()
                    .map(|(name, reason)| t!("reaper.failed", name = name.as_str(), reason = reason.as_str())),
            );
            lines.extend(stopping.unreachable.iter().map(|(kind, reason)| {
                let engine = crate::ui::settings::engine::name(*kind);
                if reason.is_empty() {
                    t!("reaper.missing", engine = engine)
                } else {
                    t!("reaper.unreachable", engine = engine, reason = reason.as_str())
                }
            }));
            lines
        }
    }
}

/// The text of `qcode reaper`, in the language the person chose in QCode or else the machine's.
///
/// The reaper has no screen and no runtime to load the text for it, so it does what the runtime
/// would: QCode's language files over the framework's, and the language picked the same way.
#[must_use]
pub fn translator(chosen: Option<&str>) -> qframe::i18n::I18n {
    let mut catalog = qframe::i18n::I18n::builtin();
    for (file, text) in crate::locales() {
        catalog.add_source(&file, &text);
    }
    let code = chosen.map(str::to_owned).or_else(|| catalog.detect(|name| std::env::var(name).ok()));
    if let Some(code) = code {
        catalog.set_active(&code);
    }
    catalog
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use qframe::diagnostics::Diagnostic;

    use super::stop::Stopping;
    use super::*;
    use crate::engine::EngineKind;

    fn translated<R>(code: &str, body: impl FnOnce() -> R) -> R {
        qframe::i18n::scope(Arc::new(translator(Some(code))), body)
    }

    /// A binary that cannot be run: the tests hand their own runner, so it is only a name.
    const NO_ENGINE: &str = "/nonexistent/qcode-test-engine";

    fn engine(kind: EngineKind) -> Option<Engine> {
        Some(Engine::new(kind, NO_ENGINE))
    }

    /// A folder of its own for one test, gone when the test is.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(what: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("qcode-service-{what}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn places(&self) -> Places {
            Places::beside(self.0.join("data").join("containers.toml"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A runner that answers `ps` with `listing` and every other command with success, and keeps
    /// the words of every command it was given in `seen`.
    fn runner(
        listing: &'static str,
        seen: &Arc<Mutex<Vec<String>>>,
    ) -> impl FnMut(&EngineCommand) -> Result<String, EngineError> + use<> {
        let seen = Arc::clone(seen);
        move |command| {
            let words: Vec<String> = command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
            let answer = if words[0] == "ps" { listing.to_owned() } else { String::new() };
            seen.lock().expect("not poisoned").push(words.join(" "));
            Ok(answer)
        }
    }

    fn listed(places: &Places) -> Vec<String> {
        Registry::load(&places.registry).value.containers.into_iter().map(|known| known.name).collect()
    }

    #[test]
    fn the_lock_lives_beside_the_list() {
        let places = Places::beside(PathBuf::from("/home/ada/.local/share/quvyta/code/containers.toml"));
        assert_eq!(places.lock, PathBuf::from("/home/ada/.local/share/quvyta/code/instances.lock"));
    }

    #[cfg(unix)]
    #[test]
    fn an_open_qcode_holds_its_lock_until_it_closes_and_the_last_to_close_stops_the_containers() {
        let scratch = Scratch::new("open");
        let places = scratch.places();
        // A first run on a machine: not even the folder is there yet.
        let first = Open::hold(places.clone()).expect("the folder is made and the lock taken");
        let second = Open::hold(places.clone()).expect("any number of QCodes are open at once");
        assert!(InstanceLock::try_exclusive(&places.lock).expect("asked").is_none(), "two QCodes are open");
        Registry::record(&places.registry, "qcode-a-base", EngineKind::Podman).expect("written");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut run = runner("qcode-a-base\trunning\n", &seen);
        let closed =
            first.close(&|| OnClose::Stop, &mut Engines { engine_for: &engine, run: &mut run }).expect("closed");
        assert_eq!(closed, None, "another QCode is still open");
        assert!(seen.lock().expect("not poisoned").is_empty());
        assert_eq!(listed(&places), ["qcode-a-base"]);

        let asked = std::cell::Cell::new(false);
        let on_close = || {
            asked.set(true);
            OnClose::Stop
        };
        let closed = second.close(&on_close, &mut Engines { engine_for: &engine, run: &mut run }).expect("closed");
        let Some(Reaped::Done(stopping)) = closed else { panic!("the last QCode does the work: {closed:?}") };
        assert!(asked.get(), "the setting is read once this is the last QCode");
        assert_eq!(stopping.stopped, ["qcode-a-base"]);
        assert!(seen.lock().expect("not poisoned").contains(&"stop qcode-a-base".to_owned()));
        assert!(listed(&places).is_empty());
        assert!(settled_exclusive(&places.lock).expect("asked").is_some(), "nothing is held any more");
    }

    #[cfg(unix)]
    #[test]
    fn the_last_qcode_leaves_the_containers_running_when_the_setting_says_so() {
        let scratch = Scratch::new("keep");
        let places = scratch.places();
        let open = Open::hold(places.clone()).expect("held");
        Registry::record(&places.registry, "qcode-a-base", EngineKind::Podman).expect("written");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut run = runner("qcode-a-base\trunning\n", &seen);
        let closed =
            open.close(&|| OnClose::Keep, &mut Engines { engine_for: &engine, run: &mut run }).expect("closed");
        assert_eq!(closed, Some(Reaped::Kept));
        assert!(seen.lock().expect("not poisoned").is_empty());
        assert_eq!(listed(&places), ["qcode-a-base"]);
    }

    #[cfg(unix)]
    #[test]
    fn with_no_qcode_open_the_reaper_does_its_work_at_once() {
        let scratch = Scratch::new("at-once");
        let places = scratch.places();
        Registry::record(&places.registry, "qcode-a-base", EngineKind::Podman).expect("written");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut run = runner("qcode-a-base\trunning\n", &seen);
        let mut out = Vec::new();
        translated("en", || {
            reaper(&mut out, &places, &|| OnClose::Stop, &mut Engines { engine_for: &engine, run: &mut run })
        })
        .expect("done");
        assert_eq!(String::from_utf8(out).expect("text"), "Stopped qcode-a-base\n");
        assert!(listed(&places).is_empty());
    }

    /// Set in the child process of the test below: the folder it keeps a QCode open in.
    const HOLD: &str = "QCODE_TEST_HOLD_INSTANCE";

    #[cfg(unix)]
    #[test]
    fn a_qcode_open_in_another_process_keeps_its_containers_until_that_process_is_gone() {
        if let Some(folder) = std::env::var_os(HOLD) {
            // The child: a QCode that stays open until it is killed, as a crashed one would be.
            let places = Places::beside(PathBuf::from(folder).join("containers.toml"));
            let _open = Open::hold(places.clone()).expect("held");
            std::fs::write(places.registry.with_file_name("ready"), "").expect("written");
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        let scratch = Scratch::new("child");
        let places = scratch.places();
        let folder = places.registry.parent().expect("a folder").to_owned();
        std::fs::create_dir_all(&folder).expect("the folder");
        Registry::record(&places.registry, "qcode-a-base", EngineKind::Podman).expect("written");
        let mut child = crate::testing::spawn_child(
            "service::tests::a_qcode_open_in_another_process_keeps_its_containers_until_that_process_is_gone",
            &[(HOLD, folder.as_os_str())],
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while !folder.join("ready").exists() {
            assert!(Instant::now() < deadline, "the child never held its lock");
            std::thread::sleep(Duration::from_millis(10));
        }

        // A QCode closing here is not the last one.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mine = Open::hold(places.clone()).expect("held");
        let mut run = runner("qcode-a-base\trunning\n", &seen);
        let closed =
            mine.close(&|| OnClose::Stop, &mut Engines { engine_for: &engine, run: &mut run }).expect("closed");
        assert_eq!(closed, None, "the child is open");

        // The service waits for the child, and wakes when it is gone.
        let (done, woke) = mpsc::channel();
        let waiting = {
            let places = places.clone();
            let mut run = runner("qcode-a-base\trunning\n", &seen);
            std::thread::spawn(move || {
                let mut out = Vec::new();
                let result = translated("en", || {
                    reaper(&mut out, &places, &|| OnClose::Stop, &mut Engines { engine_for: &engine, run: &mut run })
                });
                done.send((result.map_err(|error| error.to_string()), out)).expect("the test waits");
            })
        };
        assert!(woke.recv_timeout(Duration::from_millis(300)).is_err(), "the service waits while the child is open");
        assert!(seen.lock().expect("not poisoned").is_empty(), "and asks no engine anything");
        assert_eq!(listed(&places), ["qcode-a-base"]);

        child.kill().expect("killed");
        child.wait().expect("reaped");
        let (result, out) = woke.recv_timeout(Duration::from_secs(30)).expect("the service woke");
        waiting.join().expect("the thread ends");
        assert_eq!(result, Ok(()));
        assert_eq!(String::from_utf8(out).expect("text"), "Stopped qcode-a-base\n");
        assert!(listed(&places).is_empty());
    }

    #[test]
    fn a_run_is_reported_line_by_line_in_both_languages() {
        let mut stopping = Stopping::default();
        stopping.stopped.push("qcode-a-base".to_owned());
        stopping.failed.push(("qcode-a-codex".to_owned(), "container is paused".to_owned()));
        stopping.unreachable.push((EngineKind::Docker, String::new()));
        stopping.unreachable.push((EngineKind::Podman, "cannot connect".to_owned()));
        let done = Reaped::Done(stopping);
        let broken = Reaped::Broken(vec![Diagnostic::error(None, "bad line")]);
        for code in ["en", "tr"] {
            let (lines, kept, broken) = translated(code, || (report(&done), report(&Reaped::Kept), report(&broken)));
            assert_eq!(lines.len(), 4, "{lines:?}");
            assert!(lines[0].contains("qcode-a-base"));
            assert!(lines[1].contains("qcode-a-codex") && lines[1].contains("container is paused"));
            assert!(lines[2].contains("Docker"));
            assert!(lines[3].contains("Podman") && lines[3].contains("cannot connect"));
            assert_eq!(kept.len(), 1);
            assert!(broken[0].contains("bad line"));
            for line in lines.iter().chain(&kept).chain(&broken) {
                assert!(!line.contains('⟦'), "{code}: {line}");
            }
        }
        assert!(translated("tr", || report(&done))[0].contains("durduruldu"));
    }
}
