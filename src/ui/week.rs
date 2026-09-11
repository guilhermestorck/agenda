//! The week grid: seven day columns, an hour axis, and an all-day row above them.
//!
//! Chrome only. Events are placed on the `gtk::Fixed` that sits over the grid, which is why
//! the geometry below is public to the module: the same numbers decide where an event goes.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use chrono::{Datelike, Duration, Local, NaiveDate, Timelike, Weekday};
use gtk::{Align, Orientation};

/// Pixels per hour. The whole day is drawn and scrolled rather than collapsed to working
/// hours, because "working hours" is a preference nobody has expressed yet.
pub const HOUR_HEIGHT: f64 = 48.0;
/// Width of the hour axis down the left.
pub const AXIS_WIDTH: i32 = 56;
pub const DAYS: usize = 7;

pub struct Week {
    root: gtk::Box,
    title: gtk::Label,
    headers: Vec<gtk::Label>,
    /// Where event widgets land, in Task 12.
    pub canvas: gtk::Fixed,
    pub all_day_row: gtk::Box,
    grid: gtk::DrawingArea,
    /// The Monday the displayed week starts on.
    start: Rc<Cell<NaiveDate>>,
}

/// The Monday on or before `date`. Weeks start on Monday here; Sunday-first is a preference
/// nobody has asked for, and guessing it from the locale would be a guess.
fn monday_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday() as i64)
}

impl Week {
    pub fn new() -> Rc<Self> {
        let start = Rc::new(Cell::new(monday_of(Local::now().date_naive())));

        let title = gtk::Label::new(None);
        title.add_css_class("heading");

        let headers: Vec<gtk::Label> = (0..DAYS)
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

        let root = gtk::Box::new(Orientation::Vertical, 0);
        root.append(&header_row);
        root.append(&gtk::Separator::new(Orientation::Horizontal));
        root.append(&all_day_row);
        root.append(&gtk::Separator::new(Orientation::Horizontal));
        root.append(&scroller);

        let week = Rc::new(Self {
            root,
            title,
            headers,
            canvas,
            all_day_row,
            grid,
            start,
        });
        week.install_grid_drawing();
        week.refresh();
        week
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn title(&self) -> &gtk::Label {
        &self.title
    }

    /// The Monday of the displayed week.
    pub fn start(&self) -> NaiveDate {
        self.start.get()
    }

    pub fn shift(&self, weeks: i64) {
        self.start.set(self.start.get() + Duration::weeks(weeks));
        self.refresh();
    }

    pub fn go_to_today(&self) {
        self.start.set(monday_of(Local::now().date_naive()));
        self.refresh();
    }

    fn install_grid_drawing(self: &Rc<Self>) {
        let start = self.start.clone();
        self.grid
            .set_draw_func(move |area, context, width, height| {
                let width = f64::from(width);
                let column = width / DAYS as f64;

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
                for day in 1..DAYS {
                    let x = (day as f64 * column).floor() + 0.5;
                    context.move_to(x, 0.0);
                    context.line_to(x, f64::from(height));
                }
                let _ = context.stroke();

                // The current-time line, drawn only when today is on screen.
                let today = Local::now().date_naive();
                let offset = (today - start.get()).num_days();
                if (0..DAYS as i64).contains(&offset) {
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
        let start = self.start.get();
        let today = Local::now().date_naive();

        for (index, label) in self.headers.iter().enumerate() {
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
        let days: Vec<NaiveDate> = (0..DAYS as i64)
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
