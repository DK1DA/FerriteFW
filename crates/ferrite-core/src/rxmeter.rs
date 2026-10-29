//! TERM-44 (decision 315, a bench test): the receive audio's level at fixed frequencies, measured on the radio from
//! ADC1's own 48 kHz samples, ahead of the decoders' 5 kHz low-pass ([`crate::audio::Decim`]: -6 dB at 5 kHz, -16.6
//! at 6.3 kHz, which the raw stream, LINK-13.9, carries too). So a sweep sees the chip and the board alone, to 12.5
//! kHz, with no stream: the PC zeroes the meter (`meter on`), the bladeRF plays `fmgen.py sweep`, and `meter` reads
//! the table back.
//!
//! - **Bins:** [`FREQS`], the sweep's third-octave tones and two above them. One Goertzel filter each, over windows of
//!   [`WINDOW`] samples (100 ms; a sweep tone lasts 0.3 s, so at least two whole windows fall inside it).
//! - **Max-hold:** each bin keeps its loudest window since `meter on`, in dB against a full-scale sine (ADC1's 12
//!   bits, amplitude 2048) x10. A tone's own bin reads it; the other bins see only its leakage and the noise.
//! - **DC:** each window takes away the mean of the window before it (ADC1 rests near 2048).

/// The bins, in Hz: `tools/bench-blade/fmgen.py`'s third-octave sweep tones, 50 Hz to 8 kHz, then 10 and 12.5 kHz.
pub const FREQS: [u16; N] = [50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000, 10000, 12500];
pub const N: usize = 25;
/// ADC1's rate ([`crate::audio::ADC_HZ`]).
pub const RATE: u32 = 48_000;
/// Samples per window: 100 ms.
pub const WINDOW: u16 = 4800;
/// A bin that has had no window yet.
pub const NONE: i16 = i16::MIN;

/// 2 cos(2 pi f / 48000) per bin (the test checks them against [`FREQS`]). A named static: read per sample.
#[allow(clippy::excessive_precision)] // generated to 7 places; the test checks them
static COEF: [f32; N] = [
    1.9999572, 1.9999320, 1.9998903, 1.9998287, 1.9997323, 1.9995614, 1.9993146, 1.9989292, 1.9983000, 1.9972591, 1.9957178, 1.9932031, 1.9890438,
    1.9828897, 1.9732867, 1.9562952, 1.9318517, 1.8938603, 1.8323759, 1.7320508, 1.5867067, 1.3576015, 1.0000000, 0.5176381, -0.1308063,
];

/// What `meter` answers: on or off, whole windows since `meter on`, and each bin's loudest window (dBFS x10, or
/// [`NONE`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub on: bool,
    pub windows: u16,
    pub db10: [i16; N],
}

impl Report {
    pub const OFF: Report = Report { on: false, windows: 0, db10: [NONE; N] };
}

/// The meter while it runs.
#[derive(Clone, Copy)]
pub struct Meter {
    s1: [f32; N],
    s2: [f32; N],
    n: u16,
    sum: u32,
    dc: f32,
    pub report: Report,
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

/// A fresh meter, as a named static: the DSP task and the Radio task start one from it, and an unnamed constant
/// template would sit in slow flash (`firmware/memory.x`, tools/slowcheck.py).
pub static FRESH: Meter = Meter::new();

impl Meter {
    pub const fn new() -> Self {
        Meter { s1: [0.0; N], s2: [0.0; N], n: 0, sum: 0, dc: 2048.0, report: Report { on: true, ..Report::OFF } }
    }

