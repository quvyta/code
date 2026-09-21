//! The page that follows a change of container engine: what the new engine lacks, what of the old
//! one comes along, and what stays behind.
//!
//! It opens in two ways and offers the same thing in both. The person chose the other engine in
//! the settings; or QCode started, the saved engine did not answer and the other one did
//! ([`crate::switch::offer_at_start`]). In the second case nothing has been switched yet, so the
//! page first asks whether to use the engine that answers; what it lists is the same.
//!
//! Nothing here happens without a press: every image is built, every home copied and every
//! container removed because the person asked for that one or for all of them. Homes are only
//! ever copied, never moved, and nothing asks to delete a volume at all.

use qframe::prelude::*;
use qframe::runtime::{Task, TaskId};
use qframe::widgets::{LogBuffer, LogLevel, LogLine, LogView, ScrollView, Spinner};

use crate::engine::{Engine, EngineKind, HostUser};
use crate::profile::{Profile, SafeName};
use crate::switch::{self, Survey};

use crate::ui::profiles::work::{self, Problem};
use crate::ui::settings::engine::{help, name};
use std::sync::Arc;

/// The page's width at most, like the settings page it is opened from.
const PAGE_WIDTH: u16 = 84;

/// Rows the build log gets while a build runs.
const LOG_ROWS: u16 = 8;

/// How many lines of a build the page keeps.
const LOG_LINES: usize = 2000;

/// Where a profile's image stands in the new engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Image {
    /// Not there; the build is offered.
    Missing,
    /// Waiting for its turn in a build of all of them.
    Queued,
    /// Being built now.
    Building,
    /// Built in this page.
    Built,
    /// The build failed, with the engine's words.
    Failed(String),
}

/// Where a volume of the old engine stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// Only in the old engine; the copy is offered.
    ToCopy,
    /// The new engine has one of that name already; copying over it is only done when asked.
    Taken,
    /// Being copied.
    Copying,
    /// Copied; it is in both engines now.
    Copied,
    /// The copy failed, with the engine's words.
    Failed(String),
}

/// Where the old engine's leftover containers stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leftover {
    /// Still there: how many, their names, and the bytes they hold.
    There(Vec<String>, u64),
    /// Being removed.
    Removing,
    /// Removed.
    Removed,
    /// The removal failed, with the engine's words.
    Failed(String),
}

/// How the page finds an engine: the engine layer's own search in the application, and a
/// stand-in engine in a test, so the whole page is walked without either engine installed.
#[derive(Clone)]
pub struct Finder(Arc<dyn Fn(EngineKind) -> Option<Engine> + Send + Sync>);

impl Finder {
    /// A finder that answers the way `find` does.
    #[must_use]
    pub fn new(find: impl Fn(EngineKind) -> Option<Engine> + Send + Sync + 'static) -> Self {
        Self(Arc::new(find))
    }

    /// The engine layer's own search: the engine when it is there and answers.
    #[must_use]
    pub fn detecting() -> Self {
        Self::new(|kind| crate::engine::detect(kind).ok())
    }
}

impl std::fmt::Debug for Finder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Finder")
    }
}

/// The page.
#[derive(Debug)]
pub struct Switch {
    from: EngineKind,
    to: EngineKind,
    /// The engines as the last look found them: `None` for one that did not answer.
    old: Option<Engine>,
    new: Option<Engine>,
    finder: Finder,
    /// True while the switch is only offered: QCode started, the saved engine did not answer
    /// and nothing has been changed yet.
    offered: bool,
    user: HostUser,
    profiles: Vec<Profile>,
    /// `None` until both engines have been asked.
    surveyed: Option<Surveyed>,
    /// Whether the last look found the new engine silent.
    silent: bool,
    log: LogBuffer,
    build: Option<TaskId>,
}

/// What the survey found, as the page keeps and changes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Surveyed {
    images: Vec<(SafeName, Image)>,
    /// `None` when the old engine does not answer.
    homes: Option<Vec<(String, Home)>>,
    leftover: Option<Leftover>,
}

/// What the page asks of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Use this engine from now on: the offer at start was taken.
    Use(EngineKind),
    /// Leave the page.
    Leave,
}

