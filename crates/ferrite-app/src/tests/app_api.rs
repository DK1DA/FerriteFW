//! The App's channel bookkeeping (renumber, delete, a deleted zone, the store's own queue) and the small calls the
//! firmware makes at boot (the inbox counts, Dry TX, a notice).

use super::{store, T};
use crate::chan::{ChanWrite, Channel, ChannelStore, Name, Table, NO_ZONE};
use crate::uistate;
use core::cell::RefCell;
use ferrite_core::msg::Device;
use std::vec::Vec;

/// The tests' table with a queue of its own (LINK-41.1): it takes every write and keeps it.
struct Queued {
    t: Table<'static>,
    got: RefCell<Vec<ChanWrite>>,
}

impl ChannelStore for Queued {
    fn queue_write(&self, w: ChanWrite) -> bool {
        self.got.borrow_mut().push(w);
        true
    }
    fn zones(&self) -> crate::chan::ZoneId {
        self.t.zones()
    }
    fn zone_name(&self, z: crate::chan::ZoneId) -> Option<Name> {
        self.t.zone_name(z)
    }
    fn zone_len(&self, z: crate::chan::ZoneId) -> u16 {
        self.t.zone_len(z)
    }
    fn zone_at(&self, z: crate::chan::ZoneId, i: u16) -> Option<Channel> {
        self.t.zone_at(z, i)
    }
    fn count(&self) -> u16 {
        self.t.count()
    }
}

fn on_calling(t: &mut T) -> Channel {
    t.tap(ferrite_core::keys::Key::VfoMem);
    let c = t.app.slot(0).ch.unwrap();
    assert_eq!(c.name(), b"Calling");
    c
}

/// CHAN-18, TERM-14: a channel given another number: the record moves, a slot on it and the home channel follow.
#[test]
fn renumber_moves_slots_and_home() {
    let mut t = T::new();
    let c = on_calling(&mut t);
    t.app.set.home_ch = Some(c.num);
    t.app.renumber_channel(c, 500);
    assert_eq!(t.app.slot(0).ch.unwrap().num, 500);
    assert_eq!(t.app.set.home_ch, Some(500));
    assert!(matches!(t.app.pop_write(), Some(ChanWrite::Put { ch, from: Some(0) }) if ch.num == 500));
}

/// CHAN-13, CHAN-18: a deleted channel: the slot on it goes to VFO, and it stops being the home channel.
#[test]
fn delete_clears_slot_and_home() {
    let mut t = T::new();
    let c = on_calling(&mut t);
    t.app.set.home_ch = Some(c.num);
    t.app.delete_channel(c.num, &store());
    assert!(t.app.slot(0).ch.is_none() && !t.app.slot(0).on_channel());
    assert_eq!(t.app.set.home_ch, None);
    assert!(matches!(t.app.pop_write(), Some(ChanWrite::Delete(0))), "the table has no Data owner: the App's write");
}

/// LINK-41.1, LINK-41.2: a store with a queue of its own takes a link's channel writes and deletes straight, no
/// App write waiting; a slot on the channel still takes its new settings, or goes to VFO.
#[test]
fn queued_writes_skip_the_app() {
    let mut t = T::new();
    let c = on_calling(&mut t);
    let q = Queued { t: store(), got: RefCell::new(Vec::new()) };
    let n = Channel { rx: c.rx + 2_500, ..c };
    t.app.put_channel_queued(n, &q);
    assert!(!t.app.write_pending());
    assert_eq!(t.app.slot(0).ch.unwrap().rx, n.rx, "the slot took it");
    t.app.delete_channel_queued(c.num, &q);
    assert!(!t.app.write_pending());
    assert!(t.app.slot(0).ch.is_none());
    assert_eq!(q.got.borrow().len(), 2);
}

/// CHAN-12.1: a deleted zone leaves no slot in it, nor the slot's copy of its channel.
#[test]
fn zone_deleted_clears_slot_zone() {
    let mut t = T::new();
    on_calling(&mut t);
    let z = t.app.slots[0].zone;
    t.app.zone_deleted(z);
    assert_eq!(t.app.slots[0].zone, NO_ZONE);
    assert_eq!(t.app.slot(0).ch.unwrap().zone, NO_ZONE);
}

/// INBOX-3.1, INBOX-7: the counts at power on show on the status bar (more than 255 read as 255).
#[test]
fn inbox_counts_at_power_on() {
    let mut t = T::new();
    t.app.set_inbox(300, 2);
    assert_eq!(t.app.inbox, 255);
    assert_eq!(t.app.log_unread, 2);
    let mut l = crate::term::cmd::Line::new();
    let mut i = 0;
    let s = store();
    let dev = Device::default();
    while uistate::line(&t.app, &s, &dev, &t.live, i, &mut l) && !l.as_bytes().starts_with(b"bar") {
        i += 1;
    }
    assert!(core::str::from_utf8(l.as_bytes()).unwrap().contains(" inbox="), "the bar line");
}

/// TX-48.1: the tx-bench build starts with Dry TX on; a boot notice is a toast.
#[test]
fn dry_at_boot_and_a_notice() {
    let mut t = T::new();
    assert!(!t.app.dry_tx());
    t.app.dry_at_boot();
    assert!(t.app.dry_tx());
    t.app.notice(b"Bring-up", b"image");
    assert!(t.app.toast_up());
    let _ = t.app.page_shown();
}

/// SCAN-6: Return to the hit on a channel scan puts the slot on the hit's channel.
#[test]
fn scan_return_to_a_channel_hit() {
    let mut t = T::new();
    on_calling(&mut t);
    let s = store();
    let other = s.zone_at(0, 1).unwrap();
    t.app.set.scan_return_hit = true;
    t.app.last_hit[0] = other.rx;
    t.app.scan_move(0, other.rx, &s);
    assert_eq!(t.app.slot(0).ch.unwrap().num, other.num);
    // a frequency no channel has: the slot stays
    t.app.scan_move(0, 1, &s);
    assert_eq!(t.app.slot(0).ch.unwrap().num, other.num);
}
