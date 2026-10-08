//! GPS (manual chapter 28): an incremental NMEA 0183 reader for the radio's ATGM336H, shared by the firmware's
//! GPS task and the host tests. Feed bytes as they arrive; [`Nmea::fix`] and [`Nmea::sky`] give the shared
//! [`GpsFix`] and [`GpsSky`]. `no_std`, integer only, no allocation.
//!
//! Grown from uradio's `nmea` crate for what the GPS screens need: GGA (fix quality, altitude, HDOP), RMC
//! (validity, position, speed, course, time and date), GSA (2D/3D and which satellites are used) and GSV
//! (satellites in view with elevation, azimuth and signal, for the sky plot, GPS-18). Any talker (GP, GN, GL,
//! GA, BD, GB); checksums checked, bad or overlong sentences counted and dropped.
//!
//! RAM: 88 B for the line being read, 24 satellites of 6 B, two used-satellite masks: about 260 B, held by the
//! GPS task only.

use crate::msg::{GpsFix, GpsSky, SkySat};

/// A satellite from GSV.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Sat {
    pub prn: u8,
    /// dB-Hz; 0 = tracked but no signal.
    pub snr: u8,
    pub elev: u8,
    pub azim: u16,
    /// Index into the talker list: 0 GP, 1 GL, 2 GA, 3 BD, 4 GB, 5 GQ.
    pub talker: u8,
}

impl Sat {
    fn beidou(&self) -> bool {
        self.talker == 3 || self.talker == 4
    }
}

/// The longest sentence kept (NMEA allows 82 characters with `$` and CR LF).
const LINE: usize = 88;
/// Satellites kept from GSV.
const SATS: usize = 24;
const TALKERS: [&[u8; 2]; 6] = [b"GP", b"GL", b"GA", b"BD", b"GB", b"GQ"];

#[derive(Clone, Copy)]
pub struct Nmea {
    pub sentences: u32,
    pub checksum_errors: u32,
    pub overlong: u32,
    /// RMC status A.
    pub valid: bool,
    /// GGA fix quality, 0 = none.
    pub quality: u8,
    /// GSA mode: 1 none, 2 2D, 3 3D (0 until a GSA).
    pub mode: u8,
    /// GGA's satellites used; GSV's in view, summed over constellations.
    pub sats_used: u8,
    pub sats_view: u8,
    gsv_view: [u8; 6],
    pub sat: [Sat; SATS],
    pub nsat: u8,
    pub hdop10: u16,
    pub alt_dm: i32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// hhmmss and ddmmyy; `date_dmy` 0 until an RMC has a date.
    pub utc_hms: u32,
    pub date_dmy: u32,
    pub speed_kn10: u32,
    pub course10: u16,
    /// PRNs used in the fix from GSA, bit n = PRN n (1-63): GPS and the rest, and BeiDou.
    used_gps: u64,
    used_bds: u64,
    /// GSA sentences seen since the last GGA: the masks are rebuilt each second.
    gsa_since_gga: u8,
    line: [u8; LINE],
    len: u8,
    in_sentence: bool,
}

/// Field i (0 = the address field) of a comma-separated sentence body.
fn field(body: &[u8], i: usize) -> Option<&[u8]> {
    body.split(|&c| c == b',').nth(i)
}

/// Unsigned decimal with up to `frac` fractional digits, scaled by 10^frac. None when empty or bad.
fn fixed(s: &[u8], frac: u32) -> Option<i64> {
    if s.is_empty() || s.len() > 15 {
        return None;
    }
    let (mut v, mut seen, mut dot) = (0i64, 0, false);
    for &c in s {
        match c {
            b'.' if dot => return None,
            b'.' => dot = true,
            b'0'..=b'9' => {
                if dot {
                    if seen >= frac {
                        continue;
                    }
                    seen += 1;
                }
                v = v * 10 + (c - b'0') as i64;
            }
            _ => return None,
        }
    }
    for _ in seen..frac {
        v *= 10;
    }
    Some(v)
}

fn num(body: &[u8], idx: usize, frac: u32) -> Option<i64> {
    field(body, idx).and_then(|f| fixed(f, frac))
}