/// What happens on the page.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Both engines were looked for and asked.
    Surveyed(Box<Found>),
    /// Ask both engines again.
    Survey,
    /// Build the image of this profile in the new engine.
    Build(SafeName),
    /// Build every image the new engine lacks, one after the other.
    BuildAll,
    /// The build of this profile's image has begun.
    Building(SafeName),
    /// A line a build said.
    BuildLine(String),
    /// One profile's build ended: done, or the engine's words, or `None` when stopped.
    Built(SafeName, Result<(), Option<String>>),
    /// The builds asked for are over.
    BuildsOver,
    /// Copy this volume into the new engine.
    Copy(String),
    /// Copy every volume the new engine does not have yet.
    CopyAll,
    /// Ask before copying over the volume of this name the new engine already has.
    AskReplace(String),
    /// The person said yes: copy over it.
    Replace(String),
    /// A copy ended.
    Copied(String, Result<(), String>),
    /// Ask before removing QCode's containers from the old engine.
    AskRemove,
    /// The person said yes.
    Remove,
    /// The removal ended.
    Removed(Result<(), String>),
    /// Take the offer: use the engine that answers.
    Use,
    /// Leave the page.
    Done,
}

impl Switch {
    /// The page for a switch from `from` to `to`, finding both engines with `finder` each time it
    /// looks. `offered` is true for the offer at start, which changes nothing until it is taken.
    #[must_use]
    pub fn new(
        (from, to): (EngineKind, EngineKind),
        offered: bool,
        user: HostUser,
        profiles: Vec<Profile>,
        finder: Finder,
    ) -> Self {
        Self {
            from,
            to,
            old: None,
            new: None,
            finder,
            offered,
            user,
            profiles,
            surveyed: None,
            silent: false,
            log: LogBuffer::new(LOG_LINES),
            build: None,
        }
    }

    /// The engine switched from and the one switched to.
    #[must_use]
    pub fn engines(&self) -> (EngineKind, EngineKind) {
        (self.from, self.to)
    }

    /// Whether the switch is still only offered.
    #[must_use]
    pub fn offered(&self) -> bool {
        self.offered
    }

    /// The offer was taken and the application now uses the new engine.
    pub fn taken(&mut self) {
        self.offered = false;
    }
}

/// Finds both engines and asks them what the switch involves, off the render path. The page asks
/// this when it opens, and again when the person asks it to look again: an old engine that did
/// not answer may have been started since, and its homes can then be copied.
#[must_use]
pub fn survey(page: &Switch) -> Command<Msg> {
    let (finder, (from, to), profiles) = (page.finder.clone(), (page.from, page.to), page.profiles.clone());
    Command::perform(move || {
        let (old, new) = ((finder.0)(from), (finder.0)(to));
        let found = new.as_ref().map(|new| switch::survey(old.as_ref(), new, &profiles));
        Msg::Surveyed(Box::new(Found { old, new, survey: found }))
    })
}

/// What a look found: the two engines, and the survey when the new one answered.
#[derive(Debug, Clone)]
pub struct Found {
    old: Option<Engine>,
    new: Option<Engine>,
    survey: Option<Survey>,
}

