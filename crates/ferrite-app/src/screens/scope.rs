//! Band scope (manual chapter 21, SCOPE-1 to SCOPE-7; Scope mock-up): signal strength across a span around a
//! centre, swept by the Radio task. ◀▶ move the marker, the knob moves the centre, OK tunes the active slot
//! to the marker and goes home, MENU opens the scope options (MENU 0 3 2).
//!
//! Bars are redrawn a group at a time when their levels change (SCOPE-7). The waterfall keeps no history in
//! RAM (SCOPE-6): each sweep paints one row into a 12-row band that wraps, a dark line marking the newest.
//! (The ST7789 hardware scroll is the radio's later refinement.)

use super::activity::Mode;
use super::nav::Dest;
use super::parts;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::band_name;
use crate::{App, Screen};
use ferrite_core::freq::Freq10;
use ferrite_core::keys::Key;
use ferrite_core::msg::{ModeLive, RadioCmd, SCOPE_BARS};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Scope steps (MENU 0 3 2 2), 10 Hz units; the default is 25 kHz.
pub const STEPS: [u16; 5] = [625, 1250, 2500, 5000, 10_000];
pub const STEP_DEFAULT: u8 = 2;
const MID: i32 = SCOPE_BARS as i32 / 2;
/// Bars: x = X0 + 4 i, 3 px wide; the area's rows.
const X0: i32 = 8;
const TOP: i32 = 46;
const BASE: i32 = 148;
/// Waterfall rows.
const WF_Y: i32 = 168;
const WF_ROWS: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scope {
    /// Bar index, 0-55; the centre is 28.
    pub marker: u8,
    pub step: u8,
}

/// Hold 3 on home (SCOPE-1): the scope around the active slot's frequency.
pub fn open(app: &mut App) -> Screen {
    let f = app.slot(app.active).freq();
    app.radio.centre = f;
    let s = Scope { marker: MID as u8, step: STEP_DEFAULT };
    sweep(app, f, s.step);
    Screen::Scope(s)
}

fn sweep(app: &mut App, centre: Freq10, step: u8) {
    app.start_mode(Mode::Scope);
    app.out.push(RadioCmd::Sweep { centre, step: STEPS[step as usize % STEPS.len()], on: true });
}

/// The frequency of bar i.
pub fn bar_freq(centre: Freq10, step: u16, i: i32) -> Freq10 {
    (centre as i64 + (i - MID) as i64 * step as i64).max(0) as Freq10
}

/// The three strongest local peaks, strongest first (bar indexes).
pub fn peaks(bars: &[u8; SCOPE_BARS]) -> [Option<u8>; 3] {
    let mut p = [None; 3];
    for i in 0..SCOPE_BARS {
        let v = bars[i];
        let l = if i > 0 { bars[i - 1] } else { 0 };
        let r = if i + 1 < SCOPE_BARS { bars[i + 1] } else { 0 };
        if v == 0 || v < l || v < r || parts::level_h(v, 90) < 25 {
            continue;
        }
        for k in 0..3 {
            if p[k].is_none_or(|j: u8| bars[j as usize] < v) {
                p.copy_within(k..2, k + 1);
                p[k] = Some(i as u8);
                break;
            }
        }
    }
    p
}

