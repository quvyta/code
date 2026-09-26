//! Running the commands an [`Engine`] spells out.
//!
//! Two ways of running, because QCode asks two kinds of question. Most commands answer at once
//! and their answer is read whole ([`capture`]). Building an image does not: it talks for
//! minutes, the person watches it in a log and may change their mind, so [`stream`] hands over
//! every line as it arrives and stops when asked ([`build_image`] then clears the half-made
//! image away).
//!
//! Neither of them is a place to wait: the caller runs them inside a `qframe` `Task`, whose
//! thread they already have, whose `is_cancelled` is the `cancel` they are given and whose
//! `send` carries the lines to the log.

use super::command::{EngineCommand, ImageBuild};
use super::{Engine, EngineKind};
use qframe::i18n::I18n;
use qframe::runtime::{Line, Process, ProcessOutcome};
use std::io::Read;
use std::process::{Child, Command};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

/// A command that ran and failed. It carries what was run and everything the engine said, so
/// the person can be shown the engine's own words rather than a summary of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What was run.
    pub command: EngineCommand,
    /// The exit code, or `None` when a signal ended it.
    pub code: Option<i32>,
    /// Everything the engine printed, output and errors together, in the order it came.
    pub output: String,
}

/// Why a command did not do what was asked.
///
/// The variants are what the application acts on; the words shown to the person live in the
/// language files.
#[derive(Debug)]
pub enum EngineError {
    /// The binary could not be started at all: it went missing, or it is not runnable.
    NotRunnable {
        /// What was to be run.
        command: EngineCommand,
        /// What the operating system said.
        error: std::io::Error,
    },
    /// The engine ran and refused.
    Failed(Failure),
    /// The person cancelled it, and it was stopped.
    Cancelled {
        /// What was stopped.
        command: EngineCommand,
    },
    /// The engine did not answer within the command's deadline, and the command was stopped.
    /// An engine that hangs on a question is almost always waiting on a lock of its own, which
    /// nothing QCode does can free.
    TimedOut {
        /// What was stopped.
        command: EngineCommand,
        /// How long it was given.
        after: Duration,
    },
}

/// Where the words of [`timed_out`] come from on a thread that has no translator of its own.
type Words = Box<dyn Fn() -> Arc<I18n> + Send + Sync>;

/// The person's translator for work running away from the screen, set once by the application.
static WORDS: OnceLock<Words> = OnceLock::new();

/// Gives the engine layer the person's translator for the sentence of [`timed_out`], which is
/// mostly made on a background thread: the runtime lends its translator only to the screen's own
/// work. `words` is asked each time, so a language changed since the start is the one used.
/// Without it the sentence is English.
pub fn speak_with(words: impl Fn() -> Arc<I18n> + Send + Sync + 'static) {
    let _ = WORDS.set(Box::new(words));
}

/// The sentence the person reads when an engine did not answer in time: which engine, how long
/// it was given, and what is most likely wrong, in their language.
#[must_use]
pub fn timed_out(command: &EngineCommand, after: Duration) -> String {
    let binary = command.program.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let say = || {
        let engine = match EngineKind::from_name(&binary) {
            Some(EngineKind::Podman) => qframe::t!("settings.podman"),
            Some(EngineKind::Docker) => qframe::t!("settings.docker"),
            None => binary.clone(),
        };
        qframe::t!("engine.timed-out", engine = engine, seconds = after.as_secs().to_string())
    };
    // A translator of this thread's own, the screen's, is the one the person is reading.
    let said = say();
    if !said.starts_with('⟦') {
        return said;
    }
    let words = WORDS.get().map_or_else(|| Arc::new(crate::service::translator(Some("en"))), |words| words());
    qframe::i18n::scope(words, say)
}

/// Runs `command` to the end and returns what it printed to standard output.
///
/// It is given until the command's [`deadline`](EngineCommand::deadline) to end; past it the
/// command is stopped and [`EngineError::TimedOut`] says so.
///
/// # Errors
///
/// When the engine cannot be started, runs and fails, or does not answer in time.
pub fn capture(command: &EngineCommand) -> Result<String, EngineError> {
    use std::process::Stdio;

    let child = start(
        Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|error| EngineError::NotRunnable { command: command.clone(), error })?;
    finish(command, child)
}

