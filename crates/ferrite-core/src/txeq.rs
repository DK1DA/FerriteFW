//! TERM-33.16 (decisions 308, 310): the transmit pre-equalisers. DAC1's samples reach the modulator through the
//! BK4829's mic input; with every AF TX filter bypassed and the soft limiter off (`txtest ... bypass`, TERM-33.15)
//! the injection network leaves +14.5 dB at 100 Hz, 0 at 1 kHz, -16.6 at 5 kHz and -30 at 8 kHz, and the whole
//! band 6.8 dB under the plain `data` set-up's 1 kHz tone (bench, black radio, Oct 27, 2026). These two linear-phase
//! FIRs at [`crate::audio::DAC_HZ`] undo both, so every tone goes out at the deviation the plain `txtest sweep data`
//! 1 kHz tone gets: [`Eq::Eq`] flat 100 Hz-5 kHz, [`Eq::Eq8`] flat 100 Hz-8 kHz (a wide-channel set-up: Carson puts
//! audio to 8 kHz at 3 kHz deviation at about 22 kHz of RF).
//!
//! Designed offline by `tools/bench-blade/txeq_design.py` (weighted least squares on the smoothed measured curve,
//! reweighted towards minimax over the measured points); predicted, each tone against the plain 1 kHz tone: `eq`
//! -0.9 to +0.2 dB from 100 Hz to 5 kHz, `eq8` -0.9 to +0.5 dB from 100 Hz to 8 kHz.
//!
//! **Level.** The 6.8 dB comes from the gain stages left after the bypass and the DAC's swing: REG_7D's mic
//! sensitivity at its top, 0x3F ([`Eq::mic_steps`]: 11 steps of 0.5 dB over the data key-up's 0x34; measured
//! linear under the bypass, decision 310), `eq8` also REG_40 ([`Eq::deviation`]: 0x57A, +2 dB by the power law the
//! tone generator follows, deviation ~ REG_40^2.8: extrapolated a little past the measured 0x50A), and the taps the
//! rest ([`Eq::gain_1k_cdb`]: `eq` +1.35 dB, `eq8` -0.65 dB at 1 kHz). Decision 308's REG_40 0xD77 for `eq8` was
//! about +24 dB by that law, not the +8.5 assumed: the signal went far past the capture's band.
//!
//! **Headroom.** The DAC has 18.3 dB over the plain sweep tone's 4000. At LEVEL 100 everything fits but unshaped
//! `fsk` above about 7 kHz baud (use `rc`) and, through `eq8`, the sweep's tones above 3 kHz (`eq8` sweeps fit up to
//! LEVEL 30: measure its shape there); above 100 the boosted tops saturate (the filter clips, never wraps). The boost
//! is digital, ahead of the DAC: the DAC's and the injection's noise floors aren't boosted by it.
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

/// `eq`: 79 taps, Q15 ([`Eq::shift`]), +1.35 dB at 1 kHz.
pub static EQ_TAPS: [i16; 79] = [
    -10, -48, -138, -286, -459, -575, -532, -259, 218, 750, 1094, 1010, 395, -620, -1663, -2238,
    -1948, -717, 1075, 2669, 3202, 2130, -414, -3505, -5704, -5683, -2949, 1681, 6186, 8109, 5734, -860,
    -9455, -16361, -17933, -12304, -494, 13711, 25183, 30744, 25183, 13711, -494, -12304, -17933, -16361, -9455, -860,
    5734, 8109, 6186, 1681, -2949, -5683, -5704, -3505, -414, 2130, 3202, 2669, 1075, -717, -1948, -2238,
    -1663, -620, 395, 1010, 1094, 750, 218, -259, -532, -575, -459, -286, -138, -48, -10,
];

