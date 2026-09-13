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
use gtk::glib;
use gtk::{Align, Orientation};

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use chrono::{Duration, TimeZone};

use crate::accounts::style::resolve;
use crate::accounts::{self, Connected};
use crate::config::{Credentials, Paths};
use crate::notify;
use crate::recur::occurrences_in_window;
use crate::runtime;
use crate::store::Store;
use crate::sync::scheduler;
use crate::tray;

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
    /// Accounts Google has permanently rejected. Held in memory, not the store: it is a fact
    /// about right now, and a restart should find out for itself rather than trust a flag.
    needs_reconnect: RefCell<HashSet<String>>,
    /// `None` when no StatusNotifierItem host answered. The window works regardless.
    tray: RefCell<Option<ksni::blocking::Handle<tray::Item>>>,
    sync_button: gtk::Button,
    /// The timer for the next scheduled pass. Held so a manual sync can cancel it: starting
    /// a second pass without cancelling would leave two self-re-arming chains running for
    /// the life of the process, each doubling on every tick.
    pending_sync: RefCell<Option<glib::SourceId>>,
    syncing: Cell<bool>,
    settings: notify::Settings,
    /// The instant reminders were last checked up to. Everything due after it and at or
    /// before now is delivered, which is what makes a suspend across a reminder harmless.
    reminded_to: Cell<i64>,
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
        sync_button: gtk::Button::from_icon_name("view-refresh-symbolic"),
        pending_sync: RefCell::new(None),
        syncing: Cell::new(false),
        week: week::Week::new(),
        needs_reconnect: RefCell::new(HashSet::new()),
        tray: RefCell::new(None),
        settings: notify::Settings::load(&paths.settings()),
        // Backdated, so launching a few minutes after a reminder came due still tells the
        // user about the meeting they are about to be late for. `due` already drops anything
        // whose event has ended, so this cannot produce a flood of stale notices.
        reminded_to: Cell::new(chrono::Utc::now().timestamp() - STARTUP_GRACE),
    });

    let content = build_content(&ui);
    refresh_sidebar(&ui);
    refresh_week(&ui);
    tracing::debug!(
        lead = ui.settings.lead_minutes,
        all_day_hour = ui.settings.all_day_hour,
        "reminder settings"
    );
    start_syncing(&ui, scheduler::MIN_INTERVAL);
    start_tray(&ui);
    start_reminders(&ui);
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

    ui.sync_button.add_css_class("flat");
    ui.sync_button.set_tooltip_text(Some("Sync now"));
    let clicked = ui.clone();
    ui.sync_button.connect_clicked(move |_| sync_now(&clicked));
    header.pack_start(&ui.sync_button);

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
                    // Whatever was wrong with it, it has just consented afresh.
                    ui.needs_reconnect.borrow_mut().remove(&email);
                    refresh_sidebar(&ui);
                    refresh_week(&ui);
                    // Its calendars are on screen; its events are not until something
                    // fetches them, and waiting for the timer means an empty week.
                    sync_now(&ui);
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

        heading.append(&avatar_for(&account, 24));

        let name = gtk::Label::new(Some(
            account
                .label
                .as_deref()
                .or(account.display_name.as_deref())
                .unwrap_or(&account.email),
        ));
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

        if ui.needs_reconnect.borrow().contains(&account.email) {
            // On its own row, not beside the name: inline it squeezed a 30-character address
            // down to an ellipsis, so the button said which account was broken by hiding it.
            //
            // The account's events stay on the grid meanwhile. They were real when they were
            // synced, and blanking them would lose more than it explains.
            let reconnect = gtk::Button::with_label("Reconnect");
            reconnect.add_css_class("suggested-action");
            reconnect.set_margin_start(12);
            reconnect.set_margin_end(12);
            reconnect.set_margin_top(6);
            reconnect.set_tooltip_text(Some(&format!(
                "Google no longer accepts the stored credentials for {}",
                account.email
            )));
            let ui = ui.clone();
            reconnect.connect_clicked(move |_| start_connect(&ui));
            group.append(&reconnect);
        }

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

            let reminders = gtk::MenuButton::builder()
                .icon_name("alarm-symbolic")
                .valign(Align::Center)
                .tooltip_text("When to be reminded about this calendar")
                .build();
            reminders.add_css_class("flat");
            {
                let content = gtk::Box::new(Orientation::Vertical, 6);
                content.set_margin_top(8);
                content.set_margin_bottom(8);
                content.set_margin_start(8);
                content.set_margin_end(8);
                let ui = ui.clone();
                let (acct, id) = (calendar.account.clone(), calendar.id.clone());
                content.append(&lead_control(
                    calendar.notify_lead_minutes,
                    "this account's reminder time",
                    move |minutes| {
                        let (acct, id) = (acct.clone(), id.clone());
                        apply(&ui, move |store| {
                            store.set_calendar_notify_lead(&acct, &id, minutes)
                        });
                    },
                ));
                reminders.set_popover(Some(&gtk::Popover::builder().child(&content).build()));
            }
            if calendar.notify_lead_minutes.is_some() {
                reminders.add_css_class("accent");
            }
            row.append(&reminders);

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

