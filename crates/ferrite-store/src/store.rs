//! [`FlashStore`]: the channels, zones and range presets the UI reads ([`ChannelStore`], [`Data`]), kept in
//! SPI flash and changed through [`FlashStore::apply`] and [`Data::request`]; the settings and slots saved 2 s
//! after the last change ([`FlashStore::autosave`]) and at power off ([`FlashStore::flush`]).
//!
//! Mount ([`FlashStore::mount`]): finish an interrupted sector rewrite; check the layout header (missing: an
//! erased or new chip is formatted, a damaged header over good data is written again and the data kept); count
//! the zones; check the zone index against the channels. A store with no settings record is a first boot
//! (setup runs, BOOT-12).
//!
//! The records the UI doesn't keep in flash yet (event log, APRS, GPS, waypoints, sensors, scan skips, FM
//! presets) come from `extra`: nothing on the radio for now, the sample data in the simulator.

use crate::index::{self, BITMAP_LEN};
use crate::journal::{FlashSource, Journal};
use crate::records::*;
use crate::{swap, Crc32, Nor, NorError, Result, SECTOR};
use core::cell::{Cell, RefCell};
use ferrite_app::chan::{ChanWrite, Channel, ChannelStore, Name, MAX_CHANNELS, NO_ZONE};
use ferrite_app::data::*;
use ferrite_app::persist::{Persist, Sink, Source};
use ferrite_app::settings::Settings;
use ferrite_app::App;
use ferrite_core::freq::Freq10;
use ferrite_core::layout::{CHANNELS, LAYOUT_HEADER, SETTINGS, ZONES};
use ferrite_ui::text::Buf;

/// The layout header (0x02C000): magic, layout version, the calibration and backup CRCs the installer records
/// (0xFFFFFFFF until it does), CRC-32 of the 16 bytes before.
const HEADER_MAGIC: u32 = 0x5452_5246; // "FRRT"
pub const LAYOUT_VERSION: u16 = 1;

/// The largest slots part [`FlashStore::restore`] reads (about 160 bytes now).
const STATE_MAX: usize = 512;

/// Saves wait this long after the last change (a turning knob doesn't wear the flash).
pub const SAVE_DELAY_MS: u32 = 2000;

/// What [`FlashStore::mount`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mounted {
    /// Header and settings in place.
    Ok,
    /// No settings record: setup runs (a new or erased chip, formatted now).
    FirstBoot,
    /// The flash failed: nothing is stored this power-on.
    Failed,
}

pub struct FlashStore<N: Nor, D: Data> {
    nor: RefCell<N>,
    /// The records not in flash yet (see the module comment).
    pub extra: D,
    zones: Cell<u8>,
    ver: Cell<u32>,
    mounted: Cell<Mounted>,
    /// The save timer: CRC of the state last saved, of the state last seen, and when that changed.
    saved: Cell<u32>,
    seen: Cell<u32>,
    changed_at: Cell<u32>,
}

impl<N: Nor, D: Data> FlashStore<N, D> {
    /// Mounts the store on `nor` (see the module comment). Never fails: a flash that doesn't answer gives an
    /// empty store that keeps nothing ([`Mounted::Failed`]).
    pub fn mount(nor: N, extra: D) -> Self {
        let s = FlashStore {
            nor: RefCell::new(nor),
            extra,
            zones: Cell::new(0),
            ver: Cell::new(0),
            mounted: Cell::new(Mounted::Failed),
            saved: Cell::new(0),
            seen: Cell::new(0),
            changed_at: Cell::new(0),
        };
        let m = s.mount_inner().unwrap_or(Mounted::Failed);
        s.mounted.set(m);
        s
    }

