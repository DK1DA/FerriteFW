//! Bits to calls. A DSC call is a dot pattern, then phasing, then every symbol twice: a DX copy, and an RX copy
//! five positions later (four other characters between), the two streams interleaved:
//!
//! ```text
//! DX: 125 125 125 125 125 125  F   F   A1  ...  EOS ECC EOS EOS
//! RX: 111 110 109 108 107 106 105 104  F   F   A1  ...  ECC
//! ```
//!
//! Each symbol is a 10-bit codeword: 7 information bits (least significant first) and 3 check bits, the number
//! of zeros among them (most significant first). Phasing is found from a DX 125 and two RX phasing symbols in
//! order; then each symbol is taken from whichever copy has a valid codeword. The ECC symbol (XOR of the format
//! specifier and every symbol to the EOS) checks the result, and repairs one symbol lost in both copies.

/// The end-of-sequence symbols, as a named table (see [`CODE`]).
static EOS: [u8; 3] = crate::EOS;

/// Longest call kept, symbols (format specifiers to EOS); ITU calls are under 30.
pub const MAX_SYMBOLS: usize = 40;

/// A codeword's value, if its check bits match. `w` holds the 10 bits, first received in bit 9.
pub fn codeword(w: u32) -> Option<u8> {
    let mut v = 0u8;
    for i in 0..7 {
        v |= (((w >> (9 - i)) & 1) as u8) << i;
    }
    let check = w & 7;
    (check == 7 - v.count_ones()).then_some(v)
}

/// Every symbol's codeword, built at compile time. A `static`, not a `const`: a named table stays with the hot code
/// on firmware that places unnamed constants in slower flash (the decoder reads it per symbol).
static CODE: [u16; 128] = {
    let mut t = [0u16; 128];
    let mut v = 0;
    while v < 128 {
        let mut w = 0u16;
        let mut i = 0;
        while i < 7 {
            w |= (((v >> i) & 1) as u16) << (9 - i);
            i += 1;
        }
        t[v] = w | (7 - (v as u8).count_ones()) as u16;
        v += 1;
    }
    t
};

/// The codeword bits of a symbol (first to send in bit 9).
#[inline]
pub fn encode(v: u8) -> u32 {
    CODE[(v & 0x7F) as usize] as u32
}

/// Bits that differ between two 10-bit words (a table: the Cortex-M4 has no population-count instruction).
#[inline]
fn dist(a: u32, b: u32) -> u32 {
    static POP: [u8; 32] = {
        let mut t = [0u8; 32];
        let mut i = 0;
        while i < 32 {
            t[i] = (i as u32).count_ones() as u8;
            i += 1;
        }
        t
    };
    let x = (a ^ b) & 0x3FF;
    (POP[(x & 31) as usize] + POP[(x >> 5) as usize]) as u32
}

/// A received call: the symbols from the first format specifier to the EOS.
#[derive(Clone, Copy, Debug)]
pub struct Call<'a> {
    pub symbols: &'a [u8],
    /// The ECC matched (possibly after a repair).
    pub ecc_ok: bool,
    /// Symbols lost in both copies and rebuilt from the ECC (0 or 1), or chosen between disagreeing copies by it.
    pub repaired: u8,
}

#[derive(Clone, Copy)]
enum State {
    Hunt,
    /// Locked: the next codeword is at this position (even: DX index pos/2; odd: RX index pos/2).
    Locked { pos: u16, bad_run: u8 },
}

/// No copy received: the RX copy of the last symbols, or a codeword lost to a wrong phase.
const NONE: u16 = 0xFFFF;

/// Bits in, calls out. About 230 bytes.
pub struct Framer {
    sr: u64,
    nbits: u8,
    state: State,
    /// The raw 10-bit words received for each message symbol, DX and RX copies ([`NONE`] if not received).
    dx: [u16; MAX_SYMBOLS + 4],
    rx: [u16; MAX_SYMBOLS + 4],
    out: [u8; MAX_SYMBOLS],
}

impl Default for Framer {
    fn default() -> Self {
        Self::new()
    }
}

/// The value nearest to both copies (fewest bit errors in total), the errors, and the runner-up's errors.
fn nearest(dx: u16, rx: u16) -> (u8, u32, u32) {
    let dist = |w: u16, v: u8| if w == NONE { 5 } else { dist(w as u32, encode(v)) };
    let (mut best, mut d1, mut d2) = (0u8, u32::MAX, u32::MAX);
    for v in 0..128u8 {
        let d = dist(dx, v) + dist(rx, v);
        if d < d1 {
            (best, d2, d1) = (v, d1, d);
        } else if d < d2 {
            d2 = d;
        }
    }
    (best, d1, d2)
}

impl Framer {
    pub const fn new() -> Self {
        Framer { sr: 0, nbits: 0, state: State::Hunt, dx: [NONE; MAX_SYMBOLS + 4], rx: [NONE; MAX_SYMBOLS + 4], out: [0; MAX_SYMBOLS] }
    }

