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
//! The event log, the APRS messages, the APRS settings and quick messages, and the waypoints are kept here
//! too ([`crate::logs`]); the message being written stays in RAM. What other tasks keep in their RAM (APRS
//! heard, the beacon's state, the GPS fix and satellites, this scan's skips) come from
//! `extra`: nothing on the radio for now, the sample data in the simulator.

use crate::io::Flash;
use crate::logs::{self, Contacts, Ring, Waypoints};
use crate::records::*;
use crate::settings::{self, FlashSource, MAX_PAYLOAD};
use crate::{block, scratch, Crc32, Nor, NorError, Result, SECTOR};
use ferrite_app::aprs::AprsCfg;
use core::cell::{Cell, RefCell};
use ferrite_app::chan::{ChanWrite, Channel, ChannelStore, Name, MAX_CHANNELS, NO_ZONE};
use ferrite_app::data::*;
use ferrite_app::persist::{Persist, Sink, Source};
use ferrite_app::settings::Settings;
use ferrite_app::App;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::layout::{APRS, CHANNELS, CONTACTS, EVENT_LOG, HEARD, LAYOUT_HEADER, PRESETS, SETTINGS, SPARE, TRIMS, WAYPOINTS, ZONES};
use ferrite_core::txplan::Region;
use ferrite_ui::text::Buf;

/// The layout header: magic, layout version, 10 bytes the installer may fill (0xFF until then), CRC-32 of the
/// 16 bytes before. Bytes 6-9 hold the calibration sector's CRC-32 (CAL-7), programmed into the erased bytes on
/// the first boot that records it; the header's own CRC takes them as 0xFF, so recording it later keeps the
/// header valid (and headers written before it still are).
const HEADER_MAGIC: u32 = 0x5452_5246; // "FRRT"
/// Where in the layout header the calibration's CRC is (CAL-7).
const CAL_CRC_AT: usize = 6;
/// 2: the fixed map (Oct 4, 2026). A chip with another version is formatted.
pub const LAYOUT_VERSION: u16 = 2;

/// The largest slots part [`FlashStore::restore`] reads (about 160 bytes now).
const STATE_MAX: usize = 512;

/// Saves wait this long after the last change (a turning knob doesn't wear the flash).
pub const SAVE_DELAY_MS: u32 = 2000;

/// The zone map's mark for a number with no channel (zones are 0-253, 255 is "no zone").
const EMPTY: u8 = 254;
/// The weather zone's first channel record (WX-1): channels 991-997 as shown, out of the way of your own numbering.
pub const WX_FIRST: u16 = 990;

/// Writes waiting for the Storage task: 8 (decision 287; 16 took 1 KB more for a 1,000-channel overwrite of 22 s
/// instead of 34 s).
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

/// A queued channel write taken into another's rewrite (decision 227): its number and record (None: deleted).
type Merge = (u16, Option<[u8; CH_LEN]>);

/// A write for the Storage task (the radio), or done at once (the simulator).
#[derive(Clone, Copy)]
pub enum Job {
    Write(ChanWrite),
    Op(Op),
    /// The settings and slots in the save buffer: `len` bytes whose state CRC is `crc`; `seed` the region whose
    /// range presets a first boot gets.
    Save { crc: u32, len: u16, seed: Option<Region> },
    /// An event for the log ([`logs::event_job`]), stamped when it is written.
    Log([u8; logs::EVENT_JOB]),
    /// An APRS message: one for us (unread), or ours (pending).
    Msg { inbound: bool, peer: Buf<10>, text: Buf<67>, id: Buf<5>, ago_s: u32 },
    /// Our message to `peer` with `text` moved on (tried again, acked, failed).
    MsgState { peer: Buf<10>, text: Buf<67>, state: MsgState },
    /// A heard station's row (APRS-4, [`logs::heard_job`]), stamped when it is written.
    Heard([u8; logs::HEARD_JOB]),
    /// Event (`log`) or message `seq` read.
    Read { log: bool, seq: u32 },
    Waypoint(Waypoint),
    DeleteWaypoint(Waypoint),
    Contact(Contact),
    DeleteContact(Buf<10>),
    /// The APRS settings (kept in RAM since their last change) and the quick messages.
    AprsSave,
}

/// The APRS record: the settings (4), the quick messages (each a length, 0xFF for the factory text, and 67
/// bytes), then the settings added later (path, digipeater, beacon after voice with telemetry as its bit 1
/// (APRS-54), the SSID: [`APRS_MORE`]), then the custom path (0 5 9 2, zero-padded: [`APRS_PATH`]), then FX.25 on transmit (0 5 9 5, APRS-45:
/// [`APRS_FX25`]), the message sound, symbol, TX delay and level, then the comment and ambiguity
/// ([`APRS_COMMENT`]), then Mic-E ([`APRS_MIC_E`]). A record from before them is shorter and reads them as their
/// defaults.
const QUICK_LEN: usize = 68;
const APRS_MORE: usize = 4 + ferrite_app::words::QUICK * QUICK_LEN;
const APRS_PATH: usize = APRS_MORE + 4;
const APRS_FX25: usize = APRS_PATH + ferrite_app::aprs::CUSTOM_LEN;
const APRS_SOUND: usize = APRS_FX25 + 1;
/// APRS-47 - APRS-49: symbol, TX delay, TX level (an older record reads their defaults).
const APRS_SYMBOL: usize = APRS_SOUND + 1;
/// APRS-50, APRS-51: the comment (zero-padded) and the ambiguity (an older record reads none and exact).
const APRS_COMMENT: usize = APRS_SYMBOL + 3;
const APRS_AMBIG: usize = APRS_COMMENT + ferrite_app::aprs::COMMENT_LEN;
/// APRS-52: Mic-E (an older record reads Off).
const APRS_MIC_E: usize = APRS_AMBIG + 1;
const APRS_PAYLOAD: usize = APRS_MIC_E + 1;
const APRS_COPIES: [u32; 2] = [APRS.start, APRS.start + SECTOR];
/// APRS-32: the stored messages [`FlashStore::message_heard`] looks through, newest first.
pub const HEARD_CHECK: u16 = 16;
/// The store's clock starts here when nothing is stored and no clock is set: about UTC when Ferrite was written
/// (Sep 2026), so a GPS fix later moves it by a plausible amount rather than decades.
const TIME0: u32 = 1_790_000_000;

