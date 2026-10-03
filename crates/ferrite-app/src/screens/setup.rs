//! First-run setup (first boot, MENU 0 9 8 1; manual chapter 4, SETUP-1 to SETUP-12; SetupCall, SetupRegion
//! and SetupSummary mock-ups): eight pages, one question each, every one with a default, so OK on each gives a
//! working radio. OK next, BACK previous, # skips the rest keeping the defaults. Each answer is the menu's own
//! setting (SETUP-5).

use super::extend;
use super::forms::{dashes, field_box, para};
use super::shared::*;
use super::text::{self, Text, TextFor};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::prefs::{tz_text, LAYOUTS, NO_REGION, REGIONS, THEMES, TZ_MAX, TZ_MIN, UNITS};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::RadioCmd;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Setup {
    /// 0-7: pages 1-8.
    pub page: u8,
    pub row: u8,
    /// Callsign typing: 0 letters, 2 digits (text entry's modes), and the multi-tap key and time.
    pub mode: u8,
    pub last: u8,
    pub at: u32,
}

pub const PAGES: u8 = 8;

/// Each page's heading and what it is for.
const PAGE: [(&str, &str); 8] = [
    ("Welcome to Ferrite", "A few questions, one per page. Each has a default, so OK on every page gives a working radio. All of it can be changed later in the menu."),
    ("Your callsign", "Used for APRS, CW ID and the power-on message. Type it with the digit keys, or leave it empty and set it later (MENU 0 8 7 1)."),
    ("Region", "Sets your amateur transmit bands, and defaults like wide FM and weather alerts."),
    ("Owner card", "Shown at power on, so a lost radio finds its way home. Optional; after setup it is edited in the web app only."),
    ("Time", "With a GPS fix, the time zone comes from it."),
    ("Look", "How home is laid out, and the colours. Every layout keeps the same keys."),
    ("Battery Save", "The receiver sleeps between checks: deeper saves more, but hears the start of a call later."),
    ("All set", ""),
];

/// Battery Save levels and their trade-off (page 7).
const BSAVE: [&str; 4] = [
    "Off: always listening, the most current.",
    "Level 1: a short sleep, barely heard.",
    "Level 2: about twice the standby time.",
    "Level 3: the longest standby; misses the start of calls.",
];

/// The Look page's three layout previews: card rectangles (x, y, w, h) in a 60 x 76 thumbnail.
const PREVIEWS: [&[(i32, i32, i32, i32)]; 3] = [
    &[(4, 4, 52, 32), (4, 40, 52, 15), (4, 58, 52, 15)],
    &[(4, 4, 52, 44), (4, 52, 52, 20)],
    &[(4, 4, 52, 21), (4, 28, 52, 21), (4, 52, 52, 21)],
];

impl Setup {
    pub fn new() -> Self {
        Setup { page: 0, row: 0, mode: 0, last: 0, at: 0 }
    }

    fn rows(self) -> u8 {
        match self.page {
            2 => REGIONS.len() as u8 + 1,
            3 | 5 => 2,
            4 => 3,
            _ => 1,
        }
    }

    fn go(self, app: &mut App, page: u8) {
        let row = if page == 2 && app.prefs.region != NO_REGION { app.prefs.region } else { 0 };
        app.screen = Screen::Setup(Setup { page, row, mode: 0, last: 0, at: 0 });
    }

    /// SETUP-3: a page left with OK keeps its default.
    fn next(self, app: &mut App) {
        if self.page == 2 && app.prefs.region == NO_REGION {
            app.prefs.region = self.row.min(REGIONS.len() as u8 - 1);
        }
        if self.page + 1 < PAGES {
            self.go(app, self.page + 1);
        } else {
            finish(app);
        }
    }
}

impl Default for Setup {
    fn default() -> Self {
        Self::new()
    }
}

/// Setup ends: home, with every answer kept.
fn finish(app: &mut App) {
    if app.prefs.region == NO_REGION {
        app.prefs.region = REGIONS.len() as u8 - 1; // "Other": no region means no amateur bands but the common ones
    }
    app.go_home();
    app.show_toast(b"Setup done", b"MENU 0 9 8 1 runs it again", false);
}

