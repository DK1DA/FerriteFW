//! Drawing straight to a panel (no frame buffer): fills, frames, 1-bit text, 4-bit anti-aliased text and
//! masks. Every `Panel` gets these methods. Mirrors fw/drivers/gfx.c.
use crate::font4::{rle_row, Font4, Glyph4, Mask4, MaskRows};
use crate::panel::{Panel, H, W};

pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

// Round-7 house style (blue theme)
pub const BG: u16 = rgb565(0x0B, 0x0D, 0x10);
pub const SURFACE: u16 = rgb565(0x11, 0x14, 0x18);
pub const LINE: u16 = rgb565(0x2A, 0x2F, 0x36);
pub const TEXT: u16 = rgb565(0xF0, 0xF3, 0xF6);
pub const SUB: u16 = rgb565(0x9A, 0xA3, 0xAE);
pub const BLUE: u16 = rgb565(0x4D, 0xA3, 0xFF);
pub const BLUE_FG: u16 = rgb565(0x08, 0x12, 0x1F);
pub const GREEN: u16 = rgb565(0x7C, 0xE3, 0x8B);
pub const AMBER: u16 = rgb565(0xF5, 0xC5, 0x42);
pub const RED: u16 = rgb565(0xE5, 0x48, 0x4D);
/// Slot C's letter (slot A is BLUE, B is AMBER).
pub const VIOLET: u16 = rgb565(0xB5, 0x9C, 0xFF);
/// The focused edit cell's fill.
pub const FOCUS: u16 = rgb565(0x0F, 0x1B, 0x2A);
/// A toast's fill.
pub const TOAST: u16 = rgb565(0x1B, 0x2A, 0x3D);
/// Greyed text: a setting that doesn't apply here.
pub const DIM: u16 = rgb565(0x5A, 0x61, 0x6B);
pub const WHITE: u16 = 0xFFFF;
pub const BLACK: u16 = 0x0000;

// Role tokens (DISP-9, DISP-18): drawn as the token beside them in dark, daylight and night red, their own colour
// in cyberpunk. Their values only name them: a theme always maps them ([`crate::theme::map`]).
/// A card's frequency digits (TEXT).
pub const FREQ: u16 = TEXT - 1;
/// Key names in the hint bar and the key card, and the status bar's clock (TEXT).
pub const KEYS: u16 = TEXT - 2;
/// The status bar's and the hint bar's rules (LINE).
pub const RULE: u16 = LINE + 1;
/// The battery icon's fill (GREEN).
pub const BATT: u16 = GREEN + 1;

pub enum Align {
    Left,
    Center,
    Right,
}

fn expand(v: u16, bits: u32) -> i32 {
    (v as i32) * 255 / ((1 << bits) - 1)
}

fn mix(b: i32, f: i32, a: u8) -> i32 {
    b + ((f - b) * a as i32).div_euclid(15) // floor division, like Python's //
}

/// Blend fg over bg by a/15, through 8-bit channels; identical to gfx_blend in C and to tools/fontgen.py.
pub fn blend(bg: u16, fg: u16, a: u8) -> u16 {
    if a == 0 {
        return bg;
    }
    let r = mix(expand(bg >> 11, 5), expand(fg >> 11, 5), a);
    let g = mix(expand((bg >> 5) & 63, 6), expand((fg >> 5) & 63, 6), a);
    let b = mix(expand(bg & 31, 5), expand(fg & 31, 5), a);
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// UTF-8 to code points; a broken sequence gives '?'.
struct Chars<'a>(&'a [u8]);

impl Iterator for Chars<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        let (&c, rest) = self.0.split_first()?;
        let n = match c {
            0x00..=0x7F => 0,
            0xC0..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF7 => 3,
            _ => {
                self.0 = rest;
                return Some('?' as u32);
            }
        };
        let mut v = (c as u32) & (0x7F >> n);
        for i in 0..n {
            match rest.get(i) {
                Some(&b) if b & 0xC0 == 0x80 => v = v << 6 | (b as u32 & 0x3F),
                _ => {
                    self.0 = &rest[i..];
                    return Some('?' as u32);
                }
            }
        }
        self.0 = &rest[n..];
        Some(v)
    }
}

/// The glyph for c, or '?', or (a digits-only font) its first glyph.
fn glyph4(f: &Font4, c: u32) -> &Glyph4 {
    f.glyph(c).filter(|g| g.adv != 0).or_else(|| f.glyph('?' as u32)).unwrap_or(&f.glyphs[0])
}

