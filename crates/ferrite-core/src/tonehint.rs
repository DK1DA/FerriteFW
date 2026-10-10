//! The tone hint (manual chapter 22, TSRCH-12 to TSRCH-15): a carrier keeps a slot's noise squelch open but its
//! CTCSS or DCS tone squelch keeps the audio shut, so the Radio task measures the tone on air and the card says
//! what it heard. This is the decision, per slot: when to switch the chip's sub-tone detectors to scan, when to put
//! the slot's own detector back, and what to show. The registers are `ferrite_radio::chip`'s.

use crate::freq::Freq10;

/// The carrier must hold this long with the tone squelch shut before the probe (TSRCH-12).
pub const CLOSED_MS: u32 = 500;
/// The longest the detectors stay in scan (TSRCH-12; bench-confirm the settle time).
pub const PROBE_MS: u32 = 1200;
/// The hint stays this long after the carrier was last heard (TSRCH-14).
pub const KEEP_MS: u32 = 30_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// Carrier with the tone squelch shut since then (ms).
    Closed(u32),
    /// The detectors scan since then (ms).
    Probing(u32),
    /// Measured (or given up) for this carrier: nothing more until it drops (TSRCH-15).
    Done,
}

/// What the Radio task does to the slot's chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    None,
    /// Switch the chip's sub-tone detectors to scan.
    Start,
    /// Put the slot's own tone detector back.
    Restore,
}

/// One slot's hint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hint {
    phase: Phase,
    /// The tone word heard (CHAN-3's form), 0 none: what the card shows.
    pub heard: u16,
    /// When the carrier was last heard with a hint up (ms).
    seen: u32,
    /// The frequency and receive tone the hint is for: a change clears it (TSRCH-14).
    freq: Freq10,
    tone: u16,
}

impl Hint {
    pub const fn new() -> Self {
        Hint { phase: Phase::Idle, heard: 0, seen: 0, freq: 0, tone: 0 }
    }

    /// Whether the detectors are scanning now: the tone squelch's status bits mean nothing meanwhile.
    pub fn probing(&self) -> bool {
        matches!(self.phase, Phase::Probing(_))
    }

    /// The slot's chip was tuned again: its own detector is set up afresh, so a probe just ends.
    pub fn retuned(&mut self) {
        if self.probing() {
            self.phase = Phase::Idle;
        }
    }

    /// One receive tick for a slot whose chip is listening: `carrier` the noise squelch, `toned` the tone squelch
    /// open, `eligible` whether it may be probed (TSRCH-15: held on a signal, a receive tone set, not transmitting,
    /// scanning or monitored). `freq` and `rx_tone` are the slot's now.
    pub fn tick(&mut self, now: u32, freq: Freq10, rx_tone: u16, carrier: bool, toned: bool, eligible: bool) -> Act {
        let mut act = Act::None;
        if (freq, rx_tone) != (self.freq, self.tone) {
            (self.freq, self.tone, self.heard) = (freq, rx_tone, 0);
            if self.probing() {
                act = Act::Restore;
            }
            self.phase = Phase::Idle;
        }
        if self.heard != 0 {
            if carrier {
                self.seen = now;
            } else if now.wrapping_sub(self.seen) >= KEEP_MS {
                self.heard = 0;
            }
        }
        let shut = carrier && !toned && eligible && rx_tone != 0;
        match self.phase {
            Phase::Probing(t) => {
                if !carrier || !eligible {
                    self.phase = Phase::Idle;
                    act = Act::Restore;
                } else if now.wrapping_sub(t) >= PROBE_MS {
                    self.phase = Phase::Done;
                    act = Act::Restore;
                }
                return act;
            }
            _ if toned && carrier => {
                self.heard = 0; // the tones match now
                if self.phase != Phase::Done {
                    self.phase = Phase::Idle;
                }
            }
            Phase::Idle if shut => self.phase = Phase::Closed(now),
            Phase::Closed(_) if !shut => self.phase = Phase::Idle,
            Phase::Closed(t) if now.wrapping_sub(t) >= CLOSED_MS => {
                self.phase = Phase::Probing(now);
                act = Act::Start;
            }
            Phase::Done if !carrier => self.phase = Phase::Idle,
            _ => {}
        }
        act
    }

    /// For a slot no chip is listening to: the hint still goes 30 s after its carrier was last heard (TSRCH-14).
    pub fn age(&mut self, now: u32) {
        if self.heard != 0 && now.wrapping_sub(self.seen) >= KEEP_MS {
            self.heard = 0;
        }
    }

