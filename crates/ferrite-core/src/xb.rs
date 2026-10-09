//! The crossband repeater's engine (manual chapter 19, XB-2 to XB-17; decision 93): what each side heard in, a
//! key-up or an unkey out, and the running screen's numbers. Host-tested; the Radio task (the `tx` build) and the
//! simulators' fake radio drive the same one. The pins are the path machine's ([`crate::path::Path::key_up_xb`]),
//! the checks and the duty limit the sequencer's ([`crate::txseq::TxSeq::key_xb`]).
//!
//! As stock (`analysis/crossband_repeat.md`): the receiving chip stays in receive while the other transmits, and
//! the pass follows the receiving side's squelch. Unlike stock: both directions always (XB-4), a timeout per pass
//! with a lockout until the signal drops (XB-9), a hang time (XB-7), the self-hold window (XB-11) and the CW ID
//! (XB-12).

use crate::freq::Freq10;
use crate::msg::{XbCfg, XbLive, XB_COLS};
use crate::path::Sel;

/// XB-4: a pass starts after this long of open squelch (with the tone, tone gate on).
pub const DETECT_MS: u32 = 30;
/// XB-7: the squelch (or tone) closed this long ends the pass, then the hang time.
pub const DROP_MS: u32 = 100;
/// XB-11: a receiver opening this soon after a key-up or an unkey is self-hold, ignored until it closes.
pub const SELF_HOLD_MS: u32 = 50;
/// XB-12: the CW ID at a pass's unkey, at most this often.
pub const ID_MS: u32 = 10 * 60_000;
/// XB-16: one column of the activity strip.
pub const COL_MS: u32 = 30_000;

/// Why the radio won't start the repeater (XB-2's radio-side checks; the UI checks first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stop {
    /// A side is on HF: it needs the band relay.
    Hf = 1,
    /// Both sides on the same BK4829.
    OneChip = 2,
    /// The relay is energised now (the active slot on HF): the repeater never moves it (SAFE-20).
    Relay = 3,
    /// This build has no transmit.
    NoTx = 4,
}

impl Stop {
    pub fn from_code(c: u8) -> Option<Stop> {
        [Stop::Hf, Stop::OneChip, Stop::Relay, Stop::NoTx].into_iter().find(|s| *s as u8 == c)
    }

    pub fn text(self) -> &'static str {
        match self {
            Stop::Hf => "A side is on HF (needs the relay)",
            Stop::OneChip => "Both sides on one chip",
            Stop::Relay => "Relay set for HF: leave HF first",
            Stop::NoTx => "Receive-only build: no transmit",
        }
    }
}

/// XB-2 on the radio: two sides on different chips, neither on HF, and the relay released now (it never moves
/// while the repeater runs).
pub fn sides_ok(f: [Freq10; 2], relay: bool) -> Result<(), Stop> {
    let s = f.map(Sel::rx);
    if s.iter().any(|x| x.relay()) {
        return Err(Stop::Hf);
    }
    if s[0].chip() == s[1].chip() {
        return Err(Stop::OneChip);
    }
    if relay {
        return Err(Stop::Relay);
    }
    Ok(())
}

/// What the Radio task does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Key the other side's chip on its own receive frequency (XB-18), repeating side `from` (0 or 1).
    Key { from: u8 },
    /// Unkey (XB-8), sending the CW ID first when due (XB-12).
    Unkey { id: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    Ready,
    Pass { from: u8, last_open: u32 },
    Hang { from: u8, last_open: u32 },
}

/// The repeater (168 bytes, 100 of them the running screen's [`XbLive`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Repeater {
    pub cfg: XbCfg,
    st: St,
    /// Since when each side's squelch (and tone) has been open; None closed.
    open: [Option<u32>; 2],
    /// A side that timed out, was refused or self-held: it can't pass again until its squelch closes (XB-9).
    lock: [bool; 2],
    /// The side that timed out (shown as state 4 until it drops).
    timed_out: Option<u8>,
    /// The last key-up or unkey (XB-11's window).
    rf_t: Option<u32>,
    /// The last CW ID, and a pass since it (XB-12).
    last_id: Option<u32>,
    unided: bool,
    /// Local PTT holds the repeater (XB-14).
    held: bool,
    pass_t: u32,
    end_t: Option<u32>,
    start_t: u32,
    last_t: u32,
    /// The current strip column's start and its passed time (ms, + side 1 to 2, - side 2 to 1).
    col_t: u32,
    col_ms: i32,
    busy_ms: u32,
    pub live: XbLive,
}

