//! The transmit sequencer (TX-17): every transmission, from PTT to a KISS frame, goes through [`TxSeq::key`]
//! and comes out of [`TxSeq::release`] or [`TxSeq::tick`], with the same checks and the same orders. It keeps
//! the timeout (TX-30, with PTT released before the next key-up), the duty limit (TX-31), the battery cut-off
//! under load (TX-32) and the CW ID's ten minutes (VOX-12); the pins are the path machine's ([`Path`]).
//!
//! In the firmware it is built only with the `tx` feature; the UI makes the same checks first, so a refusal
//! from here (calibration, read-back) is rare.

use crate::freq::Freq10;
use crate::path::{Act, Blocked, Path, Sel, Steps};
use crate::txplan::{self, Refusal, Region};

/// What is being sent: the end tone follows voice only (TONE-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// PTT, VOX, remote KEY.
    Voice,
    /// APRS, KISS, packet frames, a tone burst on its own, DTMF dialling.
    Data,
}

/// A key-up request: the slot's frequencies and what the Radio task knows about it.
#[derive(Clone, Copy, Debug)]
pub struct Req {
    pub tx: Freq10,
    /// The slot's RX frequency (TX-37's pair).
    pub rx: Freq10,
    pub kind: Kind,
    /// A person pressed PTT (not a beacon, KISS, VOX or remote KEY).
    pub person: bool,
    /// The emergency prompt's second PTT was given (TX-3).
    pub confirmed: bool,
    /// The slot is in AM.
    pub am: bool,
    pub region: Region,
    pub extend: bool,
    /// The calibration sector and the MCU check passed (TX-13 to TX-15), and calibration covers `tx`.
    pub cal_ok: bool,
    /// The battery is above the cut-off.
    pub batt_ok: bool,
    /// The power level: 0 Low, 1 Mid, 2 High; it sets how fast the duty bucket fills (TX-38).
    pub power: u8,
}

/// The sequencer's settings, from the slot and the Transmit group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cfg {
    /// TX-30: 30-600 s.
    pub timeout_s: u16,
    /// The pre-alert this long before the timeout (0 = none).
    pub prealert_s: u8,
    pub end_tone: bool,
    /// CW ID every 10 min (VOX-12).
    pub cw_id: bool,
    /// CW ID at every unkey instead (0 8 7 2's "on unkey"; VOX-12).
    pub cw_each: bool,
    /// Tail elimination: a DCS turn-off code or the 55 Hz phase (TONE-9).
    pub tail: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { timeout_s: 180, prealert_s: 10, end_tone: false, cw_id: false, cw_each: false, tail: false }
    }
}

/// VOX-12: the ID goes at the next unkey once this long has passed since the last one.
pub const CW_ID_MS: u32 = 10 * 60_000;
/// TX-31's duty limit (TX-38), a leaky bucket (to confirm on the bench): keyed time fills it, weighted by the power
/// level (High heats the PA most), idle time drains it at a quarter of High's rate, and a full bucket unkeys and
/// refuses until it is half empty.
pub const DUTY_FULL_MS: u32 = 120_000;
pub const DUTY_DRAIN_DIV: u32 = 4;
/// How fast keyed time fills the bucket per power level, in quarters: Low 1/4, Mid 1/2, High 1 (decided Oct 7,
/// 2026; weights to confirm on the bench). Continuous transmit from cold: High 2 min, Mid 4 min, Low 8 min.
pub const DUTY_WEIGHT: [u32; 3] = [1, 2, 4];

/// Why the sequencer unkeyed by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The timeout's pre-alert (TX-30).
    PreAlert,
    Timeout,
    /// The duty limit (TX-31).
    Hot,
    /// The battery fell below the under-load threshold (TX-32).
    Battery,
}

/// The sequencer (28 bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct TxSeq {
    /// Keyed since (ms), and what.
    since: Option<(u32, Kind)>,
    alerted: bool,
    /// Timed out: PTT must be released before the next key-up.
    held: bool,
    /// The last CW ID (ms).
    last_id: Option<u32>,
    duty: u32,
    duty_at: u32,
    hot: bool,
    /// The keyed power level's fill weight (DUTY_WEIGHT).
    weight: u8,
}

impl TxSeq {
    pub const fn new() -> Self {
        TxSeq { since: None, alerted: false, held: false, last_id: None, duty: 0, duty_at: 0, hot: false, weight: 4 }
    }

