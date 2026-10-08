//! Squelch from home (manual chapter 12, SQL-7 and SQL-10; no mock-up, the status bar name "Squelch" is
//! inferred): hold SIDE1 and turn the knob or press ▲▼. The level 0-9 and the active slot's live signal show
//! while SIDE1 is held; the change is live (SQL-7); letting SIDE1 go returns home. Beside the live signal, the
//! noise floor (SQL-12, as nicFW's floor readout): the signal while the squelch is closed, smoothed, so the gap
//! between a station and the noise shows where to set the level.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::LiveAll;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use super::parts;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Squelch {
    /// The noise floor, dBm (SQL-12); 0 until one is read.
    pub floor: i16,
}

/// SIDE1 held + knob or ▲▼: one level up or down (stops at 0 and 9), and this screen while SIDE1 is held.
pub fn step(app: &mut App, d: i32) {
    app.keys.used(Key::Side(1));
    app.step_squelch(d);
    if !matches!(app.screen, Screen::Squelch(_)) {
        app.screen = Screen::Squelch(Squelch { floor: 0 });
    }
}

impl View for Squelch {
    fn name(self) -> &'static str {
        "Squelch"
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        if let (Key::Up | Key::Down, Press::Short | Press::Repeat) = (e.key, e.press) {
            step(app, if e.key == Key::Up { 1 } else { -1 });
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        step(app, d);
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        if !app.keys.is_held(Key::Side(1)) {
            app.screen = Screen::Home;
            return;
        }
        // SQL-12: the floor follows the signal while the squelch is closed (an eighth of each reading)
        let lv = live.slots[app.active as usize];
        if !lv.squelch_open && lv.rssi < 0 {
            let floor = if self.floor == 0 { lv.rssi } else { (self.floor * 7 + lv.rssi) / 8 };
            app.screen = Screen::Squelch(Squelch { floor });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let a = app.active;
        let sq = app.sq_level(a);
        let lv = cx.live.slots[a as usize];
        if app.dirty(1, hash(&(a, sq))) {
            w::badge(lcd, 8, 30, a, true, BG);
            w::field(lcd, 28, 200, 28, 46, 41, Align::Left, b"Squelch, active slot", &UI_12, SUB, BG);
            lcd.fill(0, 52, 240, 60, BG);
            lcd.text4_rows(100, 104, &[b'0' + sq], &DIG_46, TEXT, BG, 60, 106);
            // 0-9: the level's segments.
            for i in 0..10 {
                let c = if i < sq { BLUE } else { LINE };
                lcd.fill_r(20 + i as i32 * 20, 122, 16, 10, 1, c);
            }
            w::field(lcd, 8, 30, 136, 152, 148, Align::Left, b"open", &UI_12, SUB, BG);
            w::field(lcd, 172, 60, 136, 152, 148, Align::Right, b"tight", &UI_12, SUB, BG);
        }
        // The live signal: what the level lets through (SQL-7).
        let lit = w::meter_level(lv.rssi);
        if app.dirty(2, hash(&(lit, lv.squelch_open))) {
            lcd.fill(0, 170, 240, 1, LINE);
            let (t, c): (&[u8], u16) = if lv.squelch_open { (b"Open: you hear it", GREEN) } else { (b"Closed", SUB) };
            w::field(lcd, 8, 160, 178, 196, 191, Align::Left, t, &UI_12B, c, BG);
            w::field(lcd, 168, 64, 178, 196, 191, Align::Right, w::s_text(lv.rssi).as_bytes(), &UI_12B, TEXT, BG);
            w::meter(lcd, 20, 204, None, lit, if lv.squelch_open { GREEN } else { DIM }, BG);
        }
        // SQL-12: the floor and how far the signal is above it
        let mut fl = Buf::<40>::new();
        if self.floor != 0 {
            let _ = ufmt::uwrite!(fl, "Noise floor {} dBm", self.floor);
            if lv.squelch_open && lv.rssi > self.floor {
                let _ = ufmt::uwrite!(fl, ", signal +{} dB", lv.rssi - self.floor);
            }
        }
        if app.dirty(4, hash(&fl)) {
            w::field(lcd, 8, 224, 216, 234, 229, Align::Left, fl.as_bytes(), &UI_12, SUB, BG);
        }
        if app.dirty(3, 1) {
            let mut b = Buf::<40>::new();
            b.push(b"Per band: MENU 0 9 5 1");
            parts::wrap(lcd, 8, 224, 240, 17, 1, b.as_bytes(), &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("Knob", "Level"), ("SIDE1", "Release: done")])
    }
}
