//! APRS mode (manual chapter 25, APRS-13 and APRS-15 to APRS-21): slot C taken for APRS. The slot's own VFO or
//! channel stays as it was in [`crate::Slot`]; while the mode is on, what goes to the Radio task for C is the
//! APRS frequency and settings instead ([`App::tuned_freq`], [`App::tuned_cfg`]), C is watched whatever Watch
//! says, and nothing on the radio can change C (APRS-19). Turning the mode off sends C's own state again.
//!
//! The APRS settings that aren't in [`crate::settings::Settings`] (the frequency, beacon mode and power) are
//! [`AprsCfg`], kept by the APRS store's owner ([`crate::data::Data::aprs_cfg`], [`crate::data::Op::AprsCfg`]);
//! the App holds a copy for the commands it sends, loaded at the first tick and written back when it changes.
//!
//! The message retry schedule (APRS-13) and the timed beacon are here too, for the APRS task and the
//! simulator's engine ([`crate::sample::Sample::aprs_tick`]) alike.

use crate::data::Data;
use crate::screens::lists::{age_text, Txt};
use crate::slot::SlotCfg;
use crate::App;
use ferrite_core::freq::{mhz, needs_relay, Freq10};
use ferrite_core::msg::RadioCmd;
use ferrite_core::txplan::Region;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// The slot APRS mode takes.
pub const APRS_SLOT: u8 = 2;

/// The APRS settings kept with the APRS store (0 5 5 1, 0 5 5 4, 0 5 7 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AprsCfg {
    /// The frequency in 5 kHz units; 0 = the region's (APRS-15).
    pub f5k: u16,
    /// Beacon mode: 0 off, 1 manual, 2.. every [`BEACON_MINS`]`[n - 2]` minutes. Bit 7: off, the mode kept for
    /// "beacon on" (the heard list's and the APRS card's options).
    pub beacon: u8,
    /// Beacon and message power: 0 low (APRS-20's default), 1 mid, 2 high.
    pub power: u8,
}

impl AprsCfg {
    pub const DEFAULT: AprsCfg = AprsCfg { f5k: 0, beacon: 0, power: 0 };

    /// The beacon mode, 0 when off.
    pub fn mode(&self) -> u8 {
        if self.beacon & BEACON_PAUSED != 0 { 0 } else { self.beacon & 0x0F }
    }

    /// Minutes between timed beacons; None when off or manual.
    pub fn every_min(&self) -> Option<u8> {
        BEACON_MINS.get((self.mode() as usize).wrapping_sub(2)).copied()
    }
}

/// Bit 7 of [`AprsCfg::beacon`].
const BEACON_PAUSED: u8 = 0x80;
/// Timed beacons (0 5 5 1). APRS-9's 30 s floor is well under the shortest.
pub const BEACON_MINS: [u8; 8] = [1, 2, 5, 10, 15, 20, 30, 60];
/// Beacon on from "beacon on/off" with no mode chosen yet: every 10 minutes.
const BEACON_ON: u8 = 5;
/// Beacon modes in the menu: off, manual, then the timed ones.
pub const BEACON_MODES: usize = 2 + BEACON_MINS.len();
pub const POWERS: [&str; 3] = ["Low", "Mid", "High"];

/// A message is tried this often (APRS-13), the waits between tries, and after the last try how long an ack
/// may still come before it is marked failed.
pub const TRIES: u8 = 5;
pub const RETRY_S: [u16; 4] = [30, 60, 120, 240];
pub const LAST_WAIT_S: u16 = 240;

/// After try `n` (1-5): seconds until the next try, or (after the fifth) until it is marked failed.
pub fn retry_wait(n: u8) -> u16 {
    RETRY_S.get((n as usize).wrapping_sub(1)).copied().unwrap_or(LAST_WAIT_S)
}

/// A timed beacon is due: never sent while on, or `every` minutes since the last.
pub fn beacon_due(c: &AprsCfg, last_s: Option<u32>, now_s: u32) -> bool {
    match (c.every_min(), last_s) {
        (Some(_), None) => true,
        (Some(m), Some(t)) => now_s.wrapping_sub(t) >= m as u32 * 60,
        (None, _) => false,
    }
}

/// The region's APRS frequency (APRS-15): US, Canada and Other 144.390, UK and Europe 144.800, Australia
/// 145.175, New Zealand 144.575, Japan 144.640 MHz. No region chosen yet: 144.390.
pub fn region_freq(r: Option<Region>) -> Freq10 {
    let k = match r {
        Some(Region::Uk | Region::Europe) => 800,
        Some(Region::Australia) => 1175,
        Some(Region::NewZealand) => 575,
        Some(Region::Japan) => 640,
        _ => 390,
    };
    mhz(144) + k * 100
}

/// The frequency's range on 0 5 7 1: chip 0's (APRS-21).
const F_MIN: Freq10 = mhz(64);
const F_MAX: Freq10 = mhz(300);

