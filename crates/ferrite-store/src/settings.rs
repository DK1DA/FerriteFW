//! The settings and slots: two copies, A and B, one sector each. Each save is added after the last record in the
//! sector in use; when it has no room, the other sector is erased and the record starts it (decision 377: six
//! saves to an erase at today's 586-byte payload, against one before). Mount uses the newest valid record of either. A power cut at any
//! byte leaves the record before it whole: a cut append is a record whose CRC fails, read past; a cut erase leaves
//! the other sector, which holds the newest.
//!
//! Record: magic (2) · payload length (2) · counter (4) · payload · CRC-32 of length, counter and payload (4).
//! Records follow each other with no gap from the start of the sector; firmware from before decision 377 reads
//! only the first record of each sector (a downgrade comes back with older settings).

use crate::io::Flash;
use crate::{u16_at, u32_at, Nor, Result};
use ferrite_app::persist::Source;
use ferrite_core::layout::SETTINGS;

const MAGIC: u16 = 0x4153; // "SA"
const HEAD: u32 = 8;
/// The largest payload: the settings' length (2), the settings and the slots (586 bytes now). 1 KB of each 4 KB
/// copy's sector, so settings can grow for a long while; the RAM it costs is the store's one save buffer (the
/// Storage task writes straight from it). A record over 532 bytes is refused by firmware from before Oct 7, 2026,
/// which then starts from defaults: a downgrade only.
pub const MAX_PAYLOAD: usize = 1024;
const COPIES: [u32; 2] = [SETTINGS.start, SETTINGS.start + crate::SECTOR];

/// The copy in use: where its payload is, how long it is, its counter.
#[derive(Clone, Copy)]
pub struct Saved {
    pub at: u32,
    pub len: u32,
    pub count: u32,
}

/// The room a copy's sector gives its records: all of it for the settings, which the scratch sector never copies;
/// elsewhere (the APRS record) the last 128 bytes stay erased, as in every sector it may copy (its footer goes
/// there).
fn area(base: u32) -> u32 {
    if SETTINGS.contains(&base) { crate::SECTOR } else { crate::SECTOR - 128 }
}
/// The bytes after a payload: its CRC.
const TAIL: u32 = 4;

/// One copy's sector read through: its newest valid record, and where the next record may start (None: the rest
/// of the sector isn't blank, or there is no room).
struct Copy {
    newest: Option<Saved>,
    free: Option<u32>,
}

fn read_copy<N: Nor>(f: &Flash<N>, base: u32, max: usize) -> Result<Copy> {
    let mut c = Copy { newest: None, free: None };
    let (mut off, area) = (0, area(base));
    while off + HEAD + TAIL <= area {
        let mut h = [0u8; HEAD as usize];
        f.raw_read(base + off, &mut h)?;
        if h == [0xFF; HEAD as usize] {
            c.free = Some(off);
            break;
        }
        let len = u16_at(&h, 2) as u32;
        if u16_at(&h, 0) != MAGIC || len as usize > max || off + HEAD + len + TAIL > area {
            break; // noise (a cut erase) or a torn header: nothing more is added here
        }
        let mut t = [0u8; TAIL as usize];
        f.raw_read(base + off + HEAD + len, &mut t)?;
        if f.crc(base + off + 2, HEAD - 2 + len)? == u32_at(&t, 0) {
            let s = Saved { at: base + off + HEAD, len, count: u32_at(&h, 4) };
            if c.newest.is_none_or(|n| s.count > n.count) {
                c.newest = Some(s);
            }
        }
        off += HEAD + len + TAIL; // a torn record (CRC fails) is passed over
    }
    Ok(c)
}

/// The newest valid record.
pub fn latest<N: Nor>(f: &Flash<N>) -> Result<Option<Saved>> {
    latest_of(f, COPIES, MAX_PAYLOAD)
}

/// The newest valid record of one kept the same way in `copies` (the APRS settings use it too).
pub fn latest_of<N: Nor>(f: &Flash<N>, copies: [u32; 2], max: usize) -> Result<Option<Saved>> {
    let a = read_copy(f, copies[0], max)?.newest;
    let b = read_copy(f, copies[1], max)?.newest;
    Ok(match (a, b) {
        (Some(a), Some(b)) => Some(if b.count > a.count { b } else { a }),
        (a, b) => a.or(b),
    })
}

/// Saves `payload` as the settings' newest record.
pub async fn save<N: Nor>(f: &Flash<N>, payload: &[u8]) -> Result {
    save_to(f, COPIES, payload).await
}

/// Saves `payload` as the newest record in `copies`: after the last one in the sector in use, or at the start of
/// the other sector, erased first, when it doesn't fit (or the rest of the sector isn't blank).
pub async fn save_to<N: Nor>(f: &Flash<N>, copies: [u32; 2], payload: &[u8]) -> Result {
    let max = payload.len().max(MAX_PAYLOAD);
    let (a, b) = (read_copy(f, copies[0], max)?, read_copy(f, copies[1], max)?);
    // The sector in use holds the newest record (B on a tie never happens: counts only grow).
    let (cur, cur_base, other) = match (a.newest, b.newest) {
        (Some(x), Some(y)) if y.count > x.count => (b, copies[1], copies[0]),
        (Some(_), _) => (a, copies[0], copies[1]),
        (None, Some(_)) => (b, copies[1], copies[0]),
        (None, None) => (a, copies[0], copies[0]), // nothing saved: A, erased first
    };
    let count = cur.newest.map_or(1, |c| c.count.wrapping_add(1));
    let need = HEAD + payload.len() as u32 + TAIL;
    let at = match cur.free.filter(|_| cur.newest.is_some()) {
        Some(off) if off + need <= area(cur_base) && f.blank(cur_base + off, need)? => cur_base + off,
        _ => {
            f.erase(other).await?;
            other
        }
    };
    let mut h = [0u8; HEAD as usize];
    h[..2].copy_from_slice(&MAGIC.to_le_bytes());
    h[2..4].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    h[4..8].copy_from_slice(&count.to_le_bytes());
    let mut crc = crate::Crc32::new();
    crc.update(&h[2..]);
    crc.update(payload);
    f.program(at, &h).await?;
    f.program(at + HEAD, payload).await?;
    f.program(at + HEAD + payload.len() as u32, &crc.get().to_le_bytes()).await
}

/// Reads a copy's payload from the flash, in order.
pub struct FlashSource<'a, N: Nor> {
    pub f: &'a Flash<N>,
    pub at: u32,
    pub end: u32,
}

impl<N: Nor> Source for FlashSource<'_, N> {
    fn take(&mut self, out: &mut [u8]) -> bool {
        let n = out.len() as u32;
        if self.at + n > self.end || self.f.raw_read(self.at, out).is_err() {
            return false;
        }
        self.at += n;
        true
    }
}
