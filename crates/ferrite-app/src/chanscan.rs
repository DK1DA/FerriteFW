//! Channel scans in the Radio task (SCAN-3, SCAN-12, SCAN-22, SCAN-24, SCAN-25): the UI says how long a slot's
//! scan list is and answers for what is at each position; the Radio task's engine walks the positions
//! ([`ferrite_core::scan::ChanList`]).
//!
//! The list (decision 333) is the slot's zone's channels in number order (Local's nearest first, CHAN-34), then
//! those of the zones added with digits, each in turn: the store's member list of the slot's zone answers a
//! position at once, another zone is paged through flash from where the last position was found, so walking the
//! list costs a record read a stop. The Radio task holds no channel numbers at all, only the list's length and
//! the priority positions (the zone's priority 1 and 2: their place in the list, or after it when they are
//! outside it; then flagged ones). Starting a scan sends the length, the priority positions, the resume rule and
//! [`RadioCmd::ScanChans`]; when the channels change under a running scan, the length and priorities go again
//! (the scan goes on where it is, its frequencies asked again).

use crate::chan::{Channel, ChannelStore, ZoneId, FLAG_PRIORITY, NO_ZONE};
use crate::App;
use ferrite_core::msg::{LiveAll, RadioCmd};
use ferrite_core::scan::{Resume, POS_MAX, PRIO_MAX};

/// A channel scan being started (`full`: the resume rule and the start follow) or its list sent again after a
/// change (one at a time; starting another replaces it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feed {
    slot: u8,
    full: bool,
}

/// Starts (or restarts, after the zone set changed) a channel scan on `slot` from its channel. Its list goes out
/// at the next tick with room in the outbox ([`feed`]).
pub fn start(app: &mut App, slot: u8) {
    app.radio.feed = Some(Feed { slot, full: true });
}

/// The zones a slot's channel scan walks, in order: its own, then those added with digits (SCAN-3, 0-9 here).
fn zones(app: &App, slot: u8) -> impl Iterator<Item = ZoneId> + '_ {
    let own = app.slot(slot).zone;
    core::iter::once(own).chain((0..10 as ZoneId).filter(move |&z| z != own && app.radio.zones & 1 << z != 0))
}

/// How many positions the slot's scan list has.
pub fn len(app: &App, slot: u8, store: &impl ChannelStore) -> u16 {
    zones(app, slot).map(|z| app.zlen(store, z) as u32).sum::<u32>().min(POS_MAX as u32) as u16
}

/// The channel at position `pos` of the slot's scan list: a list position, or a zone priority channel's after it
/// (the zone's priority `pos - len`, SCAN-12).
pub fn at(app: &App, slot: u8, pos: u16, store: &impl ChannelStore) -> Option<Channel> {
    let mut p = pos;
    for z in zones(app, slot) {
        let n = app.zlen(store, z);
        if p < n {
            return app.zat(store, z, p);
        }
        p -= n;
    }
    let k = p as usize;
    zone_cfg(app, slot, store).pri.get(k).copied().flatten().and_then(|n| store.get(n))
}

fn zone_cfg(app: &App, slot: u8, store: &impl ChannelStore) -> crate::data::ZoneCfg {
    match app.slot(slot).zone {
        NO_ZONE | crate::local::LOCAL => Default::default(),
        z => store.data().zone_cfg(z),
    }
}

/// Whether a channel is a stop: not Skip-flagged, not an HF channel (CHAN-26: a scan never moves the relay,
/// SAFE-20), and a frequency a channel holds.
fn stop(c: &Channel) -> bool {
    !c.skip() && !c.is_hf() && crate::chan::rx_ok(c.rx)
}

/// The priority positions (SCAN-12): the zone's priority 1 and 2 (their place in the list, or `len` + 0 or 1 when
/// outside it), then flagged channels of the list in its order, [`PRIO_MAX`] in all.
#[inline(never)]
fn priorities(app: &App, slot: u8, len: u16, store: &impl ChannelStore) -> [u16; PRIO_MAX] {
    let mut out = [u16::MAX; PRIO_MAX];
    let mut k = 0;
    let pri = zone_cfg(app, slot, store).pri;
    let mut zone_pos = [None; 2];
    let mut p = 0u16;
    let mut flagged = [u16::MAX; PRIO_MAX];
    let mut f = 0;
    for z in zones(app, slot) {
        for i in 0..app.zlen(store, z) {
            if let Some(c) = app.zat(store, z, i) {
                if let Some(j) = pri.iter().position(|&x| x == Some(c.num)) {
                    zone_pos[j] = Some(p);
                } else if c.flags & FLAG_PRIORITY != 0 && stop(&c) && f < PRIO_MAX {
                    flagged[f] = p;
                    f += 1;
                }
            }
            p += 1;
        }
    }
    for (j, n) in pri.iter().enumerate() {
        if n.is_some() && k < PRIO_MAX {
            out[k] = zone_pos[j].unwrap_or(len + j as u16);
            k += 1;
        }
    }
    for &q in flagged.iter().take_while(|&&q| q != u16::MAX) {
        if k < PRIO_MAX {
            out[k] = q;
            k += 1;
        }
    }
    out
}

