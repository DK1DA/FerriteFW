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

/// Codeword `c` (0 = the first after the dot pattern) of a call from [`bits`] starts at this bit; codeword bit `k`
/// (bit 9 sent first) is `k` bits before its end.
fn bit_of(c: usize, k: usize) -> usize {
    40 + 10 * c + (9 - k)
}

// In `bits(call(&ROUTINE))`: the DX copy of symbol i is codeword 12 + 2i, the RX copy codeword 17 + 2i. The EOS
// is symbol 18, the ECC 19, the two closing EOS 20 and 21 (DX only).
const DX_EOS: usize = 12 + 2 * 18;
const RX_EOS: usize = 17 + 2 * 18;

#[test]
fn damaged_eos_found_from_both_copies() {
    // Both copies of the EOS and the first closing EOS one check bit off: the end is the nearest value to both
    // copies. (Check bits, and different ones: an information bit off in each copy can leave another value as near.)
    let c = call(&ROUTINE);
    let mut b = bits(&c);
    for at in [bit_of(DX_EOS, 0), bit_of(RX_EOS, 1), bit_of(12 + 2 * 20, 0)] {
        b[at] = !b[at];
    }
    assert_eq!(run(&b), vec![(ROUTINE.to_vec(), true, 0)]);
}

#[test]
fn end_unrecognised_until_the_signal_goes() {
    // The EOS copies three check bits off between them and both closing EOS damaged: the end is not seen when the
    // ECC arrives. When the signal goes (six bad codewords) the framer looks back for the most likely EOS and keeps
    // the call because its ECC agrees.
    let c = call(&ROUTINE);
    let mut b = bits(&c);
    for at in [bit_of(DX_EOS, 0), bit_of(DX_EOS, 1), bit_of(RX_EOS, 2), bit_of(12 + 2 * 20, 0), bit_of(12 + 2 * 21, 0)] {
        b[at] = !b[at];
    }
    b.extend([false; 200]);
    assert_eq!(run(&b), vec![(ROUTINE.to_vec(), true, 0)]);
    // The same with the ECC lost too: nothing to confirm the guess, so nothing is reported.
    let mut b2 = b.clone();
    for c in [12 + 2 * 19, 17 + 2 * 19] {
        for k in [9, 3] {
            b2[bit_of(c, k)] = !b2[bit_of(c, k)];
        }
    }
    assert!(run(&b2).is_empty(), "{:?}", run(&b2));
}

#[test]
fn call_cut_off_before_its_end_is_dropped() {
    // The signal goes half way through the call: no EOS to look back to, nothing reported.
    let mut b = bits(&call(&ROUTINE));
    b.truncate(bit_of(30, 0));
    b.extend([false; 200]);
    assert!(run(&b).is_empty());
}

#[test]
fn surer_format_specifier_copy_wins() {
    // The first format specifier one bit off in both copies, the second clean: both read as the second.
    let c = call(&ROUTINE);
    let mut b = bits(&c);
    for at in [bit_of(12, 2), bit_of(17, 6)] {
        b[at] = !b[at];
    }
    assert_eq!(run(&b), vec![(ROUTINE.to_vec(), true, 0)]);
}

#[test]
fn wrong_symbol_in_both_copies_fails_the_ecc() {
    // Both copies of symbol 9 replaced by another valid codeword: the ECC disagrees and cannot stand in for a
    // symbol that sure, so the call is reported with ecc_ok false.
    let c = call(&ROUTINE);
    let mut b = bits(&c);
    for cw in [12 + 2 * 9, 17 + 2 * 9] {
        let w = encode(77);
        for k in 0..10 {
            b[bit_of(cw, k)] = (w >> k) & 1 == 1;
        }
    }
    let got = run(&b);
    assert_eq!(got.len(), 1);
    let mut want = ROUTINE.to_vec();
    want[9] = 77;
    assert_eq!(got[0], (want, false, 0));
}

#[test]
fn call_too_short_is_dropped() {
    // An EOS straight after the format specifiers: too short to be a call.
    assert!(run(&bits(&call(&[120, 120, 117]))).is_empty());
}

#[test]
fn default_framer_is_new() {
    let mut f = Framer::default();
    let b = bits(&call(&ROUTINE));
    let got: Vec<Vec<u8>> = b.iter().filter_map(|&x| f.push(x).map(|c| c.symbols.to_vec())).collect();
    assert_eq!(got, vec![ROUTINE.to_vec()]);
}

