//! The seccomp profile a window's container runs under on docker, and how it reaches the engine.
//!
//! Podman needs none of this. Rootless podman already lets a process inside a container open an
//! unprivileged user namespace of its own, so a browser-engine application built its own sandbox
//! under podman's default profile untouched: its renderer processes came up in their own user, PID
//! and network namespaces, each with a filter of its own on top of the container's.
//!
//! Docker's default profile allows `unshare` and `setns` only to CAP_SYS_ADMIN and masks the
//! namespace flags out of `clone`. Under it the application cannot build the sandbox at all: it
//! prints that moving to a new namespace was not permitted and exits with 133. The two ways out
//! are giving the container no filter at all, which loses every other syscall rule with it, or
//! giving it the default profile plus those three calls. QCode carries the second, in
//! `assets/seccomp/desktop.json`, and hands it to docker by path. What it costs is stated in that
//! file's own comment: a process in the container can make a user namespace, so the host kernel's
//! unprivileged-user-namespace surface is open to it — which is what podman gives by default
//! anyway.
//!
//! `--no-sandbox` is not an option here, and the record's flags are tested for its absence. It
//! would let a page or an extension that takes over a renderer reach everything in the container
//! with the person's own rights: the project's files, the profile's home volume, the login.
//!
//! The file is carried in the binary and written out when it is first needed, named after a digest
//! of its own content, so a QCode that has been updated never hands the engine an older file left
//! in the temporary folder.

use std::io;
use std::path::PathBuf;

/// The profile, as it is carried: docker's own default with `clone`, `setns` and `unshare` allowed.
pub const PROFILE: &str = include_str!("../../assets/seccomp/desktop.json");

/// Writes the profile where the engine can read it and answers its path.
///
/// Writing it again over an existing one is fine and is what happens when two QCodes start
/// together: the content is the same, because the name is made from it.
///
/// # Errors
///
/// When the temporary folder cannot be written to.
pub fn file() -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("qcode-seccomp-{}.json", digest(PROFILE)));
    std::fs::write(&path, PROFILE)?;
    Ok(path)
}

/// FNV-1a over the bytes of `text`, as the base image's revision uses: the question is only
/// whether this is the same text, never whether someone made it collide.
fn digest(text: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let hash = text.bytes().fold(OFFSET, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME));
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_is_dockers_default_with_the_three_calls_a_sandbox_needs() {
        // Read as text rather than parsed: the point is that the file QCode carries still says
        // these things, and a parser here would only restate the engine's.
        assert!(PROFILE.contains("\"defaultAction\": \"SCMP_ACT_ERRNO\""), "the default must stay a refusal");
        assert!(PROFILE.contains("\"SCMP_ARCH_X86_64\"") && PROFILE.contains("\"SCMP_ARCH_AARCH64\""));
        let extra = PROFILE.rfind("\"unshare\"").expect("unshare is allowed");
        let last_allow = PROFILE[extra..].find("\"SCMP_ACT_ALLOW\"").expect("and allowed, not refused");
        assert!(last_allow < 400, "the allow belongs to the group that names it");
        for call in ["\"clone\"", "\"setns\"", "\"unshare\""] {
            assert!(PROFILE[extra - 200..].contains(call), "{call} is not in the group QCode adds");
        }
        // Never the whole filter thrown away, and never the application's own sandbox turned off.
        assert!(!PROFILE.contains("unconfined"), "{PROFILE}");
        assert!(PROFILE.contains("QCode adds these three"), "the file says why it is not the default");
    }

    #[test]
    fn the_file_is_named_after_its_own_content_so_an_old_one_is_never_handed_over() {
        let written = file().expect("the temporary folder is writable");
        assert_eq!(std::fs::read_to_string(&written).expect("it was written"), PROFILE);
        assert_eq!(written, file().expect("again"), "the same profile is the same file");
        let name = written.file_name().expect("a name").to_string_lossy().into_owned();
        assert!(name.starts_with("qcode-seccomp-") && name.ends_with(".json"), "{name}");
        assert_ne!(digest(PROFILE), digest(&format!("{PROFILE} ")), "a changed profile is a changed name");
        let _ = std::fs::remove_file(&written);
    }
}
