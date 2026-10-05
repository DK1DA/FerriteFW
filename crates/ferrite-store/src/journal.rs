//! The settings journal: two sectors; each save appends one record, and the newest valid record (highest
//! sequence number, good CRC) wins. When the sector holding the newest record is full, the other one is erased
//! and the record goes at its start, so the newest record is never in a sector being erased: a power cut at
//! any byte leaves the old record or the new one.
//!
//! Record, 4-byte aligned: magic (2) · payload length (2) · sequence (4) · payload · CRC-32 of length,
//! sequence and payload (4).
//!
//! A sector is scanned record by record from its start. A torn record (bad CRC) is stepped over by its length;
//! anything that isn't a record or erased ends the scan, and the sector counts as full. Before appending, the
//! bytes are checked to be erased (a cut program can leave stray zero bits past the record it tore).

use crate::{blank, Crc32, Nor, Result, PAGE, SECTOR};
use core::ops::Range;
use ferrite_app::persist::{Sink, Source};

const MAGIC: u16 = 0x4A53; // "SJ"
const HEAD: u32 = 8;
/// The largest payload this accepts (well above the settings and slots, about 450 bytes).
pub const MAX_PAYLOAD: u32 = 2048;

fn size(len: u32) -> u32 {
    (HEAD + len + 4).next_multiple_of(4)
}

/// The newest valid record: where its payload is, how long, its sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Latest {
    pub at: u32,
    pub len: u32,
    pub seq: u32,
}

/// What a scan of one sector found: its newest valid record and where the next one may go (None: full).
struct Scan {
    latest: Option<Latest>,
    free: Option<u32>,
}

fn scan(nor: &mut impl Nor, sector: u32) -> Result<Scan> {
    let end = sector + SECTOR;
    let (mut pos, mut latest) = (sector, None::<Latest>);
    loop {
        if pos + HEAD > end {
            return Ok(Scan { latest, free: None });
        }
        let mut h = [0u8; HEAD as usize];
        nor.read(pos, &mut h)?;
        if h == [0xFF; HEAD as usize] {
            return Ok(Scan { latest, free: Some(pos) });
        }
        let magic = u16::from_le_bytes([h[0], h[1]]);
        let len = u16::from_le_bytes([h[2], h[3]]) as u32;
        let seq = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
        if magic != MAGIC || len > MAX_PAYLOAD || pos + size(len) > end {
            return Ok(Scan { latest, free: None });
        }
        let mut c = Crc32::new();
        c.update(&h[2..]);
        let mut buf = [0u8; 64];
        let mut a = pos + HEAD;
        while a < pos + HEAD + len {
            let n = (pos + HEAD + len - a).min(64) as usize;
            nor.read(a, &mut buf[..n])?;
            c.update(&buf[..n]);
            a += n as u32;
        }
        let mut cb = [0u8; 4];
        nor.read(pos + HEAD + len, &mut cb)?;
        if c.get() == u32::from_le_bytes(cb) && latest.is_none_or(|l| seq > l.seq) {
            latest = Some(Latest { at: pos + HEAD, len, seq });
        }
        pos += size(len);
    }
}

/// The journal in `region` (two sectors).
pub struct Journal {
    pub region: Range<u32>,
}

impl Journal {
    fn sectors(&self) -> [u32; 2] {
        [self.region.start, self.region.start + SECTOR]
    }

    /// The newest valid record, if any.
    pub fn latest(&self, nor: &mut impl Nor) -> Result<Option<Latest>> {
        let mut best: Option<Latest> = None;
        for s in self.sectors() {
            if let Some(l) = scan(nor, s)?.latest {
                if best.is_none_or(|b| l.seq > b.seq) {
                    best = Some(l);
                }
            }
        }
        Ok(best)
    }

    /// Appends a record whose payload `write` produces (it is called twice: once to size and check it, once to
    /// program it, and must produce the same bytes both times).
    pub fn append<N: Nor>(&self, nor: &mut N, write: &dyn Fn(&mut dyn Sink)) -> Result {
        let mut c = Crc32::new();
        let mut len = 0u32;
        {
            let mut s = LenCrc { c: &mut c, n: &mut len };
            write(&mut s);
        }
        if len > MAX_PAYLOAD {
            return Err(crate::NorError);
        }
        // Where the newest record is, and the sequence after it.
        let [a, b] = self.sectors();
        let (sa, sb) = (scan(nor, a)?, scan(nor, b)?);
        let newest_in_b = match (sa.latest, sb.latest) {
            (Some(x), Some(y)) => y.seq > x.seq,
            (None, Some(_)) => true,
            _ => false,
        };
        let seq = sa.latest.map_or(0, |l| l.seq).max(sb.latest.map_or(0, |l| l.seq)).wrapping_add(1);
        let (cur, other, free) = if newest_in_b { (b, a, sb.free) } else { (a, b, sa.free) };
        let need = size(len);
        let at = match free {
            Some(p) if p + need <= cur + SECTOR && blank(nor, p, need)? => p,
            _ => {
                nor.erase(other)?;
                other
            }
        };
        let mut head = [0u8; HEAD as usize];
        head[..2].copy_from_slice(&MAGIC.to_le_bytes());
        head[2..4].copy_from_slice(&(len as u16).to_le_bytes());
        head[4..].copy_from_slice(&seq.to_le_bytes());
        let mut crc = Crc32::new();
        crc.update(&head[2..]);
        {
            let mut s = LenCrc { c: &mut crc, n: &mut 0 };
            write(&mut s);
        }
        let mut p = Prog::new(nor, at);
        p.put(&head);
        write(&mut p);
        p.put(&crc.get().to_le_bytes());
        p.finish()
    }
}

struct LenCrc<'a> {
    c: &'a mut Crc32,
    n: &'a mut u32,
}

impl Sink for LenCrc<'_> {
    fn put(&mut self, b: &[u8]) {
        self.c.update(b);
        *self.n += b.len() as u32;
    }
}

/// Programs a stream of bytes from `at` on, a page at a time; the first error sticks.
pub struct Prog<'a, N: Nor> {
    nor: &'a mut N,
    at: u32,
    buf: [u8; PAGE as usize],
    n: usize,
    err: Result,
}

impl<'a, N: Nor> Prog<'a, N> {
    pub fn new(nor: &'a mut N, at: u32) -> Self {
        Prog { nor, at, buf: [0xFF; PAGE as usize], n: 0, err: Ok(()) }
    }

    fn flush(&mut self) {
        if self.n > 0 && self.err.is_ok() {
            self.err = self.nor.program(self.at, &self.buf[..self.n]);
        }
        self.at += self.n as u32;
        self.n = 0;
    }

    pub fn finish(mut self) -> Result {
        self.flush();
        self.err
    }
}

impl<N: Nor> Sink for Prog<'_, N> {
    fn put(&mut self, mut b: &[u8]) {
        while !b.is_empty() {
            // Never across a page: the room left in this page from the current address.
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
}

/// Reads `len` bytes of flash from `at` as a [`Source`].
pub struct FlashSource<'a, N: Nor> {
    pub nor: &'a mut N,
    pub at: u32,
    pub end: u32,
}

impl<N: Nor> Source for FlashSource<'_, N> {
    fn take(&mut self, out: &mut [u8]) -> bool {
        let n = out.len() as u32;
        if self.at + n > self.end || self.nor.read(self.at, out).is_err() {
            return false;
        }
        self.at += n;
        true
    }
}
