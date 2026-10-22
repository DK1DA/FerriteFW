//! Low Power Mode's home (PWR-24.7, decision 234; the approved mock-up `LowPowerMode`): while the mode is on, home is
//! drawn this way instead of 0 7 2's layout ([`App::home_layout`]). The keys are home's (tuning, V/M, editing);
//! only the drawing differs.
//!
//! The whole 240 x 320 in fixed colours (theme-independent, so it never passes for the normal home): an amber band
//! top (LOW POWER, the clock, the battery %) and bottom (home's own hints), amber sides, and the active slot alone,
//! big, on black: its head line, the frequency, mode/tone/shift/power, then RX with an 8-block meter, or TX with
//! the time keyed. Over the frequency, while SIDE1 + SIDE2 are held, a "Leaving" box fills over the second
//! ([`App::lpm_leave_progress`]).

use super::shared::*;
use super::Cx;
use crate::chan::ChannelStore;
use crate::slot::{card_tone, mode_chip, shift_text};
use crate::App;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::{rgb565, Align, Gfx};
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

/// The mode's own colours, the same in every theme (PWR-24.7).
pub const LP_AMBER: u16 = rgb565(0xFF, 0xB0, 0x00);
const LP_BLACK: u16 = 0;
const LP_TEXT: u16 = rgb565(0xF2, 0xEF, 0xE6);
const LP_SUB: u16 = rgb565(0xB9, 0xB3, 0xA3);
const LP_DIM: u16 = rgb565(0x6F, 0x6A, 0x5E);
const LP_EMPTY: u16 = rgb565(0x3A, 0x37, 0x2F);
const LP_GREEN: u16 = rgb565(0x3D, 0xDC, 0x84);
const LP_RED: u16 = rgb565(0xE5, 0x48, 0x4D);

/// The bands' heights, and the sides' width.
const TOP_H: i32 = 26;
const BOTTOM_Y: i32 = 294;
const SIDE_W: i32 = 4;
/// The meter: 8 blocks of 18 x 12, 4 apart.
const BLOCKS: i32 = 8;
/// The leaving box (over the frequency and settings line).
const BOX: (i32, i32, i32, i32) = (16, 92, 208, 66);

