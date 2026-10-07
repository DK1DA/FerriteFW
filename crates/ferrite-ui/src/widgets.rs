//! The pieces the look review's screens are made of (`docs/img/screens/*.png`): status bar, slot letter, tags,
//! chips, frequency, segmented S-meter, list rows with number, name, value and default dot, help box, hint bar,
//! edit cell with the knob mark, toast. Each one paints its whole box, background included, so any one of them
//! can be redrawn alone (HOME-7). Numbers are pixels on the 240 x 320 portrait screen.

use crate::assets::*;
use crate::font4::Font4;
use crate::gfx::*;
use crate::panel::Panel;
use crate::text::Buf;
use ferrite_core::freq::{self, Freq10};
use ferrite_core::msg::GpsState;

/// Status bar rows 0-19 (the line is row 19), content 20-299, hint bar 300-319 (HOME-1).
pub const SBAR_H: i32 = 20;
pub const HINT_Y: i32 = 300;

/// Hint-bar arrows: these bytes in a key name draw as triangles.
pub const UP: &str = "\x18";
pub const DOWN: &str = "\x19";
pub const RIGHT: &str = "\x1a";
pub const LEFT: &str = "\x1b";

pub const LETTERS: [u8; 3] = *b"ABC";

pub fn slot_colour(slot: u8) -> u16 {
    [BLUE, AMBER, VIOLET][slot as usize % 3]
}

/// The longest start of s (whole UTF-8 characters) that fits in w pixels.
pub fn fit<'a>(s: &'a [u8], f: &Font4, w: i32) -> &'a [u8] {
    if text4_width(s, f) <= w {
        return s;
    }
    let mut end = 0;
    for i in 1..=s.len() {
        if i == s.len() || s[i] & 0xC0 != 0x80 {
            if text4_width(&s[..i], f) > w {
                break;
            }
            end = i;
        }
    }
    &s[..end]
}

/// Text in the box x..x+w, y0..y1: aligned, cut to fit, the rest of the box painted bg. The workhorse for
/// redrawing a value in place without clearing it first (no flicker).
pub fn field(
    lcd: &mut impl Panel,
    x: i32,
    w: i32,
    y0: i32,
    y1: i32,
    baseline: i32,
    align: Align,
    s: &[u8],
    f: &Font4,
    fg: u16,
    bg: u16,
) -> i32 {
    let s = fit(s, f, w);
    let tw = text4_width(s, f);
    let tx = match align {
        Align::Left => x,
        Align::Right => x + w - tw,
        Align::Center => x + (w - tw) / 2,
    };
    lcd.fill(x, y0, tx - x, y1 - y0, bg);
    lcd.text4_rows(tx, baseline, s, f, fg, bg, y0, y1);
    // the box's rows above and below the font's line box
    let top = baseline - f.ascent as i32;
    lcd.fill(tx, y0, tw, top - y0, bg);
    lcd.fill(tx, top + f.line_h as i32, tw, y1 - top - f.line_h as i32, bg);
    lcd.fill(tx + tw, y0, x + w - tx - tw, y1 - y0, bg);
    tw
}

/// The 14 x 14 slot letter: filled for the active slot, outlined for the others.
pub fn badge(lcd: &mut impl Panel, x: i32, y: i32, slot: u8, filled: bool, bg: u16) {
    let c = slot_colour(slot);
    let (fg, back) = if filled { (BLUE_FG, c) } else { (c, bg) };
    // 13 x 13: the 9 px letter sits 1 px from the top and bottom inside, 3 px from the sides (an even box
    // can't centre an odd letter). The 14 px clear wipes a box drawn at the old size.
    lcd.fill(x, y, 14, 14, bg);
    lcd.fill_r(x, y, 13, 13, 1, back);
    lcd.text4_rows(x + 4, y + 11, &[LETTERS[slot as usize % 3]], &UI_12B, fg, back, y + 1, y + 12);
    if !filled {
        lcd.frame_r(x, y, 13, 13, 1, c);
    }
}

/// An outlined tag ("RX", "EDIT") ending at x_right; returns its width.
pub fn tag(lcd: &mut impl Panel, x_right: i32, y: i32, s: &[u8], c: u16, bg: u16) -> i32 {
    let w = text4_width(s, &UI_12B) + 10;
    let x = x_right - w;
    field(lcd, x, w, y, y + 16, y + 12, Align::Center, s, &UI_12B, c, bg);
    lcd.frame_r(x, y, w, 16, 1, c);
    w
}

/// An outlined chip ("FM-N", "T 100.0"); returns its width.
pub fn chip(lcd: &mut impl Panel, x: i32, y: i32, s: &[u8], fg: u16, bg: u16) -> i32 {
    let w = text4_width(s, &UI_12) + 10;
    field(lcd, x, w, y, y + 17, y + 13, Align::Center, s, &UI_12, fg, bg);
    lcd.frame_r(x, y, w, 17, 1, LINE);
    w
}

