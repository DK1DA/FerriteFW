//! Home, watch layout (manual chapter 6; HomeA mock-up): the slots stacked A, B, C, the active one large.
//! Tuning, bands and zones, SLOT, V/M, scan, squelch and the hold-digit shortcuts ([`super::nav::hold_digit`]).
//! The other two layouts (single + scope, watch + activity: HOME-3) are still to come.

use super::nav::{self, Dest};
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{self, band_near, mode_chip, offset_text, tone_text, Item, BANDS, STEPS};
use crate::{App, Screen, PAUSE_S};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_core::msg::{RadioCmd, SLOTS};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Home has no state of its own: everything it shows is the App's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Home;

/// Watch layout: the slots always stacked A, B, C; the active one is the large card, in its own place.
const CARD_TOP: i32 = 25;
const CARD_BIG: i32 = 115;
const CARD_SMALL: i32 = 62;
const CARD_GAP: i32 = 6;

/// Card position (y, height) of slot `s` when `active` is the large one (HOME-11).
pub(crate) fn card(s: u8, active: u8) -> (i32, i32) {
    let h = |i: u8| if i == active { CARD_BIG } else { CARD_SMALL };
    let y = CARD_TOP + (0..s).map(|i| h(i) + CARD_GAP).sum::<i32>();
    (y, h(s))
}

