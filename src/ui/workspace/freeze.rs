//! When a profile's container may be frozen, and what freezing one is on this machine.
//!
//! Nothing here decides to freeze anything: [`may_freeze`] is a question with an answer, asked by
//! whatever will later do the freezing, and it answers false the moment it does not know. That is
//! the whole shape of the module. A container is worth freezing only when it certainly holds
//! nothing that is working, and the cost of being wrong is not a slow machine but a person's
//! conversation: everything a tab has written is in that container, and ending it loses the work
//! with nothing left to say so.
//!
//! So the rules are read rather than judged, and each one is a fact with an owner: the terminal for
//! how long a tab has said nothing, the relay for whether a request is on its way to a model, the
//! container's own control group for what it has been doing, and `/proc` inside the container for
//! what is running in it. Anything that cannot be read is not a yes.
//!
//! Measured on 2026-09-30, which is where the two numbers come from: a profile's container with
//! four tabs holds about 1 GB, and after `podman pause` and writing `4G` into that container's
//! `memory.reclaim` it holds about 10 MB, the rest having gone to swap (zram compresses it about
//! six times over). After `podman unpause` a tab answers within 0.1 to 0.3 s, and bytes written
//! into a tab's terminal while its container was paused arrive whole.
//!
//! The machine's side is only podman, only rootless, and only with swap: Docker's cgroup belongs
//! to root, and without swap a paused container gives back almost nothing while its pages stay
//! where they are. [`supported`] says which machine this is, and the rest of the side is what the
//! kernel and the engine are asked for.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::base::paths::KEEP_ALIVE;
use crate::engine::run::{self, EngineError};
use crate::engine::{Engine, EngineKind, Exec};
use crate::profile::{ACCESSLINT_PROGRAM, HarnessKind};

use super::shared::TAB_LOOP;

/// How long every tab of a profile must have written nothing to its terminal.
///
/// A working Claude Code or opencode redraws its timer and its spinner every second, even while it
/// waits for the model, so ten minutes of silence means the work is over rather than slow. It also
/// keeps a short break free: whoever comes back within ten minutes finds everything still in
/// memory, which is the difference between freezing a container and losing a morning's work to a
/// laptop that was closed.
pub(super) const QUIET: Duration = Duration::from_secs(10 * 60);

/// The shortest window the container's CPU use is read over.
///
/// Long enough that a moment of work is a moment rather than a rounding error, and short enough
/// that a profile which was doing something a minute ago is not judged on a stale reading. It is
/// longer than the screen's look, so a window is measured from a reading kept across looks.
pub(super) const CPU_WINDOW: Duration = Duration::from_secs(2 * 60);

/// How much of one core a quiet profile's container may use, in percent.
///
/// A build, a test or any long command inside the container breaks this at once: those spend whole
/// cores, and the reading is over a window long enough to see them. One percent is what is left of
/// a machine that is otherwise being used by the person sitting at it.
pub(super) const CPU_SHARE: u64 = 1;

/// The two lengths [`may_freeze`] judges a quiet time and a CPU window against, and the fourth
/// thing it is given rather than reading: the product's are [`QUIET`] and [`CPU_WINDOW`], and a
/// test may shorten them, since a test cannot wait ten minutes for anything.
///
/// The period of the look itself is not here: it decides how often the rules are asked, not what
/// they say, and it belongs to the screen that asks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lengths {
    /// How long every tab of a profile must have written nothing.
    pub quiet: Duration,
    /// The shortest window the container's CPU use is read over.
    pub window: Duration,
}

impl Default for Lengths {
    fn default() -> Self {
        Self { quiet: QUIET, window: CPU_WINDOW }
    }
}

