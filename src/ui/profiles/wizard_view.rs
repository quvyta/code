//! The wizard's pages as they are drawn: the steps, each choice a profile is made of, the build
//! of its image and the sign-in, in a terminal or in a window of its own.

use qframe::prelude::*;
use qframe::widgets::{
    Field, LogView, RadioGroup, SettingRow, SettingsList, ShimmerText, Switch, Terminal, TextInput, Wizard,
};

use crate::base::{Gap, Os, Refusal};
use crate::desktop::callback;
use crate::engine::Engine;
use crate::profile::{
    ASSUMED_CONTEXT_TOKENS, AccountKind, Extra, HarnessKind, MountAccess, NetworkMode, Pick, SafeName, Template,
};
use crate::ui::settings::engine::help;

use super::list::{access_word, account_word, network_word, no_login_detail, template_word};
use super::{
    Blocked, Build, CHROME_ROWS, Draft, Login, Msg, PAGE_WIDTH, Page, PickRow, Problem, Profiles, Stage, Unfinished,
    VIEWPORT_ROWS, WIZARD_ROWS, WindowBack, WindowLogin, shell_page,
};

/// Says plainly what an engine refusal QCode recognises means and what puts it right, above the
/// engine's own words, which the log or the line below keeps as they were.
fn recognised(state: &Profiles, problem: &Problem, ui: &mut View<'_, Msg>) {
    let kind = state.engine.as_ref().map(Engine::kind);
    if let Some(help) = kind.zip(problem.output()).and_then(|(kind, output)| help(kind, output)) {
        help.show(ui);
    }
}

/// The wizard, or the login page on its own when that is all the draft is for, in the middle of
/// the screen.
///
/// The column is centred across, and pushed down by half of what the terminal has beyond the
/// tallest page rather than centred on the page being shown: pages differ in height, and a wizard
/// centred on each one would move its steps up and down on every Next. A terminal too short for
/// the tallest page gets the wizard at the top, where it was, so nothing is pushed off the screen.
pub(super) fn draw_wizard(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let top = ui.size().height.saturating_sub(WIZARD_ROWS + CHROME_ROWS) / 2;
    ui.column(|ui| {
        ui.spacer().height(Length::Cells(top));
        if draft.is_only_login() {
            draw_sign_in(state, draft, ui);
        } else if draft.is_only_rebuild() {
            draw_rebuild(state, draft, ui);
        } else {
            draw_steps(state, draft, ui);
        }
    })
    .fill()
    .align(Align::Center);
}

/// The login page on its own, for a profile that exists already.
fn draw_sign_in(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.add(Text::new(t!("profiles.wizard.sign-in-title", name = draft.name.as_str())).bold());
        draw_login(state, draft, ui);
        ui.row(|ui| {
            ui.spacer();
            ui.add(Button::new(t!("profiles.wizard.close")).variant("primary").on_press(Msg::Finish)).id("login-close");
        })
        .fill_width();
    })
    .gap(1)
    .width(Length::Cells(PAGE_WIDTH));
}

/// The image page on its own, building an existing profile's image again. Closing it while the
/// build runs stops the build, which leaves the image that was there.
fn draw_rebuild(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.add(Text::new(t!("profiles.rebuild.heading", name = draft.name.as_str())).bold());
        draw_image(state, draft, ui);
        ui.row(|ui| {
            ui.spacer();
            ui.add(Button::new(t!("profiles.wizard.close")).variant("primary").on_press(Msg::Cancel))
                .id("rebuild-close");
        })
        .fill_width();
    })
    .gap(1)
    .width(Length::Cells(PAGE_WIDTH));
}

/// The whole wizard, one page at a time.
fn draw_steps(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let labels = draft.stages().iter().map(|stage| step_word(*stage));
    let wizard = Wizard::new(labels)
        .current(draft.stage.index())
        .busy(draft.is_busy())
        .on_next(Msg::Next)
        .on_back(Msg::Back)
        .on_cancel(Msg::Cancel)
        .on_finish(Msg::Finish)
        .on_step(Msg::Step);
    wizard
        .show(ui, |ui| {
            match draft.stage {
                Stage::Harness => draw_harness(draft, ui),
                Stage::System => draw_system(draft, ui),
                Stage::Template => draw_template(draft, ui),
                Stage::Account => draw_account(draft, ui),
                Stage::Permissions => draw_permissions(draft, ui),
                Stage::Image => draw_image(state, draft, ui),
                Stage::Login => draw_login(state, draft, ui),
            }
            // On the last page, once there is an image: the profile is made, and this is where
            // the person may still change it by hand before any workspace copies it.
            let last = draft.stages().last() == Some(&draft.stage);
            let signing = draft.login.is_busy() || matches!(draft.login, Login::Running { .. } | Login::Window(_));
            if last && draft.build == Build::Done && !signing {
                shell_page::offer(state, ui);
            }
        })
        .width(Length::Cells(PAGE_WIDTH));
}