    pub fn keyed(&self) -> bool {
        self.since.is_some()
    }

    /// The checks, in TX-20's order, as the Radio task applies them (TX-17).
    pub fn check(&mut self, r: &Req, path: &Path, now: u32) -> Result<(), Refusal> {
        self.heat(now);
        if self.since.is_some() || path.keyed().is_some() {
            return Err(Refusal::InProgress);
        }
        if self.held {
            return Err(Refusal::ReleasePtt);
        }
        if r.am {
            return Err(Refusal::AmRxOnly);
        }
        txplan::check(r.tx, r.region, r.extend)?;
        if !r.cal_ok {
            return Err(Refusal::Locked);
        }
        if txplan::locked_125m(r.tx) {
            return Err(Refusal::Unmeasured125);
        }
        if !txplan::relay_ok(r.rx, r.tx) {
            return Err(Refusal::RelayMismatch);
        }
        if txplan::emergency(r.tx) && !(r.person && r.confirmed) {
            return Err(Refusal::Emergency);
        }
        if !r.batt_ok {
            return Err(Refusal::Battery);
        }
        if self.hot {
            return Err(Refusal::Hot);
        }
        Ok(())
    }

    /// Key up: the checks, then the path machine's key-up into `out`.
    pub fn key(&mut self, r: &Req, path: &mut Path, now: u32, out: &mut Steps) -> Result<(), Refusal> {
        self.check(r, path, now)?;
        path.key_up(Sel::tx(r.tx), now, out).map_err(|b| match b {
            Blocked::Keyed => Refusal::InProgress,
            Blocked::NeedsRelay | Blocked::TooOften => Refusal::RelayMismatch,
        })?;
        self.since = Some((now, r.kind));
        self.weight = DUTY_WEIGHT[r.power.min(2) as usize] as u8;
        self.alerted = false;
        Ok(())
    }

    /// The crossband repeater's key-up (XB-6, XB-10): the same checks (region, calibration, relay, battery, the duty
    /// limit weighted by power), then the path machine's crossband key-up, the receiving chip left in receive.
    /// Send it as voice that no person keyed (`person: false`): it relays voice, gets the CW ID when the
    /// repeater asks for it, and is refused on an emergency frequency.
    pub fn key_xb(&mut self, r: &Req, path: &mut Path, now: u32, out: &mut Steps) -> Result<(), Refusal> {
        self.check(r, path, now)?;
        path.key_up_xb(Sel::tx(r.tx), out).map_err(|b| match b {
            Blocked::Keyed => Refusal::InProgress,
            Blocked::NeedsRelay | Blocked::TooOften => Refusal::RelayMismatch,
        })?;
        self.since = Some((now, r.kind));
        self.weight = DUTY_WEIGHT[r.power.min(2) as usize] as u8;
        self.alerted = false;
        Ok(())
    }

    /// Every PTT input released (TX-25): unkey if keyed, and a timed-out PTT may key again.
    pub fn release(&mut self, cfg: &Cfg, path: &mut Path, now: u32, out: &mut Steps) {
        self.held = false;
        self.unkey(cfg, true, path, now, out);
    }

    /// Unkey (TX-26, TONE-8): end tone (voice, unless cut short) → CW ID when due (voice only; never at a cut)
    /// → tail elimination → the path machine's unkey.
    fn unkey(&mut self, cfg: &Cfg, normal: bool, path: &mut Path, now: u32, out: &mut Steps) {
        self.heat(now);
        let Some((_, kind)) = self.since.take() else { return };
        let voice = kind == Kind::Voice && normal;
        // Crossband (decision 95): stock's RX end first, while still keyed: the mic back and the receive select
        // off, then the roger and tail, then the carrier held (path::XB_HOLD_MS) before the PA goes to 0.
        let xb = path.keyed_xb();
        if xb {
            path.xb_rx_end(out);
        }
        if voice && cfg.end_tone {
            out.push(Act::EndTone);
        }
        if voice && cfg.cw_id && (cfg.cw_each || self.last_id.is_none_or(|t| now.wrapping_sub(t) >= CW_ID_MS)) {
            out.push(Act::CwId);
            self.last_id = Some(now);
        }
        if cfg.tail {
            out.push(Act::TailElim); // inside stock's 400 ms hold (tonegen::TAIL_MS)
        } else if xb && normal {
            for _ in 0..crate::path::XB_HOLD_MS / 200 {
                out.push(Act::Wait(200));
            }
        }
        path.unkey(now, out);
    }

