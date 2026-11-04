//! BOOT-18 (decision 321): the boot animation's pixels, drawn by the radio from the logo's two masks and the
//! wordmark's font, no stored frames. The timing and colours are worked out once a frame in [`crate::boot`]; this is
//! the per-pixel part, in the fast flash (memory.x `.fast_ui`).
//!
//! The 14 windings are one mask: each pixel's winding is found from its angle round the logo's centre. They sit
//! 7-10° apart with clear gaps, so the angle is taken once per run of winding pixels along a row (a few per row,
//! an integer division each), not per pixel.
use crate::font4::Font4;
use crate::logo::{Rows, RING, WIND};
use crate::gfx::{blend, text_row, RleAt};
use crate::panel::{Panel, W};

/// The logo's masks are this square (112 px); its centre is the middle.
pub const SIZE: i32 = 112;
/// The windings, bottom left round the top to bottom right.
pub const WINDINGS: usize = 14;

/// The borders between the windings, as [`from_down`] values: winding k lies between `BOUNDS[k - 1]` and
/// `BOUNDS[k]`. The midpoints between the windings' angles in the logo's source (brand/ferrite-logo.svg); the
/// test checks each pixel against the nearest winding by its true angle.
static BOUNDS: [u16; WINDINGS - 1] = [152, 206, 284, 344, 392, 442, 512, 582, 632, 680, 740, 818, 872];

/// The halo's strength round the logo (0-255) by r²/16 from the centre, r in px: rising from 12 to 22 px (inside
/// the ring), full to 40 (the ring and the windings' inner half), then falling to nothing at the masks' edge (56):
/// the halo never shows the square's edge.
static GLOW: [u8; 197] = glow_table();

const fn glow_table() -> [u8; 197] {
    let mut t = [0u8; 197];
    let mut k = 0;
    while k < t.len() {
        let r16 = ((k as u32) * 16 * 256).isqrt(); // r in 1/16 px
        t[k] = if r16 < 12 * 16 {
            0
        } else if r16 < 22 * 16 {
            ((r16 - 12 * 16) * 255 / (10 * 16)) as u8
        } else if r16 <= 40 * 16 {
            255
        } else if r16 < 56 * 16 {
            let d = 56 * 16 - r16; // 0 at the edge, 256 at 40 px
            (d * d * 255 / (256 * 256)) as u8
        } else {
            0
        };
        k += 1;
    }
    t
}

/// The fixed-point angle round the centre (0-1023, one turn), clockwise on the screen from straight down: the
/// "diamond angle", monotonic in the true angle, one division. `dx`, `dy` in half pixels (odd: never 0, 0).
fn from_down(dx: i32, dy: i32) -> i32 {
    let p = if dy >= 0 && dx > 0 {
        256 * dy / (dx + dy)
    } else if dy > 0 {
        256 + 256 * -dx / (dy - dx)
    } else if dx < 0 {
        512 + 256 * -dy / (-dx - dy)
    } else {
        768 + 256 * dx / (dx - dy)
    };
    (p + 1024 - 256) & 1023
}

/// The winding a pixel `dx`, `dy` half pixels from the centre belongs to.
pub fn winding(dx: i32, dy: i32) -> usize {
    let q = from_down(dx, dy);
    BOUNDS.iter().take_while(|&&b| (b as i32) < q).count()
}

/// One frame's logo: the ground, the ring's colour (its alpha already in it), each winding's colour and alpha
/// (0-15), the halo's colours by level and its strength (0-15; 0 none).
pub struct Logo {
    pub bg: u16,
    pub ring: u16,
    pub wind: [(u16, u8); WINDINGS],
    pub halo: [u16; 16],
    pub peak: u8,
}

/// Draws the logo at (x, y) as `l` has it: every pixel of the 112 px square once, a row at a time. With the
/// still's colours (no halo, full alphas) the pixels are [`crate::gfx::Gfx::mask4`]'s.
#[inline(never)]
pub fn logo(lcd: &mut dyn Panel, x: i32, y: i32, l: &Logo) {
    lcd.window(x, y, x + SIZE - 1, y + SIZE - 1);
    lcd.mask_hint(x, y, x + SIZE - 1, y + SIZE - 1, l.ring);
    if lcd.text_only() {
        return;
    }
    // the masks streamed from the logo part (UPD-23): about 2.5 KB read a drawing
    let (mut r1, mut r2) = (Rows::new(RING), Rows::new(WIND));
    let (mut a1, mut a2) = ([0u8; SIZE as usize], [0u8; SIZE as usize]);
    let mut line = [0u16; SIZE as usize];
    for row in 0..SIZE {
        r1.next_row(&mut a1);
        r2.next_row(&mut a2);
        let dy = 2 * row + 1 - SIZE;
        let mut run = None; // the winding of the run of winding pixels under way
        for (col, px) in line.iter_mut().enumerate() {
            let dx = 2 * col as i32 + 1 - SIZE;
            let mut c = l.bg;
            if l.peak != 0 {
                if let Some(&g) = GLOW.get(((dx * dx + dy * dy) >> 6) as usize) {
                    c = l.halo[((g as u32 * l.peak as u32 + 128) >> 8) as usize];
                }
            }
            if a1[col] != 0 {
                c = blend(c, l.ring, a1[col]);
            }
            let a = a2[col];
            if a == 0 {
                run = None;
            } else {
                let k = *run.get_or_insert_with(|| winding(dx, dy));
                let (wc, wa) = l.wind[k];
                // a x wa / 15, exact at wa = 15
                c = blend(c, wc, ((a as u32 * wa as u32 * 17 + 128) >> 8) as u8);
            }
            *px = c;
        }
        lcd.push_buf(&line);
    }
}

