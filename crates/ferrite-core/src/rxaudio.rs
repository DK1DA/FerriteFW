//! Receive audio to the PC over the cable (LINK-13.1, decision 213, decision 288): the decoders' 24 kHz samples
//! stepped down 3:1 to 8 kHz, coded as IMA ADPCM (4 bits a sample, about 4 KB/s) in blocks of [`BLOCK`] samples,
//! each block carrying the coder's state at its start, so a block lost on the way costs only itself. The radio
//! codes ([`Packer`]), Ferrite Console decodes ([`decode`]); both are this file, so they can't disagree.

/// The stream's sample rate.
pub const RATE_HZ: u32 = 8_000;
/// Samples a block: 32 ms, 128 bytes of codes.
pub const BLOCK: usize = 256;
/// A block's codes: two samples a byte, the first in the low nibble.
pub const BLOCK_BYTES: usize = BLOCK / 2;

const N_TAPS: usize = 31;
/// The low-pass ahead of the 3:1 step (24 kHz to 8 kHz): 31 taps, Kaiser-windowed sinc (beta 6, 3.4 kHz), Q15,
/// summing to 1. Flat to 2 kHz, -2.7 dB at 3 kHz, -6 dB at 3.4 kHz, -15 dB at 4 kHz, 75 dB or more from 5 kHz:
/// a voice channel's band (the BK4829's audio is band-limited much the same). A named `static`: read per sample.
static TAPS: [i32; N_TAPS] = [7, -3, -49, -99, -61, 132, 388, 420, -42, -893, -1475, -871, 1414, 4860, 8010, 9292, 8010, 4860, 1414, -871, -1475, -893, -42, 420, 388, 132, -61, -99, -49, -3, 7];

/// IMA ADPCM's step sizes and index changes (the standard tables).
static STEP: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118,
    130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060,
    1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484,
    7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
static INDEX: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// The coder's state: the last sample it decoded to and its step index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub pred: i16,
    pub index: u8,
}

impl State {
    /// One code's effect: the next sample, the state moved on (the same for coder and decoder).
    fn step(&mut self, code: u8) -> i16 {
        let step = STEP[self.index as usize] as i32;
        let mut diff = step >> 3;
        if code & 4 != 0 {
            diff += step;
        }
        if code & 2 != 0 {
            diff += step >> 1;
        }
        if code & 1 != 0 {
            diff += step >> 2;
        }
        let p = if code & 8 != 0 { self.pred as i32 - diff } else { self.pred as i32 + diff };
        self.pred = p.clamp(-32768, 32767) as i16;
        self.index = (self.index as i32 + INDEX[(code & 7) as usize] as i32).clamp(0, 88) as u8;
        self.pred
    }

    /// Sample `x` coded: its 4-bit code.
    pub fn encode(&mut self, x: i16) -> u8 {
        let step = STEP[self.index as usize] as i32;
        let mut d = x as i32 - self.pred as i32;
        let mut code = 0u8;
        if d < 0 {
            code = 8;
            d = -d;
        }
        if d >= step {
            code |= 4;
            d -= step;
        }
        if d >= step >> 1 {
            code |= 2;
            d -= step >> 1;
        }
        if d >= step >> 2 {
            code |= 1;
        }
        self.step(code);
        code
    }
}

/// A block's codes to samples, from the state at its start: `out` takes two samples a byte.
pub fn decode(start: State, codes: &[u8], out: &mut [i16]) -> usize {
    let mut s = start;
    let mut n = 0;
    for &b in codes {
        for code in [b & 15, b >> 4] {
            if n < out.len() {
                out[n] = s.step(code);
                n += 1;
            }
        }
    }
    n
}

/// One block for the link: the state at its start and its codes.
#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub start: State,
    pub codes: [u8; BLOCK_BYTES],
}

/// `AUDIO_DATA`'s head: seq, slot, flags, pred, index (LINK-13.4).
pub const HEAD: usize = 7;
/// `AUDIO_DATA`'s flags.
pub const SQ_OPEN: u8 = 1;
/// What was heard is the broadcast receiver's (FM or AM broadcast, decision 290); the slot is the broadcast's.
pub const BCAST: u8 = 2;
/// What was heard is HF mode's (the SI4732); no slot.
pub const HF: u8 = 4;
/// No slot heard.
pub const NO_SLOT: u8 = 0xFF;

