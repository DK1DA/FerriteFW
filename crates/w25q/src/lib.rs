//! # w25q
//!
//! A driver for W25Q-family SPI NOR flash (the RT-950 Pro has 4 MB). `no_std`, no allocation.
//!
//! - The size comes from the JEDEC ID; addresses past the end are refused.
//! - **Writes only where allowed**: program and erase take effect only inside the ranges the board gives
//!   ([`W25q::new`]). On the radio that keeps factory calibration, the bootloader's area and stock's data out of
//!   reach, whatever the caller asks.
//! - **Nothing blocks for long**: a sector erase takes up to 400 ms, so [`W25q::start_erase`] returns at once and
//!   the caller polls [`W25q::busy`] (from an async timer, say). Page programs (under 3 ms) can wait in place.

#![no_std]

use core::ops::Range;
use uradio_bus::{Delay, Spi};

const READ: u8 = 0x03;
const PAGE_PROGRAM: u8 = 0x02;
const SECTOR_ERASE: u8 = 0x20;
const WRITE_ENABLE: u8 = 0x06;
const READ_STATUS1: u8 = 0x05;
const JEDEC_ID: u8 = 0x9F;
const UNIQUE_ID: u8 = 0x4B;
const POWER_DOWN: u8 = 0xB9;
const RELEASE_POWER_DOWN: u8 = 0xAB;

/// Page and sector sizes.
pub const PAGE: u32 = 256;
pub const SECTOR: u32 = 4096;

/// Why a request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Past the end of the chip.
    OutOfRange,
    /// Not inside a writable range.
    Protected,
    /// An erase address not on a sector boundary.
    Unaligned,
    /// The chip stayed busy longer than its datasheet maximum.
    Timeout,
    /// The JEDEC ID is not a W25Q-like part (no plausible size).
    UnknownChip,
}

/// The flash. `writable`: the address ranges program and erase may touch (sector-aligned on the radio).
pub struct W25q<S: Spi> {
    spi: S,
    size: u32,
    writable: &'static [Range<u32>],
}

impl<S: Spi> W25q<S> {
    /// Wakes the chip (it may be in deep power-down), reads its size from the JEDEC ID.
    pub fn new(mut spi: S, delay: &mut impl Delay, writable: &'static [Range<u32>]) -> Result<Self, Error> {
        spi.select(true);
        spi.transfer(RELEASE_POWER_DOWN);
        spi.select(false);
        delay.delay_us(50); // tRES1: 3 us; generous
        let mut f = W25q { spi, size: 0, writable };
        let id = f.jedec_id();
        // Capacity byte: 2^n bytes (0x15 = 2 MB, 0x16 = 4 MB, 0x17 = 8 MB, ...).
        let cap = id & 0xFF;
        if !(0x10..=0x19).contains(&cap) {
            return Err(Error::UnknownChip);
        }
        f.size = 1 << cap;
        Ok(f)
    }

    /// Bytes on the chip.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Manufacturer, memory type and capacity: 0xEF4016 for a 4 MB Winbond W25Q32.
    pub fn jedec_id(&mut self) -> u32 {
        let mut id = [0u8; 3];
        self.spi.select(true);
        self.spi.transfer(JEDEC_ID);
        self.spi.read(&mut id);
        self.spi.select(false);
        (id[0] as u32) << 16 | (id[1] as u32) << 8 | id[2] as u32
    }

    /// The chip's 64-bit unique ID.
    pub fn unique_id(&mut self) -> [u8; 8] {
        let mut id = [0u8; 8];
        self.spi.select(true);
        self.spi.write(&[UNIQUE_ID, 0, 0, 0, 0]);
        self.spi.read(&mut id);
        self.spi.select(false);
        id
    }

    fn command_addr(&mut self, op: u8, addr: u32) {
        self.spi.write(&[op, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]);
    }

    /// Reads `buf.len()` bytes from `addr`.
    pub fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.check(addr, buf.len() as u32)?;
        self.spi.select(true);
        self.command_addr(READ, addr);
        self.spi.read(buf);
        self.spi.select(false);
        Ok(())
    }

    fn check(&self, addr: u32, len: u32) -> Result<(), Error> {
        match addr.checked_add(len) {
            Some(end) if end <= self.size => Ok(()),
            _ => Err(Error::OutOfRange),
        }
    }

    fn check_writable(&self, addr: u32, len: u32) -> Result<(), Error> {
        self.check(addr, len)?;
        let end = addr + len;
        if self.writable.iter().any(|r| r.start <= addr && end <= r.end) {
            Ok(())
        } else {
            Err(Error::Protected)
        }
    }

    /// Status register 1, bit 0: an erase or program is running.
    pub fn busy(&mut self) -> bool {
        self.spi.select(true);
        self.spi.transfer(READ_STATUS1);
        let s = self.spi.transfer(0);
        self.spi.select(false);
        s & 1 != 0
    }

    fn write_enable(&mut self) {
        self.spi.select(true);
        self.spi.transfer(WRITE_ENABLE);
        self.spi.select(false);
    }

    /// Programs `data` at `addr` (bits only go from 1 to 0: erase first), split at page boundaries, waiting for
    /// each page (up to 3 ms).
    pub fn program(&mut self, addr: u32, data: &[u8], delay: &mut impl Delay) -> Result<(), Error> {
        self.check_writable(addr, data.len() as u32)?;
        let mut a = addr;
        let mut rest = data;
        while !rest.is_empty() {
            let n = ((PAGE - a % PAGE) as usize).min(rest.len());
            self.write_enable();
            self.spi.select(true);
            self.command_addr(PAGE_PROGRAM, a);
            self.spi.write(&rest[..n]);
            self.spi.select(false);
            self.wait(delay, 3_000)?;
            a += n as u32;
            rest = &rest[n..];
        }
        Ok(())
    }

    /// Starts erasing the 4 KB sector at `addr` and returns; poll [`W25q::busy`] (up to 400 ms).
    pub fn start_erase(&mut self, addr: u32) -> Result<(), Error> {
        if !addr.is_multiple_of(SECTOR) {
            return Err(Error::Unaligned);
        }
        self.check_writable(addr, SECTOR)?;
        self.write_enable();
        self.spi.select(true);
        self.command_addr(SECTOR_ERASE, addr);
        self.spi.select(false);
        Ok(())
    }

    /// Erases a sector and waits for it (blocking: for tools and tests, not the radio's tasks).
    pub fn erase(&mut self, addr: u32, delay: &mut impl Delay) -> Result<(), Error> {
        self.start_erase(addr)?;
        self.wait(delay, 400_000)
    }

    fn wait(&mut self, delay: &mut impl Delay, max_us: u32) -> Result<(), Error> {
        let mut t = 0;
        while self.busy() {
            if t >= max_us {
                return Err(Error::Timeout);
            }
            delay.delay_us(50);
            t += 50;
        }
        Ok(())
    }

    /// Deep power-down (about 1 uA); any later command wakes it through [`W25q::new`]'s release, or call
    /// [`W25q::wake`].
    pub fn power_down(&mut self) {
        self.spi.select(true);
        self.spi.transfer(POWER_DOWN);
        self.spi.select(false);
    }

    pub fn wake(&mut self, delay: &mut impl Delay) {
        self.spi.select(true);
        self.spi.transfer(RELEASE_POWER_DOWN);
        self.spi.select(false);
        delay.delay_us(50);
    }

    /// The bus back.
    pub fn release(self) -> S {
        self.spi
    }
}
