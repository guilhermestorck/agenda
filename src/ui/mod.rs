//! The window.
//!
//! Everything here reads the store and never the network (SPEC §6). Connecting an account is
//! the one action that reaches out, and it does so through `runtime::spawn` so the window
//! stays responsive for however long the user spends at Google's consent screen.

pub mod agenda;
pub mod layout;
pub mod month;
pub mod preferences;
pub mod span;
pub mod tray_day;
pub mod vertical;
pub mod week;

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use gtk::glib;
use gtk::{Align, Orientation};

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Duration, TimeZone};

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
    /// Border colours for the account and calendar cards, rebuilt with the sidebar.
    sidebar_palette: gtk::CssProvider,
    toasts: adw::ToastOverlay,
    connect_button: gtk::Button,
    week: Rc<week::Week>,
    agenda: Rc<agenda::List>,
    month: Rc<month::Grid>,
    /// Built on first use: most sessions never click the tray.
    day_popup: RefCell<Option<Rc<tray_day::Popup>>>,
    /// Swaps between the time grid and the list views. Only one is ever populated.
    views: gtk::Stack,
    view: Cell<View>,
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
    settings: RefCell<notify::Settings>,
    settings_path: std::path::PathBuf,
    /// Where the last-used span and sidebar state are kept. Not `settings.toml`: the user
    /// writes that one, the application writes this one.
    view_state: std::path::PathBuf,
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

    match startup(&window) {
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
fn startup(window: &adw::ApplicationWindow) -> anyhow::Result<gtk::Widget> {
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
        sidebar_palette: gtk::CssProvider::new(),
        toasts: adw::ToastOverlay::new(),
        connect_button: gtk::Button::with_label("Connect account"),
        sync_button: gtk::Button::from_icon_name("view-refresh-symbolic"),
        pending_sync: RefCell::new(None),
        syncing: Cell::new(false),
        week: week::Week::new(),
        agenda: agenda::List::new(),
        month: month::Grid::new(),
        day_popup: RefCell::new(None),
        views: gtk::Stack::new(),
        view: Cell::new(View::Grid),
        needs_reconnect: RefCell::new(HashSet::new()),
        tray: RefCell::new(None),
        settings: RefCell::new(notify::Settings::load(&paths.settings())),
        settings_path: paths.settings(),
        view_state: paths.view_state(),
        // Backdated, so launching a few minutes after a reminder came due still tells the
        // user about the meeting they are about to be late for. `due` already drops anything
        // whose event has ended, so this cannot produce a flood of stale notices.
        reminded_to: Cell::new(chrono::Utc::now().timestamp() - STARTUP_GRACE),
    });

    let content = build_content(&ui, window);
    refresh_sidebar(&ui);
    refresh_week(&ui);
    tracing::debug!(
        lead = ui.settings.borrow().lead_minutes,
        all_day_hour = ui.settings.borrow().all_day_hour,
        "reminder settings"
    );
    start_syncing(&ui, scheduler::MIN_INTERVAL);
    start_tray(&ui);
    start_reminders(&ui);
    Ok(content)
}

