//! The window.
//!
//! Everything here reads the store and never the network (SPEC §6). Connecting an account is
//! the one action that reaches out, and it does so through `runtime::spawn` so the window
//! stays responsive for however long the user spends at Google's consent screen.

pub mod layout;
pub mod week;

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use gtk::{Align, Orientation};

use std::collections::HashMap;

use chrono::{Duration, TimeZone};

use crate::accounts::style::resolve;
use crate::accounts::{self, Connected};
use crate::config::{Credentials, Paths};
use crate::recur::occurrences_in_window;
use crate::runtime;
use crate::store::Store;

/// Built once and shared with every callback that needs to redraw.
///
/// `Rc`, not `Arc`: this lives entirely on the GTK thread and holds widgets, which are
/// neither `Send` nor `Sync`. Only `store` crosses to the tokio runtime, and it carries its
/// own `Arc<Mutex<_>>`.
struct Ui {
    store: Arc<Mutex<Store>>,
    credentials: Credentials,
    sidebar: gtk::Box,
    toasts: adw::ToastOverlay,
    connect_button: gtk::Button,
    week: Rc<week::Week>,
}

pub fn build(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        tracing::debug!("already running; presenting the existing window");
        window.present();
        return;
    }

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("agenda")
        .default_width(1100)
        .default_height(760)
        .build();

    match startup() {
        Ok(content) => window.set_content(Some(&content)),
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "could not start");
            window.set_content(Some(&shell(&status(
                "dialog-error-symbolic",
                "agenda could not start",
                &format!("{error:#}"),
            ))));
        }
    }
    window.present();
}

/// Decide what the window shows. A user who has not yet created an OAuth client is the
/// expected first-run case, not a failure, so it gets an instruction rather than an error.
fn startup() -> anyhow::Result<gtk::Widget> {
    let paths = Paths::from_env()?;
    let oauth = paths.oauth();

    let Some(credentials) = Credentials::load(&oauth)? else {
        tracing::info!(path = %oauth.display(), "no OAuth client yet; showing onboarding");
        return Ok(shell(&status(
            "dialog-information-symbolic",
            "Set up Google access",
            &format!(
                "agenda needs a Google OAuth client, which only you can create.\n\n\
                 Follow docs/google-oauth-setup.md, then write the client ID and secret to {}.",
                oauth.display()
            ),
        ))
        .upcast());
    };

    let database = paths.database();
    tracing::debug!(path = %database.display(), "opening the store");
    let store = Arc::new(Mutex::new(Store::open(&database)?));

    let ui = Rc::new(Ui {
        store,
        credentials,
        sidebar: gtk::Box::new(Orientation::Vertical, 0),
        toasts: adw::ToastOverlay::new(),
        connect_button: gtk::Button::with_label("Connect account"),
        week: week::Week::new(),
    });

    let content = build_content(&ui);
    refresh_sidebar(&ui);
    refresh_week(&ui);
    Ok(content)
}

fn build_content(ui: &Rc<Ui>) -> gtk::Widget {
    let header = adw::HeaderBar::new();

    // Reachable at any time, not only on first run — SPEC §2.6.
    ui.connect_button.add_css_class("suggested-action");
    header.pack_start(&ui.connect_button);

    let clicked = ui.clone();
    ui.connect_button
        .connect_clicked(move |_| start_connect(&clicked));

    let navigation = gtk::Box::new(Orientation::Horizontal, 0);
    navigation.add_css_class("linked");
    let previous = gtk::Button::from_icon_name("go-previous-symbolic");
    let next = gtk::Button::from_icon_name("go-next-symbolic");
    navigation.append(&previous);
    navigation.append(&next);
    let today = gtk::Button::with_label("Today");

    for (button, weeks) in [(&previous, -1_i64), (&next, 1)] {
        let ui = ui.clone();
        button.connect_clicked(move |_| {
            ui.week.shift(weeks);
            refresh_week(&ui);
        });
    }
    let clone = ui.clone();
    today.connect_clicked(move |_| {
        clone.week.go_to_today();
        refresh_week(&clone);
    });

    header.pack_end(&navigation);
    header.pack_end(&today);
    header.set_title_widget(Some(ui.week.title()));

    ui.sidebar.set_margin_top(6);
    ui.sidebar.set_margin_bottom(6);
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .width_request(280)
        .child(&ui.sidebar)
        .build();

    let split = gtk::Box::new(Orientation::Horizontal, 0);
    split.append(&sidebar_scroll);
    split.append(&gtk::Separator::new(Orientation::Vertical));
    split.append(ui.week.widget());

    ui.toasts.set_child(Some(&split));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&ui.toasts));
    toolbar.upcast()
}

