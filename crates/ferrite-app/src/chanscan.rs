//! Channel scans in the Radio task (SCAN-3, SCAN-12, SCAN-22, SCAN-24): the UI says which channels a slot's
//! scan stops on and answers for their frequencies; the Radio task's engine walks them
//! ([`ferrite_core::scan::ChanList`]).
//!
//! The Radio task never holds the channels: it keeps a bit per channel number for each scanning slot
//! ([`ChanSet`], 133 bytes) and a few frequencies shared by all ([`ferrite_core::scan::FreqCache`]), asking for
//! the next ones a few stops ahead through [`LiveAll::want`]. Starting a scan streams the bitmap here, a chunk
//! of 40 channel numbers per [`RadioCmd::ScanSet`], a few per tick so the outbox never overflows, with the
//! priority channels as they are found; then the resume rule and [`RadioCmd::ScanChans`]. The first chunk
//! clears the Radio task's set and priority list.

use crate::chan::{ChannelStore, FLAG_PRIORITY, NO_ZONE};
use crate::App;
use ferrite_core::msg::{LiveAll, RadioCmd};
use ferrite_core::scan::{Resume, MAX_CHANNELS, PRIO_MAX, SET_BYTES, SET_CHUNK};

/// Bitmap chunks to send in all.
const CHUNKS: u8 = SET_BYTES.div_ceil(SET_CHUNK) as u8;
/// Commands pushed per tick at most, leaving room in the outbox for the rest.
const PER_TICK: usize = 6;

/// A channel scan's set being sent (one at a time; starting another replaces it). Everything else it needs is
/// read again as it goes: the slot's zone and channel, the zones added with digits, the zone's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feed {
    slot: u8,
    /// The next bitmap chunk to send.
    chunk: u8,
    /// Priority channels sent so far (SCAN-12: the zone's priority 1 and 2, then flagged ones in number order).
    prio: u8,
}

/// Starts (or restarts, after the zone set changed) a channel scan on `slot` from its channel. The set goes out
/// over the next ticks ([`feed`]).
pub fn start(app: &mut App, slot: u8) {
    app.radio.feed = Some(Feed { slot, chunk: 0, prio: 0 });
}

/// The zone set: the slot's zone, and zones 0-15 added with digits (SCAN-3).
fn in_set(app: &App, slot: u8, z: u8) -> bool {
    z == app.slot(slot).zone || z < 16 && app.radio.zones & 1 << z != 0
}

fn zone_cfg(app: &App, slot: u8, store: &impl ChannelStore) -> crate::data::ZoneCfg {
    match app.slot(slot).zone {
        NO_ZONE => Default::default(),
        z => store.data().zone_cfg(z),
    }
}

/// Whether channel `n` is a stop of the scan: in the set, not Skip-flagged, not an HF channel (CHAN-26: a scan
/// never moves the relay, SAFE-20); and whether it is a flagged priority channel.
fn stop(app: &App, slot: u8, store: &impl ChannelStore, n: u16) -> Option<bool> {
    if !store.zone_of(n).is_some_and(|z| in_set(app, slot, z)) {
        return None;
    }
    store.get(n).filter(|c| !c.skip() && !c.is_hf() && crate::chan::rx_ok(c.rx)).map(|c| c.flags & FLAG_PRIORITY != 0)
}

/// Sends the next part of a channel scan being started: bitmap chunks (the first resets the Radio task's set),
/// the priority channels as they are found, then the resume rule and the start.
pub fn feed(app: &mut App, store: &impl ChannelStore) {
    let Some(mut f) = app.radio.feed else { return };
    let zone_pri = zone_cfg(app, f.slot, store).pri;
    for _ in 0..PER_TICK {
        if app.out.q.len() + 2 + PRIO_MAX > app.out.q.capacity() {
            break;
        }
        if f.chunk < CHUNKS {
            let mut bits = [0u8; SET_CHUNK];
            let mut flagged = [u16::MAX; PRIO_MAX];
            let first = f.chunk as u16 * SET_CHUNK as u16 * 8;
            for k in 0..(SET_CHUNK * 8) as u16 {
                let n = first + k;
                if let Some(pri) = stop(app, f.slot, store, n).filter(|_| n < MAX_CHANNELS) {
                    bits[k as usize / 8] |= 1 << (k % 8);
                    if pri && !zone_pri.contains(&Some(n)) {
                        if let Some(p) = flagged.iter_mut().find(|p| **p == u16::MAX) {
                            *p = n;
                        }
                    }
                }
            }
            app.out.push(RadioCmd::ScanSet { slot: f.slot, chunk: f.chunk, bits });
            let zone_first = if f.chunk == 0 { zone_pri } else { [None; 2] };
            for num in zone_first.into_iter().flatten().chain(flagged.into_iter().filter(|&p| p != u16::MAX)) {
                if (f.prio as usize) < PRIO_MAX {
                    app.out.push(RadioCmd::ScanPrio { slot: f.slot, k: f.prio, num });
                    f.prio += 1;
                }
            }
            f.chunk += 1;
            continue;
        }
        let sl = app.slot(f.slot);
        let from = sl.ch.map_or(0, |c| c.num);
        let resume = Resume::code_over(zone_cfg(app, f.slot, store).resume, app.set.scan_resume);
        app.out.push(RadioCmd::ScanResume { slot: f.slot, code: resume });
        app.out.push(RadioCmd::ScanChans { slot: f.slot, from, prio_s: app.set.scan_prio });
        app.radio.feed = None;
        return;
    }
    app.radio.feed = Some(f);
}