/// One frame's wordmark: the text's colour and alpha (0-15) over the ground, and its halo: colours by level, the
/// strength (0-15; 0 none), the core box it is full over (x0, x1, y0, y1) and how far past it it fades (px).
pub struct Name {
    pub bg: u16,
    pub fg: u16,
    pub alpha: u8,
    pub halo: [u16; 16],
    pub peak: u8,
    pub core: (i32, i32, i32, i32),
    pub reach: i32,
}

/// Coverage as it is: [`text_row`] gives each pixel's ink 0-15.
static IDENT: [u16; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Draws the wordmark `s` (pen at `x`, on `baseline`) into the box x0..x1, y0..y1 as `n` has it: every pixel of
/// the box once, a row at a time. With no halo and full alpha the text's pixels are
/// [`crate::gfx::Gfx::text4`]'s and the rest is the ground.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn name(lcd: &mut dyn Panel, s: &[u8], f: &'static Font4, x: i32, baseline: i32, (x0, x1, y0, y1): (i32, i32, i32, i32), n: &Name) {
    let (x0, x1) = (x0.max(0), x1.min(W));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    lcd.window(x0, y0, x1 - 1, y1 - 1);
    lcd.text_style(f, n.fg, n.bg);
    lcd.text_hint(x0, y0, x1 - 1, y1 - 1, s);
    if lcd.text_only() {
        return;
    }
    let w = (x1 - x0) as usize;
    let (cx0, cx1, cy0, cy1) = n.core;
    let r2 = n.reach * n.reach;
    let mut at = RleAt::default();
    let mut line = [0u16; W as usize];
    for row in y0..y1 {
        text_row(&mut line[..w], &IDENT, s, f, x, x0, baseline, row, &mut at);
        let ddy = (cy0 - row).max(row - cy1).max(0);
        for (i, px) in line[..w].iter_mut().enumerate() {
            let mut c = n.bg;
            if n.peak != 0 {
                let col = x0 + i as i32;
                let ddx = (cx0 - col).max(col - cx1).max(0);
                let d2 = ddx * ddx + ddy * ddy;
                if d2 < r2 {
                    let k = (r2 - d2) * 255 / r2; // squared: soft at the edge
                    let g = (k * k / 255) as u32;
                    c = n.halo[((g * n.peak as u32 + 128) >> 8) as usize];
                }
            }
            let a = *px as u32;
            *px = if a == 0 { c } else { blend(c, n.fg, ((a * n.alpha as u32 * 17 + 128) >> 8) as u8) };
        }
        lcd.push_buf(&line[..w]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::LOGO112_WIND_RAW;
    extern crate std;
    use std::vec::Vec;

    /// BOOT-18.1: every pixel of the windings' mask is given the winding nearest it by its true angle, and by its
    /// run (the per-row shortcut the drawing takes) the same; each winding has its share of the pixels.
    #[test]
    fn each_winding_pixel_finds_its_winding() {
        // The windings' inner ends in the logo's source (120 px box, centre 60, 60).
        let inner: [(f64, f64); WINDINGS] = [
            (43.0, 77.0), (38.1, 69.8), (36.0, 61.4), (37.1, 52.9), (41.1, 45.2), (47.6, 39.5), (55.7, 36.4),
            (64.3, 36.4), (72.4, 39.5), (78.9, 45.2), (82.9, 52.9), (84.0, 61.4), (81.9, 69.8), (77.0, 77.0),
        ];
        let turn = |dx: f64, dy: f64| (dy.atan2(dx).to_degrees() - 90.0).rem_euclid(360.0);
        let ang: Vec<f64> = inner.iter().map(|&(x, y)| turn(x - 60.0, y - 60.0)).collect();
        let mut count = [0usize; WINDINGS];
        for row in 0..SIZE {
            let mut run = None;
            for col in 0..SIZE {
                let i = (row * SIZE + col) as usize;
                let a = if i.is_multiple_of(2) { LOGO112_WIND_RAW[i / 2] >> 4 } else { LOGO112_WIND_RAW[i / 2] & 15 };
                let (dx, dy) = (2 * col + 1 - SIZE, 2 * row + 1 - SIZE);
                if a == 0 {
                    run = None;
                    continue;
                }
                let t = turn(dx as f64, dy as f64);
                let near = (0..WINDINGS).min_by(|&p, &q| (ang[p] - t).abs().total_cmp(&(ang[q] - t).abs())).unwrap();
                assert_eq!(winding(dx, dy), near, "({col}, {row})");
                assert_eq!(*run.get_or_insert(near), near, "({col}, {row}): a run over two windings");
                count[near] += 1;
            }
        }
        assert!(count.iter().all(|&n| n > 100), "{count:?}");
    }

    /// The halo is 0 at the masks' edge and beyond, full on the ring.
    #[test]
    fn the_halo_stays_inside_the_square() {
        assert_eq!(GLOW[(56 * 56) / 16], 0);
        assert_eq!(GLOW[(30 * 30) / 16], 255);
        assert_eq!(GLOW[(5 * 5) / 16], 0);
        assert!(GLOW[(48 * 48) / 16] > 0 && GLOW[(48 * 48) / 16] < 255);
    }
}
