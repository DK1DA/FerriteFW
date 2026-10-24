//! AX.25, KISS, building packets, beaconing and geometry.

use uaprs::ax25::{Address, Frame, Tnc2};
use uaprs::beacon::{Beaconer, SmartBeacon};
use uaprs::build::{self, Report};
use uaprs::geo::distance_bearing;
use uaprs::kiss;
use uaprs::parse::{parse, Format, MessageKind, Packet, Timestamp};

fn a(t: &str) -> Address {
    Address::from_text(t.as_bytes()).unwrap()
}

#[test]
fn addresses() {
    assert_eq!(a("N0CALL-9").to_string(), "N0CALL-9");
    assert_eq!(a("wide2-1").to_string(), "WIDE2-1");
    assert!(a("N6EX-5*").repeated);
    for bad in ["", "TOOLONG1", "N0CALL-16", "N0_CALL", "N0CALL-", "-1", "N0CALL-123"] {
        assert!(Address::from_text(bad.as_bytes()).is_none(), "{bad}");
    }
}

#[test]
fn frame_round_trip_and_tnc2() {
    let path = [a("WIDE1-1"), a("N6EX-5*")];
    let f = Frame::new(a("N0CALL-9"), a("APRS"), &path, b"!4903.50N/07201.75W-Test\r").unwrap();
    let mut buf = [0u8; 400];
    let n = f.write(&mut buf).unwrap();
    assert_eq!(n, f.len());
    let g = Frame::parse(&buf[..n]).unwrap();
    assert_eq!(g.to_string(), "N0CALL-9>APRS,WIDE1-1,N6EX-5*:!4903.50N/07201.75W-Test<0x0d>");
    let mut text = Vec::new();
    g.emit_tnc2(|b| text.push(b));
    assert_eq!(text, g.to_string().as_bytes());
    let mut ssid = Vec::new();
    a("N0CALL-15").emit(&mut |b| ssid.push(b));
    assert_eq!(ssid, b"N0CALL-15");
    assert!(g.src.same(&a("N0CALL-9")) && g.dest.same(&a("APRS")));
    assert_eq!(g.digis().len(), 2);
    // Truncated, wrong control or PID, or nine digipeaters: rejected.
    assert!(Frame::parse(&buf[..20]).is_none());
    let mut bad = buf;
    bad[28] = 0x13;
    assert!(Frame::parse(&bad[..n]).is_none());
    assert!(Frame::new(a("N0CALL"), a("APRS"), &[a("WIDE1-1"); 9], b"").is_none());
    assert!(f.write(&mut [0u8; 20]).is_none());
}

#[test]
fn tnc2_split() {
    let t = Tnc2::split(b"N0CALL>APRS,WIDE1-1,qAR,IGATE:>status: with colons").unwrap();
    assert_eq!((t.src, t.dest, t.path, t.info), (&b"N0CALL"[..], &b"APRS"[..], &b"WIDE1-1,qAR,IGATE"[..], &b">status: with colons"[..]));
    let t = Tnc2::split(b"N0CALL>APRS:x").unwrap();
    assert!(t.path.is_empty());
    assert!(Tnc2::split(b"no header").is_none());
}

#[test]
fn kiss_round_trip() {
    let frame = [0x82, 0xC0, 0xDB, 0x00, 0xC0, 0xDB, 0xDC, 0xDD];
    let mut wire = Vec::new();
    kiss::encode(2, &frame, |b| wire.push(b));
    assert_eq!(wire.iter().filter(|&&b| b == kiss::FEND).count(), 2);
    let mut d: kiss::Decoder = kiss::Decoder::new();
    let mut got = Vec::new();
    // Noise before the first FEND and back-to-back FENDs are ignored.
    for b in [0x55, 0x66, kiss::FEND].iter().chain(&wire) {
        if let Some(r) = d.push(*b) {
            got.push((r.port, r.command, r.data.to_vec()));
        }
    }
    assert_eq!(got, vec![(2, 0, frame.to_vec())]);
    // A frame longer than the buffer is dropped, the next one is not.
    let mut small: kiss::Decoder<4> = kiss::Decoder::new();
    let mut n = 0;
    for b in [kiss::FEND, 0, 1, 2, 3, 4, 5, kiss::FEND, 0, 9, kiss::FEND] {
        if let Some(r) = small.push(b) {
            assert_eq!(r.data, [9]);
            n += 1;
        }
    }
    assert_eq!(n, 1);
}

