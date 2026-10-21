//! 4-bit fonts and masks (`tools/mkassets.py` generates them into `assets.rs`).
//!
//! A font holds only the characters it is generated with (English: printable ASCII plus the few symbols the UI
//! draws), as a sorted list of code points beside the glyphs. Lookup is direct for the dense run at the start
//! (ASCII) and a binary search after it; a character the font lacks draws as '?'.

pub struct Glyph4 {
    /// Offset of the glyph's pixels in `bits` (two pixels per byte).
    pub off: u16,
    pub w: u8,
    pub h: u8,
    pub x: i8,
    pub y: i8,
    pub adv: u8,
}

pub struct Font4 {
    pub line_h: u8,
    pub ascent: u8,
    /// Each glyph's pixels run-length encoded as a [`Mask4`]'s (the big fonts), not two pixels per byte.
    pub rle: bool,
    /// The characters, sorted; `glyphs[i]` draws `codes[i]`.
    pub codes: &'static [u16],
    pub glyphs: &'static [Glyph4],
    pub bits: &'static [u8],
}

impl Font4 {
    /// The glyph for a code point, if the font has it.
    pub fn glyph(&self, c: u32) -> Option<&Glyph4> {
        let first = *self.codes.first()? as u32;
        // the dense run: index = c - first
        if let Some(&code) = self.codes.get(c.wrapping_sub(first) as usize) {
            if code as u32 == c {
                return self.glyphs.get((c - first) as usize);
            }
        }
        let c = u16::try_from(c).ok()?;
        self.codes.binary_search(&c).ok().and_then(|i| self.glyphs.get(i))
    }

    pub fn has(&self, c: char) -> bool {
        self.glyph(c as u32).is_some()
    }
}

/// A 4-bit mask: rows top to bottom, either two pixels per byte or run-length encoded (`rle`, the format
/// `tools/mkassets.py` documents: per row, runs of 0, runs of 15 and literal pixels).
pub struct Mask4 {
    pub w: u8,
    pub h: u8,
    pub rle: bool,
    pub bits: &'static [u8],
}

/// A mask's rows, one at a time, top to bottom: no frame buffer, nothing on the heap. Damaged data reads as
/// empty pixels rather than panicking.
pub struct MaskRows<'a> {
    m: &'a Mask4,
    /// Byte position (RLE) or pixel index (plain).
    pos: usize,
}

impl<'a> MaskRows<'a> {
    pub fn new(m: &'a Mask4) -> Self {
        MaskRows { m, pos: 0 }
    }

    /// The next row's pixels (0-15) into out[..w].
    pub fn next_row(&mut self, out: &mut [u8]) {
        let w = (self.m.w as usize).min(out.len());
        let bits = self.m.bits;
        let nib = |k: usize| bits.get(k >> 1).map_or(0, |&b| if k & 1 == 0 { b >> 4 } else { b & 15 });
        if !self.m.rle {
            for (i, px) in out[..w].iter_mut().enumerate() {
                *px = nib(self.pos + i);
            }
            self.pos += self.m.w as usize;
            return;
        }
        rle_row(bits, &mut self.pos, w, out);
    }
}

/// One run-length encoded row (the format [`Mask4`] names) from `bits` at byte `pos` into out[..w], `pos` moved
/// to the next row. Damaged data reads as empty pixels.
pub fn rle_row(bits: &[u8], pos: &mut usize, w: usize, out: &mut [u8]) {
    let nib = |k: usize| bits.get(k >> 1).map_or(0, |&b| if k & 1 == 0 { b >> 4 } else { b & 15 });
    let mut x = 0;
    while x < w {
        let Some(&t) = bits.get(*pos) else {
            out[x..w].fill(0);
            return;
        };
        *pos += 1;
        match t >> 6 {
            0 | 1 => {
                let n = ((t & 63) as usize + 1).min(w - x);
                out[x..x + n].fill(if t & 0x40 != 0 { 15 } else { 0 });
                x += n;
            }
            _ => {
                let n = (t & 127) as usize + 1;
                for i in 0..n.min(w - x) {
                    out[x + i] = nib(*pos * 2 + i);
                }
                *pos += n.div_ceil(2);
                x += n.min(w - x);
            }
        }
    }
}
