//! The part of the base image no text can answer for: that the image really builds on both
//! engines, that a container of it really stays up, and that the person the container runs as
//! can really write where the contract says they can.
//!
//! Every test here is `#[ignore]`d and does nothing unless `QCODE_CONTAINER_TESTS=1`, so
//! `cargo test` stays clean on a machine with no engine. They reach the network the first time:
//! the Node image is pulled and one harness is installed from npm.
//!
//! ```text
//! QCODE_CONTAINER_TESTS=1 cargo test -- --ignored --test-threads=1
//! ```
//!
//! Everything a test makes is named `qcode-basetest-…` or `qcode/basetest-…` and is removed
//! again, so nothing on the machine that QCode itself made is touched. The one exception is
//! [`super::ensure`], whose whole subject is the real `qcode/base`: it builds that image and
//! leaves it, because it is the image the machine is meant to have and taking it away again
//! would cost the next profile build a quarter of an hour.

use std::path::{Path, PathBuf};

use super::apps::{PROGRAMS, office_text, pages, pdf_page, pdf_text, picture, sound_details};
use super::paths::{ASSETS_DIR, CODE_DIR, HOME_DIR, KEEP_ALIVE, OPEN_HOME};
use super::{Outcome, Presence, containerfile, ensure, presence};
use crate::engine::names::HOSTNAME;
use crate::engine::run::{build_image, capture};
use crate::engine::{
    Access, ContainerCreate, ContainerState, Engine, EngineKind, Exec, HostUser, ImageBuild, Mount, MountSource,
    Network, detect,
};
use crate::profile::HarnessKind;

/// The image these tests build for themselves, so the machine's own `qcode/base` is neither
/// read nor written by the tests that do not mean to.
const TEST_BASE: &str = "qcode/basetest-base";

/// The image a harness is installed into, standing in for a profile image.
const TEST_PROFILE: &str = "qcode/basetest-profile";

/// The container these tests live in.
const CONTAINER: &str = "qcode-basetest-container";

/// The volume standing in for a workspace's home.
const HOME_VOLUME: &str = "qcode-basetest-home";

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

/// A folder of this test's own, removed by `Drop`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("qcode-basetest-{name}-{stamp}"));
        std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
        Self(path)
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
        path
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let file = self.0.join(name);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("a folder in the temporary folder");
        }
        std::fs::write(&file, text).expect("a file in the temporary folder");
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Takes away whatever an earlier run left behind. The failures are the point of it being here:
/// there is usually nothing to remove.
fn clear(engine: &Engine) {
    let _ = capture(&engine.remove_container(CONTAINER));
    let _ = capture(&engine.remove_volume(HOME_VOLUME));
    let _ = capture(&engine.remove_image(TEST_PROFILE));
    let _ = capture(&engine.remove_image(TEST_BASE));
}

/// Builds `image` from `text` and says nothing unless it fails, in which case it says everything
/// the engine said.
fn build(engine: &Engine, image: &str, text: &str, scratch: &Scratch) {
    let containerfile = scratch.file(&format!("{}/Containerfile", image.replace('/', "-")), text);
    let context = containerfile.parent().expect("the file is in a folder").to_path_buf();
    let mut said = String::new();
    let built = build_image(
        engine,
        &ImageBuild { image, containerfile: &containerfile, context: &context },
        &|| false,
        &mut |line| {
            said.push_str(line);
            said.push('\n');
        },
    );
    assert!(built.is_ok(), "{image} on {:?} did not build:\n{said}", engine.kind());
    assert!(!said.is_empty(), "a build says what it is doing");
}

/// Runs `script` in the container with a shell, without a terminal, and answers with what it
/// printed. Fails with the shell's own words when it refuses.
fn run_in(engine: &Engine, script: &str) -> String {
    let command = ["sh", "-c", script];
    let asked = capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: &command }));
    match asked {
        Ok(output) => output,
        Err(error) => {
            let kind = engine.kind();
            panic!("`{script}` on {kind:?} failed: {error:?}")
        }
    }
}

