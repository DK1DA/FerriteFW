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
    /// Flicker tracking for one render pass ([`FrameBuffer::begin_pass`]): the pixels before it, the value a
    /// pixel showed in between (when that differed from before), and whether more than one such value showed.
    pass: Option<Pass>,
}

struct Pass {
    before: Vec<u16>,
    writes: Vec<u16>,
    inter: Vec<u16>,
    /// 0 no intermediate value, 1 one (in `inter`), 2 several different ones.
    shown: Vec<u8>,
    /// Flickering passes per pixel since the first [`FrameBuffer::begin_pass`] (a heat map for finding causes).
    heat: Vec<u32>,
}

/// One render pass's flicker and overdraw ([`FrameBuffer::end_pass`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PassReport {
    /// Pixels that showed a value on the panel that was neither what they had before the pass nor what they end
    /// with: on the radio's slow bus that intermediate value is visible (a blank-then-draw flicker).
    pub flicker: u32,
    /// The flickering pixels' bounding box (x0, y0, x1, y1), inclusive.
    pub bbox: Option<(i32, i32, i32, i32)>,
    /// Pixel writes in the pass, and the pixels whose value changed.
    pub writes: u64,
    pub changed: u32,
}

impl FrameBuffer {
    pub fn new() -> Self {
        FrameBuffer {
            px: std::vec![0; (W * H) as usize],
            writes: std::vec![0; (W * H) as usize],
            win: (0, 0, W - 1, H - 1),
            cur: (0, 0),
            pass: None,
        }
    }

    /// Start watching a render pass for flicker (DISP-25).
    pub fn begin_pass(&mut self) {
        let n = (W * H) as usize;
        match &mut self.pass {
            Some(p) => {
                p.before.copy_from_slice(&self.px);
                p.writes.iter_mut().for_each(|w| *w = 0);
                p.shown.iter_mut().for_each(|s| *s = 0);
            }
            None => {
                self.pass = Some(Pass { before: self.px.clone(), writes: std::vec![0; n], inter: std::vec![0; n], shown: std::vec![0; n], heat: std::vec![0; n] })
            }
        }
    }

    /// Flickering passes per pixel, summed over every pass watched so far (row-major, W x H).
    pub fn heat(&self) -> Option<&[u32]> {
        self.pass.as_ref().map(|p| p.heat.as_slice())
    }

    /// Forget the heat map.
    pub fn clear_heat(&mut self) {
        if let Some(p) = &mut self.pass {
            p.heat.iter_mut().for_each(|h| *h = 0);
        }
    }

    /// End the pass begun by [`FrameBuffer::begin_pass`]: its flicker and overdraw, added to the heat map when
    /// `heat` (a live update; a change of screen redraws everything).
    pub fn end_pass(&mut self, heat: bool) -> PassReport {
        let Some(p) = &mut self.pass else { return PassReport::default() };
        let mut r = PassReport::default();
        for i in 0..(W * H) as usize {
            r.writes += p.writes[i] as u64;
            if self.px[i] != p.before[i] {
                r.changed += 1;
            }
            let flick = match p.shown[i] {
                0 => false,
                1 => p.inter[i] != self.px[i],
                _ => true,
            };
            if flick {
                p.heat[i] += heat as u32;
                r.flicker += 1;
                let (x, y) = ((i as i32) % W, (i as i32) / W);
                r.bbox = Some(match r.bbox {
                    None => (x, y, x, y),
                    Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
                });
            }
        }
        r
    }

    pub fn at(&self, x: i32, y: i32) -> u16 {
        self.px[(y * W + x) as usize]
    }

    fn put(&mut self, c: u16) {
        let (x0, y0, x1, y1) = self.win;
        let (x, y) = self.cur;
        if x < W && y < H {
            let i = (y * W + x) as usize;
            if let Some(p) = &mut self.pass {
                // The value this write replaces was on the panel for a moment if this pass wrote it already.
                if p.writes[i] > 0 && self.px[i] != p.before[i] {
                    match p.shown[i] {
                        0 => (p.inter[i], p.shown[i]) = (self.px[i], 1),
                        1 if p.inter[i] != self.px[i] => p.shown[i] = 2,
                        _ => {}
                    }
                }
                p.writes[i] = p.writes[i].saturating_add(1);
            }
            self.px[i] = c;
            self.writes[i] = self.writes[i].saturating_add(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// DISP-25: blanking then drawing shows the blank (flicker); drawing over in one write doesn't; blanking an
    /// area that ends blank or was blank doesn't count.
    #[test]
    fn flicker_is_an_intermediate_value() {
        let mut fb = FrameBuffer::new();
        fb.window(0, 0, 9, 0);
        fb.push(5, 10);
        // blank then draw the same: the blank showed
        fb.begin_pass();
        fb.window(0, 0, 9, 0);
        fb.push(0, 10);
        fb.window(0, 0, 9, 0);
        fb.push(5, 10);
        let r = fb.end_pass(true);
        assert_eq!((r.flicker, r.bbox), (10, Some((0, 0, 9, 0))));
        // drawn over once: none
        fb.begin_pass();
        fb.window(0, 0, 9, 0);
        fb.push(7, 10);
        assert_eq!(fb.end_pass(true).flicker, 0);
        // blanked and left blank, or a pixel that was blank before: none
        fb.begin_pass();
        fb.window(0, 1, 9, 1);
        fb.push(0, 10);
        fb.window(0, 1, 9, 1);
        fb.push(3, 10);
        fb.window(0, 0, 9, 0);
        fb.push(0, 10);
        let r = fb.end_pass(true);
        assert_eq!(r.flicker, 0, "{r:?}");
        assert_eq!(r.changed, 20);
    }
}
