//! Mic-E reports built by `build::mic_e`, read back by our own parser, against the APRS 1.0.1 chapter 10
//! examples and frames heard on the air.

use uaprs::build::{self, Report};
use uaprs::parse::{parse, Format, MicEStatus, Packet, Position};

fn report(lat: i32, lon: i32) -> Report<'static> {
    Report {
        lat,
        lon,
        table: b'/',
        code: b'[',
        course_speed: None,
        altitude: None,
        messaging: true,
        timestamp: None,
        comment: b"",
        ambiguity: 0,
    }
}

fn encode(r: &Report, status: u8) -> ([u8; 6], Vec<u8>) {
    let mut dest = [0u8; 6];
    let mut info = [0u8; 128];
    let n = build::mic_e(r, status, &mut dest, &mut info).unwrap();
    (dest, info[..n].to_vec())
}

/// Built, then parsed: the position, the status and the comment.
fn round(r: &Report, status: u8) -> (Position, MicEStatus, Vec<u8>) {
    let (dest, info) = encode(r, status);
    match parse(&dest, &info) {
        Ok(Packet::Position { pos, messaging: true, mic_e: Some(m), comment, .. }) => {
            assert_eq!(pos.format, Format::MicE);
            (pos, m, comment.bytes().collect())
        }
        other => panic!("{:?} {:?}: {other:?}", core::str::from_utf8(&dest), info),
    }
}

/// Millionths of a degree to the nearest hundredth of a minute, as sent.
fn hmin(v: i32) -> i32 {
    let a = v.unsigned_abs() as i64;
    let h = (a * 6 + 500) / 1000;
    let back = (h * 1000 + 3) / 6;
    (if v < 0 { -back } else { back }) as i32
}

/// APRS-52.1: the spec's worked examples: the destination for 33 25.64 N, +0, W with bits 1/0/0 is `S32U6T`;
/// `(_f` is 112 07.74 W with the +100 offset; the speed and course bytes `n"O` (its other encoding of 20 knots,
/// 251 degrees) decode the same as ours; 200 feet is `"4T}`; `T4SQZZ` blanks two digits.
#[test]
fn spec_examples() {
    let lat = 33_000_000 + 2564 * 1000 / 6; // 33 25.64
    let mut r = report(lat, -(12_000_000 + 774 * 1000 / 6));
    let (dest, _) = encode(&r, build::MIC_E_RETURNING);
    assert_eq!(&dest, b"S32U6T");
    // the decoding example, with the +100 offset in the destination
    let spec = b"`(_fn\"Oj/";
    let Ok(Packet::Position { pos, .. }) = parse(b"S32UVT", spec) else { panic!() };
    assert_eq!((pos.lon, pos.speed, pos.course, pos.table, pos.code), (-(112_000_000 + 774 * 1000 / 6), Some(20), Some(251), b'/', b'j'));
    r = Report { lon: pos.lon, course_speed: Some((251, 20)), table: b'/', code: b'j', ..r };
    let (dest, info) = encode(&r, build::MIC_E_RETURNING);
    assert_eq!(&dest, b"S32UVT");
    assert_eq!(&info[..4], b"`(_f");
    assert_eq!(&info[4..7], &[0x1e, 0x1e, b'O']); // SP 2, DC 0 tens and 2 hundreds, SE 51
    assert_eq!(&info[7..], b"j/");
    assert!(parse(&dest, &info) == parse(b"S32UVT", spec));
    // altitude: 200 feet = 61 m
    r.altitude = Some(200);
    assert_eq!(&encode(&r, build::MIC_E_RETURNING).1[9..], b"\"4T}");
    // ambiguity 2: ZZ, and the longitude's last two digits not sent
    let r = Report { lat: 44_000_000 + 3100 * 1000 / 6, ambiguity: 2, altitude: None, ..r };
    let (dest, info) = encode(&r, build::MIC_E_IN_SERVICE);
    assert_eq!(&dest, b"T4SQZZ");
    assert_eq!(&info[1..4], &[b'(', b'_', 28]);
}