/// LINK-13.4, decision 290: what the receive audio carries now, as `slot | flags << 8` (an atomic's worth): the first
/// slot whose squelch is open (a BK4829's audio; the SI4732 is muted under it, BCR-24), else the broadcast while it
/// plays on its slot, else HF mode's receiver; else nothing ([`NO_SLOT`], no flags): keep-alives only.
pub fn source(open: [bool; 3], bcast: Option<u8>, hf: bool) -> u16 {
    let (slot, flags) = match open.iter().position(|&o| o) {
        Some(s) => (s as u8, SQ_OPEN),
        None => match bcast {
            Some(s) => (s, SQ_OPEN | BCAST),
            None if hf => (NO_SLOT, SQ_OPEN | HF),
            None => (NO_SLOT, 0),
        },
    };
    slot as u16 | (flags as u16) << 8
}

/// The slot and flags packed by [`source`]; audio goes while [`SQ_OPEN`] is set.
pub fn unpack_source(v: u16) -> (u8, u8) {
    (v as u8, (v >> 8) as u8)
}

/// An `AUDIO_DATA` payload into `out`: a block, or a keep-alive (`None`). Its length.
pub fn put(seq: u16, slot: u8, flags: u8, blk: Option<&Block>, out: &mut [u8]) -> usize {
    let st = blk.map_or(State::default(), |b| b.start);
    out[..2].copy_from_slice(&seq.to_le_bytes());
    (out[2], out[3]) = (slot, flags);
    out[4..6].copy_from_slice(&st.pred.to_le_bytes());
    out[6] = st.index;
    match blk {
        Some(b) => {
            out[HEAD..HEAD + BLOCK_BYTES].copy_from_slice(&b.codes);
            HEAD + BLOCK_BYTES
        }
        None => HEAD,
    }
}

/// A whole `AUDIO_DATA` link frame into `w` (the wire bytes, framed as every link frame is): its length, or `None`
/// if `w` is too small. The DSP task sends these itself (decision 290), so audio never waits for the UI task.
pub fn frame(seq: u16, slot: u8, flags: u8, blk: Option<&Block>, w: &mut [u8]) -> Option<usize> {
    let mut p = [0u8; HEAD + BLOCK_BYTES];
    let n = put(seq, slot, flags, blk, &mut p);
    crate::link::build(crate::link::msg::AUDIO_DATA, 0, &p[..n], w)
}

/// An `AUDIO_DATA` payload read back: seq, slot, flags, the state at its start, the codes (empty: a keep-alive).
pub fn get(p: &[u8]) -> Option<(u16, u8, u8, State, &[u8])> {
    let h = p.get(..HEAD)?;
    let st = State { pred: i16::from_le_bytes([h[4], h[5]]), index: h[6].min(88) };
    Some((u16::from_le_bytes([h[0], h[1]]), h[2], h[3], st, &p[HEAD..]))
}

/// The radio's side: 24 kHz samples in ([`Packer::push`]), blocks out. About 380 bytes.
pub struct Packer {
    hist: [i16; N_TAPS],
    at: u8,
    phase: u8,
    state: State,
    block: Block,
    n: u16,
}

impl Default for Packer {
    fn default() -> Self {
        Self::new()
    }
}

impl Packer {
    pub const fn new() -> Self {
        Packer { hist: [0; N_TAPS], at: 0, phase: 0, state: State { pred: 0, index: 0 }, block: Block { start: State { pred: 0, index: 0 }, codes: [0; BLOCK_BYTES] }, n: 0 }
    }

