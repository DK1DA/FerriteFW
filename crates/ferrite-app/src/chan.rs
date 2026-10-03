//! Channels as the UI sees them: a small record and a store to read them from. The firmware's store reads SPI
//! flash records on demand through the zone index (CHAN-7, CHAN-9); the simulator and tests supply a few in RAM.
//! The UI keeps only the current zone, position and a copy of the channel each slot is on.

use crate::data::{Data, NoData, Pos};
use crate::slot::SlotCfg;
use ferrite_core::freq::Freq10;

/// Zone numbers 0-254; this one means "no zone".
pub const NO_ZONE: u8 = 255;

/// One channel: what home and edit-in-place show of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    /// 0-999, shown as 1-1000 ("CH 012" is number 11).
    pub num: u16,
    pub zone: u8,
    pub rx: Freq10,
    /// Name: 16 bytes stored, 12 shown on home (CHAN-5); unused bytes are 0.
    pub name: [u8; 16],
    pub cfg: SlotCfg,
    /// [`FLAG_SKIP`], [`FLAG_APRS`], [`FLAG_RX_ONLY`] (CHAN-4).
    pub flags: u8,
}

/// Channel flags (CHAN-4): skipped by scans, APRS on this channel, never transmits.
pub const FLAG_SKIP: u8 = 1;
pub const FLAG_APRS: u8 = 2;
pub const FLAG_RX_ONLY: u8 = 4;

/// Channel numbers 0-999 (shown 1-1000).
pub const MAX_CHANNELS: u16 = 1000;

/// A change to the stored channels, for whoever owns them (the Storage task; the simulator's RAM store),
/// from [`crate::App::pop_write`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChanWrite {
    /// Write this channel at its number; `from` is the number it had when it was renumbered (that record is
    /// removed), None when it stays or is new.
    Put { ch: Channel, from: Option<u16> },
    /// Remove a channel (CHAN-13, from the browser).
    Delete(u16),
    /// Remove a zone: its channels move to no zone and stay in All (CHAN-12).
    DeleteZone(u8),
    /// Every channel (settings reset, its second confirm: PIN-6).
    EraseAll,
}

impl Channel {
    pub fn name(&self) -> &[u8] {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(16);
        &self.name[..n]
    }

    /// Left out of every scan (CHAN-4, SCAN-9).
    pub fn skip(&self) -> bool {
        self.flags & FLAG_SKIP != 0
    }

    /// A new name: up to 16 bytes, the rest zeroed.
    pub fn set_name(&mut self, s: &[u8]) {
        self.name = [0; 16];
        let n = s.len().min(16);
        self.name[..n].copy_from_slice(&s[..n]);
    }
}

/// Where channels come from. Zone order is channel number (CHAN-10).
pub trait ChannelStore {
    /// How many zones exist (zone numbers 0..zones()).
    fn zones(&self) -> u8;
    /// A zone's name, None if it doesn't exist.
    fn zone_name(&self, zone: u8) -> Option<&[u8]>;
    /// How many channels a zone holds.
    fn zone_len(&self, zone: u8) -> u16;
    /// The i-th channel of a zone, by number.
    fn zone_at(&self, zone: u8, i: u16) -> Option<Channel>;
    /// Channels stored in all.
    fn count(&self) -> u16;

    /// The channel with this number and its position in its zone (channel entry, the home channel). The
    /// firmware's store reads it straight from its record; this default walks the zones.
    fn by_num(&self, num: u16) -> Option<(Channel, u16)> {
        let zones = (0..self.zones()).chain(core::iter::once(NO_ZONE));
        for z in zones {
            for i in 0..self.zone_len(z) {
                match self.zone_at(z, i) {
                    Some(c) if c.num == num => return Some((c, i)),
                    _ => {}
                }
            }
        }
        None
    }

    /// The first unused number from `from` on, wrapping (save as channel), if any is free.
    fn free_num(&self, from: u16) -> Option<u16> {
        (0..MAX_CHANNELS).map(|k| (from + k) % MAX_CHANNELS).find(|&n| self.by_num(n).is_none())
    }

    /// The i-th stored channel in number order, zoned or not: the browser's "All" view (CHAN-14). The default
    /// walks the zones; a store with records by number reads one directly.
    fn by_number(&self, i: u16) -> Option<Channel> {
        let mut found: Option<Channel> = None;
        for _ in 0..=i {
            let last = found.map(|c| c.num);
            found = None;
            for z in (0..self.zones()).chain([NO_ZONE]) {
                for c in (0..self.zone_len(z)).filter_map(|k| self.zone_at(z, k)) {
                    if last.is_none_or(|l| c.num > l) && found.is_none_or(|f| c.num < f.num) {
                        found = Some(c);
                    }
                }
            }
            found?;
        }
        found
    }

    /// A channel's position, if it has one (CHAN-2, CHAN-15).
    fn position(&self, _num: u16) -> Option<Pos> {
        None
    }

    /// The other records the screens list (event log, APRS, GPS, scan lists): see [`crate::data`].
    fn data(&self) -> &dyn Data {
        &NoData
    }
}

/// A store with nothing in it.
pub struct NoChannels;

impl ChannelStore for NoChannels {
    fn zones(&self) -> u8 {
        0
    }
    fn zone_name(&self, _: u8) -> Option<&[u8]> {
        None
    }
    fn zone_len(&self, _: u8) -> u16 {
        0
    }
    fn zone_at(&self, _: u8, _: u16) -> Option<Channel> {
        None
    }
    fn count(&self) -> u16 {
        0
    }
}

/// A fixed table of channels with zones by name: the simulator's and the tests' store.
pub struct Table<'a> {
    pub zones: &'a [&'a [u8]],
    pub channels: &'a [Channel],
}

impl ChannelStore for Table<'_> {
    fn zones(&self) -> u8 {
        self.zones.len() as u8
    }
    fn zone_name(&self, zone: u8) -> Option<&[u8]> {
        self.zones.get(zone as usize).copied()
    }
    fn zone_len(&self, zone: u8) -> u16 {
        self.channels.iter().filter(|c| c.zone == zone).count() as u16
    }
    fn zone_at(&self, zone: u8, i: u16) -> Option<Channel> {
        self.channels.iter().filter(|c| c.zone == zone).nth(i as usize).copied()
    }
    fn count(&self) -> u16 {
        self.channels.len() as u16
    }
    fn by_number(&self, i: u16) -> Option<Channel> {
        // A table is in number order.
        self.channels.get(i as usize).copied()
    }
}

/// A channel record from its parts (for tables).
pub const fn channel(num: u16, zone: u8, rx: Freq10, name: &str, cfg: SlotCfg) -> Channel {
    let mut n = [0u8; 16];
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() && i < 16 {
        n[i] = b[i];
        i += 1;
    }
    Channel { num, zone, rx, name: n, cfg, flags: 0 }
}