/// A reminder lead with an explicit "inherit" state.
///
/// A spin button cannot be empty, and zero is a real choice — "tell me as it starts" — so
/// inheriting needs a control of its own rather than being spelled as a blank or a zero.
fn lead_control(
    current: Option<i64>,
    inherits_from: &str,
    on_change: impl Fn(Option<i64>) + 'static,
) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Vertical, 6);

    let inherit = gtk::CheckButton::with_label(&format!("Use {inherits_from}"));
    inherit.set_active(current.is_none());

    let minutes = gtk::SpinButton::with_range(0.0, 60.0 * 24.0 * 14.0, 5.0);
    minutes.set_value(current.unwrap_or(10) as f64);
    minutes.set_sensitive(current.is_some());

    let label = gtk::Label::new(Some("Minutes before"));
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    label.set_halign(Align::Start);

    let on_change = Rc::new(on_change);
    {
        let (minutes, on_change) = (minutes.clone(), on_change.clone());
        inherit.connect_toggled(move |button| {
            minutes.set_sensitive(!button.is_active());
            on_change(if button.is_active() {
                None
            } else {
                Some(minutes.value() as i64)
            });
        });
    }
    {
        let (inherit, on_change) = (inherit.clone(), on_change.clone());
        minutes.connect_value_changed(move |spin| {
            if !inherit.is_active() {
                on_change(Some(spin.value() as i64));
            }
        });
    }

    row.append(&inherit);
    row.append(&label);
    row.append(&minutes);
    row
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

    content.append(&gtk::Separator::new(Orientation::Horizontal));
    {
        let ui = ui.clone();
        let email = account.email.clone();
        content.append(&lead_control(
            account.notify_lead_minutes,
            "the default reminder time",
            move |minutes| {
                let email = email.clone();
                apply(&ui, move |store| {
                    store.set_account_notify_lead(&email, minutes)
                });
            },
        ));
    }
    content.append(&gtk::Separator::new(Orientation::Horizontal));

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

/// How often to look for reminders that have come due.
///
/// A wall-clock comparison on a short tick, rather than one timer per event: a timer set for
/// three hours' time does not survive a suspend that spans it, and a calendar application
/// whose reminders stop working when the laptop lid closes is not one (SPEC §2.9).
const REMINDER_TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// How far back a fresh start looks for reminders it missed while not running.
const STARTUP_GRACE: i64 = 15 * 60;

fn start_reminders(ui: &Rc<Ui>) {
    let ui = ui.clone();
    glib::timeout_add_local(REMINDER_TICK, move || {
        deliver_due_reminders(&ui);
        glib::ControlFlow::Continue
    });
}

