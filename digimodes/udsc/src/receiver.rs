//! Audio in, calls out: uafsk's FSK front end at DSC's tones, then a [`Framer`] per slicer. A call decoded by more
//! than one slicer is reported once.

use crate::framer::{Call, Framer};
use uafsk::fsk::{Config, Fsk, Tones};

/// VHF DSC: 1300 Hz (binary 1) and 2100 Hz (binary 0) at 1200 baud.
pub const DSC_VHF: Tones = Tones { mark_hz: 1300, space_hz: 2100, baud: 1200 };

/// Audio (9600 to 48000 samples/s, from the receiver's speaker path or discriminator) in, calls out.
/// `S` slicers: the default 4 includes one on raw amplitudes, which is the best of them in white noise.
pub struct Receiver<const S: usize = 4> {
    fsk: Fsk<S, 1>,
    framers: [Framer; S],
    /// Length, ECC-ish fingerprint and age of the last call reported.
    last: Option<(u32, u32)>,
    age: u32,
}

impl<const S: usize> Receiver<S> {
    pub fn new(cfg: Config) -> Self {
        Receiver { fsk: Fsk::new(cfg, DSC_VHF), framers: core::array::from_fn(|_| Framer::new()), last: None, age: 0 }
    }

    /// Feed one sample; `found` gets each call once.
    pub fn push(&mut self, x: i16, mut found: impl FnMut(&Call)) {
        self.age = self.age.wrapping_add(1);
        let Receiver { fsk, framers, last, age } = self;
        fsk.push(x, |k, mark| {
            if let Some(call) = framers[k].push(mark) {
                let key = call.symbols.iter().fold(call.symbols.len() as u32, |h, &v| h.wrapping_mul(131).wrapping_add(v as u32));
                // The same call from another slicer arrives within a few bits.
                if !matches!(*last, Some((k0, at)) if k0 == key && age.wrapping_sub(at) < 4800) {
                    *last = Some((key, *age));
                    found(&call);
                }
            }
        });
    }

    /// Carrier detect: DSC data on the channel now.
    pub fn busy(&self) -> bool {
        self.fsk.busy()
    }
}