/// Low Power Mode's home, on the panel.
pub(crate) fn render(app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
    let live = cx.live;
    let a = app.active;
    // The frame: the sides and the black inside (the bands are their own regions).
    let mut drew = false;
    if app.dirty(R_FRAME, 0x4C50) {
        lcd.fill(0, TOP_H, SIDE_W, BOTTOM_Y - TOP_H, LP_AMBER);
        lcd.fill(240 - SIDE_W, TOP_H, SIDE_W, BOTTOM_Y - TOP_H, LP_AMBER);
        lcd.fill(SIDE_W, TOP_H, 240 - 2 * SIDE_W, BOTTOM_Y - TOP_H, LP_BLACK);
        drew = true;
    }
    // The top band: LOW POWER, the clock, the battery.
    let (clock, pct) = (cx.dev.clock_min, cx.dev.battery_pct);
    if app.dirty(R_TOP, hash(&(clock, pct))) {
        lcd.fill(0, 0, 240, TOP_H, LP_AMBER);
        w::field(lcd, 8, 90, 0, TOP_H, 18, Align::Left, b"LOW POWER", &UI_12B, LP_BLACK, LP_AMBER);
        if let Some(m) = clock {
            let mut t = Buf::<8>::new();
            let _ = ufmt::uwrite!(t, "{}:{}", ferrite_ui::text::Zero((m / 60) as u32, 2), ferrite_ui::text::Zero((m % 60) as u32, 2));
            w::field(lcd, 98, 44, 0, TOP_H, 18, Align::Center, t.as_bytes(), &UI_12B, LP_BLACK, LP_AMBER);
        }
        let mut p = Buf::<6>::new();
        let _ = ufmt::uwrite!(p, "{}%", pct);
        w::field(lcd, 172, 60, 0, TOP_H, 18, Align::Right, p.as_bytes(), &UI_12B, LP_BLACK, LP_AMBER);
    }
    // The head line, and RX or TX on its right.
    let keyed = app.radio.keyed.is_some();
    let rx = live.slots[a as usize].squelch_open;
    let mut h = Buf::<32>::new();
    app.head_text(a, live, cx.store, &mut h);
    if app.dirty(R_HEAD, hash(&(&h, keyed, rx))) || drew {
        lcd.fill(SIDE_W, 34, 240 - 2 * SIDE_W, 22, LP_BLACK);
        let tag_w = if keyed || rx { 40 } else { 0 };
        w::field(lcd, 14, 212 - tag_w, 36, 54, 50, Align::Left, w::fit(h.as_bytes(), &UI_12, 212 - tag_w), &UI_12, LP_SUB, LP_BLACK);
        if keyed {
            w::tag(lcd, 226, 37, b"TX", LP_RED, LP_BLACK);
        } else if rx {
            w::tag(lcd, 226, 37, b"RX", LP_GREEN, LP_BLACK);
        }
        drew = true;
    }
    // The frequency (the transmit one while keyed), big.
    let f = if keyed { app.tuned_cfg(a).tx_of(app.tuned_freq(a)) } else { app.shown_freq(a, live) };
    if app.dirty(R_FREQ, hash(&f)) || drew {
        lcd.fill(SIDE_W, 58, 240 - 2 * SIDE_W, 70, LP_BLACK);
        w::freq(lcd, 10, 232, 122, f, &DIG_46, &DIG_28, LP_TEXT, LP_SUB, LP_BLACK);
        drew = true;
    }
    // Mode, tone, shift, power.
    let cfg = app.tuned_cfg(a);
    let mut line = Buf::<40>::new();
    let mut part = Buf::<16>::new();
    mode_chip(&cfg, &mut part);
    line.push(part.as_bytes());
    for i in 0..3 {
        part.clear();
        match i {
            0 => card_tone(&cfg, &mut part),
            1 => shift_text(&cfg, false, &mut part),
            _ => part.set(super::transmit::power_name(app, a).as_bytes()),
        }
        if !part.is_empty() {
            line.push(b"  ");
            line.push(part.as_bytes());
        }
    }
    if app.dirty(R_INFO, hash(&line)) || drew {
        w::field(lcd, 14, 212, 134, 156, 150, Align::Left, w::fit(line.as_bytes(), &UI_12, 212), &UI_12, LP_SUB, LP_BLACK);
        drew = true;
    }
    // RX: the S-meter's text and 8 blocks; TX: the time keyed; else Quiet and empty blocks.
    let rssi = live.slots[a as usize].rssi;
    let lit = if rx { ((w::meter_level(rssi) as u32 * BLOCKS as u32).div_ceil(w::METER_SEGS as u32)) as i32 } else { 0 };
    let secs = if keyed { app.now.wrapping_sub(app.radio.t0) / 1000 } else { 0 };
    if app.dirty(R_SUB, hash(&(keyed, rx, lit, secs))) || drew {
        lcd.fill(SIDE_W, 176, 240 - 2 * SIDE_W, 46, LP_BLACK);
        let mut t = Buf::<8>::new();
        let (text, c) = if keyed {
            super::activity::mmss(secs, &mut t);
            (t.as_bytes(), LP_RED)
        } else if rx {
            t.set(w::s_text(rssi).as_bytes());
            (t.as_bytes(), LP_GREEN)
        } else {
            (b"Quiet".as_slice(), LP_DIM)
        };
        w::field(lcd, 14, 120, 180, 198, 194, Align::Left, text, if keyed || rx { &UI_12B } else { &UI_12 }, c, LP_BLACK);
        if !keyed {
            for i in 0..BLOCKS {
                let x = 14 + i * 22;
                if i < lit {
                    lcd.fill(x, 206, 18, 12, LP_GREEN);
                } else {
                    lcd.frame(x, 206, 18, 12, LP_EMPTY);
                }
            }
        }
        drew = true;
    }
    // What's off, and how to leave.
    if app.dirty(R_NOTE, 0x4C51) || drew {
        w::field(lcd, 14, 212, 232, 250, 246, Align::Left, b"Scan, watch, APRS, GPS", &UI_12, LP_DIM, LP_BLACK);
        w::field(lcd, 14, 212, 250, 268, 264, Align::Left, b"and Bluetooth are off.", &UI_12, LP_DIM, LP_BLACK);
        w::field(lcd, 14, 212, 272, 292, 287, Align::Left, b"Hold SIDE1 + SIDE2 to leave", &UI_12B, LP_AMBER, LP_BLACK);
    }
    // The bottom band: home's own hints in black on amber (a hint-bar toast covers it while it shows).
    let hints = app.screen.hints(app);
    let toast = app.toast_shown();
    if app.dirty(R_LINE, hash(&(hints.as_slice(), toast))) && !toast {
        lcd.fill(0, BOTTOM_Y, 240, 320 - BOTTOM_Y, LP_AMBER);
        let items = hints.as_slice();
        let n = items.len();
        let colours = (LP_BLACK, LP_BLACK, LP_AMBER);
        for (i, (k, l)) in items.iter().enumerate() {
            let width = w::key_label_in(lcd, 0, 0, k, l, colours, false);
            let x = match i {
                0 => 8,
                _ if i == n - 1 => 232 - width,
                _ => (240 - width) / 2,
            };
            w::key_label_in(lcd, x, 312, k, l, colours, true);
        }
    }
    // Leaving: the box over the frequency, its bar filling over the second; when it goes, what was under it again.
    match app.lpm_leave_progress() {
        Some(p) => {
            let fill = (BOX.2 - 24) * p as i32 / 1000;
            if app.dirty(R_PAGE, hash(&fill)) || drew {
                let (x, y, bw, bh) = BOX;
                lcd.fill(x, y, bw, bh, LP_AMBER);
                lcd.fill(x + 2, y + 2, bw - 4, bh - 4, LP_BLACK);
                w::field(lcd, x + 12, bw - 24, y + 6, y + 24, y + 19, Align::Left, b"Leaving", &UI_12B, LP_AMBER, LP_BLACK);
                w::field(lcd, x + 12, bw - 24, y + 24, y + 42, y + 37, Align::Left, b"Keep both held.", &UI_12, LP_TEXT, LP_BLACK);
                lcd.fill(x + 12, y + 48, bw - 24, 6, LP_EMPTY);
                lcd.fill(x + 12, y + 48, fill, 6, LP_AMBER);
            }
        }
        None => {
            if app.dirty(R_PAGE, 0) {
                for r in [R_HEAD, R_FREQ, R_INFO] {
                    app.drawn.hash[r] = 1; // redrawn next time
                }
            }
        }
    }
}
