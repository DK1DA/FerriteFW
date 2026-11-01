//! Single-precision maths for satellite mode (SAT-4): `core` has no `sin` or `sqrt` without `std`, and the radio
//! has the FPU, so these are short f32 versions good to about one unit in the last place over the angles SGP4
//! uses (a mean anomaly grows to about 1,400 rad in 14 days). Sine and cosine reduce by π/2 in three parts
//! (Cody and Waite) and use Cephes' minimax polynomials; the arctangent is Cephes' `atanf`.

use core::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

/// The square root: the FPU's `vsqrt` on the radio, Newton's method from a bit-level first guess elsewhere.
#[inline]
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    #[cfg(all(target_arch = "arm", target_feature = "vfp2"))]
    {
        let r: f32;
        // SAFETY: vsqrt.f32 reads one single-precision register and writes another; nothing else.
        unsafe { core::arch::asm!("vsqrt.f32 {r}, {x}", r = out(sreg) r, x = in(sreg) x, options(pure, nomem, nostack)) };
        r
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "vfp2")))]
    {
        let mut r = f32::from_bits(0x1fbd_1df5 + (x.to_bits() >> 1));
        r = 0.5 * (r + x / r);
        r = 0.5 * (r + x / r);
        0.5 * (r + x / r)
    }
}

/// Rounds to the nearest whole number (half away from zero), for |x| < 2³¹.
#[inline]
fn round(x: f32) -> f32 {
    (if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i32 as f32
}

/// The largest whole number not above x, for |x| < 2³¹.
#[inline]
pub fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

/// x reduced to [0, 2π).
#[inline]
pub fn wrap_tau(x: f32) -> f32 {
    let r = x - floor(x * (1.0 / TAU)) * TAU;
    if r >= TAU { r - TAU } else if r < 0.0 { r + TAU } else { r }
}

/// x reduced to (-π, π].
#[inline]
pub fn wrap_pi(x: f32) -> f32 {
    let r = wrap_tau(x);
    if r > PI { r - TAU } else { r }
}

/// Sine and cosine together (one reduction).
#[inline(never)]
pub fn sin_cos(x: f32) -> (f32, f32) {
    // π/2 in three parts: the first two have few enough bits that k × part is exact for |k| < 2¹⁵.
    const P1: f32 = 1.570_312_5;
    const P2: f32 = 4.837_513e-4;
    const P3: f32 = 7.549_79e-8;
    let k = round(x * (2.0 / PI));
    let r = ((x - k * P1) - k * P2) - k * P3;
    let z = r * r;
    let s = r + r * z * (-1.666_665_5e-1 + z * (8.332_161e-3 + z * -1.951_529_6e-4));
    let c = 1.0 - 0.5 * z + z * z * (4.166_664_6e-2 + z * (-1.388_731_6e-3 + z * 2.443_315_7e-5));
    match (k as i32) & 3 {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

#[inline]
pub fn sin(x: f32) -> f32 {
    sin_cos(x).0
}

#[inline]
pub fn cos(x: f32) -> f32 {
    sin_cos(x).1
}

/// The arctangent of y / x in (-π, π], 0 for (0, 0).
#[inline(never)]
pub fn atan2(y: f32, x: f32) -> f32 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let (ax, ay) = (x.abs(), y.abs());
    // atan of the smaller over the larger, in [0, 1], then the octant
    let (n, d, swap) = if ay > ax { (ax, ay, true) } else { (ay, ax, false) };
    let t = n / d;
    // Cephes atanf: below tan(π/8) the polynomial, above it shift by π/4
    let (t, base) = if t > 0.414_213_57 { ((t - 1.0) / (t + 1.0), FRAC_PI_4) } else { (t, 0.0) };
    let z = t * t;
    let a = base + t + t * z * (((8.053_745e-2 * z - 1.387_768_5e-1) * z + 1.997_771_1e-1) * z - 3.333_295e-1);
    let a = if swap { FRAC_PI_2 - a } else { a };
    let a = if x < 0.0 { PI - a } else { a };
    if y < 0.0 { -a } else { a }
}

/// The cube root of a positive x (the semi-major axis from the mean motion, once per element set).
pub fn cbrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut r = f32::from_bits(x.to_bits() / 3 + 0x2a51_67af);
    for _ in 0..4 {
        r -= (r * r * r - x) / (3.0 * r * r);
    }
    r
}

/// Degrees to radians.
#[inline]
pub fn rad(deg: f32) -> f32 {
    deg * (PI / 180.0)
}

/// Radians to degrees.
#[inline]
pub fn deg(rad: f32) -> f32 {
    rad * (180.0 / PI)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trig_against_std() {
        let mut worst = 0f64;
        let mut x = -1500.0f32;
        while x < 1500.0 {
            let (s, c) = sin_cos(x);
            let xd = x as f64;
            worst = worst.max((s as f64 - xd.sin()).abs()).max((c as f64 - xd.cos()).abs());
            x += 0.0137;
        }
        assert!(worst < 2e-7, "sin/cos worst {worst}");
        let mut worst = 0f64;
        for i in -200..=200 {
            for j in -200..=200 {
                let (y, x) = (i as f32 * 0.37, j as f32 * 0.29);
                let e = (atan2(y, x) as f64 - (y as f64).atan2(x as f64)).abs();
                worst = worst.max(if e > 6.0 { (e - core::f64::consts::TAU).abs() } else { e });
            }
        }
        assert!(worst < 3e-7, "atan2 worst {worst}");
    }

    #[test]
    fn roots() {
        for x in [1e-6f32, 0.01, 0.5, 1.0, 2.0, 1234.5, 6.7e6] {
            assert!(((sqrt(x) as f64) - (x as f64).sqrt()).abs() / (x as f64).sqrt() < 2e-7, "sqrt {x}");
            assert!(((cbrt(x) as f64) - (x as f64).cbrt()).abs() / (x as f64).cbrt() < 3e-7, "cbrt {x}");
        }
        assert_eq!(sqrt(-1.0), 0.0);
        assert!((wrap_tau(-0.5) - (TAU - 0.5)).abs() < 1e-6 && (wrap_pi(4.0) - (4.0 - TAU)).abs() < 1e-6);
        assert_eq!(floor(-0.5), -1.0);
        assert_eq!(floor(2.0), 2.0);
    }
}
