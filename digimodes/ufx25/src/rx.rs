//! FX.25 receive: data bits in, corrected frames out.

use crate::rs::Rs;
use crate::{Mode, MAX_BLOCK, MODES};
use udsp::crc::{crc16_x25_update, CRC16_X25_GOOD};

/// Wrong bits accepted in a correlation tag by default: 8, as Dire Wolf (measured: it decodes a tag with 8 wrong
/// bits as FX.25, not one with 9). The tags are at least 32 bits apart; random bits come within 8 bits of one of
/// the 11 tags about once in 3·10^8 bits (3 days of continuous noise at 1200 bit/s), and a false start only costs
/// listening to one block's worth of bits (the plain AX.25 deframer runs alongside regardless).
pub const DEFAULT_TAG_ERRORS: u8 = 8;

/// Shortest AX.25 frame (two addresses, control, FCS).
const MIN_FRAME: usize = 17;

/// A frame recovered from an FX.25 block.
#[derive(Debug)]
pub struct Decoded<'a> {
    /// The AX.25 frame without its FCS.
    pub frame: &'a [u8],
    /// The frame's FCS (to spot the same frame from the plain AX.25 deframer).
    pub fcs: u16,
    /// Bytes the Reed-Solomon code corrected.
    pub corrected: u8,
    /// Wrong bits in the correlation tag.
    pub tag_errors: u8,
    /// The code (tag) the block used.
    pub mode: Mode,
}

/// A correlation tag found by a [`Hunter`]: the block of `mode.n` bytes starts with the next bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tag {
    pub mode: Mode,
    /// Wrong bits in the tag.
    pub errors: u8,
}

/// The tag hunt on its own: one per slicer, 16 bytes of RAM (the newest 64 bits, the 32 before them, state).
///
/// While hunting, each bit costs two shifts and a look for a flag in the 24 bits before the newest 64: a tag
/// follows the preamble's flags (or, in the specification's multi-frame blocks, the previous block), so the 64
/// bits are compared with the 11 tags only behind a flag (one of the last three, so a damaged flag or two still
/// count); in noise that is about one bit in 85. After a tag it lets the block's bits go by (a [`Collector`] keeps
/// them, if one is free) and hunts again after it, as if flags had come before whatever follows.
///
/// Several slicers each with a hunter can share one [`Collector`]: the first to find a tag claims it.
#[derive(Clone, Copy)]
pub struct Hunter {
    /// The newest 64 bits (the newest in bit 63), and the 32 before them.
    reg: u64,
    prev: u32,
    /// Bits of the block in hand still to come; 0 while hunting.
    left: u16,
    /// Its code: index + 1 into [`MODES`].
    mode: u8,
}

/// Flags (0x7E, the same either bit order) repeated: what a receiver's history holds after a block, so a tag
/// straight after it (back-to-back blocks) is still looked for.
const FLAGS: u64 = 0x7E7E_7E7E_7E7E_7E7E;

impl Hunter {
    pub const fn new() -> Self {
        Hunter { reg: 0, prev: 0, left: 0, mode: 0 }
    }

    /// Feed one data bit (0 or 1); `max_errors` wrong bits accepted in a tag (0 to 15;
    /// [`DEFAULT_TAG_ERRORS`]). Returns the tag that ends with this bit; its block's `n·8` bits follow.
    #[inline]
    pub fn push(&mut self, bit: u8, max_errors: u8) -> Option<Tag> {
        if self.left != 0 {
            self.left -= 1;
            if self.left == 0 {
                // Block over: back to hunting, as if flags had come before whatever follows.
                self.mode = 0;
                self.reg = FLAGS;
            }
            return None;
        }
        self.prev = (self.prev >> 1) | ((self.reg as u32) << 31);
        self.reg = (self.reg >> 1) | (((bit & 1) as u64) << 63);
        let p = self.prev >> 8;
        if p & 0xFF == 0x7E || (p >> 8) & 0xFF == 0x7E || p >> 16 == 0x7E {
            return self.hunt(max_errors);
        }
        None
    }

