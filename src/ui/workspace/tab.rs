//! One tab of the workspace screen: what it opens, where it stands, and the session it draws.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use qframe::widgets::TerminalSession;

use crate::base::apps::Quiet;
use crate::bridge;

use super::bridge::Letter;
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
    /// A shell in the workspace's own container, with no harness in it.
    Shell,
    /// A harness in the container of the profile of this name.
    Profile(String),
    /// A blank tab asking what it should open. It has no container and never starts one; the
    /// choice turns it into one of the others in place.
    New,
    /// A picture of the workspace, drawn by chafa in the workspace's own container. The text is the
    /// file's key in the file tree: its path inside `Work/`, written with `/`.
    Image(String),
    /// A Markdown document of the workspace, read from the disk and shown by QCode itself, with no
    /// container at all.
    Markdown(String),
    /// A file of the workspace, open in the chosen editor in the workspace's own container.
    Editor(String),
    /// A PDF of the workspace: its text, taken out in the workspace's own container and shown by
    /// QCode, or one of its pages drawn there as a picture.
    Pdf(String),
    /// A word processor's document of the workspace: its text, taken out in the workspace's own
    /// container and shown by QCode.
    Office(String),
    /// A sound of the workspace: played in a container of its own made for it, or, when it
    /// cannot or should not play, described.
    Sound(String),
    /// A window of the desktop harness of the profile of this name, open on the person's own
    /// screen from a container of its own. The tab has no terminal: it says where the window
    /// stands and offers the two things that can be done to it.
    Desktop(String),
}

impl TabKind {
    /// The key of the workspace's file the tab opens, when it opens one.
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        match self {
            Self::Image(file)
            | Self::Markdown(file)
            | Self::Editor(file)
            | Self::Pdf(file)
            | Self::Office(file)
            | Self::Sound(file) => Some(file),
            Self::Shell | Self::Profile(_) | Self::New | Self::Desktop(_) => None,
        }
    }

    /// The profile the tab belongs to, when it belongs to one: a harness in a terminal or a
    /// window on the screen.
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        match self {
            Self::Profile(name) | Self::Desktop(name) => Some(name),
            Self::Shell
            | Self::New
            | Self::Image(_)
            | Self::Markdown(_)
            | Self::Editor(_)
            | Self::Pdf(_)
            | Self::Office(_)
            | Self::Sound(_) => None,
        }
    }
}

/// Where a PDF tab stands among its pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pages {
    /// How many pages the document has, as its text counted them.
    pub count: usize,
    /// The page drawn, or the one that is drawn next; counted from 1.
    pub page: usize,
    /// Whether the tab draws the page rather than showing the text.
    pub drawn: bool,
}

impl Default for Pages {
    fn default() -> Self {
        Self { count: 1, page: 1, drawn: false }
    }
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
    /// A session is attached to the running container; for a Markdown tab, which has neither,
    /// the document is read and shown.
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
    /// The file the tab opens is not in the workspace any more.
    Missing,
    /// The file the tab opens could not be read, for this reason.
    Unreadable(String),
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
        matches!(self, Self::Ended { .. } | Self::Stopped | Self::Failed(_) | Self::Missing | Self::Unreadable(_))
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
    /// The text of a Markdown tab's file, as it was last read, or the text taken out of a PDF.
    document: Option<String>,
    /// Whether that text was cut short, being too long to show whole.
    partial: bool,
    /// Where a PDF tab stands among its pages.
    pages: Pages,
    /// Why a sound tab shows the sound's details rather than playing it, once it is known.
    quiet: Option<Quiet>,
    /// The sound server's socket on this machine, once a sound tab found one to play through.
    socket: Option<PathBuf>,
    /// Whether a sound tab waits to be asked before it plays: a tab brought back from the last
    /// session does, because opening QCode again is not asking to hear the sound again.
    held: bool,
    /// The web address the window asked to have opened, and whether QCode managed to open it in
    /// the person's browser — `None` while the opening is still on its way. A window tab only.
    sign_in: Option<(String, Option<bool>)>,
    /// Counts the sessions this tab has started, so the watch of a session that was replaced by
    /// a restart is recognised and ignored.
    run: u64,
    /// What a harness tab's agent hands QCode's bridge to say which tab it is in.
    token: String,
    /// Messages other tabs' agents sent this tab that wait in it, oldest first.
    letters: Vec<Letter>,
    /// Whether the waiting letters are shown.
    letters_shown: bool,
}

