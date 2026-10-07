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
    /// APRS, KISS, packet frames, a tone burst on its own, DTMF dialling, the crossband repeater.
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
    /// Tail elimination: a DCS turn-off code or the 55 Hz phase (TONE-9).
    pub tail: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { timeout_s: 180, prealert_s: 10, end_tone: false, cw_id: false, tail: false }
    }
}

/// VOX-12: the ID goes at the next unkey once this long has passed since the last one.
pub const CW_ID_MS: u32 = 10 * 60_000;
/// TX-31's duty limit (TX-38), a leaky bucket (to confirm on the bench): keyed time fills it, idle time drains it at a
/// quarter of the rate, and a full bucket unkeys and refuses until it is half empty.
pub const DUTY_FULL_MS: u32 = 120_000;
pub const DUTY_DRAIN_DIV: u32 = 4;

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
}

impl TxSeq {
    pub const fn new() -> Self {
        TxSeq { since: None, alerted: false, held: false, last_id: None, duty: 0, duty_at: 0, hot: false }
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
        if voice && cfg.end_tone {
            out.push(Act::EndTone);
        }
        if voice && cfg.cw_id && self.last_id.is_none_or(|t| now.wrapping_sub(t) >= CW_ID_MS) {
            out.push(Act::CwId);
            self.last_id = Some(now);
        }
        if cfg.tail {
            out.push(Act::TailElim);
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
        self.duty = if self.since.is_some() { self.duty.saturating_add(dt) } else { self.duty.saturating_sub(dt / DUTY_DRAIN_DIV) };
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
        Req { tx, rx: tx, kind: Kind::Voice, person: true, confirmed: false, am: false, region: Region::Us, extend: false, cal_ok: true, batt_ok: true }
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
}
