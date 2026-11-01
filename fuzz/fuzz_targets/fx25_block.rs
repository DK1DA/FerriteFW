//! FX.25 correction and deframing of a received block (ufx25::decode, the APRS task's Fx25Block::decode path):
//! any bytes, any of the eleven codes, any tag error count. Input: one byte picking the code, then the block (padded
//! or cut to the code's length).
#![no_main]
use libfuzzer_sys::fuzz_target;
use ufx25::{decode, rs::Rs, MODES};

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else { return };
    let mode = MODES[sel as usize % MODES.len()];
    let n = mode.n as usize;
    let mut block = [0u8; 255];
    let m = rest.len().min(n);
    block[..m].copy_from_slice(&rest[..m]);
    let mut b = block;
    if let Some(d) = decode(&mut b[..n], mode, sel >> 4) {
        assert!(d.frame.len() + 2 <= mode.k as usize);
        assert!(d.corrected as usize <= mode.check() / 2);
        let _ = uaprs::ax25::Frame::parse(d.frame);
    }
    // the Reed-Solomon decoder on its own, and a correction must leave a valid code word
    let rs = Rs::new(mode.check());
    let mut c = block;
    if rs.decode(&mut c[..n], mode.k as usize).is_ok() {
        let mut again = c;
        assert_eq!(rs.decode(&mut again[..n], mode.k as usize).ok(), Some(0), "a corrected block has no errors left");
    }
});
