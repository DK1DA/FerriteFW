//! Receive audio to the PC over the cable (LINK-13.1, LINK-13.4; decisions 213, 288, 330): the decoders' 24 kHz
//! samples stepped down 2:1 to 12 kHz through a half-band low-pass (flat to 4.5 kHz, -6 dB at 6 kHz), coded as
//! G.711 µ-law (8 bits a sample, about 12 KB/s) in blocks of [`BLOCK`] samples (16 ms). About 38 dB on a tone or on
//! speech. A block decodes on its own, so a block lost on the way costs only itself. The radio codes ([`Packer`]),
//! Ferrite Console decodes ([`ulaw_decode`]); both are this file, so they can't disagree. (Until decision 330 the
//! stream was 8 kHz IMA ADPCM, about 18 dB.)

/// The stream's sample rate.
pub const RATE_HZ: u32 = 12_000;
/// Samples (and bytes) a block: 16 ms, what fits a link frame (240 bytes of payload at most).
pub const BLOCK: usize = 192;

/// One block for the link: its µ-law codes, a byte a sample.
pub type Block = [u8; BLOCK];

/// `AUDIO_DATA`'s head: seq (u16), slot, flags (LINK-13.4).
pub const HEAD: usize = 4;
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
    out[..2].copy_from_slice(&seq.to_le_bytes());
    (out[2], out[3]) = (slot, flags);
    match blk {
        Some(b) => {
            out[HEAD..HEAD + BLOCK].copy_from_slice(b);
            HEAD + BLOCK
        }
        None => HEAD,
    }
}

/// A whole `AUDIO_DATA` link frame into `w` (the wire bytes, framed as every link frame is): its length, or `None`
/// if `w` is too small. The DSP task sends these itself (decision 290), so audio never waits for the UI task.
pub fn frame(seq: u16, slot: u8, flags: u8, blk: Option<&Block>, w: &mut [u8]) -> Option<usize> {
    let mut p = [0u8; HEAD + BLOCK];
    let n = put(seq, slot, flags, blk, &mut p);
    crate::link::build(crate::link::msg::AUDIO_DATA, 0, &p[..n], w)
}

/// An `AUDIO_DATA` payload read back: seq, slot, flags, the codes (empty: a keep-alive).
pub fn get(p: &[u8]) -> Option<(u16, u8, u8, &[u8])> {
    let h = p.get(..HEAD)?;
    Some((u16::from_le_bytes([h[0], h[1]]), h[2], h[3], &p[HEAD..]))
}

/// G.711 µ-law (the 16-bit form: bias 0x84, clipped at 32635): a sample to its byte, by bit operations (no table).
#[inline]
pub fn ulaw_encode(x: i16) -> u8 {
    let mut s = x as i32;
    let sign = if s < 0 {
        s = -s;
        0x80
    } else {
        0
    };
    let s = s.min(32635) + 0x84; // 132 ..= 32767: the top bit is bit 7 to 14
    let exp = 24 - (s as u32).leading_zeros(); // 0 ..= 7
    let mant = (s >> (exp + 3)) as u32 & 15;
    !(sign | exp << 4 | mant) as u8
}

/// A µ-law byte to its sample (at most +-32124).
#[inline]
pub fn ulaw_decode(u: u8) -> i16 {
    let u = !u;
    let t = ((((u & 15) as i32) << 3) + 0x84) << ((u >> 4) & 7);
    (if u & 0x80 != 0 { 0x84 - t } else { t - 0x84 }) as i16
}

const HB_TAPS: usize = 31;
/// The half-band low-pass ahead of the 2:1 step (24 kHz to 12 kHz, decision 330): 31 taps, Kaiser-windowed sinc
/// (beta 5, 6 kHz), Q15, summing to 1; every other tap is 0, so these are the taps either side of the centre,
/// nearest first ([`HB_MID`] the centre). Flat to 4.5 kHz, -0.3 dB at 5 kHz, -1.8 at 5.5 kHz, -6 at 6 kHz (the new
/// rate's edge), -30 at 7 kHz and 60 dB or more from 8 kHz: what folds onto the audio below 5 kHz is 30 dB down
/// here and over 55 dB with the decoders' own filter ahead of it ([`crate::audio::Decim`]). 9 multiplies an output
/// sample. A named `static`: read per sample.
static HB: [i32; 8] = [10328, -3177, 1618, -893, 481, -236, 96, -26];
const HB_MID: i32 = 16386;

