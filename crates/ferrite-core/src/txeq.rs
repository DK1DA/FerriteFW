//! TERM-33.16 (decisions 308, 310, 311): the transmit pre-equalisers. DAC1's samples reach the modulator through
//! the BK4829's mic input; with every AF TX filter bypassed and the soft limiter off (`txtest ... bypass`,
//! TERM-33.15) the channel is the injection network's first-order low-pass at about 168 Hz (minimum phase), the
//! board's coupling high-pass near 35 Hz, and the chip's linear-phase digital filter (-2.4 dB at 5 kHz, -11.8 at
//! 8 kHz), the whole band 6.8 dB under the plain `data` set-up's 1 kHz tone (identified from the low-power captures,
//! `tools/bench-blade/txident.py`, Oct 27, 2026). These two FIRs at [`crate::audio::DAC_HZ`] undo it in magnitude
//! and phase together (decision 311: decision 310's linear-phase ones fixed the magnitude only, and the pole's phase,
//! -90 degrees above a few hundred Hz, left the eye of 9600 Bd shut), so every tone goes out at the deviation the
//! plain `txtest sweep data` 1 kHz tone gets, with the group delay flat: [`Eq::Eq`] 100 Hz-5 kHz, [`Eq::Eq8`]
//! 100 Hz-8 kHz (a wide-channel set-up: Carson puts audio to 8 kHz at 3 kHz deviation at about 22 kHz of RF).
//!
//! Designed offline by `tools/bench-blade/txeq_design.py` (complex weighted least squares against the identified
//! channel, a delay allowed: `eq` 59 samples, `eq8` 65); predicted through the channel model: each tone -0.5 to
//! +0.2 dB against the plain 1 kHz tone, the group delay within 135 us from 200 Hz up (the coupling's, left in
//! place), and txdata.py's slicer finding 9600 Bd G3RUH's eye open (about 40 %, no errors in 50,000 bits).
//!
//! **Level.** The 6.8 dB comes from the gain stages left after the bypass and the DAC's swing: REG_7D's mic
//! sensitivity at its top, 0x3F ([`Eq::mic_steps`]: 11 steps of 0.5 dB over the data key-up's 0x34; measured
//! linear under the bypass, decision 310), `eq8` also REG_40 ([`Eq::deviation`]: 0x57A, +2 dB by the power law the
//! tone generator follows, deviation ~ REG_40^2.8), and the taps the rest ([`Eq::gain_1k_cdb`]).
//!
//! **Headroom.** Each equaliser itself falls 40 dB and more above its band (the chip's filter isn't counted on), so
//! the DAC's room goes to the band. At LEVEL 100 every test signal fits the DAC through `eq` (the sweep, 9600 Bd and
//! `g3ruh` just: up to 100-102 %); through `eq8` all but hard-edged `fsk 1200` (up to 89 %) and `fsk 2400` (81 %), and
//! the sweep, whose 5-8 kHz tones fit up to 29 %. Above that the filter's output saturates (clips, never wraps). The
//! boost is digital, ahead of the DAC: the DAC's and the injection's noise floors aren't boosted by it.
//!
//! **Cost.** Taps of any shape: 95 (`eq`) or 127 (`eq8`) multiply-accumulates a sample, 4.6 or 6.1 million a second
//! at 48 kHz; a 254-sample delay line (508 bytes) wherever the [`Fir`] lives. The taps are named statics and the
//! filter is core's code: both in the fast flash (firmware/memory.x).

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
pub const MAX_TAPS: usize = 127;

/// `eq`: 95 taps, Q15 ([`Eq::shift`]), +1.35 dB at 1 kHz, 59 samples' delay.
pub static EQ_TAPS: [i16; 95] = [
    -67, -27, 24, 68, 54, -20, -115, -158, -103, 34, 166, 196, 84, -107, -244, -214,
    -15, 226, 329, 199, -89, -321, -291, 26, 422, 576, 293, -308, -812, -773, -66, 936,
    1520, 1104, -290, -1896, -2590, -1628, 720, 3124, 3893, 2079, -1694, -5310, -6275, -3311, 2545, 8140,
    9814, 5641, -3026, -11742, -15061, -9545, 3968, 20195, 31672, 32392, 20946, 1466, -18203, -30206, -30349, -19880,
    -4543, 8484, 14083, 11339, 3379, -4758, -8912, -7724, -2833, 2515, 5450, 4926, 1935, -1412, -3244, -2891,
    -1018, 1026, 2081, 1796, 629, -569, -1143, -933, -276, 350, 582, 427, 47, -186, -467,
];

