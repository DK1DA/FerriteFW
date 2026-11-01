//! Satellite mode's records (manual chapter 28a, SAT-2; decision 340): up to 32 satellites in the 8 KB region at
//! 0x006000 (layout 3, `ferrite_core::layout::SATELLITES`), the satellites' info (the declination and where the list
//! came from, SAT-18, SAT-21) and each satellite's choices (trim, FD/HD, transponder, uplink power: SAT-8, SAT-11,
//! SAT-22) kept by its catalogue number, so a list Console writes again keeps them.
//!
//! Two sectors, each:
//! - 16 records of [`REC`] bytes from its start (sector 0: places 0-15, sector 1: 16-31): the link's 152-byte
//!   record (`ferrite_core::sat::wire`), 0xFF to byte 190, then a CRC-16 of the 190 before; erased or a bad CRC is
//!   an empty place;
//! - from byte 3072 to the scratch footer's 32 bytes, a log: sector 0 holds the info (31 slots of 32 bytes: the
//!   16-byte info, 0xFF, a CRC-16 at 30), sector 1 the choices (62 slots of 16 bytes: catalogue number, the 6 bytes
//!   of `wire::prefs_encode`, 0xFF, a CRC-16 at 14). A change programs the next erased slot; the newest valid one
//!   wins. A full log is rewritten once with only what is current (the info; each held satellite's newest choices).
//!
//! Wear: a satellite into an erased place is programmed only; a changed one rewrites its sector through the scratch
//! ring (crash-safe as every record, `scratch`). Console writes a whole list as a clear (one rewrite of each sector
//! holding records, `Op::ClearSats`), then the records into erased places and the info into its log: two sector
//! rewrites a sync (SAT-21: at most one a day). The choices are written 2 s after the last change (a turning knob
//! doesn't wear the flash) and at power off, a 16-byte program each, a sector rewrite every 62.

use crate::scratch::{self, FOOTER_AT};
use crate::{crc16, u32_at, Nor, Result, SECTOR};
use core::cell::{Cell, RefCell};
use ferrite_app::data::{Data, Op};
use ferrite_core::layout::SATELLITES;
use ferrite_core::sat::data::Prefs;
use ferrite_core::sat::wire::{self, Info, INFO_LEN, REC_LEN};
use ferrite_core::sat::{Sat, SatSource};

/// A record's length in flash (the link's record, padding, CRC-16).
pub const REC: usize = 192;
/// The most satellites (SAT-2), 16 a sector.
pub const PLACES: u8 = 32;
const PER: u8 = 16;
/// Where each sector's log starts.
const LOG_AT: u32 = PER as u32 * REC as u32;
const INFO_SLOT: u32 = 32;
const INFO_SLOTS: u32 = (FOOTER_AT - LOG_AT) / INFO_SLOT;
const PREF_SLOT: u32 = 16;
const PREF_SLOTS: u32 = (FOOTER_AT - LOG_AT) / PREF_SLOT;
const _: () = assert!(SATELLITES.end - SATELLITES.start == 2 * SECTOR && LOG_AT + INFO_SLOTS * INFO_SLOT <= FOOTER_AT);

/// Place `k`'s address.
pub fn rec_addr(k: u8) -> u32 {
    SATELLITES.start + (k / PER) as u32 * SECTOR + (k % PER) as u32 * REC as u32
}

fn info_addr(i: u32) -> u32 {
    SATELLITES.start + LOG_AT + i * INFO_SLOT
}

fn pref_addr(i: u32) -> u32 {
    SATELLITES.start + SECTOR + LOG_AT + i * PREF_SLOT
}

/// A satellite as its flash record.
pub fn rec_encode(s: &Sat) -> [u8; REC] {
    let mut b = [0xFFu8; REC];
    b[..REC_LEN].copy_from_slice(&wire::encode(s));
    let c = crc16(&b[..REC - 2]);
    b[REC - 2..].copy_from_slice(&c.to_le_bytes());
    b
}

/// A flash record read back (None: empty or torn).
pub fn rec_decode(b: &[u8; REC]) -> Option<Sat> {
    if crate::u16_at(b, REC - 2) != crc16(&b[..REC - 2]) {
        return None;
    }
    wire::decode(&b[..REC_LEN]).ok()
}