/// Run the consent flow off the main thread. The button is disabled meanwhile, so a second
/// click cannot open a second consent screen against a listener that has already gone.
fn start_connect(ui: &Rc<Ui>) {
    ui.connect_button.set_sensitive(false);
    ui.connect_button.set_label("Waiting for your browser…");

    let store = ui.store.clone();
    let credentials = ui.credentials.clone();
    let ui = ui.clone();

    runtime::spawn(
        async move { accounts::connect(store, credentials).await },
        move |result| {
            ui.connect_button.set_sensitive(true);
            ui.connect_button.set_label("Connect account");
            match result {
                Ok(Connected::Account { email, calendars }) => {
                    ui.toasts.add_toast(adw::Toast::new(&format!(
                        "Connected {email} — {calendars} calendars"
                    )));
                    refresh_sidebar(&ui);
                    refresh_week(&ui);
                }
                Ok(Connected::Declined) => {
                    ui.toasts
                        .add_toast(adw::Toast::new("Not connected — you declined access."));
                }
                Err(error) => {
                    tracing::error!(error = %format!("{error:#}"), "connecting failed");
                    ui.toasts
                        .add_toast(adw::Toast::new(&format!("Could not connect: {error}")));
                }
            }
        },
    );
}

/// Redraw the account list from the store. Every connected account is shown, always —
/// SPEC §1: there is no current account, and nothing is hidden to make room for another.
fn refresh_sidebar(ui: &Rc<Ui>) {
    while let Some(child) = ui.sidebar.first_child() {
        ui.sidebar.remove(&child);
    }

    let groups = match read_groups(ui) {
        Ok(groups) => groups,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "could not read the accounts");
            Vec::new()
        }
    };

    if groups.is_empty() {
        let empty = gtk::Label::new(Some("No accounts connected yet."));
        empty.add_css_class("dim-label");
        empty.set_margin_top(24);
        empty.set_wrap(true);
        ui.sidebar.append(&empty);
        return;
    }

    for (account, calendars) in groups {
        let group = gtk::Box::new(Orientation::Vertical, 0);
        group.set_margin_bottom(12);

        let heading = gtk::Box::new(Orientation::Horizontal, 8);
        heading.set_margin_start(12);
        heading.set_margin_end(12);
        heading.set_margin_top(6);

        // The account's own colour: its marker on every event, and the fill for calendars
        // Google gave none.
        let account_color = color_button(account.color.as_deref());
        {
            let ui = ui.clone();
            let email = account.email.clone();
            account_color.connect_rgba_notify(move |button| {
                let (email, color) = (email.clone(), hex_of(button.rgba()));
                apply(&ui, move |store| store.set_account_color(&email, &color));
            });
        }
        heading.append(&account_color);

        let name = gtk::Label::new(Some(account.label.as_deref().unwrap_or(&account.email)));
        name.add_css_class("heading");
        name.set_halign(Align::Start);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_tooltip_text(Some(&account.email));
        heading.append(&name);

        let menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(Align::Center)
            .build();
        menu.add_css_class("flat");
        menu.set_popover(Some(&account_menu(ui, &account)));
        heading.append(&menu);
        group.append(&heading);

        for calendar in calendars {
            let row = gtk::Box::new(Orientation::Horizontal, 8);
            row.set_margin_start(20);
            row.set_margin_end(12);
            row.set_margin_top(4);

            let shown = gtk::CheckButton::new();
            shown.set_active(calendar.visible);
            shown.set_valign(Align::Center);
            shown.set_tooltip_text(Some("Show this calendar"));
            {
                let ui = ui.clone();
                let (account, id) = (calendar.account.clone(), calendar.id.clone());
                shown.connect_toggled(move |button| {
                    let (account, id, visible) = (account.clone(), id.clone(), button.is_active());
                    apply(&ui, move |store| {
                        store.set_calendar_visible(&account, &id, visible)
                    });
                });
            }
            row.append(&shown);

            let fill = color_button(
                calendar
                    .user_color
                    .as_deref()
                    .or(calendar.color.as_deref())
                    .or(account.color.as_deref()),
            );
            {
                let ui = ui.clone();
                let (acct, id) = (calendar.account.clone(), calendar.id.clone());
                fill.connect_rgba_notify(move |button| {
                    let (acct, id, color) = (acct.clone(), id.clone(), hex_of(button.rgba()));
                    apply(&ui, move |store| {
                        store.set_calendar_user_color(&acct, &id, Some(&color))
                    });
                });
            }
            row.append(&fill);

            let label = gtk::Label::new(Some(&calendar.summary));
            label.set_halign(Align::Start);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_tooltip_text(Some(&calendar.summary));
            if !calendar.visible {
                label.add_css_class("dim-label");
            }
            row.append(&label);

            // Only offered when there is something to undo, so the row stays quiet for a
            // calendar the user has never touched.
            if calendar.user_color.is_some() {
                let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
                reset.add_css_class("flat");
                reset.set_valign(Align::Center);
                reset.set_tooltip_text(Some("Use the calendar's own colour again"));
                let ui = ui.clone();
                let (acct, id) = (calendar.account.clone(), calendar.id.clone());
                reset.connect_clicked(move |_| {
                    let (acct, id) = (acct.clone(), id.clone());
                    apply(&ui, move |store| {
                        store.set_calendar_user_color(&acct, &id, None)
                    });
                });
                row.append(&reset);
            }

            group.append(&row);
        }
        ui.sidebar.append(&group);
    }
}

