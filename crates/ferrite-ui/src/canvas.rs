//! Drawing off-screen in bands (DISP-25): a region's ordinary drawing code (blank, fill, text, frame, in any order)
//! runs against a small RAM band a few rows tall, and each band goes to the panel once. Every pixel of the region
//! is written once per redraw, so the panel never shows a blank between two layers. On the radio's bus that
//! blank flickers (found on the black radio, Oct 12, 2026).
//!
//! The band is on the stack: [`BAND_PX`] pixels (3.75 KB), in a function never inlined ([`band_out`],
//! [`band_over`]), so it is there only while a band is drawn: inlined, every drawing function that calls these kept
//! its own copy for its whole run, and a redraw nests several (render, the screen, a card, a widget: ~30 KB of
//! stack measured on the radio, SBAR-23). The drawing runs once per band, so a taller region costs drawing time,
//! not bus time.
use crate::panel::Panel;

/// Pixels in one band: 240 x 8.
pub const BAND_PX: usize = 1920;

/// One band: what the drawing code sees as the panel. Pixels outside the band are dropped; on the host's pass for
/// text hints (`fwd`), windows and hints go to the real panel and pixels nowhere.
pub struct Band<'a> {
    buf: &'a mut [u16],
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    win: (i32, i32, i32, i32),
    cur: (i32, i32),
    fwd: Option<&'a mut dyn Panel>,
    /// A canvas drawn inside another's band: everything goes to `fwd`, the outer band.
    thru: bool,
    /// [`banded_over`]: a bit per band pixel that was drawn; only those go to the panel.
    mask: Option<&'a mut [u8]>,
}

impl Band<'_> {
    /// The run of `n` pixels from the cursor along its row: the part inside the band as (buffer index, count), and
    /// the cursor moved on.
    fn run(&mut self, n: u32) -> (Option<(usize, usize, usize)>, u32) {
        let (x, y) = self.cur;
        let (x0, _, x1, _) = self.win;
        let take = ((x1 - x + 1).max(1) as u32).min(n);
        let mut inside = None;
        if y >= self.y && y < self.y + self.h {
            let a = x.max(self.x);
            let b = (x + take as i32).min(self.x + self.w);
            if b > a {
                inside = Some((((y - self.y) * self.w + (a - self.x)) as usize, (a - x) as usize, (b - a) as usize));
            }
        }
        self.cur = if x + take as i32 > x1 { (x0, y + 1) } else { (x + take as i32, y) };
        (inside, take)
    }
}

impl Band<'_> {
    fn mark(&mut self, i: usize, n: usize) {
        if let Some(m) = &mut self.mask {
            for k in i..i + n {
                m[k / 8] |= 1 << (k % 8);
            }
        }
    }
}

impl Panel for Band<'_> {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.win = (x0, y0, x1, y1);
        self.cur = (x0, y0);
        if let Some(p) = &mut self.fwd {
            p.window(x0, y0, x1, y1);
        }
    }

    fn push(&mut self, color: u16, mut count: u32) {
        if let Some(p) = &mut self.fwd {
            if self.thru {
                p.push(color, count);
            }
            return;
        }
        while count > 0 {
            let (inside, take) = self.run(count);
            if let Some((i, _, n)) = inside {
                self.buf[i..i + n].fill(color);
                self.mark(i, n);
            }
            count -= take;
        }
    }

    fn push_buf(&mut self, px: &[u16]) {
        if let Some(p) = &mut self.fwd {
            if self.thru {
                p.push_buf(px);
            }
            return;
        }
        let mut px = px;
        while !px.is_empty() {
            let (inside, take) = self.run(px.len() as u32);
            if let Some((i, off, n)) = inside {
                self.buf[i..i + n].copy_from_slice(&px[off..off + n]);
                self.mark(i, n);
            }
            px = &px[take as usize..];
        }
    }

    fn text_hint(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, s: &[u8]) {
        if let Some(p) = &mut self.fwd {
            p.text_hint(x0, y0, x1, y1, s);
        }
    }

    /// The band's rows, so text outside them isn't drawn at all (every band runs the whole drawing); all of them
    /// on the pass for text hints.
    fn rows(&self) -> (i32, i32) {
        match &self.fwd {
            Some(p) if self.thru => p.rows(),
            Some(_) => (0, crate::panel::H),
            None => (self.y, self.y + self.h),
        }
    }

    /// Off the screen, except a [`direct`] band over the panel itself: drawing through it is drawing on the panel.
    fn offscreen(&self) -> bool {
        match &self.fwd {
            Some(p) if self.thru => p.offscreen(),
            _ => true,
        }
    }

    fn input_pending(&self) -> bool {
        match &self.fwd {
            Some(p) if self.thru => p.input_pending(),
            _ => false,
        }
    }
}

/// Inside another canvas's band: draw straight into it (a second band on the stack would cost 3.75 KB more).
fn thru(lcd: &mut dyn Panel) -> Band<'_> {
    Band { buf: &mut [], x: 0, y: 0, w: 0, h: 0, win: (0, 0, 0, 0), cur: (0, 0), fwd: Some(lcd), thru: true, mask: None }
}

/// A band that passes everything straight to `lcd`, the panel or another band, and is what `lcd` is (on screen
/// or off): drawing code that runs both on the panel and inside a canvas takes a `&mut Band` and is compiled once,
/// not once per panel type (home: 14.8 KB, size review Oct 20, 2026). Each call goes through `lcd`'s
/// vtable, once per primitive (a fill, a text row, a band), never per pixel.
pub fn direct(lcd: &mut dyn Panel) -> Band<'_> {
    thru(lcd)
}

