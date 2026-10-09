//! VOX speech detection (manual chapter 18: VOX-2, VOX-3, VOX-5, VOX-14). The Radio task reads the mic level on
//! PA0 and feeds it here; this decides when speech starts and when it has stopped for the tail. Keying is the
//! UI's, with every PTT check (VOX-4): this only says whether there is speech.

/// Stock's thresholds (0x0802F73C, PA0 reading): level 1 = 101 down to level 9 = 21, in steps of 10 (VOX-14).
pub fn threshold(level: u8) -> u16 {
    111 - 10 * level.clamp(1, 9) as u16
}

/// Stock's hysteresis: speech goes on above the threshold and stays on down to this many counts below it.
pub const HYSTERESIS: u16 = 8;

/// No VOX until this long after boot (VOX-3).
pub const BOOT_HOLD_MS: u32 = 5000;

/// The detector's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Vox {
    /// Speech, the tail included.
    on: bool,
    /// When the level last stood above the off level (ms).
    last: u32,
}

impl Vox {
    pub const fn new() -> Self {
        Vox { on: false, last: 0 }
    }

    /// One mic reading at `now` (ms since boot). `level` 1-9, or 0 for VOX off; `tail_ms` the hang time;
    /// `blocked` while the slot receives or an accessory change is debounced (VOX-3). Returns whether speech is
    /// on, the tail included.
    pub fn step(&mut self, mic: u16, level: u8, tail_ms: u32, blocked: bool, now: u32) -> bool {
        if level == 0 || blocked || now < BOOT_HOLD_MS {
            *self = Vox::new();
            return false;
        }
        let t = threshold(level);
        let above = if self.on { mic + HYSTERESIS >= t } else { mic >= t };
        if above {
            (self.on, self.last) = (true, now);
        } else if self.on && now.wrapping_sub(self.last) >= tail_ms {
            self.on = false; // VOX-5: below the level for the whole tail
        }
        self.on
    }

    pub fn on(&self) -> bool {
        self.on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VOX-14: stock's thresholds, 101 at level 1 down to 21 at level 9.
    #[test]
    fn thresholds_are_stocks() {
        assert_eq!(threshold(1), 101);
        assert_eq!(threshold(5), 61);
        assert_eq!(threshold(9), 21);
    }

    /// VOX-2, VOX-5, VOX-14: on at the threshold, held down to 8 counts below it, off after the tail.
    #[test]
    fn speech_and_tail() {
        let mut v = Vox::new();
        let t = threshold(5);
        assert!(!v.step(t - 1, 5, 2000, false, 6000), "below the threshold");
        assert!(v.step(t, 5, 2000, false, 6100), "at the threshold");
        assert!(v.step(t - HYSTERESIS, 5, 2000, false, 6200), "hysteresis holds it");
        assert!(v.step(0, 5, 2000, false, 8100), "the tail");
        assert!(!v.step(0, 5, 2000, false, 8200), "2 s below: off");
    }

    /// VOX-1, VOX-3: off at level 0, during the first 5 s and while receiving.
    #[test]
    fn never_while_off_booting_or_receiving() {
        let mut v = Vox::new();
        assert!(!v.step(500, 0, 2000, false, 9000), "VOX off");
        assert!(!v.step(500, 9, 2000, false, 4999), "within 5 s of boot");
        assert!(v.step(500, 9, 2000, false, 9000));
        assert!(!v.step(500, 9, 2000, true, 9100), "the slot receives: speech drops at once");
    }
}
