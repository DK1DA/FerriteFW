//! The radio's front panel around the simulator's screen, after the drawing in Radtel's manual: the volume and
//! channel knobs on top with the antenna and the indicator, the function keys and the 3 x 4 keypad below the
//! screen, the four side buttons on the left. Drawn in window pixels into the window's buffer only; the
//! screen itself stays the firmware's own 240 x 320 pixels, scaled.
//!
//! Geometry is laid out for scale 2 (a 630 x 1012 window) and multiplied by scale / 2. The keys are drawn larger
//! than the radio's, relative to the screen, so they are easy to click at 1x.

use ferrite_core::keys::Key;
use ferrite_ui::assets::{UI_12, UI_12B, UI_15B};
use ferrite_ui::font4::Font4;

const BODY: u32 = 0x1A1D22;
const BEZEL: u32 = 0x0B0C0E;
const KEY: u32 = 0x2C3139;
const KEY_EDGE: u32 = 0x444B55;
const PRESSED: u32 = 0x4DA3FF;
const PRESSED_FG: u32 = 0x08121F;
const LABEL: u32 = 0xEEF1F4;
const SMALL: u32 = 0x9AA3AE;
const KNOB: u32 = 0x30353C;
const RIDGE: u32 = 0x23272D;

/// What a clickable part does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Part {
    Key(Key),
    /// PTT A or B: transmit on that slot (`Key::Ptt(n)` to the app).
    Ptt(u8),
    /// The channel knob: left half -1, right half +1.
    Knob,
    /// The volume knob's power switch (PE0): a click turns the radio off or on.
    Power,
}

#[derive(Clone, Copy)]
pub struct Hit {
    pub part: Part,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub label: &'static str,
    pub small: &'static str,
}

impl Hit {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

pub struct Face {
    pub w: usize,
    pub h: usize,
    /// The screen's top-left in the window.
    pub sx: usize,
    pub sy: usize,
    pub hits: Vec<Hit>,
    /// The indicator's two LEDs (stock: PC13 red TX, PC14 green RX).
    led: (i32, i32),
    base: Vec<u32>,
}

impl Face {
    /// The panel's size at a scale, without drawing it.
    pub fn size(scale: usize) -> (usize, usize) {
        (630 * scale / 2, 1012 * scale / 2)
    }

    pub fn new(scale: usize) -> Self {
        let u = |v: i32| v * scale as i32 / 2;
        let (w, h) = Self::size(scale);
        let (sx, sy) = (u(112), u(56));
        let sw = 240 * scale as i32; // the screen: 480 x 640 at scale 2
        let k = |part, x, y, w, h, label, small| Hit { part, x: u(x), y: u(y), w: u(w), h: u(h), label, small };
        let mut hits = vec![
            // Side buttons, top to bottom (stock's names small).
            k(Part::Ptt(0), 6, 190, 78, 120, "PTT A", ""),
            k(Part::Ptt(1), 6, 326, 78, 58, "PTT B", ""),
            k(Part::Key(Key::Side(1)), 6, 398, 78, 64, "SIDE1", "PTT C"),
            k(Part::Key(Key::Side(2)), 6, 476, 78, 64, "SIDE2", "Side 2"),
            // The channel knob (the encoder).
            k(Part::Knob, 214, 2, 128, 44, "", ""),
            // The volume knob: only its power switch reaches the MCU.
            k(Part::Power, 134, 6, 56, 40, "", ""),
            // Function keys.
            k(Part::Key(Key::Menu), 112, 712, 86, 40, "OK", "MENU"),
            k(Part::Key(Key::Slot), 112, 758, 86, 40, "A\u{b7}B\u{b7}C", "SLOT"),
            k(Part::Key(Key::Up), 312, 712, 80, 32, "", ""),
            k(Part::Key(Key::Down), 312, 766, 80, 32, "", ""),
            k(Part::Key(Key::Left), 268, 722, 42, 66, "", ""),
            k(Part::Key(Key::Right), 394, 722, 42, 66, "", ""),
            k(Part::Key(Key::Exit), 506, 712, 86, 40, "BACK", ""),
            k(Part::Key(Key::VfoMem), 506, 758, 86, 40, "V/M", ""),
        ];
        let digits: [(Key, &str, &str); 12] = [
            (Key::Digit(1), "1", "!<"),
            (Key::Digit(2), "2", "ABC"),
            (Key::Digit(3), "3", "DEF"),
            (Key::Digit(4), "4", "GHI"),
            (Key::Digit(5), "5", "JKL"),
            (Key::Digit(6), "6", "MNO"),
            (Key::Digit(7), "7", "PQRS"),
            (Key::Digit(8), "8", "TUV"),
            (Key::Digit(9), "9", "WXYZ"),
            (Key::Star, "*", "_"),
            (Key::Digit(0), "0", "+"),
            (Key::Hash, "#", "^"),
        ];
        for (i, (key, label, small)) in digits.into_iter().enumerate() {
            let (c, r) = (i as i32 % 3, i as i32 / 3);
            hits.push(k(Part::Key(key), 112 + c * 165, 810 + r * 50, 150, 44, label, small));
        }
        let mut f = Face { w, h, sx: sx as usize, sy: sy as usize, hits, led: (u(380), u(40)), base: vec![0; w * h] };
        f.draw_base(scale, sw);
        f
    }

