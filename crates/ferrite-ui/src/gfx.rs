//! Drawing straight to a panel (no frame buffer): fills, frames, 1-bit text, 4-bit anti-aliased text and
//! masks. Every `Panel` gets these methods. Mirrors fw/drivers/gfx.c.
use crate::font1::Font1;
use crate::font4::{Font4, Glyph4, Mask4};
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
pub const WHITE: u16 = 0xFFFF;
pub const BLACK: u16 = 0x0000;

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

/// UTF-8 to Latin-1; anything outside Latin-1 becomes '?'. Same rules as next_char in C.
struct Latin1<'a>(&'a [u8]);

impl Iterator for Latin1<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        let cont = |s: &[u8]| s.first().is_some_and(|&n| n & 0xC0 == 0x80);
        let (&c, rest) = self.0.split_first()?;
        self.0 = rest;
        if c < 0x80 {
            return Some(c as u32);
        }
        if (c == 0xC2 || c == 0xC3) && cont(rest) {
            self.0 = &rest[1..];
            return Some((c as u32 & 3) << 6 | (rest[0] as u32 & 0x3F));
        }
        while cont(self.0) {
            self.0 = &self.0[1..];
        }
        Some('?' as u32)
    }
}

fn glyph4(f: &Font4, c: u32) -> &Glyph4 {
    let at = |c: u32| c.checked_sub(f.first as u32).and_then(|i| f.glyphs.get(i as usize)).filter(|g| g.adv != 0);
    at(c).or_else(|| at('?' as u32)).unwrap_or(&f.glyphs[0]) // a digits-only font: its first glyph
}

fn nib(bits: &[u8], k: usize) -> u8 {
    let b = bits[k >> 1];
    if k & 1 != 0 { b & 15 } else { b >> 4 }
}

/// 1-bit text: every character is the same width.
pub fn text_width(s: &[u8], f: &Font1) -> i32 {
    s.len() as i32 * f.w as i32
}

pub fn text4_width(s: &[u8], f: &Font4) -> i32 {
    Latin1(s).map(|c| glyph4(f, c).adv as i32).sum()
}

