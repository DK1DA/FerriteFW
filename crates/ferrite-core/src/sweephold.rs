//! Input first on the band scope and home's strip (SCOPE-14, UI backlog 7): a key or knob step holds the sweep
//! for [`HOLD_MS`] after the last one and starts its pass again from the first point, so the Radio task (above
//! the UI) leaves the processor and the bus to the UI while it handles the input and draws it. The bars catch up
//! after.

/// How long the sweep waits after the last key or knob step: about one large redraw on the radio's bus.
pub const HOLD_MS: u32 = 200;

/// The hold (8 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepHold {
    until: Option<u32>,
}

impl SweepHold {
    pub const fn new() -> Self {
        SweepHold { until: None }
    }

    /// A key or knob step: held from now until [`HOLD_MS`] after it. True when it wasn't held: the caller starts
    /// the pass again (a held pass has already been).
    pub fn input(&mut self, now: u32) -> bool {
        let was = self.until.is_some();
        self.until = Some(now.wrapping_add(HOLD_MS));
        !was
    }

    /// Whether the sweep waits now; the hold ends by itself.
    pub fn held(&mut self, now: u32) -> bool {
        match self.until {
            Some(t) if now.wrapping_sub(t) >= 1 << 31 => true,
            _ => {
                self.until = None;
                false
            }
        }
    }

    /// When the hold ends, while one runs: the Radio task waits until then instead of its dwell.
    pub fn deadline(&self) -> Option<u32> {
        self.until
    }

    /// The sweep stopped: no hold carries into the next one.
    pub fn clear(&mut self) {
        self.until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SCOPE-14: held for 200 ms after the last input, each one renewing it; the pass restarts only at the first.
    #[test]
    fn held_after_the_last_input() {
        let mut h = SweepHold::new();
        assert!(!h.held(0));
        assert!(h.input(1_000), "the first input restarts the pass");
        assert!(h.held(1_000) && h.held(1_199));
        assert!(!h.input(1_150), "a second within the hold: no new restart");
        assert_eq!(h.deadline(), Some(1_350));
        assert!(h.held(1_349));
        assert!(!h.held(1_350), "over 200 ms after the last");
        assert_eq!(h.deadline(), None);
        assert!(h.input(5_000));
        h.clear();
        assert!(!h.held(5_001));
        // Across the u32 wrap.
        assert!(h.input(u32::MAX - 50));
        assert!(h.held(10));
        assert!(!h.held(150));
    }
}