fn build_content(ui: &Rc<Ui>, window: &adw::ApplicationWindow) -> gtk::Widget {
    let header = adw::HeaderBar::new();

    // Reachable at any time, not only on first run — SPEC §2.6.
    // Flat, and in the sidebar footer. It was the loudest widget in the window, wearing
    // suggested-action for something done four times ever.
    ui.connect_button.add_css_class("flat");

    let clicked = ui.clone();
    ui.connect_button
        .connect_clicked(move |_| start_connect(&clicked));

    ui.sync_button.add_css_class("flat");
    ui.sync_button.set_tooltip_text(Some("Sync now"));
    let clicked = ui.clone();
    ui.sync_button.connect_clicked(move |_| sync_now(&clicked));
    // Declared here so it is packed first and sits leftmost; its handler needs `split`,
    // which does not exist yet, and is connected further down.
    let reveal = gtk::ToggleButton::new();
    reveal.set_icon_name("sidebar-show-symbolic");
    reveal.add_css_class("flat");
    reveal.set_tooltip_text(Some("Show accounts"));
    header.pack_start(&reveal);

    // Collapses every account at once, or expands them if any are already collapsed. The
    // same pan carets the accounts carry, so the button shows the action it will take.
    let fold_all = gtk::Button::from_icon_name("pan-down-symbolic");
    fold_all.add_css_class("flat");
    fold_all.set_tooltip_text(Some("Collapse or expand every account"));
    {
        let ui = ui.clone();
        fold_all.connect_clicked(move |button| {
            let Ok(groups) = read_groups(&ui) else {
                return;
            };
            let state = crate::config::view_state(&ui.view_state);
            // Collapse unless everything already is, in which case expand — one button for
            // both directions, doing whichever leaves the sidebar different from now.
            let any_open = groups.iter().any(|(account, _)| {
                state
                    .get(&format!("collapsed:{}", account.email))
                    .map(String::as_str)
                    != Some("1")
            });
            for (account, _) in &groups {
                if let Err(error) = crate::config::set_view_state(
                    &ui.view_state,
                    &format!("collapsed:{}", account.email),
                    if any_open { "1" } else { "0" },
                ) {
                    tracing::warn!(error = %format!("{error:#}"), "could not remember an account");
                }
            }
            button.set_icon_name(if any_open {
                "pan-end-symbolic"
            } else {
                "pan-down-symbolic"
            });
            refresh_sidebar(&ui);
        });
    }
    header.pack_start(&fold_all);

    header.pack_start(&ui.sync_button);

    // The date range, when a view needs one. Week and day carry their dates in the column
    // headings, so it is left empty there and takes no space.
    header.pack_start(ui.week.title());

    let cog = gtk::Button::from_icon_name("emblem-system-symbolic");
    cog.add_css_class("flat");
    cog.set_tooltip_text(Some("Preferences"));
    {
        let ui = ui.clone();
        cog.connect_clicked(move |button| {
            let settings = ui.settings.borrow();
            let applied = ui.clone();
            let accounts = accounts_page(&ui);
            let dialog = preferences::dialog(
                preferences::Current {
                    timezone: settings.timezone.as_deref(),
                    secondary: settings.secondary_timezone.as_deref(),
                    core_start: settings.core_hours_start,
                    core_end: settings.core_hours_end,
                },
                &accounts,
                move |key, value| apply_setting(&applied, key, value),
            );
            drop(settings);
            dialog.present(Some(button));
        });
    }

    let previous = gtk::Button::from_icon_name("go-previous-symbolic");
    let next = gtk::Button::from_icon_name("go-next-symbolic");
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

    // Restored before the handler is connected, so restoring does not look like a click.
    let saved = crate::config::view_state(&ui.view_state);
    let span = saved
        .get("span")
        .and_then(|key| span::Span::from_key(key))
        .unwrap_or_default();
    let mut switcher_start = span::Span::ALL
        .iter()
        .position(|candidate| *candidate == span)
        .unwrap_or(0);
    match saved.get("span").map(String::as_str) {
        Some("month") => {
            ui.view.set(View::Month);
            switcher_start = span::Span::ALL.len();
        }
        Some("agenda") => {
            ui.view.set(View::Agenda);
            switcher_start = span::Span::ALL.len() + 1;
        }
        _ => {}
    }
    ui.week.set_span(span);
    ui.week
        .set_display_zone(week::display_zone(ui.settings.borrow().timezone.as_deref()));
    ui.week.set_secondary_zone(
        ui.settings
            .borrow()
            .secondary_timezone
            .as_deref()
            .and_then(|name| name.parse().ok()),
    );
    ui.week.set_core_hours(vertical::Core {
        start: ui.settings.borrow().core_hours_start,
        end: ui.settings.borrow().core_hours_end,
    });

    let mut labels: Vec<&str> = span::Span::ALL.iter().map(|span| span.label()).collect();
    labels.push("Month");
    labels.push("Agenda");
    let switcher = gtk::DropDown::from_strings(&labels);
    switcher.set_selected(switcher_start as u32);
    switcher.set_tooltip_text(Some("How many days to show"));
    let clone = ui.clone();
    switcher.connect_selected_notify(move |switcher| {
        let chosen = switcher.selected() as usize;
        let Some(span) = span::Span::ALL.get(chosen).copied() else {
            // Past the spans are the list views, which have no span of their own.
            let (view, key) = if chosen == span::Span::ALL.len() {
                (View::Month, "month")
            } else {
                (View::Agenda, "agenda")
            };
            clone.view.set(view);
            clone.views.set_visible_child_name(key);
            refresh_week(&clone);
            if let Err(error) = crate::config::set_view_state(&clone.view_state, "span", key) {
                tracing::warn!(error = %format!("{error:#}"), "could not remember the view");
            }
            return;
        };
        clone.view.set(View::Grid);
        clone.views.set_visible_child_name("grid");
        clone.week.set_span(span);
        refresh_week(&clone);
        if let Err(error) = crate::config::set_view_state(&clone.view_state, "span", span.key()) {
            // A span that fails to persist is a small loss; refusing to switch is a big one.
            tracing::warn!(error = %format!("{error:#}"), "could not remember the span");
        }
    });

    // Previous / Today / next in the middle, as the title widget — which is what a
    // GtkHeaderBar centres. The window's own title is dropped: the day columns already say
    // which dates are on screen, so it only repeated them.
    // Today sits between the arrows rather than after them: it is where you go back to, so
    // it belongs between the two directions you leave in.
    let centre = gtk::Box::new(Orientation::Horizontal, 0);
    centre.add_css_class("linked");
    centre.append(&previous);
    centre.append(&today);
    centre.append(&next);
    header.set_title_widget(Some(&centre));

    // pack_end fills from the right, so the cog ends up outermost and the switcher inside it.
    header.pack_end(&cog);
    header.pack_end(&switcher);

    ui.sidebar.set_margin_top(6);
    ui.sidebar.set_margin_bottom(6);
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .width_request(280)
        .child(&ui.sidebar)
        .build();

    // adw handles collapsing, the overlay and the swipe gesture. Hand-rolling any of that
    // over a gtk::Box was the previous arrangement and could not hide the sidebar at all.
    if let Some(display) = gtk::gdk::Display::default() {
        // USER rather than APPLICATION: libadwaita installs its own stylesheet above
        // APPLICATION, so rules here lost to `avatar.colorN` however they were written.
        gtk::style_context_add_provider_for_display(
            &display,
            &ui.sidebar_palette,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }

    // Nothing but the view. Connect account moved to Preferences → Accounts, sync to the
    // header: both act on accounts rather than describing them.
    let sidebar_root = gtk::Box::new(Orientation::Vertical, 0);
    sidebar_scroll.set_vexpand(true);
    sidebar_root.append(&sidebar_scroll);

    ui.views.add_named(ui.week.widget(), Some("grid"));
    ui.views.add_named(ui.month.widget(), Some("month"));
    ui.views.add_named(ui.agenda.widget(), Some("agenda"));
    // After the children exist, not before: naming a child of an empty stack does nothing,
    // and the first child added then wins.
    ui.views.set_visible_child_name(match ui.view.get() {
        View::Grid => "grid",
        View::Month => "month",
        View::Agenda => "agenda",
    });

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar_root)
        .content(&ui.views)
        .min_sidebar_width(280.0)
        .max_sidebar_width(320.0)
        .build();

    // Three states do not fit one toggle button: its meaning would change on every press
    // and there would be no way to skip a state. The menu names each one instead.
    // Shown or hidden. There is no third state now, so this is a boolean the toggle and
    // the breakpoint both drive, rather than an enum with a menu behind it.
    let apply = {
        let split = split.clone();
        let ui = ui.clone();
        std::rc::Rc::new(move |shown: bool| {
            split.set_show_sidebar(shown);
            if let Err(error) = crate::config::set_view_state(
                &ui.view_state,
                "sidebar",
                if shown { "shown" } else { "hidden" },
            ) {
                tracing::warn!(error = %format!("{error:#}"), "could not remember the sidebar");
            }
        })
    };

    apply(saved.get("sidebar").map(String::as_str) != Some("hidden"));

    // A window too narrow for the sidebar collapses it to an overlay rather than crushing
    // the grid. The user's own choice still wins until the window is resized again.
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        700.0,
        adw::LengthUnit::Px,
    ));
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(breakpoint);

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
/// The Accounts page of the preferences dialog.
///
/// Everything that *changes* an account or a calendar lives here. The sidebar used to carry
/// it all, which made a view of the calendar double as its control panel — so glancing at
/// what was on screen meant reading past colour pickers, reminder menus and a Reconnect
/// button.
fn accounts_page(ui: &Rc<Ui>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Accounts");
    page.set_icon_name(Some("system-users-symbolic"));

    let actions = adw::PreferencesGroup::new();
    let add = gtk::Button::with_label("Connect account");
    add.add_css_class("suggested-action");
    add.set_halign(Align::Start);
    {
        let ui = ui.clone();
        add.connect_clicked(move |_| start_connect(&ui));
    }
    actions.add(&add);
    page.add(&actions);

    let groups = match read_groups(ui) {
        Ok(groups) => groups,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "could not read the accounts");
            Vec::new()
        }
    };

    if groups.is_empty() {
        let empty = adw::PreferencesGroup::new();
        empty.set_description(Some("No accounts connected yet."));
        page.add(&empty);
        return page;
    }

    for (account, calendars) in groups {
        let group = adw::PreferencesGroup::new();
        group.set_title(
            account
                .label
                .as_deref()
                .or(account.display_name.as_deref())
                .unwrap_or(&account.email),
        );
        group.set_description(Some(&account.email));

        let header = gtk::Box::new(Orientation::Horizontal, 6);
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
        header.append(&account_color);

        let menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(Align::Center)
            .build();
        menu.add_css_class("flat");
        menu.set_popover(Some(&account_menu(ui, &account)));
        header.append(&menu);
        group.set_header_suffix(Some(&header));

        if ui.needs_reconnect.borrow().contains(&account.email) {
            // The account's events stay on the grid meanwhile. They were real when they were
            // synced, and blanking them would lose more than it explains.
            let row = adw::ActionRow::new();
            row.set_title("Needs reconnecting");
            row.set_subtitle("Google no longer accepts the stored credentials");
            let reconnect = gtk::Button::with_label("Reconnect");
            reconnect.add_css_class("suggested-action");
            reconnect.set_valign(Align::Center);
            let ui = ui.clone();
            reconnect.connect_clicked(move |_| start_connect(&ui));
            row.add_suffix(&reconnect);
            group.add(&row);
        }

        for calendar in calendars {
            let row = adw::ActionRow::new();
            row.set_title(&glib::markup_escape_text(&calendar.summary));

            let fill = color_button(
                calendar
                    .user_color
                    .as_deref()
                    .or(calendar.color.as_deref())
                    .or(account.color.as_deref()),
            );
            fill.set_valign(Align::Center);
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
            row.add_prefix(&fill);

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
            row.add_suffix(&reminders);

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
                row.add_suffix(&reset);
            }

            group.add(&row);
        }
        page.add(&group);
    }
    page
}

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

    // One border colour per card. Generated rather than fixed, because the colours are the
    // user's and change under us.
    let mut css = String::new();
    for (account, calendars) in &groups {
        for color in std::iter::once(account.color.as_deref().unwrap_or(DEFAULT_SWATCH)).chain(
            calendars.iter().map(|calendar| {
                calendar
                    .user_color
                    .as_deref()
                    .or(calendar.color.as_deref())
                    .or(account.color.as_deref())
                    .unwrap_or(DEFAULT_SWATCH)
            }),
        ) {
            // alpha() colours the border alone. Setting opacity on the widget would fade
            // its contents with it, and the text has to stay legible.
            css.push_str(&format!(
                ".card-{} {{ border: 1px solid alpha({color}, 0.5); \
                 background-color: alpha({color}, 0.3); border-radius: 8px; \
                 padding: {ROW_PADDING}px {PADDING}px; }}\n\
                 .account-card-{} {{ padding: {PADDING}px {CARD_EDGE}px; }}\n",
                crate::ui::week::class_for(color),
                crate::ui::week::class_for(color),
            ));
        }
    }
    // A row without a card carries a transparent border and the same padding, so it
    // occupies exactly the same box. Both kinds then take identical margins and line up in
    // both axes on their own — rather than one being hand-compensated for the other, which
    // is what drifted before.
    for (account, _) in &groups {
        let color = account.color.as_deref().unwrap_or(DEFAULT_SWATCH);
        css.push_str(&format!(
            // On the avatar node itself: adw draws its generated fill there as a
            // background-image, and a child selector matches nothing. border-radius keeps
            // it round, since replacing the background otherwise leaves a square.
            ".flat-avatar.avatar-{cls} {{ background-color: {color}; color: #ffffff; \
             border-radius: 9999px; font-weight: bold; }}\n",
            cls = crate::ui::week::class_for(color),
        ));
    }

    css.push_str(&format!(
        ".sidebar-account {{ font-size: 1.05em; font-weight: 700; }}\n\
         .sidebar-calendar {{ font-size: 0.95em; }}\n\
         .sidebar-row {{ border: 1px solid transparent; border-radius: 8px; \
          padding: {ROW_PADDING}px {PADDING}px; }}\n\
         .account-heading {{ padding: 0; }}\n"
    ));
    ui.sidebar_palette.load_from_string(&css);

    for (account, calendars) in groups {
        let group = gtk::Box::new(Orientation::Vertical, 0);
        group.set_margin_bottom(CARD_EDGE);
        // The account's block is a card outlined in its own colour, so where one account
        // ends and the next begins is visible rather than inferred from whitespace.
        let account_class =
            crate::ui::week::class_for(account.color.as_deref().unwrap_or(DEFAULT_SWATCH));
        group.add_css_class(&format!("card-{account_class}"));
        // Wider top and bottom, tighter sides, so the caret and the icons sit close to the
        // border while the calendars below still breathe.
        group.add_css_class(&format!("account-card-{account_class}"));
        group.set_margin_start(8);
        group.set_margin_end(8);

        let heading = gtk::Box::new(Orientation::Horizontal, 8);
        heading.add_css_class("account-heading");

        // Collapsing an account hides its calendars, not the account itself: with four
        // accounts and sixteen calendars the sidebar is mostly a list of things you are not
        // currently thinking about.
        let key = format!("collapsed:{}", account.email);
        let collapsed = crate::config::view_state(&ui.view_state)
            .get(&key)
            .map(String::as_str)
            == Some("1");
        let caret = gtk::Button::from_icon_name(if collapsed {
            "pan-end-symbolic"
        } else {
            "pan-down-symbolic"
        });
        caret.add_css_class("flat");
        caret.set_valign(Align::Center);
        caret.set_tooltip_text(Some(if collapsed {
            "Show this account's calendars"
        } else {
            "Hide this account's calendars"
        }));
        heading.append(&caret);
        // Less the row's own top padding, so the *visible* gap is the 12px asked for rather
        // than 12 plus however much padding the row happens to carry.
        heading.set_margin_bottom(ACCOUNT_TO_CALENDARS - PADDING);
        // No separate colour chip: the card's border already carries the account's colour,
        // and the avatar carries who it is — a picture when one has been fetched, initials
        // otherwise. Two marks for one account was one too many.
        heading.append(&avatar_for(&account, 24));

        let name = gtk::Label::new(Some(
            account
                .label
                .as_deref()
                .or(account.display_name.as_deref())
                .unwrap_or(&account.email),
        ));
        name.add_css_class("sidebar-account");
        name.set_halign(Align::Start);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_tooltip_text(Some(&account.email));
        heading.append(&name);

        // Whether an account is on the grid is a view question, so it stays here. Everything
        // that alters the account itself moved to Preferences → Accounts.
        let all_shown = calendars.iter().all(|calendar| calendar.visible);
        let eye = eye_button(all_shown, "account");
        {
            let ui = ui.clone();
            let ids: Vec<(String, String)> = calendars
                .iter()
                .map(|calendar| (calendar.account.clone(), calendar.id.clone()))
                .collect();
            eye.connect_clicked(move |_| {
                let (ids, visible) = (ids.clone(), !all_shown);
                apply(&ui, move |store| {
                    for (account, id) in &ids {
                        store.set_calendar_visible(account, id, visible)?;
                    }
                    Ok(())
                });
            });
        }
        reveal_on_hover(&heading, &eye);
        heading.append(&eye);
        group.append(&heading);

        // The calendars live in their own box so the caret has one thing to toggle.
        let calendar_list = gtk::Box::new(Orientation::Vertical, 0);
        calendar_list.set_visible(!collapsed);
        {
            let ui = ui.clone();
            let list = calendar_list.clone();
            let caret_clone = caret.clone();
            caret.connect_clicked(move |_| {
                let showing = !list.is_visible();
                list.set_visible(showing);
                caret_clone.set_icon_name(if showing {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                });
                caret_clone.set_tooltip_text(Some(if showing {
                    "Hide this account's calendars"
                } else {
                    "Show this account's calendars"
                }));
                if let Err(error) = crate::config::set_view_state(
                    &ui.view_state,
                    &key,
                    if showing { "0" } else { "1" },
                ) {
                    tracing::warn!(error = %format!("{error:#}"), "could not remember the account");
                }
            });
        }

        if ui.needs_reconnect.borrow().contains(&account.email) {
            // An icon beside the name rather than a line of its own: the state is about this
            // account, and a whole row of red for it crowded the sidebar. Read-only — fixing
            // it lives in Preferences — but still visible without going looking (§2.11).
            // A storm cloud, chosen by the user over the literal network-offline glyph.
            // Present in both Adwaita and Breeze, which is the bar an icon has to clear
            // here: an Adwaita-only name renders as a missing-image box under Breeze.
            let broken = gtk::Image::from_icon_name("weather-storm-symbolic");
            broken.add_css_class("error");
            broken.set_tooltip_text(Some(&format!(
                "{} needs reconnecting — Preferences → Accounts",
                account.email
            )));
            heading.append(&broken);
        }

        for (index, calendar) in calendars.into_iter().enumerate() {
            let first = index == 0;
            let row = gtk::Box::new(Orientation::Horizontal, 8);

            let own = calendar
                .user_color
                .as_deref()
                .or(calendar.color.as_deref())
                .unwrap_or(DEFAULT_SWATCH);
            let inherited = account.color.as_deref().unwrap_or(DEFAULT_SWATCH);

            // A calendar that keeps its account's colour needs no outline: the account's
            // card already says what colour its events are. One that has its own does,
            // because otherwise nothing on the row explains why its events look different.
            // A carded row is pushed in by its own border and padding, so it starts
            // further left by exactly that much. Both kinds then put their swatch — and so
            // their name — on the same vertical line, which is the point: whether a
            // calendar has its own colour should not move its label.
            // Both kinds of row carry the same padding and a border of the same width —
            // transparent on one, coloured on the other — so they need no compensating and
            // line up in both axes by construction.
            row.add_css_class("sidebar-row");
            if !own.eq_ignore_ascii_case(inherited) {
                row.add_css_class(&format!("card-{}", crate::ui::week::class_for(own)));
            }
            row.set_margin_top(if first { 0 } else { ROW_GAP });
            row.append(&swatch(Some(own)));

            let label = gtk::Label::new(Some(&calendar.summary));
            label.add_css_class("sidebar-calendar");
            label.set_halign(Align::Start);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_tooltip_text(Some(&calendar.summary));
            if !calendar.visible {
                label.add_css_class("dim-label");
            }
            row.append(&label);

            let eye = eye_button(calendar.visible, &calendar.summary);
            {
                let ui = ui.clone();
                let (account, id, visible) = (
                    calendar.account.clone(),
                    calendar.id.clone(),
                    !calendar.visible,
                );
                eye.connect_clicked(move |_| {
                    let (account, id) = (account.clone(), id.clone());
                    apply(&ui, move |store| {
                        store.set_calendar_visible(&account, &id, visible)
                    });
                });
            }
            reveal_on_hover(&row, &eye);
            row.append(&eye);

            calendar_list.append(&row);
        }
        group.append(&calendar_list);
        ui.sidebar.append(&group);
    }
}

