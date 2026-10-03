//! The Status page (# on home, or MENU 0 0; manual chapter 8, SBAR-13 to SBAR-18; StatusPage mock-up): one
//! row per radio-wide state; OK on a row opens its setting.

use super::nav::Dest;
use super::quick::Quick;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_name, Item};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::GpsState;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Status {
    pub row: u8,
    /// Opened from advanced settings (MENU 0 0): BACK returns there, else home.
    pub from_menu: bool,
}

/// The Status page's rows (SBAR-14).
pub(crate) const STATUS: [&str; 13] = [
    "Transmit",
    "Region",
    "Extend TX",
    "Inbox",
    "Scan",
    "Watch",
    "Jobs",
    "Beacon",
    "Battery Save",
    "GPS",
    "Bluetooth",
    "Battery",
    "Key lock",
];
const ROWS: u8 = STATUS.len() as u8;

impl View for Status {
    fn name(self) -> &'static str {
        "Status"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("StatusPage.png")
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let Status { row, from_menu } = self;
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => app.screen = Screen::Status(Status { row: (row + ROWS - 1) % ROWS, from_menu }),
            (Key::Down, Short | Repeat) => app.screen = Screen::Status(Status { row: (row + 1) % ROWS, from_menu }),
            // SBAR-15: OK on a row opens its setting.
            (Key::Menu, Short) => match STATUS[row as usize] {
                "Watch" => app.screen = Screen::Quick(Quick { row: 6 }),
                "Battery Save" => app.screen = Screen::Quick(Quick { row: 8 }),
                "Inbox" => app.open(Dest::EventLog),
                "GPS" => app.open(Dest::GpsPosition),
                "Bluetooth" => app.open(Dest::Sub(9, 2)),
                "Region" => app.open(Dest::Group(9)),
                "Extend TX" | "Transmit" => app.open(Dest::Group(8)),
                name => app.show_toast(name.as_bytes(), b"setting comes next", false),
            },
            (Key::Exit, Short) => {
                app.screen = if from_menu { Screen::Advanced(super::advanced::Advanced { row: 0, from_quick: false }) } else { Screen::Home }
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.screen = Screen::Status(Status { row: (self.row as i32 + d).rem_euclid(ROWS as i32) as u8, ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (live, dev) = (cx.live, cx.dev);
        for (i, label) in STATUS.iter().enumerate() {
            let mut v = Buf::<28>::new();
            let mut c = TEXT;
            match i {
                0 => {
                    // SBAR-14: why transmit is locked, if it is.
                    if dev.tx_built {
                        (v, c) = (Buf::from(b"Ready"), GREEN);
                    } else {
                        (v, c) = (Buf::from(b"RX-only build"), AMBER);
                    }
                }
                1 => v.set(b"Not set"),
                2 | 6 | 7 => v.set(if i == 6 { b"None".as_slice() } else { b"Off" }),
                3 => {
                    if app.inbox == 0 {
                        v.set(b"Empty");
                    } else {
                        let _ = uwrite!(v, "{} unread", app.inbox);
                    }
                }
                4 => {
                    for (s, l) in live.slots.iter().enumerate().filter(|(_, l)| l.scanning) {
                        if !v.is_empty() {
                            v.push(b", ");
                        }
                        let _ = uwrite!(v, "{}: {}", w::LETTERS[s] as char, band_name(l.freq));
                    }
                    if v.is_empty() {
                        v.set(b"Off");
                    }
                }
                5 => {
                    v.set(WATCH_TEXT[app.watch as usize % 3].as_bytes());
                    if app.paused_until.is_some() {
                        v.push(" \u{b7} paused".as_bytes());
                    }
                }
                8 => {
                    let mut b = Buf::<16>::new();
                    app.item_text(Item::BatterySave, true, &mut b);
                    v.set(b.as_bytes());
                }
                9 => match dev.gps {
                    GpsState::Off => v.set(b"Off"),
                    GpsState::Searching => v.set(b"Searching"),
                    GpsState::Fix => {
                        let _ = uwrite!(v, "Fix \u{b7} {} sats", dev.sats);
                    }
                },
                10 => v.set(if dev.bt { b"Connected".as_slice() } else { b"Off" }),
                11 => {
                    let mv = dev.battery_mv as u32;
                    let _ = uwrite!(v, "{}.{} V \u{b7} {}%", mv / 1000, mv % 1000 / 100, dev.battery_pct);
                    if dev.battery_cutoff {
                        c = RED;
                    } else if dev.battery_low {
                        c = AMBER;
                    }
                }
                _ => v.set(if app.locked { b"On".as_slice() } else { b"Off" }),
            }
            app.kv_row(lcd, i, 20 + 18 * i as i32, 18, label.as_bytes(), v.as_bytes(), c, i as u8 == self.row);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")])
    }
}
