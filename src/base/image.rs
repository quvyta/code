//! The base image: what it is made of, how QCode knows whether the one on the machine is still
//! the one described here, and how it is built when it is not.

use std::path::PathBuf;

use super::Os;
use crate::engine::run::{EngineError, build_image, capture};
use crate::engine::{Engine, EngineCommand, ImageBuild};

/// The Containerfile the base image is built from, carried inside the binary so an installed
/// QCode needs no files beside it.
///
/// It is the file as written; [`containerfile`] is what is handed to the engine.
pub const CONTAINERFILE: &str = include_str!("../../assets/containerfiles/base.Containerfile");

/// The label the built image carries its revision in.
pub const REVISION_LABEL: &str = "qcode.base.revision";

/// What the machine has under the base image's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// Nothing is there, or nothing that can be asked.
    Missing,
    /// An image is there, built from an older description than this one.
    Stale,
    /// The image is there and is the one described here.
    Current,
}

/// What [`ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The image was already the current one and nothing ran.
    AlreadyThere,
    /// The image was built, which is the long one.
    Built,
}

/// Why the base image is not there.
#[derive(Debug)]
pub enum Failure {
    /// The build context could not be written on this machine.
    Host(std::io::Error),
    /// The engine could not be started, refused, or was stopped.
    Engine(EngineError),
}

/// The Containerfile as the engine is given it: the description plus the label that says which
/// description it was.
///
/// The label cannot be written into the file itself, because its value is what the file says.
#[must_use]
pub fn containerfile() -> String {
    containerfile_of(Os::Debian)
}

/// The Containerfile of `os`'s base image as the engine is given it, labelled the same way.
#[must_use]
pub fn containerfile_of(os: Os) -> String {
    format!("{}LABEL {REVISION_LABEL}=\"{}\"\n", os.containerfile(), revision_of(os))
}

/// Which description the image on the machine should have been built from.
///
/// A digest of the Containerfile rather than a version somebody remembers to raise: the question
/// it answers is "was the image built from this text?", and a number bumped by hand answers it
/// only as often as it is remembered. It guards against nothing but forgetfulness, which is why
/// a short non-cryptographic digest is enough.
#[must_use]
pub fn revision() -> String {
    revision_of(Os::Debian)
}

/// Which description `os`'s base image should have been built from: the digest of its own
/// Containerfile, so a change to one system's file rebuilds that system's image and no other.
#[must_use]
pub fn revision_of(os: Os) -> String {
    format!("{:016x}", digest(os.containerfile()))
}

/// FNV-1a over the bytes of `text`: a short digest that tells two descriptions apart, where
/// nothing depends on it being hard to forge.
#[must_use]
pub fn digest(text: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    text.bytes().fold(OFFSET, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME))
}

/// What the machine has under [`BASE_IMAGE`](crate::engine::names::BASE_IMAGE), Debian's base image.
///
/// Runs an engine command and waits for it, so it belongs on a background thread.
#[must_use]
pub fn presence(engine: &Engine) -> Presence {
    presence_of(engine, Os::Debian)
}

/// What the machine has under the name of `os`'s base image.
///
/// Runs an engine command and waits for it, so it belongs on a background thread.
#[must_use]
pub fn presence_of(engine: &Engine, os: Os) -> Presence {
    // The engine layer asks whether an image is there; this is the one place that also has to
    // know which description it was built from, and the label is where that is written.
    let asked = capture(&revision_query(engine, os));
    match asked {
        Err(_) => Presence::Missing,
        Ok(answer) if answer.trim() == revision_of(os) => Presence::Current,
        Ok(_) => Presence::Stale,
    }
}

/// Makes sure [`BASE_IMAGE`](crate::engine::names::BASE_IMAGE) is on the machine and is the one described here, building it when
/// it is not.
///
/// The build takes minutes and says what it is doing, so every line goes to `line` as it
/// arrives and `cancel` stops it; an image that is already current costs one command and no
/// output at all.
///
/// A build that fails or is stopped takes the name with it, half-made image and older image
/// alike, which is [`build_image`](crate::engine::run::build_image)'s rule and the right one
/// here: a profile image built on a half-made base would be missing pieces, and an older base
/// left under the name would go on being rebuilt from at every profile.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
///
/// # Errors
///
/// When the build context cannot be written, or the engine cannot be started, refuses, or is
/// stopped.
pub fn ensure(engine: &Engine, cancel: &dyn Fn() -> bool, line: &mut dyn FnMut(&str)) -> Result<Outcome, Failure> {
    ensure_os(engine, Os::Debian, cancel, line)
}