/// The name of one step.
fn step_word(stage: Stage) -> String {
    t!(match stage {
        Stage::Harness => "profiles.wizard.step-harness",
        Stage::System => "profiles.wizard.step-system",
        Stage::Template => "profiles.wizard.step-template",
        Stage::Account => "profiles.wizard.step-account",
        Stage::Permissions => "profiles.wizard.step-permissions",
        Stage::Image => "profiles.wizard.step-image",
        Stage::Login => "profiles.wizard.step-login",
    })
}

/// Which harness, and what the profile is called.
fn draw_harness(draft: &Draft, ui: &mut View<'_, Msg>) {
    let error = match draft.blocked() {
        Some(Blocked::NameEmpty) => Some(t!("profiles.wizard.name-empty")),
        Some(Blocked::NameTaken) => Some(t!("profiles.wizard.name-taken")),
        _ => None,
    };
    let offered = draft.harnesses();
    let chosen = offered.iter().position(|harness| *harness == draft.harness);
    ui.add(Text::new(t!("profiles.wizard.harness-lead")).role("secondary")).fill_width();
    let group = RadioGroup::new(offered.iter().map(|harness| harness.record().display_name.to_owned()))
        .selected(chosen)
        .on_select(Msg::PickHarness);
    ui.add(group.wrap(true)).id("profile-harness");
    // A person who went back from the permissions page after turning the network off learns why
    // a harness is missing from the list, rather than wondering where it went.
    for harness in HarnessKind::ALL.into_iter().filter(|harness| !offered.contains(harness)) {
        let said = t!("profiles.wizard.harness-needs-network", harness = harness.record().display_name);
        ui.add(Text::new(said).role("secondary")).fill_width();
    }
    // A person who went back from the system page to change the harness learns at once that the
    // system they chose there cannot run this one, not only when they reach it again.
    if let Some(refusal) = draft.os.refuses(draft.harness) {
        ui.add(Text::new(refusal_line(refusal, draft)).color("warning")).fill_width();
    }
    // A profile being changed keeps its name, and the page says why in one quiet line rather than
    // offering a field that would not take.
    if draft.is_editing() {
        ui.add(Text::new(t!("profiles.wizard.name")).bold());
        ui.add(Text::new(draft.name.clone()));
        ui.add(Text::new(t!("profiles.edit.name-fixed")).role("secondary")).fill_width();
        return;
    }
    ui.add_with(
        Field::new(t!("profiles.wizard.name"))
            .hint(t!("profiles.wizard.name-hint"))
            .error(error.clone())
            .required(true),
        |ui| {
            ui.add(
                TextInput::new(draft.name.clone())
                    .invalid(error.is_some())
                    .max_length(SafeName::MAX_LENGTH)
                    .on_change(Msg::Name)
                    // The name is the page's last question, so Enter after it goes on.
                    .on_submit(|_| Msg::Next),
            )
            .id("profile-name")
            .fill_width();
        },
    )
    .fill_width();
    if let Some(safe) = draft.safe_name()
        && safe.as_str() != draft.name
    {
        ui.add(Text::new(t!("profiles.wizard.name-folded", name = safe.as_str())).role("secondary"));
    }
}

/// Which operating system the image is built on: each with the size its base image was measured
/// at, Alpine with the words that it is not recommended, and under the choice what that system
/// means, what its image does not carry, and — when it cannot run the chosen harness — why, in the
/// colour of a problem, with the page held until something else is chosen.
fn draw_system(draft: &Draft, ui: &mut View<'_, Msg>) {
    let chosen = Os::ALL.iter().position(|os| *os == draft.os);
    ui.add(Text::new(t!("profiles.wizard.system-lead")).role("secondary")).fill_width();
    let group = RadioGroup::new(Os::ALL.map(system_option)).selected(chosen).on_select(Msg::PickOs);
    ui.add(group.wrap(true)).id("profile-system");
    let detail = t!(match draft.os {
        Os::Debian => "profiles.wizard.system-debian-detail",
        Os::Arch => "profiles.wizard.system-arch-detail",
        Os::Ubuntu => "profiles.wizard.system-ubuntu-detail",
        Os::Alpine => "profiles.wizard.system-alpine-detail",
    });
    ui.add(Text::new(detail).role("secondary")).fill_width();
    for gap in draft.os.gaps() {
        let said = t!(match gap {
            Gap::Docx2txt => "profiles.wizard.system-gap-docx2txt",
            Gap::Sox => "profiles.wizard.system-gap-sox",
            Gap::SoxOpus => "profiles.wizard.system-gap-sox-opus",
        });
        ui.add(Text::new(said).color("warning")).fill_width();
    }
    if let Some(refusal) = draft.os.refuses(draft.harness) {
        ui.add(Text::new(refusal_line(refusal, draft)).color("danger")).fill_width();
    }
}

/// One system as the wizard offers it: its name, how large its image is, and for Alpine that it is
/// not recommended.
fn system_option(os: Os) -> String {
    let key = if os.recommended() {
        "profiles.wizard.system-option"
    } else {
        "profiles.wizard.system-option-not-recommended"
    };
    t!(key, name = os.display_name(), mb = os.image_mb().to_string())
}