/// The big card's chip row in fixed slots (HOME-4): mode, tone, shift and power each keep their own x and width, so
/// a value coming or going never moves its neighbours; power is pinned to the right edge. Widths fit the widest
/// text each can show in UI_12 (7 px a character, 4 px padding each side): "FM-N", "T 254.1", "R-12.5M" (a
/// reversed shift) and "0.25 W". x..x+w, all inside 15..226.
pub const SLOT_MODE: (i32, i32) = (15, 36);
pub const SLOT_TONE: (i32, i32) = (54, 57);
pub const SLOT_SHIFT: (i32, i32) = (114, 57);
pub const SLOT_POWER: (i32, i32) = (176, 50);

/// A chip in a fixed slot (x, w) at y..y+17: the slot is cleared first, then a chip of the slot's full width when
/// `s` isn't empty, so a shorter value never leaves pixels behind.
pub fn chip_slot(lcd: &mut impl Panel, slot: (i32, i32), y: i32, s: &[u8], fg: u16, frame: u16, bg: u16) {
    let (x, w) = slot;
    lcd.fill(x, y, w, 17, bg);
    if !s.is_empty() {
        field(lcd, x + 1, w - 2, y + 1, y + 16, y + 13, Align::Center, fit(s, &UI_12, w - 4), &UI_12, fg, bg);
        lcd.frame_r(x, y, w, 17, 1, frame);
    }
}

/// A row of chips in the box x..x+w, y..y+17.
pub fn chips(lcd: &mut impl Panel, x: i32, y: i32, w: i32, items: &[&[u8]], bg: u16) {
    chips_sel(lcd, x, y, w, items, None, bg)
}

/// [`chips`] with one of them selected (accent text and outline): a row of choices such as MENU's Recent.
pub fn chips_sel(lcd: &mut impl Panel, x: i32, y: i32, w: i32, items: &[&[u8]], sel: Option<usize>, bg: u16) {
    let mut cx = x;
    for (i, s) in items.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
        if cx + text4_width(s, &UI_12) + 10 > x + w {
            break;
        }
        let cw = chip(lcd, cx, y, s, if sel == Some(i) { BLUE } else { TEXT }, bg);
        if sel == Some(i) {
            lcd.frame_r(cx, y, cw, 17, 1, BLUE);
        }
        cx += cw + 4;
    }
    lcd.fill(cx, y, x + w - cx, 17, bg);
}

/// "146.520" in `big` and "00" in `small` on one baseline, painting only the digits' rows (the big font's line
/// box would reach into its neighbours) and clearing to x_end. Returns where the text ends.
pub fn freq(
    lcd: &mut impl Panel,
    x: i32,
    x_end: i32,
    baseline: i32,
    f: Freq10,
    big: &Font4,
    small: &Font4,
    fg: u16,
    fg_small: u16,
    bg: u16,
) -> i32 {
    let mut b = [0u8; 16];
    let n = freq::format(f, &mut b);
    let split = n.saturating_sub(3);
    let h = digit_h(big);
    let (y0, y1) = (baseline - h, baseline + 1);
    let w = lcd.text4_rows(x, baseline, &b[..split], big, fg, bg, y0, y1);
    let sx = x + w + 4;
    lcd.fill(x + w, y0, 4, y1 - y0, bg);
    let hs = digit_h(small);
    lcd.fill(sx, y0, text4_width(&b[split + 1..n], small), h - hs, bg);
    let ws = lcd.text4_rows(sx, baseline, &b[split + 1..n], small, fg_small, bg, baseline - hs, y1);
    lcd.fill(sx + ws, y0, x_end - sx - ws, y1 - y0, bg);
    sx + ws
}

/// Height of a digit above the baseline.
pub fn digit_h(f: &Font4) -> i32 {
    f.glyph('0' as u32).map_or(f.ascent as i32, |g| -(g.y as i32))
}

/// "146.520" (three decimals), for compact lines.
pub fn freq3(f: Freq10) -> Buf<12> {
    let mut b = [0u8; 16];
    let n = freq::format(f, &mut b);
    Buf::from(&b[..n.saturating_sub(3)])
}

/// The S-meter: 20 segments 8 px wide on a 10 px pitch, 8 px tall. Only segments between the old and new
/// level are drawn (None: all of them).
pub const METER_SEGS: u8 = 20;
pub fn meter(lcd: &mut impl Panel, x: i32, y: i32, old: Option<u8>, lit: u8, on: u16, bg: u16) {
    let (a, b) = match old {
        Some(o) => (o.min(lit), o.max(lit)),
        None => (0, METER_SEGS),
    };
    if old.is_none() {
        lcd.fill(x, y, METER_SEGS as i32 * 10, 8, bg);
    }
    for i in a..b.min(METER_SEGS) {
        lcd.fill_r(x + i as i32 * 10, y, 8, 8, 1, if i < lit { on } else { LINE });
    }
}

