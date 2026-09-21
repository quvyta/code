//! The engine as the settings screen and the repair strip see it, and the pattern that greys
//! out everything which would reach into a container while there is no engine.

use qframe::prelude::*;
use qframe::widgets::{CopyValue, Placement, Tooltip};

use crate::engine::known::{self, Known};
use crate::engine::{EngineKind, Unavailable};
use crate::store::Platform;
use crate::ui::setup::install::IdRanges;

/// The engine's name, as people write it rather than as its binary is called.
#[must_use]
pub fn name(kind: EngineKind) -> String {
    match kind {
        EngineKind::Podman => t!("settings.podman"),
        EngineKind::Docker => t!("settings.docker"),
    }
}

/// What to tell the person about an engine refusal QCode recognises: the plain sentence, and the
/// one line that puts it right where there is one line that does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Help {
    /// What happened and what to do, in the person's language.
    pub sentence: String,
    /// The command to run, when this system has one QCode can promise.
    pub line: Option<String>,
}

/// The help for `output`, the words `kind` refused with, when they are a refusal QCode
/// recognises; `None` leaves the engine's own words to speak for themselves.
#[must_use]
pub fn help(kind: EngineKind, output: &str) -> Option<Help> {
    known::recognise(output).map(|known| help_for(kind, known, Platform::host()))
}

/// The help for a refusal already recognised, on `platform`.
#[must_use]
pub fn help_for(kind: EngineKind, known: Known, platform: Platform) -> Help {
    let engine = name(kind);
    let sentence = match known {
        Known::ImageMissing => t!("known.image-missing", engine = engine),
        Known::NotRunning => t!("known.not-running", engine = engine),
        Known::NoPermission => t!("known.no-permission", engine = engine),
        Known::NoIdRanges => t!("known.no-id-ranges", engine = engine),
    };
    let line = match (known, kind, platform) {
        (Known::NotRunning, EngineKind::Docker, Platform::Linux) => {
            Some("sudo systemctl enable --now docker".to_owned())
        }
        (Known::NotRunning, EngineKind::Docker, Platform::MacOs) => Some("open -a Docker".to_owned()),
        // Podman on Linux needs no service of its own; one it cannot reach is the socket a
        // remote connection points at, which the person's own systemd starts.
        (Known::NotRunning, EngineKind::Podman, Platform::Linux) => {
            Some("systemctl --user enable --now podman.socket".to_owned())
        }
        (Known::NotRunning, EngineKind::Podman, _) => Some("podman machine start".to_owned()),
        (Known::NoPermission, _, Platform::Linux) => Some("sudo usermod -aG docker $USER".to_owned()),
        (Known::NoIdRanges, _, Platform::Linux) => Some(id_ranges_line()),
        _ => None,
    };
    Help { sentence, line }
}

impl Help {
    /// Draws the sentence and, under it, the line to run beside a few words saying so. The
    /// line is a value the person copies: QCode runs nothing of the kind by itself here.
    pub fn show<M: Clone + 'static>(&self, ui: &mut View<'_, M>) {
        ui.add(Text::new(self.sentence.clone()).role("secondary")).fill_width();
        if let Some(line) = &self.line {
            ui.row(|ui| {
                ui.add(Text::new(t!("known.run")).role("faint"));
                ui.add(CopyValue::new(line.clone())).id("known-line").fill_width();
            })
            .gap(2)
            .fill_width();
        }
    }
}

/// The line that gives this account its id ranges. The two files are read once, the first time
/// the line is needed, which is only ever while podman is refusing for want of them: the range
/// they lead to stays right until the line is run, and after that it is no longer shown.
fn id_ranges_line() -> String {
    static LINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINE.get_or_init(|| IdRanges::here().line()).clone()
}

/// Why the engine cannot be used, in a shape a screen can keep.
///
/// [`Unavailable`] carries a `std::io::Error`, so it can be neither cloned nor compared, while a
/// screen's state and every message travelling through it have to be both. This keeps the
/// distinctions the person is told apart by and turns the rest into the words they read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trouble {
    /// No binary anywhere QCode looked.
    NotInstalled,
    /// Docker is installed and its daemon is not answering.
    DaemonStopped,
    /// Podman is installed and the Linux virtual machine it needs is not running.
    MachineStopped,
    /// The engine ran and refused, carrying what it said.
    Refused(String),
    /// A binary was found and could not be started, carrying what the system said.
    NotRunnable(String),
}

