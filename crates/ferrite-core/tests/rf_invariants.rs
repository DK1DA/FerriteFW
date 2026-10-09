//! The RF path's safety rules, checked on random sequences of everything that asks the path machine and the
//! transmit sequencer for a plan: the plans are run on a model of the pins and every step is checked.
//! - the relay moves only with no TX select, the PA at 0, both chips receiving and the unkey wait over, and the
//!   new path is set only after its settle time (SAFE-21, TX-28);
//! - never two RX selects or two TX selects; an RX and a TX select together only for the crossband repeater,
//!   on different chips (TX-27, XB-6), and the receiving chip never transmits;
//! - the crossband repeater never moves the relay, ramps the PA only with the repeat audio selected and puts the
//!   mic back only with the PA at zero (XB-2, XB-6, XB-8);
//! - a TX select only between key-up and unkey, the PA ramp only on a TX path with its chip transmitting.

use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::path::{Act, Cause, Path, Rx, Sel, Steps};
use ferrite_core::txplan::Region;
use ferrite_core::txseq::{Cfg, Kind, Req, TxSeq};

#[derive(Default)]
struct Pins {
    sel: [bool; 6],
    relay: bool,
    pa: bool,
    chip_tx: [bool; 2],
    muted: bool,
    repeat: bool,
    /// ms waited since the last chip went back to receive (None: none has yet).
    since_rx: Option<u32>,
    /// ms waited since the relay last moved.
    since_relay: u32,
}

impl Pins {
    fn run(&mut self, steps: &Steps, keyed: bool, what: &str) {
        for &a in steps.as_slice() {
            match a {
                Act::Wait(ms) => {
                    self.since_relay += ms as u32;
                    self.since_rx = self.since_rx.map(|t| t + ms as u32);
                }
                Act::Relay(r) => {
                    assert!(!self.pa, "{what}: relay with PA drive");
                    assert!(self.chip_tx == [false; 2], "{what}: relay while a chip transmits");
                    assert!(Sel::ALL.iter().all(|s| !self.sel[*s as usize]), "{what}: relay with a path selected");
                    assert!(self.since_rx.is_none_or(|t| t >= 100), "{what}: relay in the unkey wait");
                    if r != self.relay {
                        assert!(self.muted, "{what}: relay moved unmuted");
                        self.since_relay = 0;
                    }
                    self.relay = r;
                }
                Act::Sel(s, on) => {
                    if on {
                        assert!(self.since_relay >= 10, "{what}: path before the relay settled");
                        assert_eq!(s.relay(), self.relay, "{what}: path on the wrong relay state");
                        assert!(!self.pa, "{what}: path selected with PA drive");
                        if s.is_tx() {
                            assert!(keyed, "{what}: TX select outside key-up");
                        }
                    }
                    self.sel[s as usize] = on;
                    let rx = Sel::ALL.iter().filter(|s| !s.is_tx() && self.sel[**s as usize]).count();
                    let tx = Sel::ALL.iter().filter(|s| s.is_tx() && self.sel[**s as usize]).count();
                    assert!(rx <= 1 && tx <= 1, "{what}: two selects on ({rx} RX, {tx} TX)");
                    if rx + tx == 2 {
                        let (r, t) = (self.rx_sel().unwrap(), self.tx_sel().unwrap());
                        assert_ne!(r.chip(), t.chip(), "{what}: RX and TX selects on one chip");
                    }
                }
                Act::PaZero => self.pa = false,
                Act::PaRamp => {
                    let tx = Sel::ALL.iter().find(|s| s.is_tx() && self.sel[**s as usize]).expect("PA ramp without a TX path");
                    assert!(self.chip_tx[tx.chip() as usize], "{what}: PA ramp before the chip transmits");
                    if self.rx_sel().is_some() {
                        assert!(self.repeat, "{what}: crossband PA ramp before the repeat audio");
                    }
                    self.pa = true;
                }
                Act::ChipTx(n) => {
                    let tx = Sel::ALL.iter().find(|s| s.is_tx() && self.sel[**s as usize]).expect("chip TX without a TX path");
                    assert_eq!(tx.chip(), n, "{what}: the wrong chip keyed");
                    assert!(self.rx_sel().is_none_or(|r| r.chip() != n), "{what}: the receiving chip keyed");
                    self.chip_tx[n as usize] = true;
                }
                Act::RepeatAudio(on) => {
                    if !on {
                        assert!(!self.pa, "{what}: mic back with the PA up");
                    }
                    self.repeat = on;
                }
                Act::ChipRx(n) => {
                    assert!(!self.pa, "{what}: chip off TX with PA drive");
                    self.chip_tx[n as usize] = false;
                    self.since_rx = Some(0);
                }
                Act::Mute(m) => self.muted = m,
                Act::SiInput(_) | Act::EndTone | Act::CwId | Act::TailElim => {
                    if matches!(a, Act::EndTone | Act::CwId | Act::TailElim) {
                        assert!(self.pa && keyed, "{what}: tones while not keyed");
                    }
                }
            }
        }
    }
}