/// dBm to lit segments: S0 (-127 dBm) to S9 (-73 dBm) over the first 17, then 20 dB per segment to S9 + 60.
pub fn meter_level(dbm: i16) -> u8 {
    let d = dbm as i32;
    if d <= -127 {
        0
    } else if d <= -73 {
        ((d + 127) * 17 / 54) as u8
    } else {
        (17 + (d + 73) / 20).min(METER_SEGS as i32) as u8
    }
}

/// "S7", "S9", "+20".
pub fn s_text(dbm: i16) -> Buf<4> {
    let mut b = Buf::new();
    let d = dbm as i32;
    if d < -63 {
        let s = ((d + 127).max(0) / 6).min(9);
        let _ = ufmt::uwrite!(b, "S{}", s);
    } else {
        let _ = ufmt::uwrite!(b, "+{}", ((d + 73) / 10 * 10).min(60));
    }
    b
}

/// One line of text with keys in bold: `key` (arrows as triangles), then `label`. Returns the width.
fn key_label(lcd: &mut impl Panel, x: i32, baseline: i32, key: &str, label: &str, bg: u16, draw: bool) -> i32 {
    let (y0, y1) = (baseline - 13, baseline + 4);
    let mut px = x;
    for part in key.as_bytes().split_inclusive(|&c| (0x18..=0x1b).contains(&c)) {
        let (text, arrow) = match part.last() {
            Some(&c) if (0x18..=0x1b).contains(&c) => (&part[..part.len() - 1], Some(c)),
            _ => (part, None),
        };
        if draw {
            px += lcd.text4_rows(px, baseline, text, &UI_12B, TEXT, bg, y0, y1);
        } else {
            px += text4_width(text, &UI_12B);
        }
        if let Some(c) = arrow {
            if draw {
                lcd.fill(px, y0, 7, y1 - y0, bg);
                let dir = [0, 1, 3, 2][(c - 0x18) as usize];
                lcd.tri(px, baseline - 7, 6, dir, TEXT);
            }
            px += 7;
        }
    }
    if !label.is_empty() {
        if draw {
            lcd.fill(px, y0, 4, y1 - y0, bg);
            px += 4;
            px += lcd.text4_rows(px, baseline, label.as_bytes(), &UI_12, SUB, bg, y0, y1);
        } else {
            px += 4 + text4_width(label.as_bytes(), &UI_12);
        }
    }
    px - x
}

/// A line like "Hold V/M save this VFO as a channel" at x, the rest of the row to x_end cleared.
pub fn key_line(lcd: &mut impl Panel, x: i32, x_end: i32, baseline: i32, key: &str, label: &str, bg: u16) {
    let w = key_label(lcd, x, baseline, key, label, bg, true);
    lcd.fill(x + w, baseline - 13, x_end - x - w, 17, bg);
}

/// The hint bar holds at most this many key + label pairs (KEY-9): more overflow 240 px in the 12 px font.
pub const HINT_MAX: usize = 3;

/// The hint bar's width for these pairs, with the smallest gap (8 px) between them; it fits when this is at
/// most 224 (8 px margins).
pub fn hint_width(lcd: &mut impl Panel, items: &[(&str, &str)]) -> i32 {
    let n = items.len().min(HINT_MAX);
    let w: i32 = items[..n].iter().map(|(k, l)| key_label(lcd, 0, 0, k, l, BG, false)).sum();
    w + 8 * (n as i32 - 1).max(0)
}

/// The hint bar (KEY-9): up to [`HINT_MAX`] key + label pairs, the first at the left margin, the last at the
/// right, the one between centred in the gap.
pub fn hint_bar(lcd: &mut impl Panel, items: &[(&str, &str)]) {
    let base = HINT_Y + 15; // text rows HINT_Y + 2 .. HINT_Y + 19
    lcd.fill(0, HINT_Y, 240, 1, LINE);
    lcd.fill(0, HINT_Y + 1, 240, 1, BG);
    lcd.fill(0, HINT_Y + 19, 240, 1, BG);
    let n = items.len().min(HINT_MAX);
    let mut w = [0i32; HINT_MAX];
    for i in 0..n {
        w[i] = key_label(lcd, 0, base, items[i].0, items[i].1, BG, false);
    }
    let total: i32 = w[..n].iter().sum();
    let gap = if n > 1 { (224 - total) / (n as i32 - 1) } else { 0 };
    let (mut x, mut end) = (8, 0);
    for i in 0..n {
        if i == n - 1 && n > 1 {
            x = 232 - w[i];
        }
        lcd.fill(end, HINT_Y + 2, x - end, 17, BG);
        key_label(lcd, x, base, items[i].0, items[i].1, BG, true);
        end = x + w[i];
        x = end + gap;
    }
    lcd.fill(end, HINT_Y + 2, 240 - end, 17, BG);
}

