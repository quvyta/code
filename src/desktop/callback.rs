//! The way back of a sign-in: the page opens in the person's own browser, and the answer the
//! browser is sent back with is carried to the application inside its container.
//!
//! An application that signs in with Google — Antigravity IDE is the first — listens on a port of
//! its own `localhost` and asks for a page whose `redirect_uri` names that port. When the person
//! has signed in, Google sends their browser to `http://localhost:<port>/<path>?code=…`, and the
//! application takes the code from that request.
//!
//! Showing the page inside the container would keep that `localhost` the application's, and QCode
//! did that first. Google refuses it: a browser window started by an application is turned away
//! with "Couldn't sign you in — This browser or app may not be secure." Nothing that pretends to be
//! another browser is tried here; the page goes to the person's own browser, where they are often
//! signed in already, and the way back is carried instead.
//!
//! The carrying has three parts:
//!
//! - [`return_to`] reads the address the application asked to open and finds the port and path
//!   the sign-in comes back to, looking through a `redirect_uri` that is encoded, or nested inside
//!   another address, as sign-in pages like to do. An address whose way back is not the machine's
//!   own `localhost` over plain `http` has nothing to carry, and nothing is listened for.
//! - [`Listener::bind`] listens on that port on this machine's loopback, `127.0.0.1` and, for a
//!   way back named `localhost`, `[::1]` too: a browser resolves `localhost` to both and may try
//!   either first, and a program of someone else's on the other one would be handed the code. When
//!   the port is taken, nothing is opened; another port cannot be chosen, because Google sends the
//!   browser to exactly the address it was given.
//! - Each connection the browser makes is carried by [`carrier`]: `<engine> exec -i <container>
//!   node -e …`, a program of twenty lines that connects to the same port inside the container and
//!   copies bytes both ways. The container needs no network for it — loopback is there in a
//!   container with none — and nothing is ever carried to anything but that one port.
//!
//! The listening ends when the application has answered a request for the path of the way back
//! (the sign-in has arrived), when the tab stops it (the window closed, or the person chose the
//! window of last resort instead), or after [`WAIT`], which is long enough to find a phone for the
//! second step and short enough that a port is not held for an afternoon.
//!
//! Every harness whose sign-in comes back to `localhost` can use the same road — Antigravity's
//! command-line `agy`, Gemini CLI, and opencode or Codex where their sign-in works the same way.
//! What differs is only where the address comes from: a desktop application runs `xdg-open`, which
//! reaches [`super::signin::OPEN_PROGRAM`]; a harness in a terminal prints the address instead,
//! and the address would have to be read from what it prints. [`carrier`] relies on nothing but
//! `node`, which every image QCode builds carries.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::engine::{Engine, EngineCommand, Exec};

/// How long a sign-in's way back is listened for before QCode gives the port up again.
pub const WAIT: Duration = Duration::from_secs(10 * 60);

/// How often the listener looks for a new connection and for being stopped. A browser's
/// connection waits in the system's queue meanwhile, so this is only how late it is taken.
const POLL: Duration = Duration::from_millis(100);

/// How many layers of encoding an address is looked through for its way back.
const DEPTH: usize = 4;

/// How much of a request is read before its first line is given up on.
const FIRST_LINE: usize = 16 * 1024;

/// Which of the machine's own addresses a way back names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loopback {
    /// `localhost`: either of the two below, whichever the browser tries.
    Localhost,
    /// `127.0.0.1`.
    V4,
    /// `[::1]`.
    V6,
}

impl Loopback {
    /// The addresses to listen on for this way back, and to connect to inside the container.
    fn addresses(self) -> &'static [&'static str] {
        match self {
            Self::Localhost => &["127.0.0.1", "::1"],
            Self::V4 => &["127.0.0.1"],
            Self::V6 => &["::1"],
        }
    }
}

/// Where a sign-in comes back to: a port of the application's `localhost`, and the path it asks
/// for there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Return {
    /// The port the application listens on inside its container, and QCode on this machine.
    pub port: u16,
    /// The path of the way back, without its query: the request for it is the sign-in arriving.
    pub path: String,
    /// Which loopback address the way back names.
    pub host: Loopback,
}

