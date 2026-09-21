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
    use std::io::Read;
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
            Line::Out(text) | Line::Err(text) => printable(&text),
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

/// A line an engine printed, as a terminal would have left it on screen: what a carriage return
/// wrote over is gone, colour and cursor sequences are taken out, a tab becomes spaces and no
/// other control character is left.
///
/// Engines print for a terminal. Docker ends its "Sending build context" line with `\r\r`, and a
/// build step's own program may colour its output; a log that draws what it is handed cannot
/// draw a control character at all.
#[must_use]
pub fn printable(text: &str) -> String {
    // What stands after the last carriage return is what a terminal shows; a line that ends in
    // one leaves what was written before it.
    let shown = text.split('\r').rev().find(|part| !part.is_empty()).unwrap_or_default();
    let mut out = String::with_capacity(shown.len());
    let mut chars = shown.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // An escape sequence: `ESC [` up to its final letter, or `ESC` and one character.
            '\u{1b}' => {
                if chars.next_if_eq(&'[').is_some() {
                    while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
                } else {
                    chars.next();
                }
            }
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
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
    use super::{EngineError, capture, feed, pipe, printable, stream};
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

    #[test]
    fn a_line_is_kept_the_way_a_terminal_would_have_shown_it() {
        // What docker 29's classic builder prints before every build.
        assert_eq!(
            printable("Sending build context to Docker daemon  2.048kB\r\r"),
            "Sending build context to Docker daemon  2.048kB"
        );
        assert_eq!(printable("10%\r50%\r100%"), "100%");
        assert_eq!(printable("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        assert_eq!(printable("a\tb\u{7}c"), "a    bc");
        assert_eq!(printable("STEP 1/4: FROM qcode/base"), "STEP 1/4: FROM qcode/base");
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
