//! The shortcut map (MENU 0 9 1 4; manual chapter 7, HOME-8, KEY-11, KEY-21): the ten hold-digit shortcuts
//! with what each would do now. Holding a digit on home runs its shortcut at once ([`super::nav::hold_digit`]);
//! here ▲▼ (or the knob, or a digit) pick one and OK runs it on home, as nicFW's function menu does, while the
//! help line names the digit to hold next time.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{chan_label, Item};
use crate::App;
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub sel: u8,
}

/// The ten shortcuts (KEY-11), by digit.
pub(crate) const SHORTCUTS: [&str; 10] = [
    "Home channel",
    "Band scope",
    "TX power",
    "Squelch",
    "Watch on/off",
    "Tones",
    "Step",
    "Shift",
    "Offset",
    "Zones",
];

impl View for Shortcut {
    fn name(self) -> &'static str {
        "Shortcut map"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Shortcut");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Exit, Press::Short) => app.back(),
            (Key::Up | Key::Down, Press::Short | Press::Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Digit(d), Press::Short) => app.screen = crate::Screen::Shortcut(Shortcut { sel: d }),
            // KEY-21: OK runs it from home, as holding the digit there would
            (Key::Menu, Press::Short) => {
                app.go_home();
                super::nav::hold_digit(app, self.sel, store);
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let sel = (self.sel as i32 + d).rem_euclid(SHORTCUTS.len() as i32) as u8;
        app.screen = crate::Screen::Shortcut(Shortcut { sel });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        if app.dirty(R_TOP, 1) {
            w::field(lcd, 8, 224, 22, 40, 36, Align::Left, b"Hold a digit on home", &UI_12B, TEXT, BG);
            lcd.fill(0, 41, 240, 1, LINE);
        }
        for (d, name) in SHORTCUTS.iter().enumerate() {
            let mut v = Buf::<16>::new();
            match d {
                0 => match app.set.home_ch {
                    Some(n) => {
                        let mut l = Buf::<32>::new();
                        chan_label(n, &mut l);
                        v.set(l.as_bytes());
                    }
                    None => v.set(b"none set"),
                },
                2 => app.item_text(Item::Power, true, &mut v),
                3 => app.item_text(Item::Squelch, true, &mut v),
                4 => app.item_text(Item::Watch, true, &mut v),
                5 => app.item_text(Item::Tones, true, &mut v),
                6 => app.item_text(Item::Step, true, &mut v),
                7 if app.slot(app.active()).cfg().split => v.set(b"Split"),
                7 => v.set(match app.slot(app.active()).cfg().offset.signum() {
                    1 => b"+",
                    -1 => b"-",
                    _ => b"Simplex",
                }),
                8 => app.item_text(Item::Offset, true, &mut v),
                _ => {}
            }
            let grey = d == 6 && app.greyed(Item::Step).is_some(); // VFO-8: step is VFO only
            let r = Row { num: Some(d as u8), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: d == self.sel as usize, grey };
            app.list_row(lcd, d, 44 + 22 * d as i32, 22, &r);
        }
        let mut path = Buf::<16>::new();
        path.push(b"Hold ");
        path.push(&[b'0' + self.sel % 10]);
        app.help_box(lcd, 268, 28, b"OK runs it. On home:", path.as_bytes());
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Pick"), ("OK", "Run"), ("BACK", "Back")])
    }
}
