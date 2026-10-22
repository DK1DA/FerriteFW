//! A screen's own options (MENU-25; D4): hold MENU on the event log, APRS heard, the station, messages, the GPS
//! screens or the band scope. Each list of options is a table of rows: a label and what OK does (a request to
//! the record's owner, the same after a confirm (MENU-23), or a screen). One style for every options page (FM
//! broadcast and HF mode draw theirs the same way): numbered rows, digits jump, "0 Full menu" opens advanced
//! settings, the help box names the page. BACK returns. Screens without options answer hold MENU with a toast
//! ([`crate::screens::View::options`]).

use super::nav::Dest;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use super::contacts::Sel;
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
    /// A station's page (APRS-41) and the Contacts screen (APRS-40).
    Station,
    Contacts,
}

impl Of {
    pub fn screen(self) -> Screen {
        Screen::Options(Options { of: self, row: 0, confirm: false })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    /// A reference, not the `Op` itself: `Op` is about 96 bytes (it carries drafts and buffers) and every row would
    /// be that size.
    Do(&'static Op),
    /// Asks first (MENU-23).
    Confirm(&'static Op),
    Open(Dest),
    /// Show hidden channels in the lists, or not (a setting, kept).
    ShowHidden,
    /// Beacons off, or on again in their mode (APRS-17).
    Beacon,
    /// A waypoint typed as a grid square (GPS-21): text entry.
    Grid,
    /// The arrival alarm's next radius (GPS-12, 0 6 7): off, 50 m ... 1 km.
    Arrival,
    /// On the station, contact or waypoint under the page; true asks first (MENU-23).
    Sel(super::contacts::Sel, bool),
}

pub(crate) type Opt = (&'static str, Act, &'static str);

/// The rows: label, action, help line.
const LOG: [Opt; 3] = [
    ("Mark all read", Act::Do(&Op::ReadAll), "Clears the inbox count"),
    ("Clear the log", Act::Confirm(&Op::ClearLog), "Every event goes, unread ones too"),
    ("Decoder settings", Act::Open(Dest::Group(4)), "Pagers, DSC, AIS, popups"),
];
const HEARD: [Opt; 5] = [
    ("Beacon now", Act::Do(&Op::BeaconNow), "Sends your position once (needs a callsign)"),
    ("Beacon on/off", Act::Beacon, "Automatic beacons, in their mode (0 5 5 1)"),
    ("Messages", Act::Open(Dest::AprsMessages), "Inbox, sent and compose"),
    ("Clear the heard list", Act::Confirm(&Op::ClearHeard), "Stations come back as they are heard"),
    ("APRS settings", Act::Open(Dest::Group(5)), "Beacon, path, symbol, frequency"),
];
const MESSAGES: [Opt; 4] = [
    ("Write a message", Act::Open(Dest::Compose), "Recipient, then the text"),
    ("Quick messages", Act::Open(Dest::QuickMessages), "The nine shown on an empty message"),
    ("Contacts", Act::Open(Dest::Contacts), "Saved stations and their names"),
    ("APRS settings", Act::Open(Dest::Group(5)), "Ringer, path, retries"),
];
const GPS: [Opt; 3] = [
    ("Mark here", Act::Do(&Op::MarkHere), "A waypoint at this fix"),
    ("Waypoints", Act::Open(Dest::Waypoints), "Distance and bearing to each"),
    ("GPS settings", Act::Open(Dest::Group(6)), "On or off, format, units, time zone"),
];
const SCOPE: [Opt; 1] = [("Scope settings", Act::Open(Dest::Sub(3, 2)), "Bars, step, speed, trigger, waterfall")];
/// APRS-17 (decision 181): OK on home with the APRS slot active opens this page on its "Beacon now" row (beacons
/// alone hold C) or its "Heard list" row (APRS mode, KISS).
pub(crate) const APRS_ROW_BEACON_NOW: u8 = 0;
pub(crate) const APRS_ROW_HEARD: u8 = 2;
const APRS_CARD: [Opt; 4] = [
    ("Beacon now", Act::Do(&Op::BeaconNow), "Sends your position once (needs a callsign)"),
    ("Beacon on/off", Act::Beacon, "Automatic beacons, in their mode (0 5 5 1)"),
    ("Heard list", Act::Open(Dest::AprsHeard), "Stations heard, newest first"),
    ("Messages", Act::Open(Dest::AprsMessages), "Inbox, sent and compose"),
];
const CHANNELS: [Opt; 1] = [("Show hidden channels", Act::ShowHidden, "Hidden channels stay reachable by number")];
const WAYPOINTS: [Opt; 5] = [
    ("Mark here", Act::Do(&Op::MarkHere), "A waypoint at this fix"),
    ("Add a grid square", Act::Grid, "A waypoint at a square's centre: FN31pr"),
    ("Arrival alarm", Act::Arrival, "Off, 50 m to 1 km from the one you go to"),
    ("Use as fixed position", Act::Sel(Sel::Fixed, false), "This waypoint, used when there's no fix"),
    ("GPS position", Act::Open(Dest::GpsPosition), "Fix, satellites, grid square"),
];
const STATION: [Opt; 3] = [
    ("Message", Act::Sel(Sel::Message, false), "Write this station a message"),
    ("Save contact", Act::Sel(Sel::Name, false), "Its callsign and a name; renames a saved one"),
    ("Save waypoint", Act::Sel(Sel::Waypoint, false), "Its last position, named with its callsign"),
];
const CONTACTS: [Opt; 3] = [
    ("Edit name", Act::Sel(Sel::Name, false), "Up to 12 characters"),
    ("Delete", Act::Sel(Sel::Delete, true), "This contact goes; messages stay"),
    ("Station", Act::Sel(Sel::Station, false), "Its page, when it is in the heard list"),
];

pub(crate) fn rows(of: Of) -> &'static [Opt] {
    match of {
        Of::Log => &LOG,
        Of::Heard => &HEARD,
        Of::Messages => &MESSAGES,
        Of::Gps => &GPS,
        Of::Waypoints => &WAYPOINTS,
        Of::Scope => &SCOPE,
        Of::Channels => &CHANNELS,
        Of::AprsCard => &APRS_CARD,
        Of::Station => &STATION,
        Of::Contacts => &CONTACTS,
    }
}

/// The page's name, for the help box ("Event log MENU").
pub(crate) fn title(of: Of) -> &'static [u8] {
    match of {
        Of::Log => b"Event log",
        Of::Heard => b"APRS heard",
        Of::Messages => b"APRS messages",
        Of::Gps => b"GPS position",
        Of::Waypoints => b"Waypoints",
        Of::Scope => b"Scope",
        Of::Channels => b"Channels",
        Of::AprsCard => b"APRS slot",
        Of::Station => b"APRS station",
        Of::Contacts => b"Contacts",
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
                Act::Confirm(_) | Act::Sel(_, true) if !self.confirm => app.screen = Screen::Options(Options { confirm: true, ..self }),
                Act::Sel(s, _) => {
                    app.back();
                    super::contacts::sel(app, s, store);
                }
                Act::Do(&op) | Act::Confirm(&op) => {
                    if matches!(op, Op::BeaconNow | Op::Send) && app.remote_tx_refused() {
                        return; // LINK-19: a beacon or message now is a transmission
                    }
                    app.back();
                    if store.data().request(op) {
                        if op == Op::ReadAll {
                            app.log_unread = 0; // the log's: the mail badge counts messages (INBOX-3.1)
                        }
                        app.show_toast(label.as_bytes(), b"done", false);
                    } else {
                        let why: &[u8] = match op {
                            Op::BeaconNow if !app.set.callsign.as_bytes().is_empty() => b"callsign over 6", // SETUP-13.1
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
                Act::Arrival => {
                    let n = crate::settings::ARRIVAL_M.len() as u8;
                    app.set.arrival = (app.set.arrival + 1) % n;
                    app.back();
                    let m = crate::settings::ARRIVAL_M[app.set.arrival as usize];
                    let mut v = Buf::<12>::new();
                    match m {
                        0 => v.set(b"off"),
                        1000 => v.set(b"1 km"),
                        _ => {
                            let _ = ufmt::uwrite!(v, "{} m", m);
                        }
                    }
                    app.show_toast(b"Arrival alarm", v.as_bytes(), false);
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
