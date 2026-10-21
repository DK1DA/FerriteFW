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

/// The percentage at t0..t5, then at [`FULL_MV`].
pub const KNOT_PCT: [u8; 7] = [0, 5, 15, 30, 45, 65, 100];
/// A full 2S Li-ion pack at rest under the receiver's load, mV (Alex measured 8.14 V on a full charge, Oct 16, 2026;
/// decision 199). The charger's 8.4 V cut-off isn't where a full pack sits, so it would never read 100.
pub const FULL_MV: u32 = 8100;
/// The reading for [`FULL_MV`] on the 12-bit PA1 scale (48.2 mV per step of the reading >> 4).
const FULL_RAW: u32 = FULL_MV * 160 / 482;
/// A new reading is taken at most this often (the Radio task reads PA1 every 2 s).
pub const SAMPLE_MS: u32 = 2_000;
/// Held this long after an unkey: the pack recovers from the transmit sag.
pub const HOLD_MS: u32 = 10_000;
/// A higher reading shows only once it has lasted this long (charging, not a recovery blip).
pub const RISE_MS: u32 = 60_000;

/// The percentage for a 12-bit PA1 reading on this gauge: 0 at or below the cut-off, 100 at 8.1 V and over.
pub fn percent(g: &Gauge, raw: u16) -> u8 {
    let raw = raw as u32;
    let mut x = [0u32; 7];
    for (i, t) in g.0[..6].iter().enumerate() {
        x[i] = (*t as u32) << 4;
    }
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
}

impl Battery {
    /// Takes the reading `raw` (0 = none yet) at `now` ms; `keyed` while any slot transmits.
    pub fn update(&mut self, g: &Gauge, raw: u16, now: u32, keyed: bool) -> Option<Shown> {
        if keyed {
            self.hold_until = Some(now.wrapping_add(HOLD_MS));
            self.rise_since = None;
        }
        let held = self.hold_until.is_some_and(|t| (t.wrapping_sub(now) as i32) > 0);
        if !held {
            self.hold_until = None;
        }
        if raw != 0 && !held && (self.avg.is_none() || now.wrapping_sub(self.last) >= SAMPLE_MS) {
            self.last = now;
            let r = (raw as u32) << 4;
            // a few seconds' average: each reading moves it a third of the way (about 5 s at one per 2 s)
            let a = self.avg.map_or(r, |a| (2 * a + r) / 3);
            self.avg = Some(a);
            let a = ((a + 8) >> 4) as u16;
            match self.shown {
                // falls with the average; rises only once it has stayed higher for RISE_MS
                Some(s) if a > s => {
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
        Some(Shown { raw, pct: percent(g, raw), low: v < g.0[1] as u16, cutoff: v < g.0[0] as u16 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
