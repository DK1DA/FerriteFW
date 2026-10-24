//! Bell 202 AFSK modulator: a frame to audio samples, phase-continuous, one sample at a time (no buffers).
//!
//! [`Modulator::new`] sends an AX.25 frame (flags, stuffing and FCS added); [`Modulator::from_bits`] sends any bit
//! sequence before NRZI, such as an FX.25 transmission (`ufx25::Encoder::bits`).

use crate::hdlc::Framer;
use udsp::trig::{cos_sin_phase, phase_step};

/// Settings for transmission.
#[derive(Clone, Copy, Debug)]
pub struct TxConfig {
    pub rate_hz: u32,
    /// Flags before the frame (TXDELAY): 0x7E bytes, 6.7 ms each. 30 is 200 ms, enough for most radios' keying.
    pub preamble: u16,
    /// Flags after the frame.
    pub postamble: u16,
    /// Peak amplitude, full scale 32767.
    pub amp: i16,
}

impl Default for TxConfig {
    fn default() -> Self {
        TxConfig { rate_hz: 9600, preamble: 30, postamble: 3, amp: 16000 }
    }
}

/// Audio for one frame (FCS added) or one bit sequence: an iterator of 16-bit samples. `B` is the bits' iterator:
/// [`Framer`] for [`Modulator::new`].
pub struct Modulator<B> {
    bits: B,
    phase: u32,
    step_mark: u32,
    step_space: u32,
    mark: bool,
    bit_phase: u32,
    bit_step: u32,
    amp: i32,
    done: bool,
}

impl<'a> Modulator<Framer<'a>> {
    /// At least one flag goes before and after the frame, whatever `cfg` asks for.
    pub fn new(frame: &'a [u8], cfg: TxConfig) -> Self {
        Modulator::from_bits(Framer::new(frame, cfg.preamble.max(1), cfg.postamble.max(1)), cfg)
    }
}

impl<B: Iterator<Item = u8>> Modulator<B> {
    /// Any bits (0 or 1, before NRZI: a 0 changes the tone), flags included: `cfg`'s preamble and postamble are
    /// not used. For FX.25: `Modulator::from_bits(encoder.bits(cfg.preamble, cfg.postamble), cfg)`.
    pub fn from_bits(bits: B, cfg: TxConfig) -> Self {
        assert!(cfg.rate_hz >= 2400, "rate_hz must be at least 2400");
        let rate_mhz = cfg.rate_hz as u64 * 1000;
        let mut m = Modulator {
            bits,
            phase: 0,
            step_mark: phase_step(1_200_000, rate_mhz),
            step_space: phase_step(2_200_000, rate_mhz),
            mark: true,
            bit_phase: 0,
            bit_step: phase_step(1_200_000, rate_mhz),
            amp: cfg.amp as i32,
            done: false,
        };
        m.next_bit();
        m
    }

    /// NRZI: a 0 changes the tone, a 1 keeps it.
    fn next_bit(&mut self) {
        match self.bits.next() {
            Some(0) => self.mark = !self.mark,
            Some(_) => {}
            None => self.done = true,
        }
    }
}

impl<B: Iterator<Item = u8>> Iterator for Modulator<B> {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        if self.done {
            return None;
        }
        let (_, s) = cos_sin_phase(self.phase);
        self.phase = self.phase.wrapping_add(if self.mark { self.step_mark } else { self.step_space });
        let before = self.bit_phase;
        self.bit_phase = self.bit_phase.wrapping_add(self.bit_step);
        if self.bit_phase < before {
            self.next_bit();
        }
        Some(((s * self.amp) >> 15) as i16)
    }
}
