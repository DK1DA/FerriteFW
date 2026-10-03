//! What the drawing code needs from a panel: the ST7789 on the radio (st7789.rs), a frame buffer on the
//! host (sim.rs). Portrait, 240 x 320, RGB565.

pub const W: i32 = 240;
pub const H: i32 = 320;

pub trait Panel {
    /// Inclusive; starts a RAM write. Pixels then fill it row by row.
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32);
    /// `count` pixels of one colour.
    fn push(&mut self, color: u16, count: u32);
    fn push_buf(&mut self, px: &[u16]);
}
