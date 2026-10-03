//! Storage: the only task that touches the SPI flash. The w25q driver is given only the ranges Ferrite may write
//! (ferrite_core::layout::WRITABLE), so calibration, stock's data and the bootloader's update area are refused
//! below everything else.

use crate::bus::{BusyDelay, Flash};
use crate::chan::STORAGE;
use crate::tasks::{check, CHECK_FLASH};
use ferrite_core::layout::WRITABLE;
use ferrite_core::msg::{Source, StorageCmd};
use w25q::W25q;

#[embassy_executor::task]
pub async fn task() {
    let flash = W25q::new(Flash::new(), &mut BusyDelay, &WRITABLE);
    check(Source::Storage, CHECK_FLASH, flash.is_ok());
    let Ok(_flash) = flash else {
        // No flash: settings stay at their defaults for this power-on.
        loop {
            STORAGE.receive().await;
        }
    };
    loop {
        match STORAGE.receive().await {
            StorageCmd::SaveSettings => {} // settings records: milestone 2
            StorageCmd::Log(_) => {}       // event log ring: milestone 6
        }
    }
}