/// Rename and disconnect, kept out of the row itself so the sidebar stays a list of
/// calendars rather than a control panel.
fn account_menu(ui: &Rc<Ui>, account: &crate::store::Account) -> gtk::Popover {
    let content = gtk::Box::new(Orientation::Vertical, 6);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(6);
    content.set_margin_end(6);

    let entry = gtk::Entry::builder()
        .placeholder_text("Label, e.g. work")
        .text(account.label.clone().unwrap_or_default())
        .build();
    {
        let ui = ui.clone();
        let email = account.email.clone();
        entry.connect_activate(move |entry| {
            let email = email.clone();
            let text = entry.text().trim().to_string();
            let label = (!text.is_empty()).then_some(text);
            apply(&ui, move |store| {
                store.set_account_label(&email, label.as_deref())
            });
        });
    }
    content.append(&entry);

    let remove = gtk::Button::with_label("Disconnect account");
    remove.add_css_class("destructive-action");
    {
        let ui = ui.clone();
        let email = account.email.clone();
        remove.connect_clicked(move |_| confirm_remove(&ui, &email));
    }
    content.append(&remove);

    gtk::Popover::builder().child(&content).build()
}

type Group = (crate::store::Account, Vec<crate::store::Calendar>);

fn read_groups(ui: &Rc<Ui>) -> anyhow::Result<Vec<Group>> {
    let store = ui
        .store
        .lock()
        .map_err(|_| anyhow::anyhow!("the store lock was poisoned"))?;
    store
        .accounts()?
        .into_iter()
        .map(|account| {
            let calendars = store.calendars(&account.email)?;
            Ok((account, calendars))
        })
        .collect()
}

/// Redraw the grid from the store for the week now on screen.
///
/// The colour of every event is resolved here, against the account and calendar it came
/// from — SPEC §4's rule, applied once per redraw rather than baked into the store.
fn refresh_week(ui: &Rc<Ui>) {
    let start = ui.week.start();
    let zone = ui.week.zone();
    let Some(from) = zone
        .from_local_datetime(&start.and_hms_opt(0, 0, 0).expect("midnight exists"))
        .earliest()
    else {
        tracing::warn!(%start, "the displayed week has no valid start");
        return;
    };
    let to = from + Duration::days(super::ui::week::DAYS as i64);

    let items = match collect_items(ui, from.timestamp(), to.timestamp()) {
        Ok(items) => items,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "could not read the week");
            Vec::new()
        }
    };
    tracing::debug!(week = %start, events = items.len(), "redrew the week");
    ui.week.set_items(items);
}

