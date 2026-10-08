//! Watching without Follow (WATCH-5, decision 81): the App with the real watch engine behind it. A watched slot
//! that is heard plays and shows RX; the active slot changes only when the user changes it.

use super::*;
use crate::persist::{Persist, Sink, SliceSource};
use ferrite_core::watch::{self, Watch};

/// The Radio task's side, as the firmware will run it: the App's commands in, the engine's squelch out.
struct Radio {
    w: Watch,
    set: u8,
    active: u8,
    on_chip: [Option<u8>; 2],
    heard_audio: bool,
}

impl Radio {
    fn new() -> Self {
        Radio { w: Watch::new(), set: 0, active: 0, on_chip: [None; 2], heard_audio: false }
    }

    /// 10 ms: the App's commands, one engine turn with `heard` slots carrying a signal, then the App's tick.
    fn step(&mut self, t: &mut T, heard: u8) {
        for c in t.cmds() {
            match c {
                RadioCmd::Watch(m) => self.set = m,
                RadioCmd::Active(a) => self.active = a,
                _ => {}
            }
        }
        let freq = [0, 1, 2].map(|s| t.app.slot(s).freq());
        let cfg = watch::Cfg { set: self.set, active: self.active, freq, muted: 0 };
        let sq = self.on_chip.map(|o| o.is_some_and(|s| heard & 1 << s != 0));
        let out = self.w.tick(t.t, &cfg, sq, false);
        self.on_chip = out.chip;
        self.heard_audio |= out.audio == Some(1);
        for (i, s) in t.live.slots.iter_mut().enumerate() {
            s.squelch_open = self.w.open(i as u8);
        }
        t.t += 10;
        t.app.tick(t.t, &t.live, &store());
    }

    fn run(&mut self, t: &mut T, ms: u32, heard: u8) {
        for _ in 0..ms / 10 {
            self.step(t, heard);
        }
    }
}

/// B heard for 10 s and gone: its audio plays, but A stays active on home and on any screen, in the App and
/// in the radio.
#[test]
fn heard_slot_never_becomes_active() {
    let mut t = T::new();
    t.app.out.push(RadioCmd::Watch(t.app.watch_mask()));
    let mut r = Radio::new();
    r.run(&mut t, 500, 0);
    r.run(&mut t, 10_000, 0b010);
    assert!(r.heard_audio, "B's audio played");
    assert!(t.live.slots[1].squelch_open, "B's card shows RX");
    assert_eq!((t.app.active(), r.active), (0, 0), "A stays active");
    r.run(&mut t, 5_000, 0);
    assert_eq!(t.app.active(), 0, "and after B closes");
    t.open(crate::screens::nav::Dest::Status);
    r.run(&mut t, 3_000, 0b010);
    assert_eq!(t.app.active(), 0, "a screen open: the same");
}

/// The two spare flag bits (Follow's, decision 81) keep scan inputs' bit in place: old records still load it.
#[test]
fn flags_round_trip_around_the_spare_bits() {
    struct V(Vec<u8>);
    impl Sink for V {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    let save = |s: &crate::settings::Settings| {
        let mut v = V(Vec::new());
        s.save(&mut v);
        v.0
    };
    let load = |b: &[u8]| {
        let mut s = crate::settings::Settings::DEFAULT;
        s.load(&mut SliceSource(b));
        s
    };
    let d = crate::settings::Settings::DEFAULT;
    for (h, c, x) in [(true, false, true), (false, true, false), (true, true, true)] {
        let a = crate::settings::Settings { show_hidden: h, chan_only: c, scan_tx: x, ..d };
        let b = load(&save(&a));
        assert_eq!((b.show_hidden, b.chan_only, b.scan_tx, b.spare_flag1, b.spare_flag2), (h, c, x, false, false));
    }
    // A record saved with Follow's bits set (Follow off, a change from its old default): ignored.
    let mut v = save(&crate::settings::Settings { scan_tx: !d.scan_tx, ..d });
    let i = v.len() - 20 - 1; // the flag byte, before the power-on message, subtone level, scan resume and priority check
    v[i] |= 0b1100;
    let b = load(&v);
    assert_eq!(b.scan_tx, !d.scan_tx, "scan inputs kept its bit");
    // A record from before the flag byte at all: the defaults.
    let b = load(&v[..i]);
    assert_eq!((b.show_hidden, b.chan_only, b.scan_tx), (d.show_hidden, d.chan_only, d.scan_tx));
    let n = save(&d).len();
    assert!(n <= crate::link::RECORD_MAX, "the link writes the settings as one record: {n} bytes");
}

/// Power-on message (0 7 3 2): mode and text round trip after the flag byte; a record without them keeps the
/// default (callsign); the boot line follows the mode (BOOT-9).
#[test]
fn power_on_message_round_trip_and_boot_line() {
    struct V(Vec<u8>);
    impl Sink for V {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    let mut a = crate::settings::Settings { pon: 2, chan_only: true, ..crate::settings::Settings::DEFAULT };
    a.set_callsign(b"va7fer");
    b"Hello there".iter().for_each(|&c| _ = a.pon_text.push(c));
    let mut v = V(Vec::new());
    a.save(&mut v);
    let mut b = crate::settings::Settings::DEFAULT;
    b.load(&mut SliceSource(&v.0));
    assert_eq!((b.pon, b.pon_text, b.chan_only), (2, a.pon_text, true));
    assert_eq!(b.boot_line(), Some(b"Hello there".as_slice()));
    let mut c = crate::settings::Settings::DEFAULT;
    c.load(&mut SliceSource(&v.0[..v.0.len() - 20]));
    assert_eq!((c.pon, c.chan_only), (1, true), "an older record: callsign, its flags kept");
    assert_eq!(c.boot_line(), Some(b"VA7FER".as_slice()));
    c.pon = 0;
    assert_eq!(c.boot_line(), None, "off");
    c.pon = 2;
    assert_eq!(c.boot_line(), None, "text, but none written");
}