/// Why the chosen system does not run the chosen harness, in words.
fn refusal_line(refusal: Refusal, draft: &Draft) -> String {
    let key = match refusal {
        Refusal::TerminalLibrary => "profiles.wizard.system-refused-terminal",
        Refusal::GlibcProgram => "profiles.wizard.system-refused-glibc",
        Refusal::WindowOnDebianOnly => "profiles.wizard.system-refused-window",
    };
    t!(key, harness = draft.harness.record().display_name, system = draft.os.display_name())
}

/// A ready-made set of parts, and then the list of every part, each with a switch.
fn draw_template(draft: &Draft, ui: &mut View<'_, Msg>) {
    let offered = Template::offered(draft.harness);
    let chosen = offered.iter().position(|template| *template == draft.preset());
    ui.add(Text::new(t!("profiles.wizard.template-lead")).role("secondary")).fill_width();
    let group = RadioGroup::new(offered.iter().map(|template| template_word(*template)).collect::<Vec<_>>())
        .selected(chosen)
        .horizontal(true)
        .on_select(Msg::PickTemplate);
    ui.add(group.wrap(true)).id("profile-template");
    // Every part this profile can have, one switch each, whatever the picker reads: choosing a
    // set is only a way of switching these on. The person is agreeing to downloads of hundreds of
    // megabytes and to tools of other makers in their image, and each part is theirs to refuse.
    let (plugins, parts): (Vec<Extra>, Vec<Extra>) =
        draft.offered().into_iter().partition(|extra| matches!(extra, Extra::Plugin(_)));
    extra_switches(draft, &parts, ui).id("profile-extras").fill_width();
    if !plugins.is_empty() {
        // The plugins stand together under one heading, in as many columns as fit the page with the
        // longest name whole, so the whole list keeps to the wizard's height and every plugin keeps
        // a switch of its own.
        ui.add(Text::new(t!("profiles.template.claude-plugins")).bold());
        // The page is as wide as the screen lets it be, up to its own.
        let page = ui.size().width.min(PAGE_WIDTH);
        let longest = plugins.iter().map(|plugin| plugin.id().chars().count()).max().unwrap_or_default();
        let needed = u16::try_from(longest).unwrap_or(u16::MAX).saturating_add(PLUGIN_ROW_ROOM);
        let fit = usize::from((page + PLUGIN_GAP) / (needed + PLUGIN_GAP)).clamp(1, PLUGIN_COLUMNS);
        let columns = if plugins.len() > PLUGINS_IN_ONE_COLUMN { fit } else { 1 };
        let per_column = plugins.len().div_ceil(columns);
        let spread = u16::try_from(columns).unwrap_or(1);
        let width = page.saturating_sub(PLUGIN_GAP * (spread - 1)) / spread;
        ui.row(|ui| {
            for (index, column) in plugins.chunks(per_column).enumerate() {
                let list = extra_switches(draft, column, ui).id(format!("profile-plugins-{index}"));
                if columns == 1 {
                    list.fill_width();
                } else {
                    list.width(Length::Cells(width));
                }
            }
        })
        .gap(PLUGIN_GAP)
        .fill_width();
    }
    draw_download(draft, ui);
}

/// Plugins up to this many stand in one column; more are spread over [`PLUGIN_COLUMNS`].
const PLUGINS_IN_ONE_COLUMN: usize = 6;

/// How many columns the plugins of QCode extra are spread over.
const PLUGIN_COLUMNS: usize = 3;

/// Cells between two columns of plugins.
const PLUGIN_GAP: u16 = 2;

/// Cells a plugin's row needs beside its name: the pillar before it, the gap, the switch with its
/// word in any language, and the gap after it.
const PLUGIN_ROW_ROOM: u16 = 18;

/// One list of switches, one row for each of `extras`, each saying what it is while the keyboard
/// is on its row or the pointer rests on it. Chromium is refused on a system that packages it as a
/// snap only: its switch is there, switched off and unable to move, and says why it cannot. Its row
/// stays one the keys reach, so the reason is read from the keyboard too.
fn extra_switches<'v>(draft: &Draft, extras: &[Extra], ui: &'v mut View<'_, Msg>) -> qframe::widget::NodeMut<'v, Msg> {
    SettingsList::show(ui, |list| {
        list.wrap(true);
        for extra in extras.iter().copied() {
            let (label, said) = part_word(extra, draft);
            let refused = extra == Extra::Chromium && draft.chromium_refused();
            let on = draft.has(extra);
            list.row(SettingRow::new(label).hint(said), |ui| {
                let switch = Switch::new(on).disabled(refused);
                ui.add(switch.on_toggle(move |on| Msg::SwitchExtra(extra, on)));
            });
        }
    })
}

/// What a part is called, in a word or two, and what it is, said under its row.
fn part_word(extra: Extra, draft: &Draft) -> (String, String) {
    match extra {
        Extra::Graphify => (extra.id().to_owned(), t!("profiles.wizard.high-graphify")),
        Extra::OhMyOpenAgent => (extra.id().to_owned(), t!("profiles.wizard.high-omo")),
        Extra::OhMyOpenCodeSlim => (extra.id().to_owned(), t!("profiles.template.slim-omo")),
        Extra::Rust => (t!("profiles.template.rust"), t!("profiles.template.rust-hint")),
        Extra::Chromium if draft.chromium_refused() => {
            (extra.id().to_owned(), t!("profiles.template.chromium-refused", system = draft.os.display_name()))
        }
        Extra::Chromium => (extra.id().to_owned(), t!("profiles.template.dev-chromium")),
        Extra::Settings => (t!("profiles.template.settings"), t!("profiles.template.settings-hint")),
        // Its full name, marketplace and all, is what `claude plugin install` is given.
        Extra::Plugin(plugin) => (extra.id().to_owned(), plugin.to_owned()),
    }
}

