//! The battery percentage (PWR-22): this radio's calibrated gauge (0xF200, [`Gauge`], PWR-20) as the
//! knots of a 2S Li-ion curve, read through a few seconds' average that holds while keyed and briefly after (the
//! pack sags under the PA) and rises only on a sustained increase (charging), so the number never jumps about.
//!
//! The six thresholds (t0 the transmit cut-off .. t5 stock's full icon) keep the percentages a 2S Li-ion pack has
//! at stock's default voltages (6.2, 6.5, 6.8, 7.1, 7.3, 7.7 V), and 8.1 V is 100 % (a full pack at rest under the
//! receiver's load: the charger stops at 8.4 V, but the pack settles to about 8.14 V within minutes); between knots
//! the percentage is linear. So a radio whose calibration moved its thresholds moves its curve with them, about
//! 7.4 V reads half, and the icon's steps (stock's bars, [`bars`]) fall exactly on the thresholds.

use crate::txcal::Gauge;

/// PWR-22.4: charging is judged on one-minute means of the average over this many minutes.
pub const CHARGE_WINDOW_MIN: usize = 8;
/// PWR-22.4: the window's rise, in PA1 counts (about 3 mV each), that says charging: black on its charger climbed
/// about 1 count a minute with ±2 of noise (Nov 13, 2026); each half of the window must rise too, so one step up
/// (the screen going dark, a squelch closing) isn't charging.
pub const CHARGE_RISE: u32 = 6;
/// PWR-22.4: each half of the window rises at least this much.
pub const CHARGE_HALF_RISE: u32 = 2;
/// PWR-22.4: charging ends when the minute's mean falls this far below its highest since charging began (the
/// charger taken off), or when it hasn't risen for [`CHARGE_FLAT_MIN`] minutes (full).
pub const CHARGE_DROP: u32 = 6;
pub const CHARGE_FLAT_MIN: u32 = 15;

/// The percentage at t0..t5, then at [`FULL_MV`].
pub const KNOT_PCT: [u8; 7] = [0, 5, 15, 30, 45, 65, 100];
/// A full 2S Li-ion pack at rest under the receiver's load, mV (Alex measured 8.14 V on a full charge, Oct 16, 2026;
/// decision 199). The charger's 8.4 V cut-off isn't where a full pack sits, so it would never read 100.
pub const FULL_MV: u32 = 8100;
/// The reading for [`FULL_MV`] on the 12-bit PA1 scale (48.2 mV per step of the reading >> 4).
const FULL_RAW: u32 = FULL_MV * 160 / 482;
/// PWR-22.2: the 65 % knot (t5) sits at least this far below full: some calibrations (both our radios' factory
/// template, 0xAA: 8.19 V) put stock's full-icon threshold above where a full pack rests, which never read full.
pub const T5_BELOW_FULL_MV: u32 = 150;
/// A new reading is taken at most this often (the Radio task reads PA1 every 2 s).
pub const SAMPLE_MS: u32 = 2_000;
/// Held this long after an unkey: the pack recovers from the transmit sag.
pub const HOLD_MS: u32 = 10_000;
/// A higher reading shows only once it has lasted this long (charging, not a recovery blip).
pub const RISE_MS: u32 = 60_000;
/// PWR-22.3: for this long after the first reading, the shown level follows the average both ways: the first reading
/// is taken under the power-up load (the screen, both chips starting, the boot sound's amplifier) and read low, and
/// [`RISE_MS`] then held it there for a minute (black, Oct 28, 2026: red at power-up at 51 %).
pub const SETTLE_MS: u32 = 15_000;

/// The percentage for a 12-bit PA1 reading on this gauge: 0 at or below the cut-off, 100 at 8.1 V and over.
pub fn percent(g: &Gauge, raw: u16) -> u8 {
    let raw = raw as u32;
    let mut x = [0u32; 7];
    for (i, t) in g.0[..6].iter().enumerate() {
        x[i] = (*t as u32) << 4;
    }
    x[5] = x[5].min((FULL_MV - T5_BELOW_FULL_MV) * 160 / 482);
    x[6] = FULL_RAW.max(x[5] + 16);
    if raw <= x[0] {
        return 0;
    }
    for i in 0..6 {
        if raw < x[i + 1] {
            let (p0, p1) = (KNOT_PCT[i] as u32, KNOT_PCT[i + 1] as u32);
            // a calibration with thresholds out of order gives a flat step, never a division by zero
            let span = x[i + 1].saturating_sub(x[i]).max(1);
            return (p0 + raw.saturating_sub(x[i]) * (p1 - p0) / span) as u8;
        }
    }
    100
}

