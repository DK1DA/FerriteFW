//! The SPI-flash store (ferrite-store) on the radio's W25Q: the channels, zones and range presets the UI reads,
//! and the settings and slots it saves (roadmap 1d). The UI task owns it: it mounts it before the boot screen
//! (the callsign and owner card are in the settings), reads channel records on demand, applies the app's
//! channel writes and saves the settings 2 s after a change and at power off.
//!
//! The w25q driver is given only the ranges Ferrite may write (ferrite_core::layout::WRITABLE), so calibration,
//! stock's data and the bootloader's update area are refused below everything else.
//!
//! For now a sector erase waits in place (up to 400 ms, usually about 50): a channel edit can hold the UI that
//! long. Polling the erase from a timer is the follow-up once the event log needs the flash too.

use crate::bus::{BusyDelay, Flash};
use ferrite_app::data::NoData;
use ferrite_core::layout::WRITABLE;
use ferrite_store::{FlashStore, Nor, NorError};
use w25q::W25q;

/// The W25Q, or nothing when it didn't answer at power on (then nothing is stored this power-on).
pub enum Spi {
    Chip(W25q<Flash>),
    Missing,
}

impl Nor for Spi {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> ferrite_store::Result {
        match self {
            Spi::Chip(f) => f.read(addr, buf).map_err(|_| NorError),
            Spi::Missing => Err(NorError),
        }
    }
    fn program(&mut self, addr: u32, data: &[u8]) -> ferrite_store::Result {
        match self {
            Spi::Chip(f) => f.program(addr, data, &mut BusyDelay).map_err(|_| NorError),
            Spi::Missing => Err(NorError),
        }
    }
    fn erase(&mut self, addr: u32) -> ferrite_store::Result {
        match self {
            Spi::Chip(f) => f.erase(addr, &mut BusyDelay).map_err(|_| NorError),
            Spi::Missing => Err(NorError),
        }
    }
}

pub type Store = FlashStore<Spi, NoData>;

/// Starts the flash and mounts the store: whether the chip answered (the self-test's SPI flash check), and the
/// store.
pub fn mount() -> (bool, Store) {
    let spi = match W25q::new(Flash::new(), &mut BusyDelay, &WRITABLE) {
        Ok(f) => Spi::Chip(f),
        Err(_) => Spi::Missing,
    };
    let ok = matches!(spi, Spi::Chip(_));
    (ok, FlashStore::mount(spi, NoData))
}

/// The radio-wide settings the UI borrows for good (`App::new`): the defaults, then what the store holds
/// ([`ferrite_store::FlashStore::load_settings`]). Once only.
pub fn settings() -> &'static mut ferrite_app::settings::Settings {
    cortex_m::singleton!(: ferrite_app::settings::Settings = ferrite_app::settings::Settings::DEFAULT).unwrap()
}

/// The last alert, for the alert screen (WX-5): the App's too, kept outside it. Once only.
pub fn last_alert() -> &'static mut ferrite_app::screens::alert::AlertInfo {
    cortex_m::singleton!(: ferrite_app::screens::alert::AlertInfo = ferrite_app::screens::alert::AlertInfo::NONE).unwrap()
}