impl Trouble {
    /// What the engine layer found, kept for a screen.
    #[must_use]
    pub fn of(unavailable: &Unavailable) -> Self {
        match unavailable {
            Unavailable::NotInstalled => Self::NotInstalled,
            Unavailable::DaemonStopped { .. } => Self::DaemonStopped,
            Unavailable::MachineStopped { .. } => Self::MachineStopped,
            Unavailable::InfoFailed { output, .. } => Self::Refused(output.clone()),
            Unavailable::NotRunnable { error, .. } => Self::NotRunnable(error.to_string()),
        }
    }

    /// The few words the repair strip has room for. The strip stands over every screen, so it
    /// stays one row in a narrow terminal; the whole sentence is [`headline`](Self::headline),
    /// on the settings screen, where there is room to read it.
    #[must_use]
    pub fn brief(&self, kind: EngineKind) -> String {
        let engine = name(kind);
        match self {
            Self::NotInstalled => t!("settings.brief-not-installed"),
            Self::DaemonStopped => t!("settings.brief-daemon-stopped", engine = engine),
            Self::MachineStopped => t!("settings.brief-machine-stopped", engine = engine),
            Self::Refused(_) => t!("settings.brief-refused", engine = engine),
            Self::NotRunnable(_) => t!("settings.brief-not-runnable", engine = engine),
        }
    }

    /// The one sentence that says what is wrong.
    #[must_use]
    pub fn headline(&self, kind: EngineKind) -> String {
        let engine = name(kind);
        match self {
            Self::NotInstalled => t!("settings.engine-not-installed", engine = engine),
            Self::DaemonStopped => t!("settings.engine-daemon-stopped", engine = engine),
            Self::MachineStopped => t!("settings.engine-machine-stopped", engine = engine),
            Self::Refused(_) => t!("settings.engine-refused", engine = engine),
            Self::NotRunnable(_) => t!("settings.engine-not-runnable", engine = engine),
        }
    }

    /// What the person has to do about it. Repair is named in each one, because repair is the
    /// only thing the strip offers and a sentence that stops without a next step reads as a dead
    /// end.
    #[must_use]
    pub fn remedy(&self, kind: EngineKind) -> String {
        let engine = name(kind);
        match self {
            Self::NotInstalled => t!("settings.remedy-not-installed"),
            Self::DaemonStopped => t!("settings.remedy-daemon-stopped", engine = engine),
            Self::MachineStopped => t!("settings.remedy-machine-stopped"),
            Self::Refused(_) => t!("settings.remedy-refused", engine = engine),
            Self::NotRunnable(_) => t!("settings.remedy-not-runnable"),
        }
    }

    /// What the engine or the system said, for the person to read, when there were words.
    #[must_use]
    pub fn said(&self) -> Option<&str> {
        match self {
            Self::NotInstalled | Self::DaemonStopped | Self::MachineStopped => None,
            Self::Refused(output) | Self::NotRunnable(output) => Some(output),
        }
    }
}

/// What the last engine check left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// The check has not answered yet.
    Checking,
    /// The engine answered and can be used.
    Working,
    /// The engine cannot be used, and why.
    Missing(Trouble),
}

/// The chosen engine and how it is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineState {
    kind: EngineKind,
    health: Health,
}

impl EngineState {
    /// An engine of `kind` in `health`.
    #[must_use]
    pub fn new(kind: EngineKind, health: Health) -> Self {
        Self { kind, health }
    }

    /// Which engine QCode is set to use.
    #[must_use]
    pub fn kind(&self) -> EngineKind {
        self.kind
    }

    /// How the engine is doing.
    #[must_use]
    pub fn health(&self) -> &Health {
        &self.health
    }

    /// The line that describes the engine wherever there is room for one line only.
    #[must_use]
    pub fn summary(&self) -> String {
        match &self.health {
            Health::Checking => t!("settings.engine-checking"),
            Health::Working => t!("settings.engine-working", engine = name(self.kind)),
            Health::Missing(trouble) => trouble.headline(self.kind),
        }
    }
}

/// Whether the actions that reach into a container may be used, and what to say when they may
/// not.
///
/// Every action that would start a container, copy into one or delete a volume goes through a
/// gate: while it is shut the action is drawn dead and carries the reason, instead of looking
/// alive and failing with the engine's own error once it is pressed. The reason is given twice,
/// as a tooltip for the pointer and as a line the screen prints beside the actions for the
/// keyboard, because a dead control takes no focus and a tooltip alone would only ever reach a
/// mouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    shut: Option<String>,
}

