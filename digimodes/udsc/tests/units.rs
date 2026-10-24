//! Codewords, framing with damaged copies, and message fields.

use udsc::framer::{codeword, encode, Framer};
use udsc::message::{Address, Message};

/// The bits of a call as transmitted: dot pattern, phasing, DX/RX interleaved (as ITU-R M.493 and dsc_generator).
fn bits(call: &[u8]) -> Vec<bool> {
    let ph = [125u8, 111, 125, 110, 125, 109, 125, 108, 125, 107, 125, 106, 125, 105, 125, 104];
    let mut seq: Vec<u8> = ph[..12].to_vec();
    seq.extend([call[0], ph[13], call[1], ph[15], call[2]]);
    for i in 0..call.len() - 3 {
        seq.push(call[i]);
        seq.push(call[i + 3]);
    }
    seq.push(call[call.len() - 3]);
    let mut b: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
    for v in seq {
        let w = encode(v);
        b.extend((0..10).rev().map(|k| (w >> k) & 1 == 1));
    }
    b.extend([false; 40]);
    b
}

/// A call's symbols with ECC and closing EOS added.
fn call(body: &[u8]) -> Vec<u8> {
    let n = body.len();
    let ecc = body[2..].iter().fold(body[0], |x, &v| x ^ v);
    let mut c = body.to_vec();
    c.extend([ecc, body[n - 1], body[n - 1]]);
    c
}

fn run(b: &[bool]) -> Vec<(Vec<u8>, bool, u8)> {
    let mut f = Framer::new();
    let mut out = Vec::new();
    for &x in b {
        if let Some(c) = f.push(x) {
            out.push((c.symbols.to_vec(), c.ecc_ok, c.repaired));
        }
    }
    out
}

const ROUTINE: [u8; 19] = [120, 120, 36, 61, 23, 45, 0, 100, 23, 50, 12, 34, 0, 100, 126, 90, 0, 72, 117];

#[test]
fn codewords_match_the_itu_table() {
    // From dsc_generator's table (ITU-R M.493 table 1): 0 = 0000000111, 1 = 1000000110, 125 = 1011111001.
    for (v, bits) in [(0u8, "0000000111"), (1, "1000000110"), (100, "0010011100"), (125, "1011111001"), (127, "1111111000")] {
        assert_eq!(encode(v), u32::from_str_radix(bits, 2).unwrap(), "{v}");
    }
    for v in 0..128u8 {
        assert_eq!(codeword(encode(v)), Some(v));
        for k in 0..10 {
            assert_eq!(codeword(encode(v) ^ 1 << k), None, "a single error is always detected");
        }
    }
}

#[test]
fn clean_call() {
    let c = call(&ROUTINE);
    assert_eq!(run(&bits(&c)), vec![(ROUTINE.to_vec(), true, 0)]);
}

#[test]
fn one_copy_of_every_symbol_damaged() {
    // Flip a bit in the DX copy of even symbols and the RX copy of odd ones: every symbol has one good copy.
    let c = call(&ROUTINE);
    let mut b = bits(&c);
    let first_dx = 40 + 12 * 10; // after dots and the 12 phasing codewords
    for i in 0..ROUTINE.len() {
        let (dx, rx) = (first_dx + 20 * i, first_dx + 20 * i + 50); // the RX copy is five codewords later
        let at = if i % 2 == 0 { dx } else { rx } + (i % 10);
        if at < b.len() {
            b[at] = !b[at];
        }
    }
    let got = run(&b);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, ROUTINE.to_vec());
    assert!(got[0].1);
}

#[test]
fn a_symbol_lost_in_both_copies_is_rebuilt() {
    let c = call(&ROUTINE);
    let mut b = bits(&c);
    // Symbol 9 (an MMSI digit pair): DX at codeword 12 + 2*9, RX five codewords later; two bits each.
    let dx = 40 + (12 + 2 * 9) * 10;
    for at in [dx + 1, dx + 4, dx + 50 + 2, dx + 50 + 6] {
        b[at] = !b[at];
    }
    let got = run(&b);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, ROUTINE.to_vec());
    assert!(got[0].1);
}

#[test]
fn noise_alone_finds_nothing() {
    let mut s = 1u64;
    let b: Vec<bool> = (0..200_000)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s & 1 == 1
        })
        .collect();
    assert!(run(&b).iter().all(|c| !c.1));
}

#[test]
fn message_fields() {
    let m = Message::parse(&ROUTINE).unwrap();
    assert_eq!((m.format, m.address, m.category, m.self_id), (120, Address::Mmsi(366123450), 100, 235012340));
    assert_eq!((m.tc1, m.tc2, m.channel, m.eos), (Some(100), Some(126), Some(72), 117));
    // Distress: man overboard at 51°30' N 0°07' W, 12:34 UTC, then voice.
    let d = Message::parse(&[112, 112, 23, 50, 12, 34, 0, 110, 15, 13, 0, 0, 7, 12, 34, 100, 127]).unwrap();
    let x = d.distress.unwrap();
    assert_eq!((x.mmsi, x.nature, x.utc, x.subsequent), (235012340, 110, Some((12, 34)), 100));
    assert_eq!(x.position.map(|p| (p.lat_min, p.lon_min)), Some((51 * 60 + 30, -7)));
    // Too short for its format, digits out of range: nothing.
    assert!(Message::parse(&[120, 120, 36, 61, 117]).is_none());
    assert!(Message::parse(&[120, 120, 36, 161, 23, 45, 0, 100, 23, 50, 12, 34, 0, 100, 126, 117]).is_none());
}
