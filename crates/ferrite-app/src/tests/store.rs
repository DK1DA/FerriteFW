//! The store traits' own defaults (a store that only answers the zone questions), the empty store, Recent, and the
//! small records' helpers.

use crate::chan::{channel, ChanWrite, Channel, ChannelStore, Name, NoChannels, RecentList, NO_ZONE, RECENT_MAX};
use crate::data::{Dtmf, Op, RangePreset};
use crate::slot::DEFAULT_CFG;
use ferrite_ui::text::Buf;

/// Zones 0 and 1 and no zone, the channels out of number order: only the required answers, every other the
/// trait's own.
struct Zoned(std::vec::Vec<Channel>);

impl ChannelStore for Zoned {
    fn zones(&self) -> crate::chan::ZoneId {
        2
    }
    fn zone_name(&self, z: crate::chan::ZoneId) -> Option<Name> {
        (z < 2).then(|| Name::from(&b"Z"[..]))
    }
    fn zone_len(&self, z: crate::chan::ZoneId) -> u16 {
        self.0.iter().filter(|c| c.zone == z).count() as u16
    }
    fn zone_at(&self, z: crate::chan::ZoneId, i: u16) -> Option<Channel> {
        let mut v: std::vec::Vec<_> = self.0.iter().filter(|c| c.zone == z).copied().collect();
        v.sort_by_key(|c| c.num);
        v.get(i as usize).copied()
    }
    fn count(&self) -> u16 {
        self.0.len() as u16
    }
}

fn zoned() -> Zoned {
    Zoned(std::vec![
        channel(7, 0, 14_652_000, "Seven", DEFAULT_CFG),
        channel(2, 1, 14_694_000, "Two", DEFAULT_CFG),
        channel(5, NO_ZONE, 44_600_000, "Five", DEFAULT_CFG),
        channel(1, 0, 14_600_000, "One", DEFAULT_CFG),
    ])
}

/// CHAN-14: the default "All" view walks the zones and gives every channel in number order, zoned or not; past
/// the last, none. CHAN-10: by number, with its place in its zone.
#[test]
fn default_walks_give_number_order() {
    let s = zoned();
    let nums: std::vec::Vec<u16> = (0..s.count()).map(|i| s.by_number(i).unwrap().num).collect();
    assert_eq!(nums, [1, 2, 5, 7]);
    assert!(s.by_number(4).is_none());
    assert_eq!(s.by_num(7).map(|(c, i)| (c.name().to_vec(), i)), Some((b"Seven".to_vec(), 1)));
    assert_eq!(s.zone_of(5), Some(NO_ZONE));
    assert_eq!(s.free_num(1), Some(3));
    assert!(!s.queue_write(ChanWrite::EraseAll), "no queue of its own: the App's way");
    assert_eq!(s.zone_label(NO_ZONE, b"none", b"?").as_bytes(), b"none");
    assert_eq!(s.zone_label(9, b"none", b"?").as_bytes(), b"?");
}

/// The empty store: nothing anywhere; its data the trait's defaults.
#[test]
fn no_channels() {
    let s = NoChannels;
    assert_eq!((s.zones(), s.count(), s.zone_len(0)), (0, 0, 0));
    assert!(s.zone_name(0).is_none() && s.zone_at(0, 0).is_none() && s.by_number(0).is_none());
    assert_eq!(s.free_num(0), Some(0));
    let d = s.data();
    assert_eq!((d.version(), d.events(), d.stations(), d.kiss_counts()), (0, 0, 0, (0, 0)));
    assert!(d.row_of(0).is_none() && d.contact(0).is_none() && d.sat(0).is_none() && d.skip(0).is_none());
    d.row_stored(&Buf::from(&b"VE3AA"[..]), 1);
    assert_eq!(d.beacon().digi.as_bytes(), b"");
    assert!(!d.request(Op::ReadAll));
}

/// CHAN-25: Recent, newest first without duplicates: a channel again moves to the front, the oldest drops out
/// past ten; tuning the same channel again doesn't move it.
#[test]
fn recent_list() {
    let r = RecentList::default();
    assert_eq!(r.get(0), None);
    for n in 0..RECENT_MAX as u16 + 2 {
        r.touch(n);
    }
    assert_eq!(r.get(0), Some(RECENT_MAX as u16 + 1));
    assert_eq!(r.get(RECENT_MAX as u16 - 1), Some(2), "0 and 1 dropped out");
    r.touch(5);
    assert_eq!((r.get(0), r.get(1)), (Some(5), Some(RECENT_MAX as u16 + 1)));
    r.touch(5);
    assert_eq!(r.get(1), Some(RECENT_MAX as u16 + 1), "already first: no change");
    r.tuned(3);
    assert_eq!(r.get(0), Some(3));
    r.touch(4);
    r.tuned(3);
    assert_eq!(r.get(0), Some(4), "the same channel tuned again: not moved");
}

/// VFO-20: a range preset's FM with narrow from before reads as NFM; the bandwidth follows the mode.
#[test]
fn preset_fold_bw() {
    let mut p = RangePreset { name: Buf::new(), start: 0, end: 0, step: 0, mode: 0, narrow: true, skips: 0, resume: 0 };
    p.fold_bw();
    assert_eq!((p.mode, p.narrow), (1, true));
    p.mode = 2;
    p.fold_bw();
    assert_eq!((p.mode, p.narrow), (2, false), "AM stays AM, wide");
}

/// DTMF-12: a memory's symbols: pushed up to 16, taken back, as text.
#[test]
fn dtmf_symbols() {
    let mut d = Dtmf::parse(b"*79#").unwrap();
    let mut b = Buf::<16>::new();
    d.text(&mut b);
    assert_eq!(b.as_bytes(), b"*79#");
    d.pop();
    d.text(&mut b);
    assert_eq!(b.as_bytes(), b"*79");
    let mut e = Dtmf::default();
    e.pop();
    assert_eq!(e.len, 0);
    for _ in 0..Dtmf::MAX {
        assert!(e.push(1));
    }
    assert!(!e.push(1), "16 at most");
    assert!(Dtmf::parse(b"12345678901234567").is_none());
    assert!(Dtmf::parse(b"12X").is_none());
}
