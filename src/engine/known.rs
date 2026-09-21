//! Engine refusals QCode can put into a plain sentence.
//!
//! An engine that refuses says why in its own words, and those words are written for someone who
//! already knows containers: "short-name did not resolve to an alias", "permission denied while
//! trying to connect to the docker API". A handful of these refusals come up again and again, and
//! each of them has one thing the person can do about it. Those are recognised here, so a screen
//! can say that thing first and keep the engine's words underneath as the detail.
//!
//! The matching is loose on purpose, the way [`detect`](super::detect) reads a failed `info`: a
//! release that rewords its sentence costs the person the plain sentence, never a wrong one,
//! because anything not recognised is left as the engine's own words.

/// A refusal QCode recognises, each asking one thing of the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Known {
    /// The image a container was to be made from is not in this engine. Mostly an image built
    /// with the other engine, or one removed by hand.
    ImageMissing,
    /// The engine's service is not running: Docker's daemon, or the socket or machine podman is
    /// pointed at.
    NotRunning,
    /// The engine runs and this account may not use it: Docker's socket belongs to the `docker`
    /// group and the person is not in it yet.
    NoPermission,
    /// Podman runs without root and this account has no ranges of user and group ids of its own
    /// in `/etc/subuid` and `/etc/subgid`, which every image with more than one user needs.
    NoIdRanges,
}

/// What Docker says when its daemon is not answering, in the releases QCode has met: 29 and
/// earlier on Linux, and Docker Desktop on Windows.
pub(super) const DOCKER_DOWN: &[&str] = &[
    "cannot connect to the docker daemon",
    "failed to connect to the docker api",
    "is the docker daemon running",
    "error during connect",
    "docker_engine",
    "dockerdesktoplinuxengine",
];

/// What podman says when the service or the virtual machine it is pointed at is not there.
pub(super) const PODMAN_DOWN: &[&str] =
    &["cannot connect to podman", "unable to connect to podman socket", "podman machine start", "podman machine init"];

/// Docker's words for a socket this account may not open, measured against docker 29 with a
/// socket nobody may open: "permission denied while trying to connect to the docker API at
/// unix:///…". Earlier releases said "Got permission denied while trying to connect to the
/// Docker daemon socket".
const NO_PERMISSION: &[&str] = &[
    "permission denied while trying to connect to the docker api",
    "permission denied while trying to connect to the docker daemon",
];

/// Podman's words when an account has no id ranges: containers/storage's "no subuid ranges found
/// for user", podman's "cannot find UID/GID for user … check rootless mode", and the warning an
/// image with files of several owners ends in, which names the two files itself.
const NO_ID_RANGES: &[&str] = &[
    "no subuid ranges found",
    "no subgid ranges found",
    "cannot find uid/gid for user",
    "insufficient uids or gids available in user namespace",
    "check /etc/subuid and /etc/subgid",
];

/// The engines' words for an image they do not have, measured on podman 6.1 and docker 29 for a
/// name nobody built: podman's "image not known" (with `--pull=never`) and its short-name
/// sentence (without, and with no registries configured); docker's "No such image" (with
/// `--pull=never`) and, without, its answer from Docker Hub: "pull access denied … repository
/// does not exist".
const IMAGE_MISSING: &[&str] = &[
    "image not known",
    "did not resolve to an alias",
    "no such image",
    "pull access denied",
    "repository does not exist",
];

/// Which known refusal `output` is, if it is one.
///
/// The engine's own trouble is asked about before the image: an engine that cannot be reached
/// cannot have been asked whether it has an image either, so its words are about itself.
#[must_use]
pub fn recognise(output: &str) -> Option<Known> {
    let said = output.to_lowercase();
    let says = |marks: &[&str]| marks.iter().any(|mark| said.contains(mark));
    if says(NO_PERMISSION) {
        Some(Known::NoPermission)
    } else if says(DOCKER_DOWN) || says(PODMAN_DOWN) {
        Some(Known::NotRunning)
    } else if says(NO_ID_RANGES) {
        Some(Known::NoIdRanges)
    } else if says(IMAGE_MISSING) {
        Some(Known::ImageMissing)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Known, recognise};

    #[test]
    fn a_missing_image_is_recognised_in_either_engines_words() {
        for said in [
            // What the owner met: the image built with docker, the engine switched to podman.
            "Error: short-name \"qcode/profile/claude-code\" did not resolve to an alias and no \
             containers-registries.conf(5) was found",
            "Error: qcode/profile/motortest-absent: image not known",
            "Error response from daemon: No such image: qcode/profile/motortest-absent:latest",
            "Unable to find image 'qcode/profile/x:latest' locally\nError response from daemon: pull access \
             denied for qcode/profile/x, repository does not exist or may require 'docker login'",
        ] {
            assert_eq!(recognise(said), Some(Known::ImageMissing), "{said}");
        }
    }

    #[test]
    fn an_engine_that_is_not_running_is_recognised_before_anything_it_could_not_find() {
        for said in [
            "failed to connect to the docker API at unix:///run/docker.sock; check if the path is correct and if \
             the daemon is running: dial unix /run/docker.sock: connect: no such file or directory",
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
            "Error: unable to connect to Podman socket: Get \"http://d/v6.1.1/libpod/_ping\": dial unix \
             /run/user/1000/podman/podman.sock: connect: no such file or directory",
        ] {
            assert_eq!(recognise(said), Some(Known::NotRunning), "{said}");
        }
    }

    #[test]
    fn a_socket_this_account_may_not_open_is_a_matter_of_the_docker_group() {
        let said = "permission denied while trying to connect to the docker API at unix:///var/run/docker.sock";
        assert_eq!(recognise(said), Some(Known::NoPermission));
        let older = "Got permission denied while trying to connect to the Docker daemon socket at \
                     unix:///var/run/docker.sock: Post \"http://%2Fvar%2Frun%2Fdocker.sock/v1.24/containers/create\"";
        assert_eq!(recognise(older), Some(Known::NoPermission));
    }

    #[test]
    fn an_account_without_id_ranges_is_recognised() {
        for said in [
            "Error: cannot find UID/GID for user hakan: no subuid ranges found for user \"hakan\" in /etc/subuid - \
             check rootless mode in man pages.",
            "Error: writing blob: adding layer with blob \"sha256:…\": processing tar file(potentially insufficient \
             UIDs or GIDs available in user namespace (requested 0:42 for /etc/shadow): Check /etc/subuid and \
             /etc/subgid if configured locally and run \"podman system migrate\": lchown /etc/shadow: invalid \
             argument): exit status 1",
        ] {
            assert_eq!(recognise(said), Some(Known::NoIdRanges), "{said}");
        }
    }

    #[test]
    fn anything_else_keeps_the_engines_own_words() {
        for said in ["Error: no space left on device", "exit status 1", ""] {
            assert_eq!(recognise(said), None, "{said}");
        }
    }
}
