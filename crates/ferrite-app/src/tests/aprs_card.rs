//! Home's APRS card for each holder of slot C (APRS-17, decision 181): the header word, the fact right of the
//! frequency, the bottom line, the hint bar's OK and the readback.

use super::*;
use crate::aprs::{AprsCfg, Holder, APRS_SLOT};
use crate::data::Data;
use crate::sample::Sample;
use ferrite_core::msg::Device;
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::gfx::{AMBER, VIOLET};
use std::format;
use std::string::String;
use std::vec::Vec;

struct A {
    app: App,
    s: Sample,
    t: u32,
    live: LiveAll,
}

impl A {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        app.set.backlight = 0;
        while app.pop_cmd().is_some() {}
        let mut a = A { app, s: Sample::new(), t: 1000, live: LiveAll::default() };
        a.wait(100);
        a
    }

    fn tick(&mut self) {
        self.app.tick(self.t, &self.live, &self.s);
        while let Some(w) = self.app.pop_write() {
            self.s.apply(w);
        }
        while self.app.pop_cmd().is_some() {}
    }

    fn press(&mut self, k: Key, hold: u32) {
        self.app.key_down(k, self.t, &self.s);
        let end = self.t + hold;
        while self.t < end {
            self.t += 10;
            self.tick();
        }
        self.app.key_up(k, self.t, &self.s);
        self.t += 50;
        self.tick();
    }

    fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.tick();
        }
    }

    /// C active (the large card), toasts gone.
    fn c_active(&mut self) {
        while self.app.active() != APRS_SLOT {
            self.press(Key::Slot, 100);
        }
        self.wait(3500);
    }

    fn card_line(&self) -> String {
        let dev = Device::default();
        let mut l = crate::term::cmd::Line::new();
        (0..)
            .map_while(|i| crate::uistate::line(&self.app, &self.s, &dev, &self.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
            .find(|l| l.starts_with("card C"))
            .expect("card C")
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        self.app.screen().hints(&self.app).as_slice().to_vec()
    }

    fn fb(&mut self) -> FrameBuffer {
        let mut fb = FrameBuffer::new();
        self.app.render(&mut fb, &self.live, &Device::default(), &self.s);
        fb
    }
}

/// Pixels of `c` in the box.
fn count(fb: &FrameBuffer, x: i32, y: i32, w: i32, h: i32, c: u16) -> usize {
    (x..x + w).flat_map(|px| (y..y + h).map(move |py| (px, py))).filter(|&(px, py)| fb.at(px, py) == c).count()
}

/// APRS-17.1 – APRS-17.4 (decision 181): APRS mode's card: the header "APRS Mode · VHF" with the holder in C's colour, the
/// stations heard right of the frequency, the last station at the bottom with the beacon power; OK leads to the
/// heard list (the APRS page on its "Heard list" row: one more OK opens it).
#[test]
fn aprs_card_mode() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    let k = a.app.aprs_card(&a.s).expect("C is the APRS slot");
    assert_eq!(k.holder, Holder::Mode);
    assert_eq!(k.right.as_bytes(), format!("{} heard", a.s.stations()).as_bytes());
    let st = a.s.station(0).expect("the sample has stations");
    assert!(core::str::from_utf8(k.line.as_bytes()).unwrap().starts_with(&format!("Last {} \u{b7} ", core::str::from_utf8(st.call.as_bytes()).unwrap())));
    assert_eq!(k.power, "LOW");
    assert!(k.next.is_empty(), "beacons off: no countdown");
    let l = a.card_line();
    assert!(l.contains("head=\"APRS Mode \u{b7} VHF\"") && l.contains("holder=mode") && l.contains("power=\"LOW\"") && l.contains("kind=aprs"), "{l}");
    // The header's holder word is drawn in C's colour (the small card: C is not active).
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let fb = a.fb();
    assert!(count(&fb, 34, y + 5, 80, 17, VIOLET) > 20, "the holder word in C's colour");
    // C active: OK Heard, and OK opens the APRS page on its Heard list row.
    a.c_active();
    assert_eq!(a.hints()[1], ("OK", "Heard"));
    a.press(Key::Menu, 100);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::AprsCard && o.row == crate::screens::opts::APRS_ROW_HEARD));
    a.press(Key::Menu, 100);
    assert!(matches!(a.app.screen(), Screen::Aprs(_)), "the heard list: {:?}", a.app.screen().name());
}

/// APRS-17.1 – APRS-17.4, KISS-9 (decision 181): KISS's card: "APRS KISS", the port, the frames sent and heard.
#[test]
fn aprs_card_kiss() {
    let mut a = A::new();
    a.app.set.bt = true;
    a.app.set.kiss_port = 2; // 0 5 8 on Bluetooth
    a.tick();
    a.s.kiss_frame(true);
    a.s.kiss_frame(false);
    a.s.kiss_frame(false);
    let k = a.app.aprs_card(&a.s).expect("KISS holds C");
    assert_eq!(k.holder, Holder::Kiss);
    assert_eq!(k.right.as_bytes(), b"Bluetooth");
    assert_eq!(k.line.as_bytes(), "Sent 1 \u{b7} Heard 2".as_bytes());
    let l = a.card_line();
    assert!(l.contains("head=\"APRS KISS \u{b7} VHF\"") && l.contains("holder=kiss") && l.contains("port=\"Bluetooth\"") && l.contains("sent=1") && l.contains("frames=2"), "{l}");
    a.app.set.kiss_port = 1; // the cable
    assert_eq!(a.app.aprs_card(&a.s).unwrap().right.as_bytes(), b"Cable");
    a.c_active();
    assert_eq!(a.hints()[1], ("OK", "Heard"));
}