/// What the build downloads and what the network means, in the colour of a warning; or, when every
/// part of a QCode template is switched off, that nothing more is downloaded.
fn draw_download(draft: &Draft, ui: &mut View<'_, Msg>) {
    if !draft.adds_anything() {
        ui.add(Text::new(t!("profiles.template.adds-none")).role("secondary")).fill_width();
        return;
    }
    // The system's own repositories are named, since the packages come from there. graphify alone
    // comes from them and PyPI; the plugins add npm and GitHub, and the toolchain its own.
    let graphify_alone = draft
        .offered()
        .into_iter()
        .all(|extra| matches!(extra, Extra::Graphify | Extra::Settings) || !draft.has(extra));
    let system = draft.os.display_name();
    let download = match draft.preset() {
        Template::QuvytaDev => t!("profiles.template.dev-download", system = system),
        _ if graphify_alone => t!("profiles.template.download-graphify", system = system),
        _ if draft.has(Extra::Rust) => t!("profiles.template.custom-dev-download", system = system),
        Template::Custom => t!("profiles.template.custom-download", system = system),
        preset => t!("profiles.template.download", template = template_word(preset).as_str(), system = system),
    };
    ui.add(Text::new(download).color("warning")).fill_width();
    if let Some(offline) = offline_line(draft) {
        ui.add(Text::new(offline).color("warning")).fill_width();
    }
}

/// What will not work at run time in a profile of a QCode template whose containers have no network, when
/// that is the profile being made; `None` otherwise.
///
/// Measured in containers without one: context7 of the Claude Code plugins is a server on
/// `mcp.context7.com`; oh-my-openagent brings three such servers (web search, context7, grep.app),
/// and opencode lists all three as failed. graphify, the other plugins and oh-my-openagent's own
/// agents are in the image and work.
fn offline_line(draft: &Draft) -> Option<String> {
    if draft.network != NetworkMode::None {
        return None;
    }
    // Each line names what stops answering; one whose part was switched off has nothing to say,
    // and the plain line speaks of graphify, so it goes when graphify does.
    let context7 = Extra::parse("context7").is_some_and(|context7| draft.has(context7));
    match draft.harness {
        HarnessKind::ClaudeCode if context7 => Some(t!("profiles.wizard.high-offline-claude")),
        HarnessKind::OpenCode if draft.has(Extra::OhMyOpenCodeSlim) => Some(t!("profiles.template.slim-offline")),
        HarnessKind::OpenCode if draft.has(Extra::OhMyOpenAgent) => Some(t!("profiles.wizard.high-offline-opencode")),
        _ if draft.has(Extra::Graphify) => Some(t!("profiles.wizard.high-offline")),
        _ => None,
    }
}

/// What the profile signs in with.
fn draw_account(draft: &Draft, ui: &mut View<'_, Msg>) {
    let offered = draft.harness.record().accounts;
    let chosen = offered.iter().position(|account| *account == draft.account);
    ui.add(Text::new(t!("profiles.wizard.account-lead")).role("secondary")).fill_width();
    let group = RadioGroup::new(offered.iter().map(|account| account_word(*account)))
        .selected(chosen)
        .on_select(Msg::PickAccount);
    ui.add(group.wrap(true)).id("profile-account");
    if let Some(detail) = no_login_detail(draft.account, draft.harness.record().display_name) {
        ui.add(Text::new(detail).role("secondary")).fill_width();
    }
    // Said where the account is chosen, so the sign-in at the end of the wizard is expected, and
    // so is what it is for: one login for the profile, not one for every workspace.
    if draft.account.needs_login() {
        let harness = draft.harness.record().display_name;
        ui.add(Text::new(t!("profiles.signing.ahead", harness = harness)).role("secondary")).fill_width();
    }
    if draft.account == AccountKind::Provider {
        draw_provider_choice(draft, ui);
    }
}

