//! APRS mode on slot C (APRS-13, APRS-15 to APRS-20) and the simulator's APRS task.

use super::*;
use crate::aprs::{AprsCfg, APRS_SLOT};
use crate::data::{Data, Draft, MsgState, Op};
use crate::sample::Sample;
use ferrite_core::txplan::Region;

/// An app on the sample records, kept for the whole test.
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

    fn tap(&mut self, k: Key) {
        self.press(k, 100);
    }

    fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.tick();
        }
    }

    fn cmds(&mut self) -> Vec<RadioCmd> {
        let mut v = Vec::new();
        while let Some(c) = self.app.pop_cmd() {
            v.push(c);
        }
        v
    }

    fn mode(&mut self, on: bool) {
        self.app.set_aprs_mode(on);
        self.tick();
    }
}

/// APRS-15, APRS-16: C tunes to the region's APRS frequency at the beacon power and is watched whatever Watch
/// says; off, C goes back exactly as it was.
#[test]
fn aprs_mode_takes_slot_c() {
    let mut a = A::new();
    let own = a.app.slot(APRS_SLOT).freq();
    a.app.set_watch(0);
    a.cmds();
    a.mode(true);
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: 14_439_000 }), "no region: 144.390");
    assert!(c.contains(&RadioCmd::Watch(0b100)), "C watched with Watch off: {c:?}");
    assert!(c.contains(&RadioCmd::SlotParam { slot: 2, param: Param::Power, value: 0 }), "beacon power Low");
    assert!(c.contains(&RadioCmd::SlotParam { slot: 2, param: Param::Mode, value: 0 }), "FM");
    assert_eq!(a.app.slot(APRS_SLOT).freq(), own, "C's own VFO is kept aside");
    // The region's frequency, and 0 5 7 1 over it.
    for (r, f) in [(Region::Uk, 14_480_000), (Region::Australia, 14_517_500), (Region::NewZealand, 14_457_500), (Region::Japan, 14_464_000)] {
        a.app.set.set_region(Some(r));
        assert_eq!(a.app.aprs_freq(), f);
    }
    a.app.set.set_region(Some(Region::Canada));
    a.cmds();
    a.app.step_aprs_freq(2);
    assert_eq!(a.app.aprs_freq(), 14_440_000);
    assert!(a.cmds().contains(&RadioCmd::TuneSlot { slot: 2, freq: 14_440_000 }));
    a.tick();
    assert_ne!(a.s.aprs_cfg().f5k, 0, "written back to the APRS store");
    a.app.step_aprs_freq(-2);
    assert_eq!(a.app.aprs_cfg().f5k, 0, "back on the region's: follows the region again");
    a.mode(false);
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: own }), "C back as it was");
    assert!(c.contains(&RadioCmd::Watch(0)));
}

/// APRS-19: C can be the active slot, but tuning, V/M, entry, the edit grid and quick settings refuse.
#[test]
fn aprs_slot_is_locked() {
    let mut a = A::new();
    a.mode(true);
    a.tap(Key::Slot);
    a.tap(Key::Slot);
    assert_eq!(a.app.active(), 2);
    a.cmds();
    for k in [Key::Up, Key::Down, Key::Left, Key::VfoMem, Key::Digit(1)] {
        a.tap(k);
        assert!(matches!(a.app.screen(), Screen::Home), "{k:?}");
        assert!(a.app.toast_up());
    }
    a.press(Key::Slot, 800);
    assert!(matches!(a.app.screen(), Screen::Home), "no edit grid");
    a.app.knob(1, a.t, &a.s);
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::TuneSlot { .. } | RadioCmd::SlotParam { .. })), "nothing changed C");
    assert!(a.app.greyed(Item::Power).is_some() && a.app.greyed(Item::Watch).is_none());
    // Hold MENU on it: its options (APRS-17).
    a.press(Key::Menu, 800);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::AprsCard));
    // Off: C changes again.
    a.press(Key::Exit, 800);
    a.mode(false);
    a.tap(Key::Up);
    assert!(a.cmds().iter().any(|c| matches!(c, RadioCmd::TuneSlot { slot: 2, .. })));
}

/// APRS-18: with the APRS slot (C) active, PTT A opens compose from any screen without keying; in compose PTT
/// sends. PTT B keys B as ever.
#[test]
fn ptt_on_aprs_slot_composes_and_ptt_sends() {
    let mut a = A::new();
    a.mode(true);
    a.app.active = 2;
    a.app.open(crate::screens::nav::Dest::AprsHeard);
    a.cmds();
    a.press(Key::Ptt(0), 100);
    assert!(matches!(a.app.screen(), Screen::Compose(_)), "{}", a.app.screen().name());
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "no voice");
    a.s.request(Op::Draft(Draft { to: Buf::from(b"VE3KPW-9".as_slice()), text: Buf::from(b"On my way".as_slice()) }));
    let n = a.s.messages();
    a.press(Key::Ptt(0), 100);
    assert_eq!(a.s.messages(), n + 1, "sent");
    assert!(a.s.message(0).is_some_and(|m| !m.inbound && m.state == MsgState::Pending));
    assert!(!matches!(a.app.screen(), Screen::Compose(_)));
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
    // The connector's PTT on the APRS slot composes too; PTT B never reaches C.
    a.press(Key::Exit, 800);
    a.press(Key::PTT, 100);
    assert!(matches!(a.app.screen(), Screen::Compose(_)));
    a.press(Key::Exit, 100);
    a.press(Key::Exit, 800);
    a.app.key_down(Key::Ptt(1), a.t, &a.s);
    assert!(!matches!(a.app.screen(), Screen::Compose(_)));
    a.app.key_up(Key::Ptt(1), a.t + 100, &a.s);
}

