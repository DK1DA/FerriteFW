//! Changing bytes that are already written: NOR can only clear bits, so the record's 4 KB sector is rewritten,
//! through one scratch sector so that a power cut leaves the old sector or the new one, never half of each.
//!
//! 1. The scratch sector gets the sector's new content (its first [`FOOTER_AT`] bytes), then a footer: where it
//!    goes and the content's CRC. The footer goes last, so a cut before it leaves the target untouched.
//! 2. The target is erased and the scratch copied into it.
//! 3. The scratch sector is erased.
//!
//! At mount, [`recover`] finds a valid footer and finishes step 2 (it can be repeated any number of times),
//! then erases the scratch sector. Every data sector leaves its last 32 bytes free, which is where the footer
//! fits. Wear: each rewrite erases the scratch sector and the target once.

use crate::io::Flash;
use crate::{crc16, u16_at, u32_at, Crc32, Nor, Result, PAGE, SECTOR};
use ferrite_core::layout::{APRS, CHANNELS, EVENT_LOG, PRESETS, SCRATCH, WAYPOINTS, ZONES};

/// The content a sector can hold; the footer sits after it in the scratch sector.
pub const FOOTER_AT: u32 = SECTOR - 32;
const MAGIC: u32 = 0x5243_5346; // "FSCR"

/// Rewrites `target`'s sector: `edit(offset, bytes)` changes each piece of its current content.
pub async fn rewrite<N: Nor>(f: &Flash<N>, target: u32, edit: &dyn Fn(u32, &mut [u8])) -> Result {
    if !f.blank(SCRATCH.start, SECTOR)? {
        f.erase(SCRATCH.start).await?;
    }
    let mut crc = Crc32::new();
    {
        // The page buffer ends with this block, so it isn't kept while the copy back (with its own) runs.
        let mut buf = [0u8; PAGE as usize];
        let mut off = 0;
        while off < FOOTER_AT {
            let n = (FOOTER_AT - off).min(PAGE) as usize;
            f.raw_read(target + off, &mut buf[..n])?;
            edit(off, &mut buf[..n]);
            crc.update(&buf[..n]);
            f.program(SCRATCH.start + off, &buf[..n]).await?;
            off += n as u32;
        }
    }
    let mut foot = [0u8; 14];
    foot[..4].copy_from_slice(&MAGIC.to_le_bytes());
    foot[4..8].copy_from_slice(&target.to_le_bytes());
    foot[8..12].copy_from_slice(&crc.get().to_le_bytes());
    let c = crc16(&foot[..12]);
    foot[12..14].copy_from_slice(&c.to_le_bytes());
    f.program(SCRATCH.start + FOOTER_AT, &foot).await?;
    // From here the scratch copy is the sector: readers see it while the target is erased and refilled.
    f.redirect(target);
    let r = copy_back(f, target).await;
    f.end_redirect();
    r?;
    f.erase(SCRATCH.start).await
}

async fn copy_back<N: Nor>(f: &Flash<N>, target: u32) -> Result {
    f.erase(target).await?;
    f.copy(SCRATCH.start, target, FOOTER_AT).await
}

/// A sector the scratch copy may go to.
fn data_sector(t: u32) -> bool {
    t.is_multiple_of(SECTOR) && [CHANNELS, ZONES, PRESETS, APRS, EVENT_LOG, WAYPOINTS].iter().any(|r| r.contains(&t))
}

/// Finishes a rewrite a power cut stopped (before anything else is read), and leaves the scratch sector erased.
pub async fn recover<N: Nor>(f: &Flash<N>) -> Result {
    let mut foot = [0u8; 14];
    f.raw_read(SCRATCH.start + FOOTER_AT, &mut foot)?;
    let target = u32_at(&foot, 4);
    let valid = u32_at(&foot, 0) == MAGIC
        && u16_at(&foot, 12) == crc16(&foot[..12])
        && data_sector(target)
        && f.crc(SCRATCH.start, FOOTER_AT)? == u32_at(&foot, 8);
    if valid && !f.same(SCRATCH.start, target, FOOTER_AT)? {
        copy_back(f, target).await?;
    }
    if !f.blank(SCRATCH.start, SECTOR)? {
        f.erase(SCRATCH.start).await?;
    }
    Ok(())
}