impl Gate {
    /// The gate as `engine` leaves it.
    #[must_use]
    pub fn of(engine: &EngineState) -> Self {
        let shut = match engine.health() {
            Health::Working => None,
            Health::Checking => Some(t!("settings.needs-engine", reason = t!("settings.still-looking"))),
            Health::Missing(trouble) => Some(t!("settings.needs-engine", reason = trouble.headline(engine.kind()))),
        };
        Self { shut }
    }

    /// A gate that is shut for `reason`, for the actions kept back by something nearer than the
    /// engine, such as a profile that has no login to work on.
    #[must_use]
    pub fn shut(reason: impl Into<String>) -> Self {
        Self { shut: Some(reason.into()) }
    }

    /// Whether the actions behind the gate may be used.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.shut.is_none()
    }

    /// Why they may not, while they may not.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.shut.as_deref()
    }

    /// Adds what `build` draws and, while the gate is shut, wraps it in a tooltip saying why.
    /// `build` is handed whether the gate is open, to pass straight on to `disabled`.
    pub fn show<Msg: 'static>(&self, ui: &mut View<'_, Msg>, build: impl FnOnce(&mut View<'_, Msg>, bool)) {
        match &self.shut {
            None => build(ui, true),
            Some(reason) => {
                // Above, because the same reason is written out directly below the actions and
                // a tooltip landing on top of it would tell the person nothing new.
                ui.add_with(Tooltip::new(reason.clone()).placement(Placement::Above), |ui| build(ui, false));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineKind, Unavailable};
    use std::path::PathBuf;

    #[test]
    fn every_reason_the_engine_layer_gives_keeps_its_own_answer() {
        let cases = [
            (Unavailable::NotInstalled, Trouble::NotInstalled),
            (Unavailable::DaemonStopped { output: "no socket".into() }, Trouble::DaemonStopped),
            (Unavailable::MachineStopped { output: "no vm".into() }, Trouble::MachineStopped),
            (
                Unavailable::InfoFailed { code: Some(125), output: "short-name".into() },
                Trouble::Refused("short-name".into()),
            ),
        ];
        for (unavailable, expected) in cases {
            assert_eq!(Trouble::of(&unavailable), expected);
        }
    }

    #[test]
    fn a_binary_that_cannot_be_started_keeps_what_the_system_said() {
        let unavailable = Unavailable::NotRunnable {
            bin: PathBuf::from("/usr/bin/podman"),
            error: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let Trouble::NotRunnable(said) = Trouble::of(&unavailable) else {
            panic!("a binary that cannot be started is its own answer")
        };
        assert!(!said.is_empty(), "the system's words are kept for the person to read");
    }

    #[test]
    fn a_stopped_daemon_is_not_worded_as_a_missing_engine() {
        super::super::testing::translated("en", || {
            let stopped = Trouble::DaemonStopped.headline(EngineKind::Docker);
            let missing = Trouble::NotInstalled.headline(EngineKind::Docker);
            assert_ne!(stopped, missing);
            assert!(stopped.contains("Docker"), "{stopped}");
        });
    }

    #[test]
    fn only_the_answers_that_came_with_words_have_words() {
        assert_eq!(Trouble::NotInstalled.said(), None);
        assert_eq!(Trouble::Refused("boom".into()).said(), Some("boom"));
        assert_eq!(Trouble::NotRunnable("denied".into()).said(), Some("denied"));
    }

    #[test]
    fn a_working_engine_opens_the_gate() {
        let gate = Gate::of(&EngineState::new(EngineKind::Podman, Health::Working));
        assert!(gate.is_open());
        assert_eq!(gate.reason(), None);
    }

    #[test]
    fn a_missing_engine_shuts_the_gate_and_the_reason_names_the_engine() {
        super::super::testing::translated("en", || {
            let engine = EngineState::new(EngineKind::Podman, Health::Missing(Trouble::NotInstalled));
            let gate = Gate::of(&engine);
            assert!(!gate.is_open());
            let reason = gate.reason().expect("a shut gate says why").to_owned();
            assert!(reason.contains("Podman"), "{reason}");
        });
    }

    #[test]
    fn the_gate_stays_shut_while_the_check_has_not_answered() {
        // Acting on a container before the engine answered would fail in a way nobody can read,
        // so the wait is the reason rather than a silent yes.
        super::super::testing::translated("en", || {
            let gate = Gate::of(&EngineState::new(EngineKind::Docker, Health::Checking));
            assert!(!gate.is_open());
            assert!(gate.reason().is_some());
        });
    }
}
