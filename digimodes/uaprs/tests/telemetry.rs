//! Telemetry built by `build::telemetry` and `build::telemetry_def`, against the APRS 1.0.1 chapter 13 examples,
//! and read back by our own parser (Ferrite's APRS-54).

use uaprs::build::{self, TelemDef};
use uaprs::parse::{parse, MessageKind, Packet};

fn report(seq: u16, analog: &[u8; 5], bits: u8) -> Option<Vec<u8>> {
    let mut out = [0u8; 64];
    build::telemetry(seq, analog, bits, &mut out).map(|n| out[..n].to_vec())
}

fn def(to: &[u8], d: TelemDef, fields: &[&[u8]]) -> Option<Vec<u8>> {
    let mut out = [0u8; 100];
    build::telemetry_def(to, d, fields, &mut out).map(|n| out[..n].to_vec())
}

/// APRS-54.1: the report as the spec's example, `T#005,199,000,255,073,123,01101001`; three digits each, B1 first.
#[test]
fn report_as_the_spec() {
    let r = report(5, &[199, 0, 255, 73, 123], 0b0110_1001).unwrap();
    assert_eq!(r, b"T#005,199,000,255,073,123,01101001");
    assert_eq!(parse(b"APZFER", &r), Ok(Packet::Telemetry(b"005,199,000,255,073,123,01101001")));
    assert_eq!(report(999, &[0; 5], 0).unwrap(), b"T#999,000,000,000,000,000,00000000");
    assert_eq!(report(1000, &[0; 5], 0), None, "the sequence is three digits");
    let mut small = [0u8; 20];
    assert_eq!(build::telemetry(1, &[0; 5], 0, &mut small), None, "a buffer too small");
}

/// APRS-54.4: the four definitions as the spec's examples, messages to the station itself (padded to 9).
#[test]
fn definitions_as_the_spec() {
    let to: &[u8] = b"N0QBF-11";
    let parm: [&[u8]; 10] = [b"Battery", b"Btemp", b"ATemp", b"Pres", b"Alt", b"Camra", b"Chut", b"Sun", b"10m", b"ATV"];
    assert_eq!(def(to, TelemDef::Parm, &parm).unwrap(), b":N0QBF-11 :PARM.Battery,Btemp,ATemp,Pres,Alt,Camra,Chut,Sun,10m,ATV");
    let unit: [&[u8]; 10] = [b"v/100", b"deg.F", b"deg.F", b"Mbar", b"Kft", b"Click", b"OPEN", b"on", b"on", b"hi"];
    assert_eq!(def(to, TelemDef::Unit, &unit).unwrap(), b":N0QBF-11 :UNIT.v/100,deg.F,deg.F,Mbar,Kft,Click,OPEN,on,on,hi");
    let eqns: [&[u8]; 15] = [b"0", b"5.2", b"0", b"0", b".53", b"-32", b"3", b"4.39", b"49", b"-32", b"3", b"18", b"1", b"2", b"3"];
    assert_eq!(def(to, TelemDef::Eqns, &eqns).unwrap(), b":N0QBF-11 :EQNS.0,5.2,0,0,.53,-32,3,4.39,49,-32,3,18,1,2,3");
    let bits = def(to, TelemDef::Bits, &[b"10110000", b"N0QBF's Big Balloon"]).unwrap();
    assert_eq!(bits, b":N0QBF-11 :BITS.10110000,N0QBF's Big Balloon");
    let Ok(Packet::Message(m)) = parse(b"APZFER", &bits) else { panic!("a message") };
    assert_eq!((m.addressee, m.kind, m.text, m.id), (&b"N0QBF-11"[..], MessageKind::Message, &b"BITS.10110000,N0QBF's Big Balloon"[..], None));
}

/// A definition longer than a message's 67 characters, or with a character messages can't carry, isn't built.
#[test]
fn definitions_refused() {
    let long = [b"ABCDEFGHIJ".as_slice(); 7];
    assert_eq!(def(b"N0QBF", TelemDef::Parm, &long), None);
    assert_eq!(def(b"N0QBF", TelemDef::Parm, &[b"a|b"]), None);
    assert_eq!(def(b"", TelemDef::Parm, &[b"a"]), None);
}