/// Everything the decision to freeze reads. Each field that may be unknown is an [`Option`], and
/// `None` means "not to be frozen": a profile whose tabs have not been looked at, whose relay
/// cannot be reached or whose control group cannot be read is left exactly as it is.
#[derive(Debug, Clone)]
pub(super) struct Signals {
    /// A tab of this profile is the one shown on the screen.
    pub(super) on_screen: bool,
    /// Some tab holds a line the person has not sent, a message is being pasted into one, or a
    /// message waits to be delivered to one.
    pub(super) typing: bool,
    /// The shortest time since any of its tabs last wrote output; `None` when it has no terminal
    /// tab, or when what its tabs last wrote cannot be read.
    pub(super) quiet_for: Option<Duration>,
    /// Whether a request of one of its tokens is on its way to a model; `None` when the profile
    /// does not run on a provider, which skips the provider's rules rather than failing them.
    pub(super) relay_busy: Option<bool>,
    /// How long ago the last request of one of its tokens finished; `None` when the profile does
    /// not run on a provider, and when no request of its tokens has ever finished.
    pub(super) relay_quiet_for: Option<Duration>,
    /// Microseconds of CPU the container used, and over how long a window that was read.
    pub(super) cpu: Option<(u64, Duration)>,
    /// The command line of every process in the container; `None` when they cannot be read.
    pub(super) processes: Option<Vec<String>>,
    /// The profile opens a window.
    pub(super) desktop: bool,
}

/// Whether the container of a profile carrying `signals` may be frozen, judging the processes in it
/// as those of `harness` and the two lengths against `lengths`.
///
/// Five rules, and all five must hold. They are what was approved, so they are stated as they were
/// approved rather than as this function happens to read them:
///
/// 1. The profile opens no window, is not the one on the screen, and nobody is typing into one of
///    its tabs.
/// 2. Every one of its tabs has written nothing to its terminal for [`Lengths::quiet`], which is
///    [`QUIET`] in the product.
/// 3. If it runs on a provider, no request of its tokens is being served and the last one finished
///    that long ago — a profile that has never asked is quiet.
/// 4. Its container has used less than [`CPU_SHARE`] percent of one core over a window of at least
///    [`Lengths::window`], which is [`CPU_WINDOW`] in the product.
/// 5. Every process in it is one a quiet profile may hold ([`resident`]).
///
/// Anything not known means false. The reading is the safety, and a profile whose tabs cannot be
/// seen, whose relay cannot be asked or whose control group cannot be read is one this says no to.
#[must_use]
pub(super) fn may_freeze(signals: &Signals, harness: HarnessKind, lengths: Lengths) -> bool {
    if signals.desktop || signals.on_screen || signals.typing {
        return false;
    }
    if signals.quiet_for.is_none_or(|quiet| quiet < lengths.quiet) {
        return false;
    }
    // A profile that does not run on a provider carries no relay reading at all, which skips these
    // two rules rather than failing them: it has nothing on its way to a model.
    if let Some(busy) = signals.relay_busy
        && (busy || signals.relay_quiet_for.is_some_and(|quiet| quiet < lengths.quiet))
    {
        return false;
    }
    let Some((used, window)) = signals.cpu else { return false };
    if window < lengths.window {
        return false;
    }
    // The share of a core the reading took, as hundredths of the share rather than as a fraction of
    // one: both sides are multiplied out, so no division rounds and a container on the share is a
    // no, which is what a machine that is being used deserves.
    if u128::from(used).saturating_mul(100) >= window.as_micros().saturating_mul(u128::from(CPU_SHARE)) {
        return false;
    }
    signals.processes.as_ref().is_some_and(|lines| lines.iter().all(|line| resident(line, harness)))
}

/// Whether `line`, a command line read out of a container's `/proc`, is one a quiet profile is
/// allowed to hold.
///
/// Everything else means the profile is working: a build, a test, a server the agent started, a
/// shell of its own. Those are the containers that are not frozen, and the list is short on purpose
/// — a process nobody recognised is a process nobody can say is idle.
#[must_use]
pub(super) fn resident(line: &str, harness: HarnessKind) -> bool {
    let line = line.trim();
    if line.is_empty() {
        // A kernel thread, or a process that ended between the folder being read and its own line:
        // nothing of it is running, so nothing of it is work.
        return true;
    }
    if keeps_alive(line) {
        return true;
    }
    if QCODE_SERVERS.iter().any(|server| line.contains(server)) {
        return true;
    }
    if PLUGIN_SERVERS.iter().any(|server| line.contains(server)) {
        return true;
    }
    runs_harness(line, harness) || attaches_to_shared_server(line, harness)
}

/// The container's own keep-alive: the shell that runs it, spelled with no arguments after its
/// script, and the `sleep 3600` it is waiting on. A container whose processes are these and the
/// harness is up and doing nothing at all.
fn keeps_alive(line: &str) -> bool {
    line == "sleep 3600" || line.strip_prefix("sh -c ").is_some_and(|script| script == KEEP_ALIVE[2])
}