fn info_slot(i: &Info) -> [u8; INFO_SLOT as usize] {
    let mut b = [0xFFu8; INFO_SLOT as usize];
    b[..INFO_LEN].copy_from_slice(&i.encode());
    let c = crc16(&b[..30]);
    b[30..].copy_from_slice(&c.to_le_bytes());
    b
}

fn pref_slot(id: u32, p: &Prefs) -> [u8; PREF_SLOT as usize] {
    let mut b = [0xFFu8; PREF_SLOT as usize];
    b[..4].copy_from_slice(&id.to_le_bytes());
    b[4..10].copy_from_slice(&wire::prefs_encode(p));
    let c = crc16(&b[..14]);
    b[14..].copy_from_slice(&c.to_le_bytes());
    b
}

/// The store's satellite state in RAM (about 260 bytes: the record staged for the link is most of it).
pub(crate) struct SatState {
    /// The places holding a satellite (a bit each), built at mount and kept by every write.
    pub held: Cell<u32>,
    pub ver: Cell<u32>,
    pub info: Cell<Option<Info>>,
    /// The record waiting for `Op::PutSat`.
    pub stage: RefCell<[u8; REC]>,
    pub staged: Cell<bool>,
    /// One satellite's choices, as last read or set (the pass page works one satellite at a time).
    pub prefs: Cell<Option<(u32, Option<Prefs>)>>,
    /// Choices set and not yet written: catalogue number, choices, when (ms).
    pub pending: Cell<Option<(u32, Prefs, u32)>>,
}

impl SatState {
    pub const fn new() -> Self {
        SatState { held: Cell::new(0), ver: Cell::new(0), info: Cell::new(None), stage: RefCell::new([0xFF; REC]), staged: Cell::new(false), prefs: Cell::new(None), pending: Cell::new(None) }
    }
}

impl<N: Nor, D: Data> crate::FlashStore<N, D> {
    fn sat_read(&self, k: u8) -> Option<Sat> {
        if k >= PLACES || !self.ok() {
            return None;
        }
        let mut b = [0u8; REC];
        self.f.read(rec_addr(k), &mut b).ok()?;
        rec_decode(&b)
    }

    /// At mount: which places hold a satellite, and the newest info.
    pub(crate) fn sat_mount(&self) -> Result {
        let mut held = 0;
        let mut b = [0u8; REC];
        for k in 0..PLACES {
            self.f.raw_read(rec_addr(k), &mut b)?;
            if rec_decode(&b).is_some() {
                held |= 1 << k;
            }
        }
        self.sats.held.set(held);
        self.sats.info.set(self.info_newest()?.map(|(_, i)| i));
        self.sats.ver.set(self.sats.ver.get().wrapping_add(1));
        Ok(())
    }

    /// The newest valid info and the first erased slot after it.
    fn info_newest(&self) -> Result<Option<(u32, Info)>> {
        let mut found = None;
        let mut b = [0u8; INFO_SLOT as usize];
        for i in 0..INFO_SLOTS {
            self.f.read(info_addr(i), &mut b)?;
            if b.iter().all(|&x| x == 0xFF) {
                break;
            }
            if crate::u16_at(&b, 30) == crc16(&b[..30]) {
                if let Ok(v) = Info::decode(&b[..INFO_LEN]) {
                    found = Some((i, v));
                }
            }
        }
        Ok(found)
    }

