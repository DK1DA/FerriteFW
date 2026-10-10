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
            (Key::Digit(0), Short) => to_advanced(app),
            (Key::Digit(d), Short) => app.screen = Screen::Quick(Quick { row: d - 1 }), // MENU-5
            (Key::Menu, Short) => match item {
                Item::Advanced => to_advanced(app),
                // TSRCH-16: a tone hint: OK uses the heard tone for RX, then asks about TX.
                Item::Tones if heard(app) != 0 => {
                    let (a, t) = (app.active, heard(app));
                    super::probe::set_tone(app, a, t, false);
                    super::confirm::ask(app, super::confirm::Ask::ToneTx(t));
                }
                // MENU 8: save the VFO as a channel, or edit the channel.
                Item::Channel if app.slots[app.active as usize].on_channel() => app.open(Dest::ChannelEdit),
                Item::Channel => app.open(Dest::SaveChannel),
                _ => app.open(Dest::value(item)), // MENU-17
            },
            (Key::Hash, Short) => app.reset(item), // MENU-18
            (Key::Star, Short) => app.do_undo(),   // MENU-22
            (Key::Exit, Short) => app.back(),
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
            let mut v = Buf::<32>::new();
            let mut t = Buf::<16>::new();
            app.item_text(item, true, &mut t);
            v.set(t.as_bytes());
            if item == Item::Tones && heard(app) != 0 {
                // TSRCH-16: "T 100.0, heard 88.5"
                let mut h = Buf::<16>::new();
                super::shared::heard_text(heard(app), &mut h);
                v.push(b", heard");
                v.push(&h.as_bytes()[5..]);
            }
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
        let help = if item == Item::Tones && heard(app) != 0 { "OK uses the heard tone." } else { why.unwrap_or(item.help(!sl.on_channel())) };
        app.help_box(lcd, 265, 33, help.as_bytes(), p.as_bytes());
    }

    fn hints(self, app: &App) -> Hints {
        if app.undo.is_some() {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Back")])
        } else if self.row == 9 {
            Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")])
        } else if QUICK[self.row as usize] == Item::Tones && heard(app) != 0 {
            Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Use heard"), ("BACK", "Back")])
        } else {
            Hints::new(&[(ARROWS_UD, "Move"), ("Knob", "Change"), ("BACK", "Back")])
        }
    }
}

/// The active slot's tone hint (TSRCH-16), 0 none.
fn heard(app: &App) -> u16 {
    app.heard[app.active as usize]
}

/// MENU 0 from quick settings: advanced settings, one level down (BACK returns here, MENU-24).
fn to_advanced(app: &mut App) {
    app.push_back();
    app.screen = Screen::Advanced(Advanced { row: 0, from_quick: true });
}