/// The provider and what a profile of [`AccountKind::Provider`] runs on: the person's own
/// providers first, then one list holding the chosen provider's lineups and its models, each
/// under a heading of its own. Offering nothing but their own providers and that provider's own
/// lineups and models is the point — a list borrowed from nowhere else.
///
/// A provider can offer hundreds of models, so they are a filtered list and not a group of
/// choices: a group taller than the page takes the wizard's buttons off the screen, and the
/// wizard cannot go on. The provider itself has a handful and stays a group. One list rather than
/// two, so that a lineup and a model of the same provider are chosen in the same place with the
/// same keys and the same filter, and the page's height does not depend on whether the provider
/// happens to have lineups.
fn draw_provider_choice(draft: &Draft, ui: &mut View<'_, Msg>) {
    if draft.providers.is_empty() {
        ui.add(Text::new(t!("profiles.wizard.provider-none")).role("secondary")).fill_width();
        ui.add(Button::new(t!("profiles.wizard.provider-manage")).on_press(Msg::ManageProviders))
            .id("profile-provider-manage");
        return;
    }
    let chosen_tag = draft.providers.iter().position(|entry| Some(entry.tag.as_str()) == draft.provider_tag.as_deref());
    ui.add(Text::new(t!("profiles.wizard.provider-lead")).role("secondary")).fill_width();
    let group = RadioGroup::new(draft.providers.iter().map(|entry| entry.tag.to_string()))
        .selected(chosen_tag)
        .on_select(Msg::PickProvider);
    ui.add(group.wrap(true)).id("profile-provider");
    if draft.provider_models().is_empty() {
        ui.add(Text::new(t!("profiles.wizard.provider-model-none")).role("secondary")).fill_width();
        ui.add(Button::new(t!("profiles.wizard.provider-manage")).on_press(Msg::ManageProviders))
            .id("profile-provider-model-manage");
        return;
    }
    let kind = draft.provider_tag.as_deref().and_then(|tag| draft.provider_entry(tag)).map(|entry| entry.kind);
    ui.add(
        TextInput::new(draft.model_filter.clone())
            .placeholder(t!("profiles.wizard.provider-model-filter"))
            .on_change(Msg::ModelFilter),
    )
    .id("profile-provider-model-filter")
    .fill_width();
    // The rows the list takes are the ones the page's other lines and the wizard's buttons leave
    // of the screen, so however many models a provider offers the way on stays where a hand can
    // reach it.
    let rows = model_rows(ui);
    let shown = draft.pick_rows();
    // A lineup's row carries its steps, the way the person wrote them, so an order of three models
    // is one row rather than three to read through; a model's own row carries the window its
    // server gives, which is what the harness is told and why the line under the list is silent
    // about it.
    let items = shown.iter().map(|row| match row {
        PickRow::LineupHeading => ListItem::header(t!("profiles.wizard.provider-lineups")),
        PickRow::ModelHeading => ListItem::header(t!("profiles.wizard.provider-models")),
        PickRow::Gap => ListItem::gap(),
        PickRow::Lineup(lineup) => ListItem::new(lineup.name.as_str()).detail(lineup.models.join(STEP_ARROW)),
        PickRow::Model(model) => match kind.and_then(|kind| model.window(kind)) {
            Some(window) => ListItem::new(model.id.clone()).detail(window.to_string()),
            None => ListItem::new(model.id.clone()),
        },
    });
    // The label of a row is what the person chooses by, so it is the detail that is cut when the
    // row is too narrow: a lineup's steps are read off its row, a model's window is read beside
    // every other model of the same provider.
    let list = List::new(items)
        .empty_text(t!("profiles.wizard.provider-model-filter-none"))
        .selected(draft.chosen_row())
        .label_first(true)
        .on_select(Msg::PickProviderModel)
        .on_activate(Msg::PickProviderModel);
    ui.add(list.wrap(true)).id("profile-provider-model").height(Length::Cells(rows));
    // Money spent without being asked for is said where it is chosen, not in a bill afterwards:
    // a step that costs is one the relay will fall back to on a turn that could have gone free.
    let paid = draft.paid_steps();
    if !paid.is_empty() {
        ui.add(Text::new(t!("profile.lineup-paid", models = paid.join(", "))).color("warning")).fill_width();
    }
    // Only a window QCode measured, or one a ready-made service gives for its own models, is
    // handed to Claude Code; without one it works to the room of its own models and prints a
    // warning at every start. Measuring is left to the person on the Providers page, because it
    // loads the model on their server.
    if draft.harness == HarnessKind::ClaudeCode {
        let unmeasured = draft.unknown_windows();
        if !unmeasured.is_empty() {
            let tokens = ASSUMED_CONTEXT_TOKENS.to_string();
            let said = match &draft.provider_pick {
                Some(Pick::Lineup(lineup)) => t!(
                    "profiles.wizard.provider-lineup-unmeasured",
                    lineup = lineup.as_str(),
                    models = unmeasured.join(", "),
                    tokens = tokens
                ),
                _ => t!("profiles.wizard.provider-model-unmeasured", model = unmeasured.join(", "), tokens = tokens),
            };
            ui.add(Text::new(said).role("secondary")).fill_width();
        }
    }
}

/// Between one step of a lineup and the next, on its row: what the relay asks in that order.
const STEP_ARROW: &str = " → ";

/// Rows the model list takes: what the page's other lines and the wizard's own frame leave of the
/// screen, so the way on is on the screen however many models a provider offers.
fn model_rows(ui: &View<'_, Msg>) -> u16 {
    ui.size().height.saturating_sub(ROWS_AROUND_MODELS).clamp(MODEL_ROWS_MIN, MODEL_ROWS_MAX)
}