/// Applies a message to the page, and says what the application has to do, if anything.
pub fn update(page: &mut Switch, message: Msg) -> (Command<Msg>, Option<Request>) {
    let command = match message {
        Msg::Surveyed(found) => {
            let Found { old, new, survey } = *found;
            (page.old, page.new) = (old, new);
            page.silent = survey.is_none();
            let Some(found) = survey else { return (Command::none(), None) };
            page.surveyed = Some(Surveyed {
                images: found.missing.into_iter().map(|name| (name, Image::Missing)).collect(),
                homes: found.volumes.map(|volumes| {
                    volumes
                        .into_iter()
                        .map(|volume| (volume.name, if volume.taken { Home::Taken } else { Home::ToCopy }))
                        .collect()
                }),
                leftover: found
                    .leftover
                    .filter(|left| !left.containers.is_empty())
                    .map(|left| Leftover::There(left.containers, left.bytes)),
            });
            Command::none()
        }
        Msg::Survey => {
            page.surveyed = None;
            survey(page)
        }
        Msg::Build(profile) => build(page, &[profile]),
        Msg::BuildAll => {
            let names: Vec<SafeName> =
                images(page).filter(|(_, image)| image_offered(image)).map(|(name, _)| name.clone()).collect();
            build(page, &names)
        }
        Msg::Building(profile) => {
            set_image(page, &profile, Image::Building);
            Command::none()
        }
        Msg::BuildLine(text) => {
            page.log.push(LogLine::new(LogLevel::Info, text));
            Command::none()
        }
        Msg::Built(profile, result) => {
            let state = match result {
                Ok(()) => Image::Built,
                Err(None) => Image::Missing,
                Err(Some(said)) => Image::Failed(said),
            };
            set_image(page, &profile, state);
            Command::none()
        }
        Msg::BuildsOver => {
            page.build = None;
            // Whatever was queued and did not get its turn is offered again.
            for (_, image) in images_mut(page) {
                if matches!(image, Image::Queued | Image::Building) {
                    *image = Image::Missing;
                }
            }
            Command::none()
        }
        Msg::Copy(volume) => copy(page, &volume, false),
        Msg::CopyAll => {
            let names: Vec<String> =
                homes(page).filter(|(_, home)| matches!(home, Home::ToCopy)).map(|(name, _)| name.clone()).collect();
            Command::batch(names.iter().map(|volume| copy(page, volume, false)).collect::<Vec<_>>())
        }
        Msg::AskReplace(volume) => {
            let shown = label(page, &volume);
            Command::confirm(
                Confirm::new(t!("switch.replace-title", home = shown.clone()), Msg::Replace(volume))
                    .message(t!("switch.replace-text", home = shown, engine = name(page.to), from = name(page.from)))
                    .confirm_label(t!("switch.replace-do"))
                    .danger(),
            )
        }
        Msg::Replace(volume) => copy(page, &volume, true),
        Msg::Copied(volume, result) => {
            let state = match result {
                Ok(()) => Home::Copied,
                Err(said) => Home::Failed(said),
            };
            if let Some(home) = homes_mut(page).find(|(name, _)| *name == volume).map(|(_, home)| home) {
                *home = state;
            }
            Command::none()
        }
        Msg::AskRemove => {
            let Some(Leftover::There(containers, _)) = leftover(page) else { return (Command::none(), None) };
            let count = containers.len();
            Command::confirm(
                Confirm::new(t!("switch.remove-title", engine = name(page.from)), Msg::Remove)
                    .message(t!("switch.remove-text", n = count, engine = name(page.from)))
                    .confirm_label(t!("switch.remove-do"))
                    .danger(),
            )
        }
        Msg::Remove => remove(page),
        Msg::Removed(result) => {
            if let Some(surveyed) = page.surveyed.as_mut() {
                surveyed.leftover = Some(match result {
                    Ok(()) => Leftover::Removed,
                    Err(said) => Leftover::Failed(said),
                });
            }
            Command::none()
        }
        Msg::Use => return (Command::none(), Some(Request::Use(page.to))),
        Msg::Done => {
            let stop = page.build.take().map_or_else(Command::none, Command::cancel_task);
            return (stop, Some(Request::Leave));
        }
    };
    (command, None)
}

fn images(page: &Switch) -> impl Iterator<Item = &(SafeName, Image)> {
    page.surveyed.iter().flat_map(|surveyed| surveyed.images.iter())
}

fn images_mut(page: &mut Switch) -> impl Iterator<Item = &mut (SafeName, Image)> {
    page.surveyed.iter_mut().flat_map(|surveyed| surveyed.images.iter_mut())
}

fn homes(page: &Switch) -> impl Iterator<Item = &(String, Home)> {
    page.surveyed.iter().flat_map(|surveyed| surveyed.homes.iter().flatten())
}

fn homes_mut(page: &mut Switch) -> impl Iterator<Item = &mut (String, Home)> {
    page.surveyed.iter_mut().flat_map(|surveyed| surveyed.homes.iter_mut().flatten())
}

fn leftover(page: &Switch) -> Option<&Leftover> {
    page.surveyed.as_ref().and_then(|surveyed| surveyed.leftover.as_ref())
}

fn set_image(page: &mut Switch, profile: &SafeName, state: Image) {
    if let Some(image) = images_mut(page).find(|(name, _)| name == profile).map(|(_, image)| image) {
        *image = state;
    }
}

/// Whether an image is one the page offers to build: missing, or failed and worth another try.
fn image_offered(image: &Image) -> bool {
    matches!(image, Image::Missing | Image::Failed(_))
}

/// Builds the images of `names` in the new engine, one after the other, each the whole build the
/// profile wizard runs, with every line in the page's log.
fn build(page: &mut Switch, names: &[SafeName]) -> Command<Msg> {
    if page.build.is_some() || names.is_empty() {
        return Command::none();
    }
    let Some(engine) = page.new.clone() else { return Command::none() };
    let chosen: Vec<Profile> =
        names.iter().filter_map(|name| page.profiles.iter().find(|profile| &profile.name == name).cloned()).collect();
    for name in names {
        set_image(page, name, Image::Queued);
    }
    page.log.clear();
    let task = Task::new(t!("switch.building"), move |cx| {
        for profile in chosen {
            if cx.is_cancelled() {
                break;
            }
            cx.send(Msg::Building(profile.name.clone()));
            cx.send(Msg::BuildLine(t!("switch.building-one", profile = profile.name.as_str())));
            let cancel = || cx.is_cancelled();
            let mut line = |text: &str| cx.send(Msg::BuildLine(text.to_owned()));
            let built = work::build_whole(&engine, &profile, &cancel, &mut || {}, &mut line);
            let result = built.map_err(|problem| match problem {
                Problem::Cancelled => None,
                other => Some(other.output().unwrap_or_default().to_owned()),
            });
            cx.send(Msg::Built(profile.name.clone(), result));
        }
        Ok(Msg::BuildsOver)
    });
    page.build = Some(task.id());
    Command::task(task)
}