impl Repeater {
    pub fn new(cfg: XbCfg, now: u32) -> Self {
        Repeater {
            cfg,
            st: St::Ready,
            open: [None; 2],
            lock: [false; 2],
            timed_out: None,
            rf_t: None,
            last_id: None,
            unided: false,
            held: false,
            pass_t: now,
            end_t: None,
            start_t: now,
            last_t: now,
            col_t: now,
            col_ms: 0,
            busy_ms: 0,
            live: XbLive::default(),
        }
    }

    /// The side being repeated now (its receiver must stay selected), if passing.
    pub fn from(&self) -> Option<u8> {
        match self.st {
            St::Pass { from, .. } | St::Hang { from, .. } => Some(from),
            St::Ready => None,
        }
    }

    /// Local PTT holds it (XB-14).
    pub fn held(&self) -> bool {
        self.held
    }

    /// The pass timeout (XB-9), ms.
    pub fn timeout_ms(&self) -> u32 {
        self.cfg.timeout_10s.clamp(3, 60) as u32 * 10_000
    }

    fn id_due(&mut self, now: u32) -> bool {
        let due = self.cfg.flags & XbCfg::ID != 0 && self.unided && self.last_id.is_none_or(|t| now.wrapping_sub(t) >= ID_MS);
        if due {
            (self.last_id, self.unided) = (Some(now), false);
        }
        due
    }

    fn end(&mut self, now: u32) -> Cmd {
        (self.st, self.end_t, self.rf_t) = (St::Ready, Some(now), Some(now));
        Cmd::Unkey { id: self.id_due(now) }
    }

    /// One tick: what each side heard (squelch, and the tone with the tone gate on), None for a side not
    /// listened to this tick (the receive path hops while ready; the pass keeps its side's).
    pub fn tick(&mut self, now: u32, heard: [Option<bool>; 2]) -> Option<Cmd> {
        self.stats(now);
        for (i, h) in heard.iter().enumerate() {
            match h {
                Some(true) => {
                    if self.open[i].is_none() {
                        self.open[i] = Some(now);
                        // XB-11: opening on the heels of our own key-up or unkey is self-hold.
                        if self.rf_t.is_some_and(|t| now.wrapping_sub(t) < SELF_HOLD_MS) && self.from() != Some(i as u8) {
                            self.lock[i] = true;
                        }
                    }
                }
                Some(false) => {
                    self.open[i] = None;
                    self.lock[i] = false;
                    if self.timed_out == Some(i as u8) {
                        self.timed_out = None;
                    }
                }
                None => {}
            }
        }
        if self.held {
            return None;
        }
        let cmd = match self.st {
            St::Ready => (0..2u8).find(|&i| !self.lock[i as usize] && self.open[i as usize].is_some_and(|t| now.wrapping_sub(t) >= DETECT_MS)).map(|from| {
                (self.st, self.pass_t, self.rf_t, self.unided) = (St::Pass { from, last_open: now }, now, Some(now), true);
                self.live.passes = self.live.passes.saturating_add(1);
                self.live.last_dir = from + 1;
                Cmd::Key { from }
            }),
            St::Pass { from, last_open } | St::Hang { from, last_open } => {
                let open = self.open[from as usize].is_some();
                let last_open = if open { now } else { last_open };
                if now.wrapping_sub(self.pass_t) >= self.timeout_ms() {
                    // XB-9: this side can't pass again until its squelch closes.
                    self.lock[from as usize] = true;
                    self.timed_out = Some(from);
                    Some(self.end(now))
                } else if open {
                    self.st = St::Pass { from, last_open };
                    None
                } else if now.wrapping_sub(last_open) >= DROP_MS + self.cfg.hang_ds.min(30) as u32 * 100 {
                    Some(self.end(now))
                } else {
                    self.st = if now.wrapping_sub(last_open) >= DROP_MS { St::Hang { from, last_open } } else { St::Pass { from, last_open } };
                    None
                }
            }
        };
        self.show(now);
        cmd
    }