/// APRS-17.1 – APRS-17.4, APRS-29 (decision 181): beacons alone: "APRS Beacon", the countdown to the next beacon (m:ss) or why
/// there is none, the interval and the position's source; OK leads to Beacon now (the APRS page on that row).
/// With APRS mode on too, the header says APRS Mode (the precedence) and the countdown stays in `next`.
#[test]
fn aprs_card_beacon() {
    let mut a = A::new();
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() }); // every 10 min
    a.tick();
    assert_eq!(a.app.aprs_holder(), Some(Holder::Beacon));
    // The simulated APRS task runs the beacon's timer (as the simulators do, once a second).
    a.s.aprs_tick(10, true, true);
    let k = a.app.aprs_card(&a.s).expect("beacons hold C");
    assert!(k.countdown && k.right.as_bytes().contains(&b':'), "{:?}", core::str::from_utf8(k.right.as_bytes()));
    assert_eq!(k.right.as_bytes(), k.next.as_bytes());
    assert_eq!(k.line.as_bytes(), "Every 10 min \u{b7} GPS fix".as_bytes());
    let l = a.card_line();
    assert!(l.contains("head=\"APRS Beacon \u{b7} VHF\"") && l.contains("holder=beacon") && l.contains("next=\"") && l.contains("line=\"Every 10 min"), "{l}");
    // No fix: the reason instead of the countdown.
    a.s.fix.set(None);
    let k = a.app.aprs_card(&a.s).unwrap();
    assert!(!k.countdown && k.right_warn);
    assert_eq!(k.right.as_bytes(), b"no fix");
    assert!(k.line.as_bytes().ends_with(b"no fix"));
    a.s.fix.set(Sample::sample_fix());
    // The countdown drawn: C's colour right of the frequency on the small card.
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let fb = a.fb();
    assert!(count(&fb, 150, y + 34, 76, 16, VIOLET) > 10, "the countdown in C's colour");
    a.c_active();
    assert_eq!(a.hints()[1], ("OK", "Beacon now"));
    a.press(Key::Menu, 100);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.row == crate::screens::opts::APRS_ROW_BEACON_NOW));
    a.press(Key::Exit, 100);
    // APRS mode too: the header's word is APRS Mode; the countdown is still read back.
    a.app.set_aprs_mode(true);
    a.tick();
    let k = a.app.aprs_card(&a.s).unwrap();
    assert_eq!(k.holder, Holder::Mode);
    assert!(!k.next.is_empty(), "beacons on beside APRS mode: next= still");
}

/// APRS-17.5: APRS mode with automatic beacons says both in the header, "APRS + Beacon", beside APRS-21's
/// "VHF shared"; APRS-17.6: \* (quick mute) on the APRS card shows "Muted ·" by the power in amber, and the header
/// keeps its state for the shared note.
#[test]
fn aprs_card_beacon_and_mute() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() }); // every 10 min
    a.tick();
    let l = a.card_line();
    assert!(l.contains("head=\"APRS + Beacon \u{b7} VHF\""), "{l}");
    a.c_active();
    a.press(Key::Star, 100);
    a.wait(3500);
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let fb = a.fb();
    assert!(count(&fb, 120, y + 80, 106, 19, AMBER) > 20, "Muted, by the power");
    assert!(count(&fb, 34, y + 7, 120, 17, VIOLET) > 20, "the holder word still drawn");
}

/// KISS-10: KISS on suspends the radio's own APRS sending: APRS mode and automatic beacons wait (kept as set),
/// KISS holds C, 0 5 3 and 0 5 6 1 are greyed with why, and Beacon now and sending a message are refused.
#[test]
fn kiss_suspends_the_radios_own_aprs() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() });
    a.tick();
    assert!(a.app.aprs_on() && a.app.beacons_on());
    a.app.set.kiss_port = 1; // the cable
    a.tick();
    assert!(!a.app.aprs_on() && !a.app.beacons_on(), "suspended");
    assert!(a.app.set.aprs_mode && a.app.aprs_cfg().beacons_on(), "the settings kept");
    assert_eq!(a.app.aprs_holder(), Some(Holder::Kiss));
    assert_eq!(crate::screens::groups::greyed(&a.app, &[5, 3]), Some("KISS on: the app sends"));
    assert_eq!(crate::screens::groups::greyed(&a.app, &[5, 6, 1]), Some("KISS on: the app sends"));
    a.app.set.kiss_port = 0;
    a.tick();
    assert!(a.app.aprs_on() && a.app.beacons_on(), "back as they were");
    assert_eq!(a.app.aprs_holder(), Some(Holder::Mode));
}
