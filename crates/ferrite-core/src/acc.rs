//! The side connector's speaker mic (manual chapter 32, SND-10, SND-12): stock's debounced accessory detect on PC15
//! (input, pull-up, low = a speaker mic grounds the connector's mute pin). Stock V0.27 (`analysis/pin_audit.md`,
//! `analysis/audio_path.md`):
//! - at boot (0x0801BE54) PC15 read low twice 50 ms apart: connected;
//! - then a poll (0x08010290) in one of the main loop's ten 1 ms slots, so every 10 ms: a reading that disagrees
//!   with the state counts up, one that agrees clears the count, and the fourth disagreeing reading in a row
//!   changes the state (40 ms). Connected: flag 0x2000A3B4[0x4B] set, PE1 and PB8 set. Removed: flag and PE1
//!   cleared (PB8 is left to the audio arbiter).
//!
//! The firmware's input task polls it with the keys; the Radio task switches PE1 and holds PB8.

/// How often the detect is read (stock's main loop slot).
pub const POLL_MS: u32 = 10;
/// Readings in a row against the state that change it (stock: the count reaches 3, the next one changes it).
pub const POLLS: u8 = 4;
/// The boot check's second reading comes this many polls after the first (stock: 50 ms apart).
pub const BOOT_POLLS: u8 = 5;

/// The debounced detect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccDet {
    on: bool,
    /// Readings in a row that disagree with `on`.
    n: u8,
    /// Polls taken so far, up to [`BOOT_POLLS`]: the boot check runs until then.
    boot: u8,
    /// The boot check's first reading.
    first: bool,
}

impl AccDet {
    pub const fn new() -> Self {
        AccDet { on: false, n: 0, boot: 0, first: false }
    }

    /// One reading every [`POLL_MS`] from start-up (`low`: PC15 reads low). The first and the one [`BOOT_POLLS`]
    /// later are stock's boot check: both low, connected. Then stock's debounce. Returns the new state when it
    /// changes.
    pub fn poll(&mut self, low: bool) -> Option<bool> {
        if self.boot < BOOT_POLLS {
            if self.boot == 0 {
                self.first = low;
            }
            self.boot += 1;
            return None;
        }
        if self.boot == BOOT_POLLS {
            self.boot += 1;
            self.on = self.first && low;
            return self.on.then_some(true);
        }
        if low == self.on {
            self.n = 0;
            return None;
        }
        self.n += 1;
        if self.n < POLLS {
            return None;
        }
        (self.on, self.n) = (low, 0);
        Some(low)
    }

    /// A speaker mic is connected.
    pub fn on(&self) -> bool {
        self.on
    }

    /// A change is being debounced, or the boot check runs: VOX doesn't key meanwhile (VOX-3).
    pub fn changing(&self) -> bool {
        self.n != 0 || self.boot <= BOOT_POLLS
    }

    /// The state for the other tasks in one byte: bit 0 connected, bit 1 changing.
    pub fn pack(&self) -> u8 {
        self.on as u8 | (self.changing() as u8) << 1
    }
}

/// [`AccDet::pack`]'s bits.
pub const ON: u8 = 1;
pub const CHANGING: u8 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    /// After the boot check: connected or not.
    fn booted(first: bool, second: bool) -> (AccDet, Option<bool>) {
        let mut d = AccDet::new();
        assert_eq!(d.poll(first), None);
        for _ in 1..BOOT_POLLS {
            assert!(d.changing());
            assert_eq!(d.poll(!second), None, "between the two readings nothing counts");
        }
        let r = d.poll(second);
        (d, r)
    }

    /// SND-12.1: the boot check, two readings 50 ms apart, needs both low.
    #[test]
    fn boot_check() {
        let (d, r) = booted(true, true);
        assert!(d.on() && !d.changing());
        assert_eq!(r, Some(true));
        for (a, b) in [(true, false), (false, true), (false, false)] {
            let (d, r) = booted(a, b);
            assert!(!d.on() && !d.changing());
            assert_eq!(r, None, "nothing to tell: the Radio task starts with nothing connected");
        }
        assert_eq!(BOOT_POLLS as u32 * POLL_MS, 50);
    }

    /// SND-12.1: four low readings in a row connect it, four high ones remove it (40 ms at 10 ms polls), as stock's
    /// count; VOX-3.4: meanwhile it is changing.
    #[test]
    fn four_polls_change_it() {
        let (mut d, _) = booted(false, false);
        for _ in 0..3 {
            assert_eq!(d.poll(true), None);
            assert!(d.changing());
            assert_eq!(d.pack(), CHANGING);
        }
        assert_eq!(d.poll(true), Some(true));
        assert!(d.on() && !d.changing());
        assert_eq!(d.pack(), ON);
        assert_eq!(d.poll(true), None, "no change, no news");
        for _ in 0..3 {
            assert_eq!(d.poll(false), None);
        }
        assert_eq!(d.pack(), ON | CHANGING);
        assert_eq!(d.poll(false), Some(false));
        assert_eq!(d.pack(), 0);
    }

    /// SND-12.1: a bounce (a reading back at the state) starts the count again, as stock clears it.
    #[test]
    fn a_bounce_starts_again() {
        let (mut d, _) = booted(false, false);
        for _ in 0..3 {
            d.poll(true);
        }
        assert_eq!(d.poll(false), None);
        assert!(!d.changing(), "the agreeing reading cleared the count");
        for _ in 0..3 {
            assert_eq!(d.poll(true), None);
        }
        assert_eq!(d.poll(true), Some(true));
        // Contact noise while plugged: never four highs in a row, never removed.
        for i in 0..100 {
            assert_eq!(d.poll(i % 3 == 0), None);
        }
        assert!(d.on());
    }
}
