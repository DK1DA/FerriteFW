//! A screen's own options (MENU-25; D4): hold MENU on the event log, APRS heard, the station, messages, the GPS
//! screens or the band scope. Each list of options is a table of rows: a label and what OK does (a request to
//! the record's owner, the same after a confirm (MENU-23), or a screen). One style for every options page (FM
//! broadcast and HF mode draw theirs the same way): numbered rows, digits jump, "0 Full menu" opens advanced
//! settings, the help box names the page. BACK returns. Screens without options answer hold MENU with a toast
//! ([`crate::screens::View::options`]).

use super::nav::Dest;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::Op;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::Row;

/// Whose options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Of {
    Log,
    Heard,
    Messages,
    Gps,
    Waypoints,
    /// The band scope (SCOPE): its settings list.
    Scope,
    /// The channel browser: show hidden channels (decision 63).
    Channels,
    /// Home's APRS card (APRS-17).
    AprsCard,
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
    /// Show hidden channels in the lists, or not (a setting, kept).
    ShowHidden,
    /// Beacons off, or on again in their mode (APRS-17).
    Beacon,
    /// A waypoint typed as a grid square (GPS-21): text entry.
    Grid,
}

type Opt = (&'static str, Act, &'static str);

/// The rows: label, action, help line.
const LOG: [Opt; 3] = [
    ("Mark all read", Act::Do(Op::ReadAll), "Clears the inbox count"),
    ("Clear the log", Act::Confirm(Op::ClearLog), "Every event goes, unread ones too"),
    ("Decoder settings", Act::Open(Dest::Group(4)), "Pagers, DSC, AIS, popups"),
];
const HEARD: [Opt; 5] = [
    ("Beacon now", Act::Do(Op::BeaconNow), "Sends your position once (needs a callsign)"),
    ("Beacon on/off", Act::Beacon, "Automatic beacons, in their mode (0 5 5 1)"),
    ("Messages", Act::Open(Dest::AprsMessages), "Inbox, sent and compose"),
    ("Clear the heard list", Act::Confirm(Op::ClearHeard), "Stations come back as they are heard"),
    ("APRS settings", Act::Open(Dest::Group(5)), "Beacon, path, symbol, frequency"),
];
const MESSAGES: [Opt; 3] = [
    ("Write a message", Act::Open(Dest::Compose), "Recipient, then the text"),
    ("Quick messages", Act::Open(Dest::QuickMessages), "The nine shown on an empty message"),
    ("APRS settings", Act::Open(Dest::Group(5)), "Ringer, path, retries"),
];
const GPS: [Opt; 3] = [
    ("Mark here", Act::Do(Op::MarkHere), "A waypoint at this fix"),
    ("Waypoints", Act::Open(Dest::Waypoints), "Distance and bearing to each"),
    ("GPS settings", Act::Open(Dest::Group(6)), "On or off, format, units, time zone"),
];
const SCOPE: [Opt; 1] = [("Scope settings", Act::Open(Dest::Sub(3, 2)), "Bars, step, speed, trigger, waterfall")];
const APRS_CARD: [Opt; 4] = [
    ("Beacon now", Act::Do(Op::BeaconNow), "Sends your position once (needs a callsign)"),
    ("Beacon on/off", Act::Beacon, "Automatic beacons, in their mode (0 5 5 1)"),
    ("Heard list", Act::Open(Dest::AprsHeard), "Stations heard, newest first"),
    ("Messages", Act::Open(Dest::AprsMessages), "Inbox, sent and compose"),
];
const CHANNELS: [Opt; 1] = [("Show hidden channels", Act::ShowHidden, "Hidden channels stay reachable by number")];
const WAYPOINTS: [Opt; 3] = [
    ("Mark here", Act::Do(Op::MarkHere), "A waypoint at this fix"),
    ("Add a grid square", Act::Grid, "A waypoint at a square's centre: FN31pr"),
    ("GPS position", Act::Open(Dest::GpsPosition), "Fix, satellites, grid square"),
];

fn rows(of: Of) -> &'static [Opt] {
    match of {
        Of::Log => &LOG,
        Of::Heard => &HEARD,
        Of::Messages => &MESSAGES,
        Of::Gps => &GPS,
        Of::Waypoints => &WAYPOINTS,
        Of::Scope => &SCOPE,
        Of::Channels => &CHANNELS,
        Of::AprsCard => &APRS_CARD,
    }
}

/// The page's name, for the help box ("Event log MENU").
fn title(of: Of) -> &'static [u8] {
    match of {
        Of::Log => b"Event log",
        Of::Heard => b"APRS heard",
        Of::Messages => b"APRS messages",
        Of::Gps => b"GPS position",
        Of::Waypoints => b"Waypoints",
        Of::Scope => b"Scope",
        Of::Channels => b"Channels",
        Of::AprsCard => b"APRS slot",
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
            // MENU-25: 0 the full menu; MENU-5: a digit picks its row.
            (Key::Digit(0), Short) if !self.confirm => app.open(Dest::Advanced),
            (Key::Digit(d), Short) if !self.confirm && (d as usize) <= all.len() => {
                app.screen = Screen::Options(Options { row: d - 1, ..self })
            }
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
                Act::Beacon => {
                    app.back();
                    app.toggle_beacon();
                }
                Act::Grid => {
                    app.back();
                    super::text::open(app, super::text::TextFor::GridWaypoint, store);
                }
                Act::ShowHidden => {
                    app.set.show_hidden = !app.set.show_hidden;
                    app.back();
                    let on: &[u8] = if app.set.show_hidden { b"shown" } else { b"left out" };
                    app.show_toast(b"Hidden channels", on, false);
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
        options_page(app, lcd, all.iter().map(|(l, a, _)| (l.as_bytes(), if matches!(a, Act::Open(_)) { b">".as_slice() } else { b"" })), self.row);
        let (label, _, help) = all[self.row as usize % all.len()];
        if self.confirm {
            let mut q = Buf::<32>::new();
            q.push(label.as_bytes());
            q.push(b"?");
            app.confirm_box(lcd, q.as_bytes(), help.as_bytes());
        } else {
            let mut p = Buf::<24>::new();
            p.set(title(self.of));
            p.push(b" MENU");
            app.help_box(lcd, OPTS_HELP_Y, 46, help.as_bytes(), p.as_bytes());
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.confirm {
            Hints::new(&[("OK", "Confirm"), ("BACK", "Keep")])
        } else {
            Hints::new(&[("OK", "Do it"), ("0", "Full menu"), ("BACK", "Back")])
        }
    }
}

/// Where an options page's help box goes (under five rows and "0 Full menu").
pub(crate) const OPTS_HELP_Y: i32 = 140;

/// An options page's rows, the one style (MENU-25): numbered 1-n with their values, the selected one
/// highlighted, then "0 Full menu".
pub(crate) fn options_page<'a>(app: &mut App, lcd: &mut impl Panel, rows: impl Iterator<Item = (&'a [u8], &'a [u8])>, sel: u8) {
    let mut n = 0;
    for (i, (name, value)) in rows.enumerate() {
        let r = Row { num: Some(i as u8 + 1), name, value, dot: false, selected: i as u8 == sel, grey: false };
        app.list_row(lcd, i, 23 + 22 * i as i32, 22, &r);
        n = i + 1;
    }
    let r = Row { num: Some(0), name: b"Full menu", value: b">", dot: false, selected: false, grey: false };
    app.list_row(lcd, n, 23 + 22 * n as i32, 22, &r);
}