/// Where the sign-in of `address` comes back to, when it comes back to `localhost`.
///
/// The `redirect_uri` is looked for in the address as it is and then in the address decoded, a
/// layer at a time, so one nested inside another address's query is found as well. `None` when
/// there is none, or when it leads anywhere but plain `http` on the machine's own loopback with a
/// port of its own.
#[must_use]
pub fn return_to(address: &str) -> Option<Return> {
    let mut text = address.to_owned();
    for _ in 0..DEPTH {
        let found = text
            .split(['?', '&', '#'])
            .filter_map(|part| part.strip_prefix("redirect_uri="))
            .find_map(|value| local(&decoded(value)));
        if found.is_some() {
            return found;
        }
        let next = decoded(&text);
        if next == text {
            return None;
        }
        text = next;
    }
    None
}

/// The way back `url` names, when it is plain `http` to a port of the machine's own loopback.
fn local(url: &str) -> Option<Return> {
    let scheme = url.get(..7).filter(|scheme| scheme.eq_ignore_ascii_case("http://"))?;
    let rest = &url[scheme.len()..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    // A name and a password before the host could make `localhost` mean something else entirely.
    if authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (inside, port) = bracketed.split_once("]:")?;
            (format!("[{inside}]"), port)
        }
        None => authority.rsplit_once(':').map(|(host, port)| (host.to_owned(), port))?,
    };
    let host = match host.to_ascii_lowercase().as_str() {
        "localhost" => Loopback::Localhost,
        "127.0.0.1" => Loopback::V4,
        "[::1]" => Loopback::V6,
        _ => return None,
    };
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    let path = tail.split(['?', '#']).next().filter(|path| !path.is_empty()).unwrap_or("/");
    Some(Return { port, path: path.to_owned(), host })
}

/// `text` with its `%XX` escapes turned back into the bytes they stand for. An escape that is not
/// one is left as it is.
fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The program that carries one connection inside the container: it connects to the port on the
/// container's loopback, trying each address it is given in turn, and copies its standard input
/// there and what comes back to its standard output. It ends with code 3 when nothing listens.
///
/// Written for `node -e`, with the port and the addresses as its two arguments, and without a
/// backslash, so that it is the same text in the image, in a test and in an engine's argument.
pub const CARRIER: &str = r"'use strict';
const net = require('net');
const port = Number(process.argv[1]);
const hosts = String(process.argv[2]).split(',');
const reach = (index) => {
  const socket = net.connect({ host: hosts[index], port });
  let open = false;
  socket.on('connect', () => {
    open = true;
    process.stdout.on('error', () => socket.destroy());
    process.stdin.pipe(socket);
    socket.pipe(process.stdout);
  });
  socket.on('error', () => {
    if (!open && index + 1 < hosts.length) {
      reach(index + 1);
    } else if (!open) {
      process.exitCode = 3;
      process.stdin.destroy();
    }
  });
  socket.on('close', () => {
    if (open) {
      process.stdin.destroy();
    }
  });
};
reach(0);
";

/// The command that carries one connection to the way back `back` inside `container`.
///
/// Without a terminal and with standard input open: the bytes go through the pipes, and a
/// terminal would change them.
#[must_use]
pub fn carrier(engine: &Engine, container: &str, back: &Return) -> EngineCommand {
    let port = back.port.to_string();
    let hosts = back.host.addresses().join(",");
    let command = ["node", "-e", CARRIER, port.as_str(), hosts.as_str()];
    engine.exec_reading(&Exec { container, command: &command })
}

/// How a listening ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The application answered the request for the way back: the sign-in reached it.
    Returned,
    /// Nothing came back within [`WAIT`].
    TimedOut,
    /// The tab stopped it.
    Stopped,
}

/// Why a way back could not be listened for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotListening {
    /// Another program of this machine listens on the port already.
    Taken,
    /// The system refused for another reason, in its own words.
    Refused(String),
}

/// A way back listened for on this machine's loopback, not yet served.
#[derive(Debug)]
pub struct Listener {
    sockets: Vec<TcpListener>,
    back: Return,
}