impl View for Setup {
    fn name(self) -> &'static str {
        "Setup"
    }

    fn mockup(self) -> Option<&'static str> {
        match self.page {
            1 => Some("SetupCall.png"),
            2 => Some("SetupRegion.png"),
            7 => Some("SetupSummary.png"),
            _ => None,
        }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        let _ = uwrite!(b, "Setup {}/{}", self.page + 1, PAGES);
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        let n = s.rows();
        match (e.key, e.press) {
            (Key::Menu, Short) => return s.next(app), // SETUP-4
            (Key::Hash, Short) => return finish(app),
            (Key::Exit, Short) if s.page == 1 && !app.prefs.callsign().is_empty() => {
                let mut b = Buf::<16>::from(app.prefs.callsign());
                text::delete(&mut b);
                set_call(app, b.as_bytes());
                s.last = 0;
            }
            (Key::Exit, Short) if s.page == 0 => return app.back(),
            (Key::Exit, Short) => return s.go(app, s.page - 1),
            (Key::Up, Short | Repeat) => {
                s.row = (s.row + n - 1) % n;
                if s.page == 2 && s.row < REGIONS.len() as u8 {
                    app.prefs.region = s.row;
                }
            }
            (Key::Down, Short | Repeat) => {
                s.row = (s.row + 1) % n;
                if s.page == 2 && s.row < REGIONS.len() as u8 {
                    app.prefs.region = s.row;
                }
            }
            (Key::Left | Key::Right, Short | Repeat) => return s.knob(app, if e.key == Key::Right { 1 } else { -1 }, &crate::chan::NoChannels),
            (Key::Star, Short) if s.page == 1 => s.mode = if s.mode == 0 { 2 } else { 0 },
            (Key::Digit(d), Short) if s.page == 1 => {
                let mut b = Buf::<16>::from(app.prefs.callsign());
                (s.last, s.at) = text::tap(&mut b, 10, s.mode, d, s.last, s.at, app.now);
                set_call(app, b.as_bytes());
            }
            (Key::Digit(d), Short) if s.page == 3 => {
                // A digit starts typing the focused field (A7), with that key.
                let (to, cur) = if s.row == 0 { (TextFor::OwnerName, app.prefs.owner_name()) } else { (TextFor::OwnerContact, app.prefs.owner_contact()) };
                app.text.set(cur);
                app.screen = Screen::Setup(s);
                app.open(super::nav::Dest::Text);
                let mut t = Text::new(to);
                (t.last, t.at) = text::tap(&mut app.text, to.max(), 0, d, 0, 0, app.now);
                app.screen = Screen::Text(t);
                return;
            }
            _ => return,
        }
        app.screen = Screen::Setup(s);
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let mut s = self;
        let p = &mut app.prefs;
        let step = |v: u8, n: usize| (v as i32 + d).rem_euclid(n as i32) as u8;
        match (s.page, s.row) {
            (2, r) if r < REGIONS.len() as u8 => {
                s.row = step(r, REGIONS.len());
                p.region = s.row;
            }
            (2, _) => {
                app.screen = Screen::Setup(s);
                return extend::toggle(app);
            }
            (4, 0) => p.tz = (p.tz as i32 + d).clamp(TZ_MIN as i32, TZ_MAX as i32) as i8,
            (4, 1) => p.clock_12h = !p.clock_12h,
            (4, _) => p.units = step(p.units, UNITS.len()),
            (5, 0) => p.layout = step(p.layout, LAYOUTS.len()),
            (5, _) => p.theme = step(p.theme, THEMES.len()),
            (6, _) => {
                app.bsave = step(app.bsave, 4);
                app.out.push(RadioCmd::BatterySave(crate::level(app.bsave)));
            }
            _ => {}
        }
        app.screen = Screen::Setup(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (title, intro) = PAGE[self.page as usize];
        if app.dirty(R_TOP, hash(&self.page)) {
            lcd.fill(0, 21, 240, 279, BG);
            app.drawn.hash[R_ROW..R_TOP].fill(0);
            app.drawn.hash[R_HELP..=R_NOTE].fill(0);
            w::field(lcd, 10, 150, 26, 46, 41, Align::Left, title.as_bytes(), &UI_12B, TEXT, BG);
            dashes(lcd, 232, 34, PAGES as i32, self.page as i32 + 1);
            let lines = match self.page {
                0 => 10,
                1 => 5,
                _ => 4,
            };
            para(lcd, 10, 52, 220, lines, 16, intro.as_bytes(), &UI_12, SUB, BG);
        }
        let p = app.prefs;
        match self.page {
            1 => {
                let call = p.callsign();
                if app.dirty(R_ROW, hash(&(call, self.mode))) {
                    field_box(lcd, 10, 136, 220, 44, b"Callsign", call, true, true);
                    let t: &[u8] = if self.mode == 0 { "digits type \u{b7} * letters/digits".as_bytes() } else { "digits type \u{b7} * back to letters".as_bytes() };
                    w::field(lcd, 10, 220, 186, 203, 199, Align::Left, t, &UI_12, SUB, BG);
                }
            }
            2 => self.regions(app, lcd),
            3 => {
                for (i, (label, v)) in [("Name", p.owner_name()), ("Contact", p.owner_contact())].into_iter().enumerate() {
                    if app.dirty(R_ROW + i, hash(&(v, self.row))) {
                        field_box(lcd, 10, 124 + 40 * i as i32, 220, 32, label.as_bytes(), v, self.row as usize == i, false);
                    }
                }
                if app.dirty(R_NOTE, 1) {
                    para(lcd, 10, 210, 220, 2, 16, b"A digit starts typing the highlighted line.", &UI_12, SUB, BG);
                }
            }
            4 => {
                let mut tz = Buf::<16>::new();
                tz_text(p.tz, &mut tz);
                let rows: [(&[u8], &[u8]); 3] =
                    [(b"Time zone", tz.as_bytes()), (b"Clock", if p.clock_12h { b"12 h" } else { b"24 h" }), (b"Units", UNITS[p.units as usize % 3].as_bytes())];
                for (i, (l, v)) in rows.iter().enumerate() {
                    app.kv_row(lcd, i, 120 + 26 * i as i32, 26, l, v, TEXT, self.row as usize == i);
                }
            }
            5 => {
                let rows: [(&[u8], &[u8]); 2] = [(b"Home layout", LAYOUTS[p.layout as usize % 3].as_bytes()), (b"Theme", THEMES[p.theme as usize % 3].as_bytes())];
                for (i, (l, v)) in rows.iter().enumerate() {
                    app.kv_row(lcd, i, 104 + 26 * i as i32, 26, l, v, TEXT, self.row as usize == i);
                }
                if app.dirty(R_NOTE, hash(&p.layout)) {
                    for (k, rects) in PREVIEWS.iter().enumerate() {
                        let (x, y) = (14 + k as i32 * 74, 166);
                        let sel = k as u8 == p.layout;
                        lcd.fill(x - 2, y - 2, 64, 80, BG);
                        lcd.frame_r(x - 2, y - 2, 64, 80, 4, if sel { BLUE } else { LINE });
                        for &(rx, ry, rw, rh) in rects.iter() {
                            lcd.fill_r(x + rx, y + ry, rw - 4, rh, 2, if sel { FOCUS } else { SURFACE });
                            lcd.frame_r(x + rx, y + ry, rw - 4, rh, 2, if sel { BLUE } else { LINE });
                        }
                    }
                    w::field(lcd, 10, 220, 250, 267, 263, Align::Center, LAYOUTS[p.layout as usize % 3].as_bytes(), &UI_12, SUB, BG);
                }
            }
            6 => {
                let mut v = Buf::<16>::new();
                app.item_text(crate::slot::Item::BatterySave, true, &mut v);
                app.kv_row(lcd, 0, 120, 26, b"Level", v.as_bytes(), TEXT, true);
                if app.dirty(R_NOTE, hash(&app.bsave)) {
                    para(lcd, 10, 156, 220, 2, 16, BSAVE[app.bsave as usize % 4].as_bytes(), &UI_12, TEXT, BG);
                }
            }
            7 => self.summary(app, lcd, cx),
            _ => {}
        }
    }

    fn hints(self, app: &App) -> Hints {
        // The mock-ups' "BACK Previous" doesn't fit with the others at 12 px.
        let ok = if self.page + 1 == PAGES { ("OK", "Start") } else { ("OK", "Next") };
        let back = if self.page == 1 && !app.prefs.callsign().is_empty() { ("BACK", "Delete") } else { ("BACK", "Back") };
        Hints::new(&[ok, back, ("#", "Skip setup")])
    }
}