impl View for Home {
    fn name(self) -> &'static str {
        "Home"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("HomeA.png")
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.clear(); // home's status bar shows no screen name
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let big = app.keys.is_held(Key::Side(2));
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) if app.keys.is_held(Key::Side(1)) => {
                squelch_step(app, if e.key == Key::Up { 1 } else { -1 })
            }
            (Key::Up | Key::Down, Short | Repeat) => {
                if big {
                    app.keys.used(Key::Side(2));
                }
                step(app, if e.key == Key::Up { 1 } else { -1 }, big, store);
            }
            (Key::Left | Key::Right, Short | Repeat) => group(app, if e.key == Key::Right { 1 } else { -1 }, store),
            (Key::Slot, Short) => {
                // NAV-5
                app.active = (app.active + 1) % SLOTS as u8;
                app.out.push(RadioCmd::Active(app.active));
            }
            (Key::Slot, Long) => app.screen = Screen::Edit(super::edit::Edit { cell: 0 }), // EDIT-1
            (Key::VfoMem, Short) => toggle_vfo(app, store),
            (Key::VfoMem, Long) => {
                if app.slots[app.active as usize].vfo_mode {
                    app.open(Dest::SaveChannel); // VFO-9; on a channel it does nothing
                }
            }
            (Key::Hash, Short) => {
                if app.watched_rx() {
                    // HOME-10, WATCH-6
                    app.paused_until = Some(app.now.wrapping_add(PAUSE_S as u32 * 1000));
                    app.out.push(RadioCmd::PauseWatch(PAUSE_S));
                    app.show_toast(b"Watch paused", b"for 1 min", false);
                } else {
                    app.screen = Screen::Status(super::status::Status { row: 0, from_menu: false });
                }
            }
            (Key::Menu, Short) => app.screen = Screen::Quick(super::quick::Quick { row: 0 }), // MENU-1
            (Key::Menu, Long) => app.screen = Screen::Advanced(super::advanced::Advanced { row: 0, from_quick: false }), // MENU-2
            (Key::Side(2), Short) => {
                // SCAN-1: the scan shows on the card for now (the scan screen is still to come)
                let a = app.active;
                let on = app.scan_mask & 1 << a == 0;
                app.out.push(RadioCmd::Scan { slot: a, on });
            }
            (Key::Star, Long) => app.change(Item::Reverse, 1), // VFO-7: two values, so +1 toggles
            (Key::Digit(d), Short) => app.open(Dest::Entry(d)), // NAV-4, VFO-11
            (Key::Digit(d), Long) => nav::hold_digit(app, d),  // KEY-11
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        if app.keys.is_held(Key::Side(1)) {
            return squelch_step(app, d);
        }
        let big = app.keys.is_held(Key::Side(2));
        if big {
            app.keys.used(Key::Side(2));
        }
        step(app, d, big, store);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let live = cx.live;
        // The large card moved (SLOT): the gaps between cards moved too, so clear the whole card area and draw
        // every card again; otherwise slivers of the old large card stay behind (HOME-11).
        if app.dirty(R_NOTE, hash(&app.active)) {
            lcd.fill(0, 21, 240, 279, BG);
            app.drawn.hash[R_FRAME..=R_SUB + 2].fill(0);
            app.drawn.meter = [None; SLOTS];
        }
        for s in 0..SLOTS as u8 {
            // Fixed order A, B, C: the card's region index is its slot.
            let p = s as usize;
            let (y, h) = card(s, app.active);
            let big = s == app.active;
            let lv = live.slots[s as usize];
            let (state, sc) = app.state(s, live);
            let border = if big { BLUE } else if state == "RX" { GREEN } else { LINE };
            if app.dirty(R_FRAME + p, hash(&(s, y, h, border))) {
                lcd.fill(6, y, 228, h, BG);
                lcd.fill_r(6, y, 228, h, 6, SURFACE);
                lcd.frame_r(6, y, 228, h, 6, border);
                for r in [R_HEAD, R_FREQ, R_INFO, R_SUB] {
                    app.drawn.hash[r + p] = 0;
                }
                app.drawn.meter[p] = None;
            }
            // Header: letter, VFO/CH/SCAN and band, the state at the right (HOME-4, HOME-5).
            let mut head = Buf::<32>::new();
            app.head_text(s, live, &mut head);
            // Header rows: y+7..y+24 on the big card, y+4..y+21 on the compact ones (their digits start at y+22).
            let (hy, hb) = if big { (y + 7, y + 20) } else { (y + 4, y + 17) };
            if app.dirty(R_HEAD + p, hash(&(s, head, state, big))) {
                w::badge(lcd, 14, hy + 1, s, big, SURFACE);
                lcd.fill(28, hy, 6, 17, SURFACE);
                w::field(lcd, 34, 120, hy, hy + 17, hb, Align::Left, head.as_bytes(), &UI_12, SUB, SURFACE);
                if big {
                    lcd.fill(154, y + 6, 74, 18, SURFACE);
                    if state == "RX" {
                        w::tag(lcd, 226, y + 7, b"RX", GREEN, SURFACE);
                    } else if state != "idle" {
                        w::field(lcd, 154, 72, y + 7, y + 24, y + 20, Align::Right, state.as_bytes(), &UI_12, sc, SURFACE);
                    }
                } else {
                    let f = if state == "RX" { &UI_12B } else { &UI_12 };
                    w::field(lcd, 154, 72, hy, hy + 17, hb, Align::Right, state.as_bytes(), f, sc, SURFACE);
                }
            }
            let f = app.shown_freq(s, live);
            let cfg = *app.slots[s as usize].cfg();
            if big {
                if app.dirty(R_FREQ + p, hash(&f)) {
                    w::freq(lcd, 15, 226, y + 64, f, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
                }
                // Chips: mode, tone, offset, power when not default (HOME-4), reverse.
                let (mut m, mut t, mut o, mut pw) = (Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new());
                mode_chip(&cfg, &mut m);
                tone_text(cfg.tx_tone, &mut t);
                offset_text(cfg.offset, false, &mut o);
                if app.capped(s) {
                    pw.set(b"0.25 W");
                } else if cfg.power != 0 {
                    pw.set([b"MID".as_slice(), b"HIGH"][cfg.power as usize - 1]);
                }
                let rev: &[u8] = if cfg.reverse { b"REV" } else { b"" };
                if app.dirty(R_INFO + p, hash(&(m, t, o, pw, rev))) {
                    w::chips(lcd, 15, y + 72, 211, &[m.as_bytes(), t.as_bytes(), o.as_bytes(), pw.as_bytes(), rev], SURFACE);
                }
                // S-meter: only the segments that changed (HOME-6, HOME-7).
                let lit = w::meter_level(lv.rssi);
                let col = if lv.squelch_open { GREEN } else { DIM };
                let old = app.drawn.meter[p].filter(|&(_, o)| o == lv.squelch_open).map(|(l, _)| l);
                if app.drawn.meter[p] != Some((lit, lv.squelch_open)) {
                    w::meter(lcd, 14, y + 100, old, lit, col, SURFACE);
                    app.drawn.meter[p] = Some((lit, lv.squelch_open));
                }
                let st = w::s_text(lv.rssi);
                if app.dirty(R_SUB + p, hash(&st)) {
                    w::field(lcd, 212, 21, y + 94, y + 111, y + 108, Align::Right, st.as_bytes(), &UI_12B, TEXT, SURFACE);
                }
            } else {
                if app.dirty(R_FREQ + p, hash(&f)) {
                    w::freq(lcd, 15, 130, y + 42, f, &DIG_26, &DIG_17, TEXT, SUB, SURFACE);
                }
                let sl = &app.slots[s as usize];
                let mut name = Buf::<16>::new();
                if lv.scanning {
                    let _ = uwrite!(name, "{} hits", lv.hits);
                } else if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
                    name.set(c.name()); // as much as fits (CHAN-5 says 12; the mock-up shows 13)
                }
                if app.dirty(R_INFO + p, hash(&name)) {
                    w::field(lcd, 130, 96, y + 26, y + 44, y + 42, Align::Right, name.as_bytes(), &UI_12, TEXT, SURFACE);
                }
                let mut sub = Buf::<32>::new();
                let mut b = Buf::<16>::new();
                if lv.scanning {
                    sub.push(slot::MODES[cfg.mode as usize % 6].as_bytes());
                    if lv.hits > 0 {
                        sub.push(" \u{b7} last ".as_bytes());
                        sub.push(w::freq3(lv.last_hit).as_bytes());
                    }
                } else {
                    mode_chip(&cfg, &mut b);
                    sub.push(b.as_bytes());
                    tone_text(cfg.tx_tone, &mut b);
                    if !b.is_empty() {
                        sub.push(" \u{b7} ".as_bytes());
                        sub.push(b.as_bytes());
                    }
                    offset_text(cfg.offset, false, &mut b);
                    if !b.is_empty() {
                        sub.push(" \u{b7} ".as_bytes());
                        sub.push(b.as_bytes());
                    }
                }
                if app.dirty(R_SUB + p, hash(&sub)) {
                    w::field(lcd, 15, 211, y + 44, y + 60, y + 56, Align::Left, sub.as_bytes(), &UI_12, SUB, SURFACE);
                }
            }
        }
    }

    fn hints(self, app: &App) -> Hints {
        if app.locked {
            return Hints::new(&[("#", "Hold: unlock")]);
        }
        let group = if app.slots[app.active as usize].on_channel() { "Zone" } else { "Band" };
        let mid = if app.watched_rx() { ("#", "Pause") } else { ("OK", "Quick") };
        Hints::new(&[("SLOT", "A\u{b7}B\u{b7}C"), mid, (ARROWS_LR, group)])
    }
}