/// Rows the account page spends around the model list: its own question, the account and provider
/// rows, the filter field and the sentences under the list — what a lineup or a model costs, and
/// the room a model nobody measured is given — and the wizard's steps and buttons with the row the
/// application's foot takes. A sentence is two rows on a narrow terminal, which is where this is
/// the one to be wrong.
const ROWS_AROUND_MODELS: u16 = 17;

/// Most rows the model list takes, so the account page stays within the height the wizard keeps
/// for its tallest page ([`WIZARD_ROWS`]) on a screen with rows to spare.
const MODEL_ROWS_MAX: u16 = 12;

/// Fewest rows the model list takes: a list of one row is no list to move through.
const MODEL_ROWS_MIN: u16 = 3;

/// What the container may see and reach.
fn draw_permissions(draft: &Draft, ui: &mut View<'_, Msg>) {
    ui.add(Text::new(t!("profiles.wizard.permissions-lead")).role("secondary")).fill_width();
    ui.add(Text::new(t!("profiles.wizard.permissions-code")).bold());
    ui.add(Text::new(t!("profiles.access-rw")).role("secondary"));
    ui.add(Text::new(t!("profiles.wizard.code-why")).role("secondary")).fill_width();
    ui.add(Text::new(t!("profiles.wizard.permissions-assets")).bold());
    let assets = MountAccess::ALL.iter().position(|access| *access == draft.assets);
    let group =
        RadioGroup::new(MountAccess::ALL.map(access_word)).selected(assets).horizontal(true).on_select(Msg::PickAssets);
    ui.add(group.wrap(true)).id("profile-assets");
    ui.add(Text::new(t!("profiles.wizard.permissions-network")).bold());
    let offered = draft.networks();
    let network = offered.iter().position(|mode| *mode == draft.network);
    let group = RadioGroup::new(offered.iter().map(|mode| network_word(*mode)))
        .selected(network)
        .horizontal(true)
        .on_select(Msg::PickNetwork);
    ui.add(group.wrap(true)).id("profile-network");
    // Said where the network is chosen, so the one choice missing is not a mystery.
    if offered.len() < NetworkMode::ALL.len() {
        let said = t!("profiles.wizard.network-kept", harness = draft.harness.record().display_name);
        ui.add(Text::new(said).role("secondary")).fill_width();
    }
    // Chosen here, after the template: this is where the person turns the network off, so this
    // is where they learn what of a QCode template that costs.
    if let Some(offline) = offline_line(draft) {
        ui.add(Text::new(offline).color("warning")).fill_width();
    }
}