impl View for Scope {
    fn name(self) -> &'static str {
        "Scope"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("Scope.png")
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let step = STEPS[self.step as usize % STEPS.len()];
        match (e.key, e.press) {
            (Key::Left | Key::Right, Short | Repeat) => {
                let d = if e.key == Key::Right { 1 } else { -1 };
                let m = (self.marker as i32 + d).clamp(0, SCOPE_BARS as i32 - 1) as u8;
                app.screen = Screen::Scope(Scope { marker: m, ..self });
            }
            (Key::Menu, Short) => {
                // SCOPE-3: OK (Go) tunes the active slot's VFO to the marker.
                let f = bar_freq(app.radio.centre, step, self.marker as i32);
                let a = app.active as usize;
                app.slots[a].vfo = f;
                app.slots[a].vfo_mode = true;
                app.go_home();
                app.send_slot(a as u8);
                let fq = w::freq3(f);
                app.show_toast(b"Tuned", fq.as_bytes(), false);
            }
            (Key::Menu, Long) => app.open(Dest::Sub(3, 2)), // the scope options
            (Key::Exit, Short) => {
                app.stop_mode();
                app.back();
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let step = STEPS[self.step as usize % STEPS.len()];
        let c = bar_freq(app.radio.centre, step, MID + d);
        app.radio.centre = c;
        sweep(app, c, self.step);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let step = STEPS[self.step as usize % STEPS.len()];
        let (centre, bars, sweeps, rate) = match cx.live.mode {
            ModeLive::Scope { centre, bars, sweeps, rate } => (centre, bars, sweeps, rate),
            _ => (app.radio.centre, [0; SCOPE_BARS], 0, 0),
        };
        // Header: band, centre.
        if app.dirty(1, hash(&centre)) {
            w::field(lcd, 8, 80, 21, 39, 34, Align::Left, band_name(centre).as_bytes(), &UI_12, SUB, BG);
            let fq = w::freq3(centre);
            let fw = text4_width(fq.as_bytes(), &UI_12B);
            w::field(lcd, 88, 144 - fw - 4, 21, 39, 34, Align::Right, b"centre", &UI_12, SUB, BG);
            w::field(lcd, 232 - fw, fw, 21, 39, 34, Align::Left, fq.as_bytes(), &UI_12B, TEXT, BG);
            lcd.fill(0, 40, 240, 1, LINE);
            // Scale: the edges and the centre.
            let lo = w::freq3(bar_freq(centre, step, 0));
            let hi = w::freq3(bar_freq(centre, step, SCOPE_BARS as i32 - 1));
            lcd.fill(0, BASE, 240, 1, LINE);
            w::field(lcd, 8, 70, 150, 166, 162, Align::Left, lo.as_bytes(), &UI_12, SUB, BG);
            w::field(lcd, 78, 84, 150, 166, 162, Align::Center, fq.as_bytes(), &UI_12, BLUE, BG);
            w::field(lcd, 162, 70, 150, 166, 162, Align::Right, hi.as_bytes(), &UI_12, SUB, BG);
        }
        // Bars, eight at a time (SCOPE-7): centre line blue, peaks green, the marker amber with its mark.
        let pk = peaks(&bars);
        let m = self.marker as usize;
        for g in 0..SCOPE_BARS / 8 {
            let r = &bars[g * 8..g * 8 + 8];
            let has_m = (g * 8..g * 8 + 8).contains(&m);
            let pks = pk.map(|p| p.filter(|&i| i as usize / 8 == g));
            if !app.dirty(2 + g, hash(&(r, has_m.then_some(m), pks))) {
                continue;
            }
            let x = X0 + g as i32 * 32;
            lcd.fill(x, TOP - 8, 32, BASE - TOP + 8, BG);
            for (k, &v) in r.iter().enumerate() {
                let i = g * 8 + k;
                let bx = x + k as i32 * 4;
                if i as i32 == MID {
                    lcd.fill(bx + 1, TOP, 1, BASE - TOP, BLUE);
                }
                let h = parts::level_h(v, BASE - TOP);
                let c = if i == m {
                    AMBER
                } else if pk.contains(&Some(i as u8)) {
                    GREEN
                } else {
                    rgb565(0x4D, 0x80, 0xC0)
                };
                if h > 0 {
                    lcd.fill(bx, BASE - h, 3, h, c);
                }
                if i == m {
                    lcd.tri(bx - 2, TOP - 8, 7, 1, AMBER);
                }
            }
        }
        // Waterfall: one row per sweep, wrapping; the next row's place shows dark.
        if app.dirty(10, hash(&sweeps)) {
            let row = (sweeps as i32) % WF_ROWS;
            let y = WF_Y + row * 3;
            for (i, &v) in bars.iter().enumerate() {
                lcd.fill(X0 + i as i32 * 4, y, 4, 3, heat(v));
            }
            let ny = WF_Y + ((row + 1) % WF_ROWS) * 3;
            lcd.fill(X0, ny, 224, 3, BG);
        }
        // The marker readout: frequency and level, then the peaks.
        let mf = bar_freq(centre, step, m as i32);
        let lv = bars[m];
        let st = if lv == 0 { Buf::from(b"-") } else { w::s_text(lv as i16 - 160) };
        if app.dirty(11, hash(&(mf, st.as_bytes()))) {
            let y = 208;
            lcd.fill(6, y, 228, 44, BG);
            lcd.fill_r(6, y, 228, 44, 6, SURFACE);
            lcd.frame_r(6, y, 228, 44, 6, AMBER);
            lcd.text4_rows(15, y + 20, b"Marker", &UI_12, AMBER, SURFACE, y + 4, y + 24);
            w::freq(lcd, 66, 180, y + 21, mf, &DIG_17, &DIG_17, TEXT, SUB, SURFACE);
            w::field(lcd, 180, 46, y + 4, y + 24, y + 20, Align::Right, st.as_bytes(), &UI_12B, TEXT, SURFACE);
            app.drawn.hash[12] = 0;
        }
        let mut p = Buf::<40>::new();
        p.push(b"Peaks");
        for i in pk.iter().flatten() {
            p.push(b" ");
            p.push(w::freq3(bar_freq(centre, step, *i as i32)).as_bytes());
        }
        if pk[0].is_none() {
            p.push(b" none");
        }
        if app.dirty(12, hash(&p)) {
            w::field(lcd, 15, 211, 230, 247, 243, Align::Left, p.as_bytes(), &UI_12, SUB, SURFACE);
        }
        // Span, step, sweep rate (SCOPE-2).
        let mut f = Buf::<40>::new();
        let span = step as u32 * SCOPE_BARS as u32;
        f.push(b"Span ");
        khz(span, &mut f);
        f.push(" \u{b7} step ".as_bytes());
        khz(step as u32, &mut f);
        let _ = uwrite!(f, " \u{b7} {}/s", rate);
        if app.dirty(13, hash(&f)) {
            w::field(lcd, 8, 224, 256, 273, 269, Align::Left, f.as_bytes(), &UI_12, SUB, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_LR, "Marker"), ("Knob", "Centre"), ("OK", "Go")])
    }
}

