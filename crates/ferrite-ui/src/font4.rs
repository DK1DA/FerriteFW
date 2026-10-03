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

pub struct Mask4 {
    pub w: u8,
    pub h: u8,
    pub bits: &'static [u8],
}
