//! Tone search (manual chapter 22, TSRCH-1 to TSRCH-3): the searched slot's chip listens with its sub-tone
//! detectors in scan (the tone hint's probe, TSRCH-12, held for as long as the screen is up), and this decides
//! what the screen shows. A reading counts when the next one agrees with the carrier there: with no carrier the
//! scan detectors read noise, and a single read can catch the detector settling. The result stays after the
//! carrier drops, until a different tone is found. A CTCSS reading is the mean of the last [`AVG`] with the carrier
//! before it is snapped to a tone (TSRCH-3.1): the detector wanders about ±1.4 % carrier to carrier, more than
//! the 1.5 % between 250.3 and 254.1 Hz, so a single read can land in the refused middle or on the neighbour.
//!
//! Until Oct 15, 2026 the Radio task dropped the screen's command (tone search was left for "after 2d"), so the
//! screen said "Searching" forever on the radio while the simulator faked a result (the receiving session's
//! TSRCH-3).

/// CTCSS reads averaged before snapping (TSRCH-3.1).
pub const AVG: usize = 4;

/// One search's state (14 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Search {
    /// The last CTCSS reads with the carrier (0.1 Hz), oldest first, and how many of them are filled.
    reads: [u16; AVG],
    n: u8,
    /// The last reading (CHAN-3's tone word, 0 none).
    last: u16,
    /// What the screen shows (0: still searching).
    pub found: u16,
}

impl Search {
    pub const fn new() -> Self {
        Search { reads: [0; AVG], n: 0, last: 0, found: 0 }
    }

    /// One CTCSS reading (0.1 Hz, from the scan detector) through the average (TSRCH-3.1): the mean of the last
    /// [`AVG`] while the carrier stays and each tick reads a CTCSS tone; None until there are that many. No
    /// carrier or no reading starts it again.
    pub fn average(&mut self, carrier: bool, dhz: Option<u16>) -> Option<u16> {
        let Some(d) = dhz.filter(|_| carrier) else {
            self.n = 0;
            return None;
        };
        self.reads.copy_within(1.., 0);
        self.reads[AVG - 1] = d;
        self.n = (self.n + 1).min(AVG as u8);
        let sum: u32 = self.reads.iter().map(|&r| r as u32).sum();
        (self.n as usize == AVG).then_some(((sum + AVG as u32 / 2) / AVG as u32) as u16)
    }

    /// One receive tick: `carrier` the noise squelch, `word` what the scan detectors read (CHAN-3's form, from
    /// `ferrite_radio::chip::heard_tone`). True when `found` changed.
    pub fn step(&mut self, carrier: bool, word: Option<u16>) -> bool {
        let w = if carrier { word.unwrap_or(0) } else { 0 };
        let agree = w != 0 && w == self.last;
        self.last = w;
        if agree && w != self.found {
            self.found = w;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D023N: u16 = 0x8000 | 0o23;
    const D023I: u16 = 0xC000 | 0o23;

    /// TSRCH-3: two agreeing reads with the carrier give the result; noise (no carrier) or a lone read doesn't;
    /// the result stays after the carrier, and a different tone replaces it.
    #[test]
    fn two_agreeing_reads() {
        let mut s = Search::new();
        assert!(!s.step(false, Some(885)), "no carrier: noise");
        assert!(!s.step(false, Some(885)));
        assert_eq!(s.found, 0);
        assert!(!s.step(true, Some(885)), "one read");
        assert!(s.step(true, Some(885)), "the next agrees");
        assert_eq!(s.found, 885);
        assert!(!s.step(true, Some(885)), "no change, no news");
        assert!(!s.step(false, None));
        assert_eq!(s.found, 885, "kept after the carrier");
        assert!(!s.step(true, Some(D023N)));
        assert!(!s.step(true, Some(D023I)), "a disagreeing read");
        assert!(s.step(true, Some(D023I)));
        assert_eq!(s.found, D023I, "a different tone replaces it");
        assert!(!s.step(true, None) && !s.step(true, Some(885)), "a gap breaks the agreement");
    }

    /// TSRCH-3.1: CTCSS reads are averaged over the last four with the carrier before snapping: none until four,
    /// then the mean, sliding; no carrier or no reading starts again.
    #[test]
    fn ctcss_reads_averaged() {
        let mut s = Search::new();
        // 250.3 Hz read as the detector wanders: 247.0 and 254.0 alone would land off it
        assert_eq!(s.average(true, Some(2470)), None);
        assert_eq!(s.average(true, Some(2540)), None);
        assert_eq!(s.average(true, Some(2490)), None);
        assert_eq!(s.average(true, Some(2512)), Some(2503), "the mean of four");
        assert_eq!(s.average(true, Some(2500)), Some(2511), "sliding: the oldest drops");
        assert_eq!(s.average(false, Some(2500)), None, "no carrier: starts again");
        assert_eq!(s.average(true, Some(885)), None);
        assert_eq!(s.average(true, None), None, "no reading: starts again");
    }
}