    fn mount_inner(&self) -> Result<Mounted> {
        let nor = &mut *self.nor.borrow_mut();
        swap::recover(nor)?;
        if !header_ok(nor)? {
            let journal = Journal { region: SETTINGS };
            let data = journal.latest(nor)?.is_some() || any_channel(nor)?;
            if !data {
                format(nor)?;
            }
            write_header(nor)?;
        }
        self.zones.set(count_zones(nor)?);
        index::check(nor)?;
        let first = (Journal { region: SETTINGS }).latest(nor)?.is_none();
        Ok(if first { Mounted::FirstBoot } else { Mounted::Ok })
    }

    pub fn mounted(&self) -> Mounted {
        self.mounted.get()
    }

    /// No settings were stored: setup runs (BOOT-12).
    pub fn first_boot(&self) -> bool {
        self.mounted.get() == Mounted::FirstBoot
    }

    /// The flash and the extra records back (unmounting: the simulator's power cycle, the tests).
    pub fn into_parts(self) -> (N, D) {
        (self.nor.into_inner(), self.extra)
    }

    /// The flash itself, for the simulator's image and the tests.
    pub fn nor(&self) -> core::cell::RefMut<'_, N> {
        self.nor.borrow_mut()
    }

    fn ok(&self) -> bool {
        self.mounted.get() != Mounted::Failed
    }

    fn bump(&self) {
        self.ver.set(self.ver.get().wrapping_add(1));
    }

    // ---- settings and slots ----

    /// The stored settings into `set` (left as they are when none are stored).
    pub fn load_settings(&self, set: &mut Settings) -> bool {
        self.with_record(&mut |r| {
            let mut n = 0u16;
            n.load(r);
            let mut sub = Limit { inner: r, left: n as u32 };
            set.load(&mut sub);
            sub.skip();
        })
    }

    /// The stored slots into the App (after [`FlashStore::load_settings`] and `App::new`), and the save timer set
    /// to what is stored.
    pub fn restore(&self, app: &mut App) -> bool {
        // The slots' bytes are copied out first: restoring reads channels from this store.
        let mut buf = [0u8; STATE_MAX];
        let mut len = 0;
        let found = self.with_record(&mut |r| {
            let mut n = 0u16;
            n.load(r);
            let mut sub = Limit { inner: r, left: n as u32 };
            sub.skip();
            while len < STATE_MAX && r.take(&mut buf[len..len + 1]) {
                len += 1;
            }
        });
        if found {
            app.restore_state(&mut ferrite_app::persist::SliceSource(&buf[..len]), self);
        }
        self.mark_saved(app);
        found
    }

    fn with_record(&self, f: &mut dyn FnMut(&mut dyn Source)) -> bool {
        if !self.ok() {
            return false;
        }
        let nor = &mut *self.nor.borrow_mut();
        match (Journal { region: SETTINGS }).latest(nor) {
            Ok(Some(l)) => {
                let mut src = FlashSource { nor, at: l.at, end: l.at + l.len };
                f(&mut src);
                true
            }
            _ => false,
        }
    }

    /// The save timer now agrees with the App: nothing to save.
    pub fn mark_saved(&self, app: &App) {
        let c = state_crc(app);
        self.saved.set(c);
        self.seen.set(c);
    }

    /// Call often (every UI loop): saves the settings and slots once they have stayed unchanged for
    /// [`SAVE_DELAY_MS`] since their last change. Returns true when it wrote.
    pub fn autosave(&self, now: u32, app: &App) -> bool {
        let c = state_crc(app);
        if c != self.seen.get() {
            self.seen.set(c);
            self.changed_at.set(now);
        }
        if self.seen.get() != self.saved.get() && now.wrapping_sub(self.changed_at.get()) >= SAVE_DELAY_MS {
            return self.save(app);
        }
        false
    }

    /// Saves now if anything changed since the last save (power off: PWR-17; empty battery: PWR-16).
    pub fn flush(&self, app: &App) -> bool {
        let c = state_crc(app);
        self.seen.set(c);
        c != self.saved.get() && self.save(app)
    }

    /// Writes the settings and slots now.
    pub fn save(&self, app: &App) -> bool {
        if !self.ok() {
            return false;
        }
        let nor = &mut *self.nor.borrow_mut();
        let ok = (Journal { region: SETTINGS }).append(nor, &|s: &mut dyn Sink| payload(app, s)).is_ok();
        if ok {
            self.saved.set(state_crc(app));
            if self.mounted.get() == Mounted::FirstBoot {
                self.mounted.set(Mounted::Ok);
            }
        }
        ok
    }

    // ---- channels ----

    fn read_ch(&self, num: u16) -> [u8; CH_LEN] {
        let mut b = [0xFFu8; CH_LEN];
        if num < MAX_CHANNELS && self.ok() {
            let _ = self.nor.borrow_mut().read(ch_addr(num), &mut b);
        }
        b
    }

    /// Writes one record at `num`: programmed in place when its slot is erased, else by rewriting its sector.
    fn put_rec(&self, num: u16, rec: &[u8; CH_LEN]) -> Result {
        let nor = &mut *self.nor.borrow_mut();
        let addr = ch_addr(num);
        if crate::blank(nor, addr, CH_LEN as u32)? {
            return nor.program(addr, rec);
        }
        let sector = addr - (addr - CHANNELS.start) % SECTOR;
        let off = addr - sector;
        swap::rewrite(nor, sector, &mut |o, page| {
            let (lo, hi) = (o.max(off), (o + crate::PAGE).min(off + CH_LEN as u32));
            for a in lo..hi {
                page[(a - o) as usize] = rec[(a - off) as usize];
            }
        })
    }

    fn clear_rec(&self, num: u16) -> Result {
        if !ch_valid(&self.read_ch(num)) && crate::blank(&mut *self.nor.borrow_mut(), ch_addr(num), CH_LEN as u32)? {
            return Ok(());
        }
        self.put_rec(num, &[0xFF; CH_LEN])
    }

    fn reindex(&self) -> Result {
        index::rebuild(&mut *self.nor.borrow_mut())
    }

    /// A channel write from the app ([`App::pop_write`]).
    pub fn apply(&self, w: ChanWrite) -> bool {
        if !self.ok() {
            return false;
        }
        let r = match w {
            ChanWrite::Put { ch, from } => self.put_channel(ch, from),
            ChanWrite::Delete(n) => self.clear_rec(n).and_then(|_| self.reindex()),
            ChanWrite::DeleteZone(z) => self.delete_zone(z),
            ChanWrite::EraseAll => self.erase_channels(),
        };
        self.bump();
        r.is_ok()
    }

    fn put_channel(&self, ch: Channel, from: Option<u16>) -> Result {
        if ch.num >= MAX_CHANNELS {
            return Err(NorError);
        }
        // What the UI doesn't model comes along from the record it replaces, or the one it moves from.
        let from = from.filter(|&f| f != ch.num && f < MAX_CHANNELS);
        let here = self.read_ch(ch.num);
        let old = match from {
            Some(f) if !ch_valid(&here) => self.read_ch(f),
            _ => here,
        };
        let rec = ch_encode(&ch, &old);
        if rec != here {
            self.put_rec(ch.num, &rec)?;
        }
        if let Some(f) = from {
            self.clear_rec(f)?;
        }
        self.reindex()
    }

    /// Writes a channel and sets its position (the simulator's sample; the web app's import later).
    pub fn put_with_position(&self, ch: Channel, pos: Option<Pos>) -> bool {
        let here = self.read_ch(ch.num);
        let mut rec = ch_encode(&ch, &here);
        if let Some(p) = pos {
            rec = ch_with_position(&rec, p);
        }
        let ok = self.put_rec(ch.num, &rec).is_ok();
        self.bump();
        ok
    }

    /// The index after a batch of [`FlashStore::put_with_position`].
    pub fn finish_batch(&self) -> bool {
        self.reindex().is_ok()
    }

    fn erase_channels(&self) -> Result {
        {
            let nor = &mut *self.nor.borrow_mut();
            for s in CHANNELS.clone().step_by(SECTOR as usize) {
                nor.erase(s)?;
            }
        }
        self.reindex()
    }

    // ---- zones ----

    fn read_zone(&self, z: u8) -> Option<(Name, ZoneCfg)> {
        if z == MAX_ZONES || !self.ok() {
            return None;
        }
        let mut b = [0xFFu8; ZONE_LEN];
        self.nor.borrow_mut().read(zone_addr(z), &mut b).ok()?;
        zone_decode(&b)
    }

    fn write_zone(&self, z: u8, rec: Option<[u8; ZONE_LEN]>) -> Result {
        let addr = zone_addr(z);
        let nor = &mut *self.nor.borrow_mut();
        if let Some(r) = rec {
            if crate::blank(nor, addr, ZONE_LEN as u32)? {
                return nor.program(addr, &r);
            }
        }
        let sector = addr - (addr - ZONE_RECS.start) % SECTOR;
        let off = addr - sector;
        let new = rec.unwrap_or([0xFF; ZONE_LEN]);
        swap::rewrite(nor, sector, &mut |o, page| {
            let (lo, hi) = (o.max(off), (o + crate::PAGE).min(off + ZONE_LEN as u32));
            for a in lo..hi {
                page[(a - o) as usize] = new[(a - off) as usize];
            }
        })
    }

    /// CHAN-12: the zone's channels go to no zone (they stay in All); the last zone's record goes, an earlier
    /// one stays as an empty zone (zone numbers don't move).
    fn delete_zone(&self, z: u8) -> Result {
        for num in 0..MAX_CHANNELS {
            let b = self.read_ch(num);
            if let Some(mut c) = ch_decode(num, &b).filter(|c| c.zone == z) {
                c.zone = NO_ZONE;
                self.put_rec(num, &ch_encode(&c, &b))?;
            }
        }
        if z + 1 == self.zones.get() {
            self.write_zone(z, None)?;
            self.zones.set(z);
        }
        self.reindex()
    }

    /// Adds a zone (the simulator's sample; NewZone).
    pub fn add_zone(&self, name: &[u8], cfg: &ZoneCfg) -> bool {
        let z = self.zones.get();
        let ok = z < MAX_ZONES && self.write_zone(z, Some(zone_encode(name, cfg))).is_ok();
        if ok {
            self.zones.set(z + 1);
        }
        self.bump();
        ok
    }

    // ---- range presets ----

    fn preset_slot(&self, i: u8) -> Option<u8> {
        (0..MAX_PRESETS).filter(|&k| self.read_preset(k).is_some()).nth(i as usize)
    }

    fn read_preset(&self, k: u8) -> Option<RangePreset> {
        if !self.ok() {
            return None;
        }
        let mut b = [0xFFu8; 32];
        self.nor.borrow_mut().read(preset_addr(k), &mut b).ok()?;
        preset_decode(&b)
    }

    fn write_preset(&self, k: u8, p: Option<&RangePreset>) -> Result {
        let addr = preset_addr(k);
        let nor = &mut *self.nor.borrow_mut();
        let head = p.map(preset_encode);
        if let Some(h) = head {
            if crate::blank(nor, addr, 32)? {
                return nor.program(addr, &h);
            }
        }
        let off = addr - PRESETS.start;
        swap::rewrite(nor, PRESETS.start, &mut |o, page| {
            let (lo, hi) = (o.max(off), (o + crate::PAGE).min(off + PRESET_LEN as u32));
            for a in lo..hi {
                let i = (a - off) as usize;
                // A deleted preset is erased whole; a changed one keeps its skip list.
                page[(a - o) as usize] = match head {
                    Some(h) if i < 32 => h[i],
                    Some(_) => page[(a - o) as usize],
                    None => 0xFF,
                };
            }
        })
    }

    /// Adds a range preset (the simulator's sample; NewPreset).
    pub fn add_preset(&self, p: &RangePreset) -> bool {
        let ok = match (0..MAX_PRESETS).find(|&k| self.read_preset(k).is_none()) {
            Some(k) => self.write_preset(k, Some(p)).is_ok(),
            None => false,
        };
        self.bump();
        ok
    }
}

