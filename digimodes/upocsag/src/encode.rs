//! A message to codewords: for transmitting, and for testing the receiver.

use crate::{bch, decode::NUMERIC, IDLE, SYNC};

/// What a message carries.
#[derive(Clone, Copy, Debug)]
pub enum Content<'a> {
    /// Address only: the pager just beeps.
    Tone,
    /// Characters from [`NUMERIC`]; others are sent as spaces.
    Numeric(&'a [u8]),
    /// 7-bit ASCII.
    Alpha(&'a [u8]),
}

/// Preamble length: 576 bits of alternating 1 and 0, sent before the first sync word.
pub const PREAMBLE_BITS: usize = 576;

/// The codewords of one transmission carrying one message, sync words included, from the first sync word to the
/// end of the last batch. `out` gets each 32-bit word, to send most significant bit first after the preamble.
///
/// Panics if `address` is over 21 bits or `function` over 3: a wrong capcode would page someone else. Alpha
/// characters are 7-bit; the top bit is dropped.
pub fn encode(address: u32, function: u8, content: Content, mut out: impl FnMut(u32)) {
    assert!(address < 1 << 21 && function < 4, "address is 21 bits, function 0 to 3");
    let (chars, width): (&[u8], usize) = match content {
        Content::Tone => (&[], 4),
        Content::Numeric(t) => (t, 4),
        Content::Alpha(t) => (t, 7),
    };
    let symbol = |c: u8| -> u32 {
        match content {
            Content::Alpha(_) => (c & 0x7F) as u32,
            _ => NUMERIC.iter().position(|&d| d == c).unwrap_or(12) as u32,
        }
    };
    let total = chars.len() * width;
    let chunks = total.div_ceil(20);
    // Bit i of the message: characters least significant bit first; numeric padding is spaces, alpha zeros.
    let bit = |i: usize| -> u32 {
        if i < total {
            (symbol(chars[i / width]) >> (i % width)) & 1
        } else if width == 4 {
            (12 >> (i % 4)) & 1
        } else {
            0
        }
    };
    let start = ((address & 7) * 2) as usize;
    let words = start + 1 + chunks;
    let batches = (words + 1).div_ceil(16); // at least one idle after the message
    for n in 0..batches * 16 {
        if n % 16 == 0 {
            out(SYNC);
        }
        let w = if n < start || n >= words {
            IDLE
        } else if n == start {
            bch::encode(((address >> 3) & 0x3_FFFF) << 2 | (function & 3) as u32)
        } else {
            let k = (n - start - 1) * 20;
            let v = (0..20).fold(0, |v, j| v << 1 | bit(k + j));
            bch::encode(1 << 20 | v)
        };
        out(w);
    }
}
