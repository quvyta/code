//! What a workspace's provider relay tells the person: the line under a tab whose lineup passed one
//! model over for the next, how long it stays, and what is not said at all.
//!
//! Everything here goes through the real thing: a real listener on a real socket in the workspace's
//! own folder, a real request written the way the relay script writes one and carrying the tab's
//! own token, and a real tab behind the screen that is being drawn. Only the provider is a test's
//! own — an [`Upstream`] that answers without the network — and that is the same seam the relay's
//! own tests use.
//!
//! Nothing here reaches into the relay's own bookkeeping to see that a line exists. Every assertion
//! is a line read off the drawn screen, below the terminal it belongs to, which is the only place
//! the person ever meets it.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;

use crate::provider::relay::{SOCKET_NAME, UpstreamAnswer, UpstreamAsk, UpstreamError};
use crate::ui::workspace::relay::TELL;

/// The two models of the lineup these tests fall back between, named the way a provider names them.
const BUSY: &str = "z-ai/glm-4.6:free";
const WILLING: &str = "qwen/qwen3-coder:free";

/// The lineup's name, which is what the line under the tab speaks for.
const LINEUP: &str = "coder";

/// The provider every tab of these tests runs on: an ollama entry, which needs no key, so nothing
/// here can reach the network even by accident.
const TAG: &str = "ev1";

/// A screen wide enough for the longest of these lines whole. A line longer than the middle of
/// the screen is cut with `…`, which is what one test here is about, so the tests that are about
/// the words themselves ask for a screen that can hold them.
const WIDE: (u16, u16) = (150, 32);

/// A `providers.toml` carrying one ollama provider tagged `ev1` with the lineup `coder` of the
/// models `models`, in the order they are written, which is what a person has after writing a
/// lineup on the Providers page. Writing it again with another order is what the person does when
/// they change their mind: `resolve` reads the file fresh for every request.
fn lineup_file(name: &str, models: &[&str]) -> PathBuf {
    // A folder of its own: saving narrows the folder the file is in, and that must never be
    // the machine's shared temporary folder.
    let path = std::env::temp_dir().join(format!("qcode-workspace-lineup-{name}")).join("providers.toml");
    let mut providers = crate::provider::Providers::in_memory();
    let mut entry = crate::provider::ProviderEntry::new(
        crate::provider::Tag::parse(TAG).expect("a tag"),
        crate::provider::ProviderKind::Ollama,
        "http://127.0.0.1:11434",
    );
    entry.lineups.push(crate::provider::Lineup {
        name: crate::provider::Tag::parse(LINEUP).expect("a name"),
        models: models.iter().map(|model| (*model).to_owned()).collect(),
    });
    providers.add(entry).expect("the tag is free");
    providers.at(&path).save().expect("the scratch file is written");
    path
}

/// The lineup these tests fall back through, as the person wrote it.
const LINEUP_AS_WRITTEN: [&str; 2] = [BUSY, WILLING];

/// The models the provider was asked for, in the order they were asked: what would have left the
/// machine, and which of them fell back to which.
type Asked = Arc<Mutex<Vec<String>>>;

