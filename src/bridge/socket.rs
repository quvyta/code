//! The workspace's socket: QCode listens on it while the workspace is open, and the server in every
//! profile container of the workspace asks its questions through it.
//!
//! A unix socket rather than a port, because a profile without the network has no network to
//! reach a port through, while a socket is a file in a folder the container mounts. One thread
//! waits for connections and one short thread serves each: it reads the question, hands it to
//! the screen through the [`Inbox`] and writes back whatever the screen answers. The screen
//! decides everything; this module only carries lines.
//!
//! Where the system has no unix sockets, [`Listener::open`] says so and the workspace has no
//! bridge; nothing else changes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::protocol::{Answer, Malformed, Question};
use super::{SCRIPT, SCRIPT_NAME, SOCKET_NAME};

/// How long a connection may take to send its question. The server writes it at once; a
/// connection that sends nothing is not one of ours.
const QUESTION_WAIT: Duration = Duration::from_secs(10);

/// How long a connection waits for the screen's answer. The screen answers every question
/// itself, a question for the person included (it says the message waits for them when they
/// take long), so this only ends a connection whose answer was lost with the screen.
const ANSWER_WAIT: Duration = Duration::from_secs(100);

/// The most connections served at once. Every tab asks one question at a time; more than this
/// is something in a container knocking, and the rest are closed unanswered.
const MOST_CONNECTIONS: usize = 16;

/// A question that came in, and the way back to the one who asked.
#[derive(Debug, Clone)]
pub struct Call {
    /// The question, or [`Malformed`] when the line was not one.
    pub question: Result<Question, Malformed>,
    reply: Sender<Answer>,
}

impl Call {
    /// A call carrying `question`, with the receiving end of its answer; for the screen's tests,
    /// which have no socket.
    #[must_use]
    pub fn new(question: Result<Question, Malformed>) -> (Self, Receiver<Answer>) {
        let (reply, answers) = mpsc::channel();
        (Self { question, reply }, answers)
    }

    /// Answers the call. A connection that went away meanwhile is not an error: the agent that
    /// asked is gone.
    pub fn answer(&self, answer: Answer) {
        let _ = self.reply.send(answer);
    }
}

/// Where the screen waits for the next call.
#[derive(Debug, Clone)]
pub struct Inbox(Arc<Mutex<Receiver<Call>>>);

impl Inbox {
    /// Waits for the next call, on a background thread. `None` once the listener is closed.
    #[must_use]
    pub fn next(&self) -> Option<Call> {
        let receiver = self.0.lock().ok()?;
        receiver.recv().ok()
    }
}

/// A workspace's socket, listened on for as long as this value lives.
#[derive(Debug)]
pub struct Listener {
    socket: PathBuf,
    inbox: Inbox,
    #[cfg(unix)]
    open: Arc<std::sync::atomic::AtomicBool>,
}

impl Listener {
    /// Listens in `folder`, the workspace's `Containers/MCP/`: makes the folder, writes the
    /// server beside where the socket goes, and starts waiting for connections.
    ///
    /// A socket left behind by a QCode that ended without clearing it is replaced; one another
    /// QCode still answers on is left to it, and this QCode has no bridge for the workspace.
    ///
    /// # Errors
    ///
    /// When the folder or the server cannot be written, another QCode answers on the socket
    /// ([`io::ErrorKind::AddrInUse`]), the socket cannot be made, or the system has none.
    pub fn open(folder: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            unix::open(folder)
        }
        #[cfg(not(unix))]
        {
            let _ = folder;
            Err(io::Error::new(io::ErrorKind::Unsupported, "this system has no unix sockets"))
        }
    }

    /// Where the socket is.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Where the screen waits for calls.
    #[must_use]
    pub fn inbox(&self) -> Inbox {
        self.inbox.clone()
    }
}

/// Writes the server into `folder` unless it is there already as this QCode carries it, so a
/// workspace opened again rewrites nothing.
fn write_script(folder: &Path) -> io::Result<()> {
    let path = folder.join(SCRIPT_NAME);
    if std::fs::read(&path).is_ok_and(|written| written == SCRIPT.as_bytes()) {
        return Ok(());
    }
    qframe::storage::atomic_write(&path, SCRIPT.as_bytes())
}

#[cfg(unix)]
mod unix {
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use super::super::protocol::{self, MOST_LINE, Malformed};
    use super::{ANSWER_WAIT, Call, Inbox, Listener, MOST_CONNECTIONS, QUESTION_WAIT, SOCKET_NAME, write_script};

    /// The longest socket path the system takes, less the byte its terminator needs. A path the
    /// store makes longer is reached through the folder's handle instead (see [`reach`]).
    const MOST_PATH: usize = 107;

