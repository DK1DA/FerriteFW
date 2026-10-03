//! Drawing the app's screens with `ferrite-ui`'s widgets. Each region of the screen is hashed from what it
//! shows; render draws a region only when its hash changed (HOME-7), so a meter update never touches the
//! frequency and a still screen costs nothing. Layouts follow the look review's mock-ups (`docs/img/screens`).

use crate::chan::ChannelStore;
use crate::slot::{self, band_name, chan_label, mode_chip, offset_text, tone_text, Item, GRID, QUICK};
use crate::{App, Screen};
use core::hash::{Hash, Hasher};
use ferrite_core::msg::{Device, GpsState, LiveAll, SLOTS};
use ferrite_core::VERSION;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row, StatusBar};
use ufmt::uwrite;

// Regions. Screens reuse the numbers; a screen change clears the content and forgets every hash.
const R_HINT: usize = 0;
const R_FRAME: usize = 1; // home: three cards' frames (1-3)
const R_HEAD: usize = 4; // card headers (4-6)
const R_FREQ: usize = 7; // frequencies (7-9)
const R_INFO: usize = 10; // chips, or the compact card's name (10-12)
const R_SUB: usize = 13; // S-meter text, or the compact card's second line (13-15)
const R_ROW: usize = 1; // lists and the edit grid: rows 1-13
const R_TOP: usize = 14; // menu header / Recent row / edit header
const R_HELP: usize = 15;
const R_NOTE: usize = 16;
const R_LINE: usize = 17; // edit: the other two slots (17-18)
pub(crate) const REGIONS: usize = 19;

/// Home, watch layout: the slots always stacked A, B, C; the active one is the large card, in its own place.
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

/// Advanced settings groups (MENU 0 1-9) and their help lines.
pub(crate) const GROUPS: [(&str, &str); 9] = [
    ("Watch and scan", "Watch, background jobs, scan, tone search."),
    ("Channels", "Browse channels and zones, add a repeater."),
    ("Listen", "Broadcast radio, band scope, weather alerts."),
    ("Decoders", "Event log, POCSAG, DSC, AIS, DTMF, popups."),
    ("APRS", "Heard, messages, beacons, your station."),
    ("GPS and time", "GPS, position, units, time zone, clock."),
    ("Display, sound, power", "Theme, layout, backlight, sounds, power."),
    ("Transmit", "Timeout, VOX, mic gain, DTMF, callsign, ID."),
    ("System", "Keys, links, PIN, region, receiver, reset."),
];

/// The Status page's rows (SBAR-14).
pub(crate) const STATUS: [&str; 13] = [
    "Transmit",
    "Region",
    "Extend TX",
    "Inbox",
    "Scan",
    "Watch",
    "Jobs",
    "Beacon",
    "Battery Save",
    "GPS",
    "Bluetooth",
    "Battery",
    "Key lock",
];
pub(crate) const STATUS_ROWS: i32 = STATUS.len() as i32;

/// FNV-1a, for region hashes.
struct Fnv(u32);
impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0 as u64
    }
    fn write(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u32).wrapping_mul(16_777_619);
        }
    }
}

fn hash<T: Hash>(t: &T) -> u32 {
    let mut h = Fnv(2_166_136_261);
    t.hash(&mut h);
    h.0 | 1 // never 0, which means "not drawn"
}

fn screen_id(s: Screen) -> u8 {
    match s {
        Screen::Home => 0,
        Screen::Edit { .. } => 1,
        Screen::Quick { .. } => 2,
        Screen::Advanced { .. } => 3,
        Screen::Status { .. } => 4,
    }
}

impl App {
    /// Draws what changed since the last call.
    pub fn render(&mut self, lcd: &mut impl Panel, live: &LiveAll, dev: &Device, store: &impl ChannelStore) {
        let id = screen_id(self.screen);
        if self.drawn.screen != Some(id) {
            lcd.fill(0, w::SBAR_H, 240, w::HINT_Y - w::SBAR_H, BG);
            self.drawn.hash = [0; REGIONS];
            self.drawn.meter = [None; SLOTS];
            self.drawn.screen = Some(id);
        }
        let sb = self.status_bar(live, dev);
        if self.drawn.sbar != Some(sb) {
            sb.draw(lcd, self.drawn.sbar.as_ref());
            self.drawn.sbar = Some(sb);
        }
        match self.screen {
            Screen::Home => self.render_home(lcd, live),
            Screen::Edit { cell } => self.render_edit(lcd, live, cell),
            Screen::Quick { row } => self.render_quick(lcd, row),
            Screen::Advanced { row, .. } => self.render_advanced(lcd, row, dev, store),
            Screen::Status { row, .. } => self.render_status(lcd, row, live, dev),
        }
        self.render_hint(lcd);
    }

