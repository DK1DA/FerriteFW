//! The bench's raw receive audio (LINK-13.9, decision 307): the decoders' own 24 kHz samples to the PC as plain
//! 12-bit PCM, for measurements Console's µ-law stream ([`crate::rxaudio`]) can't carry (tone sweeps to 8 kHz, bit
//! error rates of fast data). A bench build's only; Console's stream is unchanged.
//!
//! - **Why 12 bits at 12 or 16 kHz:** the cable carries about 23 KB/s sustained (230400 baud, 10 bits a byte;
//!   decision 266). The decoders' 24 kHz x 16 bits is 48 KB/s and even 12 bits at 24 kHz is 36 KB/s; a lossless
//!   entropy coder fits only on quiet, narrow audio and would drop blocks on the noise of an open squelch, which a
//!   bench capture can't have. So a fixed rate that always fits: 12 bits (ADC1's own resolution, 72 dB) at 12 kHz,
//!   19.1 KB/s on the wire with framing (83 % of 230400); or at 16 kHz (to 6.5 kHz, for sweeps to 6 kHz), 25.5
//!   KB/s, which needs the 460800 link and is above what the Mac's CH340 was measured to carry clean (about 23
//!   KB/s): the counts below say whether a capture lost anything.
//! - **The step:** a rational resampler from 24 kHz (2:1 to 12 kHz, 3:2 to 16 kHz) through a 64-tap low-pass at 48
//!   kHz (Kaiser-windowed sinc, beta 6), kept as its two 32-tap phases: flat (0.0 dB) to 4.5 kHz and 67 dB down
//!   from 7.5 kHz for 12 kHz; flat to 6.5 kHz, -2 dB at 7.5 kHz, 69 dB down from 10 kHz for 16 kHz. 32 multiplies a
//!   sample out (0.5 M a second at 16 kHz). The 16-bit result rounds to 12 bits (a full-scale swing at PA2 is
//!   +-2048); a block with a sample held at the limits says so.
//! - **Continuity:** each block carries the frame's sequence number (a gap: frames lost on the cable) and the index
//!   of its first sample since listening began (a gap with the sequence whole: blocks the radio dropped). Every
//!   block is sent, whatever the squelch (the slot and flags as [`crate::rxaudio::source`] gives them).

/// The `AUDIO` request's raw modes (LINK-13.9): 12 kHz and 16 kHz.
pub const MODE_12K: u8 = 2;
pub const MODE_16K: u8 = 3;

/// A raw mode's rate, `None` for any other mode.
pub fn rate(mode: u8) -> Option<u32> {
    match mode {
        MODE_12K => Some(12_000),
        MODE_16K => Some(16_000),
        _ => None,
    }
}

/// Samples a block: 12.7 ms at 12 kHz, 9.5 ms at 16 kHz.
pub const BLOCK: usize = 152;
/// A block's samples, two in three bytes.
pub const BLOCK_BYTES: usize = BLOCK * 3 / 2;
/// `AUDIO_RAW`'s head: seq (u16), the first sample's index (u32), slot, flags.
pub const HEAD: usize = 8;
/// A flag beside [`crate::rxaudio`]'s: a sample in this block was held at the 12-bit limits.
pub const CLIPPED: u8 = 0x80;

const NH: usize = 32;
/// The 64-tap low-pass (48 kHz, cutoff 6 kHz) for 12 kHz: only its even phase is ever used (the outputs fall on
/// inputs), oldest sample first, summing to 32768. Named statics: read per sample (fast flash).
static P12: [i16; NH] = [
    -4, -24, 20, 85, -57, -211, 128, 442, -255, -844, 477, 1574, -910, -3222, 2205, 12769, 15958, 7579, -1678, -2636,
    767, 1344, -408, -722, 217, 371, -106, -171, 45, 64, -14, -15,
];
/// The 64-tap low-pass (48 kHz, cutoff 8 kHz) for 16 kHz, as its two phases: an output on an input takes the first,
/// one between two inputs the second. Oldest sample first, each summing to 32768.
static P16: [[i16; NH]; 2] = [
    [
        10, -13, -26, 91, -74, -114, 335, -239, -333, 914, -622, -851, 2377, -1744, -2881, 13820, 20847, 4101, -4383,
        1427, 1002, -1454, 533, 391, -566, 201, 139, -185, 59, 35, -37, 8,
    ],
    [
        8, -37, 35, 59, -185, 139, 201, -566, 391, 533, -1454, 1002, 1427, -4383, 4101, 20847, 13820, -2881, -1744,
        2377, -851, -622, 914, -333, -239, 335, -114, -74, 91, -26, -13, 10,
    ],
];

