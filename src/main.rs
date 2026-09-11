//! agenda — a GTK4/libadwaita calendar showing several Google accounts in one week.

use adw::prelude::*;
use gtk::glib;
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "io.github.guilhermestorck.agenda";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("agenda=info")),
        )
        .init();

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(agenda::ui::build);
    app.run()
}