    /// A 24 kHz sample in; a whole block out through `out` every [`BLOCK`] 8 kHz samples.
    #[inline]
    pub fn push(&mut self, x: i16, mut out: impl FnMut(&Block)) {
        self.hist[self.at as usize] = x;
        self.at = if self.at as usize + 1 == N_TAPS { 0 } else { self.at + 1 };
        self.phase += 1;
        if self.phase < 3 {
            return;
        }
        self.phase = 0;
        let mut acc = 0i32;
        let mut k = self.at as usize; // the oldest sample
        for t in TAPS.iter() {
            acc += *t * self.hist[k] as i32;
            k = if k + 1 == N_TAPS { 0 } else { k + 1 };
        }
        let y = (acc >> 15).clamp(-32768, 32767) as i16;
        if self.n == 0 {
            self.block.start = self.state;
        }
        let code = self.state.encode(y);
        let i = self.n as usize / 2;
        self.block.codes[i] = if self.n.is_multiple_of(2) { code } else { self.block.codes[i] | code << 4 };
        self.n += 1;
        if self.n as usize == BLOCK {
            self.n = 0;
            out(&self.block);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// LINK-13.7, decision 290: a slot heard comes first, then the broadcast playing, then HF mode; else nothing.
    #[test]
    fn source_picks_what_is_heard() {
        assert_eq!(unpack_source(source([false, true, true], Some(0), true)), (1, SQ_OPEN));
        assert_eq!(unpack_source(source([false; 3], Some(0), false)), (0, SQ_OPEN | BCAST));
        assert_eq!(unpack_source(source([false; 3], None, true)), (NO_SLOT, SQ_OPEN | HF));
        assert_eq!(unpack_source(source([false; 3], None, false)), (NO_SLOT, 0));
    }

    /// LINK-13.6: `frame` (the DSP task's) makes the same wire frame as `put`'s payload framed by the link.
    #[test]
    fn frame_is_a_whole_link_frame() {
        let mut w = [0u8; crate::link::MAX_WIRE];
        let n = frame(7, 1, SQ_OPEN, None, &mut w).unwrap();
        let mut p = [0u8; HEAD];
        let k = put(7, 1, SQ_OPEN, None, &mut p);
        let mut x = [0u8; crate::link::MAX_WIRE];
        let m = crate::link::build(crate::link::msg::AUDIO_DATA, 0, &p[..k], &mut x).unwrap();
        assert_eq!(&w[..n], &x[..m]);
    }

    fn tone(hz: f64, n: usize, amp: f64) -> Vec<i16> {
        (0..n).map(|i| (amp * (2.0 * core::f64::consts::PI * hz * i as f64 / 24_000.0).sin()) as i16).collect()
    }

    fn blocks(x: &[i16]) -> Vec<Block> {
        let mut p = Packer::new();
        let mut v = Vec::new();
        for &s in x {
            p.push(s, |b| v.push(*b));
        }
        v
    }

    fn power(x: &[i16]) -> f64 {
        x.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / x.len() as f64
    }

    /// LINK-13.1: a 1 kHz tone survives the step to 8 kHz and the coder: decoded from each block's own start, it
    /// matches what went in (the filtered tone) to 20 dB or better (4-bit IMA ADPCM: about 22 dB on a tone), and a block decodes alone (a lost one costs
    /// only itself).
    #[test]
    fn tone_round_trips_block_by_block() {
        let x = tone(1000.0, 24_000, 12_000.0);
        let b = blocks(&x);
        assert_eq!(b.len(), 8_000 / BLOCK);
        let mut out = [0i16; BLOCK];
        let mut got = Vec::new();
        for blk in b.iter().rev().take(10).rev() {
            assert_eq!(decode(blk.start, &blk.codes, &mut out), BLOCK);
            got.extend_from_slice(&out);
        }
        // the reference: what the coder was given (the same filter and 3:1 step), for the same samples
        let mut y = Vec::new();
        for i in (2..x.len()).step_by(3) {
            let acc: i32 = (0..N_TAPS).map(|t| TAPS[t] * if i + 1 >= N_TAPS - t { x[i + 1 + t - N_TAPS] as i32 } else { 0 }).sum();
            y.push((acc >> 15).clamp(-32768, 32767) as i16);
        }
        let refx = &y[y.len() - got.len()..];
        let err: Vec<i16> = got.iter().zip(refx).map(|(a, b)| a.saturating_sub(*b)).collect();
        let snr = 10.0 * (power(refx) / power(&err)).log10();
        assert!(snr > 20.0, "SNR {snr:.1} dB");
    }

    /// LINK-13.1: the step's low-pass: 6 kHz (which would fold onto 2 kHz at 8 kHz) is 60 dB down or more.
    #[test]
    fn above_the_band_is_filtered() {
        let x = tone(6000.0, 24_000, 12_000.0);
        let b = blocks(&x);
        let mut out = [0i16; BLOCK];
        let mut got = Vec::new();
        for blk in &b[4..] {
            decode(blk.start, &blk.codes, &mut out);
            got.extend_from_slice(&out);
        }
        let db = 10.0 * (power(&got) / (12_000.0f64.powi(2) / 2.0)).log10();
        assert!(db < -45.0, "6 kHz at {db:.1} dB");
    }

    /// The coder's state stays in range on full-scale square waves and silence.
    #[test]
    fn coder_stays_in_range() {
        let mut s = State::default();
        for i in 0..10_000 {
            let x = if (i / 7) % 2 == 0 { 32767 } else { -32768 };
            let c = s.encode(x);
            assert!(c < 16 && s.index <= 88);
        }
        let mut s = State::default();
        for _ in 0..1000 {
            s.encode(0);
        }
        assert!(s.pred.abs() < 64 && s.index == 0);
    }
}