/// The colour a card falls back to when neither the calendar nor its account has one.
const DEFAULT_SWATCH: &str = "#3584e4";

/// The sidebar's default padding.
const PADDING: i32 = 8;
/// How far a card's own controls sit from its border.
const CARD_EDGE: i32 = 4;
/// The gap between an account's name and its first calendar.
const ACCOUNT_TO_CALENDARS: i32 = 12;
/// The gap between one calendar row and the next.
const ROW_GAP: i32 = 2;
/// A row's vertical padding.
///
/// Deliberately smaller than `PADDING`. Between two stacked rows the vertical figure is
/// applied twice — once as the upper row's bottom padding, once as the lower row's top —
/// while the horizontal figure is applied once against the card's edge. Using 8 in both
/// axes therefore reads as a taller gap than a wide one, even though the number matches.
const ROW_PADDING: i32 = 3;

/// A read-only colour chip. The sidebar shows which colour a calendar is; changing it is
/// Preferences' business.
fn swatch(color: Option<&str>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_size_request(16, 16);
    area.set_valign(Align::Center);
    area.set_can_target(false);
    let color = color.unwrap_or("#3584e4").to_string();
    area.set_draw_func(move |_, context, width, height| {
        if let Ok(rgba) = gtk::gdk::RGBA::parse(&color) {
            context.set_source_rgb(
                f64::from(rgba.red()),
                f64::from(rgba.green()),
                f64::from(rgba.blue()),
            );
            context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
            let _ = context.fill();
        }
    });
    area
}