impl Pins {
    fn rx_sel(&self) -> Option<Sel> {
        Sel::ALL.into_iter().find(|s| !s.is_tx() && self.sel[*s as usize])
    }

    fn tx_sel(&self) -> Option<Sel> {
        Sel::ALL.into_iter().find(|s| s.is_tx() && self.sel[*s as usize])
    }
}

/// A small LCG: the same sequences every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as u32
    }
}

const FREQS: [Freq10; 7] = [mhz(146) + 52_000, mhz(147), mhz(223) + 50_000, mhz(446), mhz(28) + 40_000, mhz(29) + 60_000, mhz(243)];

#[test]
fn random_sequences_keep_the_rules() {
    let mut rng = Rng(950);
    let cfg = Cfg { end_tone: true, cw_id: true, tail: true, timeout_s: 30, ..Cfg::default() };
    let (mut keys, mut moves, mut ptt_moves) = (0, 0, 0);
    for run in 0..200 {
        let (mut path, mut seq, mut pins) = (Path::new(), TxSeq::new(), Pins { since_relay: 1_000, ..Pins::default() });
        let mut now = 0u32;
        let mut s = Steps::default();
        path.safe(now, &mut s);
        pins.run(&s, false, "safe");
        for i in 0..300 {
            now += 1 + rng.next(5_000);
            let mut s = Steps::default();
            let what = std::format!("run {run} step {i}");
            let f = FREQS[rng.next(FREQS.len() as u32) as usize];
            let keyed_before = path.keyed().is_some();
            match rng.next(8) {
                0 | 1 => {
                    let cause = [Cause::ActiveSlot, Cause::Auto, Cause::HfReceiver, Cause::Ptt][rng.next(4) as usize];
                    let rx = if rng.next(6) == 0 { Rx::Si } else { Rx::Bk(Sel::ALL[rng.next(6) as usize]) };
                    let r = path.rx(rx, cause, now, &mut s);
                    assert!(r.is_err() || !keyed_before, "{what}: RX change while keyed");
                }
                2 | 3 => {
                    let rx = if rng.next(3) == 0 { FREQS[rng.next(FREQS.len() as u32) as usize] } else { f };
                    let r = Req {
                        tx: f,
                        rx,
                        kind: if rng.next(3) == 0 { Kind::Data } else { Kind::Voice },
                        person: rng.next(2) == 0,
                        confirmed: rng.next(2) == 0,
                        am: rng.next(10) == 0,
                        region: Region::Us,
                        extend: rng.next(2) == 0,
                        cal_ok: rng.next(20) != 0,
                        batt_ok: rng.next(20) != 0,
                        power: rng.next(3) as u8,
                    };
                    let relay = path.relay();
                    if seq.key(&r, &mut path, now, &mut s).is_ok() {
                        keys += 1;
                        ptt_moves += (path.relay() != relay) as u32;
                    }
                }
                4 => seq.release(&cfg, &mut path, now, &mut s),
                5 => {
                    let _ = seq.tick(&cfg, rng.next(30) != 0, &mut path, now, &mut s);
                }
                6 => {
                    let _ = path.rx_off(&mut s);
                }
                _ => {
                    if rng.next(10) == 0 {
                        seq.safe(&mut path, now, &mut s);
                    }
                }
            }
            moves += s.as_slice().iter().filter(|a| matches!(a, Act::Relay(_))).count();
            // Keyed during the plan: from the start for an unkey, from the TX select for a key-up.
            pins.run(&s, keyed_before || path.keyed().is_some(), &what);
            assert_eq!(seq.keyed(), path.keyed().is_some(), "{what}: the sequencer and the path disagree");
            if path.keyed().is_none() {
                assert!(pins.chip_tx == [false; 2] && !pins.pa, "{what}: unkeyed with RF on");
            }
        }
    }
    assert!(keys > 500 && moves > 500 && ptt_moves > 20, "the sequences reach it all: {keys} key-ups, {moves} relay steps, {ptt_moves} at PTT");
}