/// Sends a channel scan's list when there is room in the outbox: its length and priority positions, then (for a
/// start) the resume rule and the start at the slot's place in its zone. A running channel scan whose channels
/// changed gets its list again (the store's [`ChannelStore::chan_version`]).
#[inline(never)] // SBAR-23: its buffers in its own frame
pub fn feed(app: &mut App, store: &impl ChannelStore) {
    let ver = store.chan_version();
    if ver != app.radio.scan_ver {
        app.radio.scan_ver = ver;
        app.radio.refresh = (0..3u8).filter(|&s| app.scan_mask & 1 << s != 0 && app.slot(s).on_channel()).fold(0, |m, s| m | 1 << s);
    }
    if app.radio.feed.is_none() && app.radio.refresh != 0 {
        let s = app.radio.refresh.trailing_zeros() as u8;
        app.radio.refresh &= !(1 << s);
        if app.scan_mask & 1 << s != 0 && app.slot(s).on_channel() {
            app.radio.feed = Some(Feed { slot: s, full: false });
        }
    }
    let Some(f) = app.radio.feed else { return };
    if app.out.q.len() + 4 + PRIO_MAX > app.out.q.capacity() {
        return; // next tick
    }
    let n = len(app, f.slot, store);
    app.out.push(RadioCmd::ScanSet { slot: f.slot, len: n });
    for (k, pos) in priorities(app, f.slot, n, store).into_iter().enumerate().filter(|x| x.1 != u16::MAX) {
        app.out.push(RadioCmd::ScanPrio { slot: f.slot, k: k as u8, pos });
    }
    if f.full {
        let sl = app.slot(f.slot);
        let from = if sl.ch.is_some() { sl.pos.min(n.saturating_sub(1)) } else { 0 };
        let resume = Resume::code_over(zone_cfg(app, f.slot, store).resume, app.set.scan_resume);
        app.out.push(RadioCmd::ScanResume { slot: f.slot, code: resume });
        app.out.push(RadioCmd::ScanChans { slot: f.slot, from, prio_s: app.set.scan_prio });
    }
    app.radio.feed = None;
}

/// The Radio task asks what is at a position of a slot's list ([`LiveAll::want`]): answered once per new ask with
/// the channel's receive frequency, or 0 when it is no stop; its transmit frequency too while scan inputs are on
/// (SCAN-22). A zone priority channel is answered whatever its Skip flag (SCAN-12).
pub fn answer(app: &mut App, live: &LiveAll, store: &impl ChannelStore) {
    if live.want == app.radio.want_seen {
        return;
    }
    app.radio.want_seen = live.want;
    let Some((slot, pos)) = live.wanted() else { return };
    let n = len(app, slot, store);
    let c = at(app, slot, pos, store).filter(|c| if pos < n { stop(c) } else { !c.is_hf() && crate::chan::rx_ok(c.rx) });
    app.out.push(RadioCmd::ScanEntry { slot, pos, rx: c.map_or(0, |c| c.rx) });
    if let Some(c) = c.filter(|_| app.set.scan_tx) {
        let tx = c.cfg.tx_of(c.rx);
        if tx != c.rx {
            app.out.push(RadioCmd::ScanEntryTx { slot, pos, tx });
        }
    }
}

