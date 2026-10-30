//! Channels as the UI sees them: a small record and a store to read them from. The firmware's store reads SPI
//! flash records on demand at their fixed addresses, listed through its zone map (CHAN-7, CHAN-9); the
//! simulator and tests supply a few in RAM.
//! The UI keeps only the current zone, position and a copy of the channel each slot is on.

use crate::data::{Data, NoData, Pos};
use crate::slot::SlotCfg;
use ferrite_core::freq::{mhz, Freq10};

/// Channels below this are HF channels (CHAN-26).
pub const HF_TOP: Freq10 = mhz(64);
/// The lowest channel frequency: HF mode's bottom, 150 kHz.
pub const CH_LO: Freq10 = 15_000;

/// A frequency a channel can hold (CHAN-22): an HF one (150 kHz - 64 MHz, CHAN-26), or one the chips receive.
pub fn rx_ok(f: Freq10) -> bool {
    (CH_LO..HF_TOP).contains(&f) || ferrite_core::freq::receivable(f)
}

/// A zone's (or channel's) name: 16 bytes at most (CHAN-5).
pub type Name = ferrite_ui::text::Buf<16>;

/// Zone numbers 0-254; this one means "no zone".
pub const NO_ZONE: u8 = 255;

/// One channel: what home and edit-in-place show of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    /// 0-999, shown as 1-1000 ("CH 012" is number 11).
    pub num: u16,
    pub zone: u8,
    pub rx: Freq10,
    /// Name: 16 bytes, all shown on home (CHAN-5, HOME-4.3); unused bytes are 0.
    pub name: [u8; 16],
    pub cfg: SlotCfg,
    /// [`FLAG_SKIP`], [`FLAG_APRS`], [`FLAG_RX_ONLY`] (CHAN-4).
    pub flags: u8,
    /// This channel's end tone and tail elimination over the radio's (TONE-10, TONE-17; decision 86): bits 0-2
    /// the end tone ([`END_TONES`]), bits 3-4 tail elimination ([`TAILS`]); 0 = Global for both. Record byte 31.
    pub tones: u8,
}

/// A channel's end tone choices (TONE-10): Global follows 0 8 4, the rest are its own.
pub const END_TONES: [&str; 7] = ["Global", "Off", "Beep", "Two-tone up", "Two-tone down", "Chirp", "Quindar"];
/// A channel's tail elimination (TONE-17): Global follows 0 8 8 2, Off never, On as 0 8 8 2's phase.
pub const TAILS: [&str; 3] = ["Global", "Off", "On"];

/// Channel flags (CHAN-4): skipped by scans, APRS on this channel, never transmits.
pub const FLAG_SKIP: u8 = 1;
pub const FLAG_APRS: u8 = 2;
pub const FLAG_RX_ONLY: u8 = 4;
/// Left out of the channel lists unless "show hidden" is on (their options); still reachable by number and
/// scanned (decision 63).
pub const FLAG_HIDDEN: u8 = 0x40;
/// Checked during scans only (not while just receiving, decision 64) at the priority interval, besides the zone's
/// priority channels (decision 63).
pub const FLAG_PRIORITY: u8 = 0x80;

/// Channel numbers 0-999 (shown 1-1000).
pub const MAX_CHANNELS: u16 = ferrite_core::scan::MAX_CHANNELS;

/// A change to the stored channels, for whoever owns them (the Storage task; the simulator's RAM store),
/// from [`crate::App::pop_write`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChanWrite {
    /// Write this channel at its number; `from` is the number it had when it was renumbered (that record is
    /// removed), None when it stays or is new.
    Put { ch: Channel, from: Option<u16> },
    /// Write this channel at its number and set its position, or clear it with None (CHAN-2: the terminal's
    /// `ch N pos=`, TERM-9).
    PutPos { ch: Channel, pos: Option<Pos> },
    /// Set or clear (None) the position of whatever channel is stored at this number when the write is made, its
    /// other fields as they are then (LINK-53: the link's `WRITE_MANY` of positions, queued behind channel writes
    /// to the same sector and merged with them, decision 227). No channel there: nothing.
    Pos { num: u16, pos: Option<Pos> },
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

    /// An HF channel (CHAN-26): below 64 MHz, HF mode's, on the SI4732 or chip 1 through the relay. It opens HF
    /// mode on it; a home slot never sits on one, and no slot scan stops on one (SAFE-20: a scan never moves the
    /// relay; the SI4732 is HF mode's).
    pub fn is_hf(&self) -> bool {
        self.rx < HF_TOP
    }

    /// Left out of every scan (CHAN-4, SCAN-9).
    pub fn skip(&self) -> bool {
        self.flags & FLAG_SKIP != 0
    }

    /// In scans (0), skipped (1) or priority (2): Skip and Priority exclude each other (decision 63), and a record
    /// with both reads as Priority.
    pub fn scan(&self) -> u8 {
        if self.flags & FLAG_PRIORITY != 0 { 2 } else { (self.flags & FLAG_SKIP != 0) as u8 }
    }

    pub fn set_scan(&mut self, v: u8) {
        self.flags = self.flags & !(FLAG_SKIP | FLAG_PRIORITY) | [0, FLAG_SKIP, FLAG_PRIORITY][v as usize % 3];
    }

    /// The end tone ([`END_TONES`] index; 0 Global): out of range reads as Global.
    pub fn end_tone(&self) -> u8 {
        Some(self.tones & 7).filter(|&v| (v as usize) < END_TONES.len()).unwrap_or(0)
    }

    /// Tail elimination ([`TAILS`] index; 0 Global): out of range reads as Global.
    pub fn tail(&self) -> u8 {
        Some(self.tones >> 3 & 3).filter(|&v| (v as usize) < TAILS.len()).unwrap_or(0)
    }

    pub fn set_end_tone(&mut self, v: u8) {
        self.tones = self.tones & !7 | v.min(END_TONES.len() as u8 - 1);
    }

    pub fn set_tail(&mut self, v: u8) {
        self.tones = self.tones & !0x18 | v.min(TAILS.len() as u8 - 1) << 3;
    }

    /// A new name: up to 16 bytes, the rest zeroed; a character outside the name set becomes '-' (CHAN-31). Every
    /// way a name gets in comes through here: text entry, the terminal's editor, `ch`, `load`, the link, the store.
    pub fn set_name(&mut self, s: &[u8]) {
        self.name = [0; 16];
        for (d, &c) in self.name.iter_mut().zip(s) {
            *d = ferrite_core::name::map_name(c);
        }
    }
}

