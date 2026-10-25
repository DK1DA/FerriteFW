//! Link compression (LINK-51, decisions 285 and 286): LZSS over a window of what one direction of a session has
//! carried, kept across frames, byte-compatible with `tools/linkz.py` (the reference). A flag byte before every 8
//! tokens (bit 0 first): 1 a match, two bytes little endian, the offset back less 1 in the low `log2(W)` bits and
//! the length less 3 above; 0 a literal byte. Both ends must have seen the same frames in the same order.
//!
//! Each direction has its own window: [`DOWN`] (radio to PC: UI readbacks, Live View, reads; 1 KB, offsets 10
//! bits, lengths 3-66) and [`UP`] (PC to radio: mostly channel writes; 512 bytes, offsets 9 bits, lengths 3-130).
//! The encoder keeps only the newest position for each of [`HEADS`] hashes (no chain): Alex's smaller-RAM ask, Oct
//! 24, 2026; on the traffic corpus it keeps 88-91 % of a 32-deep chain's saving, in 2.3 KB for the radio's whole
//! session (a 1 KB window and 1 KB of heads to encode, a 512-byte window to decode) instead of 4.6 KB a direction.

/// The radio-to-PC window.
pub const DOWN: usize = 1024;
/// The PC-to-radio window.
pub const UP: usize = 512;
/// Hash heads (the encoder's newest position for each).
pub const HEADS: usize = 512;
const MIN: usize = 3;

const fn bits(w: usize) -> u32 {
    w.trailing_zeros()
}

const fn max_len(w: usize) -> usize {
    MIN + (1 << (16 - bits(w))) - 1
}

fn hash(a: u8, b: u8, c: u8) -> usize {
    ((a as usize) << 6 ^ (b as usize) << 3 ^ c as usize) & (HEADS - 1)
}

/// One direction's window, for decoding.
#[derive(Clone)]
pub struct LzDec<const W: usize> {
    win: [u8; W],
    /// Bytes carried since the start (the window's write position, wrapping).
    n: u32,
}

impl<const W: usize> Default for LzDec<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const W: usize> LzDec<W> {
    pub const fn new() -> Self {
        LzDec { win: [0; W], n: 0 }
    }

    /// Starts over (a new session, an error).
    pub fn reset(&mut self) {
        self.n = 0;
    }

    /// A plain frame's bytes (one sent uncompressed) into the window, as its encoder did.
    pub fn seen(&mut self, b: &[u8]) {
        for &x in b {
            self.win[self.n as usize % W] = x;
            self.n = self.n.wrapping_add(1);
        }
    }

    /// A frame decoded into `out`; its length. None if malformed (the session must start over).
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
                    let (off, len) = ((v & (W - 1)) + 1, (v >> bits(W)) + MIN);
                    if off as u32 > self.n {
                        return None;
                    }
                    for _ in 0..len {
                        let b = self.win[(self.n as usize).wrapping_sub(off) % W];
                        *out.get_mut(o)? = b;
                        o += 1;
                        self.seen(&[b]);
                    }
                } else {
                    *out.get_mut(o)? = data[i];
                    o += 1;
                    self.seen(&[data[i]]);
                    i += 1;
                }
            }
        }
        Some(o)
    }
}

/// One direction's window and hash heads, for encoding.
#[derive(Clone)]
pub struct LzEnc<const W: usize> {
    win: [u8; W],
    n: u32,
    /// The newest position with each hash (+1, wrapping at 2^16; 0 none).
    heads: [u16; HEADS],
}

impl<const W: usize> Default for LzEnc<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const W: usize> LzEnc<W> {
    pub const fn new() -> Self {
        LzEnc { win: [0; W], n: 0, heads: [0; HEADS] }
    }

    /// Starts over (a new session, an error).
    pub fn reset(&mut self) {
        self.n = 0;
        self.heads = [0; HEADS];
    }

    fn at(&self, p: u32) -> u8 {
        self.win[p as usize % W]
    }

    /// One byte into the window, and the 3 bytes ending with it into the heads.
    fn push(&mut self, b: u8) {
        self.win[self.n as usize % W] = b;
        self.n = self.n.wrapping_add(1);
        if self.n >= 3 {
            let p = self.n - 3;
            self.heads[hash(self.at(p), self.at(p + 1), self.at(p + 2))] = (p as u16).wrapping_add(1);
        }
    }