/// `eq8`: 127 taps, Q13, -0.65 dB at 1 kHz, 65 samples' delay.
pub static EQ8_TAPS: [i16; 127] = [
    29, 19, -18, -59, -36, 46, 95, 25, -107, -138, 5, 174, 147, -83, -251, -125,
    179, 294, 50, -280, -288, 57, 347, 230, -150, -337, -127, 202, 273, 69, -127, -149,
    -91, -37, 102, 317, 287, -215, -742, -469, 638, 1330, 392, -1488, -1977, 108, 2699, 2349,
    -1355, -4316, -2288, 3366, 6025, 1345, -6377, -7724, 787, 10780, 9906, -3584, -16340, -12767, 7246, 25653,
    23500, 804, -22371, -25563, -7919, 12131, 16325, 4088, -9457, -10804, -1159, 7459, 6480, -1058, -5911, -3531,
    2074, 4308, 1525, -2242, -2786, -256, 1970, 1623, -306, -1412, -774, 472, 890, 314, -372, -479,
    -126, 193, 234, 95, -58, -152, -147, -23, 137, 168, 12, -171, -170, 19, 177, 124,
    -64, -162, -68, 87, 122, 21, -80, -71, 13, 62, 35, -16, -28, -5, 19,
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

    /// The taps' scale: `eq` Q15, `eq8` Q13 (its taps go past 1).
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
        // The newest first: w[k] is the sample k back, met by h[k] (taps of any shape).
        let w = &self.buf[at..at + n];
        let mut acc = 0i64;
        for (&t, &v) in h.iter().zip(w) {
            acc += t as i64 * v as i64;
        }
        self.at = if at == 0 { (n - 1) as u8 } else { (at - 1) as u8 };
        (acc >> self.eq.shift()).clamp(-32767, 32767) as i16
    }
}

/// The filter's response at `hz`, from its taps (re, im), at unit scale.
#[cfg(test)]
fn resp(eq: Eq, hz: f64) -> (f64, f64) {
    let (mut re, mut im) = (0.0, 0.0);
    for (k, &t) in eq.taps().iter().enumerate() {
        let a = -2.0 * core::f64::consts::PI * hz * k as f64 / DAC_HZ as f64;
        re += t as f64 * a.cos();
        im += t as f64 * a.sin();
    }
    let s = (1u32 << eq.shift()) as f64;
    (re / s, im / s)
}