    /// The first erased slot of a log (`n` slots of `size` at `addr(i)`), None when full.
    fn log_free(&self, n: u32, size: u32, addr: fn(u32) -> u32) -> Result<Option<u32>> {
        for i in 0..n {
            if self.f.blank(addr(i), size)? {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// The newest choices kept for satellite `id` (from flash).
    fn prefs_read(&self, id: u32) -> Option<Prefs> {
        let mut found = None;
        let mut b = [0u8; PREF_SLOT as usize];
        for i in 0..PREF_SLOTS {
            self.f.read(pref_addr(i), &mut b).ok()?;
            if b.iter().all(|&x| x == 0xFF) {
                break;
            }
            if u32_at(&b, 0) == id && crate::u16_at(&b, 14) == crc16(&b[..14]) {
                let mut p = [0u8; 6];
                p.copy_from_slice(&b[4..10]);
                found = Some(wire::prefs_decode(&p));
            }
        }
        found
    }

    fn sat_changed(&self) {
        self.sats.ver.set(self.sats.ver.get().wrapping_add(1));
    }

    /// The Storage task's side of the satellite requests.
    pub(crate) async fn sat_perform(&self, op: Op) -> Result {
        match op {
            Op::PutSat(k) => {
                let rec = *self.sats.stage.borrow();
                self.sats.staged.set(false);
                let a = rec_addr(k);
                let mut old = [0u8; REC];
                self.f.read(a, &mut old)?;
                if old != rec {
                    if old.iter().all(|&x| x == 0xFF) {
                        self.f.program(a, &rec).await?;
                    } else {
                        self.splice(a, Some(&rec), REC as u32).await?;
                    }
                }
                self.sats.held.set(self.sats.held.get() | 1 << k);
            }
            Op::DeleteSat(k) => {
                self.clear(rec_addr(k), REC as u32).await?;
                self.sats.held.set(self.sats.held.get() & !(1 << k));
            }
            Op::ClearSats => {
                for s in [SATELLITES.start, SATELLITES.start + SECTOR] {
                    if !self.f.blank(s, LOG_AT)? {
                        scratch::rewrite(&self.f, s, &|o, piece| {
                            for (i, b) in piece.iter_mut().enumerate() {
                                if o + (i as u32) < LOG_AT {
                                    *b = 0xFF;
                                }
                            }
                        })
                        .await?;
                    }
                }
                self.sats.held.set(0);
            }
            Op::SatInfo(i) => {
                let i = i.unwrap_or(Info { decl: None, stamp: 0, written: 0 });
                if self.info_newest()?.map(|x| x.1) != Some(i) {
                    let slot = info_slot(&i);
                    match self.log_free(INFO_SLOTS, INFO_SLOT, info_addr)? {
                        Some(n) => self.f.program(info_addr(n), &slot).await?,
                        // full: the log rewritten with only this one
                        None => {
                            scratch::rewrite(&self.f, SATELLITES.start, &|o, piece| {
                                for (j, b) in piece.iter_mut().enumerate() {
                                    let a = o + j as u32;
                                    if a >= LOG_AT {
                                        let k = (a - LOG_AT) as usize;
                                        *b = if k < slot.len() { slot[k] } else { 0xFF };
                                    }
                                }
                            })
                            .await?
                        }
                    }
                }
                self.sats.info.set((i != Info { decl: None, stamp: 0, written: 0 }).then_some(i));
            }
            Op::SatPrefs(id, p) => {
                if self.prefs_read(id) != Some(p) {
                    match self.log_free(PREF_SLOTS, PREF_SLOT, pref_addr)? {
                        Some(n) => self.f.program(pref_addr(n), &pref_slot(id, &p)).await?,
                        None => self.prefs_compact(id, p).await?,
                    }
                }
                if self.sats.prefs.get().is_some_and(|c| c.0 == id) && self.sats.pending.get().is_none_or(|q| q.0 != id) {
                    self.sats.prefs.set(Some((id, Some(p))));
                }
            }
            _ => {}
        }
        if !matches!(op, Op::SatPrefs(..)) {
            self.sat_changed();
        }
        Ok(())
    }

    /// A full choices log rewritten with the newest choices of each satellite held, and `id`'s new ones.
    async fn prefs_compact(&self, id: u32, p: Prefs) -> Result {
        let mut keep = [None::<(u32, Prefs)>; PLACES as usize + 1];
        let mut n = 0;
        for k in 0..PLACES {
            if let Some(s) = self.sat_read(k).filter(|s| s.id != id) {
                if let Some(q) = self.prefs_read(s.id) {
                    keep[n] = Some((s.id, q));
                    n += 1;
                }
            }
        }
        keep[n] = Some((id, p));
        scratch::rewrite(&self.f, SATELLITES.start + SECTOR, &|o, piece| {
            for (j, b) in piece.iter_mut().enumerate() {
                let a = o + j as u32;
                if a >= LOG_AT {
                    let k = (a - LOG_AT) / PREF_SLOT;
                    *b = match keep.get(k as usize).copied().flatten() {
                        Some((id, p)) => pref_slot(id, &p)[((a - LOG_AT) % PREF_SLOT) as usize],
                        None => 0xFF,
                    };
                }
            }
        })
        .await
    }

    /// Choices set (the pass page, the options page): kept in RAM now, written [`crate::store::SAVE_DELAY_MS`] after
    /// the last change; another satellite's waiting choices go first.
    pub(crate) fn sat_prefs_later(&self, id: u32, p: Prefs) {
        if let Some((pid, pp, _)) = self.sats.pending.get().filter(|x| x.0 != id) {
            self.request(Op::SatPrefs(pid, pp));
        }
        self.sats.pending.set(Some((id, p, self.now_ms.get())));
        self.sats.prefs.set(Some((id, Some(p))));
    }

    /// The choices waiting, written once quiet for the save delay (or `now`: power off).
    pub(crate) fn sat_prefs_due(&self, now: Option<u32>) {
        if let Some((id, p, at)) = self.sats.pending.get() {
            if now.is_none_or(|t| t.wrapping_sub(at) >= crate::store::SAVE_DELAY_MS) && self.request(Op::SatPrefs(id, p)) {
                self.sats.pending.set(None);
            }
        }
    }

    /// Data's satellite answers.
    pub(crate) fn sat_prefs_get(&self, id: u32) -> Option<Prefs> {
        if let Some((_, p)) = self.sats.prefs.get().filter(|x| x.0 == id) {
            return p;
        }
        let p = self.prefs_read(id);
        self.sats.prefs.set(Some((id, p)));
        p
    }

    pub(crate) fn sat_stage_rec(&self, s: &Sat) -> bool {
        if self.sats.staged.get() || !self.ok() {
            return false;
        }
        *self.sats.stage.borrow_mut() = rec_encode(s);
        self.sats.staged.set(true);
        true
    }

    pub(crate) fn sat_rec_at(&self, k: u8) -> Option<Sat> {
        (self.sats.held.get() & 1u32.checked_shl(k as u32).unwrap_or(0) != 0).then(|| self.sat_read(k)).flatten()
    }

    /// Whether the store takes `op` now (None: not a satellite request).
    pub(crate) fn sat_accepts(&self, op: &Op) -> Option<bool> {
        Some(match *op {
            Op::PutSat(k) => k < PLACES && self.sats.staged.get(),
            Op::DeleteSat(k) => k < PLACES,
            Op::ClearSats | Op::SatInfo(_) | Op::SatPrefs(..) => true,
            _ => return None,
        })
    }
}

/// The store's satellites as satellite mode reads them (SAT-2): the places held, in place order, their choices
/// by catalogue number, the declination.
pub struct StoreSats<'a, N: Nor, D: Data>(pub &'a crate::FlashStore<N, D>);

impl<N: Nor, D: Data> StoreSats<'_, N, D> {
    /// The place of the `i`-th satellite held.
    fn place(&self, i: u8) -> Option<u8> {
        let held = self.0.sats.held.get();
        (0..PLACES).filter(|k| held & 1 << k != 0).nth(i as usize)
    }
}

impl<N: Nor, D: Data> SatSource for StoreSats<'_, N, D> {
    fn count(&self) -> u8 {
        self.0.sats.held.get().count_ones() as u8
    }
    fn get(&self, i: u8) -> Option<Sat> {
        self.0.sat_read(self.place(i)?)
    }
    fn version(&self) -> u32 {
        // the choices don't change the passes: only the records and the info move it
        self.0.sats.ver.get()
    }
    fn declination(&self) -> Option<i16> {
        self.0.sats.info.get().and_then(|i| i.decl)
    }
    fn prefs(&self, i: u8) -> Option<Prefs> {
        let id = self.get(i)?.id;
        self.0.sat_prefs_get(id)
    }
    fn set_prefs(&self, i: u8, p: Prefs) {
        if let Some(s) = self.get(i) {
            self.0.sat_prefs_later(s.id, p);
        }
    }
}