    /// One ADC1 sample (12 bits, unsigned); true when it ended a window (the report changed).
    pub fn push(&mut self, s: u16) -> bool {
        let x = s as f32 - self.dc;
        self.sum += s as u32;
        for k in 0..N {
            let v = x + COEF[k] * self.s1[k] - self.s2[k];
            self.s2[k] = self.s1[k];
            self.s1[k] = v;
        }
        self.n += 1;
        if self.n < WINDOW {
            return false;
        }
        for k in 0..N {
            let (a, b) = (self.s1[k], self.s2[k]);
            let p = a * a + b * b - COEF[k] * a * b;
            let d = db10(p) + FULL_DB10;
            let best = &mut self.report.db10[k];
            if d > *best {
                *best = d;
            }
        }
        self.dc = self.sum as f32 / WINDOW as f32;
        (self.s1, self.s2, self.n, self.sum) = ([0.0; N], [0.0; N], 0, 0);
        self.report.windows = self.report.windows.saturating_add(1);
        true
    }
}

/// dB x10 of a Goertzel power to dBFS x10: a sine of amplitude A gives |X| = A N / 2, so full scale (2048) is
/// 10 log10((2048 x 4800 / 2)^2) = 133.83 dB.
const FULL_DB10: i16 = -1338;

/// 10 log10(p) x10, to about 0.15 dB: log2 from the float's exponent and a quadratic in its mantissa.
fn db10(p: f32) -> i16 {
    if p <= 1e-6 {
        return -600;
    }
    let b = p.to_bits();
    let e = ((b >> 23) & 0xFF) as i32 - 127;
    let m = f32::from_bits((b & 0x007F_FFFF) | 0x3F80_0000) - 1.0;
    let l2 = e as f32 + m * (1.346_574 - 0.346_574 * m);
    let v = l2 * 30.103;
    (if v < 0.0 { v - 0.5 } else { v + 0.5 }) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefs_match_freqs() {
        for (f, c) in FREQS.iter().zip(COEF) {
            let want = 2.0 * (2.0 * core::f64::consts::PI * *f as f64 / RATE as f64).cos();
            assert!((c as f64 - want).abs() < 1e-6, "{f}");
        }
    }

    #[test]
    fn db10_is_close() {
        for p in [1.0f32, 2.0, 3.0, 10.0, 1234.5, 1e9, 7.7e12] {
            let want = 100.0 * (p as f64).log10();
            assert!((db10(p) as f64 - want).abs() <= 2.0, "{p}: {} vs {want}", db10(p));
        }
    }

    fn run(hz: f64, amp: f64, windows: usize) -> Report {
        let mut m = Meter::new();
        for i in 0..WINDOW as usize * windows {
            let s = 2048.0 + amp * (2.0 * core::f64::consts::PI * hz * i as f64 / RATE as f64).sin();
            m.push(s.round() as u16);
        }
        m.report
    }

    /// TERM-44: a tone at a bin reads its level against full scale in its own bin, whatever the bin (the low ones,
    /// whose coefficients sit near 2, included), and stays well above the other bins.
    #[test]
    fn tone_reads_its_level() {
        for (k, &f) in FREQS.iter().enumerate() {
            let r = run(f as f64, 1024.0, 3);
            assert_eq!(r.windows, 3);
            assert!((r.db10[k] + 60).abs() <= 3, "{f} Hz: {}", r.db10[k]);
            let others = r.db10.iter().enumerate().filter(|&(j, _)| j != k).map(|(_, &d)| d).max().unwrap();
            assert!(others < r.db10[k] - 100, "{f} Hz: others {others} vs {}", r.db10[k]);
        }
        // -40 dBFS at 6.3 kHz
        assert!((run(6300.0, 20.48, 2).db10[21] + 400).abs() <= 3);
    }

    /// TERM-44: the bins keep their loudest window, and a fresh meter has none.
    #[test]
    fn max_hold() {
        let mut m = Meter::new();
        assert_eq!(m.report.db10, [NONE; N]);
        for (amp, hz) in [(1024.0, 1000.0), (100.0, 1000.0)] {
            for i in 0..WINDOW as usize {
                let s = 2048.0 + amp * (2.0 * core::f64::consts::PI * hz * i as f64 / RATE as f64).sin();
                m.push(s.round() as u16);
            }
        }
        assert!((m.report.db10[13] + 60).abs() <= 3);
    }
}
