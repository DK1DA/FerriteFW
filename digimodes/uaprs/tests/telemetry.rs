//! Other stations' telemetry as the parser reads it, against the APRS 1.0.1 chapter 13 examples. Ferrite sends
//! none of its own (its values go in the position comment, APRS-54).

use uaprs::parse::{parse, MessageKind, Packet};

/// A report, `T#005,199,000,255,073,123,01101001`: what follows the `T#`.
#[test]
fn report_as_the_spec() {
    assert_eq!(parse(b"APRS", b"T#005,199,000,255,073,123,01101001"), Ok(Packet::Telemetry(b"005,199,000,255,073,123,01101001")));
}

/// A definition is a message to the reporting station itself (its addressee padded to 9).
#[test]
fn definitions_are_messages() {
    let Ok(Packet::Message(m)) = parse(b"APRS", b":N0QBF-11 :BITS.10110000,N0QBF's Big Balloon") else { panic!("a message") };
    assert_eq!((m.addressee, m.kind, m.text, m.id), (&b"N0QBF-11"[..], MessageKind::Message, &b"BITS.10110000,N0QBF's Big Balloon"[..], None));
}
