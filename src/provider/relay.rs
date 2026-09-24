//! The relay that lets a harness inside a container talk to a provider without the provider's
//! key ever entering that container.
//!
//! A profile container has no network of its own, so a harness inside it cannot reach a provider
//! directly. [`assets/provider/qcode-relay.mjs`](../../../assets/provider/qcode-relay.mjs) is a
//! small HTTP server the container runs on its loopback interface; a harness is pointed at it the
//! way it would be pointed at the provider itself. The script forwards every request over a unix
//! socket to this module, which asks the real provider with the tab's key added, and streams the
//! answer back.
//!
//! This is the same shape as [`crate::bridge`]: a socket in the workspace's `Containers/MCP/`
//! folder ([`crate::base::paths::MCP_DIR`] in the container), a script written beside it, and a
//! token every harness tab already carries in [`crate::bridge::TOKEN_VARIABLE`] so QCode knows
//! which tab is asking and therefore which provider entry to use. No second token is minted: a
//! tab has one identity, and the bridge already gives it one nobody outside this process can
//! guess.
//!
//! # Framing
//!
//! One connection to the socket carries one request and its answer. The container writes one
//! line of JSON — the token, the method, the path, the headers it read from the harness minus
//! `authorization`, `x-api-key` and `api-key` — then the request body as raw bytes, ending the write side of
//! the connection when the body is done (a harness's request is small, so this is read whole
//! before it goes on). QCode reads that line, decides whether to carry the request at all, and
//! if so writes back its own line of JSON — the status and the headers the provider answered
//! with — followed by the answer's body as raw bytes, copied across as they arrive rather than
//! collected first. A line of JSON for what fits on one line and needs to be decided before
//! anything else, raw bytes for a body that must never wait to be whole: the same trade [`crate::
//! bridge::protocol`] makes, and a harness reading an answer as server-sent events depends on it
//! exactly the way a tab's own agent depends on the bridge answering promptly.
//!
//! QCode never has to speak HTTP to do this: the container speaks HTTP to the harness and to
//! nothing else, and everything on this side of the socket is the small framing above.
//!
//! # What is allowed
//!
//! A connection may send a message (`POST`, at the path [`crate::provider::Wire::messages_path`]
//! names for either shape) or list the provider's models (`GET /v1/models`, which both shapes
//! serve at the same path). Nothing else is carried: the relay forwards one conversation, not
//! whatever address a container asks for.
//!
//! Which of the two message paths a request takes is the harness's to say, not the provider
//! entry's. Nothing translates between the shapes, so the only shape that can work is the one
//! the harness speaks — Claude Code the Anthropic one, opencode the OpenAI one — and every kind
//! of provider QCode knows serves both. Holding a tab to its provider's recorded shape would only
//! refuse the one request its harness can make. The shape does decide where the request goes,
//! since one service answers the two shapes under different roots ([`crate::provider::
//! ProviderKind::api_root`]), and the key goes in the header the entry's kind reads
//! ([`crate::provider::ProviderKind::key_header`]).

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::ProviderEntry;
use super::ask::{Method, Secret};

/// The name of the relay's script in `Containers/MCP/`, beside the bridge's.
pub const SCRIPT_NAME: &str = "qcode-relay.mjs";

/// The name of the relay's socket in `Containers/MCP/`, beside the bridge's `bridge.sock`.
pub const SOCKET_NAME: &str = "relay.sock";

/// The port the relay listens on inside the container, on its loopback interface alone. Chosen
/// for being unlikely to be anything else a harness image already runs; the script and the tests
/// here are the only two places this number may appear.
pub const PORT: u16 = 41417;

/// The relay's script, carried inside the binary and written into each open workspace's
/// `Containers/MCP/` folder, the way [`crate::bridge::SCRIPT`] is.
pub const SCRIPT: &str = include_str!("../../assets/provider/qcode-relay.mjs");

/// Where the relay's script is inside a profile container, in the folder the workspace's
/// `Containers/MCP/` is mounted at.
#[must_use]
pub fn script_in_container() -> String {
    format!("{}/{SCRIPT_NAME}", crate::base::paths::MCP_DIR)
}

/// The program a tab of a provider profile runs: the relay, with the harness after it.
///
/// The relay starts the harness itself, once its server is listening, and lives exactly as long
/// as the harness does. Started beside it instead, the harness could ask before the server was
/// up and take the refusal for a provider that does not work.
#[must_use]
pub fn wrapping(harness: &[String]) -> Vec<String> {
    let mut command = vec!["node".to_owned(), script_in_container()];
    command.extend(harness.iter().cloned());
    command
}

/// How long a connection may take to send its head line and body, and how long QCode waits for
/// the provider to answer before giving up on it. Generous, because the provider may have to
/// load a model first; finite, because a container waiting forever is a container stuck.
const PATIENCE: Duration = Duration::from_secs(120);

/// The longest a head line may be. Wide enough for a generous set of headers, narrow enough that
/// a line this long is never mistaken for a request that meant to carry a body on it.
const MOST_HEAD: usize = 64 * 1024;