/// A provider that answers each model of the lineup with the [`(u16, &str)`] written beside it,
/// and records every model it was asked for before it answers.
///
/// A model that is not named here is answered `400` naming itself, so a test that expected a model
/// to be asked fails on what the harness was told rather than on a line that never came.
fn by_model(answers: Vec<(&'static str, (u16, &'static str))>, asked: Asked) -> crate::provider::Upstream {
    crate::provider::Upstream::new(move |mut ask: UpstreamAsk| {
        let mut sent = Vec::new();
        let _ = ask.body.read_to_end(&mut sent);
        let model: Option<String> =
            serde_json::from_slice::<Value>(&sent).ok().and_then(|body| body["model"].as_str().map(str::to_owned));
        asked.lock().expect("the asks are not poisoned").extend(model.clone());
        let reply = model
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

/// A provider that could not be reached at all, as a connection refused looks from here.
fn refused(url: &str) -> UpstreamError {
    UpstreamError { url: url.to_owned(), reason: "connection refused".to_owned() }
}

/// The relay's socket in the workspace `paths`, reached the way a container reaches it: through the
/// folder rather than by a long path, since a socket's own name is written into the connection and
/// a scratch folder can be longer than a system allows.
fn socket(paths: &WorkspacePaths) -> PathBuf {
    paths.mcp().join(SOCKET_NAME)
}

fn connect(socket: &Path) -> UnixStream {
    if socket.as_os_str().len() <= 107 {
        return UnixStream::connect(socket).expect("the relay answers");
    }
    let folder = socket.parent().expect("a folder of the socket's own");
    let handle = std::fs::File::open(folder).expect("the folder opens");
    let short = format!("/proc/self/fd/{}/{SOCKET_NAME}", std::os::fd::AsRawFd::as_raw_fd(&handle));
    UnixStream::connect(short).expect("the relay answers")
}

/// One request the way the relay script would write it: the head line carrying `token`, then the
/// body naming any model at all, then the write side closed. Answers the head line the relay sent
/// back and the whole of its body, so a test that expects a `200` fails on the answer the harness
/// would have got rather than on silence.
fn ask(socket: &Path, token: &str, body: &[u8]) -> (Value, Vec<u8>) {
    let mut stream = connect(socket);
    let head = json!({ "token": token, "method": "POST", "path": "/v1/messages", "headers": {} }).to_string();
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

/// A harness's own body, naming a model of its own choosing: the relay pins the one the profile
/// chose, and what the harness asked for must make no difference to what the provider is told.
const A_HARNESS_ASKING: &[u8] =
    br#"{"model":"claude-haiku-4-5","max_tokens":1024,"messages":[{"role":"user","content":"selam"}]}"#;

/// A workspace carrying one profile of a provider, with the upstream given, and a tab of that
/// profile open behind a harness that renders the screen.
fn lined_up(scratch: &Scratch, upstream: crate::provider::Upstream, path: PathBuf) -> Harness<Screen> {
    lined_up_on(scratch, upstream, path, SIZE)
}

/// [`lined_up`] on a screen of `size`, for a test whose line is longer than the ordinary one.
fn lined_up_on(
    scratch: &Scratch,
    upstream: crate::provider::Upstream,
    path: PathBuf,
    size: (u16, u16),
) -> Harness<Screen> {
    let profiles = vec![lineup_profile("ev-tab", TAG, LINEUP)];
    let screen = WorkspaceScreen::new(
        Some(engine()),
        HostUser::Ids { uid: 1000, gid: 1000 },
        vec![workspace("firefly", "Firefly", scratch.paths(), profiles)],
    )
    .with_providers_path(Some(path))
    .with_upstream(upstream);
    let mut harness = harness(screen, size.0, size.1);
    open_in(&mut harness, Choice::NewChat("ev-tab".to_owned()));
    harness
}

/// The token of the tab at `index`, which is what its harness's requests carry and what the relay
/// resolves them by.
fn token(harness: &Harness<Screen>, index: usize) -> String {
    harness.app().0.workspace().expect("a workspace is open").tabs()[index].token().to_owned()
}

/// What the middle of the screen says, one row per line with its own spacing out of the way: the
/// panel beside it has rows of its own, and everything a tab says under its terminal is in here.
fn middle(harness: &Harness<Screen>) -> Vec<String> {
    let panel = usize::from(harness.buffer().area.width - super::super::PANEL_WIDTH);
    let screen = harness.screen();
    screen
        .lines()
        .filter(|row| row.len() - row.trim_start().len() < panel)
        .map(|row| row.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect()
}

/// The one line under the open tab's terminal that names `what`, with the panic of a test that
/// wanted one. A line is found by a model or a status it names, which nothing else on the screen
/// does: the terminal of a tab that cannot start says what the engine refused, not this.
fn the_line(harness: &Harness<Screen>, what: &str) -> String {
    let said: Vec<String> = middle(harness).into_iter().filter(|line| line.contains(what)).collect();
    assert_eq!(said.len(), 1, "one line names {what}, not {}:\n{}", said.len(), harness.screen());
    said.into_iter().next().expect("the one line")
}

/// Whether nothing under the open tab's terminal names `what`.
fn says_nothing_about(harness: &Harness<Screen>, what: &str) -> bool {
    middle(harness).iter().all(|line| !line.contains(what))
}

/// Gives the screen as long as it needs to be told what the relay found, and then draws it.
///
/// What the relay reports travels from its own thread and is applied in a later update, so a
/// single step of the harness is not by itself long enough; the bound is generous, and a report
/// that never arrives fails here rather than in an assertion about a line that was never written.
fn told<F: Fn(&Harness<Screen>) -> bool>(harness: &mut Harness<Screen>, said: F) -> &mut Harness<Screen> {
    let started = std::time::Instant::now();
    while !said(harness) {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the relay's report never reached the screen:\n{}",
            harness.screen()
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.advance(Duration::ZERO);
    }
    harness
}

#[test]
fn a_tab_whose_model_was_too_busy_is_told_which_one_is_asked_instead() {
    let scratch = Scratch::new("relay-line");
    let path = lineup_file("line", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let upstream = by_model(
        vec![(BUSY, (429, r#"{"error":"too many requests"}"#)), (WILLING, (200, r#"{"ok":true}"#))],
        Arc::clone(&asked),
    );
    let mut harness = lined_up(&scratch, upstream, path.clone());

    let (head, _) = ask(&socket(&scratch.paths()), &token(&harness, 0), A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "the second model answers: {head}");
    assert_eq!(
        *asked.lock().expect("the asks are not poisoned"),
        [BUSY, WILLING],
        "the lineup is asked in the order the person wrote it, whatever the harness named"
    );

    // The line names the lineup the person chose rather than the provider's own tag behind it,
    // both models, and the status that passed the first one over. It is the last thing in the
    // middle of the screen, which is what under a terminal means.
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    let said = the_line(&harness, BUSY);
    assert_eq!(said, format!("{LINEUP}: {BUSY} answered 429, now {WILLING}"));
    assert_eq!(middle(&harness).last(), Some(&said), "it is under the terminal:\n{}", harness.screen());
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_line_is_under_the_tab_it_happened_to_and_under_no_other() {
    let scratch = Scratch::new("relay-line-tab");
    let path = lineup_file("line-tab", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let upstream = by_model(vec![(BUSY, (429, "{}")), (WILLING, (200, r#"{"ok":true}"#))], Arc::clone(&asked));
    let mut harness = lined_up(&scratch, upstream, path.clone());
    // A second tab of the same profile, so the two tokens differ and the line could be under
    // either of them by mistake.
    open_in(&mut harness, Choice::NewChat("ev-tab".to_owned()));
    let (first, second) = (token(&harness, 0), token(&harness, 1));
    assert_ne!(first, second, "each tab is asked in by a token of its own");
    harness.send(Msg::OpenTab(0));

    // Only the first tab's request falls back.
    let (head, _) = ask(&socket(&scratch.paths()), &first, A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "{head}");
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    assert_eq!(the_line(&harness, BUSY), format!("{LINEUP}: {BUSY} answered 429, now {WILLING}"));

    // The second tab is on screen: it took no part, and says nothing.
    harness.send(Msg::OpenTab(1));
    assert!(says_nothing_about(&harness, BUSY), "the other tab says nothing:\n{}", harness.screen());

    harness.send(Msg::OpenTab(0));
    assert!(!says_nothing_about(&harness, BUSY), "and the line is still where it was:\n{}", harness.screen());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_model_that_could_not_be_reached_at_all_is_said_in_its_own_words() {
    let scratch = Scratch::new("relay-unreachable");
    let path = lineup_file("unreachable", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let asking = asked;
    // The first model cannot be reached at all rather than answering with a status, which is what
    // a provider that is down or a machine with no route to it looks like from here.
    let upstream = crate::provider::Upstream::new(move |mut ask: UpstreamAsk| {
        let mut sent = Vec::new();
        let _ = ask.body.read_to_end(&mut sent);
        let model: Option<String> =
            serde_json::from_slice::<Value>(&sent).ok().and_then(|body| body["model"].as_str().map(str::to_owned));
        asking.lock().expect("the asks are not poisoned").extend(model.clone());
        if model.as_deref() == Some(BUSY) {
            return Err(refused(&ask.url));
        }
        Ok(UpstreamAnswer {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Box::new(std::io::Cursor::new(r#"{"ok":true}"#.as_bytes().to_vec())),
        })
    });
    let mut harness = lined_up_on(&scratch, upstream, path.clone(), WIDE);

    let (head, _) = ask(&socket(&scratch.paths()), &token(&harness, 0), A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "{head}");
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    assert_eq!(the_line(&harness, BUSY), format!("{LINEUP}: {BUSY} could not be reached, now {WILLING}"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_line_longer_than_the_screen_is_cut_rather_than_wrapped() {
    let scratch = Scratch::new("relay-cut");
    let path = lineup_file("cut", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let upstream = by_model(vec![(BUSY, (429, "{}")), (WILLING, (200, r#"{"ok":true}"#))], Arc::clone(&asked));
    let mut harness = lined_up(&scratch, upstream, path.clone());

    let (head, _) = ask(&socket(&scratch.paths()), &token(&harness, 0), A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "{head}");
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    let whole = the_line(&harness, BUSY);

    // A narrow screen has no room for the whole sentence, and a harness's terminal must not be
    // rebuilt to make room for one: the line is cut with `…` and stays a single row.
    harness.resize(80, 24).render();
    let cut = the_line(&harness, BUSY);
    assert!(cut.ends_with('…'), "the sentence is cut where the width ends: {cut}");
    assert!(whole.starts_with(cut.trim_end_matches('…')), "the same sentence, shorter: {cut} of {whole}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_request_that_was_carried_through_says_nothing_under_any_tab() {
    let scratch = Scratch::new("relay-plain");
    let path = lineup_file("plain", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    // The first model answers, so there is nothing to fall back from; a second request is one that
    // does fall back, so a screen that stays quiet after the first is quiet by its own doing.
    let turn = Arc::new(Mutex::new(0));
    let counting = Arc::clone(&turn);
    let noting = Arc::clone(&asked);
    let upstream = crate::provider::Upstream::new(move |mut ask: UpstreamAsk| {
        let mut sent = Vec::new();
        let _ = ask.body.read_to_end(&mut sent);
        let model: Option<String> =
            serde_json::from_slice::<Value>(&sent).ok().and_then(|body| body["model"].as_str().map(str::to_owned));
        noting.lock().expect("the asks are not poisoned").extend(model.clone());
        let first = {
            let mut turn = counting.lock().expect("the turn is not poisoned");
            *turn += 1;
            *turn
        };
        let status = match (model.as_deref(), first) {
            (Some(BUSY), 1) => 200,
            (Some(BUSY), _) => 429,
            (Some(WILLING), _) => 200,
            _ => 400,
        };
        Ok(UpstreamAnswer {
            status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Box::new(std::io::Cursor::new(r#"{"ok":true}"#.as_bytes().to_vec())),
        })
    });
    let mut harness = lined_up(&scratch, upstream, path.clone());
    let token = token(&harness, 0);

    // The turn that worked: the harness got the model's own answer, and only the model it chose
    // was asked.
    let (head, body) = ask(&socket(&scratch.paths()), &token, A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "{head}");
    assert_eq!(body, br#"{"ok":true}"#, "the harness got the provider's own answer");
    assert_eq!(*asked.lock().expect("the asks are not poisoned"), [BUSY], "only the model it chose");

    // Then a turn that does fall back, and is said. The relay reports both over one channel in
    // the order they happened, so the line being on screen is proof the worked turn was applied
    // too: what it reported was not said, because there was nothing to say, and the one line
    // there is the fall back's.
    let (head, _) = ask(&socket(&scratch.paths()), &token, A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "{head}");
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    assert_eq!(the_line(&harness, BUSY), format!("{LINEUP}: {BUSY} answered 429, now {WILLING}"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_line_is_gone_ten_seconds_later_without_the_person_asking_for_it_to_be() {
    let scratch = Scratch::new("relay-tell");
    let path = lineup_file("tell", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let upstream = by_model(vec![(BUSY, (429, "{}")), (WILLING, (200, r#"{"ok":true}"#))], Arc::clone(&asked));
    let mut harness = lined_up(&scratch, upstream, path.clone());

    let (head, _) = ask(&socket(&scratch.paths()), &token(&harness, 0), A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "{head}");
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    assert!(!says_nothing_about(&harness, BUSY), "the line is there to begin with:\n{}", harness.screen());

    // Nothing the person does takes it away; the screen's own clock does, on the timer the fall
    // back was given. Ten seconds is spent by the harness's clock rather than by the machine's,
    // which is what makes this a test of the line's end rather than of a wait.
    harness.advance(TELL * 2);
    assert!(says_nothing_about(&harness, BUSY), "it is gone by itself:\n{}", harness.screen());

    // And it is gone whichever tab is looked at afterwards: a line that had been taken away does
    // not come back when the person comes back to the tab.
    harness.send(Msg::OpenTab(0)).advance(TELL * 2);
    assert!(says_nothing_about(&harness, BUSY), "and stays gone:\n{}", harness.screen());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_second_fall_back_says_the_newer_one_and_the_older_line_does_not_take_it_away() {
    let scratch = Scratch::new("relay-newer");
    let path = lineup_file("newer", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    // Both models refuse, so a request falls back from the first to the second and the harness
    // gets the second's own refusal.
    let upstream = by_model(vec![(BUSY, (429, "{}")), (WILLING, (503, "{}"))], Arc::clone(&asked));
    let mut harness = lined_up(&scratch, upstream, path.clone());
    let token = token(&harness, 0);

    let (head, _) = ask(&socket(&scratch.paths()), &token, A_HARNESS_ASKING);
    assert_eq!(head["status"], 503, "the last step's own answer reaches the harness: {head}");
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    assert_eq!(the_line(&harness, BUSY), format!("{LINEUP}: {BUSY} answered 429, now {WILLING}"));

    // While that line is on screen the person writes the lineup the other way round, which the
    // relay reads afresh for the very next request: the same two busy models in the other order,
    // and so a fall back in the other direction. Both are cooling down by now, which is why it
    // asks the one that refused last time first.
    let path = lineup_file("newer", &[WILLING, BUSY]);
    harness.advance(Duration::from_secs(5));
    let (head, _) = ask(&socket(&scratch.paths()), &token, A_HARNESS_ASKING);
    assert_eq!(head["status"], 429, "the last step's own answer reaches the harness: {head}");
    told(&mut harness, |harness| !says_nothing_about(harness, "503"));
    assert_eq!(the_line(&harness, "503"), format!("{LINEUP}: {WILLING} answered 503, now {BUSY}"));

    // The older line's own ten seconds are up and take nothing with them: what is on screen is the
    // newer fall back's, on its own ten seconds, counted from when it was said.
    harness.advance(Duration::from_secs(6));
    assert_eq!(the_line(&harness, "503"), format!("{LINEUP}: {WILLING} answered 503, now {BUSY}"));

    // And the newer line goes when its own are.
    harness.advance(Duration::from_secs(5));
    assert!(says_nothing_about(&harness, WILLING), "and then it is gone:\n{}", harness.screen());
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_line_reads_in_turkish_too() {
    let scratch = Scratch::new("relay-tr");
    let path = lineup_file("tr", &LINEUP_AS_WRITTEN);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let upstream = by_model(vec![(BUSY, (429, "{}")), (WILLING, (200, r#"{"ok":true}"#))], Arc::clone(&asked));
    let mut harness = lined_up_on(&scratch, upstream, path.clone(), WIDE);
    harness.set_locale("tr");

    let (head, _) = ask(&socket(&scratch.paths()), &token(&harness, 0), A_HARNESS_ASKING);
    assert_eq!(head["status"], 200, "{head}");
    told(&mut harness, |harness| !says_nothing_about(harness, BUSY));
    assert_eq!(the_line(&harness, BUSY), format!("{LINEUP}: {BUSY} 429 verdi, şimdi {WILLING}"));
    let _ = std::fs::remove_file(path);
}
