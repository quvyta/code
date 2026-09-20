//! The base image every QCode container comes from, and the places inside it every other layer
//! agrees on.
//!
//! A project's plain shell runs in this image, a git clone runs in it, and every profile image
//! is `FROM` it with a harness installed on top. So it carries exactly what those need — a Node
//! runtime the harnesses install with, git, certificates, a shell — the built-in apps a file of
//! the project is opened with, and the directories the rest of QCode mounts onto.
//!
//! Three things live here. [`paths`] is the contract: where the project, its material and the
//! home directory are, and what a container runs to stay up. [`apps`] is what the image carries
//! for opening files: which program opens which file, and with which words. [`ensure`] is the
//! one piece of work: build the image when the machine has not got it, or has an older one, and
//! say so line by line while it happens.

pub mod apps;
mod image;
#[cfg(test)]
mod live;
pub mod paths;

pub use image::{
    CONTAINERFILE, Failure, Outcome, Presence, REVISION_LABEL, containerfile, digest, ensure, presence, revision,
};
