//! Rewriting a sector so that a power cut leaves either all of the old content or all of the new.
//!
//! [`ferrite_core::layout::SWAP`] is two sectors: DATA takes a copy of the sector's new content, LOG a record
//! per rewrite. A rewrite:
//! 1. erases DATA and programs the new content there (the old sector, changed by the caller page by page);
//! 2. appends a log record: target address and the CRC of DATA, with its own CRC;
//! 3. erases the target and copies DATA into it;
//! 4. marks the record done (one byte programmed to 0).
//!
//! At mount, [`recover`] finishes a rewrite whose record is valid but not done (steps 3 and 4 again: they can be
//! repeated any number of times). A cut before the record is valid leaves the target untouched.
//!
//! Wear: every rewrite erases DATA once; the log is erased every 256 rewrites. A channel edit is one rewrite.

use crate::{blank, crc16, crc_of, Nor, Result, PAGE, SECTOR};
use ferrite_core::layout::SWAP;

const DATA: u32 = SWAP.start;
const LOG: u32 = SWAP.start + SECTOR;
const MAGIC: u16 = 0x5753; // "SW"
const REC: u32 = 16;

/// A log record: magic (2), reserved (2), target (4), CRC-32 of DATA (4), CRC-16 of the 12 bytes before (2),
/// done (1: 0xFF pending, 0x00 done), reserved (1).
fn rec_bytes(target: u32, crc: u32) -> [u8; 14] {
    let mut r = [0xFFu8; 14];
    r[..2].copy_from_slice(&MAGIC.to_le_bytes());
    r[4..8].copy_from_slice(&target.to_le_bytes());
    r[8..12].copy_from_slice(&crc.to_le_bytes());
    let c = crc16(&r[..12]);
    r[12..14].copy_from_slice(&c.to_le_bytes());
    r
}

/// A log record: its address, target, CRC of DATA, and whether it is done.
type Rec = (u32, u32, u32, bool);

/// The last valid log record, and the first free place for one (None: the log is full).
fn last(nor: &mut impl Nor) -> Result<(Option<Rec>, Option<u32>)> {
    let mut found = None;
    let mut a = LOG;
    while a < LOG + SECTOR {
        let mut r = [0u8; REC as usize];
        nor.read(a, &mut r)?;
        if r == [0xFF; REC as usize] {
            return Ok((found, Some(a)));
        }
        let magic = u16::from_le_bytes([r[0], r[1]]);
        let c = u16::from_le_bytes([r[12], r[13]]);
        if magic == MAGIC && crc16(&r[..12]) == c {
            let target = u32::from_le_bytes([r[4], r[5], r[6], r[7]]);
            let crc = u32::from_le_bytes([r[8], r[9], r[10], r[11]]);
            found = Some((a, target, crc, r[14] == 0));
        }
        a += REC;
    }
    Ok((found, None))
}

/// Copies DATA into the target sector (erasing it first).
fn copy_in(nor: &mut impl Nor, target: u32) -> Result {
    nor.erase(target)?;
    let mut buf = [0u8; PAGE as usize];
    for off in (0..SECTOR).step_by(PAGE as usize) {
        nor.read(DATA + off, &mut buf)?;
        if buf.iter().any(|&b| b != 0xFF) {
            nor.program(target + off, &buf)?;
        }
    }
    Ok(())
}

/// Finishes a rewrite a power cut interrupted. Call at mount, before reading anything.
pub fn recover(nor: &mut impl Nor) -> Result {
    if let (Some((at, target, crc, false)), _) = last(nor)? {
        if crc_of(nor, DATA, SECTOR)? == crc {
            copy_in(nor, target)?;
        }
        nor.program(at + 14, &[0])?;
    }
    Ok(())
}

/// Rewrites the sector at `target`: `change(offset, page)` gets each 256-byte page of the old content (offset
/// from the sector's start) and changes it in place.
pub fn rewrite(nor: &mut impl Nor, target: u32, change: &mut dyn FnMut(u32, &mut [u8; PAGE as usize])) -> Result {
    nor.erase(DATA)?;
    let mut buf = [0u8; PAGE as usize];
    for off in (0..SECTOR).step_by(PAGE as usize) {
        nor.read(target + off, &mut buf)?;
        change(off, &mut buf);
        if buf.iter().any(|&b| b != 0xFF) {
            nor.program(DATA + off, &buf)?;
        }
    }
    let crc = crc_of(nor, DATA, SECTOR)?;
    // The log: the next free record, or a fresh log (every earlier record is done: rewrites run one at a time
    // and recover() finished any left over).
    let at = match last(nor)?.1 {
        Some(a) if blank(nor, a, REC)? => a,
        _ => {
            nor.erase(LOG)?;
            LOG
        }
    };
    nor.program(at, &rec_bytes(target, crc))?;
    copy_in(nor, target)?;
    nor.program(at + 14, &[0])
}