/// The one control the sidebar keeps: whether this is drawn on the grid.
fn eye_button(visible: bool, what: &str) -> gtk::Button {
    let icon = if visible {
        "view-reveal-symbolic"
    } else {
        "view-conceal-symbolic"
    };
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("flat");
    button.set_valign(Align::Center);
    button.set_tooltip_text(Some(&if visible {
        format!("Hide {what}")
    } else {
        format!("Show {what}")
    }));
    button
}

/// Show `control` only while the pointer is over `row`.
///
/// Opacity rather than visibility: hiding it outright would reflow the row on every hover,
/// and a sidebar that twitches as the pointer crosses it is worse than one slightly busier.
/// A hidden calendar keeps its eye showing regardless, or there would be no way to find the
/// thing you turned off.
fn reveal_on_hover(row: &gtk::Box, control: &gtk::Button) {
    let concealed = control.icon_name().as_deref() == Some("view-conceal-symbolic");
    control.set_opacity(if concealed { 1.0 } else { 0.0 });

    let motion = gtk::EventControllerMotion::new();
    let entered = control.clone();
    motion.connect_enter(move |_, _, _| entered.set_opacity(1.0));
    let left = control.clone();
    motion.connect_leave(move |_| {
        if !concealed {
            left.set_opacity(0.0);
        }
    });
    row.add_controller(motion);
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
                ui.settings.borrow().lead_minutes,
            );
            let at = notify::notify_at(&occurrence, lead, ui.settings.borrow().all_day_hour, zone)?;
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
                tray::Request::ShowDay => show_day(ui),
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

