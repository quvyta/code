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
//! # Which model, and which one after that
//!
//! The harness names the model it wants in the body it sends, and that name is not QCode's
//! business to obey: the person chose a model when they made the profile, not the harness when it
//! started a background task. So the `model` in a request that has one is set to the step's model
//! before the request goes out, and a body that is not a JSON object naming a model — a model
//! listing, a form-encoded body — goes on exactly as it was written. That pinning is the one
//! assurance that no request of a person's tab ever reaches a model they did not choose, which on
//! a provider that bills per model is money spent on somebody else's idea of a small task.
//!
//! A profile that chose one model has one step and nothing to fall back to. A profile that chose
//! a lineup has its steps in the order the person wrote them, and a step is given up on only on
//! the answer's head: the provider could not be reached, `402`, `403`, `408`, `429` or any `5xx`,
//! and `400` and `404` only when what they say names the model that was asked — those two are the
//! same complaint for every model unless they are about one. A `401` is never a reason: there is
//! one key, and the next step would be refused the same way. Nothing is decided on a `2xx`: once a
//! good answer has started going to the harness, the model for this request is settled and a
//! stream that breaks halfway is passed on as it broke.
//!
//! The last step's answer is the harness's whatever it is, body intact, so a tab shows the
//! provider's own error instead of one QCode made up. A step that was given up on is not asked
//! again for a minute, so a busy free model stops costing a round trip per turn; that order is
//! kept in this listener, behind one lock its connections share.
//!
//! Which of the two message paths a request takes is the harness's to say, not the provider
//! entry's. Nothing translates between the shapes, so the only shape that can work is the one
//! the harness speaks — Claude Code the Anthropic one, opencode the OpenAI one — and every kind
//! of provider QCode knows serves both. Holding a tab to its provider's recorded shape would only
//! refuse the one request its harness can make. The shape does decide where the request goes,
//! since one service answers the two shapes under different roots ([`crate::provider::
//! ProviderKind::api_root`]), and the key goes in the header the entry's kind reads
//! ([`crate::provider::ProviderKind::key_header`]).

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::ProviderEntry;
use super::ask::{Method, Secret};

/// Where one request goes and what it may be asked with: the provider entry to ask, the models
/// to try for it in order, and the lineup's name when the profile chose one.
///
/// A profile that chose a single model sends one step. A profile that chose a lineup sends its
/// steps in the order the person wrote them, and the relay walks down that order whenever a step
/// answers with a reason the next one may not have.
#[derive(Debug, Clone)]
pub struct Route {
    /// The provider to ask, as `providers.toml` holds it and as [`Listener::open`]'s `resolve`
    /// read it for this very request.
    pub entry: ProviderEntry,
    /// The models to ask, in the order they are tried. At least one: a request with no step to
    /// ask has nowhere to go, and a `resolve` that says so is better than a relay that decides.
    pub models: Vec<String>,
    /// The lineup's name, when the profile chose one rather than a single model. It is carried
    /// for the relay's own reports only: which lineup a tab runs on is the person's own choice,
    /// and the relay only has to repeat it in what it says.
    pub lineup: Option<String>,
}

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

/// The most a request's body may be, and the reason it is read whole rather than handed on as it
/// arrives: the model in it is set to the one the profile chose before anything goes out, and
/// every step of a lineup must be asked the very same bytes. A harness's own request is a
/// conversation and far smaller, so the bound is against a body that never ends rather than
/// against a real one.
const MOST_BODY: usize = 64 * 1024 * 1024;

/// How much of an error body is read to decide whether it names the model that was asked. Wide
/// enough for the sentence any provider has been seen to put in one, and a bound rather than a
/// policy: what is not in the first 64 KiB of a complaint is not in the complaint.
const MOST_SAID: usize = 64 * 1024;

/// How long a step that answered with a reason to fall back is not asked again. Long enough that
/// a busy free model stops costing a request per turn, short enough that a model which is back is
/// used without the person thinking about it.
const COOLING: Duration = Duration::from_secs(60);

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

