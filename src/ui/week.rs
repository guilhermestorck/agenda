//! The week grid: seven day columns, an hour axis, and an all-day row above them.
//!
//! Chrome only. Events are placed on the `gtk::Fixed` that sits over the grid, which is why
//! the geometry below is public to the module: the same numbers decide where an event goes.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

use adw::prelude::*;
use chrono::{Datelike, Duration, Local, NaiveDate, Timelike, Weekday};
use chrono_tz::Tz;
use gtk::glib;
use gtk::{Align, Orientation};

use crate::accounts::style::{Colors, text_on};

use super::layout::{columns, segments};
use super::span::Span;

/// Pixels per hour. The whole day is drawn and scrolled rather than collapsed to working
/// hours, because "working hours" is a preference nobody has expressed yet.
pub const HOUR_HEIGHT: f64 = 48.0;
/// Width of the hour axis down the left.
pub const AXIS_WIDTH: i32 = 56;
/// Columns at the widest span. Labels are built once at this count and hidden when the
/// span is narrower, rather than rebuilt on every switch.
const MAX_DAYS: usize = 7;
/// The hour the grid is scrolled to on open.
const FIRST_VISIBLE_HOUR: f64 = 7.0;

/// One drawable event: everything the grid needs, with the store and Google already out of
/// the picture.
#[derive(Debug, Clone)]
pub struct Item {
    pub summary: String,
    pub start_utc: i64,
    pub end_utc: i64,
    pub all_day: bool,
    pub colors: Colors,
    /// Shown as the avatar's initials and in the tooltip, so an event's account is legible
    /// even when two accounts' colours are hard to tell apart in isolation.
    pub account: String,
    /// The account's cached profile picture, if one has been downloaded. Resolved once per
    /// redraw rather than per event, and never fetched here — the grid does no I/O.
    pub picture: Option<std::path::PathBuf>,
}

pub struct Week {
    root: gtk::Box,
    title: gtk::Label,
    headers: Vec<gtk::Label>,
    /// Where event widgets land, in Task 12.
    pub canvas: gtk::Fixed,
    pub all_day_row: gtk::Box,
    grid: gtk::DrawingArea,
    /// The day the user is looking at. The first column is derived from it and the span,
    /// because spans 1 and 3 anchor here while 5 and 7 anchor on this day's Monday.
    focus: Rc<Cell<NaiveDate>>,
    span: Rc<Cell<Span>>,
    items: RefCell<Vec<Item>>,
    /// The zone the grid is drawn in: the user's own, not any calendar's.
    zone: Tz,
    palette: gtk::CssProvider,
}