    /// Compare the newest 64 bits with every tag. Out of line: most bits never get here.
    #[inline(never)]
    fn hunt(&mut self, max: u8) -> Option<Tag> {
        let max = max as u32;
        for (i, m) in MODES.iter().enumerate() {
            let x = self.reg ^ m.tag;
            // The low half first: most positions miss already there.
            if (x as u32).count_ones() <= max {
                let d = x.count_ones();
                if d <= max {
                    self.mode = i as u8 + 1;
                    self.left = m.n as u16 * 8;
                    return Some(Tag { mode: *m, errors: d as u8 });
                }
            }
        }
        None
    }

    /// The code of the block going by, if any.
    pub fn following(&self) -> Option<Mode> {
        (self.mode != 0).then(|| MODES[self.mode as usize - 1])
    }
}

impl Default for Hunter {
    fn default() -> Self {
        Self::new()
    }
}

/// The block behind a tag: collects its bits, then holds it until [`Collector::clear`] or the next
/// [`Collector::start`]. 260 bytes of RAM (the 255-byte block, state); `Copy`, to hand to another task.
#[derive(Clone, Copy)]
pub struct Collector {
    block: [u8; MAX_BLOCK],
    nbits: u16,
    /// The code (its `id`, which is its index + 1 into [`MODES`]); 0: empty.
    id: u8,
    tag_errors: u8,
    full: bool,
}

impl Collector {
    pub const fn new() -> Self {
        Collector { block: [0; MAX_BLOCK], nbits: 0, id: 0, tag_errors: 0, full: false }
    }

    /// Collect the block behind `tag` from the next bit on (whatever it held is dropped).
    pub fn start(&mut self, tag: Tag) {
        self.id = tag.mode.id;
        self.tag_errors = tag.errors;
        self.nbits = 0;
        self.full = false;
    }

    /// Feed one data bit of the block; returns the code when the block is complete. Ignored unless collecting.
    #[inline]
    pub fn push(&mut self, bit: u8) -> Option<Mode> {
        if self.id == 0 || self.full {
            return None;
        }
        let (i, s) = (self.nbits as usize / 8, self.nbits % 8);
        if s == 0 {
            self.block[i] = 0;
        }
        self.block[i] |= (bit & 1) << s;
        self.nbits += 1;
        let mode = MODES[self.id as usize - 1];
        if (self.nbits as usize) < mode.n as usize * 8 {
            return None;
        }
        self.full = true;
        Some(mode)
    }

    /// The code of the block being collected, if any.
    pub fn collecting(&self) -> Option<Mode> {
        (self.id != 0 && !self.full).then(|| MODES[self.id as usize - 1])
    }

    /// Bits of the block collected so far.
    pub fn bits(&self) -> u16 {
        self.nbits
    }

    /// The code of the complete block held, if any.
    pub fn mode(&self) -> Option<Mode> {
        (self.id != 0 && self.full).then(|| MODES[self.id as usize - 1])
    }

    /// The complete block (n bytes as received: information, then check bytes); empty if none.
    pub fn block(&self) -> &[u8] {
        match self.mode() {
            Some(m) => &self.block[..m.n as usize],
            None => &[],
        }
    }

    /// The same, to correct in place.
    pub fn block_mut(&mut self) -> &mut [u8] {
        match self.mode() {
            Some(m) => &mut self.block[..m.n as usize],
            None => &mut [],
        }
    }

    /// Wrong bits in the tag of the block being collected or held.
    pub fn tag_errors(&self) -> u8 {
        self.tag_errors
    }

    /// Empty: free for the next tag.
    pub fn clear(&mut self) {
        self.id = 0;
        self.full = false;
    }

