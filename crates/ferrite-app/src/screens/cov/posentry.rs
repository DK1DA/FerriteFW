//! Position entry (A8, GPS-3): typing each row in each coordinate format, the checks, the signs, the grid and
//! From GPS rows.

use super::*;
use crate::screens::posentry::PosEntry;

fn open(fmt: u8) -> T {
    let mut t = T::new();
    t.app.set.coord_fmt = fmt;
    t.open(Dest::PositionEntry);
    assert_eq!(t.name(), "Position entry");
    t.reads("title=\"Position\"");
    t
}

fn at(row: u8) -> Screen {
    let mut p = PosEntry::new();
    p.row = row;
    Screen::PosEntry(p)
}

fn digits(t: &mut T, ds: &[u8]) {
    for &d in ds {
        t.tap(Key::Digit(d));
    }
}

fn row(t: &T) -> u8 {
    match t.screen() {
        Screen::PosEntry(p) => p.row,
        s => panic!("not position entry: {}", s.name()),
    }
}

/// GPS-3: decimal degrees: the digits fill the template as typed; \u{25bc} keeps the row; * makes it south.
#[test]
fn decimal_latitude() {
    let mut t = open(0);
    t.shows("Used when there is no GPS fix");
    t.reads("hints \"0-9\" \"Type\" \"*\" \"N/S\" \"OK\" \"Save\"");
    digits(&mut t, &[4, 5, 4]);
    t.shows("45.4____\u{b0}");
    digits(&mut t, &[2, 1, 0, 0]);
    t.tap(Key::Digit(9)); // full: no more
    t.tap(Key::Down);
    assert!(t.app.set.fixed_on);
    assert_eq!(t.app.set.fixed.lat, 454_210_000);
    assert_eq!(row(&t), 1);
    t.reads("hints \"0-9\" \"Type\" \"*\" \"E/W\" \"OK\" \"Save\"");
    t.tap(Key::Up);
    t.tap(Key::Star);
    assert_eq!(t.app.set.fixed.lat, -454_210_000);
    t.shows("45.42100\u{b0} S");
}

/// GPS-3: degrees, minutes and seconds: minutes or seconds past 59 are out of range; a row not typed in full
/// says so; BACK takes a digit back.
#[test]
fn dms_checks() {
    let mut t = open(2);
    digits(&mut t, &[0, 7, 5]);
    t.tap(Key::Down);
    assert_eq!(t.toast(), "Latitude / type every digit");
    assert_eq!(row(&t), 0);
    for _ in 0..3 {
        t.tap(Key::Exit);
    }
    digits(&mut t, &[4, 5, 7, 5, 0, 0, 0]);
    t.tap(Key::Down);
    assert_eq!(t.toast(), "Latitude / out of range", "75 minutes");
    for _ in 0..7 {
        t.tap(Key::Exit);
    }
    digits(&mut t, &[4, 5, 2, 5, 3, 0, 1]);
    t.tap(Key::Menu);
    assert!(t.app.set.fixed_on);
    assert!(t.toast().starts_with("Fixed position / "), "{}", t.toast());
}

/// GPS-3: longitude in degrees and minutes, west with *; past 180 is out of range.
#[test]
fn ddm_longitude() {
    let mut t = open(1);
    t.tap(Key::Down);
    digits(&mut t, &[1, 9, 0, 0, 0, 0, 0, 0]);
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Longitude / out of range");
    for _ in 0..8 {
        t.tap(Key::Exit);
    }
    digits(&mut t, &[0, 7, 5, 4, 2, 0, 0, 0]);
    t.tap(Key::Star);
    t.tap(Key::Down);
    assert_eq!(t.app.set.fixed.lon, -757_000_000);
    t.shows("075\u{b0}42.000' W");
}

/// GPS-3: the altitude: metres as typed, its sign with *; # clears the fixed position.
#[test]
fn altitude_and_clear() {
    let mut t = open(1);
    t.taps(&[Key::Down, Key::Down]);
    t.reads("\"+/-\"");
    digits(&mut t, &[3, 5, 0]);
    t.shows("350 m");
    t.tap(Key::Star);
    t.tap(Key::Up);
    assert_eq!(t.app.set.fixed_alt, -350);
    t.tap(Key::Down);
    t.shows("-350 m");
    t.tap(Key::Slot); // nothing
    t.tap(Key::Hash);
    assert!(!t.app.set.fixed_on);
    assert_eq!(t.toast(), "Fixed position / none");
    t.shows("-");
    t.tap(Key::Exit);
    assert_eq!(t.screen(), Screen::Home, "BACK with nothing typed leaves");
}

/// GPS-21, GPS-35: the grid row types a square, # clears; From GPS keeps the fix, refused without one.
#[test]
fn grid_and_from_gps_rows() {
    let mut t = open(1);
    t.taps(&[Key::Up, Key::Up]); // grid square
    assert_eq!(row(&t), 3);
    t.shows("OK types a grid square");
    t.reads("hints \"^v\" \"Row\" \"OK\" \"Type it\" \"#\" \"Clear\"");
    t.app.set.fixed_on = true;
    t.tap(Key::Hash);
    assert!(!t.app.set.fixed_on);
    t.tap(Key::Slot); // nothing
    t.tap(Key::Down); // From GPS
    assert_eq!(row(&t), 4);
    t.shows("3D fix");
    t.shows("OK keeps the current GPS fix");
    t.reads("hints \"^v\" \"Row\" \"OK\" \"Use fix\"");
    t.tap(Key::Menu);
    assert!(t.app.set.fixed_on);
    assert!(t.toast().starts_with("Fixed position / "));
    t.s.fix.set(None);
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "No GPS fix / wait for a lock");
    t.shows("No fix");
    t.tap(Key::Slot); // nothing
    t.tap(Key::Up);
    assert_eq!(row(&t), 3);
    t.tap(Key::Down);
    t.tap(Key::Down);
    assert_eq!(row(&t), 0, "wraps to the latitude");
    t.taps(&[Key::Up, Key::Up]);
    t.tap(Key::Up); // grid's up: altitude
    assert_eq!(row(&t), 2);
    t.tap(Key::Down);
    t.tap(Key::Menu); // the grid square in text entry
    assert!(matches!(t.screen(), Screen::Text(_)), "{}", t.name());
    for (row, name) in [(3, "row 3"), (4, "row 4")] {
        let mut t = T::new();
        t.open(Dest::PositionEntry);
        t.app.screen = at(row);
        t.tap(Key::Exit);
        assert_eq!(t.screen(), Screen::Home, "{name}: BACK leaves");
    }
}

/// GPS-3: \* while typing gives the typed value the other sign, so a first position can be south or west (and a
/// first altitude below sea level) before anything is saved; a stored one keeps its sign until \* again.
#[test]
fn sign_while_typing_a_first_position() {
    let mut t = open(1);
    assert_eq!(t.app.set.fixed.lat, 0);
    digits(&mut t, &[3, 3, 5, 1, 0, 0, 0]);
    t.tap(Key::Star);
    t.shows("33\u{b0}51.000' S");
    t.tap(Key::Star);
    t.shows("33\u{b0}51.000' N");
    t.tap(Key::Star);
    t.tap(Key::Down);
    assert_eq!(t.app.set.fixed.lat, -338_500_000, "south");
    t.tap(Key::Down);
    digits(&mut t, &[1, 2]);
    t.tap(Key::Star);
    t.shows("-12 m");
    t.tap(Key::Menu);
    assert_eq!(t.app.set.fixed_alt, -12);
}