/// SIDE1 held + knob or ▲▼: the active slot's squelch, shown as a toast (KEY table, chapter 7). Stops at 0
/// and 9 rather than wrapping.
fn squelch_step(app: &mut App, d: i32) {
    app.keys.used(Key::Side(1));
    let sq = app.slots[app.active as usize].cfg().squelch as i32;
    let to = (sq + d).clamp(0, 9);
    app.change(Item::Squelch, to - sq);
    app.show_toast(b"Squelch", &[b'0' + to as u8], false);
}

/// Knob or ▲▼ on home: a step in VFO mode (NAV-2), a channel in channel mode (NAV-1); big with SIDE2 (NAV-3).
fn step(app: &mut App, d: i32, big: bool, store: &impl ChannelStore) {
    let a = app.active as usize;
    let s = &mut app.slots[a];
    if s.on_channel() {
        let n = store.zone_len(s.zone) as i32;
        if n == 0 {
            return;
        }
        s.pos = (s.pos as i32 + d * if big { 10 } else { 1 }).rem_euclid(n) as u16;
        s.ch = store.zone_at(s.zone, s.pos);
        app.send_slot(a as u8);
    } else {
        let step = if big { 100_000 } else { STEPS[s.cfg.step as usize % STEPS.len()].0 as i32 };
        // Plain steps from where it is (146.520 stays on its 5 kHz raster with a 12.5 kHz step); snapping to
        // the step is for typed entry (VFO-12). Kept inside what the chips tune.
        let f = (s.vfo as i32 + d * step).clamp(mhz(1) as i32 + 80_000, mhz(999) as i32) as Freq10;
        s.vfo = f;
        s.mem[band_near(f)] = f;
        app.out.push(RadioCmd::TuneSlot { slot: a as u8, freq: f });
    }
}

/// ◀▶ on home: next band in VFO mode (NAV-2), next zone with channels in channel mode (NAV-1, CHAN-11).
fn group(app: &mut App, d: i32, store: &impl ChannelStore) {
    let a = app.active as usize;
    let s = &mut app.slots[a];
    if s.on_channel() {
        let zones = store.zones() as i32;
        let mut z = s.zone as i32;
        for _ in 0..zones {
            z = (z + d).rem_euclid(zones);
            if store.zone_len(z as u8) > 0 {
                break;
            }
        }
        if z as u8 != s.zone {
            (s.zone, s.pos) = (z as u8, 0);
            s.ch = store.zone_at(s.zone, 0);
            app.send_slot(a as u8);
        }
    } else {
        let b = band_near(s.vfo);
        s.mem[b] = s.vfo;
        let nb = (b as i32 + d).rem_euclid(BANDS.len() as i32) as usize;
        s.vfo = s.mem[nb];
        s.cfg.mode = BANDS[nb].mode;
        s.cfg.step = BANDS[nb].step;
        if s.cfg.offset != 0 {
            s.cfg.offset = s.cfg.offset.signum() * BANDS[nb].shift;
        }
        app.send_slot(a as u8);
    }
}

/// V/M: VFO <-> channel (only with channels stored).
fn toggle_vfo(app: &mut App, store: &impl ChannelStore) {
    let a = app.active as usize;
    let s = &mut app.slots[a];
    if s.vfo_mode {
        if s.ch.is_none() {
            match (0..store.zones()).find(|&z| store.zone_len(z) > 0) {
                Some(z) => {
                    (s.zone, s.pos) = (z, 0);
                    s.ch = store.zone_at(z, 0);
                }
                None => return app.show_toast(b"No channels yet", b"", false),
            }
        }
        s.vfo_mode = false;
    } else {
        s.vfo_mode = true;
    }
    app.send_slot(a as u8);
}