/// Creates and starts the test container from `image`, with the mounts of the path contract.
fn start(engine: &Engine, image: &str, workspace: &Path, assets: &Path) {
    let mounts = [
        Mount { source: MountSource::Path(workspace), target: Path::new(CODE_DIR), access: Access::ReadWrite },
        Mount { source: MountSource::Path(assets), target: Path::new(ASSETS_DIR), access: Access::ReadOnly },
        Mount { source: MountSource::Volume(HOME_VOLUME), target: Path::new(HOME_DIR), access: Access::ReadWrite },
    ];
    capture(&engine.create_container(&ContainerCreate {
        name: CONTAINER,
        hostname: HOSTNAME,
        labels: &[],
        image,
        mounts: &mounts,
        network: Network::Full,
        // The whole point: the container runs as the person, and the image was built by someone
        // else entirely.
        user: HostUser::current().expect("the current user"),
        workdir: Some(Path::new(CODE_DIR)),
        command: KEEP_ALIVE,
    }))
    .expect("the container is made");
    capture(&engine.start_container(CONTAINER)).expect("the container starts");
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_image_builds_and_keeps_a_container_that_writes_where_the_contract_says() {
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("contract");
        build(&engine, TEST_BASE, &containerfile(), &scratch);

        let workspace = scratch.dir("Work");
        let assets = scratch.dir("Assets");
        start(&engine, TEST_BASE, &workspace, &assets);
        assert_eq!(
            ContainerState::parse(&capture(&engine.container_state(CONTAINER)).expect("a state")),
            ContainerState::Running,
            "{:?}: the keep-alive command holds the container up",
            engine.kind()
        );

        // The workspace reaches the host, and what it leaves there belongs to the person.
        run_in(&engine, &format!("echo from-the-container > {CODE_DIR}/marker"));
        let marker = workspace.join("marker");
        assert_eq!(
            std::fs::read_to_string(&marker).expect("the host sees the file").trim(),
            "from-the-container",
            "{:?}",
            engine.kind()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let HostUser::Ids { uid, .. } = HostUser::current().expect("the current user") else {
                panic!("a unix machine has ids")
            };
            assert_eq!(
                std::fs::metadata(&marker).expect("the file is there").uid(),
                uid,
                "{:?}: what the container writes belongs to the person, not to root",
                engine.kind()
            );
        }

        // The home directory is the one place a harness keeps anything, so the person must be
        // able to write into it and into a directory they make inside it.
        run_in(&engine, &format!("test -w {HOME_DIR}"));
        run_in(&engine, &format!("mkdir -p {HOME_DIR}/.qcode-probe && echo kept > {HOME_DIR}/.qcode-probe/file"));
        assert_eq!(run_in(&engine, &format!("cat {HOME_DIR}/.qcode-probe/file")).trim(), "kept");
        assert_eq!(run_in(&engine, "echo $HOME").trim(), HOME_DIR, "{:?}: the image names the home", engine.kind());

        // Assets are mounted read-only here, and a read-only mount that can be written to is a
        // permission the profile never gave.
        let output = capture(&engine.exec_without_terminal(&Exec {
            container: CONTAINER,
            command: &["sh", "-c", &format!("echo no > {ASSETS_DIR}/no")],
        }));
        assert!(output.is_err(), "{:?}: a read-only mount was written to", engine.kind());

        // Stopping is asked of the keep-alive command itself; one that ignores the request is
        // killed by the engine only after its ten-second timeout, which every stop would pay.
        let asked = std::time::Instant::now();
        capture(&engine.stop_container(CONTAINER)).expect("the container stops");
        let took = asked.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "{:?}: stopping took {took:?}", engine.kind());
        assert_eq!(
            ContainerState::parse(&capture(&engine.container_state(CONTAINER)).expect("a state")),
            ContainerState::Exited,
            "{:?}",
            engine.kind()
        );

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_repository_is_cloned_inside_the_container() {
    // A workspace can be made from a git address without the person having git, which only holds
    // if the image has it. The repository is made inside the container too, so the test needs
    // neither the network nor git on this machine.
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("clone");
        build(&engine, TEST_BASE, &containerfile(), &scratch);
        start(&engine, TEST_BASE, &scratch.dir("Work"), &scratch.dir("Assets"));

        run_in(
            &engine,
            &format!(
                "set -e\n\
                 mkdir -p $HOME/seed && cd $HOME/seed\n\
                 git init --quiet --initial-branch=main .\n\
                 echo merhaba > hello.txt\n\
                 git add hello.txt\n\
                 git -c user.email=qcode@example.invalid -c user.name=qcode commit --quiet -m seed\n\
                 cd {CODE_DIR}\n\
                 git clone --progress -- $HOME/seed cloned"
            ),
        );
        assert_eq!(run_in(&engine, &format!("cat {CODE_DIR}/cloned/hello.txt")).trim(), "merhaba");

        clear(&engine);
    }
}

