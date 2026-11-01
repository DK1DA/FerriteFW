//! Satellite records as bytes (SAT-2, LINK-63 – LINK-65): the link's `what::SAT` record (the same bytes the store
//! keeps, before its CRC), the `what::SAT_INFO` record (the declination and where the list came from, SAT-18,
//! SAT-21), and the per-satellite choices the store keeps beside the records. Little-endian throughout.
//!
//! `what::SAT`, [`REC_LEN`] bytes:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0 | status: 0 active, 1 inactive, 2 unknown |
//! | 1 | flags: bit 0 half duplex to start, bit 1 Doppler on the 70 cm side only to start |
//! | 2 | transponders used, 0-5 |
//! | 3 | 0 |
//! | 4-7 | NORAD catalogue number |
//! | 8-23 | name, printable ASCII, NUL-padded |
//! | 24-27 | epoch: seconds since 2000-01-01 00:00 UTC |
//! | 28-29 | epoch: milliseconds |
//! | 30-31 | 0 |
//! | 32-47 | inclination, RAAN, argument of perigee, mean anomaly: u32 each, 1e-7 degree |
//! | 48-51 | eccentricity, 1e-7 |
//! | 52-55 | mean motion, 1e-8 revolution a day |
//! | 56-59 | B* as f32 bits |
//! | 60-71 | 0 (room for more elements) |
//! | 72 + 16 k | transponder k: uplink u32 (10 Hz, 0 receive only), downlink u32 (10 Hz), access tone u16 (0.1 Hz, 0 none), arming tone u16 (0.1 Hz, 0 none), kind u8 (0 voice, 1 APRS, 2 beacon), 3 bytes 0 |

use super::data::Prefs;
use super::{Elements, Kind, Sat, Status, Transponder, F32, TPS};

/// A satellite record's length on the link.
pub const REC_LEN: usize = 72 + 16 * TPS;
/// `what::SAT_INFO`'s length.
pub const INFO_LEN: usize = 16;
/// The declination's "none".
pub const NO_DECL: i16 = 0x7FFF;

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// A satellite as its record.
pub fn encode(s: &Sat) -> [u8; REC_LEN] {
    let mut b = [0u8; REC_LEN];
    (b[0], b[1], b[2]) = (s.status as u8, s.flags, s.ntp.min(TPS as u8));
    b[4..8].copy_from_slice(&s.id.to_le_bytes());
    b[8..24].copy_from_slice(&s.name);
    let e = &s.el;
    b[24..28].copy_from_slice(&e.epoch.to_le_bytes());
    b[28..30].copy_from_slice(&e.epoch_ms.to_le_bytes());
    for (k, v) in [e.incl, e.raan, e.argp, e.ma, e.ecc, e.n, e.bstar.0].into_iter().enumerate() {
        b[32 + 4 * k..36 + 4 * k].copy_from_slice(&v.to_le_bytes());
    }
    for (k, t) in s.tps[..s.ntp.min(TPS as u8) as usize].iter().enumerate() {
        let o = 72 + 16 * k;
        b[o..o + 4].copy_from_slice(&t.up.to_le_bytes());
        b[o + 4..o + 8].copy_from_slice(&t.down.to_le_bytes());
        b[o + 8..o + 10].copy_from_slice(&t.tone.to_le_bytes());
        b[o + 10..o + 12].copy_from_slice(&t.arm.to_le_bytes());
        b[o + 12] = t.kind as u8;
    }
    b
}

/// A record read back, checked as the link's writes are (SAT-2, SAT-13): the reason when it isn't one.
pub fn decode(b: &[u8]) -> Result<Sat, &'static str> {
    if b.len() != REC_LEN {
        return Err("a satellite record is 152 bytes");
    }
    let status = match b[0] {
        0 => Status::Active,
        1 => Status::Inactive,
        2 => Status::Unknown,
        _ => return Err("status: 0 active, 1 inactive, 2 unknown"),
    };
    let ntp = b[2];
    if ntp as usize > TPS {
        return Err("5 transponders at most");
    }
    let mut name = [0u8; 16];
    name.copy_from_slice(&b[8..24]);
    let n = name.iter().position(|&c| c == 0).unwrap_or(16);
    if n == 0 || !name[..n].iter().all(|&c| (0x20..0x7F).contains(&c)) || name[n..].iter().any(|&c| c != 0) {
        return Err("name: 1 to 16 printable characters");
    }
    let w = |k: usize| u32_at(b, 32 + 4 * k);
    let el = Elements { epoch: u32_at(b, 24), epoch_ms: u16_at(b, 28), incl: w(0), raan: w(1), argp: w(2), ma: w(3), ecc: w(4), n: w(5), bstar: F32(w(6)) };
    if el.n == 0 || el.ecc >= 10_000_000 || el.incl > 1_800_000_000 || el.epoch_ms > 999 || !el.bstar.get().is_finite() {
        return Err("elements out of range");
    }
    let mut tps = [Transponder::default(); TPS];
    for (k, t) in tps[..ntp as usize].iter_mut().enumerate() {
        let o = 72 + 16 * k;
        let kind = match b[o + 12] {
            0 => Kind::Voice,
            1 => Kind::Aprs,
            2 => Kind::Beacon,
            _ => return Err("transponder kind: 0 voice, 1 APRS, 2 beacon"),
        };
        *t = Transponder { up: u32_at(b, o), down: u32_at(b, o + 4), tone: u16_at(b, o + 8), arm: u16_at(b, o + 10), kind };
        if !t.in_band() {
            return Err("transponders: 144-148 and 430-440 MHz only (SAT-13)");
        }
        if t.tone > 2_541 || t.arm > 2_541 {
            return Err("tones: 254.1 Hz at most");
        }
    }
    Ok(Sat { id: u32_at(b, 4), name, status, el, tps, ntp, flags: b[1] & (Sat::HD | Sat::UHF_ONLY) })
}

