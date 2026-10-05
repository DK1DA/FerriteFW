//! The zone index: one sector, derived from the channel records, so the UI can list a zone or the whole store
//! without reading every record. Rebuilt after every change to the channels and at mount when its check value
//! doesn't match them; a torn index (bad CRC) is just rebuilt.
//!
//! Layout: magic (4) · channels stored (2) · version (2) · CRC of the channels it was built from (4) · CRC of
//! everything after the header (4) · 257 zone starts (zones 0-254, "no zone", the end; 2 each) · a bitmap of the
//! numbers in use (125) · pad (1) · the numbers, grouped by zone, each zone in number order (2 each).

use crate::records::{ch_addr, ch_valid, CH_LEN, INDEX};
use crate::{crc_of, Crc32, Nor, Result, PAGE};
use ferrite_app::chan::{MAX_CHANNELS, NO_ZONE};

const MAGIC: u32 = 0x5844_495A; // "ZIDX"
const VERSION: u16 = 1;
const HEAD: u32 = 16;
const STARTS: u32 = HEAD;
const BITMAP: u32 = STARTS + 257 * 2;
pub const BITMAP_LEN: u32 = MAX_CHANNELS as u32 / 8;
const NUMS: u32 = BITMAP + BITMAP_LEN + 1;
const END: u32 = NUMS + MAX_CHANNELS as u32 * 2;

/// A zone's bucket: zones 0-254, then "no zone".
fn bucket(zone: u8) -> u32 {
    zone as u32
}

/// What the channels are: each one's zone (0xFF also for none: the bitmap says which exist), and the CRC
/// the index records over (number, zone) of every channel.
fn read_channels(nor: &mut impl Nor, zones: &mut [u8; MAX_CHANNELS as usize], used: &mut [u8; BITMAP_LEN as usize]) -> Result<(u32, u16)> {
    let mut crc = Crc32::new();
    let mut n = 0u16;
    let mut b = [0u8; CH_LEN];
    for num in 0..MAX_CHANNELS {
        nor.read(ch_addr(num), &mut b)?;
        if ch_valid(&b) {
            zones[num as usize] = b[18];
            used[num as usize / 8] |= 1 << (num % 8);
            crc.update(&num.to_le_bytes());
            crc.update(&[b[18]]);
            n += 1;
        }
    }
    Ok((crc.get(), n))
}

/// Checks the index against the channels and rebuilds it if they differ. Returns whether it rebuilt.
pub fn check(nor: &mut impl Nor) -> Result<bool> {
    let mut zones = [NO_ZONE; MAX_CHANNELS as usize];
    let mut used = [0u8; BITMAP_LEN as usize];
    let (crc, n) = read_channels(nor, &mut zones, &mut used)?;
    let mut h = [0u8; HEAD as usize];
    nor.read(INDEX.start, &mut h)?;
    let ok = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) == MAGIC
        && u16::from_le_bytes([h[6], h[7]]) == VERSION
        && u32::from_le_bytes([h[8], h[9], h[10], h[11]]) == crc
        && u32::from_le_bytes([h[12], h[13], h[14], h[15]]) == crc_of(nor, INDEX.start + HEAD, END - HEAD)?;
    if !ok {
        write(nor, &zones, &used, crc, n)?;
    }
    Ok(!ok)
}

/// Rebuilds the index from the channel records.
pub fn rebuild(nor: &mut impl Nor) -> Result {
    let mut zones = [NO_ZONE; MAX_CHANNELS as usize];
    let mut used = [0u8; BITMAP_LEN as usize];
    let (crc, n) = read_channels(nor, &mut zones, &mut used)?;
    write(nor, &zones, &used, crc, n)
}