/// The Radio task asks for a channel's frequencies ([`LiveAll::want`]): answered once per new ask.
pub fn answer(app: &mut App, live: &LiveAll, store: &impl ChannelStore) {
    if live.want == app.radio.want_seen {
        return;
    }
    app.radio.want_seen = live.want;
    let Some(num) = live.wanted() else { return };
    let c = store.get(num);
    let rx = c.map_or(0, |c| c.rx);
    app.out.push(RadioCmd::ScanEntry { num, rx });
    if let Some(c) = c.filter(|_| app.set.scan_tx) {
        let tx = c.cfg.tx_of(c.rx);
        if tx != c.rx {
            app.out.push(RadioCmd::ScanEntryTx { num, tx });
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
    use ferrite_core::scan::{ChanSet, NO_WANT};

    /// Drains the app's commands into a ChanSet as the Radio task would.
    fn drain(app: &mut App, store: &Sample, set: &mut ChanSet) -> std::vec::Vec<RadioCmd> {
        let mut all = std::vec::Vec::new();
        for _ in 0..40 {
            feed(app, store);
            while let Some(c) = app.pop_cmd() {
                if let RadioCmd::ScanSet { chunk, bits, .. } = c {
                    set.put_chunk(chunk, bits);
                }
                if let RadioCmd::ScanPrio { k, num, .. } = c {
                    set.set_prio(k, num);
                }
                all.push(c);
            }
        }
        all
    }

    /// SCAN-25: a channel scan needs no copy of the channels in the Radio task: the UI streams which channel
    /// numbers are stops, a few chunks per tick, without overflowing the outbox.
    #[test]
    fn the_set_streams_without_overflow() {
        let store = Sample::new();
        let leak = || -> &'static mut crate::settings::Settings { std::boxed::Box::leak(std::boxed::Box::new(crate::settings::Settings::DEFAULT)) };
        let alert = || std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        let mut app = App::new(leak(), alert());
        app.set_channel(0, 1, 0, &store);
        while app.pop_cmd().is_some() {}
        start(&mut app, 0);
        let mut set = ChanSet::new();
        let cmds = drain(&mut app, &store, &mut set);
        let sets = cmds.iter().filter(|c| matches!(c, RadioCmd::ScanSet { .. })).count();
        assert_eq!(sets, CHUNKS as usize, "every chunk sent, none dropped");
        assert!(matches!(cmds.last(), Some(RadioCmd::ScanChans { slot: 0, .. })), "the start comes last");
        // exactly the zone's channels that a scan stops on
        let zone = app.slot(0).zone;
        let want: std::vec::Vec<u16> = (0..store.zone_len(zone)).filter_map(|i| store.zone_at(zone, i)).filter(|c| !c.skip() && !c.is_hf()).map(|c| c.num).collect();
        let got: std::vec::Vec<u16> = (0..MAX_CHANNELS).filter(|&n| set.has(n)).collect();
        assert_eq!(got, want);
        assert!(!want.is_empty());
        // the zone's priority channel (sample zone 1: channel 11)
        let pri = store.data().zone_cfg(zone).pri[0];
        assert_eq!(set.prio(0), pri);
        let _ = NO_WANT;
    }

    #[test]
    fn wants_are_answered_once() {
        let store = Sample::new();
        let leak = || -> &'static mut crate::settings::Settings { std::boxed::Box::leak(std::boxed::Box::new(crate::settings::Settings::DEFAULT)) };
        let alert = || std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        let mut app = App::new(leak(), alert());
        while app.pop_cmd().is_some() {}
        let c = store.by_number(0).unwrap();
        let mut live = LiveAll::default();
        live.set_want(Some(c.num));
        answer(&mut app, &live, &store);
        answer(&mut app, &live, &store);
        let mut n = 0;
        while let Some(cmd) = app.pop_cmd() {
            assert_eq!(cmd, RadioCmd::ScanEntry { num: c.num, rx: c.rx });
            n += 1;
        }
        assert_eq!(n, 1, "once per ask");
        live.set_want(Some(999));
        answer(&mut app, &live, &store);
        assert_eq!(app.pop_cmd(), Some(RadioCmd::ScanEntry { num: 999, rx: 0 }), "no such channel: 0");
    }
}