fn collect_items(ui: &Rc<Ui>, from: i64, to: i64) -> anyhow::Result<Vec<week::Item>> {
    let store = ui
        .store
        .lock()
        .map_err(|_| anyhow::anyhow!("the store lock was poisoned"))?;

    let accounts: HashMap<String, crate::store::Account> = store
        .accounts()?
        .into_iter()
        .map(|account| (account.email.clone(), account))
        .collect();
    let mut calendars: HashMap<(String, String), crate::store::Calendar> = HashMap::new();
    for email in accounts.keys() {
        for calendar in store.calendars(email)? {
            calendars.insert((calendar.account.clone(), calendar.id.clone()), calendar);
        }
    }

    let occurrences = occurrences_in_window(&store, from, to)?;
    Ok(occurrences
        .into_iter()
        .filter_map(|occurrence| {
            let account = accounts.get(&occurrence.event.account)?;
            let calendar = calendars.get(&(
                occurrence.event.account.clone(),
                occurrence.event.calendar_id.clone(),
            ))?;
            Some(week::Item {
                summary: if occurrence.event.summary.is_empty() {
                    "(no title)".to_string()
                } else {
                    occurrence.event.summary.clone()
                },
                start_utc: occurrence.start_utc,
                end_utc: occurrence.end_utc,
                all_day: occurrence.event.all_day,
                colors: resolve(account, calendar),
                account: account
                    .label
                    .clone()
                    .unwrap_or_else(|| account.email.clone()),
            })
        })
        .collect())
}

/// `#RRGGBB` for a colour the user picked. Alpha is dropped: a translucent event on a
/// translucent event is unreadable, and nothing else in the app has a use for it.
fn hex_of(rgba: gtk::gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    )
}

fn color_button(current: Option<&str>) -> gtk::ColorDialogButton {
    let button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    button.set_valign(Align::Center);
    if let Some(rgba) = current.and_then(|color| gtk::gdk::RGBA::parse(color).ok()) {
        button.set_rgba(&rgba);
    }
    button
}

/// Apply a store change, then redraw both surfaces that depend on it.
fn apply(ui: &Rc<Ui>, change: impl FnOnce(&crate::store::Store) -> anyhow::Result<()>) {
    let result = ui
        .store
        .lock()
        .map_err(|_| anyhow::anyhow!("the store lock was poisoned"))
        .and_then(|store| change(&store));

    match result {
        Ok(()) => {
            refresh_sidebar(ui);
            refresh_week(ui);
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "could not save the change");
            ui.toasts
                .add_toast(adw::Toast::new(&format!("Could not save: {error}")));
        }
    }
}

/// Ask before disconnecting. Removing an account cascades to its calendars and every event
/// on them, and re-adding it means consenting again.
fn confirm_remove(ui: &Rc<Ui>, email: &str) {
    let dialog = adw::AlertDialog::new(
        Some("Disconnect this account?"),
        Some(&format!(
            "{email} and its calendars will be removed from agenda. \
             Nothing in your Google account changes, and you can reconnect it later."
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Disconnect");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));

    // The parent is taken before the closure captures anything, because `connect_response`
    // is FnMut and would otherwise hold the only handle to it.
    let parent = ui.toasts.root().and_downcast::<gtk::Window>();
    let handler = ui.clone();
    let email = email.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "remove" {
            return;
        }
        let ui = handler.clone();
        let account = email.clone();
        apply(&ui, move |store| store.remove_account(&account));
        // The tokens outlive the row otherwise, and the next connect would silently reuse
        // a grant the user believes they revoked.
        let account = email.clone();
        runtime::spawn(
            async move { crate::auth::keyring::delete(&account).await },
            |result| {
                if let Err(error) = result {
                    tracing::warn!(error = %format!("{error:#}"), "could not clear the keyring entry");
                }
            },
        );
    });
    dialog.present(parent.as_ref());
}

fn shell(child: &impl IsA<gtk::Widget>) -> adw::ToolbarView {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(child));
    toolbar
}

fn status(icon: &str, title: &str, description: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(description)
        .build()
}
