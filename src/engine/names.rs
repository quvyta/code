//! What QCode calls the images, containers and volumes it makes.
//!
//! The names are a contract with the engine, not a display: the same project and profile always
//! lead back to the same container, so QCode finds its own work again after a restart. They are
//! built in one place so that nothing has to spell them out a second time.
//!
//! Every part that goes into a name comes from a [`SafeName`](crate::profile::SafeName) or a
//! [`ProjectId`](crate::workspace::ProjectId), both of which hold only characters podman and
//! docker accept; the tests below prove that what those two can produce is a name the engine
//! takes, so nothing here has to escape anything.

/// The image every profile image is built on.
pub const BASE_IMAGE: &str = "qcode/base";

/// The machine name every container QCode creates answers to.
///
/// It is one name for all of them, never the engine's random one, because a harness may bake
/// the machine name into its login. Gemini CLI does: its installed package
/// (`packages/core/dist/src/services/fileKeychain.js`) encrypts `gemini-credentials.json` with
/// `deriveEncryptionKey() { const salt = `${os.hostname()}-${os.userInfo().username}-gemini-cli`;
/// return crypto.scryptSync("gemini-cli-oauth", salt, 32); }`. A login made in the sign-in
/// container is copied into every project's home, and it only decrypts there when the sign-in
/// container, the courier and the project container all report the same machine name.
///
/// The other half of that salt, the user name, is already the same everywhere: the image's
/// user is `qcode` at uid 1000 ([`crate::base::paths::USER`]), and podman's `--userns=keep-id`
/// maps the person onto it. Docker's `--user <uid>:<gid>` path does not: a person whose uid is
/// not 1000 has no passwd entry in the container, so `os.userInfo()` throws there and the
/// login is not written at all. That is a separate gap, not closed by this name.
pub const HOSTNAME: &str = "qcode";

/// The image a profile is installed into.
#[must_use]
pub fn profile_image(profile: &str) -> String {
    format!("qcode/profile/{profile}")
}

/// The container a project's profile lives in.
#[must_use]
pub fn profile_container(project: &str, profile: &str) -> String {
    format!("qcode-{project}-{profile}")
}

/// The container a project's plain shell lives in.
#[must_use]
pub fn base_container(project: &str) -> String {
    format!("qcode-{project}-base")
}

/// The container a sound of a project is played in while the tab `tab` plays it.
///
/// The dot is what no project id and no profile name can hold, so this name can never be the
/// container of a profile, whatever the profile is called.
#[must_use]
pub fn sound_container(project: &str, tab: u64) -> String {
    format!("qcode-{project}.play-{tab}")
}

/// The container the window of a project's desktop profile is open in.
///
/// One per project and profile, not one per tab: the application is single-instance for a home
/// directory, so a second container on the same home volume would only tell the first to show
/// itself. The dot is what no project id and no profile name can hold, so this can never be the
/// container the same profile's command-line work would live in.
#[must_use]
pub fn desktop_container(project: &str, profile: &str) -> String {
    format!("qcode-{project}-{profile}.desk")
}

/// The volume holding a profile's login, shared by every project that uses the profile.
#[must_use]
pub fn credential_volume(profile: &str) -> String {
    format!("qcode-cred-{profile}")
}

/// The volume holding one project's copy of a profile's home: its history, memory and settings,
/// which stay inside that project.
#[must_use]
pub fn home_volume(project: &str, profile: &str) -> String {
    format!("qcode-home-{project}-{profile}")
}

#[cfg(test)]
mod tests {
    use super::{
        BASE_IMAGE, HOSTNAME, base_container, credential_volume, desktop_container, home_volume, profile_container,
        profile_image, sound_container,
    };
    use crate::profile::SafeName;

    /// What podman and docker accept as a container or volume name:
    /// `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
    fn is_object_name(name: &str) -> bool {
        let mut chars = name.chars();
        chars.next().is_some_and(|first| first.is_ascii_alphanumeric())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    }

    /// What podman and docker accept as an image name: path components of
    /// `[a-z0-9]+([._-][a-z0-9]+)*` separated by `/`.
    fn is_image_name(name: &str) -> bool {
        name.split('/').all(|component| {
            !component.is_empty()
                && component.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && component.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && component
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
                && !component.contains("--")
        })
    }

    fn safe(text: &str) -> SafeName {
        SafeName::from_display(text).expect("the text has usable characters")
    }

    #[test]
    fn turkish_project_and_profile_names_still_give_valid_object_names() {
        let project = safe("İstanbul Şubesi");
        let profile = safe("Günlük Çalışma");
        assert!(is_image_name(&profile_image(profile.as_str())), "{}", profile_image(profile.as_str()));
        assert!(is_image_name(BASE_IMAGE));
        for object in [
            profile_container(project.as_str(), profile.as_str()),
            base_container(project.as_str()),
            credential_volume(profile.as_str()),
            home_volume(project.as_str(), profile.as_str()),
        ] {
            assert!(is_object_name(&object), "{object}");
        }
    }

    #[test]
    fn every_name_a_safe_name_can_produce_is_accepted_by_the_engine() {
        for text in ["a", "9lives", "a-b-c", &"ab ".repeat(50), "v2.0/build", "IŞIK", "_x_"] {
            let single = safe(text);
            let name = single.as_str();
            assert!(is_image_name(&profile_image(name)), "{text:?}");
            assert!(is_object_name(&profile_container(name, name)), "{text:?}");
            assert!(is_object_name(&home_volume(name, name)), "{text:?}");
            assert!(is_object_name(&credential_volume(name)), "{text:?}");
            assert!(is_object_name(&base_container(name)), "{text:?}");
            assert!(is_object_name(&sound_container(name, 7)), "{text:?}");
            assert!(is_object_name(&desktop_container(name, name)), "{text:?}");
            // No profile, whatever its name, has the container a sound plays in or a window opens
            // in: both are told apart by a dot, which a safe name never holds.
            assert!(!profile_container(name, name).contains('.'), "{text:?}");
            assert_ne!(desktop_container(name, name), profile_container(name, name), "{text:?}");
        }
    }

    #[test]
    fn the_machine_name_is_one_the_engines_and_a_resolver_take() {
        // RFC 1123 host label: letters, digits and dashes, neither at the ends.
        assert!(is_object_name(HOSTNAME) && !HOSTNAME.contains(['_', '.']) && !HOSTNAME.ends_with('-'));
    }

    #[test]
    fn the_names_are_the_ones_the_design_settled_on() {
        assert_eq!(BASE_IMAGE, "qcode/base");
        assert_eq!(HOSTNAME, "qcode");
        assert_eq!(profile_image("claude-sub"), "qcode/profile/claude-sub");
        assert_eq!(profile_container("my-app", "claude-sub"), "qcode-my-app-claude-sub");
        assert_eq!(base_container("my-app"), "qcode-my-app-base");
        assert_eq!(credential_volume("claude-sub"), "qcode-cred-claude-sub");
        assert_eq!(home_volume("my-app", "claude-sub"), "qcode-home-my-app-claude-sub");
        assert_eq!(sound_container("my-app", 3), "qcode-my-app.play-3");
        assert_eq!(desktop_container("my-app", "antigravity"), "qcode-my-app-antigravity.desk");
    }
}
