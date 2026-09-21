//! The base image every QCode container comes from, and the places inside it every other layer
//! agrees on.
//!
//! A workspace's plain shell runs in this image, a git clone runs in it, and every profile image
//! is `FROM` it with a harness installed on top. So it carries exactly what those need — a Node
//! runtime the harnesses install with, git, certificates, a shell — the built-in apps a file of
//! the workspace is opened with, and the directories the rest of QCode mounts onto.
//!
//! A profile can have its image built on another system than Debian; [`Os`] is that choice,
//! with a base image of its own for each system, and what each system cannot carry or run.
//!
//! Three things live here. [`paths`] is the contract: where the workspace, its material and the
//! home directory are, and what a container runs to stay up. [`apps`] is what the image carries
//! for opening files: which program opens which file, and with which words. [`ensure`] is the
//! one piece of work: build the image when the machine has not got it, or has an older one, and
//! say so line by line while it happens.

pub mod apps;
mod image;
#[cfg(test)]
pub(crate) mod live;
mod os;
pub mod paths;

pub use image::{
    CONTAINERFILE, Failure, Outcome, Presence, REVISION_LABEL, containerfile, containerfile_of, digest, ensure,
    ensure_os, presence, presence_of, revision, revision_of,
};
pub use os::{Gap, Os, Refusal};