fn fixed_signed(s: &[u8], frac: u32) -> Option<i64> {
    match s {
        [b'-', rest @ ..] => fixed(rest, frac).map(|v| -v),
        _ => fixed(s, frac),
    }
}

/// ddmm.mmmm or dddmm.mmmm plus hemisphere -> degrees x 1e7.
fn coord(s: &[u8], h: &[u8], out: &mut i32) {
    let Some(v) = fixed(s, 5) else { return }; // ddmm.mmmmm x 1e5
    if h.is_empty() {
        return;
    }
    let (deg, min_e5) = (v / 10_000_000, v % 10_000_000);
    let e7 = deg * 10_000_000 + (min_e5 * 10 + 3) / 6; // minutes / 60, rounded
    *out = if h[0] == b'S' || h[0] == b'W' { -e7 } else { e7 } as i32;
}

fn hexval(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm), for years 2000-2099 here.
fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as i32;
    era * 146_097 + doe - 719_468
}

/// UTC seconds since 1970 from RMC's ddmmyy and hhmmss; None for a missing or impossible date.
pub fn unix_time(date_dmy: u32, utc_hms: u32) -> Option<u32> {
    let (d, m, y) = (date_dmy / 10_000, date_dmy / 100 % 100, date_dmy % 100);
    let (hh, mm, ss) = (utc_hms / 10_000, utc_hms / 100 % 100, utc_hms % 100);
    if !(1..=31).contains(&d) || !(1..=12).contains(&m) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(2000 + y as i32, m, d);
    Some(days as u32 * 86_400 + hh * 3600 + mm * 60 + ss)
}

impl Default for Nmea {
    fn default() -> Self {
        Self::new()
    }
}

impl Nmea {
    pub const fn new() -> Self {
        Nmea {
            sentences: 0,
            checksum_errors: 0,
            overlong: 0,
            valid: false,
            quality: 0,
            mode: 0,
            sats_used: 0,
            sats_view: 0,
            gsv_view: [0; 6],
            sat: [Sat { prn: 0, snr: 0, elev: 0, azim: 0, talker: 0 }; SATS],
            nsat: 0,
            hdop10: 0,
            alt_dm: 0,
            lat_e7: 0,
            lon_e7: 0,
            utc_hms: 0,
            date_dmy: 0,
            speed_kn10: 0,
            course10: 0,
            used_gps: 0,
            used_bds: 0,
            gsa_since_gga: 0,
            line: [0; LINE],
            len: 0,
            in_sentence: false,
        }
    }

    /// A position fix: RMC valid or GGA quality above 0.
    pub fn has_fix(&self) -> bool {
        self.valid || self.quality > 0
    }

    /// True when a sentence with a good checksum was just parsed.
    pub fn feed(&mut self, c: u8) -> bool {
        if c == b'$' {
            self.in_sentence = true;
            self.len = 0;
            return false;
        }
        if !self.in_sentence {
            return false;
        }
        if c == b'\r' || c == b'\n' {
            self.in_sentence = false;
            let line = self.line;
            let mut s = &line[..self.len as usize];
            if let Some(nul) = s.iter().position(|&b| b == 0) {
                s = &s[..nul];
            }
            let ok = match s.iter().rposition(|&b| b == b'*') {
                Some(star) if star + 2 < s.len() => {
                    let sum = s[..star].iter().fold(0u8, |a, &b| a ^ b);
                    match (hexval(s[star + 1]), hexval(s[star + 2])) {
                        (Some(hi), Some(lo)) if sum == hi << 4 | lo => Some(star),
                        _ => None,
                    }
                }
                _ => None,
            };
            let Some(star) = ok else {
                self.checksum_errors += 1;
                return false;
            };
            self.sentences += 1;
            self.parse(&s[..star]);
            return true;
        }
        if self.len as usize >= LINE - 1 {
            self.in_sentence = false;
            self.overlong += 1;
            return false;
        }
        self.line[self.len as usize] = c;
        self.len += 1;
        false
    }

    /// Forget the fix (the module was powered off, GPS-1): the next one starts from nothing.
    pub fn reset(&mut self) {
        *self = Nmea { sentences: self.sentences, checksum_errors: self.checksum_errors, overlong: self.overlong, ..Nmea::new() };
    }

