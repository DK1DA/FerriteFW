//! POCSAG's codeword code: BCH(31,21), generator x^10 + x^9 + x^8 + x^6 + x^5 + x^3 + 1, plus an even parity bit.
//!
//! A codeword, most significant bit first: 21 data bits (a flag and 20 bits of address or message), 10 check bits,
//! 1 parity bit. The code corrects any two bit errors; the parity bit then rejects most triple errors.

const GEN: u32 = 0x769;

/// Remainder of a 31-bit word (data and check bits, no parity) divided by the generator.
const fn rem(mut w: u32) -> u32 {
    let mut i = 30;
    while i >= 10 {
        if (w >> i) & 1 != 0 {
            w ^= GEN << (i - 10);
        }
        i -= 1;
    }
    w
}

/// Syndrome of a single error at each bit of the 31-bit word. A `static`, not a `const`: a named table stays with
/// the hot code on firmware that places unnamed constants in slower flash (read for every codeword).
static SYN: [u32; 31] = {
    let mut t = [0; 31];
    let mut i = 0;
    while i < 31 {
        t[i] = rem(1 << i);
        i += 1;
    }
    t
};

/// A codeword from its 21 data bits (the flag in bit 20): check bits and parity added.
pub const fn encode(data: u32) -> u32 {
    let w = (data & 0x1F_FFFF) << 10;
    let w = w | rem(w);
    (w << 1) | (w.count_ones() & 1)
}

/// The codeword with up to two bit errors corrected, and how many were; `None` if it cannot be corrected.
pub fn correct(cw: u32) -> Option<(u32, u8)> {
    let s = rem(cw >> 1);
    let parity_ok = cw.count_ones() & 1 == 0;
    if s == 0 {
        // Data and check bits agree; at most the parity bit is wrong.
        return Some(if parity_ok { (cw, 0) } else { (cw ^ 1, 1) });
    }
    if let Some(i) = SYN.iter().position(|&e| e == s) {
        // One error in the 31 bits; if the parity still holds, the parity bit is wrong too.
        let c = cw ^ (2 << i);
        return Some(if parity_ok { (c ^ 1, 2) } else { (c, 1) });
    }
    if !parity_ok {
        // Two errors in the 31 bits keep the parity even; odd parity here means three or more.
        return None;
    }
    for i in 0..31 {
        let rest = s ^ SYN[i];
        if let Some(j) = SYN[i + 1..].iter().position(|&e| e == rest) {
            return Some((cw ^ (2 << i) ^ (2 << (i + 1 + j)), 2));
        }
    }
    None
}
