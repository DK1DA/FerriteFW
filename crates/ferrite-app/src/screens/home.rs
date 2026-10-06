//! Home, watch layout (manual chapter 6; HomeA mock-up): the slots stacked A, B, C, the active one large.
//! Tuning, bands and zones, SLOT, V/M, scan, squelch and the hold-digit shortcuts ([`super::nav::hold_digit`]).
//! The other two layouts (single + scope, watch + activity: HOME-3) are drawn by [`super::layouts`].

use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{self, band_near, mode_chip, shift_chip, shift_text, tone_text, Item, BANDS, STEPS};
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
    /// KEY-19: ◀▶ step the band or zone (NAV-2).
    fn own_arrows(self) -> bool {
        true
    }

    fn own_hold_menu(self) -> bool {
        true // MENU-2: advanced settings
    }

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
                super::squelch::step(app, if e.key == Key::Up { 1 } else { -1 })
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
            (Key::Slot, Long) if app.slots[app.active as usize].on_channel() => app.screen = Screen::ChEdit(super::chedit::ChEdit { cell: 0 }), // EDIT-2
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
            (Key::Side(2), Short) => super::scan::side2(app), // SCAN-1
            (Key::Star, Long) => app.change(Item::Reverse, 1), // VFO-7: two values, so +1 toggles
            (Key::Digit(d), Short) => app.open(Dest::Entry(d)), // NAV-4, VFO-11
            // HOME-8: the shortcut runs at the long press, a popup names it (KEY-11).
            (Key::Digit(d), Long) => super::nav::hold_digit(app, d, store),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        if app.keys.is_held(Key::Side(1)) {
            return super::squelch::step(app, d);
        }
        let big = app.keys.is_held(Key::Side(2));
        if big {
            app.keys.used(Key::Side(2));
        }
        step(app, d, big, store);
    }

    fn tick(self, app: &mut App, _: &ferrite_core::msg::LiveAll) {
        super::layouts::tick(app);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        // HOME-3: the other two layouts draw differently; the keys are the same.
        match app.set.layout {
            1 => return super::layouts::single(app, lcd, cx),
            2 => return super::layouts::activity(app, lcd, cx),
            _ => {}
        }
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
            let cfg = *app.slots[s as usize].cfg();
            if big && cfg.split && !lv.scanning {
                // A split's TX frequency takes the band's place in the header: its chip slot only says SPLIT.
                let n = head.as_bytes().iter().position(|&c| c == b' ').unwrap_or(head.len());
                let mut h = Buf::<32>::new();
                h.push(&head.as_bytes()[..n]);
                h.push(" \u{b7} TX ".as_bytes());
                h.push(slot::mhz3(cfg.offset.max(0) as Freq10).as_bytes());
                head = h;
            }
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
            // Channel mode puts the name first, the frequency small under it (HOME-4); a VFO, a scan or a channel
            // without a name shows the frequency large.
            let name = app.chan_name(s, live);
            if big {
                if app.dirty(R_FREQ + p, hash(&(f, name))) {
                    lcd.fill(15, y + 25, 211, 49, SURFACE);
                    if name.is_empty() {
                        w::freq(lcd, 15, 226, y + 64, f, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
                    } else {
                        // 26 px when the whole name fits (13 characters), else 22 px (16 always fit).
                        let nb = name.as_bytes();
                        let (font, base) = if text4_width(nb, &NAME_26) <= 211 { (&NAME_26, y + 51) } else { (&NAME_22, y + 49) };
                        w::field(lcd, 15, 211, y + 26, y + 57, base, Align::Left, w::fit(nb, font, 211), font, TEXT, SURFACE);
                        w::freq(lcd, 15, 226, y + 72, f, &DIG_17, &DIG_17, SUB, SUB, SURFACE);
                    }
                }
                // Chips in fixed slots (HOME-4): mode, tone, shift (with R while reversed), power pinned right. An
                // absent value leaves its slot empty; nothing moves.
                let (mut m, mut t, mut o, mut pw) = (Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new());
                mode_chip(&cfg, &mut m);
                tone_text(cfg.tx_tone, &mut t);
                shift_chip(&cfg, &mut o);
                if app.capped(s) {
                    pw.set(b"0.25 W");
                } else {
                    pw.set([b"LOW".as_slice(), b"MID", b"HIGH"][cfg.power as usize % 3]);
                }
                if app.dirty(R_INFO + p, hash(&(m, t, o, pw))) {
                    let cy = y + 76;
                    w::chip_slot(lcd, w::SLOT_MODE, cy, m.as_bytes(), TEXT, LINE, SURFACE);
                    w::chip_slot(lcd, w::SLOT_TONE, cy, t.as_bytes(), TEXT, LINE, SURFACE);
                    w::chip_slot(lcd, w::SLOT_SHIFT, cy, o.as_bytes(), TEXT, LINE, SURFACE);
                    w::chip_slot(lcd, w::SLOT_POWER, cy, pw.as_bytes(), TEXT, LINE, SURFACE);
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
                // Left: the name (channel) or the frequency; right: the frequency small (channel) or the scan's hits.
                if app.dirty(R_FREQ + p, hash(&(f, name))) {
                    lcd.fill(15, y + 22, 211, 24, SURFACE);
                    app.drawn.hash[R_INFO + p] = 0; // the right-hand text shares the row
                    if name.is_empty() {
                        w::freq(lcd, 15, 130, y + 42, f, &DIG_26, &DIG_17, TEXT, SUB, SURFACE);
                    } else {
                        // The 22 px name when it fits beside the frequency, else 15 px bold.
                        let nb = name.as_bytes();
                        let font = if text4_width(nb, &NAME_22) <= 150 { &NAME_22 } else { &UI_15B };
                        w::field(lcd, 15, 150, y + 22, y + 46, y + 41, Align::Left, w::fit(nb, font, 150), font, TEXT, SURFACE);
                    }
                }
                let mut right = Buf::<16>::new();
                if lv.scanning {
                    let _ = uwrite!(right, "{} hits", lv.hits);
                } else if !name.is_empty() {
                    right.set(w::freq3(f).as_bytes());
                }
                if app.dirty(R_INFO + p, hash(&right)) {
                    let (x, c) = if name.is_empty() { (130, TEXT) } else { (170, SUB) };
                    w::field(lcd, x, 226 - x, y + 26, y + 44, y + 42, Align::Right, right.as_bytes(), &UI_12, c, SURFACE);
                }
                // The value line: the scan's mode and last hit, or mode, tone and shift in fixed columns (HOME-4), so a
                // value coming or going never moves the others.
                let mut cols = [Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new()];
                if lv.scanning {
                    let c = &mut cols[0];
                    c.set(slot::MODES[cfg.mode as usize % 6].as_bytes());
                    if lv.hits > 0 && lv.last_hit != f {
                        let mut b = Buf::<16>::new();
                        b.set(b"last ");
                        b.push(w::freq3(lv.last_hit).as_bytes());
                        cols[1] = b;
                    }
                } else {
                    mode_chip(&cfg, &mut cols[0]);
                    tone_text(cfg.tx_tone, &mut cols[1]);
                    shift_text(&cfg, false, &mut cols[2]); // "TX 146.520" for a split
                }
                if app.dirty(R_SUB + p, hash(&(lv.scanning, &cols))) {
                    lcd.fill(15, y + 46, 211, 15, SURFACE);
                    let xs: [(i32, i32); 3] = if lv.scanning { [(15, 36), (51, 175), (226, 0)] } else { [(15, 36), (51, 56), (107, 119)] };
                    for (c, (x, wd)) in cols.iter().zip(xs) {
                        if wd > 0 && !c.is_empty() {
                            w::field(lcd, x, wd, y + 46, y + 60, y + 56, Align::Left, c.as_bytes(), &UI_12, SUB, SURFACE);
                        }
                    }
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
    let region = app.set.region();
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
        s.cfg.step = crate::slot::band_step(nb, region);
        if s.cfg.offset != 0 && !s.cfg.split {
            s.cfg.offset = s.cfg.offset.signum() * BANDS[nb].shift; // a split's TX stays where it is
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
