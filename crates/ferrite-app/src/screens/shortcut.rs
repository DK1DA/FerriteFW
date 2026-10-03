//! The shortcut map (holding a digit on home; manual chapters 7 and 6, HOME-8, KEY-11): the ten hold-digit
//! shortcuts with what each would do now, the held one highlighted. Releasing the digit runs it
//! ([`super::nav::shortcut`]); BACK while holding cancels.

use super::nav;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{chan_label, Item};
use crate::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::LiveAll;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    /// The digit being held.
    pub digit: u8,
}

/// The ten shortcuts (KEY-11), by digit.
pub(crate) const SHORTCUTS: [&str; 10] = [
    "Home channel",
    "Watch on/off",
    "FM broadcast",
    "Band scope",
    "APRS heard",
    "TX power",
    "Bandwidth",
    "Weather radio",
    "Battery Save",
    "Tone search",
];

impl View for Shortcut {
    fn name(self) -> &'static str {
        "Shortcut map"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Shortcuts");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        match (e.key, e.press) {
            // HOME-8: the release runs it.
            (Key::Digit(d), Press::LongUp) if d == self.digit => {
                app.go_home();
                nav::shortcut(app, d, store);
            }
            (Key::Exit, Press::Short) => app.go_home(),
            _ => {}
        }
    }

    /// A release the map didn't hear (the lock, a lost key-up): home, running nothing.
    fn tick(self, app: &mut App, _: &LiveAll) {
        if !app.keys.is_held(Key::Digit(self.digit)) {
            app.go_home();
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        if app.dirty(R_TOP, 1) {
            w::field(lcd, 8, 224, 22, 40, 36, Align::Left, b"Release to run it", &UI_12B, TEXT, BG);
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
                1 => app.item_text(Item::Watch, true, &mut v),
                5 => app.item_text(Item::Power, true, &mut v),
                6 => app.item_text(Item::Bandwidth, true, &mut v),
                8 => app.item_text(Item::BatterySave, true, &mut v),
                _ => {}
            }
            let grey = d == 7 && !app.set.wx_allowed(); // WX-1: hold 7 does nothing outside the US and Canada
            let r = Row { num: Some(d as u8), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: d as u8 == self.digit, grey };
            app.list_row(lcd, d, 44 + 25 * d as i32, 25, &r);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("Release", "Run"), ("BACK", "Cancel")])
    }
}
