//! The KISS decoder (cable and Bluetooth TNC), byte by byte, and the parameter commands it carries; data frames go
//! on to the AX.25 parser as the APRS task's KISS path does.
#![no_main]
use libfuzzer_sys::fuzz_target;
use uaprs::ax25::Frame;
use uaprs::kiss::{encode, Command, Decoder, Params};

fuzz_target!(|data: &[u8]| {
    let mut d: Decoder = Decoder::new();
    let mut p = Params::default();
    for &b in data {
        if let Some(r) = d.push(b) {
            let _ = r.for_single_port();
            if let Command::Data(f) = p.apply(&r) {
                let _ = Frame::parse(f);
                // a frame encodes and decodes back to itself
                let mut e = Vec::new();
                encode(0, f, |c| e.push(c));
                let mut d2: Decoder = Decoder::new();
                let mut got = None;
                for &c in &e {
                    if let Some(r2) = d2.push(c) {
                        got = Some(r2.data.to_vec());
                    }
                }
                if !f.is_empty() {
                    assert_eq!(got.as_deref(), Some(f));
                }
            }
            let _ = p.flags(1200);
        }
    }
});
