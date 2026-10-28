//! A slot turned off (HOME-15.1 – HOME-15.4): \* cycles the active slot Normal, Muted, Off; the off card; muted while
//! receiving.

use super::*;
use crate::persist::{Persist, Sink};
use crate::screens::home::card;
use ferrite_core::keys::PTT_B;
use ferrite_ui::gfx::{AMBER, DIM, SUB, SURFACE};
use std::string::String;

/// The readback's line starting with `head`.
fn line(t: &T, head: &str) -> String {
    let (s, dev) = (store(), Device::default());
    let mut l = crate::term::cmd::Line::new();
    (0..)
        .map_while(|i| crate::uistate::line(&t.app, &s, &dev, &t.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
        .find(|l| l.starts_with(head))
        .unwrap_or_else(|| panic!("no {head} line"))
}

/// Pixels of `c` in the box.
fn px(fb: &FrameBuffer, x: i32, y: i32, w: i32, h: i32, c: u16) -> usize {
    (x..x + w).flat_map(|px| (y..y + h).map(move |py| (px, py))).filter(|&(px, py)| fb.at(px, py) == c).count()
}

/// The toast's two lines.
fn toast(t: &T) -> (String, String) {
    let x = t.app.toast.as_ref().expect("a toast");
    (String::from_utf8_lossy(x.head.as_bytes()).into_owned(), String::from_utf8_lossy(x.body.as_bytes()).into_owned())
}

/// HOME-15.1, WATCH-2.1: \* on a muted slot turns it off: the off mask goes to the Radio task, the watch set loses it
/// (its turns go to the others), a toast says so and how to undo it; \* again turns it on, unmuted, and it is
/// watched again. The readback's card state is "Off". A latched monitor goes off with it.
#[test]
fn star_turns_a_slot_off_and_on() {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.tap(Key::Slot); // B active
    t.tap(Key::Side(1)); // monitor latched on B
    t.tap(Key::Star);
    assert_eq!(toast(&t), ("Slot B muted".into(), "* again turns it off".into()));
    t.cmds();
    t.tap(Key::Star);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::SlotOff(0b010)) && c.contains(&RadioCmd::Mute(0)), "{c:?}");
    assert!(c.contains(&RadioCmd::Watch(0b001)), "B out of the watch: {c:?}");
    assert!(c.contains(&RadioCmd::Monitor { slot: 1, on: false }), "{c:?}");
    assert_eq!(toast(&t), ("Slot B off".into(), "* again to turn on".into()));
    assert_eq!(t.app.state(1, &t.live), ("Off", SUB));
    t.live.slots[1].squelch_open = true;
    t.wait(100);
    assert_eq!(t.app.state(1, &t.live).0, "Off", "whatever it would hear");
    assert!(line(&t, "card B").contains("state=\"Off\""), "{}", line(&t, "card B"));
    t.tap(Key::Star);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::SlotOff(0)) && c.contains(&RadioCmd::Mute(0)) && c.contains(&RadioCmd::Watch(0b011)), "{c:?}");
    assert_eq!(toast(&t).0, "Slot B on");
    assert_eq!(t.app.state(1, &t.live).0, "RX");
}

/// HOME-15.1, APRS-2: the APRS slot keeps Normal and Muted: \* on C in APRS mode never turns it off.
#[test]
fn aprs_slot_is_never_off() {
    let mut t = T::new();
    t.app.set_aprs_mode(true);
    while t.app.active() != crate::aprs::APRS_SLOT {
        t.tap(Key::Slot);
    }
    let muted = t.app.mute & 0b100 != 0;
    for _ in 0..3 {
        t.tap(Key::Star);
        assert_eq!(t.app.off, 0);
    }
    assert_ne!(t.app.mute & 0b100 != 0, muted, "three presses: flipped three times");
}

/// HOME-15.1: C turned off, then APRS mode takes it: it is on again (the APRS slot is never off).
#[test]
fn aprs_mode_turns_c_back_on() {
    let mut t = T::new();
    t.tap(Key::Slot);
    t.tap(Key::Slot);
    t.tap(Key::Star);
    t.tap(Key::Star);
    assert_eq!(t.app.off, 0b100);
    t.cmds();
    t.app.set_aprs_mode(true);
    assert_eq!(t.app.off, 0);
    assert!(t.cmds().contains(&RadioCmd::SlotOff(0)));
}

/// HOME-15.4: an off slot doesn't transmit, scan or latch monitor: PTT, SIDE2 and SIDE1 are refused with a toast
/// saying how to turn it on, and nothing goes to the Radio task; PTT B still keys slot B.
#[test]
fn off_slot_refuses_ptt_scan_and_monitor() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    t.tap(Key::Star);
    t.tap(Key::Star);
    t.wait(3500);
    t.cmds();
    for k in [Key::PTT, Key::Side(2), Key::Side(1)] {
        t.tap(k);
        let c = t.cmds();
        assert!(!c.iter().any(|c| matches!(c, RadioCmd::Transmit { .. } | RadioCmd::Scan { .. } | RadioCmd::ScanChans { .. } | RadioCmd::Monitor { .. })), "{k:?}: {c:?}");
        assert_eq!(toast(&t), ("Slot A is off".into(), "* turns it back on".into()), "{k:?}");
        assert_eq!(t.app.screen(), Screen::Home);
    }
    t.tap(Key::Ptt(PTT_B));
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 1, on: true }), "PTT B keys B");
}

