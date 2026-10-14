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

/// TERM-32: how many PA0 samples `mic` takes, and how far apart (8 kHz: 32 ms in all).
pub const MIC_SAMPLES: usize = 256;
pub const MIC_PERIOD_US: u32 = 125;

/// TERM-32 (a bench test): a burst of PA0 readings summed up, to tell raw mic audio (it swings through its mean at
/// the tone's frequency) from a level or envelope (it sits). 12-bit counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MicCapture {
    pub min: u16,
    pub max: u16,
    pub mean: u16,
    /// Crossings of the mean, with a hysteresis of an eighth of the swing either side so noise doesn't count.
    pub crossings: u16,
    /// The time the burst took, microseconds.
    pub us: u32,
    /// The first readings, 12 bits shifted down to 8.
    pub first: [u8; 16],
}

impl MicCapture {
    pub fn from_samples(s: &[u16], us: u32) -> Self {
        let (mut min, mut max, mut sum) = (u16::MAX, 0u16, 0u32);
        for &v in s {
            (min, max, sum) = (min.min(v), max.max(v), sum + v as u32);
        }
        let mean = if s.is_empty() { 0 } else { (sum / s.len() as u32) as u16 };
        let band = max.saturating_sub(min) / 8;
        let (mut crossings, mut side) = (0u16, None);
        for &v in s {
            let now = if v > mean + band { Some(true) } else if v + band < mean { Some(false) } else { side };
            if side.is_some() && now != side {
                crossings += 1;
            }
            side = now;
        }
        let mut first = [0u8; 16];
        for (f, &v) in first.iter_mut().zip(s) {
            *f = (v >> 4) as u8;
        }
        MicCapture { min: if s.is_empty() { 0 } else { min }, max, mean, crossings, us, first }
    }

    /// The frequency the crossings would mean for raw audio (two per cycle), Hz.
    pub fn hz(&self) -> u32 {
        (self.crossings as u32 * 500_000).checked_div(self.us).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {

    /// TERM-32: a 1 kHz sine at 8 kHz gives two crossings a cycle (about 1000 Hz); a steady level, none.
    #[test]
    fn mic_capture_tells_audio_from_a_level() {
        let sine: [u16; 256] = core::array::from_fn(|i| (2048.0 + 600.0 * (i as f64 * core::f64::consts::TAU / 8.0 + 0.3).sin()) as u16);
        let c = MicCapture::from_samples(&sine, 32_000);
        assert!((970..=1010).contains(&c.hz()), "{}", c.hz()); // the first crossing has no side before it
        assert!(c.min < 1500 && c.max > 2600 && c.mean.abs_diff(2048) < 20);
        let level: [u16; 256] = core::array::from_fn(|i| 300 + (i % 3) as u16);
        let c = MicCapture::from_samples(&level, 32_000);
        assert!(c.crossings <= 2 * 3 && c.hz() < 100, "noise of a few counts mustn't read as a tone: {:?}", c);
        assert_eq!(MicCapture::from_samples(&[], 0).hz(), 0);
    }

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