/// The status bar icon's fill, 1-4 (SBAR-9): stock's bars, one step less at each threshold from t5 down to t2.
/// The percentages at the thresholds are the knots, so the icon steps where stock's does.
pub fn bars(pct: u8) -> u8 {
    match pct {
        p if p >= KNOT_PCT[5] => 4,
        p if p >= KNOT_PCT[4] => 3,
        p if p >= KNOT_PCT[3] => 2,
        _ => 1,
    }
}

/// SBAR-9.1, DISP-21: the battery's colour by its level, for the bar's icon and the LED heartbeat alike: 0 green
/// above 80 %, 1 yellow (amber) down to 31 %, 2 red at 30 % and below, or at the low warning or cut-off whatever
/// the percentage says (Alex, Oct 22, 2026; decision 239).
pub fn tier(pct: u8, low: bool) -> u8 {
    match pct {
        _ if low => 2,
        p if p > 80 => 0,
        p if p > 30 => 1,
        _ => 2,
    }
}

/// What the battery shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The reading shown (12-bit PA1 scale, averaged).
    pub raw: u16,
    pub pct: u8,
    /// Below the second threshold: the warning (PWR-20).
    pub low: bool,
    /// Below the first: the transmit cut-off (PWR-20).
    pub cutoff: bool,
    /// PWR-22.4: the battery is charging, judged from a sustained rise (no charger input is known on this board).
    pub charging: bool,
}

/// PWR-22.4: charging from the averaged reading's one-minute means: a steady rise over [`CHARGE_WINDOW_MIN`]
/// minutes starts it; a fall from its peak or [`CHARGE_FLAT_MIN`] minutes without a new one ends it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Charge {
    /// The minute being summed (x16 readings) and how many readings so far.
    sum: u32,
    n: u32,
    started: Option<u32>,
    /// The last minutes' means (x16), oldest first; `len` of them filled.
    means: [u32; CHARGE_WINDOW_MIN + 1],
    len: usize,
    pub charging: bool,
    /// While charging: the highest mean and the minute count since it was reached.
    peak: u32,
    flat: u32,
}

impl Charge {
    /// A transmission: its sag and recovery would read as a rise, so the window starts again and the peak is
    /// taken afresh.
    pub fn keyed(&mut self) {
        (self.sum, self.n, self.started, self.len) = (0, 0, None, 0);
        self.peak = 0;
    }

    /// One averaged reading (x16) at `now` ms.
    pub fn sample(&mut self, avg16: u32, now: u32) {
        let start = *self.started.get_or_insert(now);
        self.sum += avg16;
        self.n += 1;
        if now.wrapping_sub(start) < 60_000 {
            return;
        }
        let mean = self.sum / self.n;
        (self.sum, self.n, self.started) = (0, 0, Some(now));
        self.minute(mean);
    }

    fn minute(&mut self, mean: u32) {
        if self.len == self.means.len() {
            self.means.copy_within(1.., 0);
            self.len -= 1;
        }
        self.means[self.len] = mean;
        self.len += 1;
        if self.charging {
            if mean > self.peak {
                (self.peak, self.flat) = (mean, 0);
            } else {
                self.flat += 1;
            }
            if self.peak != 0 && (mean + CHARGE_DROP * 16 <= self.peak || self.flat >= CHARGE_FLAT_MIN) {
                self.charging = false;
                self.len = 0; // a fresh window before it can start again
            } else if self.peak == 0 {
                self.peak = mean; // after a transmission
            }
            return;
        }
        if self.len == self.means.len() {
            let (a, m, b) = (self.means[0], self.means[CHARGE_WINDOW_MIN / 2], self.means[CHARGE_WINDOW_MIN]);
            if b >= a + CHARGE_RISE * 16 && m >= a + CHARGE_HALF_RISE * 16 && b >= m + CHARGE_HALF_RISE * 16 {
                (self.charging, self.peak, self.flat) = (true, b, 0);
            }
        }
    }
}

/// The average and what it shows; fed every UI pass with the latest PA1 reading.
#[derive(Clone, Copy, Debug, Default)]
pub struct Battery {
    /// The average, x16 (fractions kept), None before the first reading.
    avg: Option<u32>,
    shown: Option<u16>,
    last: u32,
    /// Readings wait until this time (keyed, then [`HOLD_MS`]).
    hold_until: Option<u32>,
    /// Since when the average has been above what shows.
    rise_since: Option<u32>,
    /// When the first reading came (PWR-22.3's settling window).
    first: Option<u32>,
    charge: Charge,
}