fn reparse(info: &[u8]) -> Packet<'_> {
    parse(b"APRS", info).unwrap()
}

#[test]
fn build_positions_parse_back() {
    let places = [(49_058_333, -72_029_167), (-33_850_000, 151_200_000), (0, 0), (89_999_000, 179_999_000), (-45_000_001, -1)];
    for (lat, lon) in places {
        for ts in [None, Some(Timestamp::DayHourMin { day: 9, hour: 23, min: 45, utc: true })] {
            let r = Report {
                lat,
                lon,
                table: b'/',
                code: b'>',
                course_speed: Some((88, 36)),
                altitude: Some(1234),
                messaging: true,
                timestamp: ts,
                comment: b"Ferrite test",
                ambiguity: 0,
            };
            let mut buf = [0u8; 128];
            let n = build::position(&r, &mut buf).unwrap();
            let Packet::Position { pos, messaging, timestamp, comment, .. } = reparse(&buf[..n]) else { panic!() };
            assert!((pos.lat - lat).abs() <= 85 && (pos.lon - lon).abs() <= 85, "{lat} {lon}: {pos:?}");
            assert_eq!((pos.course, pos.speed, pos.altitude), (Some(88), Some(36), Some(1234)));
            assert!(messaging);
            assert_eq!(timestamp, ts);
            assert_eq!(comment.bytes().collect::<Vec<_>>(), b"Ferrite test");

            let n = build::compressed(&r, &mut buf).unwrap();
            let Packet::Position { pos, comment, .. } = reparse(&buf[..n]) else { panic!() };
            assert_eq!(pos.format, Format::Compressed);
            assert!((pos.lat - lat).abs() <= 3 && (pos.lon - lon).abs() <= 6, "{lat} {lon}: {pos:?}");
            assert_eq!(pos.course, Some(88));
            assert!((pos.speed.unwrap() as i32 - 36).abs() <= 1);
            assert_eq!(pos.altitude, Some(1234));
            assert_eq!(comment.bytes().collect::<Vec<_>>(), b"Ferrite test");
        }
    }
}

/// APRS-50: position ambiguity blanks the same trailing digits of latitude and longitude, and a receiver reads
/// it back as such (the middle of the box).
#[test]
fn build_position_ambiguity() {
    // 49 03.50 N, 072 01.75 W
    let want = [
        "=4903.50N/07201.75W[",
        "=4903.5 N/07201.7 W[",
        "=4903.  N/07201.  W[",
        "=490 .  N/0720 .  W[",
        "=49  .  N/072  .  W[",
    ];
    let mut buf = [0u8; 64];
    for (amb, w) in want.iter().enumerate() {
        let r = Report { lat: 49_058_333, lon: -72_029_167, table: b'/', code: b'[', course_speed: None, altitude: None, messaging: true, timestamp: None, comment: b"", ambiguity: amb as u8 };
        let n = build::position(&r, &mut buf).unwrap();
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), *w);
        let Packet::Position { pos, .. } = reparse(&buf[..n]) else { panic!() };
        assert_eq!(pos.ambiguity, amb as u8);
        // within half the box the blanks leave (0.01, 0.1, 1, 10 minutes, then a degree) and the rounding
        let box_ = [85, 920, 8_420, 83_420, 500_085][amb];
        assert!((pos.lat - 49_058_333).abs() <= box_ && (pos.lon + 72_029_167).abs() <= box_, "{amb}: {pos:?}");
    }
    // more than 4 reads as 4
    let r = Report { lat: 49_058_333, lon: -72_029_167, table: b'/', code: b'[', course_speed: None, altitude: None, messaging: true, timestamp: None, comment: b"", ambiguity: 9 };
    let n = build::position(&r, &mut buf).unwrap();
    assert_eq!(&buf[..n], want[4].as_bytes());
}