impl Setup {
    /// Page 3: five regions at a time, the chosen one framed blue, then Extend TX (SetupRegion).
    fn regions(self, app: &mut App, lcd: &mut impl Panel) {
        let n = REGIONS.len() as i32;
        let at = if app.prefs.region == NO_REGION { self.row.min(n as u8 - 1) } else { app.prefs.region };
        let first = (at as i32 - 2).clamp(0, n - 5);
        for k in 0..5 {
            let i = first + k;
            let sel = at as i32 == i;
            let foc = self.row as i32 == i;
            if app.dirty(R_ROW + k as usize, hash(&(i, sel, foc))) {
                let y = 104 + k * 27;
                let (bg, border) = if sel { (FOCUS, BLUE) } else { (BG, LINE) };
                lcd.fill(8, y, 224, 24, BG);
                lcd.fill_r(10, y, 220, 24, 4, bg);
                let (name, range) = REGIONS[i as usize];
                w::field(lcd, 18, 140, y + 2, y + 22, y + 17, Align::Left, name.as_bytes(), if foc { &UI_12B } else { &UI_12 }, TEXT, bg);
                w::field(lcd, 158, 64, y + 2, y + 22, y + 17, Align::Right, range.as_bytes(), &UI_12, SUB, bg);
                lcd.frame_r(10, y, 220, 24, 4, border);
            }
        }
        let foc = self.row as usize == REGIONS.len();
        let on = app.prefs.extend_tx;
        app.kv_row(lcd, 5, 241, 22, b"Extend TX", if on { b"On" } else { b"Off" }, if on { AMBER } else { TEXT }, foc);
        if app.dirty(R_NOTE, 1) {
            para(lcd, 10, 265, 220, 2, 16, b"On: beyond the amateur bands. It warns first.", &UI_12, SUB, BG);
        }
    }

