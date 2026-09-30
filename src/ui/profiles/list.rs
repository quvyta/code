//! The list of profiles: the screen's view, the table of profiles with what the engine says of
//! each, and the words the list and the wizard both use for a profile's choices.

use qframe::diagnostics::Severity;
use qframe::prelude::*;
use qframe::widgets::{Column, ColumnWidth, EmptyState, Table, TableCell, TableRow};

use crate::base::Os;
use crate::profile::{AccountKind, HarnessKind, MountAccess, NetworkMode, Profile, Template};
use crate::ui::page;

use super::wizard_view::draw_wizard;
use super::{CHROME_ROWS, LIST_WIDTH, Msg, Profiles, Readiness, Revision, Row, ShellMsg, WIZARD_ROWS, shell_page};

/// Draws the screen: the list of profiles, or the wizard while one is being made.
pub fn view(state: &Profiles, ui: &mut View<'_, Msg>) {
    if let Some(page) = &state.shell {
        let top = ui.size().height.saturating_sub(WIZARD_ROWS + CHROME_ROWS) / 2;
        ui.column(|ui| {
            ui.spacer().height(Length::Cells(top));
            shell_page::draw(page, ui);
        })
        .fill()
        .align(Align::Center);
        return;
    }
    match state.draft() {
        Some(draft) => draw_wizard(state, draft, ui),
        None => draw_list(state, ui),
    }
}

/// The control that takes the keyboard when the screen opens, once there is one: the list of
/// profiles, or on an empty screen its one button, so Enter makes the first profile without a
/// Tab the screen never mentions.
#[must_use]
pub fn entry(state: &Profiles) -> Option<&'static str> {
    if state.loading {
        None
    } else if state.rows().is_empty() {
        Some("profiles-empty")
    } else {
        Some("profiles")
    }
}

/// The keys of the profiles screen that are not in the keymap, for the key list.
#[must_use]
pub fn hints(icons: &qframe::icons::Icons) -> Vec<(String, String)> {
    let move_keys = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
    vec![(move_keys, t!("hints.move")), (icons.glyph("enter").into_owned(), t!("hints.open"))]
}