/// Open the compact day view from the tray.
///
/// Built on first use and reused after, so a session that never touches the tray never pays
/// for it, and one that does keeps the same window rather than stacking new ones.
fn show_day(ui: &Rc<Ui>) {
    let existing = ui.day_popup.borrow().clone();
    let popup = match existing {
        Some(popup) => popup,
        None => {
            let full = ui.clone();
            let popup = tray_day::Popup::new(move || toggle_window(&full));
            // The same core band as the main grid: two views of one day that compress
            // different hours would be two different calendars.
            popup.grid().set_core_hours(vertical::Core {
                start: ui.settings.borrow().core_hours_start,
                end: ui.settings.borrow().core_hours_end,
            });
            popup
                .grid()
                .set_display_zone(week::display_zone(ui.settings.borrow().timezone.as_deref()));
            *ui.day_popup.borrow_mut() = Some(popup.clone());
            popup
        }
    };

    if popup.is_visible() {
        return;
    }
    popup.present(
        tray_day::DEFAULT_BEFORE_HOURS,
        tray_day::DEFAULT_AFTER_HOURS,
    );
    refresh_day_popup(ui, &popup);
}

/// Fill the popup from the same store and the same rules as the main window, so two views of
/// the same day cannot disagree.
fn refresh_day_popup(ui: &Rc<Ui>, popup: &Rc<tray_day::Popup>) {
    let grid = popup.grid();
    let zone = grid.zone();
    let start = grid.start();
    let Some(from) = zone
        .from_local_datetime(&start.and_hms_opt(0, 0, 0).expect("midnight exists"))
        .earliest()
    else {
        return;
    };
    let to = from + Duration::days(1);
    match collect_items(ui, from.timestamp(), to.timestamp()) {
        Ok(items) => {
            tracing::debug!(day = %start, events = items.len(), "redrew the tray day");
            grid.set_items(items);
        }
        Err(error) => tracing::error!(error = %format!("{error:#}"), "could not read the day"),
    }
}

