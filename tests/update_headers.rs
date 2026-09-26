//! What the update question sends, measured on the wire, so the README's list of headers stays
//! true for the binary that is shipped.
//!
//! The framework asks with a plain `ureq` agent and one `User-Agent` header. Which other headers go
//! out depends on the features `ureq` is compiled with, and those are the union of every crate in
//! this build: the framework alone has no gzip, but qcode's own `ureq` dependency brings it in. The
//! request below is built the way the framework's `fetch` builds it, inside this crate's build, and
//! read back by a listener of the test's own.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

#[test]
fn the_update_question_carries_only_the_headers_the_readme_names() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port on loopback");
    let port = listener.local_addr().expect("its address").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the question arrives");
        stream.set_read_timeout(Some(Duration::from_secs(30))).expect("a finite wait");
        let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("readable") == 0 || line.trim().is_empty() {
                break;
            }
            headers.push(line.trim().to_owned());
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").expect("answered");
        headers
    });

    let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build();
    let agent: ureq::Agent = config.into();
    let address = format!("http://127.0.0.1:{port}/qu/vy/quvyta-code");
    agent.get(&address).header("User-Agent", "quvyta-code/0.0.0").call().expect("the listener answers");

    let headers = server.join().expect("the listener ends");
    assert_eq!(headers[0], "GET /qu/vy/quvyta-code HTTP/1.1", "{headers:?}");
    let mut names: Vec<String> = headers[1..].iter().map(|line| line.to_ascii_lowercase()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "accept-encoding: gzip".to_owned(),
            "accept: */*".to_owned(),
            format!("host: 127.0.0.1:{port}"),
            "user-agent: quvyta-code/0.0.0".to_owned(),
        ],
        "the README's network section lists exactly these"
    );
}
