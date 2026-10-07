//! [`FlashStore`]: the channels, zones and range presets the UI reads ([`ChannelStore`], [`Data`]), kept at
//! fixed places in SPI flash; the settings and slots saved 2 s after the last change ([`FlashStore::autosave`])
//! and at power off ([`FlashStore::flush`]).
//!
//! Mount ([`FlashStore::mount_async`]): finish a rewrite a power cut stopped; check the layout header (missing
//! or another layout: the chip is formatted, a first boot); read every channel's zone into the zone map (one
//! byte per channel number, the only copy in RAM); count the zones. No settings stored: a first boot (setup,
//! BOOT-12), and the first save after setup seeds the region's range presets.
//!
//! Writes: in the simulator and the tests they happen at once. On the radio ([`FlashStore::set_deferred`]) the
//! UI only queues them ([`Job`]) and the Storage task carries them out ([`FlashStore::next_job`],
//! [`FlashStore::run`]); erases wait on a timer there, so the UI never waits for the flash. A queued change
//! shows in the lists once it is written (well under a second); the slots hold their own copy meanwhile.
//!
//! The records the UI doesn't keep in flash yet (event log, APRS, GPS, waypoints, sensors, scan skips, FM
//! presets) come from `extra`: nothing on the radio for now, the sample data in the simulator.

use crate::io::Flash;
use crate::records::*;
use crate::settings::{self, FlashSource, MAX_PAYLOAD};
use crate::{block, scratch, Crc32, Nor, NorError, Result, SECTOR};
use core::cell::{Cell, RefCell};
use ferrite_app::chan::{ChanWrite, Channel, ChannelStore, Name, MAX_CHANNELS, NO_ZONE};
use ferrite_app::data::*;
use ferrite_app::persist::{Persist, Sink, Source};
use ferrite_app::settings::Settings;
use ferrite_app::App;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::layout::{CHANNELS, LAYOUT_HEADER, PRESETS, SCRATCH, SETTINGS, ZONES};
use ferrite_core::txplan::Region;
use ferrite_ui::text::Buf;

/// The layout header: magic, layout version, 10 bytes the installer may fill (0xFF until then), CRC-32 of the
/// 16 bytes before.
const HEADER_MAGIC: u32 = 0x5452_5246; // "FRRT"
/// 2: the fixed map (Oct 4, 2026). A chip with another version is formatted.
pub const LAYOUT_VERSION: u16 = 2;

/// The largest slots part [`FlashStore::restore`] reads (about 160 bytes now).
const STATE_MAX: usize = 512;

/// Saves wait this long after the last change (a turning knob doesn't wear the flash).
pub const SAVE_DELAY_MS: u32 = 2000;

/// The zone map's mark for a number with no channel (zones are 0-253, 255 is "no zone").
const EMPTY: u8 = 254;

/// Writes waiting for the Storage task.
const QUEUE: usize = 8;

/// What [`FlashStore::mount`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mounted {
    /// Header and settings in place.
    Ok,
    /// No settings stored: setup runs (a new or erased chip, formatted now).
    FirstBoot,
    /// The flash failed (or isn't mounted yet): nothing is stored this power-on.
    Failed,
}

/// A write for the Storage task (the radio), or done at once (the simulator).
#[derive(Clone, Copy)]
pub enum Job {
    Write(ChanWrite),
    Op(Op),
    /// The settings and slots in the save buffer: `len` bytes whose state CRC is `crc`; `seed` the region whose
    /// range presets a first boot gets.
    Save { crc: u32, len: u16, seed: Option<Region> },
}

pub struct FlashStore<N: Nor, D: Data> {
    f: Flash<N>,
    /// The records not in flash yet (see the module comment).
    pub extra: D,
    zones: Cell<u8>,
    /// Each channel number's zone, or [`EMPTY`].
    zmap: [Cell<u8>; MAX_CHANNELS as usize],
    ver: Cell<u32>,
    mounted: Cell<Mounted>,
    /// The save timer: CRC of the state last saved, of the state last seen, and when that changed.
    saved: Cell<u32>,
    seen: Cell<u32>,
    changed_at: Cell<u32>,
    /// Writes are queued for the Storage task (the radio) instead of done at once.
    deferred: Cell<bool>,
    jobs: RefCell<[Option<Job>; QUEUE]>,
    head: Cell<u8>,
    queued: Cell<u8>,
    running: Cell<bool>,
    /// The settings and slots being saved, and whether a save is under way (the UI leaves the buffer alone).
    save_buf: RefCell<[u8; MAX_PAYLOAD]>,
    saving: Cell<bool>,
}