/// Runs `command`, writes `input` to its standard input and closes it, and returns what it
/// printed to standard output, the way [`capture`] does, deadline and all.
///
/// The input is written on a thread of its own while the output is read, so a command that
/// prints before it has read everything cannot leave both sides waiting on a full pipe.
///
/// # Errors
///
/// When the engine cannot be started, runs and fails, or does not answer in time.
pub fn feed(command: &EngineCommand, input: &[u8]) -> Result<String, EngineError> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = start(
        Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|error| EngineError::NotRunnable { command: command.clone(), error })?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        // Not waited for: it ends when everything is written or when the command is gone, and a
        // command that stops reading early says why in its output, which is what the person is
        // shown; the broken pipe itself adds nothing.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    finish(command, child)
}

/// How many times a start refused with "text file busy" is tried again, and how long apart.
const BUSY_TRIES: u32 = 20;
const BUSY_PAUSE: Duration = Duration::from_millis(50);

/// Starts `command`, trying again for a moment when the system refuses because the program's
/// file is still open for writing somewhere.
///
/// That happens when a program was written just before it is started while another thread of
/// the same process forks: the child holds a copy of the file's descriptor until it execs,
/// usually for microseconds. Nothing is wrong with the program then, so a start that fails for
/// that reason alone is tried again, for up to a second; every other refusal is answered at once.
fn start(command: &mut Command) -> std::io::Result<Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < BUSY_TRIES => {
                tries += 1;
                std::thread::sleep(BUSY_PAUSE);
            }
            answer => return answer,
        }
    }
}

/// How long a command that did not answer in time is given to end by itself once asked to,
/// before it is ended outright.
const GRACE: Duration = Duration::from_secs(5);

/// Reads everything `child` prints and waits for it to end, within `command`'s deadline.
///
/// Both streams are read on threads of their own, so neither can fill up and stall the other.
/// Past the deadline the command is asked to stop and, if it has not after [`GRACE`], ended; the
/// threads are then left to finish on their own, since whatever the command started may still
/// hold its streams open, and waiting for them would be the very wait the deadline is for.
fn finish(command: &EngineCommand, mut child: Child) -> Result<String, EngineError> {
    let (sender, said) = mpsc::channel();
    for (index, stream) in [child.stdout.take().map(boxed), child.stderr.take().map(boxed)].into_iter().enumerate() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            let _ = sender.send((index, bytes));
        });
    }
    drop(sender);
    let started = Instant::now();
    let left = || command.deadline.map(|deadline| deadline.saturating_sub(started.elapsed()));
    let mut streams = [Vec::new(), Vec::new()];
    for _ in 0..2 {
        let received = match left() {
            Some(left) => said.recv_timeout(left).map_err(|_| ()),
            None => said.recv().map_err(|_| ()),
        };
        match received {
            Ok((index, bytes)) => streams[index] = bytes,
            Err(()) => return Err(stuck(command, child)),
        }
    }
    let status = loop {
        if command.deadline.is_none() {
            break child.wait().map_err(|error| EngineError::NotRunnable { command: command.clone(), error })?;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if left().is_some_and(|left| left.is_zero()) => return Err(stuck(command, child)),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(EngineError::NotRunnable { command: command.clone(), error }),
        }
    };
    let [out, err] = streams;
    if status.success() {
        return Ok(String::from_utf8_lossy(&out).into_owned());
    }
    Err(EngineError::Failed(Failure {
        command: command.clone(),
        code: status.code(),
        output: both(&String::from_utf8_lossy(&out), &String::from_utf8_lossy(&err)),
    }))
}

/// One of a child's output streams, whichever it is.
fn boxed(stream: impl Read + Send + 'static) -> Box<dyn Read + Send> {
    Box::new(stream)
}