    fn draw_base(&mut self, scale: usize, sw: i32) {
        let u = |v: i32| v * scale as i32 / 2;
        let (w, h) = (self.w as i32, self.h as i32);
        let mut base = std::mem::take(&mut self.base);
        let mut c = Canvas { px: &mut base, w, h };
        c.fill(0, 0, w, h, 0x0E0F12);
        // Antenna (right of the knobs) and the knobs on the top edge.
        c.rrect(u(414), 0, u(30), u(46), u(4), 0x15171A);
        for i in 0..6 {
            c.fill(u(412), u(22 + i * 4), u(34), u(2), 0x26292E);
        }
        c.text(u(134) - 4 - width("VOL", &UI_12), u(42) - UI_12.line_h as i32 - 2, "VOL", &UI_12, SMALL, Align::Left);
        // Body and the screen's bezel.
        c.rrect(u(90), u(42), u(530), h - u(46), u(28), BODY);
        c.rrect(self.sx as i32 - u(10), self.sy as i32 - u(10), sw + u(20), 320 * scale as i32 + u(20), u(10), BEZEL);
        // The d-pad's surround.
        c.rrect(u(264), u(708), u(176), u(94), u(12), 0x23272D);
        let hits = self.hits.clone();
        for hit in &hits {
            draw_hit(&mut c, hit, false, scale);
        }
        self.base = base;
    }

    /// The panel with keys highlighted (`pressed` holds indexes into `hits`), the knob half last clicked
    /// (+1 / -1) and the LEDs; the caller then copies the screen in at (sx, sy).
    pub fn draw(&self, out: &mut [u32], pressed: &[usize], knob: i8, rx: bool, tx: bool, scale: usize) {
        out.copy_from_slice(&self.base);
        let mut c = Canvas { px: out, w: self.w as i32, h: self.h as i32 };
        for &i in pressed {
            draw_hit(&mut c, &self.hits[i], true, scale);
        }
        if knob != 0 {
            if let Some(k) = self.hits.iter().find(|h| h.part == Part::Knob) {
                let mut half = *k;
                half.w /= 2;
                if knob > 0 {
                    half.x += half.w;
                }
                c.rrect(half.x + 2, half.y + 2, half.w - 4, half.h - 4, 4, 0x3A5A80);
            }
        }
        // The indicator: one window, red (TX) and green (RX) dies as in stock (PC13, PC14).
        let u = |v: i32| v * scale as i32 / 2;
        let (lx, ly) = self.led;
        c.rrect(lx, ly - u(4), u(30), u(10), u(5), 0x08090B);
        c.rrect(lx + u(3), ly - u(2), u(11), u(6), u(3), if tx { 0xFF3B30 } else { 0x3A1A1A });
        c.rrect(lx + u(16), ly - u(2), u(11), u(6), u(3), if rx && !tx { 0x5CFF6E } else { 0x16301A });
    }

    /// The screen into the panel at (sx, sy), scaled; `px(x, y)` gives each screen pixel as RGB.
    pub fn put_screen(&self, out: &mut [u32], scale: usize, px: impl Fn(i32, i32) -> u32) {
        for y in 0..320 {
            for x in 0..240 {
                let c = px(x, y);
                for dy in 0..scale {
                    let row = (self.sy + y as usize * scale + dy) * self.w + self.sx + x as usize * scale;
                    out[row..row + scale].fill(c);
                }
            }
        }
    }

    /// The part under a window position.
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.hits.iter().position(|h| h.contains(x, y))
    }

    /// The part a keyboard key stands for (PTT is PTT A).
    pub fn find(&self, key: Key) -> Option<usize> {
        self.hits.iter().position(|h| match h.part {
            Part::Key(k) => k == key,
            Part::Ptt(n) => key == Key::Ptt(n),
            _ => false,
        })
    }
}

