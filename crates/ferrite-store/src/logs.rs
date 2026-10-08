//! The records that grow with use (`docs/firmware-design.md`, "SPI flash layout"; manual INBOX-6 – INBOX-8,
//! APRS-11, GPS-10):
//!
//! - [`Ring`]: the event log (EVENT_LOG, 992 records) and the APRS messages (APRS's last six sectors, 186
//!   records). 128-byte records, 31 to a sector, record `seq` at slot `seq % capacity`; entering a sector erases
//!   it, so the oldest 31 go at once (INBOX-8). Appends only program erased bytes; marking an event or message
//!   read, and a message's tries, acks and failure, only clear bits in bytes the CRC leaves out, so none of it
//!   erases anything. A power cut mid-append leaves a torn record after the newest: mount blanks it through the
//!   scratch sector (or erases a sector a cut erase left as noise).
//! - [`Waypoints`]: up to 256 records of 32 bytes in WAYPOINTS, kept packed oldest first, so the list (newest
//!   first) is the slots backwards. Adding programs the next slot; deleting moves the later ones down a slot,
//!   sector by sector through the scratch sector (a cut can leave one waypoint twice, never lose one).
//! - The APRS settings and the quick messages: one record in two copies (APRS's first two sectors), kept like
//!   the settings ([`crate::settings`]).
//!
//! Times are the store's own seconds ([`crate::FlashStore::set_time`]): UTC once a clock (GPS, or set by hand) is
//! set; before that they count on from the newest record stored, so "ago" never runs backwards, but the time the
//! radio was off isn't counted (there is no RTC: decision 76).

use crate::io::Flash;
use crate::{crc16, scratch, u16_at, u32_at, Nor, Result, SECTOR};
use core::cell::Cell;
use ferrite_app::data::{Event, Message, MsgState, Pos, Src, Waypoint, WpSrc};
use ferrite_ui::text::Buf;

pub const REC: u32 = 128;
/// Records in a sector: the last 128 bytes stay free for the scratch copy's footer.
pub const PER: u32 = 31;
const MAGIC: [u8; 2] = [0xE7, 0x01];
/// Offsets in a record: seq (0..4), time (4..8), magic (8..10), the read byte (10, outside the CRC), body from 11,
/// CRC-16 at 126 over 0..10 and 11..body end.
const READ: usize = 10;
const BODY: usize = 11;
const CRC_AT: usize = 126;

/// A ring of 128-byte records over whole sectors.
pub struct Ring {
    start: u32,
    sectors: u32,
    /// Where the CRC stops (bytes after it, up to 126, may be programmed later).
    body_end: usize,
    newest: Cell<Option<u32>>,
    oldest: Cell<u32>,
}

impl Ring {
    pub const fn new(start: u32, end: u32, body_end: usize) -> Self {
        Ring { start, sectors: (end - start) / SECTOR, body_end, newest: Cell::new(None), oldest: Cell::new(0) }
    }

    pub fn cap(&self) -> u32 {
        self.sectors * PER
    }

    fn addr(&self, seq: u32) -> u32 {
        let s = seq % self.cap();
        self.start + s / PER * SECTOR + s % PER * REC
    }

    fn crc(&self, b: &[u8; REC as usize]) -> u16 {
        let mut t = [0u8; CRC_AT];
        t[..READ].copy_from_slice(&b[..READ]);
        let n = self.body_end - BODY;
        t[READ..READ + n].copy_from_slice(&b[BODY..self.body_end]);
        crc16(&t[..READ + n])
    }

    fn valid(&self, b: &[u8; REC as usize], seq: u32) -> bool {
        u32_at(b, 0) == seq && b[8..10] == MAGIC && self.crc(b) == u16_at(b, CRC_AT)
    }

    /// Records stored.
    pub fn len(&self) -> u16 {
        self.newest.get().map_or(0, |n| (n - self.oldest.get() + 1) as u16)
    }

    pub fn is_empty(&self) -> bool {
        self.newest.get().is_none()
    }

    /// The newest record's time, if any.
    pub fn newest_time<N: Nor>(&self, f: &Flash<N>) -> Option<u32> {
        self.read(f, 0).map(|b| u32_at(&b, 4))
    }