/// The settings record's payload: the settings' length (2), the settings, the App's slots.
pub fn payload(app: &App, s: &mut dyn Sink) {
    let mut n = ferrite_app::persist::Count(0);
    app.settings().save(&mut n);
    (n.0 as u16).save(s);
    app.settings().save(s);
    app.save_state(s);
}

fn state_crc(app: &App) -> u32 {
    let mut c = Crc32::new();
    payload(app, &mut c);
    c.get()
}

/// At most `left` bytes of `inner`.
struct Limit<'a> {
    inner: &'a mut dyn Source,
    left: u32,
}

impl Limit<'_> {
    /// Skips what is left.
    fn skip(&mut self) {
        let mut b = [0u8; 16];
        while self.left > 0 {
            let n = self.left.min(16) as usize;
            if !self.inner.take(&mut b[..n]) {
                return;
            }
            self.left -= n as u32;
        }
    }
}

impl Source for Limit<'_> {
    fn take(&mut self, out: &mut [u8]) -> bool {
        if out.len() as u32 > self.left || !self.inner.take(out) {
            return false;
        }
        self.left -= out.len() as u32;
        true
    }
}

fn header_ok(nor: &mut impl Nor) -> Result<bool> {
    let mut h = [0u8; 20];
    nor.read(LAYOUT_HEADER.start, &mut h)?;
    let mut c = Crc32::new();
    c.update(&h[..16]);
    Ok(u32::from_le_bytes([h[0], h[1], h[2], h[3]]) == HEADER_MAGIC
        && u16::from_le_bytes([h[4], h[5]]) == LAYOUT_VERSION
        && u32::from_le_bytes([h[16], h[17], h[18], h[19]]) == c.get())
}

