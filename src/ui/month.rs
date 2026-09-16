//! The month grid: whole weeks, one cell per day, events as chips.
//!
//! No hour axis, so none of `ui::vertical` applies here — the compressed quiet hours and the
//! lead-in band are the time grid's business. What this module needs is which cells an event
//! occupies, and `layout::segments` already answers that: it splits an occurrence into
//! per-day pieces clamped to a window, and does not care whether the window is seven days or
//! forty-two.

use chrono::{Datelike, Duration, NaiveDate};
use chrono_tz::Tz;

use super::layout::segments;
use super::span::monday_of;
use adw::prelude::*;
use gtk::gdk;
use gtk::glib;

use super::week::Item;

/// Six weeks, always. Five would fit some months and not others, and a grid that changes
/// height between September and October reflows the whole window on every navigation.
pub const CELLS: usize = 42;

/// The date the grid starts on: the Monday on or before the first of the month.
pub fn grid_start(any_day_in_month: NaiveDate) -> NaiveDate {
    let first = any_day_in_month.with_day(1).unwrap_or(any_day_in_month);
    monday_of(first)
}

/// The events in each of the forty-two cells, in start order.
///
/// A multi-day event appears in every cell it covers, which is what `segments` already
/// produces — one piece per day touched.
pub fn cells(items: &[Item], start: NaiveDate, zone: Tz) -> Vec<Vec<Item>> {
    let mut cells: Vec<Vec<Item>> = vec![Vec::new(); CELLS];
    for item in items {
        for segment in segments(item.start_utc, item.end_utc, start, zone, CELLS) {
            if let Some(cell) = cells.get_mut(segment.day) {
                cell.push(item.clone());
            }
        }
    }
    for cell in &mut cells {
        cell.sort_by(|a, b| {
            b.all_day
                .cmp(&a.all_day)
                .then(a.start_utc.cmp(&b.start_utc))
                .then_with(|| a.summary.cmp(&b.summary))
        });
    }
    cells
}

/// How many chips fit, and how many are left over.
///
/// The overflow is counted, never estimated: "+3 more" that is really four is worse than no
/// indicator at all, because the user stops checking.
pub fn visible_and_overflow(total: usize, room: usize) -> (usize, usize) {
    if total <= room {
        return (total, 0);
    }
    // One slot goes to the "+N more" chip itself, or the count would omit the event it
    // displaced.
    let shown = room.saturating_sub(1);
    (shown, total - shown)
}

/// Whether a cell belongs to the month being shown, rather than the days either side.
pub fn in_month(cell: usize, start: NaiveDate, month: u32) -> bool {
    (start + Duration::days(cell as i64)).month() == month
}

/// What the grid was last asked to show, kept so the chips can be re-placed once GTK has
/// given the cells a height.
struct Shown {
    items: Vec<Item>,
    start: NaiveDate,
    month: u32,
    zone: Tz,
}

/// The month grid widget.
pub struct Grid {
    root: gtk::Box,
    cells: Vec<gtk::Box>,
    headings: Vec<gtk::Label>,
    /// Held so the chips can be re-placed once GTK has allocated the cells. How many fit
    /// depends on the cell's height, which is zero until the first allocation.
    last: std::cell::RefCell<Option<Shown>>,
}

/// Roughly how tall one chip is, used to work out how many fit.
const CHIP_HEIGHT: i32 = 20;

