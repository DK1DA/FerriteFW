//! The shortcut map (manual chapter 7, HOME-8, KEY-11; no mock-up): holding a digit on home shows the ten
//! fixed shortcuts with the held one marked; letting the digit go runs it.

use super::nav;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::KeyEvent;
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::LiveAll;
use ferrite_ui::panel::Panel;
use ferrite_ui::widgets::Row;

/// KEY-11's table, 0-9.
pub const SHORTCUTS: [(&str, &str); 10] = [
    ("Home channel", ""),
    ("Watch", "on/off"),
    ("FM broadcast", ""),
    ("Band scope", ""),
    ("APRS heard", ""),
    ("TX power", "low, mid, high"),
    ("Bandwidth", "wide, narrow"),
    ("Weather radio", "US, Canada"),
    ("Battery Save", "level"),
    ("Tone search", ""),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShortcutMap {
    pub digit: u8,
}

impl View for ShortcutMap {
    fn name(self) -> &'static str {
        "Shortcut map"
    }

    fn title(self, _: &App, b: &mut ferrite_ui::text::Buf<10>) {
        b.set(b"Shortcuts");
    }

    fn key(self, _: &mut App, _: KeyEvent, _: &impl ChannelStore) {}

    /// The digit came up: home, then its shortcut (HOME-8).
    fn tick(self, app: &mut App, _: &LiveAll) {
        if !app.keys.is_held(Key::Digit(self.digit)) {
            app.screen = Screen::Home;
            nav::hold_digit(app, self.digit);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let no_wx = app.radio.region.is_some_and(|r| !r.has_weather());
        for (i, (name, what)) in SHORTCUTS.iter().enumerate() {
            let r = Row {
                num: Some(i as u8),
                name: name.as_bytes(),
                value: what.as_bytes(),
                dot: false,
                selected: i as u8 == self.digit,
                grey: i == 7 && no_wx, // WX-1
            };
            app.list_row(lcd, 1 + i, 24 + 26 * i as i32, 26, &r);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("Release", "Run it"), ("BACK", "Hold: home")])
    }
}