    /// Correct the complete block in place and find its frame ([`decode`]); None if there is no complete block.
    /// The block stays held (corrected) until cleared.
    pub fn decode(&mut self) -> Option<Decoded<'_>> {
        let mode = self.mode()?;
        let te = self.tag_errors;
        decode(&mut self.block[..mode.n as usize], mode, te)
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

/// Bits in (after NRZI decoding), frames out: a [`Hunter`] and a [`Collector`]. RAM: 280 bytes.
///
/// After a tag it collects the block (n bytes), corrects it, and looks for a flag-delimited frame with a good FCS
/// in the information part.
///
/// [`Receiver::push`] decodes the block in the call that completes it (0.27 to 1.03 M instructions on a Cortex-M4).
/// [`Receiver::push_raw`] only collects: the block is left in [`Receiver::block`] for [`decode`] to correct later,
/// in some other task, so the per-bit path stays a few dozen instructions. Several slicers can share one
/// collector instead: a [`Hunter`] each (16 bytes) and one [`Collector`], as uafsk's demodulator does.
pub struct Receiver {
    hunter: Hunter,
    rx: Collector,
    max_tag_errors: u8,
}

impl Receiver {
    pub const fn new() -> Self {
        Self::with_tag_errors(DEFAULT_TAG_ERRORS)
    }

    /// Accept up to `max` wrong bits in a tag (0 to 15).
    pub const fn with_tag_errors(max: u8) -> Self {
        Receiver { hunter: Hunter::new(), rx: Collector::new(), max_tag_errors: max }
    }

    /// Collecting a block (to hold off transmitting, or to keep a decoder running until it finishes).
    pub fn busy(&self) -> bool {
        self.hunter.following().is_some()
    }

    /// The code of the block being collected, if any.
    pub fn collecting(&self) -> Option<Mode> {
        self.hunter.following()
    }

    /// Feed one data bit (0 or 1). Returns a frame when a block ends with one inside that passes its FCS. The
    /// Reed-Solomon decode runs in this call; see [`Receiver::push_raw`] to run it elsewhere.
    pub fn push(&mut self, bit: u8) -> Option<Decoded<'_>> {
        self.push_raw(bit)?;
        self.rx.decode()
    }

    /// Feed one data bit (0 or 1), without decoding: returns the code when a block is complete. The block, as
    /// received, is then in [`Receiver::block`] (its tag's wrong bits in [`Receiver::tag_errors`]) until this
    /// receiver finds its next tag, at least 64 bits later; correct it with [`decode`] (a copy, or in place
    /// through [`Receiver::block_mut`]).
    #[inline]
    pub fn push_raw(&mut self, bit: u8) -> Option<Mode> {
        if let Some(tag) = self.hunter.push(bit, self.max_tag_errors) {
            self.rx.start(tag);
            return None;
        }
        self.rx.push(bit)
    }

    /// The last block completed by [`Receiver::push_raw`] (n bytes as received: information, then check bytes);
    /// empty before the first. Valid until the next tag.
    pub fn block(&self) -> &[u8] {
        self.rx.block()
    }

    /// The same, to correct in place.
    pub fn block_mut(&mut self) -> &mut [u8] {
        self.rx.block_mut()
    }

    /// Wrong bits in the tag of the block being collected or last completed.
    pub fn tag_errors(&self) -> u8 {
        self.rx.tag_errors()
    }
}

