//! Codewords to messages: addresses, function bits and message data, from batches of 16 codewords.
//!
//! An address codeword starts a message (its frame, the slot pair it sits in, gives the address's low three bits);
//! message codewords add 20 bits each; the next address, an idle codeword or the end of the transmission ends it.

use crate::{bch, IDLE};

/// Digits of a numeric message, codes 0 to 15 (A is spare; B urgent; E and F brackets), as multimon-ng shows them.
pub const NUMERIC: &[u8; 16] = b"0123456789.U -][";

/// A received message. The data is the message bits as sent; [`Message::numeric`] and [`Message::alpha`] read them
/// as either kind, since which one a pager expects is set per address (function 0 is usually numeric and 3
/// alphanumeric, but systems differ).
pub struct Message<'a> {
    /// Symbol rate it arrived at, when known.
    pub baud: u16,
    /// 21-bit pager address (capcode).
    pub address: u32,
    /// Function bits, 0 to 3.
    pub function: u8,
    /// Codewords that could not be corrected; their bits are in the data as received.
    pub errors: u8,
    /// The message was longer than the decoder's buffer; the end is missing.
    pub truncated: bool,
    data: &'a [u8],
    bits: usize,
}

impl<'a> Message<'a> {
    /// Number of message bits.
    pub fn bits(&self) -> usize {
        self.bits
    }

    /// Message bits as sent, packed most significant bit first.
    pub fn data(&self) -> &'a [u8] {
        &self.data[..self.bits.div_ceil(8)]
    }

    fn take(&self, at: usize, n: usize) -> u8 {
        // Characters are sent least significant bit first.
        (0..n).fold(0, |v, k| v | ((self.data[(at + k) / 8] >> (7 - (at + k) % 8)) & 1) << k)
    }

    /// The message as numeric characters (4 bits each).
    pub fn numeric(&self) -> impl Iterator<Item = u8> + '_ {
        (0..self.bits / 4).map(|i| NUMERIC[self.take(i * 4, 4) as usize])
    }

    /// The message as 7-bit ASCII (control characters included; senders often end with EOT and NUL padding).
    pub fn alpha(&self) -> impl Iterator<Item = u8> + '_ {
        (0..self.bits / 7).map(|i| self.take(i * 7, 7))
    }
}

/// Codewords in, messages out. `N` bytes of message buffer: the default, 256, holds 292 alphanumeric characters.
pub struct Decoder<const N: usize = 256> {
    /// Symbol rate to report in messages.
    pub baud: u16,
    buf: [u8; N],
    bits: usize,
    address: Option<u32>,
    function: u8,
    errors: u8,
    /// Uncorrectable codewords at the end of the message so far.
    tail_bad: u8,
    truncated: bool,
    slot: u8,
}

impl<const N: usize> Default for Decoder<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Decoder<N> {
    pub const fn new() -> Self {
        Decoder {
            baud: 0,
            buf: [0; N],
            bits: 0,
            address: None,
            function: 0,
            errors: 0,
            tail_bad: 0,
            truncated: false,
            slot: 0,
        }
    }

    /// A batch starts: the next codeword is in slot 0.
    pub fn sync(&mut self) {
        self.slot = 0;
    }

    /// The next codeword of the batch. Call [`Decoder::sync`] before each batch's 16 codewords: an address's low
    /// bits come from its slot.
    pub fn word(&mut self, cw: u32, found: impl FnMut(&Message)) {
        let frame = (self.slot / 2).min(7) as u32;
        self.slot = (self.slot + 1).min(16);
        match bch::correct(cw) {
            Some((IDLE, _)) => self.end(found),
            Some((c, _)) if c >> 31 == 0 => {
                self.end(found);
                self.address = Some(((c >> 13) & 0x3_FFFF) << 3 | frame);
                self.function = ((c >> 11) & 3) as u8;
            }
            Some((c, _)) => {
                self.tail_bad = 0;
                self.append((c >> 11) & 0xF_FFFF);
            }
            None if self.address.is_some() => {
                self.errors = self.errors.saturating_add(1);
                self.tail_bad = self.tail_bad.saturating_add(1);
                self.append((cw >> 11) & 0xF_FFFF);
            }
            None => {}
        }
    }

    /// The transmission ended (or sync was lost): report the message in progress. Uncorrectable codewords at its
    /// end (a fade, or the receiver losing the signal) are dropped rather than shown as noise.
    pub fn end(&mut self, mut found: impl FnMut(&Message)) {
        if !self.truncated && self.tail_bad > 0 {
            let keep = self.bits - 20 * self.tail_bad as usize;
            for b in keep..self.bits {
                self.buf[b / 8] &= !(0x80 >> (b % 8));
            }
            self.bits = keep;
            self.errors -= self.tail_bad;
        }
        self.tail_bad = 0;
        if let Some(address) = self.address.take() {
            found(&Message {
                baud: self.baud,
                address,
                function: self.function,
                errors: self.errors,
                truncated: self.truncated,
                data: &self.buf,
                bits: self.bits,
            });
        }
        self.buf.fill(0);
        self.bits = 0;
        self.errors = 0;
        self.truncated = false;
    }

    fn append(&mut self, v: u32) {
        if self.address.is_none() {
            return;
        }
        for k in (0..20).rev() {
            if self.bits == N * 8 {
                self.truncated = true;
                return;
            }
            self.buf[self.bits / 8] |= (((v >> k) & 1) as u8) << (7 - self.bits % 8);
            self.bits += 1;
        }
    }
}
