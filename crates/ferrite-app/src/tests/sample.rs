//! The simulators' store ([`Sample`]): each request does to the sample records what the Storage task does to SPI
//! flash, and answers false where the real one would refuse.

use crate::chan::{ChanWrite, ChannelStore, Name, NO_ZONE};
use crate::data::{Data, Draft, Op, RangePreset, ZoneCfg};
use crate::sample::Sample;
use ferrite_core::freq::mhz;
use ferrite_ui::text::Buf;

fn preset(name: &str) -> RangePreset {
    RangePreset { name: Buf::from(name.as_bytes()), start: mhz(430), end: mhz(440), step: 3, mode: 0, narrow: false, skips: 7, resume: 0 }
}

/// Every change bumps the version (the screens redraw on it); a refused one doesn't.
#[test]
fn changes_bump_the_version_refusals_dont() {
    let s = Sample::new();
    let v = s.version();
    assert!(s.request(Op::ReadAll));
    assert_eq!(s.version(), v + 1);
    assert!(!s.request(Op::DeleteChannel(999)), "no channel 999");
    assert_eq!(s.version(), v + 1);
    assert!(!s.request(Op::Queue(Buf::new(), Buf::new())), "the APRS task's, not the store's");
    assert!(!s.request(Op::Unqueue(Buf::new(), Buf::new())));
}

/// The log: read all, then clear.
#[test]
fn log_read_all_and_clear() {
    let s = Sample::new();
    assert!(s.events() > 0);
    assert!((0..s.events()).any(|i| s.event(i).unwrap().unread));
    assert!(s.request(Op::ReadAll));
    assert!((0..s.events()).all(|i| !s.event(i).unwrap().unread));
    assert!(s.request(Op::ClearLog));
    assert_eq!(s.events(), 0);
    assert!(s.event(0).is_none());
}

/// Messages: mark all read, read one, clear them; heard cleared.
#[test]
fn messages_and_heard() {
    let s = Sample::new();
    let n = s.messages();
    assert!(n > 0);
    assert!(s.request(Op::MessageRead(0)));
    assert!(s.request(Op::MessagesReadAll));
    assert!((0..n).all(|i| s.message(i).unwrap().state != crate::data::MsgState::Unread));
    assert!(s.request(Op::ClearMessages));
    assert_eq!(s.messages(), 0);
    assert!(s.message(0).is_none());
    assert!(s.stations() > 0);
    assert!(s.request(Op::ClearHeard));
    assert_eq!(s.stations(), 0);
    assert!(s.station(0).is_none());
}

/// A message sent from the draft is the store's first message, and reading it marks it read.
#[test]
fn a_sent_message_comes_first() {
    let s = Sample::new();
    let n = s.messages();
    assert!(!s.request(Op::Send), "no draft: nothing to send");
    assert!(s.request(Op::Draft(Draft { to: Buf::from(&b"VA3RTX"[..]), text: Buf::from(&b"hello"[..]) })));
    assert!(s.request(Op::Send));
    assert_eq!(s.messages(), n + 1);
    assert_eq!(s.draft().to.as_bytes(), b"", "the draft is sent and emptied");
    assert!(s.request(Op::MessageRead(0)), "a sent one: the APRS side's");
    assert!(s.request(Op::MessagesReadAll));
    assert!(s.request(Op::ClearMessages));
    assert_eq!(s.messages(), 0, "the sent ones go too");
}

/// Zones: a new one (up to eight), put one by number (the next one adds it), rename, delete (its channels to no zone,
/// the last one's number free again).
#[test]
fn zones_new_put_rename_delete() {
    let s = Sample::new();
    let n = s.zones();
    assert!(s.request(Op::PutZone(n, Name::from(&b"Hills"[..]), ZoneCfg::default())), "the next number adds a zone");
    assert_eq!(s.zones(), n + 1);
    assert_eq!(s.zone_name(n).unwrap().as_bytes(), b"Hills");
    assert!(!s.request(Op::PutZone(n + 2, Name::from(&b"Gap"[..]), ZoneCfg::default())), "not past the next");
    assert!(s.request(Op::PutZone(0, Name::from(&b"Home"[..]), ZoneCfg::default())), "one already there");
    assert_eq!(s.zone_name(0).unwrap().as_bytes(), b"Home");
    assert!(s.request(Op::NewZone));
    assert_eq!(s.zones(), 8);
    assert!(!s.request(Op::NewZone), "eight at most");
    assert!(!s.request(Op::PutZone(8, Name::from(&b"Nine"[..]), ZoneCfg::default())), "eight at most");
    assert!(s.request(Op::RenameZone(1, Name::from(&b"Rpt"[..]))));
    assert_eq!(s.zone_name(1).unwrap().as_bytes(), b"Rpt");
    assert!(!s.request(Op::RenameZone(9, Name::from(&b"No"[..]))));
    let in1 = s.zone_len(1);
    assert!(in1 > 0);
    assert!(s.request(Op::DeleteZone(1)));
    assert_eq!(s.zone_len(1), 0, "its channels have no zone now");
    assert_eq!(s.zones(), 8, "not the last: the count stays");
    assert!(s.request(Op::DeleteZone(7)));
    assert_eq!(s.zones(), 7, "the last one goes");
}