/// The step from 24 kHz: time in 48 kHz ticks, an input every 2, an output every `step` (4 or 3).
#[derive(Clone, Copy)]
pub struct Resamp {
    hist: [i16; NH],
    at: u8,
    /// The newest input's time less the next output's, in ticks: an output is due when it reaches 0 or 1.
    lag: i8,
    step: u8,
}

impl Resamp {
    /// For a raw mode (12 kHz unless [`MODE_16K`]).
    pub const fn new(mode: u8) -> Self {
        Resamp { hist: [0; NH], at: 0, lag: -2, step: if mode == MODE_16K { 3 } else { 4 } }
    }

    /// A 24 kHz sample in; at most one sample out (Q15 of the input's scale, before rounding: the accumulator).
    #[inline]
    pub fn push(&mut self, x: i16) -> Option<i32> {
        self.hist[self.at as usize] = x;
        self.at = if self.at as usize + 1 == NH { 0 } else { self.at + 1 };
        self.lag += 2;
        if self.lag < 0 {
            return None;
        }
        let taps = if self.step == 4 { &P12 } else { &P16[self.lag as usize] };
        self.lag -= self.step as i8;
        let mut acc = 0i32;
        let mut k = self.at as usize; // the oldest
        for &t in taps.iter() {
            acc += t as i32 * self.hist[k] as i32;
            k = if k + 1 == NH { 0 } else { k + 1 };
        }
        Some(acc)
    }
}

/// The accumulator (Q15 of a 16-bit sample) rounded to 12 bits; `true` if held at a limit.
#[inline]
pub fn to12(acc: i32) -> (i16, bool) {
    let q = (acc + (1 << 18)) >> 19;
    (q.clamp(-2048, 2047) as i16, !(-2048..=2047).contains(&q))
}

/// Two 12-bit samples into three bytes: `a | b << 12`, little-endian.
#[inline]
pub fn pack2(a: i16, b: i16, out: &mut [u8]) {
    let w = (a as u32 & 0xFFF) | (b as u32 & 0xFFF) << 12;
    out[0] = w as u8;
    out[1] = (w >> 8) as u8;
    out[2] = (w >> 16) as u8;
}

/// Three bytes back to their two samples.
pub fn unpack2(p: &[u8]) -> (i16, i16) {
    let w = p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16;
    let s = |v: u32| (((v & 0xFFF) as i16) << 4) >> 4;
    (s(w), s(w >> 12))
}

/// One block for the link.
#[derive(Clone, Copy)]
pub struct Block {
    /// The first sample's index since listening began.
    pub first: u32,
    pub clipped: bool,
    pub bytes: [u8; BLOCK_BYTES],
}

/// The radio's side: decoder samples in ([`Packer::push`]), blocks out. About 300 bytes.
#[derive(Clone, Copy)]
pub struct Packer {
    rs: Resamp,
    /// Samples made since listening began.
    made: u32,
    /// The first of a pair, waiting for the second.
    odd: Option<i16>,
    block: Block,
}

impl Packer {
    pub const fn new(mode: u8) -> Self {
        Packer { rs: Resamp::new(mode), made: 0, odd: None, block: Block { first: 0, clipped: false, bytes: [0; BLOCK_BYTES] } }
    }

