//! Changing bytes that are already written: NOR can only clear bits, so the record's 4 KB sector is rewritten,
//! through a scratch sector so that a power cut leaves the old sector or the new one, never half of each.
//!
//! 1. A scratch sector gets the sector's new content (its first [`FOOTER_AT`] bytes), then a footer: where it
//!    goes and the content's CRC. The footer goes last, so a cut before it leaves the target untouched.
//! 2. The target is erased and the scratch copied into it.
//! 3. The scratch sector is erased.
//!
//! The scratch sectors are a ring of 32 (`SCRATCH_RING`, decision 227), each rewrite taking the next, so each
//! takes a 32nd of the erases one sector used to take; where the ring starts moves on every power-on. At mount,
//! [`recover`] finds a valid footer in the ring and finishes step 2 (it can be repeated any number of times),
//! then erases that sector. A scratch sector a cut left
//! half filled, without its footer, is erased when its turn comes. Every data sector leaves its last 32 bytes
//! free, which is where the footer fits.

use crate::io::Flash;
use crate::{crc16, u16_at, u32_at, Crc32, Nor, Result, PAGE, SECTOR};
use ferrite_core::layout::{APRS, CHANNELS, CONTACTS, EVENT_LOG, HEARD, MESSAGES, PRESETS, SCRATCH_RING, TRIMS, WAYPOINTS, ZONES};

/// The content a sector can hold; the footer sits after it in the scratch sector.
pub const FOOTER_AT: u32 = SECTOR - 32;
const MAGIC: u32 = 0x5243_5346; // "FSCR"

/// Rewrites `target`'s sector: `edit(offset, bytes)` changes each piece of its current content.
pub async fn rewrite<N: Nor>(f: &Flash<N>, target: u32, edit: &dyn Fn(u32, &mut [u8])) -> Result {
    let s = f.scratch_next();
    if !f.blank(s, SECTOR)? {
        f.erase(s).await?;
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
            f.program(s + off, &buf[..n]).await?;
            off += n as u32;
        }
    }
    let mut foot = [0u8; 14];
    foot[..4].copy_from_slice(&MAGIC.to_le_bytes());
    foot[4..8].copy_from_slice(&target.to_le_bytes());
    foot[8..12].copy_from_slice(&crc.get().to_le_bytes());
    let c = crc16(&foot[..12]);
    foot[12..14].copy_from_slice(&c.to_le_bytes());
    f.program(s + FOOTER_AT, &foot).await?;
    // From here the scratch copy is the sector: readers see it while the target is erased and refilled.
    f.redirect(target, s);
    let r = copy_back(f, s, target).await;
    f.end_redirect();
    r?;
    f.erase(s).await
}

async fn copy_back<N: Nor>(f: &Flash<N>, from: u32, target: u32) -> Result {
    f.erase(target).await?;
    f.copy(from, target, FOOTER_AT).await
}

/// A sector the scratch copy may go to.
fn data_sector(t: u32) -> bool {
    t.is_multiple_of(SECTOR) && [CHANNELS, ZONES, PRESETS, TRIMS, APRS, EVENT_LOG, WAYPOINTS, CONTACTS, HEARD, MESSAGES].iter().any(|r| r.contains(&t))
}

/// Finishes a rewrite a power cut stopped (before anything else is read): any scratch sector with a valid footer
/// is copied to its target if they differ, then erased.
pub async fn recover<N: Nor>(f: &Flash<N>) -> Result {
    for s in SCRATCH_RING.step_by(SECTOR as usize) {
        let mut foot = [0u8; 14];
        f.raw_read(s + FOOTER_AT, &mut foot)?;
        let target = u32_at(&foot, 4);
        let valid = u32_at(&foot, 0) == MAGIC
            && u16_at(&foot, 12) == crc16(&foot[..12])
            && data_sector(target)
            && f.crc(s, FOOTER_AT)? == u32_at(&foot, 8);
        if valid && !f.same(s, target, FOOTER_AT)? {
            copy_back(f, s, target).await?;
        }
        if valid && !f.blank(s, SECTOR)? {
            f.erase(s).await?;
        }
    }
    Ok(())
}
