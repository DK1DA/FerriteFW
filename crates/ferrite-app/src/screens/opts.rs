//! A screen's own options (MENU-25; D4 for the list screens): hold MENU on the event log, APRS heard, the
//! station, messages or the GPS screen. Each list of options is a table of rows: a label and what OK does
//! (a request to the record's owner, the same after a confirm (MENU-23), or a screen). BACK returns.

use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::Op;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;

/// Whose options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Of {
    Log,
    Heard,
    Messages,
    Gps,
    Waypoints,
}

impl Of {
    pub fn screen(self) -> Screen {
        Screen::Options(Options { of: self, row: 0, confirm: false })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Do(Op),
    /// Asks first (MENU-23).
    Confirm(Op),
    Open(Dest),
}

type Opt = (&'static str, Act, &'static str);

/// The rows: label, action, help line.
const LOG: [Opt; 3] = [
    ("Mark all read", Act::Do(Op::ReadAll), "Clears the inbox count"),
    ("Clear the log", Act::Confirm(Op::ClearLog), "Every event goes, unread ones too"),
    ("Decoder settings", Act::Open(Dest::Group(4)), "Pagers, DSC, AIS, sensors, popups"),
];
const HEARD: [Opt; 4] = [
    ("Beacon now", Act::Do(Op::BeaconNow), "Sends your position once (needs a callsign)"),
    ("Messages", Act::Open(Dest::AprsMessages), "Inbox, sent and compose"),
    ("Clear the heard list", Act::Confirm(Op::ClearHeard), "Stations come back as they are heard"),
    ("APRS settings", Act::Open(Dest::Group(5)), "Beacon, path, symbol, frequency"),
];
const MESSAGES: [Opt; 2] = [
    ("Write a message", Act::Open(Dest::Compose), "Recipient, then the text"),
    ("APRS settings", Act::Open(Dest::Group(5)), "Ringer, path, retries"),
];
const GPS: [Opt; 3] = [
    ("Mark here", Act::Do(Op::MarkHere), "A waypoint at this fix"),
    ("Waypoints", Act::Open(Dest::Waypoints), "Distance and bearing to each"),
    ("GPS settings", Act::Open(Dest::Group(6)), "On or off, format, units, time zone"),
];
const WAYPOINTS: [Opt; 2] = [
    ("Mark here", Act::Do(Op::MarkHere), "A waypoint at this fix"),
    ("GPS position", Act::Open(Dest::GpsPosition), "Fix, satellites, grid square"),
];

fn rows(of: Of) -> &'static [Opt] {
    match of {
        Of::Log => &LOG,
        Of::Heard => &HEARD,
        Of::Messages => &MESSAGES,
        Of::Gps => &GPS,
        Of::Waypoints => &WAYPOINTS,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    pub of: Of,
    pub row: u8,
    pub confirm: bool,
}

impl View for Options {
    fn name(self) -> &'static str {
        "Options"
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let all = rows(self.of);
        let (label, act, _) = all[self.row as usize % all.len()];
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) if !self.confirm => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) => match act {
                Act::Confirm(_) if !self.confirm => app.screen = Screen::Options(Options { confirm: true, ..self }),
                Act::Do(op) | Act::Confirm(op) => {
                    app.back();
                    if store.data().request(op) {
                        if op == Op::ReadAll {
                            app.inbox = 0;
                        }
                        app.show_toast(label.as_bytes(), b"done", false);
                    } else {
                        let why: &[u8] = match op {
                            Op::BeaconNow => b"set a callsign first", // APRS-8
                            Op::MarkHere => b"needs a GPS fix",
                            _ => b"not possible now",
                        };
                        app.show_toast(label.as_bytes(), why, false);
                    }
                }
                Act::Open(d) => {
                    app.back();
                    app.open(d);
                }
            },
            (Key::Exit, Short) if self.confirm => app.screen = Screen::Options(Options { confirm: false, ..self }),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        if !self.confirm {
            let n = rows(self.of).len() as i32;
            app.screen = Screen::Options(Options { row: (self.row as i32 + d).rem_euclid(n) as u8, ..self });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let all = rows(self.of);
        app.page(lcd, (self.of, self.confirm));
        let mut h = Buf::<24>::new();
        h.push(match self.of {
            Of::Log => b"Event log".as_slice(),
            Of::Heard => b"APRS heard",
            Of::Messages => b"APRS messages",
            Of::Gps => b"GPS position",
            Of::Waypoints => b"Waypoints",
        });
        app.list_head(lcd, h.as_bytes(), b"options");
        for (i, (label, act, _)) in all.iter().enumerate() {
            let v: &[u8] = if matches!(act, Act::Open(_)) { b">" } else { b"" };
            app.row1(lcd, i, label.as_bytes(), v, false, i as u8 == self.row, false);
        }
        let (label, _, help) = all[self.row as usize % all.len()];
        if self.confirm {
            let mut q = Buf::<32>::new();
            q.push(label.as_bytes());
            q.push(b"?");
            app.confirm_box(lcd, q.as_bytes(), help.as_bytes());
        } else {
            app.help_box(lcd, 242, 34, help.as_bytes(), b"");
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.confirm {
            Hints::new(&[("OK", "Confirm"), ("BACK", "Keep")])
        } else {
            Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Do it"), ("BACK", "Back")])
        }
    }
}
