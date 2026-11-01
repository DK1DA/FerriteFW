//! Satellite mode's data on the radio (chapter 28a), phase 1: the satellites (the bench builds' sample from
//! data.ferritefw.dev's fixture; none in a user build until the SPI flash region and Console's transfer come, SAT-2,
//! SAT-15) and their passes: predicted here with `sat-predict` (SGP4 and the pass predictor, SAT-4, SAT-6), else
//! Console's interpolated tracks (none yet).

use ferrite_core::sat::data::SatData;
use ferrite_core::sat::{Sat, SatSource};

/// The satellites the radio holds.
pub struct RadioSats {
    #[cfg(feature = "bench")]
    list: [Sat; 4],
}

impl SatSource for RadioSats {
    fn count(&self) -> u8 {
        #[cfg(feature = "bench")]
        return self.list.len() as u8;
        #[cfg(not(feature = "bench"))]
        0
    }
    fn get(&self, i: u8) -> Option<Sat> {
        #[cfg(feature = "bench")]
        return self.list.get(i as usize).copied();
        #[cfg(not(feature = "bench"))]
        {
            let _ = i;
            None
        }
    }
}

fn sats() -> RadioSats {
    RadioSats {
        #[cfg(feature = "bench")]
        list: ferrite_app::sat::sample(),
    }
}

/// The App's satellite data, once at start-up.
pub fn data() -> &'static mut dyn SatData {
    #[cfg(feature = "sat-predict")]
    {
        use ferrite_core::sat::data::Live;
        cortex_m::singleton!(: Live<RadioSats> = Live::new(sats())).unwrap()
    }
    #[cfg(not(feature = "sat-predict"))]
    {
        use ferrite_core::sat::track::{NoTracks, Tracks};
        cortex_m::singleton!(: Tracks<RadioSats, NoTracks> = Tracks::new(sats(), NoTracks)).unwrap()
    }
}