/// Building the image, with everything the engine says as it says it.
fn draw_image(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    let rebuild = draft.is_only_rebuild() || (draft.is_editing() && draft.rebuilds());
    let kept = draft.is_editing() && !draft.rebuilds();
    let lead = match &draft.build {
        // A change that does not reach the image saves the file and builds nothing.
        Build::Waiting | Build::Running(_) if kept => t!("profiles.edit.saving"),
        Build::Done if kept => t!("profiles.edit.kept"),
        Build::Done if draft.is_editing() => t!("profiles.edit.rebuilt", name = draft.name.as_str()),
        Build::Failed(_) if kept => t!("profiles.edit.unsaved"),
        // A rebuild keeps the image that was there until the new one is finished, so its words
        // for a build under way, stopped or failed are not the wizard's.
        Build::Running(_) if rebuild => t!("profiles.rebuild.running"),
        Build::Done if rebuild => t!("profiles.rebuild.done", name = draft.name.as_str()),
        Build::Stopped if rebuild => t!("profiles.rebuild.stopped"),
        Build::Failed(_) if rebuild => t!("profiles.rebuild.failed"),
        Build::Waiting => t!("profiles.wizard.build-waiting"),
        Build::Running(_) => t!("profiles.wizard.build-running"),
        Build::Done => t!("profiles.wizard.build-done"),
        Build::Stopped => t!("profiles.wizard.build-stopped"),
        Build::Failed(_) if draft.dev_download_failed() => t!("profiles.template.dev-build-failed"),
        Build::Failed(_) if draft.extra_download_failed() => {
            t!("profiles.template.build-failed", template = template_word(Template::High).as_str())
        }
        Build::Failed(_) if draft.slim_download_failed() => {
            t!("profiles.template.build-failed", template = template_word(Template::Slim).as_str())
        }
        Build::Failed(_) if draft.recommended_download_failed() => {
            t!("profiles.template.build-failed", template = template_word(Template::Recommended).as_str())
        }
        Build::Failed(_) if draft.custom_download_failed() => {
            t!("profiles.template.build-failed", template = template_word(Template::Custom).as_str())
        }
        Build::Failed(_) => t!("profiles.wizard.build-failed"),
    };
    // While the build runs its words shine, the one thing on the page that moves besides the log:
    // the page is waiting, and says so without a spinner of its own.
    // A shimmer is one line that is never wrapped, so it carries the short word and the sentence
    // under it the rest.
    if matches!(draft.build, Build::Running(_)) {
        ui.add(ShimmerText::new(t!("profiles.working.build"))).id("build-working");
    }
    ui.add(Text::new(lead).role(if matches!(draft.build, Build::Failed(_)) { "danger" } else { "secondary" }))
        .fill_width();
    // A build has no time limit of its own, so nothing here stops one; what a build that has said
    // nothing for a while gets is the truth about its log, under the sentence that says the page is
    // waiting. The Stop button below stays the person's own way out of it.
    if draft.is_stuck() {
        ui.add(Text::new(t!("build.stuck", minutes = state.stall.minutes())).color("warning")).fill_width();
    }
    if let Build::Failed(problem) = &draft.build {
        recognised(state, problem, ui);
    }
    if draft.is_editing() {
        match draft.build {
            // What happens to the containers already made from the profile.
            Build::Done => {
                ui.add(Text::new(t!("profiles.edit.runtime")).role("secondary")).fill_width();
            }
            Build::Failed(_) | Build::Stopped if rebuild => {
                ui.add(Text::new(t!("profiles.edit.unsaved")).color("warning")).fill_width();
            }
            _ => {}
        }
    }
    if draft.build == Build::Done && !draft.account.needs_login() && !rebuild && !draft.is_editing() {
        let harness = draft.harness.record().display_name;
        ui.add(Text::new(t!("profiles.wizard.free-ready", harness = harness)).color("success")).fill_width();
    }
    // A window's image carries a whole desktop application, which is an order of magnitude more
    // than a command-line harness; the person is told before the build rather than after.
    if let Some(desktop) = draft.harness.desktop()
        && matches!(draft.build, Build::Waiting | Build::Running(_))
        && !kept
    {
        let size = t!(
            "profiles.wizard.desktop-size",
            harness = draft.harness.record().display_name,
            version = desktop.version,
            mib = desktop.image_mib.to_string()
        );
        ui.add(Text::new(size).role("secondary")).fill_width();
    }
    // Said again right before the build starts, which is when the download happens.
    if draft.adds_anything() && matches!(draft.build, Build::Waiting | Build::Running(_)) && !kept {
        draw_download(draft, ui);
    }
    // Saving a change the image does not see needs no engine.
    let engineless = engineless && !kept;
    if engineless {
        ui.add(Text::new(t!("profiles.no-engine-detail")).color("warning")).fill_width();
    }
    ui.add(LogView::new(&draft.log).empty_text(t!("profiles.wizard.build-empty")))
        .id("build-log")
        .fill_width()
        .height(Length::Cells(VIEWPORT_ROWS));
    ui.row(|ui| {
        match draft.build {
            Build::Running(_) => {
                ui.add(Button::new(t!("profiles.wizard.build-cancel")).on_press(Msg::BuildCancel)).id("build-cancel");
            }
            Build::Done => {}
            _ => {
                let label = if draft.build == Build::Waiting {
                    t!("profiles.wizard.build-start")
                } else {
                    t!("profiles.wizard.build-again")
                };
                let mut button = Button::new(label).variant("primary").disabled(engineless);
                if !engineless {
                    button = button.on_press(Msg::BuildStart);
                }
                ui.add(button).id("build-start");
            }
        }
        ui.spacer();
    })
    .gap(2)
    .fill_width();
}