/// APRS-52.1, APRS-52.2: every longitude fold (0-9, 10-99, 100-109, 110-179 degrees; minutes under and over 10), both
/// hemispheres, round trip to the hundredth of a minute; 179.99999 is sent as 179 59.99, not 180.
#[test]
fn longitude_folds_and_hemispheres() {
    for deg in [0, 9, 10, 99, 100, 109, 110, 179] {
        for min_h in [0, 501, 1000, 5999] {
            for (ns, ew) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let lon = ew * (deg * 1_000_000 + min_h * 1000 / 6 + 1);
                let lat = ns * (45_000_000 + min_h * 1000 / 6 + 1);
                let (p, _, _) = round(&report(lat, lon), build::MIC_E_EN_ROUTE);
                assert_eq!((p.lat, p.lon), (hmin(lat), hmin(lon)), "{deg} {min_h} {ns} {ew}");
            }
        }
    }
    for lon in [179_999_990, -179_999_990, 180_000_000, -180_000_000] {
        let (p, _, _) = round(&report(0, lon), build::MIC_E_EN_ROUTE);
        assert_eq!(p.lon, lon.signum() * (179_000_000 + (5999 * 1000 + 3) / 6));
    }
    let (p, _, _) = round(&report(89_999_999, 0), build::MIC_E_EN_ROUTE);
    assert_eq!(p.lat, 89_000_000 + (5999 * 1000 + 3) / 6);
    // 90 00.00 exactly (the pole, as another encoder rounds 89.999999) reads; past it doesn't
    assert!(matches!(parse(b"YP0PP0", b"`vX\x1c\x1c\x1c\x1c>/"), Ok(Packet::Position { pos: Position { lat: 90_000_000, .. }, .. })));
    assert!(parse(b"YP0PP1", b"`vX\x1c\x1c\x1c\x1c>/").is_err());
    let mut d = [0; 6];
    assert!(build::mic_e(&report(90_000_001, 0), 7, &mut d, &mut [0; 64]).is_none());
    assert!(build::mic_e(&report(0, 0), 8, &mut d, &mut [0; 64]).is_none());
    assert!(build::mic_e(&report(0, 0), 7, &mut d, &mut [0; 8]).is_none()); // the buffer
}

/// APRS-52.2: ambiguity 0-4 blanks the same digits of both coordinates; the parser puts the position in the
/// middle of what's left.
#[test]
fn ambiguity() {
    let (lat, lon) = (45_425_511, -75_690_211); // 45 25.53 N, 75 41.41 W
    let want = [(45_425_500, -75_690_167), (45_425_833, -75_690_833), (45_425_000, -75_691_667), (45_416_667, -75_750_000), (45_500_000, -75_500_000)];
    for amb in 0..=4u8 {
        let r = Report { ambiguity: amb, ..report(lat, lon) };
        let (dest, _) = encode(&r, build::MIC_E_OFF_DUTY);
        let blank = dest.iter().filter(|&&c| c == b'L' || c == b'Z').count();
        assert_eq!(blank, amb as usize);
        let (p, _, _) = round(&r, build::MIC_E_OFF_DUTY);
        assert_eq!(p.ambiguity, amb);
        assert_eq!((p.lat, p.lon), want[amb as usize], "{amb}");
    }
    // south and east blank as L (0 bits)
    let r = Report { ambiguity: 4, ..report(-lat, -lon) };
    assert_eq!(&encode(&r, build::MIC_E_OFF_DUTY).0, b"TUZLLL");
}

/// APRS-52.1: the seven standard messages and Emergency, as the bits A, B, C (standard, not custom).
#[test]
fn statuses() {
    for bits in 0..8u8 {
        let (_, m, _) = round(&report(45_000_000, -75_000_000), bits);
        assert_eq!(m, MicEStatus { bits, custom: false, device: None });
    }
    assert_eq!(&encode(&report(45_000_000, -75_000_000), build::MIC_E_EMERGENCY).0[..3], b"450");
    assert_eq!(&encode(&report(45_000_000, -75_000_000), build::MIC_E_OFF_DUTY).0[..3], b"TUP");
}

/// APRS-52.1: speed and course: none is 0 and 0 (course unknown); 360 is north and stays 360; 0 is unknown, as in
/// the uncompressed report; over 360 wraps; speed up to 799 knots, capped there.
#[test]
fn speed_and_course() {
    let r = report(45_000_000, -75_000_000);
    let (p, _, _) = round(&r, build::MIC_E_EN_ROUTE);
    assert_eq!((p.speed, p.course), (Some(0), None));
    for (cs, want) in [((360, 0), (0, Some(360))), ((0, 5), (5, None)), ((361, 5), (5, Some(1))), ((720, 5), (5, Some(360))), ((1, 799), (799, Some(1))), ((359, 800), (799, Some(359))), ((99, 9), (9, Some(99))), ((200, 10), (10, Some(200))), ((721, 65), (65, Some(1)))] {
        let (p, _, _) = round(&Report { course_speed: Some(cs), ..r }, build::MIC_E_EN_ROUTE);
        assert_eq!((p.speed, p.course), (Some(want.0), want.1), "{cs:?}");
    }
}

/// APRS-52.1: altitude as `xxx}` metres (to the metre: feet back within 2), low and high; with and without a
/// comment after it.
#[test]
fn altitude() {
    for ft in [-1000, -1, 0, 1, 200, 1234, 29_029, 100_000] {
        for comment in [&b""[..], b"hi}"] {
            let r = Report { altitude: Some(ft), comment, ..report(45_000_000, -75_000_000) };
            let (p, _, c) = round(&r, build::MIC_E_EN_ROUTE);
            assert!((p.altitude.unwrap() - ft).abs() <= 2, "{ft} {:?}", p.altitude);
            assert_eq!(c, comment);
        }
    }
}

