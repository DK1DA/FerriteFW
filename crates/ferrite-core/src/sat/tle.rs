//! A two-line element set into [`Elements`] (SAT-3: a pasted TLE; the simulator's and bench builds' sample
//! satellites). Fixed columns, read as integers so nothing is lost to floats; the checksums are not checked
//! (a pasted set that is wrong shows as a wrong pass, and Console sends OMM).

use super::{Elements, F32};

/// A decimal field (spaces around it allowed) as an integer with `places` decimals: "51.6332" with 7 places is
/// 516_332_000.
fn fixed64(s: &[u8], places: u32) -> Option<u64> {
    let s = trim(s);
    let mut v: u64 = 0;
    let mut frac: Option<u32> = None;
    for &c in s {
        match c {
            b'0'..=b'9' => {
                if frac.is_some_and(|f| f >= places) {
                    continue; // beyond the precision kept
                }
                v = v * 10 + (c - b'0') as u64;
                if let Some(f) = frac.as_mut() {
                    *f += 1;
                }
            }
            b'.' if frac.is_none() => frac = Some(0),
            _ => return None,
        }
    }
    let f = frac.unwrap_or(0);
    v.checked_mul(10u64.pow(places - f))
}

fn fixed(s: &[u8], places: u32) -> Option<u32> {
    u32::try_from(fixed64(s, places)?).ok()
}

fn trim(s: &[u8]) -> &[u8] {
    let a = s.iter().position(|&c| c != b' ').unwrap_or(s.len());
    let b = s.iter().rposition(|&c| c != b' ').map_or(a, |i| i + 1);
    &s[a..b]
}

/// B* as a TLE writes it: " 12291-3" is 0.12291e-3.
fn exp_field(s: &[u8]) -> Option<f32> {
    let s = trim(s);
    let (neg, s) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if s.len() < 2 {
        return None;
    }
    let (m, e) = s.split_at(s.len() - 2);
    let p10 = |n: i32| (0..n.unsigned_abs()).fold(1f32, |a, _| if n < 0 { a * 0.1 } else { a * 10.0 });
    let mant = fixed(m, 0)? as f32 * p10(-(m.len() as i32));
    let ev = match e {
        [b'-', d] => -((d - b'0') as i32),
        [b'+' | b' ', d] => (d - b'0') as i32,
        _ => return None,
    };
    let v = mant * p10(ev);
    Some(if neg { -v } else { v })
}

/// Days from 2000-01-01 to January 1 of `year` (2000-2099).
fn days_to_year(year: u32) -> u32 {
    let y = year - 2000;
    y * 365 + y.div_ceil(4)
}

/// Lines 1 and 2 of a TLE (69 columns each; a name line is the caller's).
pub fn parse(l1: &[u8], l2: &[u8]) -> Option<Elements> {
    if l1.len() < 61 || l2.len() < 63 || l1[0] != b'1' || l2[0] != b'2' {
        return None;
    }
    let yy = fixed(&l1[18..20], 0)?;
    if yy >= 57 {
        return None; // 1957-1999: older than any element set worth keeping
    }
    let doy = fixed64(&l1[20..32], 8)?; // day of year with 8 decimals
    let (day, frac) = ((doy / 100_000_000) as u32, doy % 100_000_000);
    if day == 0 {
        return None;
    }
    let ms = frac * 86_400_000 / 100_000_000;
    let epoch = (days_to_year(2000 + yy) + day - 1) * 86_400 + (ms / 1000) as u32;
    Some(Elements {
        epoch,
        epoch_ms: (ms % 1000) as u16,
        incl: fixed(&l2[8..16], 7)?,
        raan: fixed(&l2[17..25], 7)?,
        ecc: fixed(&l2[26..33], 0)?,
        argp: fixed(&l2[34..42], 7)?,
        ma: fixed(&l2[43..51], 7)?,
        n: fixed(&l2[52..63], 8)?,
        bstar: F32::new(exp_field(&l1[53..61])?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const L1: &[u8] = b"1 25544U 98067A   26304.75703384  .00006360  00000+0  12291-3 0  9994";
    const L2: &[u8] = b"2 25544  51.6332 344.2755 0007028 332.5119  27.5497 15.49131714592920";

    #[test]
    fn the_iss() {
        let e = parse(L1, L2).unwrap();
        assert_eq!((e.incl, e.raan, e.ecc, e.argp, e.ma, e.n), (516_332_000, 3_442_755_000, 7028, 3_325_119_000, 275_497_000, 1_549_131_714));
        assert!((e.bstar.get() - 1.2291e-4).abs() < 1e-9);
        // 2026-10-31 18:10:07.723 UTC (the fixture's OMM epoch 2026-10-31T18:10:07.723776)
        assert_eq!(e.epoch_unix(), 1_793_470_207);
        assert_eq!(e.epoch_ms, 723);
    }

    #[test]
    fn refuses_junk() {
        assert!(parse(b"1 short", L2).is_none());
        assert!(parse(L2, L1).is_none());
        assert_eq!(fixed(b" 1.5 ", 2), Some(150));
        assert_eq!(fixed(b"1x", 0), None);
        assert!((exp_field(b"-11606-4").unwrap() + 0.11606e-4).abs() < 1e-10);
        assert_eq!(exp_field(b" 00000+0"), Some(0.0));
    }
}