impl Grid {
    pub fn new() -> std::rc::Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // Homogeneous, so every column is the same width. hexpand alone only shares out
        // what is left over after each child has claimed its natural width — which for a
        // cell means however wide its longest chip happens to be, and a calendar whose
        // columns move with the length of an event title is not a grid.
        let weekdays = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        weekdays.set_homogeneous(true);
        for label in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
            let heading = gtk::Label::new(Some(label));
            heading.set_hexpand(true);
            heading.add_css_class("dim-label");
            heading.add_css_class("caption");
            heading.set_margin_top(4);
            heading.set_margin_bottom(4);
            weekdays.append(&heading);
        }
        root.append(&weekdays);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let mut cells = Vec::with_capacity(CELLS);
        let mut headings = Vec::with_capacity(CELLS);
        for row in 0..6 {
            let week = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            week.set_homogeneous(true);
            week.set_vexpand(true);
            for column in 0..7 {
                let cell = gtk::Box::new(gtk::Orientation::Vertical, 1);
                cell.set_hexpand(true);
                cell.set_vexpand(true);
                cell.add_css_class("month-cell");

                let heading = gtk::Label::new(None);
                heading.set_xalign(0.0);
                heading.add_css_class("caption");
                heading.set_margin_start(4);
                cell.append(&heading);

                headings.push(heading);
                week.append(&cell);
                cells.push(cell);
                let _ = (row, column);
            }
            root.append(&week);
            if row < 5 {
                root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
        }

        let grid = std::rc::Rc::new(Self {
            root,
            cells,
            headings,
            last: std::cell::RefCell::new(None),
        });

        // A cell's height is zero until GTK has laid the window out, so a placement made
        // now fits nothing and reports every event as overflow. Re-place once the layout
        // has happened, and again whenever the grid is shown.
        //
        // ponytail: no hook for a live window resize — gtk::Box has no resize signal, so
        // the chips only re-fit on the next redraw. Add a sizing probe if that grates.
        let on_map = grid.clone();
        grid.root.connect_map(move |_| {
            let again = on_map.clone();
            glib::idle_add_local_once(move || again.replace_chips());
        });
        grid
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_items(
        self: &std::rc::Rc<Self>,
        items: &[Item],
        start: NaiveDate,
        month: u32,
        zone: Tz,
    ) {
        *self.last.borrow_mut() = Some(Shown {
            items: items.to_vec(),
            start,
            month,
            zone,
        });
        self.replace_chips();
        // Again once the cells have a height, which they do not yet.
        let again = self.clone();
        glib::idle_add_local_once(move || again.replace_chips());
    }

    fn replace_chips(&self) {
        let held = self.last.borrow();
        let Some(shown) = held.as_ref() else {
            return;
        };
        let (start, month, zone) = (shown.start, shown.month, shown.zone);
        let buckets = cells(&shown.items, start, zone);
        let today = chrono::Utc::now().with_timezone(&zone).date_naive();

        for (index, cell) in self.cells.iter().enumerate() {
            // Everything but the date heading, which is rebuilt in place.
            while let Some(child) = cell.last_child() {
                if child == self.headings[index].clone().upcast::<gtk::Widget>() {
                    break;
                }
                cell.remove(&child);
            }

            let date = start + Duration::days(index as i64);
            let heading = &self.headings[index];
            heading.set_text(&date.format("%-d").to_string());
            heading.remove_css_class("dim-label");
            heading.remove_css_class("accent");
            if !in_month(index, start, month) {
                heading.add_css_class("dim-label");
            }
            if date == today {
                heading.add_css_class("accent");
            }

            // How many chips fit is measured, not assumed: the cell's height depends on the
            // window, and a hard-coded count is wrong on every size but one.
            let available = (cell.height() - CHIP_HEIGHT).max(0);
            let room = ((available / CHIP_HEIGHT) as usize).max(1);
            let events = &buckets[index];
            let (shown, hidden) = visible_and_overflow(events.len(), room);

            for item in events.iter().take(shown) {
                cell.append(&chip(item));
            }
            if hidden > 0 {
                let more = gtk::Label::new(Some(&format!("+{hidden} more")));
                more.set_xalign(0.0);
                more.add_css_class("caption");
                more.add_css_class("dim-label");
                more.set_margin_start(4);
                cell.append(&more);
            }
        }
    }
}

