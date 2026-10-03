//! 1-bit bitmap fonts (the hardware test's 8x16 and 16x32, assets1.rs): rows top to bottom, (w+7)/8 bytes
//! per row, MSB is the leftmost pixel. Mirrors fw/drivers/font.h.

pub struct Font1 {
    pub w: u8,
    pub h: u8,
    pub first: u8,
    pub count: u8,
    pub data: &'static [u8],
}

impl Font1 {
    pub fn bytes_per_row(&self) -> usize {
        (self.w as usize).div_ceil(8)
    }

    /// Characters outside the font draw as '?'.
    pub fn glyph(&self, c: u8) -> &'static [u8] {
        let i = if c < self.first || c - self.first >= self.count { b'?' } else { c };
        let n = self.bytes_per_row() * self.h as usize;
        &self.data[(i - self.first) as usize * n..][..n]
    }
}
