//! Home's other two layouts (manual chapter 6, HOME-3; HomeB and HomeC mock-ups), chosen with MENU 0 7 2. The
//! keys are home's in every layout (HOME-3, NAV-6); only the drawing differs.
//!
//! - Single + scope: the active slot with its settings, S-meter and a scope strip around it; the other slots
//!   one line each.
//! - Watch + activity: three equal cards, each with its last minute of activity (`RadioUi::act`).

use super::activity::{Mode, ACT_BUCKETS};
use super::scope;
use super::shared::*;
use super::Cx;
use crate::chan::ChannelStore;
use crate::slot::{band_name, mode_chip, offset_text, tone_text, Item, STEPS};
use crate::App;
use ferrite_core::msg::{LiveAll, ModeLive, RadioCmd, SCOPE_BARS, SLOTS};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

/// The strip's step: 4 kHz, so 56 bars cover about ±112 kHz.
pub const STRIP_STEP: u16 = 400;

/// Home's tick: the single + scope layout keeps a narrow sweep running around the active frequency.
pub fn tick(app: &mut App) {
    if app.set.layout != 1 || !matches!(app.radio.mode, Mode::Off | Mode::Strip) {
        return;
    }
    let f = app.slot(app.active).freq();
    if app.radio.mode != Mode::Strip || app.radio.centre != f {
        app.radio.mode = Mode::Strip;
        app.radio.centre = f;
        app.out.push(RadioCmd::Sweep { centre: f, step: STRIP_STEP, on: true });
    }
}

/// Single + scope (HomeB).
pub fn single(app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
    let live = cx.live;
    let a = app.active;
    let lv = live.slots[a as usize];
    let f = app.shown_freq(a, live);
    let (state, sc) = app.state(a, live);
    if app.dirty(1, hash(&(a, band_name(f), state))) {
        w::badge(lcd, 8, 25, a, true, BG);
        let mut h = Buf::<32>::new();
        app.head_text(a, live, &mut h);
        w::field(lcd, 26, 140, 22, 40, 36, Align::Left, h.as_bytes(), &UI_12, SUB, BG);
        lcd.fill(166, 22, 74, 20, BG);
        if state == "RX" {
            w::tag(lcd, 232, 24, b"RX", GREEN, BG);
        } else if state != "idle" {
            w::field(lcd, 166, 66, 22, 40, 36, Align::Right, state.as_bytes(), &UI_12, sc, BG);
        }
    }
    if app.dirty(2, hash(&f)) {
        w::freq(lcd, 8, 232, 88, f, &DIG_46, &DIG_28, TEXT, SUB, BG);
    }
    // The settings, label over value (HOME-4): mode, tone, power; step, offset, squelch.
    let cfg = *app.slot(a).cfg();
    let mut v = [Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new()];
    mode_chip(&cfg, &mut v[0]);
    tone_text(cfg.tx_tone, &mut v[1]);
    if v[1].is_empty() {
        v[1].set(b"None");
    }
    v[2].set(super::transmit::power_name(app, a).as_bytes());
    let st = STEPS[cfg.step as usize % STEPS.len()].1;
    match st.strip_suffix(" kHz") {
        Some(k) => {
            v[3].set(k.as_bytes());
            v[3].push(b"k");
        }
        None => v[3].set(st.as_bytes()),
    }
    offset_text(cfg.offset, false, &mut v[4]);
    if v[4].is_empty() {
        v[4].set(b"Simplex");
    }
    let mut sq = Buf::<16>::new();
    app.item_text(Item::Squelch, false, &mut sq);
    v[5] = sq;
    const LABELS: [&str; 6] = ["Mode", "Tone", "Power", "Step", "Offset", "Squelch"];
    for (i, l) in LABELS.iter().enumerate() {
        let (x, y) = (8 + (i as i32 % 3) * 76, 96 + (i as i32 / 3) * 36);
        if app.dirty(3 + i, hash(&v[i])) {
            w::field(lcd, x, 74, y, y + 16, y + 12, Align::Left, l.as_bytes(), &UI_12, SUB, BG);
            w::field(lcd, x, 74, y + 16, y + 34, y + 30, Align::Left, v[i].as_bytes(), &UI_12B, TEXT, BG);
        }
    }
    // S-meter with its scale.
    let lit = w::meter_level(lv.rssi);
    let col = if lv.squelch_open { GREEN } else { DIM };
    let old = app.drawn.meter[a as usize].filter(|&(_, o)| o == lv.squelch_open).map(|(l, _)| l);
    if app.drawn.meter[a as usize] != Some((lit, lv.squelch_open)) {
        w::meter(lcd, 8, 170, old, lit, col, BG);
        app.drawn.meter[a as usize] = Some((lit, lv.squelch_open));
    }
    let s = w::s_text(lv.rssi);
    if app.dirty(9, hash(&s)) {
        for (i, t) in ["1", "3", "5", "7", "9", "+20", "+40"].iter().enumerate() {
            let x = 8 + [0, 20, 40, 60, 80, 120, 160][i];
            lcd.text4_rows(x, 192, t.as_bytes(), &UI_12, SUB, BG, 180, 196);
        }
        w::field(lcd, 208, 26, 178, 196, 192, Align::Right, s.as_bytes(), &UI_12B, TEXT, BG);
    }
    // The scope strip (SCOPE strip from the sweep the tick keeps running).
    let (centre, bars) = match live.mode {
        ModeLive::Scope { centre, bars, .. } => (centre, bars),
        _ => (f, [0; SCOPE_BARS]),
    };
    scope::strip(app, lcd, 10, 200, 34, centre, STRIP_STEP, &bars);
    // The other slots, a line each.
    for (i, o) in (0..SLOTS as u8).filter(|&o| o != a).enumerate() {
        let (st, c) = app.state(o, live);
        let sl = app.slot(o);
        let mut t = Buf::<40>::new();
        t.push(w::freq3(app.shown_freq(o, live)).as_bytes());
        t.push(" \u{b7} ".as_bytes());
        match sl.ch.filter(|_| sl.on_channel()) {
            Some(ch) => t.push(ch.name()),
            None => t.push(band_name(sl.freq()).as_bytes()),
        }
        let y = 256 + 21 * i as i32;
        if app.dirty(11 + i, hash(&(o, t, st))) {
            w::badge(lcd, 8, y + 2, o, false, BG);
            w::field(lcd, 28, 150, y, y + 18, y + 14, Align::Left, t.as_bytes(), &UI_12, TEXT, BG);
            w::field(lcd, 178, 54, y, y + 18, y + 14, Align::Right, st.as_bytes(), &UI_12, c, BG);
        }
    }
}