/// The most connections served at once, mirroring [`crate::bridge::socket`]'s limit: one per tab
/// asking at a time, and the rest are something else knocking.
const MOST_CONNECTIONS: usize = 16;

/// The headers a request or an answer never carries across the relay because the framing on
/// either side already speaks for them, or because letting them through would carry someone
/// else's idea of authentication into a request QCode is about to add its own key to.
///
/// `api-key` is among them because it is the header Xiaomi reads a key from: a harness that was
/// given one of its own must not have it reach a provider beside the key QCode adds.
const STRIPPED: [&str; 7] =
    ["authorization", "x-api-key", "api-key", "host", "content-length", "transfer-encoding", "connection"];

/// One request read off the socket, before it is decided whether to carry it anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Incoming {
    /// The token of the tab asking, as the relay script found it.
    token: String,
    /// `GET` or `POST`; anything else is refused before a provider is even looked up.
    method: String,
    /// The path asked for, exactly as the harness sent it, query string included.
    path: String,
    /// The headers the harness sent, already without its own `authorization` or `x-api-key`.
    headers: Vec<(String, String)>,
}

/// A head line that was not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Malformed;

/// Reads one head line's worth of JSON.
fn parse_incoming(line: &str) -> Result<Incoming, Malformed> {
    let value: Value = serde_json::from_str(line).map_err(|_| Malformed)?;
    let object = value.as_object().ok_or(Malformed)?;
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned).ok_or(Malformed);
    let token = text("token")?;
    let method = text("method")?;
    let path = text("path")?;
    let headers = match object.get("headers") {
        None => Vec::new(),
        Some(value) => {
            let map = value.as_object().ok_or(Malformed)?;
            map.iter()
                .map(|(name, value)| value.as_str().map(|value| (name.to_lowercase(), value.to_owned())))
                .collect::<Option<Vec<_>>>()
                .ok_or(Malformed)?
        }
    };
    Ok(Incoming { token, method, path, headers })
}

/// Where Codex sends its conversation: OpenAI's Responses shape, the only one Codex still speaks
/// to a provider of one's own (its 0.156.1 program answers `wire_api = "chat"` with "is no longer
/// supported"). It is a conversation in the OpenAI shape like the completion path, so it goes to
/// the same root.
pub const RESPONSES_PATH: &str = "/v1/responses";

/// The path without its query string, which is what a rule about what may be asked checks
/// against; the query string still travels with the request that is actually sent.
fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// Whether `entry` may be asked for `method` `path` through the relay. See the module's doc
/// comment for why these two and nothing else.
fn allowed(method: &str, path: &str) -> bool {
    let path = path_only(path);
    match method {
        "POST" => super::Wire::ALL.iter().any(|wire| path == wire.messages_path()) || path == RESPONSES_PATH,
        "GET" => path == "/v1/models",
        _ => false,
    }
}

/// A line of JSON answering the container, with `status` and the headers the provider (or the
/// relay itself, for a refusal) answered with.
fn head_line(status: u16, headers: &[(String, String)]) -> String {
    let mut object = Map::new();
    object.insert("status".to_owned(), Value::from(status));
    let headers: Map<String, Value> = headers.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    object.insert("headers".to_owned(), Value::Object(headers));
    let mut line = Value::Object(object).to_string();
    line.push('\n');
    line
}

/// A short refusal, written back as a whole answer: the head line, a small JSON body naming why,
/// nothing else. Never the request that was refused and never a key: refusing a connection is not
/// a place either could belong.
fn refusal_body(why: &str) -> Vec<u8> {
    json!({ "error": why }).to_string().into_bytes()
}

/// One request sent on to a provider, with its key already decided.
pub struct UpstreamAsk {
    /// `GET` for a model listing, `POST` for a message.
    pub method: Method,
    /// Where it goes, the provider's base with the requested path (and any query) after it.
    pub url: String,
    /// The headers the harness sent, minus the ones the relay never carries across.
    pub headers: Vec<(String, String)>,
    /// The provider's key, for a provider that needs one.
    pub secret: Option<Secret>,
    /// The request's body, read as it is sent rather than collected first.
    pub body: Box<dyn Read + Send>,
}

/// What a provider answered, read incrementally so an answer streamed as server-sent events is
/// never held back waiting for the rest of it.
pub struct UpstreamAnswer {
    /// The status the provider answered with.
    pub status: u16,
    /// The headers to carry back, minus the ones framing already speaks for.
    pub headers: Vec<(String, String)>,
    /// The body, read as it arrives.
    pub body: Box<dyn Read + Send>,
}

/// Why a provider could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamError {
    /// Where the request was going. Never the key: a key is a header value, never part of an
    /// address, so naming where a request went never names what it carried.
    pub url: String,
    /// What went wrong, in the transport's own words.
    pub reason: String,
}

/// How a request is carried to the provider.
///
/// [`network`](Upstream::network) is the real one. [`new`](Upstream::new) takes anything else,
/// which is what every test here uses, the same seam [`super::ask::Web`] gives the rest of this
/// crate: no test in this module reaches the network, whatever it asks for.
#[derive(Clone)]
pub struct Upstream(Arc<dyn Fn(UpstreamAsk) -> Result<UpstreamAnswer, UpstreamError> + Send + Sync>);