/// Apply a preference and write it back, without a restart.
///
/// The write preserves the rest of `settings.toml` — comments included — because the user
/// owns that file and may well have opened it themselves.
fn apply_setting(ui: &Rc<Ui>, key: &str, value: Option<String>) {
    if let Err(error) = crate::config::set_setting(&ui.settings_path, key, value.as_deref()) {
        tracing::warn!(error = %format!("{error:#}"), key, "could not save a preference");
    }

    {
        let mut settings = ui.settings.borrow_mut();
        match key {
            "timezone" => settings.timezone = value.clone(),
            "secondary_timezone" => settings.secondary_timezone = value.clone(),
            "core_hours_start" => {
                if let Some(hour) = value.as_deref().and_then(|v| v.parse().ok()) {
                    settings.core_hours_start = hour;
                }
            }
            "core_hours_end" => {
                if let Some(hour) = value.as_deref().and_then(|v| v.parse().ok()) {
                    settings.core_hours_end = hour;
                }
            }
            _ => {}
        }
    }

    let settings = ui.settings.borrow();
    ui.week
        .set_display_zone(week::display_zone(settings.timezone.as_deref()));
    ui.week.set_secondary_zone(
        settings
            .secondary_timezone
            .as_deref()
            .and_then(|name| name.parse().ok()),
    );
    ui.week.set_core_hours(vertical::Core {
        start: settings.core_hours_start,
        end: settings.core_hours_end,
    });
    drop(settings);
    refresh_week(ui);
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
    let to = from + Duration::days(ui.week.days() as i64);

    // Each view asks for its own window. The grid's seven days are not the agenda's month
    // and not the month grid's forty-two.
    let (from, to) = if ui.view.get() == View::Month {
        let grid_start = month::grid_start(start);
        match zone
            .from_local_datetime(&grid_start.and_hms_opt(0, 0, 0).expect("midnight exists"))
            .earliest()
        {
            Some(midnight) => (midnight, midnight + Duration::days(month::CELLS as i64)),
            None => (from, to),
        }
    } else {
        (from, to)
    };

    // The agenda looks a month ahead from the start of today rather than at the grid's
    // span: a list of "what is next" that stops on Sunday is not what is next.
    let (from, to) = if ui.view.get() == View::Agenda {
        let today = chrono::Local::now().date_naive();
        match zone
            .from_local_datetime(&today.and_hms_opt(0, 0, 0).expect("midnight exists"))
            .earliest()
        {
            Some(midnight) => (midnight, midnight + Duration::days(30)),
            None => (from, to),
        }
    } else {
        (from, to)
    };

    let items = match collect_items(ui, from.timestamp(), to.timestamp()) {
        Ok(items) => items,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "could not read the week");
            Vec::new()
        }
    };
    match ui.view.get() {
        View::Grid => {
            tracing::debug!(week = %start, events = items.len(), "redrew the week");
            // The column headings carry the dates, so the header says nothing.
            ui.week.title().set_text("");
            ui.week.set_items(items);
        }
        View::Month => {
            let grid_start = month::grid_start(start);
            tracing::debug!(month = %start.format("%B %Y"), events = items.len(), "redrew the month");
            ui.week.title().set_text(&start.format("%B %Y").to_string());
            ui.month.set_items(&items, grid_start, start.month(), zone);
        }
        View::Agenda => {
            tracing::debug!(events = items.len(), "redrew the agenda");
            // The grid's date range is not this view's range, and leaving it up claims the
            // list stops on Sunday when it runs a month.
            // The same formatter the grid used, rather than a second one that would drift:
            // it names both months when a range crosses one, and both years across a year.
            ui.week
                .title()
                .set_text(&week::title_for(from.date_naive(), 30));
            let secondary = ui
                .settings
                .borrow()
                .secondary_timezone
                .as_deref()
                .and_then(|name| name.parse().ok());
            ui.agenda.set_items(&items, zone, secondary);
        }
    }
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
            // The same call the scheduler makes. Two resolutions of the same cascade would
            // eventually disagree, and a band promising a reminder that never comes is
            // worse than no band.
            let lead = notify::lead_minutes(
                occurrence.event.reminder_minutes,
                calendar.notify_lead_minutes,
                account.notify_lead_minutes,
                ui.settings.borrow().lead_minutes,
            );
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
                // All-day events take their reminder from notify's own all_day_hour rule,
                // which has no place on an hour axis — the band is for timed events.
                lead_minutes: (!occurrence.event.all_day).then_some(lead),
            })
        })
        .collect())
}