fn write_header(nor: &mut impl Nor) -> Result {
    let mut h = [0xFFu8; 20];
    h[..4].copy_from_slice(&HEADER_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&LAYOUT_VERSION.to_le_bytes());
    let mut c = Crc32::new();
    c.update(&h[..16]);
    h[16..20].copy_from_slice(&c.get().to_le_bytes());
    nor.erase(LAYOUT_HEADER.start)?;
    nor.program(LAYOUT_HEADER.start, &h)
}

/// Erases the settings, channels and zones (a new or erased chip). The header is written after, so a cut
/// here formats again at the next power on.
fn format(nor: &mut impl Nor) -> Result {
    for r in [SETTINGS, CHANNELS, ZONES] {
        for s in r.step_by(SECTOR as usize) {
            nor.erase(s)?;
        }
    }
    Ok(())
}

fn any_channel(nor: &mut impl Nor) -> Result<bool> {
    let mut b = [0u8; CH_LEN];
    for num in 0..MAX_CHANNELS {
        nor.read(ch_addr(num), &mut b)?;
        if ch_valid(&b) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Zones in use: up to the last valid zone record.
fn count_zones(nor: &mut impl Nor) -> Result<u8> {
    let mut n = 0;
    let mut b = [0u8; ZONE_LEN];
    for z in 0..MAX_ZONES {
        nor.read(zone_addr(z), &mut b)?;
        if zone_decode(&b).is_some() {
            n = z + 1;
        }
    }
    Ok(n)
}

impl<N: Nor, D: Data> ChannelStore for FlashStore<N, D> {
    fn zones(&self) -> u8 {
        self.zones.get()
    }

    fn zone_name(&self, zone: u8) -> Option<Name> {
        if zone >= self.zones.get() {
            return None;
        }
        // An earlier zone deleted (its record kept empty) or never named still exists by number.
        Some(self.read_zone(zone).map_or_else(Name::new, |z| z.0))
    }

    fn zone_len(&self, zone: u8) -> u16 {
        if !self.ok() {
            return 0;
        }
        index::zone_span(&mut *self.nor.borrow_mut(), zone).1
    }

    fn zone_at(&self, zone: u8, i: u16) -> Option<Channel> {
        if !self.ok() {
            return None;
        }
        let num = {
            let nor = &mut *self.nor.borrow_mut();
            let (s, n) = index::zone_span(nor, zone);
            if i >= n {
                return None;
            }
            index::num_at(nor, s + i)
        };
        ch_decode(num, &self.read_ch(num))
    }

    fn count(&self) -> u16 {
        if !self.ok() {
            return 0;
        }
        index::count(&mut *self.nor.borrow_mut())
    }

    fn by_num(&self, num: u16) -> Option<(Channel, u16)> {
        let c = ch_decode(num, &self.read_ch(num))?;
        let pos = index::pos_in_zone(&mut *self.nor.borrow_mut(), c.zone, num).unwrap_or(0);
        Some((c, pos))
    }

    fn free_num(&self, from: u16) -> Option<u16> {
        let mut used = [0u8; BITMAP_LEN as usize];
        if !self.ok() || index::bitmap(&mut *self.nor.borrow_mut(), &mut used).is_err() {
            return None;
        }
        (0..MAX_CHANNELS).map(|k| (from + k) % MAX_CHANNELS).find(|&n| used[n as usize / 8] & 1 << (n % 8) == 0)
    }

    fn by_number(&self, i: u16) -> Option<Channel> {
        let mut used = [0u8; BITMAP_LEN as usize];
        if !self.ok() || index::bitmap(&mut *self.nor.borrow_mut(), &mut used).is_err() {
            return None;
        }
        let num = (0..MAX_CHANNELS).filter(|&n| used[n as usize / 8] & 1 << (n % 8) != 0).nth(i as usize)?;
        ch_decode(num, &self.read_ch(num))
    }

    fn position(&self, num: u16) -> Option<Pos> {
        ch_position(&self.read_ch(num))
    }

    fn data(&self) -> &dyn Data {
        self
    }
}

impl<N: Nor, D: Data> Data for FlashStore<N, D> {
    fn version(&self) -> u32 {
        self.ver.get().wrapping_add(self.extra.version())
    }
    fn events(&self) -> u16 {
        self.extra.events()
    }
    fn event(&self, i: u16) -> Option<Event> {
        self.extra.event(i)
    }
    fn stations(&self) -> u16 {
        self.extra.stations()
    }
    fn station(&self, i: u16) -> Option<Station> {
        self.extra.station(i)
    }
    fn callsign(&self) -> Option<Buf<10>> {
        self.extra.callsign()
    }
    fn aprs_freq(&self) -> Freq10 {
        self.extra.aprs_freq()
    }
    fn messages(&self) -> u16 {
        self.extra.messages()
    }
    fn message(&self, i: u16) -> Option<Message> {
        self.extra.message(i)
    }
    fn draft(&self) -> Draft {
        self.extra.draft()
    }
    fn waypoints(&self) -> u16 {
        self.extra.waypoints()
    }
    fn waypoint(&self, i: u16) -> Option<Waypoint> {
        self.extra.waypoint(i)
    }
    fn sensors(&self) -> u16 {
        self.extra.sensors()
    }
    fn sensor(&self, i: u16) -> Option<Sensor> {
        self.extra.sensor(i)
    }
    fn noise(&self) -> Option<Noise> {
        self.extra.noise()
    }
    fn fix(&self) -> Option<Fix> {
        self.extra.fix()
    }
    fn sats(&self) -> u8 {
        self.extra.sats()
    }
    fn sat(&self, i: u8) -> Option<Sat> {
        self.extra.sat(i)
    }
    fn presets(&self) -> u8 {
        (0..MAX_PRESETS).filter(|&k| self.read_preset(k).is_some()).count() as u8
    }
    fn preset(&self, i: u8) -> Option<RangePreset> {
        self.read_preset(self.preset_slot(i)?)
    }
    fn skips(&self) -> u16 {
        self.extra.skips()
    }
    fn skip(&self, i: u16) -> Option<Freq10> {
        self.extra.skip(i)
    }
    fn zone_cfg(&self, zone: u8) -> ZoneCfg {
        self.read_zone(zone).map(|z| z.1).unwrap_or(NEW_ZONE)
    }
    fn fm_presets(&self) -> u8 {
        self.extra.fm_presets()
    }
    fn fm_preset(&self, i: u8) -> Option<FmPreset> {
        self.extra.fm_preset(i)
    }

    fn request(&self, op: Op) -> bool {
        let edit = |num: u16, f: &dyn Fn(&mut Channel)| -> bool {
            match ch_decode(num, &self.read_ch(num)) {
                Some(mut c) => {
                    f(&mut c);
                    self.apply(ChanWrite::Put { ch: c, from: None })
                }
                None => false,
            }
        };
        let ok = match op {
            Op::DeleteChannel(n) => ch_valid(&self.read_ch(n)) && self.apply(ChanWrite::Delete(n)),
            Op::Skip(n, on) => edit(n, &|c| c.flags = if on { c.flags | ferrite_app::chan::FLAG_SKIP } else { c.flags & !ferrite_app::chan::FLAG_SKIP }),
            Op::Move(n, z) => edit(n, &|c| c.zone = z),
            Op::Renumber(n, to) => {
                to < MAX_CHANNELS
                    && !ch_valid(&self.read_ch(to))
                    && match ch_decode(n, &self.read_ch(n)) {
                        Some(mut c) => {
                            c.num = to;
                            self.apply(ChanWrite::Put { ch: c, from: Some(n) })
                        }
                        None => false,
                    }
            }
            Op::NewZone => {
                let mut name = Buf::<16>::new();
                let _ = ufmt_zone(&mut name, self.zones.get());
                self.add_zone(name.as_bytes(), &NEW_ZONE)
            }
            Op::DeleteZone(z) => z < self.zones.get() && self.apply(ChanWrite::DeleteZone(z)),
            Op::Zone(z, cfg) => {
                z < self.zones.get() && {
                    let name = self.read_zone(z).map_or_else(Name::new, |x| x.0);
                    self.write_zone(z, Some(zone_encode(name.as_bytes(), &cfg))).is_ok()
                }
            }
            Op::RenameZone(z, name) => {
                z < self.zones.get() && {
                    let cfg = self.read_zone(z).map_or(NEW_ZONE, |x| x.1);
                    self.write_zone(z, Some(zone_encode(name.as_bytes(), &cfg))).is_ok()
                }
            }
            Op::Preset(i, p) => match self.preset_slot(i) {
                Some(k) => self.write_preset(k, Some(&p)).is_ok(),
                None => false,
            },
            Op::NewPreset => self.add_preset(&RangePreset {
                name: Buf::from(b"New range"),
                start: ferrite_core::freq::mhz(144),
                end: ferrite_core::freq::mhz(148),
                step: 10,
                mode: 0,
                narrow: true,
                skips: 0,
            }),
            Op::DeletePreset(i) => match self.preset_slot(i) {
                Some(k) => self.write_preset(k, None).is_ok(),
                None => false,
            },
            other => return self.extra.request(other),
        };
        self.bump();
        ok
    }
}

/// "Zone 7" for zone number 6.
fn ufmt_zone(b: &mut Buf<16>, z: u8) -> core::fmt::Result {
    b.set(b"Zone ");
    let n = z as u16 + 1;
    if n >= 100 {
        b.push(&[b'0' + (n / 100) as u8]);
    }
    if n >= 10 {
        b.push(&[b'0' + (n / 10 % 10) as u8]);
    }
    b.push(&[b'0' + (n % 10) as u8]);
    Ok(())
}