#[test]
fn build_messages_parse_back() {
    let mut buf = [0u8; 100];
    let n = build::message(b"N0CALL-9", b"Hello from Ferrite", Some(b"42"), &mut buf).unwrap();
    assert_eq!(&buf[..n], b":N0CALL-9 :Hello from Ferrite{42");
    let Packet::Message(m) = reparse(&buf[..n]) else { panic!() };
    assert_eq!((m.addressee, m.text, m.id, m.kind), (&b"N0CALL-9"[..], &b"Hello from Ferrite"[..], Some(&b"42"[..]), MessageKind::Message));
    let n = build::ack(b"N0CALL-9", b"42", &mut buf).unwrap();
    let Packet::Message(m) = reparse(&buf[..n]) else { panic!() };
    assert_eq!((m.kind, m.id), (MessageKind::Ack, Some(&b"42"[..])));
    let n = build::rej(b"N0CALL-9", b"42", &mut buf).unwrap();
    let Packet::Message(m) = reparse(&buf[..n]) else { panic!() };
    assert_eq!(m.kind, MessageKind::Rej);
    // Out of range: addressee over 9, id over 5, text over 67 or with a '{'.
    assert!(build::message(b"TOOLONGNAME", b"x", None, &mut buf).is_none());
    assert!(build::message(b"N0CALL", b"x", Some(b"123456"), &mut buf).is_none());
    assert!(build::message(b"N0CALL", &[b'x'; 68], None, &mut buf).is_none());
    assert!(build::message(b"N0CALL", b"a{b", None, &mut buf).is_none());
    assert!(build::message(b"N0CALL", b"x", None, &mut [0u8; 5]).is_none());
}

#[test]
fn smart_beacon() {
    let mut b = Beaconer::new(SmartBeacon::default());
    assert!(b.due(0, 0, 0)); // nothing sent yet
    b.sent(0, 90);
    // Parked: every 30 minutes.
    assert!(!b.due(1799, 0, 90));
    assert!(b.due(1800, 0, 90));
    // 10 km/h and over: every 3 minutes; half the fast speed (a walk): twice that; a slow amble capped at parked.
    assert!(b.due(180, 120, 90) && !b.due(179, 120, 90));
    assert!(b.due(180, 50, 90) && !b.due(179, 50, 90));
    assert!(b.due(360, 5, 90) && !b.due(359, 5, 90));
    assert!(b.due(600, 3, 90) && !b.due(599, 3, 90));
    // A turn: 28 + 255 / 50 = 33 degrees at 50 km/h, after 30 s.
    assert!(b.due(31, 50, 90 + 34) && !b.due(31, 50, 90 + 32) && !b.due(29, 50, 90 + 90));
    // Across north.
    b.sent(1000, 350);
    assert!(b.due(1040, 50, 25) && !b.due(1040, 50, 10));
}

#[test]
fn distances_and_bearings() {
    // Reference values from the haversine formula on a 6371 km sphere.
    let cases = [
        ((49_058_333, -72_029_167), (49_158_333, -72_029_167), 11_119, 0),
        ((49_000_000, -72_000_000), (49_000_000, -71_900_000), 7_295, 90),
        ((33_750_000, -118_000_000), (34_052_000, -118_244_000), 40_433, 326),
        ((0, 179_950_000), (0, -179_950_000), 11_119, 90),
        ((-33_850_000, 151_200_000), (-34_850_000, 150_200_000), 144_193, 219),
    ];
    for (from, to, metres, bearing) in cases {
        let (d, b) = distance_bearing(from, to);
        assert!((d as i64 - metres).abs() * 200 <= metres, "{from:?} -> {to:?}: {d} m, want {metres}");
        assert!(((b as i32 - bearing + 540) % 360 - 180).abs() <= 1, "{from:?} -> {to:?}: {b} degrees, want {bearing}");
    }
    assert_eq!(distance_bearing((1, 1), (1, 1)), (0, 0));
}