    /// The key-up was refused (region, calibration, battery, duty): back to ready, that side locked until its
    /// signal drops, so it isn't asked again every tick.
    pub fn refused(&mut self, now: u32) {
        if let Some(from) = self.from() {
            self.lock[from as usize] = true;
            self.live.passes = self.live.passes.saturating_sub(1);
        }
        (self.st, self.rf_t) = (St::Ready, Some(now));
        self.live.reduced = true;
        self.show(now);
    }

    /// The sequencer unkeyed by itself (duty limit, battery under load, its backstop): the pass ends, its side
    /// locked until it drops.
    pub fn cut(&mut self, now: u32) {
        if let Some(from) = self.from() {
            self.lock[from as usize] = true;
        }
        (self.st, self.end_t, self.rf_t) = (St::Ready, Some(now), Some(now));
        self.live.reduced = true;
        self.show(now);
    }

    /// XB-14: local PTT holds the repeater (the Radio task unkeys any pass first, with [`Repeater::cut`]'s
    /// bookkeeping through `release`); it carries on after.
    pub fn hold(&mut self, on: bool, now: u32) {
        self.held = on;
        if on && self.from().is_some() {
            (self.st, self.end_t) = (St::Ready, Some(now));
        }
        self.rf_t = Some(now);
        self.show(now);
    }

    /// The level heard on the input side (dBm), for the screen.
    pub fn set_rssi(&mut self, rssi: i16) {
        self.live.rssi = rssi;
    }

    /// Time passing: run and pass seconds, the busy total and the activity strip (XB-16, XB-17).
    fn stats(&mut self, now: u32) {
        let dt = now.wrapping_sub(self.last_t);
        self.last_t = now;
        if let Some(from) = self.from() {
            self.busy_ms = self.busy_ms.saturating_add(dt);
            self.col_ms += if from == 0 { dt as i32 } else { -(dt as i32) };
        }
        while now.wrapping_sub(self.col_t) >= COL_MS {
            self.col_t = self.col_t.wrapping_add(COL_MS);
            self.live.strip.copy_within(1.., 0);
            self.live.strip[XB_COLS - 1] = 0;
            self.col_ms = 0;
        }
        self.live.strip[XB_COLS - 1] = (self.col_ms / 1000).clamp(-30, 30) as i8;
    }

    fn show(&mut self, now: u32) {
        let from = self.from();
        let x = &mut self.live;
        x.state = match self.st {
            St::Pass { from, .. } => from + 1,
            St::Hang { .. } => 3,
            St::Ready if self.timed_out.is_some() => 4,
            St::Ready => 0,
        };
        x.run_s = now.wrapping_sub(self.start_t) / 1000;
        if from.is_some() {
            x.pass_s = (now.wrapping_sub(self.pass_t) / 1000) as u16;
            x.idle_s = 0;
        } else {
            x.idle_s = self.end_t.map_or(x.run_s, |t| now.wrapping_sub(t) / 1000).min(u16::MAX as u32) as u16;
        }
        x.busy_s = (self.busy_ms / 1000).min(u16::MAX as u32) as u16;
        // XB-17: duty over the last 10 minutes (20 columns of 30 s).
        let busy: u32 = x.strip[XB_COLS - 20..].iter().map(|&s| s.unsigned_abs() as u32).sum();
        x.duty = (busy * 100 / 600).min(100) as u8;
        x.next_id_s = match self.last_id {
            _ if self.cfg.flags & XbCfg::ID == 0 => 0,
            None => 0,
            Some(t) => (ID_MS.saturating_sub(now.wrapping_sub(t)) / 1000) as u16,
        };
    }

