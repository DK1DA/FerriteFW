//! 4-bit fonts and masks, the same layout as fw/drivers/font4.h.

pub struct Glyph4 {
    pub off: u32,
    pub w: u8,
    pub h: u8,
    pub x: i8,
    pub y: i8,
    pub adv: u8,
}

pub struct Font4 {
    pub line_h: u8,
    pub ascent: u8,
    pub first: u8,
    pub glyphs: &'static [Glyph4],
    pub bits: &'static [u8],
}

pub struct Mask4 {
    pub w: u8,
    pub h: u8,
    pub bits: &'static [u8],
}
