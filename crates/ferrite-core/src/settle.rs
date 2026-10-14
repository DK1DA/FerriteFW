//! PWR-21 (decision 119): the BK4829's settle waits around its power changes, a deliberate change from stock, which
//! writes them back to back. On the black radio (Oct 13, 2026) each watch hop that swapped chips and each Battery
//! Save phase clicked in the shared speaker amp while FM broadcast played, but the same writes typed seconds apart
//! by hand did not: the guess is that the chip's AF output glitches when receive comes on before the LDOs,
//! crystal and band-gap have settled after REG_37's wake, or when REG_37 drops right after REG_30 goes off.
//!
//! - Wake: REG_37's supplies on, then at least [`Settle::wake_ms`] before the rest of the receive set-up and
//!   REG_30's receive on. The Radio task turns on the supplies of the chip the hop turns to a tick early
//!   (`watch::Hop::due`), so the wait overlaps the current dwell and the hop's timing is unchanged.
//! - Sleep: the AF route muted and REG_30 off, then [`Settle::sleep_ms`], then REG_37's supplies off.
//!
//! Live-tunable from the terminal (`bk settle`, TERM-31), RAM only; 0 is stock's behaviour. The defaults are a
//! starting point for the bench to tune.

/// The longest settle the terminal accepts, either way.
pub const MAX_MS: u8 = 20;

/// The two waits, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settle {
    /// REG_37 on to the rest of the receive set-up.
    pub wake_ms: u8,
    /// REG_30 off to REG_37 off.
    pub sleep_ms: u8,
}

impl Settle {
    /// At boot (to be tuned on the bench).
    pub const DEFAULT: Settle = Settle { wake_ms: 2, sleep_ms: 1 };
    /// Stock's: no wait either way.
    pub const STOCK: Settle = Settle { wake_ms: 0, sleep_ms: 0 };

    /// Both waits, if each is within [`MAX_MS`].
    pub fn new(wake_ms: u8, sleep_ms: u8) -> Option<Settle> {
        (wake_ms <= MAX_MS && sleep_ms <= MAX_MS).then_some(Settle { wake_ms, sleep_ms })
    }

    /// What is left of the wake settle at `now` (ms) for a chip whose supplies came on at `woke`: 0 once it has
    /// passed, so a chip woken early (or long awake) is tuned at once.
    pub fn wake_left(&self, woke: u32, now: u32) -> u32 {
        (self.wake_ms as u32).saturating_sub(now.wrapping_sub(woke))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_left_counts_down_from_the_wake() {
        let s = Settle::DEFAULT;
        assert_eq!(s.wake_left(100, 100), 2, "just woken: the whole wait");
        assert_eq!(s.wake_left(100, 101), 1);
        assert_eq!(s.wake_left(100, 102), 0);
        assert_eq!(s.wake_left(100, 5_000), 0, "long awake: none");
        assert_eq!(s.wake_left(u32::MAX, 0), 1, "across the millisecond counter's wrap");
        assert_eq!(Settle::STOCK.wake_left(100, 100), 0, "0: stock's, no wait");
    }

    #[test]
    fn terminal_range() {
        assert_eq!(Settle::new(0, 0), Some(Settle::STOCK));
        assert_eq!(Settle::new(20, 20), Some(Settle { wake_ms: 20, sleep_ms: 20 }));
        assert_eq!(Settle::new(21, 0), None);
        assert_eq!(Settle::new(0, 21), None);
    }
}