    /// Finds the records, and repairs what a power cut left (see the module comment).
    pub async fn mount<N: Nor>(&self, f: &Flash<N>) -> Result {
        let (mut newest, mut oldest): (Option<u32>, u32) = (None, u32::MAX);
        for sec in 0..self.sectors {
            let base = self.start + sec * SECTOR;
            // Scanned in a block of its own: the record buffer isn't kept across the repairs' waits.
            let (good, bad) = {
                let mut b = [0u8; REC as usize];
                let (mut good, mut bad) = (0, 0u32);
                for k in 0..PER {
                    f.raw_read(base + k * REC, &mut b)?;
                    if b == [0xFF; REC as usize] {
                        continue;
                    }
                    let seq = u32_at(&b, 0);
                    if seq % self.cap() == sec * PER + k && self.valid(&b, seq) {
                        good += 1;
                        newest = Some(newest.map_or(seq, |n: u32| n.max(seq)));
                        oldest = oldest.min(seq);
                    } else {
                        bad |= 1 << k;
                    }
                }
                (good, bad)
            };
            if bad != 0 && good == 0 {
                f.erase(base).await?;
            } else if bad != 0 {
                scratch::rewrite(f, base, &|o, piece| {
                    for (i, x) in piece.iter_mut().enumerate() {
                        let k = (o + i as u32) / REC;
                        if k < PER && bad & 1 << k != 0 {
                            *x = 0xFF;
                        }
                    }
                })
                .await?;
            }
        }
        // Records from an older lap than the newest's window (a cut between erasing and appending can't leave
        // any, but a noisy chip might): not counted.
        if let Some(n) = newest {
            oldest = oldest.max(n.saturating_sub(self.cap() - 1));
        }
        self.newest.set(newest);
        self.oldest.set(if newest.is_some() { oldest } else { 0 });
        Ok(())
    }

    /// Record `i`, newest first.
    pub fn read<N: Nor>(&self, f: &Flash<N>, i: u16) -> Option<[u8; REC as usize]> {
        let seq = self.seq(i)?;
        let mut b = [0u8; REC as usize];
        f.read(self.addr(seq), &mut b).ok()?;
        self.valid(&b, seq).then_some(b)
    }

    /// Record `i`'s sequence number, newest first.
    pub fn seq(&self, i: u16) -> Option<u32> {
        (i < self.len()).then(|| self.newest.get().unwrap_or(0) - i as u32)
    }

    /// The record with this sequence number, if still stored.
    pub fn by_seq<N: Nor>(&self, f: &Flash<N>, seq: u32) -> Option<[u8; REC as usize]> {
        let n = self.newest.get()?;
        if seq > n || seq < self.oldest.get() {
            return None;
        }
        self.read(f, (n - seq) as u16)
    }

    /// Appends a record: `rec`'s seq, magic and CRC are filled in here, its time and body by the caller.
    pub async fn append<N: Nor>(&self, f: &Flash<N>, rec: &mut [u8; REC as usize]) -> Result<u32> {
        let seq = self.newest.get().map_or(0, |n| n + 1);
        let a = self.addr(seq);
        let base = a - a % SECTOR;
        if (seq % self.cap()).is_multiple_of(PER) {
            if !f.blank(base, SECTOR)? {
                f.erase(base).await?;
            }
            // The sector held the lap before's records seq - cap .. that sector's end.
            let lap_end = (seq - (seq % self.cap()) % PER + PER).saturating_sub(self.cap());
            self.oldest.set(self.oldest.get().max(lap_end));
        } else if !f.blank(a, REC)? {
            // A torn append this power-on (the flash failed under it): blank that slot only.
            let off = a - base;
            scratch::rewrite(f, base, &|o, piece| {
                for (i, x) in piece.iter_mut().enumerate() {
                    if (off..off + REC).contains(&(o + i as u32)) {
                        *x = 0xFF;
                    }
                }
            })
            .await?;
        }
        rec[..4].copy_from_slice(&seq.to_le_bytes());
        rec[8..10].copy_from_slice(&MAGIC);
        let c = self.crc(rec);
        rec[CRC_AT..].copy_from_slice(&c.to_le_bytes());
        f.program(a, &rec[..]).await?;
        if self.newest.get().is_none() {
            self.oldest.set(seq);
        }
        self.newest.set(Some(seq));
        Ok(seq)
    }

