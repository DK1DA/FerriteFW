//! The FX.25 receiver bit by bit (tag hunter, collector, correction, deframing): each input byte gives 8 bits, least
//! significant first. The first byte sets the tag errors accepted (0 to 15).
#![no_main]
use libfuzzer_sys::fuzz_target;
use ufx25::Receiver;

fuzz_target!(|data: &[u8]| {
    let Some((&t, rest)) = data.split_first() else { return };
    let mut rx = Receiver::with_tag_errors(t & 15);
    for &byte in rest {
        for i in 0..8 {
            if let Some(d) = rx.push((byte >> i) & 1) {
                assert!(d.frame.len() <= 255);
                let _ = uaprs::ax25::Frame::parse(d.frame);
            }
            let _ = rx.busy();
        }
    }
    let _ = rx.block();
});
