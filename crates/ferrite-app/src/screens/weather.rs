//! Weather radio and the alert screen (manual chapter 23).
//!
//! Weather radio (hold 7; US and Canada only, WX-1): the manual doesn't define this screen. Here it lists the
//! seven weather channels (162.400-162.550 MHz, WX-2) and listens on the highlighted one with the active slot
//! until the screen is left; OK keeps it as the slot's VFO. Below, the SAME state and the last alert.
//!
//! Alert (over any screen, WX-4; Alert mock-up): the event, areas, when it arrived and the tone, inside a
//! flashing red border (WX-6). OK silences; once silenced, OK opens the event log (WX-7: the manual gives OK
//! and MENU different jobs, but they are one key); BACK goes home.

use super::activity::Mode;
use super::nav::Dest;
use super::parts;
use super::scan::ago;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_core::msg::RadioCmd;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;

/// WX1-WX7 in NOAA's numbering.
pub const CHANNELS: [Freq10; 7] = [
    mhz(162) + 55_000,
    mhz(162) + 40_000,
    mhz(162) + 47_500,
    mhz(162) + 42_500,
    mhz(162) + 45_000,
    mhz(162) + 50_000,
    mhz(162) + 52_500,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Weather {
    pub ch: u8,
}

/// Hold 7 (WX-1): nothing outside the US and Canada once a region is set.
pub fn open(app: &mut App) -> Option<Screen> {
    if app.radio.region.is_some_and(|r| !r.has_weather()) {
        return None;
    }
    // Start on the slot's own weather channel if it is on one.
    let f = app.slot(app.active).freq();
    let ch = CHANNELS.iter().position(|&c| c == f).unwrap_or(0) as u8;
    listen(app, ch);
    Some(Screen::Weather(Weather { ch }))
}

fn listen(app: &mut App, ch: u8) {
    app.start_mode(Mode::Weather);
    app.out.push(RadioCmd::TuneSlot { slot: app.active, freq: CHANNELS[ch as usize % CHANNELS.len()] });
}

impl View for Weather {
    fn name(self) -> &'static str {
        "Weather radio"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Weather");
    }

    fn key(self, app: &mut App, e: KeyEvent, s: &impl ChannelStore) {
        use Press::*;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Down { 1 } else { -1 }, s),
            (Key::Menu, Short) => {
                // Keep it: the active slot's VFO on this channel.
                let a = app.active as usize;
                app.slots[a].vfo = CHANNELS[self.ch as usize];
                app.slots[a].vfo_mode = true;
                app.radio.mode = Mode::Off;
                app.go_home();
                app.send_slot(a as u8);
            }
            (Key::Exit, Short) => {
                app.stop_mode();
                app.back();
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let ch = (self.ch as i32 + d).rem_euclid(CHANNELS.len() as i32) as u8;
        listen(app, ch);
        app.screen = Screen::Weather(Weather { ch });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let lv = cx.live.slots[app.active as usize];
        let st = if lv.squelch_open { w::s_text(lv.rssi) } else { Buf::from(b"-") };
        if app.dirty(1, hash(&app.active)) {
            w::badge(lcd, 8, 24, app.active, true, BG);
            w::field(lcd, 26, 206, 22, 40, 35, Align::Left, b"listening on the active slot", &UI_12, SUB, BG);
            lcd.fill(0, 41, 240, 1, LINE);
        }
        for (i, &f) in CHANNELS.iter().enumerate() {
            let mut name = Buf::<8>::new();
            let _ = uwrite!(name, "WX{}", i + 1);
            let fq = w::freq3(f);
            let sel = i as u8 == self.ch;
            let mut v = Buf::<16>::new();
            v.push(fq.as_bytes());
            if sel {
                v.push(b"  ");
                v.push(st.as_bytes());
            }
            let r = Row { num: None, name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: sel, grey: false };
            app.list_row(lcd, 1 + i, 42 + 22 * i as i32, 22, &r);
        }
        // SAME (WX-3, WX-10) and the last alert.
        let note = app.radio.alert;
        let mut last = Buf::<48>::new();
        if note.at != 0 || !note.head().is_empty() {
            let mut a = Buf::<16>::new();
            ago(app.now.wrapping_sub(note.at), &mut a);
            last.push(note.head());
            last.push(b", ");
            last.push(a.as_bytes());
            last.push(b" ago");
        } else {
            last.set(b"None");
        }
        if app.dirty(10, hash(&last)) {
            lcd.fill(0, 200, 240, 1, LINE);
            parts::kv(lcd, 8, 90, 232, 220, b"SAME alerts", b"On, every area", TEXT, BG);
            parts::kv(lcd, 8, 90, 232, 238, b"Last alert", last.as_bytes(), TEXT, BG);
            w::field(lcd, 8, 224, 246, 264, 259, Align::Left, b"Location codes: MENU 0 3 3 2", &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Channel"), ("OK", "Keep"), ("BACK", "Back")])
    }
}

// ---- Alert ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Alert;

impl View for Alert {
    fn name(self) -> &'static str {
        "Alert"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("Alert.png")
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Menu, Press::Short) if !app.radio.alert.silenced => app.radio.alert.silenced = true,
            (Key::Menu, Press::Short | Press::Long) => {
                app.radio.alert.silenced = true;
                app.screen = Screen::Home;
                app.open(Dest::EventLog);
            }
            (Key::Exit, Press::Short) => {
                app.radio.alert.silenced = true;
                app.go_home();
            }
            _ => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let note = app.radio.alert;
        // WX-6: the border flashes until silenced.
        let on = note.silenced || (app.now / 500).is_multiple_of(2);
        let border = if on { RED } else { rgb565(0x5A, 0x1A, 0x1C) };
        let inner = rgb565(0x16, 0x0C, 0x0E);
        if app.dirty(1, hash(&border)) {
            lcd.fill_r(4, 23, 232, 274, 8, border);
            lcd.fill_r(8, 27, 224, 266, 5, inner);
            app.drawn.hash[2..6].fill(0);
        }
        if app.dirty(2, 1) {
            warn(lcd, 18, 37, RED);
            lcd.text4_rows(38, 48, b"WEATHER ALERT", &UI_12B, RED, inner, 34, 52);
            parts::wrap(lcd, 18, 204, 76, 20, 3, note.head(), &UI_12B, TEXT, inner);
        }
        // When it arrived: the clock then, and how long ago.
        let mins = app.now.wrapping_sub(note.at) / 60_000;
        let mut when = Buf::<28>::new();
        if let Some(c) = cx.dev.clock_min {
            let t = (c as u32 + 1440 - mins % 1440) % 1440;
            let _ = uwrite!(when, "{}:{}{}", t / 60, t % 60 / 10, t % 10);
            let _ = uwrite!(when, " ({} min ago)", mins);
        } else {
            let _ = uwrite!(when, "{} min ago", mins);
        }
        if app.dirty(3, hash(&when)) {
            parts::kv(lcd, 18, 90, 222, 140, b"Received", when.as_bytes(), TEXT, inner);
            parts::kv(lcd, 18, 90, 222, 158, b"From", b"SAME", TEXT, inner);
            parts::kv(lcd, 18, 90, 222, 176, b"Areas", b"in the event log", TEXT, inner);
            parts::kv(lcd, 18, 90, 222, 194, b"Until", b"in the event log", TEXT, inner);
        }
        if app.dirty(4, hash(&note.silenced)) {
            let (t, c): (&[u8], u16) = if note.silenced { (b"Tone silenced", SUB) } else { (b"Tone sounding", RED) };
            bell(lcd, 18, 268, c, inner);
            w::field(lcd, 34, 180, 266, 284, 279, Align::Left, t, &UI_12, c, inner);
        }
    }

    fn hints(self, app: &App) -> Hints {
        if app.radio.alert.silenced {
            Hints::new(&[("OK", "Log"), ("BACK", "Home")])
        } else {
            Hints::new(&[("OK", "Silence"), ("BACK", "Home")])
        }
    }
}

/// The warning triangle, 14 px, with its "!".
fn warn(lcd: &mut impl Panel, x: i32, y: i32, c: u16) {
    for i in 0..13 {
        let half = i / 2;
        lcd.fill(x + 6 - half, y + i, 2 * half + 1, 1, c);
    }
    lcd.fill(x + 6, y + 4, 1, 5, BLACK);
    lcd.fill(x + 6, y + 10, 1, 1, BLACK);
}

/// A bell, 11 px.
fn bell(lcd: &mut impl Panel, x: i32, y: i32, c: u16, bg: u16) {
    lcd.fill(x, y, 12, 13, bg);
    lcd.frame_r(x + 2, y, 8, 9, 3, c);
    lcd.fill(x, y + 8, 12, 1, c);
    lcd.fill(x + 5, y + 10, 2, 2, c);
}
