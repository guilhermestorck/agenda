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
use super::vertical::{self, Core, HOUR_HEIGHT, day_height, hour_height, y_for};

/// Width of the hour axis down the left.
pub const AXIS_WIDTH: i32 = 56;
/// Columns at the widest span. Labels are built once at this count and hidden when the
/// span is narrower, rather than rebuilt on every switch.
const MAX_DAYS: usize = 7;
/// The hour the grid is scrolled to on open.
const FIRST_VISIBLE_HOUR: f64 = 7.0;
/// Narrowest a day column is allowed to get before the grid scrolls instead of squeezing.
/// Below roughly this, a column holds no readable title and the grid stops being a calendar.
pub const MIN_COLUMN_WIDTH: f64 = 110.0;

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
    /// How many minutes before the start the reminder fires, already resolved through
    /// `notify`'s cascade. `None` when nothing will fire.
    ///
    /// Resolved once, where the colours are resolved, so the band the grid draws and the
    /// notification the scheduler sends cannot disagree about when the user gets told.
    pub lead_minutes: Option<i64>,
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
    zone: Cell<Tz>,
    /// Hour labels, resized when compression changes rather than rebuilt.
    hours: Vec<gtk::Label>,
    /// Held so a caller can scroll to a given minute of the day through the mapping.
    scroller: gtk::ScrolledWindow,
    header_spacer: gtk::Box,
    all_day_spacer: gtk::Box,
    /// The optional second zone's readings, beside the first.
    secondary_hours: Vec<gtk::Label>,
    secondary_axis: gtk::Box,
    secondary_zone: Rc<Cell<Option<Tz>>>,
    /// Which hours hold an event, across every day on screen. Shared with the draw closure
    /// so the grid lines and the events cannot disagree about where an hour sits.
    occupied: Rc<RefCell<std::collections::BTreeSet<u32>>>,
    core: Rc<Cell<Core>>,
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
        for label in &headers {
            header_row.append(label);
        }

        // Kept out of the timed grid entirely: an all-day event has no position on an hour
        // axis, and squeezing it onto one is how it ends up looking like a midnight meeting.
        let all_day_row = gtk::Box::new(Orientation::Horizontal, 0);
        all_day_row.set_margin_top(2);
        all_day_row.set_margin_bottom(2);
        // A minimum height so the row stays a visible band even on a week with no all-day
        // events; a strip that appears and disappears reads as a layout glitch.
        all_day_row.set_size_request(-1, 24);

        let occupied = Rc::new(RefCell::new(std::collections::BTreeSet::new()));
        let core = Rc::new(Cell::new(Core::default()));

        // Left of the primary axis, so the local time stays adjacent to the grid it
        // labels and the foreign one reads as an annotation rather than the main clock.
        let secondary_axis = gtk::Box::new(Orientation::Vertical, 0);
        secondary_axis.set_size_request(AXIS_WIDTH, -1);
        secondary_axis.set_visible(false);
        let mut secondary_hours = Vec::with_capacity(24);
        for _ in 0..24 {
            let label = gtk::Label::new(None);
            label.add_css_class("dim-label");
            label.add_css_class("caption");
            label.set_valign(Align::Start);
            label.set_halign(Align::End);
            label.set_margin_end(6);
            label.set_size_request(-1, HOUR_HEIGHT as i32);
            secondary_axis.append(&label);
            secondary_hours.push(label);
        }

        let axis = gtk::Box::new(Orientation::Vertical, 0);
        axis.set_size_request(AXIS_WIDTH, -1);
        let mut hours = Vec::with_capacity(24);
        for hour in 0..24 {
            let label = gtk::Label::new(Some(&format!("{hour:02}:00")));
            label.add_css_class("dim-label");
            label.add_css_class("caption");
            label.set_valign(Align::Start);
            label.set_halign(Align::End);
            label.set_margin_end(6);
            label.set_size_request(-1, HOUR_HEIGHT as i32);
            axis.append(&label);
            hours.push(label);
        }

        let grid = gtk::DrawingArea::new();
        grid.set_hexpand(true);
        grid.set_content_height(day_height(&occupied.borrow(), core.get()) as i32);

        let canvas = gtk::Fixed::new();
        canvas.set_hexpand(true);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&grid));
        overlay.add_overlay(&canvas);

        let columns_box = gtk::Box::new(Orientation::Horizontal, 0);
        columns_box.append(&overlay);

        // The hour axis stays put while the day columns scroll sideways, so it keeps
        // labelling the rows it is next to. Only the columns go in the horizontal scroller.
        let column_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .child(&columns_box)
            .build();

        let scrollable = gtk::Box::new(Orientation::Horizontal, 0);
        scrollable.append(&secondary_axis);
        scrollable.append(&axis);
        scrollable.append(&column_scroller);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&scrollable)
            .build();

        // The day headings and the all-day band ride the same horizontal adjustment as the
        // columns below them. Previously they sat outside the scrolled area entirely and
        // could not shrink past their own text, so on a narrow window they overflowed and
        // the last days were clipped with no way to reach them.
        let header_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hadjustment(&column_scroller.hadjustment())
            .child(&header_row)
            .build();
        let all_day_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hadjustment(&column_scroller.hadjustment())
            .child(&all_day_row)
            .build();

        // Open on the working day rather than at midnight. The hours before dawn are the
        // least likely to hold anything, and scrolling past them on every launch is a chore.
        // Measured through the mapping, not multiplied: those early hours are compressed, so
        // multiplying scrolls far past where 07:00 actually sits.
        let adjustment = scroller.vadjustment();
        let scroll_occupied = occupied.clone();
        let scroll_core = core.clone();
        glib::idle_add_local_once(move || {
            adjustment.set_value(y_for(
                FIRST_VISIBLE_HOUR * 60.0,
                &scroll_occupied.borrow(),
                scroll_core.get(),
            ));
        });

        // Both rows are inset by the axis columns so their days line up with the grid's.
        let header_line = gtk::Box::new(Orientation::Horizontal, 0);
        let header_spacer = gtk::Box::new(Orientation::Horizontal, 0);
        header_spacer.set_size_request(AXIS_WIDTH, -1);
        header_line.append(&header_spacer);
        header_line.append(&header_scroller);

        let all_day_line = gtk::Box::new(Orientation::Horizontal, 0);
        let all_day_spacer = gtk::Box::new(Orientation::Horizontal, 0);
        all_day_spacer.set_size_request(AXIS_WIDTH, -1);
        all_day_line.append(&all_day_spacer);
        all_day_line.append(&all_day_scroller);

        let root = gtk::Box::new(Orientation::Vertical, 0);
        root.append(&header_line);
        root.append(&gtk::Separator::new(Orientation::Horizontal));
        root.append(&all_day_line);
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
            hours,
            scroller: scroller.clone(),
            header_spacer,
            all_day_spacer,
            secondary_hours,
            secondary_axis,
            secondary_zone: Rc::new(Cell::new(None)),
            occupied,
            core,
            items: RefCell::new(Vec::new()),
            zone: Cell::new(local_zone()),
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
        self.recompute_occupancy();
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
    /// Which hours hold an event, across every day on screen.
    ///
    /// Computed once per redraw over the whole span rather than per column: the hour axis is
    /// shared, so an hour is tall for every day or for none.
    fn recompute_occupancy(self: &Rc<Self>) {
        let start = self.start();
        let days = self.span.get().days();
        let items = self.items.borrow();
        let spans = items
            .iter()
            .filter(|item| !item.all_day)
            .flat_map(|item| segments(item.start_utc, item.end_utc, start, self.zone.get(), days))
            .map(|segment| (segment.top_minutes, segment.height_minutes));
        *self.occupied.borrow_mut() = vertical::occupied_hours(spans);
        self.resize_axis();
    }

    /// Match the hour labels and the canvas to the current mapping.
    fn resize_axis(self: &Rc<Self>) {
        let occupied = self.occupied.borrow();
        let core = self.core.get();
        for (hour, label) in self
            .hours
            .iter()
            .chain(self.secondary_hours.iter())
            .enumerate()
        {
            // Both columns share one mapping, so the second zone's readings line up with
            // the rows they annotate even where an hour is compressed.
            let hour = (hour % 24) as u32;
            label.set_size_request(-1, hour_height(hour, &occupied, core) as i32);
        }
        self.grid
            .set_content_height(day_height(&occupied, core) as i32);
        // Squeeze to fit, but only down to a point. Past this the grid scrolls sideways
        // rather than shrinking columns into unreadable slivers.
        self.grid
            .set_content_width((self.span.get().days() as f64 * MIN_COLUMN_WIDTH) as i32);
    }

    /// Scroll so `minute` of the day sits at the top.
    ///
    /// Measured through the mapping, not multiplied: the hours above may be compressed.
    pub fn scroll_to_minute(self: &Rc<Self>, minute: f64) {
        let target = y_for(minute, &self.occupied.borrow(), self.core.get());
        let adjustment = self.scroller.vadjustment();
        glib::idle_add_local_once(move || adjustment.set_value(target));
    }

    /// Show a second zone's readings beside the hour axis, or none.
    pub fn set_secondary_zone(self: &Rc<Self>, zone: Option<Tz>) {
        self.secondary_zone.set(zone);
        self.secondary_axis.set_visible(zone.is_some());
        // The headings are inset by however many axis columns are showing, or they stop
        // lining up with the days below them.
        let inset = if zone.is_some() {
            AXIS_WIDTH * 2
        } else {
            AXIS_WIDTH
        };
        for spacer in [&self.header_spacer, &self.all_day_spacer] {
            spacer.set_size_request(inset, -1);
        }
        self.refresh();
    }

    /// The hours drawn at full height whatever they hold.
    pub fn set_core_hours(self: &Rc<Self>, core: Core) {
        self.core.set(core);
        self.recompute_occupancy();
        self.place_items();
        self.grid.queue_draw();
    }

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
                for segment in segments(item.start_utc, item.end_utc, start, self.zone.get(), days)
                {
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
                for segment in segments(item.start_utc, item.end_utc, start, self.zone.get(), days)
                {
                    if segment.day == day {
                        placed.push((item, segment));
                    }
                }
            }

            let geometry: Vec<super::layout::Segment> =
                placed.iter().map(|(_, segment)| *segment).collect();
            for ((item, segment), (column, of)) in placed.iter().zip(columns(&geometry)) {
                // Never `duration × scale`: an event crossing the core boundary spans two
                // different hour heights, so it is measured as the distance between its ends.
                let occupied = self.occupied.borrow();
                let core = self.core.get();
                let top = y_for(segment.top_minutes, &occupied, core);
                let height = y_for(
                    segment.top_minutes + segment.height_minutes,
                    &occupied,
                    core,
                ) - top;
                let slot = column_width / of as f64;
                let x = day as f64 * column_width + column as f64 * slot + 1.0;

                // The reminder band, behind the event. A segment continuing from the
                // previous day starts at midnight, and `band` returns nothing there — so a
                // meeting running past midnight is not reminded about twice, without
                // needing to ask which day it began on.
                if let Some((band_top, band_height)) = item
                    .lead_minutes
                    .and_then(|lead| vertical::band(segment.top_minutes, lead, &occupied, core))
                {
                    let band = band_widget(&item.colors.fill);
                    band.set_size_request((slot as i32 - 2).max(1), (band_height as i32).max(1));
                    self.canvas.put(&band, x, band_top);
                }

                let widget = event_widget(item, height as i32, slot as i32);
                widget.set_size_request((slot as i32 - 2).max(1), (height as i32 - 1).max(1));
                self.canvas.put(&widget, x, top);
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
            // Headings share the grid's minimum column width, or the two disagree about how
            // wide a day is — which is what left them misaligned on a narrow window.
            label.set_size_request(MIN_COLUMN_WIDTH as i32, -1);
        }
        self.refresh();
    }

    pub fn days(&self) -> usize {
        self.span.get().days()
    }

    pub fn zone(&self) -> Tz {
        self.zone.get()
    }

    /// Change the zone the grid is drawn in. Events keep their instants; only where they
    /// land moves.
    pub fn set_display_zone(self: &Rc<Self>, zone: Tz) {
        self.zone.set(zone);
        self.recompute_occupancy();
        self.place_items();
        self.refresh();
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
        let occupied = self.occupied.clone();
        let core = self.core.clone();
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

                let occupied = occupied.borrow();
                let core = core.get();
                for hour in 0..=24 {
                    let y = y_for(f64::from(hour) * 60.0, &occupied, core).floor() + 0.5;
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
                    let y = y_for(minutes, &occupied, core);
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

        if let Some(other) = self.secondary_zone.get() {
            // Recomputed for the displayed day, because the gap between two zones changes
            // partway through the days when one leaves summer time before the other.
            for (label, reading) in
                self.secondary_hours
                    .iter()
                    .zip(secondary_hours(start, self.zone.get(), other))
            {
                label.set_text(&reading);
            }
        }

        self.title.set_text(&title_for(start, days));

        self.grid.queue_draw();
    }
}

