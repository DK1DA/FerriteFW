//! The radio's AFSK decoder as the DSP task runs it (ferrite_core::decoders::Decoder::afsk at 24 kHz: three
//! slicers, repair, FX.25), on any audio: input bytes are little-endian 16-bit samples, fed in blocks. FX.25 blocks
//! it hands over are corrected as the APRS task does.
#![no_main]
use ferrite_core::decoders::{Decoder, Found};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let samples: Vec<i16> = data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let mut d = Decoder::afsk();
    for block in samples.chunks(96) {
        d.push(block, |f| match f {
            Found::Frame(f) => {
                assert!(f.len() <= uafsk::hdlc::MAX_FRAME);
                let _ = uaprs::ax25::Frame::parse(f);
            }
            Found::Fx25(b) => {
                let mut b = *b;
                if let Some(fr) = b.decode() {
                    let _ = uaprs::ax25::Frame::parse(fr.frame);
                }
            }
            _ => {}
        });
    }
});
