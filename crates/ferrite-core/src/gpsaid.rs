//! GPS-38: start aiding. As the module powers up, the radio tells it where it last was (and the time, when the
//! radio's clock is set) with a CASIC binary AID-INI message (CASIC protocol 3.16.1, class 0x0B id 0x01), so it
//! searches for the satellites over that place first instead of a full cold search.
//!
//! - **Position:** the last GPS fix only (never the fixed position, 0 6 9, nor anything built in); no fix known,
//!   nothing is sent ([`aid_ini`] returns `None`).
//! - **Time:** GPS week and time of week from the radio's UTC clock and the GPS-UTC leap seconds ([`LEAP_S`]),
//!   only when the clock is set; otherwise the message carries the position alone (no time-valid bit).
//! - **The frame (CASIC 3.2):** `BA CE`, the payload length (U2), class, id, the payload, and a U4 checksum: the
//!   word sum of `(id << 24) + (class << 16) + len` and the payload's words, all little-endian.

/// The AID-INI message's class and id (CASIC 3.16.1).
pub const AID_CLASS: u8 = 0x0B;
pub const AID_ID: u8 = 0x01;
/// Its payload: 56 bytes.
pub const AID_PAYLOAD: usize = 56;
/// The whole frame: header 2, length 2, class and id 2, payload, checksum 4.
pub const AID_LEN: usize = 6 + AID_PAYLOAD + 4;
/// GPS time runs ahead of UTC by this many seconds (since 2017).
pub const LEAP_S: u32 = 18;
/// The GPS epoch, 1980-01-06 00:00 UTC, in Unix seconds.
pub const GPS_EPOCH_UNIX: u32 = 315_964_800;
/// The position's error, as the variance of its 3D error in m^2: the last fix may be hours and kilometres old, so
/// 5 km (the module only uses it to pick the satellites in view, which barely change over that).
pub const AID_PACC_M2: f32 = 5_000.0 * 5_000.0;
/// The time's error: 2 s (the radio's clock runs from the last fix or a manual set, GPS-23), as the spec scales
/// it: the variance in s^2 times c^2 (`tAcc = 9` is 10 ns).
pub const AID_TACC: f32 = (2.0 * 299_792_458.0) * (2.0 * 299_792_458.0);

/// Flag bits (CASIC 3.16.1 note 1).
pub const AID_POS_VALID: u8 = 1 << 0;
pub const AID_TIME_VALID: u8 = 1 << 1;
pub const AID_LLA: u8 = 1 << 5;
pub const AID_ALT_INVALID: u8 = 1 << 6;

/// The last fix the aid is built from: latitude and longitude in 1e-7 degrees, and the altitude in decimetres when
/// the fix had one (a 3D fix).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AidFix {
    pub lat: i32,
    pub lon: i32,
    pub alt_dm: Option<i32>,
}

impl AidFix {
    /// From a GPS report: only one with a fix; its altitude only from a 3D fix.
    pub fn from_fix(f: &crate::msg::GpsFix) -> Option<AidFix> {
        f.has_fix().then_some(AidFix { lat: f.lat, lon: f.lon, alt_dm: (f.fix >= 3).then_some(f.alt_dm) })
    }
}

/// GPS week and time of week in milliseconds for UTC `unix` seconds plus `ms` (0-999).
pub fn gps_time(unix: u32, ms: u32) -> (u16, u32) {
    let s = unix.wrapping_sub(GPS_EPOCH_UNIX).wrapping_add(LEAP_S);
    ((s / 604_800) as u16, (s % 604_800) * 1000 + ms)
}

/// The CASIC binary frame for class `class`, id `id` and `payload` (a multiple of 4 bytes) into `out`; returns the
/// length (`payload.len() + 10`).
pub fn casbin(class: u8, id: u8, payload: &[u8], out: &mut [u8]) -> usize {
    let len = payload.len();
    out[..2].copy_from_slice(&[0xBA, 0xCE]);
    out[2..4].copy_from_slice(&(len as u16).to_le_bytes());
    (out[4], out[5]) = (class, id);
    out[6..6 + len].copy_from_slice(payload);
    let mut sum = ((id as u32) << 24).wrapping_add((class as u32) << 16).wrapping_add(len as u32);
    for w in payload.as_chunks::<4>().0 {
        sum = sum.wrapping_add(u32::from_le_bytes(*w));
    }
    out[6 + len..10 + len].copy_from_slice(&sum.to_le_bytes());
    len + 10
}

