//! SQL-19 (decision 298): the APRS slot's squelch over the chip's own decision (SQL-18.4), which opens at once
//! (SQL-17's open delay 0) and so on single noise spikes. Three things on top of the chip:
//! - a retuned chip's first [`SETTLE_MS`] are ignored (on a shared VHF chip C is retuned every turn);
//! - closed, it opens on the chip's open seen on [`OPEN_TICKS`] ticks in a row (a packet starts with 300 ms of
//!   flags, so 20 ms costs nothing);
//! - open, it follows the chip (its hold through a noisy reading and its tail, which a packet's end needs).
//!
//! And the counts that tell which of these the radio meets ([`Counts`]), for the readback and the bench log.

/// A retuned chip's opens are ignored this long.
pub const SETTLE_MS: u32 = 30;
/// The chip's open on this many 10 ms ticks in a row opens the slot.
pub const OPEN_TICKS: u8 = 2;
/// An open shorter than this counts as short (noise, not a packet: the flags alone last 300 ms).
pub const SHORT_MS: u32 = 50;

/// Since power-on, each saturating.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Counts {
    /// Opens of the slot.
    pub opens: u16,
    /// Of those, the ones shorter than [`SHORT_MS`].
    pub short: u16,
    /// The chip's opens too brief to open the slot (fewer than [`OPEN_TICKS`] ticks).
    pub spikes: u16,
    /// The chip's opens ignored inside [`SETTLE_MS`] of a retune.
    pub settle: u16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AprsSq {
    tuned_at: u32,
    run: u8,
    open_at: Option<u32>,
    /// The chip read open at the last tick (a new open edge counts once).
    chip_was: bool,
    /// When the run of open readings that opened the slot began (APRS-56.4).
    seen_at: u32,
    /// The last open's lag: from the first tick that read the chip open to the slot opening, ms (APRS-56.4).
    pub lag_ms: u32,
    pub counts: Counts,
}

impl AprsSq {
    pub const NEW: AprsSq = AprsSq { tuned_at: 0, run: 0, open_at: None, chip_was: false, seen_at: 0, lag_ms: 0, counts: Counts { opens: 0, short: 0, spikes: 0, settle: 0 } };

    /// The chip was retuned (or the slot came to it) at `now`: closed, settling.
    pub fn retuned(&mut self, now: u32) {
        let (c, lag_ms) = (self.counts, self.lag_ms);
        *self = AprsSq { tuned_at: now, counts: c, lag_ms, ..Default::default() };
    }

    /// APRS-56.5: the chip read open on this tick's run of readings but the slot isn't open yet (fewer than
    /// [`OPEN_TICKS`]): the capture starts here, a tick ahead of the slot (a spike costs one capture's tail).
    pub fn arming(&self) -> bool {
        self.run > 0 && self.open_at.is_none()
    }

    /// A tick with the chip's decision `chip_open`: the slot open or not, and how long an open that just closed
    /// lasted.
    pub fn step(&mut self, now: u32, chip_open: bool) -> (bool, Option<u32>) {
        let edge = chip_open && !self.chip_was;
        self.chip_was = chip_open;
        let c = &mut self.counts;
        if let Some(at) = self.open_at {
            if chip_open {
                return (true, None);
            }
            self.open_at = None;
            let ms = now.wrapping_sub(at);
            if ms < SHORT_MS {
                c.short = c.short.saturating_add(1);
            }
            return (false, Some(ms));
        }
        if now.wrapping_sub(self.tuned_at) < SETTLE_MS {
            if edge {
                c.settle = c.settle.saturating_add(1);
            }
            return (false, None);
        }
        if !chip_open {
            if self.run > 0 {
                c.spikes = c.spikes.saturating_add(1);
            }
            self.run = 0;
            return (false, None);
        }
        if self.run == 0 {
            self.seen_at = now;
        }
        self.run = self.run.saturating_add(1);
        if self.run < OPEN_TICKS {
            return (false, None);
        }
        (self.open_at, self.run, self.lag_ms) = (Some(now), 0, now.wrapping_sub(self.seen_at));
        c.opens = c.opens.saturating_add(1);
        (true, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sq: &mut AprsSq, from: u32, chip: &[bool]) -> std::vec::Vec<bool> {
        chip.iter().enumerate().map(|(i, &c)| sq.step(from + i as u32 * 10, c).0).collect()
    }

    /// SQL-19.1: a single tick's open is a spike, two in a row open the slot.
    #[test]
    fn spike_filtered_two_ticks_open() {
        let mut s = AprsSq::default();
        s.retuned(0);
        assert_eq!(run(&mut s, 100, &[true, false, true, true, true]), [false, false, false, true, true]);
        assert_eq!((s.counts.spikes, s.counts.opens), (1, 1));
    }

    /// APRS-56.4: the lag is from the first open reading of the run that opened the slot (a spike before it
    /// doesn't count): one tick at 10 ms, five at 50.
    #[test]
    fn lag_from_the_runs_first_reading() {
        let mut s = AprsSq::default();
        s.retuned(0);
        run(&mut s, 100, &[true, false, true, true]);
        assert_eq!(s.lag_ms, 10);
        let mut s = AprsSq::default();
        s.retuned(0);
        for (i, c) in [false, true, true].iter().enumerate() {
            s.step(100 + i as u32 * 50, *c);
        }
        assert_eq!(s.lag_ms, 50);
    }

    /// APRS-56.5: arming on the first open reading, not while settling, and no longer once the slot is open.
    #[test]
    fn arms_on_the_first_reading() {
        let mut s = AprsSq::default();
        s.retuned(0);
        s.step(10, true);
        assert!(!s.arming(), "settling");
        s.step(100, true);
        assert!(s.arming());
        s.step(110, true);
        assert!(!s.arming(), "open now");
        s.step(120, false);
        s.step(130, true);
        s.step(140, false);
        assert!(!s.arming(), "a spike ended");
    }

    /// SQL-19.2: inside the settle window after a retune the chip's opens are ignored and counted.
    #[test]
    fn settle_after_retune() {
        let mut s = AprsSq::default();
        s.retuned(1000);
        assert_eq!(run(&mut s, 1000, &[true, true, true, true, true]), [false, false, false, false, true]);
        assert_eq!(s.counts.settle, 1, "one open edge inside the window");
        // an open slot is closed by a retune
        s.retuned(2000);
        assert!(!s.step(2005, true).0);
    }

    /// SQL-19.3: open, it follows the chip; an open under 50 ms counts as short, a packet's doesn't.
    #[test]
    fn follows_chip_counts_short() {
        let mut s = AprsSq::default();
        s.retuned(0);
        run(&mut s, 100, &[true, true, true]); // open at 110
        assert_eq!(s.step(130, false), (false, Some(20)));
        assert_eq!(s.counts.short, 1);
        run(&mut s, 200, &[true; 40]); // open at 210, 390 ms
        assert_eq!(s.step(600, false), (false, Some(390)));
        assert_eq!((s.counts.opens, s.counts.short), (2, 1));
    }
}