    /// The safe state from anything (a fault, power off, a failed read-back): unkeyed at once, no tones.
    pub fn safe(&mut self, path: &mut Path, now: u32, out: &mut Steps) {
        self.heat(now);
        self.since = None;
        path.safe(now, out);
    }

    /// The duty bucket up to `now`.
    fn heat(&mut self, now: u32) {
        let dt = now.wrapping_sub(self.duty_at);
        self.duty_at = now;
        self.duty = if self.since.is_some() {
            self.duty.saturating_add(dt * self.weight as u32 / 4)
        } else {
            self.duty.saturating_sub(dt / DUTY_DRAIN_DIV)
        };
        if self.duty >= DUTY_FULL_MS {
            self.hot = true;
        } else if self.duty <= DUTY_FULL_MS / 2 {
            self.hot = false;
        }
    }

    /// Time passing while keyed: the pre-alert, then the timeout (PTT must be released after), the duty limit and
    /// the battery under load each unkey without the end tone (TONE-11).
    pub fn tick(&mut self, cfg: &Cfg, batt_ok: bool, path: &mut Path, now: u32, out: &mut Steps) -> Option<Event> {
        self.heat(now);
        let (t0, _) = self.since?;
        let on = now.wrapping_sub(t0);
        let limit = cfg.timeout_s.clamp(30, 600) as u32 * 1000;
        let ev = if on >= limit {
            self.held = true;
            Event::Timeout
        } else if self.hot {
            Event::Hot
        } else if !batt_ok {
            Event::Battery
        } else {
            if !self.alerted && cfg.prealert_s > 0 && on + cfg.prealert_s as u32 * 1000 >= limit {
                self.alerted = true;
                return Some(Event::PreAlert);
            }
            return None;
        };
        self.unkey(cfg, false, path, now, out);
        Some(ev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;
    use crate::path::{Cause, Rx};

    fn req(tx: Freq10) -> Req {
        Req { tx, rx: tx, kind: Kind::Voice, person: true, confirmed: false, am: false, region: Region::Us, extend: false, cal_ok: true, batt_ok: true, power: 2 }
    }

    fn rx_on(f: Freq10) -> Path {
        let mut p = Path::new();
        p.rx(Rx::Bk(Sel::rx(f)), Cause::ActiveSlot, 0, &mut Steps::default()).unwrap();
        p
    }

    #[test]
    fn refusals() {
        let mut t = TxSeq::new();
        let p = rx_on(mhz(146));
        let ok = req(mhz(146) + 52_000);
        assert_eq!(t.check(&ok, &p, 0), Ok(()));
        assert_eq!(t.check(&Req { am: true, ..ok }, &p, 0), Err(Refusal::AmRxOnly));
        assert_eq!(t.check(&Req { cal_ok: false, ..ok }, &p, 0), Err(Refusal::Locked));
        assert_eq!(t.check(&Req { batt_ok: false, ..ok }, &p, 0), Err(Refusal::Battery));
        assert_eq!(t.check(&req(mhz(162) + 55_000), &p, 0), Err(Refusal::OutOfBand));
        assert_eq!(t.check(&Req { extend: true, ..req(mhz(162) + 55_000) }, &p, 0), Ok(()));
        assert_eq!(t.check(&req(mhz(223) + 50_000), &p, 0), Err(Refusal::Unmeasured125));
        assert_eq!(t.check(&Req { rx: mhz(28) + 40_000, ..ok }, &p, 0), Err(Refusal::RelayMismatch));
        let sos = Req { extend: true, ..req(mhz(121) + 50_000) };
        assert_eq!(t.check(&sos, &p, 0), Err(Refusal::NotCalibrated), "airband is outside the TX hardware");
        let sos = Req { extend: true, ..req(mhz(243)) };
        assert_eq!(t.check(&sos, &p, 0), Err(Refusal::Emergency));
        assert_eq!(t.check(&Req { confirmed: true, ..sos }, &p, 0), Ok(()));
        assert_eq!(t.check(&Req { confirmed: true, person: false, ..sos }, &p, 0), Err(Refusal::Emergency), "TX-35");
    }

    #[test]
    fn unkey_order_and_cw_id() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(146));
        let cfg = Cfg { end_tone: true, cw_id: true, tail: true, ..Cfg::default() };
        let r = req(mhz(146) + 52_000);
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.key(&r, &mut p, 0, &mut Steps::default()), Err(Refusal::InProgress));
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 5_000, &mut s);
        assert_eq!(s.as_slice()[..4], [Act::EndTone, Act::CwId, Act::TailElim, Act::PaZero]);
        // Again within ten minutes: no ID.
        t.key(&r, &mut p, 60_000, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 65_000, &mut s);
        assert_eq!(s.as_slice()[..3], [Act::EndTone, Act::TailElim, Act::PaZero]);
        // "On unkey": the ID every time.
        let each = Cfg { cw_each: true, ..cfg };
        t.key(&r, &mut p, 70_000, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        t.release(&each, &mut p, 75_000, &mut s);
        assert_eq!(s.as_slice()[..3], [Act::EndTone, Act::CwId, Act::TailElim]);
        // Ten minutes after the last ID, data: no end tone, no ID.
        t.key(&Req { kind: Kind::Data, ..r }, &mut p, 700_000, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 700_500, &mut s);
        assert_eq!(s.as_slice()[..2], [Act::TailElim, Act::PaZero]);
        // Not keyed: release does nothing.
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 701_000, &mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn timeout_then_release_before_rekey() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(446));
        let cfg = Cfg { timeout_s: 30, prealert_s: 5, end_tone: true, ..Cfg::default() };
        let r = req(mhz(446));
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&cfg, true, &mut p, 20_000, &mut Steps::default()), None);
        assert_eq!(t.tick(&cfg, true, &mut p, 25_000, &mut Steps::default()), Some(Event::PreAlert));
        assert_eq!(t.tick(&cfg, true, &mut p, 26_000, &mut Steps::default()), None);
        let mut s = Steps::default();
        assert_eq!(t.tick(&cfg, true, &mut p, 30_000, &mut s), Some(Event::Timeout));
        assert_eq!(s.as_slice()[0], Act::PaZero, "no end tone at a timeout (TONE-11)");
        assert!(!t.keyed() && p.keyed().is_none());
        assert_eq!(t.key(&r, &mut p, 31_000, &mut Steps::default()), Err(Refusal::ReleasePtt));
        t.release(&cfg, &mut p, 32_000, &mut Steps::default());
        assert_eq!(t.key(&r, &mut p, 33_000, &mut Steps::default()), Ok(()));
    }

    #[test]
    fn battery_under_load() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(446));
        t.key(&req(mhz(446)), &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&Cfg::default(), false, &mut p, 1_000, &mut Steps::default()), Some(Event::Battery));
        assert!(!t.keyed());
    }

    #[test]
    fn duty_limit() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(446));
        let cfg = Cfg { timeout_s: 600, prealert_s: 0, ..Cfg::default() };
        let r = req(mhz(446));
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&cfg, true, &mut p, DUTY_FULL_MS - 1, &mut Steps::default()), None);
        assert_eq!(t.tick(&cfg, true, &mut p, DUTY_FULL_MS, &mut Steps::default()), Some(Event::Hot));
        assert_eq!(t.key(&r, &mut p, DUTY_FULL_MS + 1000, &mut Steps::default()), Err(Refusal::Hot));
        // Half empty: 60 s drained at a quarter rate is 240 s.
        let cool = DUTY_FULL_MS + DUTY_FULL_MS / 2 * DUTY_DRAIN_DIV;
        assert_eq!(t.key(&r, &mut p, cool, &mut Steps::default()), Ok(()));
    }

    /// TX-38: the power level weights the duty bucket: Low lasts four times as long as High from cold.
    #[test]
    fn duty_by_power() {
        let cfg = Cfg { timeout_s: 600, prealert_s: 0, ..Cfg::default() };
        for (power, full) in [(2u8, DUTY_FULL_MS), (1, DUTY_FULL_MS * 2), (0, DUTY_FULL_MS * 4)] {
            let mut t = TxSeq::new();
            let mut p = rx_on(mhz(446));
            let r = Req { power, ..req(mhz(446)) };
            t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
            assert_eq!(t.tick(&cfg, true, &mut p, full - 4, &mut Steps::default()), None, "power {power}");
            assert_eq!(t.tick(&cfg, true, &mut p, full, &mut Steps::default()), Some(Event::Hot), "power {power}");
        }
    }
}