impl Listener {
    /// Listens for `back` on this machine: on `127.0.0.1`, `[::1]` or both, as it names.
    ///
    /// A machine with no IPv6 at all is not a reason to refuse a `localhost` way back, since the
    /// browser there has no `[::1]` to try either; a program of someone else's on `[::1]` is.
    ///
    /// # Errors
    ///
    /// When the port is taken, or the system refuses to listen on it.
    pub fn bind(back: &Return) -> Result<Self, NotListening> {
        let mut sockets = Vec::new();
        for (index, host) in back.host.addresses().iter().enumerate() {
            let address: SocketAddr = match *host {
                "::1" => (Ipv6Addr::LOCALHOST, back.port).into(),
                _ => (Ipv4Addr::LOCALHOST, back.port).into(),
            };
            match TcpListener::bind(address) {
                Ok(socket) => {
                    socket.set_nonblocking(true).map_err(|error| NotListening::Refused(error.to_string()))?;
                    sockets.push(socket);
                }
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Err(NotListening::Taken),
                Err(_) if index > 0 && back.host == Loopback::Localhost => {}
                Err(error) => return Err(NotListening::Refused(error.to_string())),
            }
        }
        Ok(Self { sockets, back: back.clone() })
    }

    /// The port listened on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.back.port
    }

    /// Carries every connection that arrives to the application with `carrier`, each on a thread
    /// of its own, until the sign-in has come back, `wait` says to stop, or [`WAIT`] has passed.
    ///
    /// `wait` sleeps for the time it is given and answers whether to go on; the time is counted in
    /// those sleeps, so a clock of a test's own decides when the wait is over. The port is given
    /// up when this returns; connections still being carried are finished by their threads.
    pub fn serve(self, carrier: &EngineCommand, mut wait: impl FnMut(Duration) -> bool) -> Ending {
        let answered = Arc::new(AtomicBool::new(false));
        let mut waited = Duration::ZERO;
        loop {
            for socket in &self.sockets {
                while let Ok((stream, _)) = socket.accept() {
                    let (carrier, path, answered) = (carrier.clone(), self.back.path.clone(), Arc::clone(&answered));
                    std::thread::spawn(move || carry(stream, &carrier, &path, &answered));
                }
            }
            if answered.load(Ordering::SeqCst) {
                return Ending::Returned;
            }
            if waited >= WAIT {
                return Ending::TimedOut;
            }
            if !wait(POLL) {
                return Ending::Stopped;
            }
            waited += POLL;
        }
    }
}

