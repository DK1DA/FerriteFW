//! The SPI flash, read only (BACKUP-9.5, decision 264). Two locks, both at build time:
//! - w25q is built without its `write` feature (reader/Cargo.toml): page program, sector erase and the write
//!   enable they need are not compiled, opcodes included. The assert below stops the build if feature
//!   unification ever turns it back on.
//! - [`ReadOnly`] owns the driver and the bus and hands out reads, the IDs and status register 1 (a read) only;
//!   nothing here gives the raw bus back.
use crate::bus::{BusyDelay, Flash};
use ferrite_core::chipid;
use w25q::W25q;

const _: () = assert!(!w25q::WRITES, "the reader must be built without w25q's write feature");

pub struct ReadOnly {
    chip: Option<W25q<Flash>>,
}

impl ReadOnly {
    pub fn new() -> Self {
        let mut f = ReadOnly { chip: None };
        f.probe();
        f
    }

    /// Wakes the chip and reads its ID again if it didn't answer before.
    pub fn probe(&mut self) {
        if self.chip.is_none() {
            // No writable ranges: the driver couldn't write even with the feature (it has none here).
            self.chip = W25q::new(Flash::new(), &mut BusyDelay, &[]).ok();
        }
    }

    pub fn jedec(&mut self) -> u32 {
        self.chip.as_mut().map_or(0, |c| c.jedec_id())
    }

    /// The chip's size as hwtest gave it (`chipid::flash_bytes`): 0 when unknown.
    pub fn bytes(&mut self) -> u32 {
        chipid::flash_bytes(self.jedec())
    }

    pub fn unique_id(&mut self) -> [u8; 8] {
        self.chip.as_mut().map_or([0; 8], |c| c.unique_id())
    }

    pub fn status(&mut self) -> u8 {
        self.chip.as_mut().map_or(0, |c| c.status())
    }

    /// Fills `buf` from `addr`; 0xFF where the chip can't be read (no chip, past its end).
    pub fn read(&mut self, addr: u32, buf: &mut [u8]) {
        if !self.chip.as_mut().is_some_and(|c| c.read(addr, buf).is_ok()) {
            buf.fill(0xFF);
        }
    }
}
