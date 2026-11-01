//! The event log (chapters 24 and 27): every source's row and detail, SAME headers decoded, the filter, Go, and
//! the wrapped message. The sample's log has APRS, pager and SAME events; the rest come from a store of their own.

use super::*;
use crate::chan::{Channel, ChannelStore, Name};
use crate::data::{Data, Event, Fix, Pos, Src};
use crate::screens::log::Log;
use ferrite_core::freq::mhz;
use ferrite_ui::text::Buf;

/// A store with only these events (and the sample's fix, for distances).
struct Events {
    ev: Vec<Event>,
    fix: Option<Fix>,
}

impl Data for Events {
    fn version(&self) -> u32 {
        1
    }
    fn events(&self) -> u16 {
        self.ev.len() as u16
    }
    fn event(&self, i: u16) -> Option<Event> {
        self.ev.get(i as usize).copied()
    }
    fn fix(&self) -> Option<Fix> {
        self.fix
    }
}

impl ChannelStore for Events {
    fn zones(&self) -> crate::chan::ZoneId {
        0
    }
    fn zone_name(&self, _: crate::chan::ZoneId) -> Option<Name> {
        None
    }
    fn zone_len(&self, _: crate::chan::ZoneId) -> u16 {
        0
    }
    fn zone_at(&self, _: crate::chan::ZoneId, _: u16) -> Option<Channel> {
        None
    }
    fn count(&self) -> u16 {
        0
    }
    fn data(&self) -> &dyn Data {
        self
    }
}

fn ev(src: Src, id: u32, x: u16, y: u16, freq: u32, name: &str, text: &str, pos: Option<Pos>) -> Event {
    Event { src, ago_s: 300, freq, slot: 1, unread: true, id, x, y, pos, name: Buf::from(name.as_bytes()), text: Buf::from(text.as_bytes()) }
}

fn store() -> Events {
    let near = Pos { lat: 454_300_000, lon: -756_900_000 };
    Events {
        ev: std::vec![
            ev(Src::Dsc, 316_001_234, 3, 0, mhz(156) + 52_500, "", "Distress: sinking", Some(near)),
            ev(Src::Ais, 316_005_678, 270, 125, mhz(161) + 97_500, "MAPLE LEAF", "", Some(near)),
            ev(Src::Dtmf, 0, 0, 0, mhz(146) + 52_000, "", "*79#", None),
            ev(Src::Selcall, 12345, 0, 0, 0, "ZVEI", "12345", None),
            ev(Src::Gps, 0, 0, 0, 0, "Club shack", "Arrived, 40 m", None),
            ev(Src::Same, 0, 0, 0, mhz(162) + 55_000, "", "ZCZC-CIV-CEM-035001+0130-2761410-KOTT/NWS-", None),
            ev(Src::Same, 0, 0, 0, mhz(162) + 55_000, "", "ZCZC-EAS-XYZ-035001-035003-035005+0200-2761410-KOTT/NWS-", None),
            ev(Src::Same, 0, 0, 0, mhz(162) + 55_000, "", "ZCZC-PEP-EAN-035001", None),
            ev(Src::Same, 0, 0, 0, mhz(162) + 55_000, "", "ZCZC-ABC-RWT-035001+0010-27614", None),
            ev(Src::Pocsag, 1_234_567, 1200, 3, mhz(439) + 98_750, "", "AVERYLONGWORDWITHOUTANYSPACESTHATCANNOTWRAPATALLONONELINEOFTHEBOX", None),
        ],
        fix: Sample::sample_fix(),
    }
}

/// Opens the log on `st` and the detail of event `i`.
fn detail(t: &mut T, st: &Events, i: u16) {
    t.app.screen = Screen::Log(Log { cur: crate::screens::lists::Cursor { sel: i, top: i.saturating_sub(3) }, detail: true, ..Log::new() });
    t.app.render(&mut t.fb, &t.live, &t.dev, st);
}

/// INBOX-6, INBOX-8: each source's row (who, what) in the list.
#[test]
fn rows_for_every_source() {
    let mut t = T::new();
    let st = store();
    t.open(Dest::EventLog);
    t.app.render(&mut t.fb, &t.live, &t.dev, &st);
    let s = t.texts_in(&st);
    for want in ["MMSI 316001234", "MAPLE LEAF", "270\u{b0} \u{b7} 12.5 kn", "*79#", "Slot B", "ZVEI 12345"] {
        assert!(s.contains(want), "{want:?} in {s}");
    }
}