#[test]
fn hostile_packets_do_not_panic() {
    // Every vector packet, mutated: bytes replaced, inserted, deleted, truncated; and random bytes. Runs with
    // overflow checks in debug builds, so arithmetic overflow fails here too.
    let mut seeds: Vec<Vec<u8>> = Vec::new();
    for set in ["wa8lmf", "suite", "spec"] {
        let path = format!("{}/../../testdata/aprs/{set}.hex", env!("CARGO_MANIFEST_DIR"));
        if let Ok(t) = std::fs::read_to_string(path) {
            seeds.extend(t.lines().map(|l| (0..l.len() / 2).map(|i| u8::from_str_radix(&l[2 * i..2 * i + 2], 16).unwrap()).collect()));
        }
    }
    for l in include_str!("spec_packets.txt").lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        seeds.push(l.as_bytes().to_vec());
    }
    let mut s = 0x1234_5678_9ABC_DEF0u64;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let specials = [0u8, b' ', b'/', b'-', b'!', b'}', b'{', b'.', b'9', b'0', 0x7F, 0xFF, b':', b'*'];
    for round in 0..60_000usize {
        let mut p = seeds[round % seeds.len()].clone();
        for _ in 0..1 + rnd() % 4 {
            let n = p.len().max(1);
            let at = (rnd() as usize) % n;
            let b = if rnd() % 2 == 0 { specials[rnd() as usize % specials.len()] } else { rnd() as u8 };
            match rnd() % 4 {
                0 if !p.is_empty() => p[at] = b,
                1 => p.insert(at.min(p.len()), b),
                2 if !p.is_empty() => {
                    p.remove(at);
                }
                _ => p.truncate(at),
            }
        }
        if round % 10 == 0 {
            p = (0..rnd() % 80).map(|_| rnd() as u8).collect();
        }
        if let Some(t) = Tnc2::split(&p) {
            let mut got = parse(t.dest, t.info);
            while let Ok(Packet::ThirdParty(inner)) = got {
                got = parse(inner.dest, inner.info);
            }
        }
        if let Some(f) = Frame::parse(&p) {
            let _ = parse(f.dest.call(), f.info);
            let _ = f.to_string();
        }
        let mut d: kiss::Decoder<64> = kiss::Decoder::new();
        for &b in &p {
            let _ = d.push(b);
        }
    }
}

#[test]
fn edge_values() {
    // Mic-E altitude at its maximum, Ultimeter extremes, overlong NMEA numbers, NMEA out of range.
    assert!(matches!(parse(b"S32U6T", b"`d(((((>/{{{}"), Ok(Packet::Position { .. }) | Err(_)));
    assert!(parse(b"APRS", b"$ULTW7FFF80007FFF80007FFF80007FFF80007FFF80007FFF80007FFF").is_ok());
    // An absurd speed loses the speed, not the position.
    let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"$GPRMC,013244,A,4903.50,N,07201.75,W,4294967296,088.0,240512,,") else { panic!() };
    assert_eq!(pos.speed, None);
    let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,99999999999.5,M,46.9,M,,") else { panic!() };
    assert_eq!(pos.altitude, None);
    assert!(parse(b"APRS", b"$GPGLL,9903.50,N,99901.75,W,225444,A").is_err());
    // Weather with a sign on a field that cannot be negative.
    let Ok(Packet::Weather { weather, .. }) = parse(b"APRS", b"_10090556c-12s004t-05") else { panic!() };
    assert_eq!(weather.wind_dir, None);
    // Impossible timestamps.
    assert!(parse(b"APRS", b"/992345z4903.50N/07201.75W>").is_err());
    assert!(parse(b"APRS", b"@092375z4903.50N/07201.75W>").is_err());
}

