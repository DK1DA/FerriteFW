//! Link compression, three codecs to choose from (decision 285, step 3; none is on by default), byte-compatible with
//! `tools/linkz.py` (the reference, and the trainer of the table and dictionary in [`crate::linkz_data`]):
//!
//! - T, a fixed table, stateless: each frame alone. Bytes 0x00-0xFC are themselves; 0xFD n is n + 3 zero bytes;
//!   0xFE n is table entry n; 0xFF b is the byte b. No RAM, nothing to keep in step.
//! - L, LZSS over a [`W`]-byte window of what this direction carried before (kept across frames): a flag byte before
//!   every 8 tokens (bit 0 first), 1 a match (two bytes, little endian: offset back less 1 in the low 10 bits, length
//!   less 3 above, 3-66), 0 a literal. Both ends must have seen the same frames.
//! - LD, L with the window starting as [`crate::linkz_data::DICT`].
//!
//! The encoder finds matches through a hash chain over the window ([`CHAIN`] links at most per byte), so it is
//! bounded in time; its output decodes with any decoder of the format, not necessarily byte for byte the reference's.

use crate::linkz_data::{DICT, TABLE, TABLE_AT, TABLE_FIRST, TABLE_ORDER};

/// The window.
pub const W: usize = 1024;
const OFF_BITS: u32 = 10;
const MIN: usize = 3;
const MAX: usize = MIN + (1 << (16 - OFF_BITS)) - 1;
/// Positions a match search looks at, at most.
pub const CHAIN: usize = 32;

fn entry(n: usize) -> &'static [u8] {
    &TABLE[TABLE_AT[n] as usize..TABLE_AT[n + 1] as usize]
}

/// T: `data` coded into `out`; its length (None if `out` is too small).
pub fn t_encode(data: &[u8], out: &mut [u8]) -> Option<usize> {
    let (mut i, mut o) = (0, 0);
    let mut put = |o: &mut usize, b: &[u8]| -> Option<()> {
        out.get_mut(*o..*o + b.len())?.copy_from_slice(b);
        *o += b.len();
        Some(())
    };
    while i < data.len() {
        if data[i] == 0 {
            let n = data[i..].iter().take(258).take_while(|&&b| b == 0).count();
            if n >= 3 {
                put(&mut o, &[0xFD, (n - 3) as u8])?;
                i += n;
                continue;
            }
        }
        let mut hit = None;
        if data.len() - i >= 3 {
            let b0 = data[i] as usize;
            for &n in &TABLE_ORDER[TABLE_FIRST[b0] as usize..TABLE_FIRST[b0 + 1] as usize] {
                let e = entry(n as usize);
                if data[i..].starts_with(e) {
                    hit = Some((e.len(), n as usize)); // longest first: the first that fits
                    break;
                }
            }
        }
        if let Some((l, n)) = hit {
            put(&mut o, &[0xFE, n as u8])?;
            i += l;
            continue;
        }
        let b = data[i];
        if b >= 0xFD { put(&mut o, &[0xFF, b])? } else { put(&mut o, &[b])? }
        i += 1;
    }
    Some(o)
}

/// T: decoded into `out`; its length (None if malformed or `out` too small).
pub fn t_decode(data: &[u8], out: &mut [u8]) -> Option<usize> {
    let (mut i, mut o) = (0, 0);
    while i < data.len() {
        let b = data[i];
        let piece: &[u8] = match b {
            0xFD => {
                let n = *data.get(i + 1)? as usize + 3;
                out.get_mut(o..o + n)?.fill(0);
                (i, o) = (i + 2, o + n);
                continue;
            }
            0xFE => entry(*data.get(i + 1)? as usize).get(..)?,
            0xFF => core::slice::from_ref(data.get(i + 1)?),
            _ => core::slice::from_ref(&data[i]),
        };
        out.get_mut(o..o + piece.len())?.copy_from_slice(piece);
        o += piece.len();
        i += if b >= 0xFD { 2 } else { 1 };
    }
    Some(o)
}

fn hash(a: u8, b: u8, c: u8) -> usize {
    ((a as usize) << 5 ^ (b as usize) << 2 ^ c as usize) & 0xFF
}

/// One direction's LZ window, for decoding (1 KB).
pub struct LzDec {
    win: [u8; W],
    /// Bytes ever carried, mod 2^32 (the window's write position).
    n: u32,
}

impl LzDec {
    /// Empty (L) or primed with the dictionary (LD).
    pub fn new(dict: bool) -> Self {
        let mut d = LzDec { win: [0; W], n: 0 };
        if dict {
            d.push_all(&DICT);
        }
        d
    }

    fn push_all(&mut self, b: &[u8]) {
        for &x in b {
            self.win[self.n as usize % W] = x;
            self.n = self.n.wrapping_add(1);
        }
    }

    /// A frame decoded into `out`; its length (None if malformed: the session must start over).
    pub fn decode(&mut self, data: &[u8], out: &mut [u8]) -> Option<usize> {
        let (mut i, mut o) = (0, 0);
        while i < data.len() {
            let flags = data[i];
            i += 1;
            for bit in 0..8 {
                if i >= data.len() {
                    break;
                }
                if flags >> bit & 1 != 0 {
                    let v = u16::from_le_bytes([data[i], *data.get(i + 1)?]) as usize;
                    i += 2;
                    let (off, len) = ((v & (W - 1)) + 1, (v >> OFF_BITS) + MIN);
                    if off as u32 > self.n {
                        return None;
                    }
                    for _ in 0..len {
                        let b = self.win[(self.n as usize).wrapping_sub(off) % W];
                        *out.get_mut(o)? = b;
                        o += 1;
                        self.win[self.n as usize % W] = b;
                        self.n = self.n.wrapping_add(1);
                    }
                } else {
                    *out.get_mut(o)? = data[i];
                    o += 1;
                    self.push_all(&[data[i]]);
                    i += 1;
                }
            }
        }
        Some(o)
    }
}