/// Makes sure `os`'s base image is on the machine and is the one described here, building it
/// when it is not, the way [`ensure`] does for Debian's.
///
/// Runs engine commands and waits for them, so it belongs on a background thread.
///
/// # Errors
///
/// When the build context cannot be written, or the engine cannot be started, refuses, or is
/// stopped.
pub fn ensure_os(
    engine: &Engine,
    os: Os,
    cancel: &dyn Fn() -> bool,
    line: &mut dyn FnMut(&str),
) -> Result<Outcome, Failure> {
    if presence_of(engine, os) == Presence::Current {
        return Ok(Outcome::AlreadyThere);
    }
    let context = context_dir();
    let _ = std::fs::remove_dir_all(&context);
    std::fs::create_dir_all(&context).map_err(Failure::Host)?;
    let written = context.join("Containerfile");
    let result = match std::fs::write(&written, containerfile_of(os)) {
        // Nothing is copied into the image from the context, so the context is the one file and
        // the directory that holds it. (Ubuntu's copies Node in from another image, which is
        // not the context.)
        Ok(()) => {
            let request = ImageBuild { image: os.image(), containerfile: &written, context: &context };
            build_image(engine, &request, cancel, line).map(|()| Outcome::Built).map_err(Failure::Engine)
        }
        Err(error) => Err(Failure::Host(error)),
    };
    let _ = std::fs::remove_dir_all(&context);
    result
}

/// The directory the build context is written into: this machine's temporary directory, named
/// after this process so two QCodes never write over each other.
fn context_dir() -> PathBuf {
    std::env::temp_dir().join(format!("qcode-base-{}", std::process::id()))
}

/// The command that asks the image which description it was built from.
fn revision_query(engine: &Engine, os: Os) -> EngineCommand {
    engine.image_label(os.image(), REVISION_LABEL)
}

#[cfg(test)]
mod tests {
    use super::{CONTAINERFILE, REVISION_LABEL, containerfile, digest, revision, revision_query};
    use crate::engine::names::BASE_IMAGE;
    use crate::engine::{Engine, EngineKind};
    use crate::profile::HarnessKind;

    #[test]
    fn the_file_handed_to_the_engine_is_the_description_plus_its_own_revision() {
        let built = containerfile();
        assert!(built.starts_with(CONTAINERFILE), "the description is handed over as it is written");
        assert!(built.ends_with(&format!("LABEL {REVISION_LABEL}=\"{}\"\n", revision())), "{built}");
        assert_eq!(built.lines().filter(|line| line.starts_with("FROM ")).count(), 1);
    }

    #[test]
    fn a_changed_description_is_a_changed_revision() {
        // This is the whole promise of the label: an image built before an edit must not pass
        // for one built after it.
        let before = digest(CONTAINERFILE);
        assert_ne!(before, digest(&format!("{CONTAINERFILE}RUN true\n")));
        assert_ne!(before, digest(&CONTAINERFILE.replace("/home/qcode", "/home/user")));
        assert_eq!(before, digest(CONTAINERFILE), "the same text is the same revision");
        assert_eq!(revision().len(), 16);
    }

    #[test]
    fn the_image_is_asked_for_the_label_of_the_name_the_design_settled_on() {
        let engine = Engine::new(EngineKind::Podman, "/usr/bin/podman");
        let command = revision_query(&engine, crate::base::Os::Debian);
        let args: Vec<String> = command.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            ["image", "inspect", "--format", "{{index .Config.Labels \"qcode.base.revision\"}}", "qcode/base"]
        );
        assert_eq!(BASE_IMAGE, "qcode/base");
        let arch = revision_query(&engine, crate::base::Os::Arch);
        assert_eq!(arch.args.last().map(|arg| arg.to_string_lossy().into_owned()).as_deref(), Some("qcode/base-arch"));
    }

    #[test]
    fn debians_revision_is_the_one_every_image_on_a_machine_was_labelled_with() {
        // The choice of a system must not make every machine rebuild the Debian image it has:
        // its revision is still the digest of the same file, and each system has its own.
        use crate::base::{Os, containerfile_of, revision_of};
        assert_eq!(revision_of(Os::Debian), revision());
        assert_eq!(revision(), format!("{:016x}", digest(CONTAINERFILE)), "the digest of the file alone, as before");
        assert_eq!(containerfile_of(Os::Debian), containerfile());
        let revisions: Vec<String> = Os::ALL.map(revision_of).to_vec();
        for (index, revision) in revisions.iter().enumerate() {
            assert!(!revisions[..index].contains(revision), "two systems share a revision");
        }
        for os in Os::ALL {
            assert!(containerfile_of(os).starts_with(os.containerfile()));
            assert!(containerfile_of(os).ends_with(&format!("LABEL {REVISION_LABEL}=\"{}\"\n", revision_of(os))));
        }
    }

    #[test]
    fn the_image_brings_what_every_harness_is_installed_with() {
        // The image carries Node and nothing else that installs software. A harness that needed
        // anything else would be installed into an image that cannot have it, so it is caught
        // here rather than in a build that fails on the person's machine.
        for harness in HarnessKind::ALL {
            for step in harness.record().install {
                assert!(
                    step.starts_with("npm "),
                    "{harness:?} is installed with `{step}`, which the image has no way to run"
                );
            }
        }
        assert!(CONTAINERFILE.contains("FROM docker.io/library/node:"), "the image brings no npm");
    }

    #[test]
    fn the_image_brings_what_a_harness_and_a_clone_need_beside_node() {
        // A workspace can be made by cloning, and everything a harness does goes over TLS.
        assert!(CONTAINERFILE.contains(" git"), "no git");
        assert!(CONTAINERFILE.contains("ca-certificates"), "no certificates");
    }
}