impl<N: Nor, D: Data> FlashStore<N, D> {
    /// The store on `nor`, not mounted yet ([`FlashStore::mount_async`]).
    pub fn new(nor: N, extra: D) -> Self {
        FlashStore {
            f: Flash::new(nor),
            extra,
            zones: Cell::new(0),
            zmap: core::array::from_fn(|_| Cell::new(EMPTY)),
            ver: Cell::new(0),
            mounted: Cell::new(Mounted::Failed),
            saved: Cell::new(0),
            seen: Cell::new(0),
            changed_at: Cell::new(0),
            deferred: Cell::new(false),
            jobs: RefCell::new([None; QUEUE]),
            head: Cell::new(0),
            queued: Cell::new(0),
            running: Cell::new(false),
            save_buf: RefCell::new([0; MAX_PAYLOAD]),
            saving: Cell::new(false),
        }
    }

    /// Mounts at once (the simulator, the tests).
    pub fn mount(nor: N, extra: D) -> Self {
        let s = Self::new(nor, extra);
        block(s.mount_async());
        s
    }

    /// Mounts the store (see the module comment). Never fails: a flash that doesn't answer gives an empty store
    /// that keeps nothing ([`Mounted::Failed`]).
    pub async fn mount_async(&self) {
        let m = self.mount_inner().await.unwrap_or(Mounted::Failed);
        self.mounted.set(m);
    }

    async fn mount_inner(&self) -> Result<Mounted> {
        scratch::recover(&self.f).await?;
        if !self.header_ok()? {
            self.format().await?;
        }
        let mut b = [0u8; CH_LEN];
        for num in 0..MAX_CHANNELS {
            self.f.raw_read(ch_addr(num), &mut b)?;
            self.zmap[num as usize].set(if ch_valid(&b) { zkey(b[18]) } else { EMPTY });
        }
        let mut n = 0;
        let mut z = [0u8; ZONE_LEN];
        for k in 0..MAX_ZONES {
            self.f.raw_read(zone_addr(k), &mut z)?;
            if zone_decode(&z).is_some() {
                n = k + 1;
            }
        }
        self.zones.set(n);
        Ok(if settings::latest(&self.f)?.is_none() { Mounted::FirstBoot } else { Mounted::Ok })
    }

    fn header_ok(&self) -> Result<bool> {
        let mut h = [0u8; 20];
        self.f.raw_read(LAYOUT_HEADER.start, &mut h)?;
        let mut c = Crc32::new();
        c.update(&h[..16]);
        Ok(crate::u32_at(&h, 0) == HEADER_MAGIC && crate::u16_at(&h, 4) == LAYOUT_VERSION && crate::u32_at(&h, 16) == c.get())
    }

    /// A blank or foreign chip: every area erased, then the header (a cut before it formats again next time).
    async fn format(&self) -> Result {
        for r in [LAYOUT_HEADER, SETTINGS, CHANNELS, ZONES, PRESETS, SCRATCH] {
            for s in r.step_by(SECTOR as usize) {
                if !self.f.blank(s, SECTOR)? {
                    self.f.erase(s).await?;
                }
            }
        }
        let mut h = [0xFFu8; 20];
        h[..4].copy_from_slice(&HEADER_MAGIC.to_le_bytes());
        h[4..6].copy_from_slice(&LAYOUT_VERSION.to_le_bytes());
        let mut c = Crc32::new();
        c.update(&h[..16]);
        h[16..20].copy_from_slice(&c.get().to_le_bytes());
        self.f.program(LAYOUT_HEADER.start, &h).await
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
        (self.f.into_inner(), self.extra)
    }

