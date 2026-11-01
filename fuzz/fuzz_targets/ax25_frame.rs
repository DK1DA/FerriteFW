//! AX.25 frame decode (address field, control and PID, information field), the radio's TNC2 text of it, and the
//! TNC2 splitter and callsign reader. A parsed frame must write back to the same bytes, apart from the H bits of the
//! destination and source (which `Frame::parse` clears) and the reserved bits.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::fmt::Write;
use uaprs::ax25::{Address, Frame, Tnc2};

fuzz_target!(|data: &[u8]| {
    if let Some(f) = Frame::parse(data) {
        assert!(f.digis().len() <= uaprs::ax25::MAX_DIGIS);
        let mut out = [0u8; 400];
        let mut big = vec![0u8; f.len()];
        let n = f.write(&mut big).expect("room for the frame");
        assert_eq!(n, data.len());
        // control, PID and info as received (address bytes may differ in their reserved and H bits)
        let at = 14 + 7 * f.digis().len();
        assert_eq!(&big[at..], &data[at..], "control, PID and info round trip");
        let _ = f.write(&mut out);
        let mut t = Vec::new();
        f.emit_tnc2(|c| t.push(c));
        let mut s = String::new();
        let _ = write!(s, "{f}");
        assert_eq!(s.as_bytes(), &t[..], "Display and emit_tnc2 agree");
        let _ = Frame::new(f.src, f.dest, f.digis(), f.info);
        for d in f.digis() {
            let _ = d.call();
            let mut e = Vec::new();
            d.emit(&mut |c| e.push(c));
            let back = Address::from_text(&e).expect("a parsed address reads back as text");
            assert!(back.call().eq_ignore_ascii_case(d.call()) && back.ssid == d.ssid);
        }
    }
    if let Some(t) = Tnc2::split(data) {
        let _ = Address::from_text(t.src);
        let _ = Address::from_text(t.dest);
        for p in t.path.split(|&c| c == b',') {
            let _ = Address::from_text(p);
        }
    }
    let _ = Address::from_text(data);
    let _ = Address::from_field(data);
});