/// Where channels come from. Zone order is channel number (CHAN-10).
pub trait ChannelStore {
    /// LINK-41.1: a link's `WRITE_MANY` record straight into the store's own queue (several a pass, so same-sector
    /// writes merge, decision 227), not through the App's one write; false if it has no room or no queue of its own
    /// (the record then goes the App's way).
    fn queue_write(&self, _w: ChanWrite) -> bool {
        false
    }
    /// How many zones exist (zone numbers 0..zones()).
    fn zones(&self) -> u8;
    /// A zone's name, None if it doesn't exist. By value: the firmware's store reads it from SPI flash.
    fn zone_name(&self, zone: u8) -> Option<Name>;
    /// A zone's name for display: `none` for no zone, `missing` when it doesn't exist.
    fn zone_label(&self, zone: u8, none: &[u8], missing: &[u8]) -> Name {
        match self.zone_name(zone) {
            _ if zone == NO_ZONE => Name::from(none),
            _ if zone == crate::local::LOCAL => Name::from(b"Local".as_slice()), // CHAN-34
            Some(n) => n,
            None => Name::from(missing),
        }
    }
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

    /// The zone of channel `n` (NO_ZONE for none), None when there is no such channel: a channel scan's set
    /// ([`crate::chanscan`]) asks this of every number. The firmware's store answers from its zone map in RAM.
    fn zone_of(&self, n: u16) -> Option<u8> {
        self.get(n).map(|c| c.zone)
    }

    /// The channel with this number, if it exists: the terminal's sorted list reads every channel by number for
    /// each row it fills (TERM-14), so a store with records by number answers without the zone position.
    fn get(&self, num: u16) -> Option<Channel> {
        self.by_num(num).map(|(c, _)| c)
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

    /// A number that changes whenever a channel may have (CHAN-34: Local is worked out again when it does). The
    /// default is [`Data::version`]; the firmware's store gives its own records' count alone, so a GPS fix or a
    /// heard station doesn't count.
    fn chan_version(&self) -> u32 {
        self.data().version()
    }

    /// A channel and its position, when it has one (Nearby, the zones page: CHAN-15, CHAN-29). A store with records
    /// answers from one record read.
    fn located(&self, num: u16) -> Option<(Channel, Pos)> {
        let p = self.position(num)?;
        Some((self.get(num)?, p))
    }

    /// Recent (CHAN-25): the number of the r-th channel used, newest first; a store without the list has none.
    fn recent(&self, _r: u16) -> Option<u16> {
        None
    }

    /// A channel was used: tuned to, transmitted on, or heard (it moves to the front of Recent).
    fn used(&self, _num: u16) {}

    /// The active slot is on this channel: used once when it changes to it, not every time it is looked at.
    fn tuned(&self, _num: u16) {}

    /// The other records the screens list (event log, APRS, GPS, scan lists): see [`crate::data`].
    fn data(&self) -> &dyn Data {
        &NoData
    }
}

/// The channels used most recently (CHAN-25), newest first, without duplicates: 10 numbers, kept in RAM by the
/// store that owns the channels (not saved: a power cycle starts it again), and the active slot's channel last
/// put there, so tuning to one counts once.
pub struct RecentList([core::cell::Cell<u16>; RECENT_MAX], core::cell::Cell<u16>);

/// How many channels Recent holds.
pub const RECENT_MAX: usize = 10;

impl RecentList {
    pub const fn new() -> Self {
        RecentList([const { core::cell::Cell::new(u16::MAX) }; RECENT_MAX], core::cell::Cell::new(u16::MAX))
    }

    pub fn get(&self, r: u16) -> Option<u16> {
        self.0.get(r as usize).map(|c| c.get()).filter(|&n| n != u16::MAX)
    }

    /// `num` to the front; the others move down, its older place (or the oldest) drops out.
    pub fn touch(&self, num: u16) {
        if self.0[0].get() == num {
            return;
        }
        let at = self.0.iter().position(|c| c.get() == num).unwrap_or(RECENT_MAX - 1);
        for i in (1..=at).rev() {
            self.0[i].set(self.0[i - 1].get());
        }
        self.0[0].set(num);
    }

    /// The active slot is on `num`: to the front if it wasn't the last channel tuned.
    pub fn tuned(&self, num: u16) {
        if self.1.replace(num) != num {
            self.touch(num);
        }
    }
}

impl Default for RecentList {
    fn default() -> Self {
        Self::new()
    }
}

/// A store with nothing in it.
pub struct NoChannels;

impl ChannelStore for NoChannels {
    fn zones(&self) -> u8 {
        0
    }
    fn zone_name(&self, _: u8) -> Option<Name> {
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
    fn zone_name(&self, zone: u8) -> Option<Name> {
        self.zones.get(zone as usize).map(|z| Name::from(z))
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
    Channel { num, zone, rx, name: n, cfg, flags: 0, tones: 0 }
}
