//! Ferrite's SPI-flash store (roadmap 1d; `docs/firmware-design.md`, "SPI flash layout" and "Records").
//!
//! - [`journal`]: the settings and the slots, one record appended per save in a pair of sectors, the newest
//!   valid one winning; a full sector is never erased while it holds the newest record.
//! - [`swap`]: rewriting a sector (a changed or deleted channel, zone or range preset) through a copy and a log,
//!   so a power cut leaves either the old sector or the new one.
//! - [`records`]: the 64-byte channel record, the 32-byte zone record and the 224-byte range preset.
//! - [`index`]: the zone index, rebuilt from the channel records whenever they change (and at mount, when it
//!   doesn't match them).
//! - [`store`]: [`FlashStore`], the UI's [`ferrite_app::chan::ChannelStore`] and [`ferrite_app::data::Data`]
//!   on top of all that, plus the 2-second save timer.
//!
//! Everything goes through the [`Nor`] trait: the radio's W25Q driver and the simulator's [`sim::SimFlash`]
//! (which can cut the power at any byte). Nothing here writes outside [`ferrite_core::layout::WRITABLE`]: the
//! driver refuses it, and the simulated flash panics on it.
//!
//! RAM: no copy of the channels. Channel records are read on demand (64 B); a zone costs a few bytes of index
//! reads. The only buffers are a 256-byte page on the stack while copying and, while the index is rebuilt, a
//! 1 KB zone table on the stack.
#![cfg_attr(not(feature = "std"), no_std)]

pub mod index;
pub mod journal;
pub mod records;
pub mod store;
pub mod swap;

#[cfg(feature = "std")]
pub mod sim;

pub use store::FlashStore;

/// The erase unit.
pub const SECTOR: u32 = 4096;
/// The program unit (a program never crosses one).
pub const PAGE: u32 = 256;

/// The flash failed (or, in the simulator, the power went).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NorError;

pub type Result<T = ()> = core::result::Result<T, NorError>;

/// NOR flash: erased bytes read 0xFF, a program only turns bits from 1 to 0, an erase sets a whole 4 KB sector
/// back to 0xFF.
pub trait Nor {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result;
    /// Programs `data` at `addr`; the caller has made sure the bytes are erased.
    fn program(&mut self, addr: u32, data: &[u8]) -> Result;
    /// Erases the sector at `addr` (a multiple of [`SECTOR`]).
    fn erase(&mut self, addr: u32) -> Result;
}

/// Whether `len` bytes at `addr` are all erased.
pub fn blank(nor: &mut impl Nor, addr: u32, len: u32) -> Result<bool> {
    let mut buf = [0u8; 64];
    let mut a = addr;
    while a < addr + len {
        let n = (addr + len - a).min(64) as usize;
        nor.read(a, &mut buf[..n])?;
        if buf[..n].iter().any(|&b| b != 0xFF) {
            return Ok(false);
        }
        a += n as u32;
    }
    Ok(true)
}

/// CRC-32 (IEEE 802.3, reflected), fed in pieces.
#[derive(Clone, Copy)]
pub struct Crc32(u32);

impl Crc32 {
    pub fn new() -> Self {
        Crc32(0xFFFF_FFFF)
    }

    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.0 ^= b as u32;
            for _ in 0..8 {
                self.0 = if self.0 & 1 != 0 { (self.0 >> 1) ^ 0xEDB8_8320 } else { self.0 >> 1 };
            }
        }
    }

    pub fn get(self) -> u32 {
        !self.0
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl ferrite_app::persist::Sink for Crc32 {
    fn put(&mut self, b: &[u8]) {
        self.update(b)
    }
}

/// CRC-16/CCITT-FALSE, for the small records.
pub fn crc16(data: &[u8]) -> u16 {
    let mut c: u16 = 0xFFFF;
    for &b in data {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

/// CRC-32 of `len` bytes of flash at `addr`.
pub fn crc_of(nor: &mut impl Nor, addr: u32, len: u32) -> Result<u32> {
    let mut buf = [0u8; 64];
    let mut c = Crc32::new();
    let mut a = addr;
    while a < addr + len {
        let n = (addr + len - a).min(64) as usize;
        nor.read(a, &mut buf[..n])?;
        c.update(&buf[..n]);
        a += n as u32;
    }
    Ok(c.get())
}

#[cfg(test)]
mod tests {
    #[test]
    fn crcs_match_their_check_values() {
        let mut c = super::Crc32::new();
        c.update(b"1234");
        c.update(b"56789");
        assert_eq!(c.get(), 0xCBF4_3926);
        assert_eq!(super::crc16(b"123456789"), 0x29B1);
    }
}
