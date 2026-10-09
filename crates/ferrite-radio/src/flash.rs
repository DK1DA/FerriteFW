//! The SPI flash driver's guard (SAFE-1, SAFE-2): which program and erase requests reach the chip. The firmware's
//! storage driver asks these before any command goes out; reads are never limited.

use core::ops::Range;

/// The chip's program page and erase sector (ferrite-store's `PAGE` and `SECTOR`).
pub const PAGE: u32 = 256;
pub const SECTOR: u32 = 4096;

/// Wholly inside one of the writable ranges (`ferrite_core::layout::WRITABLE`, or none in the bring-up build).
pub fn writable(ok: &[Range<u32>], addr: u32, len: u32) -> bool {
    addr.checked_add(len).is_some_and(|end| ok.iter().any(|r| r.start <= addr && end <= r.end))
}

/// A page program: not empty, writable, and inside one page (writable first: its sum can't overflow).
pub fn program_ok(ok: &[Range<u32>], addr: u32, len: u32) -> bool {
    len != 0 && len <= PAGE && writable(ok, addr, len) && addr / PAGE == (addr + len - 1) / PAGE
}

/// A 4 KB sector erase, the only erase there is: sector-aligned, and the whole sector writable (the check is on the
/// erase unit, never the range the caller meant).
pub fn erase_ok(ok: &[Range<u32>], addr: u32) -> bool {
    addr.is_multiple_of(SECTOR) && writable(ok, addr, SECTOR)
}