    /// A 24 kHz decoder sample in; a whole block out through `out` every [`BLOCK`] samples.
    #[inline]
    pub fn push(&mut self, x: i16, mut out: impl FnMut(&Block)) {
        let Some(acc) = self.rs.push(x) else { return };
        let (y, clip) = to12(acc);
        let n = self.made.wrapping_sub(self.block.first) as usize;
        if n == 0 {
            self.block.clipped = false;
        }
        self.block.clipped |= clip;
        self.made = self.made.wrapping_add(1);
        match self.odd.take() {
            None => self.odd = Some(y),
            Some(a) => {
                let i = n / 2 * 3;
                pack2(a, y, &mut self.block.bytes[i..i + 3]);
                if n + 1 == BLOCK {
                    out(&self.block);
                    self.block.first = self.made;
                }
            }
        }
    }
}

/// An `AUDIO_RAW` payload into `out`: its length.
pub fn put(seq: u16, slot: u8, flags: u8, b: &Block, out: &mut [u8]) -> usize {
    out[..2].copy_from_slice(&seq.to_le_bytes());
    out[2..6].copy_from_slice(&b.first.to_le_bytes());
    out[6] = slot;
    out[7] = flags | if b.clipped { CLIPPED } else { 0 };
    out[HEAD..HEAD + BLOCK_BYTES].copy_from_slice(&b.bytes);
    HEAD + BLOCK_BYTES
}

/// A whole `AUDIO_RAW` link frame into `w` (framed as every link frame is): its length, or `None` if `w` is too small.
pub fn frame(seq: u16, slot: u8, flags: u8, b: &Block, w: &mut [u8]) -> Option<usize> {
    let mut p = [0u8; HEAD + BLOCK_BYTES];
    let n = put(seq, slot, flags, b, &mut p);
    crate::link::build(crate::link::msg::AUDIO_RAW, 0, &p[..n], w)
}

/// An `AUDIO_RAW` payload read back: seq, the first sample's index, slot, flags and the packed samples.
pub fn get(p: &[u8]) -> Option<(u16, u32, u8, u8, &[u8])> {
    let h = p.get(..HEAD)?;
    Some((u16::from_le_bytes([h[0], h[1]]), u32::from_le_bytes([h[2], h[3], h[4], h[5]]), h[6], h[7], &p[HEAD..]))
}

/// The PC's check of a block against the last (LINK-13.9): frames lost on the cable (a gap in seq) and samples
/// missing (a gap in the first sample's index: the cable's losses and the radio's drops together).
pub fn gap(last: Option<(u16, u32, u16)>, seq: u16, first: u32) -> (u16, u32) {
    match last {
        None => (0, 0),
        Some((s, f, n)) => (seq.wrapping_sub(s).wrapping_sub(1), first.wrapping_sub(f).wrapping_sub(n as u32)),
    }
}

