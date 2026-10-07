//! Drawing pieces the radio-activity screens share and `ferrite_ui::widgets` doesn't have yet: a progress bar,
//! a short segmented meter, wrapped text, a dialog box, a framed card, a tick and an
//! arrow (marks are drawn shapes: the fonts are cut to text). Candidates for the display library; each paints
//! its whole box, so it can be redrawn alone.

use ferrite_ui::assets::*;
use ferrite_ui::font4::Font4;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::widgets as w;

/// A rounded bar filled `num / den` in `fg` over LINE.
pub fn bar(lcd: &mut impl Panel, x: i32, y: i32, wd: i32, h: i32, num: u32, den: u32, fg: u16, bg: u16) {
    let f = (wd as u32 * num.min(den)).checked_div(den).unwrap_or(0) as i32;
    lcd.fill(x, y, wd, h, bg);
    lcd.fill_r(x, y, wd, h, h / 2, LINE);
    if f > 0 {
        lcd.fill_r(x, y, f.max(h), h, h / 2, fg);
    }
}

/// `n` segments `sw` wide on a `pitch`, `h` tall, the first `lit` in `on`.
pub fn segs(lcd: &mut impl Panel, x: i32, y: i32, n: u8, lit: u8, sw: i32, pitch: i32, h: i32, on: u16, bg: u16) {
    lcd.fill(x, y, n as i32 * pitch, h, bg);
    for i in 0..n {
        lcd.fill_r(x + i as i32 * pitch, y, sw, h, 1, if i < lit { on } else { LINE });
    }
}

/// Text wrapped on words into lines of `wd` pixels from baseline `y`, `lh` apart, at most `max` lines; each
/// line's box painted. Returns the lines used.
pub fn wrap(lcd: &mut impl Panel, x: i32, wd: i32, y: i32, lh: i32, max: i32, s: &[u8], f: &Font4, fg: u16, bg: u16) -> i32 {
    let (mut rest, mut n) = (s, 0);
    while !rest.is_empty() && n < max {
        let mut cut = rest.len();
        if text4_width(rest, f) > wd {
            cut = 0;
            for (i, &c) in rest.iter().enumerate() {
                if c == b' ' && text4_width(&rest[..i], f) <= wd {
                    cut = i;
                }
            }
            if cut == 0 {
                cut = w::fit(rest, f, wd).len().max(1);
            }
        }
        let base = y + n * lh;
        w::field(lcd, x, wd, base - lh + 4, base + 4, base, Align::Left, &rest[..cut], f, fg, bg);
        rest = rest[cut..].strip_prefix(b" ").unwrap_or(&rest[cut..]);
        n += 1;
    }
    n
}

/// How many lines `wrap` makes of s (at most `max`).
pub fn lines(s: &[u8], f: &Font4, wd: i32, max: i32) -> i32 {
    let (mut rest, mut n) = (s, 0);
    while !rest.is_empty() && n < max {
        let mut cut = rest.len();
        if text4_width(rest, f) > wd {
            cut = 0;
            for (i, &c) in rest.iter().enumerate() {
                if c == b' ' && text4_width(&rest[..i], f) <= wd {
                    cut = i;
                }
            }
            if cut == 0 {
                cut = w::fit(rest, f, wd).len().max(1);
            }
        }
        rest = rest[cut..].strip_prefix(b" ").unwrap_or(&rest[cut..]);
        n += 1;
    }
    n
}

/// A card: SURFACE, rounded, a 1 px border.
pub fn card(lcd: &mut impl Panel, x: i32, y: i32, wd: i32, h: i32, border: u16) {
    lcd.fill(x, y, wd, h, BG);
    lcd.fill_r(x, y, wd, h, 3, SURFACE);
    lcd.frame_r(x, y, wd, h, 3, border);
}

/// A dialog: a card with a coloured title band ("Can't transmit"). The body is the caller's, on SURFACE.
pub fn dialog(lcd: &mut impl Panel, y: i32, h: i32, title: &[u8], c: u16) {
    card(lcd, 10, y, 220, h, c);
    lcd.fill_r(11, y + 1, 218, 24, 2, c);
    lcd.fill(11, y + 14, 218, 11, c);
    lcd.text4_rows(19, y + 18, title, &UI_12B, WHITE, c, y + 2, y + 25);
}

/// A key chip like the hint bar's ("MENU 0 8 9"), blue outlined; returns its width.
pub fn path_chip(lcd: &mut impl Panel, x: i32, y: i32, s: &[u8], bg: u16) -> i32 {
    // 17 px tall like the chips, so the 9 px capitals centre (DISP-19)
    let wd = text4_width(s, &UI_12B) + 12;
    lcd.fill(x, y + 17, wd, 1, bg);
    w::field(lcd, x, wd, y, y + 17, y + 13, Align::Center, s, &UI_12B, BLUE, bg);
    lcd.frame_r(x, y, wd, 17, 1, BLUE);
    wd
}

/// A tick: 9 x 7.
pub fn tick(lcd: &mut impl Panel, x: i32, y: i32, c: u16) {
    lcd.line(x, y + 3, x + 3, y + 6, c);
    lcd.line(x + 3, y + 6, x + 8, y, c);
    lcd.line(x + 1, y + 3, x + 3, y + 5, c);
}

/// A right arrow (`dir` 3) or left arrow (2) `len` long, 2 px thick, centred on y.
pub fn arrow(lcd: &mut impl Panel, x: i32, y: i32, len: i32, dir: u8, c: u16) {
    if dir == 3 {
        lcd.fill(x, y - 1, len - 5, 2, c);
        lcd.tri(x + len - 9, y - 5, 10, 3, c);
    } else {
        lcd.fill(x + 5, y - 1, len - 5, 2, c);
        lcd.tri(x, y - 5, 10, 2, c);
    }
}

/// A two-headed arrow (crossband's sides).
pub fn arrow2(lcd: &mut impl Panel, x: i32, y: i32, len: i32, c: u16) {
    lcd.fill(x + 4, y - 1, len - 8, 2, c);
    lcd.tri(x, y - 4, 8, 2, c);
    lcd.tri(x + len - 8, y - 4, 8, 3, c);
}

/// A filled dot (status lights).
pub fn dot(lcd: &mut impl Panel, x: i32, y: i32, d: i32, c: u16, bg: u16) {
    lcd.fill(x, y, d, d, bg);
    lcd.fill_r(x, y, d, d, d / 2, c);
}

/// Level 0-255 (dBm + 160) as a bar height out of `h`, from -130 dBm (empty) to -40 dBm (full).
pub fn level_h(v: u8, h: i32) -> i32 {
    if v == 0 {
        return 0;
    }
    let dbm = v as i32 - 160;
    ((dbm + 130).clamp(1, 90) * h / 90).max(1)
}
