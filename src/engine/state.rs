//! What the engine says about a container, read back from its output.
//!
//! Parsing is pure and never fails: an engine QCode has not met before may print a word this
//! table does not know, and the name of that word still reaches the person.

/// The state a container is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    /// Made, never started.
    Created,
    /// Up; a tab can `exec` into it.
    Running,
    /// Up but frozen.
    Paused,
    /// On its way back up.
    Restarting,
    /// On its way out.
    Removing,
    /// Was up, is not any more.
    Exited,
    /// The engine could not stop it cleanly.
    Dead,
    /// A word this version of QCode does not know, kept so it can still be shown.
    Unknown(String),
}

impl ContainerState {
    /// Reads one state word, as `container inspect --format {{.State.Status}}` and
    /// `ps --format {{.State}}` both print it.
    #[must_use]
    pub fn parse(word: &str) -> Self {
        match word.trim() {
            "created" | "configured" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "removing" => Self::Removing,
            "exited" | "stopped" => Self::Exited,
            "dead" => Self::Dead,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Whether a tab can `exec` into the container as it is.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// One container as [`Engine::list_containers`](super::Engine::list_containers) lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// Its name.
    pub name: String,
    /// What it is doing.
    pub state: ContainerState,
}

impl Container {
    /// Reads the output of [`Engine::list_containers`](super::Engine::list_containers): a name
    /// and a state per line, separated by a tab.
    ///
    /// A line without a tab is not a container QCode can act on, so it is left out rather than
    /// guessed at.
    #[must_use]
    pub fn parse_list(output: &str) -> Vec<Self> {
        output
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter(|(name, _)| !name.trim().is_empty())
            .map(|(name, state)| Self { name: name.trim().to_owned(), state: ContainerState::parse(state) })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Container, ContainerState};

    #[test]
    fn reads_the_state_words_both_engines_print() {
        assert_eq!(ContainerState::parse("running"), ContainerState::Running);
        assert_eq!(ContainerState::parse("exited\n"), ContainerState::Exited);
        assert_eq!(ContainerState::parse("configured"), ContainerState::Created);
        assert!(ContainerState::parse("running").is_running());
        assert!(!ContainerState::parse("paused").is_running());
    }

    #[test]
    fn an_unknown_word_survives_as_itself() {
        assert_eq!(ContainerState::parse("hibernating"), ContainerState::Unknown("hibernating".to_owned()));
    }

    #[test]
    fn reads_a_listing_and_skips_lines_that_are_not_containers() {
        let output = "qcode-p-base\trunning\nqcode-p-claude\texited\nEmergency stop\n\n";
        assert_eq!(
            Container::parse_list(output),
            [
                Container { name: "qcode-p-base".to_owned(), state: ContainerState::Running },
                Container { name: "qcode-p-claude".to_owned(), state: ContainerState::Exited },
            ]
        );
    }

    #[test]
    fn nothing_running_is_an_empty_list_and_not_a_mistake() {
        assert_eq!(Container::parse_list(""), []);
    }
}