    pub(super) fn open(folder: &Path) -> io::Result<Listener> {
        std::fs::create_dir_all(folder)?;
        // Only the person's own processes and the containers running as them reach the socket.
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        write_script(folder)?;
        let socket = folder.join(SOCKET_NAME);
        if socket.exists() {
            if reach(&socket, |path| UnixStream::connect(path)).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, socket.display().to_string()));
            }
            std::fs::remove_file(&socket)?;
        }
        let listener = reach(&socket, |path| UnixListener::bind(path))?;
        let (calls, received) = mpsc::channel();
        let open = Arc::new(AtomicBool::new(true));
        let still_open = Arc::clone(&open);
        std::thread::Builder::new()
            .name("qcode-bridge".to_owned())
            .spawn(move || accept(&listener, &calls, &still_open))?;
        Ok(Listener { socket, inbox: Inbox(Arc::new(Mutex::new(received))), open })
    }

    /// Runs `with` on the socket path, or, when the path is too long for a socket address, on the
    /// same socket named through the folder's open handle in `/proc/self/fd`, which is short.
    fn reach<T>(socket: &Path, with: impl Fn(&Path) -> io::Result<T>) -> io::Result<T> {
        if socket.as_os_str().len() <= MOST_PATH {
            return with(socket);
        }
        let folder = socket.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let handle = std::fs::File::open(folder)?;
        let short = PathBuf::from(format!("/proc/self/fd/{}/{SOCKET_NAME}", std::os::fd::AsRawFd::as_raw_fd(&handle)));
        let result = with(&short);
        drop(handle);
        result
    }

    /// Waits for connections until the listener is closed, serving each on a thread of its own.
    fn accept(listener: &UnixListener, calls: &Sender<Call>, open: &AtomicBool) {
        let serving = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            if !open.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            if serving.load(Ordering::SeqCst) >= MOST_CONNECTIONS {
                continue;
            }
            serving.fetch_add(1, Ordering::SeqCst);
            let (calls, done) = (calls.clone(), Arc::clone(&serving));
            let spawned = std::thread::Builder::new().name("qcode-bridge-call".to_owned()).spawn(move || {
                serve(stream, &calls);
                done.fetch_sub(1, Ordering::SeqCst);
            });
            if spawned.is_err() {
                serving.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Reads one question, hands it on and writes the answer back.
    fn serve(stream: UnixStream, calls: &Sender<Call>) {
        if stream.set_read_timeout(Some(QUESTION_WAIT)).is_err() {
            return;
        }
        let Ok(reading) = stream.try_clone() else { return };
        let mut line = String::new();
        let limit = u64::try_from(MOST_LINE).unwrap_or(u64::MAX) + 1;
        let read = BufReader::new(reading.take(limit)).read_line(&mut line);
        let question = match read {
            Ok(_) if line.ends_with('\n') => protocol::parse(line.trim_end_matches(['\n', '\r'])),
            _ => Err(Malformed),
        };
        let (call, answers) = Call::new(question);
        if calls.send(call).is_err() {
            return;
        }
        if let Ok(answer) = answers.recv_timeout(ANSWER_WAIT) {
            let mut writing = stream;
            let _ = writing.write_all(answer.line().as_bytes());
        }
    }

    impl Drop for Listener {
        /// Stops listening and takes the socket away. The thread waiting for connections is
        /// woken by one last connection of its own, so it sees the listener is closed and ends.
        fn drop(&mut self) {
            self.open.store(false, Ordering::SeqCst);
            let _ = reach(&self.socket, |path| UnixStream::connect(path));
            let _ = std::fs::remove_file(&self.socket);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    use super::super::protocol::{Answer, Malformed, Question, Request};
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let stamp =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            Self(std::env::temp_dir().join(format!("qcode-bridge-{name}-{stamp}")))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Asks `socket` one line the way the server in a container does, and reads the answer.
    fn ask(socket: &Path, line: &str) -> String {
        let mut stream = connect(socket);
        stream.write_all(line.as_bytes()).expect("the question is written");
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).expect("an answer comes");
        answer
    }

    fn connect(socket: &Path) -> UnixStream {
        if socket.as_os_str().len() <= 107 {
            return UnixStream::connect(socket).expect("the socket answers");
        }
        let handle = std::fs::File::open(socket.parent().expect("a folder")).expect("the folder opens");
        let short = format!("/proc/self/fd/{}/{SOCKET_NAME}", std::os::fd::AsRawFd::as_raw_fd(&handle));
        UnixStream::connect(short).expect("the socket answers")
    }

    /// Answers every call the way `answer` says, on a thread, until the listener closes.
    fn answering(inbox: Inbox, answer: fn(&Result<Question, Malformed>) -> Answer) -> std::thread::JoinHandle<usize> {
        std::thread::spawn(move || {
            let mut served = 0;
            while let Some(call) = inbox.next() {
                call.answer(answer(&call.question));
                served += 1;
            }
            served
        })
    }

    fn echo(question: &Result<Question, Malformed>) -> Answer {
        match question {
            Ok(Question { request: Request::Send { text, .. }, .. }) => Answer::done(text.clone()),
            Ok(Question { request: Request::List, .. }) => Answer::listed("none".to_owned(), Vec::new()),
            Err(Malformed) => Answer::refused("malformed".to_owned()),
        }
    }

    #[test]
    fn a_question_reaches_the_screen_and_its_answer_comes_back() {
        let scratch = Scratch::new("round");
        let listener = Listener::open(&scratch.0).expect("the socket opens");
        assert!(scratch.0.join(SCRIPT_NAME).exists(), "the server is written beside the socket");
        let served = answering(listener.inbox(), echo);

        let answer = ask(listener.socket(), "{\"token\":\"t\",\"op\":\"send\",\"tab\":\"2\",\"text\":\"hello\"}\n");
        assert_eq!(answer, "{\"ok\":true,\"text\":\"hello\"}\n");
        let answer = ask(listener.socket(), "not json\n");
        assert_eq!(answer, "{\"ok\":false,\"text\":\"malformed\"}\n");

        let socket = listener.socket().to_path_buf();
        drop(listener);
        assert_eq!(served.join().expect("the answering thread ends"), 2, "closing ends the inbox");
        assert!(!socket.exists(), "closing takes the socket away");
    }

    #[test]
    fn a_line_without_its_end_is_malformed_rather_than_waited_for_forever() {
        let scratch = Scratch::new("cut");
        let listener = Listener::open(&scratch.0).expect("the socket opens");
        let served = answering(listener.inbox(), echo);
        let mut stream = connect(listener.socket());
        stream.write_all(b"{\"token\":\"t\",\"op\":\"list\"}").expect("written");
        stream.shutdown(std::net::Shutdown::Write).expect("the writing side closes");
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).expect("an answer comes");
        assert_eq!(answer, "{\"ok\":false,\"text\":\"malformed\"}\n");
        drop(listener);
        let _ = served.join();
    }

    #[test]
    fn a_socket_another_qcode_answers_on_is_left_to_it() {
        let scratch = Scratch::new("twice");
        let first = Listener::open(&scratch.0).expect("the socket opens");
        let second = Listener::open(&scratch.0);
        assert_eq!(second.map(|_| ()).map_err(|error| error.kind()), Err(io::ErrorKind::AddrInUse));
        let served = answering(first.inbox(), echo);
        assert!(ask(first.socket(), "{\"token\":\"t\",\"op\":\"list\"}\n").contains("\"ok\":true"));
        drop(first);
        let _ = served.join();
    }

    /// Waits until nothing answers on `socket` any more, which is what a socket left behind by a
    /// QCode that ended is.
    ///
    /// Dropping the listener is not yet that, in a test binary. Other tests start processes on
    /// other threads, and a process being started is a copy of this one until it runs its
    /// program: for that moment it holds every descriptor this one has, the listener just dropped
    /// included, and the socket still takes connections. `Listener::open` then finds it answered
    /// and rightly leaves it alone. Measured: 13 of 2000 opens right after a drop found it
    /// answered while another thread started `true` over and over, none of 2000 when nothing was
    /// started, and none of 2000 with this wait in between. A QCode
    /// that ended has no such copy left, so the product has nothing to wait for; the test waits
    /// for the state it means to set up, for as long as a loaded machine could take.
    fn refused(socket: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while UnixStream::connect(socket).is_ok() {
            assert!(std::time::Instant::now() < deadline, "something still answers on {}", socket.display());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_socket_left_behind_is_replaced() {
        let scratch = Scratch::new("stale");
        std::fs::create_dir_all(&scratch.0).expect("a folder");
        let stale = std::os::unix::net::UnixListener::bind(scratch.0.join(SOCKET_NAME)).expect("a socket");
        drop(stale);
        refused(&scratch.0.join(SOCKET_NAME));
        let listener = Listener::open(&scratch.0).expect("the stale socket is replaced");
        let served = answering(listener.inbox(), echo);
        assert!(ask(listener.socket(), "{\"token\":\"t\",\"op\":\"list\"}\n").contains("none"));
        drop(listener);
        let _ = served.join();
    }

    #[test]
    fn a_store_deep_enough_to_outgrow_a_socket_address_still_gets_its_socket() {
        let scratch = Scratch::new("deep");
        let deep = scratch.0.join("a-folder-name-long-enough".repeat(4)).join("Containers").join("MCP");
        assert!(deep.join(SOCKET_NAME).as_os_str().len() > 108, "{}", deep.display());
        let listener = Listener::open(&deep).expect("the socket opens");
        assert_eq!(listener.socket(), deep.join(SOCKET_NAME));
        assert!(listener.socket().exists());
        let served = answering(listener.inbox(), echo);
        assert!(ask(listener.socket(), "{\"token\":\"t\",\"op\":\"list\"}\n").contains("none"));
        drop(listener);
        let _ = served.join();
    }

    #[test]
    fn the_folder_is_the_persons_alone_and_the_server_is_written_once() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new("folder");
        let listener = Listener::open(&scratch.0).expect("the socket opens");
        let mode = std::fs::metadata(&scratch.0).expect("the folder").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert_eq!(std::fs::read_to_string(scratch.0.join(SCRIPT_NAME)).expect("the server"), SCRIPT);
        drop(listener);
    }
}