pub trait Gfx: Panel {
    fn fill(&mut self, mut x: i32, mut y: i32, mut w: i32, mut h: i32, c: u16) {
        if x < 0 {
            w += x;
            x = 0;
        }
        if y < 0 {
            h += y;
            y = 0;
        }
        w = w.min(W - x);
        h = h.min(H - y);
        if w <= 0 || h <= 0 {
            return;
        }
        self.window(x, y, x + w - 1, y + h - 1);
        self.push(c, (w * h) as u32);
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

    /// 1-bit text, top-left at x, y. Characters that would not fit on the panel are skipped. Returns the width.
    fn text(&mut self, x: i32, y: i32, s: &[u8], f: &Font1, fg: u16, bg: u16) -> i32 {
        let (w, h, bpr) = (f.w as i32, f.h as i32, f.bytes_per_row());
        let mut line = [0u16; 32];
        let mut pen = x;
        for &c in s {
            if pen >= 0 && y >= 0 && pen + w <= W && y + h <= H {
                let g = f.glyph(c);
                self.window(pen, y, pen + w - 1, y + h - 1);
                for row in g.chunks_exact(bpr) {
                    for (col, px) in line[..w as usize].iter_mut().enumerate() {
                        *px = if row[col / 8] & (0x80 >> (col % 8)) != 0 { fg } else { bg };
                    }
                    self.push_buf(&line[..w as usize]);
                }
            }
            pen += w;
        }
        pen - x
    }

    /// 1-bit text clipped to a field `w` wide, the rest of the field cleared: never spills into the next one.
    fn text_field(&mut self, x: i32, y: i32, w: i32, s: &[u8], f: &Font1, fg: u16, bg: u16) {
        let max = ((w / f.w as i32) as usize).min(63);
        let used = self.text(x, y, &s[..s.len().min(max)], f, fg, bg);
        if used < w {
            self.fill(x + used, y, w - used, f.h as i32, bg);
        }
    }

    /// 4-bit text (UTF-8; Latin-1 draws, the rest shows '?'). x is the pen start, baseline the baseline.
    /// Paints the whole line box, ink blended over bg, so redrawing a value needs no clear. Returns the advance.
    fn text4(&mut self, x: i32, baseline: i32, s: &[u8], f: &Font4, fg: u16, bg: u16) -> i32 {
        let mut lut = [0u16; 16];
        for (a, v) in lut.iter_mut().enumerate() {
            *v = blend(bg, fg, a as u8);
        }
        let w = text4_width(s, f);
        let top = baseline - f.ascent as i32;
        let (x0, x1) = (x.max(0), (x + w).min(W));
        let (y0, y1) = (top.max(0), (top + f.line_h as i32).min(H));
        if x1 <= x0 || y1 <= y0 {
            return w;
        }
        let mut cov = [0u8; W as usize];
        let mut line = [0u16; W as usize];
        let n = (x1 - x0) as usize;
        self.window(x0, y0, x1 - 1, y1 - 1);
        for row in y0..y1 {
            cov[..n].fill(0);
            let mut pen = x;
            for c in Latin1(s) {
                let g = glyph4(f, c);
                let gy = row - (baseline + g.y as i32);
                if gy >= 0 && gy < g.h as i32 {
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
            for i in 0..n {
                line[i] = lut[cov[i] as usize];
            }
            self.push_buf(&line[..n]);
        }
        w
    }

    /// Left draws from x, Right ends at x, Center centres the visible ink on x.
    fn text4_aligned(&mut self, x: i32, baseline: i32, align: Align, s: &[u8], f: &Font4, fg: u16, bg: u16) -> i32 {
        let w = text4_width(s, f);
        let x = match align {
            Align::Left => x,
            Align::Right => x - w,
            Align::Center => {
                let (mut pen, mut ink) = (0, None::<(i32, i32)>);
                for c in Latin1(s) {
                    let g = glyph4(f, c);
                    if g.w != 0 {
                        let (l, r) = (pen + g.x as i32, pen + g.x as i32 + g.w as i32);
                        ink = Some(ink.map_or((l, r), |(a, b)| (a.min(l), b.max(r))));
                    }
                    pen += g.adv as i32;
                }
                x - ink.map_or(w / 2, |(l, r)| (l + r) / 2)
            }
        };
        self.text4(x, baseline, s, f, fg, bg)
    }

    /// m1 in c1 over bg, then optionally m2 in c2 over that (the logo is a ring mask plus a windings mask).
    fn mask4(&mut self, x: i32, y: i32, m1: &Mask4, c1: u16, m2: Option<(&Mask4, u16)>, bg: u16) {
        let (w, h) = (m1.w as i32, m1.h as i32);
        let (x0, x1) = (x.max(0), (x + w).min(W));
        let (y0, y1) = (y.max(0), (y + h).min(H));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let mut lut = [0u16; 16];
        for (a, v) in lut.iter_mut().enumerate() {
            *v = blend(bg, c1, a as u8);
        }
        let mut line = [0u16; W as usize];
        self.window(x0, y0, x1 - 1, y1 - 1);
        for row in y0..y1 {
            for col in x0..x1 {
                let k = ((row - y) * w + (col - x)) as usize;
                let mut px = lut[nib(m1.bits, k) as usize];
                if let Some((m2, c2)) = m2 {
                    px = blend(px, c2, nib(m2.bits, k));
                }
                line[(col - x0) as usize] = px;
            }
            self.push_buf(&line[..(x1 - x0) as usize]);
        }
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

    /// Naive reference: per pixel, strongest ink of any glyph covering it, through blend.
    fn ref_pixel(px: i32, py: i32, x: i32, base: i32, codes: &[u32], f: &Font4, fg: u16, bg: u16) -> u16 {
        let (mut best, mut pen) = (0, x);
        for &c in codes {
            let g = &f.glyphs[(c - f.first as u32) as usize];
            let (gx, gy) = (px - (pen + g.x as i32), py - (base + g.y as i32));
            if gx >= 0 && gx < g.w as i32 && gy >= 0 && gy < g.h as i32 {
                best = best.max(nib(&f.bits[g.off as usize..], (gy * g.w as i32 + gx) as usize));
            }
            pen += g.adv as i32;
        }
        blend(bg, fg, best)
    }

    #[test]
    fn text_matches_reference() {
        let codes = [b'C' as u32, b'h' as u32, b' ' as u32, b'0' as u32, b'1' as u32, b'2' as u32, 0xB7, b'g' as u32, b'y' as u32];
        let s = "Ch 012\u{B7}gy".as_bytes(); // UTF-8 middle dot must draw the Latin-1 glyph
        let mut fb = FrameBuffer::new();
        fb.px.fill(0x5555);
        let (x, base) = (20, 100);
        let w = fb.text4(x, base, s, &UI_15, TEXT, SURFACE);
        let want_w: i32 = codes.iter().map(|&c| UI_15.glyphs[(c - 32) as usize].adv as i32).sum();
        assert_eq!(w, want_w);
        assert_eq!(text4_width(s, &UI_15), want_w);
        let top = base - UI_15.ascent as i32;
        let (mut bad, mut ink) = (0, 0);
        for py in top..top + UI_15.line_h as i32 {
            for px in x..x + w {
                let r = ref_pixel(px, py, x, base, &codes, &UI_15, TEXT, SURFACE);
                bad += (fb.at(px, py) != r) as u32;
                ink += (r != SURFACE) as u32;
            }
        }
        assert_eq!(bad, 0, "pixels differ from the reference");
        assert!(ink > 50, "too little ink ({ink} px): glyphs missing?");
        assert!(fb.at(x, top - 1) == 0x5555 && fb.at(x, top + UI_15.line_h as i32) == 0x5555 && fb.at(x + w, base) == 0x5555,
                "painted outside the line box");
    }

    #[test]
    fn unknown_chars_and_clipping() {
        let mut fb = FrameBuffer::new();
        assert_eq!(text4_width("\u{20AC}".as_bytes(), &UI_12), text4_width(b"?", &UI_12)); // euro sign: as '?'
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
        fb.mask4(10, 10, &LOGO40_RING, SUB, Some((&LOGO40_WIND, BLUE)), BG);
        for y in 0..40 {
            for x in 0..40 {
                let k = (y * 40 + x) as usize;
                let want = blend(blend(BG, SUB, nib(LOGO40_RING.bits, k)), BLUE, nib(LOGO40_WIND.bits, k));
                assert_eq!(fb.at(10 + x, 10 + y), want);
            }
        }
        fb.mask4(W - 10, H - 10, &LOGO112_RING, SUB, None, BG); // mostly off screen: clipped
    }
}