/// Watch + activity (HomeC): three equal cards, each with its last minute.
pub fn activity(app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
    let live: &LiveAll = cx.live;
    for s in 0..SLOTS as u8 {
        let p = s as usize;
        let (y, h) = (25 + 82 * s as i32, 76);
        let lv = live.slots[p];
        let (state, sc) = app.state(s, live);
        let active = s == app.active;
        let border = if active { BLUE } else if state == "RX" { GREEN } else { LINE };
        if app.dirty(R_FRAME + p, hash(&(s, border))) {
            lcd.fill(6, y, 228, h, BG);
            lcd.fill_r(6, y, 228, h, 6, SURFACE);
            lcd.frame_r(6, y, 228, h, 6, border);
            for r in [R_HEAD, R_FREQ, R_INFO, R_SUB] {
                app.drawn.hash[r + p] = 0;
            }
            app.drawn.hash[16 + p] = 0;
        }
        let mut head = Buf::<32>::new();
        app.head_text(s, live, &mut head);
        let mut stt = Buf::<16>::new();
        stt.set(state.as_bytes());
        if state == "RX" {
            stt.push(b" ");
            stt.push(w::s_text(lv.rssi).as_bytes());
        }
        if app.dirty(R_HEAD + p, hash(&(head, stt, active))) {
            w::badge(lcd, 14, y + 5, s, active, SURFACE);
            w::field(lcd, 34, 120, y + 4, y + 21, y + 17, Align::Left, head.as_bytes(), &UI_12, SUB, SURFACE);
            let f = if state == "RX" { &UI_12B } else { &UI_12 };
            w::field(lcd, 154, 72, y + 4, y + 21, y + 17, Align::Right, stt.as_bytes(), f, sc, SURFACE);
        }
        let f = app.shown_freq(s, live);
        if app.dirty(R_FREQ + p, hash(&f)) {
            w::freq(lcd, 15, 140, y + 44, f, &DIG_26, &DIG_17, TEXT, SUB, SURFACE);
        }
        let sl = *app.slot(s);
        let mut name = Buf::<16>::new();
        if lv.scanning {
            let _ = ufmt::uwrite!(name, "{} hits", lv.hits);
        } else if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
            name.set(c.name());
        }
        if app.dirty(R_INFO + p, hash(&name)) {
            w::field(lcd, 140, 86, y + 26, y + 44, y + 42, Align::Right, name.as_bytes(), &UI_12B, TEXT, SURFACE);
        }
        let cfg = *sl.cfg();
        let mut sub = Buf::<40>::new();
        let mut b = Buf::<16>::new();
        if lv.scanning {
            sub.push(crate::slot::MODES[cfg.mode as usize % 6].as_bytes());
            if lv.hits > 0 {
                sub.push(" \u{b7} last ".as_bytes());
                sub.push(w::freq3(lv.last_hit).as_bytes());
            }
        } else {
            mode_chip(&cfg, &mut b);
            sub.push(b.as_bytes());
            tone_text(cfg.tx_tone, &mut b);
            if b.is_empty() {
                b.set(super::transmit::power_name(app, s).as_bytes());
            }
            sub.push(" \u{b7} ".as_bytes());
            sub.push(b.as_bytes());
        }
        sub.push(" \u{b7} last 60 s".as_bytes());
        if app.dirty(R_SUB + p, hash(&sub)) {
            w::field(lcd, 15, 211, y + 46, y + 61, y + 58, Align::Left, sub.as_bytes(), &UI_12, SUB, SURFACE);
        }
        // The last minute: a bar per 1.2 s bucket heard, newest at the right, on a dotted baseline.
        let act = app.radio.act[p];
        if app.dirty(16 + p, hash(&act)) {
            let (by, bh) = (y + 62, 10);
            lcd.fill(14, by, 212, bh + 1, SURFACE);
            for i in 0..ACT_BUCKETS as i32 {
                let x = 18 + (ACT_BUCKETS as i32 - 1 - i) * 6 + 10;
                if act & 1 << i != 0 {
                    lcd.fill(x, by, 4, bh, w::slot_colour(s));
                } else {
                    lcd.fill(x, by + bh - 1, 2, 1, DIM);
                }
            }
        }
    }
}