/// The account's picture if one has been cached, its initials otherwise.
///
/// Falling back rather than waiting: a cache that is not there yet is a monogram, never a
/// blank space and never a network call from a paint.
pub fn avatar_for(account: &crate::store::Account, size: i32) -> gtk::Widget {
    let shown = account
        .label
        .as_deref()
        .or(account.display_name.as_deref())
        .unwrap_or(&account.email);

    let cached = Paths::from_env()
        .ok()
        .map(|paths| paths.avatar(&account.email));
    if let Some(path) = cached.filter(|path| path.exists()) {
        match gtk::gdk::Texture::from_filename(&path) {
            Ok(texture) => {
                let avatar = adw::Avatar::new(size, Some(shown), true);
                avatar.set_custom_image(Some(&texture));
                avatar.set_valign(Align::Center);
                return avatar.upcast();
            }
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not read a cached avatar")
            }
        }
    }

    // Drawn here rather than left to adw::Avatar, whose generated fill is a gradient that
    // reads as a shadow at this size. Five attempts to override it in CSS all lost to its
    // own `avatar.colorN` rule — at matching specificity and above its priority — so the
    // fallback is a plain circle instead of a fight with the stylesheet.
    let initials: String = shown
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let label = gtk::Label::new(Some(if initials.is_empty() { "?" } else { &initials }));
    label.set_size_request(size, size);
    label.set_valign(Align::Center);
    label.add_css_class("flat-avatar");
    if let Some(color) = account.color.as_deref() {
        label.add_css_class(&format!("avatar-{}", crate::ui::week::class_for(color)));
    }
    label.upcast()
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

/// Which view is on screen. The spans all share the time grid; the list views do not.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum View {
    Grid,
    Month,
    Agenda,
}