/// A 4 × 2 picture in the PNG format, written by hand so the test needs no image crate: 8-bit
/// RGB rows, each after a filter byte of 0, in one stored deflate block, which a PNG decoder
/// reads like any other.
fn png() -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let (width, height) = (4_u32, 2_u32);
    let mut raw = Vec::new();
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            // A band of colour, so the drawing cannot come out blank.
            let shade = u8::try_from(x * 60 + y * 40).unwrap_or(u8::MAX);
            raw.extend_from_slice(&[200, shade, 255 - shade]);
        }
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    let length = u16::try_from(raw.len()).unwrap_or(0);
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_built_in_apps_answer_and_a_picture_is_drawn() {
    // A file opened from the file tree runs one of these in the workspace's container. A program
    // the image does not really carry would fail only when the person opens a file, so every one
    // is asked here, and a picture is drawn with the very words a tab uses.
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("apps");
        build(&engine, TEST_BASE, &containerfile(), &scratch);
        let workspace = scratch.dir("Work");
        std::fs::write(workspace.join("band of colour.png"), png()).expect("the picture is written");
        start(&engine, TEST_BASE, &workspace, &scratch.dir("Assets"));

        for program in PROGRAMS {
            let answer = capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: program }));
            assert!(answer.is_ok(), "{:?}: `{}` does not answer: {answer:?}", engine.kind(), program.join(" "));
        }

        let command = picture(&format!("{CODE_DIR}/band of colour.png"));
        let parts: Vec<&str> = command.iter().map(String::as_str).collect();
        let drawn = capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: &parts }))
            .unwrap_or_else(|error| panic!("{:?}: the picture is not drawn: {error:?}", engine.kind()));
        // Without a terminal chafa falls back to a view of its own; what matters is that it read
        // the file and drew it in coloured cells.
        assert!(drawn.contains("\u{1b}[38;2;"), "{:?}: no full colour in {drawn:?}", engine.kind());
        assert!(drawn.lines().count() > 1, "{:?}: {drawn:?}", engine.kind());

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_harness_installs_on_top_of_the_image_and_answers() {
    // The base image exists to be built on. This builds what a profile image is — the harness
    // installed with its own command, the configuration of the `recommended` template copied
    // into the home directory — and then checks the two things that make it usable: the harness
    // runs, and the person the container runs as can write beside the settings the build wrote,
    // which is where the login goes.
    let harness = HarnessKind::ClaudeCode;
    let record = harness.record();
    let settings = record.settings.expect("Claude Code has a settings file");
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("harness");
        build(&engine, TEST_BASE, &containerfile(), &scratch);

        let mut lines = vec![format!("FROM {TEST_BASE}")];
        lines.extend(record.image_steps());
        // The same shape `ui::profiles::recipe` writes: the file is staged and moved into the
        // home directory by the image's own shell, so the recipe never names that directory.
        lines.push(format!(
            "RUN mkdir -p \"$HOME/$(dirname '{path}')\" && printf '%s' '{contents}' > \"$HOME/{path}\"",
            path = settings.path,
            contents = settings.contents.replace('\n', " ")
        ));
        lines.push(format!("RUN {OPEN_HOME}"));
        lines.push(String::new());
        build(&engine, TEST_PROFILE, &lines.join("\n"), &scratch);

        start(&engine, TEST_PROFILE, &scratch.dir("Work"), &scratch.dir("Assets"));
        let version = capture(
            &engine.exec_without_terminal(&Exec { container: CONTAINER, command: &[record.command, "--version"] }),
        )
        .unwrap_or_else(|error| panic!("{} does not answer on {:?}: {error:?}", record.command, engine.kind()));
        assert!(version.trim().starts_with(|c: char| c.is_ascii_digit()), "{:?}: `{version}`", engine.kind());

        // The login goes beside the settings the build wrote, in a directory the build made.
        let login = record.identity.first().expect("the harness says where its login lives");
        run_in(&engine, &format!("printf '%s' '{{}}' > \"$HOME/{login}\""));
        assert_eq!(run_in(&engine, &format!("cat \"$HOME/{login}\"")).trim(), "{}");
        // And the settings themselves can still be changed by whoever runs.
        run_in(&engine, &format!("test -w \"$HOME/{}\"", settings.path));

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn the_image_is_built_once_and_then_left_alone() {
    // This one works on the machine's real `qcode/base`, because that is what `ensure` is for,
    // and leaves it behind on purpose: it is the image the machine is meant to have.
    for engine in engines() {
        let mut lines = 0_usize;
        let first = ensure(&engine, &|| false, &mut |_| lines += 1)
            .unwrap_or_else(|error| panic!("the base image does not build on {:?}: {error:?}", engine.kind()));
        if first == Outcome::Built {
            assert!(lines > 0, "{:?}: a build says what it is doing", engine.kind());
        }
        assert_eq!(presence(&engine), Presence::Current, "{:?}", engine.kind());

        let mut again = 0_usize;
        let second = ensure(&engine, &|| false, &mut |_| again += 1).expect("the image is already there");
        assert_eq!(second, Outcome::AlreadyThere, "{:?}: an image that is current is not built again", engine.kind());
        assert_eq!(again, 0, "{:?}: and nothing runs to find that out", engine.kind());
    }
}

/// A two-page PDF with a line of text on each page, written by hand so the test needs no PDF
/// library: the objects one after another, then the table of where each one starts.
fn pdf() -> Vec<u8> {
    let page = |text: &str, contents: usize| {
        let stream = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
        [
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 7 0 R >> >> /Contents {contents} 0 R >>"
            ),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        ]
    };
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_owned(),
    ];
    objects.extend(page("Harbour notes", 4));
    objects.extend(page("Second page", 6));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned());
    let mut out = String::from("%PDF-1.4\n");
    let mut starts = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        starts.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let table = out.len();
    out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for start in starts {
        out.push_str(&format!("{start:010} 00000 n \n"));
    }
    out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n", objects.len() + 1));
    out.into_bytes()
}