    /// True (and remembered) when region r's content differs from what is on screen.
    fn dirty(&mut self, r: usize, h: u32) -> bool {
        let d = self.drawn.hash[r] != h;
        self.drawn.hash[r] = h;
        d
    }

    fn status_bar(&self, live: &LiveAll, dev: &Device) -> StatusBar {
        let mut name = Buf::<10>::new();
        match self.screen {
            Screen::Home => {}
            Screen::Edit { .. } => {
                let _ = uwrite!(name, "EDIT {}", w::LETTERS[self.active as usize] as char);
            }
            Screen::Quick { .. } => name.set(b"Quick"),
            Screen::Advanced { .. } => name.set(b"Advanced"),
            Screen::Status { .. } => name.set(b"Status"),
        }
        // SBAR-16: TW with three slots watched, DW with two, counting the relay rule's drops.
        let m = self.watch_mask() & !self.relay_mask;
        let n = m.count_ones() as u8;
        StatusBar {
            name,
            locked: self.locked,
            // SBAR-11: the bar shows what changes what a key does, so SCN is the active slot scanning; a
            // watched slot's scan shows on its card.
            scan: live.slots[self.active as usize].scanning,
            watch: if self.watch == 0 || n < 2 { 0 } else { n },
            inbox: self.inbox,
            alert: self.alert,
            clock: dev.clock_min,
            utc: dev.utc,
            gps: dev.gps,
            bt: dev.bt,
            battery: ((dev.battery_pct as u32 + 12) / 25).clamp(1, 4) as u8,
            battery_low: dev.battery_low,
            battery_cutoff: dev.battery_cutoff,
            blink_off: dev.battery_cutoff && (self.now / 500) % 2 == 1,
        }
    }

    // ---- home ----

    fn head_text(&self, s: u8, live: &LiveAll, b: &mut Buf<32>) {
        let sl = &self.slots[s as usize];
        let lv = &live.slots[s as usize];
        b.clear();
        if lv.scanning {
            b.push(b"SCAN");
        } else if sl.on_channel() {
            chan_label(sl.ch.map_or(0, |c| c.num), b);
        } else {
            b.push(b"VFO");
        }
        b.push(" \u{b7} ".as_bytes());
        b.push(band_name(self.shown_freq(s, live)).as_bytes());
    }

    fn shown_freq(&self, s: u8, live: &LiveAll) -> u32 {
        let lv = &live.slots[s as usize];
        if lv.scanning { lv.freq } else { self.slots[s as usize].freq() }
    }

    /// The slot's state word and colour on a card: RX, relay, off (not watched), scanning, idle.
    fn state(&self, s: u8, live: &LiveAll) -> (&'static str, u16) {
        let lv = &live.slots[s as usize];
        let watched = s == self.active || self.watch_mask() & 1 << s != 0;
        if lv.relay {
            ("relay", AMBER)
        } else if !watched {
            ("off", DIM)
        } else if lv.squelch_open {
            ("RX", GREEN)
        } else if lv.scanning {
            ("scanning", SUB)
        } else if self.paused_until.is_some() && s != self.active {
            ("paused", SUB)
        } else {
            ("idle", SUB)
        }
    }