fn draw_hit(c: &mut Canvas, h: &Hit, pressed: bool, scale: usize) {
    let u = |v: i32| v * scale as i32 / 2;
    let (fill, fg, sub) = if pressed { (PRESSED, PRESSED_FG, PRESSED_FG) } else { (KEY, LABEL, SMALL) };
    let cx = h.x + h.w / 2;
    match h.part {
        Part::Power => {
            c.rrect(h.x, h.y + u(4), h.w, h.h - u(4), u(8), if pressed { PRESSED } else { KNOB });
            for i in 0..6 {
                c.fill(h.x + u(6 + i * 9), h.y + u(8), u(3), h.h - u(12), if pressed { PRESSED_FG } else { RIDGE });
            }
        }
        Part::Knob => {
            c.rrect(h.x, h.y + u(4), h.w, h.h - u(4), u(10), KNOB);
            for i in 0..(h.w - u(12)) / u(9) + 1 {
                c.fill(h.x + u(6 + i * 9), h.y + u(8), u(3), h.h - u(12), RIDGE);
            }
            // left steps down, right steps up
            let cy = h.y + u(4) + (h.h - u(4)) / 2;
            c.rrect(h.x + u(6), cy - u(12), u(30), u(24), u(6), KNOB);
            c.rrect(h.x + h.w - u(36), cy - u(12), u(30), u(24), u(6), KNOB);
            c.tri(h.x + u(21), cy, u(12), 2, LABEL);
            c.tri(h.x + h.w - u(21), cy, u(12), 3, LABEL);
        }
        Part::Key(k @ (Key::Up | Key::Down | Key::Left | Key::Right)) => {
            c.rrect(h.x, h.y, h.w, h.h, u(6), if pressed { PRESSED } else { 0x2C3139 });
            let dir = match k {
                Key::Up => 0,
                Key::Down => 1,
                Key::Left => 2,
                _ => 3,
            };
            c.tri(cx, h.y + h.h / 2, u(7), dir, fg);
        }
        _ => {
            c.rrect(h.x, h.y, h.w, h.h, u(8), KEY_EDGE);
            c.rrect(h.x + 1, h.y + 1, h.w - 2, h.h - 2, u(8), fill);
            let side = matches!(h.part, Part::Ptt(_) | Part::Key(Key::Side(_)));
            let big = if scale >= 2 { &UI_15B } else { &UI_12B };
            if side {
                // Ferrite's role, stock's name under it.
                let fits = !h.small.is_empty() && width(h.small, &UI_12) <= h.w - 4 && h.h >= 32;
                let y = h.y + h.h / 2 - if fits { 15 } else { 8 };
                c.text(cx, y, h.label, &UI_12B, fg, Align::Center);
                if fits {
                    c.text(cx, y + 15, h.small, &UI_12, sub, Align::Center);
                }
            } else if h.label.len() == 1 || h.label == "A\u{b7}B\u{b7}C" || matches!(h.label, "OK" | "BACK" | "V/M") {
                // Main legend, then the small one to its right.
                let lw = width(h.label, big);
                // the small legend only where it fits (at 1x the keys are narrow)
                let small = if lw + width(h.small, &UI_12) + 6 <= h.w - 8 { h.small } else { "" };
                let sw = if small.is_empty() { 0 } else { width(small, &UI_12) + 6 };
                let x0 = cx - (lw + sw) / 2;
                let y = h.y + (h.h - big.line_h as i32) / 2;
                c.text(x0, y, h.label, big, fg, Align::Left);
                match small {
                    "_" => {
                        // the space bar mark
                        let (x, y) = (x0 + lw + 6, h.y + h.h / 2 + 3);
                        c.fill(x, y, u(12), 2, sub);
                        c.fill(x, y - u(4), 2, u(4), sub);
                        c.fill(x + u(12) - 2, y - u(4), 2, u(4), sub);
                    }
                    "^" => c.tri(x0 + lw + 6 + u(5), h.y + h.h / 2, u(5), 0, sub),
                    s => c.text(x0 + lw + 6, y + 3, s, &UI_12, sub, Align::Left),
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
}

fn width(s: &str, f: &Font4) -> i32 {
    ferrite_ui::gfx::text4_width(s.as_bytes(), f)
}

/// A u32 RGB buffer to draw the panel into.
struct Canvas<'a> {
    px: &'a mut [u32],
    w: i32,
    h: i32,
}

impl Canvas<'_> {
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        for yy in y.max(0)..(y + h).min(self.h) {
            for xx in x.max(0)..(x + w).min(self.w) {
                self.px[(yy * self.w + xx) as usize] = c;
            }
        }
    }

    /// Filled rectangle with rounded corners.
    fn rrect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u32) {
        let r = r.min(w / 2).min(h / 2);
        for yy in 0..h {
            let dy = if yy < r { r - yy } else if yy >= h - r { yy - (h - r) + 1 } else { 0 };
            let inset = if dy > 0 { r - ((r * r - (dy - 1) * (dy - 1)) as f32).sqrt() as i32 } else { 0 };
            self.fill(x + inset, y + yy, w - 2 * inset, 1, c);
        }
    }

    /// A triangle centred on (cx, cy), half-size s, pointing up, down, left or right (0-3).
    fn tri(&mut self, cx: i32, cy: i32, s: i32, dir: u8, c: u32) {
        for i in 0..=s {
            let half = i;
            match dir {
                0 => self.fill(cx - half, cy - s / 2 + i, 2 * half + 1, 1, c),
                1 => self.fill(cx - half, cy + s / 2 - i, 2 * half + 1, 1, c),
                2 => self.fill(cx - s / 2 + i, cy - half, 1, 2 * half + 1, c),
                _ => self.fill(cx + s / 2 - i, cy - half, 1, 2 * half + 1, c),
            }
        }
    }

    /// 4-bit anti-aliased text from ferrite-ui's fonts, top of the line box at y.
    fn text(&mut self, x: i32, y: i32, s: &str, f: &Font4, fg: u32, align: Align) {
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - width(s, f) / 2,
        };
        let base = y + f.ascent as i32;
        for ch in s.chars() {
            let code = ch as u32;
            let Some(g) = f.glyph(code) else { continue };
            for gy in 0..g.h as i32 {
                for gx in 0..g.w as i32 {
                    let k = (gy * g.w as i32 + gx) as usize;
                    let b = f.bits[g.off as usize + k / 2];
                    let a = if k & 1 == 0 { b >> 4 } else { b & 15 } as u32;
                    let (px, py) = (pen + g.x as i32 + gx, base + g.y as i32 + gy);
                    if a > 0 && px >= 0 && px < self.w && py >= 0 && py < self.h {
                        let i = (py * self.w + px) as usize;
                        self.px[i] = mix(self.px[i], fg, a);
                    }
                }
            }
            pen += g.adv as i32;
        }
    }
}

