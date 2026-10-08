//! Follow and follow return (WATCH-5, 0 1 2 2 and 0 1 2 3): the App with the real watch engine behind it.

use super::*;
use crate::persist::{Persist, Sink, SliceSource};
use ferrite_core::msg::FollowLive;
use ferrite_core::watch::{self, Follow, Watch};

/// The Radio task's side, as the firmware will run it: the App's commands in, the engine's follow out.
struct Radio {
    w: Watch,
    set: u8,
    active: u8,
    follow: Follow,
    on_chip: [Option<u8>; 2],
}

impl Radio {
    fn new() -> Self {
        Radio { w: Watch::new(), set: 0, active: 0, follow: Follow::Off, on_chip: [None; 2] }
    }

    /// 10 ms: the App's commands, one engine turn with `heard` slots carrying a signal, then the App's tick.
    fn step(&mut self, t: &mut T, heard: u8) {
        for c in t.cmds() {
            match c {
                RadioCmd::Watch(m) => self.set = m,
                RadioCmd::Active(a) => self.active = a,
                RadioCmd::Follow(f) => self.follow = f,
                _ => {}
            }
        }
        let freq = [0, 1, 2].map(|s| t.app.slot(s).freq());
        let cfg = watch::Cfg { set: self.set, active: self.active, freq, muted: 0, follow: self.follow };
        let sq = self.on_chip.map(|o| o.is_some_and(|s| heard & 1 << s != 0));
        let out = self.w.tick(t.t, &cfg, sq, false);
        self.on_chip = out.chip;
        if let Some(s) = out.follow {
            t.live.follow = FollowLive { n: t.live.follow.n.wrapping_add(1), slot: s };
        }
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

/// A fresh App and its radio, A active and B watched (Watch's default).
fn setup(follow: bool, back: bool) -> (T, Radio) {
    let mut t = T::new();
    (t.app.set.follow, t.app.set.follow_back) = (follow, back);
    t.app.out.push(RadioCmd::Watch(t.app.watch_mask()));
    t.app.send_radio_settings();
    let mut r = Radio::new();
    r.run(&mut t, 500, 0);
    assert_eq!(t.app.active(), 0);
    (t, r)
}

#[test]
fn defaults_follow_and_switch_back() {
    let s = crate::settings::Settings::DEFAULT;
    assert!(s.follow && s.follow_back);
    let mut t = T::new();
    let mut m = Buf::<24>::new();
    let dev = Device::default();
    assert_eq!(crate::screens::groups::wired(&t.app, &dev, &[1, 2, 2], &mut m), Some(false));
    assert_eq!(m.as_bytes(), b"On");
    assert_eq!(crate::screens::groups::wired(&t.app, &dev, &[1, 2, 3], &mut m), Some(false));
    assert_eq!(m.as_bytes(), b"Switch back");
    assert!(crate::screens::groups::change(&mut t.app, &[1, 2, 3], 1));
    assert!(t.cmds().contains(&RadioCmd::Follow(Follow::On)));
    assert!(crate::screens::groups::change(&mut t.app, &[1, 2, 2], 1));
    assert!(t.cmds().contains(&RadioCmd::Follow(Follow::Off)));
    crate::screens::groups::reset(&mut t.app, &[1, 2, 2]);
    crate::screens::groups::reset(&mut t.app, &[1, 2, 3]);
    let follows: Vec<_> = t.cmds().into_iter().filter(|c| matches!(c, RadioCmd::Follow(_))).collect();
    assert!(follows.ends_with(&[RadioCmd::Follow(Follow::On), RadioCmd::Follow(Follow::Return)]));
}

#[test]
fn follows_then_returns_2_s_after_it_closes() {
    let (mut t, mut r) = setup(true, true);
    r.run(&mut t, 200, 0b010); // B heard
    assert_eq!(t.app.active(), 1, "B became active");
    r.run(&mut t, 3_000, 0b010);
    assert_eq!(t.app.active(), 1, "still B while it is open");
    r.run(&mut t, 1_900, 0);
    assert_eq!(t.app.active(), 1, "within the 2 s after it closed");
    r.run(&mut t, 300, 0);
    assert_eq!(t.app.active(), 0, "back to A");
    assert_eq!(r.active, 0, "and the radio told");
}

#[test]
fn follow_off_never_switches() {
    let (mut t, mut r) = setup(false, true);
    r.run(&mut t, 3_000, 0b010);
    assert_eq!(t.app.active(), 0);
    // Off in the App wins even over a radio that still follows.
    let (mut t, mut r) = setup(true, true);
    t.app.set.follow = false;
    r.run(&mut t, 1_000, 0b010);
    assert_eq!(t.app.active(), 0);
}

#[test]
fn stay_does_not_return() {
    let (mut t, mut r) = setup(true, false);
    r.run(&mut t, 200, 0b010);
    assert_eq!(t.app.active(), 1);
    r.run(&mut t, 10_000, 0);
    assert_eq!(t.app.active(), 1, "stays on B");
}

#[test]
fn only_on_home_and_not_to_a_scan_stop() {
    let (mut t, mut r) = setup(true, true);
    t.open(crate::screens::nav::Dest::Status);
    r.run(&mut t, 1_000, 0b010);
    assert_eq!(t.app.active(), 0, "a screen open: the slot it shows doesn't change");
    t.app.go_home();
    r.run(&mut t, 100, 0b010);
    assert_eq!(t.app.active(), 1, "home again, B still heard: followed");
    let (mut t, mut r) = setup(true, true);
    t.live.slots[1].scanning = true;
    r.run(&mut t, 1_000, 0b010);
    assert_eq!(t.app.active(), 0);
}

#[test]
fn follow_settings_round_trip() {
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
    for (f, b) in [(true, true), (true, false), (false, true), (false, false)] {
        let a = crate::settings::Settings { follow: f, follow_back: b, chan_only: true, ..crate::settings::Settings::DEFAULT };
        let c = load(&save(&a));
        assert_eq!((c.follow, c.follow_back, c.chan_only, c.show_hidden), (f, b, true, false));
    }
    // A record from before the follow bits: its last byte had show_hidden and chan_only only.
    let mut v = save(&crate::settings::Settings::DEFAULT);
    v.truncate(v.len() - 18); // before the power-on message and subtone level, which follow the flags
    *v.last_mut().unwrap() = 0b11;
    let c = load(&v);
    assert_eq!((c.show_hidden, c.chan_only, c.follow, c.follow_back), (true, true, true, true), "the defaults");
    // And from before that byte at all.
    let c = load(&v[..v.len() - 1]);
    assert!(c.follow && c.follow_back);
    let n = save(&crate::settings::Settings::DEFAULT).len();
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
    let mut a = crate::settings::Settings { pon: 2, follow: false, ..crate::settings::Settings::DEFAULT };
    a.set_callsign(b"va7fer");
    b"Hello there".iter().for_each(|&c| _ = a.pon_text.push(c));
    let mut v = V(Vec::new());
    a.save(&mut v);
    let mut b = crate::settings::Settings::DEFAULT;
    b.load(&mut SliceSource(&v.0));
    assert_eq!((b.pon, b.pon_text, b.follow), (2, a.pon_text, false));
    assert_eq!(b.boot_line(), Some(b"Hello there".as_slice()));
    let mut c = crate::settings::Settings::DEFAULT;
    c.load(&mut SliceSource(&v.0[..v.0.len() - 18]));
    assert_eq!((c.pon, c.follow), (1, false), "an older record: callsign, its flags kept");
    assert_eq!(c.boot_line(), Some(b"VA7FER".as_slice()));
    c.pon = 0;
    assert_eq!(c.boot_line(), None, "off");
    c.pon = 2;
    assert_eq!(c.boot_line(), None, "text, but none written");
}
