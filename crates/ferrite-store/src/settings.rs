//! The settings and slots: two fixed copies, A and B, one sector each. A save erases the older copy (or a bad
//! one) and writes the record there; mount uses the newer valid one. A power cut at any byte leaves the other
//! copy whole.
//!
//! Record: magic (2) · payload length (2) · counter (4) · payload · CRC-32 of length, counter and payload (4).

use crate::io::Flash;
use crate::{u16_at, u32_at, Nor, Result};
use ferrite_app::persist::Source;
use ferrite_core::layout::SETTINGS;

const MAGIC: u16 = 0x4153; // "SA"
const HEAD: u32 = 8;
/// The largest payload (the settings and slots are about 450 bytes).
pub const MAX_PAYLOAD: usize = 512;
const COPIES: [u32; 2] = [SETTINGS.start, SETTINGS.start + crate::SECTOR];

/// The copy in use: where its payload is, how long it is, its counter.
#[derive(Clone, Copy)]
pub struct Saved {
    pub at: u32,
    pub len: u32,
    pub count: u32,
}

fn read_copy<N: Nor>(f: &Flash<N>, base: u32) -> Result<Option<Saved>> {
    let mut h = [0u8; HEAD as usize];
    f.raw_read(base, &mut h)?;
    let len = u16_at(&h, 2) as u32;
    if u16_at(&h, 0) != MAGIC || len as usize > MAX_PAYLOAD {
        return Ok(None);
    }
    let mut c = [0u8; 4];
    f.raw_read(base + HEAD + len, &mut c)?;
    Ok((f.crc(base + 2, HEAD - 2 + len)? == u32_at(&c, 0)).then_some(Saved { at: base + HEAD, len, count: u32_at(&h, 4) }))
}

/// The newer valid copy.
pub fn latest<N: Nor>(f: &Flash<N>) -> Result<Option<Saved>> {
    let a = read_copy(f, COPIES[0])?;
    let b = read_copy(f, COPIES[1])?;
    Ok(match (a, b) {
        (Some(a), Some(b)) => Some(if b.count > a.count { b } else { a }),
        (a, b) => a.or(b),
    })
}

/// Writes `payload` over the older copy.
pub async fn save<N: Nor>(f: &Flash<N>, payload: &[u8]) -> Result {
    let cur = latest(f)?;
    let base = match cur {
        Some(c) if c.at - HEAD == COPIES[0] => COPIES[1],
        _ => COPIES[0],
    };
    let count = cur.map_or(1, |c| c.count.wrapping_add(1));
    let mut h = [0u8; HEAD as usize];
    h[..2].copy_from_slice(&MAGIC.to_le_bytes());
    h[2..4].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    h[4..8].copy_from_slice(&count.to_le_bytes());
    let mut crc = crate::Crc32::new();
    crc.update(&h[2..]);
    crc.update(payload);
    f.erase(base).await?;
    f.program(base, &h).await?;
    f.program(base + HEAD, payload).await?;
    f.program(base + HEAD + payload.len() as u32, &crc.get().to_le_bytes()).await
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