    /// Clears bits of one byte outside the CRC (`at` is 10 or past the body) of the record with this seq.
    pub async fn mark<N: Nor>(&self, f: &Flash<N>, seq: u32, at: usize, value: u8) -> Result {
        let Some(b) = self.by_seq(f, seq) else { return Ok(()) };
        if b[at] & value != b[at] {
            f.program(self.addr(seq) + at as u32, &[b[at] & value]).await?;
        }
        Ok(())
    }

    /// Every record gone.
    pub async fn clear<N: Nor>(&self, f: &Flash<N>) -> Result {
        for sec in 0..self.sectors {
            let base = self.start + sec * SECTOR;
            if !f.blank(base, SECTOR)? {
                f.erase_blanked(base).await?;
            }
        }
        self.forget();
        Ok(())
    }

    /// The area was erased behind the ring's back (the reset).
    pub fn forget(&self) {
        self.newest.set(None);
        self.oldest.set(0);
    }
}

fn put<const N: usize>(b: &mut [u8], s: &Buf<N>) {
    let n = s.len().min(b.len());
    b[..n].copy_from_slice(&s.as_bytes()[..n]);
}

fn text<const N: usize>(b: &[u8]) -> Buf<N> {
    let n = b.iter().position(|&c| c == 0 || c == 0xFF).unwrap_or(b.len()).min(N);
    Buf::from(&b[..n])
}

// ---- events ----

/// Where an event's body ends (bytes 118..126 stay erased).
pub const EVENT_END: usize = 118;
/// What a queued event carries: the read byte and the body (11..118).
pub const EVENT_JOB: usize = 1 + EVENT_END - BODY;

/// An event as the Storage task queues it ([`crate::store::Job::Log`]): no seq or time yet.
pub fn event_job(e: &Event) -> [u8; EVENT_JOB] {
    let mut r = [0u8; REC as usize];
    r[READ] = if e.unread { 0xFF } else { 0 };
    r[11] = Src::ALL.iter().position(|s| *s == e.src).unwrap_or(0) as u8;
    r[12] = e.slot;
    r[13] = e.pos.is_some() as u8;
    r[14..18].copy_from_slice(&e.freq.to_le_bytes());
    r[18..22].copy_from_slice(&e.id.to_le_bytes());
    r[22..24].copy_from_slice(&e.x.to_le_bytes());
    r[24..26].copy_from_slice(&e.y.to_le_bytes());
    let p = e.pos.unwrap_or_default();
    r[26..30].copy_from_slice(&p.lat.to_le_bytes());
    r[30..34].copy_from_slice(&p.lon.to_le_bytes());
    put(&mut r[34..54], &e.name);
    put(&mut r[54..118], &e.text);
    let mut j = [0u8; EVENT_JOB];
    j[0] = r[READ];
    j[1..].copy_from_slice(&r[BODY..EVENT_END]);
    j
}

/// The record for a queued event, stamped `t`.
pub fn event_rec(j: &[u8; EVENT_JOB], t: u32) -> [u8; REC as usize] {
    let mut r = [0xFFu8; REC as usize];
    r[4..8].copy_from_slice(&t.to_le_bytes());
    r[READ] = j[0];
    r[BODY..EVENT_END].copy_from_slice(&j[1..]);
    r
}

pub fn event_decode(b: &[u8; REC as usize], now: u32) -> Event {
    let pos = (b[13] & 1 != 0).then(|| Pos { lat: u32_at(b, 26) as i32, lon: u32_at(b, 30) as i32 });
    Event {
        src: Src::ALL[(b[11] as usize).min(Src::ALL.len() - 1)],
        ago_s: now.saturating_sub(u32_at(b, 4)),
        freq: u32_at(b, 14),
        slot: b[12],
        unread: b[READ] == 0xFF,
        id: u32_at(b, 18),
        x: u16_at(b, 22),
        y: u16_at(b, 24),
        pos,
        name: text(&b[34..54]),
        text: text(&b[54..118]),
    }
}