    /// The flash itself, for the simulator's image and the tests.
    pub fn nor(&self) -> core::cell::RefMut<'_, N> {
        self.f.nor()
    }

    fn ok(&self) -> bool {
        self.mounted.get() != Mounted::Failed
    }

    fn bump(&self) {
        self.ver.set(self.ver.get().wrapping_add(1));
    }

    // ---- the queue (the radio) ----

    /// Writes go to the queue for the Storage task from now on.
    pub fn set_deferred(&self, on: bool) {
        self.deferred.set(on);
    }

    fn push(&self, j: Job) -> bool {
        let n = self.queued.get() as usize;
        if n == QUEUE {
            return false;
        }
        self.jobs.borrow_mut()[(self.head.get() as usize + n) % QUEUE] = Some(j);
        self.queued.set(n as u8 + 1);
        true
    }

    /// The next queued write, for the Storage task.
    pub fn next_job(&self) -> Option<Job> {
        if self.queued.get() == 0 {
            return None;
        }
        let h = self.head.get() as usize;
        let j = self.jobs.borrow_mut()[h].take();
        self.head.set(((h + 1) % QUEUE) as u8);
        self.queued.set(self.queued.get() - 1);
        j
    }

    /// Writes are waiting.
    pub fn has_jobs(&self) -> bool {
        self.queued.get() > 0
    }

    /// Nothing queued and nothing being written (power off waits for this: PWR-17).
    pub fn idle(&self) -> bool {
        self.queued.get() == 0 && !self.running.get()
    }

    /// Carries out a write (the Storage task; at once in the simulator).
    pub async fn run(&self, job: Job) -> bool {
        if !self.ok() {
            self.saving.set(false);
            return false;
        }
        self.running.set(true);
        let ok = match job {
            Job::Write(w) => self.apply_async(w).await.is_ok(),
            Job::Op(op) => self.perform(op).await.is_ok(),
            Job::Save { crc, len, seed } => self.save_async(crc, len as usize, seed).await,
        };
        self.bump();
        self.running.set(false);
        ok
    }

    fn submit(&self, j: Job) -> bool {
        if self.deferred.get() {
            self.push(j)
        } else {
            block(self.run(j))
        }
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
        match settings::latest(&self.f) {
            Ok(Some(c)) => {
                f(&mut FlashSource { f: &self.f, at: c.at, end: c.at + c.len });
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
    /// [`SAVE_DELAY_MS`] since their last change. Returns true when it wrote (or queued the write).
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

    /// Writes the settings and slots now (queues them on the radio). False while a save is still under way.
    pub fn save(&self, app: &App) -> bool {
        if !self.ok() || self.saving.get() {
            return false;
        }
        let len = {
            let mut buf = self.save_buf.borrow_mut();
            let mut sink = SliceSink { buf: &mut buf[..], len: 0, over: false };
            payload(app, &mut sink);
            if sink.over {
                return false;
            }
            sink.len
        };
        let set = app.settings();
        let seed = (self.first_boot() && set.setup_done).then(|| set.region().unwrap_or(Region::Other));
        self.saving.set(true);
        let ok = self.submit(Job::Save { crc: state_crc(app), len: len as u16, seed });
        if !ok {
            self.saving.set(false);
        }
        ok
    }

    async fn save_async(&self, crc: u32, len: usize, seed: Option<Region>) -> bool {
        let mut buf = [0u8; MAX_PAYLOAD];
        buf[..len].copy_from_slice(&self.save_buf.borrow()[..len]);
        let ok = settings::save(&self.f, &buf[..len]).await.is_ok();
        if ok {
            self.saved.set(crc);
            if let Some(r) = seed.filter(|_| self.first_boot()) {
                // After setup: the region's range presets, unless some are there already (the sample radio).
                if self.presets() == 0 {
                    for (k, p) in default_presets(r).enumerate() {
                        let _ = self.put_rec(preset_addr(k as u8), &preset_encode(&p)).await;
                    }
                }
                self.mounted.set(Mounted::Ok);
            }
        }
        self.saving.set(false);
        ok
    }

    // ---- reading ----

    fn read_ch(&self, num: u16) -> [u8; CH_LEN] {
        let mut b = [0xFFu8; CH_LEN];
        if num < MAX_CHANNELS && self.ok() {
            let _ = self.f.read(ch_addr(num), &mut b);
        }
        b
    }

    fn read_zone(&self, z: u8) -> Option<(Name, ZoneCfg)> {
        if z >= MAX_ZONES || !self.ok() {
            return None;
        }
        let mut b = [0xFFu8; ZONE_LEN];
        self.f.read(zone_addr(z), &mut b).ok()?;
        zone_decode(&b)
    }

    fn read_preset(&self, k: u8) -> Option<RangePreset> {
        if !self.ok() {
            return None;
        }
        let mut b = [0xFFu8; 32];
        self.f.read(preset_addr(k), &mut b).ok()?;
        preset_decode(&b)
    }

    fn preset_slot(&self, i: u8) -> Option<u8> {
        (0..MAX_PRESETS).filter(|&k| self.read_preset(k).is_some()).nth(i as usize)
    }

    /// The `i`th channel number (in number order) whose zone map entry passes `f`.
    fn nth_num(&self, i: u16, f: impl Fn(u8) -> bool) -> Option<u16> {
        (0..MAX_CHANNELS).filter(|&n| f(self.zmap[n as usize].get())).nth(i as usize)
    }

    // ---- writing ----

    /// Writes a record (up to 64 bytes): programmed in place when its bytes are erased, else by rewriting its
    /// sector through the scratch sector.
    async fn put_rec(&self, addr: u32, rec: &[u8]) -> Result {
        if self.f.blank(addr, rec.len() as u32)? {
            return self.f.program_record(addr, rec).await;
        }
        self.splice(addr, Some(rec), rec.len() as u32).await
    }

    /// Erases `len` bytes of records.
    async fn clear(&self, addr: u32, len: u32) -> Result {
        if self.f.blank(addr, len)? {
            return Ok(());
        }
        self.splice(addr, None, len).await
    }

    async fn splice(&self, addr: u32, new: Option<&[u8]>, len: u32) -> Result {
        let sector = addr - addr % SECTOR;
        let off = addr - sector;
        scratch::rewrite(&self.f, sector, &|o, piece| {
            for (i, b) in piece.iter_mut().enumerate() {
                let a = o + i as u32;
                if (off..off + len).contains(&a) {
                    *b = new.map_or(0xFF, |n| n[(a - off) as usize]);
                }
            }
        })
        .await
    }

    fn raw_ch(&self, num: u16) -> Result<[u8; CH_LEN]> {
        let mut b = [0xFFu8; CH_LEN];
        self.f.raw_read(ch_addr(num), &mut b)?;
        Ok(b)
    }

    /// A channel write from the app ([`App::pop_write`]).
    pub fn apply(&self, w: ChanWrite) -> bool {
        self.ok() && self.submit(Job::Write(w))
    }

    async fn apply_async(&self, w: ChanWrite) -> Result {
        match w {
            ChanWrite::Put { ch, from } => self.put_channel(ch, from).await,
            ChanWrite::Delete(n) => self.delete(n).await,
            ChanWrite::DeleteZone(z) => self.delete_zone(z).await,
            ChanWrite::EraseAll => self.erase_channels().await,
        }
    }

    async fn put_channel(&self, ch: Channel, from: Option<u16>) -> Result {
        if ch.num >= MAX_CHANNELS {
            return Err(NorError);
        }
        // What the UI doesn't model comes along from the record it replaces, or the one it moves from.
        let from = from.filter(|&f| f != ch.num && f < MAX_CHANNELS);
        let here = self.raw_ch(ch.num)?;
        let old = match from {
            Some(f) if !ch_valid(&here) => self.raw_ch(f)?,
            _ => here,
        };
        self.write_ch(ch.num, &ch_encode(&ch, &old)).await?;
        if let Some(f) = from {
            self.delete(f).await?;
        }
        Ok(())
    }

    async fn write_ch(&self, num: u16, rec: &[u8; CH_LEN]) -> Result {
        if *rec != self.raw_ch(num)? {
            self.put_rec(ch_addr(num), rec).await?;
        }
        self.zmap[num as usize].set(zkey(rec[18]));
        Ok(())
    }

    async fn delete(&self, num: u16) -> Result {
        if num >= MAX_CHANNELS {
            return Err(NorError);
        }
        self.clear(ch_addr(num), CH_LEN as u32).await?;
        self.zmap[num as usize].set(EMPTY);
        Ok(())
    }

    /// Writes a channel and sets its position (the simulator's sample; the web app's import later).
    pub fn put_with_position(&self, ch: Channel, pos: Option<Pos>) -> bool {
        let ok = block(async {
            let here = self.raw_ch(ch.num)?;
            let mut rec = ch_encode(&ch, &here);
            if let Some(p) = pos {
                rec = ch_with_position(&rec, p);
            }
            self.write_ch(ch.num, &rec).await
        })
        .is_ok();
        self.bump();
        ok
    }

    async fn erase_channels(&self) -> Result {
        for z in &self.zmap {
            z.set(EMPTY);
        }
        for s in CHANNELS.step_by(SECTOR as usize) {
            if !self.f.blank(s, SECTOR)? {
                self.f.erase_blanked(s).await?;
            }
        }
        Ok(())
    }

    /// CHAN-12: the zone's channels go to no zone (they stay in All); the last zone's record goes, an earlier
    /// one stays as an empty zone (zone numbers don't move).
    async fn delete_zone(&self, z: u8) -> Result {
        for num in 0..MAX_CHANNELS {
            if self.zmap[num as usize].get() != z {
                continue;
            }
            let b = self.raw_ch(num)?;
            if let Some(mut c) = ch_decode(num, &b) {
                c.zone = NO_ZONE;
                self.write_ch(num, &ch_encode(&c, &b)).await?;
            }
        }
        if z + 1 == self.zones.get() {
            self.clear(zone_addr(z), ZONE_LEN as u32).await?;
            self.zones.set(z);
        }
        Ok(())
    }

    async fn write_zone(&self, z: u8, name: &[u8], cfg: &ZoneCfg) -> Result {
        self.put_rec(zone_addr(z), &zone_encode(name, cfg)).await
    }

    async fn new_zone(&self, name: &[u8], cfg: &ZoneCfg) -> Result {
        let z = self.zones.get();
        if z >= MAX_ZONES {
            return Err(NorError);
        }
        self.write_zone(z, name, cfg).await?;
        self.zones.set(z + 1);
        Ok(())
    }

    /// Adds a zone (the simulator's sample).
    pub fn add_zone(&self, name: &[u8], cfg: &ZoneCfg) -> bool {
        let ok = block(self.new_zone(name, cfg)).is_ok();
        self.bump();
        ok
    }

    async fn new_preset(&self, p: &RangePreset) -> Result {
        match (0..MAX_PRESETS).find(|&k| self.read_preset(k).is_none()) {
            Some(k) => self.put_rec(preset_addr(k), &preset_encode(p)).await,
            None => Err(NorError),
        }
    }

    /// Adds a range preset (the simulator's sample).
    pub fn add_preset(&self, p: &RangePreset) -> bool {
        let ok = block(self.new_preset(p)).is_ok();
        self.bump();
        ok
    }

    // ---- requests from the screens ----

    /// Whether `op` can go ahead now (None: not a store record, the extra records answer it).
    fn accepts(&self, op: &Op) -> Option<bool> {
        let exists = |n: u16| n < MAX_CHANNELS && ch_valid(&self.read_ch(n));
        Some(match *op {
            Op::DeleteChannel(n) | Op::Skip(n, _) | Op::Hide(n, _) | Op::Move(n, _) => exists(n),
            Op::Renumber(n, to) => to < MAX_CHANNELS && !exists(to) && exists(n),
            Op::NewZone => self.zones.get() < MAX_ZONES,
            Op::DeleteZone(z) | Op::Zone(z, _) | Op::RenameZone(z, _) => z < self.zones.get(),
            Op::Preset(i, _) | Op::DeletePreset(i) => self.preset_slot(i).is_some(),
            Op::NewPreset => self.presets() < MAX_PRESETS,
            // The next one is created: a zone up to the last, a preset while there is room.
            Op::PutZone(z, ..) => z < MAX_ZONES && z <= self.zones.get(),
            Op::PutPreset(i, _) => self.preset_slot(i).is_some() || (i == self.presets() && self.presets() < MAX_PRESETS),
            Op::Dtmf(i, _) => i < DTMF_MEMS,
            _ => return None,
        })
    }

    async fn perform(&self, op: Op) -> Result {
        let edit = async |num: u16, f: &dyn Fn(&mut Channel)| -> Result {
            let b = self.raw_ch(num)?;
            let mut c = ch_decode(num, &b).ok_or(NorError)?;
            f(&mut c);
            self.write_ch(num, &ch_encode(&c, &b)).await
        };
        match op {
            Op::DeleteChannel(n) => self.delete(n).await,
            Op::Skip(n, on) => edit(n, &|c| c.flags = if on { c.flags | ferrite_app::chan::FLAG_SKIP } else { c.flags & !ferrite_app::chan::FLAG_SKIP }).await,
            Op::Hide(n, on) => edit(n, &|c| c.flags = if on { c.flags | ferrite_app::chan::FLAG_HIDDEN } else { c.flags & !ferrite_app::chan::FLAG_HIDDEN }).await,
            Op::Move(n, z) => edit(n, &|c| c.zone = z).await,
            Op::Renumber(n, to) => {
                if ch_valid(&self.raw_ch(to)?) {
                    return Err(NorError);
                }
                let mut c = ch_decode(n, &self.raw_ch(n)?).ok_or(NorError)?;
                c.num = to;
                self.put_channel(c, Some(n)).await
            }
            Op::NewZone => {
                let mut name = Buf::<16>::new();
                zone_name(&mut name, self.zones.get());
                self.new_zone(name.as_bytes(), &NEW_ZONE).await
            }
            Op::DeleteZone(z) => self.delete_zone(z).await,
            Op::Zone(z, cfg) => {
                let name = self.read_zone(z).map_or_else(Name::new, |x| x.0);
                self.write_zone(z, name.as_bytes(), &cfg).await
            }
            Op::RenameZone(z, name) => {
                let cfg = self.read_zone(z).map_or(NEW_ZONE, |x| x.1);
                self.write_zone(z, name.as_bytes(), &cfg).await
            }
            Op::Preset(i, p) => match self.preset_slot(i) {
                Some(k) => self.put_rec(preset_addr(k), &preset_encode(&p)).await,
                None => Err(NorError),
            },
            Op::NewPreset => {
                self.new_preset(&RangePreset { name: Buf::from(b"New range"), start: mhz(144), end: mhz(148), step: 10, mode: 0, narrow: true, skips: 0 })
                    .await
            }
            Op::DeletePreset(i) => match self.preset_slot(i) {
                Some(k) => self.clear(preset_addr(k), PRESET_LEN as u32).await,
                None => Err(NorError),
            },
            Op::PutZone(z, name, cfg) if z == self.zones.get() => self.new_zone(name.as_bytes(), &cfg).await,
            Op::PutZone(z, name, cfg) => self.write_zone(z, name.as_bytes(), &cfg).await,
            Op::PutPreset(i, p) => match self.preset_slot(i) {
                Some(k) => {
                    let skips = self.read_preset(k).map_or(0, |o| o.skips);
                    self.put_rec(preset_addr(k), &preset_encode(&RangePreset { skips, ..p })).await
                }
                None => self.new_preset(&RangePreset { skips: 0, ..p }).await,
            },
            Op::Dtmf(i, Some(m)) => self.put_rec(dtmf_addr(i), &dtmf_encode(&m)).await,
            Op::Dtmf(i, None) => self.clear(dtmf_addr(i), DTMF_LEN as u32).await,
            _ => Ok(()),
        }
    }
}

/// The zone map's key for a channel's zone byte (anything past the last zone counts as no zone).
fn zkey(z: u8) -> u8 {
    if z < MAX_ZONES { z } else { NO_ZONE }
}

/// The range presets a first boot gets after setup, by region (`0 1 7`'s default: 2 m, 70 cm, airband, marine,
/// plus what the region has: weather, GMRS/FRS, 1.25 m, PMR446). Steps index `slot::STEPS`, modes `slot::MODES`.
pub fn default_presets(r: Region) -> impl Iterator<Item = RangePreset> {
    use Region::*;
    let na = matches!(r, Us | Canada);
    let wide2m = matches!(r, Us | Canada | Australia | NewZealand);
    let wide70 = matches!(r, Us | Canada | Australia);
    let eu = matches!(r, Uk | Europe);
    // name, start, end (10 Hz), step, mode, narrow
    let all: [(bool, &str, Freq10, Freq10, u8, u8, bool); 9] = [
        (true, "2 m", mhz(144), if wide2m { mhz(148) } else { mhz(146) }, 10, 0, true),
        (true, "70 cm", if wide70 { mhz(420) } else { mhz(430) }, if wide70 { mhz(450) } else { mhz(440) }, 10, 0, true),
        (r == Us, "1.25 m", mhz(222), mhz(225), 6, 0, true),
        (true, "Airband", mhz(118), mhz(137), if eu { 8 } else { 11 }, 2, false),
        (true, "Marine", mhz(156), mhz(162) + 2_500, 11, 0, false),
        (na, "Weather", mhz(162) + 40_000, mhz(162) + 55_000, 11, 0, false),
        (na, "GMRS/FRS", mhz(462) + 55_000, mhz(467) + 72_500, 10, 0, true),
        (eu, "PMR446", mhz(446) + 625, mhz(446) + 19_375, 7, 0, true),
        (false, "", 0, 0, 0, 0, false),
    ];
    all.into_iter().filter(|p| p.0).map(|(_, n, start, end, step, mode, narrow)| RangePreset {
        name: Buf::from(n.as_bytes()),
        start,
        end,
        step,
        mode,
        narrow,
        skips: 0,
    })
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

/// Bytes into a slice; `over` when they didn't fit.
struct SliceSink<'a> {
    buf: &'a mut [u8],
    len: usize,
    over: bool,
}

impl Sink for SliceSink<'_> {
    fn put(&mut self, b: &[u8]) {
        if self.len + b.len() > self.buf.len() {
            self.over = true;
            return;
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
    }
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
        self.zmap.iter().filter(|z| z.get() == zone).count() as u16
    }

    fn zone_at(&self, zone: u8, i: u16) -> Option<Channel> {
        let num = self.nth_num(i, |z| z == zone)?;
        ch_decode(num, &self.read_ch(num))
    }

    fn count(&self) -> u16 {
        self.zmap.iter().filter(|z| z.get() != EMPTY).count() as u16
    }

    fn by_num(&self, num: u16) -> Option<(Channel, u16)> {
        let c = ch_decode(num, &self.read_ch(num))?;
        let key = zkey(c.zone);
        let pos = (0..num).filter(|&n| self.zmap[n as usize].get() == key).count() as u16;
        Some((c, pos))
    }

    fn free_num(&self, from: u16) -> Option<u16> {
        (0..MAX_CHANNELS).map(|k| (from + k) % MAX_CHANNELS).find(|&n| self.zmap[n as usize].get() == EMPTY)
    }

    fn get(&self, num: u16) -> Option<Channel> {
        if self.zmap.get(num as usize)?.get() == EMPTY {
            return None;
        }
        ch_decode(num, &self.read_ch(num))
    }

    fn by_number(&self, i: u16) -> Option<Channel> {
        let num = self.nth_num(i, |z| z != EMPTY)?;
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
    fn aprs_cfg(&self) -> AprsCfg {
        self.extra.aprs_cfg()
    }
    fn beacon(&self) -> Beacon {
        self.extra.beacon()
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
    fn quick(&self, i: usize) -> Option<Buf<67>> {
        self.extra.quick(i)
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
    fn dtmf(&self, i: u8) -> Option<Dtmf> {
        if !self.ok() || i >= DTMF_MEMS {
            return None;
        }
        let mut b = [0xFFu8; DTMF_LEN];
        self.f.read(dtmf_addr(i), &mut b).ok()?;
        dtmf_decode(&b)
    }
    fn fm_presets(&self) -> u8 {
        self.extra.fm_presets()
    }
    fn fm_preset(&self, i: u8) -> Option<FmPreset> {
        self.extra.fm_preset(i)
    }

    fn busy(&self) -> bool {
        !self.idle()
    }

    fn request(&self, op: Op) -> bool {
        match self.accepts(&op) {
            Some(ok) => self.ok() && ok && self.submit(Job::Op(op)),
            None => self.extra.request(op),
        }
    }
}

/// "Zone 7" for zone number 6.
fn zone_name(b: &mut Buf<16>, z: u8) {
    b.set(b"Zone ");
    let n = z as u16 + 1;
    if n >= 100 {
        b.push(&[b'0' + (n / 100) as u8]);
    }
    if n >= 10 {
        b.push(&[b'0' + (n / 10 % 10) as u8]);
    }
    b.push(&[b'0' + (n % 10) as u8]);
}
