//! DTMF heard (DEC-6): the digits the BK4829's own detector reports on a slot, kept for its card, where they take
//! the state word's place ("DTMF 101#"): a PTT ID or a dialled sequence, read at a glance, no popup. A digit after
//! [`GAP_MS`] starts a new string; once the carrier is gone the string stays [`HOLD_MS`], then clears.

/// The digits kept: the newest, older ones dropped from the front.
pub const MAX: usize = 12;
/// A digit this long after the last starts a new string.
pub const GAP_MS: u32 = 3000;
/// Shown this long after the last digit once the slot's carrier has gone.
pub const HOLD_MS: u32 = 5000;

/// The BK4829's REG_0B code order.
const SYMBOLS: &[u8; 16] = b"0123456789ABCD*#";

/// A slot's digits, a nibble each (the code, [`SYMBOLS`]' order): small, as the live snapshot holds three.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Heard {
    d: [u8; MAX / 2],
    n: u8,
}

impl Heard {
    pub const NONE: Heard = Heard { d: [0; MAX / 2], n: 0 };

    pub fn len(&self) -> usize {
        self.n as usize
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    fn code(&self, i: usize) -> u8 {
        self.d[i / 2] >> (i % 2 * 4) & 15
    }

    /// Digit `i` as its character.
    pub fn char(&self, i: usize) -> u8 {
        SYMBOLS[self.code(i) as usize]
    }

    /// A code (0-15, REG_0B's) on the end; full, the oldest goes.
    pub fn push(&mut self, code: u8) {
        if self.len() == MAX {
            for i in 1..MAX {
                let c = self.code(i);
                self.set(i - 1, c);
            }
            self.n -= 1;
        }
        self.set(self.len(), code & 15);
        self.n += 1;
    }

    fn set(&mut self, i: usize, c: u8) {
        let sh = i % 2 * 4;
        self.d[i / 2] = self.d[i / 2] & !(15 << sh) | c << sh;
    }
}

/// A slot's digits and when the last came: the Radio task keeps one per slot and publishes [`Track::heard`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Track {
    pub heard: Heard,
    at: u32,
}

impl Track {
    pub const NEW: Track = Track { heard: Heard::NONE, at: 0 };

    /// A digit from the detector at `now` (ms).
    pub fn digit(&mut self, code: u8, now: u32) {
        if now.wrapping_sub(self.at) >= GAP_MS {
            self.heard = Heard::NONE;
        }
        self.heard.push(code);
        self.at = now;
    }

    /// Every tick: the string clears [`HOLD_MS`] after its last digit once the carrier is gone. True if it changed.
    pub fn tick(&mut self, now: u32, carrier: bool) -> bool {
        if self.heard.is_empty() || carrier || now.wrapping_sub(self.at) < HOLD_MS {
            return false;
        }
        self.heard = Heard::NONE;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(h: &Heard) -> [u8; MAX] {
        let mut b = [b' '; MAX];
        for (i, c) in b.iter_mut().enumerate().take(h.len()) {
            *c = h.char(i);
        }
        b
    }

    /// DEC-6.1: a PTT ID's digits in order, the detector's codes as characters.
    #[test]
    fn digits_in_order() {
        let mut t = Track::NEW;
        for (i, c) in [1, 0, 1, 15].into_iter().enumerate() {
            t.digit(c, 1000 + i as u32 * 100);
        }
        assert_eq!(&text(&t.heard)[..4], b"101#");
        assert_eq!(t.heard.len(), 4);
    }

    /// DEC-6.1: twelve kept, the newest; a digit after the gap starts again.
    #[test]
    fn newest_kept_and_a_gap_starts_again() {
        let mut t = Track::NEW;
        for i in 0..14u32 {
            t.digit((i % 10) as u8, 1000 + i * 100);
        }
        assert_eq!(&text(&t.heard), b"234567890123");
        t.digit(14, 2300 + GAP_MS);
        assert_eq!(&text(&t.heard)[..2], b"* ");
    }

    /// DEC-6.1: shown while the carrier stays, then HOLD_MS after the last digit once it has gone.
    #[test]
    fn held_then_cleared() {
        let mut t = Track::NEW;
        t.digit(5, 1000);
        assert!(!t.tick(1000 + HOLD_MS + 10, true), "the carrier is still there");
        assert!(!t.tick(1000 + HOLD_MS - 10, false));
        assert!(t.tick(1000 + HOLD_MS, false));
        assert!(t.heard.is_empty());
        assert!(!t.tick(1000 + HOLD_MS * 2, false), "nothing to clear");
    }
}