/// The resume rule a frequency scan takes: a range preset's own, or the radio's (SCAN-24).
pub fn preset_resume(app: &App, preset_resume: u8) -> u8 {
    Resume::code_over(preset_resume, app.set.scan_resume)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;

    fn app() -> App {
        let set: &'static mut crate::settings::Settings = std::boxed::Box::leak(std::boxed::Box::new(crate::settings::Settings::DEFAULT));
        let alert = std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        App::new(set, alert)
    }

    /// The commands a feed sends.
    fn drain(app: &mut App, store: &Sample) -> std::vec::Vec<RadioCmd> {
        feed(app, store);
        core::iter::from_fn(|| app.pop_cmd()).collect()
    }

    /// The list a slot's scan walks, as the UI answers it position by position.
    fn walk(app: &App, slot: u8, store: &Sample) -> std::vec::Vec<u16> {
        (0..len(app, slot, store)).filter_map(|p| at(app, slot, p, store)).map(|c| c.num).collect()
    }

    /// SCAN-25 (decision 333): a channel scan needs no copy of the channels in the Radio task, nor a bit per
    /// channel: the UI sends the list's length, the priority positions, the resume rule and the start; the list is
    /// the slot's zone's channels in number order, and the zone's priority channel is at its place in it.
    #[test]
    fn a_start_sends_the_length() {
        let store = Sample::new();
        let mut app = app();
        app.set_channel(0, 1, 0, &store);
        while app.pop_cmd().is_some() {}
        start(&mut app, 0);
        let cmds = drain(&mut app, &store);
        let zone = app.slot(0).zone;
        let want: std::vec::Vec<u16> = (0..store.zone_len(zone)).filter_map(|i| store.zone_at(zone, i)).map(|c| c.num).collect();
        assert!(!want.is_empty());
        assert_eq!(walk(&app, 0, &store), want, "the zone's channels in number order");
        assert_eq!(cmds[0], RadioCmd::ScanSet { slot: 0, len: want.len() as u16 });
        assert!(matches!(cmds.last(), Some(RadioCmd::ScanChans { slot: 0, .. })), "the start comes last");
        // the zone's priority channel (sample zone 1: channel 11) at its place in the list
        let pri = store.data().zone_cfg(zone).pri[0].unwrap();
        let p = want.iter().position(|&n| n == pri).unwrap() as u16;
        assert!(cmds.contains(&RadioCmd::ScanPrio { slot: 0, k: 0, pos: p }), "{cmds:?}");
        assert_eq!(app.radio.feed, None, "all sent in one go");
    }

    /// SCAN-3 through the list: zones added with digits follow the slot's own, each in number order.
    #[test]
    fn added_zones_follow_the_slots_own() {
        let store = Sample::new();
        let mut app = app();
        app.set_channel(0, 1, 0, &store);
        let other = (0..10).find(|&z| z != 1 && store.zone_len(z) > 0).unwrap();
        app.radio.zones = 1 << other | 1 << 1; // 1 is the slot's own: not twice
        let own: std::vec::Vec<u16> = (0..store.zone_len(1)).filter_map(|i| store.zone_at(1, i)).map(|c| c.num).collect();
        let more: std::vec::Vec<u16> = (0..store.zone_len(other)).filter_map(|i| store.zone_at(other, i)).map(|c| c.num).collect();
        assert_eq!(walk(&app, 0, &store), [own, more].concat());
    }

    /// CHAN-34.2: a slot on Local scans Local's members, nearest first as the list is.
    #[test]
    fn local_scans_its_members() {
        let store = Sample::new();
        let mut app = app();
        crate::local::tick(&mut app, &store);
        app.set_channel(0, crate::local::LOCAL, 0, &store);
        let want: std::vec::Vec<u16> = (0..app.local().len()).filter_map(|i| app.local().num(i)).collect();
        assert!(!want.is_empty());
        assert_eq!(walk(&app, 0, &store), want);
    }

    /// SCAN-25: each ask is answered once, with the receive frequency at that position, or 0 for a position that
    /// is no stop (a Skip-flagged channel, an HF one, none there).
    #[test]
    fn wants_are_answered_once() {
        let store = Sample::new();
        let mut app = app();
        app.set_channel(1, 1, 0, &store);
        while app.pop_cmd().is_some() {}
        let c = at(&app, 1, 0, &store).unwrap();
        let mut live = LiveAll::default();
        live.set_want(Some((1, 0)));
        answer(&mut app, &live, &store);
        answer(&mut app, &live, &store);
        let rx = if stop(&c) { c.rx } else { 0 };
        assert_eq!(core::iter::from_fn(|| app.pop_cmd()).collect::<std::vec::Vec<_>>(), [RadioCmd::ScanEntry { slot: 1, pos: 0, rx }], "once per ask");
        live.set_want(Some((1, 999)));
        answer(&mut app, &live, &store);
        assert_eq!(app.pop_cmd(), Some(RadioCmd::ScanEntry { slot: 1, pos: 999, rx: 0 }), "nothing there: 0");
        // a Skip-flagged channel is no stop
        let n = len(&app, 1, &store);
        if let Some(p) = (0..n).find(|&p| at(&app, 1, p, &store).is_some_and(|c| c.skip())) {
            live.set_want(Some((1, p)));
            answer(&mut app, &live, &store);
            assert_eq!(app.pop_cmd(), Some(RadioCmd::ScanEntry { slot: 1, pos: p, rx: 0 }));
        }
    }

    /// SCAN-25: when the channels change under a running channel scan, its list goes again (no restart: no
    /// resume rule, no start).
    #[test]
    fn a_change_sends_the_list_again() {
        let store = Sample::new();
        let mut app = app();
        app.set_channel(0, 1, 0, &store);
        while app.pop_cmd().is_some() {}
        app.scan_mask = 1;
        feed(&mut app, &store);
        while app.pop_cmd().is_some() {}
        app.radio.scan_ver = app.radio.scan_ver.wrapping_add(1); // as if the store's version moved
        let cmds = drain(&mut app, &store);
        assert!(matches!(cmds.first(), Some(RadioCmd::ScanSet { slot: 0, .. })), "{cmds:?}");
        assert!(!cmds.iter().any(|c| matches!(c, RadioCmd::ScanChans { .. } | RadioCmd::ScanResume { .. })));
        let again = drain(&mut app, &store);
        assert!(again.is_empty(), "once: {again:?}");
    }
}
