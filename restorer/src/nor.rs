//! The W25Q for the restore (ferrite-store's [`Nor`]): reads, page programs and sector erases, each program and
//! erase only inside [`WRITABLE`] (the firmware's own guard, `ferrite_radio::flash`, SAFE-1 and SAFE-2), so
//! calibration, the bootloader's fonts and its update area can't be written whatever the PC sends. As the
//! firmware's `store::Chip` without the suspend: nothing here reads while a program or erase runs (the restore
//! waits for each, ferrite_store::restore).
use crate::bus::{BusyDelay, Flash};
use ferrite_core::layout::WRITABLE;
use ferrite_store::{Nor, NorError, Result};
use uradio_bus::Spi;
use w25q::W25q;

const READ: u8 = 0x03;
const PAGE_PROGRAM: u8 = 0x02;
const SECTOR_ERASE: u8 = 0x20;
const WRITE_ENABLE: u8 = 0x06;
const READ_STATUS1: u8 = 0x05;

/// Where programs and erases may go: Ferrite's own writable ranges, as the firmware's driver.
pub const WRITE_OK: &[core::ops::Range<u32>] = &WRITABLE;

pub struct Chip {
    spi: Option<Flash>,
    size: u32,
}

impl Chip {
    /// Wakes the chip and reads its size (0, and nothing works, when it doesn't answer).
    pub fn new() -> Self {
        match W25q::new(Flash::new(), &mut BusyDelay, WRITE_OK) {
            Ok(f) => {
                let size = f.size();
                Chip { spi: Some(f.release()), size }
            }
            Err(_) => Chip { spi: None, size: 0 },
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    fn command(spi: &mut Flash, op: u8) {
        spi.select(true);
        spi.transfer(op);
        spi.select(false);
    }

    fn command_addr(spi: &mut Flash, op: u8, addr: u32) {
        spi.write(&[op, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]);
    }

    fn start(&mut self, op: u8, addr: u32, data: &[u8]) -> Result {
        let spi = self.spi.as_mut().ok_or(NorError)?;
        Self::command(spi, WRITE_ENABLE);
        spi.select(true);
        Self::command_addr(spi, op, addr);
        spi.write(data);
        spi.select(false);
        Ok(())
    }
}

impl Nor for Chip {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result {
        if addr.checked_add(buf.len() as u32).is_none_or(|e| e > self.size) {
            return Err(NorError);
        }
        let spi = self.spi.as_mut().ok_or(NorError)?;
        spi.select(true);
        Self::command_addr(spi, READ, addr);
        spi.read(buf);
        spi.select(false);
        Ok(())
    }

    fn start_program(&mut self, addr: u32, data: &[u8]) -> Result {
        if !ferrite_radio::flash::program_ok(WRITE_OK, addr, data.len() as u32) {
            return Err(NorError);
        }
        self.start(PAGE_PROGRAM, addr, data)
    }

    fn start_erase(&mut self, addr: u32) -> Result {
        if !ferrite_radio::flash::erase_ok(WRITE_OK, addr) {
            return Err(NorError);
        }
        self.start(SECTOR_ERASE, addr, &[])
    }

    fn busy(&mut self) -> Result<bool> {
        let spi = self.spi.as_mut().ok_or(NorError)?;
        spi.select(true);
        spi.transfer(READ_STATUS1);
        let s = spi.transfer(0);
        spi.select(false);
        Ok(s & 1 != 0)
    }

    /// A busy wait: the restorer has one thread and nothing else to run.
    async fn pause() {
        crate::hal::time::delay_ms_busy(1);
    }
}
