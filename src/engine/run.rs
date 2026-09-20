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

use super::Engine;
use super::command::{EngineCommand, ImageBuild};
use qframe::runtime::{Line, Process, ProcessOutcome};
use std::process::Command;

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
}

/// Runs `command` to the end and returns what it printed to standard output.
///
/// # Errors
///
/// When the engine cannot be started, or runs and fails.
pub fn capture(command: &EngineCommand) -> Result<String, EngineError> {
    let output = match Command::new(&command.program).args(&command.args).output() {
        Ok(output) => output,
        Err(error) => return Err(EngineError::NotRunnable { command: command.clone(), error }),
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(EngineError::Failed(Failure {
        command: command.clone(),
        code: output.status.code(),
        output: both(&String::from_utf8_lossy(&output.stdout), &String::from_utf8_lossy(&output.stderr)),
    }))
}

/// Runs `command`, writes `input` to its standard input and closes it, and returns what it
/// printed to standard output, the way [`capture`] does.
///
/// The input is written on a thread of its own while the output is read here, so a command that
/// prints before it has read everything cannot leave both sides waiting on a full pipe.
///
/// # Errors
///
/// When the engine cannot be started, or runs and fails.
pub fn feed(command: &EngineCommand, input: &[u8]) -> Result<String, EngineError> {
    use std::io::Write;
    use std::process::Stdio;

    let not_runnable = |error| EngineError::NotRunnable { command: command.clone(), error };
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(not_runnable)?;
    let writer = child.stdin.take().map(|mut stdin| {
        let input = input.to_vec();
        std::thread::spawn(move || {
            // A command that stops reading early says why in its output, which is what the
            // person is shown; the broken pipe itself adds nothing.
            let _ = stdin.write_all(&input);
        })
    });
    let output = child.wait_with_output().map_err(not_runnable)?;
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(EngineError::Failed(Failure {
        command: command.clone(),
        code: output.status.code(),
        output: both(&String::from_utf8_lossy(&output.stdout), &String::from_utf8_lossy(&output.stderr)),
    }))
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
            Line::Out(text) | Line::Err(text) => text,
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
    use super::{EngineError, capture, feed, stream};
    use crate::engine::EngineCommand;
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// A command that needs no container engine: every machine QCode runs on has a shell or
    /// `cmd`, which is enough to check how output, failure and cancelling are handled.
    #[cfg(unix)]
    fn shell(script: &str) -> EngineCommand {
        EngineCommand { program: PathBuf::from("/bin/sh"), args: vec![OsString::from("-c"), OsString::from(script)] }
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
        let command = EngineCommand { program: PathBuf::from("/qcode/no/such/engine"), args: Vec::new() };
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
}