impl Battery {
    /// Takes the reading `raw` (0 = none yet) at `now` ms; `keyed` while any slot transmits.
    pub fn update(&mut self, g: &Gauge, raw: u16, now: u32, keyed: bool) -> Option<Shown> {
        if keyed {
            self.hold_until = Some(now.wrapping_add(HOLD_MS));
            self.rise_since = None;
            self.charge.keyed();
        }
        let held = self.hold_until.is_some_and(|t| (t.wrapping_sub(now) as i32) > 0);
        if !held {
            self.hold_until = None;
        }
        if raw != 0 && !held && (self.avg.is_none() || now.wrapping_sub(self.last) >= SAMPLE_MS) {
            self.last = now;
            let first = *self.first.get_or_insert(now);
            let settling = now.wrapping_sub(first) < SETTLE_MS;
            let r = (raw as u32) << 4;
            // a few seconds' average: each reading moves it a third of the way (about 5 s at one per 2 s)
            let a = self.avg.map_or(r, |a| (2 * a + r) / 3);
            self.avg = Some(a);
            if !settling {
                self.charge.sample(a, now); // PWR-22.4
            }
            let a = ((a + 8) >> 4) as u16;
            match self.shown {
                // falls with the average; rises only once it has stayed higher for RISE_MS
                Some(s) if a > s && !settling => {
                    let since = *self.rise_since.get_or_insert(now);
                    if now.wrapping_sub(since) >= RISE_MS {
                        self.shown = Some(a);
                        self.rise_since = None;
                    }
                }
                _ => {
                    self.shown = Some(a);
                    self.rise_since = None;
                }
            }
        }
        let raw = self.shown?;
        let v = raw >> 4;
        Some(Shown { raw, pct: percent(g, raw), low: v < g.0[1] as u16, cutoff: v < g.0[0] as u16, charging: self.charge.charging })
    }
}

#[cfg(test)]
mod tests {

    /// SBAR-9.1: green above 80 %, amber down to 31 %, red at 30 % and below, and red at the low warning or
    /// cut-off whatever the percentage.
    #[test]
    fn colour_tiers() {
        assert_eq!([100, 81, 80, 31, 30, 0].map(|p| tier(p, false)), [0, 0, 1, 1, 2, 2]);
        assert_eq!(tier(90, true), 2);
    }
    use super::*;
    extern crate std;
    use std::{vec, vec::Vec};

    /// Black on its charger, Nov 13, 2026: the averaged PA1 reading's one-minute means from the cable log
    /// (bench-results/charging-nov13), the charger put on just before the first.
    const ON_CHARGER: [u32; 32] = [
        2600, 2610, 2613, 2614, 2613, 2632, 2632, 2633, 2634, 2636, 2637, 2638, 2640, 2641, 2642, 2643, 2645, 2646,
        2646, 2649, 2649, 2650, 2651, 2653, 2654, 2655, 2657, 2657, 2658, 2660, 2660, 2662,
    ];

    fn minutes(c: &mut Charge, means: &[u32]) -> Vec<bool> {
        means.iter().map(|m| {
            c.minute(m * 16);
            c.charging
        }).collect()
    }

    /// PWR-22.4: the real charge reads as charging within the window (9 minutes' means) and stays so.
    #[test]
    fn charging_seen_on_the_real_curve() {
        let mut c = Charge::default();
        let seen = minutes(&mut c, &ON_CHARGER);
        let first = seen.iter().position(|&x| x).expect("never seen charging");
        assert_eq!(first, CHARGE_WINDOW_MIN, "as soon as the window is full");
        assert!(seen[first..].iter().all(|&x| x), "stays charging while it climbs");
    }

    /// PWR-22.4: not charging: a slow discharge, flat with noise, and one step up (a load ending) then flat.
    #[test]
    fn no_charging_without_a_steady_rise() {
        let falling: Vec<u32> = (0..40).map(|i| 2600 - i / 3).collect();
        let noisy: Vec<u32> = (0..40).map(|i| 2600 + [0, 2, 1, 3, 0, 2][i % 6]).collect();
        let step: Vec<u32> = (0..40).map(|i| if i < 12 { 2590 } else { 2605 }).collect();
        for m in [falling, noisy, step] {
            let mut c = Charge::default();
            assert!(minutes(&mut c, &m).iter().all(|&x| !x), "{m:?}");
        }
    }