fn write(nor: &mut impl Nor, zones: &[u8; MAX_CHANNELS as usize], used: &[u8; BITMAP_LEN as usize], chans: u32, n: u16) -> Result {
    let has = |num: usize| used[num / 8] & 1 << (num % 8) != 0;
    nor.erase(INDEX.start)?;
    // The body first, page by page, with its CRC; the header (magic and CRCs) last, so a torn write never
    // looks valid.
    let mut body = Body { nor, at: INDEX.start + HEAD, buf: [0xFF; PAGE as usize], n: 0, crc: Crc32::new(), err: Ok(()) };
    let mut start = 0u16;
    for b in 0..=256u32 {
        body.put(&start.to_le_bytes());
        if b < 256 {
            start += (0..MAX_CHANNELS as usize).filter(|&k| has(k) && bucket(zones[k]) == b).count() as u16;
        }
    }
    body.put(used);
    body.put(&[0]);
    for b in 0..256u32 {
        for k in (0..MAX_CHANNELS as usize).filter(|&k| has(k) && bucket(zones[k]) == b) {
            body.put(&(k as u16).to_le_bytes());
        }
    }
    // The unused numbers: erased, but counted in the CRC as written (0xFF).
    let rest = (MAX_CHANNELS - n) as usize * 2;
    for _ in 0..rest {
        body.crc.update(&[0xFF]);
    }
    body.flush();
    let crc = body.crc.get();
    body.err?;
    let mut h = [0u8; HEAD as usize];
    h[..4].copy_from_slice(&MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&n.to_le_bytes());
    h[6..8].copy_from_slice(&VERSION.to_le_bytes());
    h[8..12].copy_from_slice(&chans.to_le_bytes());
    h[12..16].copy_from_slice(&crc.to_le_bytes());
    nor.program(INDEX.start, &h)
}

struct Body<'a, N: Nor> {
    nor: &'a mut N,
    at: u32,
    buf: [u8; PAGE as usize],
    n: usize,
    crc: Crc32,
    err: Result,
}

impl<N: Nor> Body<'_, N> {
    fn put(&mut self, mut b: &[u8]) {
        self.crc.update(b);
        while !b.is_empty() {
            let room = (PAGE - (self.at + self.n as u32) % PAGE) as usize;
            let k = room.min(b.len());
            self.buf[self.n..self.n + k].copy_from_slice(&b[..k]);
            self.n += k;
            b = &b[k..];
            if k == room {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        if self.n > 0 && self.err.is_ok() {
            self.err = self.nor.program(self.at, &self.buf[..self.n]);
        }
        self.at += self.n as u32;
        self.n = 0;
    }
}

// ---- reads ----

fn read_u16(nor: &mut impl Nor, at: u32) -> u16 {
    let mut b = [0u8; 2];
    match nor.read(at, &mut b) {
        Ok(()) => u16::from_le_bytes(b),
        Err(_) => 0,
    }
}

/// Channels stored.
pub fn count(nor: &mut impl Nor) -> u16 {
    read_u16(nor, INDEX.start + 4)
}

/// A zone's channels: where they start among the numbers, and how many.
pub fn zone_span(nor: &mut impl Nor, zone: u8) -> (u16, u16) {
    let b = bucket(zone);
    let s = read_u16(nor, INDEX.start + STARTS + b * 2);
    let e = read_u16(nor, INDEX.start + STARTS + (b + 1) * 2);
    (s, e.saturating_sub(s))
}

/// The i-th number in the grouped list.
pub fn num_at(nor: &mut impl Nor, i: u16) -> u16 {
    read_u16(nor, INDEX.start + NUMS + i as u32 * 2)
}

/// The used-numbers bitmap.
pub fn bitmap(nor: &mut impl Nor, out: &mut [u8; BITMAP_LEN as usize]) -> Result {
    nor.read(INDEX.start + BITMAP, out)
}

/// A channel's position in its zone, by binary search over the zone's numbers.
pub fn pos_in_zone(nor: &mut impl Nor, zone: u8, num: u16) -> Option<u16> {
    let (s, n) = zone_span(nor, zone);
    let (mut lo, mut hi) = (0u16, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let v = num_at(nor, s + mid);
        if v == num {
            return Some(mid);
        }
        if v < num {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    None
}