/// HOME-15.1: off is RAM only, as the mute: nothing saved changes, and a power cycle starts with every slot on.
#[test]
fn off_is_never_saved() {
    struct V(Vec<u8>);
    impl Sink for V {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    let mut t = T::new();
    let save = |t: &T| {
        let mut v = V(Vec::new());
        t.app.save_state(&mut v);
        t.app.settings().save(&mut v);
        v.0
    };
    let before = save(&t);
    t.tap(Key::Star);
    t.tap(Key::Star);
    assert_eq!(t.app.off, 1);
    assert_eq!(save(&t), before);
    assert_eq!(T::new().app.off, 0);
}

/// HOME-15.2: an off slot's card is powered down: no surface (the page shows through), a dashed outline (dim on a
/// small card, blue on the active one, which keeps its size), a dim "OFF" in the middle with "\* turns it back on"
/// under it, and "Off" at the header's right.
#[test]
fn off_card_is_powered_down() {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.tap(Key::Star);
    t.tap(Key::Star);
    t.tap(Key::Slot); // B active: A's small card is off
    t.wait(3500);
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    let (y, h) = card(0, 1);
    // (the text's blended edges can land on the surface's colour: a few dozen pixels, of some 14,000)
    assert!(px(&fb, 8, y + 2, 224, h - 4, SURFACE) < 200, "no surface");
    let top = px(&fb, 6, y, 228, 1, DIM);
    assert!(top > 80 && top < 180, "a dashed top edge: {top}");
    let side = px(&fb, 6, y + 3, 1, h - 6, DIM) as i32;
    assert!(side > 20 && side < h - 6, "a dashed side: {side}");
    assert!(px(&fb, 90, y + 22, 60, 30, DIM) > 60, "OFF");
    let hint = (40..200).flat_map(|x| (y + 47..y + 64).map(move |yy| (x, yy))).filter(|&(x, yy)| fb.at(x, yy) != BG).count();
    assert!(hint > 150, "the hint under it: {hint}");
    assert!(px(&fb, 190, y + 5, 36, 17, SUB) > 10, "Off at the header's right");
    assert_eq!(fb.at(6, card(1, 1).0 + 40), BLUE, "B's card as before");
    // A active: the large card, the outline blue
    t.tap(Key::Slot);
    t.tap(Key::Slot);
    t.wait(3500);
    t.render(&mut fb);
    let (y, h) = card(0, 0);
    assert_eq!(h, card(1, 1).1, "the large card's size");
    assert!(px(&fb, 8, y + 2, 224, h - 4, SURFACE) < 200);
    let top = px(&fb, 6, y, 228, 1, BLUE);
    assert!(top > 80 && top < 180, "dashed blue: {top}");
    assert!(px(&fb, 70, y + 40, 100, 40, DIM) > 80, "OFF, larger");
    // on again: a normal card
    t.tap(Key::Star);
    t.wait(3500);
    t.render(&mut fb);
    assert!(px(&fb, 8, y + 30, 224, 20, SURFACE) > 1000, "the surface back");
    assert_eq!(fb.at(6, y + 40), BLUE);
}

/// HOME-15.3: muted while receiving the card keeps "Muted" in amber and shows the green RX tag (the small card's
/// bold RX) right of it, with the green border of any receive; the readback says both ("RX muted").
#[test]
fn muted_receiving_shows_muted_and_rx() {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.tap(Key::Star);
    t.live.slots[0].squelch_open = true;
    t.live.slots[0].rssi = -80;
    t.wait(3500);
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    assert!(line(&t, "card A").contains("state=\"RX muted\""));
    // the large card: the tag at the right, Muted left of it, the active card's blue border
    let (y, _) = card(0, 0);
    assert!(px(&fb, 196, y + 7, 30, 16, GREEN) > 20, "the RX tag");
    assert_eq!(px(&fb, 196, y + 7, 30, 16, AMBER), 0);
    assert!(px(&fb, 140, y + 7, 54, 17, AMBER) > 20, "Muted left of it");
    assert_eq!(fb.at(6, y + 40), BLUE);
    // the small card: bold RX, Muted left of it, a green border
    t.tap(Key::Slot);
    t.wait(3500);
    t.render(&mut fb);
    let (y, _) = card(0, 1);
    assert!(px(&fb, 206, y + 5, 20, 17, GREEN) > 10, "RX");
    assert_eq!(px(&fb, 206, y + 5, 20, 17, AMBER), 0);
    assert!(px(&fb, 150, y + 5, 54, 17, AMBER) > 15, "Muted left of it");
    assert_eq!(fb.at(6, y + 30), GREEN, "the receive border");
}

/// HOME-15.1, WATCH-2.1, APRS-21: an off slot's chip time goes to the others: A on 2 m beside APRS mode's C shares
/// the VHF chip ("APRS first" on C's card); A off, C has it alone and the note goes.
#[test]
fn off_slot_gives_c_its_chip_back() {
    let mut t = T::new();
    t.app.set_aprs_mode(true);
    t.wait(100);
    assert_eq!(t.app.aprs_shared(), Some(0), "A on 146.520 shares C's chip");
    t.tap(Key::Star);
    assert!(t.app.aprs_shared().is_some(), "muted, it still listens");
    t.tap(Key::Star);
    assert_eq!(t.app.aprs_shared(), None);
    assert!(t.app.aprs_priority().is_none());
}