/// One event in a cell: a colour bar and as much of the title as fits.
fn chip(item: &Item) -> gtk::Box {
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    chip.set_margin_start(2);
    chip.set_margin_end(2);

    let swatch = gtk::DrawingArea::new();
    swatch.set_size_request(3, -1);
    let fill = item.colors.fill.clone();
    swatch.set_draw_func(move |_, context, width, height| {
        if let Ok(rgba) = gdk::RGBA::parse(&fill) {
            context.set_source_rgb(
                f64::from(rgba.red()),
                f64::from(rgba.green()),
                f64::from(rgba.blue()),
            );
            context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
            let _ = context.fill();
        }
    });
    chip.append(&swatch);

    let label = gtk::Label::new(Some(&item.summary));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // An ellipsizing label still asks for its full text as its natural width. Capping it
    // stops one long title from arguing for a wider column even inside a homogeneous row.
    label.set_max_width_chars(1);
    label.add_css_class("caption");
    label.set_tooltip_text(Some(&format!("{} — {}", item.summary, item.account)));
    chip.append(&label);

    chip
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::style::Colors;

    const MADRID: Tz = chrono_tz::Europe::Madrid;

    fn on(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn at(local: &str) -> i64 {
        let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").unwrap();
        use chrono::TimeZone;
        MADRID
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn item(summary: &str, from: &str, to: &str, all_day: bool) -> Item {
        Item {
            summary: summary.to_string(),
            start_utc: at(from),
            end_utc: at(to),
            all_day,
            colors: Colors {
                fill: "#3584e4".to_string(),
                marker: "#3584e4".to_string(),
            },
            account: "work@example.com".to_string(),
            picture: None,
            lead_minutes: None,
        }
    }

    #[test]
    fn the_grid_starts_on_the_monday_on_or_before_the_first() {
        // September 2026 opens on a Tuesday, so the grid starts on 31 August.
        assert_eq!(grid_start(on("2026-09-20")), on("2026-08-31"));
    }

    #[test]
    fn a_month_that_opens_on_a_monday_starts_on_that_monday() {
        // June 2026 opens on a Monday: no leading days from May.
        assert_eq!(grid_start(on("2026-06-15")), on("2026-06-01"));
    }

    #[test]
    fn every_month_gets_the_same_forty_two_cells() {
        // February 2027 is 28 days opening on a Monday — the shortest possible grid, and it
        // still gets six rows so the layout never changes height.
        for day in ["2026-09-01", "2027-02-10", "2026-06-01", "2028-02-29"] {
            let cells = cells(&[], grid_start(on(day)), MADRID);
            assert_eq!(cells.len(), CELLS, "{day}");
        }
    }

    #[test]
    fn a_multi_day_event_appears_in_every_cell_it_covers() {
        // Thursday to Saturday, three cells.
        let items = vec![item(
            "conference",
            "2026-09-17 09:00:00",
            "2026-09-19 18:00:00",
            false,
        )];
        let start = grid_start(on("2026-09-01"));
        let cells = cells(&items, start, MADRID);
        let occupied: Vec<usize> = cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.is_empty())
            .map(|(index, _)| index)
            .collect();
        // 31 Aug is cell 0, so 17 September is cell 17.
        assert_eq!(occupied, vec![17, 18, 19]);
    }

    #[test]
    fn days_outside_the_month_are_still_filled_and_still_marked_as_outside() {
        let start = grid_start(on("2026-09-01"));
        assert!(!in_month(0, start, 9), "31 August is not September");
        assert!(in_month(1, start, 9), "1 September is");
        let items = vec![item(
            "august",
            "2026-08-31 10:00:00",
            "2026-08-31 11:00:00",
            false,
        )];
        assert!(!cells(&items, start, MADRID)[0].is_empty(), "still drawn");
    }

    #[test]
    fn overflow_is_counted_not_estimated() {
        assert_eq!(visible_and_overflow(3, 5), (3, 0));
        assert_eq!(visible_and_overflow(5, 5), (5, 0));
        // Six into five: one slot goes to the indicator, so four show and two are hidden.
        assert_eq!(visible_and_overflow(6, 5), (4, 2));
        assert_eq!(visible_and_overflow(8, 3), (2, 6));
    }

    #[test]
    fn the_overflow_count_always_adds_up() {
        for total in 0..20usize {
            for room in 1..8usize {
                let (shown, hidden) = visible_and_overflow(total, room);
                assert_eq!(shown + hidden, total, "total={total} room={room}");
                assert!(shown <= room, "overflowed the cell");
            }
        }
    }

    #[test]
    fn all_day_events_lead_a_cell() {
        let items = vec![
            item(
                "standup",
                "2026-09-16 09:00:00",
                "2026-09-16 09:15:00",
                false,
            ),
            item(
                "holiday",
                "2026-09-16 02:00:00",
                "2026-09-17 02:00:00",
                true,
            ),
        ];
        let cells = cells(&items, grid_start(on("2026-09-01")), MADRID);
        let names: Vec<&str> = cells[16].iter().map(|i| i.summary.as_str()).collect();
        assert_eq!(names.first(), Some(&"holiday"));
    }
}