#[test]
fn dao_keeps_the_hemisphere_at_zero() {
    let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"!0000.00S/00000.00W>!W99!") else { panic!() };
    assert_eq!((pos.lat, pos.lon), (-150, -150));
    let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"!4903.50S/07201.75W>!W12!") else { panic!() };
    assert_eq!((pos.lat, pos.lon), (-49_058_350, -72_029_200));
}

#[test]
fn builder_courses_near_north_and_data_like_comments() {
    let mut buf = [0u8; 128];
    for course in [1u16, 2, 3, 357, 358, 359, 360] {
        let r = Report { lat: 1, lon: 1, table: b'/', code: b'>', course_speed: Some((course, 20)), altitude: None, messaging: false, timestamp: None, comment: b"", ambiguity: 0 };
        let n = build::compressed(&r, &mut buf).unwrap();
        let Packet::Position { pos, .. } = reparse(&buf[..n]) else { panic!() };
        // Within 2 degrees of north, compressed course reads back as "none" (c = 0); never as a range.
        assert_eq!(pos.range, None, "course {course}");
        assert!(pos.speed.is_some());
        if let Some(c) = pos.course {
            assert!(((c as i32 - course as i32 + 540) % 360 - 180).abs() <= 2, "course {course} -> {c}");
        }
    }
    for comment in [&b"123/045 hello"[..], b"RNG0050 ok", b"PHG5132 x", b"/slash first", b" space first"] {
        for (cs, alt) in [(None, None), (Some((90u16, 10u16)), None), (None, Some(100))] {
            let r = Report { lat: 1, lon: 1, table: b'/', code: b'>', course_speed: cs, altitude: alt, messaging: false, timestamp: None, comment, ambiguity: 0 };
            let n = build::position(&r, &mut buf).unwrap();
            let Packet::Position { pos, comment: c, .. } = reparse(&buf[..n]) else { panic!() };
            assert_eq!(c.bytes().collect::<Vec<_>>(), comment.trim_ascii(), "{}", String::from_utf8_lossy(&buf[..n]));
            assert_eq!((pos.course, pos.speed, pos.range, pos.phg), (cs.map(|c| c.0), cs.map(|c| c.1), None, None));
        }
    }
}

/// KISS-11: framing as the KISS spec: a bad byte after FESC is dropped, a frame cut off mid-escape too.
#[test]
fn kiss_escapes_and_full_buffer() {
    let mut d: kiss::Decoder<8> = kiss::Decoder::new();
    let mut got = Vec::new();
    // A bad escape drops its byte (the spec: "no action is taken"); FESC then FEND ends the frame mid-escape, so
    // it is dropped.
    for b in [kiss::FEND, 0, 1, kiss::FESC, 0x42, 2, kiss::FEND, 0, kiss::FESC, kiss::FEND, 0, 7, kiss::FEND] {
        if let Some(r) = d.push(b) {
            got.push(r.data.to_vec());
        }
    }
    assert_eq!(got, vec![vec![1, 2], vec![7]]);
    // Exactly full is fine.
    let mut d: kiss::Decoder<4> = kiss::Decoder::new();
    let mut n = 0;
    for b in [kiss::FEND, 0, 1, 2, 3, kiss::FEND] {
        n += d.push(b).is_some_and(|r| r.data == [1, 2, 3]) as u32;
    }
    assert_eq!(n, 1);
}

#[test]
fn smart_beacon_odd_settings() {
    let cfg = SmartBeacon { slow_speed: 0, fast_speed: 65535, fast_rate: u32::MAX, ..SmartBeacon::default() };
    let mut b = Beaconer::new(cfg);
    b.sent(0, 0);
    let _ = b.due(10, 0, 0);
    let _ = b.due(10, 1, 180);
}

#[test]
fn geo_far_and_polar() {
    let check = |from: (i32, i32), to: (i32, i32), metres: i64| {
        let (d, _) = distance_bearing(from, to);
        assert!((d as i64 - metres).abs() * 200 <= metres, "{from:?} -> {to:?}: {d} m, want {metres}");
    };
    // 1000 km east-west at 45 N, and north-south near the pole (haversine references).
    check((45_000_000, 0), (45_000_000, 12_700_000), 997_535);
    check((80_000_000, 10_000_000), (88_000_000, 10_000_000), 889_559);
}

