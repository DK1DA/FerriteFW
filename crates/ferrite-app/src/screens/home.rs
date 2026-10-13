//! Home, watch layout (manual chapter 6; HomeA mock-up): the slots stacked A, B, C, the active one large.
//! Tuning, bands and zones, SLOT, V/M, scan, squelch and the hold-digit shortcuts ([`super::nav::hold_digit`]).
//! The other two layouts (single + scope, watch + activity: HOME-3) are drawn by [`super::layouts`].

use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::Data;
use crate::keys::{KeyEvent, Press};
use crate::slot::{self, band_near, mode_chip, shift_chip, shift_text, tone_text, Item, BANDS, STEPS};
use crate::{App, Screen, PAUSE_S};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_core::msg::{RadioCmd, SLOTS};
use ferrite_ui::assets::*;
use ferrite_ui::canvas;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Home has no state of its own: everything it shows is the App's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Home;

/// Watch layout: the slots always stacked A, B, C; the active one is the large card, in its own place.
/// The three cards fill the content area with 5 px clear above the hint bar, as below the status bar (HOME-11).
const CARD_TOP: i32 = 25;
const CARD_BIG: i32 = 124;
const CARD_SMALL: i32 = 67;
const CARD_GAP: i32 = 6;
/// Top of the big card's chip row, from the card's top.
pub(crate) const CHIP_Y: i32 = 82;

/// Card position (y, height) of slot `s` when `active` is the large one (HOME-11).
pub(crate) fn card(s: u8, active: u8) -> (i32, i32) {
    let h = |i: u8| if i == active { CARD_BIG } else { CARD_SMALL };
    let y = CARD_TOP + (0..s).map(|i| h(i) + CARD_GAP).sum::<i32>();
    (y, h(s))
}

