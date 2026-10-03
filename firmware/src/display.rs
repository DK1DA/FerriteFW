//! The ST7789 (uradio) as the drawing layer's panel. Only the UI task has one.

use crate::bus::Lcd;
use ferrite_ui::panel::{Panel, H, W};
use st7789::St7789;

pub struct Screen(pub St7789<Lcd>);

impl Screen {
    pub fn new() -> Self {
        Screen(St7789::new(Lcd::new(), W as u16, H as u16))
    }
}

impl Panel for Screen {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        // The drawing layer clips before it gets here.
        self.0.window(x0 as u16, y0 as u16, (x1 - x0 + 1) as u16, (y1 - y0 + 1) as u16);
    }
    fn push(&mut self, color: u16, count: u32) {
        self.0.repeat(color, count)
    }
    fn push_buf(&mut self, px: &[u16]) {
        self.0.pixels(px)
    }
}
