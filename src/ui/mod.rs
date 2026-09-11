//! The window.
//!
//! Everything here reads the store and never the network (SPEC §6). Connecting an account is
//! the one action that reaches out, and it does so through `runtime::spawn` so the window
//! stays responsive for however long the user spends at Google's consent screen.

pub mod week;

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use gtk::{Align, Orientation};

use crate::accounts::{self, Connected};
use crate::config::{Credentials, Paths};
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
        let week = ui.week.clone();
        button.connect_clicked(move |_| week.shift(weeks));
    }
    let week = ui.week.clone();
    today.connect_clicked(move |_| week.go_to_today());

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
        // The account marker. §10 leaves its final form to `views`; a swatch is enough to
        // tell accounts apart in a list, and the week grid will decide its own.
        heading.append(&swatch(account.color.as_deref()));
        let name = gtk::Label::new(Some(account.label.as_deref().unwrap_or(&account.email)));
        name.add_css_class("heading");
        name.set_halign(Align::Start);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        heading.append(&name);
        group.append(&heading);

        for calendar in calendars {
            let row = gtk::Box::new(Orientation::Horizontal, 8);
            row.set_margin_start(28);
            row.set_margin_end(12);
            row.set_margin_top(4);
            // Full fill resolution lands with the styling task; the rule that already holds
            // here is that the user's override wins over Google's colour.
            row.append(&swatch(
                calendar.user_color.as_deref().or(calendar.color.as_deref()),
            ));
            let label = gtk::Label::new(Some(&calendar.summary));
            label.set_halign(Align::Start);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if !calendar.visible {
                label.add_css_class("dim-label");
            }
            row.append(&label);
            group.append(&row);
        }
        ui.sidebar.append(&group);
    }
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

fn swatch(color: Option<&str>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(12);
    area.set_content_height(12);
    area.set_valign(Align::Center);
    let rgba = color
        .and_then(|color| gtk::gdk::RGBA::parse(color).ok())
        .unwrap_or_else(|| gtk::gdk::RGBA::parse("#77767b").expect("a literal colour parses"));
    area.set_draw_func(move |_, context, width, height| {
        context.set_source_rgba(
            rgba.red() as f64,
            rgba.green() as f64,
            rgba.blue() as f64,
            rgba.alpha() as f64,
        );
        let radius = f64::from(width.min(height)) / 2.0;
        context.arc(
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
            radius,
            0.0,
            std::f64::consts::TAU,
        );
        let _ = context.fill();
    });
    area
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