/// QCode's own programs inside a container: the bridge between its tabs, the keeper of a shared
/// opencode server, and the relay that carries a request on to a provider. They are asked for by
/// name, and a container holds them whether or not a tab is looking at it right now.
const QCODE_SERVERS: [&str; 3] = ["qcode-bridge.mjs", "qcode-opencode.mjs", "qcode-relay.mjs"];

/// The servers the templates' own plugins start in a container and leave there for the life of it:
/// accesslint's MCP server, which the image installs beside the harness, and rust-analyzer, which
/// the harness starts for a Rust workspace and its proc-macro server beside that.
const PLUGIN_SERVERS: [&str; 3] = [ACCESSLINT_PROGRAM, "rust-analyzer", "rust-analyzer-proc-macro-srv"];

/// Whether `line` is the harness itself: the program named by its record, whether it was named by
/// its path or is the script a `node` of its own runs.
fn runs_harness(line: &str, harness: HarnessKind) -> bool {
    let wanted = harness.record().command;
    let mut words = line.split_whitespace();
    let first = words.next().unwrap_or_default();
    if named(first, wanted) {
        return true;
    }
    named(first, "node") && words.next().is_some_and(|script| named(script, wanted))
}

/// The opencode programs that are the harness by another name: the server a profile's tabs share
/// and the interface attached to it, both named by the same program, and the shell loop that
/// attaches one.
///
/// The loop is a shell, so it is looked for by the text of its own script. What the reading keeps
/// of a command line is 200 characters and the loop is three times that, so the two are compared
/// over whichever of them is shorter: a line cut short is a beginning of the loop, and a whole one
/// begins with it.
fn attaches_to_shared_server(line: &str, harness: HarnessKind) -> bool {
    harness == HarnessKind::OpenCode
        && line.strip_prefix("sh -c ").is_some_and(|said| said.starts_with(TAB_LOOP) || TAB_LOOP.starts_with(said))
}

/// Whether the program `word` runs has the file name `wanted`, so that a path names it as well as
/// the bare word does.
fn named(word: &str, wanted: &str) -> bool {
    Path::new(word).file_name().is_some_and(|name| name == wanted)
}

/// Whether this machine can freeze a container at all: only podman, only running as the person who
/// started it, and only with swap to put the pages in.
///
/// A rootful engine's control group belongs to root, so the one write [`freeze`] makes would be
/// refused; and with no swap the pages of a paused container stay exactly where they were, which
/// makes the freeze a thing QCode does for nothing while a tab in it stops answering.
///
/// Asked once when the screen gets its engine, on a background thread, since it runs one.
#[must_use]
pub(super) fn supported(engine: &Engine) -> bool {
    rootless(engine) && std::fs::read_to_string(SWAPS).is_ok_and(|listing| swap_areas(&listing) > 0)
}

/// Whether `engine` puts a container's control group where the person who ran it may write to it:
/// podman, and podman running as that person rather than as root.
///
/// A rootful podman is started by someone else and its cgroup belongs to that someone, so the one
/// write [`freeze`] makes would be refused. Docker is not even asked: its daemon is root whatever
/// the person does, so it has no such answer to give.
fn rootless(engine: &Engine) -> bool {
    engine.kind() == EngineKind::Podman && run::capture(&engine.rootless()).is_ok_and(|said| said.trim() == "true")
}

/// How many swap areas a listing of `/proc/swaps` holds, counting what the kernel wrote under its
/// own heading. A listing of the heading alone is a machine with no swap.
fn swap_areas(listing: &str) -> usize {
    listing.lines().skip(1).filter(|area| !area.trim().is_empty()).count()
}

/// Where this machine keeps every control group, which is the only place a frozen container's
/// pages can be handed back from. The screen's [`Where`](super::freezing::Where) carries it, so
/// that a test can point the reading at a folder of its own.
pub(super) const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// The file the kernel lists the machine's swap areas in, and the one place it is read from.
const SWAPS: &str = "/proc/swaps";

/// Where `container`'s control group is under `cgroups`, and `None` when it is not there.
///
/// The engine answers with a path under that tree, which is the person's own to write to only
/// under a rootless podman. A container that has never run has none.
///
/// Runs an engine command, so it belongs on a background thread.
#[must_use]
pub(super) fn cgroup(engine: &Engine, container: &str, cgroups: &Path) -> Option<PathBuf> {
    let said = run::capture(&engine.cgroup_of(container)).ok()?;
    let under = said.trim().strip_prefix('/')?;
    let path = cgroups.join(under);
    path.is_dir().then_some(path)
}