    fn render_home(&mut self, lcd: &mut impl Panel, live: &LiveAll) {
        for s in 0..SLOTS as u8 {
            // Fixed order A, B, C: the card's region index is its slot.
            let p = s as usize;
            let (y, h) = card(s, self.active);
            let big = s == self.active;
            let lv = live.slots[s as usize];
            let (state, sc) = self.state(s, live);
            let border = if big { BLUE } else if state == "RX" { GREEN } else { LINE };
            if self.dirty(R_FRAME + p, hash(&(s, y, h, border))) {
                lcd.fill(6, y, 228, h, BG);
                lcd.fill_r(6, y, 228, h, 6, SURFACE);
                lcd.frame_r(6, y, 228, h, 6, border);
                for r in [R_HEAD, R_FREQ, R_INFO, R_SUB] {
                    self.drawn.hash[r + p] = 0;
                }
                self.drawn.meter[p] = None;
            }
            // Header: letter, VFO/CH/SCAN and band, the state at the right (HOME-4, HOME-5).
            let mut head = Buf::<32>::new();
            self.head_text(s, live, &mut head);
            // Header rows: y+7..y+24 on the big card, y+4..y+21 on the compact ones (their digits start at y+22).
            let (hy, hb) = if big { (y + 7, y + 20) } else { (y + 4, y + 17) };
            if self.dirty(R_HEAD + p, hash(&(s, head, state, big))) {
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
            let f = self.shown_freq(s, live);
            let cfg = *self.slots[s as usize].cfg();
            if big {
                if self.dirty(R_FREQ + p, hash(&f)) {
                    w::freq(lcd, 15, 226, y + 64, f, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
                }
                // Chips: mode, tone, offset, power when not default (HOME-4), reverse.
                let (mut m, mut t, mut o, mut pw) = (Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new());
                mode_chip(&cfg, &mut m);
                tone_text(cfg.tx_tone, &mut t);
                offset_text(cfg.offset, false, &mut o);
                if self.capped(s) {
                    pw.set(b"0.25 W");
                } else if cfg.power != 0 {
                    pw.set([b"MID".as_slice(), b"HIGH"][cfg.power as usize - 1]);
                }
                let rev: &[u8] = if cfg.reverse { b"REV" } else { b"" };
                if self.dirty(R_INFO + p, hash(&(m, t, o, pw, rev))) {
                    w::chips(lcd, 15, y + 72, 211, &[m.as_bytes(), t.as_bytes(), o.as_bytes(), pw.as_bytes(), rev], SURFACE);
                }
                // S-meter: only the segments that changed (HOME-6, HOME-7).
                let lit = w::meter_level(lv.rssi);
                let col = if lv.squelch_open { GREEN } else { DIM };
                let old = self.drawn.meter[p].filter(|&(_, o)| o == lv.squelch_open).map(|(l, _)| l);
                if self.drawn.meter[p] != Some((lit, lv.squelch_open)) {
                    w::meter(lcd, 14, y + 100, old, lit, col, SURFACE);
                    self.drawn.meter[p] = Some((lit, lv.squelch_open));
                }
                let st = w::s_text(lv.rssi);
                if self.dirty(R_SUB + p, hash(&st)) {
                    w::field(lcd, 212, 21, y + 94, y + 111, y + 108, Align::Right, st.as_bytes(), &UI_12B, TEXT, SURFACE);
                }
            } else {
                if self.dirty(R_FREQ + p, hash(&f)) {
                    w::freq(lcd, 15, 130, y + 42, f, &DIG_26, &DIG_17, TEXT, SUB, SURFACE);
                }
                let sl = &self.slots[s as usize];
                let mut name = Buf::<16>::new();
                if lv.scanning {
                    let _ = uwrite!(name, "{} hits", lv.hits);
                } else if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
                    name.set(c.name()); // as much as fits (CHAN-5 says 12; the mock-up shows 13)
                }
                if self.dirty(R_INFO + p, hash(&name)) {
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
                if self.dirty(R_SUB + p, hash(&sub)) {
                    w::field(lcd, 15, 211, y + 44, y + 60, y + 56, Align::Left, sub.as_bytes(), &UI_12, SUB, SURFACE);
                }
            }
        }
    }

    // ---- edit in place ----

    fn render_edit(&mut self, lcd: &mut impl Panel, live: &LiveAll, cell: u8) {
        let a = self.active;
        let mut head = Buf::<32>::new();
        self.head_text(a, live, &mut head);
        if self.dirty(R_TOP, hash(&(a, head))) {
            w::badge(lcd, 7, 26, a, true, BG);
            w::field(lcd, 21, 6, 25, 42, 38, Align::Left, b"", &UI_12, SUB, BG);
            w::field(lcd, 27, 150, 25, 42, 38, Align::Left, head.as_bytes(), &UI_12, SUB, BG);
            w::tag(lcd, 232, 25, b"EDIT", BLUE, BG); // EDIT-1
        }
        let f = self.shown_freq(a, live);
        if self.dirty(R_FREQ, hash(&f)) {
            w::freq(lcd, 7, 232, 76, f, &DIG_46, &DIG_28, TEXT, SUB, BG);
        }
        let cfg = *self.slots[a as usize].cfg();
        for (i, item) in GRID.into_iter().enumerate() {
            let mut v = Buf::<16>::new();
            if item == Item::Power && self.capped(a) {
                v.set(b"0.25 W");
            } else {
                item.text(&cfg, false, &mut v);
            }
            let focused = i as u8 == cell;
            let grey = self.greyed(item).is_some();
            if self.dirty(R_ROW + i, hash(&(item, v, focused, grey))) {
                let (x, y) = (7 + (i as i32 % 3) * 77, 85 + (i as i32 / 3) * 36);
                w::edit_cell(lcd, x, y, 72, 31, item.label().as_bytes(), v.as_bytes(), focused, grey);
            }
        }
        // The note under the grid: why the focused setting is greyed (EDIT-7), or what hold V/M does.
        let why = self.greyed(GRID[cell as usize]);
        let vfo = !self.slots[a as usize].on_channel();
        if self.dirty(R_NOTE, hash(&(why, vfo))) {
            match why {
                Some(t) => {
                    let mut b = Buf::<40>::new();
                    b.push(GRID[cell as usize].label().as_bytes());
                    b.push(b": ");
                    b.push(t.as_bytes());
                    w::field(lcd, 7, 226, 190, 207, 202, Align::Left, b.as_bytes(), &UI_12, AMBER, BG);
                }
                None if vfo => w::key_line(lcd, 7, 233, 202, "Hold V/M", "save as a channel", BG),
                None => w::key_line(lcd, 7, 233, 202, "Hold 0", "jumps to the home channel", BG),
            }
        }
        // The other two slots, one line each.
        for (k, s) in [(a + 1) % 3, (a + 2) % 3].into_iter().enumerate() {
            let mut t = Buf::<48>::new();
            t.push(w::freq3(self.shown_freq(s, live)).as_bytes());
            let sl = &self.slots[s as usize];
            let lv = &live.slots[s as usize];
            if lv.scanning {
                let _ = uwrite!(t, " \u{b7} {} \u{b7} {} hits", band_name(lv.freq), lv.hits);
            } else {
                if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
                    t.push(" \u{b7} ".as_bytes());
                    t.push(c.name());
                }
                let (st, _) = self.state(s, live);
                t.push(" \u{b7} ".as_bytes());
                t.push(st.as_bytes());
            }
            if self.dirty(R_LINE + k, hash(&(s, t))) {
                let y = 268 + k as i32 * 17;
                w::badge(lcd, 7, y, s, false, BG);
                w::field(lcd, 21, 4, y - 1, y + 16, y + 11, Align::Left, b"", &UI_12, SUB, BG);
                w::field(lcd, 25, 211, y - 1, y + 16, y + 11, Align::Left, t.as_bytes(), &UI_12, SUB, BG);
            }
        }
    }

    // ---- menus ----

    fn render_quick(&mut self, lcd: &mut impl Panel, row: u8) {
        let a = self.active;
        let sl = self.slots[a as usize];
        let mut head = Buf::<32>::new();
        if sl.on_channel() {
            chan_label(sl.ch.map_or(0, |c| c.num), &mut head);
        } else {
            head.push("VFO \u{b7} ".as_bytes());
            head.push(band_name(sl.freq()).as_bytes());
        }
        let mut m = Buf::<16>::new();
        mode_chip(sl.cfg(), &mut m);
        let f = sl.freq();
        if self.dirty(R_TOP, hash(&(a, head, m, f))) {
            // MENU-13: whose settings these are.
            w::badge(lcd, 8, 23, a, true, BG);
            w::field(lcd, 22, 4, 21, 39, 35, Align::Left, b"", &UI_12, SUB, BG);
            w::field(lcd, 26, 66, 21, 39, 35, Align::Left, head.as_bytes(), &UI_12, SUB, BG);
            let mut fb = [0u8; 16];
            let n = ferrite_core::freq::format(f, &mut fb);
            w::field(lcd, 92, 108, 21, 39, 35, Align::Left, &fb[..n], &UI_12B, TEXT, BG);
            w::field(lcd, 200, 32, 21, 39, 35, Align::Right, m.as_bytes(), &UI_12, SUB, BG);
            lcd.fill(0, 40, 240, 1, LINE);
        }
        for (i, item) in QUICK.into_iter().enumerate() {
            let mut v = Buf::<16>::new();
            self.item_text(item, true, &mut v);
            let sel = i as u8 == row;
            let grey = self.greyed(item).is_some();
            let dot = !self.is_default(item);
            if self.dirty(R_ROW + i, hash(&(item, v, sel, grey, dot))) {
                let num = ((i + 1) % 10) as u8;
                let r = Row { num: Some(num), name: item.name().as_bytes(), value: v.as_bytes(), dot, selected: sel, grey };
                w::list_row(lcd, 41 + 22 * i as i32, 22, &r);
            }
        }
        let item = QUICK[row as usize];
        let why = self.greyed(item);
        let vfo = !sl.on_channel();
        if self.dirty(R_HELP, hash(&(row, why, vfo))) {
            let mut p = Buf::<16>::new();
            let _ = uwrite!(p, "MENU {}", (row + 1) % 10);
            w::help_box(lcd, 265, 33, why.unwrap_or(item.help(vfo)).as_bytes(), p.as_bytes());
        }
    }

    fn render_advanced(&mut self, lcd: &mut impl Panel, row: u8, dev: &Device, store: &impl ChannelStore) {
        let recent = self.recent;
        if self.dirty(R_TOP, hash(&recent)) {
            w::field(lcd, 0, 54, 21, 40, 35, Align::Left, b"", &UI_12, SUB, BG);
            w::field(lcd, 8, 46, 21, 40, 35, Align::Left, b"Recent", &UI_12, SUB, BG);
            let names: [&[u8]; 3] = recent.map(|r| r.map_or(b"".as_slice(), |i| i.label().as_bytes()));
            if names[0].is_empty() {
                w::field(lcd, 54, 178, 21, 40, 35, Align::Left, b"nothing yet", &UI_12, DIM, BG);
            } else {
                lcd.fill(54, 21, 178, 1, BG);
                w::chips(lcd, 54, 22, 178, &names, BG);
                lcd.fill(54, 39, 178, 1, BG);
            }
        }
        for (i, (name, _)) in GROUPS.iter().enumerate() {
            let mut v = Buf::<24>::new();
            match i {
                0 => {
                    v.push(["Off", "A+B", "A+B+C"][self.watch as usize % 3].as_bytes());
                    if self.scan_mask != 0 {
                        v.push(" \u{b7} scan".as_bytes());
                    }
                }
                1 => {
                    let _ = uwrite!(v, "{} used", store.count());
                }
                3 => v.set(b"None running"),
                4 if self.inbox > 0 => {
                    let _ = uwrite!(v, "{} new", self.inbox);
                }
                5 => match dev.gps {
                    GpsState::Off => v.set(b"GPS off"),
                    GpsState::Searching => v.set(b"Searching"),
                    GpsState::Fix => {
                        let _ = uwrite!(v, "{} sats", dev.sats);
                    }
                },
                6 => v.set(b"Dark"),
                7 => v.set(if dev.tx_built { b"Ready".as_slice() } else { b"Receive only" }),
                8 => v.set(VERSION.as_bytes()),
                _ => {}
            }
            let sel = i as u8 == row;
            if self.dirty(R_ROW + i, hash(&(i, v, sel))) {
                let r = Row { num: Some(i as u8 + 1), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: sel, grey: false };
                w::list_row(lcd, 41 + 22 * i as i32, 22, &r);
            }
        }
        if self.dirty(R_HELP, hash(&row)) {
            let mut p = Buf::<16>::new();
            let _ = uwrite!(p, "MENU 0 {}", row + 1);
            w::help_box(lcd, 242, 34, GROUPS[row as usize].1.as_bytes(), p.as_bytes());
        }
    }

    fn render_status(&mut self, lcd: &mut impl Panel, row: u8, live: &LiveAll, dev: &Device) {
        for (i, label) in STATUS.iter().enumerate() {
            let mut v = Buf::<28>::new();
            let mut c = TEXT;
            match i {
                0 => {
                    // SBAR-14: why transmit is locked, if it is.
                    if dev.tx_built {
                        (v, c) = (Buf::from(b"Ready"), GREEN);
                    } else {
                        (v, c) = (Buf::from(b"RX-only build"), AMBER);
                    }
                }
                1 => v.set(b"Not set"),
                2 | 6 | 7 => v.set(if i == 6 { b"None".as_slice() } else { b"Off" }),
                3 => {
                    if self.inbox == 0 {
                        v.set(b"Empty");
                    } else {
                        let _ = uwrite!(v, "{} unread", self.inbox);
                    }
                }
                4 => {
                    for (s, l) in live.slots.iter().enumerate().filter(|(_, l)| l.scanning) {
                        if !v.is_empty() {
                            v.push(b", ");
                        }
                        let _ = uwrite!(v, "{}: {}", w::LETTERS[s] as char, band_name(l.freq));
                    }
                    if v.is_empty() {
                        v.set(b"Off");
                    }
                }
                5 => {
                    v.set(["Off", "A+B", "A+B+C"][self.watch as usize % 3].as_bytes());
                    if self.paused_until.is_some() {
                        v.push(" \u{b7} paused".as_bytes());
                    }
                }
                8 => {
                    let mut b = Buf::<16>::new();
                    self.item_text(Item::BatterySave, true, &mut b);
                    v.set(b.as_bytes());
                }
                9 => match dev.gps {
                    GpsState::Off => v.set(b"Off"),
                    GpsState::Searching => v.set(b"Searching"),
                    GpsState::Fix => {
                        let _ = uwrite!(v, "Fix \u{b7} {} sats", dev.sats);
                    }
                },
                10 => v.set(if dev.bt { b"Connected".as_slice() } else { b"Off" }),
                11 => {
                    let mv = dev.battery_mv as u32;
                    let _ = uwrite!(v, "{}.{} V \u{b7} {}%", mv / 1000, mv % 1000 / 100, dev.battery_pct);
                    if dev.battery_cutoff {
                        c = RED;
                    } else if dev.battery_low {
                        c = AMBER;
                    }
                }
                _ => v.set(if self.locked { b"On".as_slice() } else { b"Off" }),
            }
            let sel = i as u8 == row;
            if self.dirty(R_ROW + i, hash(&(i, v, c, sel))) {
                w::kv_row(lcd, 20 + 18 * i as i32, 18, label.as_bytes(), v.as_bytes(), c, sel);
            }
        }
    }

    // ---- hint bar ----

    fn render_hint(&mut self, lcd: &mut impl Panel) {
        if let Some(t) = self.toast {
            if self.dirty(R_HINT, hash(&(1u8, t.head, t.body, t.icon))) {
                w::toast(lcd, t.head.as_bytes(), t.body.as_bytes(), t.icon);
            }
            return;
        }
        const ARROWS_LR: &str = "\x1b\x1a";
        const ARROWS_UD: &str = "\x18\x19";
        const ARROWS_4: &str = "\x1b\x18\x19\x1a";
        let undo = self.undo.is_some();
        let items: &[(&str, &str)] = match self.screen {
            Screen::Home if self.locked => &[("#", "Hold: unlock")],
            Screen::Home => {
                let group = if self.slots[self.active as usize].on_channel() { "Zone" } else { "Band" };
                let mid = if self.watched_rx() { ("#", "Pause") } else { ("OK", "Quick") };
                &[("SLOT", "A\u{b7}B\u{b7}C"), mid, (ARROWS_LR, group)]
            }
            // Four pairs don't fit at 12 px where the mock-ups' smaller text has them; the knob always stays
            // named (MENU-16) and "* Undo" takes a place while it is offered (EDIT-4, MENU-22).
            Screen::Edit { .. } if undo => &[("Knob", "Change"), ("*", "Undo"), ("BACK", "Done")],
            Screen::Edit { .. } => &[(ARROWS_4, "Pick"), ("Knob", "Change"), ("BACK", "Done")],
            Screen::Quick { .. } if undo => &[("Knob", "Change"), ("*", "Undo"), ("BACK", "Home")],
            Screen::Quick { row: 9 } => &[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Home")],
            Screen::Quick { .. } => &[(ARROWS_UD, "Move"), ("Knob", "Change"), ("BACK", "Home")],
            Screen::Advanced { .. } => &[(ARROWS_UD, "Move"), ("1-9", "Jump"), ("OK", "Open"), ("BACK", "Back")],
            Screen::Status { .. } => &[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")],
        };
        if self.dirty(R_HINT, hash(&(0u8, items))) {
            w::hint_bar(lcd, items);
        }
    }
}
