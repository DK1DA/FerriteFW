//! Tone search (manual chapter 22, TSRCH-1 to TSRCH-3): the searched slot's chip listens with its sub-tone
//! detectors in scan (the tone hint's probe, TSRCH-12, held for as long as the screen is up), and this decides
//! what the screen shows. A reading counts when the next one agrees with the carrier there: with no carrier the
//! scan detectors read noise, and a single read can catch the detector settling. The result stays after the
//! carrier drops, until a different tone is found.
//!
//! Until Oct 15, 2026 the Radio task dropped the screen's command (tone search was left for "after 2d"), so the
//! screen said "Searching" forever on the radio while the simulator faked a result (the receiving session's
//! TSRCH-3).

/// One search's state (4 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Search {
    /// The last reading (CHAN-3's tone word, 0 none).
    last: u16,
    /// What the screen shows (0: still searching).
    pub found: u16,
}

impl Search {
    pub const fn new() -> Self {
        Search { last: 0, found: 0 }
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
}
