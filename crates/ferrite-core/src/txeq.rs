//! TERM-33.16 (decision 308): the transmit pre-equalisers. DAC1's samples reach the modulator through the BK4829's
//! mic input; with every AF TX filter bypassed (REG_47<0>, `txtest ... bypass`) the injection network leaves about
//! +9 dB at 100 Hz, 0 at 1 kHz, -16.5 at 5 kHz and -29 at 8 kHz (bench, black radio, Oct 27, 2026). These two
//! linear-phase FIRs at [`crate::audio::DAC_HZ`] undo that in the samples, so every tone goes out at the deviation the plain 1 kHz
//! tone gets: [`Eq::Eq`] flat 100 Hz-5 kHz, [`Eq::Eq8`] flat 100 Hz-8 kHz (a wide-channel set-up: Carson puts audio
//! to 8 kHz at 3 kHz deviation at about 22 kHz of RF).
//!
//! Designed offline by `tools/bench-blade/txeq_design.py` (weighted least squares on the smoothed measured curve,
//! reweighted towards minimax over the measured points); its predicted residual, measured points x EQ: `eq` -0.4 to
//! +0.9 dB from 100 Hz to 5 kHz, `eq8` -0.5 to +1.2 dB from 100 Hz to 8 kHz, both but for the 630-800 Hz points
//! (+1.2 to +2.7: the measured curve's step between 800 Hz and 1 kHz, which no smooth response follows; the bench
//! sweep settles whether it's real).
//!
//! **Headroom.** The boost would take the loudest test signals past the DAC's full scale (`fsk 9600` at 150 % to
//! 55000 through `eq`; the sweep's 8 kHz tone to 165000 through `eq8`), so the taps carry a back-off
//! ([`Eq::back_off_cdb`]: 5.0 dB for `eq`, 14.0 dB for `eq8`) and the key-up wins it back in the BK4829's gain
//! stages ([`Eq::mic_steps`]: REG_7D's mic sensitivity, 0.5 dB a step, from the data key-up's 0x34; then
//! [`Eq::deviation`]: REG_40's deviation field, from 0x50A), so the equalised 1 kHz tone is at the plain one's
//! deviation. Both stages are taken as the register tables describe them (linear); `txtest sweep data bypass eq`
//! against `txtest sweep data bypass` compares the two 1 kHz levels on the air. The boost itself is digital, ahead
//! of the DAC: the DAC's and the injection's noise floors are not boosted by it, only raised by the back-off won back
//! (5 and 14 dB: the 1 kHz tone stays 58 and 46 dB over the 12-bit floor in band).
//!
//! **Cost.** Symmetric taps: 40 (`eq`) or 48 (`eq8`) multiply-accumulates a sample, 1.9 or 2.3 million a second at
//! 48 kHz; a 190-sample delay line (380 bytes) wherever the [`Fir`] lives. The taps are named statics and the filter
//! is core's code: both in the fast flash (firmware/memory.x).

#[cfg(test)]
use crate::audio::DAC_HZ;

/// Which equaliser, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eq {
    Off,
    /// Flat 100 Hz-5 kHz, the boost rolling off above (none past about 6.5 kHz).
    Eq,
    /// Flat 100 Hz-8 kHz, rolling off above (none past about 10 kHz).
    Eq8,
}

/// The longest filter's taps (`eq8`'s).
pub const MAX_TAPS: usize = 95;

/// `eq`: 79 taps, Q15, the 5.0 dB back-off folded in (1 kHz at -5.0 dB).
pub static EQ_TAPS: [i16; 79] = [
    27, 65, 98, 93, 30, -82, -191, -222, -118, 111, 370, 506, 381, -31, -600, -1059, -1122, -638, 285, 1266, 1788, 1450, 216, -1471, -2815, -3010,
    -1687, 782, 3309, 4503, 3347, -190, -4928, -8831, -9853, -6917, -545, 7194, 13471, 16482, 13471, 7194, -545, -6917, -9853, -8831, -4928, -190,
    3347, 4503, 3309, 782, -1687, -3010, -2815, -1471, 216, 1450, 1788, 1266, 285, -638, -1122, -1059, -600, -31, 381, 506, 370, 111, -118, -222,
    -191, -82, 30, 93, 98, 65, 27,
];

/// `eq8`: 95 taps, Q15, the 14.0 dB back-off folded in (1 kHz at -14.0 dB).
pub static EQ8_TAPS: [i16; 95] = [
    4, 21, 57, 99, 107, 40, -100, -235, -250, -85, 187, 368, 266, -110, -475, -448, 65, 644, 651, -98, -951, -933, 172, 1328, 1162, -452, -1913,
    -1412, 899, 2622, 1510, -1743, -3655, -1572, 2878, 4768, 1123, -4974, -6682, -785, 7557, 8919, -393, -12850, -15741, -3858, 14044, 22752,
    14044, -3858, -15741, -12850, -393, 8919, 7557, -785, -6682, -4974, 1123, 4768, 2878, -1572, -3655, -1743, 1510, 2622, 899, -1412, -1913,
    -452, 1162, 1328, 172, -933, -951, -98, 651, 644, 65, -448, -475, -110, 266, 368, 187, -85, -250, -235, -100, 40, 107, 99, 57, 21, 4,
];

