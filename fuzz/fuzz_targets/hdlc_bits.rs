//! The HDLC deframer (flags, bit stuffing, FCS, frame length) bit by bit, plain and with soft-decision repair. Each
//! input byte gives one bit (bit 0) and a confidence (the other 7 bits); the first byte sets the repair depth.
#![no_main]
use libfuzzer_sys::fuzz_target;
use uafsk::hdlc::{ax25_addresses_ok, Deframer, MAX_FRAME};

fuzz_target!(|data: &[u8]| {
    let Some((&k, rest)) = data.split_first() else { return };
    let mut plain = Deframer::new();
    let mut soft = Deframer::new();
    soft.set_repair(k & 7);
    for &b in rest {
        let bit = b & 1;
        if let Some(f) = plain.push(bit) {
            assert!(f.len() <= MAX_FRAME);
            let _ = ax25_addresses_ok(f);
            let _ = uaprs::ax25::Frame::parse(f);
        }
        if let Some((f, _fcs)) = soft.push_soft(bit, (b >> 1) as u32 * 1000) {
            assert!(f.len() <= MAX_FRAME);
            let _ = ax25_addresses_ok(f);
            let _ = soft.repaired();
        }
    }
});
