//! Zones (chapter 10): the zone editor's rows changed and reset, the resume names, new zones until there is no
//! room, and an empty list.

use super::*;
use crate::data::Data;
use crate::screens::zones::Zones;

fn row(t: &T) -> u8 {
    match t.screen() {
        Screen::Zones(z) => z.mode,
        s => panic!("not zones: {}", s.name()),
    }
}

/// CHAN-11, SCAN-5, SCAN-12, MENU-18: the zone editor: each row turns with the knob (shown, by distance, resume
/// with its names, dwell, priorities) and # puts it back.
#[test]
fn editor_rows() {
    let mut t = T::new();
    t.open(Dest::ZoneEdit);
    assert_eq!(t.name(), "Zone editor");
    t.reads("title=\"Zone\"");
    t.tap(Key::Down);
    t.tap(Key::Down); // shown on home
    assert_eq!(row(&t), 4);
    let c0 = t.s.zone_cfg(0);
    t.knob(1);
    assert_ne!(t.s.zone_cfg(0).shown, c0.shown);
    t.tap(Key::Hash);
    assert!(t.s.zone_cfg(0).shown);
    t.tap(Key::Down); // by distance
    t.knob(1);
    assert!(t.s.zone_cfg(0).by_distance);
    t.tap(Key::Hash);
    assert!(!t.s.zone_cfg(0).by_distance);
    t.tap(Key::Down); // resume
    for (n, name) in [(11, "Carrier drop"), (1, "Stop on hit"), (4, "After 3 s")] {
        t.knob(n);
        t.shows(name);
    }
    t.tap(Key::Hash);
    t.shows("Radio's");
    t.tap(Key::Down); // dwell
    t.knob(2);
    assert_eq!(t.s.zone_cfg(0).dwell, 1);
    t.tap(Key::Hash);
    assert_eq!(t.s.zone_cfg(0).dwell, 255);
    t.tap(Key::Down); // priority 1
    t.knob(1);
    assert!(t.s.zone_cfg(0).pri[0].is_some());
    t.tap(Key::Hash);
    assert!(t.s.zone_cfg(0).pri[0].is_none());
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.tap(Key::Hash); // the name's row: nothing to reset
    t.knob(1); // nor to turn
    t.tap(Key::Digit(3)); // nothing
    t.tap(Key::Exit);
    assert_eq!(t.name(), "Zones");
}

/// CHAN-12: # makes new zones until there is no room, then says so.
#[test]
fn new_zones_until_full() {
    let mut t = T::new();
    t.open(Dest::Zones);
    for _ in 0..16 {
        t.tap(Key::Hash);
        if t.toast().starts_with("No room") {
            break;
        }
    }
    assert_eq!(t.toast(), "No room / 512 zones at most");
}

/// CHAN-10: with no zones (and Local off, CHAN-34) the list says how to make one.
#[test]
fn no_zones() {
    let mut t = T::new();
    t.app.settings_mut().local = 0;
    t.app.screen = Screen::Zones(Zones::new());
    let empty = crate::chan::Table { zones: &[], channels: &[] };
    let s = t.texts_in(&empty);
    assert!(s.contains("No zones"), "{s}");
}
