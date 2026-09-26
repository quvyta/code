//! Folders of this machine that a container is handed for one piece of work: the folder a login
//! is taken out into, the folder a sign-in window leaves addresses in, the context an image is
//! built from.
//!
//! What passes through them can be the person's own: a login is a token that signs in to their
//! account. So a folder is the person's alone (0700), under the session's own runtime folder when
//! there is one, and made fresh under a name nobody could have guessed: another user of the
//! machine can neither read it nor put a folder of their own where QCode is about to write. It is
//! taken away when its owner is done with it.

use std::io;
use std::path::{Path, PathBuf};

/// How many names are tried before making a folder is given up on. Each is random, so a clash is
/// someone else's folder in the way, never bad luck; a few tries cover a clash and no more.
const TRIES: usize = 8;

/// A private folder, removed with everything in it when this is dropped.
#[derive(Debug, PartialEq, Eq)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Makes a new private folder for `purpose`, a word that ends up in its name so a person
    /// looking at the folder can tell what left it.
    ///
    /// # Errors
    ///
    /// When no folder can be made in either place.
    pub fn new(purpose: &str) -> io::Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        Self::in_session(runtime.as_deref(), purpose)
    }

    /// The same, with the session's runtime folder given: that folder when it is usable, since
    /// it is the person's own and gone when they log out, and this machine's temporary folder
    /// otherwise.
    fn in_session(runtime: Option<&Path>, purpose: &str) -> io::Result<Self> {
        if let Some(dir) = runtime.filter(|dir| dir.is_absolute())
            && let Ok(scratch) = Self::within(dir, purpose)
        {
            return Ok(scratch);
        }
        Self::within(&std::env::temp_dir(), purpose)
    }

    /// Makes a new private folder for `purpose` inside `parent`.
    ///
    /// # Errors
    ///
    /// When `parent` cannot be written to, or every name tried was taken.
    pub fn within(parent: &Path, purpose: &str) -> io::Result<Self> {
        let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
        for _ in 0..TRIES {
            let path = parent.join(format!("qcode-{purpose}-{}", random_word()));
            // Making the folder, never finding one: a path that is already there, whoever made
            // it, is not taken over but passed by.
            match make_private(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = error,
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }

    /// Where the folder is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A folder already taken away by its owner is what is wanted anyway.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Makes one folder, readable by its owner alone from the moment it exists, so there is no
/// instant in which it is open to others.
fn make_private(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// A word nobody else on the machine can know in advance. The standard library's hasher keys come
/// from the operating system's randomness, fresh for each `RandomState`, which is exactly that.
fn random_word() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    hasher.write_u128(now.as_nanos());
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::Scratch;
    use std::path::PathBuf;

    /// A parent folder of the test's own, so the checks never depend on what the session offers.
    fn parent(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = std::env::temp_dir().join(format!("qcode-scratch-test-{name}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a parent folder");
        dir
    }

    #[cfg(unix)]
    #[test]
    fn the_folder_is_its_owners_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = parent("mode");
        let scratch = Scratch::within(&dir, "login-mode").expect("a folder");
        let mode = std::fs::metadata(scratch.path()).expect("it is there").permissions().mode() & 0o777;
        drop(scratch);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn two_folders_for_the_same_work_have_different_names() {
        let dir = parent("names");
        let one = Scratch::within(&dir, "login-same").expect("a folder");
        let two = Scratch::within(&dir, "login-same").expect("a folder");
        let (one_path, two_path) = (one.path().to_owned(), two.path().to_owned());
        drop((one, two));
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(one_path, two_path);
        assert!(one_path.starts_with(&dir) && two_path.starts_with(&dir), "{}", one_path.display());
    }

    #[test]
    fn the_folder_and_what_is_in_it_go_when_its_owner_is_done() {
        let dir = parent("drop");
        let scratch = Scratch::within(&dir, "login-drop").expect("a folder");
        let path = scratch.path().to_owned();
        std::fs::create_dir_all(path.join("inner")).expect("something inside");
        std::fs::write(path.join("inner").join("token.json"), "{}").expect("a file inside");
        drop(scratch);
        let gone = !path.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(gone, "{} is still there", path.display());
    }

    #[test]
    fn a_folder_someone_made_ahead_of_time_is_never_taken_over() {
        let dir = parent("taken");
        // A folder where QCode is about to make one, made ahead of time the way another user
        // of the machine could.
        let planted = dir.join("qcode-login-planted");
        std::fs::create_dir_all(&planted).expect("a planted folder");
        std::fs::write(planted.join("theirs"), "").expect("a file of theirs");
        let made = super::make_private(&planted);
        let kept = planted.join("theirs").exists();
        let _ = std::fs::remove_dir_all(&dir);
        let error = made.expect_err("an existing folder is not made again");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(kept);
    }

    #[test]
    fn the_sessions_runtime_folder_is_used_when_it_can_be_and_the_temporary_folder_otherwise() {
        let dir = parent("runtime");
        let inside = Scratch::in_session(Some(&dir), "signin-runtime").expect("a folder");
        let missing = dir.join("gone");
        let elsewhere = Scratch::in_session(Some(&missing), "signin-runtime").expect("a folder");
        let relative = Scratch::in_session(Some(std::path::Path::new("run")), "signin-runtime").expect("a folder");
        let (inside_path, elsewhere_path, relative_path) =
            (inside.path().to_owned(), elsewhere.path().to_owned(), relative.path().to_owned());
        drop((inside, elsewhere, relative));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(inside_path.starts_with(&dir), "{}", inside_path.display());
        assert!(elsewhere_path.starts_with(std::env::temp_dir()), "{}", elsewhere_path.display());
        assert!(relative_path.starts_with(std::env::temp_dir()), "{}", relative_path.display());
    }
}