/// Everything scheduled in the near future, with the instant each should fire.
fn scheduled_reminders(ui: &Rc<Ui>, from: i64, to: i64) -> Vec<(crate::recur::Occurrence, i64)> {
    let Ok(store) = ui.store.lock() else {
        return Vec::new();
    };

    let leads: HashMap<String, Option<i64>> = match store.accounts() {
        Ok(accounts) => accounts
            .into_iter()
            .map(|account| (account.email, account.notify_lead_minutes))
            .collect(),
        Err(_) => return Vec::new(),
    };
    let mut calendar_leads: HashMap<(String, String), Option<i64>> = HashMap::new();
    for email in leads.keys() {
        if let Ok(calendars) = store.calendars(email) {
            for calendar in calendars {
                calendar_leads.insert(
                    (calendar.account.clone(), calendar.id.clone()),
                    calendar.notify_lead_minutes,
                );
            }
        }
    }

    let occurrences = occurrences_in_window(&store, from, to).unwrap_or_default();
    let zone = ui.week.zone();
    occurrences
        .into_iter()
        .filter(|occurrence| occurrence.event.status != "cancelled")
        .filter_map(|occurrence| {
            let lead = notify::lead_minutes(
                occurrence.event.reminder_minutes,
                calendar_leads
                    .get(&(
                        occurrence.event.account.clone(),
                        occurrence.event.calendar_id.clone(),
                    ))
                    .copied()
                    .flatten(),
                leads.get(&occurrence.event.account).copied().flatten(),
                ui.settings.lead_minutes,
            );
            let at = notify::notify_at(&occurrence, lead, ui.settings.all_day_hour, zone)?;
            Some((occurrence, at))
        })
        .collect()
}

fn deliver_due_reminders(ui: &Rc<Ui>) {
    let now = chrono::Utc::now().timestamp();
    let since = ui.reminded_to.get();
    if now <= since {
        // The clock went backwards — a correction, or a resume. Re-anchor rather than
        // replaying every reminder in between.
        ui.reminded_to.set(now);
        return;
    }

    // Wide enough on both sides that a long suspend still finds what it slept through, and
    // an all-day reminder set days ahead is already in the window.
    let scheduled = scheduled_reminders(ui, since - 7 * 24 * 3600, now + 7 * 24 * 3600);
    let zone = ui.week.zone();
    for occurrence in notify::due(&scheduled, since, now) {
        let title = if occurrence.event.summary.is_empty() {
            "(no title)"
        } else {
            &occurrence.event.summary
        };
        tracing::info!(event = %occurrence.event.id, "reminding");
        notify::show(title, &notify::body_for(occurrence, zone, now));
    }
    ui.reminded_to.set(now);
}

/// Put agenda in the system tray, and act on what the user does there.
///
/// A missing tray is not a failure: some sessions have no StatusNotifierItem host, and the
/// window is perfectly usable without one.
fn start_tray(ui: &Rc<Ui>) {
    let (handle, requests) = match tray::start(tray_line(ui)) {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "no system tray; carrying on without one");
            return;
        }
    };
    *ui.tray.borrow_mut() = Some(handle);

    // The tray runs on its own thread and must never touch a widget, so it posts requests
    // and the main loop acts on them.
    let pump = ui.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let ui = &pump;
        while let Ok(request) = requests.try_recv() {
            match request {
                tray::Request::ToggleWindow => toggle_window(ui),
                tray::Request::Quit => {
                    if let Some(app) = ui
                        .toasts
                        .root()
                        .and_downcast::<gtk::Window>()
                        .and_then(|window| window.application())
                    {
                        app.quit();
                    }
                }
            }
        }
        glib::ControlFlow::Continue
    });

    refresh_tray(ui);
}

fn toggle_window(ui: &Rc<Ui>) {
    let Some(window) = ui.toasts.root().and_downcast::<gtk::Window>() else {
        return;
    };
    if window.is_visible() {
        window.set_visible(false);
    } else {
        window.present();
    }
}