pub struct FlashStore<N: Nor, D: Data> {
    f: Flash<N>,
    /// The records not in flash yet (see the module comment).
    pub extra: D,
    zones: Cell<u8>,
    /// Each channel number's zone, or [`EMPTY`].
    zmap: [Cell<u8>; MAX_CHANNELS as usize],
    /// Recent (CHAN-25): 10 channel numbers in RAM, not saved.
    recent: ferrite_app::chan::RecentList,
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
    /// The running job is a channel write (LINK-41.1: only those let a link's channel records queue behind them).
    running_write: Cell<bool>,
    /// The settings and slots being saved, and whether a save is under way (the UI leaves the buffer alone).
    save_buf: RefCell<[u8; MAX_PAYLOAD]>,
    saving: Cell<bool>,
    events: Ring,
    msgs: Ring,
    /// The heard list's rows (APRS-4).
    heard: Ring,
    wps: Waypoints,
    contacts: Contacts,
    /// The APRS settings, and when they last changed while not yet written (saved like the settings).
    aprs: Cell<AprsCfg>,
    aprs_changed: Cell<Option<u32>>,
    /// The message being written (APRS-7): RAM only, as it changes with every key.
    draft: Cell<Draft>,
    /// The store's clock: seconds at uptime 0, and the uptime the UI last gave.
    time0: Cell<u32>,
    now_ms: Cell<u32>,
    /// The clock is UTC (GPS, or set by hand), and what it was at mount: this power-on's own frame (INBOX-6.1).
    utc: Cell<bool>,
    boot0: Cell<u32>,
}

