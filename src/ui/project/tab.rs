//! One tab of the project screen: what it opens, where it stands, and the session it draws.

use std::time::{SystemTime, UNIX_EPOCH};

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
    /// A blank tab asking what it should open. It has no container and never starts one; the
    /// choice turns it into one of the others in place.
    New,
}

/// Where a tab stands.
///
/// Nothing here is a guess: a tab is `Running` only once a session is really attached to a
/// container, and every way of failing carries what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabState {
    /// Not started yet. A tab brought back from the last session waits like a background tab of
    /// a browser: it starts its container the first time it is shown, so bringing back a dozen
    /// tabs does not start a dozen containers at once. A blank tab waits for its choice.
    Waiting,
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
    /// When the tab was opened, in seconds since the Unix epoch.
    opened: u64,
    /// The harness conversation the tab shows, when it is known.
    conversation: Option<String>,
    session: Option<TerminalSession>,
    /// Counts the sessions this tab has started, so the watch of a session that was replaced by
    /// a restart is recognised and ignored.
    run: u64,
}

impl Tab {
    /// A tab of `kind` opened now, waiting for its container.
    #[must_use]
    pub fn new(key: TabKey, kind: TabKind) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        Self { key, kind, state: TabState::Starting, opened: now, conversation: None, session: None, run: 0 }
    }

    /// A blank tab opened now, waiting for the person to choose what it opens.
    #[must_use]
    pub fn blank(key: TabKey) -> Self {
        Self { state: TabState::Waiting, ..Self::new(key, TabKind::New) }
    }

    /// A tab of `kind` brought back from the last session, as it was recorded: opened at
    /// `opened` and showing `conversation`. It waits to be shown before it starts anything.
    #[must_use]
    pub fn restored(key: TabKey, kind: TabKind, opened: u64, conversation: Option<String>) -> Self {
        Self { key, kind, state: TabState::Waiting, opened, conversation, session: None, run: 0 }
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

    /// When the tab was opened, in seconds since the Unix epoch.
    #[must_use]
    pub fn opened(&self) -> u64 {
        self.opened
    }

    /// The harness conversation the tab shows, when it is known.
    #[must_use]
    pub fn conversation(&self) -> Option<&str> {
        self.conversation.as_deref()
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

    /// Turns a blank tab into a tab of `kind` showing `conversation`, opened now and waiting for
    /// its container. The key stays, so the tab keeps its place in the strip.
    pub fn choose(&mut self, kind: TabKind, conversation: Option<String>) {
        let key = self.key;
        *self = Self { conversation, ..Self::new(key, kind) };
    }

    /// Records that the tab shows `conversation`, once that is known, so the session file keeps
    /// it and the tab opens it again next time.
    pub fn show_conversation(&mut self, conversation: String) {
        self.conversation = Some(conversation);
    }

    /// Puts the tab back to waiting for its container and gives up the session it had, which
    /// ends the program still attached to it.
    pub fn restarting(&mut self) {
        self.close_session();
        self.state = TabState::Starting;
        self.run += 1;
    }

    /// Starts a tab that was waiting to be shown: it now waits for its container instead.
    pub fn wake(&mut self) {
        self.state = TabState::Starting;
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
