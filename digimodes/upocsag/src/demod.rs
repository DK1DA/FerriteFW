//! Discriminator audio to messages, at 512, 1200 and 2400 baud at once. Integer only.
//!
//! POCSAG is two-level FSK (±4.5 kHz), so after an FM discriminator the bits are the audio's level: no tones to
//! detect. For each rate:
//! 1. **Average over one bit** (a running sum over a shared ring of samples): the matched filter for square bits.
//! 2. **Threshold** half-way between a peak and a valley tracker, so a frequency offset (DC) does not matter.
//! 3. **Recover the clock** with a digital PLL pulled a quarter of the way to each crossing (its time interpolated
//!    between samples, which matters at 4 samples a bit), sampling half-way between crossings.
//! 4. **Find the sync word**, either polarity (receivers differ), with up to two bit errors.
//!
//! Each rate locks on its own sync word and checks its codewords; two uncorrectable codewords in a row, or a missing
//! sync word at the start of a batch, drop the lock (noise passes the code's correction about one time in four, so
//! a false lock lasts a few codewords). The first locked rate owns the [`Decoder`]; when it drops, another locked
//! rate takes over at its next batch.
//!
//! Audio must be flat, from a discriminator: a speaker output's high-pass and de-emphasis distort 512-baud bits.

use crate::decode::{Decoder, Message};
use crate::{bch, BAUDS, SYNC};

/// Longest bit, in samples: 512 baud at 32768 samples/s.
const RING: usize = 64;

#[derive(Clone, Copy)]
struct Channel {
    len: usize,
    sum: i32,
    peak: i32,
    valley: i32,
    pll: i32,
    step: i32,
    /// Last sample's distance from the threshold.
    d: i32,
    sr: u32,
}

#[derive(Clone, Copy)]
struct Lock {
    invert: u32,
    /// Uncorrectable codewords in a row.
    bad: u8,
    word: u32,
    nbits: u8,
    words: u8,
}

/// Audio in, messages out. RAM on a Cortex-M4 with the default `N` of 256: 564 bytes (128 of samples, about 180 of
/// state, and the decoder's `N`-byte message buffer).
pub struct Receiver<const N: usize = 256> {
    ring: [i16; RING],
    pos: usize,
    decay: u32,
    ch: [Channel; 3],
    locks: [Option<Lock>; 3],
    /// The rate whose lock feeds the decoder.
    owner: Option<usize>,
    dec: Decoder<N>,
}

impl<const N: usize> Receiver<N> {
    /// `rate_hz`: 9600 to 32768 samples/s.
    pub fn new(rate_hz: u32) -> Self {
        assert!((9600..=32768).contains(&rate_hz), "rate must be 9600 to 32768 samples/s");
        Receiver {
            ring: [0; RING],
            pos: 0,
            // Trackers decay with a time constant of about 0.1 s.
            decay: (rate_hz / 8).ilog2(),
            ch: BAUDS.map(|b| Channel {
                len: ((rate_hz + b as u32 / 2) / b as u32) as usize,
                sum: 0,
                peak: 0,
                valley: 0,
                pll: 0,
                step: ((b as u64) << 32).div_ceil(rate_hz as u64) as u32 as i32,
                d: 0,
                sr: 0,
            }),
            locks: [None; 3],
            owner: None,
            dec: Decoder::new(),
        }
    }

    /// Feed one sample. `found` gets each message when it ends.
    pub fn push(&mut self, x: i16, mut found: impl FnMut(&Message)) {
        let pos = self.pos;
        self.pos = (pos + 1) % RING;
        for i in 0..3 {
            let c = &mut self.ch[i];
            c.sum += x as i32 - self.ring[(pos + RING - c.len) % RING] as i32;
            let y = c.sum;
            if y > c.peak {
                c.peak += (y - c.peak) >> 1;
            } else {
                c.peak -= (c.peak - y) >> self.decay;
            }
            if y < c.valley {
                c.valley -= (c.valley - y) >> 1;
            } else {
                c.valley += (y - c.valley) >> self.decay;
            }
            let d = y - ((c.peak >> 1) + (c.valley >> 1));
            let high = d > 0;
            let before = c.pll;
            c.pll = c.pll.wrapping_add(c.step);
            if high != (c.d > 0) {
                // The crossing was a fraction of a sample ago (linear interpolation, Q8); the clock should read
                // that fraction of a step now. Pull it 1/4 of the way there.
                let t = (d.unsigned_abs() << 8) / (d - c.d).unsigned_abs().max(1);
                let ideal = (c.step >> 8) * t as i32;
                c.pll = c.pll.wrapping_sub(c.pll.wrapping_sub(ideal) >> 2);
            }
            c.d = d;
            if before >= 0 && c.pll < 0 {
                let bit = high as u32;
                c.sr = c.sr << 1 | bit;
                let sr = c.sr;
                self.bit(i, bit, sr, &mut found);
            }
        }
        self.ring[pos] = x;
    }

    /// The transmission in progress, if any, ends now (the squelch closed): report its message.
    pub fn flush(&mut self, found: impl FnMut(&Message)) {
        self.locks = [None; 3];
        self.owner = None;
        self.dec.end(found);
    }

    fn bit(&mut self, i: usize, bit: u32, sr: u32, found: &mut impl FnMut(&Message)) {
        let Some(l) = &mut self.locks[i] else {
            let invert = if (sr ^ SYNC).count_ones() <= 2 {
                0
            } else if (!sr ^ SYNC).count_ones() <= 2 {
                !0
            } else {
                return;
            };
            self.locks[i] = Some(Lock { invert, bad: 0, word: 0, nbits: 0, words: 0 });
            self.batch(i);
            return;
        };
        l.word = l.word << 1 | bit;
        l.nbits += 1;
        if l.nbits < 32 {
            return;
        }
        l.nbits = 0;
        let w = l.word ^ l.invert;
        let keep = if l.words < 16 {
            l.words += 1;
            l.bad = if bch::correct(w).is_some() { 0 } else { l.bad + 1 };
            if self.owner == Some(i) {
                self.dec.word(w, &mut *found);
            }
            l.bad < 2
        } else if (w ^ SYNC).count_ones() <= 4 {
            l.words = 0;
            self.batch(i);
            true
        } else {
            false
        };
        if !keep {
            self.locks[i] = None;
            if self.owner == Some(i) {
                self.owner = None;
                self.dec.end(found);
            }
        }
    }

    /// Rate `i` starts a batch: it takes the decoder if no other rate holds it.
    fn batch(&mut self, i: usize) {
        if self.owner.is_none() {
            self.owner = Some(i);
            self.dec.baud = BAUDS[i];
        }
        if self.owner == Some(i) {
            self.dec.sync();
        }
    }
}
