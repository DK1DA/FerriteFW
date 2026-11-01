//! Satellite mode's data on the radio (chapter 28a): the satellites from the store's SPI flash region (SAT-2,
//! written by Console over the link or by the terminal's `:sat`; `ferrite_store::sats`) and their passes: predicted
//! here with `sat-predict` (SGP4 and the pass predictor, SAT-4, SAT-6), else Console's interpolated tracks (none
//! yet).

use crate::store::Store;
use ferrite_core::sat::data::SatData;
use ferrite_store::sats::StoreSats;

/// The App's satellite data, once at start-up, on the store (a singleton: the store is only read through it).
#[allow(clippy::mut_from_ref)]
pub fn data(store: &'static Store) -> &'static mut dyn SatData {
    let sats = StoreSats(store);
    #[cfg(feature = "sat-predict")]
    {
        use ferrite_core::sat::data::Live;
        cortex_m::singleton!(: Live<StoreSats<'static, crate::store::Chip, ferrite_app::radio_data::RadioData>> = Live::new(sats)).unwrap()
    }
    #[cfg(not(feature = "sat-predict"))]
    {
        use ferrite_core::sat::track::{NoTracks, Tracks};
        cortex_m::singleton!(: Tracks<StoreSats<'static, crate::store::Chip, ferrite_app::radio_data::RadioData>, NoTracks> = Tracks::new(sats, NoTracks)).unwrap()
    }
}