impl Week {
    pub fn new() -> Rc<Self> {
        let focus = Rc::new(Cell::new(Local::now().date_naive()));
        let span = Rc::new(Cell::new(Span::default()));

        let title = gtk::Label::new(None);
        title.add_css_class("heading");

        let headers: Vec<gtk::Label> = (0..MAX_DAYS)
            .map(|_| {
                let label = gtk::Label::new(None);
                label.set_hexpand(true);
                label.set_margin_top(6);
                label.set_margin_bottom(6);
                label
            })
            .collect();

        let header_row = gtk::Box::new(Orientation::Horizontal, 0);
        let spacer = gtk::Box::new(Orientation::Horizontal, 0);
        spacer.set_size_request(AXIS_WIDTH, -1);
        header_row.append(&spacer);
        for label in &headers {
            header_row.append(label);
        }

        // Kept out of the timed grid entirely: an all-day event has no position on an hour
        // axis, and squeezing it onto one is how it ends up looking like a midnight meeting.
        let all_day_row = gtk::Box::new(Orientation::Horizontal, 0);
        all_day_row.set_margin_start(AXIS_WIDTH);
        all_day_row.set_margin_top(2);
        all_day_row.set_margin_bottom(2);
        // A minimum height so the row stays a visible band even on a week with no all-day
        // events; a strip that appears and disappears reads as a layout glitch.
        all_day_row.set_size_request(-1, 24);

        let axis = gtk::Box::new(Orientation::Vertical, 0);
        axis.set_size_request(AXIS_WIDTH, -1);
        for hour in 0..24 {
            let label = gtk::Label::new(Some(&format!("{hour:02}:00")));
            label.add_css_class("dim-label");
            label.add_css_class("caption");
            label.set_valign(Align::Start);
            label.set_halign(Align::End);
            label.set_margin_end(6);
            label.set_size_request(-1, HOUR_HEIGHT as i32);
            axis.append(&label);
        }

        let grid = gtk::DrawingArea::new();
        grid.set_hexpand(true);
        grid.set_content_height((HOUR_HEIGHT * 24.0) as i32);

        let canvas = gtk::Fixed::new();
        canvas.set_hexpand(true);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&grid));
        overlay.add_overlay(&canvas);

        let scrollable = gtk::Box::new(Orientation::Horizontal, 0);
        scrollable.append(&axis);
        scrollable.append(&overlay);

        // The axis is inside the scrolled area so it moves with the hours it labels; the day
        // headers are outside it so they stay put.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&scrollable)
            .build();

        // Open on the working day rather than at midnight. The hours before dawn are the
        // least likely to hold anything, and scrolling past them on every launch is a chore.
        let adjustment = scroller.vadjustment();
        glib::idle_add_local_once(move || {
            adjustment.set_value(FIRST_VISIBLE_HOUR * HOUR_HEIGHT);
        });

        let root = gtk::Box::new(Orientation::Vertical, 0);
        root.append(&header_row);
        root.append(&gtk::Separator::new(Orientation::Horizontal));
        root.append(&all_day_row);
        root.append(&gtk::Separator::new(Orientation::Horizontal));
        root.append(&scroller);

        let palette = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &palette,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let week = Rc::new(Self {
            root,
            title,
            headers,
            canvas,
            all_day_row,
            grid,
            focus,
            span,
            items: RefCell::new(Vec::new()),
            zone: local_zone(),
            palette,
        });
        week.install_grid_drawing();
        week.install_relayout_on_resize();
        week.refresh();
        week
    }

    /// Replace everything on the grid. Called after a sync or a styling change; the week
    /// view never reads the store itself.
    pub fn set_items(self: &Rc<Self>, items: Vec<Item>) {
        *self.items.borrow_mut() = items;
        self.rebuild_palette();
        self.place_items();
    }

    fn install_relayout_on_resize(self: &Rc<Self>) {
        let week = self.clone();
        self.grid.connect_resize(move |_, _, _| week.place_items());
    }

    /// One stylesheet per redraw, with a class per distinct colour.
    ///
    /// Generated rather than set per widget: GTK4 deprecated per-widget style contexts, and
    /// a handful of classes is cheaper than a provider for every event on screen.
    fn rebuild_palette(&self) {
        let mut colors: BTreeSet<String> = BTreeSet::new();
        for item in self.items.borrow().iter() {
            colors.insert(item.colors.fill.clone());
            colors.insert(item.colors.marker.clone());
        }

        let mut css = String::new();
        for color in colors {
            let class = class_for(&color);
            css.push_str(&format!(
                ".{class} {{ background-color: {color}; color: {}; }}\n",
                text_on(&color)
            ));
        }
        css.push_str(
            ".agenda-event { border-radius: 5px; }\n             .agenda-event label { padding: 0 4px; }\n",
        );
        self.palette.load_from_string(&css);
    }

    /// Position every item on the grid for the current week and width.
    fn place_items(self: &Rc<Self>) {
        while let Some(child) = self.canvas.first_child() {
            self.canvas.remove(&child);
        }
        while let Some(child) = self.all_day_row.first_child() {
            self.all_day_row.remove(&child);
        }

        let start = self.start();
        let width = f64::from(self.grid.width());
        if width <= 0.0 {
            return;
        }
        let days = self.span.get().days();
        let column_width = width / days as f64;

        let items = self.items.borrow();

        // All-day events go in their own band; they have no position on an hour axis.
        for day in 0..days {
            let lane = gtk::Box::new(Orientation::Vertical, 2);
            lane.set_hexpand(true);
            lane.set_size_request((column_width as i32).max(1), -1);
            for item in items.iter().filter(|item| item.all_day) {
                for segment in segments(item.start_utc, item.end_utc, start, self.zone, days) {
                    if segment.day == day {
                        lane.append(&event_widget(item, 18, column_width as i32));
                    }
                }
            }
            self.all_day_row.append(&lane);
        }

        // Timed events, one day at a time so overlap is resolved within a column.
        for day in 0..days {
            let mut placed: Vec<(&Item, super::layout::Segment)> = Vec::new();
            for item in items.iter().filter(|item| !item.all_day) {
                for segment in segments(item.start_utc, item.end_utc, start, self.zone, days) {
                    if segment.day == day {
                        placed.push((item, segment));
                    }
                }
            }

            let geometry: Vec<super::layout::Segment> =
                placed.iter().map(|(_, segment)| *segment).collect();
            for ((item, segment), (column, of)) in placed.iter().zip(columns(&geometry)) {
                let height = segment.height_minutes / 60.0 * HOUR_HEIGHT;
                let slot = column_width / of as f64;
                let widget = event_widget(item, height as i32, slot as i32);
                widget.set_size_request((slot as i32 - 2).max(1), (height as i32 - 1).max(1));
                self.canvas.put(
                    &widget,
                    day as f64 * column_width + column as f64 * slot + 1.0,
                    segment.top_minutes / 60.0 * HOUR_HEIGHT,
                );
            }
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn title(&self) -> &gtk::Label {
        &self.title
    }

    /// The date of the first column, derived from the focused day and the span.
    pub fn start(&self) -> NaiveDate {
        self.span.get().start_for(self.focus.get())
    }

    pub fn span(&self) -> Span {
        self.span.get()
    }

    /// Switch span, keeping the focused day on screen.
    pub fn set_span(&self, span: Span) {
        self.span.set(span);
        for (index, label) in self.headers.iter().enumerate() {
            label.set_visible(index < span.days());
        }
        self.refresh();
    }

    pub fn days(&self) -> usize {
        self.span.get().days()
    }

    pub fn zone(&self) -> Tz {
        self.zone
    }

    /// Move by one press of previous/next. The distance is the span's, not the column
    /// count's — a working week steps a whole week or the next press starts it on a Saturday.
    pub fn shift(&self, presses: i64) {
        let step = self.span.get().step();
        self.focus
            .set(self.focus.get() + Duration::days(presses * step));
        self.refresh();
    }

    pub fn go_to_today(&self) {
        self.focus.set(Local::now().date_naive());
        self.refresh();
    }

    fn install_grid_drawing(self: &Rc<Self>) {
        let focus = self.focus.clone();
        let span = self.span.clone();
        self.grid
            .set_draw_func(move |area, context, width, height| {
                let width = f64::from(width);
                let days = span.get().days();
                let column = width / days as f64;

                // Taken from the widget's own foreground colour so the grid reads correctly in
                // both the light and dark Adwaita themes rather than being hard-coded to one.
                let line = area.color();
                context.set_source_rgba(
                    f64::from(line.red()),
                    f64::from(line.green()),
                    f64::from(line.blue()),
                    0.12,
                );
                context.set_line_width(1.0);

                for hour in 0..=24 {
                    let y = (f64::from(hour) * HOUR_HEIGHT).floor() + 0.5;
                    context.move_to(0.0, y);
                    context.line_to(width, y);
                }
                for day in 1..days {
                    let x = (day as f64 * column).floor() + 0.5;
                    context.move_to(x, 0.0);
                    context.line_to(x, f64::from(height));
                }
                let _ = context.stroke();

                // The current-time line, drawn only when today is on screen.
                let today = Local::now().date_naive();
                let offset = (today - span.get().start_for(focus.get())).num_days();
                if (0..days as i64).contains(&offset) {
                    let now = Local::now();
                    let minutes = f64::from(now.hour() * 60 + now.minute());
                    let y = minutes / 60.0 * HOUR_HEIGHT;
                    context.set_source_rgba(0.88, 0.11, 0.16, 0.9);
                    context.set_line_width(2.0);
                    context.move_to(offset as f64 * column, y);
                    context.line_to((offset as f64 + 1.0) * column, y);
                    let _ = context.stroke();
                }
            });
    }

    /// Redraw the labels and the grid for the current week.
    pub fn refresh(&self) {
        let start = self.start();
        let today = Local::now().date_naive();
        let days = self.span.get().days();

        for (index, label) in self.headers.iter().take(days).enumerate() {
            let date = start + Duration::days(index as i64);
            label.set_markup(&format!(
                "<b>{}</b>\n<span size=\"small\">{}</span>",
                weekday_name(date.weekday()),
                date.format("%-d %b")
            ));
            // Today is marked rather than merely present: a week view whose current day is
            // not obvious makes the user count columns.
            if date == today {
                label.add_css_class("accent");
            } else {
                label.remove_css_class("accent");
            }
        }

        let end = start + Duration::days(6);
        self.title.set_text(&if start.month() == end.month() {
            format!("{} – {}", start.format("%-d"), end.format("%-d %B %Y"))
        } else {
            format!("{} – {}", start.format("%-d %b"), end.format("%-d %b %Y"))
        });

        self.grid.queue_draw();
    }
}

/// A CSS class name for a colour. Hex digits only, because a class cannot contain a '#'.
fn class_for(color: &str) -> String {
    let sanitised: String = color
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("agenda-c{sanitised}")
}

/// The user's own zone. The grid is drawn in one zone — theirs — however many zones the
/// events themselves carry.
///
/// Read from the system rather than through a crate: `/etc/localtime` is a symlink into the
/// zoneinfo tree on every Linux this targets, and `chrono::Local` exposes an offset but not
/// the zone name that DST arithmetic needs.
fn local_zone() -> Tz {
    std::env::var("TZ")
        .ok()
        .or_else(|| {
            std::fs::read_link("/etc/localtime")
                .ok()
                .and_then(|path| zone_name_of(&path.to_string_lossy()))
        })
        .and_then(|name| name.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::UTC)
}

fn zone_name_of(link: &str) -> Option<String> {
    link.split_once("/zoneinfo/")
        .map(|(_, zone)| zone.to_string())
}

/// One event on the grid: a leading stripe in the account's colour, an avatar for the
/// account, and the title.
///
/// The stripe carries the account on every event, however small. The avatar appears only
/// when there is room for it, because a circle squeezed into a 20-minute slot pushes the
/// title out and tells the user less than the stripe already did.
fn event_widget(item: &Item, height: i32, width: i32) -> gtk::Widget {
    let row = gtk::Box::new(Orientation::Horizontal, 0);
    row.add_css_class("agenda-event");
    row.add_css_class(&class_for(&item.colors.fill));
    row.set_overflow(gtk::Overflow::Hidden);
    row.set_tooltip_text(Some(&format!("{}\n{}", item.summary, item.account)));

    let stripe = gtk::Box::new(Orientation::Vertical, 0);
    stripe.set_size_request(4, -1);
    stripe.add_css_class(&class_for(&item.colors.marker));
    row.append(&stripe);

    // Width matters as much as height: on a day split three ways the avatar would leave no
    // room for the title, and a title reduced to an ellipsis tells the user nothing.
    if height >= 36 && width >= 110 {
        let avatar = adw::Avatar::new(20, Some(&item.account), true);
        if let Some(picture) = &item.picture {
            match gtk::gdk::Texture::from_filename(picture) {
                Ok(texture) => avatar.set_custom_image(Some(&texture)),
                Err(error) => {
                    tracing::warn!(%error, path = %picture.display(), "could not read a cached avatar")
                }
            }
        }
        avatar.set_margin_start(4);
        avatar.set_valign(Align::Start);
        avatar.set_margin_top(2);
        row.append(&avatar);
    }

    let label = gtk::Label::new(Some(&item.summary));
    label.set_halign(Align::Start);
    label.set_valign(Align::Start);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.add_css_class("caption");
    label.set_margin_top(1);
    row.append(&label);

    row.upcast()
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::span::monday_of;

    #[test]
    fn the_zone_name_is_read_out_of_the_localtime_symlink() {
        assert_eq!(
            zone_name_of("/usr/share/zoneinfo/Europe/Madrid").as_deref(),
            Some("Europe/Madrid")
        );
        assert_eq!(
            zone_name_of("../usr/share/zoneinfo/America/Sao_Paulo").as_deref(),
            Some("America/Sao_Paulo")
        );
        assert_eq!(zone_name_of("/etc/something-else"), None);
    }

    #[test]
    fn this_machines_zone_resolves_to_a_real_one() {
        // A grid drawn in the wrong zone puts every event at the wrong hour, so this is
        // worth knowing at test time rather than by looking at a screenshot.
        let zone = local_zone();
        assert!(!zone.name().is_empty());
    }

    #[test]
    fn a_week_starts_on_the_monday_on_or_before_the_date() {
        let wednesday = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
        assert_eq!(
            monday_of(wednesday),
            NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
        );
    }

    #[test]
    fn a_monday_is_its_own_week_start() {
        let monday = NaiveDate::from_ymd_opt(2026, 9, 14).unwrap();
        assert_eq!(monday_of(monday), monday);
    }

    #[test]
    fn a_sunday_belongs_to_the_week_that_began_six_days_earlier() {
        // The off-by-one that puts Sunday in the wrong week is the classic one here.
        let sunday = NaiveDate::from_ymd_opt(2026, 9, 20).unwrap();
        assert_eq!(
            monday_of(sunday),
            NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
        );
    }

    #[test]
    fn the_week_containing_a_dst_transition_still_has_seven_days() {
        // 29 March 2026 is the Madrid spring forward. Calendar arithmetic on dates must not
        // care, and a 23-hour day must not turn into a six-day week.
        let start = monday_of(NaiveDate::from_ymd_opt(2026, 3, 29).unwrap());
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 3, 23).unwrap());
        let days: Vec<NaiveDate> = (0..Span::Week.days() as i64)
            .map(|d| start + Duration::days(d))
            .collect();
        assert_eq!(days.len(), 7);
        assert_eq!(days[6], NaiveDate::from_ymd_opt(2026, 3, 29).unwrap());
    }

    #[test]
    fn stepping_a_week_across_a_month_boundary_lands_on_a_monday() {
        let mut date = monday_of(NaiveDate::from_ymd_opt(2026, 9, 28).unwrap());
        for _ in 0..6 {
            date += Duration::weeks(1);
            assert_eq!(date.weekday(), Weekday::Mon, "{date} is not a Monday");
        }
    }
}