/// The list of profiles, what the engine says about the chosen one, and the way to a new one.
fn draw_list(state: &Profiles, ui: &mut View<'_, Msg>) {
    let engineless = state.engine.is_none();
    // While a login is being removed the button that asked for it shows the work and takes no
    // second press.
    let busy = state.signing_out;
    let rows = state.rows();
    let selected = state.selected().cloned();
    // Only a file that could not be read is counted as one: a warning is about a profile that
    // loaded, and what it says is spelled out beside that profile instead.
    let problems = state.problems().iter().filter(|problem| problem.severity == Severity::Error).count();
    let loading = state.is_loading();
    let store = state.root.is_some();

    page::column(ui, LIST_WIDTH, |ui| {
        ui.column(|ui| {
            // Only the title stands above the list: what a profile is for belongs on the empty
            // screen, where it is the answer to a question, and a full list needs its rows instead.
            ui.add(Text::new(t!("profiles.title")).bold());
            if engineless {
                ui.add(Text::new(t!("profiles.no-engine")).color("warning")).fill_width();
            }
            if !store {
                ui.add(Text::new(t!("profiles.no-folder")).color("warning")).fill_width();
                return;
            }
            if problems > 0 {
                ui.add(
                    Text::new(t!("profiles.broken", n = i64::try_from(problems).unwrap_or(i64::MAX))).color("warning"),
                );
            }
            if loading {
                ui.add(Text::new(t!("profiles.reading")).role("secondary"));
                return;
            }
            if rows.is_empty() {
                ui.add(
                    EmptyState::new(t!("profiles.empty-title"))
                        .icon("inbox")
                        .message(t!("profiles.empty-message"))
                        .action(Button::new(t!("profiles.new")).variant("primary").on_press(Msg::New)),
                )
                .id("profiles-empty")
                .fill();
                return;
            }

            // Every profile on a row of its own, what the engine said about it in columns that line
            // up, so the state of all of them is read at a glance. The way to a new profile is the
            // first row, the way it is on the workspaces screen, so the keyboard reaches it like any
            // profile; moving onto it only chooses it, and Enter or a click opens the wizard.
            let new = TableRow::new([TableCell::new(t!("profiles.new")).icon("add", Some("accent"))]);
            let table_rows: Vec<TableRow> = std::iter::once(new).chain(rows.iter().map(table_row)).collect();
            let height = u16::try_from(table_rows.len() + 1).unwrap_or(u16::MAX);
            let columns = [
                Column::new(t!("profiles.column.name")).width(ColumnWidth::Fit),
                Column::new(t!("profiles.column.harness")).width(ColumnWidth::Fit),
                Column::new(t!("profiles.column.account")).width(ColumnWidth::Fit),
                Column::new(t!("profiles.column.image")).width(ColumnWidth::Fit),
                Column::new(t!("profiles.column.sign-in")).width(ColumnWidth::Fill(1)),
            ];
            let chosen = if state.on_new { 0 } else { state.selected + 1 };
            let table = Table::new(columns, table_rows)
                .selected(Some(chosen))
                .on_select(|row| row.checked_sub(1).map_or(Msg::SelectNew, Msg::Select))
                .on_activate(|row| row.checked_sub(1).map_or(Msg::New, Msg::Select));
            ui.add(table.wrap(true)).id("profiles").height(Length::Cells(height)).fill_width();

            // What can be done to the chosen profile stands once, right under the rows, and nothing
            // stands there while the new-profile row is chosen: the rows themselves stay plain.
            if let Some(row) = selected.filter(|_| !state.on_new) {
                let needs_login = row.profile.account.needs_login();
                let signed_in = row.is_signed_in();
                ui.row(|ui| {
                    // Signing in and out mean nothing to a profile without an account, so it is not
                    // offered two buttons that could never do anything.
                    if needs_login {
                        let mut again = Button::new(t!("profiles.sign-in"));
                        if !engineless && row.image == Readiness::Present {
                            again = again.on_press(Msg::SignInAsked);
                        }
                        ui.add(again.disabled(engineless || row.image != Readiness::Present)).id("profile-sign-in");
                        let mut out = Button::new(t!("profiles.sign-out")).loading(busy);
                        if !engineless && signed_in && !busy {
                            out = out.on_press(Msg::SignOutAsked);
                        }
                        ui.add(out.variant("danger").disabled(engineless || !signed_in)).id("profile-sign-out");
                    }
                    // Everything but the name can be changed, whatever the engine says.
                    ui.add(Button::new(t!("profiles.edit.button")).on_press(Msg::EditAsked)).id("profile-edit");

                    // Only an image that is there can be built again; one that is not is built from
                    // the tab that needs it, with the words for that.
                    let rebuildable = !engineless && row.image == Readiness::Present;
                    let mut rebuild = Button::new(t!("profiles.rebuild.button"));
                    if rebuildable {
                        rebuild = rebuild.on_press(Msg::RebuildAsked);
                    }
                    ui.add(rebuild.disabled(!rebuildable)).id("profile-rebuild");
                    let mut delete =
                        Button::new(t!("profiles.removal.button")).variant("danger").loading(state.deleting);
                    if !state.deleting {
                        delete = delete.on_press(Msg::DeleteAsked);
                    }
                    ui.add(delete).id("profile-delete");
                    ui.spacer();
                })
                .gap(2)
                .fill_width();
                ui.add(Text::new(summary(&row.profile)).role("secondary")).fill_width();
                ui.add(Text::new(mounts(&row.profile)).role("secondary")).fill_width();
                if let Some(closed) = closed_sign_in(&row.profile) {
                    ui.add(Text::new(closed).color("warning")).fill_width();
                }
                if row.image == Readiness::Present && row.revision == Revision::Earlier {
                    ui.add(Text::new(t!("profiles.rebuild.earlier")).color("warning")).fill_width();
                }
                // On a row of its own beside what was added in it, so the actions above keep their
                // room on a narrow screen. Only an image that is there has a container to open.
                let shell = !engineless && row.image == Readiness::Present;
                ui.row(|ui| {
                    let mut open = Button::new(t!("profiles.shell.button"));
                    if shell {
                        open = open.on_press(Msg::Shell(ShellMsg::Asked));
                    }
                    ui.add(open.disabled(!shell)).id("profile-shell");
                    ui.spacer();
                })
                .fill_width();
                if let Some(own) = state.own.get(row.profile.name.as_str()) {
                    shell_page::draw_own(own, ui);
                }
            }
        })
        .fill()
        .gap(1);
    });
}

/// One profile's row: its name, its harness, what it signs in with, and what the engine said
/// about its image and its login.
fn table_row(row: &Row) -> TableRow {
    let sign_in = if row.profile.account.needs_login() {
        state_cell(row.identity, "profiles.identity")
    } else {
        // Nothing to ask the engine: a profile that signs in to nothing is as ready as its image,
        // and its cell says so, quietly, rather than "not signed in".
        TableCell::new(t!("profiles.identity-free")).icon("dot-outline", None)
    };
    let account = match &row.profile.provider {
        Some(_) => account_word(AccountKind::Provider),
        None => account_word(row.profile.account),
    };
    TableRow::new([
        TableCell::new(row.profile.name.to_string()),
        TableCell::new(row.profile.harness.record().display_name),
        TableCell::new(account),
        state_cell(row.image, "profiles.image"),
        sign_in,
    ])
}

