//! The compact day view opened from the tray.
//!
//! It is the time grid with one column and a bounded vertical window, not a second grid.
//! What is new here is only which slice of the day opens first.

use adw::prelude::*;
use chrono::Timelike;
use gtk::glib;

/// Minutes in a day.
const DAY: f64 = 24.0 * 60.0;

pub const DEFAULT_BEFORE_HOURS: f64 = 1.0;
pub const DEFAULT_AFTER_HOURS: f64 = 4.0;

/// The slice of the day to open on, in minutes past midnight.
///
/// One hour back and four forward by default. Near either end of the day the slice would run
/// off it, so it slides along the day rather than shrinking: a five-hour window that becomes
/// a one-hour window at 23:30 shows the user almost nothing, which is the opposite of the
/// point. It only shrinks if the whole day is shorter than the requested span.
pub fn window(now_minutes: f64, before_hours: f64, after_hours: f64) -> (f64, f64) {
    let span = ((before_hours + after_hours) * 60.0).clamp(0.0, DAY);
    let now = now_minutes.clamp(0.0, DAY);

    let mut start = now - before_hours * 60.0;
    if start < 0.0 {
        start = 0.0;
    }
    if start + span > DAY {
        start = DAY - span;
    }

    (start.max(0.0), (start + span).min(DAY))
}

/// The compact day view: the time grid at one column, opened from the tray.
///
/// A StatusNotifierItem menu is a D-Bus menu and cannot hold a widget, so this is its own
/// window rather than part of the tray item.
pub struct Popup {
    window: adw::Window,
    grid: std::rc::Rc<super::week::Week>,
    title: gtk::Label,
}

impl Popup {
    pub fn new(open_full: impl Fn() + 'static) -> std::rc::Rc<Self> {
        let grid = super::week::Week::new();
        grid.set_span(super::span::Span::Day);

        let title = gtk::Label::new(None);
        title.add_css_class("heading");

        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        navigation.add_css_class("linked");
        let previous = gtk::Button::from_icon_name("go-previous-symbolic");
        let next = gtk::Button::from_icon_name("go-next-symbolic");
        navigation.append(&previous);
        navigation.append(&next);
        let today = gtk::Button::with_label("Today");
        header.pack_start(&navigation);
        header.pack_start(&today);

        let open = gtk::Button::from_icon_name("view-fullscreen-symbolic");
        open.set_tooltip_text(Some("Open agenda"));
        header.pack_end(&open);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(grid.widget()));

        let window = adw::Window::builder()
            .title("agenda")
            .default_width(420)
            .default_height(560)
            .hide_on_close(true)
            .content(&toolbar)
            .build();

        let popup = std::rc::Rc::new(Self {
            window,
            grid,
            title,
        });

        for (button, presses) in [(&previous, -1_i64), (&next, 1)] {
            let popup = popup.clone();
            button.connect_clicked(move |_| {
                popup.grid.shift(presses);
                popup.refresh_title();
            });
        }
        let clone = popup.clone();
        today.connect_clicked(move |_| {
            clone.grid.go_to_today();
            clone.refresh_title();
        });
        let clone = popup.clone();
        open.connect_clicked(move |_| {
            clone.window.set_visible(false);
            open_full();
        });

        // Escape closes it. Without layer-shell there is no dismissal on focus loss, and a
        // popup that can only be closed by its title bar is not a popup.
        let controller = gtk::EventControllerKey::new();
        let clone = popup.clone();
        controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                clone.window.set_visible(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        popup.window.add_controller(controller);

        popup
    }

    pub fn grid(&self) -> &std::rc::Rc<super::week::Week> {
        &self.grid
    }

    fn refresh_title(&self) {
        let day = self.grid.start().format("%A %-d %B").to_string();
        self.title.set_text(&day);
        // Distinct from the main window's, so the taskbar and the window switcher can tell
        // the two apart.
        self.window.set_title(Some(&format!("agenda — {day}")));
    }

    /// Show the popup, always on today and always at the current time.
    ///
    /// Reopening returns to now rather than to wherever it was last scrolled: the tray is
    /// asked "what is next", and answering with yesterday afternoon is not an answer.
    pub fn present(&self, before_hours: f64, after_hours: f64) {
        self.grid.go_to_today();
        self.refresh_title();
        self.window.present();

        let now = chrono::Local::now();
        let minutes = f64::from(now.hour() * 60 + now.minute());
        let (start, _) = window(minutes, before_hours, after_hours);
        self.grid.scroll_to_minute(start);
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: f64) -> f64 {
        hour * 60.0
    }

    #[test]
    fn the_default_window_is_an_hour_back_and_four_forward() {
        let (start, end) = window(at(14.0), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
        assert_eq!((start, end), (at(13.0), at(18.0)));
    }

    #[test]
    fn early_morning_slides_the_window_rather_than_shrinking_it() {
        // 00:30 — an hour back is yesterday. The window starts at midnight and keeps its
        // five hours, because a window that shrinks to half an hour shows nothing.
        let (start, end) = window(at(0.5), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
        assert_eq!((start, end), (at(0.0), at(5.0)));
        assert_eq!(end - start, at(5.0));
    }

    #[test]
    fn late_evening_slides_the_window_back_off_the_end() {
        // 23:30 — four hours forward is tomorrow. The window ends at midnight and keeps its
        // five hours by starting at 19:00.
        let (start, end) = window(at(23.5), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
        assert_eq!((start, end), (at(19.0), at(24.0)));
        assert_eq!(end - start, at(5.0));
    }

    #[test]
    fn the_window_keeps_its_span_at_every_minute_of_the_day() {
        for minute in 0..(24 * 60) {
            let (start, end) = window(f64::from(minute), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
            assert_eq!(end - start, at(5.0), "shrank at minute {minute}");
            assert!(start >= 0.0 && end <= DAY, "ran off the day at {minute}");
        }
    }

    #[test]
    fn a_span_longer_than_a_day_is_the_whole_day() {
        let (start, end) = window(at(12.0), 20.0, 20.0);
        assert_eq!((start, end), (0.0, DAY));
    }

    #[test]
    fn a_configured_span_is_honoured() {
        let (start, end) = window(at(12.0), 2.0, 2.0);
        assert_eq!((start, end), (at(10.0), at(14.0)));
    }
}