/// Beacon mode, power and on/off (0 5 5 1, 0 5 5 4, APRS-17).
#[test]
fn beacon_settings() {
    let mut a = A::new();
    let mut b = Buf::<24>::new();
    a.app.beacon_mode_text(&mut b);
    assert_eq!(b.as_bytes(), b"Off");
    a.app.toggle_beacon();
    a.app.beacon_mode_text(&mut b);
    assert_eq!(b.as_bytes(), b"Every 10 min", "on with none chosen");
    a.app.step_beacon(-4);
    a.app.beacon_mode_text(&mut b);
    assert_eq!(b.as_bytes(), b"Manual");
    a.app.toggle_beacon();
    assert_eq!(a.app.aprs_cfg().mode(), 0);
    a.app.toggle_beacon();
    assert_eq!(a.app.aprs_cfg().mode(), 1, "on again in its mode");
    a.app.set_aprs(AprsCfg { power: 2, ..a.app.aprs_cfg() });
    a.mode(true);
    assert!(a.cmds().contains(&RadioCmd::SlotParam { slot: 2, param: Param::Power, value: 2 }));
}

/// The simulator's APRS task: retries on APRS-13's schedule, an ack from a heard station on the second try,
/// failed after five tries, messages for us, beacons waiting for the relay (APRS-20).
#[test]
fn engine_timeline() {
    let s = Sample::new();
    let draft = |to: &[u8]| Draft { to: Buf::from(to), text: Buf::from(b"Test".as_slice()) };
    s.request(Op::Draft(draft(b"VE3KPW-9")));
    assert!(s.request(Op::Send));
    let n = s.aprs_tick(0, true, true);
    assert_eq!(n.tx, Some("Message"));
    assert_eq!(s.message(0).unwrap().state, MsgState::Pending);
    for t in 1..30 {
        assert!(s.aprs_tick(t, true, true).tx != Some("Message"), "{t}");
    }
    assert_eq!(s.aprs_tick(30, true, true).tx, Some("Message"));
    assert_eq!(s.message(0).unwrap().state, MsgState::Retrying(2));
    for t in 31..34 {
        s.aprs_tick(t, true, true);
    }
    assert_eq!(s.message(0).unwrap().state, MsgState::Acked, "a heard station acks the second try");

    // Nobody heard: tried 5 times at 0, 30, 90, 210 and 450 s, failed 240 s after the last.
    let s = Sample::new();
    s.request(Op::Draft(draft(b"NOCALL")));
    s.request(Op::Send);
    let mut tries = Vec::new();
    for t in 0..800 {
        let n = s.aprs_tick(t, true, true);
        if n.tx == Some("Message") {
            tries.push(t);
        }
        let m = (0..s.messages()).filter_map(|i| s.message(i)).find(|m| !m.inbound).unwrap();
        if t == 689 {
            assert_eq!(m.state, MsgState::Retrying(5));
        }
        if t == 690 {
            assert_eq!(m.state, MsgState::Failed);
        }
    }
    assert_eq!(tries, [0, 30, 90, 210, 450]);
    assert!(s.stations() > 11, "new stations heard");
    assert!((0..s.messages()).filter_map(|i| s.message(i)).any(|m| m.inbound && m.state == MsgState::Unread && m.ago_s < 800));

    // A timed beacon waits while the relay is the other way, then goes.
    let s = Sample::new();
    s.request(Op::AprsCfg(AprsCfg { beacon: 2, ..AprsCfg::DEFAULT })); // every 1 min
    assert_eq!(s.aprs_tick(0, true, false).tx, None);
    assert!(s.beacon().waiting);
    let mut sent = None;
    for t in 1..10 {
        let n = s.aprs_tick(t, true, true);
        if n.tx == Some("Beacon") {
            sent = Some(t);
            break;
        }
    }
    assert_eq!(sent, Some(1), "as soon as the relay allows");
    assert!(!s.beacon().waiting);
    assert_eq!(s.beacon().next_s, Some(60));
    // Off: nothing goes.
    for t in 100..400 {
        assert_eq!(s.aprs_tick(t, false, true).tx, None);
    }
}

/// APRS-17, APRS-20: the card's lines.
#[test]
fn card_text() {
    let mut a = A::new();
    a.mode(true);
    a.app.set_aprs(AprsCfg { beacon: 2, ..a.app.aprs_cfg() });
    a.tick();
    a.s.aprs_tick(0, true, false);
    let mut b = Buf::<28>::new();
    a.app.set_vfo(0, 1_407_400); // A on HF holds the relay
    a.app.beacon_text(&a.s, &mut b);
    assert_eq!(b.as_bytes(), b"Beacon waiting: HF");
    a.s.aprs_tick(1, true, true);
    a.app.beacon_text(&a.s, &mut b);
    assert!(b.as_bytes().starts_with(b"Next beacon in"), "{:?}", core::str::from_utf8(b.as_bytes()));
    let mut l = Buf::<80>::new();
    crate::aprs::last_text(&a.s, &mut l);
    assert!(l.as_bytes().starts_with(b"VE3KPW-9"), "{:?}", core::str::from_utf8(l.as_bytes()));
    let mut fb = FrameBuffer::new();
    let dev = Device::default();
    a.app.render(&mut fb, &a.live, &dev, &a.s);
}