/// The header's date range. A single day names itself rather than claiming a range, and a
/// range spanning two months names both — "14 – 20 September" is wrong when the 20th is in
/// October.
fn title_for(start: NaiveDate, days: usize) -> String {
    if days <= 1 {
        return start.format("%A %-d %B %Y").to_string();
    }
    let end = start + Duration::days(days as i64 - 1);
    if start.month() == end.month() {
        format!("{} – {}", start.format("%-d"), end.format("%-d %B %Y"))
    } else if start.year() == end.year() {
        format!("{} – {}", start.format("%-d %b"), end.format("%-d %b %Y"))
    } else {
        format!(
            "{} – {}",
            start.format("%-d %b %Y"),
            end.format("%-d %b %Y")
        )
    }
}

/// The translucent run-up to an event, from when its reminder fires to when it starts.
///
/// Fades out backwards, toward the reminder: strongest where the event begins, vanishing at
/// the moment the user will be told. That is the direction the user described — the colour
/// fading away until the notification time — and it reads as the event casting a shadow
/// forwards rather than a second block sitting above it.
fn band_widget(fill: &str) -> gtk::DrawingArea {
    let band = gtk::DrawingArea::new();
    let fill = fill.to_string();
    band.set_can_target(false);
    band.set_draw_func(move |_, context, width, height| {
        let Ok(rgba) = gtk::gdk::RGBA::parse(&fill) else {
            return;
        };
        let (r, g, b) = (
            f64::from(rgba.red()),
            f64::from(rgba.green()),
            f64::from(rgba.blue()),
        );
        let gradient = gtk::cairo::LinearGradient::new(0.0, 0.0, 0.0, f64::from(height));
        gradient.add_color_stop_rgba(0.0, r, g, b, 0.0);
        gradient.add_color_stop_rgba(1.0, r, g, b, 0.28);
        if context.set_source(&gradient).is_ok() {
            context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
            let _ = context.fill();
        }
    });
    band
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
/// The zone to draw in: the user's setting if they have one, the system's otherwise.
///
/// An unreadable or unknown name falls back rather than failing to start — a typo in a
/// preferences file should cost the user their preference, not their calendar. The fallback
/// is logged, because silently drawing in the wrong zone is the kind of bug that gets
/// noticed twice a year.
pub fn display_zone(configured: Option<&str>) -> Tz {
    match configured {
        Some(name) => match name.parse::<Tz>() {
            Ok(zone) => zone,
            Err(_) => {
                tracing::warn!(zone = name, "unknown timezone; using the system's");
                local_zone()
            }
        },
        None => local_zone(),
    }
}

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

/// The secondary zone's clock reading at each hour of `day` in the primary zone.
///
/// Computed per hour, never once. Two zones do not change their offsets on the same date —
/// the EU moves on the last Sunday of October and the United States on the first of
/// November — so for a week each autumn the gap between them is one thing in the morning and
/// another in the afternoon. A single cached offset is right for fifty weeks of the year and
/// quietly wrong for the other two.
pub fn secondary_hours(day: NaiveDate, primary: Tz, secondary: Tz) -> Vec<String> {
    use chrono::TimeZone;
    (0..24)
        .map(|hour| {
            let Some(naive) = day.and_hms_opt(hour, 0, 0) else {
                return String::new();
            };
            // A local hour that DST skipped has no instant; the one that repeats has two,
            // and the earlier reading is the one the grid is already drawing.
            match primary.from_local_datetime(&naive).earliest() {
                Some(instant) => instant
                    .with_timezone(&secondary)
                    .format("%H:%M")
                    .to_string(),
                None => String::new(),
            }
        })
        .collect()
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

    fn on(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_single_day_names_itself_rather_than_claiming_a_range() {
        assert_eq!(title_for(on("2026-09-14"), 1), "Monday 14 September 2026");
    }

    #[test]
    fn a_range_inside_one_month_names_the_month_once() {
        assert_eq!(title_for(on("2026-09-14"), 7), "14 – 20 September 2026");
        assert_eq!(title_for(on("2026-09-14"), 5), "14 – 18 September 2026");
        assert_eq!(title_for(on("2026-09-14"), 3), "14 – 16 September 2026");
    }

    #[test]
    fn a_range_crossing_a_month_names_both() {
        // "28 – 4 September" would be a lie about where the range ends.
        assert_eq!(title_for(on("2026-09-28"), 7), "28 Sep – 4 Oct 2026");
    }

    #[test]
    fn a_range_crossing_a_year_names_both_years() {
        assert_eq!(title_for(on("2026-12-28"), 7), "28 Dec 2026 – 3 Jan 2027");
    }

    #[test]
    fn a_second_zone_reads_across_from_the_first() {
        let day = NaiveDate::from_ymd_opt(2026, 9, 14).unwrap();
        let hours = secondary_hours(day, chrono_tz::Europe::Madrid, chrono_tz::America::New_York);
        // Madrid is CEST (+02:00) and New York EDT (-04:00) in September: six hours behind.
        assert_eq!(hours[12], "06:00");
        assert_eq!(hours[0], "18:00");
    }

    #[test]
    fn the_offset_is_recomputed_for_every_hour_not_cached_once() {
        // 25 October 2026: the EU leaves summer time at 03:00 local, the United States does
        // not until 1 November. So Madrid is six hours ahead of New York before the change
        // and five after — within the same day, on the same axis.
        let day = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let hours = secondary_hours(day, chrono_tz::Europe::Madrid, chrono_tz::America::New_York);

        assert_eq!(hours[1], "19:00", "01:00 CEST is 19:00 EDT the day before");
        assert_eq!(hours[12], "07:00", "12:00 CET is 07:00 EDT");
    }

    #[test]
    fn a_zone_with_no_transition_reads_evenly_all_day() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let hours = secondary_hours(day, chrono_tz::Europe::Madrid, chrono_tz::UTC);
        assert_eq!(hours.len(), 24);
        assert!(hours.iter().all(|reading| !reading.is_empty()));
    }

    #[test]
    fn a_configured_zone_wins_over_the_system() {
        assert_eq!(
            display_zone(Some("America/New_York")),
            chrono_tz::America::New_York
        );
        assert_eq!(display_zone(Some("Asia/Tokyo")), chrono_tz::Asia::Tokyo);
    }

    #[test]
    fn an_unknown_zone_falls_back_instead_of_failing_to_start() {
        // A typo should cost the preference, not the calendar.
        let fallback = display_zone(None);
        assert_eq!(display_zone(Some("Mars/Olympus_Mons")), fallback);
        assert_eq!(display_zone(Some("")), fallback);
    }

    #[test]
    fn no_configured_zone_leaves_the_system_zone_alone() {
        assert_eq!(display_zone(None), super::local_zone());
    }

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