/// One direction's LZ window and hash chain, for encoding (1 KB window, 2 KB of chain links, 512 B of heads).
pub struct LzEnc {
    win: [u8; W],
    n: u32,
    /// The newest position with each hash (+1, mod 2^16; 0 none).
    heads: [u16; 256],
    /// The position before each one with the same hash (+1, mod 2^16), by position mod W.
    prev: [u16; W],
}

impl LzEnc {
    /// Empty (L) or primed with the dictionary (LD).
    pub fn new(dict: bool) -> Self {
        let mut e = LzEnc { win: [0; W], n: 0, heads: [0; 256], prev: [0; W] };
        if dict {
            for &b in DICT.iter() {
                e.push(b);
            }
        }
        e
    }

    fn at(&self, p: u32) -> u8 {
        self.win[p as usize % W]
    }

    /// One byte into the window, and the 3 bytes ending with it into the hash chain.
    fn push(&mut self, b: u8) {
        self.win[self.n as usize % W] = b;
        self.n = self.n.wrapping_add(1);
        if self.n >= 3 {
            let p = self.n - 3;
            let h = hash(self.at(p), self.at(p + 1), self.at(p + 2));
            self.prev[p as usize % W] = self.heads[h];
            self.heads[h] = (p as u16).wrapping_add(1);
        }
    }

    /// A frame coded into `out`; its length (None if `out` is too small: send it plain).
    pub fn encode(&mut self, data: &[u8], out: &mut [u8]) -> Option<usize> {
        let (mut i, mut o, mut flags_at, mut bit) = (0, 0, 0, 8);
        while i < data.len() {
            if bit == 8 {
                flags_at = o;
                *out.get_mut(o)? = 0;
                o += 1;
                bit = 0;
            }
            let (mut best, mut best_off) = (0, 0);
            if data.len() - i >= MIN {
                let h = hash(data[i], data[i + 1], data[i + 2]);
                let mut link = self.heads[h];
                let now = self.n;
                for _ in 0..CHAIN {
                    if link == 0 {
                        break;
                    }
                    // the position: the newest one whose low 16 bits are link - 1
                    let low = link.wrapping_sub(1);
                    let p = now.wrapping_sub(now.wrapping_sub(low as u32) & 0xFFFF);
                    let off = now.wrapping_sub(p) as usize;
                    if off == 0 || off > W - MAX {
                        break; // gone from the window (keeping room so a match never reads what it writes over)
                    }
                    let mut len = 0;
                    while len < MAX && i + len < data.len() {
                        let src = if len < off { self.at(p + len as u32) } else { data[i + len - off] };
                        if src != data[i + len] {
                            break;
                        }
                        len += 1;
                    }
                    if len > best {
                        (best, best_off) = (len, off);
                        if len == MAX {
                            break;
                        }
                    }
                    link = self.prev[p as usize % W];
                }
            }
            if best >= MIN {
                let v = (best_off - 1) | (best - MIN) << OFF_BITS;
                out.get_mut(o..o + 2)?.copy_from_slice(&(v as u16).to_le_bytes());
                o += 2;
                out[flags_at] |= 1 << bit;
                for k in 0..best {
                    self.push(data[i + k]);
                }
                i += best;
            } else {
                *out.get_mut(o)? = data[i];
                o += 1;
                self.push(data[i]);
                i += 1;
            }
            bit += 1;
        }
        Some(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linkz_data::SAMPLES;
    use crate::linkz_vectors::VECTORS;

    /// Decision 285, step 3: each codec round trips the sample frames in one session; Python's encodings (the
    /// reference) decode here to the same frames; the encoders here come within a few bytes of the reference's.
    #[test]
    fn codecs_round_trip_and_match_the_reference() {
        let (mut le, mut ld, mut de, mut dd) = (LzEnc::new(false), LzDec::new(false), LzEnc::new(true), LzDec::new(true));
        let (mut pl, mut pd) = (LzDec::new(false), LzDec::new(true));
        let mut b = [0u8; 600];
        let mut back = [0u8; 600];
        let (mut ours, mut theirs) = (0, 0);
        for (f, (t, l, d)) in SAMPLES.iter().zip(VECTORS) {
            let n = t_encode(f, &mut b).unwrap();
            assert_eq!(&b[..n], *t, "T is byte for byte the reference's");
            { let k = t_decode(t, &mut back).unwrap(); assert_eq!(&back[..k], *f); }
            { let k = pl.decode(l, &mut back).unwrap(); assert_eq!(&back[..k], *f, "the reference's L decodes"); }
            { let k = pd.decode(d, &mut back).unwrap(); assert_eq!(&back[..k], *f, "the reference's LD decodes"); }
            let n = le.encode(f, &mut b).unwrap();
            { let k = ld.decode(&b[..n], &mut back).unwrap(); assert_eq!(&back[..k], *f, "L round trips"); }
            let m = de.encode(f, &mut b).unwrap();
            { let k = dd.decode(&b[..m], &mut back).unwrap(); assert_eq!(&back[..k], *f, "LD round trips"); }
            (ours, theirs) = (ours + n, theirs + l.len());
        }
        assert!(ours <= theirs + theirs / 10, "the bounded search costs little: {ours} against {theirs}");
        assert!(t_decode(&[0xFE], &mut back).is_none() && LzDec::new(false).decode(&[1, 5, 0], &mut back).is_none());
    }
}