impl std::fmt::Debug for Upstream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Upstream")
    }
}

impl Upstream {
    /// The real one, which goes out over the network and streams the answer back rather than
    /// reading it whole.
    #[must_use]
    pub fn network() -> Self {
        Self::new(|ask| {
            let config =
                ureq::Agent::config_builder().timeout_global(Some(PATIENCE)).http_status_as_error(false).build();
            let agent = ureq::Agent::new_with_config(config);
            let mut builder = match ask.method {
                Method::Get => agent.get(&ask.url).force_send_body(),
                Method::Post => agent.post(&ask.url),
            };
            for (name, value) in &ask.headers {
                builder = builder.header(name, value);
            }
            if let Some(secret) = &ask.secret {
                builder = builder.header(&secret.header, &format!("{}{}", secret.prefix, secret.key.expose()));
            }
            let sent = match ask.method {
                Method::Get => builder.send_empty(),
                Method::Post => builder.send(ureq::SendBody::from_owned_reader(ask.body)),
            };
            let answer = sent.map_err(|error| UpstreamError { url: ask.url.clone(), reason: error.to_string() })?;
            let status = answer.status().as_u16();
            let headers = answer
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    let name = name.as_str().to_lowercase();
                    (!STRIPPED.contains(&name.as_str())).then(|| Some((name, value.to_str().ok()?.to_owned())))?
                })
                .collect();
            let body = answer.into_body().into_reader();
            Ok(UpstreamAnswer { status, headers, body: Box::new(body) })
        })
    }

    /// An upstream that carries requests the way `carry` says.
    #[must_use]
    pub fn new(carry: impl Fn(UpstreamAsk) -> Result<UpstreamAnswer, UpstreamError> + Send + Sync + 'static) -> Self {
        Self(Arc::new(carry))
    }

    /// Asks `carry` to send `ask`.
    fn call(&self, ask: UpstreamAsk) -> Result<UpstreamAnswer, UpstreamError> {
        (self.0)(ask)
    }
}

/// Something the relay found out, in a shape a screen can show: a request refused, one carried
/// through, or a provider that could not be reached. Never printed on its own; the screen that
/// opened the workspace decides what the person sees and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A connection carried no token, or one no open tab answers to.
    UnknownTab,
    /// The head line the relay script sent was not one QCode reads.
    Malformed,
    /// A connection asked for a method or a path the relay does not carry.
    NotAllowed {
        /// The method that was asked for.
        method: String,
        /// The path that was asked for.
        path: String,
    },
    /// The provider could not be reached, or refused the transport itself.
    Unreachable {
        /// The tag of the provider that was asked.
        tag: String,
        /// What the transport said.
        reason: String,
    },
    /// A request was carried to the provider and its answer's head came back.
    Forwarded {
        /// The tag of the provider that answered.
        tag: String,
        /// The path that was asked for.
        path: String,
        /// The status the provider answered with.
        status: u16,
    },
}

/// The workspace's provider relay, listened on for as long as this value lives.
#[derive(Debug)]
pub struct Listener {
    socket: PathBuf,
    #[cfg(unix)]
    open: Arc<std::sync::atomic::AtomicBool>,
}

impl Listener {
    /// Listens in `folder`, the workspace's `Containers/MCP/`, beside the bridge's socket: makes
    /// the folder if it is not there already, writes the relay script, and starts waiting for
    /// connections. `resolve` finds the provider entry a token belongs to; `upstream` carries the
    /// request the entry describes; `report` is told what happened to each connection, in a shape
    /// a screen can show.
    ///
    /// A socket left behind by a QCode that ended without clearing it is replaced; one another
    /// QCode still answers on is left to it.
    ///
    /// # Errors
    ///
    /// When the folder or the script cannot be written, another QCode answers on the socket
    /// ([`io::ErrorKind::AddrInUse`]), the socket cannot be made, or the system has none.
    pub fn open(
        folder: &Path,
        resolve: impl Fn(&str) -> Option<ProviderEntry> + Send + Sync + 'static,
        upstream: Upstream,
        report: impl Fn(Event) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            unix::open(folder, resolve, upstream, report)
        }
        #[cfg(not(unix))]
        {
            let _ = (folder, resolve, upstream, report);
            Err(io::Error::new(io::ErrorKind::Unsupported, "this system has no unix sockets"))
        }
    }

    /// Where the socket is.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

