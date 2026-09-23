//! The way back of a sign-in, carried for real: a container with no network, a program inside it
//! that asks for a sign-in page the way an application does and waits on its own `localhost` for
//! the answer, and this test playing the browser Google sends back.
//!
//! No real browser and no real Google: the program inside is a small server of this test's own,
//! and the browser is one plain HTTP request. Everything between them is the application's own —
//! the program that leaves the address ([`signin::opener_step`]), the reading of the way back
//! ([`callback::return_to`]), the listening ([`callback::Listener`]) and the command that carries
//! each connection in ([`callback::carrier`]).
//!
//! `#[ignore]`d, and doing nothing unless `QCODE_CONTAINER_TESTS=1`:
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test callback_live -- --ignored --test-threads=1
//! ```
//!
//! Everything a run makes is named `qcode/profile/callbacktest` or `qcode-callbacktest` and is
//! removed again. The machine's own `qcode/base` is built when it is missing and left.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{callback, signin};
use crate::base::paths::USER;
use crate::engine::run::{build_image, capture};
use crate::engine::{
    Access, ContainerCreate, Engine, EngineKind, Exec, HostUser, ImageBuild, Mount, MountSource, Network, detect, names,
};

/// The image and the container a run makes.
const IMAGE: &str = "qcode/profile/callbacktest";
const CONTAINER: &str = "qcode-callbacktest";

/// The program inside: it listens on the container's `127.0.0.1`, then asks for a sign-in page
/// whose way back is that port by running the program an application's `xdg-open` runs, and
/// writes down every request it is sent before answering it with the words it was given.
///
/// Its arguments are the port, the opener, the words, and the file the requests go into.
const APPLICATION: &str = r"'use strict';
const http = require('http');
const fs = require('fs');
const { execFile } = require('child_process');
const [port, opener, words, seen] = process.argv.slice(1);
const server = http.createServer((request, response) => {
  fs.appendFileSync(seen, request.method + ' ' + request.url + ' ' + request.headers.host + '\n');
  response.end(words);
});
server.listen(Number(port), '127.0.0.1', () => {
  const back = encodeURIComponent('http://localhost:' + port + '/oauth-callback');
  execFile(opener, ['https://accounts.example/o/oauth2/auth?client_id=test&redirect_uri=' + back + '&state=s']);
});
";

/// Where the program inside writes down the requests it is sent.
const SEEN: &str = "/tmp/qcode-seen";

/// The engines installed on this machine, or nothing at all when the tests are switched off.
fn engines() -> Vec<Engine> {
    if std::env::var("QCODE_CONTAINER_TESTS").as_deref() != Ok("1") {
        return Vec::new();
    }
    let found: Vec<Engine> =
        [EngineKind::Podman, EngineKind::Docker].into_iter().filter_map(|kind| detect(kind).ok()).collect();
    assert!(!found.is_empty(), "these tests were asked for and no engine answered");
    found
}

/// A folder of this run's own, removed when the run ends however it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-callbacktest-{name}-{stamp}"));
        std::fs::create_dir_all(&path).expect("a folder of this test's own");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Takes away the container and the image, whatever state a run left them in.
struct Cleared<'a>(&'a Engine);

impl Drop for Cleared<'_> {
    fn drop(&mut self) {
        let _ = capture(&self.0.remove_container(CONTAINER));
        let _ = capture(&self.0.remove_image(IMAGE));
    }
}

/// Builds the image: QCode's base with the program that leaves an address, put there by the very
/// step a desktop harness's image is given.
fn build(engine: &Engine, folder: &Path) {
    crate::base::ensure(engine, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("the base image does not build on {:?}: {error:?}", engine.kind()));
    let containerfile = folder.join("Containerfile");
    let text = format!(
        "FROM {base}\nUSER root\n{opener}\nUSER {USER}\n",
        base = crate::base::Os::Debian.image(),
        opener = signin::opener_step()
    );
    std::fs::write(&containerfile, text).expect("a Containerfile");
    let mut said = String::new();
    let built = build_image(
        engine,
        &ImageBuild { image: IMAGE, containerfile: &containerfile, context: folder },
        &|| false,
        &mut |line| {
            said.push_str(line);
            said.push('\n');
        },
    );
    assert!(built.is_ok(), "the test image does not build on {:?}:\n{said}", engine.kind());
}