/// How many microseconds of CPU everything in `cgroup` has used so far, or `None` when the kernel
/// would not say. Read twice, a moment apart, this is what tells a container that is working from
/// one that is not.
#[must_use]
pub(super) fn cpu_used(cgroup: &Path) -> Option<u64> {
    let stat = std::fs::read_to_string(cgroup.join("cpu.stat")).ok()?;
    usage_micros(&stat)
}

/// The `usage_usec` line of a cgroup's `cpu.stat`, in microseconds. Every cgroup v2 `cpu.stat`
/// carries it; the rest of the file counts periods and throttling, which say nothing about how much
/// work the container has done.
fn usage_micros(stat: &str) -> Option<u64> {
    stat.lines().find_map(|line| line.strip_prefix("usage_usec")).and_then(|said| said.trim().parse().ok())
}

/// The command line of every process in `container`, read out of `/proc` inside it; `None` when
/// the engine would not run the reading.
///
/// The loop is the one the memory measurements use: the images promise no `ps`, so every process
/// is found under `/proc` and its own line read there. What is read here is only the command line,
/// and the reading shell's own line is left out — it is QCode asking the question, not the profile
/// working. A line the kernel gave nothing for is kept: a kernel thread, or a process that ended
/// while the folder was being read, is not a process that is working.
///
/// A paused container refuses this, which is one of the reasons the rules hold a container that is
/// doing nothing rather than one that is already frozen: a look never reads a frozen container, and
/// a woken one is read again only by the next look.
///
/// Runs an engine command, so it belongs on a background thread.
#[must_use]
pub(super) fn processes(engine: &Engine, container: &str) -> Option<Vec<String>> {
    let script = "for p in /proc/[0-9]*; do \
                  c=$(tr '\\000' ' ' < \"$p/cmdline\" 2>/dev/null | cut -c1-200); \
                  echo \"$c\"; done";
    let said = run::capture(&engine.exec_without_terminal(&Exec { container, command: &["sh", "-c", script] })).ok()?;
    Some(said.lines().map(str::trim).filter(|line| !line.starts_with("sh -c for p in")).map(str::to_owned).collect())
}

/// Freezes `container` and hands its pages back to the machine, leaving it up with every tab in it
/// still running and holding everything it held. Its control group is looked for under `cgroups`.
///
/// Pausing first is what makes the reading right: nothing can start while the pages are being
/// counted away. The reclaim then asks for [`RECLAIMED`] and takes what it can.
///
/// The kernel answers "resource temporarily unavailable" when it could not take all of it, which is
/// normal and not a failure: the rest is left to swap by the kernel's own accounting and by the
/// next reclaim. Any other refusal of the write is returned, with the container still frozen,
/// because frozen is still correct and the next reading will try again.
///
/// Runs an engine command and writes to the machine's control group, so it belongs on a background
/// thread.
///
/// # Errors
///
/// When the engine will not pause the container, or refuses the write for any reason other than
/// having taken as much as it could.
pub(super) fn freeze(engine: &Engine, container: &str, cgroups: &Path) -> Result<(), FreezeTrouble> {
    run::capture(&engine.pause_container(container)).map_err(FreezeTrouble::Engine)?;
    let Some(cgroup) = cgroup(engine, container, cgroups) else { return Err(FreezeTrouble::NoCgroup) };
    match std::fs::write(cgroup.join("memory.reclaim"), RECLAIMED) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(error) => Err(FreezeTrouble::Reclaim(error)),
    }
}

/// Wakes `container`: it goes on from the state it was frozen in, so every tab in it is still the
/// tab it was.
///
/// # Errors
///
/// When the engine will not wake the container.
pub(super) fn thaw(engine: &Engine, container: &str) -> Result<(), EngineError> {
    run::capture(&engine.unpause_container(container)).map(|_| ())
}

/// How much is asked back from a frozen container. Measured: four gigabytes is what brings a
/// four-tab profile's container from about 1 GB to about 10 MB, the rest going to swap.
const RECLAIMED: &str = "4G";