/// The filter's gain at `hz`, dB.
#[cfg(test)]
fn gain_db(eq: Eq, hz: f64) -> f64 {
    let (re, im) = resp(eq, hz);
    20.0 * (re * re + im * im).sqrt().log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// The identified bypass channel's analogue part (decision 311): the injection's pole at 168 Hz, the coupling's
    /// high-pass at 35 Hz; (magnitude dB, phase rad) at `hz`.
    fn channel(hz: f64) -> (f64, f64) {
        let (p, h) = (hz / 168.0, hz / 35.0);
        let mag = -10.0 * (1.0 + p * p).log10() + 20.0 * h.log10() - 10.0 * (1.0 + h * h).log10();
        (mag, -p.atan() + core::f64::consts::FRAC_PI_2 - h.atan())
    }

    /// Equaliser x channel at `hz`: (dB against 1 kHz, phase rad).
    fn total(e: Eq, hz: f64) -> (f64, f64) {
        let (re, im) = resp(e, hz);
        let (cm, cp) = channel(hz);
        (gain_db(e, hz) + cm - gain_db(e, 1000.0) - channel(1000.0).0, im.atan2(re) + cp)
    }

    /// TERM-33.16: the taps at 1 kHz and the gain stages make up the bypass's 6.84 dB loss together, REG_7D at its
    /// top and REG_40 never past +2 dB by the power law; the equaliser undoes the channel's pole in magnitude and
    /// phase: equaliser x channel flat within 0.7 dB from 100 Hz to 3 kHz (above it the chip's own
    /// linear-phase roll-off, which the design takes from the sweep, comes in), its group delay within 150 us of
    /// the 1 kHz value from 200 Hz to 3 kHz (the pole alone: 500 us at 200 Hz) and near the design's delay; and the
    /// boost falls away above each band.
    #[test]
    fn gains() {
        let gd = |e: Eq, hz: f64| {
            let d = 1.0;
            let (_, a) = total(e, hz + d);
            let (_, b) = total(e, hz - d);
            let mut dp = a - b;
            while dp > core::f64::consts::PI {
                dp -= 2.0 * core::f64::consts::PI;
            }
            while dp < -core::f64::consts::PI {
                dp += 2.0 * core::f64::consts::PI;
            }
            -dp / (2.0 * core::f64::consts::PI * 2.0 * d) * 1e6
        };
        for (e, delay) in [(Eq::Eq, 59.0), (Eq::Eq8, 65.0)] {
            assert!(e.taps().len() <= MAX_TAPS);
            assert!((gain_db(e, 1000.0) * 100.0 - e.gain_1k_cdb() as f64).abs() < 3.0, "{e:?} 1 kHz: {:.2}", gain_db(e, 1000.0));
            assert!((e.gain_1k_cdb() as f64 + e.stages_cdb() - BYPASS_LOSS_CDB as f64).abs() < 10.0, "{e:?}: {:.0}", e.stages_cdb());
            assert!(0x34 + e.mic_steps() == 0x3F && e.stages_cdb() - 550.0 <= 210.0);
            let mut hz = 100.0;
            while hz <= 3000.0 {
                let (m, _) = total(e, hz);
                assert!((-0.7..=0.7).contains(&m), "{e:?} at {hz:.0} Hz: {m:+.2} dB");
                hz *= 1.12;
            }
            let g1 = gd(e, 1000.0);
            assert!((g1 - delay / 48.0 * 1000.0).abs() < 60.0, "{e:?}: {g1:.0} us at 1 kHz");
            let mut hz = 200.0;
            while hz <= 3000.0 {
                assert!((gd(e, hz) - g1).abs() < 150.0, "{e:?} at {hz:.0} Hz: {:.0} us against {g1:.0}", gd(e, hz));
                hz *= 1.12;
            }
        }
        // Above each band the equaliser itself falls (the chip's own filter isn't counted on): with the analogue
        // channel alone, -40 dB from 7 kHz (`eq`) or 10 kHz (`eq8`) up.
        for hz in [7000.0, 9000.0, 12000.0, 16000.0, 20000.0] {
            assert!(total(Eq::Eq, hz).0 < -40.0, "eq {hz}: {:.1}", total(Eq::Eq, hz).0);
        }
        for hz in [10000.0, 12000.0, 16000.0, 20000.0] {
            assert!(total(Eq::Eq8, hz).0 < -40.0, "eq8 {hz}: {:.1}", total(Eq::Eq8, hz).0);
        }
    }

    /// TERM-33.16: the running filter is the taps' convolution (an impulse comes out as the taps, newest first),
    /// off passes samples through, a 1 kHz sine comes out at the taps' 1 kHz gain, and a full-scale input
    /// saturates instead of wrapping.
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
            let mut f = Fir::new(e);
            let x: Vec<i16> = (0..4800).map(|i| (5000.0 * (2.0 * core::f64::consts::PI * 1000.0 * i as f64 / DAC_HZ as f64).sin()) as i16).collect();
            let peak = x.iter().map(|&v| f.run(v)).skip(n).map(|v| v.unsigned_abs()).max().unwrap() as f64;
            let want = 5000.0 * 10f64.powf(e.gain_1k_cdb() as f64 / 2000.0);
            assert!((peak - want).abs() < want * 0.02 + 2.0, "{e:?}: {peak} vs {want}");
            let mut f = Fir::new(e);
            assert!((0..400).map(|i| f.run(if i % 4 < 2 { 32767 } else { -32767 })).all(|v| v.unsigned_abs() <= 32767));
        }
        let mut f = Fir::new(Eq::Off);
        assert_eq!([f.run(5), f.run(-7)], [5, -7]);
        assert_eq!((Eq::parse(b"EQ8"), Eq::parse(b"eq"), Eq::parse(b"eq5")), (Some(Eq::Eq8), Some(Eq::Eq), None));
    }
}