/// Carries one connection of the browser's to the application and back, and marks `answered`
/// when this connection asked for the way back and the application began its answer.
///
/// The browser's bytes go in on a thread of their own while the answer comes out here, so neither
/// side waits for the other; when the browser has finished sending, the application is told so,
/// and when the application has finished answering, the browser's connection is closed.
fn carry(browser: TcpStream, carrier: &EngineCommand, path: &str, answered: &AtomicBool) {
    // A connection taken from a listener that does not wait must itself wait for its bytes.
    let _ = browser.set_nonblocking(false);
    let Ok(mut child) = Command::new(&carrier.program)
        .args(&carrier.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let (Some(mut into), Some(mut out), Ok(mut from_browser)) =
        (child.stdin.take(), child.stdout.take(), browser.try_clone())
    else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let asked = Arc::new(AtomicBool::new(false));
    let up = {
        let (asked, path) = (Arc::clone(&asked), path.to_owned());
        std::thread::spawn(move || {
            let mut first = Vec::new();
            let mut looked = false;
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                let count = match from_browser.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                if !looked {
                    first.extend_from_slice(&buffer[..count]);
                    if let Some(line) = first_line(&first) {
                        asked.store(asks_for(line, &path), Ordering::SeqCst);
                        looked = true;
                    } else if first.len() > FIRST_LINE {
                        looked = true;
                    }
                }
                if into.write_all(&buffer[..count]).is_err() {
                    break;
                }
            }
            // Dropping the pipe is what tells the application the browser has finished.
        })
    };
    let mut browser = browser;
    let mut buffer = [0_u8; 16 * 1024];
    let mut finished = false;
    loop {
        match out.read(&mut buffer) {
            Ok(0) => {
                finished = true;
                break;
            }
            Err(_) => break,
            Ok(count) => {
                // Marked before the browser sees a byte of it: the request reached the
                // application, which is answering.
                if asked.load(Ordering::SeqCst) {
                    answered.store(true, Ordering::SeqCst);
                }
                if browser.write_all(&buffer[..count]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = browser.shutdown(Shutdown::Both);
    let _ = up.join();
    drop(out);
    if !finished {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// The first line of a request, once all of it has arrived.
fn first_line(bytes: &[u8]) -> Option<&str> {
    let end = bytes.iter().position(|byte| *byte == b'\n')?;
    std::str::from_utf8(&bytes[..end]).ok().map(str::trim_end)
}

/// Whether the request line `line` asks for `path`, whatever its query.
fn asks_for(line: &str, path: &str) -> bool {
    let mut words = line.split(' ');
    let (Some(_method), Some(target)) = (words.next(), words.next()) else { return false };
    target.split(['?', '#']).next() == Some(path)
}

/// Stops a listening when it is dropped: a tab holds one while its sign-in's way back is
/// listened for, so a tab that closes, or moves on, gives the port up by itself.
#[derive(Debug)]
pub struct Stop {
    flag: Arc<AtomicBool>,
    id: u64,
}

impl Stop {
    /// A stop not yet pulled, with the flag the listening watches.
    #[must_use]
    pub fn new() -> (Self, Arc<AtomicBool>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let flag = Arc::new(AtomicBool::new(false));
        (Self { flag: Arc::clone(&flag), id: NEXT.fetch_add(1, Ordering::Relaxed) }, flag)
    }

    /// Which listening this is, so the answer of one stopped earlier is told apart.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::{Ending, Listener, Loopback, NotListening, Return, Stop, WAIT, asks_for, decoded, return_to};
    use crate::engine::{Engine, EngineCommand, EngineKind};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    fn back(port: u16, path: &str, host: Loopback) -> Return {
        Return { port, path: path.to_owned(), host }
    }

    #[test]
    fn a_google_address_names_the_port_and_path_it_comes_back_to() {
        let address = "https://accounts.google.com/o/oauth2/v2/auth?client_id=x.apps.googleusercontent.com\
                       &redirect_uri=http%3A%2F%2Flocalhost%3A45049%2Foauth-callback&response_type=code&state=s";
        assert_eq!(return_to(address), Some(back(45049, "/oauth-callback", Loopback::Localhost)));
    }

    #[test]
    fn a_way_back_written_plainly_or_to_either_loopback_is_found() {
        let plain = "https://idp.example/auth?redirect_uri=http://127.0.0.1:8085/cb?x=1&state=s";
        assert_eq!(return_to(plain), Some(back(8085, "/cb", Loopback::V4)));
        let v6 = "https://idp.example/auth?redirect_uri=http%3A%2F%2F%5B%3A%3A1%5D%3A9000";
        assert_eq!(return_to(v6), Some(back(9000, "/", Loopback::V6)), "no path is the root");
        let loud = "https://idp.example/auth?redirect_uri=HTTP%3A%2F%2FLocalHost%3A1455%2Fauth%2Fcallback";
        assert_eq!(return_to(loud), Some(back(1455, "/auth/callback", Loopback::Localhost)));
    }

    #[test]
    fn a_way_back_nested_inside_another_address_is_found() {
        // A sign-in page of the maker's that hands on to Google's, whose own address carries the
        // way back encoded once more.
        let inner =
            "https://accounts.google.com/o/oauth2/auth?redirect_uri=http%3A%2F%2Flocalhost%3A51121%2Foauth-callback";
        let address = format!("https://antigravity.example/signin?continue={}", encoded(inner));
        assert_eq!(return_to(&address), Some(back(51121, "/oauth-callback", Loopback::Localhost)));
    }

    fn encoded(text: &str) -> String {
        text.bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
                _ => format!("%{byte:02X}"),
            })
            .collect()
    }

    #[test]
    fn an_address_that_does_not_come_back_to_this_machine_is_listened_for_nowhere() {
        for address in [
            "https://accounts.google.com/o/oauth2/auth?client_id=x",
            "https://example.com/docs/getting-started",
            "https://idp.example/auth?redirect_uri=https%3A%2F%2Fapp.example%2Fcallback",
            // Not plain http: a browser would speak TLS to it, which nothing here pretends to.
            "https://idp.example/auth?redirect_uri=https%3A%2F%2Flocalhost%3A8443%2Fcb",
            // No port, or a port of nobody's.
            "https://idp.example/auth?redirect_uri=http%3A%2F%2Flocalhost%2Fcb",
            "https://idp.example/auth?redirect_uri=http%3A%2F%2Flocalhost%3A0%2Fcb",
            "https://idp.example/auth?redirect_uri=http%3A%2F%2Flocalhost%3A99999%2Fcb",
            // A host that only looks like the machine's own.
            "https://idp.example/auth?redirect_uri=http%3A%2F%2Flocalhost.evil.example%3A8080%2Fcb",
            "https://idp.example/auth?redirect_uri=http%3A%2F%2Flocalhost%3A8080%40evil.example%2Fcb",
            "https://idp.example/auth?redirect_uri=http%3A%2F%2F10.0.0.1%3A8080%2Fcb",
            // Named something else.
            "https://idp.example/auth?return_to=http%3A%2F%2Flocalhost%3A8080%2Fcb",
        ] {
            assert_eq!(return_to(address), None, "{address}");
        }
    }

    #[test]
    fn an_escape_that_is_not_one_is_left_as_it_is() {
        assert_eq!(decoded("a%2Fb%zz%4"), "a/b%zz%4");
        assert_eq!(decoded("%E2%9C%93"), "\u{2713}");
    }

    #[test]
    fn only_a_request_for_the_path_of_the_way_back_is_the_sign_in_arriving() {
        assert!(asks_for("GET /oauth-callback?code=4%2F0Ab&state=s HTTP/1.1", "/oauth-callback"));
        assert!(asks_for("GET /oauth-callback HTTP/1.1", "/oauth-callback"));
        assert!(!asks_for("GET /favicon.ico HTTP/1.1", "/oauth-callback"));
        assert!(!asks_for("GET /oauth-callback-not HTTP/1.1", "/oauth-callback"));
        assert!(!asks_for("", "/"));
    }

    /// A port nobody on this machine listens on right now.
    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").and_then(|socket| socket.local_addr()).expect("a free port").port()
    }

    #[test]
    fn a_port_another_program_holds_is_said_to_be_taken_and_nothing_else_is_chosen() {
        let holder = TcpListener::bind("127.0.0.1:0").expect("a port of someone else's");
        let port = holder.local_addr().expect("its address").port();
        assert_eq!(Listener::bind(&back(port, "/", Loopback::V4)).err(), Some(NotListening::Taken));
        assert_eq!(Listener::bind(&back(port, "/", Loopback::Localhost)).err(), Some(NotListening::Taken));
    }

    #[test]
    fn a_localhost_way_back_is_listened_for_on_both_loopbacks_where_the_machine_has_both() {
        let port = free_port();
        let listener = Listener::bind(&back(port, "/", Loopback::Localhost)).expect("it listens");
        assert_eq!(listener.port(), port);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        if TcpListener::bind("[::1]:0").is_ok() {
            assert!(TcpStream::connect(("::1", port)).is_ok(), "a browser that tries [::1] first finds it too");
        }
    }

    /// Whether `port` can be listened on again. Asked a few times over a generous while, because
    /// a test beside this one may hold the same port for an instant while it looks for a free one.
    fn given_up(port: u16) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A stand-in carrier: a script that writes the request it is given into `seen` and answers
    /// with `answer`, the way the application inside would.
    fn stand_in(dir: &Path, answer: &str) -> (EngineCommand, PathBuf) {
        let seen = dir.join("seen");
        let script = dir.join("carrier");
        std::fs::write(&script, format!("#!/bin/sh\nhead -n 1 > '{}'\nprintf '{answer}'\n", seen.display()))
            .expect("a stand-in carrier");
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        (EngineCommand { program: script, args: Vec::new() }, seen)
    }

    fn folder(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-callback-{name}-{stamp}"));
        std::fs::create_dir_all(&path).expect("a folder of this test's own");
        path
    }

    /// Plays the browser: one request to the port, and everything the answer says.
    fn browse(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the listener takes it");
        stream.set_read_timeout(Some(Duration::from_secs(60))).expect("a finite wait");
        write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").expect("the request goes");
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer
    }

    #[test]
    fn the_request_for_the_way_back_is_carried_in_its_answer_carried_out_and_the_port_given_up() {
        let dir = folder("carried");
        let (carrier, seen) = stand_in(&dir, "HTTP/1.1 200 OK\\r\\nContent-Length: 9\\r\\n\\r\\nsigned-in");
        let port = free_port();
        let listener = Listener::bind(&back(port, "/oauth-callback", Loopback::V4)).expect("it listens");
        let serving = std::thread::spawn(move || {
            listener.serve(&carrier, |pause| {
                std::thread::sleep(pause);
                true
            })
        });
        // Something else first, which is carried as well but is not the sign-in.
        let favicon = browse(port, "/favicon.ico");
        assert!(favicon.ends_with("signed-in"), "{favicon}");
        assert!(!serving.is_finished(), "a request for another path is not the sign-in");
        let answer = browse(port, "/oauth-callback?code=4%2F0Ab-7&state=s");
        assert!(answer.starts_with("HTTP/1.1 200 OK") && answer.ends_with("signed-in"), "{answer}");
        assert_eq!(
            std::fs::read_to_string(&seen).expect("the request was carried").trim_end(),
            "GET /oauth-callback?code=4%2F0Ab-7&state=s HTTP/1.1"
        );
        assert_eq!(serving.join().expect("the listener ends"), Ending::Returned);
        assert!(given_up(port), "the port is given up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_coming_back_ends_the_listening_after_the_wait_counted_in_its_own_sleeps() {
        let port = free_port();
        let listener = Listener::bind(&back(port, "/cb", Loopback::V4)).expect("it listens");
        let mut slept = Duration::ZERO;
        let carrier = EngineCommand { program: PathBuf::from("/qcode-nothing-here"), args: Vec::new() };
        let ending = listener.serve(&carrier, |pause| {
            slept += pause;
            true
        });
        assert_eq!(ending, Ending::TimedOut);
        assert_eq!(slept, WAIT, "exactly the wait, and not a moment of it spent really sleeping");
        assert!(given_up(port), "the port is given up");
    }

    #[test]
    fn a_stop_that_is_dropped_ends_the_listening_and_gives_the_port_up() {
        let port = free_port();
        let listener = Listener::bind(&back(port, "/cb", Loopback::V4)).expect("it listens");
        let (stop, flag) = Stop::new();
        let carrier = EngineCommand { program: PathBuf::from("/qcode-nothing-here"), args: Vec::new() };
        let serving = std::thread::spawn(move || {
            listener.serve(&carrier, |pause| {
                std::thread::sleep(pause);
                !flag.load(Ordering::SeqCst)
            })
        });
        // The tab that held it closes.
        drop(stop);
        assert_eq!(serving.join().expect("the listener ends"), Ending::Stopped);
        assert!(given_up(port), "the port is given up");
    }

    #[test]
    fn the_carrier_is_node_inside_the_container_with_no_terminal_and_the_way_back_as_its_words() {
        let engine = Engine::new(EngineKind::Podman, Path::new("/usr/bin/podman"));
        let command = super::carrier(&engine, "qcode-firefly-anti.desk", &back(45049, "/cb", Loopback::Localhost));
        let words: Vec<String> = command.args.iter().map(|word| word.to_string_lossy().into_owned()).collect();
        assert_eq!(words[..5], ["exec", "--interactive", "qcode-firefly-anti.desk", "node", "-e"]);
        assert_eq!(words[5], super::CARRIER);
        assert_eq!(words[6..], ["45049", "127.0.0.1,::1"]);
        assert!(!words.iter().any(|word| word == "--tty"), "a terminal would change the bytes");
        assert!(!super::CARRIER.contains('\\'));
    }
}