/// The widest glyph of a run-length encoded font (tools/fontgen.py checks it).
const RLE_W: usize = 64;
/// The characters of a line in a run-length encoded font that keep their place in their glyph's rows; any after
/// them decode their glyph from the top for each row (slower, same pixels). The big fonts draw 16 at most.
const RLE_CHARS: usize = 32;

fn nib(bits: &[u8], k: usize) -> u8 {
    let b = bits[k >> 1];
    if k & 1 != 0 { b & 15 } else { b >> 4 }
}

pub fn text4_width(s: &[u8], f: &Font4) -> i32 {
    Chars(s).map(|c| glyph4(f, c).adv as i32).sum()
}

/// A glyph's first and last columns with any ink (the fonts store whole cells, blank columns included).
fn glyph_cols(f: &Font4, g: &Glyph4) -> Option<(i32, i32)> {
    let bits = &f.bits[g.off as usize..];
    let (w, h) = (g.w as usize, g.h as usize);
    let mut any = [0u8; RLE_W];
    if f.rle {
        let (mut row, mut pos) = ([0u8; RLE_W], 0);
        for _ in 0..h {
            rle_row(bits, &mut pos, w.min(RLE_W), &mut row);
            any.iter_mut().zip(row).for_each(|(a, p)| *a |= p);
        }
    }
    let lit = |i: usize| if f.rle { any.get(i).is_some_and(|&a| a != 0) } else { (0..h).any(|y| nib(bits, y * w + i) != 0) };
    let l = (0..w).find(|&i| lit(i))?;
    let r = (0..w).rev().find(|&i| lit(i))?;
    Some((l as i32, r as i32 + 1))
}

/// Where the drawn pixels of s start and end, from the pen position: centring on these puts the ink in the
/// middle, where centring on advances leaves a label a pixel or two off (DISP-19).
pub fn text4_ink(s: &[u8], f: &Font4) -> Option<(i32, i32)> {
    let (mut pen, mut ink) = (0, None::<(i32, i32)>);
    for c in Chars(s) {
        let g = glyph4(f, c);
        if let Some((a, b)) = glyph_cols(f, g) {
            let (l, r) = (pen + g.x as i32 + a, pen + g.x as i32 + b);
            ink = Some(ink.map_or((l, r), |(x, y)| (x.min(l), y.max(r))));
        }
        pen += g.adv as i32;
    }
    ink
}

/// Where each character of a line in a run-length font is in its glyph's rows (byte, rows decoded): rows are
/// drawn top to bottom, so each glyph row is decoded once.
#[derive(Default)]
#[doc(hidden)]
pub struct RleAt {
    at: [u16; RLE_CHARS],
    done: [u8; RLE_CHARS],
}

/// One pixel row of a line of text, the pen starting at x, into `line` (its pixels from x0 on): the strongest ink
/// of the glyphs over each pixel, through `lut`. Not generic: the same code for every panel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_row(line: &mut [u16], lut: &[u16; 16], s: &[u8], f: &Font4, x: i32, x0: i32, baseline: i32, row: i32, rle: &mut RleAt) {
    let n = line.len();
    let x1 = x0 + n as i32;
    let mut cov = [0u8; W as usize];
    let mut grow = [0u8; RLE_W];
    let mut pen = x;
    for (k, c) in Chars(s).enumerate() {
        let g = glyph4(f, c);
        let gy = row - (baseline + g.y as i32);
        if gy >= 0 && gy < g.h as i32 && f.rle {
            let (mut p, mut d) = if k < RLE_CHARS { (rle.at[k] as usize, rle.done[k] as i32) } else { (0, 0) };
            let w = (g.w as usize).min(RLE_W);
            crate::prof::count(crate::prof::ROWS_DECODED, (gy + 1 - d).max(0) as u32);
            crate::prof::count(crate::prof::ROWS_DRAWN, 1);
            while d <= gy {
                rle_row(&f.bits[g.off as usize..], &mut p, w, &mut grow);
                d += 1;
            }
            if k < RLE_CHARS {
                (rle.at[k], rle.done[k]) = (p as u16, d as u8);
            }
            for (i, &v) in grow[..w].iter().enumerate() {
                let px = pen + g.x as i32 + i as i32;
                if px >= x0 && px < x1 {
                    let slot = &mut cov[(px - x0) as usize];
                    *slot = (*slot).max(v); // overlapping glyphs: strongest ink
                }
            }
        } else if gy >= 0 && gy < g.h as i32 {
            for i in 0..g.w as i32 {
                let px = pen + g.x as i32 + i;
                if px >= x0 && px < x1 {
                    let v = nib(&f.bits[g.off as usize..], (gy * g.w as i32 + i) as usize);
                    let slot = &mut cov[(px - x0) as usize];
                    *slot = (*slot).max(v); // overlapping glyphs: strongest ink
                }
            }
        }
        pen += g.adv as i32;
    }
    for (px, &c) in line.iter_mut().zip(&cov[..n]) {
        *px = lut[c as usize];
    }
}