/// Stops a command that did not answer in time and says so: asked to end first, the way a person
/// pressing Ctrl+C would, so an engine can let go of what it holds; ended outright if it has not
/// after [`GRACE`].
fn stuck(command: &EngineCommand, mut child: Child) -> EngineError {
    #[cfg(unix)]
    {
        let _ = rustix::process::kill_process(rustix::process::Pid::from_child(&child), rustix::process::Signal::TERM);
        let asked = Instant::now();
        while asked.elapsed() < GRACE {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    EngineError::TimedOut { command: command.clone(), after: command.deadline.unwrap_or_default() }
}

/// Runs `from` and `to` side by side with what `from` prints going straight into `to`, the way a
/// shell's `|` does: the bytes pass from one process to the other and never through QCode, so a
/// home of any size is copied in the memory of a pipe.
///
/// Each side's errors are read on a thread of its own, so a side that says a lot cannot stall
/// the other on a full pipe.
///
/// # Errors
///
/// When either cannot be started, or either ends in failure: the one that failed, with what it
/// said. When both fail, the reading side is named, because the writing side then had nothing
/// to write.
pub fn pipe(from: &EngineCommand, to: &EngineCommand) -> Result<(), EngineError> {
    use std::process::Stdio;

    let not_runnable = |command: &EngineCommand| {
        let command = command.clone();
        move |error| EngineError::NotRunnable { command, error }
    };
    let mut reader = Command::new(&from.program)
        .args(&from.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(not_runnable(from))?;
    let Some(packed) = reader.stdout.take() else {
        let _ = reader.kill();
        return Err(EngineError::NotRunnable { command: from.clone(), error: std::io::Error::other("no output") });
    };
    let writer = Command::new(&to.program)
        .args(&to.args)
        .stdin(Stdio::from(packed))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let writer = match writer {
        Ok(writer) => writer,
        Err(error) => {
            let _ = reader.kill();
            let _ = reader.wait();
            return Err(EngineError::NotRunnable { command: to.clone(), error });
        }
    };
    let said = reader.stderr.take().map(|mut stream| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stream.read_to_string(&mut text);
            text
        })
    });
    let written = writer.wait_with_output().map_err(not_runnable(to))?;
    let read = reader.wait().map_err(not_runnable(from))?;
    let said = said.and_then(|thread| thread.join().ok()).unwrap_or_default();
    if !read.success() {
        return Err(EngineError::Failed(Failure {
            command: from.clone(),
            code: read.code(),
            output: said.trim_end().to_owned(),
        }));
    }
    if !written.status.success() {
        return Err(EngineError::Failed(Failure {
            command: to.clone(),
            code: written.status.code(),
            output: both(&String::from_utf8_lossy(&written.stdout), &String::from_utf8_lossy(&written.stderr)),
        }));
    }
    Ok(())
}

/// Joins what a command printed to its two streams into the one text the person is shown.
fn both(out: &str, err: &str) -> String {
    match (out.trim_end(), err.trim_end()) {
        ("", err) => err.to_owned(),
        (out, "") => out.to_owned(),
        (out, err) => format!("{out}\n{err}"),
    }
}

