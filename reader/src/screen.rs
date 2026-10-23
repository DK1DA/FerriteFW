//! The ST7789 as ferrite-ui's panel (as firmware/src/display.rs), and the backlight on.
use crate::bus::{BusyDelay, Lcd};
use crate::hal::backlight;
use ferrite_ui::gfx::BG;
use ferrite_ui::panel::{Panel, H, W};
use st7789::St7789;

pub struct Screen(St7789<Lcd>);

impl Screen {
    /// The panel's start-up (about 400 ms), cleared, on; the screen light full, the keypad light off.
    pub fn new() -> Self {
        let mut p = St7789::new(Lcd::new(), W as u16, H as u16);
        p.init(&mut BusyDelay, BG);
        backlight::init();
        backlight::set(1000);
        backlight::keypad(0);
        Screen(p)
    }
}

impl Panel for Screen {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.0.window(x0 as u16, y0 as u16, (x1 - x0 + 1) as u16, (y1 - y0 + 1) as u16);
    }
    fn push(&mut self, color: u16, count: u32) {
        self.0.repeat(color, count)
    }
    fn push_buf(&mut self, px: &[u16]) {
        self.0.pixels(px)
    }
}