    /// A plain frame's bytes (sent uncompressed) into the window, as the decoder will take them.
    pub fn seen(&mut self, b: &[u8]) {
        for &x in b {
            self.push(x);
        }
    }

    /// A frame coded into `out`; its length. None if it doesn't fit `out`: send it plain (the window has taken it
    /// either way, as the decoder's [`LzDec::seen`] will).
    pub fn encode(&mut self, data: &[u8], out: &mut [u8]) -> Option<usize> {
        let max = max_len(W);
        let (mut i, mut o, mut flags_at, mut bit) = (0, 0, 0, 8);
        let mut ok = true;
        while i < data.len() {
            if bit == 8 {
                flags_at = o;
                if let Some(x) = out.get_mut(o) {
                    *x = 0;
                } else {
                    ok = false;
                }
                o += 1;
                bit = 0;
            }
            let mut best = (0, 0);
            if data.len() - i >= MIN {
                let link = self.heads[hash(data[i], data[i + 1], data[i + 2])];
                if link != 0 {
                    let now = self.n;
                    let low = link.wrapping_sub(1);
                    let p = now.wrapping_sub(now.wrapping_sub(low as u32) & 0xFFFF);
                    let off = now.wrapping_sub(p) as usize;
                    if off > 0 && off <= W {
                        let mut len = 0;
                        while len < max && i + len < data.len() {
                            let src = if len < off { self.at(p + len as u32) } else { data[i + len - off] };
                            if src != data[i + len] {
                                break;
                            }
                            len += 1;
                        }
                        best = (len, off);
                    }
                }
            }
            if best.0 >= MIN {
                let v = (best.1 - 1) | (best.0 - MIN) << bits(W);
                if let Some(x) = out.get_mut(o..o + 2) {
                    x.copy_from_slice(&(v as u16).to_le_bytes());
                    out[flags_at] |= 1 << bit;
                } else {
                    ok = false;
                }
                o += 2;
                for k in 0..best.0 {
                    self.push(data[i + k]);
                }
                i += best.0;
            } else {
                if let Some(x) = out.get_mut(o) {
                    *x = data[i];
                } else {
                    ok = false;
                }
                o += 1;
                self.push(data[i]);
                i += 1;
            }
            bit += 1;
        }
        ok.then_some(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linkz_data::SAMPLES;

    /// LINK-51: each direction round trips the sample frames in one session, frames sent plain (`seen`) kept in
    /// step; the reference's encodings (tools/linkz.py, VECTORS) decode here; a match never reaches back before the
    /// start; a frame that doesn't fit is reported, and the windows stay in step when it is sent plain.
    #[test]
    fn lz_round_trips_both_ways() {
        let (mut e, mut d) = (LzEnc::<DOWN>::new(), LzDec::<DOWN>::new());
        let (mut eu, mut du) = (LzEnc::<UP>::new(), LzDec::<UP>::new());
        let (mut b, mut back) = ([0u8; 600], [0u8; 600]);
        let (mut plain, mut coded) = (0, 0);
        for (k, f) in SAMPLES.iter().enumerate() {
            if k == 3 {
                // sent plain: both windows take it
                let _ = e.encode(f, &mut b);
                d.seen(f);
                continue;
            }
            let n = e.encode(f, &mut b).unwrap();
            let m = d.decode(&b[..n], &mut back).unwrap();
            assert_eq!(&back[..m], *f);
            let n2 = eu.encode(f, &mut b).unwrap();
            let m2 = du.decode(&b[..n2], &mut back).unwrap();
            assert_eq!(&back[..m2], *f);
            (plain, coded) = (plain + f.len(), coded + n);
        }
        assert!(coded * 10 < plain * 8, "it compresses: {coded} of {plain}");
        let (mut r, mut e2) = (LzDec::<DOWN>::new(), LzEnc::<DOWN>::new());
        for (f, v) in SAMPLES.iter().zip(crate::linkz_vectors::VECTORS) {
            let m = r.decode(v, &mut back).unwrap();
            assert_eq!(&back[..m], *f, "the reference's DOWN frames decode");
            let n = e2.encode(f, &mut b).unwrap();
            assert_eq!(&b[..n], *v, "and this encoder makes the same bytes");
        }
        assert!(LzDec::<DOWN>::new().decode(&[1, 5, 0], &mut back).is_none(), "nothing before the start");
        let mut small = [0u8; 4];
        assert!(e.encode(SAMPLES[0], &mut small).is_none(), "too small: sent plain");
    }
}