/// A port this machine does not listen on right now: the one the application inside is told to
/// wait on, so that QCode can listen on the same one here.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").and_then(|socket| socket.local_addr()).expect("a free port").port()
}

/// Runs `script` with a shell in the container and answers what it printed.
fn inside(engine: &Engine, script: &str) -> String {
    let command = ["sh", "-c", script];
    capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: &command }))
        .unwrap_or_else(|error| panic!("{script} failed on {:?}: {error:?}", engine.kind()))
}

/// Plays the browser Google sends back: one request to `port` of this machine, and the answer.
fn come_back(port: u16, target: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("QCode listens there");
    stream.set_read_timeout(Some(Duration::from_secs(120))).expect("a finite wait");
    write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n")
        .expect("the request goes");
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_sign_in_comes_back_from_this_machine_to_a_container_with_no_network_on_both_engines() {
    for engine in engines() {
        let kind = engine.kind();
        let _cleared = Cleared(&engine);
        let _ = capture(&engine.remove_container(CONTAINER));
        let context = Scratch::new("image");
        build(&engine, &context.0);

        // The folder the application leaves its addresses in, shared the way a window's is.
        let open = Scratch::new("open");
        let port = free_port();
        let words = format!("signed-in-{port}");
        let port_word = port.to_string();
        let command = ["node", "-e", APPLICATION, port_word.as_str(), signin::OPEN_PROGRAM, words.as_str(), SEEN];
        let mounts = [Mount {
            source: MountSource::Path(&open.0),
            target: Path::new(signin::OPEN_DIR),
            access: Access::ReadWrite,
        }];
        let create = ContainerCreate {
            name: CONTAINER,
            hostname: names::HOSTNAME,
            labels: &[],
            image: IMAGE,
            mounts: &mounts,
            network: Network::None,
            user: HostUser::current().expect("the current user"),
            workdir: None,
            command: &command,
        };
        capture(&engine.create_container(&create)).unwrap_or_else(|error| panic!("{kind:?} creates it: {error:?}"));
        capture(&engine.start_container(CONTAINER)).unwrap_or_else(|error| panic!("{kind:?} starts it: {error:?}"));

        // The container has its loopback and nothing else to reach anything by.
        let interfaces =
            inside(&engine, "node -e \"console.log(Object.keys(require('os').networkInterfaces()).join(' '))\"");
        assert_eq!(interfaces.trim(), "lo", "{kind:?}: the container has no network");

        // The application asks for its page; QCode finds it where the program leaves it.
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut addresses = Vec::new();
        while addresses.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            addresses = signin::taken(&open.0);
        }
        let address = addresses.pop().unwrap_or_else(|| panic!("{kind:?}: the application asked for no page"));
        let back = callback::return_to(&address).unwrap_or_else(|| panic!("{kind:?}: no way back in {address}"));
        assert_eq!((back.port, back.path.as_str()), (port, "/oauth-callback"), "{address}");

        // QCode listens here and carries every connection in.
        let listener = callback::Listener::bind(&back).unwrap_or_else(|why| panic!("{kind:?}: {why:?}"));
        let carrier = callback::carrier(&engine, CONTAINER, &back);
        let serving = std::thread::spawn(move || {
            listener.serve(&carrier, |pause| {
                std::thread::sleep(pause);
                true
            })
        });

        // The browser, sent back with the code.
        let code = format!("4/0Ab-{port}");
        let target = format!("/oauth-callback?code={code}&state=s");
        let started = Instant::now();
        let answer = come_back(port, &target);
        eprintln!("{kind:?}: the browser's request was carried in and answered in {:?}", started.elapsed());
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{kind:?}: {answer}");
        assert!(answer.ends_with(&words), "{kind:?}: the application's own answer came back: {answer}");
        assert_eq!(serving.join().expect("the listening ends"), callback::Ending::Returned, "{kind:?}");

        // The application received exactly that request, as a browser sent it.
        let seen = inside(&engine, &format!("cat {SEEN}"));
        assert_eq!(seen, format!("GET {target} localhost:{port}\n"), "{kind:?}");
        // And the port is this machine's to give again.
        assert!(TcpListener::bind(("127.0.0.1", port)).is_ok(), "{kind:?}: the port is given up");
    }
}
