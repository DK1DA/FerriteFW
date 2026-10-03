//! Quick settings (MENU on home; manual chapter 11, MENU-1 and MENU-12 to MENU-22; MenuQuick mock-up): the
//! active slot's nine settings and "0 Advanced". The knob or ◀▶ change the highlighted value live.

use super::advanced::Advanced;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_name, chan_label, mode_chip, Item, QUICK};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Quick {
    /// The highlighted row, 0-9 (9 is "0 Advanced").
    pub row: u8,
}

impl View for Quick {
    fn name(self) -> &'static str {
        "Quick"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("MenuQuick.png")
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS) // MENU-28
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let row = self.row;
        let item = QUICK[row as usize];
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => app.screen = Screen::Quick(Quick { row: (row + 9) % 10 }),
            (Key::Down, Short | Repeat) => app.screen = Screen::Quick(Quick { row: (row + 1) % 10 }),
            (Key::Left | Key::Right, Short | Repeat) => app.change(item, if e.key == Key::Right { 1 } else { -1 }), // MENU-16
            (Key::Digit(0), Short) => app.screen = Screen::Advanced(Advanced { row: 0, from_quick: true }),
            (Key::Digit(d), Short) => app.screen = Screen::Quick(Quick { row: d - 1 }), // MENU-5
            (Key::Menu, Short) => match item {
                Item::Advanced => app.screen = Screen::Advanced(Advanced { row: 0, from_quick: true }),
                // MENU 8: save the VFO as a channel, or edit the channel.
                Item::Channel if app.slots[app.active as usize].on_channel() => app.open(Dest::ChannelEdit),
                Item::Channel => app.open(Dest::SaveChannel),
                _ => app.open(Dest::value(item)), // MENU-17
            },
            (Key::Hash, Short) => app.reset(item), // MENU-18
            (Key::Star, Short) => app.do_undo(),   // MENU-22
            (Key::Exit, Short) => app.go_home(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.change(QUICK[self.row as usize], d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let row = self.row;
        let a = app.active;
        let sl = app.slots[a as usize];
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
        if app.dirty(R_TOP, hash(&(a, head, m, f))) {
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
            app.item_text(item, true, &mut v);
            let r = Row {
                num: Some(((i + 1) % 10) as u8),
                name: item.name().as_bytes(),
                value: v.as_bytes(),
                dot: !app.is_default(item),
                selected: i as u8 == row,
                grey: app.greyed(item).is_some(),
            };
            app.list_row(lcd, i, 41 + 22 * i as i32, 22, &r);
        }
        let item = QUICK[row as usize];
        let why = app.greyed(item);
        let mut p = Buf::<16>::new();
        let _ = uwrite!(p, "MENU {}", (row + 1) % 10);
        app.help_box(lcd, 265, 33, why.unwrap_or(item.help(!sl.on_channel())).as_bytes(), p.as_bytes());
    }

    fn hints(self, app: &App) -> Hints {
        if app.undo.is_some() {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Home")])
        } else if self.row == 9 {
            Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Home")])
        } else {
            Hints::new(&[(ARROWS_UD, "Move"), ("Knob", "Change"), ("BACK", "Home")])
        }
    }
}