/// Copies `volume` from the old engine into the new one, off the render path.
fn copy(page: &mut Switch, volume: &str, replace: bool) -> Command<Msg> {
    let (Some(old), Some(new)) = (page.old.clone(), page.new.clone()) else { return Command::none() };
    if let Some(home) = homes_mut(page).find(|(name, _)| name == volume).map(|(_, home)| home) {
        // A volume the new engine has is only copied over once the person said so.
        if *home == Home::Taken && !replace {
            return Command::none();
        }
        *home = Home::Copying;
    }
    let (user, volume) = (page.user, volume.to_owned());
    Command::perform(move || {
        let result = switch::copy(&old, &new, &volume, user, replace);
        Msg::Copied(volume, result)
    })
}

/// Removes QCode's containers from the old engine, off the render path. Their volumes stay.
fn remove(page: &mut Switch) -> Command<Msg> {
    let Some(old) = page.old.clone() else { return Command::none() };
    let Some(surveyed) = page.surveyed.as_mut() else { return Command::none() };
    let Some(Leftover::There(containers, _)) = surveyed.leftover.clone() else { return Command::none() };
    surveyed.leftover = Some(Leftover::Removing);
    Command::perform(move || Msg::Removed(switch::remove(&old, &containers)))
}

/// A volume in the person's words: whose home or whose login it is. The names are QCode's own
/// (`qcode-home-<workspace>-<profile>`, `qcode-cred-<profile>`); a dash may stand inside a
/// workspace's id as well as between it and the profile, so the profile is found among the ones
/// the store has, and a volume of a profile the store no longer has keeps its own name.
fn label(page: &Switch, volume: &str) -> String {
    if let Some(profile) = volume.strip_prefix("qcode-cred-") {
        return t!("switch.login-of", profile = profile);
    }
    let Some(rest) = volume.strip_prefix("qcode-home-") else { return volume.to_owned() };
    page.profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .filter_map(|profile| {
            let workspace = rest.strip_suffix(profile)?.strip_suffix('-')?;
            (!workspace.is_empty()).then_some((workspace, profile))
        })
        .max_by_key(|(_, profile)| profile.len())
        .map_or_else(
            || volume.to_owned(),
            |(workspace, profile)| t!("switch.home-of", profile = profile, workspace = workspace),
        )
}

/// The control the page hands the keyboard to when it opens.
#[must_use]
pub fn entry(page: &Switch) -> &'static str {
    if page.offered { "switch-use" } else { "switch-done" }
}

/// The keys of the page that are not in the keymap.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    vec![(icons.glyph("enter").into_owned(), t!("hints.open"))]
}

/// Draws the page.
pub fn view(page: &Switch, ui: &mut View<'_, Msg>) {
    let side = (ui.size().width.saturating_sub(PAGE_WIDTH) / 2).max(2);
    let (from, to) = (name(page.from), name(page.to));
    ui.add_with(ScrollView::new(), |ui| {
        ui.row(|ui| {
            ui.column(|ui| {
                ui.add(Text::new(t!("switch.title", from = from.clone(), to = to.clone())).bold()).fill_width();
                if page.offered {
                    ui.add(Text::new(t!("switch.offer", from = from.clone(), to = to.clone()))).fill_width();
                    ui.add(Text::new(t!("switch.offer-nothing-yet")).role("secondary")).fill_width();
                    ui.add(Button::new(t!("switch.use", engine = to.clone())).variant("primary").on_press(Msg::Use))
                        .id("switch-use");
                }
                match &page.surveyed {
                    None if page.silent => {
                        ui.add(Text::new(t!("switch.new-silent", engine = to.clone())).color("warning")).fill_width();
                    }
                    None => {
                        ui.add(Spinner::new().label(t!("switch.looking")));
                    }
                    Some(surveyed) => {
                        images_block(page, surveyed, ui);
                        homes_block(page, surveyed, ui);
                        leftover_block(page, surveyed, ui);
                    }
                }
                ui.row(|ui| {
                    ui.add(Button::new(t!("switch.again")).on_press(Msg::Survey)).id("switch-again");
                    ui.spacer();
                    ui.add(Button::new(t!("switch.done")).on_press(Msg::Done)).id("switch-done");
                })
                .fill_width();
            })
            .fill_width()
            .gap(1)
            .padding(Padding { top: 1, bottom: 1, ..Padding::default() })
            .id("switch-page");
        })
        .fill_width()
        // The page is narrowed by its margins rather than given a width of its own: a column of
        // a set width is measured as if its sentences did not wrap, and its last rows fall off.
        .padding(Padding { left: side, right: side, ..Padding::default() });
    })
    .fill();
}