/// INBOX-8: DSC's detail: the category first (red for distress), the MMSI and call, the position with its
/// distance and bearing from here.
#[test]
fn dsc_detail() {
    let mut t = T::new();
    let st = store();
    t.open(Dest::EventLog);
    detail(&mut t, &st, 0);
    let s = t.texts_in(&st);
    for want in ["Marine DSC", "Distress", "316001234", "Distress: sinking", "Position", "Distance"] {
        assert!(s.contains(want), "{want:?} in {s}");
    }
    assert!(t.px(50, 80, ferrite_ui::gfx::RED) > 10, "distress in red");
}

/// INBOX-8: AIS, DTMF, Selcall and waypoint arrivals each name their fields.
#[test]
fn other_details() {
    let mut t = T::new();
    let st = store();
    t.open(Dest::EventLog);
    for (i, wants) in [
        (1, &["AIS position", "Vessel", "MAPLE LEAF", "Course"][..]),
        (2, &["DTMF sequence", "Sequence", "*79#", "Slot"][..]),
        (3, &["Selcall", "Format", "ZVEI", "Unit", "Sent"][..]),
        (4, &["Waypoint arrival", "Waypoint", "Club shack", "Arrival", "Arrived, 40 m"][..]),
    ] {
        detail(&mut t, &st, i);
        let s = t.texts_in(&st);
        for want in wants {
            assert!(s.contains(want), "{want:?} in {s}");
        }
    }
}

/// WX-11: a SAME header decoded: who sent it (civil authority, broadcast station, primary entry point), the
/// event (by code, else "Alert"), the areas, how long it lasts (minutes, hours, both), when it was issued.
#[test]
fn same_details() {
    let mut t = T::new();
    let st = store();
    t.open(Dest::EventLog);
    for (i, wants) in [
        (5, &["Weather alert (SAME)", "Civil Emergency Message", "Civil authority", "1 h 30", "14:10 UTC, day 276"][..]),
        (6, &["Broadcast station", "Alert", "035001 and 2 more", "2 h"][..]),
        (7, &["Primary entry point", "Emergency Action", "Notification"][..]),
        (8, &["Required Weekly Test", "10 min"][..]),
    ] {
        detail(&mut t, &st, i);
        let s = t.texts_in(&st);
        for want in wants {
            assert!(s.contains(want), "{want:?} in {s}");
        }
        assert!(!s.contains("and 0 more"), "one area is named alone: {s}");
    }
}

/// A word longer than the line is cut where it fits rather than lost.
#[test]
fn long_word_wraps() {
    let mut t = T::new();
    let st = store();
    t.open(Dest::EventLog);
    detail(&mut t, &st, 9);
    let s = t.texts_in(&st);
    assert!(s.contains("AVERYLONGWORD"), "{s}");
    assert!(s.contains(" TCANNOTWRAP"), "the rest on the next line: {s}");
}

/// INBOX-9, INBOX-10: \u{25c0} from every source goes to the last source (none here: "other sources"); in a detail
/// \u{25c0}\u{25b6} step the events; # with no frequency does nothing.
#[test]
fn filter_step_and_go() {
    let mut t = T::new();
    t.open(Dest::EventLog);
    t.tap(Key::Left); // the last source: GPS
    assert!(matches!(t.screen(), Screen::Log(l) if l.filter == Src::ALL.len() as u8));
    t.shows("Left and right: other sources");
    t.tap(Key::Right); // every source again
    t.tap(Key::Menu); // the newest's detail
    t.tap(Key::Right);
    assert!(matches!(t.screen(), Screen::Log(l) if l.detail && l.cur.sel == 1));
    t.tap(Key::Left);
    assert!(matches!(t.screen(), Screen::Log(l) if l.cur.sel == 0));
    // the sample's SAME event, opened from the list
    t.tap(Key::Right);
    t.tap(Key::Right);
    t.shows("Severe Thunderstorm");
    t.shows("Warning");
    t.shows("Weather service");
    // an event without a frequency: # does nothing
    let st = store();
    detail(&mut t, &st, 3);
    let f = t.app.slot(t.app.active).freq();
    t.tap_in(Key::Hash, &st);
    assert_eq!(t.app.slot(t.app.active).freq(), f);
}