/// Runs `command`, handing every line to `line` as it arrives, and stops it as soon as `cancel`
/// says so.
///
/// Engines write progress to both of their streams, so both are read and shown in the order they
/// come. The child gets no standard input, so an image build that asks a question cannot take
/// the keys meant for the application, and it runs in a process group of its own: cancelling
/// ends the build steps the engine started, not just the engine.
///
/// # Errors
///
/// When the engine cannot be started, runs and fails, or is cancelled.
pub fn stream(
    command: &EngineCommand,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<(), EngineError> {
    let mut collected = String::new();
    let mut on_line = |text: Line| {
        let text = match text {
            Line::Out(text) | Line::Err(text) => qframe::text::printable(&text).into_owned(),
        };
        collected.push_str(&text);
        collected.push('\n');
        line(&text);
    };
    let outcome = Process::new(&command.program).args(&command.args).no_stdin().run(cancel, &mut on_line);
    let code = match outcome {
        Ok(ProcessOutcome::Finished { code }) => code,
        Ok(ProcessOutcome::Cancelled) => return Err(EngineError::Cancelled { command: command.clone() }),
        Err(error) => return Err(EngineError::NotRunnable { command: command.clone(), error }),
    };
    if code == Some(0) {
        return Ok(());
    }
    Err(EngineError::Failed(Failure { command: command.clone(), code, output: collected.trim_end().to_owned() }))
}

/// Builds an image, showing the build as it happens and leaving nothing behind when it is
/// cancelled or fails.
///
/// A build that stops halfway can leave its tag on an image that was never finished; the next
/// run would then start a container from it and the harness inside would be missing pieces. So
/// anything but success takes the tag away again, and a removal that fails because there is
/// nothing to remove is exactly what is wanted anyway.
///
/// # Errors
///
/// When the engine cannot be started, the build fails, or the person cancels it.
pub fn build_image(
    engine: &Engine,
    request: &ImageBuild<'_>,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<(), EngineError> {
    let result = stream(&engine.build_image(request), cancel, line);
    if result.is_err() {
        let _ = capture(&engine.remove_image(request.image));
    }
    result
}

#[cfg(test)]
mod tests {

    #[cfg(unix)]
    #[test]
    fn a_program_still_open_for_writing_a_moment_is_started_once_it_is_closed() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = std::env::temp_dir().join(format!("qcode-busy-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let program = dir.join("engine");
        let mut open = std::fs::File::create(&program).expect("the program is written");
        open.write_all(b"#!/bin/sh\necho started\n").expect("written");
        open.flush().expect("flushed");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        // Held open for writing, as a forked thread of the test process holds it for a moment.
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(open);
        });
        let command =
            EngineCommand { program: program.clone(), args: Vec::new(), deadline: Some(Duration::from_secs(30)) };
        let said = capture(&command);
        closer.join().expect("closed");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(said.expect("started once the file was closed").trim(), "started");
    }
    use super::{EngineError, capture, feed, pipe, stream};
    use crate::engine::EngineCommand;
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::Duration;

    /// A command that needs no container engine: every machine QCode runs on has a shell or
    /// `cmd`, which is enough to check how output, failure and cancelling are handled.
    #[cfg(unix)]
    fn shell(script: &str) -> EngineCommand {
        EngineCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![OsString::from("-c"), OsString::from(script)],
            deadline: Some(crate::engine::ANSWER_WITHIN),
        }
    }

    /// A stand-in engine named `podman` in a folder of its own, running `script`, which is given
    /// `deadline` to answer; the folder goes when the test is done with it.
    #[cfg(unix)]
    fn stuck_engine(name: &str, script: &str, deadline: Duration) -> (EngineCommand, crate::engine::scratch::Scratch) {
        use std::os::unix::fs::PermissionsExt as _;
        let folder = crate::engine::scratch::Scratch::new(&format!("stuck-{name}")).expect("a folder");
        let bin = folder.path().join("podman");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).expect("the stand-in is written");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        (EngineCommand { program: bin, args: Vec::new(), deadline: Some(deadline) }, folder)
    }

    /// Runs `run` on a thread of its own and answers what it returned, failing the test when it
    /// has not returned after a generous while: a deadline that does not hold would otherwise
    /// hang the test run instead of failing it.
    fn within_bound<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(run());
        });
        answer.recv_timeout(Duration::from_secs(30)).expect("the command was never given up on")
    }

    /// Whether the process `pid` is gone, or only a zombie waiting to be reaped, within a
    /// generous while.
    #[cfg(target_os = "linux")]
    fn ends(pid: &str) -> bool {
        let stat = format!("/proc/{}/stat", pid.trim());
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(20) {
            match std::fs::read_to_string(&stat) {
                Ok(stat) if !stat[stat.rfind(')').expect("name") + 2..].starts_with('Z') => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => return true,
            }
        }
        false
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_engine_that_does_not_answer_is_stopped_at_its_deadline_and_said_to_be_stuck() {
        let (command, folder) =
            stuck_engine("capture", "echo $$ > \"$(dirname \"$0\")/pid\"; exec sleep 600", Duration::from_millis(500));
        let started = std::time::Instant::now();
        let asked = command.clone();
        let error = within_bound(move || capture(&asked)).expect_err("it never answers");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        let EngineError::TimedOut { command: stopped, after } = error else { panic!("{error:?}") };
        assert_eq!((stopped, after), (command, Duration::from_millis(500)));
        let pid = std::fs::read_to_string(folder.path().join("pid")).expect("it started");
        assert!(ends(&pid), "the stuck engine was ended");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_engine_that_will_not_stop_when_asked_is_ended_after_the_grace() {
        let script = "trap '' TERM; echo $$ > \"$(dirname \"$0\")/pid\"; while :; do sleep 1; done";
        let (command, folder) = stuck_engine("stubborn", script, Duration::from_millis(300));
        let error = within_bound(move || capture(&command)).expect_err("it never answers");
        assert!(matches!(error, EngineError::TimedOut { .. }), "{error:?}");
        let pid = std::fs::read_to_string(folder.path().join("pid")).expect("it started");
        assert!(ends(&pid), "the engine that ignored the request was ended");
    }

    #[cfg(unix)]
    #[test]
    fn an_engine_fed_input_it_never_reads_is_given_up_on_too() {
        let (command, _folder) = stuck_engine("feed", "exec sleep 600", Duration::from_millis(500));
        // More than a pipe holds, so the writer is left blocked on a reader that never reads.
        let input = vec![b'x'; 300_000];
        let error = within_bound(move || feed(&command, &input)).expect_err("it never answers");
        assert!(matches!(error, EngineError::TimedOut { .. }), "{error:?}");
    }

    #[cfg(unix)]
    #[test]
    fn work_without_a_deadline_is_waited_for_however_long_it_takes() {
        let (mut command, _folder) = stuck_engine("long", "sleep 1; echo done", Duration::from_millis(100));
        command.deadline = None;
        assert_eq!(within_bound(move || capture(&command)).expect("it finishes"), "done\n");
    }

    #[test]
    fn a_stuck_engine_is_named_with_how_long_it_was_given() {
        let env = qframe::env::Env::load(&qframe::env::AssetDirs {
            locale_sources: crate::locales(),
            ..qframe::env::AssetDirs::default()
        })
        .expect("the built-in files load");
        let mut i18n = env.i18n().clone();
        let command = crate::engine::Engine::new(crate::engine::EngineKind::Docker, "/usr/bin/docker").list_volumes();
        for (language, said) in [
            (
                "en",
                "Docker did not answer in 60 seconds, so QCode stopped waiting. It may be stuck on a lock of its own.",
            ),
            (
                "tr",
                "Docker 60 saniye içinde cevap vermedi, QCode da beklemeyi bıraktı. Kendi kilitlerinden birinde takılı kalmış olabilir.",
            ),
        ] {
            assert!(i18n.set_active(language), "{language}");
            let sentence = qframe::i18n::scope(std::sync::Arc::new(i18n.clone()), || {
                super::timed_out(&command, Duration::from_secs(60))
            });
            assert_eq!(sentence, said);
        }
    }

    #[cfg(unix)]
    #[test]
    fn captures_what_a_command_prints() {
        assert_eq!(capture(&shell("echo qcode")).expect("the shell runs"), "qcode\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_failure_carries_the_code_and_every_word_of_the_output() {
        let error = capture(&shell("echo out; echo err >&2; exit 3")).expect_err("the script fails");
        let EngineError::Failed(failure) = error else { panic!("the command ran and refused") };
        assert_eq!(failure.code, Some(3));
        assert_eq!(failure.output, "out\nerr");
    }

    #[cfg(unix)]
    #[test]
    fn what_is_fed_reaches_the_command_whole_even_past_the_pipe_size() {
        // Larger than any pipe buffer, so a writer that waited for the reader would hang here.
        let input = "x".repeat(300_000);
        assert_eq!(feed(&shell("wc -c"), input.as_bytes()).expect("the shell runs").trim(), "300000");
        let error = feed(&shell("cat > /dev/null; exit 4"), b"text").expect_err("the script fails");
        let EngineError::Failed(failure) = error else { panic!("the command ran and refused") };
        assert_eq!(failure.code, Some(4));
    }

    #[test]
    fn a_missing_binary_is_not_the_same_as_a_failing_one() {
        let command =
            EngineCommand { program: PathBuf::from("/qcode/no/such/engine"), args: Vec::new(), deadline: None };
        let error = capture(&command).expect_err("there is no such binary");
        assert!(matches!(error, EngineError::NotRunnable { .. }), "{error:?}");
    }

    #[cfg(unix)]
    #[test]
    fn streams_both_streams_line_by_line() {
        let mut lines = Vec::new();
        stream(&shell("echo one; echo two >&2; echo three"), &|| false, &mut |line| lines.push(line.to_owned()))
            .expect("the shell runs");
        lines.sort();
        assert_eq!(lines, ["one", "three", "two"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_long_command_stops_when_it_is_cancelled() {
        let seen = Cell::new(0_usize);
        let error = stream(&shell("echo started; sleep 30"), &|| seen.get() > 0, &mut |_| seen.set(seen.get() + 1))
            .expect_err("it was cancelled");
        assert!(matches!(error, EngineError::Cancelled { .. }), "{error:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_streamed_command_reads_no_input_and_cancelling_ends_what_it_started() {
        // An engine build starts steps of its own; the person cancels the build, not the engine.
        let seen = std::cell::RefCell::new(Vec::new());
        let error = stream(
            &shell("readlink /proc/$$/fd/0; sleep 60 & echo $!; wait"),
            &|| seen.borrow().len() == 2,
            &mut |line| seen.borrow_mut().push(line.to_owned()),
        )
        .expect_err("it was cancelled");
        assert!(matches!(error, EngineError::Cancelled { .. }), "{error:?}");
        let seen = seen.into_inner();
        assert_eq!(seen[0], "/dev/null");
        let stat = format!("/proc/{}/stat", seen[1]);
        let started = std::time::Instant::now();
        while std::fs::read_to_string(&stat)
            .is_ok_and(|stat| !stat[stat.rfind(')').expect("name") + 2..].starts_with('Z'))
        {
            assert!(started.elapsed() < std::time::Duration::from_secs(20), "the step still runs");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_hands_every_byte_from_one_command_to_the_other() {
        let scratch = std::env::temp_dir().join(format!("qcode-pipe-{}", std::process::id()));
        let _ = std::fs::remove_file(&scratch);
        // Binary and larger than a pipe's buffer, so neither a text conversion nor a writer that
        // waits for the reader would get it across whole.
        pipe(
            &shell("head -c 300000 /dev/urandom; head -c 1000 /dev/zero"),
            &shell(&format!("cat > {}", scratch.display())),
        )
        .expect("both ran");
        let got = std::fs::read(&scratch).expect("the copy was written");
        let _ = std::fs::remove_file(&scratch);
        assert!(got.len() == 301_000 && got.ends_with(&[0_u8; 1000]), "{}", got.len());
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_says_which_side_failed_and_in_its_own_words() {
        let reading = pipe(&shell("echo gone >&2; exit 3"), &shell("cat > /dev/null")).expect_err("the reader fails");
        let EngineError::Failed(failure) = reading else { panic!("{reading:?}") };
        assert_eq!((failure.code, failure.output.as_str()), (Some(3), "gone"));
        let writing =
            pipe(&shell("echo data"), &shell("cat > /dev/null; echo full >&2; exit 4")).expect_err("the writer fails");
        let EngineError::Failed(failure) = writing else { panic!("{writing:?}") };
        assert_eq!((failure.code, failure.output.as_str()), (Some(4), "full"));
    }

    #[cfg(unix)]
    #[test]
    fn what_a_stream_hands_on_has_no_control_character_left() {
        let mut lines = Vec::new();
        stream(&shell("printf 'context 2kB\\r\\r\\n\\033[1mbold\\033[0m\\n'"), &|| false, &mut |line| {
            lines.push(line.to_owned());
        })
        .expect("the shell runs");
        assert_eq!(lines, ["context 2kB", "bold"]);
    }
}