/// The taps' scale: Q15.
pub const SHIFT: u32 = 15;

impl Eq {
    /// The terminal's word for it.
    pub fn name(self) -> &'static str {
        match self {
            Eq::Off => "",
            Eq::Eq => "eq",
            Eq::Eq8 => "eq8",
        }
    }

    pub fn parse(w: &[u8]) -> Option<Eq> {
        [Eq::Eq, Eq::Eq8].into_iter().find(|e| e.name().as_bytes().eq_ignore_ascii_case(w))
    }

    pub fn taps(self) -> &'static [i16] {
        match self {
            Eq::Off => &[],
            Eq::Eq => &EQ_TAPS,
            Eq::Eq8 => &EQ8_TAPS,
        }
    }

    /// The flat band's top, Hz.
    pub fn top_hz(self) -> u32 {
        match self {
            Eq::Off => 0,
            Eq::Eq => 5000,
            Eq::Eq8 => 8000,
        }
    }

    /// The back-off folded into the taps, hundredths of a dB.
    pub fn back_off_cdb(self) -> u16 {
        match self {
            Eq::Off => 0,
            Eq::Eq => 500,
            Eq::Eq8 => 1404,
        }
    }

    /// REG_7D[5:0] steps (0.5 dB each) above the data key-up's mic sensitivity, winning the back-off back.
    pub fn mic_steps(self) -> u8 {
        match self {
            Eq::Off => 0,
            Eq::Eq => 10,
            Eq::Eq8 => 11,
        }
    }

    /// REG_40[11:0] for the key-up (the key-up's own is 0x50A), winning back what the mic steps don't.
    pub fn deviation(self) -> u16 {
        match self {
            Eq::Off | Eq::Eq => 0x50A,
            Eq::Eq8 => 0xD77,
        }
    }
}

/// The filter's delay line: each sample written twice (at `at` and `at + n`, `at` counting down), so the last `n`
/// are always one contiguous run, newest first, at `at..at + n`.
#[derive(Clone, Copy, Debug)]
pub struct Fir {
    buf: [i16; 2 * MAX_TAPS],
    at: u8,
    eq: Eq,
}

impl Fir {
    pub const fn new(eq: Eq) -> Self {
        Fir { buf: [0; 2 * MAX_TAPS], at: 0, eq }
    }

    pub fn eq(&self) -> Eq {
        self.eq
    }

    /// One sample through: `x` itself when off. Saturates at the DAC's full scale (the back-off keeps the test
    /// signals inside it; louder input clips rather than wrapping).
    #[inline]
    pub fn run(&mut self, x: i16) -> i16 {
        if self.eq == Eq::Off {
            return x;
        }
        self.step(x)
    }

    #[inline(never)]
    fn step(&mut self, x: i16) -> i16 {
        let h = self.eq.taps();
        let n = h.len();
        let at = self.at as usize;
        self.buf[at] = x;
        self.buf[at + n] = x;
        let w = &self.buf[at..at + n];
        let c = n / 2;
        // Symmetric taps: h[k] = h[n - 1 - k], so pairs share a multiply.
        let mut acc = h[c] as i64 * w[c] as i64;
        for k in 0..c {
            acc += h[k] as i64 * (w[k] as i32 + w[n - 1 - k] as i32) as i64;
        }
        self.at = if at == 0 { (n - 1) as u8 } else { (at - 1) as u8 };
        (acc >> SHIFT).clamp(-32767, 32767) as i16
    }
}