/// Signing in: why a terminal opens, what to do in it, and how QCode decides it worked.
fn draw_login(state: &Profiles, draft: &Draft, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    let harness = draft.harness.record().display_name;
    match &draft.login {
        Login::Waiting => {
            // Gemini CLI's own dialog is kept to the key in the image, so the page says where the
            // key comes from and what the dialog will ask, rather than speak of a sign-in flow.
            // A window is signed in to in a window, so its page says so, and what QCode reads to be
            // sure of it is the application's own record rather than a file.
            let window = draft.harness.desktop().is_some();
            let (why, what, proof) = if window {
                ("profiles.window.why", "profiles.window.what", "profiles.window.proof")
            } else if draft.harness == HarnessKind::GeminiCli && draft.account == AccountKind::ApiKey {
                ("profiles.gemini.login-why", "profiles.gemini.login-what", "profiles.wizard.login-proof")
            } else {
                ("profiles.wizard.login-why", "profiles.wizard.login-what", "profiles.wizard.login-proof")
            };
            ui.add(Text::new(t!("profiles.signing.kept")).bold()).fill_width();
            ui.add(Text::new(t!(why, harness = harness)).role("secondary")).fill_width();
            ui.add(Text::new(t!(what, harness = harness)).role("secondary")).fill_width();
            ui.add(Text::new(t!(proof)).role("secondary")).fill_width();
            // A profile being made can be finished from here without signing in; what that costs
            // is said before the person decides, not after.
            if !draft.is_only_login() {
                ui.add(Text::new(t!("profiles.signing.skip", harness = harness)).role("secondary")).fill_width();
            }
            if engineless {
                ui.add(Text::new(t!("profiles.no-engine-detail")).color("warning")).fill_width();
            }
            let open = if window { t!("profiles.window.open") } else { t!("profiles.wizard.login-open") };
            let mut button = Button::new(open).variant("primary").disabled(engineless);
            if !engineless {
                button = button.on_press(Msg::LoginStart);
            }
            ui.add(button).id("login-open");
        }
        Login::Opening => {
            ui.add(ShimmerText::new(t!("profiles.wizard.login-opening"))).id("login-working");
        }
        Login::Running { session, .. } => {
            ui.add(Text::new(t!("profiles.wizard.login-running", harness = harness)).role("secondary")).fill_width();
            ui.add(Terminal::new(session)).id("login-terminal").fill_width().height(Length::Cells(VIEWPORT_ROWS));
            ui.row(|ui| {
                ui.add(Button::new(t!("profiles.wizard.login-done")).variant("primary").on_press(Msg::LoginDone))
                    .id("login-done");
                ui.add(Button::new(t!("profiles.wizard.login-stop")).on_press(Msg::LoginCancel)).id("login-stop");
                ui.spacer();
            })
            .gap(2)
            .fill_width();
        }
        Login::Window(window) => draw_window(window, harness, ui),
        Login::Storing => {
            ui.add(ShimmerText::new(t!("profiles.wizard.login-storing"))).id("login-working");
        }
        Login::Stored(_) if draft.harness.desktop().is_some() => {
            ui.add(Text::new(t!("profiles.window.stored", harness = harness)).color("success")).fill_width();
            ui.add(Text::new(t!("profiles.signing.stored")).role("secondary")).fill_width();
        }
        Login::Stored(files) => {
            ui.add(
                Text::new(t!("profiles.wizard.login-stored", n = i64::try_from(*files).unwrap_or(i64::MAX)))
                    .color("success"),
            )
            .fill_width();
            ui.add(Text::new(t!("profiles.signing.stored")).role("secondary")).fill_width();
        }
        Login::Unfinished(why) => {
            let text = match why {
                Unfinished::Stopped => t!("profiles.wizard.login-interrupted"),
                Unfinished::Missing if draft.harness.desktop().is_some() => {
                    t!("profiles.window.not-found", harness = harness)
                }
                Unfinished::Missing => t!("profiles.wizard.login-not-found"),
            };
            ui.add(Text::new(text).color("warning")).fill_width();
            ui.add(Text::new(t!("profiles.wizard.login-later")).role("secondary")).fill_width();
            ui.add(Button::new(t!("profiles.wizard.login-again")).on_press(Msg::LoginStart)).id("login-again");
        }
        Login::Failed(problem) => {
            ui.add(Text::new(t!("profiles.wizard.login-failed")).color("danger")).fill_width();
            recognised(state, problem, ui);
            if let Some(output) = problem.output() {
                ui.add(Text::new(output.to_owned()).role("secondary")).fill_width();
            }
            ui.add(Text::new(t!("profiles.wizard.login-later")).role("secondary")).fill_width();
            ui.add(Button::new(t!("profiles.wizard.login-again")).on_press(Msg::LoginStart)).id("login-again");
        }
    }
}

/// The sign-in window's page: where the window is, what to do in it, where the sign-in page went,
/// and the two things that can be done from here.
fn draw_window(window: &WindowLogin, harness: &str, ui: &mut View<'_, Msg>) {
    ui.add(Text::new(t!("profiles.window.running", harness = harness))).fill_width();
    ui.add(Text::new(t!("profiles.window.noticed", harness = harness)).role("secondary")).fill_width();
    let minutes = (callback::WAIT.as_secs() / 60).to_string();
    match &window.page {
        None => {}
        Some((address, Page::Asked | Page::InBrowser)) => {
            ui.add(Text::new(t!("workspace.window.signin-in-browser")).role("secondary")).fill_width();
            ui.add(Text::new(address.clone()).role("secondary")).fill_width();
        }
        Some((address, Page::Nowhere | Page::Held)) => {
            ui.add(Text::new(t!("workspace.window.signin-open-yourself")).color("warning")).fill_width();
            ui.add(Text::new(address.clone())).fill_width();
        }
    }
    match &window.back {
        None => {}
        Some((port, WindowBack::Listening(_))) => {
            let text = t!("workspace.window.signin-listening", port = port.to_string(), minutes = minutes.clone());
            ui.add(Text::new(text).role("secondary")).fill_width();
        }
        Some((port, WindowBack::Returned)) => {
            let text = t!("workspace.window.signin-returned", port = port.to_string());
            ui.add(Text::new(text).color("success")).fill_width();
        }
        Some((port, WindowBack::TimedOut)) => {
            let text = t!("workspace.window.signin-timed-out", port = port.to_string(), minutes = minutes);
            ui.add(Text::new(text).color("warning")).fill_width();
        }
        Some((port, WindowBack::Taken)) => {
            ui.add(Text::new(t!("profiles.window.port-taken", port = port.to_string())).color("warning")).fill_width();
        }
        Some((port, WindowBack::Refused(reason))) => {
            let text = t!("profiles.window.no-listen", port = port.to_string(), reason = reason.clone());
            ui.add(Text::new(text).color("warning")).fill_width();
        }
    }
    ui.row(|ui| {
        ui.add(Button::new(t!("profiles.wizard.login-done")).variant("primary").on_press(Msg::LoginDone))
            .id("login-done");
        ui.add(Button::new(t!("profiles.wizard.login-stop")).on_press(Msg::LoginCancel)).id("login-stop");
        ui.spacer();
    })
    .gap(2)
    .fill_width();
}