/// The envelope: 12 x 9.
pub fn envelope(lcd: &mut impl Panel, x: i32, y: i32, c: u16) {
    lcd.frame(x, y, 12, 9, c);
    lcd.line(x + 1, y + 1, x + 5, y + 5, c);
    lcd.line(x + 6, y + 5, x + 10, y + 1, c);
}

/// A bell, 11 x 11 (alert tone; with `slash`, alerts paused: SBAR-18).
pub fn bell(lcd: &mut impl Panel, x: i32, y: i32, c: u16, bg: u16, slash: bool) {
    lcd.fill(x, y, 11, 11, bg);
    lcd.fill_r(x + 2, y + 1, 7, 8, 3, c);
    lcd.fill(x + 1, y + 7, 9, 2, c);
    lcd.fill(x + 4, y + 9, 3, 2, c);
    lcd.fill_r(x + 4, y + 3, 3, 4, 1, bg);
    if slash {
        lcd.line(x, y, x + 10, y + 10, bg);
        lcd.line(x + 1, y, x + 10, y + 9, c);
    }
}

/// A warning triangle, 13 x 12, with its "!" in bg.
pub fn warn_icon(lcd: &mut impl Panel, x: i32, y: i32, c: u16, bg: u16) {
    for i in 0..12 {
        let n = i / 2;
        lcd.fill(x + 6 - n, y + i, 2 * n + 1, 1, c);
    }
    lcd.fill(x + 6, y + 4, 1, 4, bg);
    lcd.fill(x + 6, y + 9, 1, 2, bg);
}

/// A progress bar: `pct` of w filled in `c`, the rest LINE.
pub fn progress(lcd: &mut impl Panel, x: i32, y: i32, w: i32, h: i32, pct: u8, c: u16) {
    let f = w * pct.min(100) as i32 / 100;
    lcd.fill(x, y, f, h, c);
    lcd.fill(x + f, y, w - f, h, LINE);
}

/// Text wrapped on words into lines `step` apart from `baseline`, in the box x..x+w, rows y0..y1; the rest of
/// the box painted bg. Returns the lines used.
pub fn wrap(lcd: &mut impl Panel, x: i32, w: i32, y0: i32, y1: i32, baseline: i32, step: i32, s: &[u8], f: &Font4, fg: u16, bg: u16) -> i32 {
    let (mut rest, mut base, mut n) = (s, baseline, 0);
    let top = baseline - f.ascent as i32;
    lcd.fill(x, y0, w, top - y0, bg);
    let mut y = top;
    while y < y1 {
        let mut cut = rest.len();
        if text4_width(rest, f) > w {
            cut = 0;
            for (i, &c) in rest.iter().enumerate() {
                if c == b' ' && text4_width(&rest[..i], f) <= w {
                    cut = i;
                }
            }
            if cut == 0 {
                cut = fit(rest, f, w).len();
            }
        }
        let line = &rest[..cut];
        let bottom = (base - f.ascent as i32 + step).min(y1);
        field(lcd, x, w, y, bottom, base, Align::Left, line, f, fg, bg);
        if !line.is_empty() {
            n += 1;
        }
        rest = rest[cut..].strip_prefix(b" ").unwrap_or(&rest[cut..]);
        y = bottom;
        base += step;
    }
    n
}

/// A toast in place of the hint bar (HOME-9): bold head, then body.
/// The largest box [`popup`] draws (x, y, w, h), for clearing it.
pub const POPUP_BOX: (i32, i32, i32, i32) = (24, 128, 192, 64);

/// A hold-digit shortcut's popup in the middle of the content area (HOME-8): what ran, and under it the new
/// value when it has one.
pub fn popup(lcd: &mut impl Panel, head: &[u8], body: &[u8]) {
    let (x, _, w, _) = POPUP_BOX;
    let (y, h) = if body.is_empty() { (138, 44) } else { (128, 64) };
    lcd.fill_r(x, y, w, h, 4, BLUE);
    lcd.fill_r(x + 1, y + 1, w - 2, h - 2, 3, TOAST);
    let hb = if body.is_empty() { y + 28 } else { y + 26 };
    lcd.text4_aligned(120, hb, Align::Center, fit(head, &UI_15B, w - 16), &UI_15B, TEXT, TOAST);
    if !body.is_empty() {
        lcd.text4_aligned(120, y + 50, Align::Center, fit(body, &UI_12, w - 16), &UI_12, TEXT, TOAST);
    }
}

pub fn toast(lcd: &mut impl Panel, head: &[u8], body: &[u8], icon: bool) {
    lcd.fill(0, HINT_Y, 240, 1, BLUE);
    lcd.fill(0, HINT_Y + 1, 240, 19, TOAST);
    let mut x = 8;
    if icon {
        envelope(lcd, x, HINT_Y + 6, BLUE);
        x += 18;
    }
    let base = HINT_Y + 15;
    x += lcd.text4_rows(x, base, fit(head, &UI_12B, 232 - x), &UI_12B, TEXT, TOAST, HINT_Y + 1, HINT_Y + 20);
    if !body.is_empty() && x < 225 {
        x += 7;
        lcd.text4_rows(x, base, fit(body, &UI_12, 232 - x), &UI_12, TEXT, TOAST, HINT_Y + 1, HINT_Y + 20);
    }
}