const LOADED: u8 = 1;
const DIRTY: u8 = 2;
const TOAST_HEAD: &[u8] = b"APRS mode:";
const TOAST_BODY: &[u8] = b"off at MENU 0 5 3";

impl App {
    /// APRS mode is on (0 5 3).
    pub fn aprs_on(&self) -> bool {
        self.set.aprs_mode
    }

    /// Slot `s` is the APRS slot now.
    pub fn aprs_slot(&self, s: u8) -> bool {
        s == APRS_SLOT && self.set.aprs_mode
    }

    pub fn aprs_cfg(&self) -> AprsCfg {
        self.aprs
    }

    /// The APRS frequency: 0 5 7 1, or the region's.
    pub fn aprs_freq(&self) -> Freq10 {
        match self.aprs.f5k {
            0 => region_freq(self.set.region()),
            k => k as Freq10 * 500,
        }
    }

    /// What the slot is tuned to: its own frequency, or the APRS frequency on the APRS slot.
    pub fn tuned_freq(&self, s: u8) -> Freq10 {
        if self.aprs_slot(s) { self.aprs_freq() } else { self.slot(s).freq() }
    }

    /// The slot's settings as the radio has them: on the APRS slot FM, no tones, simplex, busy lockout by
    /// carrier (APRS-10) and the beacon power (APRS-20); the squelch is C's own.
    pub fn tuned_cfg(&self, s: u8) -> SlotCfg {
        let own = *self.slot(s).cfg();
        if !self.aprs_slot(s) {
            return own;
        }
        SlotCfg { mode: 0, narrow: false, power: self.aprs.power % 3, busy: 1, squelch: own.squelch, ..SlotCfg::default() }
    }

    /// APRS mode on or off (0 5 3, APRS-15, APRS-16): C tunes to APRS and is watched, or goes back as it was.
    pub(crate) fn set_aprs_mode(&mut self, on: bool) {
        self.set.aprs_mode = on;
        if self.scan_mask & 1 << APRS_SLOT != 0 {
            self.scan_mask &= !(1 << APRS_SLOT);
            self.out.push(RadioCmd::Scan { slot: APRS_SLOT, on: false }); // C stops scanning to listen to APRS
        }
        self.send_slot(APRS_SLOT);
        self.out.push(RadioCmd::Watch(self.watch_mask()));
        if on {
            let mut f = Buf::<20>::new();
            let _ = uwrite!(f, "C on {}", Txt(crate::slot::mhz3(self.aprs_freq()).as_bytes()));
            self.show_toast(b"APRS mode on", f.as_bytes(), false);
        } else {
            self.show_toast(b"APRS mode off", b"C is back as it was", false);
        }
    }

    /// New APRS settings: kept, written back at the next tick, and C retuned when it is the APRS slot.
    pub(crate) fn set_aprs(&mut self, c: AprsCfg) {
        self.aprs = c;
        self.aprs_st |= DIRTY;
        if self.set.aprs_mode {
            self.send_slot(APRS_SLOT);
        }
    }

    /// 0 5 7 1 by the knob: 5 kHz steps from where it is, within chip 0's range.
    pub(crate) fn step_aprs_freq(&mut self, d: i32) {
        let f = (self.aprs_freq() as i64 + d as i64 * 500).clamp(F_MIN as i64, F_MAX as i64) as Freq10;
        let f5k = if f == region_freq(self.set.region()) { 0 } else { (f / 500) as u16 };
        self.set_aprs(AprsCfg { f5k, ..self.aprs });
    }

    /// 0 5 5 1 by the knob.
    pub(crate) fn step_beacon(&mut self, d: i32) {
        let m = (self.aprs.mode() as i32 + d).rem_euclid(BEACON_MODES as i32) as u8;
        self.set_aprs(AprsCfg { beacon: m, ..self.aprs });
    }

    /// "Beacon on/off" (APRS-17): off keeps the mode for turning it on again; on with none chosen is every 10 min.
    pub(crate) fn toggle_beacon(&mut self) {
        let b = self.aprs.beacon;
        let beacon = if self.aprs.mode() != 0 {
            b | BEACON_PAUSED
        } else if b & 0x0F != 0 {
            b & 0x0F
        } else {
            BEACON_ON
        };
        self.set_aprs(AprsCfg { beacon, ..self.aprs });
        let mut t = Buf::<24>::new();
        self.beacon_mode_text(&mut t);
        self.show_toast(b"Beacon", t.as_bytes(), false);
    }

    /// "Off", "Manual", "Every 10 min".
    pub(crate) fn beacon_mode_text<const N: usize>(&self, b: &mut Buf<N>) {
        b.clear();
        match (self.aprs.mode(), self.aprs.every_min()) {
            (0, _) => b.set(b"Off"),
            (_, Some(m)) => {
                let _ = uwrite!(b, "Every {} min", m);
            }
            _ => b.set(b"Manual"),
        }
    }

