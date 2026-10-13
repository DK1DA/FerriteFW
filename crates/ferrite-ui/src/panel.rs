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
    /// The text just drawn into the box (x0, y0)-(x1, y1) inclusive, before its pixels: for the simulator's
    /// end-to-end tests, which read the screen as text. Panels ignore it (the radio's inlines to nothing).
    #[inline(always)]
    fn text_hint(&mut self, _x0: i32, _y0: i32, _x1: i32, _y1: i32, _s: &[u8]) {}
    /// The rows y0..y1 this panel keeps; drawing may skip the others. A band of an off-screen canvas keeps a few
    /// (canvas.rs); the panel itself keeps all, which compiles to nothing.
    #[inline(always)]
    fn rows(&self) -> (i32, i32) {
        (0, H)
    }
    /// Drawing here is off the screen already (a band of a canvas): a canvas inside draws straight into it.
    #[inline(always)]
    fn offscreen(&self) -> bool {
        false
    }
}