/// A menu row: number, name (with a dot when not default, MENU-12), value at the right. Selected rows are
/// filled blue; greyed rows (MENU-14) draw their name dim.
pub struct Row<'a> {
    pub num: Option<u8>,
    pub name: &'a [u8],
    pub value: &'a [u8],
    pub dot: bool,
    pub selected: bool,
    pub grey: bool,
}

pub fn list_row(lcd: &mut impl Panel, y: i32, h: i32, r: &Row) {
    let bg = if r.selected { BLUE } else { BG };
    let (fg, sub) = if r.selected { (BLUE_FG, BLUE_FG) } else { (if r.grey { DIM } else { TEXT }, SUB) };
    let base = y + h / 2 + 5;
    let x = if let Some(n) = r.num {
        field(lcd, 0, 25, y, y + h, base, Align::Center, &[b'0' + n % 10], &UI_12, sub, bg);
        25
    } else {
        lcd.fill(0, y, 8, h, bg);
        8
    };
    // The name keeps its room; the value gets the rest.
    let name = fit(r.name, &UI_12, 232 - x - 40);
    let nw = field(lcd, x, text4_width(name, &UI_12), y, y + h, base, Align::Left, name, &UI_12, fg, bg);
    let x = x + nw;
    let value = fit(r.value, &UI_12, 232 - x - if r.dot { 18 } else { 8 });
    let vw = text4_width(value, &UI_12);
    let vx = 232 - vw;
    lcd.fill(x, y, vx - x, h, bg);
    if r.dot {
        lcd.fill_r(x + 4, base - 7, 6, 6, 3, if r.selected { BLUE_FG } else { AMBER });
    }
    field(lcd, vx, vw, y, y + h, base, Align::Right, value, &UI_12, sub, bg);
    lcd.fill(232, y, 8, h, bg);
}

/// How a label + value row is laid out. One drawer ([`kv`]) for every "label: value" row: the Status page and
/// the system pages ([`KV_STATUS`]), detail pages ([`KV_DETAIL`]) and a pair on a card ([`KV_PAIR`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvStyle {
    /// Where the value starts; it runs to x = 232.
    pub vx: i16,
    /// The value against x = 232 instead of at vx.
    pub right: bool,
    /// The baseline below the row's top; 0 centres it in the row.
    pub base: i8,
    /// A LINE under the row, in its last pixel row.
    pub line: bool,
    /// A value too wide for one line goes on to a second (the row is then 36 px tall, else 19).
    pub wrap: bool,
}

/// The Status page: value at the right, a line under each row.
pub const KV_STATUS: KvStyle = KvStyle { vx: 108, right: true, base: 0, line: true, wrap: false };
/// A detail page (the AprsStation mock-up): value from x = 68, on two lines when it needs them.
pub const KV_DETAIL: KvStyle = KvStyle { vx: 68, right: false, base: 13, line: true, wrap: true };
/// A pair on a card ("Last alert   14:28"): no line, 17 px.
pub const KV_PAIR: KvStyle = KvStyle { vx: 90, right: false, base: 13, line: false, wrap: false };

/// A row's height: `h`, or for a wrapping style 19 px, 36 for a value on two lines.
pub fn kv_h(st: &KvStyle, value: &[u8], h: i32) -> i32 {
    if !st.wrap {
        h
    } else if text4_width(value, &UI_12) <= 232 - st.vx as i32 {
        19
    } else {
        36
    }
}

/// A label + value row from y, `h` tall ([`kv_h`]): the label in SUB at x = 8, the value in `vc`; all white on
/// blue when selected. Paints the whole row.
pub fn kv(lcd: &mut impl Panel, y: i32, h: i32, st: &KvStyle, label: &[u8], value: &[u8], vc: u16, selected: bool) {
    let bg = if selected { BLUE } else { BG };
    let (lc, vc) = if selected { (BLUE_FG, BLUE_FG) } else { (SUB, vc) };
    let (vx, vw) = (st.vx as i32, 232 - st.vx as i32);
    let y1 = y + h - st.line as i32;
    let base = y + if st.base == 0 { h / 2 + 5 } else { st.base as i32 };
    lcd.fill(0, y, 240, y1 - y, bg);
    lcd.text4_rows(8, base, label, &UI_12, lc, bg, y, y1);
    let mut v = fit(value, &UI_12, vw);
    if st.wrap && v.len() < value.len() {
        // the second line from the last space, comma or dash past half way
        let cut = match v.iter().rposition(|&ch| ch == b' ' || ch == b',' || ch == b'-') {
            Some(p) if p > v.len() / 2 => p + 1,
            _ => v.len(),
        };
        lcd.text4_rows(vx, base + 17, fit(value[cut..].trim_ascii_start(), &UI_12, vw), &UI_12, vc, bg, y, y1);
        v = &value[..cut];
    }
    let x = if st.right { 232 - text4_width(v, &UI_12) } else { vx };
    lcd.text4_rows(x, base, v, &UI_12, vc, bg, y, y1);
    if st.line {
        lcd.fill(0, y1, 240, 1, LINE);
    }
}