/// The profiles whose image the new engine lacks, each with its build, and the log while one runs.
fn images_block(page: &Switch, surveyed: &Surveyed, ui: &mut View<'_, Msg>) {
    let to = name(page.to);
    ui.add(Text::new(t!("switch.images")).role("secondary").bold());
    if surveyed.images.is_empty() {
        ui.add(Text::new(t!("switch.images-none", engine = to))).fill_width();
        return;
    }
    ui.add(Text::new(t!("switch.images-missing", n = surveyed.images.len(), engine = to))).fill_width();
    let busy = page.build.is_some();
    for (profile, image) in &surveyed.images {
        ui.row(|ui| {
            ui.add(Text::new(profile.as_str().to_owned())).width(Length::Cells(28));
            match image {
                Image::Missing | Image::Failed(_) => {
                    let label =
                        if matches!(image, Image::Failed(_)) { t!("switch.build-again") } else { t!("switch.build") };
                    ui.add(Button::new(label).disabled(busy).on_press(Msg::Build(profile.clone())))
                        .id(format!("switch-build-{profile}"));
                }
                Image::Queued => {
                    ui.add(Text::new(t!("switch.queued")).role("faint"));
                }
                Image::Building => {
                    ui.add(Spinner::new().label(t!("switch.building")));
                }
                Image::Built => {
                    ui.add(Text::new(t!("switch.built")).color("success"));
                }
            }
        })
        .gap(2)
        .fill_width();
        if let Image::Failed(said) = image {
            failure(page, said, ui);
        }
    }
    if surveyed.images.iter().filter(|(_, image)| image_offered(image)).count() > 1 {
        ui.add(Button::new(t!("switch.build-all")).variant("primary").disabled(busy).on_press(Msg::BuildAll))
            .id("switch-build-all");
    }
    if busy || !page.log.is_empty() {
        ui.add(LogView::new(&page.log)).fill_width().height(Length::Cells(LOG_ROWS)).id("switch-log");
    }
}

/// The homes and logins of the old engine, each with its copy.
fn homes_block(page: &Switch, surveyed: &Surveyed, ui: &mut View<'_, Msg>) {
    let (from, to) = (name(page.from), name(page.to));
    ui.add(Text::new(t!("switch.homes")).role("secondary").bold());
    let Some(homes) = &surveyed.homes else {
        ui.add(Text::new(t!("switch.old-silent", engine = from)).role("secondary")).fill_width();
        return;
    };
    if homes.is_empty() {
        ui.add(Text::new(t!("switch.homes-none", engine = from))).fill_width();
        return;
    }
    ui.add(Text::new(t!("switch.homes-there", engine = from.clone(), to = to.clone()))).fill_width();
    for (volume, home) in homes {
        ui.row(|ui| {
            ui.add(Text::new(label(page, volume))).width(Length::Cells(40));
            match home {
                Home::ToCopy | Home::Failed(_) => {
                    ui.add(Button::new(t!("switch.copy")).on_press(Msg::Copy(volume.clone())))
                        .id(format!("switch-copy-{volume}"));
                }
                Home::Taken => {
                    ui.add(Text::new(t!("switch.taken", engine = to.clone())).role("faint"));
                    ui.add(Button::new(t!("switch.replace")).on_press(Msg::AskReplace(volume.clone())))
                        .id(format!("switch-replace-{volume}"));
                }
                Home::Copying => {
                    ui.add(Spinner::new().label(t!("switch.copying")));
                }
                Home::Copied => {
                    ui.add(Text::new(t!("switch.copied", engine = from.clone())).color("success"));
                }
            }
        })
        .gap(2)
        .fill_width();
        if let Home::Failed(said) = home {
            failure(page, said, ui);
        }
    }
    if homes.iter().filter(|(_, home)| matches!(home, Home::ToCopy)).count() > 1 {
        ui.add(Button::new(t!("switch.copy-all")).variant("primary").on_press(Msg::CopyAll)).id("switch-copy-all");
    }
}

