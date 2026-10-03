//! The Extend TX warning (turning Extend TX on: setup page 3, MENU 0 8 9; manual chapters 4 and 15, SETUP-8,
//! TX-2), shown the first time only. OK turns it on, BACK leaves it off.

use super::forms::para;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::App;
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::widgets as w;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExtendTx;

/// SETUP-8's three points, one paragraph each.
const POINTS: [&str; 3] = [
    "It may be illegal where you are.",
    "It can interfere with services people rely on.",
    "You are responsible for every transmission.",
];

/// Extend TX on or off: on asks first, the first time (SETUP-8); off never asks.
pub fn toggle(app: &mut App) {
    if app.prefs.extend_tx {
        app.prefs.extend_tx = false;
    } else if app.prefs.extend_warned {
        app.prefs.extend_tx = true;
    } else {
        app.open(super::nav::Dest::ExtendTx);
    }
}

impl View for ExtendTx {
    fn name(self) -> &'static str {
        "Extend TX warning"
    }

    fn title(self, _: &App, b: &mut ferrite_ui::text::Buf<10>) {
        b.set(b"Extend TX");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Menu, Press::Short) => {
                (app.prefs.extend_tx, app.prefs.extend_warned) = (true, true);
                app.show_toast(b"Extend TX on", b"", false);
                app.back();
            }
            (Key::Exit, Press::Short) => app.back(),
            _ => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        if app.dirty(R_TOP, 1) {
            lcd.fill(0, 21, 240, 279, BG);
            lcd.fill(0, 21, 4, 279, AMBER);
            w::field(lcd, 14, 216, 28, 48, 43, Align::Left, b"Extend TX: read this first", &UI_12B, AMBER, BG);
            let mut y = 52 + 17 * para(lcd, 14, 52, 216, 3, 17, b"The radio will transmit wherever its hardware can, not only on your amateur bands.", &UI_12, TEXT, BG) + 6;
            for p in POINTS {
                lcd.fill_r(14, y + 6, 5, 5, 2, AMBER);
                y += 17 * para(lcd, 26, y, 204, 3, 17, p.as_bytes(), &UI_12, TEXT, BG) + 6;
            }
            para(lcd, 14, y + 4, 216, 2, 17, b"Status shows it while on. Off: MENU 0 8 9.", &UI_12, SUB, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("OK", "Turn on"), ("BACK", "Keep off")])
    }
}
