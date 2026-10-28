//! Save as channel and Add repeater (A9, A10): every row turned and reset, the refusals, and the HF save.

use super::*;
use crate::chan::{MAX_CHANNELS, NO_ZONE};
use crate::screens::save::Save;
use ferrite_core::freq::mhz;

fn form(t: &T) -> Save {
    match t.screen() {
        Screen::Save(s) => s,
        s => panic!("not the save form: {}", s.name()),
    }
}

/// A10, MENU-18: Add repeater: the frequency steps, the shift goes simplex/+/- with the knob and # back to the
/// usual one, the tone and # off, the number to the next free one, the zone to none with #; each row's tip;
/// the Save row's hints.
#[test]
fn repeater_rows() {
    let mut t = T::ready();
    t.open(Dest::AddRepeater);
    t.knob(0); // the form gets its number and zone from the store
    let f0 = form(&t).freq;
    t.knob(1); // the frequency
    assert!(form(&t).freq > f0);
    t.tap(Key::Down); // shift
    t.shows("Knob: change it. #: back to the usual one.");
    let auto = form(&t).offset;
    t.knob(1);
    assert_ne!(form(&t).offset, auto);
    t.tap(Key::Hash);
    assert_eq!(form(&t).offset, auto, "# puts the usual shift back");
    t.tap(Key::Down); // tone
    t.knob(3);
    assert_ne!(form(&t).tone, 0);
    t.tap(Key::Hash);
    assert_eq!(form(&t).tone, 0);
    t.tap(Key::Down); // name
    t.shows("OK: type the name. It can stay empty.");
    t.tap(Key::Hash); // nothing to reset
    t.tap(Key::Down); // number
    let n = form(&t).num;
    t.knob(1);
    assert_ne!(form(&t).num, n);
    t.tap(Key::Down); // zone
    t.knob(1);
    t.tap(Key::Hash);
    assert_eq!(form(&t).zone, NO_ZONE);
    t.knob(-1);
    assert_ne!(form(&t).zone, NO_ZONE, "back from none: the last zone");
    t.tap(Key::Down); // save
    t.shows("OK saves it.");
    t.reads("hints \"^v\" \"Move\" \"OK\" \"Save\" \"BACK\" \"Cancel\"");
    t.tap(Key::Slot); // nothing
    assert_eq!(t.name(), form(&t).kind_name());
}

/// CHAN-26: a repeater below 64 MHz can't be saved here: the note says why, in amber, and OK refuses.
#[test]
fn repeater_below_64() {
    let mut t = T::ready();
    t.open(Dest::AddRepeater);
    t.knob(0);
    let mut s = form(&t);
    s.set_freq(mhz(29) + 60_000);
    s.row = 6;
    t.app.screen = Screen::Save(s);
    t.draw();
    t.shows("Below 64 MHz: save it from HF mode");
    assert!(t.px(100, 296, ferrite_ui::gfx::AMBER) > 0);
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Not saved: / Below 64 MHz");
}

/// A9: with every number used there is none free: the row says so and OK refuses.
#[test]
fn no_free_number() {
    let mut t = T::ready();
    t.open(Dest::SaveChannel);
    t.knob(0);
    let mut s = form(&t);
    s.num = MAX_CHANNELS;
    s.row = 3;
    t.app.screen = Screen::Save(s);
    t.draw();
    t.shows("none free");
    t.shows("No free channel number: all 1000 are used.");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Not saved: / No free channel number");
}

/// CHAN-26: hold 9 in HF mode saves its frequency and mode.
#[test]
fn hf_save_heading() {
    let mut t = T::ready();
    t.app.set.hf.freq = 707_400;
    t.open(Dest::HfMode);
    t.wait(100);
    t.hold(Key::Digit(9));
    assert!(matches!(t.screen(), Screen::Save(_)), "{}", t.name());
    t.shows("Save this HF frequency");
}

trait KindName {
    fn kind_name(self) -> &'static str;
}

impl KindName for Save {
    fn kind_name(self) -> &'static str {
        Screen::Save(self).name()
    }
}
