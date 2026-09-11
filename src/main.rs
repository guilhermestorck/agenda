//! agenda — a GTK4/libadwaita calendar showing several Google accounts in one week.

use adw::prelude::*;
use gtk::glib;
use tracing_subscriber::EnvFilter;

use agenda::config::{Credentials, Paths};

const APP_ID: &str = "io.github.guilhermestorck.agenda";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("agenda=info")),
        )
        .init();

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(activate);
    app.run()
}

/// Activation rather than startup, so a second launch raises the existing window instead of
/// opening a rival copy of the same calendar.
fn activate(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        tracing::debug!("already running; presenting the existing window");
        window.present();
        return;
    }

    let content = adw::ToolbarView::new();
    content.add_top_bar(&adw::HeaderBar::new());
    content.set_content(Some(&startup_state()));

    adw::ApplicationWindow::builder()
        .application(app)
        .title("agenda")
        .default_width(1100)
        .default_height(760)
        .content(&content)
        .build()
        .present();
}

/// What the window shows before any account is connected. A user who has not yet created an
/// OAuth client is the *expected* first-run case, not a failure, so it gets an instruction
/// rather than an error.
fn startup_state() -> adw::StatusPage {
    let paths = match Paths::from_env() {
        Ok(paths) => paths,
        Err(error) => {
            tracing::error!(%error, "could not resolve the XDG directories");
            return status(
                "dialog-error-symbolic",
                "Cannot find your home directory",
                &format!("{error}"),
            );
        }
    };

    tracing::debug!(
        database = %paths.database().display(),
        "the store the UI will read; nothing creates it yet"
    );

    let oauth = paths.oauth();
    match Credentials::load(&oauth) {
        Ok(Some(credentials)) => {
            tracing::debug!(?credentials, "loaded OAuth client credentials");
            status(
                "x-office-calendar-symbolic",
                "No accounts connected",
                "Connecting a Google account is not implemented yet.",
            )
        }
        Ok(None) => {
            tracing::info!(path = %oauth.display(), "no OAuth client yet; showing onboarding");
            status(
                "dialog-information-symbolic",
                "Set up Google access",
                &format!(
                    "agenda needs a Google OAuth client, which only you can create.\n\n\
                     Follow docs/google-oauth-setup.md, then write the client ID and secret \
                     to {}.",
                    oauth.display()
                ),
            )
        }
        Err(error) => {
            tracing::warn!(%error, path = %oauth.display(), "OAuth client file is unusable");
            status(
                "dialog-warning-symbolic",
                "Check your Google credentials",
                &format!("{} could not be read: {error}", oauth.display()),
            )
        }
    }
}

fn status(icon: &str, title: &str, description: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(description)
        .build()
}