/// APRS-52.3: a comment that would read as a radio type, telemetry or an altitude goes out behind a space or two;
/// the parser gives it back as written.
#[test]
fn comments_protected() {
    for comment in [&b"Ferrite"[..], b"`x", b"'x", b">x", b"]x", b"abc}", b"xabc} later", b"!x}y", b"]abc}=", b"} ok", b"Ferrite 73 }"] {
        let r = Report { comment, ..report(45_000_000, -75_000_000) };
        let (p, _, c) = round(&r, build::MIC_E_EN_ROUTE);
        assert_eq!((c.as_slice(), p.altitude), (comment, None), "{:?}", core::str::from_utf8(comment));
        let (_, info) = encode(&r, build::MIC_E_EN_ROUTE);
        let pad = info[9..].iter().take_while(|&&b| b == b' ').count();
        let risky = comment[0] == b'`' || comment[0] == b'\'' || comment[0] == b'>' || comment[0] == b']' || comment.get(3) == Some(&b'}') || comment.get(4) == Some(&b'}');
        assert_eq!(pad > 0, risky, "{:?}", core::str::from_utf8(comment));
    }
}

/// APRS-52.1: frames heard on the air (WA8LMF TNC Test CD), decoded and built again: the same destination and
/// symbol, the same longitude bytes when exact, and the same position, speed and course once read (these radios
/// send speed and course in the spec's other encoding, and a type byte and altitude before the comment).
#[test]
fn heard_frames_rebuilt() {
    let heard: [(&[u8], &[u8]); 7] = [
        (b"S4QVYV", b"'.4&l-/k/]\"7q}"),
        (b"SS5PPQ", b"`.](n->>/\"4W}"),
        (b"S3UWTS", b"`-)\x1dl \x1cv\\\":r}"),
        (b"ST0XPR", b"`-T0m4\x7f>/"),
        (b"SSUUUP", b"',Leo}(j/]\":Z}"),
        (b"S4QUYZ", b"'.Kel ~v/]\"6A}TM-D700"),
        (b"STPYVU", b"'._wlzzv/]Stopped"),
    ];
    for (dest, info) in heard {
        let Ok(Packet::Position { pos, mic_e: Some(m), .. }) = parse(dest, info) else { panic!("{dest:?}") };
        let cs = (pos.course.unwrap_or(0), pos.speed.unwrap());
        let r = Report { course_speed: Some(cs), table: pos.table, code: pos.code, ambiguity: pos.ambiguity, ..report(pos.lat, pos.lon) };
        let (d, i) = encode(&r, m.bits);
        assert_eq!(&d, dest);
        if pos.ambiguity == 0 {
            assert_eq!(&i[1..4], &info[1..4], "{dest:?}");
        }
        assert_eq!(&i[7..9], &info[7..9]);
        let Ok(Packet::Position { pos: back, mic_e, .. }) = parse(&d, &i) else { panic!() };
        assert_eq!((back.lat, back.lon, back.speed, back.course, back.ambiguity, mic_e), (pos.lat, pos.lon, pos.speed, pos.course, pos.ambiguity, Some(MicEStatus { device: None, ..m })));
    }
}

/// APRS-5.2: the device codes leave the comment and name the radio (aprs-deviceid): the type byte first, the
/// model suffix last, the altitude after the type byte; an unknown suffix stays as text.
#[test]
fn device_codes() {
    use uaprs::parse::{mic_e_device, MIC_E_DEVICES};
    let name = |d: Option<u8>| d.map(|i| MIC_E_DEVICES[i as usize].1);
    let cases: [(&[u8], &[u8], Option<&str>); 9] = [
        (b"]=", b"", Some("Kenwood TM-D710")),
        (b"`_\"", b"", Some("Yaesu FTM-350")),
        (b"`_3", b"", Some("Yaesu FT5D")),
        (b">&", b"", Some("Kenwood TH-D75")),
        (b"`146.520[1", b"146.520", Some("APRSdroid")),
        (b"]TM-D700", b"TM-D700", Some("Kenwood TM-D700")),
        (b">Hello^", b"Hello", Some("Kenwood TH-D74")),
        (b"'Stopped", b"Stopped", None),
        (b"Plain text", b"Plain text", None),
    ];
    for (c, text, dev) in cases {
        let (t, d) = mic_e_device(c);
        assert_eq!((t, name(d)), (text, dev), "{:?}", core::str::from_utf8(c));
    }
    // In a frame: the altitude after the type byte, the model at the end, neither in the comment.
    let Ok(Packet::Position { pos, mic_e: Some(m), comment, .. }) = parse(b"S4QVYV", b"`.4&l-/k/`\"7q}Hi_3") else { panic!() };
    assert!(pos.altitude.is_some());
    assert_eq!((comment.bytes().collect::<Vec<u8>>(), name(m.device)), (b"Hi".to_vec(), Some("Yaesu FT5D")));
}
