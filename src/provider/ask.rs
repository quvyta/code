//! Asking a provider what it can do, over a seam a test can stand in for.
//!
//! Every request in QCode that leaves the machine goes through [`Web`], for the same reason
//! every installation goes through [`Installer`](crate::ui::setup::install::Installer): the real
//! one is built in one place, and a test is handed one that answers from a string it was given.
//! No test in this crate can reach the network, whatever it asks for, and the suite is the same
//! on a machine with no server anywhere.
//!
//! Three things are asked here.
//!
//! **What models there are**, and **what window each of them claims** — both from the
//! provider's own API, never guessed and never scraped: ollama answers `/api/tags` and
//! `/api/show`, OpenRouter answers `/api/v1/models`.
//!
//! **What window the server really gives**, which is the one that matters and the one nobody
//! tells you. A model's record can say 262 144 while the endpoint a harness actually uses pins
//! the window to 4 096 and throws the front of every larger prompt away without a word or an
//! error. [`measure`] finds that edge the only way it can be found: it sends prompts of growing
//! size and watches where the input token count the server reports stops growing.
//!
//! A request goes out only when the person asked for it. Nothing here runs because a page was
//! opened.

use std::sync::Arc;
use std::time::Duration;

use super::{Key, Measured, Model, ProviderEntry, ProviderKind, Wire};

/// How long a single request may take before it is given up on. Generous rather than tight: a
/// large probe on a loaded machine makes the server load a model first, and a short limit would
/// report a working server as broken. Finite, because a page that waits forever is a page that
/// is stuck.
const PATIENCE: Duration = Duration::from_secs(120);

/// How much of a server's answer is ever repeated back to the person. Enough to recognise the
/// complaint, short enough that a page is not filled with someone's HTML error page.
const QUOTED: usize = 240;

/// The two methods anything here uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Reads something the provider publishes.
    Get,
    /// Sends something the provider reads.
    Post,
}

impl Method {
    /// The word as it goes on the wire, which is also what the page shows before the request.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// How a key is carried into a request.
///
/// It is a field of its own rather than another header so that no code path can put a key in a
/// URL or print one with the headers: the key inside is a [`Key`], whose `Debug` is the last
/// four characters.
#[derive(Debug, Clone)]
pub struct Secret {
    /// The header the key goes in.
    pub header: String,
    /// What stands in front of the key in that header, such as `Bearer `.
    pub prefix: String,
    /// The key itself.
    pub key: Key,
}

/// One request to a provider.
#[derive(Debug, Clone)]
pub struct Ask {
    /// What is being done.
    pub method: Method,
    /// Where it goes. A key is never part of an address, so this is safe to show and to record.
    pub url: String,
    /// The headers that hold nothing secret.
    pub headers: Vec<(String, String)>,
    /// The body, for a request that has one.
    pub body: Option<String>,
    /// The key, for a request that carries one.
    pub secret: Option<Secret>,
}

impl Ask {
    /// The line the page shows before a request goes out, so that nothing leaves the machine
    /// towards an address the person has not read.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{} {}", self.method.name(), self.url)
    }
}

/// What a provider answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The status it answered with.
    pub status: u16,
    /// What it said.
    pub body: String,
}

/// Why a question could not be answered.
///
/// None of these carries a key: a key never goes in an address, and what is quoted back is the
/// server's own words, not the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    /// The address could not be reached at all.
    Unreachable {
        /// Where the request was going.
        url: String,
        /// What the network said.
        reason: String,
    },
    /// The server answered, and refused.
    Refused {
        /// Where the request went.
        url: String,
        /// The status it refused with.
        status: u16,
        /// The beginning of what it said.
        said: String,
    },
    /// The answer was not the shape this provider's API promises.
    Unreadable {
        /// Where the request went.
        url: String,
        /// What was looked for and not found.
        wanted: String,
    },
}

impl AskError {
    /// Where the request was going, which is what the person reads beside the trouble.
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Self::Unreachable { url, .. } | Self::Refused { url, .. } | Self::Unreadable { url, .. } => url,
        }
    }
}