impl Tab {
    /// A tab of `kind` opened now, waiting for its container.
    #[must_use]
    pub fn new(key: TabKey, kind: TabKind) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        Self {
            key,
            kind,
            state: TabState::Starting,
            opened: now,
            conversation: None,
            session: None,
            document: None,
            partial: false,
            pages: Pages::default(),
            quiet: None,
            sign_in: None,
            socket: None,
            held: false,
            run: 0,
            token: bridge::token(),
            letters: Vec::new(),
            letters_shown: false,
        }
    }

    /// A blank tab opened now, waiting for the person to choose what it opens.
    #[must_use]
    pub fn blank(key: TabKey) -> Self {
        Self { state: TabState::Waiting, ..Self::new(key, TabKind::New) }
    }

    /// A tab of `kind` opened now that waits to be started, which the screen does as soon as it
    /// is shown. A tab of one of the workspace's files is opened this way, so it starts exactly
    /// the way a tab brought back from the last session does.
    #[must_use]
    pub fn waiting(key: TabKey, kind: TabKind) -> Self {
        Self { state: TabState::Waiting, ..Self::new(key, kind) }
    }

    /// A tab of `kind` brought back from the last session, as it was recorded: opened at
    /// `opened` and showing `conversation`. It waits to be shown before it starts anything.
    #[must_use]
    pub fn restored(key: TabKey, kind: TabKind, opened: u64, conversation: Option<String>) -> Self {
        Self { opened, conversation, ..Self::waiting(key, kind) }
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

    /// The text of a Markdown tab's file, once it has been read.
    #[must_use]
    pub fn document(&self) -> Option<&str> {
        self.document.as_deref()
    }

    /// Whether the text the tab shows was cut short.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        self.partial
    }

    /// Where a PDF tab stands among its pages.
    #[must_use]
    pub fn pages(&self) -> Pages {
        self.pages
    }

    /// Moves a PDF tab to `pages`.
    pub fn turn(&mut self, pages: Pages) {
        self.pages = pages;
    }

    /// Why a sound tab shows the sound's details rather than playing it.
    #[must_use]
    pub fn quiet(&self) -> Option<Quiet> {
        self.quiet
    }

    /// Records why a sound tab shows the sound's details.
    pub fn keep_quiet(&mut self, why: Quiet) {
        self.quiet = Some(why);
    }

    /// The sound server's socket a sound tab plays through.
    #[must_use]
    pub fn socket(&self) -> Option<&Path> {
        self.socket.as_deref()
    }

    /// Records the socket a sound tab plays through, and that it plays rather than describes.
    pub fn play_through(&mut self, socket: PathBuf) {
        self.socket = Some(socket);
        self.quiet = None;
    }

    /// Whether a sound tab waits to be asked before it plays.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Makes a sound tab wait to be asked before it plays, or lets it play.
    pub fn hold(&mut self, held: bool) {
        self.held = held;
    }

    /// The web address the window asked to have opened, with whether it was opened here; that
    /// is `None` until the opening answers.
    #[must_use]
    pub fn sign_in(&self) -> Option<(&str, Option<bool>)> {
        self.sign_in.as_ref().map(|(address, opened)| (address.as_str(), *opened))
    }

    /// Records the address the window asked to have opened. The address is shown at once, before
    /// anything is known about the browser, because reading it is what the person needs most.
    pub fn asked_to_open(&mut self, address: String) {
        self.sign_in = Some((address, None));
    }

    /// Records how the opening of that address went, if it is still the address being opened.
    pub fn opened_here(&mut self, address: &str, opened: bool) {
        if let Some((waiting, how)) = self.sign_in.as_mut()
            && waiting == address
        {
            *how = Some(opened);
        }
    }

    /// Which session of this tab is the current one.
    #[must_use]
    pub fn run(&self) -> u64 {
        self.run
    }

    /// The token a harness tab is started with, by which the bridge knows it.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The messages that wait in this tab, oldest first.
    #[must_use]
    pub fn letters(&self) -> &[Letter] {
        &self.letters
    }

    /// Whether the waiting messages are shown.
    #[must_use]
    pub fn letters_shown(&self) -> bool {
        self.letters_shown
    }

    /// Leaves `letter` waiting in the tab.
    pub fn receive(&mut self, letter: Letter) {
        self.letters.push(letter);
    }

    /// Shows the waiting messages, or hides them.
    pub fn show_letters(&mut self, shown: bool) {
        self.letters_shown = shown;
    }

    /// Throws the waiting messages away.
    pub fn discard_letters(&mut self) {
        self.letters.clear();
        self.letters_shown = false;
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

    /// Takes the text of a Markdown tab's file, or of a PDF, and shows it; `partial` says it
    /// was cut short.
    pub fn read(&mut self, text: String, partial: bool) {
        self.document = Some(text);
        self.partial = partial;
        self.state = TabState::Running;
    }

    /// Moves the tab to `state`, keeping the session so its last screen stays readable: a tab
    /// that says the container stopped still shows what the harness printed before it did.
    pub fn settled(&mut self, state: TabState) {
        self.state = state;
    }

    /// Ends the tab's session, which is what closing a tab or restarting it does. The container
    /// itself is left alone: other tabs and other workspaces may be using it.
    pub fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.kill();
        }
    }
}