/// The cell of one answer of the engine, which says plainly when there is no answer yet: a dot in
/// the answer's colour, and its words.
fn state_cell(readiness: Readiness, key: &str) -> TableCell {
    let (word, colour) = match readiness {
        Readiness::Present => ("ready", Some("success")),
        Readiness::Missing => ("missing", Some("warning")),
        Readiness::Unknown => ("unknown", None),
    };
    TableCell::new(t!(&format!("{key}-{word}"))).icon("dot", colour)
}

/// What a profile runs, in one line. The template is named without the recommendation the
/// wizard gives it: that is advice for choosing, and this profile has chosen already.
fn summary(profile: &Profile) -> String {
    let template = t!(match profile.template {
        Template::Base => "profiles.template.summary-base",
        Template::Recommended => "profiles.template.name-recommended",
        Template::Slim => "profiles.template.name-slim",
        Template::High => "profiles.template.name-extra",
        Template::QuvytaDev => "profiles.summary-quvyta-dev",
        Template::Custom => "profiles.template.name-custom",
    });
    let account = match &profile.provider {
        Some(provider) => {
            t!("profiles.account-provider-named", tag = provider.tag.as_str(), model = provider.asked())
        }
        None => account_word(profile.account),
    };
    let summary = t!(
        "profiles.summary",
        harness = profile.harness.record().display_name,
        template = template.as_str(),
        account = account.as_str()
    );
    // Debian goes unsaid, as it did before there was a choice; any other system is named, since
    // it is the one thing about the profile the rest of the line does not show.
    if profile.os == Os::Debian {
        summary
    } else {
        t!("profiles.summary-system", summary = summary.as_str(), system = profile.os.display_name())
    }
}

/// What a profile's containers may see and reach, in one line.
fn mounts(profile: &Profile) -> String {
    t!(
        "profiles.mounts",
        assets = access_word(profile.assets).as_str(),
        network = network_word(profile.network).as_str()
    )
}

/// What the person is told about a profile whose account its harness's makers closed, in plain
/// words: who closed it, what the harness now answers, for whom it still works, and the ways on.
/// `None` for every profile whose sign-in still stands.
#[must_use]
pub fn closed_sign_in(profile: &Profile) -> Option<String> {
    if !profile.harness.withdrawn(profile.account) {
        return None;
    }
    match profile.harness {
        HarnessKind::GeminiCli => Some(t!("profiles.gemini.closed")),
        HarnessKind::ClaudeCode
        | HarnessKind::OpenCode
        | HarnessKind::Codex
        | HarnessKind::KimiCode
        | HarnessKind::QwenCode
        | HarnessKind::AntigravityIde => None,
    }
}

/// The word for a template.
pub(super) fn template_word(template: Template) -> String {
    t!(match template {
        Template::Base => "profiles.template.name-base",
        Template::Recommended => "profiles.template.name-recommended",
        Template::Slim => "profiles.template.name-slim",
        Template::High => "profiles.template.name-extra",
        Template::QuvytaDev => "profiles.template-quvyta-dev",
        Template::Custom => "profiles.template.name-custom",
    })
}

/// The word for an account type.
pub(super) fn account_word(account: AccountKind) -> String {
    t!(match account {
        AccountKind::Free => "profiles.account-free",
        AccountKind::Subscription => "profiles.account-subscription",
        AccountKind::ApiKey => "profiles.account-api-key",
        AccountKind::InApp => "profiles.account-in-app",
        AccountKind::Provider => "profiles.account-provider",
    })
}

/// Why a profile has no sign-in step, for the one account that has none: the harness needs no
/// account at all.
pub(super) fn no_login_detail(account: AccountKind, harness: &str) -> Option<String> {
    match account {
        AccountKind::Free => Some(t!("profiles.wizard.account-free-detail", harness = harness)),
        AccountKind::Subscription | AccountKind::ApiKey | AccountKind::InApp | AccountKind::Provider => None,
    }
}

/// The word for a mount access.
pub(super) fn access_word(access: MountAccess) -> String {
    t!(match access {
        MountAccess::ReadWrite => "profiles.access-rw",
        MountAccess::ReadOnly => "profiles.access-ro",
    })
}

/// The word for a network mode.
pub(super) fn network_word(mode: NetworkMode) -> String {
    t!(match mode {
        NetworkMode::Full => "profiles.network-full",
        NetworkMode::None => "profiles.network-none",
    })
}