/// APRS-17: slot C in APRS mode is the APRS card: the frequency, stations heard, the last station or message and
/// the beacon ("beacon waiting: HF" while the relay is the other way, APRS-20). Below the header, which says APRS.
fn aprs_card(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, s: u8, y: i32, big: bool) {
    let p = s as usize;
    let f = app.aprs_freq();
    let mut heard = Buf::<16>::new();
    let _ = uwrite!(heard, "{} heard", data.stations());
    let mut last = Buf::<80>::new();
    crate::aprs::last_text(data, &mut last);
    let mut beacon = Buf::<28>::new();
    app.beacon_text(data, &mut beacon);
    let waiting = data.beacon().waiting;
    app.drawn.meter[p] = None;
    if big {
        if let Some(clear) = app.dirty_shape(R_FREQ + p, hash(&(f, 1u8)), true) {
            redraw_box!(clear, lcd, (15, y + 28, 211, 49, SURFACE), {
                w::freq(lcd, 15, 226, y + 67, f, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
            });
        }
        if app.dirty(R_INFO + p, hash(&last)) {
            w::field(lcd, 15, 211, y + 80, y + 98, y + 94, Align::Left, w::fit(last.as_bytes(), &UI_12, 211), &UI_12, TEXT, SURFACE);
        }
        if app.dirty(R_SUB + p, hash(&(&beacon, &heard, waiting))) {
            lcd.fill(14, y + 102, 212, 20, SURFACE);
            let c = if waiting { AMBER } else { SUB };
            w::field(lcd, 15, 140, y + 103, y + 120, y + 117, Align::Left, beacon.as_bytes(), &UI_12, c, SURFACE);
            w::field(lcd, 155, 71, y + 103, y + 120, y + 117, Align::Right, heard.as_bytes(), &UI_12B, TEXT, SURFACE);
        }
    } else {
        if let Some(clear) = app.dirty_shape(R_FREQ + p, hash(&(f, &heard, 1u8)), true) {
            redraw_box!(clear, lcd, (15, y + 25, 211, 24, SURFACE), {
                w::freq(lcd, 15, 130, y + 45, f, &DIG_26, &DIG_17, TEXT, SUB, SURFACE);
                w::field(lcd, 130, 96, y + 29, y + 47, y + 45, Align::Right, heard.as_bytes(), &UI_12, TEXT, SURFACE);
            });
        }
        let (line, c) = if waiting { (beacon.as_bytes(), AMBER) } else { (last.as_bytes(), SUB) };
        if app.dirty(R_SUB + p, hash(&(line, c))) {
            ferrite_ui::canvas::banded(lcd, 15, y + 51, 211, 15, SURFACE, |lcd| {
                w::field(lcd, 15, 211, y + 51, y + 65, y + 61, Align::Left, w::fit(line, &UI_12, 211), &UI_12, c, SURFACE);
            });
        }
    }
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
        // PIN-5: channel-only mode: no V/M, editing, settings or MENU 0.
        // A digit on a VFO slot would open frequency entry, which channel-only mode hides too.
        if (matches!(
            (e.key, e.press),
            (Key::VfoMem, _) | (Key::Slot | Key::Menu, Long) | (Key::Menu | Key::Star, Short) | (Key::Star, Long) | (Key::Side(2), LongUp)
        ) || matches!((e.key, e.press), (Key::Digit(_), Short)) && !app.slots[app.active as usize].on_channel())
            && app.chan_only_refuse()
        {
            return;
        }
        // APRS-19: the APRS slot can be looked at, not changed; APRS-17: hold MENU on it is its options.
        if app.aprs_slot(app.active) {
            match (e.key, e.press) {
                (Key::Menu, Long) => return app.open_screen(super::opts::Of::AprsCard.screen()),
                // the shortcuts that don't touch the slot (scope, watch) still run
                (Key::Digit(1 | 4), Long) => {}
                (Key::Up | Key::Down | Key::Left | Key::Right, Short | Repeat)
                | (Key::Slot | Key::VfoMem | Key::Digit(_), Long)
                | (Key::VfoMem | Key::Side(2) | Key::Digit(_), Short) => {
                    app.aprs_refuse(app.active);
                    return;
                }
                _ => {}
            }
        }
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
            // VFO-22: SIDE1 + ◀▶ is the clarifier (SIDE1 + ▲▼ the squelch: SIDE1 holds the receive side)
            (Key::Left | Key::Right, Short | Repeat) if app.keys.is_held(Key::Side(1)) => {
                app.keys.used(Key::Side(1));
                app.step_clarifier(if e.key == Key::Right { 1 } else { -1 });
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
                    app.open(Dest::SaveChannel); // VFO-9
                } else {
                    copy_to_vfo(app); // VFO-23
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
            // KEY-22: SIDE2 held alone opens the frequency counter when it comes up; SIDE2 + knob or ▲▼ (big
            // steps) never does, whenever the knob turns.
            (Key::Side(2), LongUp) => app.open(Dest::Counter),
            (Key::Side(1), Short) => {
                // A tap latches monitor on the active slot until tapped again; its card says MON.
                let a = app.active;
                app.monitor ^= 1 << a;
                let on = app.monitor & 1 << a != 0;
                app.out.push(RadioCmd::Monitor { slot: a, on });
                app.show_toast(b"Monitor", if on { b"on: tap SIDE1 again" } else { b"off" }, false);
            }
            (Key::Star, Long) => app.change(Item::Reverse, 1), // VFO-7: two values, so +1 toggles
            (Key::Digit(d), Short) => app.open(Dest::Entry(d)), // NAV-4, VFO-11
            // HOME-8: the shortcut runs at the long press, a popup names it (KEY-11).
            (Key::Digit(d), Long) => super::nav::hold_digit(app, d, store),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        if app.aprs_refuse(app.active) {
            return; // APRS-19
        }
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
        // The large card moved (SLOT): the gaps between cards moved too, so clear them and draw every card again;
        // otherwise slivers of the old large card stay behind (HOME-11). Only the gaps: each card paints its own
        // rectangle (clearing the whole area first was one of five passes over it, slow on the radio's bus).
        if app.dirty(R_NOTE, hash(&app.active)) {
            lcd.fill(0, 21, 6, 279, BG);
            lcd.fill(234, 21, 6, 279, BG);
            let mut top = 21;
            for s in 0..SLOTS as u8 {
                let (y, h) = card(s, app.active);
                lcd.fill(6, top, 228, y - top, BG);
                top = y + h;
            }
            lcd.fill(6, top, 228, 300 - top, BG);
            app.drawn.hash[R_FRAME..=R_SUB + 2].fill(0);
            app.drawn.meter = [None; SLOTS];
        }
        for s in 0..SLOTS as u8 {
            let p = s as usize;
            let (y, h) = card(s, app.active);
            let geom = hash(&(s, y, h, app.aprs_slot(s))) | 1;
            if app.drawn.hash[R_FRAME + p] != 0 && app.drawn.card[p] == geom {
                draw_card(app, lcd, cx, s);
                continue;
            }
            // DISP-25: a card in a new place or size (SLOT moved the large card, a scan began) is drawn whole in
            // bands, frame and content, so the panel goes from the old picture to the new with no blank card
            // between. Each band runs the card's drawing afresh: what it has drawn is forgotten before each.
            let (hash0, meter0) = (app.drawn.hash, app.drawn.meter);
            canvas::banded(lcd, 6, y, 228, h, BG, |b| {
                (app.drawn.hash, app.drawn.meter) = (hash0, meter0);
                draw_card(app, b, cx, s);
            });
        }
    }

    fn hints(self, app: &App) -> Hints {
        if app.locked {
            return Hints::new(&[("#", "Hold: unlock")]);
        }
        let group = if app.slots[app.active as usize].on_channel() { "Zone" } else { "Band" };
        let mid = if app.watched_rx() { ("#", "Pause") } else { ("OK", "Quick") };
        if app.aprs_slot(app.active) {
            return Hints::new(&[("SLOT", "A\u{b7}B\u{b7}C"), mid, ("Hold MENU", "APRS")]); // APRS-17
        }
        Hints::new(&[("SLOT", "A\u{b7}B\u{b7}C"), mid, (ARROWS_LR, group)])
    }
}

/// Card `s` of the cards layout: its frame when it changed, and every part of it that changed.
fn draw_card(app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>, s: u8) {
    let live = cx.live;
    // Fixed order A, B, C: the card's region index is its slot.
    let p = s as usize;
    let (y, h) = card(s, app.active);
    let big = s == app.active;
    let lv = live.slots[s as usize];
    let (state, sc) = app.state(s, live);
    let border = if big { BLUE } else if state == "RX" { GREEN } else { LINE };
    let aprs = app.aprs_slot(s);
    let geom = hash(&(s, y, h, aprs)) | 1;
    let drawn_before = app.drawn.hash[R_FRAME + p] != 0;
    if !app.dirty(R_FRAME + p, hash(&(s, y, h, border, aprs))) {
        // unchanged
    } else if drawn_before && app.drawn.card[p] == geom {
        // Only the border's colour changed (RX, idle): redraw the border alone. Redrawing the whole card blanked
        // its content, which flickered on the radio (DISP-25).
        lcd.frame_r(6, y, 228, h, 3, border);
    } else {
        app.drawn.card[p] = geom;
        // The background only where the rounded corners leave it showing (3 px squares), not the whole card.
        for (cx, cy) in [(6, y), (231, y), (6, y + h - 3), (231, y + h - 3)] {
            lcd.fill(cx, cy, 3, 3, BG);
        }
        lcd.fill_r(6, y, 228, h, 3, SURFACE);
        lcd.frame_r(6, y, 228, h, 3, border);
        for r in [R_HEAD, R_FREQ, R_INFO, R_SUB] {
            app.drawn.hash[r + p] = 0;
        }
        app.drawn.hash[R_FREQ + p] = FRESH; // the frequency box is plain surface: no blank needed
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
    // Header rows: y+7..y+24 on the big card, y+5..y+22 on the compact ones (their digits start at y+25).
    let (hy, hb) = if big { (y + 7, y + 20) } else { (y + 5, y + 18) };
    // Beside the state (left of an RX or MON tag): the clarifier (VFO-22), or on the APRS card that it takes
    // turns on chip 0 with A or B, so a packet can be missed (APRS-21).
    let mut clr = Buf::<16>::new();
    let c = app.slots[s as usize].clar;
    if aprs && app.aprs_shared().is_some() {
        clr.set(b"VHF shared");
    } else if c != 0 && !lv.scanning {
        slot::clar_text(c, &mut clr);
    }
    // TSRCH-13: a tone hint takes the state's place, in a wider field.
    let heard = state == "heard";
    let mut ht = Buf::<16>::new();
    if heard {
        heard_text(lv.heard_tone, &mut ht);
    }
    if app.dirty(R_HEAD + p, hash(&(s, head, state, big, clr, &ht))) {
        w::badge(lcd, 14, hy + 1, s, big, SURFACE);
        lcd.fill(28, hy, 6, 17, SURFACE);
        let hw = if heard { 226 - HEARD_W - 34 } else { 120 };
        w::field(lcd, 34, hw, hy, hy + 17, hb, Align::Left, w::fit(head.as_bytes(), &UI_12, hw), &UI_12, SUB, SURFACE);
        if heard {
            let top = if big { y + 6 } else { hy };
            ferrite_ui::canvas::banded(lcd, 226 - HEARD_W, top, HEARD_W + 2, 18, SURFACE, |lcd| {
                w::field(lcd, 226 - HEARD_W, HEARD_W, hy, hy + 17, hb, Align::Right, ht.as_bytes(), &UI_12, sc, SURFACE);
            });
        } else if big {
            ferrite_ui::canvas::banded(lcd, 154, y + 6, 74, 18, SURFACE, |lcd| {
                let tagged = state == "RX" || state == "MON";
                if tagged {
                    w::tag(lcd, 226, y + 7, state.as_bytes(), sc, SURFACE);
                } else if state != "idle" {
                    w::field(lcd, 154, 72, y + 7, y + 24, y + 20, Align::Right, state.as_bytes(), &UI_12, sc, SURFACE);
                }
                if !clr.is_empty() && (tagged || state == "idle") {
                    let right = if tagged { 194 } else { 226 };
                    w::field(lcd, 154, right - 154, y + 7, y + 24, y + 20, Align::Right, clr.as_bytes(), &UI_12, AMBER, SURFACE);
                }
            });
        } else {
            let f = if state == "RX" { &UI_12B } else { &UI_12 };
            let (t, c): (&[u8], u16) = if state == "idle" && !clr.is_empty() { (clr.as_bytes(), AMBER) } else { (state.as_bytes(), sc) };
            w::field(lcd, 154, 72, hy, hy + 17, hb, Align::Right, t, f, c, SURFACE);
        }
    }
    if aprs {
        aprs_card(app, lcd, cx.store.data(), s, y, big);
        return;
    }
    let f = app.shown_freq(s, live);
    // Channel mode puts the name first, the frequency small under it (HOME-4); a VFO, a scan or a channel
    // without a name shows the frequency large.
    let name = app.chan_name(s, live);
    if big {
        if let Some(clear) = app.dirty_shape(R_FREQ + p, hash(&(f, name)), name.is_empty()) {
            redraw_box!(clear, lcd, (15, y + 28, 211, 49, SURFACE), {
                if name.is_empty() {
                    w::freq(lcd, 15, 226, y + 67, f, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
                } else {
                    // 26 px when the whole name fits (13 characters), else 22 px (16 always fit).
                    let nb = name.as_bytes();
                    let (font, base) = if text4_width(nb, &NAME_26) <= 211 { (&NAME_26, y + 54) } else { (&NAME_22, y + 52) };
                    w::field(lcd, 15, 211, y + 29, y + 60, base, Align::Left, w::fit(nb, font, 211), font, TEXT, SURFACE);
                    w::freq(lcd, 15, 226, y + 75, f, &DIG_17, &DIG_17, SUB, SUB, SURFACE);
                }
            });
        }
        // Chips in fixed slots (HOME-4): mode, tone, shift (with R while reversed), power pinned right. An
        // absent value leaves its slot empty; nothing moves.
        let (mut m, mut t, mut o, mut pw) = (Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new());
        mode_chip(&cfg, &mut m);
        tone_text(cfg.sent_tone(), &mut t);
        shift_chip(&cfg, &mut o);
        if app.capped(s) {
            pw.set(b"0.25 W");
        } else {
            pw.set([b"LOW".as_slice(), b"MID", b"HIGH"][cfg.power as usize % 3]);
        }
        if app.dirty(R_INFO + p, hash(&(m, t, o, pw))) {
            let cy = y + CHIP_Y;
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
            w::meter(lcd, 14, y + 109, old, lit, col, SURFACE);
            app.drawn.meter[p] = Some((lit, lv.squelch_open));
        }
        let st = w::s_text(lv.rssi);
        if app.dirty(R_SUB + p, hash(&st)) {
            w::field(lcd, 212, 21, y + 103, y + 120, y + 117, Align::Right, st.as_bytes(), &UI_12B, TEXT, SURFACE);
        }
    } else {
        // Left: the name (channel) or the frequency; right: the frequency small (channel) or the scan's hits.
        let mut right = Buf::<16>::new();
        if lv.scanning {
            let _ = uwrite!(right, "{} hits", lv.hits);
        } else if !name.is_empty() {
            right.set(w::freq3(f).as_bytes());
        }
        let (x, c) = if name.is_empty() { (130, TEXT) } else { (170, SUB) };
        let info = app.dirty(R_INFO + p, hash(&right));
        let freq = app.dirty_shape(R_FREQ + p, hash(&(f, name)), name.is_empty());
        if let Some(clear) = freq {
            // A new layout redraws the row in bands with the right-hand text, which shares it.
            redraw_box!(clear, lcd, (15, y + 25, 211, 24, SURFACE), {
                if name.is_empty() {
                    w::freq(lcd, 15, 130, y + 45, f, &DIG_26, &DIG_17, TEXT, SUB, SURFACE);
                } else {
                    // The 22 px name when it fits beside the frequency, else 15 px bold.
                    let nb = name.as_bytes();
                    let font = if text4_width(nb, &NAME_22) <= 150 { &NAME_22 } else { &UI_15B };
                    w::field(lcd, 15, 150, y + 25, y + 49, y + 44, Align::Left, w::fit(nb, font, 150), font, TEXT, SURFACE);
                }
                if clear {
                    w::field(lcd, x, 226 - x, y + 29, y + 47, y + 45, Align::Right, right.as_bytes(), &UI_12, c, SURFACE);
                }
            });
        }
        if info && freq != Some(true) {
            w::field(lcd, x, 226 - x, y + 29, y + 47, y + 45, Align::Right, right.as_bytes(), &UI_12, c, SURFACE);
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
            tone_text(cfg.sent_tone(), &mut cols[1]);
            shift_text(&cfg, false, &mut cols[2]); // "TX 146.520" for a split
        }
        if app.dirty(R_SUB + p, hash(&(lv.scanning, &cols))) {
            // Off-screen, sent once (DISP-25).
            canvas::banded(lcd, 15, y + 51, 211, 15, SURFACE, |b| {
                let xs: [(i32, i32); 3] = if lv.scanning { [(15, 36), (51, 175), (226, 0)] } else { [(15, 36), (51, 56), (107, 119)] };
                for (c, (x, wd)) in cols.iter().zip(xs) {
                    if wd > 0 && !c.is_empty() {
                        w::field(b, x, wd, y + 51, y + 65, y + 61, Align::Left, c.as_bytes(), &UI_12, SUB, SURFACE);
                    }
                }
            });
        }
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
        let mut p = (s.pos as i32 + d * if big { 10 } else { 1 }).rem_euclid(n);
        // CHAN-26: HF channels are HF mode's; stepping goes past them
        for _ in 0..n {
            if store.zone_at(s.zone, p as u16).is_some_and(|c| !c.is_hf()) {
                break;
            }
            p = (p + d.signum()).rem_euclid(n);
        }
        s.pos = p as u16;
        s.ch = store.zone_at(s.zone, s.pos);
        app.send_slot(a as u8);
    } else {
        let step = if big { 100_000 } else { STEPS[s.cfg.step as usize % STEPS.len()].0 as i32 };
        // Plain steps from where it is (146.520 stays on its 5 kHz raster with a 12.5 kHz step); snapping to
        // the step is for typed entry (VFO-12). Kept inside what the chips tune.
        let f = (s.vfo as i32 + d * step).clamp(mhz(1) as i32 + 80_000, mhz(1300) as i32 - 1) as Freq10;
        (s.vfo, s.clar) = (f, 0); // VFO-22: a retune clears the clarifier
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
            if first_slot_channel(store, z as u8).is_some() {
                break;
            }
        }
        if let Some(p) = first_slot_channel(store, z as u8).filter(|_| z as u8 != s.zone) {
            (s.zone, s.pos) = (z as u8, p);
            s.ch = store.zone_at(s.zone, p);
            app.send_slot(a as u8);
        }
    } else {
        let b = band_near(s.vfo);
        s.mem[b] = s.vfo;
        let nb = (b as i32 + d).rem_euclid(BANDS.len() as i32) as usize;
        s.vfo = s.mem[nb];
        s.cfg.mode = crate::slot::band_mode(nb, region);
        s.cfg.narrow = s.cfg.mode == 1;
        s.cfg.step = crate::slot::band_step(nb, region);
        if s.cfg.offset != 0 && !s.cfg.split {
            s.cfg.offset = s.cfg.offset.signum() * BANDS[nb].shift; // a split's TX stays where it is
        }
        app.send_slot(a as u8);
    }
}

/// V/M: VFO <-> channel (only with channels stored).
/// VFO-23 (nicFW's "Chan 2 VFO"): hold V/M on a channel puts the slot's VFO on it, frequency and settings
/// (shift or split, tones, mode, power, step, busy lock), and switches to the VFO. The channel is untouched.
fn copy_to_vfo(app: &mut App) {
    let a = app.active as usize;
    let s = &mut app.slots[a];
    let Some(c) = s.ch.filter(|_| !s.vfo_mode) else { return };
    let mut cfg = c.cfg;
    if cfg.squelch == crate::slot::SQ_GLOBAL {
        cfg.squelch = s.cfg.squelch; // "Global" means the VFO's own level (SQL-6)
    }
    (s.vfo, s.cfg, s.vfo_mode) = (c.rx, cfg, true);
    app.send_slot(a as u8);
    app.show_toast(b"Copied to VFO", b"V/M: channel", false);
}

/// The first channel in a zone a home slot can sit on: not an HF channel (CHAN-26).
fn first_slot_channel(store: &impl ChannelStore, z: u8) -> Option<u16> {
    (0..store.zone_len(z)).find(|&i| store.zone_at(z, i).is_some_and(|c| !c.is_hf()))
}

fn toggle_vfo(app: &mut App, store: &impl ChannelStore) {
    let a = app.active as usize;
    let s = &mut app.slots[a];
    if s.vfo_mode {
        if s.ch.is_none() {
            match (0..store.zones()).find_map(|z| Some(z).zip(first_slot_channel(store, z))) {
                Some((z, p)) => {
                    (s.zone, s.pos) = (z, p);
                    s.ch = store.zone_at(z, p);
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