/// Why a container could not be frozen. The container is paused in every one of these but the
/// first, which is the point: a profile nobody is waiting on may stay frozen until the next reading
/// and the next, and waking it costs a person their work.
///
/// The words each of these carries are the ones a person is given when a container they are waiting
/// on could not be frozen. A profile nobody is looking at is not one of those, and its failure is
/// left quiet, so the words are read by the tests of the freezing rather than by a screen today.
#[derive(Debug)]
#[expect(
    dead_code,
    reason = "a failed freeze says nothing to the person; its words are read by the tests until a screen has one to say them to"
)]
pub(super) enum FreezeTrouble {
    /// The engine would not pause the container.
    Engine(EngineError),
    /// The container has no control group on this machine to hand pages back from.
    NoCgroup,
    /// The kernel refused the reclaim for a reason of its own.
    Reclaim(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::super::shared;
    use super::*;

    /// The lengths the product judges by, which every test here is written against.
    const MEASURED: Lengths = Lengths { quiet: QUIET, window: CPU_WINDOW };

    #[test]
    fn a_shared_opencode_tabs_shell_loop_is_resident_although_the_reading_cut_it_short() {
        // What the reading actually hands over: 200 characters of a command line, and this loop is
        // three times that long, so what is left of it names no script of QCode's.
        let line = shared::tab_program(Some("01a0cf22-23dc-7fd0-8c28-6a9fef67a234")).join(" ");
        let cut: String = line.chars().take(200).collect();
        assert!(!cut.contains("qcode-opencode.mjs"), "the script the tab was given is past what is left: {cut}");
        assert!(resident(&cut, HarnessKind::OpenCode), "but it is still the loop a shared tab runs: {cut}");
        assert!(resident(&line, HarnessKind::OpenCode), "and the whole of it when nothing cut it");
        // It is opencode's loop and no other harness's, and a loop of someone's own is work.
        assert!(!resident(&cut, HarnessKind::ClaudeCode));
        assert!(!resident("sh -c while :; do make; done", HarnessKind::OpenCode));
    }

    /// A profile nobody is looking at: every rule holds, and one change at a time is what each of
    /// the tests below breaks.
    fn quiet() -> Signals {
        Signals {
            on_screen: false,
            typing: false,
            quiet_for: Some(QUIET),
            relay_busy: Some(false),
            relay_quiet_for: Some(QUIET),
            cpu: Some((1_000, CPU_WINDOW)),
            processes: Some(vec![
                KEEP_ALIVE.join(" "),
                "sleep 3600".to_owned(),
                "claude --dangerously-skip-permissions".to_owned(),
            ]),
            desktop: false,
        }
    }

    #[test]
    fn a_profile_nobody_is_looking_at_may_be_frozen() {
        assert!(may_freeze(&quiet(), HarnessKind::ClaudeCode, MEASURED));
        // A profile that never runs on a provider carries no relay reading at all, which skips
        // those rules rather than failing them.
        let mut without = quiet();
        without.relay_busy = None;
        without.relay_quiet_for = None;
        assert!(may_freeze(&without, HarnessKind::ClaudeCode, MEASURED));
        // And one that has never asked is quiet however long its tabs have been silent.
        let mut never_asked = without.clone();
        never_asked.relay_busy = Some(false);
        never_asked.relay_quiet_for = None;
        assert!(may_freeze(&never_asked, HarnessKind::ClaudeCode, MEASURED));
    }

    #[test]
    fn anything_unknown_is_a_no() {
        let none = |change: &dyn Fn(&mut Signals)| {
            let mut signals = quiet();
            change(&mut signals);
            may_freeze(&signals, HarnessKind::ClaudeCode, MEASURED)
        };
        assert!(!none(&|s| s.quiet_for = None), "a profile whose tabs cannot be seen");
        assert!(!none(&|s| s.cpu = None), "a container whose cgroup cannot be read");
        assert!(!none(&|s| s.processes = None), "processes that cannot be read");
        assert!(!none(&|s| s.quiet_for = Some(QUIET - Duration::from_secs(1))), "not quiet for long enough");
        assert!(!none(&|s| s.on_screen = true), "the profile on screen");
        assert!(!none(&|s| s.typing = true), "a line the person has not sent");
        assert!(!none(&|s| s.desktop = true), "a profile that opens a window");
    }

    #[test]
    fn a_request_on_its_way_to_a_model_is_a_no_however_quiet_the_tabs_are() {
        let mut asking = quiet();
        asking.relay_busy = Some(true);
        assert!(!may_freeze(&asking, HarnessKind::ClaudeCode, MEASURED));
        let mut just_asked = quiet();
        just_asked.relay_quiet_for = Some(QUIET - Duration::from_secs(1));
        assert!(
            !may_freeze(&just_asked, HarnessKind::ClaudeCode, MEASURED),
            "the answer is still on its way to the tab"
        );
    }

    #[test]
    fn a_container_that_has_been_working_is_a_no_and_a_window_longer_than_the_shortest_is_what_says_so() {
        let mut working = quiet();
        // A whole core and a bit over a window of two minutes is about 87 percent of one.
        working.cpu = Some((105_000_000, CPU_WINDOW));
        assert!(!may_freeze(&working, HarnessKind::ClaudeCode, MEASURED));
        // Under the share is under it: one percent of a window of two minutes is 1.2 s of CPU.
        let mut idle = quiet();
        idle.cpu = Some((1_199_000, CPU_WINDOW));
        assert!(may_freeze(&idle, HarnessKind::ClaudeCode, MEASURED));
        idle.cpu = Some((1_201_000, CPU_WINDOW));
        assert!(!may_freeze(&idle, HarnessKind::ClaudeCode, MEASURED));
        // A window too short to see a moment of work says nothing either way.
        let mut brief = quiet();
        brief.cpu = Some((0, CPU_WINDOW - Duration::from_secs(1)));
        assert!(!may_freeze(&brief, HarnessKind::ClaudeCode, MEASURED), "the window is shorter than it must be");
    }

    #[test]
    fn one_process_that_is_working_is_a_no_for_the_whole_profile() {
        let mut working = quiet();
        working.processes.as_mut().expect("a list").push("cargo build".to_owned());
        assert!(!may_freeze(&working, HarnessKind::ClaudeCode, MEASURED));
        // The harness of another profile is another profile's work.
        let mut other = quiet();
        other.processes.as_mut().expect("a list").push("opencode --auto".to_owned());
        assert!(!may_freeze(&other, HarnessKind::ClaudeCode, MEASURED), "Claude Code's profile is not opencode's");
    }

    #[test]
    fn a_quiet_profile_may_hold_the_container_itself_its_harness_and_qcodes_own_helpers() {
        for line in [
            // The container's own keep-alive, spelled as `/proc` spells it.
            "sh -c trap 'exit 0' TERM INT; while :; do sleep 3600 & wait $!; done",
            "sleep 3600",
            // The harness, by its own name and by the path it is installed on.
            "claude --dangerously-skip-permissions --settings {}",
            "/usr/local/bin/claude --dangerously-skip-permissions",
            // QCode's own programs, which a container holds whether or not a tab is in it.
            "node /run/qcode-mcp/qcode-bridge.mjs",
            "node /run/qcode-mcp/qcode-opencode.mjs keep",
            "node /run/qcode-mcp/qcode-relay.mjs claude --dangerously-skip-permissions",
            // The servers the templates' own plugins start and leave.
            "/usr/local/lib/node_modules/@accesslint/mcp/bin/accesslint-mcp",
            "rust-analyzer proc-macro-srv",
            "rust-analyzer",
            // A kernel thread, or a process that ended while its line was being read.
            "",
        ] {
            assert!(resident(line, HarnessKind::ClaudeCode), "{line:?} may be held");
        }
        // opencode's own programs, and the shell loop that attaches an interface to its server. The
        // server is one binary in the image, and a harness that is a script is named by the script
        // `node` runs rather than by the program that started it.
        for line in [
            "opencode --auto",
            "node /usr/local/lib/node_modules/opencode/bin/opencode serve --port 41418",
            "/usr/local/lib/node_modules/opencode-linux-x64/bin/opencode serve --port 41418",
            "/usr/local/lib/node_modules/opencode-linux-x64/bin/opencode attach http://127.0.0.1:41418 --dir /work",
        ] {
            assert!(resident(line, HarnessKind::OpenCode), "{line:?} may be held");
        }
    }

    #[test]
    fn anything_else_means_the_profile_is_working() {
        for line in [
            "cargo build",
            "node server.js",
            "npm run dev",
            "python3 -m pytest",
            "sleep 30",
            "sh -c make",
            "npm install -g opencode-ai",
            "python3 /home/qcode/.claude/security/agent-sdk-venv/bin/review.py",
        ] {
            assert!(!resident(line, HarnessKind::ClaudeCode), "{line:?} is work");
            assert!(!resident(line, HarnessKind::OpenCode), "{line:?} is work");
        }
    }

    /// The lengths are the screen's, and what a look is judged against is the two it was given
    /// rather than the measured pair: this is what lets a test reach a ten-minute rule at all.
    #[test]
    fn the_quiet_time_and_the_cpu_window_judged_against_are_the_two_the_caller_gave() {
        let brief = Lengths { quiet: Duration::from_secs(1), window: Duration::from_secs(1) };
        // Quiet for two seconds, over a two-second window with a hundredth of a second of CPU in
        // it: half the share, and nowhere near the product's ten minutes and two-minute window.
        let mut signals = quiet();
        signals.quiet_for = Some(Duration::from_secs(2));
        signals.relay_quiet_for = Some(Duration::from_secs(2));
        signals.cpu = Some((10_000, Duration::from_secs(2)));
        assert!(!may_freeze(&signals, HarnessKind::ClaudeCode, MEASURED), "not quiet for the product's ten minutes");
        assert!(may_freeze(&signals, HarnessKind::ClaudeCode, brief), "quiet for the two seconds the caller asked");

        // And a window shorter than the one asked for is not a window at all, whatever the CPU.
        signals.cpu = Some((0, Duration::from_millis(500)));
        assert!(!may_freeze(&signals, HarnessKind::ClaudeCode, brief));
        assert!(!may_freeze(
            &quiet(),
            HarnessKind::ClaudeCode,
            Lengths { quiet: QUIET + Duration::from_secs(1), ..MEASURED }
        ));
    }

    #[test]
    fn the_swap_areas_of_a_machine_are_counted_from_what_the_kernel_writes() {
        let listing = "Filename\t\t\t\tType\tSize\tUsed\tPriority\n\
                       /dev/zram0                               partition\t16654336\t\t100\t100\n\
                       /swapfile                                 file\t\t8388604\t\t0\t-2\n";
        assert_eq!(swap_areas(listing), 2, "zram counts like any other: it is swap that takes the pages");
        let without = "Filename\t\t\t\tType\tSize\tUsed\tPriority\n";
        assert_eq!(swap_areas(without), 0, "a machine with no swap is a machine QCode does not freeze on");
        assert_eq!(swap_areas(""), 0);
    }

    #[test]
    fn the_cpu_a_container_has_used_is_read_out_of_its_cgroups_cpu_stat() {
        let stat = "usage_usec 1234567\nuser_usec 1000000\nsystem_usec 234567\nnr_periods 0\nnr_throttled 0\n\
                    throttled_usec 0\n";
        assert_eq!(usage_micros(stat), Some(1_234_567));
        assert_eq!(usage_micros("user_usec 1\nusage_usec 42\n"), Some(42), "the line it names, wherever it is");
        assert_eq!(usage_micros("user_usec 1\n"), None, "a file without that line says nothing");
        assert_eq!(usage_micros("usage_usec many\n"), None, "and one that is not a number says nothing either");
    }

    #[test]
    fn only_podman_running_as_the_person_answers_yes_about_its_own_control_group() {
        let (saying, _folder) = stand_in("true");
        assert!(rootless(&Engine::new(EngineKind::Podman, &saying)), "podman, asked, says the cgroup is the person's");
        // Docker's daemon is root whoever started it, so it is not asked and is never a yes.
        assert!(!rootless(&Engine::new(EngineKind::Docker, &saying)));
        let (rootful, _folder) = stand_in("false");
        assert!(!rootless(&Engine::new(EngineKind::Podman, &rootful)), "a rootful podman's cgroup is not writable");
        assert!(
            !rootless(&Engine::new(EngineKind::Podman, "/nonexistent/qcode-no-podman")),
            "and one that cannot be asked"
        );
        assert!(!supported(&Engine::new(EngineKind::Docker, &saying)), "so this machine never freezes through docker");
    }

    /// A stand-in engine that answers the rootless question with `said`, in a folder of its own.
    fn stand_in(said: &str) -> (PathBuf, PathBuf) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let folder = std::env::temp_dir().join(format!("qcode-freeze-{stamp}"));
        std::fs::create_dir_all(&folder).expect("a folder");
        let binary = folder.join("podman");
        std::fs::write(&binary, format!("#!/bin/sh\nprintf '{said}\\n'\n")).expect("the stand-in engine");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
        (binary, folder)
    }
}