/// What the tray should say, for the week now loaded.
fn tray_line(ui: &Rc<Ui>) -> String {
    let now = chrono::Utc::now().timestamp();
    let occurrences = match ui.store.lock() {
        // A generous horizon: the next thing on the calendar may well be next week.
        Ok(store) => occurrences_in_window(&store, now, now + 14 * 24 * 3600).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    tray::next_event(&occurrences, now)
        .describe(ui.week.zone(), tray::next_start(&occurrences, now))
}

/// Redraw the tray, and arrange to do it again when the answer next changes.
fn refresh_tray(ui: &Rc<Ui>) {
    let line = tray_line(ui);
    if let Some(handle) = ui.tray.borrow().as_ref() {
        let line = line.clone();
        handle.update(move |item| item.set_line(line));
    }

    // Scheduled for the moment the answer changes rather than on a tick, so a tray showing
    // "in 3 hours" is not redrawing every second to say the same thing.
    let now = chrono::Utc::now().timestamp();
    let occurrences = match ui.store.lock() {
        Ok(store) => occurrences_in_window(&store, now, now + 14 * 24 * 3600).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let wake_in = tray::next_change(&occurrences, now)
        .map(|instant| (instant - now).clamp(1, 3_600))
        .unwrap_or(3_600);

    let ui = ui.clone();
    glib::timeout_add_local_once(std::time::Duration::from_secs(wake_in as u64), move || {
        refresh_tray(&ui);
    });
}

/// Run a sync pass, then schedule the next one.
///
/// Re-armed after each pass rather than on a fixed timer, so the interval can follow what
/// the last pass found: a minute while things are moving, five when they are not.
/// Sync immediately, cancelling whatever pass was scheduled.
///
/// Used by the refresh button and after a connect. Without this, a freshly connected account
/// shows its calendars but an empty week until the next tick — which, once the interval has
/// backed off, is up to five minutes of looking at nothing.
fn sync_now(ui: &Rc<Ui>) {
    if ui.syncing.get() {
        return;
    }
    if let Some(pending) = ui.pending_sync.borrow_mut().take() {
        pending.remove();
    }
    start_syncing(ui, scheduler::MIN_INTERVAL);
}

fn start_syncing(ui: &Rc<Ui>, interval: std::time::Duration) {
    ui.syncing.set(true);
    ui.sync_button.set_sensitive(false);
    let store = ui.store.clone();
    let credentials = ui.credentials.clone();
    let ui = ui.clone();

    runtime::spawn(
        async move { scheduler::sync_all(store, credentials).await },
        move |pass| {
            ui.syncing.set(false);
            ui.sync_button.set_sensitive(true);
            let previously = ui.needs_reconnect.borrow().clone();
            let now: HashSet<String> = pass.needs_reconnect.iter().cloned().collect();
            *ui.needs_reconnect.borrow_mut() = now.clone();

            for account in now.difference(&previously) {
                // Said once, when it becomes true. A toast on every pass would be a toast
                // every minute for as long as the account stays disconnected.
                ui.toasts
                    .add_toast(adw::Toast::new(&format!("{account} needs reconnecting")));
            }

            if pass.changed() || now != previously {
                refresh_sidebar(&ui);
                refresh_week(&ui);
                refresh_tray(&ui);
            }

            let next = scheduler::next_interval(interval, pass.changed());
            tracing::debug!(
                stored = pass.stored,
                deleted = pass.deleted,
                reconnect = now.len(),
                next_in = next.as_secs(),
                "sync pass complete"
            );
            let again = ui.clone();
            let pending = glib::timeout_add_local_once(next, move || {
                again.pending_sync.borrow_mut().take();
                start_syncing(&again, next);
            });
            *ui.pending_sync.borrow_mut() = Some(pending);
        },
    );
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

    // One filesystem check per account, not per event.
    let paths = Paths::from_env().ok();
    let pictures: HashMap<String, std::path::PathBuf> = accounts
        .keys()
        .filter_map(|email| {
            let path = paths.as_ref()?.avatar(email);
            path.exists().then_some((email.clone(), path))
        })
        .collect();

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
                    .or_else(|| account.display_name.clone())
                    .unwrap_or_else(|| account.email.clone()),
                picture: pictures.get(&account.email).cloned(),
            })
        })
        .collect())
}

/// The account's picture if one has been cached, its initials otherwise.
///
/// Falling back rather than waiting: a cache that is not there yet is a monogram, never a
/// blank space and never a network call from a paint.
pub fn avatar_for(account: &crate::store::Account, size: i32) -> adw::Avatar {
    let shown = account
        .label
        .as_deref()
        .or(account.display_name.as_deref())
        .unwrap_or(&account.email);
    let avatar = adw::Avatar::new(size, Some(shown), true);
    avatar.set_valign(Align::Center);

    if let Ok(paths) = Paths::from_env() {
        let path = paths.avatar(&account.email);
        if path.exists() {
            match gtk::gdk::Texture::from_filename(&path) {
                Ok(texture) => avatar.set_custom_image(Some(&texture)),
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "could not read a cached avatar")
                }
            }
        }
    }
    avatar
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
