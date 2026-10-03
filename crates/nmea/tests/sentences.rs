//! Sentences from real modules' formats: fixes, hemispheres, satellites, bad checksums, overlong lines.

use nmea::Nmea;

/// Feed "$body*CS\r\n" with a correct checksum (or a wrong one when bad).
fn feed(n: &mut Nmea, body: &str, bad: bool) -> usize {
    let cs = body.bytes().fold(0u8, |a, b| a ^ b) ^ bad as u8;
    format!("${body}*{cs:02X}\r\n").bytes().filter(|&c| n.feed(c)).count()
}

#[test]
fn sentences() {
    let mut n = Nmea::new();
    // Tsawwassen-ish: 49 deg 00.6000 N, 123 deg 05.4000 W
    assert_eq!(feed(&mut n, "GNGGA,183412.00,4900.60000,N,12305.40000,W,1,09,0.9,12.3,M,-17.0,M,,", false), 1);
    assert_eq!(n.utc_hms, 183412);
    assert!(n.quality == 1 && n.sats_used == 9 && n.hdop10 == 9 && n.alt_dm == 123);
    assert_eq!(n.lat_e7, 490_100_000); // 49 + 0.6/60 = 49.01
    assert_eq!(n.lon_e7, -1_230_900_000); // 123 + 5.4/60 = 123.09, west
    assert_eq!(&n.last_type, b"GNGGA");

    assert_eq!(feed(&mut n, "GPRMC,183413.00,A,4900.60000,N,12305.40000,W,0.0,,290926,,,A", false), 1);
    assert!(n.valid && n.date_dmy == 290926 && n.utc_hms == 183413);
assert!(n.has_fix());
// Moving: 23.4 knots at 271.5 degrees.
assert_eq!(feed(&mut n, "GNRMC,183413.50,A,4900.60000,N,12305.40000,W,23.4,271.5,290926,,,A", false), 1);
assert!(n.speed_kn10 == 234 && n.course10 == 2715);
    assert_eq!(feed(&mut n, "GPRMC,183414.00,V,,,,,,,290926,,,N", false), 1);
    assert!(!n.valid);
    assert_eq!(n.lat_e7, 490_100_000); // last good position kept

    assert_eq!(feed(&mut n, "GPGSV,3,1,11,01,40,083,46,02,17,308,41,12,07,344,39,14,22,228,45", false), 1);
    assert_eq!(feed(&mut n, "GLGSV,2,1,07,65,20,100,30,66,40,160,35,,,,,,,,", false), 1);
    assert_eq!(n.sats_view, 18);
    assert_eq!(n.nsat, 6); // 4 GP + 2 GL (empty GL slots skipped)
    assert!(n.sat[0].prn == 1 && n.sat[0].snr == 46 && n.sat[3].prn == 14 && n.sat[4].talker == 1);
    assert_eq!(feed(&mut n, "GPGSV,3,2,11,15,10,100,,17,50,200,30", false), 1);
    assert!(n.nsat == 8 && n.sat[6].prn == 15 && n.sat[6].snr == 0 && n.sat[7].snr == 30);
    assert_eq!(feed(&mut n, "GPGSV,3,1,12,01,40,083,46", false), 1);
    assert_eq!(n.sats_view, 19);
    assert_eq!(n.nsat, 3); // GP restarted: 2 GL + 1 GP
    assert_eq!(feed(&mut n, "GAGSV,1,1,01,05,40,083,46,7", false), 1); // NMEA 4.10: trailing signal ID
    assert!(n.nsat == 4 && n.sat[3].prn == 5 && n.sat[3].talker == 2);

    // no fix: GGA quality 0 must not move the position
    assert_eq!(feed(&mut n, "GPGGA,183415.00,,,,,0,00,99.99,,,,,,", false), 1);
    assert!(n.quality == 0 && n.sats_used == 0 && n.lat_e7 == 490_100_000);
    // southern / eastern hemisphere, negative altitude
    assert_eq!(feed(&mut n, "GPGGA,000001,3351.0000,S,15112.0000,E,1,05,1.2,-3.5,M,,,,", false), 1);
    assert!(n.lat_e7 == -338_500_000 && n.lon_e7 == 1_512_000_000 && n.alt_dm == -35);

    let before = n.sentences;
    assert_eq!(feed(&mut n, "GPGGA,1,2,3", true), 0);
    assert!(n.checksum_errors == 1 && n.sentences == before);

    // garbage, missing checksum, overlong line
    for &c in b"\x01\xFF noise $GPGGA,no,star\r\n" {
        n.feed(c);
    }
    assert_eq!(n.checksum_errors, 2);
    n.feed(b'$');
    for _ in 0..200 {
        n.feed(b'A');
    }
    assert_eq!(n.overlong, 1);
    assert_eq!(feed(&mut n, "GPRMC,1,A,4900.6,N,12305.4,W,,,010126,,,A", false), 1); // recovers
}