/// The radio's coder (decision 330): 24 kHz samples in ([`Packer::push`]), 12 kHz µ-law blocks out. About 320
/// bytes.
pub struct Packer {
    /// The last [`HB_TAPS`] inputs twice over, so the window is always one straight run.
    hist: [i16; 2 * HB_TAPS],
    at: u8,
    odd: bool,
    n: u8,
    block: Block,
}

impl Default for Packer {
    fn default() -> Self {
        Self::new()
    }
}

impl Packer {
    pub const fn new() -> Self {
        Packer { hist: [0; 2 * HB_TAPS], at: 0, odd: false, n: 0, block: [0; BLOCK] }
    }

    /// A 24 kHz sample in; a whole block out through `out` every [`BLOCK`] 12 kHz samples.
    #[inline]
    pub fn push(&mut self, x: i16, mut out: impl FnMut(&Block)) {
        self.at = if self.at as usize + 1 == HB_TAPS { 0 } else { self.at + 1 };
        let a = self.at as usize;
        self.hist[a] = x;
        self.hist[a + HB_TAPS] = x;
        self.odd = !self.odd;
        if self.odd {
            return;
        }
        let w = &self.hist[a + 1..a + 1 + HB_TAPS]; // oldest first
        const C: usize = HB_TAPS / 2;
        let mut acc = HB_MID * w[C] as i32;
        for (k, t) in HB.iter().enumerate() {
            let d = 2 * k + 1;
            acc += *t * (w[C - d] as i32 + w[C + d] as i32);
        }
        let y = (acc >> 15).clamp(-32768, 32767) as i16;
        self.block[self.n as usize] = ulaw_encode(y);
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

    fn power(x: &[i16]) -> f64 {
        x.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / x.len() as f64
    }

    /// The stream's samples: what [`Packer`] was given, filtered and stepped 2:1 (the reference).
    fn halfband_ref(x: &[i16]) -> Vec<i16> {
        let mut taps = [0i32; HB_TAPS];
        taps[HB_TAPS / 2] = HB_MID;
        for (k, t) in HB.iter().enumerate() {
            taps[HB_TAPS / 2 - 2 * k - 1] = *t;
            taps[HB_TAPS / 2 + 2 * k + 1] = *t;
        }
        assert_eq!(taps.iter().sum::<i32>(), 32768, "the taps sum to 1");
        (1..x.len())
            .step_by(2)
            .map(|i| {
                let acc: i32 = (0..HB_TAPS).map(|t| taps[t] * if i + 1 >= HB_TAPS - t { x[i + 1 + t - HB_TAPS] as i32 } else { 0 }).sum();
                (acc >> 15).clamp(-32768, 32767) as i16
            })
            .collect()
    }

    fn blocks(x: &[i16]) -> Vec<Block> {
        let mut p = Packer::new();
        let mut v = Vec::new();
        for &s in x {
            p.push(s, |b| v.push(*b));
        }
        v
    }

    fn samples(b: &[Block]) -> Vec<i16> {
        b.iter().flat_map(|b| b.iter().map(|&u| ulaw_decode(u))).collect()
    }

    fn snr(got: &[i16], refx: &[i16]) -> f64 {
        let err: Vec<i16> = got.iter().zip(refx).map(|(a, b)| a.saturating_sub(*b)).collect();
        10.0 * (power(refx) / power(&err)).log10()
    }

    /// LINK-13.4 (decision 330): µ-law is G.711's: known values both ways, every byte decodes and codes back to
    /// itself (bar the two zeros), and the coding error stays within half a step of its segment.
    #[test]
    fn ulaw_is_g711() {
        assert_eq!((ulaw_encode(0), ulaw_encode(-1), ulaw_encode(32767), ulaw_encode(-32768)), (0xFF, 0x7F, 0x80, 0x00));
        assert_eq!((ulaw_decode(0xFF), ulaw_decode(0x80), ulaw_decode(0x00)), (0, 32124, -32124));
        for u in 0..=255u8 {
            let x = ulaw_decode(u);
            let back = ulaw_encode(x);
            assert!(back == u || (x == 0 && (back == 0xFF || back == 0x7F)), "{u:#x} -> {x} -> {back:#x}");
        }
        for x in (-32768i32..=32767).step_by(7) {
            let y = ulaw_decode(ulaw_encode(x as i16)) as i32;
            let seg = 24 - ((x.unsigned_abs().min(32635) + 0x84).leading_zeros() as i32);
            assert!((x.clamp(-32635, 32635) - y).abs() <= 4 << seg, "{x} -> {y}");
        }
    }

    /// LINK-13.1, LINK-13.4 (decision 330): a 1 kHz tone through the 2:1 step and µ-law, each block decoded alone,
    /// matches the filtered tone to 35 dB or better (8-bit µ-law: about 38-40 dB), 12,000 samples a second in 16 ms
    /// blocks; a quiet one (-30 dBFS) keeps 33 dB (µ-law's steps scale with the level).
    #[test]
    fn tone_round_trips_block_by_block() {
        let x = tone(1000.0, 24_000, 12_000.0);
        let b = blocks(&x);
        assert_eq!(b.len(), 12_000 / BLOCK);
        let got = samples(&b);
        let refx = halfband_ref(&x);
        let s = snr(&got[600..], &refx[600..got.len()]); // past the filter's start
        assert!(s >= 35.0, "SNR {s:.1} dB");
        let x = tone(1000.0, 24_000, 1_000.0);
        let got = samples(&blocks(&x));
        let refx = halfband_ref(&x);
        let s = snr(&got[600..], &refx[600..got.len()]);
        assert!(s >= 33.0, "quiet SNR {s:.1} dB");
    }

    /// LINK-13.1 (decision 330): the 2:1 step's half-band: flat to 4.5 kHz (within 0.2 dB), 5 kHz within 0.5 dB, 7
    /// kHz (which would fold onto 5 kHz) 25 dB down or more, 8 kHz (onto 4 kHz) 55 dB, 10 kHz 60; and the packer
    /// gives the reference's samples through the coder.
    #[test]
    fn above_the_band_is_filtered() {
        let level = |hz: f64| {
            let y = halfband_ref(&tone(hz, 24_000, 12_000.0));
            10.0 * (power(&y[600..]) / (12_000.0f64.powi(2) / 2.0)).log10()
        };
        for hz in [300.0, 1000.0, 2000.0, 3000.0, 4000.0, 4500.0] {
            assert!(level(hz).abs() < 0.2, "{hz} Hz at {:.2} dB", level(hz));
        }
        assert!(level(5000.0).abs() < 0.5, "5 kHz at {:.2} dB", level(5000.0));
        assert!(level(7000.0) < -25.0, "7 kHz at {:.1} dB", level(7000.0));
        assert!(level(8000.0) < -55.0, "8 kHz at {:.1} dB", level(8000.0));
        assert!(level(10_000.0) < -60.0, "10 kHz at {:.1} dB", level(10_000.0));
        let x = tone(3000.0, 4_800, 12_000.0);
        let got = samples(&blocks(&x));
        let refx = halfband_ref(&x);
        assert!(got.iter().zip(&refx).all(|(a, b)| *a == ulaw_decode(ulaw_encode(*b))));
    }

    /// LINK-13.4, LINK-13.6 (decision 330): a block goes out as one link frame (196 bytes of payload, within the
    /// link's 240) and reads back through the link's own parser to its bytes; a keep-alive has no codes; a payload
    /// shorter than the head is none; too small a buffer for the wire is none.
    #[test]
    fn block_frames_through_the_link() {
        let b = blocks(&tone(440.0, 2 * 2 * BLOCK, 8_000.0));
        assert_eq!(b.len(), 2);
        let mut w = [0u8; crate::link::MAX_WIRE];
        let n = frame(0x0203, 1, SQ_OPEN, Some(&b[1]), &mut w).unwrap();
        assert!(HEAD + BLOCK <= crate::link::MAX_PAYLOAD && n <= crate::link::MAX_WIRE);
        let mut wire: Vec<u8> = w[..n].iter().copied().filter(|&c| c != 0).collect(); // COBS: the 0 is the delimiter only
        assert_eq!(wire.len(), n - 1);
        let f = crate::link::parse(&mut wire).unwrap();
        assert_eq!(f.ty, crate::link::msg::AUDIO_DATA);
        assert_eq!(get(f.payload), Some((0x0203, 1, SQ_OPEN, &b[1][..])));
        let mut p = [0u8; HEAD + BLOCK];
        let k = put(9, NO_SLOT, 0, None, &mut p);
        assert_eq!(get(&p[..k]), Some((9, NO_SLOT, 0, &[][..])));
        assert_eq!(get(&p[..HEAD - 1]), None);
        let mut w = [0u8; 8];
        assert_eq!(frame(1, 0, 0, Some(&b[0]), &mut w), None, "too small for the wire");
    }

    /// A fresh packer gives silence for silence, a block every 384 inputs.
    #[test]
    fn silence_is_silence() {
        let mut v = Vec::new();
        let mut p = Packer::default();
        for _ in 0..2 * BLOCK + 1 {
            p.push(0, |b| v.push(*b));
        }
        assert_eq!(v.len(), 1);
        assert!(v[0].iter().all(|&u| ulaw_decode(u) == 0));
    }
}