/// `count` as the bound [`Read::take`] takes, for a constant written as a `usize`.
fn bound(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// `body` with its `model` set to `model`, or `body` itself unchanged when it is not a JSON
/// object naming one: a body QCode cannot read the model out of is a body QCode does not touch,
/// which is what a `GET /v1/models` and anything not written in the shapes QCode knows arrive as.
///
/// The rest of the body goes on exactly as the harness wrote it. Only the model is QCode's
/// business, and the model is the person's choice rather than the harness's.
fn pinned(body: Vec<u8>, model: &str) -> Vec<u8> {
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(&body) else { return body };
    if !fields.contains_key("model") {
        return body;
    }
    fields.insert("model".to_owned(), Value::from(model));
    serde_json::to_vec(&Value::Object(fields)).unwrap_or(body)
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
    pub(super) fn call(&self, ask: UpstreamAsk) -> Result<UpstreamAnswer, UpstreamError> {
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
    /// One model of a lineup was left for the next: a person who chose an order of models should
    /// see which one was passed over and which one is being asked instead, since only QCode knows
    /// that both happened.
    FellBack {
        /// The token of the tab whose request this was, which is the only thing that says whose
        /// conversation fell back: two tabs of one workspace can be on different providers, and a
        /// screen writes this under the tab it belongs to rather than wherever it happens to be.
        token: String,
        /// The tag of the provider that was asked.
        tag: String,
        /// The lineup's name, when the profile chose one rather than a single model.
        lineup: Option<String>,
        /// The model that was asked and passed over.
        from: String,
        /// The model that is asked instead.
        to: String,
        /// The status the first step answered with, or `None` when it could not be reached at all.
        status: Option<u16>,
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
    /// connections. `resolve` finds the route a token belongs to — the provider to ask and the
    /// models to ask it with; `upstream` carries the request the route describes; `report` is
    /// told what happened to each connection, in a shape a screen can show.
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
        resolve: impl Fn(&str) -> Option<Route> + Send + Sync + 'static,
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use super::super::Wire;
    use super::super::ask::{Method, secret_of};
    use super::{
        Cooling, Event, Listener, MOST_BODY, MOST_CONNECTIONS, MOST_HEAD, MOST_SAID, PATIENCE, Route, SOCKET_NAME,
        STRIPPED, Upstream, UpstreamAsk, allowed, bound, cool, head_line, names, order_now, parse_incoming, pinned,
        refusal_body, write_script,
    };

    /// The longest socket path the system takes, less the byte its terminator needs; the same
    /// limit [`crate::bridge::socket`] works around, for the same reason.
    const MOST_PATH: usize = 107;

    pub(super) fn open(
        folder: &Path,
        resolve: impl Fn(&str) -> Option<Route> + Send + Sync + 'static,
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
        // Which models of this workspace have refused, and until when each is not asked again.
        // Held by the thread that waits for connections and shared with the ones that serve them,
        // so it lives exactly as long as the relay listens and every connection of this relay
        // shares it. See [`ordered`].
        let cooling = Arc::new(Mutex::new(Cooling::new()));
        let cooling_in_threads = Arc::clone(&cooling);
        std::thread::Builder::new()
            .name("qcode-relay".to_owned())
            .spawn(move || accept(&listener, &still_open, &resolve, &upstream, &cooling_in_threads, &report))?;
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
        resolve: &Arc<impl Fn(&str) -> Option<Route> + Send + Sync + 'static>,
        upstream: &Upstream,
        cooling: &Arc<Mutex<Cooling>>,
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
            let (resolve, upstream, cooling, report, done) =
                (Arc::clone(resolve), upstream.clone(), Arc::clone(cooling), Arc::clone(report), Arc::clone(&serving));
            let spawned = std::thread::Builder::new().name("qcode-relay-call".to_owned()).spawn(move || {
                serve(stream, resolve.as_ref(), &upstream, &cooling, report.as_ref());
                done.fetch_sub(1, Ordering::SeqCst);
            });
            if spawned.is_err() {
                serving.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Reads one request off `stream`, decides whether it may be carried, and either refuses it
    /// or forwards it, walking down the route's steps until one of them answers, and streams that
    /// answer back.
    fn serve(
        stream: UnixStream,
        resolve: &(impl Fn(&str) -> Option<Route> + ?Sized),
        upstream: &Upstream,
        cooling: &Arc<Mutex<super::Cooling>>,
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

        let Some(route) = resolve(&incoming.token) else {
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
        // A route with no step to ask has nowhere to go. A profile that names a model brings one,
        // and one that does not is QCode's own doing rather than the harness's.
        if route.models.is_empty() {
            refuse(&mut writing, 500, "the profile names no model");
            return;
        }

        let headers: Vec<(String, String)> =
            incoming.headers.into_iter().filter(|(name, _)| !STRIPPED.contains(&name.as_str())).collect();
        let secret = secret_of(&route.entry);
        // Under the provider's API rather than its bare address: a harness asks for
        // `/v1/messages` the way it would ask Anthropic, OpenRouter answers that path only under
        // `/api`, and Xiaomi only under `/anthropic` while it answers the other shape at its root.
        let url = route.entry.api_address(Wire::of_path(&incoming.path), &incoming.path);

        // Read whole rather than carried as it arrives: the model in the body is set to the one
        // the person chose before anything goes out, and every step of a lineup is asked the very
        // same bytes. Over the limit is refused here rather than asked about, since a provider has
        // no business answering a request QCode itself will not carry.
        let mut body = Vec::new();
        let read = reading.take(bound(MOST_BODY).saturating_add(1)).read_to_end(&mut body);
        if read.is_err() {
            refuse(&mut writing, 400, "the request body could not be read");
            return;
        }
        if body.len() > MOST_BODY {
            refuse(&mut writing, 413, "the request body is too large");
            return;
        }

        // The order the lineup is tried in, with the models that just refused at the back for a
        // minute, so a busy one stops costing a request per turn. Fixed before the first ask, so
        // every step of this request is asked of the same models in the same order.
        let asked_at = Instant::now();
        let key = |model: &str| (route.entry.tag.to_string(), route.lineup.clone(), model.to_owned());
        let steps = order_now(cooling, &route, &key, asked_at);

        let mut waiting = steps.iter();
        while let Some(model) = waiting.next() {
            let to = waiting.clone().next().map(String::as_str);
            let ask = UpstreamAsk {
                method,
                url: url.clone(),
                headers: headers.clone(),
                secret: secret.clone(),
                body: Box::new(io::Cursor::new(pinned(body.clone(), model))),
            };
            match upstream.call(ask) {
                Ok(mut answer) => {
                    // A `400` and a `404` only say which model they are about in what they say, so
                    // that much is read before the step is given up on. What is read is kept: a
                    // step that turns out not to be a reason to fall back still answers the
                    // harness whole, and so does the last step of all.
                    let mut said = Vec::new();
                    if matches!(answer.status, 400 | 404) {
                        let _ = answer.body.by_ref().take(bound(MOST_SAID)).read_to_end(&mut said);
                    }
                    let status = answer.status;
                    // Only ever on the answer's head. Once a `2xx` has started, the body goes to the
                    // harness as it arrives and this request's model is decided for good: a
                    // stream that breaks halfway is a thing the harness must see, not a reason to
                    // spend the same question twice.
                    let giving_up = to.is_some()
                        && (matches!(status, 402 | 403 | 408 | 429)
                            || status >= 500
                            || (matches!(status, 400 | 404) && names(&said, model)));
                    if giving_up {
                        let Some(to) = to else { return };
                        cool(cooling, &key, model);
                        report(Event::FellBack {
                            token: incoming.token.clone(),
                            tag: route.entry.tag.to_string(),
                            lineup: route.lineup.clone(),
                            from: (*model).to_owned(),
                            to: to.to_owned(),
                            status: Some(status),
                        });
                        continue;
                    }
                    report(Event::Forwarded { tag: route.entry.tag.to_string(), path: incoming.path, status });
                    let head = head_line(status, &answer.headers);
                    if writing.write_all(head.as_bytes()).is_ok() {
                        // The part of the body read to be decided on, then the rest of it as it
                        // arrives, so an answer that starts is streamed all the same.
                        let mut body = answer.body;
                        if writing.write_all(&said).is_ok() {
                            let _ = io::copy(&mut body, &mut writing);
                        }
                    }
                    return;
                }
                Err(error) => {
                    // The last step's own answer, whatever it is, is the harness's: a provider
                    // that cannot be reached has said so in its own words, and a harness can show
                    // those where a refusal QCode made up would only hide them.
                    let Some(to) = to else {
                        report(Event::Unreachable { tag: route.entry.tag.to_string(), reason: error.reason });
                        refuse(&mut writing, 502, "the provider could not be reached");
                        return;
                    };
                    cool(cooling, &key, model);
                    report(Event::FellBack {
                        token: incoming.token.clone(),
                        tag: route.entry.tag.to_string(),
                        lineup: route.lineup.clone(),
                        from: (*model).to_owned(),
                        to: to.to_owned(),
                        status: None,
                    });
                }
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

/// One step of one route, named the way the cool-down names it: a model is only skipped for the
/// provider tag and the lineup it refused in, since the same name on someone else's provider is a
/// different machine with different models.
type Step = (String, Option<String>, String);

/// A step of a route and until when it is not to be asked.
type Cooling = HashMap<Step, Instant>;

/// The steps to ask at `now`: those not being skipped first, and the ones being skipped after
/// them, each group in the order the profile wrote them. When every step is being skipped the
/// order is the plain one — a model that keeps refusing is still the one the person chose, and
/// asking it beats an answer from nobody.
///
/// A pure function of the clock rather than of the wall, so what a minute of skipping looks like
/// is settled by looking at a minute later rather than by waiting for one.
fn ordered(models: &[String], cooling: &Cooling, key: &dyn Fn(&str) -> Step, now: Instant) -> Vec<String> {
    let (mut ready, mut waiting): (Vec<String>, Vec<String>) =
        models.iter().cloned().partition(|model| cooling.get(&key(model)).is_none_or(|until| *until <= now));
    ready.append(&mut waiting);
    ready
}

/// The steps of `route` in the order they are asked at `now`, with the models that refused
/// recently at the back until their cool-down is up and the ones that have run out of it forgotten.
/// A lock that cannot be taken is not a reason to refuse a request: the order is then the plain
/// one, which is what a relay with no memory of any refusal would do.
fn order_now(cooling: &Mutex<Cooling>, route: &Route, key: &dyn Fn(&str) -> Step, now: Instant) -> Vec<String> {
    let Ok(mut seen) = cooling.lock() else { return route.models.clone() };
    seen.retain(|_, until| *until > now);
    ordered(&route.models, &seen, key, now)
}

/// Whether `said` is an error body that names `model`, which is what makes a `400` or a `404` a
/// complaint about that model rather than about the request. Read in bytes rather than in words:
/// what a provider says about a model it does not have quotes the name, in a shape nobody can
/// promise to write a rule for.
fn names(said: &[u8], model: &str) -> bool {
    let model = model.as_bytes();
    !model.is_empty() && said.windows(model.len()).any(|window| window == model)
}

/// Remembers that `model` is not to be asked of this route again for [`COOLING`]. Nothing is done
/// with a lock that cannot be taken: the step is asked again on the next request rather than
/// never, which is what a plain order does.
fn cool(cooling: &Mutex<Cooling>, key: &dyn Fn(&str) -> Step, model: &str) {
    let Ok(mut cooling) = cooling.lock() else { return };
    cooling.insert(key(model), Instant::now() + COOLING);
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

    /// The model a profile is taken to have chosen, for the tests that are about the road and not
    /// about which model: no default, so a test that forgets to pin one cannot pass quietly.
    const CHOSEN: &str = "qwen/qwen3-coder:free";

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

    /// One route answered for `token`, nothing for anything else: the entry and the single model
    /// it is asked for, which is what a profile that names a model rather than a lineup resolves
    /// to today.
    fn resolve_one(
        token: &'static str,
        entry: ProviderEntry,
        model: &'static str,
    ) -> impl Fn(&str) -> Option<Route> + Send + Sync {
        let models = [model.to_owned()];
        move |asked| (asked == token).then(|| Route { entry: entry.clone(), models: models.to_vec(), lineup: None })
    }

    /// One route answered for `token` with `models` to try in order, under `lineup`'s name.
    fn resolve_steps(
        token: &'static str,
        entry: ProviderEntry,
        models: &'static [&'static str],
        lineup: Option<&'static str>,
    ) -> impl Fn(&str) -> Option<Route> + Send + Sync {
        let models: Vec<String> = models.iter().map(|model| (*model).to_owned()).collect();
        let lineup = lineup.map(str::to_owned);
        move |asked| {
            (asked == token).then(|| Route { entry: entry.clone(), models: models.clone(), lineup: lineup.clone() })
        }
    }

    /// The events a listener was told about, in the order it was told them.
    fn taken(reported: &Arc<Mutex<Vec<Event>>>) -> Vec<Event> {
        reported.lock().expect("the events are not poisoned").clone()
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

    /// What the stand-in says to one model: the status and the body of its answer.
    type Reply = (u16, &'static str);

    /// An upstream that answers each model of `answers` with the [`Reply`] written beside it, and
    /// hands every [`UpstreamAsk`] to `seen` before it answers. A model asked that is not named
    /// here is answered `400` naming itself, so a test that expected another model to be asked
    /// fails on what the harness was told rather than on silence.
    fn by_model(answers: Vec<(&'static str, Reply)>, seen: Sender<SeenAsk>) -> Upstream {
        Upstream::new(move |mut ask| {
            let mut sent = Vec::new();
            let _ = ask.body.read_to_end(&mut sent);
            let asked: Option<String> =
                serde_json::from_slice::<Value>(&sent).ok().and_then(|body| body["model"].as_str().map(str::to_owned));
            let _ = seen.send(SeenAsk {
                url: ask.url.clone(),
                headers: ask.headers.clone(),
                secret: ask.secret.clone(),
                body: sent,
            });
            let reply = asked
                .as_deref()
                .and_then(|model| answers.iter().find(|(known, _)| *known == model).map(|(_, reply)| *reply))
                .unwrap_or((400, r#"{"error":"the stand-in has no answer for that model"}"#));
            let (status, body) = reply;
            Ok(UpstreamAnswer {
                status,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body: Box::new(std::io::Cursor::new(body.as_bytes().to_vec())),
            })
        })
    }

    /// Every model the stand-in was asked for, in the order they were asked, read out of the
    /// bodies: what the provider would have seen, whatever the harness's own body said.
    fn asked_models(seen: &Receiver<SeenAsk>, count: usize) -> Vec<String> {
        (0..count)
            .map(|_| {
                let ask = seen.try_recv().expect("the request reached the stand-in");
                serde_json::from_slice::<Value>(&ask.body).expect("a JSON body")["model"]
                    .as_str()
                    .expect("a model in the body")
                    .to_owned()
            })
            .collect()
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

    /// [`ask`], writing a body of `size` bytes that is never held whole in this test: the point
    /// of the bound is a body too large to keep, and a test that kept one would only move the
    /// cost. The byte is written over and over, so the body is the size asked for and nothing of
    /// it is a message.
    fn ask_huge(socket: &Path, token: &str, method: &str, path: &str, size: usize) -> (Value, Vec<u8>) {
        let mut stream = connect(socket);
        let head = json!({ "token": token, "method": method, "path": path, "headers": {} }).to_string();
        stream.write_all(format!("{head}\n").as_bytes()).expect("the head line is written");
        let written = io::copy(&mut std::io::repeat(b'x').take(bound(size)), &mut stream).expect("the body is written");
        assert_eq!(written, bound(size), "the whole body is on its way");
        stream.shutdown(std::net::Shutdown::Write).expect("the write side closes");
        let mut reading = BufReader::new(stream);
        let mut line = String::new();
        reading.read_line(&mut line).expect("an answer head comes");
        let head: Value = serde_json::from_str(line.trim_end()).expect("the head is JSON");
        let mut answer = Vec::new();
        reading.read_to_end(&mut answer).expect("the answer body is read");
        (head, answer)
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

    /// What a harness's body says, as the person reading this test would write it.
    const A_HARNESS_ASKING: &[u8] = br#"{"model":"claude-haiku-4-5","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":"selam"}]}"#;

    /// The same body with the model QCode pins to it, and nothing else changed.
    const PINNED_TO_THE_PROFILE: &str = r#"{"model":"qwen/qwen3-coder:free","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":"selam"}]}"#;

    #[test]
    fn the_provider_is_asked_for_the_model_the_profile_chose_and_not_the_one_the_harness_named() {
        let scratch = Scratch::new("pin");
        let (seen_tx, seen_rx) = mpsc::channel();
        let listener = Listener::open(
            &scratch.0,
            resolve_one("tok", ollama_entry(None), CHOSEN),
            canned(200, r#"{"ok":true}"#, seen_tx),
            |_| {},
        )
        .expect("the socket opens");

        // Claude Code asks for its own small model by its own name for background work, which is
        // a model the person never chose and, on a provider that bills for it, a paid one.
        let (head, _) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 200);
        let seen = seen_rx.recv().expect("the request reached the stand-in");
        let sent: Value = serde_json::from_slice(&seen.body).expect("the body that left is JSON");
        assert_eq!(sent, serde_json::from_str::<Value>(PINNED_TO_THE_PROFILE).expect("the expected body is JSON"));
    }

    #[test]
    fn a_body_that_is_not_json_goes_on_byte_for_byte() {
        let scratch = Scratch::new("notjson");
        let (seen_tx, seen_rx) = mpsc::channel();
        let listener = Listener::open(
            &scratch.0,
            resolve_one("tok", ollama_entry(None), CHOSEN),
            canned(200, "{}", seen_tx),
            |_| {},
        )
        .expect("the socket opens");

        let body = b"model=qwen3-coder:free&stream=1";
        let (head, _) = ask(listener.socket(), "tok", "POST", "/v1/messages", body);
        assert_eq!(head["status"], 200);
        let seen = seen_rx.recv().expect("the request reached the stand-in");
        assert_eq!(seen.body, body, "a body QCode cannot read a model out of is not one it touches");
    }

    #[test]
    fn a_body_over_the_limit_is_refused_before_any_provider_is_asked() {
        let scratch = Scratch::new("huge");
        let calls = Arc::new(Mutex::new(0));
        let listener = Listener::open(
            &scratch.0,
            resolve_one("tok", ollama_entry(None), CHOSEN),
            unreachable_if_called(Arc::clone(&calls)),
            |_| {},
        )
        .expect("the socket opens");

        let (head, body) = ask_huge(listener.socket(), "tok", "POST", "/v1/messages", MOST_BODY + 1);
        assert_eq!(head["status"], 413);
        assert!(!String::from_utf8_lossy(&body).contains('x'), "nothing of the refused body is sent back");
        assert_eq!(*calls.lock().expect("the lock"), 0, "a body too large never reaches a provider");

        // The same relay still answers a body within the limit, so the bound is a bound and not a
        // refusal of the message shape.
        let (head, _) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 200);
        assert_eq!(*calls.lock().expect("the lock"), 1);
    }

    #[test]
    fn a_good_token_is_forwarded_with_the_key_added_and_the_answer_comes_back() {
        let scratch = Scratch::new("round");
        let entry = ollama_entry(None);
        let (seen_tx, seen_rx) = mpsc::channel();
        let upstream = canned(200, r#"{"ok":true}"#, seen_tx);
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry, CHOSEN), upstream, |_| {}).expect("the socket opens");

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
            Listener::open(&scratch.0, resolve_one("tok", entry, CHOSEN), upstream, |_| {}).expect("the socket opens");

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
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry, CHOSEN), canned(200, "{}", seen_tx), |_| {})
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
        let listener = Listener::open(
            &scratch.0,
            resolve_one("tok", entry, CHOSEN),
            unreachable_if_called(Arc::clone(&calls)),
            |_| {},
        )
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
        let listener = Listener::open(
            &scratch.0,
            resolve_one("tok", entry, CHOSEN),
            unreachable_if_called(Arc::clone(&calls)),
            |_| {},
        )
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

    /// A body that gives a piece of itself and then breaks, as a stream cut in the middle of an
    /// answer does. The error is not a refusal: nothing has refused, the answer simply stopped
    /// coming, which is what must not be mistaken for a reason to ask another model.
    struct Broken(Vec<u8>);

    impl Read for Broken {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Err(io::Error::other("the stream broke"));
            }
            let n = buf.len().min(self.0.len()).min(16);
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }

    /// The models of a route in the order a test asks them, and the first that is expected to
    /// refuse with `status` while the second answers `200` with `body`. The two are the ones the
    /// fall-back tests use, so a test that reads them cannot pass with the order reversed.
    const A_LINEUP: &[&str] = &["a/one", "b/two"];

    /// What the first model of [`A_LINEUP`] says when the provider will not have it, quoted the way
    /// a provider quotes a model it does not have.
    const NO_SUCH_MODEL: &str = r#"{"error":{"message":"model a/one not found"}}"#;

    /// What the second model answers when it is asked.
    const THE_ANSWER: &str = r#"{"id":"chatcmpl-kyaz-1","choices":[{"text":"done"}]}"#;

    /// A stand-in that refuses `A_LINEUP`'s first model and answers its second, and records every
    /// request that reaches it.
    fn one_busy_one_willing(seen: Sender<SeenAsk>) -> Upstream {
        by_model(vec![(A_LINEUP[0], (429, r#"{"error":"busy"}"#)), (A_LINEUP[1], (200, THE_ANSWER))], seen)
    }

    /// The order a route of two models is asked in at `now`, with the first one cooling down. The
    /// pure function behind the listener's own ordering, tested on a clock rather than on a wait.
    fn order_of(models: &[&str], cooling_until: Instant, now: Instant) -> Vec<String> {
        let models: Vec<String> = models.iter().map(|model| (*model).to_owned()).collect();
        let seen: Cooling =
            [(("ev".to_owned(), Some("bilim".to_owned()), "a/one".to_owned()), cooling_until)].into_iter().collect();
        ordered(&models, &seen, &|model| ("ev".to_owned(), Some("bilim".to_owned()), model.to_owned()), now)
    }

    #[test]
    fn a_busy_first_model_hands_its_turn_to_the_next_and_the_harness_gets_that_answer() {
        let scratch = Scratch::new("fallback");
        let (seen_tx, seen_rx) = mpsc::channel();
        let reported: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let told = Arc::clone(&reported);
        let listener = Listener::open(
            &scratch.0,
            resolve_steps("tok", ollama_entry(None), A_LINEUP, Some("bilim")),
            one_busy_one_willing(seen_tx),
            move |event| told.lock().expect("the events are not poisoned").push(event),
        )
        .expect("the socket opens");

        let (head, body) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 200);
        assert_eq!(body, THE_ANSWER.as_bytes(), "the harness gets the answer of the model that had one");
        assert_eq!(asked_models(&seen_rx, 2), A_LINEUP, "the lineup is asked in the order it was written");
        assert_eq!(
            taken(&reported),
            [
                Event::FellBack {
                    token: "tok".to_owned(),
                    tag: "ev".to_owned(),
                    lineup: Some("bilim".to_owned()),
                    from: "a/one".to_owned(),
                    to: "b/two".to_owned(),
                    status: Some(429),
                },
                Event::Forwarded { tag: "ev".to_owned(), path: "/v1/messages".to_owned(), status: 200 },
            ]
        );
    }

    #[test]
    fn a_bad_request_about_the_model_it_names_hands_its_turn_to_the_next_and_one_about_something_else_does_not() {
        let scratch = Scratch::new("badmodel");
        for (named, expected) in [(true, 200), (false, 400)] {
            let (seen_tx, seen_rx) = mpsc::channel();
            let about_the_request = r#"{"error":{"message":"too many tokens"}}"#;
            let first = if named { NO_SUCH_MODEL } else { about_the_request };
            let answers = vec![(A_LINEUP[0], (400, first)), (A_LINEUP[1], (200, THE_ANSWER))];
            let listener = Listener::open(
                &scratch.0,
                resolve_steps("tok", ollama_entry(None), A_LINEUP, None),
                by_model(answers, seen_tx),
                |_| {},
            )
            .expect("the socket opens");

            let (head, body) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
            assert_eq!(head["status"], expected, "whether the body names the model asked decides it");
            if named {
                assert_eq!(body, THE_ANSWER.as_bytes());
            } else {
                assert_eq!(body, about_the_request.as_bytes(), "the request's own complaint reaches the harness whole");
                assert_eq!(asked_models(&seen_rx, 1), ["a/one".to_owned()], "no other model was asked");
            }
        }
    }

    #[test]
    fn a_refusal_of_the_key_is_never_a_reason_to_ask_another_model() {
        let scratch = Scratch::new("unauthorized");
        let (seen_tx, seen_rx) = mpsc::channel();
        let listener = Listener::open(
            &scratch.0,
            resolve_steps("tok", ollama_entry(None), A_LINEUP, None),
            by_model(vec![(A_LINEUP[0], (401, r#"{"error":"no key"}"#)), (A_LINEUP[1], (200, THE_ANSWER))], seen_tx),
            |_| {},
        )
        .expect("the socket opens");

        // One key, one provider, one answer: the next model of the same provider would be refused
        // the same way, and the person is better served by what the provider actually said.
        let (head, body) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 401);
        assert_eq!(body, br#"{"error":"no key"}"#);
        assert_eq!(asked_models(&seen_rx, 1), ["a/one".to_owned()], "no other model was asked");
    }

    #[test]
    fn a_model_that_cannot_be_reached_hands_its_turn_to_the_next() {
        let scratch = Scratch::new("unreachable");
        let (seen_tx, seen_rx) = mpsc::channel();
        // An address that is not there for the first model and is for the second: what a provider
        // that has come back looks like to a request that is still on its way.
        let upstream = Upstream::new(move |mut ask| {
            let mut sent = Vec::new();
            let _ = ask.body.read_to_end(&mut sent);
            let asked: Option<String> =
                serde_json::from_slice::<Value>(&sent).ok().and_then(|body| body["model"].as_str().map(str::to_owned));
            let _ = seen_tx.send(SeenAsk {
                url: ask.url.clone(),
                headers: ask.headers.clone(),
                secret: ask.secret.clone(),
                body: sent,
            });
            if asked.as_deref() == Some(A_LINEUP[0]) {
                return Err(UpstreamError { url: ask.url.clone(), reason: "connection refused".to_owned() });
            }
            Ok(UpstreamAnswer {
                status: 200,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body: Box::new(std::io::Cursor::new(THE_ANSWER.as_bytes().to_vec())),
            })
        });
        let reported: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let told = Arc::clone(&reported);
        let listener = Listener::open(
            &scratch.0,
            resolve_steps("tok", ollama_entry(None), A_LINEUP, None),
            upstream,
            move |event| told.lock().expect("the events are not poisoned").push(event),
        )
        .expect("the socket opens");

        let (head, body) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 200, "the second model answers");
        assert_eq!(body, THE_ANSWER.as_bytes());
        assert_eq!(asked_models(&seen_rx, 2), A_LINEUP, "both were asked");
        assert_eq!(
            taken(&reported).first(),
            Some(&Event::FellBack {
                token: "tok".to_owned(),
                tag: "ev".to_owned(),
                lineup: None,
                from: "a/one".to_owned(),
                to: "b/two".to_owned(),
                status: None,
            }),
            "an answer that never came is a fall-back with no status to name"
        );
    }

    #[test]
    fn the_last_step_s_own_refusal_reaches_the_harness_as_it_is() {
        let scratch = Scratch::new("laststep");
        let (seen_tx, seen_rx) = mpsc::channel();
        let said = r#"{"error":"the model server is down"}"#;
        let listener = Listener::open(
            &scratch.0,
            resolve_steps("tok", ollama_entry(None), A_LINEUP, None),
            by_model(vec![(A_LINEUP[0], (429, r#"{"error":"busy"}"#)), (A_LINEUP[1], (503, said))], seen_tx),
            |_| {},
        )
        .expect("the socket opens");

        // The harness shows its own tab's error rather than dying, so the tab stays and the
        // person is still there when the provider comes back.
        let (head, body) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 503);
        assert_eq!(body, said.as_bytes(), "the last step's body is not QCode's to change");
        assert_eq!(asked_models(&seen_rx, 2), A_LINEUP, "both were asked and there was nowhere else to go");
    }

    #[test]
    fn an_answer_that_breaks_half_way_is_not_asked_a_second_time() {
        let scratch = Scratch::new("broken");
        let asked_count = Arc::new(Mutex::new(0));
        let counted = Arc::clone(&asked_count);
        let upstream = Upstream::new(move |_| {
            *counted.lock().expect("the lock") += 1;
            Ok(UpstreamAnswer {
                status: 200,
                headers: vec![("content-type".to_owned(), "text/event-stream".to_owned())],
                body: Box::new(Broken(b"data: one\n\ndata: tw".to_vec())),
            })
        });
        let listener =
            Listener::open(&scratch.0, resolve_steps("tok", ollama_entry(None), A_LINEUP, None), upstream, |_| {})
                .expect("the socket opens");

        // What the harness got is what the model managed to say before the stream broke, and the
        // second model is never asked: a turn already begun cannot be begun again.
        let (head, body) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 200);
        assert!(!body.is_empty(), "what came before the break is the harness's: {body:?}");
        assert_eq!(*asked_count.lock().expect("the lock"), 1, "one model was asked and no other");
    }

    #[test]
    fn a_model_that_refused_is_not_asked_again_until_its_minute_is_up() {
        let scratch = Scratch::new("cooling");
        let (seen_tx, seen_rx) = mpsc::channel();
        let listener = Listener::open(
            &scratch.0,
            resolve_steps("tok", ollama_entry(None), A_LINEUP, None),
            one_busy_one_willing(seen_tx),
            |_| {},
        )
        .expect("the socket opens");

        let (head, _) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 200);
        assert_eq!(asked_models(&seen_rx, 2), A_LINEUP);

        // The next request goes straight to the model that answered: a busy free model that is
        // asked first every time costs a round trip per turn for nothing.
        let (head, body) = ask(listener.socket(), "tok", "POST", "/v1/messages", A_HARNESS_ASKING);
        assert_eq!(head["status"], 200);
        assert_eq!(body, THE_ANSWER.as_bytes());
        assert_eq!(asked_models(&seen_rx, 1), ["b/two".to_owned()], "the model that refused is not asked again");
    }

    #[test]
    fn a_cooled_model_is_asked_first_again_once_its_minute_is_up() {
        let now = Instant::now();
        assert_eq!(order_of(A_LINEUP, now + COOLING, now), ["b/two".to_owned(), "a/one".to_owned()]);
        assert_eq!(
            order_of(A_LINEUP, now + COOLING, now + COOLING + Duration::from_secs(1)),
            A_LINEUP.iter().map(|m| (*m).to_owned()).collect::<Vec<_>>(),
            "a model that is back is used in the order the person wrote it"
        );
    }

    #[test]
    fn a_route_of_models_that_are_all_cooling_is_still_asked_in_the_order_it_was_written() {
        let now = Instant::now();
        let every: Vec<(String, Option<String>, String)> =
            A_LINEUP.iter().map(|m| ("ev".to_owned(), Some("bilim".to_owned()), (*m).to_owned())).collect();
        let seen: Cooling = every.iter().map(|step| (step.clone(), now + COOLING)).collect();
        let models: Vec<String> = A_LINEUP.iter().map(|m| (*m).to_owned()).collect();
        assert_eq!(
            ordered(&models, &seen, &|model| ("ev".to_owned(), Some("bilim".to_owned()), model.to_owned()), now),
            A_LINEUP.iter().map(|m| (*m).to_owned()).collect::<Vec<_>>(),
            "a model that keeps refusing is still the one the person chose"
        );
    }

    #[test]
    fn a_cooling_step_is_only_skipped_for_the_provider_and_lineup_it_refused_in() {
        let now = Instant::now();
        let seen: Cooling = [("ev".to_owned(), Some("bilim".to_owned()), "a/one".to_owned())]
            .into_iter()
            .map(|step| (step, now + COOLING))
            .collect();
        let models = vec!["a/one".to_owned()];
        let order = |tag: &str, lineup: Option<&str>| {
            ordered(&models, &seen, &|model| (tag.to_owned(), lineup.map(str::to_owned), model.to_owned()), now)
        };
        assert_eq!(order("ev", Some("bilim")), ["a/one".to_owned()], "every step is cooling, so the plain order");
        assert_eq!(order("başka", Some("bilim")), ["a/one".to_owned()], "another provider's models are not");
        assert_eq!(order("ev", Some("başka")), ["a/one".to_owned()], "another lineup's steps are not");
        assert_eq!(order("ev", None), ["a/one".to_owned()], "nor a single model's own");
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
            Listener::open(&scratch.0, resolve_one("tok", entry, CHOSEN), upstream, |_| {}).expect("the socket opens");

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
        // The stand-in reaches the address it was told of, as a harness does; the relay moves it
        // to another port when the usual one is taken (by another test, say).
        let reach = "const u=new URL(process.env.QCODE_TEST_BASE);\
             const s=require('node:net').connect(Number(u.port),'127.0.0.1');\
             s.on('connect',()=>{s.end();process.exit(23)});s.on('error',()=>process.exit(1));";
        let ran = std::process::Command::new(&node)
            .args([
                script.as_os_str(),
                std::ffi::OsStr::new(&node),
                std::ffi::OsStr::new("-e"),
                std::ffi::OsStr::new(reach),
            ])
            .env("QCODE_TEST_BASE", format!("http://127.0.0.1:{PORT}"))
            .output()
            .expect("the script runs");
        assert_eq!(
            ran.status.code(),
            Some(23),
            "the harness reached the relay and its own code came back: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
    }

    /// A second tab of the same profile shares its container, and so the relay's usual port. The
    /// second relay listens on a free port instead and tells its harness that one, in the
    /// environment and on the command line where QCode wrote the usual address; before, it died on
    /// the taken port and the tab never started (seen in the endurance trial, 2026-09-24).
    #[test]
    fn a_relay_whose_port_is_taken_listens_on_another_and_tells_its_harness_so() {
        let Some(node) = node() else { return };
        // Held for the whole test; if something else on this machine holds it, it is taken all the same.
        let _held = std::net::TcpListener::bind(("127.0.0.1", PORT));
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/provider/qcode-relay.mjs");
        let address = format!("http://127.0.0.1:{PORT}/v1");
        let reach = "const u=new URL(process.env.QCODE_TEST_BASE),a=new URL(process.argv[1]);\
             console.log(u.port+' '+a.port);\
             const s=require('node:net').connect(Number(u.port),'127.0.0.1');\
             s.on('connect',()=>{s.end();process.exit(23)});s.on('error',()=>process.exit(1));";
        let ran = std::process::Command::new(&node)
            .args([
                script.as_os_str(),
                std::ffi::OsStr::new(&node),
                std::ffi::OsStr::new("-e"),
                std::ffi::OsStr::new(reach),
                std::ffi::OsStr::new(&address),
            ])
            .env("QCODE_TEST_BASE", &address)
            .output()
            .expect("the script runs");
        assert_eq!(
            ran.status.code(),
            Some(23),
            "the harness reached its relay: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
        let printed = String::from_utf8_lossy(&ran.stdout);
        let ports: Vec<&str> = printed.split_whitespace().collect();
        assert_eq!(ports.len(), 2, "{printed}");
        assert_eq!(ports[0], ports[1], "the environment and the arguments name the same port: {printed}");
        assert_ne!(ports[0], PORT.to_string(), "not the taken one: {printed}");
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
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry, CHOSEN), canned(200, "{}", seen_tx), |_| {})
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
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry, CHOSEN), canned(200, "{}", seen_tx), |_| {})
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
        let listener =
            Listener::open(&scratch.0, resolve_one("tok", entry, CHOSEN), canned(200, "{}", seen_tx), |_| {})
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
