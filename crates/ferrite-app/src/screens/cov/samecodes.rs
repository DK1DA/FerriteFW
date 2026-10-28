//! SAME location codes (A13, chapter 23): typing, keeping, clearing, and the regions without SAME.

use super::*;

fn open(region: ferrite_core::txplan::Region) -> T {
    let mut t = T::new();
    t.app.set_region(Some(region), false);
    t.open(Dest::LocationCodes);
    assert_eq!(t.name(), "Location codes");
    t
}

fn digits(t: &mut T, ds: &[u8]) {
    for &d in ds {
        t.tap(Key::Digit(d));
    }
}

/// WX-10: OK edits a place, six digits kept with OK (a toast says the code), fewer or all zeros refused; BACK
/// deletes a digit, then cancels; * clears a code; up to eight places, wrapping.
#[test]
fn type_keep_and_clear() {
    let mut t = open(ferrite_core::txplan::Region::Us);
    t.reads("title=\"Location\"");
    t.shows("FIPS codes");
    t.shows("0 of 8");
    t.shows("None set: every warning shows. Six digits each.");
    t.reads("hints \"OK\" \"Edit\" \"*\" \"Clear\" \"BACK\" \"Back\"");
    assert_eq!(t.screen().timeout(), Some(crate::MENU_TIMEOUT_MS));
    t.tap(Key::Star); // nothing to clear
    assert_eq!(t.toast(), "");
    t.tap(Key::Menu);
    t.reads("hints \"0-9\" \"Type\" \"OK\" \"Keep\" \"BACK\" \"Delete\"");
    digits(&mut t, &[0, 5, 3]);
    t.shows("053___");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Bad code / six digits, not all 0");
    digits(&mut t, &[0, 3, 3, 9]); // the seventh is ignored
    t.shows("053033");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Location code / 053033");
    assert_eq!(t.app.set.same_codes[0], 53_033);
    t.shows("1 of 8");
    t.shows("A warning shows when one of its areas is here.");
    t.tap(Key::Up); // wraps to the eighth
    t.tap(Key::Menu);
    digits(&mut t, &[0, 0, 0, 0, 0, 0]);
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Bad code / six digits, not all 0");
    t.tap(Key::Exit);
    t.shows("00000_");
    for _ in 0..5 {
        t.tap(Key::Exit);
    }
    t.tap(Key::Exit); // nothing typed: back to the list
    t.tap(Key::Slot); // nothing
    t.tap(Key::Down);
    t.tap(Key::Star);
    assert_eq!(t.toast(), "Location code / cleared");
    assert_eq!(t.app.set.same_codes[0], 0);
    t.tap(Key::Exit);
    assert_ne!(t.name(), "Location codes");
}

/// WX-10: Canada's are CLC codes.
#[test]
fn canada_clc() {
    let mut t = open(ferrite_core::txplan::Region::Canada);
    t.shows("CLC codes");
}

/// WX-3: outside the US and Canada the page says so; keys do nothing but BACK.
#[test]
fn elsewhere_none() {
    let mut t = open(ferrite_core::txplan::Region::Europe);
    t.shows("US and Canada only");
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Location codes");
    t.tap(Key::Exit);
    assert_ne!(t.name(), "Location codes");
}