    /// PWR-22.4: charging ends when the charger comes off (a fall from the peak) or the rise stops (full).
    #[test]
    fn charging_ends() {
        let mut c = Charge::default();
        minutes(&mut c, &ON_CHARGER);
        assert!(c.charging);
        minutes(&mut c, &[2662, 2640]);
        assert!(!c.charging, "taken off the charger");
        let mut c = Charge::default();
        minutes(&mut c, &ON_CHARGER);
        let flat = vec![2662; CHARGE_FLAT_MIN as usize - 1];
        minutes(&mut c, &flat);
        assert!(c.charging, "a short flat stretch still charging");
        minutes(&mut c, &[2662]);
        assert!(!c.charging, "full: no rise for 15 minutes");
    }

    /// PWR-22.4: a transmission starts the window again, so its recovery isn't a rise; Battery feeds it per sample.
    #[test]
    fn charging_through_battery_and_keyed() {
        let mut c = Charge::default();
        minutes(&mut c, &ON_CHARGER[..CHARGE_WINDOW_MIN]);
        c.keyed();
        minutes(&mut c, &ON_CHARGER[CHARGE_WINDOW_MIN..2 * CHARGE_WINDOW_MIN]);
        assert!(!c.charging, "the window started again at the key-up");
        minutes(&mut c, &[ON_CHARGER[2 * CHARGE_WINDOW_MIN]]);
        assert!(c.charging);
        // through Battery: readings every 2 s rising one count a minute
        let g = Gauge::defaults();
        let mut b = Battery::default();
        let mut last = None;
        for i in 0..(20 * 30) {
            let now = i * SAMPLE_MS;
            last = b.update(&g, (2600 + i / 30) as u16, now, false);
        }
        assert!(last.unwrap().charging);
    }

    fn raw_mv(mv: u32) -> u16 {
        (mv * 160 / 482) as u16
    }

    /// PWR-22, PWR-22.1: the knots sit on the calibrated thresholds; 0 at the cut-off, about half at 7.4 V, 100 at 8.1 V
    /// (a full pack at rest, 8.14 V, reads 100) and flat above; the percentage never falls as the voltage rises.
    #[test]
    fn percent_on_the_thresholds() {
        let g = Gauge::defaults();
        for (i, t) in g.0[..6].iter().enumerate() {
            assert_eq!(percent(&g, (*t as u16) << 4), KNOT_PCT[i], "t{i}");
        }
        assert_eq!(percent(&g, raw_mv(6000)), 0);
        assert_eq!(percent(&g, raw_mv(8400)), 100);
        assert_eq!(percent(&g, raw_mv(8140)), 100, "Alex's full pack at rest");
        assert!(percent(&g, raw_mv(8000)) < 100, "below 8.1 V isn't full");
        assert_eq!(percent(&g, raw_mv(9000)), 100);
        let half = percent(&g, raw_mv(7400));
        assert!((45..=55).contains(&half), "7.4 V: {half}");
        let mut last = 0;
        for r in 1900..3000u16 {
            let p = percent(&g, r);
            assert!(p >= last, "{r}: {p} < {last}");
            last = p;
        }
        // a calibration with its thresholds moved moves the curve: shifted 3 steps up, t3's reading reads 30 %
        let mut c = g;
        for t in &mut c.0[..6] {
            *t += 3;
        }
        assert_eq!(percent(&c, (c.0[3] as u16) << 4), 30);
        assert!(percent(&c, (g.0[3] as u16) << 4) < 30);
        // thresholds out of order don't divide by zero
        let bad = Gauge([0x90, 0x90, 0x80, 0x93, 0x98, 0xA0, 0xBB]);
        for r in (0..4096).step_by(7) {
            assert!(percent(&bad, r) <= 100);
        }
    }

    /// PWR-22.2: a calibration whose full-icon threshold (t5) is above where a full pack rests (both our radios'
    /// template: 0xAA, 8.19 V) still reads a full pack, 8.14 V, as 100 % and four bars; t5 is capped at 7.95 V, and
    /// the warning and cut-off stay on the calibrated thresholds.
    #[test]
    fn a_high_full_threshold_still_reads_full() {
        let g = Gauge([0x7F, 0x85, 0x91, 0x9A, 0xA2, 0xAA, 0xFF]);
        assert_eq!(percent(&g, raw_mv(8140)), 100);
        assert_eq!(bars(percent(&g, raw_mv(8140))), 4);
        assert_eq!(percent(&g, raw_mv(7950)), 65, "the capped t5");
        assert!(percent(&g, raw_mv(7900)) < 65 && percent(&g, raw_mv(7900)) > 45);
        assert_eq!(percent(&g, (0xA2u16) << 4), 45, "t4 as calibrated");
        let mut last = 0;
        for r in 1900..3000u16 {
            let p = percent(&g, r);
            assert!(p >= last, "{r}: {p} < {last}");
            last = p;
        }
    }

