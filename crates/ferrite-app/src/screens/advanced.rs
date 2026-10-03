//! Advanced settings (MENU 0, or hold MENU on home; manual chapter 11, MENU-2, MENU-8; MenuTop mock-up): the
//! Recent row and the nine groups, each with a summary value. A digit or OK opens a group ([`Dest::Group`]);
//! 0 opens the Status page (SBAR-13).

use super::nav::Dest;
use super::shared::*;
use super::status::Status;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::GpsState;
use ferrite_core::VERSION;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Advanced {
    /// The highlighted group, 0-8 (MENU 0 1 to MENU 0 9).
    pub row: u8,
    /// Opened from quick settings: BACK returns there (MENU-24), else home.
    pub from_quick: bool,
}

/// Advanced settings groups (MENU 0 1-9) and their help lines.
pub(crate) const GROUPS: [(&str, &str); 9] = [
    ("Watch and scan", "Watch, background jobs, scan, tone search."),
    ("Channels", "Browse channels and zones, add a repeater."),
    ("Listen", "Broadcast radio, band scope, weather alerts."),
    ("Decoders", "Event log, POCSAG, DSC, AIS, DTMF, popups."),
    ("APRS", "Heard, messages, beacons, your station."),
    ("GPS and time", "GPS, position, units, time zone, clock."),
    ("Display, sound, power", "Theme, layout, backlight, sounds, power."),
    ("Transmit", "Timeout, VOX, mic gain, DTMF, callsign, ID."),
    ("System", "Keys, links, PIN, region, receiver, reset."),
];

impl View for Advanced {
    fn name(self) -> &'static str {
        "Advanced"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("MenuTop.png")
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let Advanced { row, from_quick } = self;
        // The group's row stays highlighted, so BACK from the group lands on it (MENU-24).
        let open = |app: &mut App, r: u8| {
            app.screen = Screen::Advanced(Advanced { row: r, from_quick });
            app.open(Dest::Group(r + 1));
        };
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => app.screen = Screen::Advanced(Advanced { row: (row + 8) % 9, from_quick }),
            (Key::Down, Short | Repeat) => app.screen = Screen::Advanced(Advanced { row: (row + 1) % 9, from_quick }),
            (Key::Digit(0), Short) => app.screen = Screen::Status(Status { row: 0, from_menu: true }), // SBAR-13
            (Key::Digit(d), Short) => open(app, d - 1),
            (Key::Menu, Short) => open(app, row),
            (Key::Exit, Short) => {
                // MENU-24: up one level, keeping the place
                app.screen = if from_quick { Screen::Quick(super::quick::Quick { row: 9 }) } else { Screen::Home };
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.screen = Screen::Advanced(Advanced { row: (self.row as i32 + d).rem_euclid(9) as u8, ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (row, dev) = (self.row, cx.dev);
        let recent = app.recent;
        if app.dirty(R_TOP, hash(&recent)) {
            w::field(lcd, 0, 54, 21, 40, 35, Align::Left, b"", &UI_12, SUB, BG);
            w::field(lcd, 8, 46, 21, 40, 35, Align::Left, b"Recent", &UI_12, SUB, BG);
            let names: [&[u8]; 3] = recent.map(|r| r.map_or(b"".as_slice(), |i| i.label().as_bytes()));
            if names[0].is_empty() {
                w::field(lcd, 54, 178, 21, 40, 35, Align::Left, b"nothing yet", &UI_12, DIM, BG);
            } else {
                lcd.fill(54, 21, 178, 1, BG);
                w::chips(lcd, 54, 22, 178, &names, BG);
                lcd.fill(54, 39, 178, 1, BG);
            }
        }
        for (i, (name, _)) in GROUPS.iter().enumerate() {
            let mut v = Buf::<24>::new();
            match i {
                0 => {
                    v.push(WATCH_TEXT[app.watch as usize % 3].as_bytes());
                    if app.scan_mask != 0 {
                        v.push(" \u{b7} scan".as_bytes());
                    }
                }
                1 => {
                    let _ = uwrite!(v, "{} used", cx.store.count());
                }
                3 => v.set(b"None running"),
                4 if app.inbox > 0 => {
                    let _ = uwrite!(v, "{} new", app.inbox);
                }
                5 => match dev.gps {
                    GpsState::Off => v.set(b"GPS off"),
                    GpsState::Searching => v.set(b"Searching"),
                    GpsState::Fix => {
                        let _ = uwrite!(v, "{} sats", dev.sats);
                    }
                },
                6 => v.set(b"Dark"),
                7 => v.set(if dev.tx_built { b"Ready".as_slice() } else { b"Receive only" }),
                8 => v.set(VERSION.as_bytes()),
                _ => {}
            }
            let r = Row { num: Some(i as u8 + 1), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: i as u8 == row, grey: false };
            app.list_row(lcd, i, 41 + 22 * i as i32, 22, &r);
        }
        let mut p = Buf::<16>::new();
        let _ = uwrite!(p, "MENU 0 {}", row + 1);
        app.help_box(lcd, 242, 34, GROUPS[row as usize].1.as_bytes(), p.as_bytes());
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("1-9", "Jump"), ("OK", "Open"), ("BACK", "Back")])
    }
}