// ---- APRS messages ----

/// Where a message's body ends; 96 (tries, one bit cleared per try) and 97 (bit 0 cleared: acked, bit 1:
/// failed) are programmed later.
pub const MSG_END: usize = 96;
const TRIES: usize = 96;
const FINAL: usize = 97;

pub fn msg_rec(inbound: bool, peer: &Buf<10>, txt: &Buf<67>, t: u32) -> [u8; REC as usize] {
    let mut r = [0xFFu8; REC as usize];
    r[4..8].copy_from_slice(&t.to_le_bytes());
    r[BODY..MSG_END].fill(0);
    r[11] = inbound as u8;
    put(&mut r[12..22], peer);
    put(&mut r[22..89], txt);
    r
}

pub fn msg_decode(b: &[u8; REC as usize], now: u32) -> Message {
    let inbound = b[11] != 0;
    let tries = (!b[TRIES]).count_ones() as u8;
    let state = if inbound {
        if b[READ] == 0xFF { MsgState::Unread } else { MsgState::Read }
    } else if b[FINAL] & 1 == 0 {
        MsgState::Acked
    } else if b[FINAL] & 2 == 0 {
        MsgState::Failed
    } else if tries >= 2 {
        MsgState::Retrying(tries)
    } else {
        MsgState::Pending
    };
    Message { inbound, peer: text(&b[12..22]), text: text(&b[22..89]), ago_s: now.saturating_sub(u32_at(b, 4)), state }
}

/// The byte and value that move an outgoing message to `state` (bits only cleared).
pub fn msg_state_byte(state: MsgState) -> Option<(usize, u8)> {
    match state {
        MsgState::Acked => Some((FINAL, !1)),
        MsgState::Failed => Some((FINAL, !2)),
        MsgState::Retrying(n) => Some((TRIES, 0xFFu8.checked_shl(n.min(8) as u32).unwrap_or(0))),
        MsgState::Pending => Some((TRIES, 0xFE)),
        MsgState::Read => Some((READ, 0)),
        MsgState::Unread => None,
    }
}

/// Whether record `b` is our message to `peer` with `txt`, still waiting for its end.
pub fn msg_open(b: &[u8; REC as usize], peer: &Buf<10>, txt: &Buf<67>) -> bool {
    let m = msg_decode(b, 0);
    !m.inbound && m.peer == *peer && m.text == *txt && matches!(m.state, MsgState::Pending | MsgState::Retrying(_))
}

// ---- waypoints ----

pub const WP_LEN: u32 = 32;
/// Waypoints in a sector (the last 32 bytes stay free for the scratch footer).
pub const WP_PER: u32 = 127;
/// GPS-10.
pub const WP_MAX: u16 = 256;

/// Name (16) · latitude (4) · longitude (4) · source (1) · 0xFF (5) · CRC-16 of the 30 bytes before (2).
pub fn wp_encode(w: &Waypoint) -> [u8; WP_LEN as usize] {
    let mut b = [0xFFu8; WP_LEN as usize];
    b[..16].fill(0);
    put(&mut b[..16], &w.name);
    b[16..20].copy_from_slice(&w.pos.lat.to_le_bytes());
    b[20..24].copy_from_slice(&w.pos.lon.to_le_bytes());
    b[24] = w.src as u8;
    let c = crc16(&b[..30]);
    b[30..32].copy_from_slice(&c.to_le_bytes());
    b
}

pub fn wp_decode(b: &[u8; WP_LEN as usize]) -> Option<Waypoint> {
    if *b == [0xFF; WP_LEN as usize] || crc16(&b[..30]) != u16_at(b, 30) {
        return None;
    }
    let src = [WpSrc::Marked, WpSrc::Aprs, WpSrc::Link, WpSrc::Grid][(b[24] as usize).min(3)];
    Some(Waypoint { name: text(&b[..16]), pos: Pos { lat: u32_at(b, 16) as i32, lon: u32_at(b, 20) as i32 }, src })
}