/// The help line at the foot of a menu (MENU-15): a rounded box, the text wrapped on words, then the path.
pub fn help_box(lcd: &mut impl Panel, y: i32, h: i32, text: &[u8], path: &[u8]) {
    lcd.fill(0, y, 240, h, BG);
    lcd.fill_r(8, y, 224, h, 2, SURFACE);
    let max = 208;
    let (mut line, mut rest) = (0, text);
    while !rest.is_empty() && line < 2 {
        // the longest run of whole words that fits
        let mut cut = rest.len();
        if text4_width(rest, &UI_12) > max {
            cut = 0;
            for (i, &c) in rest.iter().enumerate() {
                if c == b' ' && text4_width(&rest[..i], &UI_12) <= max {
                    cut = i;
                }
            }
            if cut == 0 {
                cut = fit(rest, &UI_12, max).len();
            }
        }
        let base = y + 13 + line * 15;
        let w = lcd.text4_rows(16, base, &rest[..cut], &UI_12, TEXT, SURFACE, y + 2, y + h - 2);
        rest = rest[cut..].strip_prefix(b" ").unwrap_or(&rest[cut..]);
        if rest.is_empty() && !path.is_empty() {
            let pw = text4_width(path, &UI_12);
            if 16 + w + 7 + pw <= 16 + max {
                lcd.text4_rows(16 + w + 7, base, path, &UI_12, SUB, SURFACE, y + 2, y + h - 2);
            } else if line == 0 {
                lcd.text4_rows(16, base + 15, path, &UI_12, SUB, SURFACE, y + 2, y + h - 2);
            }
        }
        line += 1;
    }
}

/// A setting in the edit grid: label above, value below; the focused one is blue with the knob mark.
pub fn edit_cell(lcd: &mut impl Panel, x: i32, y: i32, w: i32, h: i32, label: &[u8], value: &[u8], focused: bool, grey: bool) {
    let (bg, border, lc) = if focused { (FOCUS, BLUE, BLUE) } else { (BG, LINE, SUB) };
    lcd.fill(x, y, w, h, bg);
    let lw = lcd.text4_rows(x + 6, y + 12, label, &UI_12, lc, bg, y + 1, y + 15);
    if focused && lw + 14 < w - 6 {
        knob_mark(lcd, x + 6 + lw + 4, y + 4, BLUE, bg);
    }
    let v = fit(value, &UI_12, w - 10);
    lcd.text4_rows(x + 6, y + 26, v, &UI_12, if grey { DIM } else { TEXT }, bg, y + 15, y + h - 1);
    lcd.frame_r(x, y, w, h, 1, border);
}

/// The knob mark: a small ring, open at the top right, with a tick (the knob changes this).
pub fn knob_mark(lcd: &mut impl Panel, x: i32, y: i32, c: u16, bg: u16) {
    lcd.frame_r(x, y, 8, 8, 3, c);
    lcd.fill(x + 5, y, 3, 3, bg);
    lcd.fill(x + 4, y, 3, 1, c);
    lcd.fill(x + 6, y + 1, 1, 2, c);
}

/// What the status bar shows (manual chapter 8). Compared with what is on screen, so only changed parts redraw.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct StatusBar {
    /// The screen's own name; empty on home (SBAR-5).
    pub name: Buf<10>,
    pub locked: bool,
    pub scan: bool,
    /// Watched slots: 3 = TW, 2 = DW, else nothing (SBAR-16).
    pub watch: u8,
    pub inbox: u8,
    pub alert: bool,
    pub clock: Option<u16>,
    pub utc: bool,
    pub gps: GpsState,
    pub bt: bool,
    /// 0-4 bars.
    pub battery: u8,
    pub battery_low: bool,
    pub battery_cutoff: bool,
    /// The cut-off blink's off phase.
    pub blink_off: bool,
    /// Weather alerts paused (HF mode with SAME on): a slashed bell after the inbox (SBAR-18).
    pub alerts_paused: bool,
}

impl StatusBar {
    fn left_eq(&self, o: &Self) -> bool {
        (self.name, self.locked, self.scan, self.watch, self.inbox, self.alert, self.alerts_paused)
            == (o.name, o.locked, o.scan, o.watch, o.inbox, o.alert, o.alerts_paused)
    }