/// Whether a request could be sent to `url` at all, judged the way [`Web::network`] judges it.
///
/// The address is read with the very parser the transport reads it with, then held to the two
/// things the transport asks of it before it opens a connection: a scheme it speaks and a host.
/// Two addresses run together, `http://127.0.0.1:11434http://192.168.122.1:11434`, look like one
/// to the eye and to a check on the first seven characters; this parser sees an authority that
/// cannot be one, and the person hears it while the dialog is still open instead of on every
/// request afterwards.
#[must_use]
pub fn can_be_sent_to(url: &str) -> bool {
    let Ok(uri) = ureq::http::Uri::try_from(url) else {
        return false;
    };
    let spoken = matches!(uri.scheme_str(), Some("http" | "https"));
    spoken && uri.host().is_some_and(|host| !host.is_empty())
}

/// How a request is carried out.
///
/// [`network`](Web::network) is the real one. [`new`](Web::new) takes anything else, which is
/// what every test in this crate uses.
#[derive(Clone)]
pub struct Web(Arc<Carry>);

/// What a [`Web`] does with a request.
type Carry = dyn Fn(&Ask) -> Result<Answer, AskError> + Send + Sync;

impl std::fmt::Debug for Web {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Web")
    }
}

impl Web {
    /// The real one, which goes out over the network.
    #[must_use]
    pub fn network() -> Self {
        Self::new(|ask| {
            let config = ureq::Agent::config_builder()
                .timeout_global(Some(PATIENCE))
                // A refusal is an answer, and its body is what tells the person what the server
                // objected to; it would be lost if the status alone became an error.
                .http_status_as_error(false)
                .build();
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
            let sent = match &ask.body {
                Some(body) => builder.content_type("application/json").send(body.as_str()),
                None => builder.send_empty(),
            };
            // What the transport says can name the address; it can never name the key, which is
            // only ever a header value and never part of the address.
            let mut answer =
                sent.map_err(|error| AskError::Unreachable { url: ask.url.clone(), reason: error.to_string() })?;
            let status = answer.status().as_u16();
            let body = answer
                .body_mut()
                .read_to_string()
                .map_err(|error| AskError::Unreachable { url: ask.url.clone(), reason: error.to_string() })?;
            Ok(Answer { status, body })
        })
    }

    /// A web that carries requests the way `carry` says.
    #[must_use]
    pub fn new(carry: impl Fn(&Ask) -> Result<Answer, AskError> + Send + Sync + 'static) -> Self {
        Self(Arc::new(carry))
    }

    /// Carries out `ask`, turning anything but a plain success into an [`AskError`].
    ///
    /// # Errors
    ///
    /// When the address cannot be reached, when the server refuses, or when what came back is
    /// not JSON.
    pub fn json(&self, ask: &Ask) -> Result<serde_json::Value, AskError> {
        let answer = (self.0)(ask)?;
        if !(200..300).contains(&answer.status) {
            return Err(AskError::Refused { url: ask.url.clone(), status: answer.status, said: shorten(&answer.body) });
        }
        serde_json::from_str(&answer.body)
            .map_err(|error| AskError::Unreadable { url: ask.url.clone(), wanted: error.to_string() })
    }
}

/// The beginning of `said`, so that a page is never filled with someone's error page.
fn shorten(said: &str) -> String {
    let trimmed = said.trim();
    match trimmed.char_indices().nth(QUOTED) {
        Some((end, _)) => format!("{}…", &trimmed[..end]),
        None => trimmed.to_owned(),
    }
}

/// The request that lists what a provider offers.
#[must_use]
pub fn listing(entry: &ProviderEntry) -> Ask {
    match entry.kind {
        ProviderKind::Ollama => plain(Method::Get, entry.address("/api/tags")),
        ProviderKind::OpenRouter => plain(Method::Get, entry.address("/api/v1/models")),
    }
}

/// The request that asks an ollama server about one model, which is where its claimed window
/// comes from.
#[must_use]
pub fn showing(entry: &ProviderEntry, model: &str) -> Ask {
    let body = serde_json::json!({ "model": model }).to_string();
    Ask { method: Method::Post, url: entry.address("/api/show"), headers: Vec::new(), body: Some(body), secret: None }
}

/// The request that tries the connection: the one thing that only ever goes out because the
/// person pressed something.
#[must_use]
pub fn trial(entry: &ProviderEntry) -> Ask {
    match entry.kind {
        ProviderKind::Ollama => plain(Method::Get, entry.address("/api/version")),
        // The key's own endpoint, so that the trial really tries the key rather than an address
        // that answers whether or not the key is any good — and so that trying a connection
        // never spends anything.
        ProviderKind::OpenRouter => with_key(Method::Get, entry.address("/api/v1/key"), entry),
    }
}

