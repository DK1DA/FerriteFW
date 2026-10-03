//! The sensor reader (MENU 0 4 5; manual chapter 27, DEC-10 to DEC-12; C7): the devices heard (type, ID,
//! value, signal, first and last seen) under the noise floor against its baseline and the jamming state. OK
//! opens the event log on sensor readings.

use super::lists::*;
use super::log::Log;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::Src;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sensors {
    pub cur: Cursor,
}

const ALL: fn(u16) -> bool = |_| true;
/// Seven rows under the noise line.
const ROWS: u16 = 7;

impl View for Sensors {
    fn name(self) -> &'static str {
        "Sensor reader"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Sensors");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) => {
                // the readings in the log
                let filter = Src::ALL.iter().position(|&s| s == Src::Sensor).map_or(0, |i| i as u8 + 1);
                let k = |i| store.data().event(i).is_some_and(|e| e.src == Src::Sensor);
                app.open_screen(Screen::Log(Log { cur: Cursor::first(store.data().events(), &k), filter, detail: false }));
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        app.screen = Screen::Sensors(Sensors { cur: self.cur.step(d, store.data().sensors(), ROWS, &ALL) });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, 0u8);
        if !app.gate((self, data.version(), cx.dev.clock_min)) {
            return;
        }
        let n = data.sensors();
        let mut l = Buf::<24>::new();
        let _ = uwrite!(l, "{} {} heard", n, if n == 1 { "device" } else { "devices" });
        app.list_head(lcd, l.as_bytes(), b"433.92 MHz");
        // DEC-12: the noise floor against its baseline, and the alarm
        let noise = data.noise();
        if app.dirty(R_NOTE, hash(&noise)) {
            let y = LIST_Y;
            lcd.fill(0, y, 240, ROW2_H, BG);
            if let Some(nz) = noise {
                let mut b = Buf::<40>::new();
                let _ = uwrite!(b, "Noise {} dBm \u{b7} baseline {}", nz.floor, nz.baseline);
                lcd.text4(8, y + 13, b.as_bytes(), &UI_12, SUB, BG);
                let (t, c): (&[u8], u16) = match (nz.alarm_db, nz.jamming) {
                    (0, _) => (b"Jamming alarm off", DIM),
                    (_, true) => (b"JAMMING", RED),
                    (db, false) => (ferrite_ui::v!(b, "No jamming \u{b7} alarm at +{} dB", db), GREEN),
                };
                lcd.text4(8, y + 29, t, &UI_12, c, BG);
            }
            lcd.fill(0, y + ROW2_H - 1, 240, 1, LINE);
        }
        let rows = self.cur.rows(n, ROWS, &ALL, |row, i| {
            if let Some(s) = data.sensor(i) {
                let (mut rssi, mut sub, mut age, mut first) = (Buf::<12>::new(), Buf::<48>::new(), Buf::<12>::new(), Buf::<12>::new());
                let _ = uwrite!(rssi, "{} dBm", s.rssi);
                age_text(s.last_ago_s, &mut age);
                age_text(s.first_ago_s, &mut first);
                let _ = uwrite!(sub, "{} \u{b7} since {}", Txt(s.value.as_bytes()), Txt(first.as_bytes()));
                let mut t = Buf::<28>::new();
                let _ = uwrite!(t, "{} #{}", Txt(s.model.as_bytes()), s.id);
                let r = Row2 { title: t.as_bytes(), right: rssi.as_bytes(), sub: sub.as_bytes(), sub_right: age.as_bytes(), selected: i == self.cur.sel, ..Row2::default() };
                // one row down: the noise line is first
                if app.dirty(R_ROW + row, hash(&r)) {
                    row2(lcd, LIST_Y + (row as i32 + 1) * ROW2_H, &r);
                }
            }
        });
        for i in rows..ROWS as usize {
            if app.dirty(R_ROW + i, 1) {
                lcd.fill(0, LIST_Y + (i as i32 + 1) * ROW2_H, 240, ROW2_H, BG);
            }
        }
        if n == 0 {
            app.empty(lcd, b"No devices heard", b"Bands: MENU 0 4 5");
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Readings"), ("BACK", "Back")])
    }
}
