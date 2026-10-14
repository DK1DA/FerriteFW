//! The audio between the DSP task's converters and the decoders and modulators (roadmap 2f, 5b, 5c), shared by
//! the firmware and the host tests so both run the same arithmetic.
//!
//! - **In:** ADC1 samples PA2 (the BK4829s' EARO) at [`ADC_HZ`], 12 bits unsigned. [`Decim`] takes away the DC
//!   bias, low-passes and keeps every fourth sample: [`crate::decoders::RATE_HZ`] signed 16-bit, which is what
//!   every decoder in the slot takes. Stock samples at 9.6 kHz with no filter (`analysis/audio_path.md`); 48 kHz
//!   leaves room for the faster modes (POCSAG 2400, 4FSK) and the filter keeps what is above 6 kHz out.
//! - **Out:** DAC1 (PA4) plays [`DAC_HZ`] samples of 12 bits into the transmitting BK4829's microphone input
//!   (PC12 high, as stock: `analysis/audio_path.md`); [`to_dac`] maps a signed sample onto the DAC's range.

use crate::decoders::RATE_HZ;
/// The modem, for the firmware's DSP task (it plays [`uafsk::Modulator`]'s samples).
pub use uafsk;

/// ADC1's rate on PA2.
pub const ADC_HZ: u32 = 48_000;
/// ADC samples per decoder sample.
pub const DECIM: usize = (ADC_HZ / RATE_HZ) as usize;
/// DAC1's rate for AFSK (and later other modulators). Stock plays AFSK at 38.4 kHz; 48 kHz is the same timer
/// arithmetic as the ADC's.
pub const DAC_HZ: u32 = 48_000;
/// AFSK's peak, full scale 32767: about a quarter of the DAC's range, the level stock's table gives the mic
/// input. Bench-confirm: the deviation it makes (3 kHz is APRS's target; roadmap 5c).
pub const AFSK_AMP: i16 = 8_000;

/// Each half of the ADC's DMA buffer: 2 ms at 48 kHz. The DSP task gets 500 blocks a second of
/// [`BLOCK_OUT`] decoder samples.
pub const BLOCK_IN: usize = 96;
pub const BLOCK_OUT: usize = BLOCK_IN / DECIM;
/// Each half of the DAC's DMA buffer: 2.7 ms at 48 kHz.
pub const BLOCK_DAC: usize = 128;

const N_TAPS: usize = 15;
/// The low-pass ahead of the 4:1 step: 15 taps (Blackman-windowed sinc, 4.5 kHz), Q15, summing to 1. Flat to
/// within 1.1 dB across AFSK's tones (1200 Hz -0.3 dB, 2200 Hz -1.1 dB); what folds back onto 0-3 kHz (9 to
/// 15 kHz) is 25 dB down or more. A `static`: read per sample by the DSP task, and only named statics stay in
/// the firmware's fast flash (unnamed constants go to the slow part: firmware/memory.x).
static TAPS: [i32; N_TAPS] = [0, -15, 43, 515, 1849, 4063, 6299, 7260, 6299, 4063, 1849, 515, 43, -15, 0];

/// The ADC side: DC removal, the low-pass and the 4:1 step, one block at a time. 36 bytes.
pub struct Decim {
    /// The last [`TAPS`] inputs, centred, as a ring.
    hist: [i16; N_TAPS],
    at: u8,
    /// Inputs since the last output.
    phase: u8,
    /// The running mean (DC), Q16 of an ADC count: a one-pole average over 2,048 samples (43 ms).
    dc_q16: i32,
}

impl Default for Decim {
    fn default() -> Self {
        Self::new()
    }
}

impl Decim {
    pub const fn new() -> Self {
        Decim { hist: [0; N_TAPS], at: 0, phase: 0, dc_q16: 2048 << 16 }
    }

    /// ADC counts (0-4095) in, decoder samples out through `out`, about one in four. The ADC's 12 bits become
    /// the top of a 16-bit sample (a full-scale swing is +-32767).
    #[inline]
    pub fn push(&mut self, adc: u16, mut out: impl FnMut(i16)) {
        let x = (adc as i32) << 16;
        self.dc_q16 += (x - self.dc_q16) >> 11;
        let s = ((x - self.dc_q16) >> 12).clamp(-32768, 32767) as i16;
        self.hist[self.at as usize] = s;
        self.at = if self.at as usize + 1 == N_TAPS { 0 } else { self.at + 1 };
        self.phase += 1;
        if self.phase as usize == DECIM {
            self.phase = 0;
            // The ring's oldest sample is at `at`.
            let mut acc = 0i32;
            let mut j = self.at as usize;
            for &t in TAPS.iter() {
                acc += t * self.hist[j] as i32;
                j = if j + 1 == N_TAPS { 0 } else { j + 1 };
            }
            out((acc >> 15).clamp(-32768, 32767) as i16);
        }
    }

    /// A block of ADC counts.
    pub fn block(&mut self, adc: &[u16], mut out: impl FnMut(i16)) {
        for &a in adc {
            self.push(a, &mut out);
        }
    }
}

/// A signed sample as a 12-bit DAC code, centred on 2048.
#[inline]
pub fn to_dac(s: i16) -> u16 {
    ((s as i32 >> 4) + 2048).clamp(0, 4095) as u16
}

/// The AFSK settings for transmission at [`DAC_HZ`]: `txdelay_ms` of flags first (KISS's TX delay, 300 ms by
/// default: KISS rules in chapter 26), so the transmitter is up before the frame.
pub fn afsk_tx(txdelay_ms: u16) -> uafsk::TxConfig {
    // A flag is 8 bits at 1200 Bd: 6.67 ms.
    let flags = (txdelay_ms as u32 * 3).div_ceil(20) as u16;
    uafsk::TxConfig { rate_hz: DAC_HZ, preamble: flags.max(1), postamble: 3, amp: AFSK_AMP }
}

const _: () = assert!(ADC_HZ.is_multiple_of(RATE_HZ) && BLOCK_IN.is_multiple_of(DECIM));

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// A tone through the decimator: its level after the filter, in Q15 of the input's.
    fn gain(hz: f64) -> f64 {
        let mut d = Decim::new();
        let mut out = Vec::new();
        for n in 0..48_000 {
            let x = 2048.0 + 1000.0 * (2.0 * core::f64::consts::PI * hz * n as f64 / ADC_HZ as f64).sin();
            d.push(x.round() as u16, |s| out.push(s));
        }
        let tail = &out[out.len() / 2..];
        let peak = tail.iter().map(|s| (*s as i32).abs()).max().unwrap() as f64;
        peak / (1000.0 * 16.0)
    }

    #[test]
    fn decimator_passes_afsk_and_stops_aliases() {
        assert!(gain(1200.0) > 0.9 && gain(2200.0) > 0.85, "AFSK tones pass: {} {}", gain(1200.0), gain(2200.0));
        assert!(gain(10_000.0) < 0.06, "10 kHz folds onto 2 kHz: kept out ({})", gain(10_000.0));
        // One output per four inputs, and DC is gone.
        let mut d = Decim::new();
        let mut n = 0;
        let mut last = 0;
        for _ in 0..40_000 {
            d.push(3000, |s| {
                n += 1;
                last = s;
            });
        }
        assert_eq!(n, 10_000);
        assert!(last.abs() < 40, "DC removed: {last}");
    }

    #[test]
    fn dac_codes() {
        assert_eq!(to_dac(0), 2048);
        assert_eq!(to_dac(i16::MAX), 4095);
        assert_eq!(to_dac(i16::MIN), 0);
        assert_eq!(afsk_tx(300).preamble, 45);
    }
}