    fn right_eq(&self, o: &Self) -> bool {
        (self.gps, self.bt, self.battery, self.battery_low, self.battery_cutoff, self.blink_off)
            == (o.gps, o.bt, o.battery, o.battery_low, o.battery_cutoff, o.blink_off)
    }

    /// Draws what differs from `old` (everything with None).
    pub fn draw(&self, lcd: &mut impl Panel, old: Option<&StatusBar>) {
        if old.is_none() {
            lcd.fill(0, SBAR_H - 1, 240, 1, LINE);
        }
        if old.is_none_or(|o| !self.left_eq(o)) {
            self.draw_left(lcd);
        }
        if old.is_none_or(|o| (o.clock, o.utc) != (self.clock, self.utc)) {
            let mut b = Buf::<8>::new();
            let t = match self.clock {
                Some(m) => {
                    let _ = ufmt::uwrite!(b, "{}:{}", crate::text::Zero((m / 60) as u32, 2), crate::text::Zero((m % 60) as u32, 2));
                    if self.utc {
                        b.push(b"Z");
                    }
                    b.as_bytes()
                }
                None => b"--:--",
            };
            field(lcd, 100, 40, 0, SBAR_H - 1, 14, Align::Center, t, &UI_12B, TEXT, BG);
        }
        if old.is_none_or(|o| !self.right_eq(o)) {
            self.draw_right(lcd);
        }
    }

    fn draw_left(&self, lcd: &mut impl Panel) {
        let (y0, y1, base, gap, x_end) = (0, SBAR_H - 1, 14, 5, 100);
        let mut count = Buf::<4>::new();
        let _ = ufmt::uwrite!(count, "{}", self.inbox.min(99));
        // Icons first, in order (SBAR-3): the name gets what is left of the ~90 px (SBAR-5).
        let icons = if self.locked { 14 + gap } else { 0 }
            + if self.scan { 21 + gap } else { 0 }
            + if self.watch >= 2 { 14 + gap } else { 0 }
            + if self.inbox > 0 { 14 + text4_width(count.as_bytes(), &UI_12B) + gap } else { 0 }
            + if self.alerts_paused { 11 + gap } else { 0 };
        let mut x = 8;
        lcd.fill(0, y0, 8, y1, BG);
        if !self.name.is_empty() {
            let name = fit(self.name.as_bytes(), &UI_12, 92 - icons);
            x += field(lcd, x, text4_width(name, &UI_12), y0, y1, base, Align::Left, name, &UI_12, SUB, BG);
            lcd.fill(x, y0, gap, y1, BG);
            x += gap;
        }
        if self.locked {
            lcd.mask4(x, 2, &ICON_LOCK, SUB, None, BG);
            lcd.fill(x, 16, 14, 3, BG);
            lcd.fill(x + 14, y0, gap, y1, BG);
            x += 14 + gap;
        }
        for (on, t) in [(self.scan, b"SCN".as_slice()), (self.watch >= 2, if self.watch >= 3 { b"TW" } else { b"DW" })] {
            if on {
                let w = text4_width(t, &UI_12);
                field(lcd, x, w + gap, y0, y1, base, Align::Left, t, &UI_12, SUB, BG);
                x += w + gap;
            }
        }
        if self.inbox > 0 {
            let c = if self.alert { RED } else { SUB };
            lcd.fill(x, y0, 14, y1, BG);
            envelope(lcd, x, 5, c);
            x += 14;
            let w = text4_width(count.as_bytes(), &UI_12B);
            field(lcd, x, w, y0, y1, base, Align::Left, count.as_bytes(), &UI_12B, if self.alert { RED } else { TEXT }, BG);
            x += w;
        }
        if self.alerts_paused {
            lcd.fill(x, y0, gap, y1, BG);
            x += gap;
            lcd.fill(x, y0, 11, y1, BG);
            bell(lcd, x, 4, SUB, BG, true);
            x += 11;
        }
        lcd.fill(x, y0, x_end - x, y1, BG);
    }