    /// Page 8: the choices, the transmit state, and how to add channels (SETUP-10).
    fn summary(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let p = app.prefs;
        let mut tz = Buf::<16>::new();
        tz_text(p.tz, &mut tz);
        let mut look = Buf::<32>::new();
        let _ = uwrite!(look, "{} \u{b7} {}", LAYOUTS[p.layout as usize % 3], THEMES[p.theme as usize % 3]);
        let mut bs = Buf::<16>::new();
        app.item_text(crate::slot::Item::BatterySave, true, &mut bs);
        let mut ch = Buf::<16>::new();
        match cx.store.count() {
            0 => ch.set(b"none yet"),
            n => {
                let _ = uwrite!(ch, "{}", n);
            }
        }
        let tx: &[u8] = if !cx.dev.tx_built { b"RX-only build" } else if p.extend_tx { b"Extend TX on" } else { b"Amateur bands" };
        let rows: [(&[u8], &[u8]); 8] = [
            (b"Callsign", if p.callsign().is_empty() { b"not set" } else { p.callsign() }),
            (b"Region", p.region_name().as_bytes()),
            (b"Transmit", tx),
            (b"Time zone", tz.as_bytes()),
            (b"Home", look.as_bytes()),
            (b"Battery Save", bs.as_bytes()),
            (b"Owner card", if p.owner_name().is_empty() { b"empty" } else { p.owner_name() }),
            (b"Channels", ch.as_bytes()),
        ];
        for (i, (l, v)) in rows.iter().enumerate() {
            app.kv_row(lcd, i, 52 + 22 * i as i32, 22, l, v, TEXT, false);
        }
        if app.dirty(R_NOTE, 1) {
            para(lcd, 10, 232, 220, 4, 16, b"Add channels with hold V/M on a VFO, or load them with the web app. Run setup again: MENU 0 9 8 1.", &UI_12, SUB, BG);
        }
    }
}

fn set_call(app: &mut App, s: &[u8]) {
    app.prefs.callsign = [0; 10];
    for (d, c) in app.prefs.callsign.iter_mut().zip(s) {
        *d = c.to_ascii_uppercase(); // SETUP-12
    }
}