/// `what::SAT_INFO` (SAT-18, SAT-21): the declination at the radio (0.1°, east positive), the time the satellite
/// data Console wrote was built (UTC seconds, 0 unknown) and when Console wrote it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub decl: Option<i16>,
    pub stamp: u32,
    pub written: u32,
}

impl Info {
    pub fn encode(&self) -> [u8; INFO_LEN] {
        let mut b = [0u8; INFO_LEN];
        b[..2].copy_from_slice(&self.decl.unwrap_or(NO_DECL).to_le_bytes());
        b[4..8].copy_from_slice(&self.stamp.to_le_bytes());
        b[8..12].copy_from_slice(&self.written.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8]) -> Result<Info, &'static str> {
        if b.len() != INFO_LEN {
            return Err("the satellite info is 16 bytes");
        }
        let d = u16_at(b, 0) as i16;
        if d != NO_DECL && d.unsigned_abs() > 1_800 {
            return Err("declination: -180.0 to 180.0 degrees");
        }
        Ok(Info { decl: (d != NO_DECL).then_some(d), stamp: u32_at(b, 4), written: u32_at(b, 8) })
    }
}

/// The per-satellite choices as the store keeps them, by catalogue number (8 bytes after it): trim (10 Hz), bits
/// (0 half duplex, 1 the 70 cm side only), transponder, uplink power (0 Low, 1 Mid, 2 High).
pub fn prefs_encode(p: &Prefs) -> [u8; 6] {
    let t = p.trim.to_le_bytes();
    [t[0], t[1], p.hd as u8 | (p.uhf_only as u8) << 1, p.tp, p.power, 0]
}

pub fn prefs_decode(b: &[u8; 6]) -> Prefs {
    Prefs { trim: i16::from_le_bytes([b[0], b[1]]).clamp(-500, 500), hd: b[2] & 1 != 0, uhf_only: b[2] & 2 != 0, tp: b[3].min(TPS as u8 - 1), power: b[4].min(2) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn so50() -> Sat {
        let mut name = [0u8; 16];
        name[..5].copy_from_slice(b"SO-50");
        let el = super::super::tle::parse(
            b"1 27607U 02058C   26304.82911449  .00000904  00000+0  12403-3 0  9995",
            b"2 27607  64.5527  66.1378 0068979 237.1936 122.2515 14.83296838284727",
        )
        .unwrap();
        let mut tps = [Transponder::default(); TPS];
        tps[0] = Transponder { up: 14_585_000, down: 43_679_500, tone: 670, arm: 744, kind: Kind::Voice };
        Sat { id: 27_607, name, status: Status::Active, el, tps, ntp: 1, flags: Sat::UHF_ONLY }
    }

    /// SAT-2, LINK-63: a satellite goes through its record unchanged.
    #[test]
    fn a_record_round_trips() {
        let s = so50();
        let b = encode(&s);
        assert_eq!(b.len(), 152);
        assert_eq!(decode(&b), Ok(s));
        assert_eq!(&b[4..8], &27_607u32.to_le_bytes());
        assert_eq!(&b[72 + 10..72 + 12], &744u16.to_le_bytes(), "SAT-10: the arming tone");
    }

    /// SAT-13: a transponder outside 2 m and 70 cm is refused; so is a malformed record.
    #[test]
    fn refuses_what_isnt_a_satellite() {
        let mut s = so50();
        s.tps[0].down = 29_400_000; // 10 m
        assert!(decode(&encode(&s)).unwrap_err().contains("SAT-13"));
        let mut b = encode(&so50());
        b[8] = 0;
        assert!(decode(&b).is_err(), "no name");
        let mut b = encode(&so50());
        b[2] = 6;
        assert!(decode(&b).is_err());
        assert!(decode(&b[..100]).is_err());
    }

    /// SAT-18, SAT-21: the info record; the per-satellite choices.
    #[test]
    fn info_and_prefs() {
        let i = Info { decl: Some(-130), stamp: 1_793_500_000, written: 1_793_541_600 };
        assert_eq!(Info::decode(&i.encode()), Ok(i));
        assert_eq!(Info::decode(&Info::default().encode()).unwrap().decl, None);
        let mut b = i.encode();
        b[..2].copy_from_slice(&2000i16.to_le_bytes());
        assert!(Info::decode(&b).is_err());
        let p = Prefs { trim: -40, hd: true, uhf_only: false, tp: 2, power: 1 };
        assert_eq!(prefs_decode(&prefs_encode(&p)), p);
    }
}