    /// The simulators only: a past to show, as if the repeater had been running (the mock-ups' "1 h 12 m in").
    pub fn demo_past(&mut self, strip: [i8; XB_COLS], run_s: u32, passes: u16, busy_s: u16) {
        self.live.strip = strip;
        self.start_t = self.start_t.wrapping_sub(run_s * 1000);
        self.live.passes = passes;
        self.busy_ms = busy_s as u32 * 1000;
        self.last_id = Some(self.last_t.wrapping_sub(6 * 60_000));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;
    use std::vec::Vec;

    fn run(r: &mut Repeater, t: &mut u32, until: u32, heard: [Option<bool>; 2], out: &mut Vec<(u32, Cmd)>) {
        while *t < until {
            if let Some(c) = r.tick(*t, heard) {
                out.push((*t, c));
            }
            *t += 10;
        }
    }

    #[test]
    fn start_checks() {
        assert_eq!(sides_ok([mhz(146), mhz(446)], false), Ok(()));
        assert_eq!(sides_ok([mhz(146), mhz(223)], false), Err(Stop::OneChip));
        assert_eq!(sides_ok([mhz(28), mhz(446)], false), Err(Stop::Hf));
        assert_eq!(sides_ok([mhz(146), mhz(446)], true), Err(Stop::Relay));
    }

    #[test]
    fn a_pass_with_hang_and_id() {
        let cfg = XbCfg { hang_ds: 10, flags: XbCfg::TONE_GATE | XbCfg::ID, ..XbCfg::DEFAULT };
        let mut r = Repeater::new(cfg, 0);
        let (mut t, mut out) = (0, Vec::new());
        run(&mut r, &mut t, 1_000, [Some(false); 2], &mut out);
        assert!(out.is_empty());
        // Side 1 opens: a pass after 30 ms.
        run(&mut r, &mut t, 5_000, [Some(true), Some(false)], &mut out);
        assert_eq!(out, [(1_030, Cmd::Key { from: 0 })]);
        assert_eq!((r.live.state, r.live.passes, r.from()), (1, 1, Some(0)));
        // The other side is ignored while passing (XB-5).
        run(&mut r, &mut t, 6_000, [Some(true), Some(true)], &mut out);
        assert_eq!(out.len(), 1);
        // Closed: 100 ms, then 1 s hang, then the unkey with the first ID (XB-12).
        run(&mut r, &mut t, 6_500, [Some(false), None], &mut out);
        assert_eq!(r.live.state, 3, "hang");
        run(&mut r, &mut t, 8_000, [Some(false), None], &mut out);
        assert_eq!(out[1], (7_090, Cmd::Unkey { id: true }), "last open 5 990 + 100 ms + 1 s hang");
        // A reopening in the hang continues the same pass.
        let mut r2 = Repeater::new(cfg, 0);
        let (mut t2, mut o2) = (0, Vec::new());
        run(&mut r2, &mut t2, 1_000, [Some(true), Some(false)], &mut o2);
        run(&mut r2, &mut t2, 1_500, [Some(false), Some(false)], &mut o2);
        run(&mut r2, &mut t2, 2_000, [Some(true), Some(false)], &mut o2);
        assert_eq!(o2.len(), 1);
        assert_eq!(r2.live.passes, 1);
        // Side 1 again soon: no ID until 10 min have passed (XB-12).
        run(&mut r, &mut t, 9_000, [Some(false), Some(true)], &mut out);
        assert_eq!(out[2].1, Cmd::Key { from: 1 });
        assert_eq!(r.live.state, 2);
        run(&mut r, &mut t, 11_000, [Some(false), Some(false)], &mut out);
        assert_eq!(out[3].1, Cmd::Unkey { id: false });
        t = ID_MS + 20_000;
        run(&mut r, &mut t, ID_MS + 21_000, [Some(true), Some(false)], &mut out);
        run(&mut r, &mut t, ID_MS + 24_000, [Some(false), Some(false)], &mut out);
        assert_eq!(out.last().unwrap().1, Cmd::Unkey { id: true });
    }

    /// Decision 94: ID off (the default) never asks for CW, however many passes and minutes.
    #[test]
    fn id_off_never_ids() {
        assert_eq!(XbCfg::DEFAULT.flags & XbCfg::ID, 0, "off by default");
        let mut r = Repeater::new(XbCfg { hang_ds: 0, ..XbCfg::DEFAULT }, 0);
        let (mut t, mut out) = (0, Vec::new());
        for k in 0..4u32 {
            let start = k * (ID_MS + 30_000);
            t = t.max(start);
            run(&mut r, &mut t, start + 3_000, [Some(true), Some(false)], &mut out);
            run(&mut r, &mut t, start + 6_000, [Some(false), Some(false)], &mut out);
        }
        let unkeys: Vec<_> = out.iter().filter(|(_, c)| matches!(c, Cmd::Unkey { .. })).collect();
        assert_eq!(unkeys.len(), 4);
        assert!(unkeys.iter().all(|(_, c)| *c == Cmd::Unkey { id: false }), "{unkeys:?}");
    }

    #[test]
    fn timeout_locks_until_it_drops() {
        let cfg = XbCfg { timeout_10s: 3, ..XbCfg::DEFAULT };
        let mut r = Repeater::new(cfg, 0);
        let (mut t, mut out) = (0, Vec::new());
        run(&mut r, &mut t, 40_000, [Some(true), Some(false)], &mut out);
        assert_eq!(out[0].1, Cmd::Key { from: 0 });
        assert!(matches!(out[1], (_, Cmd::Unkey { .. })));
        assert_eq!(out.len(), 2, "a timed-out side doesn't pass again while it stays open (XB-9)");
        assert_eq!(r.live.state, 4);
        run(&mut r, &mut t, 40_100, [Some(false), Some(false)], &mut out);
        assert_eq!(r.live.state, 0);
        run(&mut r, &mut t, 40_200, [Some(true), Some(false)], &mut out);
        assert_eq!(out.len(), 3, "after it dropped, it passes again");
    }

    #[test]
    fn self_hold_and_refusals() {
        let mut r = Repeater::new(XbCfg::DEFAULT, 0);
        let (mut t, mut out) = (0, Vec::new());
        run(&mut r, &mut t, 40, [Some(true), Some(false)], &mut out);
        // Side 2 opens 10 ms after our key-up: self-hold, ignored until it closes (XB-11).
        run(&mut r, &mut t, 1_100, [Some(true), Some(true)], &mut out);
        run(&mut r, &mut t, 3_000, [Some(false), Some(true)], &mut out);
        assert_eq!(out.len(), 2, "the pass ended; side 2 is held: {out:?}");
        run(&mut r, &mut t, 3_100, [Some(false), Some(false)], &mut out);
        run(&mut r, &mut t, 3_500, [Some(false), Some(true)], &mut out);
        assert_eq!(out.last().unwrap().1, Cmd::Key { from: 1 }, "once it closed, a real signal passes");
        // A refused key-up locks that side until it drops; nothing is asked again meanwhile.
        r.refused(t);
        let n = out.len();
        run(&mut r, &mut t, 5_000, [Some(false), Some(true)], &mut out);
        assert_eq!(out.len(), n);
        assert!(r.live.reduced);
    }

    #[test]
    fn hold_for_local_ptt() {
        let mut r = Repeater::new(XbCfg::DEFAULT, 0);
        let (mut t, mut out) = (0, Vec::new());
        run(&mut r, &mut t, 500, [Some(true), Some(false)], &mut out);
        r.hold(true, t);
        assert_eq!(r.from(), None);
        run(&mut r, &mut t, 2_000, [Some(true), Some(true)], &mut out);
        assert_eq!(out.len(), 1, "held: no key-ups");
        r.hold(false, t);
        run(&mut r, &mut t, 2_100, [Some(true), Some(false)], &mut out);
        assert_eq!(out.last().unwrap().1, Cmd::Key { from: 0 }, "carries on after (XB-14)");
    }

    #[test]
    fn strip_and_duty() {
        let mut r = Repeater::new(XbCfg { hang_ds: 0, ..XbCfg::DEFAULT }, 0);
        let (mut t, mut out) = (0, Vec::new());
        run(&mut r, &mut t, 10_000, [Some(true), Some(false)], &mut out);
        run(&mut r, &mut t, 30_000, [Some(false), Some(false)], &mut out);
        run(&mut r, &mut t, 35_000, [Some(false), Some(true)], &mut out);
        assert_eq!(r.live.strip[XB_COLS - 2], 10, "side 1 to 2 about 10 s in the first column");
        assert!(r.live.strip[XB_COLS - 1] < 0, "side 2 to 1 below the line");
        assert!(r.live.duty > 0 && r.live.busy_s >= 13);
    }
}