/// An f32 as the spec's R8 (an f64, little-endian), widened by hand: the FPU does f32 only, and f64 arithmetic
/// would link soft-float routines into the fast flash. f32 is plenty for an aid stated to 5 km and 2 s (under a
/// metre in latitude and longitude, 1/16 s in time of week). Normal numbers and zero only (all the aid carries).
fn r8(x: f32) -> [u8; 8] {
    let b = x.to_bits();
    let (sign, exp, man) = ((b >> 31) as u64, (b >> 23) & 0xFF, (b & 0x7F_FFFF) as u64);
    let e = if exp == 0 { 0 } else { (exp + 1023 - 127) as u64 };
    (sign << 63 | e << 52 | man << 29).to_le_bytes()
}

/// GPS-38: the AID-INI frame for the last fix `fix` and, when the radio's clock is set, `utc` (Unix seconds and
/// milliseconds); `None` with no fix. Position valid in LLA form, the altitude invalid when the fix had none, the
/// time valid only with `utc`.
#[inline(never)] // in .slow_text (memory.x): once a module power-up
pub fn aid_ini(fix: Option<AidFix>, utc: Option<(u32, u32)>, out: &mut [u8; AID_LEN]) -> Option<usize> {
    let f = fix?;
    let mut p = [0u8; AID_PAYLOAD];
    p[0..8].copy_from_slice(&r8(f.lat as f32 * 1e-7));
    p[8..16].copy_from_slice(&r8(f.lon as f32 * 1e-7));
    p[16..24].copy_from_slice(&r8(f.alt_dm.unwrap_or(0) as f32 * 0.1));
    let mut flags = AID_POS_VALID | AID_LLA;
    if f.alt_dm.is_none() {
        flags |= AID_ALT_INVALID;
    }
    // freqBias (32) and fAcc (44) stay 0: no clock drift is given (B2, B4 clear); res (48) 0.
    p[36..40].copy_from_slice(&AID_PACC_M2.to_le_bytes());
    if let Some((unix, ms)) = utc {
        let (wn, tow_ms) = gps_time(unix, ms);
        p[24..32].copy_from_slice(&r8(tow_ms as f32 * 1e-3));
        p[40..44].copy_from_slice(&AID_TACC.to_le_bytes());
        p[52..54].copy_from_slice(&wn.to_le_bytes());
        flags |= AID_TIME_VALID;
    }
    p[55] = flags;
    Some(casbin(AID_CLASS, AID_ID, &p, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: AidFix = AidFix { lat: 490_912_345, lon: -1_226_812_345, alt_dm: Some(1234) };

    fn f64_at(b: &[u8], o: usize) -> f64 {
        f64::from_le_bytes(b[6 + o..6 + o + 8].try_into().unwrap())
    }

    fn f32_at(b: &[u8], o: usize) -> f32 {
        f32::from_le_bytes(b[6 + o..6 + o + 4].try_into().unwrap())
    }

    /// GPS-38: the frame byte for byte, its checksum by CASIC 3.2's rule worked here apart.
    #[test]
    fn gps_38_frame() {
        let mut b = [0u8; AID_LEN];
        // 2026-11-04 12:00:00.250 UTC.
        let n = aid_ini(Some(HOME), Some((1_793_793_600, 250)), &mut b).unwrap();
        assert_eq!(n, 66);
        assert_eq!(b[..6], [0xBA, 0xCE, 56, 0, 0x0B, 0x01]);
        assert!((f64_at(&b, 0) - 49.0912345).abs() < 1e-5); // f32: under a metre
        assert!((f64_at(&b, 8) + 122.6812345).abs() < 1e-5);
        assert!((f64_at(&b, 16) - 123.4).abs() < 1e-4);
        // GPS week 2443 (from Sunday 2026-11-01), Wednesday 12:00:18.25 GPS time.
        assert_eq!(u16::from_le_bytes([b[6 + 52], b[6 + 53]]), 2443);
        assert!((f64_at(&b, 24) - (3.0 * 86_400.0 + 12.0 * 3600.0 + 18.25)).abs() < 0.07);
        assert_eq!(f32_at(&b, 32), 0.0);
        assert_eq!(f32_at(&b, 36), 25e6);
        assert_eq!(f32_at(&b, 40), AID_TACC);
        assert_eq!(f32_at(&b, 44), 0.0);
        assert_eq!(b[6 + 48..6 + 52], [0; 4]);
        assert_eq!(b[6 + 54], 0);
        assert_eq!(b[6 + 55], 0b0010_0011);
        // The checksum: the header word (id, class, len) plus the payload as little-endian words.
        let mut sum: u32 = (0x01 << 24) + (0x0B << 16) + 56;
        for i in 0..14 {
            let o = 6 + 4 * i;
            sum = sum.wrapping_add(b[o] as u32 | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16 | (b[o + 3] as u32) << 24);
        }
        assert_eq!(b[62..66], sum.to_le_bytes());
    }

    /// GPS-38: a known small frame's checksum by hand.
    #[test]
    fn gps_38_casbin_checksum() {
        let mut b = [0u8; 14];
        assert_eq!(casbin(0x06, 0x01, &[1, 2, 3, 4], &mut b), 14);
        // 0x01060004 + 0x04030201 = 0x05090205.
        assert_eq!(b, [0xBA, 0xCE, 4, 0, 0x06, 0x01, 1, 2, 3, 4, 0x05, 0x02, 0x09, 0x05]);
    }

    /// GPS-38: without the radio's clock, the position alone: no time-valid bit, no week, time or its error.
    #[test]
    fn gps_38_no_time() {
        let mut b = [0u8; AID_LEN];
        aid_ini(Some(HOME), None, &mut b).unwrap();
        assert_eq!(b[6 + 55], AID_POS_VALID | AID_LLA);
        assert_eq!(f64_at(&b, 24), 0.0);
        assert_eq!(f32_at(&b, 40), 0.0);
        assert_eq!(b[6 + 52..6 + 54], [0, 0]);
    }

    /// GPS-38: no fix known, nothing is sent.
    #[test]
    fn gps_38_no_fix() {
        let mut b = [0u8; AID_LEN];
        assert_eq!(aid_ini(None, Some((1_793_793_600, 0)), &mut b), None);
    }

    /// GPS-38: a 2D fix's altitude is marked invalid; a report without a fix gives no aid.
    #[test]
    fn gps_38_altitude_and_from_fix() {
        let mut b = [0u8; AID_LEN];
        aid_ini(Some(AidFix { alt_dm: None, ..HOME }), None, &mut b).unwrap();
        assert_eq!(b[6 + 55], AID_POS_VALID | AID_LLA | AID_ALT_INVALID);
        assert_eq!(f64_at(&b, 16), 0.0);
        let f = crate::msg::GpsFix { on: true, fix: 2, lat: 1, lon: 2, alt_dm: 50, ..Default::default() };
        assert_eq!(AidFix::from_fix(&f), Some(AidFix { lat: 1, lon: 2, alt_dm: None }));
        assert_eq!(AidFix::from_fix(&crate::msg::GpsFix { fix: 3, ..f }).unwrap().alt_dm, Some(50));
        assert_eq!(AidFix::from_fix(&crate::msg::GpsFix { fix: 0, ..f }), None);
        assert_eq!(AidFix::from_fix(&crate::msg::GpsFix { on: false, ..f }), None);
    }

    /// GPS-38: R8 widened by hand matches the f64 of the same f32.
    #[test]
    fn gps_38_r8() {
        for x in [0.0f32, -0.0, 1.0, -122.68123, 49.091234, 302418.25, 1e-7, 3.4e38] {
            assert_eq!(r8(x), (x as f64).to_le_bytes(), "{x}");
        }
    }

    /// GPS-38: GPS time at its epoch and across the week.
    #[test]
    fn gps_38_gps_time() {
        assert_eq!(gps_time(GPS_EPOCH_UNIX - LEAP_S, 0), (0, 0));
        assert_eq!(gps_time(GPS_EPOCH_UNIX - LEAP_S + 604_800 + 1, 500), (1, 1500));
    }
}
