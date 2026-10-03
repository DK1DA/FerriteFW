//! Squelch from home (manual chapter 12, SQL-7 and SQL-10; no mock-up, the status bar name "Squelch" is
//! inferred): hold SIDE1 and turn the knob or press ▲▼. The level 0-9 and the active slot's live signal show
//! while SIDE1 is held; the change is live (SQL-7); letting SIDE1 go returns home.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::Item;
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
pub struct Squelch;

/// SIDE1 held + knob or ▲▼: one level up or down (stops at 0 and 9), and this screen while SIDE1 is held.
pub fn step(app: &mut App, d: i32) {
    app.keys.used(Key::Side(1));
    let sq = app.slot(app.active).cfg().squelch as i32;
    let to = (sq + d).clamp(0, 9);
    app.change(Item::Squelch, to - sq);
    if !matches!(app.screen, Screen::Squelch(_)) {
        app.screen = Screen::Squelch(Squelch);
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

    fn tick(self, app: &mut App, _: &LiveAll) {
        if !app.keys.is_held(Key::Side(1)) {
            app.screen = Screen::Home;
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let a = app.active;
        let sq = app.slot(a).cfg().squelch;
        let lv = cx.live.slots[a as usize];
        if app.dirty(1, hash(&(a, sq))) {
            w::badge(lcd, 8, 30, a, true, BG);
            w::field(lcd, 28, 200, 28, 46, 41, Align::Left, b"Squelch, active slot", &UI_12, SUB, BG);
            lcd.fill(0, 52, 240, 60, BG);
            lcd.text4_rows(100, 104, &[b'0' + sq], &DIG_46, TEXT, BG, 60, 106);
            // 0-9: the level's segments.
            for i in 0..10 {
                let c = if i < sq { BLUE } else { LINE };
                lcd.fill_r(20 + i as i32 * 20, 122, 16, 10, 2, c);
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