/// "800k", "12.5k", "1.4M": 10 Hz units, short.
pub fn khz(f: u32, b: &mut Buf<40>) {
    if f >= 100_000 {
        let _ = uwrite!(b, "{}", f / 100_000);
        if f % 100_000 >= 10_000 {
            let _ = uwrite!(b, ".{}", f % 100_000 / 10_000);
        }
        b.push(b"M");
    } else {
        let _ = uwrite!(b, "{}", f / 100);
        let r = f % 100;
        if r != 0 {
            let _ = uwrite!(b, ".{}", if r.is_multiple_of(10) { r / 10 } else { r });
        }
        b.push(b"k");
    }
}

/// Waterfall colour for a level: dark blue through teal and green to amber.
pub fn heat(v: u8) -> u16 {
    match parts::level_h(v, 8) {
        0 | 1 => rgb565(0x0F, 0x2A, 0x45),
        2 => rgb565(0x1A, 0x55, 0x80),
        3 | 4 => rgb565(0x2A, 0x9D, 0xA8),
        5 | 6 => rgb565(0x5C, 0xD0, 0x9A),
        _ => AMBER,
    }
}

/// The small scope under the active slot (home, single + scope; HOME-3): bars, the centre in blue, the edges.
pub fn strip(app: &mut App, lcd: &mut impl Panel, region: usize, y: i32, h: i32, centre: Freq10, step: u16, bars: &[u8; SCOPE_BARS]) {
    if !app.dirty(region, hash(&(bars, centre))) {
        return;
    }
    lcd.fill(0, y, 240, h + 18, BG);
    for (i, &v) in bars.iter().enumerate() {
        let bx = X0 + i as i32 * 4;
        let bh = parts::level_h(v, h).max(2);
        let c = if i as i32 == MID { BLUE } else { DIM };
        lcd.fill(bx, y + h - bh, 3, bh, c);
    }
    lcd.fill(X0, y + h, 224, 1, LINE);
    let half = step as u32 * MID as u32;
    let mut a = Buf::<40>::new();
    a.push(b"-");
    khz(half, &mut a);
    let mut b = Buf::<40>::new();
    b.push(b"+");
    khz(half, &mut b);
    w::field(lcd, 8, 60, y + h + 2, y + h + 18, y + h + 14, Align::Left, a.as_bytes(), &UI_12, SUB, BG);
    w::field(lcd, 80, 80, y + h + 2, y + h + 18, y + h + 14, Align::Center, w::freq3(centre).as_bytes(), &UI_12, SUB, BG);
    w::field(lcd, 172, 60, y + h + 2, y + h + 18, y + h + 14, Align::Right, b.as_bytes(), &UI_12, SUB, BG);
}