    fn parse(&mut self, body: &[u8]) {
        let Some(addr) = field(body, 0).filter(|a| a.len() == 5) else { return };
        let talker = TALKERS.iter().position(|t| addr[..2] == t[..]);
        match &addr[2..] {
            b"GGA" => {
                if let Some(v) = num(body, 1, 0) {
                    self.utc_hms = v as u32;
                }
                self.quality = num(body, 6, 0).unwrap_or(0) as u8;
                self.sats_used = num(body, 7, 0).unwrap_or(0) as u8;
                self.hdop10 = num(body, 8, 1).unwrap_or(0) as u16;
                if let Some(v) = field(body, 9).and_then(|f| fixed_signed(f, 1)) {
                    self.alt_dm = v as i32;
                }
                if self.quality != 0 {
                    if let (Some(f), Some(g)) = (field(body, 2), field(body, 3)) {
                        coord(f, g, &mut self.lat_e7);
                    }
                    if let (Some(f), Some(g)) = (field(body, 4), field(body, 5)) {
                        coord(f, g, &mut self.lon_e7);
                    }
                }
                self.gsa_since_gga = 0;
            }
            b"RMC" => {
                if let Some(v) = num(body, 1, 0) {
                    self.utc_hms = v as u32;
                }
                self.valid = field(body, 2) == Some(&b"A"[..]);
                if self.valid {
                    if let (Some(f), Some(g)) = (field(body, 3), field(body, 4)) {
                        coord(f, g, &mut self.lat_e7);
                    }
                    if let (Some(f), Some(g)) = (field(body, 5), field(body, 6)) {
                        coord(f, g, &mut self.lon_e7);
                    }
                    self.speed_kn10 = num(body, 7, 1).unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
                    self.course10 = num(body, 8, 1).unwrap_or(0).clamp(0, 3599) as u16;
                }
                if let Some(v) = num(body, 9, 0) {
                    self.date_dmy = v as u32;
                }
            }
            b"GSA" => {
                // One GSA per constellation each second: the first after a GGA starts the masks afresh.
                if self.gsa_since_gga == 0 {
                    (self.used_gps, self.used_bds) = (0, 0);
                }
                self.gsa_since_gga = self.gsa_since_gga.saturating_add(1);
                self.mode = num(body, 2, 0).unwrap_or(0) as u8;
                // NMEA 4.10 ends with a system ID (1 GPS, 2 GLONASS, 3 Galileo, 4 BeiDou); before that, the talker.
                let nf = 1 + body.iter().filter(|&&c| c == b',').count();
                let sys = if nf >= 19 { num(body, 18, 0) } else { None };
                let bds = sys == Some(4) || (sys.is_none() && matches!(talker, Some(3 | 4)));
                for i in 3..15 {
                    if let Some(prn) = num(body, i, 0).filter(|&p| (1..64).contains(&p)) {
                        if bds {
                            self.used_bds |= 1 << prn;
                        } else {
                            self.used_gps |= 1 << prn;
                        }
                    }
                }
            }
            b"GSV" => {
                let Some(slot) = talker else { return };
                if let Some(v) = num(body, 3, 0) {
                    self.gsv_view[slot] = v as u8;
                    let sum: u32 = self.gsv_view.iter().map(|&n| n as u32).sum();
                    self.sats_view = sum.min(255) as u8;
                }
                // Message 1 of a talker's group starts that talker's list afresh.
                if num(body, 2, 0) == Some(1) {
                    let mut keep = 0;
                    for i in 0..self.nsat as usize {
                        if self.sat[i].talker as usize != slot {
                            self.sat[keep] = self.sat[i];
                            keep += 1;
                        }
                    }
                    self.nsat = keep as u8;
                }
                let nfields = 1 + body.iter().filter(|&&c| c == b',').count();
                let blocks = if nfields >= 4 { (nfields - 4) / 4 } else { 0 }; // 4.10 adds a trailing signal ID
                for k in 0..blocks.min(4) {
                    let prn = num(body, 4 + 4 * k, 0).unwrap_or(-1);
                    if prn <= 0 || prn > 255 || self.nsat as usize >= SATS {
                        continue;
                    }
                    let elev = num(body, 5 + 4 * k, 0).unwrap_or(0).clamp(0, 90) as u8;
                    let azim = num(body, 6 + 4 * k, 0).unwrap_or(0).clamp(0, 359) as u16;
                    let snr = num(body, 7 + 4 * k, 0).unwrap_or(0).clamp(0, 99) as u8;
                    self.sat[self.nsat as usize] = Sat { prn: prn as u8, snr, elev, azim, talker: slot as u8 };
                    self.nsat += 1;
                }
            }
            _ => {}
        }
    }