pub trait Gfx: Panel {
    fn fill(&mut self, mut x: i32, mut y: i32, mut w: i32, mut h: i32, c: u16) {
        if let Some(r) = self.rec() {
            return r.fill(x, y, w, h, c); // DISP-29
        }
        if w > 2 && h > 2 && crate::theme::outlined(c) {
            // DISP-18: night red draws a dim fill as a red outline, black inside
            self.fill(x + 1, y + 1, w - 2, h - 2, BLACK);
            return self.frame(x, y, w, h, c);
        }
        if x < 0 {
            w += x;
            x = 0;
        }
        if y < 0 {
            h += y;
            y = 0;
        }
        w = w.min(W - x);
        // DISP-29: only the rows the panel keeps (a band's): the rest would be dropped pixel by pixel
        let (r0, r1) = self.rows();
        if y < r0 {
            h -= r0 - y;
            y = r0;
        }
        h = h.min(H - y).min(r1 - y);
        if w <= 0 || h <= 0 {
            return;
        }
        self.window(x, y, x + w - 1, y + h - 1);
        self.push(crate::theme::map(c), (w * h) as u32); // every shape draws through here: themed once
    }

    /// 1 px outline.
    fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, c: u16) {
        self.fill(x, y, w, 1, c);
        self.fill(x, y + h - 1, w, 1, c);
        self.fill(x, y, 1, h, c);
        self.fill(x + w - 1, y, 1, h, c);
    }

    /// Rounded 1 px outline, corner radius r (0 = square).
    fn frame_r(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u16) {
        if let Some(rec) = self.rec() {
            return rec.rounded(true, x, y, w, h, r, c); // DISP-29
        }
        if r <= 0 || w < 2 * r + 2 || h < 2 * r + 2 {
            return self.frame(x, y, w, h, c);
        }
        self.fill(x + r, y, w - 2 * r, 1, c);
        self.fill(x + r, y + h - 1, w - 2 * r, 1, c);
        self.fill(x, y + r, 1, h - 2 * r, c);
        self.fill(x + w - 1, y + r, 1, h - 2 * r, c);
        // corners: midpoint circle, one octant pair per corner
        let (mut px, mut py, mut err) = (r, 0, 1 - r);
        while px >= py {
            for (sx, sy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let cx = if sx < 0 { x + r } else { x + w - 1 - r };
                let cy = if sy < 0 { y + r } else { y + h - 1 - r };
                self.fill(cx + sx * px, cy + sy * py, 1, 1, c);
                self.fill(cx + sx * py, cy + sy * px, 1, 1, c);
            }
            py += 1;
            if err < 0 {
                err += 2 * py + 1;
            } else {
                px -= 1;
                err += 2 * (py - px) + 1;
            }
        }
    }

    /// Filled rectangle with rounded corners (radius r).
    fn fill_r(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u16) {
        if let Some(rec) = self.rec() {
            return rec.rounded(false, x, y, w, h, r, c); // DISP-29
        }
        if w > 2 && h > 2 && crate::theme::outlined(c) {
            // DISP-18: an empty meter segment or a track in night red: a red outline, black inside
            self.fill_r(x + 1, y + 1, w - 2, h - 2, (r - 1).max(0), BLACK);
            return self.frame_r(x, y, w, h, r.min(w / 2).min(h / 2).max(0), c);
        }
        let r = r.min(w / 2).min(h / 2).max(0);
        if self.text_only() && r > 0 {
            // LINK-19.5: the listener takes a rounded fill whole: a dot as an icon, anything bigger as one fill
            if w <= 12 && h <= 12 {
                return self.mask_hint(x, y, x + w - 1, y + h - 1, crate::theme::map(c));
            }
            return self.fill(x, y, w, h, c);
        }
        for dy in 0..r {
            // the circle's half-width at the middle of row dy, in half pixels: (2u)^2 <= (2r)^2 - (2r - 2dy - 1)^2
            let v = 2 * (r - dy) - 1;
            let mut u = 0;
            while 4 * (u + 1) * (u + 1) <= 4 * r * r - v * v {
                u += 1;
            }
            let i = r - u;
            self.fill(x + i, y + dy, w - 2 * i, 1, c);
            self.fill(x + i, y + h - 1 - dy, w - 2 * i, 1, c);
        }
        self.fill(x, y + r, w, h - 2 * r, c);
    }

    /// A filled triangle pointing up, down, left or right (dir 0-3), centred in a size x size box.
    fn tri(&mut self, x: i32, y: i32, size: i32, dir: u8, c: u16) {
        let h = size / 2 + 1; // height; the base is 2h - 1 wide
        let o = (size - h) / 2;
        for i in 0..h {
            let n = if dir == 0 || dir == 2 { i } else { h - 1 - i }; // half-width of this row or column
            match dir {
                0 | 1 => self.fill(x + h - 1 - n, y + o + i, 2 * n + 1, 1, c),
                _ => self.fill(x + o + i, y + h - 1 - n, 1, 2 * n + 1, c),
            }
        }
    }

    /// 1 px line (icons: the envelope's flap).
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u16) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.fill(x, y, 1, 1, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// 4-bit text (UTF-8; characters the font lacks show '?'). x is the pen start, baseline the baseline.
    /// Paints the whole line box, ink blended over bg, so redrawing a value needs no clear. Returns the advance.
    fn text4(&mut self, x: i32, baseline: i32, s: &[u8], f: &'static Font4, fg: u16, bg: u16) -> i32 {
        let top = baseline - f.ascent as i32;
        self.text4_rows(x, baseline, s, f, fg, bg, top, top + f.line_h as i32)
    }

    /// [`Gfx::text4`] painting only the rows of the line box inside `y0..y1`: a big font's box can overlap its
    /// neighbours.
    fn text4_rows(&mut self, x: i32, baseline: i32, s: &[u8], f: &'static Font4, fg: u16, bg: u16, y0: i32, y1: i32) -> i32 {
        self.text4_rows_at(x, baseline, s, f, fg, bg, y0, y1, &mut RleAt::default())
    }

    /// [`Gfx::text4_rows`] continuing a run-length font's glyphs from `at` (a band below the last one drawn).
    #[allow(clippy::too_many_arguments)]
    #[doc(hidden)]
    fn text4_rows_at(&mut self, x: i32, baseline: i32, s: &[u8], f: &'static Font4, fg: u16, bg: u16, y0: i32, y1: i32, at: &mut RleAt) -> i32 {
        let w = text4_width(s, f);
        #[cfg(feature = "clipcheck")]
        if !s.is_empty() && (x < 0 || x + w > W) {
            crate::clipcheck::note("screen", s, W - x.max(0), w);
        }
        if let Some(r) = self.rec() {
            r.text(x, baseline, s, f, fg, bg, y0, y1); // DISP-29
            return w;
        }
        let top = baseline - f.ascent as i32;
        let (x0, x1) = (x.max(0), (x + w).min(W));
        let (r0, r1) = self.rows();
        let (y0, y1) = (y0.max(top).max(r0), y1.min(top + f.line_h as i32).min(r1));
        if x1 <= x0 || y1 <= y0 {
            return w;
        }
        // themed before blending, so the anti-aliasing runs between the colours actually shown
        let bg = crate::theme::map_bg(bg);
        let fg = crate::theme::ink_on(crate::theme::map(fg), bg); // DISP-18: black on night red's solid red
        let mut lut = [0u16; 16];
        for (a, v) in lut.iter_mut().enumerate() {
            *v = blend(bg, fg, a as u8);
        }
        let mut line = [0u16; W as usize];
        let n = (x1 - x0) as usize;
        self.window(x0, y0, x1 - 1, y1 - 1);
        self.text_style(f, fg, bg);
        self.text_hint(x0, y0, x1 - 1, y1 - 1, s);
        if self.text_only() {
            return w; // LINK-19.5: the listener reads the text; nothing to paint
        }
        let t0 = crate::prof::now();
        for row in y0..y1 {
            text_row(&mut line[..n], &lut, s, f, x, x0, baseline, row, at);
            self.push_buf(&line[..n]);
        }
        crate::prof::add(if f.rle { crate::prof::BIG } else { crate::prof::TEXT }, t0);
        w
    }

    /// Left draws from x, Right ends at x, Center centres the visible ink on x.
    fn text4_aligned(&mut self, x: i32, baseline: i32, align: Align, s: &[u8], f: &'static Font4, fg: u16, bg: u16) -> i32 {
        let w = text4_width(s, f);
        let x = match align {
            Align::Left => x,
            Align::Right => x - w,
            Align::Center => x - text4_ink(s, f).map_or(w / 2, |(l, r)| (l + r) / 2),
        };
        self.text4(x, baseline, s, f, fg, bg)
    }

    /// m1 in c1 over bg, then optionally m2 in c2 over that (the logo is a ring mask plus a windings mask, the
    /// same size). Masks decode a row at a time, plain or run-length encoded, straight into the panel.
    fn mask4(&mut self, x: i32, y: i32, m1: &'static Mask4, c1: u16, m2: Option<(&'static Mask4, u16)>, bg: u16) {
        if let Some(r) = self.rec() {
            return r.mask(x, y, m1, c1, m2, bg); // DISP-29
        }
        let (w, h) = (m1.w as i32, m1.h as i32);
        let (x0, x1) = (x.max(0), (x + w).min(W));
        let (y0, y1) = (y.max(0), (y + h).min(H));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let bg = crate::theme::map_bg(bg);
        let (c1, m2) = (crate::theme::ink_on(crate::theme::map(c1), bg), m2.map(|(m, c)| (m, crate::theme::ink_on(crate::theme::map(c), bg))));
        let mut lut = [0u16; 16];
        for (a, v) in lut.iter_mut().enumerate() {
            *v = blend(bg, c1, a as u8);
        }
        let (mut r1, mut r2) = (MaskRows::new(m1), m2.map(|(m, c)| (MaskRows::new(m), c)));
        let (mut a1, mut a2) = ([0u8; 256], [0u8; 256]);
        let mut line = [0u16; W as usize];
        self.window(x0, y0, x1 - 1, y1 - 1);
        self.mask_hint(x0, y0, x1 - 1, y1 - 1, c1);
        if self.text_only() {
            return; // LINK-19.5: the listener notes the icon; nothing to paint
        }
        let t0 = crate::prof::now();
        for row in y..y1 {
            // rows above the screen are decoded and dropped: runs only go forward
            r1.next_row(&mut a1);
            if let Some((r, _)) = &mut r2 {
                r.next_row(&mut a2);
            }
            if row < y0 {
                continue;
            }
            for col in x0..x1 {
                let k = (col - x) as usize;
                let mut px = lut[a1[k] as usize];
                if let Some((_, c2)) = r2 {
                    px = blend(px, c2, a2[k]);
                }
                line[(col - x0) as usize] = px;
            }
            self.push_buf(&line[..(x1 - x0) as usize]);
        }
        crate::prof::add(crate::prof::MASK, t0);
    }
}

