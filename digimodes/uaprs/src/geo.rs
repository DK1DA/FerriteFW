//! Distance and bearing between positions (millionths of a degree), integer only. A flat-earth approximation
//! around the midpoint: within 0.5% to about 1000 km, ample for a radio's heard list.

use udsp::trig::cos_sin;

/// Metres per millionth of a degree of latitude, x 2^20 (111,195 m a degree).
const M_PER_UDEG_Q20: i64 = 116_598;

/// Distance in metres and bearing in degrees (0 to 359, from north) from `a` to `b`, each `(lat, lon)`.
pub fn distance_bearing(a: (i32, i32), b: (i32, i32)) -> (u32, u16) {
    let dlat = (b.0 - a.0) as i64;
    let mut dlon = (b.1 as i64 - a.1 as i64) % 360_000_000;
    if dlon > 180_000_000 {
        dlon -= 360_000_000;
    } else if dlon < -180_000_000 {
        dlon += 360_000_000;
    }
    // cos of the mid latitude: 4096 table steps a turn.
    let mid = (a.0 as i64 + b.0 as i64) / 2;
    let k = (mid.unsigned_abs() * 4096 / 360_000_000) as usize;
    let (c, _) = cos_sin(k);
    let north = (dlat * M_PER_UDEG_Q20) >> 20;
    let east = (((dlon * c as i64) >> 15) * M_PER_UDEG_Q20) >> 20;
    let d = ((north * north + east * east) as u64).isqrt() as u32;
    (d, bearing(north, east))
}

/// Degrees from north of the vector (north, east), to about 0.3 degrees.
fn bearing(north: i64, east: i64) -> u16 {
    if north == 0 && east == 0 {
        return 0;
    }
    let (an, ae) = (north.unsigned_abs(), east.unsigned_abs());
    // atan(x) in degrees for 0 <= x <= 1: 45 x + 15.64 x (1 - x), x in Q15.
    let (num, den) = if ae <= an { (ae, an) } else { (an, ae) };
    let x = ((num << 15) / den) as i64;
    let a = (45 * x + ((1564 * x * ((1 << 15) - x)) >> 15) / 100 + (1 << 14)) >> 15;
    let a = if ae <= an { a } else { 90 - a }; // angle from north within the quadrant
    let deg = match (north >= 0, east >= 0) {
        (true, true) => a,
        (false, true) => 180 - a,
        (false, false) => 180 + a,
        (true, false) => 360 - a,
    };
    (deg % 360) as u16
}