fn mix(bg: u32, fg: u32, a: u32) -> u32 {
    let ch = |s: u32| {
        let (b, f) = ((bg >> s) & 255, (fg >> s) & 255);
        (b * (15 - a) + f * a) / 15
    };
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_maps_back_through_the_centred_panel() {
        use crate::window::{fit_scale, to_panel};
        let buf = (616, 900);
        assert_eq!(to_panel((0.0, 0.0), buf, buf), (0, 0));
        assert_eq!(to_panel((308.0, 450.0), buf, buf), (308, 450));
        // a bigger window: the panel centred, 1:1
        assert_eq!(to_panel((100.0 + 308.0, 450.0), (816, 900), buf), (308, 450));
        assert!(to_panel((50.0, 10.0), (816, 900), buf).0 < 0, "in the margin: outside the panel");
        let sizes = [(308, 450), (616, 900), (924, 1350)];
        assert_eq!(fit_scale((700, 1000), &sizes), 2);
        assert_eq!(fit_scale((616, 899), &sizes), 1);
        assert_eq!(fit_scale((100, 100), &sizes), 1);
    }

    #[test]
    fn fits_a_laptop_and_parts_dont_overlap() {
        let f = Face::new(2);
        assert!(f.h <= 1012, "{} px tall", f.h);
        let screen = (f.sx as i32, f.sy as i32, 480, 640);
        for (i, a) in f.hits.iter().enumerate() {
            assert!(a.x >= 0 && a.y >= 0 && a.x + a.w <= f.w as i32 && a.y + a.h <= f.h as i32, "{:?} off the window", a.part);
            let over = |x: i32, y: i32, w: i32, h: i32| a.x < x + w && x < a.x + a.w && a.y < y + h && y < a.y + a.h;
            assert!(!over(screen.0, screen.1, screen.2, screen.3), "{:?} over the screen", a.part);
            for b in &f.hits[i + 1..] {
                assert!(!over(b.x, b.y, b.w, b.h), "{:?} overlaps {:?}", a.part, b.part);
            }
        }
        assert_eq!(f.find(Key::Ptt(0)).map(|i| f.hits[i].part), Some(Part::Ptt(0)));
        assert!(f.find(Key::Digit(0)).is_some() && f.find(Key::Side(2)).is_some());
    }

    /// `cargo test -p ferrite-sim face_png -- --ignored`: the bare front panel at 1x and 2x, for a look.
    #[test]
    #[ignore]
    fn face_png() {
        for scale in [1, 2] {
            let f = Face::new(scale);
            let mut out = vec![0u32; f.w * f.h];
            f.draw(&mut out, &[], 0, false, false, scale);
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../target/sim/face-{scale}x.png"));
            crate::save_rgb(&out, f.w, f.h, &p);
        }
    }
}
