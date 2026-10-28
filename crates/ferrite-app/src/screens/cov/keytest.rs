//! The key test (KEY-2, D9): every key's name and press type, the knob, raw codes, and the map filling up.

use super::*;

fn open() -> T {
    let mut t = T::new();
    t.open(Dest::KeyTest);
    assert_eq!(t.name(), "Key test");
    t.shows("Press a key");
    t.shows("Hold BACK to leave");
    t
}

/// KEY-2: each key shows its name and "Short press" with its code; every key and the knob light their cell
/// until all 24 are seen.
#[test]
fn every_key_named() {
    let mut t = open();
    for (k, name) in [
        (Key::Menu, "MENU/OK"),
        (Key::Exit, "BACK"),
        (Key::Up, "Up"),
        (Key::Down, "Down"),
        (Key::Left, "Left"),
        (Key::Right, "Right"),
        (Key::Slot, "SLOT"),
        (Key::VfoMem, "V/M"),
        (Key::Star, "*"),
        (Key::Hash, "#"),
        (Key::Side(1), "SIDE1"),
        (Key::Side(2), "SIDE2"),
    ] {
        t.tap(k);
        assert_eq!(t.name(), "Key test", "{name}: still the key test");
        t.shows(name);
        t.shows("Short press \u{b7} code ");
    }
    for d in 0..10 {
        t.tap(Key::Digit(d));
    }
    // PTT A: its code first (the firmware's raw_key), then the PTT event
    t.app.raw_key(ferrite_core::keys::CODE_PTT_A, true);
    t.ptt(true);
    t.shows("PTT");
    t.shows("Pressed");
    t.app.raw_key(ferrite_core::keys::CODE_PTT_A, false);
    t.ptt(false);
    t.shows("PTT");
    t.knob(1);
    t.shows("Knob");
    t.shows("Clockwise");
    t.knob(-1);
    t.shows("Anticlockwise");
    t.shows("24 of 24 keys seen");
}

/// KEY-2: a long press and a repeat say so; a code with no key shows as Unmapped.
#[test]
fn long_repeat_and_unmapped() {
    let mut t = open();
    t.down(Key::Digit(5), true);
    t.wait(800);
    t.shows("Long press");
    t.down(Key::Digit(5), false);
    t.shows("Released");
    t.down(Key::Up, true);
    t.wait(2000);
    t.shows("Repeat");
    t.down(Key::Up, false);
    t.app.raw_key(14, true); // an unmapped matrix code
    t.draw();
    t.shows("Unmapped");
    t.shows("Pressed \u{b7} code 14");
}
