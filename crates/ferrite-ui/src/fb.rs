//! Host stand-in for the ST7789: draws into a 240 x 320 frame buffer instead of the panel, so the real page
//! and gfx code can be rendered and compared on the PC. Mirrors fw/sim/sim_st7789.c.
use crate::panel::{Panel, H, W};
use std::vec::Vec;

pub struct FrameBuffer {
    pub px: Vec<u16>, // row-major, W x H
    /// How many times each pixel was written: overdraw (on the radio every write costs bus time).
    pub writes: Vec<u16>,
    win: (i32, i32, i32, i32),
    cur: (i32, i32),
}

impl FrameBuffer {
    pub fn new() -> Self {
        FrameBuffer { px: std::vec![0; (W * H) as usize], writes: std::vec![0; (W * H) as usize], win: (0, 0, W - 1, H - 1), cur: (0, 0) }
    }

    pub fn at(&self, x: i32, y: i32) -> u16 {
        self.px[(y * W + x) as usize]
    }

    fn put(&mut self, c: u16) {
        let (x0, y0, x1, y1) = self.win;
        let (x, y) = self.cur;
        if x < W && y < H {
            self.px[(y * W + x) as usize] = c;
            self.writes[(y * W + x) as usize] = self.writes[(y * W + x) as usize].saturating_add(1);
        }
        self.cur = if x + 1 > x1 { (x0, if y + 1 > y1 { y0 } else { y + 1 }) } else { (x + 1, y) };
    }

    /// As 8-bit RGB, the way the C simulator writes its .ppm files.
    pub fn rgb(&self) -> Vec<u8> {
        self.px.iter().flat_map(|&c| [(c >> 8) as u8 & 0xF8, (c >> 3) as u8 & 0xFC, (c << 3) as u8 & 0xF8]).collect()
    }

    pub fn write_ppm(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut out = std::format!("P6\n{W} {H}\n255\n").into_bytes();
        out.extend(self.rgb());
        std::fs::write(path, out)
    }
}

impl Panel for FrameBuffer {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.win = (x0, y0, x1, y1);
        self.cur = (x0, y0);
    }

    fn push(&mut self, color: u16, count: u32) {
        for _ in 0..count {
            self.put(color);
        }
    }

    fn push_buf(&mut self, px: &[u16]) {
        for &c in px {
            self.put(c);
        }
    }
}

/// The RGB bytes of a binary .ppm (P6, 240 x 320) as written by fw/sim.
pub fn read_ppm(path: &std::path::Path) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let header = std::format!("P6\n{W} {H}\n255\n");
    data.strip_prefix(header.as_bytes()).map(|rgb| rgb.to_vec())
}