    /// Feed one bit (1 = the 1300 Hz tone). Returns a call when one ends.
    pub fn push(&mut self, bit: bool) -> Option<Call<'_>> {
        self.sr = self.sr << 1 | bit as u64;
        match self.state {
            State::Hunt => {
                // ... DX 125, RX v+1, DX 125, RX v: the last four codewords against the phasing pattern for each RX
                // value v; lock when one is within 3 bit errors in all (noise: about once in 10^11 bits).
                let w = |k: u32| ((self.sr >> (10 * k)) & 0x3FF) as u32;
                let (w0, w1, w2, w3) = (w(0), w(1), w(2), w(3));
                let dx = dist(w1, encode(125)) + dist(w3, encode(125));
                if dx <= 3 && self.sr >> 39 != 0 {
                    for v in 104..=110u8 {
                        if dx + dist(w0, encode(v)) + dist(w2, encode(v + 1)) <= 3 {
                            self.dx = [NONE; MAX_SYMBOLS + 4];
                            self.rx = [NONE; MAX_SYMBOLS + 4];
                            // RX index of v is 111 - v; next comes DX index 111 - v + 1.
                            let rx_index = (111 - v) as u16;
                            self.state = State::Locked { pos: 2 * (rx_index + 1), bad_run: 0 };
                            self.nbits = 0;
                            break;
                        }
                    }
                }
                None
            }
            State::Locked { pos, bad_run } => {
                self.nbits += 1;
                if self.nbits < 10 {
                    return None;
                }
                self.nbits = 0;
                let word = (self.sr & 0x3FF) as u16;
                let bad_run = if codeword(word as u32).is_some() { 0 } else { bad_run + 1 };
                let index = (pos / 2) as usize;
                // Message symbol i is DX index 6 + i and RX index 8 + i.
                let done = if pos % 2 == 0 {
                    if index >= 6 && index - 6 < self.dx.len() {
                        self.dx[index - 6] = word;
                    }
                    false
                } else if index >= 8 && index - 8 < self.rx.len() {
                    self.rx[index - 8] = word;
                    // The RX copy of the symbol after an EOS is the ECC: the call is complete. The EOS is the
                    // nearest value to its copies, or (if those are poor) known from the two EOS that follow the
                    // ECC in the DX stream.
                    let i = index - 8;
                    let eos = |w: u16| codeword(w as u32).is_some_and(|v| EOS.contains(&v));
                    i >= 2
                        && (eos(self.rx[i - 1]) || eos(self.dx[i - 1])
                            || (i + 2 < self.dx.len() && eos(self.dx[i + 1]) && eos(self.dx[i + 2]))
                            || {
                                // Neither copy is a clean EOS: the nearest value to both, if it is one and close.
                                let (v, d, _) = nearest(self.dx[i - 1], self.rx[i - 1]);
                                EOS.contains(&v) && d <= 2
                            })
                } else {
                    false
                };
                if done {
                    self.state = State::Hunt;
                    return self.finish((pos / 2 - 8) as usize);
                }
                if bad_run >= 6 || pos as usize >= 2 * (MAX_SYMBOLS + 8) {
                    // The signal is gone (or the call too long) without the end being recognised: look back for
                    // the first symbol that is most likely an EOS and finish there; the ECC says if that was right.
                    self.state = State::Hunt;
                    let last = ((pos / 2) as usize).saturating_sub(8).min(MAX_SYMBOLS);
                    let eos = (4..last.saturating_sub(1)).find(|&i| {
                        let (v, d, d2) = nearest(self.dx[i], self.rx[i]);
                        EOS.contains(&v) && d2 > d
                    })?;
                    return self.finish(eos + 1).filter(|c| c.ecc_ok);
                }
                self.state = State::Locked { pos: pos + 1, bad_run };
                None
            }
        }
    }

    /// `ecc_at`: index of the ECC symbol; the EOS is just before it.
    fn finish(&mut self, ecc_at: usize) -> Option<Call<'_>> {
        let n = ecc_at; // symbols 0..n: format specifiers to EOS
        if !(4..=MAX_SYMBOLS).contains(&n) {
            return None;
        }
        // Each symbol: the value nearest both copies, and how sure that is (runner-up's distance minus its own).
        let mut margin = [0u32; MAX_SYMBOLS];
        let mut errors = [0u32; MAX_SYMBOLS];
        for i in 0..n {
            let (v, d1, d2) = nearest(self.dx[i], self.rx[i]);
            self.out[i] = v;
            errors[i] = d1;
            margin[i] = d2 - d1;
        }
        let (e, ed, _) = nearest(self.dx[ecc_at], self.rx[ecc_at]);
        // The format specifier is sent twice: take the surer copy for both.
        if margin[1] > margin[0] {
            self.out[0] = self.out[1];
        }
        // ECC = format specifier ^ every symbol after the two format specifiers, to the EOS.
        let xor = |out: &[u8]| out[2..n].iter().fold(out[0], |x, &v| x ^ v);
        if ed <= 2 && xor(&self.out) == e {
            return Some(Call { symbols: &self.out[..n], ecc_ok: true, repaired: 0 });
        }
        // Not consistent: the ECC can stand in for the least sure symbol. Take it if that leaves the symbol close
        // to its copies.
        let weakest = (2..n).min_by_key(|&i| (margin[i], u32::MAX - errors[i]))?;
        let old = self.out[weakest];
        self.out[weakest] = 0;
        let v = xor(&self.out) ^ e;
        let d = |w: u16| if w == NONE { 5 } else { dist(w as u32, encode(v)) };
        if ed <= 2 && v < 128 && d(self.dx[weakest]) + d(self.rx[weakest]) <= errors[weakest] + 3 {
            self.out[weakest] = v;
            return Some(Call { symbols: &self.out[..n], ecc_ok: true, repaired: 1 });
        }
        self.out[weakest] = old;
        Some(Call { symbols: &self.out[..n], ecc_ok: false, repaired: 0 })
    }
}