/// Writes the relay's script into `folder` unless it is there already as this QCode carries it.
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::super::ask::{Method, secret_of};
    use super::super::{ProviderEntry, Wire};
    use super::{
        Event, Listener, MOST_CONNECTIONS, MOST_HEAD, PATIENCE, SOCKET_NAME, STRIPPED, Upstream, UpstreamAsk, allowed,
        head_line, parse_incoming, refusal_body, write_script,
    };

    /// The longest socket path the system takes, less the byte its terminator needs; the same
    /// limit [`crate::bridge::socket`] works around, for the same reason.
    const MOST_PATH: usize = 107;

    pub(super) fn open(
        folder: &Path,
        resolve: impl Fn(&str) -> Option<ProviderEntry> + Send + Sync + 'static,
        upstream: Upstream,
        report: impl Fn(Event) + Send + Sync + 'static,
    ) -> io::Result<Listener> {
        std::fs::create_dir_all(folder)?;
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
        let open = Arc::new(AtomicBool::new(true));
        let still_open = Arc::clone(&open);
        let resolve = Arc::new(resolve);
        let report = Arc::new(report);
        std::thread::Builder::new()
            .name("qcode-relay".to_owned())
            .spawn(move || accept(&listener, &still_open, &resolve, &upstream, &report))?;
        Ok(Listener { socket, open })
    }

    /// Runs `with` on the socket path, or, when the path is too long for a socket address, on the
    /// same socket named through the folder's open handle in `/proc/self/fd`.
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
    fn accept(
        listener: &UnixListener,
        open: &AtomicBool,
        resolve: &Arc<impl Fn(&str) -> Option<ProviderEntry> + Send + Sync + 'static>,
        upstream: &Upstream,
        report: &Arc<impl Fn(Event) + Send + Sync + 'static>,
    ) {
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
            let (resolve, upstream, report, done) =
                (Arc::clone(resolve), upstream.clone(), Arc::clone(report), Arc::clone(&serving));
            let spawned = std::thread::Builder::new().name("qcode-relay-call".to_owned()).spawn(move || {
                serve(stream, resolve.as_ref(), &upstream, report.as_ref());
                done.fetch_sub(1, Ordering::SeqCst);
            });
            if spawned.is_err() {
                serving.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Reads one request off `stream`, decides whether it may be carried, and either refuses it
    /// or forwards it and streams the answer back.
    fn serve(
        stream: UnixStream,
        resolve: &(impl Fn(&str) -> Option<ProviderEntry> + ?Sized),
        upstream: &Upstream,
        report: &(impl Fn(Event) + ?Sized),
    ) {
        if stream.set_read_timeout(Some(PATIENCE)).is_err() {
            return;
        }
        let Ok(writing) = stream.try_clone() else { return };
        let mut writing = writing;
        let mut reading = BufReader::new(stream);

        let mut line = String::new();
        let limit = u64::try_from(MOST_HEAD).unwrap_or(u64::MAX) + 1;
        let read = (&mut reading).take(limit).read_line(&mut line);
        let Ok(incoming) = (match read {
            Ok(_) if line.ends_with('\n') => parse_incoming(line.trim_end_matches(['\n', '\r'])),
            _ => Err(super::Malformed),
        }) else {
            report(Event::Malformed);
            refuse(&mut writing, 400, "malformed");
            return;
        };

        let Some(entry) = resolve(&incoming.token) else {
            report(Event::UnknownTab);
            refuse(&mut writing, 401, "unknown tab");
            return;
        };
        let Some(method) = (match incoming.method.as_str() {
            "GET" => Some(Method::Get),
            "POST" => Some(Method::Post),
            _ => None,
        }) else {
            report(Event::NotAllowed { method: incoming.method.clone(), path: incoming.path.clone() });
            refuse(&mut writing, 405, "method not served here");
            return;
        };
        if !allowed(&incoming.method, &incoming.path) {
            report(Event::NotAllowed { method: incoming.method.clone(), path: incoming.path.clone() });
            refuse(&mut writing, 404, "not served here");
            return;
        }

        let headers: Vec<(String, String)> =
            incoming.headers.into_iter().filter(|(name, _)| !STRIPPED.contains(&name.as_str())).collect();
        let secret = secret_of(&entry);
        // Under the provider's API rather than its bare address: a harness asks for
        // `/v1/messages` the way it would ask Anthropic, OpenRouter answers that path only under
        // `/api`, and Xiaomi only under `/anthropic` while it answers the other shape at its root.
        let url = entry.api_address(Wire::of_path(&incoming.path), &incoming.path);
        let ask = UpstreamAsk { method, url, headers, secret, body: Box::new(reading) };

        match upstream.call(ask) {
            Ok(answer) => {
                report(Event::Forwarded { tag: entry.tag.to_string(), path: incoming.path, status: answer.status });
                let head = head_line(answer.status, &answer.headers);
                if writing.write_all(head.as_bytes()).is_ok() {
                    let mut body = answer.body;
                    let _ = io::copy(&mut body, &mut writing);
                }
            }
            Err(error) => {
                report(Event::Unreachable { tag: entry.tag.to_string(), reason: error.reason });
                refuse(&mut writing, 502, "the provider could not be reached");
            }
        }
    }

    /// Writes a short refusal and nothing else.
    fn refuse(writing: &mut UnixStream, status: u16, why: &str) {
        let head = head_line(status, &[("content-type".to_owned(), "application/json".to_owned())]);
        if writing.write_all(head.as_bytes()).is_ok() {
            let _ = writing.write_all(&refusal_body(why));
        }
    }

    impl Drop for Listener {
        /// Stops listening and takes the socket away, the same way [`crate::bridge::socket::
        /// Listener`] does: one last connection of its own wakes the accepting thread so it sees
        /// the listener is closed.
        fn drop(&mut self) {
            self.open.store(false, Ordering::SeqCst);
            let _ = reach(&self.socket, |path| UnixStream::connect(path));
            let _ = std::fs::remove_file(&self.socket);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    use serde_json::Value;

    use super::super::{Key, ProviderKind, Tag};
    use super::*;

    /// Not a key of anyone's: the characters spell what it is.
    const MADE_UP_KEY: &str = "not-a-real-key-0000-wxyz";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let stamp =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            Self(std::env::temp_dir().join(format!("qcode-relay-{name}-{stamp}")))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tag(name: &str) -> Tag {
        Tag::parse(name).expect("a tag")
    }

    fn ollama_entry(key: Option<&str>) -> ProviderEntry {
        let mut entry = ProviderEntry::new(tag("ev"), ProviderKind::Ollama, "http://192.168.122.1:11434");
        entry.key = key.map(|key| Key::new(key).expect("a key"));
        entry
    }

    /// One entry answered for `token`, nothing for anything else.
    fn resolve_one(token: &'static str, entry: ProviderEntry) -> impl Fn(&str) -> Option<ProviderEntry> + Send + Sync {
        move |asked| (asked == token).then(|| entry.clone())
    }

    /// What reached the stand-in upstream, for a test to look at exactly what would have left
    /// the machine.
    struct SeenAsk {
        url: String,
        headers: Vec<(String, String)>,
        secret: Option<Secret>,
        body: Vec<u8>,
    }

    /// An upstream that answers `body` for every request, and hands every [`UpstreamAsk`] it was
    /// given to `seen` as a [`SeenAsk`].
    fn canned(status: u16, body: &'static str, seen: Sender<SeenAsk>) -> Upstream {
        Upstream::new(move |mut ask| {
            let mut sent = Vec::new();
            let _ = ask.body.read_to_end(&mut sent);
            let _ = seen.send(SeenAsk {
                url: ask.url.clone(),
                headers: ask.headers.clone(),
                secret: ask.secret.clone(),
                body: sent,
            });
            Ok(UpstreamAnswer {
                status,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body: Box::new(std::io::Cursor::new(body.as_bytes().to_vec())),
            })
        })
    }

    /// An upstream that never answers, for a test proving it was never called.
    fn unreachable_if_called(count: Arc<Mutex<usize>>) -> Upstream {
        Upstream::new(move |_| {
            *count.lock().expect("the lock") += 1;
            Ok(UpstreamAnswer { status: 200, headers: Vec::new(), body: Box::new(std::io::empty()) })
        })
    }

    fn connect(socket: &Path) -> UnixStream {
        if socket.as_os_str().len() <= 107 {
            return UnixStream::connect(socket).expect("the socket answers");
        }
        let handle = std::fs::File::open(socket.parent().expect("a folder")).expect("the folder opens");
        let short = format!("/proc/self/fd/{}/{SOCKET_NAME}", std::os::fd::AsRawFd::as_raw_fd(&handle));
        UnixStream::connect(short).expect("the socket answers")
    }

    /// Sends one request the way the relay script would: the head line, then the body, then the
    /// write side is closed. Answers the head line QCode sent back and the whole of its body.
    fn ask(socket: &Path, token: &str, method: &str, path: &str, body: &[u8]) -> (Value, Vec<u8>) {
        ask_with_headers(socket, token, method, path, &[], body)
    }

    /// [`ask`], carrying `headers` the way a harness's own request would.
    fn ask_with_headers(
        socket: &Path,
        token: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (Value, Vec<u8>) {
        let mut stream = connect(socket);
        let headers: Map<String, Value> =
            headers.iter().map(|(name, value)| ((*name).to_owned(), json!(value))).collect();
        let head = json!({ "token": token, "method": method, "path": path, "headers": headers }).to_string();
        stream.write_all(format!("{head}\n").as_bytes()).expect("the head line is written");
        stream.write_all(body).expect("the body is written");
        stream.shutdown(std::net::Shutdown::Write).expect("the write side closes");
        let mut reading = BufReader::new(stream);
        let mut line = String::new();
        reading.read_line(&mut line).expect("an answer head comes");
        let head: Value = serde_json::from_str(line.trim_end()).expect("the head is JSON");
        let mut answer = Vec::new();
        reading.read_to_end(&mut answer).expect("the answer body is read");
        (head, answer)
    }

    #[test]
    fn a_good_token_is_forwarded_with_the_key_added_and_the_answer_comes_back() {
        let scratch = Scratch::new("round");
        let entry = ollama_entry(None);
        let (seen_tx, seen_rx) = mpsc::channel();
        let upstream = canned(200, r#"{"ok":true}"#, seen_tx);
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry), upstream, |_| {}).expect("the socket opens");

        let headers = [("content-type", "application/json"), ("authorization", "Bearer harness-own-key")];
        let (head, body) = ask_with_headers(listener.socket(), "tok", "POST", "/v1/messages", &headers, br#"{"hi":1}"#);
        assert_eq!(head["status"], 200);
        assert_eq!(body, br#"{"ok":true}"#);
        let seen = seen_rx.recv().expect("the request reached the stand-in");
        assert_eq!(seen.url, "http://192.168.122.1:11434/v1/messages");
        assert_eq!(seen.body, br#"{"hi":1}"#, "the harness's body reaches the provider whole");
        assert!(seen.headers.iter().any(|(name, value)| name == "content-type" && value == "application/json"));
        assert!(
            seen.headers.iter().all(|(name, _)| name != "authorization"),
            "the harness's own authorization header never reaches the provider"
        );
        assert!(seen.secret.is_none(), "an ollama entry with no key adds none");
    }

    #[test]
    fn the_provider_gets_a_key_the_container_never_sees() {
        let scratch = Scratch::new("key");
        let entry = ollama_entry(Some(MADE_UP_KEY));
        let (seen_tx, seen_rx) = mpsc::channel();
        let upstream = canned(200, r#"{"ok":true}"#, seen_tx);
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry), upstream, |_| {}).expect("the socket opens");

        let (head, body) = ask(listener.socket(), "tok", "GET", "/v1/models", b"");
        assert_eq!(head["status"], 200);
        assert!(!format!("{head}").contains(MADE_UP_KEY));
        assert!(!String::from_utf8_lossy(&body).contains(MADE_UP_KEY), "the container's own answer holds no key");

        let seen = seen_rx.recv().expect("the request reached the stand-in");
        let secret = seen.secret.expect("the key was added for the provider");
        assert_eq!(secret.header, "Authorization");
        assert_eq!(secret.key.expose(), MADE_UP_KEY, "the real key reached the provider's own request");
    }

    #[test]
    fn a_harness_asking_openrouter_the_way_it_asks_anthropic_reaches_its_api() {
        let scratch = Scratch::new("openrouter");
        let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
        entry.key = Some(Key::new(MADE_UP_KEY).expect("a key"));
        let (seen_tx, seen_rx) = mpsc::channel();
        let listener = Listener::open(&scratch.0, resolve_one("tok", entry), canned(200, "{}", seen_tx), |_| {})
            .expect("the socket opens");
        // What Claude Code really sends: its own path, with the query string it adds.
        let (head, _) = ask(listener.socket(), "tok", "POST", "/v1/messages?beta=true", b"{}");
        assert_eq!(head["status"], 200);
        let seen = seen_rx.recv().expect("the request reached the stand-in");
        assert_eq!(seen.url, "https://openrouter.ai/api/v1/messages?beta=true", "the API, not the website");
    }

    #[test]
    fn no_token_or_a_wrong_one_is_refused_and_nothing_is_forwarded() {
        let scratch = Scratch::new("badtoken");
        let entry = ollama_entry(None);
        let calls = Arc::new(Mutex::new(0));
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry), unreachable_if_called(Arc::clone(&calls)), |_| {})
                .expect("the socket opens");

        let (head, _) = ask(listener.socket(), "wrong", "GET", "/v1/models", b"");
        assert_eq!(head["status"], 401);
        assert_eq!(*calls.lock().expect("the lock"), 0, "a bad token never reaches the provider");
    }

    #[test]
    fn a_path_that_is_not_allowed_is_refused() {
        let scratch = Scratch::new("badpath");
        let entry = ollama_entry(None);
        let calls = Arc::new(Mutex::new(0));
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry), unreachable_if_called(Arc::clone(&calls)), |_| {})
                .expect("the socket opens");

        let (head, _) = ask(listener.socket(), "tok", "GET", "/v1/admin", b"");
        assert_eq!(head["status"], 404);
        let (head, _) = ask(listener.socket(), "tok", "DELETE", "/v1/messages", b"");
        assert_eq!(head["status"], 405);
        assert_eq!(*calls.lock().expect("the lock"), 0, "a path outside the allowed set never reaches the provider");
    }

    /// A body that hands out what it is given one piece at a time, blocking until the next piece
    /// or the end is sent. A relay that buffered a streamed answer before writing any of it would
    /// make the second half of this test indistinguishable from the first; only reading in
    /// pieces, ahead of the last piece being sent, proves it does not.
    struct Trickle(Receiver<Option<Vec<u8>>>, Vec<u8>);

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(Some(piece)) => self.1 = piece,
                    _ => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn a_streamed_answer_arrives_in_pieces_rather_than_only_at_the_end() {
        let scratch = Scratch::new("stream");
        let entry = ollama_entry(None);
        let (pieces_tx, pieces_rx) = mpsc::channel();
        // A `Receiver` is not `Sync`, and `Upstream` must be; held behind a lock, it is taken out
        // once, the one time this stand-in is ever called.
        let pieces_rx = Mutex::new(Some(pieces_rx));
        let upstream = Upstream::new(move |_| {
            let pieces_rx = pieces_rx.lock().expect("the lock").take().expect("the stand-in is called once");
            Ok(UpstreamAnswer {
                status: 200,
                headers: vec![("content-type".to_owned(), "text/event-stream".to_owned())],
                body: Box::new(Trickle(pieces_rx, Vec::new())),
            })
        });
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry), upstream, |_| {}).expect("the socket opens");

        let mut stream = connect(listener.socket());
        let head = json!({ "token": "tok", "method": "GET", "path": "/v1/models", "headers": {} }).to_string();
        stream.write_all(format!("{head}\n").as_bytes()).expect("the head is written");
        stream.shutdown(std::net::Shutdown::Write).expect("the write side closes");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).expect("a read timeout");
        let mut reading = BufReader::new(stream);
        let mut line = String::new();
        reading.read_line(&mut line).expect("the answer head comes");
        assert_eq!(serde_json::from_str::<Value>(line.trim_end()).expect("json")["status"], 200);

        pieces_tx.send(Some(b"first piece".to_vec())).expect("the first piece is sent");
        let mut buf = [0u8; 64];
        let n = reading.read(&mut buf).expect("the first piece arrives on its own");
        assert_eq!(&buf[..n], b"first piece", "only what was sent so far has arrived");

        pieces_tx.send(Some(b", second piece".to_vec())).expect("the second piece is sent");
        pieces_tx.send(None).expect("the stream ends");
        let mut rest = Vec::new();
        reading.read_to_end(&mut rest).expect("the rest is read");
        assert_eq!(rest, b", second piece", "the second piece follows once it was sent");
    }

    #[test]
    fn the_script_names_the_socket_the_port_and_the_headers_it_strips() {
        assert!(SCRIPT.contains(&format!("\"{SOCKET_NAME}\"")), "the script looks for this socket");
        assert!(SCRIPT.contains(&PORT.to_string()), "the script listens on this port");
        assert!(SCRIPT.contains(crate::bridge::TOKEN_VARIABLE), "the script reads the same token the bridge does");
        assert!(SCRIPT.to_lowercase().contains("authorization"), "the script strips the harness's own header");
        assert!(SCRIPT.to_lowercase().contains("x-api-key"), "the script strips the harness's other header");
        assert!(SCRIPT.contains("\"api-key\""), "and the one Xiaomi reads a key from");
    }

    #[test]
    fn the_program_of_a_provider_tab_is_the_relay_with_the_harness_after_it() {
        let harness = ["claude".to_owned(), "--dangerously-skip-permissions".to_owned()];
        assert_eq!(
            wrapping(&harness),
            ["node", &script_in_container(), "claude", "--dangerously-skip-permissions"],
            "the harness is started by the relay, not beside it"
        );
        assert!(script_in_container().ends_with(SCRIPT_NAME), "the script is where the container sees the folder");
    }

    /// The whole reason the relay starts the harness itself: a harness started beside it can ask
    /// before the server is up. The child here is the harness's stand-in and it does the one
    /// thing a harness does first — reach the relay's address — so a script that started it too
    /// early would fail this, and one that never started it would hang rather than answer.
    #[test]
    fn the_script_starts_what_it_is_given_only_once_it_is_listening_and_ends_with_it() {
        let Some(node) = node() else { return };
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/provider/qcode-relay.mjs");
        let reach = format!(
            "const s=require('node:net').connect({PORT},'127.0.0.1');\
             s.on('connect',()=>{{s.end();process.exit(23)}});s.on('error',()=>process.exit(1));"
        );
        let ran = std::process::Command::new(&node)
            .args([
                script.as_os_str(),
                std::ffi::OsStr::new(&node),
                std::ffi::OsStr::new("-e"),
                std::ffi::OsStr::new(&reach),
            ])
            .output()
            .expect("the script runs");
        assert_eq!(
            ran.status.code(),
            Some(23),
            "the harness reached the relay and its own code came back: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
    }

    /// Node, when this machine has one. The script is run in a profile image, which always has
    /// it; a machine building QCode need not, and a missing node is not a failing QCode.
    fn node() -> Option<std::path::PathBuf> {
        let found = std::process::Command::new("sh").args(["-c", "command -v node"]).output().ok()?;
        found.status.success().then(|| std::path::PathBuf::from(String::from_utf8_lossy(&found.stdout).trim()))
    }

    #[test]
    fn every_path_the_script_or_the_host_disagrees_on_is_refused_the_same_way() {
        assert!(allowed("POST", "/v1/messages"), "what Claude Code sends");
        assert!(allowed("POST", "/v1/chat/completions"), "what opencode sends");
        assert!(allowed("POST", "/v1/responses"), "what Codex sends");
        assert!(!allowed("GET", "/v1/responses"), "a stored answer is not asked for");
        assert!(!allowed("POST", "/v1/responses/compact"), "only the conversation itself");
        assert!(allowed("GET", "/v1/models"));
        assert!(allowed("GET", "/v1/models?x=1"), "a query string does not change what path was asked for");
        assert!(!allowed("POST", "/v1/models"));
        assert!(!allowed("GET", "/v1/messages"));
        assert!(!allowed("GET", "/v1/chat/completions"));
        assert!(!allowed("DELETE", "/v1/messages"));
        assert!(!allowed("GET", "/anything/else"));
        assert!(!allowed("POST", "/v1/completions"), "only a conversation, in either shape");
    }

    #[test]
    fn an_openai_shaped_harness_is_carried_to_openrouter_whatever_shape_the_entry_was_measured_in() {
        let scratch = Scratch::new("openai-shape");
        let mut entry = ProviderEntry::new(tag("yol"), ProviderKind::OpenRouter, "https://openrouter.ai");
        entry.key = Some(Key::new(MADE_UP_KEY).expect("a key"));
        assert_eq!(entry.wire, super::super::Wire::Anthropic, "the entry was written down in the other shape");
        let (seen_tx, seen_rx) = mpsc::channel();
        let listener = Listener::open(&scratch.0, resolve_one("tok", entry), canned(200, "{}", seen_tx), |_| {})
            .expect("the socket opens");
        let (head, _) = ask(listener.socket(), "tok", "POST", "/v1/chat/completions", b"{}");
        assert_eq!(head["status"], 200);
        let seen = seen_rx.recv().expect("the request reached the stand-in");
        assert_eq!(seen.url, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(seen.secret.expect("the key is added").key.expose(), MADE_UP_KEY);
    }

    /// A ready-made entry of `kind` at its first address, with the made-up key.
    fn ready_made(kind: ProviderKind) -> ProviderEntry {
        let mut entry = ProviderEntry::new(tag("hazir"), kind, kind.suggested_base());
        entry.key = Some(Key::new(MADE_UP_KEY).expect("a key"));
        entry
    }

    /// Every header a harness could have been given a key of its own in, which must all stop at
    /// the relay whichever of them the provider reads.
    const HARNESS_OWN: [(&str, &str); 4] = [
        ("content-type", "application/json"),
        ("authorization", "Bearer harness-own-key"),
        ("x-api-key", "harness-own-key"),
        ("api-key", "harness-own-key"),
    ];

    #[test]
    fn xiaomi_gets_its_key_in_api_key_at_the_root_each_shape_answers_under() {
        let scratch = Scratch::new("mimo");
        let (seen_tx, seen_rx) = mpsc::channel();
        let entry = ready_made(ProviderKind::MimoTokenPlan);
        let listener = Listener::open(&scratch.0, resolve_one("tok", entry), canned(200, "{}", seen_tx), |_| {})
            .expect("the socket opens");
        for (method, path, url) in [
            ("POST", "/v1/messages?beta=true", "https://token-plan-ams.xiaomimimo.com/anthropic/v1/messages?beta=true"),
            ("POST", "/v1/chat/completions", "https://token-plan-ams.xiaomimimo.com/v1/chat/completions"),
            ("GET", "/v1/models", "https://token-plan-ams.xiaomimimo.com/v1/models"),
        ] {
            let (head, _) = ask_with_headers(listener.socket(), "tok", method, path, &HARNESS_OWN, b"{}");
            assert_eq!(head["status"], 200, "{path}");
            let seen = seen_rx.recv().expect("the request reached the stand-in");
            assert_eq!(seen.url, url, "{path}");
            let secret = seen.secret.expect("the key is added");
            assert_eq!((secret.header.as_str(), secret.prefix.as_str()), ("api-key", ""), "{path}");
            assert_eq!(secret.key.expose(), MADE_UP_KEY);
            for own in ["authorization", "x-api-key", "api-key"] {
                assert!(
                    seen.headers.iter().all(|(name, _)| name != own),
                    "{path}: the harness's own {own} never reaches the provider: {:?}",
                    seen.headers.iter().map(|(name, _)| name).collect::<Vec<_>>()
                );
            }
            assert!(seen.headers.iter().any(|(name, _)| name == "content-type"), "{path}: the rest is carried");
        }
    }

    #[test]
    fn kimi_gets_its_key_in_x_api_key_under_coding_in_both_shapes() {
        let scratch = Scratch::new("kimi");
        let (seen_tx, seen_rx) = mpsc::channel();
        let entry = ready_made(ProviderKind::KimiCode);
        let listener = Listener::open(&scratch.0, resolve_one("tok", entry), canned(200, "{}", seen_tx), |_| {})
            .expect("the socket opens");
        for (path, url) in [
            ("/v1/messages", "https://api.kimi.com/coding/v1/messages"),
            ("/v1/chat/completions", "https://api.kimi.com/coding/v1/chat/completions"),
        ] {
            let (head, body) = ask_with_headers(listener.socket(), "tok", "POST", path, &HARNESS_OWN, b"{}");
            assert_eq!(head["status"], 200);
            assert!(!format!("{head}").contains(MADE_UP_KEY) && !String::from_utf8_lossy(&body).contains(MADE_UP_KEY));
            let seen = seen_rx.recv().expect("the request reached the stand-in");
            assert_eq!(seen.url, url);
            let secret = seen.secret.expect("the key is added");
            assert_eq!((secret.header.as_str(), secret.prefix.as_str()), ("x-api-key", ""));
            assert!(
                seen.headers.iter().all(|(name, value)| !value.contains("harness-own-key") && name != "x-api-key"),
                "only QCode's key goes out: {:?}",
                seen.headers
            );
        }
    }
}