/// A request that carries nothing secret.
fn plain(method: Method, url: String) -> Ask {
    Ask { method, url, headers: Vec::new(), body: None, secret: None }
}

/// The same request with `entry`'s key on it, when it has one.
fn with_key(method: Method, url: String, entry: &ProviderEntry) -> Ask {
    let secret =
        entry.key.clone().map(|key| Secret { header: "Authorization".to_owned(), prefix: "Bearer ".to_owned(), key });
    Ask { method, url, headers: Vec::new(), body: None, secret }
}

/// What a provider offers, with the window each model claims.
///
/// The claimed window is asked for, not assumed: OpenRouter gives it with the listing, and an
/// ollama server is asked about each model in turn. A model whose record cannot be read keeps
/// its place in the list with nothing claimed, because a model that is there is worth naming
/// even when its record is not.
///
/// # Errors
///
/// When the provider could not be reached or its listing could not be read.
pub fn list_models(web: &Web, entry: &ProviderEntry) -> Result<Vec<Model>, AskError> {
    let ask = listing(entry);
    let answered = web.json(&ask)?;
    match entry.kind {
        ProviderKind::Ollama => {
            let names = answered
                .get("models")
                .and_then(|models| models.as_array())
                .ok_or_else(|| AskError::Unreadable { url: ask.url.clone(), wanted: "models".to_owned() })?;
            let names: Vec<String> =
                names.iter().filter_map(|model| model.get("name")?.as_str().map(str::to_owned)).collect();
            Ok(names
                .into_iter()
                .map(|id| {
                    let claimed = claimed_by_ollama(web, entry, &id);
                    Model { id, claimed, measured: None }
                })
                .collect())
        }
        ProviderKind::OpenRouter => {
            let models = answered
                .get("data")
                .and_then(|data| data.as_array())
                .ok_or_else(|| AskError::Unreadable { url: ask.url.clone(), wanted: "data".to_owned() })?;
            Ok(models
                .iter()
                .filter_map(|model| {
                    let id = model.get("id")?.as_str()?.to_owned();
                    let claimed = model.get("context_length").and_then(serde_json::Value::as_u64);
                    Some(Model { id, claimed, measured: None })
                })
                .collect())
        }
    }
}

/// The window `model`'s own record claims on an ollama server, or `None` when the server would
/// not say. A record that cannot be read never costs the model its place in the list.
fn claimed_by_ollama(web: &Web, entry: &ProviderEntry, model: &str) -> Option<u64> {
    let answered = web.json(&showing(entry, model)).ok()?;
    let info = answered.get("model_info")?.as_object()?;
    // The key is named after the architecture the model was built with, so it is found by its
    // ending rather than by a name QCode would have to keep a list of.
    info.iter().find(|(name, _)| name.ends_with(".context_length")).and_then(|(_, value)| value.as_u64())
}

/// What a trial found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reached {
    /// The server answered and named its own version.
    Version(String),
    /// The key was accepted.
    KeyAccepted,
}

/// Tries the connection to `entry` and says what answered.
///
/// # Errors
///
/// When the address could not be reached, or the server refused; for a provider reached with a
/// key, a refusal is what a key that is no longer good looks like.
pub fn try_connection(web: &Web, entry: &ProviderEntry) -> Result<Reached, AskError> {
    let ask = trial(entry);
    let answered = web.json(&ask)?;
    match entry.kind {
        ProviderKind::Ollama => {
            let version = answered
                .get("version")
                .and_then(|version| version.as_str())
                .ok_or_else(|| AskError::Unreadable { url: ask.url.clone(), wanted: "version".to_owned() })?;
            Ok(Reached::Version(version.to_owned()))
        }
        ProviderKind::OpenRouter => match answered.get("data") {
            Some(_) => Ok(Reached::KeyAccepted),
            None => Err(AskError::Unreadable { url: ask.url.clone(), wanted: "data".to_owned() }),
        },
    }
}

/// How large each probe's prompt is, in words. Four sizes, growing by about four times each, so
/// that a window pinned anywhere in the usual range falls between two of them. The largest is
/// well past any window a harness could use, because a server whose edge is never reached can
/// only be reported as "at least", and "at least twelve thousand" was measured reading as good
/// news on a server that in truth stops at sixteen.
const PROBE_WORDS: [usize; 4] = [400, 3_000, 12_000, 48_000];