/// A channel write that deletes a zone moves its channels to no zone; erase all empties the store.
#[test]
fn channel_writes() {
    let s = Sample::new();
    let ch = s.zone_at(4, 0).unwrap();
    s.apply(ChanWrite::DeleteZone(4));
    assert_eq!(s.zone_len(4), 0);
    assert_eq!(s.by_number_num(ch.num).zone, NO_ZONE);
    s.apply(ChanWrite::Delete(ch.num));
    assert!(s.find(ch.num).is_none());
    s.apply(ChanWrite::EraseAll);
    assert_eq!(s.count(), 0);
}

trait Find {
    fn find(&self, num: u16) -> Option<crate::chan::Channel>;
    fn by_number_num(&self, num: u16) -> crate::chan::Channel {
        self.find(num).expect("channel")
    }
}

impl Find for Sample {
    fn find(&self, num: u16) -> Option<crate::chan::Channel> {
        (0..self.count()).filter_map(|i| self.by_number(i)).find(|c| c.num == num)
    }
}

/// Range presets: edit by index, a new one, put (the next index adds one, keeping no skips; an edit keeps the old
/// skips), delete.
#[test]
fn presets_new_put_delete() {
    let s = Sample::new();
    let n = s.presets();
    assert!(s.request(Op::Preset(0, preset("Edited"))));
    assert_eq!(s.preset(0).unwrap().name.as_bytes(), b"Edited");
    assert!(!s.request(Op::Preset(n, preset("None"))));
    let skips0 = s.preset(1).unwrap().skips;
    assert!(s.request(Op::PutPreset(1, preset("Put"))));
    assert_eq!(s.preset(1).unwrap().skips, skips0, "an edit keeps the skips");
    assert!(s.request(Op::PutPreset(n, preset("Added"))));
    assert_eq!(s.presets(), n + 1);
    assert_eq!(s.preset(n).unwrap().skips, 0, "a new one starts with none");
    assert!(!s.request(Op::PutPreset(n + 5, preset("Gap"))));
    assert!(s.request(Op::NewPreset));
    assert_eq!(s.preset(n + 1).unwrap().name.as_bytes(), b"New range");
    while s.request(Op::NewPreset) {}
    assert_eq!(s.presets(), 16, "until the store is full");
    assert!(!s.request(Op::PutPreset(16, preset("Full"))));
    assert!(s.request(Op::DeletePreset(0)));
    assert_eq!(s.presets(), 15);
    assert!(!s.request(Op::DeletePreset(15)));
}

/// The skip list: one taken off, then all.
#[test]
fn skips_unskip_and_clear() {
    let s = Sample::new();
    let n = s.skips();
    let second = s.skip(1);
    assert!(s.request(Op::Unskip(0)));
    assert_eq!(s.skips(), n - 1);
    assert_eq!(s.skip(0), second);
    assert!(!s.request(Op::Unskip(n)));
    assert!(s.request(Op::ClearSkips));
    assert_eq!(s.skips(), 0);
}

/// Waypoints: delete by index; past the end refused.
#[test]
fn waypoints_delete() {
    let s = Sample::new();
    let n = s.waypoints();
    let second = s.waypoint(1).unwrap().name;
    assert!(s.request(Op::DeleteWaypoint(0)));
    assert_eq!(s.waypoints(), n - 1);
    assert_eq!(s.waypoint(0).unwrap().name, second);
    assert!(!s.request(Op::DeleteWaypoint(n)));
}

/// DTMF memories: a cleared one loses its name too.
#[test]
fn dtmf_cleared_loses_its_name() {
    let s = Sample::new();
    assert_eq!(s.dtmf_name(0).as_bytes(), b"Link on");
    assert!(s.request(Op::Dtmf(0, None)));
    assert!(s.dtmf(0).is_none());
    assert_eq!(s.dtmf_name(0).as_bytes(), b"");
    assert!(!s.request(Op::Dtmf(200, None)));
}

/// Reset (MENU 0 9 7 2's Wipe): no channels, zones, presets, DTMF, log, heard, messages or waypoints; the draft
/// and APRS settings as new.
#[test]
fn wipe_empties_everything() {
    let s = Sample::new();
    assert!(s.request(Op::Draft(Draft { to: Buf::from(&b"X"[..]), text: Buf::from(&b"y"[..]) })));
    assert!(s.request(Op::Wipe));
    assert_eq!(s.count(), 0);
    assert_eq!(s.zones(), 0);
    assert_eq!(s.presets(), 0);
    assert!(s.dtmf(0).is_none() && s.dtmf_name(0).is_empty());
    assert_eq!((s.events(), s.stations(), s.messages(), s.waypoints()), (0, 0, 0, 0));
    assert!(s.draft().to.is_empty());
    assert!(s.quick(0).is_none());
}