    /// APRS-19: the APRS slot can't be changed; the toast says how to get it back. True when refused.
    pub(crate) fn aprs_refuse(&mut self, s: u8) -> bool {
        if !self.aprs_slot(s) {
            return false;
        }
        self.show_toast(TOAST_HEAD, TOAST_BODY, false);
        true
    }

    /// Every tick: the APRS settings from their owner once, and written back after a change.
    pub(crate) fn aprs_tick(&mut self, data: &dyn Data) {
        if self.aprs_st & LOADED == 0 {
            self.aprs_st |= LOADED;
            let c = data.aprs_cfg();
            if c != self.aprs {
                self.aprs = c;
                if self.set.aprs_mode {
                    self.send_slot(APRS_SLOT);
                }
            }
        }
        if self.aprs_st & DIRTY != 0 {
            data.request(crate::data::Op::AprsCfg(self.aprs)); // NoData (the radio until the APRS store): kept in RAM
            self.aprs_st &= !DIRTY;
        }
    }

    /// The APRS card's beacon line (APRS-17, APRS-20): "Beacon off", "Beacon manual", "Next beacon in 4 min",
    /// "Beacon waiting: HF" while the active slot holds the relay in the HF state.
    pub(crate) fn beacon_text<const N: usize>(&self, data: &dyn Data, b: &mut Buf<N>) {
        b.clear();
        let st = data.beacon();
        if st.waiting && needs_relay(self.tuned_freq(self.active)) {
            b.set(b"Beacon waiting: HF");
        } else if st.waiting {
            b.set(b"Beacon waiting");
        } else {
            match (self.aprs.mode(), st.next_s) {
                (0, _) => b.set(b"Beacon off"),
                (1, _) => b.set(b"Beacon manual"),
                (_, Some(s)) if s < 60 => {
                    let _ = uwrite!(b, "Next beacon in {} s", s);
                }
                (_, Some(s)) => {
                    let _ = uwrite!(b, "Next beacon in {} min", s.div_ceil(60));
                }
                (_, None) => b.set(b"Beacon soon"),
            }
        }
    }
}

/// The card's "last" line: the newest of the last station heard and the last message in (APRS-17).
pub(crate) fn last_text<const N: usize>(data: &dyn Data, b: &mut Buf<N>) {
    b.clear();
    let st = data.station(0);
    let msg = (0..data.messages().min(8)).filter_map(|i| data.message(i)).find(|m| m.inbound);
    match (st, msg) {
        (_, Some(m)) if st.is_none_or(|s| m.ago_s <= s.ago_s) => {
            let _ = uwrite!(b, "{}: {}", Txt(m.peer.as_bytes()), Txt(m.text.as_bytes()));
        }
        (Some(s), _) => {
            let mut a = Buf::<12>::new();
            age_text(s.ago_s, &mut a);
            let _ = uwrite!(b, "{} \u{b7} {} ago", Txt(s.call.as_bytes()), Txt(a.as_bytes()));
        }
        _ => b.set(b"Nothing heard yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_frequencies() {
        assert_eq!(region_freq(Some(Region::Us)), 14_439_000);
        assert_eq!(region_freq(Some(Region::Canada)), 14_439_000);
        assert_eq!(region_freq(Some(Region::Other)), 14_439_000);
        assert_eq!(region_freq(None), 14_439_000);
        assert_eq!(region_freq(Some(Region::Uk)), 14_480_000);
        assert_eq!(region_freq(Some(Region::Europe)), 14_480_000);
        assert_eq!(region_freq(Some(Region::Australia)), 14_517_500);
        assert_eq!(region_freq(Some(Region::NewZealand)), 14_457_500);
        assert_eq!(region_freq(Some(Region::Japan)), 14_464_000);
    }

    #[test]
    fn retry_schedule() {
        let waits: [u16; 5] = core::array::from_fn(|i| retry_wait(i as u8 + 1));
        assert_eq!(waits, [30, 60, 120, 240, LAST_WAIT_S]);
        assert_eq!(TRIES, 5);
    }

    #[test]
    fn beacon_modes() {
        let mut c = AprsCfg::DEFAULT;
        assert_eq!((c.mode(), c.every_min()), (0, None));
        assert!(!beacon_due(&c, None, 0));
        c.beacon = 1;
        assert_eq!((c.mode(), c.every_min()), (1, None));
        assert!(!beacon_due(&c, None, 0));
        c.beacon = 2 + 3; // every 10 min
        assert_eq!(c.every_min(), Some(10));
        assert!(beacon_due(&c, None, 5));
        assert!(!beacon_due(&c, Some(100), 699));
        assert!(beacon_due(&c, Some(100), 700));
        c.beacon |= BEACON_PAUSED;
        assert_eq!(c.mode(), 0);
        assert!(!beacon_due(&c, Some(0), 10_000));
    }
}