impl<N: Nor, D: Data> FlashStore<N, D> {
    /// The store on `nor`, not mounted yet ([`FlashStore::mount_async`]).
    pub fn new(nor: N, extra: D) -> Self {
        FlashStore {
            f: Flash::new(nor),
            extra,
            zones: Cell::new(0),
            zmap: core::array::from_fn(|_| Cell::new(EMPTY)),
            recent: ferrite_app::chan::RecentList::new(),
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
            running_write: Cell::new(false),
            save_buf: RefCell::new([0; MAX_PAYLOAD]),
            saving: Cell::new(false),
            events: Ring::new(EVENT_LOG.start, EVENT_LOG.end, logs::EVENT_END),
            msgs: Ring::new(APRS.start + 2 * SECTOR, APRS.end, logs::MSG_END),
            heard: Ring::new(HEARD.start, HEARD.end, logs::HEARD_END),
            wps: Waypoints::new(WAYPOINTS.start),
            contacts: Contacts::new(CONTACTS.start),
            aprs: Cell::new(AprsCfg::DEFAULT),
            aprs_changed: Cell::new(None),
            draft: Cell::new(Draft::default()),
            time0: Cell::new(TIME0),
            now_ms: Cell::new(0),
            utc: Cell::new(false),
            boot0: Cell::new(TIME0),
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
        // Decision 227: the scratch ring starts where the settings' save count says, a different place each
        // power-on (they are saved at every power-off).
        self.f.scratch_seed(settings::latest(&self.f)?.map_or(0, |c| c.count));
        if !self.header_ok()? {
            self.format().await?;
        }
        {
            // In a block: the buffers aren't kept across the waits below (they would grow the UI task).
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
        }
        self.events.mount(&self.f).await?;
        self.msgs.mount(&self.f).await?;
        // APRS-13.5: ours left open at power-off (their tries were in RAM) end as failed, so nothing stale goes out
        // at power-on; Send again (APRS-13.2) sends one on purpose.
        if let Some((at, v)) = logs::msg_state_byte(MsgState::Failed) {
            for i in 0..self.msgs.len() {
                if let (Some(seq), Some(b)) = (self.msgs.seq(i), self.msgs.read(&self.f, i)) {
                    if logs::msg_is_open(&b) {
                        self.msgs.mark(&self.f, seq, at, v).await?;
                    }
                }
            }
        }
        self.heard.mount(&self.f).await?;
        self.wps.mount(&self.f).await?;
        self.contacts.0.mount(&self.f).await?;
        let t = [self.events.newest_time(&self.f), self.msgs.newest_time(&self.f), self.heard.newest_time(&self.f)].into_iter().flatten().max();
        self.time0.set(t.map_or(TIME0, |t| t.saturating_add(1)).max(TIME0));
        self.boot0.set(self.time0.get());
        if let Some(c) = settings::latest_of(&self.f, APRS_COPIES, APRS_PAYLOAD)? {
            let mut b = [0u8; 4];
            self.f.raw_read(c.at, &mut b)?;
            let mut cfg = AprsCfg { f5k: crate::u16_at(&b, 0), beacon: b[2], power: b[3].min(2), ..AprsCfg::DEFAULT };
            if c.len as usize > APRS_FX25 {
                let mut x = [0u8; 1];
                self.f.raw_read(c.at + APRS_FX25 as u32, &mut x)?;
                if ferrite_app::aprs::FX25_CHECKS.contains(&x[0]) {
                    cfg.fx25 = x[0];
                }
            }
            if c.len as usize > APRS_SOUND {
                let mut x = [0u8; 1];
                self.f.raw_read(c.at + APRS_SOUND as u32, &mut x)?;
                if (x[0] as usize) < ferrite_core::tonegen::CHIMES.len() {
                    cfg.sound = x[0];
                }
            }
            if c.len as usize >= APRS_PAYLOAD {
                let mut x = [0u8; 1];
                self.f.raw_read(c.at + APRS_MIC_E as u32, &mut x)?;
                if (x[0] as usize) < ferrite_app::aprs::MIC_E_NAMES.len() {
                    cfg.mic_e = x[0];
                }
            }
            if c.len as usize >= APRS_MIC_E {
                let mut t = [0u8; ferrite_app::aprs::COMMENT_LEN + 1];
                self.f.raw_read(c.at + APRS_COMMENT as u32, &mut t)?;
                // printable text up to the first zero; anything else reads as none
                let n = t[..t.len() - 1].iter().position(|&b| b == 0).unwrap_or(t.len() - 1);
                if t[..n].iter().all(|&b| (b' '..=b'~').contains(&b)) {
                    cfg.comment[..n].copy_from_slice(&t[..n]);
                }
                cfg.ambiguity = if t[t.len() - 1] <= 4 { t[t.len() - 1] } else { 0 };
            }
            if c.len as usize >= APRS_COMMENT {
                let mut x = [0u8; 3];
                self.f.raw_read(c.at + APRS_SYMBOL as u32, &mut x)?;
                if (x[0] as usize) < ferrite_app::aprs::SYMBOLS.len() {
                    cfg.symbol = x[0];
                }
                if (ferrite_app::aprs::TXDELAY_MIN..=ferrite_app::aprs::TXDELAY_MAX).contains(&x[1]) {
                    cfg.txdelay = x[1];
                }
                if x[2] <= 2 {
                    cfg.tx_level = x[2];
                }
            }
            if c.len as usize >= APRS_FX25 {
                let mut t = [0u8; ferrite_app::aprs::CUSTOM_LEN];
                self.f.raw_read(c.at + APRS_PATH as u32, &mut t)?;
                // kept only if it still checks (APRS-26); anything else reads as none
                let n = t.iter().position(|&b| b == 0).unwrap_or(t.len());
                if ferrite_app::aprs::path_ok(&t[..n]) {
                    cfg.custom[..n].copy_from_slice(&t[..n]);
                }
            }
            if c.len as usize >= APRS_PATH {
                let mut m = [0u8; 4];
                self.f.raw_read(c.at + APRS_MORE as u32, &mut m)?;
                // APRS-26: custom keeps its value 3 from before APRSAT and ARISS (4, 5); kept only with a custom path set
                cfg.path = if cfg.path_ok(m[0]) { m[0] } else { cfg.path };
                cfg.digi = m[1] & 0x73;
                // APRS-54: the after-voice byte's bit 1 is telemetry (the record's length unchanged); erased (0xFF)
                // reads both off
                (cfg.after_voice, cfg.telemetry) = if m[2] == 0xFF { (false, false) } else { (m[2] & 1 != 0, m[2] & 2 != 0) };
                // APRS-30: the spare byte; erased (0xFF, a record from before it) reads as the default
                cfg.ssid = if m[3] < 16 { m[3] } else { ferrite_app::aprs::SSID_DEFAULT };
            }
            self.aprs.set(cfg);
            self.extra.request(Op::AprsCfg(cfg)); // the APRS task's copy
        }
        Ok(if settings::latest(&self.f)?.is_none() { Mounted::FirstBoot } else { Mounted::Ok })
    }

    fn header_ok(&self) -> Result<bool> {
        let mut h = [0u8; 20];
        self.f.raw_read(LAYOUT_HEADER.start, &mut h)?;
        let mut c = Crc32::new();
        let mut body = [0u8; 16];
        body.copy_from_slice(&h[..16]);
        body[CAL_CRC_AT..CAL_CRC_AT + 4].fill(0xFF);
        c.update(&body);
        Ok(crate::u32_at(&h, 0) == HEADER_MAGIC && crate::u16_at(&h, 4) == LAYOUT_VERSION && crate::u32_at(&h, 16) == c.get())
    }

    /// CAL-7 (TX audit F8): the calibration sector (read only) against the CRC the layout header recorded; none
    /// recorded yet (the first boot of a build that records it, or after a format) records it now, into the
    /// header's erased bytes. Unknown when the store isn't mounted. Only the layout header is ever written here.
    pub async fn check_cal(&self) -> ferrite_core::txcal::CalCrc {
        use ferrite_core::txcal::CalCrc;
        if !matches!(self.mounted.get(), Mounted::Ok | Mounted::FirstBoot) {
            return CalCrc::Unknown;
        }
        let cal = ferrite_core::layout::CALIBRATION;
        let Ok(now) = self.f.crc(cal.start, cal.end - cal.start) else { return CalCrc::Unknown };
        let mut rec = [0u8; 4];
        if self.f.raw_read(LAYOUT_HEADER.start + CAL_CRC_AT as u32, &mut rec).is_err() {
            return CalCrc::Unknown;
        }
        if rec == [0xFF; 4] {
            return match self.f.program(LAYOUT_HEADER.start + CAL_CRC_AT as u32, &now.to_le_bytes()).await {
                Ok(()) => CalCrc::Recorded,
                Err(_) => CalCrc::Unknown,
            };
        }
        if u32::from_le_bytes(rec) == now {
            CalCrc::Same
        } else {
            CalCrc::Changed
        }
    }

    /// A blank or foreign chip: every area erased, then the header (a cut before it formats again next time).
    async fn format(&self) -> Result {
        for r in [LAYOUT_HEADER, SETTINGS, CHANNELS, ZONES, PRESETS, SPARE, APRS, EVENT_LOG, WAYPOINTS, CONTACTS, HEARD] {
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

    /// HF-20: the SSB patch in SPI flash, if its header parses and its bytes hash to the header's SHA-256.
    pub fn ssb_check(&self) -> Result<Option<ferrite_core::ssb::Patch>> {
        crate::ssb::check(&self.f)
    }

    /// HF-18: stores `patch` (with `p` from its bundle or link header) in SSB_PATCH, unless the same verified copy is
    /// already there (Ok(false)); a format never erases it (SAFE-9). Verified after writing; Err if it doesn't check.
    pub async fn ssb_install(&self, p: ferrite_core::ssb::Patch, patch: &[u8], source: u8) -> Result<bool> {
        crate::ssb::install(&self.f, p, patch, source).await
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
        self.running_write.set(matches!(job, Job::Write(_)));
        let ok = match job {
            Job::Write(w) => self.apply_async(w).await.is_ok(),
            Job::Op(op) => self.perform(op).await.is_ok(),
            Job::Save { crc, len, seed } => self.save_async(crc, len as usize, seed).await,
            Job::Log(j) => {
                let mut r = logs::event_rec(&j, self.now_s(), self.utc.get());
                self.events.append(&self.f, &mut r).await.is_ok()
            }
            Job::Msg { inbound, peer, text, id, ago_s } => {
                let mut r = logs::msg_rec(inbound, &peer, &text, &id, self.now_s().saturating_sub(ago_s), self.utc.get());
                self.msgs.append(&self.f, &mut r).await.is_ok()
            }
            Job::MsgState { peer, text, state } => self.msg_state_async(peer, text, state).await.is_ok(),
            Job::Heard(j) => {
                let mut r = logs::heard_rec(&j, self.now_s(), self.utc.get());
                match self.heard.append(&self.f, &mut r).await {
                    Ok(seq) => {
                        self.extra.row_stored(&logs::heard_call(&r), seq); // APRS-4.5: where its details are now
                        true
                    }
                    Err(_) => false,
                }
            }
            Job::Read { log, seq } => {
                let ring = if log { &self.events } else { &self.msgs };
                ring.mark(&self.f, seq, 10, 0).await.is_ok()
            }
            Job::Waypoint(w) => self.wps.add(&self.f, &w).await.unwrap_or(false),
            Job::DeleteWaypoint(w) => self.wps.delete(&self.f, &w).await.unwrap_or(false),
            Job::Contact(c) => self.contacts.put(&self.f, &c).await.unwrap_or(false),
            Job::DeleteContact(c) => self.contacts.delete(&self.f, c.as_bytes()).await.unwrap_or(false),
            Job::AprsSave => self.aprs_save(None).await.is_ok(),
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
            set.sanitize();
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
        // INBOX-3.1: the mail badge counts unread messages to us; the log keeps its own unread count.
        let mail = (0..self.msgs.len()).filter(|&i| self.message(i).is_some_and(|m| m.inbound && m.state == MsgState::Unread)).count();
        app.set_inbox(mail as u16, (0..self.events.len()).filter(|&i| self.event(i).is_some_and(|e| e.unread)).count() as u16);
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
        self.now_ms.set(now);
        if self.aprs_changed.get().is_some_and(|t| now.wrapping_sub(t) >= SAVE_DELAY_MS) && self.ok() && self.submit(Job::AprsSave) {
            self.aprs_changed.set(None);
        }
        let c = state_crc(app);
        if c != self.seen.get() {
            self.seen.set(c);
            self.changed_at.set(now);
        }
        // HF-29: HF mode's VFOs and recent list are written on leaving it, never while tuning: nothing is
        // saved while its screen is open (power off still flushes).
        let in_hf = matches!(app.screen(), ferrite_app::screens::Screen::Hf(_));
        if !in_hf && self.seen.get() != self.saved.get() && now.wrapping_sub(self.changed_at.get()) >= SAVE_DELAY_MS {
            return self.save(app);
        }
        false
    }

    /// Saves now if anything changed since the last save (power off: PWR-17; empty battery: PWR-16).
    pub fn flush(&self, app: &App) -> bool {
        if self.aprs_changed.get().is_some() && self.ok() && self.submit(Job::AprsSave) {
            self.aprs_changed.set(None);
        }
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

    // The save buffer's borrow is held across the flash writes on purpose: the only other borrow is
    // `FlashStore::save`'s, which returns before borrowing while `saving` is set (until this ends).
    #[allow(clippy::await_holding_refcell_ref)]
    async fn save_async(&self, crc: u32, len: usize, seed: Option<Region>) -> bool {
        // Straight from the save buffer, no second copy: the UI leaves it alone while `saving` is set
        // ([`FlashStore::save`] returns at once), so the borrow held across the writes is the only one.
        let ok = {
            let buf = self.save_buf.borrow();
            settings::save(&self.f, &buf[..len]).await.is_ok()
        };
        if ok {
            self.saved.set(crc);
            if let Some(r) = seed.filter(|_| self.first_boot()) {
                // After setup: the region's range presets, unless some are there already (the sample radio).
                if self.presets() == 0 {
                    for (k, p) in default_presets(r).enumerate() {
                        let _ = self.put_rec(preset_addr(k as u8), &preset_encode(&p)).await;
                    }
                }
                if r.has_weather() && self.count() == 0 {
                    let _ = self.seed_wx().await; // WX-1, at setup in the US and Canada
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
        // Decision 227: channel writes queued behind this one for the same sector go in the same rewrite.
        let mut more = [None; QUEUE];
        let n = if CHANNELS.contains(&sector) { self.take_merges(sector, &mut more)? } else { 0 };
        scratch::rewrite(&self.f, sector, &|o, piece| {
            for (i, b) in piece.iter_mut().enumerate() {
                let a = o + i as u32;
                if (off..off + len).contains(&a) {
                    *b = new.map_or(0xFF, |n| n[(a - off) as usize]);
                }
                for &(num, rec) in more[..n].iter().flatten() {
                    let at = ch_addr(num) - sector;
                    if (at..at + CH_LEN as u32).contains(&a) {
                        *b = rec.map_or(0xFF, |r: [u8; CH_LEN]| r[(a - at) as usize]);
                    }
                }
            }
        })
        .await?;
        for &(num, rec) in more[..n].iter().flatten() {
            self.zmap[num as usize].set(rec.map_or(EMPTY, |r| zkey(r[18])));
        }
        Ok(())
    }

    /// Decision 227: takes the plain channel writes (a put that doesn't move a channel, or a delete) from the
    /// front of the queue that land in `sector`, as the records they would write, in queue order; it stops at the
    /// first job of another kind, so nothing is reordered past it. Writes to other sectors stay queued.
    fn take_merges(&self, sector: u32, out: &mut [Option<Merge>; QUEUE]) -> Result<usize> {
        let (head, queued) = (self.head.get() as usize, self.queued.get() as usize);
        let mut jobs = self.jobs.borrow_mut();
        let mut n = 0;
        let mut kept = [None; QUEUE];
        let mut k = 0;
        let mut i = 0;
        while i < queued {
            let job = jobs[(head + i) % QUEUE];
            let num = match job {
                Some(Job::Write(ChanWrite::Put { ch, from: None })) => ch.num,
                Some(Job::Write(ChanWrite::Delete(num))) => num,
                _ => break,
            };
            if num < MAX_CHANNELS && ch_addr(num) - ch_addr(num) % SECTOR == sector {
                // the record as it stands: a merged one before it, or the flash's
                let cur = out[..n].iter().rev().flatten().find(|m| m.0 == num).map_or_else(|| self.raw_ch(num), |m| Ok(m.1.unwrap_or([0xFF; CH_LEN])))?;
                let rec = match job {
                    Some(Job::Write(ChanWrite::Put { ch, .. })) => Some(ch_encode(&ch, &cur)),
                    _ => None,
                };
                out[n] = Some((num, rec));
                n += 1;
            } else {
                kept[k] = job;
                k += 1;
            }
            i += 1;
        }
        // The rest, after the run looked at, stays in order behind the jobs kept.
        while i < queued {
            kept[k] = jobs[(head + i) % QUEUE];
            k += 1;
            i += 1;
        }
        if n > 0 {
            for (j, slot) in jobs.iter_mut().enumerate() {
                *slot = if j < k { kept[j] } else { None };
            }
            self.head.set(0);
            self.queued.set(k as u8);
        }
        Ok(n)
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
            ChanWrite::PutPos { ch, pos } => self.put_channel_pos(ch, pos).await,
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

    /// A channel with its position set or cleared (CHAN-2; the terminal's `ch N pos=`).
    async fn put_channel_pos(&self, ch: Channel, pos: Option<Pos>) -> Result {
        if ch.num >= MAX_CHANNELS {
            return Err(NorError);
        }
        let rec = ch_encode(&ch, &self.raw_ch(ch.num)?);
        let rec = match pos {
            Some(p) => ch_with_position(&rec, p),
            None => ch_without_position(&rec),
        };
        self.write_ch(ch.num, &rec).await
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

    /// The reset (BOOT-14, [`Op::Wipe`]): every record area erased but the settings (the App saves its defaults)
    /// and never the SSB patch or calibration; then a first boot again, so setup's save seeds presets and the WX
    /// zone as on a new radio.
    async fn wipe(&self) -> Result {
        for z in &self.zmap {
            z.set(EMPTY);
        }
        self.zones.set(0);
        for r in [CHANNELS, ZONES, PRESETS, APRS, EVENT_LOG, TRIMS, WAYPOINTS, CONTACTS, HEARD] {
            for s in r.step_by(SECTOR as usize) {
                if !self.f.blank(s, SECTOR)? {
                    self.f.erase_blanked(s).await?;
                }
            }
        }
        self.events.forget();
        self.msgs.forget();
        self.heard.forget();
        self.wps.forget();
        self.contacts.0.forget();
        self.aprs.set(AprsCfg::DEFAULT);
        self.aprs_changed.set(None);
        self.draft.set(Draft::default());
        self.mounted.set(Mounted::FirstBoot);
        Ok(())
    }

    /// WX-1: in the US and Canada, the seven weather channels (162.400-162.550 MHz, receive only, wide FM) as a
    /// "Weather" zone on channels 991-997 (records 990-996), out of the way of your own numbering (decision 73);
    /// a number already in use is left alone.
    async fn seed_wx(&self) -> Result {
        let z = self.zones.get();
        self.new_zone(b"Weather", &ZoneCfg { shown: true, dwell: 255, ..ZoneCfg::default() }).await?;
        let cfg = ferrite_app::slot::SlotCfg { mode: 0, narrow: false, step: ferrite_app::slot::BANDS[4].step, ..ferrite_app::slot::DEFAULT_CFG };
        for k in 0..7u16 {
            let num = WX_FIRST + k;
            if self.zmap[num as usize].get() != EMPTY {
                continue;
            }
            let rec = {
                let mut c = ferrite_app::chan::channel(num, z, mhz(162) + 40_000 + k as Freq10 * 2_500, "", cfg);
                c.set_name(&[b'W', b'X', b'1' + k as u8]);
                c.flags |= ferrite_app::chan::FLAG_RX_ONLY;
                ch_encode(&c, &[0xFF; CH_LEN])
            };
            self.write_ch(num, &rec).await?;
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
            Op::Dtmf(i, _) | Op::DtmfName(i, _) => i < DTMF_MEMS,
            Op::Wipe | Op::ReadAll | Op::ClearLog | Op::MessagesReadAll | Op::ClearMessages | Op::ClearHeard => true,
            Op::Quick(i, _) => (i as usize) < ferrite_app::words::QUICK,
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
            Op::Wipe => self.wipe().await,
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
                self.new_preset(&RangePreset { name: Buf::from(b"New range"), start: mhz(144), end: mhz(148), step: 10, mode: 0, narrow: true, skips: 0, resume: 0 })
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
            Op::Dtmf(i, None) => {
                self.clear(dtmf_addr(i), DTMF_LEN as u32).await?;
                self.clear(dtmf_name_addr(i), DTMF_NAME_LEN as u32).await
            }
            Op::DtmfName(i, n) if n.is_empty() => self.clear(dtmf_name_addr(i), DTMF_NAME_LEN as u32).await,
            Op::DtmfName(i, n) => self.put_rec(dtmf_name_addr(i), &dtmf_name_encode(n.as_bytes())).await,
            Op::ReadAll => {
                for i in 0..self.events.len() {
                    if let Some(seq) = self.events.seq(i) {
                        self.events.mark(&self.f, seq, 10, 0).await?;
                    }
                }
                Ok(())
            }
            Op::ClearLog => self.events.clear(&self.f).await,
            Op::ClearHeard => self.heard.clear(&self.f).await, // APRS-4: the stored rows too
            // APRS-46, INBOX-7.1: each unread message to us read, and its log entry (among the last 64) with it.
            Op::MessagesReadAll => {
                for i in 0..self.msgs.len() {
                    let Some(m) = self.message(i).filter(|m| m.inbound && m.state == MsgState::Unread) else { continue };
                    if let Some(seq) = self.msgs.seq(i) {
                        self.msgs.mark(&self.f, seq, 10, 0).await?;
                    }
                    let entry = (0..self.events.len().min(64)).find(|&j| {
                        self.event(j).is_some_and(|e| e.unread && e.src == Src::Aprs && e.name.as_bytes() == m.peer.as_bytes() && e.text.as_bytes() == m.text.as_bytes())
                    });
                    if let Some(seq) = entry.and_then(|j| self.events.seq(j)) {
                        self.events.mark(&self.f, seq, 10, 0).await?;
                    }
                }
                Ok(())
            }
            Op::ClearMessages => self.msgs.clear(&self.f).await,
            Op::Quick(i, t) => self.aprs_save(Some((i, t))).await,
            _ => Ok(()),
        }
    }
}

impl<N: Nor, D: Data> FlashStore<N, D> {
    // ---- the event log, messages, waypoints and APRS settings ([`crate::logs`]) ----

    /// The store's clock, in seconds ([`crate::logs`]).
    pub fn now_s(&self) -> u32 {
        self.time0.get().wrapping_add(self.now_ms.get() / 1000)
    }

    /// The clock is known (GPS, or set by hand): `secs` UTC now. Records stored from now on are stamped in UTC.
    pub fn set_time(&self, secs: u32) {
        self.time0.set(secs.wrapping_sub(self.now_ms.get() / 1000));
        self.utc.set(true);
        self.bump();
    }

    /// What records' ages are reckoned from (INBOX-6.1).
    fn clock(&self) -> logs::Clock {
        logs::Clock { now: self.now_s(), utc: self.utc.get(), boot_now: self.boot0.get().wrapping_add(self.now_ms.get() / 1000) }
    }

    /// An event for the log (INBOX-6): a decoder's, an alert, a message. On the radio it is queued and stamped
    /// when written; at once (the simulator, the tests) its `ago_s` dates it back from now (the sample's).
    pub fn log(&self, e: &Event) -> bool {
        let j = logs::event_job(e);
        if !self.ok() {
            return false;
        }
        if self.deferred.get() {
            return self.submit(Job::Log(j));
        }
        let mut r = logs::event_rec(&j, self.now_s().saturating_sub(e.ago_s), self.utc.get());
        let ok = block(self.events.append(&self.f, &mut r)).is_ok();
        self.bump();
        ok
    }

    /// APRS-4: a heard station's row, queued for the Storage task (which stamps it). When to write one is the
    /// APRS task's (`AprsLive`'s row rule).
    pub fn heard_row(&self, s: &Station) -> bool {
        self.ok() && self.submit(Job::Heard(logs::heard_job(s)))
    }

    /// APRS-4: the stored heard list at power-on: the newest row of each station, newest first, at most `max`
    /// stations, each with its age (INBOX-6.1: [`AGO_UNKNOWN`] when not real).
    pub fn heard_rows(&self, max: usize, f: &mut dyn FnMut(Station, u32)) {
        let mut seen = [0u32; 64];
        let mut n = 0;
        let c = self.clock();
        for i in 0..self.heard.len() {
            if n >= max.min(seen.len()) {
                break;
            }
            let (Some(b), Some(seq)) = (self.heard.read(&self.f, i), self.heard.seq(i)) else { continue };
            let s = logs::heard_decode(&b, self.heard.ago(&b, c));
            let key = s.call.as_bytes().iter().fold(0x811C_9DC5u32, |h, &x| (h ^ x as u32).wrapping_mul(0x0100_0193));
            if s.call.is_empty() || seen[..n].contains(&key) {
                continue;
            }
            seen[n] = key;
            n += 1;
            f(s, seq);
        }
    }

    /// The heard rows stored (the build report, tests).
    pub fn heard_stored(&self) -> u16 {
        self.heard.len()
    }

    /// An APRS message: for us (`inbound`, unread) or ours (pending, the APRS task tries it); `ago_s` dates it
    /// back from now (the simulator's sample).
    pub fn message_in(&self, inbound: bool, peer: &[u8], text: &[u8], ago_s: u32) -> bool {
        self.ok() && self.submit(Job::Msg { inbound, peer: Buf::from(peer), text: Buf::from(text), id: Buf::new(), ago_s })
    }

    /// A message to us with its sender's message number `id` (empty: none), kept for APRS-32's check after a power
    /// cycle ([`FlashStore::message_heard`]).
    pub fn message_to_us(&self, peer: &[u8], text: &[u8], id: &[u8]) -> bool {
        self.ok() && self.submit(Job::Msg { inbound: true, peer: Buf::from(peer), text: Buf::from(text), id: Buf::from(id), ago_s: 0 })
    }

    /// APRS-32: the message to us from `peer` numbered `id` with `text` is among the newest [`HEARD_CHECK`] stored
    /// (a sender's retry heard again after a power cycle, which the APRS task's RAM table has forgotten). False
    /// without a number: those are told apart in RAM only.
    pub fn message_heard(&self, peer: &[u8], id: &[u8], text: &[u8]) -> bool {
        let (peer, id, text) = (Buf::from(peer), Buf::from(id), Buf::from(text));
        !id.is_empty() && (0..self.msgs.len().min(HEARD_CHECK)).any(|i| self.msgs.read(&self.f, i).is_some_and(|b| logs::msg_same(&b, &peer, &id, &text)))
    }

    /// A waypoint (the simulator's sample; the link's `WAYPOINT` later, GPS-13).
    pub fn add_waypoint(&self, w: &Waypoint) -> bool {
        self.ok() && self.wps.len() < logs::WP_MAX && self.submit(Job::Waypoint(*w))
    }

    /// Our message to `peer` with `text` moved on (the APRS task: tried again, acked, failed).
    pub fn message_state(&self, peer: &[u8], text: &[u8], state: MsgState) -> bool {
        self.ok() && self.submit(Job::MsgState { peer: Buf::from(peer), text: Buf::from(text), state })
    }

    async fn msg_state_async(&self, peer: Buf<10>, text: Buf<67>, state: MsgState) -> Result {
        let Some((at, v)) = logs::msg_state_byte(state) else { return Ok(()) };
        for i in 0..self.msgs.len() {
            if let (Some(seq), Some(b)) = (self.msgs.seq(i), self.msgs.read(&self.f, i)) {
                if logs::msg_open(&b, &peer, &text) {
                    return self.msgs.mark(&self.f, seq, at, v).await;
                }
            }
        }
        Ok(())
    }

    /// Writes the APRS record: the settings from RAM, the quick messages as stored, quick message `q` changed.
    async fn aprs_save(&self, q: Option<(u8, Option<Buf<67>>)>) -> Result {
        let mut p = [0xFFu8; APRS_PAYLOAD];
        let c = self.aprs.get();
        p[..2].copy_from_slice(&c.f5k.to_le_bytes());
        (p[2], p[3]) = (c.beacon, c.power);
        (p[APRS_MORE], p[APRS_MORE + 1], p[APRS_MORE + 2], p[APRS_MORE + 3]) = (c.path, c.digi, c.after_voice as u8 | (c.telemetry as u8) << 1, c.ssid);
        p[APRS_PATH..APRS_FX25].copy_from_slice(&c.custom);
        p[APRS_FX25] = c.fx25;
        p[APRS_SOUND] = c.sound;
        (p[APRS_SYMBOL], p[APRS_SYMBOL + 1], p[APRS_SYMBOL + 2]) = (c.symbol, c.txdelay, c.tx_level);
        p[APRS_COMMENT..APRS_AMBIG].copy_from_slice(&c.comment);
        p[APRS_AMBIG] = c.ambiguity;
        p[APRS_MIC_E] = c.mic_e;
        for i in 0..ferrite_app::words::QUICK {
            let t = match q {
                Some((k, t)) if k as usize == i => t,
                _ => self.quick(i),
            };
            if let Some(t) = t {
                let o = 4 + i * QUICK_LEN;
                p[o] = t.len() as u8;
                p[o + 1..o + 1 + t.len()].copy_from_slice(t.as_bytes());
            }
        }
        settings::save_to(&self.f, APRS_COPIES, &p).await
    }

    /// The requests this store answers from its own records (None: the others, below).
    fn request_kept(&self, op: Op) -> Option<bool> {
        if !self.ok() {
            return None;
        }
        Some(match op {
            Op::Read(i) => match self.events.seq(i) {
                Some(seq) => self.submit(Job::Read { log: true, seq }),
                None => false,
            },
            // INBOX-7.1: an unread message from someone also marks its log entry read (the newest unread one with
            // its sender and text, among the last 64).
            Op::MessageRead(i) => match (self.msgs.seq(i), self.message(i)) {
                (Some(seq), Some(m)) => {
                    if !self.submit(Job::Read { log: false, seq }) {
                        return Some(false);
                    }
                    let entry = (m.inbound && m.state == MsgState::Unread)
                        .then(|| {
                            (0..self.events.len().min(64)).find(|&j| {
                                self.event(j).is_some_and(|e| e.unread && e.src == Src::Aprs && e.name.as_bytes() == m.peer.as_bytes() && e.text.as_bytes() == m.text.as_bytes())
                            })
                        })
                        .flatten()
                        .and_then(|j| self.events.seq(j));
                    if let Some(seq) = entry {
                        self.submit(Job::Read { log: true, seq });
                    }
                    true
                }
                _ => false,
            },
            Op::Draft(d) => {
                self.draft.set(d);
                true
            }
            // APRS-13.1: ours being tried stops: the APRS task drops it, the record says cancelled.
            Op::StopMessage(i) => match self.message(i) {
                Some(m) if !m.inbound && matches!(m.state, MsgState::Pending | MsgState::Retrying(_)) => {
                    self.extra.request(Op::Unqueue(m.peer, m.text));
                    self.message_state(m.peer.as_bytes(), m.text.as_bytes(), MsgState::Cancelled)
                }
                _ => false,
            },
            // APRS-13.2: ours that failed or was cancelled goes again as a new message (the old one stays as it ended).
            Op::ResendMessage(i) => match self.message(i) {
                Some(m) if !m.inbound && matches!(m.state, MsgState::Failed | MsgState::Cancelled) => {
                    let ok = self.extra.callsign().is_some_and(|c| ferrite_app::data::ax25_call_ok(c.as_bytes())) && self.extra.send_room();
                    ok && self.message_in(false, m.peer.as_bytes(), m.text.as_bytes(), 0) && self.extra.request(Op::Queue(m.peer, m.text))
                }
                _ => false,
            },
            Op::Send => {
                let d = self.draft.get();
                // APRS-8: a callsign to send as, someone to send to, something to say; APRS-13.3: room to try it
                // (checked before it is saved, so none is left pending for ever).
                let ok = self.extra.callsign().is_some_and(|c| ferrite_app::data::ax25_call_ok(c.as_bytes())) && !d.to.is_empty() && !d.text.is_empty() && self.extra.send_room(); // SETUP-13
                if ok && self.message_in(false, d.to.as_bytes(), d.text.as_bytes(), 0) {
                    // The APRS task tries it (the simulator's sends it from its own copy of the draft).
                    self.extra.request(Op::Draft(d));
                    self.extra.request(Op::Send);
                    self.draft.set(Draft::default());
                    true
                } else {
                    false
                }
            }
            Op::AprsCfg(c) => {
                if c != self.aprs.get() {
                    self.aprs.set(c);
                    self.aprs_changed.set(Some(self.now_ms.get()));
                }
                self.extra.request(op);
                true
            }
            // GPS-10: at a fix, never at the fixed position standing in for one (GPS-3).
            Op::MarkHere => match self.extra.fix().filter(|f| f.age_s != ferrite_app::gpsdata::FIXED_AGE) {
                Some(f) if self.wps.len() < logs::WP_MAX => {
                    let mut name = Buf::<16>::from(b"Marked ");
                    let n = self.wps.len() + 1;
                    for d in [n / 100, n / 10 % 10, n % 10].into_iter().skip_while(|&d| d == 0) {
                        name.push(&[b'0' + d as u8]);
                    }
                    self.submit(Job::Waypoint(Waypoint { name, pos: f.pos, src: WpSrc::Marked }))
                }
                _ => false,
            },
            Op::SaveStation(i) => match self.extra.station(i) {
                Some(Station { call, pos: Some(pos), .. }) if self.wps.len() < logs::WP_MAX => {
                    self.submit(Job::Waypoint(Waypoint { name: Buf::from(call.as_bytes()), pos, src: WpSrc::Aprs }))
                }
                _ => false,
            },
            Op::AddWaypoint(pos, name) => self.wps.len() < logs::WP_MAX && self.submit(Job::Waypoint(Waypoint { name, pos, src: WpSrc::Grid })),
            Op::DeleteWaypoint(i) => match self.wps.get(&self.f, i) {
                Some(w) => self.submit(Job::DeleteWaypoint(w)),
                None => false,
            },
            // APRS-37: a new callsign needs room; a saved one is renamed.
            Op::SaveContact(c) => (!self.contacts.0.full() || self.contacts.find(&self.f, c.call.as_bytes()).is_ok()) && self.submit(Job::Contact(c)),
            Op::DeleteContact(c) => self.contacts.find(&self.f, c.as_bytes()).is_ok() && self.submit(Job::DeleteContact(c)),
            _ => return None,
        })
    }
}

/// The zone map's key for a channel's zone byte (anything past the last zone counts as no zone).
fn zkey(z: u8) -> u8 {
    if z < MAX_ZONES { z } else { NO_ZONE }
}

/// The range presets a first boot gets after setup, by region (`0 1 4`'s default: 2 m, 70 cm, airband, marine,
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
        resume: 0,
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
    fn queue_write(&self, w: ChanWrite) -> bool {
        self.apply(w)
    }
    fn recent(&self, r: u16) -> Option<u16> {
        self.recent.get(r)
    }

    fn used(&self, num: u16) {
        self.recent.touch(num);
    }

    fn tuned(&self, num: u16) {
        self.recent.tuned(num);
    }

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

    fn zone_of(&self, n: u16) -> Option<u8> {
        self.zmap.get(n as usize).map(Cell::get).filter(|&z| z != EMPTY)
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

    fn located(&self, num: u16) -> Option<(Channel, Pos)> {
        if self.zmap.get(num as usize)?.get() == EMPTY {
            return None;
        }
        let r = self.read_ch(num);
        Some((ch_decode(num, &r)?, ch_position(&r)?))
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
        self.events.len()
    }
    fn event(&self, i: u16) -> Option<Event> {
        self.events.read(&self.f, i).map(|b| logs::event_decode(&b, self.events.ago(&b, self.clock())))
    }
    fn event_seq(&self, i: u16) -> Option<u32> {
        self.events.seq(i)
    }
    fn stations(&self) -> u16 {
        self.extra.stations()
    }
    /// APRS-4.5: the heard list keeps its stations' details in RAM only for the most recent; the rest come from
    /// each station's last stored row (a 128-byte read).
    fn station(&self, i: u16) -> Option<Station> {
        let mut s = self.extra.station(i)?;
        let Some((seq, missing)) = self.extra.row_of(i) else { return Some(s) };
        let Some(b) = self.heard.by_seq(&self.f, seq) else { return Some(s) };
        let r = logs::heard_decode(&b, 0);
        if r.call != s.call {
            return Some(s);
        }
        use ferrite_app::aprs_live::{DET_COMMENT, DET_COURSE, DET_FX25, DET_VIA};
        if missing & DET_VIA != 0 {
            s.via = r.via;
        }
        if missing & DET_COURSE != 0 {
            (s.course, s.alt_m, s.device) = (r.course, r.alt_m, r.device);
        }
        if missing & DET_COMMENT != 0 {
            s.comment = r.comment;
        }
        if missing & DET_FX25 != 0 {
            s.fx25 = r.fx25;
        }
        Some(s)
    }
    fn callsign(&self) -> Option<Buf<10>> {
        self.extra.callsign()
    }
    fn aprs_cfg(&self) -> AprsCfg {
        self.aprs.get()
    }
    fn beacon(&self) -> Beacon {
        self.extra.beacon()
    }
    fn beacons_sent(&self) -> u16 {
        self.extra.beacons_sent()
    }
    fn beacon_sent(&self, i: u16) -> Option<ferrite_app::data::SentBeacon> {
        self.extra.beacon_sent(i)
    }
    fn messages(&self) -> u16 {
        self.msgs.len()
    }
    fn message(&self, i: u16) -> Option<Message> {
        self.msgs.read(&self.f, i).map(|b| logs::msg_decode(&b, self.msgs.ago(&b, self.clock())))
    }
    fn draft(&self) -> Draft {
        self.draft.get()
    }
    fn send_room(&self) -> bool {
        self.extra.send_room()
    }
    fn quick(&self, i: usize) -> Option<Buf<67>> {
        if i >= ferrite_app::words::QUICK || !self.ok() {
            return None;
        }
        let c = settings::latest_of(&self.f, APRS_COPIES, APRS_PAYLOAD).ok()??;
        let mut b = [0u8; QUICK_LEN];
        self.f.read(c.at + 4 + (i * QUICK_LEN) as u32, &mut b).ok()?;
        (b[0] != 0xFF).then(|| Buf::from(&b[1..1 + (b[0] as usize).min(67)]))
    }
    fn contacts(&self) -> u16 {
        self.contacts.0.len()
    }
    fn contact(&self, i: u16) -> Option<Contact> {
        self.contacts.get(&self.f, i)
    }
    fn contact_of(&self, call: &[u8]) -> Option<Contact> {
        self.contacts.get(&self.f, self.contacts.find(&self.f, call).ok()?)
    }
    fn waypoints(&self) -> u16 {
        self.wps.len()
    }
    fn waypoint(&self, i: u16) -> Option<Waypoint> {
        self.wps.get(&self.f, i)
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
    fn dtmf_name(&self, i: u8) -> Buf<10> {
        let mut b = [0xFFu8; DTMF_NAME_LEN];
        if !self.ok() || i >= DTMF_MEMS || self.f.read(dtmf_name_addr(i), &mut b).is_err() {
            return Buf::new();
        }
        dtmf_name_decode(&b).unwrap_or_default()
    }

    /// LINK-41.1: room for a link's channel records only while everything queued or running is a channel write (a
    /// zone being created, say, must be stored before a channel that names it is checked).
    fn write_room(&self) -> usize {
        let (head, n) = (self.head.get() as usize, self.queued.get() as usize);
        let jobs = self.jobs.borrow();
        let others = (0..n).any(|i| !matches!(jobs[(head + i) % QUEUE], Some(Job::Write(_))));
        if !self.ok() || others || self.running.get() && !self.running_write.get() { 0 } else { QUEUE - n }
    }

    fn busy(&self) -> bool {
        !self.idle()
    }

    fn request(&self, op: Op) -> bool {
        if matches!(op, Op::Wipe | Op::ClearHeard | Op::ClearMessages) {
            self.extra.request(op); // its records go too (the heard list in RAM; messages tried no more, APRS-46)
        }
        if let Some(ok) = self.request_kept(op) {
            if ok {
                self.bump();
            }
            return ok;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimFlash;

    type Store = FlashStore<SimFlash, NoData>;

    /// Every APRS setting set to something other than its default.
    fn full_cfg() -> AprsCfg {
        let mut comment = [0u8; ferrite_app::aprs::COMMENT_LEN];
        comment[..7].copy_from_slice(b"On foot");
        let mut custom = [0u8; ferrite_app::aprs::CUSTOM_LEN];
        custom[..7].copy_from_slice(b"WIDE2-2");
        AprsCfg { f5k: 28_878, beacon: 3, power: 1, path: ferrite_app::aprs::PATH_CUSTOM, digi: 0x23, after_voice: true, custom, ssid: 9, fx25: 32, sound: 3, symbol: 4, txdelay: 45, tx_level: 2, comment, ambiguity: 3, mic_e: 5 }
    }

    /// The APRS record's payload as it is stored for `cfg`.
    fn aprs_payload(cfg: AprsCfg) -> Vec<u8> {
        let s = Store::mount(SimFlash::new(), NoData);
        assert!(s.request(Op::AprsCfg(cfg)));
        assert!(block(s.run(Job::AprsSave)));
        let c = settings::latest_of(&s.f, APRS_COPIES, APRS_PAYLOAD).unwrap().unwrap();
        let mut p = vec![0u8; c.len as usize];
        s.f.raw_read(c.at, &mut p).unwrap();
        p
    }

    /// The APRS settings a mount reads from a record holding `p`.
    fn mounted_with(p: &[u8]) -> AprsCfg {
        let s = Store::mount(SimFlash::new(), NoData);
        block(settings::save_to(&s.f, APRS_COPIES, p)).unwrap();
        Store::mount(s.into_parts().0, NoData).aprs_cfg()
    }

    /// A record from before each later setting (shorter) reads those as their defaults and the rest as stored:
    /// the path, digipeater, beacon after voice and SSID (APRS-26, APRS-30), the custom path, FX.25 (APRS-45), the
    /// sound, the symbol, TX delay and level (APRS-47), the comment and ambiguity (APRS-51), Mic-E (APRS-52).
    #[test]
    fn an_older_aprs_record_reads_later_settings_as_defaults() {
        let full = full_cfg();
        let p = aprs_payload(full);
        assert_eq!(p.len(), APRS_PAYLOAD);
        assert!(mounted_with(&p) == full, "a whole record reads back");
        let d = AprsCfg::DEFAULT;
        for len in [APRS_MORE, APRS_PATH, APRS_FX25, APRS_SOUND, APRS_SYMBOL, APRS_COMMENT, APRS_MIC_E] {
            let c = mounted_with(&p[..len]);
            let pick = |kept: bool, f: &dyn Fn(&AprsCfg) -> u32| if kept { f(&full) } else { f(&d) };
            assert_eq!((c.f5k, c.beacon, c.power), (full.f5k, full.beacon, full.power), "{len}: the first settings");
            assert_eq!(c.ssid as u32, pick(len >= APRS_PATH, &|x| x.ssid as u32), "{len}: SSID");
            assert_eq!(c.digi as u32, pick(len >= APRS_PATH, &|x| x.digi as u32), "{len}: digipeater");
            assert_eq!(c.after_voice as u32, pick(len >= APRS_PATH, &|x| x.after_voice as u32), "{len}: after voice");
            assert_eq!(c.custom == full.custom, len >= APRS_FX25, "{len}: custom path");
            // custom (3) is chosen only once the custom path itself is stored
            assert_eq!(c.path as u32, pick(len >= APRS_FX25, &|x| x.path as u32), "{len}: path");
            assert_eq!(c.fx25 as u32, pick(len > APRS_FX25, &|x| x.fx25 as u32), "{len}: FX.25");
            assert_eq!(c.sound as u32, pick(len > APRS_SOUND, &|x| x.sound as u32), "{len}: sound");
            let sym = len >= APRS_COMMENT;
            assert_eq!((c.symbol as u32, c.txdelay as u32, c.tx_level as u32), (pick(sym, &|x| x.symbol as u32), pick(sym, &|x| x.txdelay as u32), pick(sym, &|x| x.tx_level as u32)), "{len}: symbol");
            assert_eq!((c.comment == full.comment, c.ambiguity as u32), (len >= APRS_MIC_E, pick(len >= APRS_MIC_E, &|x| x.ambiguity as u32)), "{len}: comment");
            assert_eq!(c.mic_e, d.mic_e, "{len}: Mic-E");
        }
    }

    /// A record whose later bytes are out of range (a damaged record, or a value a newer build added) reads each of
    /// them as its default; the power is held to High, the digipeater to its own bits, the custom path kept only if
    /// it still checks (APRS-26), the SSID 0-15 (APRS-30), the ambiguity 0-4 (APRS-51), and a comment only when it
    /// is printable.
    #[test]
    fn a_damaged_aprs_record_reads_defaults() {
        let mut p = aprs_payload(full_cfg());
        p[3] = 9; // power
        p[APRS_MORE..APRS_MORE + 4].copy_from_slice(&[9, 0xFF, 2, 16]); // path, digi, after voice, SSID
        p[APRS_PATH..APRS_PATH + 3].copy_from_slice(b"!!!");
        p[APRS_FX25] = 17;
        p[APRS_SOUND] = 200;
        p[APRS_SYMBOL..APRS_SYMBOL + 3].copy_from_slice(&[200, ferrite_app::aprs::TXDELAY_MAX + 1, 3]);
        p[APRS_COMMENT] = 0x01;
        p[APRS_AMBIG] = 5;
        p[APRS_MIC_E] = 200;
        let c = mounted_with(&p);
        let d = AprsCfg::DEFAULT;
        assert_eq!((c.f5k, c.beacon, c.power), (28_878, 3, 2));
        assert_eq!((c.path, c.digi, c.after_voice, c.ssid), (d.path, 0x73, false, ferrite_app::aprs::SSID_DEFAULT));
        assert!(c.custom == d.custom && c.comment == d.comment);
        assert_eq!((c.fx25, c.sound, c.symbol, c.txdelay, c.tx_level, c.ambiguity, c.mic_e), (d.fx25, d.sound, d.symbol, d.txdelay, d.tx_level, 0, d.mic_e));
    }

    /// Zone names count from 1: "Zone 1", "Zone 10", "Zone 100", "Zone 254".
    #[test]
    fn new_zone_names() {
        let mut b = Buf::<16>::new();
        for (z, want) in [(0u8, &b"Zone 1"[..]), (9, b"Zone 10"), (99, b"Zone 100"), (253, b"Zone 254")] {
            zone_name(&mut b, z);
            assert_eq!(b.as_bytes(), want);
        }
    }

    /// Settings that outgrow the save buffer are refused whole rather than written in part.
    #[test]
    fn a_payload_too_big_is_refused() {
        let mut buf = [0u8; 4];
        let mut s = SliceSink { buf: &mut buf, len: 0, over: false };
        s.put(b"abc");
        s.put(b"de");
        assert!(s.over && s.len == 3);
        assert_eq!(&buf[..3], b"abc");
    }
}