/// The waypoints, packed oldest first from slot 0.
pub struct Waypoints {
    start: u32,
    count: Cell<u16>,
}

impl Waypoints {
    pub const fn new(start: u32) -> Self {
        Waypoints { start, count: Cell::new(0) }
    }

    fn addr(&self, slot: u16) -> u32 {
        let s = slot as u32;
        self.start + s / WP_PER * SECTOR + s % WP_PER * WP_LEN
    }

    pub fn len(&self) -> u16 {
        self.count.get()
    }

    pub fn is_empty(&self) -> bool {
        self.count.get() == 0
    }

    /// Counts the packed waypoints, and blanks whatever follows them: a torn one (a cut add), or old data in the
    /// area (a chip that held something else there), so the next adds program erased bytes.
    pub async fn mount<N: Nor>(&self, f: &Flash<N>) -> Result {
        let n = {
            let mut b = [0u8; WP_LEN as usize];
            let mut n = 0u16;
            while n < WP_MAX {
                f.raw_read(self.addr(n), &mut b)?;
                if wp_decode(&b).is_none() {
                    break;
                }
                n += 1;
            }
            n
        };
        self.count.set(n);
        let sectors = (WP_MAX as u32).div_ceil(WP_PER);
        for sec in 0..sectors {
            let base = self.start + sec * SECTOR;
            // The first slot of this sector that must be blank.
            let from = (n as u32).saturating_sub(sec * WP_PER).min(WP_PER);
            if from == WP_PER || f.blank(base + from * WP_LEN, SECTOR - from * WP_LEN)? {
                continue;
            }
            if from == 0 {
                f.erase(base).await?;
            } else {
                scratch::rewrite(f, base, &|o, piece| {
                    for (i, x) in piece.iter_mut().enumerate() {
                        if o + i as u32 >= from * WP_LEN {
                            *x = 0xFF;
                        }
                    }
                })
                .await?;
            }
        }
        Ok(())
    }

    /// Waypoint `i`, newest first.
    pub fn get<N: Nor>(&self, f: &Flash<N>, i: u16) -> Option<Waypoint> {
        let slot = self.count.get().checked_sub(1 + i)?;
        let mut b = [0u8; WP_LEN as usize];
        f.read(self.addr(slot), &mut b).ok()?;
        wp_decode(&b)
    }

    pub async fn add<N: Nor>(&self, f: &Flash<N>, w: &Waypoint) -> Result<bool> {
        let n = self.count.get();
        if n >= WP_MAX {
            return Ok(false);
        }
        f.program(self.addr(n), &wp_encode(w)).await?;
        self.count.set(n + 1);
        Ok(true)
    }

    /// Deletes the newest waypoint equal to `w`: the later ones move down a slot.
    pub async fn delete<N: Nor>(&self, f: &Flash<N>, w: &Waypoint) -> Result<bool> {
        let n = self.count.get();
        let Some(k) = (0..n).rev().find(|&s| {
            let mut b = [0u8; WP_LEN as usize];
            f.raw_read(self.addr(s), &mut b).is_ok() && wp_decode(&b).as_ref() == Some(w)
        }) else {
            return Ok(false);
        };
        let first = k as u32 / WP_PER;
        let last = (n as u32 - 1) / WP_PER;
        for sec in first..=last {
            let base = self.start + sec * SECTOR;
            scratch::rewrite(f, base, &|o, piece| {
                for (r, chunk) in piece.chunks_mut(WP_LEN as usize).enumerate() {
                    let slot = sec * WP_PER + (o / WP_LEN) + r as u32;
                    if (o / WP_LEN) + (r as u32) >= WP_PER || slot < k as u32 || slot >= n as u32 {
                        continue;
                    }
                    if slot + 1 < n as u32 {
                        let _ = f.raw_read(self.addr(slot as u16 + 1), chunk);
                    } else {
                        chunk.fill(0xFF);
                    }
                }
            })
            .await?;
        }
        self.count.set(n - 1);
        Ok(true)
    }

    pub fn forget(&self) {
        self.count.set(0);
    }
}