/// Draw the region (x, y, w, h) with `draw`, band by band, each band sent to `lcd` once. `draw` paints the whole
/// region in absolute coordinates, as it would on the panel (anything it leaves unpainted shows as `bg`). On the
/// host (`std`) a last pass hands the text hints to the panel, for the simulator's screen reading.
pub fn banded<P: Panel>(lcd: &mut P, x: i32, y: i32, w: i32, h: i32, bg: u16, mut draw: impl FnMut(&mut Band)) {
    if w <= 0 || h <= 0 {
        return;
    }
    if lcd.offscreen() {
        crate::gfx::Gfx::fill(lcd, x, y, w, h, bg);
        return draw(&mut thru(lcd));
    }
    band_out(lcd, x, y, w, h, bg, &mut draw);
    if cfg!(feature = "std") {
        let mut band = Band { buf: &mut [], x, y, w: 0, h: 0, win: (0, 0, 0, 0), cur: (0, 0), fwd: Some(lcd), thru: false, mask: None };
        draw(&mut band);
    }
}

/// [`banded`]'s bands: the buffer lives in this frame alone.
#[inline(never)]
fn band_out<P: Panel>(lcd: &mut P, x: i32, y: i32, w: i32, h: i32, bg: u16, draw: &mut dyn FnMut(&mut Band)) {
    let w = w.min(BAND_PX as i32);
    let rows = (BAND_PX as i32 / w).max(1);
    let bg = crate::theme::map_bg(bg);
    let mut buf = [0u16; BAND_PX];
    let mut by = y;
    while by < y + h {
        let bh = rows.min(y + h - by);
        let n = (w * bh) as usize;
        buf[..n].fill(bg);
        {
            let mut band = Band { buf: &mut buf[..n], x, y: by, w, h: bh, win: (0, 0, 0, 0), cur: (0, 0), fwd: None, thru: false, mask: None };
            draw(&mut band);
        }
        lcd.window(x, by, x + w - 1, by + bh - 1);
        lcd.push_buf(&buf[..n]);
        by += bh;
    }
}

/// As [`banded`], for a shape drawn over what is already on screen (a popup with rounded corners): only the
/// pixels `draw` paints go to the panel, a run at a time, so the rest keeps what was under it.
pub fn banded_over<P: Panel>(lcd: &mut P, x: i32, y: i32, w: i32, h: i32, mut draw: impl FnMut(&mut Band)) {
    if w <= 0 || h <= 0 {
        return;
    }
    if lcd.offscreen() {
        return draw(&mut thru(lcd));
    }
    band_over(lcd, x, y, w, h, &mut draw);
}

/// [`banded_over`]'s bands: the buffer and mask live in this frame alone.
#[inline(never)]
fn band_over<P: Panel>(lcd: &mut P, x: i32, y: i32, w: i32, h: i32, draw: &mut dyn FnMut(&mut Band)) {
    let w = w.min(BAND_PX as i32);
    let rows = (BAND_PX as i32 / w).max(1);
    let mut buf = [0u16; BAND_PX];
    let mut mask = [0u8; BAND_PX / 8];
    let mut by = y;
    while by < y + h {
        let bh = rows.min(y + h - by);
        let n = (w * bh) as usize;
        mask.fill(0);
        {
            let mut band =
                Band { buf: &mut buf[..n], x, y: by, w, h: bh, win: (0, 0, 0, 0), cur: (0, 0), fwd: None, thru: false, mask: Some(&mut mask) };
            draw(&mut band);
        }
        for r in 0..bh {
            let mut c = 0;
            while c < w {
                let k = (r * w + c) as usize;
                if mask[k / 8] & 1 << (k % 8) == 0 {
                    c += 1;
                    continue;
                }
                let start = c;
                while c < w && mask[((r * w + c) as usize) / 8] & 1 << ((r * w + c) as usize % 8) != 0 {
                    c += 1;
                }
                lcd.window(x + start, by + r, x + c - 1, by + r);
                let a = (r * w + start) as usize;
                lcd.push_buf(&buf[a..a + (c - start) as usize]);
            }
        }
        by += bh;
    }
    if cfg!(feature = "std") {
        let mut band = Band { buf: &mut [], x, y, w: 0, h: 0, win: (0, 0, 0, 0), cur: (0, 0), fwd: Some(lcd), thru: false, mask: None };
        draw(&mut band);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;
    use crate::gfx::Gfx;

    /// DISP-25: a blank-then-draw region drawn through the bands ends as drawn directly, with every pixel written
    /// once and no flicker.
    #[test]
    fn banded_drawing_is_flicker_free() {
        let draw = |p: &mut dyn FnMut(i32, i32, i32, i32, u16)| {
            p(10, 20, 100, 30, 0x1111);
            p(15, 25, 20, 10, 0x2222);
            p(60, 22, 30, 25, 0x3333);
        };
        let mut direct = FrameBuffer::new();
        draw(&mut |x, y, w, h, c| direct.fill(x, y, w, h, c));
        let mut fb = FrameBuffer::new();
        fb.window(0, 0, 239, 319);
        fb.push(0x7777, 240 * 320);
        fb.begin_pass();
        banded(&mut fb, 10, 20, 100, 30, 0, |b| draw(&mut |x, y, w, h, c| b.fill(x, y, w, h, c)));
        let r = fb.end_pass(true);
        assert_eq!(r.flicker, 0);
        assert_eq!(r.writes, 3000, "each pixel of the region once");
        for y in 20..50 {
            for x in 10..110 {
                assert_eq!(fb.at(x, y), direct.at(x, y), "{x},{y}");
            }
        }
    }
}