const _: () = assert!(HEAD + BLOCK_BYTES <= crate::link::MAX_PAYLOAD && BLOCK.is_multiple_of(2));

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn tone(hz: f64, n: usize, amp: f64) -> Vec<i16> {
        (0..n).map(|i| (amp * (2.0 * core::f64::consts::PI * hz * i as f64 / 24_000.0).sin()) as i16).collect()
    }

    /// Blocks made from `x`, framed, read back as the PC reads them: the samples, and each block's seq and first.
    fn through_link(mode: u8, x: &[i16]) -> (Vec<i16>, Vec<(u16, u32)>) {
        let mut p = Packer::new(mode);
        let mut blocks = Vec::new();
        for &s in x {
            p.push(s, |b| blocks.push(*b));
        }
        let (mut got, mut heads) = (Vec::new(), Vec::new());
        for (i, b) in blocks.iter().enumerate() {
            let mut w = [0u8; crate::link::MAX_WIRE];
            let n = frame(i as u16, 1, 1, b, &mut w).unwrap();
            let f = crate::link::parse(&mut w[..n - 1]).expect("a whole frame");
            assert_eq!(f.ty, crate::link::msg::AUDIO_RAW);
            let (seq, first, slot, flags, bytes) = get(f.payload).unwrap();
            assert_eq!((slot, flags & !CLIPPED), (1, 1));
            assert_eq!(bytes.len(), BLOCK_BYTES);
            heads.push((seq, first));
            for c in bytes.chunks(3) {
                let (a, b) = unpack2(c);
                got.extend_from_slice(&[a, b]);
            }
        }
        (got, heads)
    }

    fn power(x: &[i16]) -> f64 {
        x.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / x.len() as f64
    }

    /// LINK-13.9: every 12-bit pair packs into three bytes and back bit-exactly.
    #[test]
    fn pairs_round_trip() {
        let mut b = [0u8; 3];
        for a in -2048..2048i16 {
            for c in [-2048i16, -1, 0, 1, 2047, a / 3, -a] {
                let c = c.clamp(-2048, 2047);
                pack2(a, c, &mut b);
                assert_eq!(unpack2(&b), (a, c));
            }
        }
    }

    /// LINK-13.9: what the PC reads is bit for bit what the resampler made (rounded to 12 bits), the rate is exact
    /// (24,000 in, 12,000 or 16,000 out) and the blocks' heads count on without gaps.
    #[test]
    fn stream_is_exact_and_continuous() {
        let x = tone(1234.0, 24_000 * 2, 15_000.0);
        for mode in [MODE_12K, MODE_16K] {
            let mut rs = Resamp::new(mode);
            let want: Vec<i16> = x.iter().filter_map(|&s| rs.push(s)).map(|a| to12(a).0).collect();
            assert_eq!(want.len() as u32, rate(mode).unwrap() * 2);
            let (got, heads) = through_link(mode, &x);
            assert_eq!(got.len(), want.len() / BLOCK * BLOCK);
            assert_eq!(got, want[..got.len()]);
            let mut last = None;
            for &(seq, first) in &heads {
                assert_eq!(gap(last, seq, first), (0, 0));
                last = Some((seq, first, BLOCK as u16));
            }
        }
    }

    /// LINK-13.9: a frame lost on the cable shows in both counts; a block the radio dropped (seq whole) only in the
    /// samples'.
    #[test]
    fn gaps_are_found() {
        let n = BLOCK as u16;
        assert_eq!(gap(Some((7, 1000, n)), 8, 1000 + BLOCK as u32), (0, 0));
        assert_eq!(gap(Some((7, 1000, n)), 9, 1000 + 2 * BLOCK as u32), (1, BLOCK as u32));
        assert_eq!(gap(Some((7, 1000, n)), 8, 1000 + 3 * BLOCK as u32), (0, 2 * BLOCK as u32));
        assert_eq!(gap(Some((0xFFFF, u32::MAX - 151, n)), 0, 0), (0, 0), "both wrap");
    }

    /// LINK-13.9: the passbands and what would fold into them: 4 kHz (12 kHz mode) and 6 kHz (16 kHz mode) pass
    /// within 0.5 dB; 8 kHz (onto 4 kHz at 12 kHz) and 10.5 kHz (onto 5.5 kHz at 16 kHz) 60 dB down or more.
    #[test]
    fn passband_and_folding() {
        let amp = 20_000.0;
        let full = (amp / 16.0f64).powi(2) / 2.0; // the tone's power in 12-bit units
        for (mode, hz, lo, hi) in [(MODE_12K, 4000.0, -0.5, 0.5), (MODE_16K, 6000.0, -0.5, 0.5), (MODE_12K, 8000.0, -200.0, -60.0), (MODE_16K, 10_500.0, -200.0, -60.0)] {
            let (got, _) = through_link(mode, &tone(hz, 24_000, amp));
            let db = 10.0 * ((power(&got[200..]) + 1e-9) / full).log10();
            assert!(db > lo && db < hi, "mode {mode} {hz} Hz at {db:.2} dB");
        }
    }

    /// LINK-13.9: a swing past PA2's full scale is held at 12 bits and the block says so.
    #[test]
    fn clipping_is_flagged() {
        let mut p = Packer::new(MODE_12K);
        let mut flags = Vec::new();
        for s in tone(1000.0, 4_000, 32_767.0).into_iter().chain(core::iter::repeat_n(0, 4_000)) {
            p.push(s.saturating_mul(2), |b| flags.push(b.clipped));
        }
        assert!(flags[1] && !flags[flags.len() - 1]);
    }
}