/// The filter's gain at `hz`, from its taps (host tests and the terminal's notes).
#[cfg(test)]
fn gain_db(eq: Eq, hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (k, &t) in eq.taps().iter().enumerate() {
        let a = -2.0 * core::f64::consts::PI * hz * k as f64 / DAC_HZ as f64;
        re += t as f64 * a.cos();
        im += t as f64 * a.sin();
    }
    20.0 * ((re * re + im * im).sqrt() / (1 << SHIFT) as f64).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// TERM-33.16: both filters are symmetric (linear phase); with the back-off won back by the gain stages
    /// (the mic steps and REG_40) the 1 kHz gain is 0 dB; DC is about -9 dB (the low end's +9 pulled down); the
    /// top of each band is boosted by what the measured curve lost there (+17.3 dB at 5 kHz, +28.6 at 8 kHz), and
    /// the boost falls away above the band.
    #[test]
    fn gains() {
        let won = |e: Eq| e.mic_steps() as f64 * 0.5 + 20.0 * (e.deviation() as f64 / 0x50A as f64).log10();
        for e in [Eq::Eq, Eq::Eq8] {
            let h = e.taps();
            assert!(h.len() % 2 == 1 && h.len() <= MAX_TAPS);
            for k in 0..h.len() {
                assert_eq!(h[k], h[h.len() - 1 - k], "{e:?} tap {k}");
            }
            assert!((won(e) * 100.0 - e.back_off_cdb() as f64).abs() < 5.0, "{e:?}: the stages give back {:.2} dB", won(e));
            assert!(0x34 + e.mic_steps() <= 0x3F && e.deviation() <= 0xFFF);
            let g = |hz: f64| gain_db(e, hz) + won(e);
            assert!(g(1000.0).abs() < 0.05, "{e:?} 1 kHz: {:.2}", g(1000.0));
            assert!((g(0.0) + 9.0).abs() < 0.6, "{e:?} DC: {:.2}", g(0.0));
        }
        let g = |e: Eq, hz: f64| gain_db(e, hz) + e.mic_steps() as f64 * 0.5 + 20.0 * (e.deviation() as f64 / 0x50A as f64).log10();
        assert!((g(Eq::Eq, 5000.0) - 17.3).abs() < 0.6, "eq 5 kHz: {:.2}", g(Eq::Eq, 5000.0));
        assert!(g(Eq::Eq, 7500.0) < -10.0 && g(Eq::Eq, 12000.0) < -20.0);
        assert!((g(Eq::Eq8, 8000.0) - 28.6).abs() < 0.6, "eq8 8 kHz: {:.2}", g(Eq::Eq8, 8000.0));
        assert!(g(Eq::Eq8, 11000.0) < -10.0 && g(Eq::Eq8, 16000.0) < -20.0);
    }

    /// TERM-33.16: the predicted residual, the measured bypass curve times the equaliser (both bands, but for the
    /// 630-800 Hz step): within -0.6 to +1.3 dB of the plain 1 kHz tone.
    #[test]
    fn residual() {
        let meas = [
            (100.0, 9.7), (125.0, 9.6), (160.0, 9.1), (200.0, 8.5), (250.0, 7.9), (315.0, 7.2), (400.0, 6.4), (500.0, 5.7), (1000.0, 0.0),
            (1250.0, -1.9), (1600.0, -4.0), (2000.0, -6.1), (2500.0, -8.2), (3150.0, -10.6), (4000.0, -13.8), (5000.0, -16.5), (6300.0, -21.4),
            (8000.0, -29.3),
        ];
        for e in [Eq::Eq, Eq::Eq8] {
            let won = e.back_off_cdb() as f64 / 100.0;
            for &(hz, m) in meas.iter().filter(|p| p.0 <= e.top_hz() as f64) {
                let r = m + gain_db(e, hz) + won;
                assert!((-0.6..=1.3).contains(&r), "{e:?} at {hz} Hz: {r:+.2} dB");
            }
        }
    }

    /// TERM-33.16: the running filter is the taps' convolution (an impulse comes out as the taps, scaled), off
    /// passes samples through, and a full-scale input saturates instead of wrapping.
    #[test]
    fn runs() {
        for e in [Eq::Eq, Eq::Eq8] {
            let mut f = Fir::new(e);
            let n = e.taps().len();
            let y: Vec<i16> = (0..n + 5).map(|i| f.run(if i == 0 { 1 << 14 } else { 0 })).collect();
            for (k, (&got, &t)) in y.iter().zip(e.taps()).enumerate() {
                assert!((got as i32 - (t as i32 >> 1)).abs() <= 1, "{e:?} {k}: {got} vs {}", t >> 1);
            }
            assert!(y[n..].iter().all(|&v| v == 0));
            // A 1 kHz sine through it: the gain at 1 kHz (the back-off), after the filter fills.
            let mut f = Fir::new(e);
            let x: Vec<i16> = (0..4800).map(|i| (10000.0 * (2.0 * core::f64::consts::PI * 1000.0 * i as f64 / DAC_HZ as f64).sin()) as i16).collect();
            let peak = x.iter().map(|&v| f.run(v)).skip(n).map(|v| v.unsigned_abs()).max().unwrap() as f64;
            let want = 10000.0 * 10f64.powf(-(e.back_off_cdb() as f64) / 2000.0);
            assert!((peak - want).abs() < want * 0.02 + 2.0, "{e:?}: {peak} vs {want}");
            let mut f = Fir::new(e);
            assert!((0..400).map(|i| f.run(if i % 4 < 2 { 32767 } else { -32767 })).all(|v| v.unsigned_abs() <= 32767));
        }
        let mut f = Fir::new(Eq::Off);
        assert_eq!([f.run(5), f.run(-7)], [5, -7]);
        assert_eq!((Eq::parse(b"EQ8"), Eq::parse(b"eq"), Eq::parse(b"eq5")), (Some(Eq::Eq8), Some(Eq::Eq), None));
    }
}