    /// Whether this satellite is used in the fix (GSA).
    pub fn used(&self, s: &Sat) -> bool {
        let mask = if s.beidou() { self.used_bds } else { self.used_gps };
        s.prn < 64 && mask & 1 << s.prn != 0
    }

    /// The shared fix (module on), stamped `at_ms`.
    pub fn fix(&self, at_ms: u32) -> GpsFix {
        let fix = match (self.has_fix(), self.mode) {
            (false, _) => 0,
            (true, 3) => 3,
            (true, 2) => 2,
            // no GSA: GGA's altitude with 4 or more satellites is as good as 3D
            (true, _) => if self.sats_used >= 4 { 3 } else { 2 },
        };
        GpsFix {
            on: true,
            fix,
            used: self.sats_used,
            view: self.sats_view,
            lat: self.lat_e7,
            lon: self.lon_e7,
            alt_dm: self.alt_dm,
            speed_kn10: self.speed_kn10.min(u16::MAX as u32) as u16,
            course10: self.course10,
            hdop10: self.hdop10,
            unix: if self.date_dmy != 0 { unix_time(self.date_dmy, self.utc_hms).unwrap_or(0) } else { 0 },
            at_ms,
        }
    }

    /// The satellites in view for the sky plot: used ones first, then by signal, 16 at most.
    pub fn sky(&self) -> GpsSky {
        let mut sky = GpsSky::default();
        let mut idx: [u8; SATS] = core::array::from_fn(|i| i as u8);
        let n = self.nsat as usize;
        let key = |i: u8| {
            let s = &self.sat[i as usize];
            (!self.used(s), u8::MAX - s.snr)
        };
        idx[..n].sort_unstable_by_key(|&i| key(i));
        for &i in idx[..n].iter().take(sky.sat.len()) {
            let s = &self.sat[i as usize];
            sky.sat[sky.n as usize] = SkySat {
                prn: s.prn,
                snr: s.snr,
                elev: s.elev,
                azim_2: (s.azim / 2) as u8,
                flags: self.used(s) as u8 | (s.beidou() as u8) << 1,
            };
            sky.n += 1;
        }
        sky
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    /// One second of the ATGM336H's output, written as it sends it (GPS and BeiDou, NMEA 4.1).
    fn feed(n: &mut Nmea, lines: &[&str]) {
        for l in lines {
            let body = l.trim_start_matches('$');
            let sum = body.bytes().fold(0u8, |a, b| a ^ b);
            let s = std::format!("${body}*{sum:02X}\r\n");
            for b in s.bytes() {
                n.feed(b);
            }
        }
    }

    #[test]
    fn unix_from_rmc() {
        assert_eq!(unix_time(10_100, 0), Some(946_684_800)); // 1 Jan 2000
        assert_eq!(unix_time(71_026, 143_210), Some(1_791_383_530)); // 7 Oct 2026 14:32:10 UTC
        assert_eq!(unix_time(290_224, 0), Some(1_709_164_800)); // 29 Feb 2024
        assert_eq!(unix_time(0, 0), None);
        assert_eq!(unix_time(320_126, 0), None);
    }

    #[test]
    fn checksum_and_overlong() {
        let mut n = Nmea::new();
        for b in b"$GNRMC,1*00\r\n" {
            n.feed(*b);
        }
        assert_eq!((n.sentences, n.checksum_errors), (0, 1));
        let long = [b'A'; 120];
        n.feed(b'$');
        for b in long {
            n.feed(b);
        }
        assert_eq!(n.overlong, 1);
    }

    #[test]
    fn a_second_of_output() {
        let mut n = Nmea::new();
        feed(&mut n, FIX_SECOND);
        let f = n.fix(1234);
        assert!(f.has_fix());
        assert_eq!(f.fix, 3);
        assert_eq!((f.used, f.view), (9, 15));
        assert_eq!(f.lat, 454_215_833); // 45 25.2950 N
        assert_eq!(f.lon, -756_976_667); // 075 41.8600 W
        assert_eq!(f.alt_dm, 712);
        assert_eq!((f.speed_kn10, f.course10, f.hdop10), (126, 873, 9));
        assert_eq!(f.unix, 1_791_383_530);
        assert_eq!(f.at_ms, 1234);
        let sky = n.sky();
        assert_eq!(sky.n, 15);
        // used first (9 of them), strongest first among those
        assert!(sky.sat[..9].iter().all(|s| s.flags & 1 != 0));
        assert!(sky.sat[9..15].iter().all(|s| s.flags & 1 == 0));
        assert!(sky.sat[..9].windows(2).all(|w| w[0].snr >= w[1].snr));
        let bd = sky.sat[..15].iter().find(|s| s.prn == 6 && s.flags & 2 != 0).unwrap();
        assert_eq!((bd.elev, bd.azim_2 as u16 * 2, bd.snr, bd.flags), (62, 120, 38, 3));
        let gp = sky.sat[..15].iter().find(|s| s.prn == 6 && s.flags & 2 == 0).unwrap();
        assert_eq!(gp.flags & 1, 0, "GPS 6 isn't used; BeiDou 6 is (two masks)");
    }

    #[test]
    fn no_fix_yet_still_has_time() {
        let mut n = Nmea::new();
        feed(&mut n, NO_FIX_SECOND);
        let f = n.fix(0);
        assert!(!f.has_fix());
        assert_eq!(f.fix, 0);
        assert_eq!(f.unix, 1_791_383_400, "the clock can be set before a position fix (GPS-7)");
        assert_eq!(n.sky().n, 3);
    }

    #[test]
    fn reset_forgets() {
        let mut n = Nmea::new();
        feed(&mut n, FIX_SECOND);
        n.reset();
        assert!(!n.fix(0).has_fix());
        assert_eq!(n.sky().n, 0);
        assert!(n.sentences > 0, "the counters stay");
    }

    /// 7 Oct 2026, 14:32:10 UTC near Ottawa, moving at 12.6 kn: what the ATGM336H sends each second.
    pub(crate) const FIX_SECOND: &[&str] = &[
        "GNGGA,143210.000,4525.2950,N,07541.8600,W,1,09,0.9,71.2,M,-34.0,M,,",
        "GNGLL,4525.2950,N,07541.8600,W,143210.000,A,A",
        "GNGSA,A,3,02,05,12,25,29,,,,,,,,1.6,0.9,1.3,1",
        "GNGSA,A,3,06,09,16,22,,,,,,,,,1.6,0.9,1.3,4",
        "GPGSV,3,1,09,02,45,080,42,05,30,150,40,06,10,300,22,12,70,020,45,0",
        "GPGSV,3,2,09,15,05,210,0,19,15,260,18,24,08,330,20,25,55,110,44,0",
        "GPGSV,3,3,09,29,35,060,41,0",
        "BDGSV,2,1,06,06,62,120,38,09,40,200,36,16,25,250,33,22,50,170,37,0",
        "BDGSV,2,2,06,30,12,030,15,35,03,090,0,0",
        "GNRMC,143210.000,A,4525.2950,N,07541.8600,W,12.6,87.3,071026,,,A,V",
        "GNVTG,87.3,T,,M,12.6,N,23.3,K,A",
        "GNZDA,143210.000,07,10,2026,00,00",
        "GPTXT,01,01,01,ANTENNA OK",
    ];

    /// Searching: time and date already known, no position, three satellites heard.
    const NO_FIX_SECOND: &[&str] = &[
        "GNGGA,143000.000,,,,,0,00,25.5,,,,,,",
        "GNGSA,A,1,,,,,,,,,,,,,25.5,25.5,25.5,1",
        "GPGSV,1,1,03,02,45,080,22,12,70,020,25,25,55,110,0,0",
        "GNRMC,143000.000,V,,,,,,,071026,,,N,V",
    ];
}