    fn draw_right(&self, lcd: &mut impl Panel) {
        lcd.fill(140, 0, 100, SBAR_H - 1, BG);
        // Battery at the right edge (SBAR-9): body 17 x 10 and a nub.
        let c = if self.battery_cutoff { RED } else if self.battery_low { AMBER } else { SUB };
        let fillc = if self.battery_cutoff { RED } else if self.battery_low { AMBER } else { GREEN };
        if !(self.battery_cutoff && self.blink_off) {
            lcd.frame_r(212, 5, 17, 10, 1, c);
            lcd.fill(229, 8, 2, 4, c);
            let bars = if self.battery_cutoff { 1 } else { self.battery.clamp(1, 4) as i32 };
            lcd.fill(214, 7, bars * 13 / 4, 6, fillc);
        }
        let mut x = 212;
        if self.bt {
            x -= 16;
            lcd.mask4(x, 4, &ICON_BT, BLUE, None, BG);
        }
        if self.gps != GpsState::Off {
            x -= 17;
            lcd.mask4(x, 4, &ICON_GPS, SUB, None, BG);
            if self.gps == GpsState::Fix {
                lcd.fill_r(x + 3, 7, 6, 6, 3, SUB);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;

    #[test]
    fn meter_levels() {
        assert_eq!(meter_level(-140), 0);
        assert_eq!(meter_level(-85), 13); // S7, as in the HomeA mock-up
        assert_eq!(meter_level(-73), 17);
        assert_eq!(meter_level(0), METER_SEGS);
        assert_eq!(s_text(-85).as_bytes(), b"S7");
        assert_eq!(s_text(-60).as_bytes(), b"+10");
        assert_eq!(s_text(-140).as_bytes(), b"S0");
    }

    #[test]
    fn fit_whole_chars() {
        let s = "A·B·C".as_bytes();
        assert_eq!(fit(s, &UI_12, 1000), s);
        assert_eq!(fit(s, &UI_12, 14), "A·".as_bytes()); // never half a middle dot
        assert_eq!(fit(s, &UI_12, 0), b"");
    }

    #[test]
    fn field_paints_its_box_only() {
        let mut fb = FrameBuffer::new();
        fb.px.fill(0x1234);
        field(&mut fb, 20, 100, 30, 47, 43, Align::Right, b"Narrow", &UI_12, TEXT, BG);
        assert_eq!(fb.at(19, 35), 0x1234);
        assert_eq!(fb.at(120, 35), 0x1234);
        assert_eq!(fb.at(20, 29), 0x1234);
        assert_eq!(fb.at(20, 47), 0x1234);
        assert_eq!(fb.at(21, 31), BG);
        assert!((70..120).any(|x| fb.at(x, 40) != BG), "text at the right");
    }

    #[test]
    fn status_bar_redraws_only_changes() {
        struct Count(FrameBuffer, u64);
        impl Panel for Count {
            fn window(&mut self, a: i32, b: i32, c: i32, d: i32) {
                self.0.window(a, b, c, d)
            }
            fn push(&mut self, c: u16, n: u32) {
                self.1 += n as u64;
                self.0.push(c, n)
            }
            fn push_buf(&mut self, p: &[u16]) {
                self.1 += p.len() as u64;
                self.0.push_buf(p)
            }
        }
        let mut lcd = Count(FrameBuffer::new(), 0);
        let a = StatusBar { clock: Some(14 * 60 + 32), watch: 3, gps: GpsState::Fix, battery: 4, ..Default::default() };
        a.draw(&mut lcd, None);
        assert!(lcd.1 >= 240 * 20);
        lcd.1 = 0;
        a.draw(&mut lcd, Some(&a));
        assert_eq!(lcd.1, 0, "nothing changed, nothing drawn");
        let b = StatusBar { clock: Some(14 * 60 + 33), ..a };
        b.draw(&mut lcd, Some(&a));
        assert!(lcd.1 > 0 && lcd.1 <= 40 * 19, "only the clock: {} px", lcd.1);
    }

    #[test]
    fn widgets_draw_inside_their_boxes() {
        let mut fb = FrameBuffer::new();
        fb.px.fill(0x1234);
        freq(&mut fb, 15, 225, 89, 14_652_000, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
        assert!(fb.at(15, 89 - digit_h(&DIG_46) - 1) == 0x1234, "freq painted above its digits");
        assert!(fb.at(15, 90) == 0x1234, "freq painted below the baseline");
        chips(&mut fb, 15, 97, 210, &[b"FM-N", "T 100.0".as_bytes()], SURFACE);
        meter(&mut fb, 15, 124, None, 7, GREEN, SURFACE);
        assert_eq!(fb.at(16, 128), GREEN);
        assert_eq!(fb.at(15 + 8 * 10 + 1, 128), LINE);
        hint_bar(&mut fb, &[("SLOT", "A·B·C"), ("OK", "Quick"), (LEFT, "Band")]);
        hint_bar(&mut fb, &[(UP, ""), ("Knob", "Change")]);
        toast(&mut fb, b"VE3KPW-9", b"On my way", true);
        list_row(&mut fb, 41, 22, &Row { num: Some(1), name: b"Squelch", value: b"4", dot: true, selected: true, grey: false });
        kv(&mut fb, 20, 18, &KV_STATUS, b"Transmit", b"Ready", GREEN, false);
        help_box(&mut fb, 265, 33, b"Audio opens above this level. Home: SIDE1 + knob.", b"MENU 1");
        edit_cell(&mut fb, 161, 85, 72, 31, b"Tone", b"T 100.0", true, false);
        badge(&mut fb, 14, 33, 2, false, SURFACE);
        tag(&mut fb, 226, 32, b"RX", GREEN, SURFACE);
        // nothing escaped the screen's bands: the status bar rows were never touched
        assert!((0..240).all(|x| fb.at(x, 0) == 0x1234));
    }
}
