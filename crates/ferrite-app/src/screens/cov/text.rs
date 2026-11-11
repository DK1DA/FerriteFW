//! Text entry (A7, KEY-12): each target's label, starting text and what OK does with it; the digits-only and
//! path kinds; the cursor; a full text.

use super::*;
use crate::screens::text::{self, TextFor};

fn open(t: &mut T, to: TextFor) {
    t.open(Dest::Status); // something to come back to
    text::open(&mut t.app, to, &t.s);
    t.draw();
    assert!(matches!(t.screen(), Screen::Text(_)), "{}", t.name());
}

fn typed(t: &T) -> String {
    String::from_utf8_lossy(t.app.text.as_bytes()).into_owned()
}

/// BOOT-9: the power-on message: its label, typed and kept.
#[test]
fn power_on_message() {
    let mut t = T::new();
    open(&mut t, TextFor::PowerOn);
    t.shows("Power-on text");
    t.tap(Key::Digit(4));
    t.tap(Key::Digit(4));
    t.wait(1100);
    t.tap(Key::Digit(4));
    t.tap(Key::Digit(4));
    t.tap(Key::Digit(4));
    t.tap(Key::Menu);
    assert_eq!(t.app.set.pon_text.as_bytes(), b"HI");
}

/// DEC-4: the pager capcode is digits only: * changes nothing, the hints have no mode; it starts as the
/// capcode kept, and a capcode over 21 bits is refused.
#[test]
fn capcode_digits() {
    let mut t = T::new();
    t.app.set.pocsag_cap = 1234;
    open(&mut t, TextFor::Capcode);
    t.shows("Pager capcode");
    t.shows("The capcode's digits. Empty keeps every capcode.");
    assert_eq!(typed(&t), "1234");
    t.reads("hints \"OK\" \"Keep\" \"BACK\" \"Delete\"");
    t.tap(Key::Star);
    t.tap(Key::Digit(5));
    assert_eq!(typed(&t), "12345", "still digits");
    t.tap(Key::Digit(6));
    t.tap(Key::Digit(7));
    t.tap(Key::Digit(8)); // the eighth: no room
    assert_eq!(typed(&t), "1234567");
    t.app.text = ferrite_ui::text::Buf::from(b"9999999");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Capcode / up to 2097151");
    assert_eq!(t.app.set.pocsag_cap, 1234);
}

/// GPS-23: the clock set by hand: its label and tip; a date that isn't one is refused.
#[test]
fn clock_by_hand() {
    let mut t = T::new();
    open(&mut t, TextFor::Clock);
    t.shows("Date and time");
    t.shows("Year, month, day, hour, minute: 202610081430. Local time.");
    for d in [2, 0, 2, 6, 1, 3] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Date and time / like 202610081430");
}

/// APRS-26: the custom path starts as the one kept; # types the comma; a path that isn't one is refused.
#[test]
fn aprs_path() {
    let mut t = T::new();
    t.wait(100);
    t.app.set_aprs_custom_path(b"WIDE1-1");
    open(&mut t, TextFor::AprsPath);
    assert_eq!(typed(&t), "WIDE1-1");
    t.tap(Key::Star);
    assert_eq!(typed(&t), "WIDE1-1,");
    t.app.text = ferrite_ui::text::Buf::from(b"A,B,C"); // three entries
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "APRS path / like WIDE1-1,WIDE2-1");
}

/// SETUP-12, GPS-21: the callsign starts as the one set; a grid square for the fixed position: a wrong one
/// refused, a right one kept.
#[test]
fn callsign_and_grid() {
    let mut t = T::new();
    t.app.set.set_callsign(b"VA7FER");
    open(&mut t, TextFor::Callsign);
    assert_eq!(typed(&t), "VA7FER");
    open(&mut t, TextFor::GridFixed);
    t.app.text = ferrite_ui::text::Buf::from(b"FN3");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Grid square / like FN31 or FN31PR");
    t.app.text = ferrite_ui::text::Buf::from(b"FN31");
    t.tap(Key::Menu);
    assert!(t.app.set.fixed_on);
    assert!(t.toast().starts_with("Fixed position / FN31"), "{}", t.toast());
}

/// TEXT-12: \u{25c0} puts the cursor inside the text, drawn there; a full text takes no more (# nor a letter).
#[test]
fn cursor_and_full() {
    let mut t = T::new();
    open(&mut t, TextFor::Zone(0));
    t.app.text = ferrite_ui::text::Buf::from(b"ABCDEFGHIJKLMNOP"); // 16, the most
    t.tap(Key::Hash);
    t.tap(Key::Digit(2));
    assert_eq!(typed(&t), "ABCDEFGHIJKLMNOP");
    t.tap(Key::Left);
    t.tap(Key::Left);
    assert!(matches!(t.screen(), Screen::Text(x) if x.tail == 2));
    let before = t.px(44, 75, ferrite_ui::gfx::TEXT);
    assert!(before > 0, "the cursor drawn inside");
    t.tap(Key::Right);
    assert!(matches!(t.screen(), Screen::Text(x) if x.tail == 1));
}

/// C8: renaming a channel a slot is on shows the new name on the slot too.
#[test]
fn rename_a_channel_on_a_slot() {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    let n = t.app.slot(a).ch.unwrap().num;
    open(&mut t, TextFor::Channel(n));
    t.app.text = ferrite_ui::text::Buf::from(b"Renamed");
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(a).ch.unwrap().name(), b"Renamed");
}