/// The crossband repeater (XB-2, XB-6, XB-8, decision 93): the receive path hops between the two sides while
/// ready, a pass keys the other chip on its own frequency with the receiver left on, and through it all the relay
/// never moves, the receiving chip never transmits and the mic is never back with the PA up.
#[test]
fn crossband_sequences_keep_the_rules() {
    use ferrite_core::msg::XbCfg;
    use ferrite_core::xb::{Cmd, Repeater};
    let pairs = [(mhz(146) + 52_000, mhz(446)), (mhz(145) + 50_000, mhz(433) + 50_000), (mhz(147), mhz(442))];
    let mut rng = Rng(93);
    let (mut keys, mut refused) = (0, 0);
    for run in 0..100 {
        let (f1, f2) = pairs[run % pairs.len()];
        let f = [f1, f2];
        let cfg = Cfg { cw_id: true, cw_each: true, timeout_s: 200, ..Cfg::default() };
        let (mut path, mut seq, mut pins) = (Path::new(), TxSeq::new(), Pins { since_relay: 1_000, ..Pins::default() });
        let mut rep = Repeater::new(XbCfg { timeout_10s: 3, hang_ds: rng.next(30) as u8, ..XbCfg::DEFAULT }, 0);
        let mut now = 0u32;
        let mut s = Steps::default();
        path.safe(now, &mut s);
        pins.run(&s, false, "safe");
        let mut s = Steps::default();
        path.rx(Rx::Bk(Sel::rx(f1)), Cause::ActiveSlot, now, &mut s).unwrap();
        pins.run(&s, false, "start");
        let relay = pins.relay;
        let mut open = [false; 2];
        let mut hop = 0usize;
        for i in 0..2_000 {
            now += 10;
            let what = std::format!("run {run} step {i}");
            if rng.next(40) == 0 {
                open[rng.next(2) as usize] ^= true;
            }
            let mut s = Steps::default();
            // Ready: the receive path hops between the sides every 50 ms; a pass keeps its side's.
            let heard = match rep.from() {
                Some(from) => {
                    let mut h = [None; 2];
                    h[from as usize] = Some(open[from as usize]);
                    h
                }
                None => {
                    if i % 5 == 0 {
                        hop ^= 1;
                        let _ = path.rx(Rx::Bk(Sel::rx(f[hop])), Cause::Auto, now, &mut s);
                    }
                    let mut h = [None; 2];
                    h[hop] = Some(open[hop]);
                    h
                }
            };
            let keyed_before = path.keyed().is_some();
            match rep.tick(now, heard) {
                Some(Cmd::Key { from }) => {
                    let to = 1 - from as usize;
                    let r = Req {
                        tx: f[to],
                        rx: f[from as usize],
                        kind: Kind::Voice,
                        person: false,
                        confirmed: false,
                        am: false,
                        region: Region::Us,
                        extend: false,
                        cal_ok: true,
                        batt_ok: rng.next(30) != 0,
                        power: rng.next(2) as u8,
                    };
                    match seq.key_xb(&r, &mut path, now, &mut s) {
                        Ok(()) => keys += 1,
                        Err(_) => {
                            refused += 1;
                            rep.refused(now);
                        }
                    }
                }
                Some(Cmd::Unkey { id }) => seq.release(&Cfg { cw_id: id, ..cfg }, &mut path, now, &mut s),
                None => {
                    if let Some(e) = seq.tick(&cfg, true, &mut path, now, &mut s) {
                        if e != ferrite_core::txseq::Event::PreAlert {
                            rep.cut(now);
                        }
                    }
                }
            }
            if rng.next(500) == 0 {
                seq.safe(&mut path, now, &mut s);
                rep.cut(now);
            }
            assert!(!s.as_slice().iter().any(|a| matches!(a, Act::Relay(r) if *r != relay)), "{what}: the relay moved");
            pins.run(&s, keyed_before || path.keyed().is_some(), &what);
            assert_eq!(pins.relay, relay, "{what}: the relay moved");
            if path.keyed().is_none() {
                assert!(pins.chip_tx == [false; 2] && !pins.pa && !pins.repeat, "{what}: unkeyed with RF or the repeat audio on");
            }
        }
    }
    assert!(keys > 200 && refused > 0, "the sequences reach it all: {keys} passes, {refused} refused");
}

/// The Radio task's RAM for these: kept small (each documents its size).
#[test]
fn sizes() {
    use core::mem::size_of;
    assert_eq!(size_of::<Path>(), 48);
    assert_eq!(size_of::<Steps>(), 33);
    assert_eq!(size_of::<TxSeq>(), 28);
    assert_eq!(size_of::<ferrite_core::watch::Watch>(), 36);
    assert_eq!(size_of::<ferrite_core::scan::Scan>(), 60);
    // The repeater: 168 bytes, 100 of them the running screen's XbLive (the 60-column strip); held only while
    // the repeater runs, in the Radio task's state.
    assert_eq!(size_of::<ferrite_core::xb::Repeater>(), 168);
}