/// Runs `command` in the test container without a terminal, the way a tab reads a file's text.
fn read_with(engine: &Engine, command: &[String]) -> String {
    let parts: Vec<&str> = command.iter().map(String::as_str).collect();
    capture(&engine.exec_without_terminal(&Exec { container: CONTAINER, command: &parts }))
        .unwrap_or_else(|error| panic!("{:?}: `{}` failed: {error:?}", engine.kind(), parts.join(" ")))
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_pdf_gives_its_text_and_draws_its_pages() {
    // The words a PDF tab runs, run for real on a real PDF: its text comes out with a page
    // break after each page, and a page is drawn in coloured cells.
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("pdf");
        build(&engine, TEST_BASE, &containerfile(), &scratch);
        let workspace = scratch.dir("Work");
        std::fs::write(workspace.join("tide tables.pdf"), pdf()).expect("the PDF is written");
        start(&engine, TEST_BASE, &workspace, &scratch.dir("Assets"));
        let path = format!("{CODE_DIR}/tide tables.pdf");

        let text = read_with(&engine, &pdf_text(&path));
        assert!(text.contains("Harbour notes") && text.contains("Second page"), "{:?}: {text:?}", engine.kind());
        assert_eq!(pages(&text), 2, "{:?}: {text:?}", engine.kind());

        let drawn = read_with(&engine, &pdf_page(&path, 2));
        assert!(drawn.contains("\u{1b}["), "{:?}: the page is not drawn: {drawn:?}", engine.kind());
        assert!(drawn.lines().count() > 1, "{:?}: {drawn:?}", engine.kind());

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_word_document_and_an_opendocument_text_give_their_text() {
    // The smallest files each program reads, zipped in the container with the image's own zip,
    // so the test carries no office file and needs no zip library.
    let make = format!(
        "set -e\n\
         cd \"$(mktemp -d)\" && mkdir -p word && \
         printf '%s' '<?xml version=\"1.0\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>' \
           > '[Content_Types].xml' && \
         printf '%s' '<?xml version=\"1.0\"?><w:document \
           xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r>\
           <w:t>Dear harbour master, şimdi</w:t></w:r></w:p></w:body></w:document>' > word/document.xml && \
         zip -q -r '{CODE_DIR}/to the harbour master.docx' .\n\
         cd \"$(mktemp -d)\" && printf '%s' 'application/vnd.oasis.opendocument.text' > mimetype && \
         printf '%s' '<?xml version=\"1.0\"?><office:document-content \
           xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
           xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\"><office:body><office:text>\
           <text:p>The tide turns at six, ğ</text:p></office:text></office:body></office:document-content>' \
           > content.xml && \
         zip -q -X -0 '{CODE_DIR}/Tide.odt' mimetype && zip -q -X '{CODE_DIR}/Tide.odt' content.xml"
    );
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("office");
        build(&engine, TEST_BASE, &containerfile(), &scratch);
        start(&engine, TEST_BASE, &scratch.dir("Work"), &scratch.dir("Assets"));
        run_in(&engine, &make);

        let docx = office_text(&format!("{CODE_DIR}/to the harbour master.docx")).expect("a docx is read");
        let text = read_with(&engine, &docx);
        assert!(text.contains("Dear harbour master, şimdi"), "{:?}: {text:?}", engine.kind());
        let odt = office_text(&format!("{CODE_DIR}/Tide.odt")).expect("an odt is read");
        let text = read_with(&engine, &odt);
        assert!(text.contains("The tide turns at six, ğ"), "{:?}: {text:?}", engine.kind());

        clear(&engine);
    }
}

#[test]
#[ignore = "needs a container engine; run with QCODE_CONTAINER_TESTS=1"]
fn a_sound_is_described_and_every_kind_a_tab_plays_is_one_sox_reads() {
    // Nothing is played here: a sound is made in the container, then described with the very
    // words a tab uses when it does not play. That sox can play each kind a tab offers, and send
    // it to a sound server, is asked of sox's own list of what it reads and writes.
    for engine in engines() {
        clear(&engine);
        let scratch = Scratch::new("sound");
        build(&engine, TEST_BASE, &containerfile(), &scratch);
        start(&engine, TEST_BASE, &scratch.dir("Work"), &scratch.dir("Assets"));
        run_in(&engine, &format!("sox -n -r 8000 -c 1 '{CODE_DIR}/fog horn.wav' synth 1.5 sine 440"));
        // Ogg audio often comes as .oga, a name sox has no handler for; it knows the file by
        // what is in it instead.
        run_in(&engine, &format!("cd {CODE_DIR} && sox 'fog horn.wav' tide.ogg && cp tide.ogg tide.oga"));

        let details = read_with(&engine, &sound_details(&format!("{CODE_DIR}/fog horn.wav")));
        assert!(details.contains("00:00:01.50"), "{:?}: {details:?}", engine.kind());
        assert!(details.contains("Sample Rate    : 8000"), "{:?}: {details:?}", engine.kind());
        let details = read_with(&engine, &sound_details(&format!("{CODE_DIR}/tide.oga")));
        assert!(details.contains("Vorbis") && details.contains("00:00:01.50"), "{:?}: {details:?}", engine.kind());

        let help = run_in(&engine, "sox -h");
        let listed = |heading: &str| -> Vec<String> {
            let after = help.split(heading).nth(1).unwrap_or_default();
            after.lines().next().unwrap_or_default().split_whitespace().map(str::to_owned).collect()
        };
        let formats = listed("AUDIO FILE FORMATS:");
        for kind in ["mp3", "ogg", "opus", "flac", "wav"] {
            assert!(formats.iter().any(|known| known == kind), "{:?}: sox reads no {kind}: {formats:?}", engine.kind());
        }
        let devices = listed("AUDIO DEVICE DRIVERS:");
        assert!(devices.iter().any(|known| known == "pulseaudio"), "{:?}: {devices:?}", engine.kind());

        clear(&engine);
    }
}