/// QCode's containers still in the old engine, and the offer to remove them.
fn leftover_block(page: &Switch, surveyed: &Surveyed, ui: &mut View<'_, Msg>) {
    let from = name(page.from);
    let Some(left) = &surveyed.leftover else { return };
    ui.add(Text::new(t!("switch.leftover", engine = from.clone())).role("secondary").bold());
    match left {
        Leftover::There(containers, bytes) => {
            let size = crate::ui::workspace::bytes(*bytes);
            ui.add(Text::new(t!("switch.leftover-there", n = containers.len(), size = size, engine = from.clone())))
                .fill_width();
            ui.add(Button::new(t!("switch.remove", n = containers.len(), engine = from)).on_press(Msg::AskRemove))
                .id("switch-remove");
        }
        Leftover::Removing => {
            ui.add(Spinner::new().label(t!("switch.removing")));
        }
        Leftover::Removed => {
            ui.add(Text::new(t!("switch.removed", engine = from)).color("success")).fill_width();
        }
        Leftover::Failed(said) => failure(page, said, ui),
    }
}

/// A failure under its row: the plain sentence when QCode recognises it, and the engine's words.
fn failure(page: &Switch, said: &str, ui: &mut View<'_, Msg>) {
    if let Some(help) = help(page.to, said) {
        help.show(ui);
    }
    ui.add(Text::new(said.to_owned()).role("faint")).fill_width();
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use qframe::env::{AssetDirs, Env};
    use qframe::icons::GlyphMode;
    use qframe::prelude::*;
    use qframe::runtime::Harness;

    use super::{Finder, Msg, Request, Switch, update, view};
    use crate::engine::{Engine, EngineKind, HostUser};
    use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, SafeName, Template};

    /// The page on its own, keeping what it asked of the application.
    struct Page {
        page: Switch,
        asked: Vec<Request>,
    }

    impl App for Page {
        type Msg = Msg;

        fn init(&mut self) -> Command<Msg> {
            super::survey(&self.page)
        }

        fn update(&mut self, message: Msg) -> Command<Msg> {
            let (command, request) = update(&mut self.page, message);
            self.asked.extend(request);
            command
        }

        fn view(&self, ui: &mut View<'_, Msg>) {
            view(&self.page, ui);
        }
    }

    /// Two stand-in engines in a folder of the test's own, writing every call they are given
    /// into one list. The old one, podman, holds a home, a login, a volume of someone else's and
    /// two containers of QCode's; the new one, docker, has the base image and no profile image
    /// until one is built, and keeps what a copy writes into it.
    struct Engines {
        folder: PathBuf,
    }

    impl Engines {
        fn new(name: &str, docker_has_home: bool) -> Self {
            let folder = std::env::temp_dir().join(format!("qcode-switch-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).expect("a folder");
            let log = folder.join("calls");
            let common = format!(
                "#!/bin/sh\nprintf '%s %s\\n' \"$(basename \"$0\")\" \"$*\" >> {log}\nfor last; do :; done\n",
                log = log.display()
            );
            let podman = format!(
                "{common}\
                 case \"$1 $2\" in\n\
                 'volume ls') printf 'qcode-home-ads-claude-code\\nqcode-cred-claude-code\\nqcode-leadvol\\n'; exit 0 ;;\n\
                 'ps --all') printf 'qcode-ads-claude-code\\nqcode-ads-base\\n'; exit 0 ;;\n\
                 'container inspect') echo 1500000; exit 0 ;;\n\
                 'image inspect') echo 0123; exit 0 ;;\n\
                 esac\n\
                 [ \"$1\" = run ] && {{ printf 'HOME-ARCHIVE'; exit 0; }}\n\
                 exit 0\n"
            );
            let docker = format!(
                "{common}\
                 case \"$1 $2\" in\n\
                 'volume ls') {listed}; exit 0 ;;\n\
                 'image inspect') case \"$last\" in qcode/base) echo 0123; exit 0 ;; esac;\n\
                   [ -e {folder}/built-$(basename \"$last\") ] && {{ echo sha256:1; exit 0; }};\n\
                   echo \"Error response from daemon: No such image: $last\" >&2; exit 1 ;;\n\
                 esac\n\
                 [ \"$1\" = build ] && {{ printf 'Sending build context to Docker daemon  2.048kB\\r\\r\\nStep 1/2 : FROM qcode/base\\n'; touch {folder}/built-$(basename \"$3\"); exit 0; }}\n\
                 [ \"$1 $3\" = 'run --interactive' ] && {{ cat > {folder}/received; exit 0; }}\n\
                 exit 0\n",
                listed = if docker_has_home { "echo qcode-home-ads-claude-code" } else { "true" },
                folder = folder.display(),
            );
            for (name, script) in [("podman", podman), ("docker", docker)] {
                let path = folder.join(name);
                std::fs::write(&path, script).expect("a stand-in engine");
                std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("runnable");
            }
            Self { folder }
        }

        fn finder(&self) -> Finder {
            let folder = self.folder.clone();
            Finder::new(move |kind| Some(Engine::new(kind, folder.join(kind.name()))))
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.folder.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
        }

        fn received(&self) -> Option<String> {
            std::fs::read_to_string(self.folder.join("received")).ok()
        }
    }

    impl Drop for Engines {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }

    fn profile(name: &str) -> Profile {
        Profile {
            name: SafeName::parse(name).expect("a usable name"),
            harness: HarnessKind::ClaudeCode,
            template: Template::Base,
            account: AccountKind::Subscription,
            provider: None,
            assets: MountAccess::ReadOnly,
            network: NetworkMode::Full,
            without: Vec::new(),
            os: crate::base::Os::Debian,
        }
    }

    /// Where `button` stands on the row that starts with `row`, as a cell to click.
    fn on_row(harness: &Harness<Page>, row: &str, button: &str) -> (i32, i32) {
        let screen = harness.screen();
        let (y, line) =
            screen.lines().enumerate().find(|(_, line)| line.trim_start().starts_with(row)).expect("the row");
        let start = line.find(row).map_or(0, |at| at + row.len());
        let at = line[start..].find(button).map(|at| at + start).expect("the button on that row");
        let x = line[..at].chars().count();
        (i32::try_from(x).unwrap_or(0) + 1, i32::try_from(y).unwrap_or(0))
    }

    /// Clicks the last `text` on screen, which is a dialog's own button when a dialog is open
    /// over a page that says the same word.
    fn click_last(harness: &mut Harness<Page>, text: &str) {
        let screen = harness.screen();
        let (y, line) = screen.lines().enumerate().filter(|(_, line)| line.contains(text)).last().expect("on screen");
        let x = line.rfind(text).map_or(0, |at| line[..at].chars().count());
        harness.click(i32::try_from(x).unwrap_or(0) + 1, i32::try_from(y).unwrap_or(0)).render();
    }

    fn env() -> Env {
        Env::load(&AssetDirs { locale_sources: crate::locales(), ..AssetDirs::default() }).expect("the built-in files")
    }

    fn harness(engines: &Engines, offered: bool, finder: Finder) -> Harness<Page> {
        let profiles = vec![profile("claude-code"), profile("codex")];
        let page = Switch::new(
            (EngineKind::Podman, EngineKind::Docker),
            offered,
            HostUser::Ids { uid: 1000, gid: 1000 },
            profiles,
            finder,
        );
        let _ = engines;
        let mut harness = Harness::with_env(Page { page, asked: Vec::new() }, env(), 110, 80);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        harness.render();
        harness
    }

    #[test]
    fn the_new_engine_is_found_missing_the_images_and_each_is_built_from_its_own_button() {
        let engines = Engines::new("images", false);
        let mut harness = harness(&engines, false, engines.finder());
        let screen = harness.screen();
        assert!(screen.contains("Moving from Podman to Docker"), "{screen}");
        assert!(screen.contains("Docker does not have the images of these 2 profiles"), "{screen}");
        assert!(screen.contains("Build all"), "{screen}");
        let (x, y) = on_row(&harness, "claude-code", "Build");
        harness.click(x, y).render();
        let calls = engines.calls();
        assert!(
            calls.iter().any(|call| call.starts_with("docker build --tag qcode/profile/claude-code")),
            "{calls:#?}"
        );
        assert!(!calls.iter().any(|call| call.contains("qcode/profile/codex") && call.contains("build")), "{calls:#?}");
        let screen = harness.screen();
        assert!(screen.contains("Built"), "{screen}");
        // Docker's own first line, which it ends in two carriage returns, is in the log.
        assert!(screen.contains("Sending build context to Docker daemon  2.048kB"), "{screen}");
        let (x, y) = on_row(&harness, "codex", "Build");
        harness.click(x, y).render();
        assert!(engines.calls().iter().any(|call| call.starts_with("docker build --tag qcode/profile/codex")));
    }

    #[test]
    fn homes_and_logins_are_listed_by_whose_they_are_and_copied_through_a_pipe() {
        let engines = Engines::new("copy", false);
        let mut harness = harness(&engines, false, engines.finder());
        let screen = harness.screen();
        assert!(screen.contains("claude-code in ads"), "{screen}");
        assert!(screen.contains("Login of claude-code"), "{screen}");
        assert!(!screen.contains("qcode-leadvol"), "a volume that is not QCode's is not offered:\n{screen}");
        let (x, y) = on_row(&harness, "claude-code in ads", "Copy");
        harness.click(x, y).render();
        let calls = engines.calls();
        let packed =
            calls.iter().find(|call| call.starts_with("podman run --rm") && call.contains("tar -C /home/qcode -cf -"));
        let unpacked =
            calls.iter().find(|call| call.starts_with("docker run --rm --interactive") && call.contains("-xf -"));
        assert!(packed.is_some_and(|call| call.contains("qcode-home-ads-claude-code:/home/qcode:ro")), "{calls:#?}");
        assert!(unpacked.is_some_and(|call| call.contains("qcode-home-ads-claude-code:/home/qcode:rw")), "{calls:#?}");
        assert_eq!(engines.received().as_deref(), Some("HOME-ARCHIVE"), "what one side packed, the other received");
        assert!(!calls.iter().any(|call| call.starts_with("podman volume rm")), "the old volume is never removed");
        assert!(harness.screen().contains("Copied, and still in Podman"), "{}", harness.screen());
    }

    #[test]
    fn a_home_the_new_engine_already_has_is_only_replaced_after_asking() {
        let engines = Engines::new("taken", true);
        let mut harness = harness(&engines, false, engines.finder());
        let screen = harness.screen();
        assert!(screen.contains("already in Docker"), "{screen}");
        harness.click_text("Replace").render();
        assert!(harness.screen().contains("Replace claude-code in ads?"), "it asks first:\n{}", harness.screen());
        assert!(!engines.calls().iter().any(|call| call.starts_with("docker volume rm")), "nothing yet");
        // The dialog's own button, the last "Replace" on screen.
        click_last(&mut harness, "Replace");
        let calls = engines.calls();
        assert!(calls.iter().any(|call| call == "docker volume rm qcode-home-ads-claude-code"), "{calls:#?}");
        assert!(!calls.iter().any(|call| call.starts_with("podman volume rm")), "{calls:#?}");
    }

    #[test]
    fn the_containers_left_in_the_old_engine_are_counted_and_only_removed_when_asked() {
        let engines = Engines::new("leftover", false);
        let mut harness = harness(&engines, false, engines.finder());
        let screen = harness.screen();
        assert!(screen.contains("2 QCode containers are still in Podman, holding 3.0 MB"), "{screen}");
        assert!(!engines.calls().iter().any(|call| call.starts_with("podman rm")), "nothing is removed by itself");
        harness.click_text("Remove them from Podman").render();
        assert!(harness.screen().contains("Remove QCode's containers from Podman?"), "{}", harness.screen());
        let calls_before = engines.calls().len();
        click_last(&mut harness, "Remove");
        let calls: Vec<String> = engines.calls().split_off(calls_before);
        assert!(calls.iter().any(|call| call.contains("rm") && call.ends_with("qcode-ads-claude-code")), "{calls:#?}");
        assert!(!calls.iter().any(|call| call.contains("volume rm")), "volumes stay: {calls:#?}");
    }

    #[test]
    fn at_start_the_switch_is_only_offered_and_an_old_engine_that_does_not_answer_is_said() {
        let engines = Engines::new("offer", false);
        let folder = engines.folder.clone();
        // Podman does not answer: nothing can be read from it, and only docker is found.
        let finder =
            Finder::new(move |kind| (kind == EngineKind::Docker).then(|| Engine::new(kind, folder.join("docker"))));
        let mut harness = harness(&engines, true, finder);
        let screen = harness.screen();
        assert!(screen.contains("Podman does not answer, and Docker does."), "{screen}");
        assert!(screen.contains("Use Docker"), "{screen}");
        assert!(screen.contains("Docker does not have the images"), "the same offer is listed:\n{screen}");
        assert!(screen.contains("kept in it cannot be"), "{screen}");
        harness.click_text("Use Docker").render();
        assert_eq!(harness.app().asked, [Request::Use(EngineKind::Docker)]);
    }

    #[test]
    fn a_volume_label_names_whose_it_is_even_when_dashes_are_in_both_names() {
        let page = Switch::new(
            (EngineKind::Podman, EngineKind::Docker),
            false,
            HostUser::ImageDefault,
            vec![profile("claude-code"), profile("code")],
            Finder::new(|_| None),
        );
        crate::ui::settings::testing::translated("en", || {
            assert_eq!(super::label(&page, "qcode-home-my-app-claude-code"), "claude-code in my-app");
            assert_eq!(super::label(&page, "qcode-cred-claude-code"), "Login of claude-code");
            assert_eq!(super::label(&page, "qcode-home-x-gone"), "qcode-home-x-gone", "a profile nobody has any more");
        });
        let _ = Path::new("");
    }
}
