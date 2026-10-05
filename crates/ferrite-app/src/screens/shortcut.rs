//! The shortcut map (MENU 0 9 1 4; manual chapter 7, HOME-8, KEY-11): a reference page listing the ten hold-digit
//! shortcuts with what each would do now. Holding a digit on home runs its shortcut at once
//! ([`super::nav::hold_digit`]); this page is only for looking them up.

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
pub struct Shortcut;

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
        b.set(b"Shortcuts");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        if (e.key, e.press) == (Key::Exit, Press::Short) {
            app.back();
        }
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
            let r = Row { num: Some(d as u8), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: false, grey };
            app.list_row(lcd, d, 44 + 25 * d as i32, 25, &r);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[])
    }
}
