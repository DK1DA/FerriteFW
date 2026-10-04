//! Weather radio (manual chapter 23). The alert screen is [`super::alert`].
//!
//! Weather radio (hold 7; US and Canada only, WX-1): the manual doesn't define this screen. Here it lists the
//! seven weather channels (162.400-162.550 MHz, WX-2) and listens on the highlighted one with the active slot
//! until the screen is left; OK keeps it as the slot's VFO. Below, the SAME state and the last alert.

use super::activity::Mode;
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
    if !app.set.wx_allowed() {
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
        let note = &*app.alert_info;
        let mut last = Buf::<48>::new();
        if note.event.is_empty() {
            last.set(b"None");
        } else {
            last.push(note.event.as_bytes());
            if note.issued != super::alert::NO_TIME {
                let _ = uwrite!(last, ", {}:{}{}", note.issued / 60, note.issued % 60 / 10, note.issued % 10);
            }
        }
        if app.dirty(10, hash(&last)) {
            lcd.fill(0, 200, 240, 1, LINE);
            w::kv(lcd, 207, 17, &w::KV_PAIR, b"SAME alerts", b"On, every area", TEXT, false);
            w::kv(lcd, 225, 17, &w::KV_PAIR, b"Last alert", last.as_bytes(), TEXT, false);
            w::field(lcd, 8, 224, 246, 264, 259, Align::Left, b"Location codes: MENU 0 3 3 2", &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Channel"), ("OK", "Keep"), ("BACK", "Back")])
    }
}
