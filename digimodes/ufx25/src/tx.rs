//! FX.25 transmit: an AX.25 frame to the bits to send (before NRZI).

use crate::rs::Rs;
use crate::{Mode, FLAG, MAX_BLOCK, MODES};
use udsp::crc::crc16_x25;

/// The bit-stuffed frame does not fit the largest block of the chosen code (239, 223 or 191 bytes with 16, 32 or
/// 64 check bytes, flags included). Send it as plain AX.25 instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLong;

/// One FX.25 block, built and Reed-Solomon encoded. RAM: 272 bytes (the 255-byte block and the mode).
///
/// The information part holds, least significant bit first like every byte on air: a flag, the bit-stuffed frame
/// and FCS, a closing flag, then more flag bits (continuing the flag pattern, not byte-aligned) to fill the
/// block. Those pad bits are what Dire Wolf sends; the specification recommends 0x7E pad bytes after a partial
/// flag; to a receiver both are idle flags.
pub struct Encoder {
    block: [u8; MAX_BLOCK],
    mode: Mode,
}

impl Encoder {
    /// Wrap `frame` (AX.25, without FCS: the FCS is added here, low byte first, as `uafsk::hdlc::Framer` does) in
    /// the smallest block with `check` check bytes (16, 32 or 64; any other value panics) that holds it.
    pub fn new(frame: &[u8], check: usize) -> Result<Self, TooLong> {
        assert!(matches!(check, 16 | 32 | 64), "FX.25 uses 16, 32 or 64 check bytes");
        let fcs = crc16_x25(frame).to_le_bytes();
        let bytes = || frame.iter().chain(fcs.iter()).copied();
        let bits = 16 + stuffed_bits(bytes());
        let mode = MODES
            .iter()
            .filter(|m| m.check() == check && m.k as usize * 8 >= bits)
            .min_by_key(|m| m.k)
            .copied()
            .ok_or(TooLong)?;
        let mut enc = Encoder { block: [0; MAX_BLOCK], mode };
        let k = mode.k as usize;
        let mut w = BitWriter { buf: &mut enc.block[..k], pos: 0 };
        w.byte(FLAG);
        let mut ones = 0;
        for b in bytes() {
            for i in 0..8 {
                let bit = (b >> i) & 1;
                w.bit(bit);
                if bit == 1 {
                    ones += 1;
                    if ones == 5 {
                        w.bit(0);
                        ones = 0;
                    }
                } else {
                    ones = 0;
                }
            }
        }
        // Closing flag, then flag bits to the end of the information part.
        let mut i = 0;
        while w.pos < k * 8 {
            w.bit((FLAG >> (i & 7)) & 1);
            i += 1;
        }
        Rs::new(check).encode(&mut enc.block[..mode.n as usize], k);
        Ok(enc)
    }

    /// The code chosen.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The block as sent after the tag: information then check bytes.
    pub fn block(&self) -> &[u8] {
        &self.block[..self.mode.n as usize]
    }

    /// The block, to change (tests inject errors here).
    pub fn block_mut(&mut self) -> &mut [u8] {
        &mut self.block[..self.mode.n as usize]
    }

    /// The bits to send, one per item (0 or 1), before NRZI: `pre` flags (the specification asks for at least
    /// 4; this is TXDELAY), the 64-bit tag, the block, `post` flags (at least 2).
    pub fn bits(&self, pre: u16, post: u16) -> Bits<'_> {
        Bits { enc: self, tag: self.mode.tag, pre: pre as usize * 8, post: post as usize * 8, pos: 0 }
    }
}

/// Bits of a stuffed frame (no flags).
fn stuffed_bits(bytes: impl Iterator<Item = u8>) -> usize {
    let (mut n, mut ones) = (0, 0);
    for b in bytes {
        for i in 0..8 {
            n += 1;
            if (b >> i) & 1 == 1 {
                ones += 1;
                if ones == 5 {
                    n += 1;
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
    }
    n
}

struct BitWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl BitWriter<'_> {
    fn bit(&mut self, b: u8) {
        let (i, s) = (self.pos / 8, self.pos % 8);
        if s == 0 {
            self.buf[i] = 0;
        }
        self.buf[i] |= b << s;
        self.pos += 1;
    }
    fn byte(&mut self, b: u8) {
        for i in 0..8 {
            self.bit((b >> i) & 1);
        }
    }
}

/// The bit sequence of one FX.25 transmission; see [`Encoder::bits`].
pub struct Bits<'a> {
    enc: &'a Encoder,
    tag: u64,
    pre: usize,
    post: usize,
    pos: usize,
}

impl Iterator for Bits<'_> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        let n = self.enc.mode.n as usize * 8;
        let p = self.pos;
        let bit = if p < self.pre {
            (FLAG >> (p & 7)) & 1
        } else if p < self.pre + 64 {
            ((self.tag >> (p - self.pre)) & 1) as u8
        } else if p < self.pre + 64 + n {
            let q = p - self.pre - 64;
            (self.enc.block[q / 8] >> (q % 8)) & 1
        } else if p < self.pre + 64 + n + self.post {
            (FLAG >> ((p - self.pre - 64 - n) & 7)) & 1
        } else {
            return None;
        };
        self.pos += 1;
        Some(bit)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.pre + 64 + self.enc.mode.n as usize * 8 + self.post - self.pos;
        (left, Some(left))
    }
}

impl ExactSizeIterator for Bits<'_> {}
