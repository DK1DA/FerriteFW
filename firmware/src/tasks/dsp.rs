//! DSP: audio blocks from ADC1 (PA2, the BK4829's EARO) into the one decoder slot, and DAC1 out (beeps, AFSK).
//! Runs on the second interrupt executor, below Radio.
//!
//! The HAL has no ADC/DAC DMA yet (milestone 2 brings it, with the baseband check that decides each decoder's
//! audio source); until then the slot is switched but fed nothing.

use crate::chan::DSP;
use ferrite_core::decoders::Decoder;
use ferrite_core::msg::{DecoderKind, DspCmd};

#[embassy_executor::task]
pub async fn task() {
    // The slot lives in this task's future, which is a static: one decoder's memory, whichever is running.
    let mut slot = Decoder::Off;
    loop {
        match DSP.receive().await {
            DspCmd::Decoder(kind) => {
                slot = match kind {
                    DecoderKind::Off => Decoder::Off,
                    DecoderKind::Afsk => Decoder::afsk(),
                    DecoderKind::Pocsag => Decoder::pocsag(),
                    DecoderKind::Dsc => Decoder::dsc(),
                };
            }
            DspCmd::Beep { .. } => {} // DAC1 tone: with the DAC DMA
        }
        let _ = &mut slot;
    }
}