/// KISS-11: commands 1-6 and Return, p-persistence, full duplex never skipping the channel check, port 0 only.
#[test]
fn kiss_settings_and_channel_access() {
    use kiss::{Access, Command, Params, Received};
    let mut p = Params::default();
    let r = |port: u8, command: u8, data: &'static [u8]| Received { port, command, data };
    assert_eq!(p.apply(&r(0, 1, &[30])), Command::Set);
    assert_eq!(p.apply(&r(0, 2, &[127])), Command::Set);
    assert_eq!(p.apply(&r(0, 3, &[5])), Command::Set);
    assert_eq!(p.apply(&r(0, 4, &[2])), Command::Set);
    assert_eq!((p.tx_delay, p.persistence, p.slot_time, p.tx_tail), (30, 127, 5, 2));
    assert_eq!(p.apply(&r(0, 0, b"frame")), Command::Data(b"frame"));
    assert_eq!(p.apply(&r(0, 6, &[1, 2])), Command::Hardware(&[1, 2]));
    assert_eq!(p.apply(&r(15, 15, &[])), Command::Exit);
    assert_eq!(p.apply(&r(0, 1, &[])), Command::Unknown);
    // 300 ms at 1200 baud is 45 flags; no tail still gets one closing flag.
    assert_eq!(Params { tx_delay: 30, tx_tail: 0, ..p }.flags(1200), (45, 1));

    let mut a = Access::default();
    // Busy: never. Clear and lucky: now. Clear and unlucky: not until a slot time has passed.
    assert!(!a.may_send(&p, 0, true, 0));
    assert!(a.may_send(&p, 10, false, 127));
    assert!(!a.may_send(&p, 20, false, 128));
    assert!(!a.may_send(&p, 60, false, 0)); // slot is 50 ms
    assert!(a.may_send(&p, 70, false, 0));
    // The channel going busy cancels the wait; full duplex from the host changes nothing (half-duplex radio).
    assert!(!a.may_send(&p, 80, false, 200));
    assert!(!a.may_send(&p, 90, true, 0));
    assert!(a.may_send(&p, 95, false, 0));
    assert!(!a.may_send(&Params { full_duplex: true, ..p }, 100, true, 0), "busy stays busy");
    // A single-port TNC takes port 0 and Return only.
    assert!(r(0, 0, b"x").for_single_port() && r(15, 15, &[]).for_single_port());
    assert!(!r(1, 0, b"x").for_single_port() && !r(2, 1, &[30]).for_single_port());
    // Over many tries, P = 63 sends about a quarter of the time in free slots.
    let p = Params::default();
    let mut a = Access::default();
    let (mut sent, mut tries, mut t, mut x) = (0, 0, 0u32, 1u32);
    while tries < 4000 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        if a.may_send(&p, t, false, (x >> 16) as u8) {
            sent += 1;
        }
        tries += 1;
        t += 100;
    }
    assert!((800..1200).contains(&sent), "{sent} of {tries}");
}

/// KISS-12: hardware commands (command 6) never become data or settings: Mobilinkd's PTT test tones (0x07-0x09),
/// a host's frequency set (0xEA) and save (0x2A) come back as `Hardware`, which the firmware ignores, and leave
/// the TNC settings as they were.
#[test]
fn kiss_hardware_commands_do_nothing() {
    use kiss::{Command, Params, Received};
    let mut p = Params::default();
    let before = p;
    for sub in [&[0x07u8][..], &[0x08], &[0x09], &[0xEA, 0x08, 0xAA, 0x6B, 0x60], &[0x2A], &[0x28]] {
        let r = Received { port: 0, command: 6, data: sub };
        assert_eq!(p.apply(&r), Command::Hardware(sub));
    }
    assert_eq!(p, before);
}
