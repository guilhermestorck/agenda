//! The preferences dialog.
//!
//! Everything here was previously reachable only by hand-editing `settings.toml`, which is
//! not a control so much as a rumour. Changes apply immediately and are written back through
//! `config::set_setting`, which rewrites one line and leaves the rest of the file — comments
//! included — exactly as its author left it.

use adw::prelude::*;

/// Every zone chrono-tz knows, which is the IANA database.
fn zone_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = chrono_tz::TZ_VARIANTS
        .iter()
        .map(|zone| zone.name())
        .collect();
    names.sort_unstable();
    names
}

/// A dropdown of zones, with a leading "None" when the setting is optional.
fn zone_row(title: &str, subtitle: &str, current: Option<&str>, optional: bool) -> adw::ComboRow {
    let mut names = zone_names();
    if optional {
        names.insert(0, "None");
    }
    let model = gtk::StringList::new(&names);

    let row = adw::ComboRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_model(Some(&model));
    // Six hundred zones is a list nobody scrolls. Typing narrows it — but only with an
    // expression: enable_search on its own gives a search box with nothing to match
    // against, which looks like a working search that ignores everything typed into it.
    row.set_expression(Some(gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    )));
    row.set_enable_search(true);

    let selected = current
        .and_then(|name| names.iter().position(|candidate| *candidate == name))
        .unwrap_or(0);
    row.set_selected(selected as u32);
    row
}

/// Build the dialog. `on_change` is called with the key and its new value — `None` clears it.
pub fn dialog(
    timezone: Option<&str>,
    secondary: Option<&str>,
    core_start: u32,
    core_end: u32,
    accounts: &adw::PreferencesPage,
    on_change: impl Fn(&str, Option<String>) + Clone + 'static,
) -> adw::PreferencesDialog {
    let page = adw::PreferencesPage::new();
    page.set_title("Settings");
    page.set_icon_name(Some("preferences-system-symbolic"));

    let zones = adw::PreferencesGroup::new();
    zones.set_title("Time zones");

    let primary = zone_row(
        "Display time zone",
        "Where the grid places your events. Defaults to the system's.",
        timezone,
        false,
    );
    let second = zone_row(
        "Second time zone",
        "Shown beside the hour axis and on agenda rows.",
        secondary,
        true,
    );
    zones.add(&primary);
    zones.add(&second);

    let hours = adw::PreferencesGroup::new();
    hours.set_title("Working hours");
    hours.set_description(Some(
        "Hours outside this range are drawn at half height, unless something is happening in them.",
    ));
    let start = adw::SpinRow::with_range(0.0, 23.0, 1.0);
    start.set_title("Day starts");
    start.set_value(f64::from(core_start));
    let end = adw::SpinRow::with_range(1.0, 24.0, 1.0);
    end.set_title("Day ends");
    end.set_value(f64::from(core_end));
    hours.add(&start);
    hours.add(&end);

    page.add(&zones);
    page.add(&hours);

    {
        let on_change = on_change.clone();
        primary.connect_selected_notify(move |row| {
            if let Some(name) = selected_name(row) {
                on_change("timezone", Some(name));
            }
        });
    }
    {
        let on_change = on_change.clone();
        second.connect_selected_notify(move |row| {
            let chosen = selected_name(row);
            // "None" is the absence of a setting, not a zone called None.
            let value = chosen.filter(|name| name != "None");
            on_change("secondary_timezone", value);
        });
    }
    {
        let on_change = on_change.clone();
        let other = end.clone();
        start.connect_value_notify(move |row| {
            // The parser rejects a band that ends before it starts, so the dialog must not
            // offer one: a preference that refuses to save is worse than one that clamps.
            if row.value() >= other.value() {
                other.set_value(row.value() + 1.0);
            }
            on_change("core_hours_start", Some(format!("{}", row.value() as u32)));
        });
    }
    {
        let on_change = on_change.clone();
        let other = start.clone();
        end.connect_value_notify(move |row| {
            if row.value() <= other.value() {
                other.set_value(row.value() - 1.0);
            }
            on_change("core_hours_end", Some(format!("{}", row.value() as u32)));
        });
    }

    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Preferences");
    // Two pages give adw its own switcher along the top — the categories the dialog is
    // organised by, rather than one scrolling list of everything.
    dialog.add(&page);
    dialog.add(accounts);
    dialog
}

fn selected_name(row: &adw::ComboRow) -> Option<String> {
    row.selected_item()
        .and_downcast::<gtk::StringObject>()
        .map(|item| item.string().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz;

    #[test]
    fn every_zone_the_settings_parser_accepts_is_offered() {
        let names = zone_names();
        // A zone the dialog lists but the parser rejects would be a setting that silently
        // falls back; one it accepts but does not list cannot be chosen at all.
        for name in [
            "Europe/Madrid",
            "America/New_York",
            "Asia/Tokyo",
            "America/Sao_Paulo",
            "UTC",
        ] {
            assert!(names.contains(&name), "{name} is missing");
            assert!(name.parse::<Tz>().is_ok(), "{name} does not parse");
        }
    }

    #[test]
    fn the_zone_list_is_sorted_and_free_of_duplicates() {
        let names = zone_names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
    }

    #[test]
    fn none_is_not_a_real_zone_name() {
        // The optional row uses it as a sentinel, so a zone actually called None would make
        // it unselectable.
        assert!(!zone_names().contains(&"None"));
    }
}
