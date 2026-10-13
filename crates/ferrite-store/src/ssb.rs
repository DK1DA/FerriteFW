//! The SSB patch in SPI flash (HF-18 to HF-21): installing the flasher's bundle or a link import, and checking it.
//! Only SSB_PATCH is written; a format leaves it alone (SAFE-9).

use crate::io::Flash;
use crate::{Nor, NorError, Result, SECTOR};
use ferrite_core::layout::SSB_PATCH;
use ferrite_core::ssb::{self, Patch, HEADER};

/// The stored patch, if its header parses, its records look like a patch download and its hash matches.
pub fn check<N: Nor>(f: &Flash<N>) -> Result<Option<Patch>> {
    let mut h = [0u8; HEADER];
    f.raw_read(SSB_PATCH.start, &mut h)?;
    let Some(p) = ssb::stored(&h) else { return Ok(None) };
    let mut sha = ssb::hasher();
    let mut buf = [0u8; 256];
    let mut off = 0u32;
    while off < p.len {
        let n = (p.len - off).min(buf.len() as u32) as usize;
        f.raw_read(SSB_PATCH.start + HEADER as u32 + off, &mut buf[..n])?;
        if off == 0 && buf[0] != 0x15 {
            return Ok(None);
        }
        if !buf[..n].chunks(8).all(|r| r[0] == 0x15 || r[0] == 0x16) {
            return Ok(None);
        }
        sha.update(&buf[..n]);
        off += n as u32;
    }
    Ok((sha.finish() == p.sha).then_some(p))
}

/// Writes `patch` with its header, unless the same patch is there already and checks (false). Refuses a patch
/// whose bytes don't match `p` (length, framing, hash) before erasing anything.
pub async fn install<N: Nor>(f: &Flash<N>, p: Patch, patch: &[u8], source: u8) -> Result<bool> {
    if patch.len() as u32 != p.len || !ssb::records_ok(patch) || ferrite_core::sha256::sha256(patch) != p.sha {
        return Err(NorError);
    }
    if check(f)?.is_some_and(|have| have == p) {
        return Ok(false);
    }
    for s in SSB_PATCH.clone().step_by(SECTOR as usize) {
        if !f.blank(s, SECTOR)? {
            f.erase(s).await?;
        }
    }
    // The patch first, the header last: a cut leaves no header, so nothing half-written ever checks.
    f.program(SSB_PATCH.start + HEADER as u32, patch).await?;
    f.program(SSB_PATCH.start, &ssb::store_header(&p, source)).await?;
    match check(f)? {
        Some(have) if have == p => Ok(true),
        _ => Err(NorError),
    }
}