/// Correct a received block (`block.len()` must be `mode.n`) and find the frame in it: the Reed-Solomon decode and
/// the deframing that [`Receiver::push`] runs when a block completes. 0.27 to 1.03 M instructions on a Cortex-M4
/// (16 check bytes and no errors, to 64 and 32 errors); up to about 0.5 KB of stack. `tag_errors` is passed
/// through to the result.
pub fn decode(block: &mut [u8], mode: Mode, tag_errors: u8) -> Option<Decoded<'_>> {
    let (n, k) = (mode.n as usize, mode.k as usize);
    assert_eq!(block.len(), n, "a block of the mode's length");
    let corrected = Rs::new(mode.check()).decode(block, k).ok()?;
    let len = deframe(&mut block[..k])?;
    let fcs = u16::from_le_bytes([block[len - 2], block[len - 1]]);
    Some(Decoded { frame: &block[..len - 2], fcs, corrected: corrected as u8, tag_errors, mode })
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the first flag-delimited frame with a good FCS in `buf` (bits least significant first, stuffed), and
/// leave it destuffed at the start of `buf`. Returns its length with the FCS. Works in place: an output byte is
/// always written behind the byte being read.
fn deframe(buf: &mut [u8]) -> Option<usize> {
    let (mut ones, mut byte, mut nbits, mut len, mut in_frame) = (0u8, 0u8, 0u8, 0usize, false);
    for pos in 0..buf.len() * 8 {
        let bit = (buf[pos / 8] >> (pos % 8)) & 1;
        if bit == 1 {
            ones += 1;
            if ones > 6 {
                in_frame = false;
                ones = 7;
                continue;
            }
        } else {
            match ones {
                5 => {
                    ones = 0;
                    continue;
                }
                6 => {
                    ones = 0;
                    // The flag's leading 0 and six 1s went into `byte`: an aligned frame has exactly 7 bits there.
                    if in_frame && nbits == 7 && len >= MIN_FRAME && crc16_x25_update(0xFFFF, &buf[..len]) == CRC16_X25_GOOD {
                        return Some(len);
                    }
                    in_frame = true;
                    len = 0;
                    nbits = 0;
                    continue;
                }
                _ => ones = 0,
            }
        }
        if in_frame {
            byte = (byte >> 1) | (bit << 7);
            nbits += 1;
            if nbits == 8 {
                buf[len] = byte;
                len += 1;
                nbits = 0;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Encoder;

    const FRAME: &[u8] = &[
        0x82, 0xA0, 0xA4, 0xA6, 0x40, 0x40, 0xE0, 0x9C, 0x60, 0x86, 0x82, 0x98, 0x98, 0xE1, 0x03, 0xF0, b'>', b'h',
        b'e', b'l', b'l', b'o', 0xFF, 0xFF, 0xFF, 0x7E, 0x7E,
    ];

    fn run(rx: &mut Receiver, bits: impl Iterator<Item = u8>) -> Vec<(Vec<u8>, u8, u8)> {
        let mut out = Vec::new();
        for b in bits {
            if let Some(d) = rx.push(b) {
                out.push((d.frame.to_vec(), d.corrected, d.tag_errors));
            }
        }
        out
    }

    extern crate std;
    use std::vec::Vec;

    #[test]
    fn round_trip_every_code() {
        for check in [16, 32, 64] {
            let mut sizes = Vec::new();
            for len in [17usize, 25, 40, 60, 100, 120, 150, 185, 200, 230] {
                let frame: Vec<u8> = (0..len).map(|i| FRAME[i % FRAME.len()]).collect();
                let Ok(mut enc) = Encoder::new(&frame, check) else {
                    continue;
                };
                sizes.push(enc.mode().id);
                let t = check / 2;
                let n = enc.block().len();
                for e in 0..t {
                    enc.block_mut()[(e * 37) % n] ^= 0x5A;
                }
                let mut rx = Receiver::new();
                let got = run(&mut rx, enc.bits(4, 2));
                assert_eq!(got, [(frame.clone(), t as u8, 0)], "check {check} len {len}");
            }
            sizes.dedup();
            assert_eq!(sizes.len(), 4 - (check == 64) as usize, "every block size used: {sizes:?}");
        }
    }

    #[test]
    fn too_long() {
        assert!(Encoder::new(&[0x55; 230], 64).is_err());
        assert!(Encoder::new(&[0x55; 230], 16).is_ok());
        // 0xFF bytes stuff: 200 of them are 240 bytes on air.
        assert!(Encoder::new(&[0xFF; 200], 16).is_err());
    }

    #[test]
    fn tag_errors_and_back_to_back() {
        let enc = Encoder::new(FRAME, 16).unwrap();
        let mut bits: Vec<u8> = enc.bits(4, 0).collect();
        // Flip 8 tag bits (accepted by default), then a second block straight after with 9 (not accepted) and a
        // third with none.
        for i in 0..8 {
            bits[32 + i * 7] ^= 1;
        }
        let mut second: Vec<u8> = enc.bits(0, 0).collect();
        for i in 0..9 {
            second[i * 7] ^= 1;
        }
        bits.extend(second);
        // The third after one flag: a tag is only looked for behind a flag or a completed block, and the second
        // block was never collected.
        bits.extend(enc.bits(1, 2));
        let mut rx = Receiver::new();
        let got = run(&mut rx, bits.into_iter());
        assert_eq!(got, [(FRAME.to_vec(), 0, 8), (FRAME.to_vec(), 0, 0)]);
    }

    #[test]
    fn tag_needs_a_flag_before_it() {
        let enc = Encoder::new(FRAME, 16).unwrap();
        // No flag in the 24 bits before the tag: not looked for.
        let mut bits: Vec<u8> = (0..40).map(|i| (i % 3 != 0) as u8).collect();
        bits.extend(enc.bits(0, 2));
        assert!(run(&mut Receiver::new(), bits.into_iter()).is_empty());
        // The last two flags damaged, the third still counts.
        let mut bits: Vec<u8> = enc.bits(4, 2).collect();
        bits[17] ^= 1;
        bits[25] ^= 1;
        assert_eq!(run(&mut Receiver::new(), bits.into_iter()).len(), 1);
    }

    #[test]
    fn raw_then_decode() {
        let mut enc = Encoder::new(FRAME, 32).unwrap();
        for e in 0..16 {
            enc.block_mut()[e * 3] ^= 0x81;
        }
        let mut rx = Receiver::new();
        let mut got = None;
        for b in enc.bits(4, 2) {
            if let Some(mode) = rx.push_raw(b) {
                let mut copy = rx.block().to_vec();
                let d = decode(&mut copy, mode, rx.tag_errors()).unwrap();
                got = Some((d.frame.to_vec(), d.corrected, d.mode));
            }
        }
        assert_eq!(got, Some((FRAME.to_vec(), 16, enc.mode())));
    }

    #[test]
    fn uncorrectable_block_is_dropped() {
        let mut enc = Encoder::new(FRAME, 16).unwrap();
        for e in 0..9 {
            enc.block_mut()[e * 5] ^= 0xA5;
        }
        let mut rx = Receiver::new();
        assert!(run(&mut rx, enc.bits(4, 2)).is_empty());
    }

    #[test]
    fn plain_deframer_still_sees_the_frame() {
        // Legacy compatibility: the bits of an FX.25 transmission, NRZI aside, carry the AX.25 frame as is.
        let enc = Encoder::new(FRAME, 32).unwrap();
        let bits: Vec<u8> = enc.bits(4, 2).collect();
        let mut found = 0;
        let mut ones = 0;
        let mut buf = Vec::new();
        let (mut byte, mut nb, mut inf) = (0u8, 0, false);
        for &b in &bits {
            if b == 1 {
                ones += 1;
                if ones > 6 {
                    inf = false;
                }
                if ones > 6 {
                    continue;
                }
            } else if ones == 5 {
                ones = 0;
                continue;
            } else if ones == 6 {
                ones = 0;
                if inf && nb == 7 && buf.len() >= MIN_FRAME && crc16_x25_update(0xFFFF, &buf) == CRC16_X25_GOOD {
                    assert_eq!(&buf[..buf.len() - 2], FRAME);
                    found += 1;
                }
                inf = true;
                buf.clear();
                nb = 0;
                continue;
            } else {
                ones = 0;
            }
            if inf {
                byte = (byte >> 1) | (b << 7);
                nb += 1;
                if nb == 8 {
                    buf.push(byte);
                    nb = 0;
                }
            }
        }
        assert_eq!(found, 1);
    }
}