    /// The detectors read a tone (word in CHAN-3's form) while probing: the hint, unless it is the slot's own.
    pub fn found(&mut self, now: u32, word: u16) -> Act {
        if !self.probing() {
            return Act::None;
        }
        self.phase = Phase::Done;
        self.heard = if word == self.tone { 0 } else { word };
        self.seen = now;
        Act::Restore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: Freq10 = 14_652_000;
    const T100: u16 = 1000;

    /// TSRCH-12: the probe starts only after the carrier has held 0.5 s with the tone squelch shut, and a match
    /// meanwhile starts nothing.
    #[test]
    fn probe_waits_for_half_a_second() {
        let mut h = Hint::new();
        assert_eq!(h.tick(0, F, T100, true, false, true), Act::None);
        assert_eq!(h.tick(400, F, T100, true, false, true), Act::None);
        assert_eq!(h.tick(500, F, T100, true, false, true), Act::Start);
        assert!(h.probing());
        // a carrier that opens the tone squelch never probes
        let mut h = Hint::new();
        for t in (0..2000).step_by(100) {
            assert_eq!(h.tick(t, F, T100, true, true, true), Act::None);
        }
        // nor a short one
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(300, F, T100, false, false, true);
        assert_eq!(h.tick(600, F, T100, true, false, true), Act::None);
    }

    /// TSRCH-12, TSRCH-13: a tone found restores the detector and becomes the hint; the slot's own tone shows nothing.
    #[test]
    fn found_restores_and_shows() {
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        assert_eq!(h.found(700, 885), Act::Restore);
        assert_eq!(h.heard, 885);
        assert!(!h.probing());
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        assert_eq!(h.found(700, T100), Act::Restore);
        assert_eq!(h.heard, 0);
        // not probing: a reading is ignored
        assert_eq!(Hint::new().found(0, 885), Act::None);
    }

    /// TSRCH-12: nothing found within 1.2 s, or the carrier gone, puts the detector back.
    #[test]
    fn probe_ends_by_itself() {
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        assert_eq!(h.tick(1600, F, T100, true, false, true), Act::None);
        assert_eq!(h.tick(1700, F, T100, true, false, true), Act::Restore);
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        assert_eq!(h.tick(600, F, T100, false, false, true), Act::Restore);
        // not eligible any more (watch moved on, a scan started, transmit)
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        assert_eq!(h.tick(600, F, T100, true, false, false), Act::Restore);
    }

    /// TSRCH-15: one probe per carrier; the next after it drops and comes back. Never without a receive tone or
    /// when not eligible.
    #[test]
    fn once_per_carrier() {
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        h.found(800, 885);
        for t in (900..5000).step_by(100) {
            assert_eq!(h.tick(t, F, T100, true, false, true), Act::None, "same carrier at {t}");
        }
        h.tick(5100, F, T100, false, false, true);
        h.tick(6000, F, T100, true, false, true);
        assert_eq!(h.tick(6500, F, T100, true, false, true), Act::Start, "a new carrier");
        for (tone, ok) in [(0, true), (T100, false)] {
            let mut h = Hint::new();
            for t in (0..3000).step_by(100) {
                assert_eq!(h.tick(t, F, tone, true, false, ok), Act::None);
            }
        }
    }

    /// TSRCH-14: the hint lasts 30 s after the carrier was last heard, and goes at once on a retune, a new
    /// receive tone or a match.
    #[test]
    fn hint_clears() {
        let shown = || {
            let mut h = Hint::new();
            h.tick(0, F, T100, true, false, true);
            h.tick(500, F, T100, true, false, true);
            h.found(800, 885);
            h
        };
        let mut h = shown();
        h.tick(10_000, F, T100, true, false, true); // still on air
        h.tick(10_100, F, T100, false, false, true);
        h.tick(39_000, F, T100, false, false, true);
        assert_eq!(h.heard, 885);
        h.tick(40_000, F, T100, false, false, true);
        assert_eq!(h.heard, 0);
        let mut h = shown();
        h.age(30_799);
        assert_eq!(h.heard, 885, "unheard slots age too");
        h.age(30_800);
        assert_eq!(h.heard, 0);
        let mut h = shown();
        h.tick(900, F + 500, T100, true, false, true);
        assert_eq!(h.heard, 0, "retuned");
        let mut h = shown();
        h.tick(900, F, 885, true, false, true);
        assert_eq!(h.heard, 0, "new tone");
        let mut h = shown();
        h.tick(900, F, T100, true, true, true);
        assert_eq!(h.heard, 0, "the tones match");
    }

    /// TSRCH-14: a retune or tone change during the probe puts the detector back; a chip retune just ends it.
    #[test]
    fn change_during_probe() {
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        assert_eq!(h.tick(600, F, 885, true, false, true), Act::Restore);
        assert!(!h.probing());
        let mut h = Hint::new();
        h.tick(0, F, T100, true, false, true);
        h.tick(500, F, T100, true, false, true);
        h.retuned();
        assert!(!h.probing());
    }
}