/// How much of a probe has to come back counted for the prompt to have been taken whole. The
/// filler is ordinary short words, so a tokeniser gives about one token a word; this leaves room
/// for one that gives fewer without calling an untouched prompt cut.
const WHOLE_SHARE: u64 = 80;

/// The ordinary words a probe's prompt is built from. Plain prose rather than one word repeated,
/// because a repeated word is not what a real prompt looks like to a tokeniser.
const FILLER: [&str; 8] = ["the", "quiet", "river", "carries", "another", "small", "stone", "downstream"];

/// Measures the window `model` really gets on this server, by sending prompts of growing size
/// and watching for the first one the server does not read whole.
///
/// This is the number the page shows beside what the model claims, and the two are often not the
/// same: the endpoint a harness uses can pin the window far below the model's own capacity and
/// throw the front of every larger prompt away in silence. Probing stops at the first prompt
/// that was cut, so a server with a small window is asked twice rather than four times.
///
/// Every probe carries its own opening, and `run` makes this measurement's openings unlike any
/// other's. That is not decoration. A server that remembers the front of a prompt it has already
/// read counts only what it had to read afresh, so probes that shared an opening were answered
/// with numbers far below the prompts they were sent — on the owner's own server a window of
/// 16 384 was reported as 9 512, and asking twice reported 4. Nothing shares an opening now, so
/// the count is of the prompt rather than of the part that was new.
///
/// # Errors
///
/// When the provider could not be reached, refused, or answered without a token count.
pub fn measure(web: &Web, entry: &ProviderEntry, model: &str) -> Result<Measured, AskError> {
    let run = run_mark();
    let mut whole = 0;
    for words in PROBE_WORDS {
        let tokens = probe(web, entry, model, words, &run)?;
        // A prompt that came back counted far short of what was sent was cut, and where it was
        // cut is the window. A larger prompt would be cut to the same place and teach nothing.
        if cut(words, tokens) {
            return Ok(Measured::About(tokens));
        }
        whole = whole.max(tokens);
    }
    // Every probe was taken whole. All that is known is that the window holds the largest one;
    // where it ends was not found, and a number for it would be invented.
    Ok(Measured::AtLeast(whole))
}

/// Whether a probe of `words` words came back counted too short to have been read whole.
fn cut(words: usize, tokens: u64) -> bool {
    let sent = u64::try_from(words).unwrap_or(u64::MAX);
    tokens < sent.saturating_mul(WHOLE_SHARE) / 100
}

/// An opening no other measurement will use, so that no probe of this run can be answered out of
/// what a server remembers of an earlier one.
fn run_mark() -> String {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("m{}", since.as_nanos())
}

/// The request one probe of `words` words is made of, opening with `run` and its own size so
/// that no two probes begin alike. See [`measure`] for why that matters.
#[must_use]
pub fn probing(entry: &ProviderEntry, model: &str, words: usize, run: &str) -> Ask {
    // The opening is this run's mark and this probe's size, so no two probes of one measurement
    // begin alike either, and a server that remembers a prefix has nothing of theirs to remember.
    let filler = (0..words).map(|index| FILLER[index % FILLER.len()].to_owned());
    let opening = [run.to_owned(), words.to_string()];
    let prompt: String = opening.into_iter().chain(filler).collect::<Vec<String>>().join(" ");
    // One token of answer: the question is what the server read, never what it writes.
    let body = match entry.wire {
        Wire::Anthropic => serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{ "role": "user", "content": prompt }],
        }),
        Wire::OpenAi => serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{ "role": "user", "content": prompt }],
        }),
    };
    let mut ask = with_key(Method::Post, entry.messages_address(), entry);
    if entry.wire == Wire::Anthropic {
        ask.headers.push(("anthropic-version".to_owned(), "2023-06-01".to_owned()));
    }
    ask.body = Some(body.to_string());
    ask
}

/// Sends one probe and answers how many input tokens the server says it read.
fn probe(web: &Web, entry: &ProviderEntry, model: &str, words: usize, run: &str) -> Result<u64, AskError> {
    let ask = probing(entry, model, words, run);
    let answered = web.json(&ask)?;
    let usage = answered.get("usage");
    let field = match entry.wire {
        Wire::Anthropic => "input_tokens",
        Wire::OpenAi => "prompt_tokens",
    };
    usage
        .and_then(|usage| usage.get(field))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| AskError::Unreadable { url: ask.url.clone(), wanted: format!("usage.{field}") })
}
