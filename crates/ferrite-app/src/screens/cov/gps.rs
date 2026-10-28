//! The GPS pages (chapter 28): position in each format and moving, satellites, waypoints and go to one.

use super::*;
use crate::chan::{Channel, ChannelStore, Name};
use crate::data::{Data, Fix};
use crate::screens::gps::{Gps, Page};

/// A store with a fix and nothing else: no waypoints, no satellites.
struct OnlyFix(Option<Fix>);

impl Data for OnlyFix {
    fn fix(&self) -> Option<Fix> {
        self.0
    }
}

impl ChannelStore for OnlyFix {
    fn zones(&self) -> u8 {
        0
    }
    fn zone_name(&self, _: u8) -> Option<Name> {
        None
    }
    fn zone_len(&self, _: u8) -> u16 {
        0
    }
    fn zone_at(&self, _: u8, _: u16) -> Option<Channel> {
        None
    }
    fn count(&self) -> u16 {
        0
    }
    fn data(&self) -> &dyn Data {
        self
    }
}

fn moving(t: &mut T) {
    let mut f = Sample::sample_fix().unwrap();
    (f.speed_kmh, f.heading) = (48, 90);
    t.s.fix.set(Some(f));
}

fn page(t: &T) -> Page {
    match t.screen() {
        Screen::Gps(g) => g.page,
        s => panic!("not GPS: {}", s.name()),
    }
}

/// GPS-15: the position page as a grid square (the coordinates small under it), moving: course by compass, the
/// readback's position lines; the clock not set; GPS off.
#[test]
fn position_page() {
    let mut t = T::new();
    t.app.set.coord_fmt = 3;
    moving(&mut t);
    t.open(Dest::GpsPosition);
    t.reads("title=\"GPS\"");
    t.shows("E 90\u{b0}");
    let grid = String::from_utf8_lossy(&crate::screens::lists::grid(t.s.fix.get().unwrap().pos)).into_owned();
    t.shows(&grid);
    t.reads("gps");
    t.dev.clock_min = None;
    t.draw();
    t.shows("--:--");
    t.dev.gps = ferrite_core::msg::GpsState::Off;
    t.draw();
    t.shows("GPS off");
}

/// GPS-10: # marks a waypoint here, refused without a fix.
#[test]
fn mark_needs_a_fix() {
    let mut t = T::new();
    t.open(Dest::GpsPosition);
    t.s.fix.set(None);
    t.tap(Key::Hash);
    assert_eq!(t.toast(), "No GPS fix / can't mark here");
}

/// GPS-15: \u{25c0}\u{25b6} go between position and satellites; with no fix the sky says so; with none in view, says that.
#[test]
fn satellites_page() {
    let mut t = T::new();
    t.open(Dest::GpsPosition);
    t.tap(Key::Right);
    assert_eq!(page(&t), Page::Satellites);
    t.reads("title=\"Sky\"");
    t.knob(1); // the knob does nothing here
    assert_eq!(page(&t), Page::Satellites);
    t.dev.gps = ferrite_core::msg::GpsState::Searching;
    t.draw();
    t.shows("no fix");
    t.tap(Key::Left);
    assert_eq!(page(&t), Page::Position);
    let st = OnlyFix(None);
    t.app.screen = Screen::Gps(Gps::new(Page::Satellites));
    assert!(t.texts_in(&st).contains("No satellites in view"));
}

/// GPS-15: with a fix but no waypoints, the position page's nearest says how to mark one.
#[test]
fn no_waypoint_near() {
    let mut t = T::new();
    let st = OnlyFix(Sample::sample_fix());
    t.app.screen = Screen::Gps(Gps::new(Page::Position));
    let s = t.texts_in(&st);
    assert!(s.contains("None: # marks one here"), "{s}");
}

/// GPS-19: going to a waypoint while moving: course up with the speed, and the time to get there; \u{25c0}\u{25b6} step
/// the waypoints; without a fix, no distance.
#[test]
fn go_while_moving() {
    let mut t = T::new();
    moving(&mut t);
    t.open(Dest::Waypoints);
    t.tap(Key::Right);
    t.tap(Key::Left);
    t.tap(Key::Menu);
    assert_eq!(page(&t), Page::Go);
    t.reads("title=\"Go to\"");
    t.shows("Course up \u{b7} 48 km/h");
    t.shows("ETA");
    t.tap(Key::Right); // the next waypoint
    t.s.fix.set(None);
    t.draw();
    t.shows("No GPS fix: no distance");
    t.shows("North up");
}