#[test]
fn message_formats_and_fields() {
    use udsc::message::Position;
    // All ships, safety: no address; telecommands, no channel.
    let m = Message::parse(&[116, 116, 108, 23, 50, 12, 34, 0, 100, 126, 127]).unwrap();
    assert_eq!((m.format, m.address, m.category, m.self_id), (116, Address::All, 108, 235012340));
    assert_eq!((m.tc1, m.tc2, m.channel, m.position, m.eos), (Some(100), Some(126), None, None, 127));
    // Area: quadrant 1 (NW), corner 51 N 7 W, 5 by 10 degrees.
    let m = Message::parse(&[102, 102, 15, 10, 7, 5, 10, 110, 23, 50, 12, 34, 0, 100, 126, 117]).unwrap();
    assert_eq!(m.address, Address::Area { quadrant: 1, lat: 51, lon: 7, dlat: 5, dlon: 10 });
    assert_eq!(m.category, 110);
    // A position call (telecommand 121) in each quadrant; quadrant digits above 3 and all nines give none.
    let pos = |p: [u8; 5]| {
        let mut s = vec![120, 120, 36, 61, 23, 45, 0, 108, 23, 50, 12, 34, 0, 121, 126];
        s.extend(p);
        s.push(127);
        Message::parse(&s).unwrap().position
    };
    // 33 52 S 151 12 E is 2 3352 15112.
    assert_eq!(pos([23, 35, 21, 51, 12]), Some(Position { lat_min: -(33 * 60 + 52), lon_min: 151 * 60 + 12 }));
    assert_eq!(pos([33, 35, 21, 51, 12]), Some(Position { lat_min: -(33 * 60 + 52), lon_min: -(151 * 60 + 12) }));
    assert_eq!(pos([3, 35, 21, 51, 12]), Some(Position { lat_min: 33 * 60 + 52, lon_min: 151 * 60 + 12 }));
    assert_eq!(pos([43, 35, 21, 51, 12]), None);
    assert_eq!(pos([99; 5]), None);
    assert_eq!(pos([23, 35, 121, 51, 12]), None, "a symbol that is not two digits");
    // A working channel field with a non-digit symbol: the call is rejected.
    assert!(Message::parse(&[120, 120, 36, 61, 23, 45, 0, 100, 23, 50, 12, 34, 0, 100, 126, 90, 100, 72, 117]).is_none());
}

#[test]
fn distress_acknowledgement_and_relay() {
    // All ships, distress category, telecommand 110 (acknowledgement): the vessel's MMSI and the alert's fields;
    // position unknown (all nines) and time unknown (88 88).
    let s = [116, 116, 112, 23, 50, 12, 34, 0, 110, 36, 61, 23, 45, 0, 101, 99, 99, 99, 99, 99, 88, 88, 109, 127];
    let m = Message::parse(&s).unwrap();
    assert_eq!((m.category, m.self_id, m.tc1, m.tc2), (112, 235012340, Some(110), None));
    let d = m.distress.unwrap();
    assert_eq!((d.mmsi, d.nature, d.position, d.utc, d.subsequent), (366123450, 101, None, None, 109));
    // A relay (112) cut short before its subsequent-communications symbol: rejected.
    let mut r = s;
    r[8] = 112;
    assert!(Message::parse(&r).is_some());
    assert!(Message::parse(&[&r[..22], &[127]].concat()).is_none());
}

#[test]
fn malformed_calls() {
    for s in [
        &[][..],
        &[120],
        &[104, 104, 1, 2, 3, 117],                         // not a format specifier
        &[112, 112, 23, 50, 12, 34, 0, 110, 15, 117],       // distress alert too short
        &[112, 112, 23, 150, 12, 34, 0, 110, 15, 13, 0, 0, 7, 12, 34, 100, 127], // MMSI not digits
        &[102, 102, 15, 10, 7, 5, 117],                     // area too short
        &[102, 102, 15, 110, 7, 5, 10, 110, 23, 50, 12, 34, 0, 100, 126, 117], // area not digits
        &[116, 116, 108, 23, 50, 12, 34, 0, 117],           // no telecommand
        &[116, 116, 108, 23, 50, 12, 34, 0, 100, 117],      // no second telecommand
        &[116, 116, 112, 23, 50, 12, 34, 0, 110, 36, 117],  // acknowledgement without the alert
    ] {
        assert!(Message::parse(s).is_none(), "{s:?}");
    }
}

/// DSC audio at 9600 samples/s: 1300 Hz for 1, 2100 Hz for 0, 1200 baud, phase continuous.
fn afsk(bits: &[bool]) -> Vec<i16> {
    let mut out = vec![0i16; 2000];
    let mut phase = 0f64;
    for &b in bits {
        let f = if b { 1300.0 } else { 2100.0 };
        for _ in 0..8 {
            phase += 2.0 * std::f64::consts::PI * f / 9600.0;
            out.push((8000.0 * phase.sin()) as i16);
        }
    }
    out.extend([0i16; 4000]);
    out
}

#[test]
fn receiver_reports_each_call_once() {
    use udsc::receiver::{Receiver, DSC_VHF};
    use uafsk::fsk::Config;
    assert_eq!((DSC_VHF.mark_hz, DSC_VHF.space_hz, DSC_VHF.baud), (1300, 2100, 1200));
    let audio = afsk(&bits(&call(&ROUTINE)));
    // Four slicers (the default) each decode the call; it is reported once.
    let mut rx: Receiver = Receiver::new(Config::new(9600));
    let mut got = Vec::new();
    let mut busy = 0;
    for &x in &audio {
        rx.push(x, |c| got.push((c.symbols.to_vec(), c.ecc_ok)));
        busy += rx.busy() as usize;
    }
    assert_eq!(got, vec![(ROUTINE.to_vec(), true)]);
    assert!(busy > 0 && !rx.busy(), "carrier detect during the call only ({busy} samples)");
    // The same call again a second later is a new call.
    let mut again = Vec::new();
    for &x in audio.iter().chain(&audio) {
        rx.push(x, |c| again.push(c.symbols.to_vec()));
    }
    assert_eq!(again, vec![ROUTINE.to_vec(), ROUTINE.to_vec()]);
}
