//! One tab of the project screen: what it opens, where it stands, and the session it draws.

use qframe::widgets::TerminalSession;

use super::plan::LaunchFailure;

/// A tab's identity, which stays the same while tabs are closed and dragged around it.
///
/// Work started for a tab comes back as a message long after the tab may have moved, so the
/// answer names the tab rather than its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabKey(pub u64);

/// What a tab opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKind {
    /// A shell in the project's own container, with no harness in it.
    Shell,
    /// A harness in the container of the profile of this name.
    Profile(String),
}

/// Where a tab stands.
///
/// Nothing here is a guess: a tab is `Running` only once a session is really attached to a
/// container, and every way of failing carries what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabState {
    /// The container is being created or started; nothing is attached yet.
    Starting,
    /// A session is attached to the running container.
    Running,
    /// The program inside the container ended, and the container is still up.
    Ended {
        /// Its exit code, or `None` when the engine did not report one.
        code: Option<u32>,
    },
    /// The container is not running any more, so the tab lost its ground rather than its
    /// program. This is the case the design asks to be named and offered a restart.
    Stopped,
    /// The engine refused, and said why.
    Failed(LaunchFailure),
}

impl TabState {
    /// Whether the tab is waiting for its container.
    #[must_use]
    pub fn is_starting(&self) -> bool {
        matches!(self, Self::Starting)
    }

    /// Whether the tab can be started again. A tab that is starting or running has nothing to
    /// restart, so its restart is never offered.
    #[must_use]
    pub fn can_restart(&self) -> bool {
        matches!(self, Self::Ended { .. } | Self::Stopped | Self::Failed(_))
    }
}

/// One tab: a container to enter, the state it is in and the session it draws while it runs.
#[derive(Debug)]
pub struct Tab {
    key: TabKey,
    kind: TabKind,
    state: TabState,
    session: Option<TerminalSession>,
    /// Counts the sessions this tab has started, so the watch of a session that was replaced by
    /// a restart is recognised and ignored.
    run: u64,
}

impl Tab {
    /// A tab of `kind`, waiting for its container.
    #[must_use]
    pub fn new(key: TabKey, kind: TabKind) -> Self {
        Self { key, kind, state: TabState::Starting, session: None, run: 0 }
    }

    /// The tab's identity.
    #[must_use]
    pub fn key(&self) -> TabKey {
        self.key
    }

    /// What the tab opens.
    #[must_use]
    pub fn kind(&self) -> &TabKind {
        &self.kind
    }

    /// Where the tab stands.
    #[must_use]
    pub fn state(&self) -> &TabState {
        &self.state
    }

    /// The session drawn in the middle, while there is one.
    #[must_use]
    pub fn session(&self) -> Option<&TerminalSession> {
        self.session.as_ref()
    }

    /// Which session of this tab is the current one.
    #[must_use]
    pub fn run(&self) -> u64 {
        self.run
    }

    /// Puts the tab back to waiting for its container and gives up the session it had, which
    /// ends the program still attached to it.
    pub fn restarting(&mut self) {
        self.close_session();
        self.state = TabState::Starting;
        self.run += 1;
    }

    /// Attaches `session` and marks the tab as running.
    pub fn attached(&mut self, session: TerminalSession) {
        self.session = Some(session);
        self.state = TabState::Running;
    }

    /// Moves the tab to `state`, keeping the session so its last screen stays readable: a tab
    /// that says the container stopped still shows what the harness printed before it did.
    pub fn settled(&mut self, state: TabState) {
        self.state = state;
    }

    /// Ends the tab's session, which is what closing a tab or restarting it does. The container
    /// itself is left alone: other tabs and other projects may be using it.
    pub fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.kill();
        }
    }
}
