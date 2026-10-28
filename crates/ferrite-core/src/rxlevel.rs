//! PA2's level (decision 294): the clip counter (TERM-32.1, the readback's `audio` line) and the digital level stage
//! ahead of the PC stream's coder (LINK-13.8).
//!
//! The sources' own levels stay fixed (the SI4732's volume, the BK4829's REG_48): turning them live would make the
//! speaker's loudness wander, as both sit ahead of the volume knob. What the ADC sees is then scaled here, per
//! consumer: a gentle automatic gain for the PC stream; nothing for the APRS decoder; for FT8/FT4/JS8 later a gain
//! measured and fixed at each cycle's boundary ([`Agc::fix`]), never changed mid-cycle.

/// A raw 12-bit ADC count this close to either rail is counted as clipped (TERM-32.1).
pub const RAIL: u16 = 4;

/// Whether a raw ADC count is at a rail.
#[inline]
pub fn at_rail(adc: u16) -> bool {
    adc <= RAIL || adc >= 4095 - RAIL
}

/// Unity gain, Q8.
pub const UNITY: u16 = 256;
/// The most gain the stream gets: x16 (24 dB), enough to bring a BK4829's ~146 mV voice (a small slice of the ADC's
/// range) up to a comfortable level without lifting a quiet channel's hiss further.
pub const MAX_GAIN: u16 = 16 * UNITY;
/// Peaks are held under this after the gain (about -3 dB of full scale): the attack.
pub const PEAK_HI: i32 = 23_000;
/// The gain rises only while peaks stay under this (about -6 dB): the release's ceiling.
pub const PEAK_LO: i32 = 16_000;

/// The stream's automatic gain: Q8, updated once a chunk (the DSP task's 2 ms half, 48 samples at 24 kHz).
///
/// - Attack: before a chunk is scaled, the gain drops at once so its peak stays under [`PEAK_HI`] (it looks at the
///   chunk first, so nothing it passes clips).
/// - Release: while the squelch is open and peaks stay under [`PEAK_LO`], the gain rises by 1/512 a chunk, about
///   0.27 dB every 32 ms (8 dB a second): slow enough that voice between words doesn't pump.
/// - Hold: with the squelch shut nothing changes, so a channel's noise between calls never winds the gain up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Agc {
    gain: u16,
    fixed: bool,
}

impl Default for Agc {
    fn default() -> Self {
        Agc::new()
    }
}

impl Agc {
    pub const fn new() -> Self {
        Agc { gain: UNITY, fixed: false }
    }

    /// The gain now, Q8.
    pub fn gain(&self) -> u16 {
        self.gain
    }

    /// A fixed gain (Q8) that [`Agc::chunk`] leaves alone, or back to automatic with `None`. The hook for the
    /// weak-signal modes: measured, then set at a cycle's boundary and held through the cycle.
    pub fn fix(&mut self, g: Option<u16>) {
        match g {
            Some(g) => (self.gain, self.fixed) = (g.clamp(1, MAX_GAIN), true),
            None => self.fixed = false,
        }
    }

    /// Scales `x` in place: the gain set for this chunk first (attack), the samples scaled, then the release.
    /// `open`: a squelch is open (the stream sends this audio).
    pub fn chunk(&mut self, x: &mut [i16], open: bool) {
        let peak = x.iter().map(|&v| (v as i32).abs()).max().unwrap_or(0);
        if !self.fixed && peak > 0 && peak * self.gain as i32 > PEAK_HI * UNITY as i32 {
            self.gain = ((PEAK_HI * UNITY as i32) / peak).clamp(1, MAX_GAIN as i32) as u16;
        }
        let g = self.gain as i32;
        for v in x.iter_mut() {
            *v = ((*v as i32 * g) >> 8).clamp(-32768, 32767) as i16;
        }
        if !self.fixed && open && peak * g < PEAK_LO * UNITY as i32 {
            self.gain = (self.gain + (self.gain >> 9).max(1)).min(MAX_GAIN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amp: i16) -> [i16; 48] {
        core::array::from_fn(|i| if i % 8 < 4 { amp } else { -amp })
    }

    /// TERM-32.1: counts within RAIL of either end are clipped.
    #[test]
    fn rails() {
        assert!(at_rail(0) && at_rail(4) && at_rail(4091) && at_rail(4095));
        assert!(!at_rail(5) && !at_rail(2048) && !at_rail(4090));
    }

    /// LINK-13.8: a quiet voice (a BK4829's level, about a tenth of full scale) is brought up while the squelch is
    /// open, to near PEAK_LO, never past PEAK_HI, and no further than MAX_GAIN.
    #[test]
    fn quiet_voice_comes_up() {
        let mut a = Agc::new();
        let mut out = 0;
        for _ in 0..2000 {
            let mut x = tone(2_000);
            a.chunk(&mut x, true);
            out = x.iter().map(|&v| (v as i32).abs()).max().unwrap();
        }
        assert!(out > PEAK_LO / 2 && out <= PEAK_HI, "settled at {out}");
        let mut b = Agc::new();
        for _ in 0..5000 {
            b.chunk(&mut tone(50), true);
        }
        assert_eq!(b.gain(), MAX_GAIN, "a whisper stops at the cap");
    }

    /// LINK-13.8: a loud chunk is turned down before it's scaled, so nothing the stage passes clips.
    #[test]
    fn attack_before_scaling() {
        let mut a = Agc::new();
        a.fix(Some(MAX_GAIN));
        a.fix(None);
        let mut x = tone(20_000);
        a.chunk(&mut x, true);
        assert!(x.iter().all(|&v| (v as i32).abs() <= PEAK_HI), "{:?}", &x[..4]);
    }

    /// LINK-13.8: with the squelch shut the gain holds: noise between calls doesn't wind it up.
    #[test]
    fn holds_while_shut() {
        let mut a = Agc::new();
        for _ in 0..1000 {
            a.chunk(&mut tone(500), false);
        }
        assert_eq!(a.gain(), UNITY);
    }

    /// LINK-13.8: the weak-signal hook: a fixed gain isn't changed by any chunk, loud or quiet, until released.
    #[test]
    fn fixed_gain_holds() {
        let mut a = Agc::new();
        a.fix(Some(3 * UNITY));
        a.chunk(&mut tone(20_000), true);
        a.chunk(&mut tone(10), true);
        assert_eq!(a.gain(), 3 * UNITY);
        a.fix(None);
        a.chunk(&mut tone(10), true);
        assert!(a.gain() > 3 * UNITY);
    }

    #[test]
    fn default_is_new() {
        assert_eq!(Agc::default(), Agc::new());
        assert_eq!(Agc::default().gain(), Agc::new().gain());
    }
}