impl<P: Panel + ?Sized> Gfx for P {}

/// The cases of fw/tests/host/test_gfx4.c, drawn into the host frame buffer.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::*;
    use crate::fb::FrameBuffer;

    #[test]
    fn blend_matches_python() {
        let t: [(u16, u16, [u16; 6]); 4] = [
            (0x0862, 0xF79E, [0x0862, 0x10C3, 0x73AF, 0x8431, 0xE71C, 0xF79E]),
            (0xF79E, 0x0862, [0xF79E, 0xE71C, 0x8431, 0x73AF, 0x10C3, 0x0862]),
            (0x0862, 0x4D1F, [0x0862, 0x08A3, 0x228F, 0x2AD1, 0x44BD, 0x4D1F]),
            (0x4D1F, 0xF628, [0x4D1F, 0x551E, 0x9D94, 0xA5B3, 0xEE09, 0xF628]),
        ];
        for (bg, fg, want) in t {
            for (k, a) in [0, 1, 7, 8, 14, 15].into_iter().enumerate() {
                assert_eq!(blend(bg, fg, a), want[k], "blend {bg:04X}->{fg:04X} a={a}");
            }
        }
    }

    /// One pixel of a glyph, decoded on its own: a run-length glyph from its top every time.
    fn glyph_px(f: &Font4, g: &Glyph4, gx: i32, gy: i32) -> u8 {
        if !f.rle {
            return nib(&f.bits[g.off as usize..], (gy * g.w as i32 + gx) as usize);
        }
        let (mut row, mut pos) = ([0u8; RLE_W], 0);
        for _ in 0..=gy {
            rle_row(&f.bits[g.off as usize..], &mut pos, g.w as usize, &mut row);
        }
        row[gx as usize]
    }

    /// Naive reference: per pixel, strongest ink of any glyph covering it, through blend.
    fn ref_pixel(px: i32, py: i32, x: i32, base: i32, codes: &[u32], f: &Font4, fg: u16, bg: u16) -> u16 {
        let (mut best, mut pen) = (0, x);
        for &c in codes {
            let g = f.glyph(c).unwrap();
            let (gx, gy) = (px - (pen + g.x as i32), py - (base + g.y as i32));
            if gx >= 0 && gx < g.w as i32 && gy >= 0 && gy < g.h as i32 {
                best = best.max(glyph_px(f, g, gx, gy));
            }
            pen += g.adv as i32;
        }
        blend(bg, fg, best)
    }

    #[test]
    fn text_matches_reference() {
        let codes = [b'C' as u32, b'h' as u32, b' ' as u32, b'0' as u32, b'1' as u32, b'2' as u32, 0xB7, b'g' as u32, b'y' as u32];
        let s = "Ch 012\u{B7}gy".as_bytes(); // the UTF-8 middle dot draws its own glyph
        text_like_reference(s, &codes, &UI_12B, 20);
        // the run-length fonts (each character keeps its place in its glyph's rows), a line partly off the left
        // (the name fonts have the name set, CHAN-31: a '-' for the middle dot)
        let nc: [u32; 9] = core::array::from_fn(|i| b"Ch 012-gy"[i] as u32);
        text_like_reference(b"Ch 012-gy", &nc, &NAME_22, -9);
        text_like_reference(b"Ch 012-gy", &nc, &NAME_26, 3);
        let d: [u32; 7] = core::array::from_fn(|i| b"146.520"[i] as u32);
        text_like_reference(b"146.520", &d, &DIG_46, 1);
        text_like_reference(b"146.520", &d, &DIG_28, 5);
        let wm: [u32; 7] = core::array::from_fn(|i| b"FERRITE"[i] as u32);
        text_like_reference(b"FERRITE", &wm, &WORD_40, 30);
        // past the characters that keep their place: same pixels, decoded from the top
        let long = [b'8'; RLE_CHARS + 3];
        let lc = [b'8' as u32; RLE_CHARS + 3];
        let adv = DIG_28.glyph(b'8' as u32).unwrap().adv as i32;
        text_like_reference(&long, &lc, &DIG_28, 20 - RLE_CHARS as i32 * adv); // the last three on the screen
        assert!(NAME_26.rle && DIG_46.rle && !UI_12.rle && !UI_15B.rle && !DIG_17.rle);
        let mut fb = FrameBuffer::new();
        fb.text4(0, 100, &long, &DIG_28, TEXT, BG); // off the right edge, past the cursors: no panic
    }

    /// The line drawn by text4 matches [`ref_pixel`] pixel for pixel, and paints only its line box.
    fn text_like_reference(s: &[u8], codes: &[u32], f: &'static Font4, x: i32) {
        let mut fb = FrameBuffer::new();
        fb.px.fill(0x5555);
        let base = 100;
        let w = fb.text4(x, base, s, f, TEXT, SURFACE);
        let want_w: i32 = codes.iter().map(|&c| f.glyph(c).unwrap().adv as i32).sum();
        assert_eq!(w, want_w);
        assert_eq!(text4_width(s, f), want_w);
        let top = base - f.ascent as i32;
        let (mut bad, mut ink) = (0, 0);
        for py in top..top + f.line_h as i32 {
            for px in x.max(0)..(x + w).min(W) {
                let r = ref_pixel(px, py, x, base, codes, f, TEXT, SURFACE);
                bad += (fb.at(px, py) != r) as u32;
                ink += (r != SURFACE) as u32;
            }
        }
        assert_eq!(bad, 0, "pixels differ from the reference");
        assert!(ink > 50, "too little ink ({ink} px): glyphs missing?");
        let x0 = x.max(0);
        assert!(fb.at(x0, top - 1) == 0x5555 && fb.at(x0, top + f.line_h as i32) == 0x5555, "painted outside the line box");
        if x + w < W {
            assert_eq!(fb.at(x + w, base), 0x5555, "painted outside the line box");
        }
    }

    /// Each run-length glyph's rows end exactly where the next glyph starts: no glyph reads its neighbour's data.
    #[test]
    fn rle_glyphs_decode_whole() {
        for f in [&NAME_26, &NAME_22, &DIG_46, &DIG_28, &WORD_40] {
            for (i, g) in f.glyphs.iter().enumerate() {
                let end = f.glyphs.get(i + 1).map_or(f.bits.len(), |n| n.off as usize);
                let (mut row, mut pos) = ([0u8; RLE_W], 0);
                for _ in 0..g.h {
                    rle_row(&f.bits[g.off as usize..], &mut pos, g.w as usize, &mut row);
                }
                assert_eq!(g.off as usize + pos, end, "glyph {i}");
            }
        }
    }

    #[test]
    fn unknown_chars_and_clipping() {
        let mut fb = FrameBuffer::new();
        assert_eq!(text4_width("\u{20AC}".as_bytes(), &UI_12), text4_width(b"?", &UI_12)); // euro sign: as '?'
        // the cut fonts: ASCII and the middle dot by direct index or search; anything else is missing
        assert!(UI_12.has(' ') && UI_12.has('~') && UI_12.has('\u{b7}') && !UI_12.has('\u{e9}') && !UI_12.has('\u{20AC}'));
        assert!(UI_15B.has('W') && UI_15B.has('/') && UI_15B.has('w') && !UI_15B.has('\u{e9}'), "headings font: the text set");
        assert!(DIG_46.has('7') && !DIG_46.has('A'));
        assert_eq!(text4_width("\u{e9}t\u{e9}".as_bytes(), &UI_12), 3 * text4_width(b"?", &UI_12)); // 2-byte UTF-8
        assert_eq!(text4_width(b"\xC3", &UI_12), text4_width(b"?", &UI_12)); // a cut sequence
        assert_eq!(text4_width(b"12", &DIG_46), 2 * text4_width(b"1", &DIG_46), "digits not tabular");
        assert_eq!(text4_width(b"1", &DIG_46), text4_width(b"8", &DIG_46), "digits not tabular");
        fb.text4(-30, 10, b"146.520", &DIG_46, TEXT, BG); // off the left and top edges
        fb.text4(200, 330, b"146.520", &DIG_46, TEXT, BG); // off the right and bottom edges
        fb.text4_aligned(120, 160, Align::Center, b"FERRITE", &WORD_40, TEXT, BG);
        let (mut l, mut r) = (W, 0);
        let top = 160 - WORD_40.ascent as i32;
        for py in top..top + WORD_40.line_h as i32 {
            for px in 0..W {
                if fb.at(px, py) != BG && fb.at(px, py) != 0 {
                    (l, r) = (l.min(px), r.max(px));
                }
            }
        }
        assert!(l < W && (118..=121).contains(&((l + r) / 2)), "centred text off centre ({l}..{r})");
    }

    #[test]
    fn mask_layers() {
        let mut fb = FrameBuffer::new();
        fb.mask4(10, 10, &LOGO112_RING, SUB, Some((&LOGO112_WIND, BLUE)), BG);
        for y in 0..112 {
            for x in 0..112 {
                let k = (y * 112 + x) as usize;
                let want = blend(blend(BG, SUB, nib(&LOGO112_RING_RAW, k)), BLUE, nib(&LOGO112_WIND_RAW, k));
                assert_eq!(fb.at(10 + x, 10 + y), want);
            }
        }
        fb.mask4(W - 10, H - 10, &LOGO112_RING, SUB, None, BG); // mostly off screen: clipped
        // off the top-left: the rows and columns that show are the right ones
        fb.mask4(-50, -60, &LOGO112_RING, SUB, None, BG);
        for (sx, sy) in [(0, 0), (30, 20), (61, 51)] {
            let k = ((sy + 60) * 112 + sx + 50) as usize;
            assert_eq!(fb.at(sx, sy), blend(BG, SUB, nib(&LOGO112_RING_RAW, k)));
        }
        fb.mask4(100, 100, &ICON_LOCK, TEXT, None, BG); // a plain (not run-length) mask
        assert_eq!(fb.at(100 + 7, 100 + 10), blend(BG, TEXT, nib(ICON_LOCK.bits, 10 * 14 + 7)));
    }

    #[test]
    fn rle_masks_decode_to_the_originals() {
        for (m, raw) in [(&LOGO112_RING, &LOGO112_RING_RAW[..]), (&LOGO112_WIND, &LOGO112_WIND_RAW[..])] {
            assert!(m.rle && m.bits.len() * 2 < raw.len(), "compressed");
            let mut rows = MaskRows::new(m);
            let mut px = [0u8; 112];
            for y in 0..m.h as usize {
                rows.next_row(&mut px);
                for (x, &p) in px.iter().enumerate() {
                    assert_eq!(p, nib(raw, y * 112 + x), "pixel {x},{y}");
                }
            }
        }
        // damaged data: empty pixels, no panic
        let bad = Mask4 { w: 8, h: 2, rle: true, bits: &[0x87, 0xFF] };
        let (mut rows, mut px) = (MaskRows::new(&bad), [9u8; 8]);
        rows.next_row(&mut px);
        assert_eq!(px, [15, 15, 0, 0, 0, 0, 0, 0]);
        rows.next_row(&mut px);
        assert_eq!(px, [0; 8]);
    }

    /// UTF-8 up to four bytes decodes; a character the font lacks, a stray byte or a cut sequence draws as '?'.
    #[test]
    fn utf8_and_missing_characters_draw_as_question_marks() {
        let q = text4_width(b"?", &UI_12);
        assert_eq!(Chars("é€😀".as_bytes()).collect::<std::vec::Vec<_>>(), ['é' as u32, '€' as u32, '😀' as u32]);
        assert_eq!(Chars(&[0xFF, b'a', 0xE2, 0x82]).collect::<std::vec::Vec<_>>(), ['?' as u32, 'a' as u32, '?' as u32]);
        assert_eq!(text4_width("😀".as_bytes(), &UI_12), q);
    }

    /// Left draws from x, Right ends at x, Center puts the ink's middle on x; text with no ink (spaces) centres
    /// on its advance.
    #[test]
    fn aligned_text_lands_where_asked() {
        let ink_cols = |fb: &FrameBuffer| {
            let cols: std::vec::Vec<i32> = (0..240).filter(|&x| (80..100).any(|y| fb.at(x, y) != 0)).collect();
            (cols[0], *cols.last().unwrap() + 1)
        };
        let s = b"Hi";
        let (w, (l, r)) = (text4_width(s, &UI_12), text4_ink(s, &UI_12).unwrap());
        for (k, x) in [(0, 50), (1, 150), (2, 120)] {
            let align = || [Align::Left, Align::Right, Align::Center].into_iter().nth(k).unwrap();
            let mut fb = FrameBuffer::new();
            assert_eq!(fb.text4_aligned(x, 95, align(), s, &UI_12, 0xFFFF, 0), w);
            let (a, b) = ink_cols(&fb);
            match align() {
                Align::Left => assert_eq!(a, 50 + l),
                Align::Right => assert_eq!(b, 150 - w + r),
                Align::Center => assert!((a + b - 240).abs() <= 1, "ink {a}..{b} centred on 120"),
            }
        }
        assert_eq!(text4_ink(b"  ", &UI_12), None);
    }

    /// A fill partly off the top left keeps only what is on the panel; a mask wholly off it draws nothing; a line
    /// goes either way along both axes.
    #[test]
    fn clipping_and_lines() {
        let mut fb = FrameBuffer::new();
        fb.fill(-5, -3, 10, 8, 0x1111);
        assert_eq!((fb.at(4, 4), fb.at(5, 4), fb.at(4, 5)), (0x1111, 0, 0));
        assert_eq!(fb.writes.iter().filter(|&&w| w > 0).count(), 25);
        fb.mask4(-200, 10, &LOGO112_RING, 0xFFFF, None, 0);
        fb.mask4(10, 400, &LOGO112_RING, 0xFFFF, None, 0);
        assert_eq!(fb.writes.iter().filter(|&&w| w > 0).count(), 25, "masks off the panel draw nothing");
        fb.line(20, 40, 10, 35, 0x2222);
        fb.line(30, 50, 30, 40, 0x2222);
        assert_eq!((fb.at(20, 40), fb.at(10, 35), fb.at(30, 45)), (0x2222, 0x2222, 0x2222));
        assert_eq!((10..=20).filter(|&x| (35..=40).any(|y| fb.at(x, y) == 0x2222)).count(), 11, "a pixel in every column");
    }
}