/// `eq8`: 95 taps, Q13, -0.65 dB at 1 kHz.
pub static EQ8_TAPS: [i16; 95] = [
    -24, -83, -144, -122, 32, 239, 303, 95, -262, -429, -169, 336, 566, 170, -559, -845,
    -227, 791, 1092, 140, -1214, -1426, 23, 1748, 1708, -444, -2561, -2072, 972, 3396, 2099, -2144,
    -4791, -2258, 3503, 6159, 1724, -6081, -8599, -1522, 9003, 11293, 452, -14676, -18688, -5030, 16028, 26308,
    16028, -5030, -18688, -14676, 452, 11293, 9003, -1522, -8599, -6081, 1724, 6159, 3503, -2258, -4791, -2144,
    2099, 3396, 972, -2072, -2561, -444, 1708, 1748, 23, -1426, -1214, 140, 1092, 791, -227, -845,
    -559, 170, 566, 336, -169, -429, -262, 95, 303, 239, 32, -122, -144, -83, -24,
];

/// The bypass's loss at 1 kHz against the plain `data` sweep tone (1385 against 3046 Hz at the same DAC level),
/// hundredths of a dB: what each equaliser's taps and gain stages make up together.
pub const BYPASS_LOSS_CDB: i16 = 684;

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

    /// The taps' scale: `eq` Q15, `eq8` Q13 (its centre tap needs the room).
    pub fn shift(self) -> u32 {
        match self {
            Eq::Eq8 => 13,
            _ => 15,
        }
    }

    /// The taps' gain at 1 kHz, hundredths of a dB (with [`Eq::mic_steps`] and [`Eq::deviation`]: the bypass's
    /// loss, [`BYPASS_LOSS_CDB`]).
    pub fn gain_1k_cdb(self) -> i16 {
        match self {
            Eq::Off => 0,
            Eq::Eq => 135,
            Eq::Eq8 => -65,
        }
    }

    /// REG_7D[5:0] steps (0.5 dB each) above the data key-up's mic sensitivity (0x34): to the top, 0x3F.
    pub fn mic_steps(self) -> u8 {
        match self {
            Eq::Off => 0,
            Eq::Eq | Eq::Eq8 => 11,
        }
    }

    /// REG_40[11:0] for the key-up (the key-up's own is 0x50A): `eq8` 0x57A, +2.0 dB by the power law.
    pub fn deviation(self) -> u16 {
        match self {
            Eq::Off | Eq::Eq => 0x50A,
            Eq::Eq8 => 0x57A,
        }
    }

    /// The gain stages' dB, hundredths: the mic steps and REG_40 by the power law (exponent 2.76, the tone
    /// generator's, Oct 13).
    #[cfg(test)]
    fn stages_cdb(self) -> f64 {
        self.mic_steps() as f64 * 50.0 + 2000.0 * 2.76 * (self.deviation() as f64 / 0x50A as f64).log10()
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

    /// One sample through: `x` itself when off. Saturates at the DAC's full scale (louder than the DAC holds clips
    /// rather than wrapping).
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
        (acc >> self.eq.shift()).clamp(-32767, 32767) as i16
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
    20.0 * ((re * re + im * im).sqrt() / (1u32 << eq.shift()) as f64).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// TERM-33.16: both filters are symmetric (linear phase); the taps at 1 kHz and the gain stages make up the
    /// bypass's 6.84 dB loss together, REG_7D at its top and REG_40 never past +2 dB by the power law; DC is about
    /// -15 dB under 1 kHz (the low end's +14.5 pulled down); the top of each band is boosted by what the measured
    /// curve lost there (+17.0 dB at 5 kHz, +29.6 at 8 kHz against 1 kHz), and the boost falls away above the band.
    #[test]
    fn gains() {
        for e in [Eq::Eq, Eq::Eq8] {
            let h = e.taps();
            assert!(h.len() % 2 == 1 && h.len() <= MAX_TAPS);
            for k in 0..h.len() {
                assert_eq!(h[k], h[h.len() - 1 - k], "{e:?} tap {k}");
            }
            assert!((gain_db(e, 1000.0) * 100.0 - e.gain_1k_cdb() as f64).abs() < 3.0, "{e:?} 1 kHz: {:.2}", gain_db(e, 1000.0));
            assert!((e.gain_1k_cdb() as f64 + e.stages_cdb() - BYPASS_LOSS_CDB as f64).abs() < 10.0, "{e:?}: {:.0}", e.stages_cdb());
            assert!(0x34 + e.mic_steps() == 0x3F && e.stages_cdb() - 550.0 <= 210.0);
            let g = |hz: f64| gain_db(e, hz) - gain_db(e, 1000.0);
            assert!((g(0.0) + 15.2).abs() < 0.6, "{e:?} DC: {:.2}", g(0.0));
        }
        let g = |e: Eq, hz: f64| gain_db(e, hz) - gain_db(e, 1000.0);
        assert!((g(Eq::Eq, 5000.0) - 17.0).abs() < 0.7, "eq 5 kHz: {:.2}", g(Eq::Eq, 5000.0));
        assert!(g(Eq::Eq, 7500.0) < -10.0 && g(Eq::Eq, 12000.0) < -20.0);
        assert!((g(Eq::Eq8, 8000.0) - 29.6).abs() < 0.7, "eq8 8 kHz: {:.2}", g(Eq::Eq8, 8000.0));
        assert!(g(Eq::Eq8, 11000.0) < -10.0 && g(Eq::Eq8, 16000.0) < -20.0);
    }

    /// TERM-33.16: the predicted level of each tone against the plain `data` 1 kHz tone: the measured bypass curve
    /// (limiter off), the bypass's loss, the taps and the gain stages: within -1.0 to +0.6 dB over each band.
    #[test]
    fn residual() {
        let meas = [
            (100.0, 14.4), (125.0, 13.9), (160.0, 12.9), (200.0, 11.8), (250.0, 10.5), (315.0, 8.7), (400.0, 7.0), (500.0, 5.4), (630.0, 3.7),
            (800.0, 1.8), (1000.0, 0.0), (1250.0, -1.9), (1600.0, -4.0), (2000.0, -6.1), (2500.0, -8.2), (3150.0, -10.6), (4000.0, -13.4),
            (5000.0, -16.6), (6300.0, -21.4), (8000.0, -30.2),
        ];
        for e in [Eq::Eq, Eq::Eq8] {
            for &(hz, m) in meas.iter().filter(|p| p.0 <= e.top_hz() as f64) {
                let r = m - BYPASS_LOSS_CDB as f64 / 100.0 + gain_db(e, hz) + e.stages_cdb() / 100.0;
                assert!((-1.0..=0.6).contains(&r), "{e:?} at {hz} Hz: {r:+.2} dB");
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
            let y: Vec<i16> = (0..n + 5).map(|i| f.run(if i == 0 { 1 << (e.shift() - 1) } else { 0 })).collect();
            for (k, (&got, &t)) in y.iter().zip(e.taps()).enumerate() {
                assert!((got as i32 - (t as i32 >> 1)).abs() <= 1, "{e:?} {k}: {got} vs {}", t >> 1);
            }
            assert!(y[n..].iter().all(|&v| v == 0));
            // A 1 kHz sine through it: the taps' 1 kHz gain, after the filter fills.
            let mut f = Fir::new(e);
            let x: Vec<i16> = (0..4800).map(|i| (10000.0 * (2.0 * core::f64::consts::PI * 1000.0 * i as f64 / DAC_HZ as f64).sin()) as i16).collect();
            let peak = x.iter().map(|&v| f.run(v)).skip(n).map(|v| v.unsigned_abs()).max().unwrap() as f64;
            let want = 10000.0 * 10f64.powf(e.gain_1k_cdb() as f64 / 2000.0);
            assert!((peak - want).abs() < want * 0.02 + 2.0, "{e:?}: {peak} vs {want}");
            let mut f = Fir::new(e);
            assert!((0..400).map(|i| f.run(if i % 4 < 2 { 32767 } else { -32767 })).all(|v| v.unsigned_abs() <= 32767));
        }
        let mut f = Fir::new(Eq::Off);
        assert_eq!([f.run(5), f.run(-7)], [5, -7]);
        assert_eq!((Eq::parse(b"EQ8"), Eq::parse(b"eq"), Eq::parse(b"eq5")), (Some(Eq::Eq8), Some(Eq::Eq), None));
    }
}