    /// SBAR-9, PWR-22: the icon steps at the thresholds, as stock's bars.
    #[test]
    fn bars_step_at_the_thresholds() {
        let g = Gauge::defaults();
        let at = |i: usize, d: i32| bars(percent(&g, (((g.0[i] as i32) << 4) + d) as u16));
        assert_eq!((at(5, 0), at(5, -1)), (4, 3));
        assert_eq!((at(4, 0), at(4, -1)), (3, 2));
        assert_eq!((at(3, 0), at(3, -1)), (2, 1));
        assert_eq!(at(0, -100), 1);
    }

    /// PWR-22: a few seconds' average; held while keyed and for HOLD_MS after; a rise shows only after RISE_MS.
    #[test]
    fn averaged_held_and_slow_to_rise() {
        let g = Gauge::defaults();
        let mut b = Battery::default();
        assert_eq!(b.update(&g, 0, 0, false), None, "nothing before the first reading");
        let r0 = raw_mv(7700);
        assert_eq!(b.update(&g, r0, 0, false).unwrap().raw, r0, "the first reading shows at once");
        // a single low reading moves the average only part of the way
        let s = b.update(&g, raw_mv(7100), 2000, false).unwrap();
        assert!(s.raw < r0 && s.raw > raw_mv(7400), "{}", s.raw);
        let mut t = 2000;
        for _ in 0..10 {
            t += 2000;
            b.update(&g, r0, t, false);
        }
        let before = b.update(&g, r0, t, false).unwrap();
        // keyed: the sag is not taken, nor for HOLD_MS after the unkey
        t += 2000;
        assert_eq!(b.update(&g, raw_mv(6500), t, true).unwrap(), before);
        for _ in 0..4 {
            t += 2000;
            assert_eq!(b.update(&g, raw_mv(6900), t, false).unwrap(), before, "held after the unkey ({t})");
        }
        t += HOLD_MS;
        b.update(&g, raw_mv(7000), t, false);
        let fell = b.update(&g, raw_mv(7000), t + 2000, false).unwrap();
        assert!(fell.raw < before.raw, "falls once the hold ends: {fell:?} {before:?}");
        // charging: higher readings wait RISE_MS before they show
        t += 4000;
        let start = t;
        while t - start < RISE_MS {
            assert!(b.update(&g, raw_mv(8200), t, false).unwrap().raw <= fell.raw, "no rise yet at {}", t - start);
            t += 2000;
        }
        assert!(b.update(&g, raw_mv(8200), t, false).unwrap().raw > fell.raw, "a sustained rise shows");
    }

    /// PWR-22.3: a first reading taken under the power-up load (low) doesn't hold for RISE_MS: within SETTLE_MS of
    /// the first reading the level follows the average up; after it, rises wait RISE_MS again.
    #[test]
    fn power_up_settles_quickly() {
        let g = Gauge::defaults();
        let mut b = Battery::default();
        let low = raw_mv(6900);
        let real = raw_mv(7700);
        assert_eq!(b.update(&g, low, 0, false).unwrap().raw, low, "the sagging first reading shows");
        let mut t = 0;
        let mut shown = low;
        while t < 12_000 {
            t += 2000;
            shown = b.update(&g, real, t, false).unwrap().raw;
        }
        assert!(shown > real - (real - low) / 10, "settled close to the real level within 12 s: {shown} of {real}");
        // after the window, a rise waits RISE_MS again
        t = SETTLE_MS + 4000;
        let before = b.update(&g, real, t, false).unwrap().raw;
        t += 2000;
        assert_eq!(b.update(&g, raw_mv(8100), t, false).unwrap().raw, before, "no rise yet after the window");
    }

    /// PWR-20: the warning below t1, the cut-off below t0, from the shown reading.
    #[test]
    fn low_and_cutoff() {
        let g = Gauge::defaults();
        let s = |r: u16| Battery::default().update(&g, r, 0, false).unwrap();
        let t = |i: usize| (g.0[i] as u16) << 4;
        assert_eq!((s(t(1)).low, s(t(1) - 1).low), (false, true));
        assert_eq!((s(t(0)).cutoff, s(t(0) - 1).cutoff), (false, true));
        assert_eq!(s(t(0)).pct, 0);
    }
}
