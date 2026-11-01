//! Where a satellite is from the observer (SAT-7, SAT-17): sidereal time from whole UTC seconds in fixed point
//! (an f32 day count alone costs 2-3°, research §5), the observer on the WGS84 ellipsoid, azimuth, elevation,
//! range and range rate; Doppler per side (SAT-8); the sun's azimuth and elevation (SAT-19); the bearing on a
//! magnetic compass (SAT-18).

use super::fmath::{atan2, deg, rad, sin_cos, sqrt, wrap_tau};
use super::sgp4::{Error, Sgp4, State};
use super::Elements;
use crate::freq::Freq10;

/// The earth's rotation, rad/s.
const OMEGA: f32 = 7.292_116e-5;
/// Light, km/s.
pub const C_KM_S: f32 = 299_792.47;
/// UTC seconds since 1970 of J2000.0 (2000-01-01 12:00).
const J2000_NOON: u32 = 946_728_000;

/// Greenwich mean sidereal time (IAU 1982, as the `sgp4` crate's `iau_epoch_to_sidereal_time`) at UTC `unix`, in
/// radians [0, 2π). The linear part is a fraction of a turn in 64-bit fixed point (wrapping is the modulo), exact
/// to about 1e-11 turn over decades; the quadratic term (under 0.1 s) in f32.
pub fn gmst(unix: u32) -> f32 {
    // 67310.54841 s at J2000.0 and (876600 h + 8640184.812866 s) per Julian century, as turns × 2⁶⁴
    const C0: u64 = 14_371_070_138_660_965_073;
    const RATE: u64 = 214_088_536_884_267; // per second
    let s = unix as i64 - J2000_NOON as i64;
    let frac = C0.wrapping_add((s as u64).wrapping_mul(RATE));
    let c = s as f32 / 3_155_760_000.0; // Julian centuries
    let turns = (frac >> 40) as f32 * (1.0 / 16_777_216.0) + 0.093_104 * c * c / 86_400.0;
    wrap_tau(turns * core::f32::consts::TAU)
}

/// An observer: geodetic latitude and longitude, its sines and cosines, and its place in earth-fixed axes (km).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Observer {
    pub lat: f32,
    pub lon: f32,
    sl: f32,
    cl: f32,
    so: f32,
    co: f32,
    xyz: [f32; 3],
}

impl Observer {
    /// From latitude and longitude in 1e-7 degree (the GPS's units) and height in metres.
    pub fn new(lat_e7: i32, lon_e7: i32, alt_m: i32) -> Observer {
        let split = |v: i32| rad((v / 10_000_000) as f32) + rad((v % 10_000_000) as f32 * 1e-7);
        let (lat, lon) = (split(lat_e7), split(lon_e7));
        let (sl, cl) = sin_cos(lat);
        let (so, co) = sin_cos(lon);
        // WGS84
        const A: f32 = 6378.137;
        const E2: f32 = 6.694_38e-3;
        let n = A / sqrt(1.0 - E2 * sl * sl);
        let h = alt_m as f32 * 1e-3;
        let xyz = [(n + h) * cl * co, (n + h) * cl * so, (n * (1.0 - E2) + h) * sl];
        Observer { lat, lon, sl, cl, so, co, xyz }
    }

    /// Azimuth (degrees from true north, [0, 360)), elevation (degrees), range (km) and range rate (km/s,
    /// positive going away) of a TEME state at UTC `unix`.
    pub fn look(&self, s: &State, unix: u32) -> Look {
        let (st, ct) = sin_cos(gmst(unix));
        // TEME to earth-fixed (polar motion left out), and the velocity seen from the turning earth
        let x = ct * s.r[0] + st * s.r[1];
        let y = -st * s.r[0] + ct * s.r[1];
        let z = s.r[2];
        let vx = ct * s.v[0] + st * s.v[1] + OMEGA * y;
        let vy = -st * s.v[0] + ct * s.v[1] - OMEGA * x;
        let vz = s.v[2];
        let (dx, dy, dz) = (x - self.xyz[0], y - self.xyz[1], z - self.xyz[2]);
        let range = sqrt(dx * dx + dy * dy + dz * dz);
        let rate = (dx * vx + dy * vy + dz * vz) / range;
        let south = self.sl * self.co * dx + self.sl * self.so * dy - self.cl * dz;
        let east = -self.so * dx + self.co * dy;
        let up = self.cl * self.co * dx + self.cl * self.so * dy + self.sl * dz;
        let el = deg(atan2(up, sqrt(south * south + east * east)));
        let mut az = deg(atan2(east, -south));
        if az < 0.0 {
            az += 360.0;
        }
        Look { az, el, range, rate }
    }

    /// The sun's azimuth and elevation in degrees at UTC `unix` (the Astronomical Almanac's low-precision formula,
    /// about 0.01° in the sun's place; SAT-19 needs a degree).
    pub fn sun(&self, unix: u32) -> (f32, f32) {
        let s = unix as i64 - J2000_NOON as i64;
        let (d, f) = (s.div_euclid(86_400), s.rem_euclid(86_400) as f32 / 86_400.0);
        let turn = |a: f32, r: f32| ((r * d as f32) % 360.0 + r * f + a) % 360.0;
        let l = turn(280.460, 0.985_647_4);
        let g = rad(turn(357.528, 0.985_600_3));
        let (sg, cg) = sin_cos(g);
        let lam = rad(l + 1.915 * sg + 0.040 * sg * cg);
        let eps = rad(23.439 - 4e-7 * (d as f32));
        let (sl, cl) = sin_cos(lam);
        let (se, ce) = sin_cos(eps);
        // the sun as a far-away unit vector in TEME-like axes, then the same rotation and local axes
        let r = [cl, ce * sl, se * sl];
        let (st, ct) = sin_cos(gmst(unix));
        let x = ct * r[0] + st * r[1];
        let y = -st * r[0] + ct * r[1];
        let z = r[2];
        let south = self.sl * self.co * x + self.sl * self.so * y - self.cl * z;
        let east = -self.so * x + self.co * y;
        let up = self.cl * self.co * x + self.cl * self.so * y + self.sl * z;
        let mut az = deg(atan2(east, -south));
        if az < 0.0 {
            az += 360.0;
        }
        (az, deg(atan2(up, sqrt(south * south + east * east))))
    }
}

/// Where a satellite is from the observer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    pub az: f32,
    pub el: f32,
    pub range: f32,
    pub rate: f32,
}

impl Look {
    /// The Doppler shift heard on a downlink at `f`, in 10 Hz (positive while the satellite comes closer).
    pub fn down_shift(&self, f: Freq10) -> i32 {
        r10(-(f as f32) * self.rate / C_KM_S)
    }
    /// The correction to transmit on an uplink at `f` so the satellite hears `f`, in 10 Hz (negative while it
    /// comes closer).
    pub fn up_shift(&self, f: Freq10) -> i32 {
        r10(f as f32 * self.rate / C_KM_S)
    }
}

fn r10(x: f32) -> i32 {
    (if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i32
}

/// One satellite tracked from one observer: its SGP4 constants, where its epoch is, the observer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracker {
    pub g: Sgp4,
    pub epoch: u32,
    pub obs: Observer,
}

impl Tracker {
    pub fn new(e: &Elements, obs: Observer) -> Result<Tracker, Error> {
        Ok(Tracker { g: Sgp4::new(e)?, epoch: e.epoch_unix(), obs })
    }

    /// Where the satellite is at UTC `unix`.
    #[inline(never)]
    pub fn look(&self, unix: u32) -> Result<Look, Error> {
        let secs = unix.wrapping_sub(self.epoch) as i32;
        let s = self.g.at(secs)?;
        Ok(self.obs.look(&s, unix))
    }
}

/// SAT-18: the bearing on a magnetic compass for a true bearing, with the declination in 0.1° (east positive),
/// in whole degrees [0, 360).
pub fn magnetic(true_deg: f32, decl_10: i16) -> u16 {
    let m = true_deg - decl_10 as f32 * 0.1;
    ((m + 360.5) as i32 % 360) as u16
}

/// SAT-19: where the satellite is from the sun, in degrees: across (negative left, positive right, [-180, 180))
/// and up (positive above).
pub fn from_sun(sat: (f32, f32), sun: (f32, f32)) -> (i16, i16) {
    let mut d = sat.0 - sun.0;
    while d >= 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    (r10(d) as i16, r10(sat.1 - sun.1) as i16)
}

/// SAT-17: the compass point of a bearing, 16 of them ("SSW").
pub fn point16(deg: f32) -> &'static str {
    const P: [&str; 16] = ["N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW"];
    P[((deg * (16.0 / 360.0) + 0.5) as i32 & 15) as usize]
}

/// The rise direction for the toast and the plot ("SW"): 8 points.
pub fn point8(deg: f32) -> &'static str {
    const P: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    P[((deg * (8.0 / 360.0) + 0.5) as i32 & 7) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SAT-4: sidereal time from whole seconds in fixed point, against the f64 IAU expression.
    #[test]
    fn sidereal_time_matches_the_iau_formula() {
        // f64 reference, the sgp4 crate's expression
        for unix in [946_728_000u32, 1_000_000_000, 1_791_383_530, 1_793_500_000, 2_100_000_000] {
            let years = (unix as f64 - 946_728_000.0) / 86_400.0 / 365.25;
            let want = sgp4::iau_epoch_to_sidereal_time(years);
            let got = gmst(unix) as f64;
            let mut e = (got - want).abs();
            if e > 3.0 {
                e = (e - core::f64::consts::TAU).abs();
            }
            assert!(e < 2e-6, "gmst at {unix}: {got} vs {want}");
        }
    }

    /// SAT-19: the sun's place within 0.1° of a fuller formula (the rule asks for 1°).
    #[test]
    fn sun_against_a_fuller_formula() {
        // Meeus chapter 25 (the equation of centre to three terms, the true obliquity left at its mean) in f64
        let obs = Observer::new(453_000_000, -756_000_000, 70);
        let mut worst = 0f64;
        for k in 0..400u32 {
            let unix = 1_790_000_000 + k * 77_777;
            let jd = unix as f64 / 86_400.0 + 2_440_587.5;
            let t = (jd - 2_451_545.0) / 36_525.0;
            let l0 = 280.46646 + 36000.76983 * t;
            let m = (357.52911 + 35999.05029 * t).to_radians();
            let c = (1.914602 - 0.004817 * t) * m.sin() + (0.019993 - 0.000101 * t) * (2.0 * m).sin() + 0.000289 * (3.0 * m).sin();
            let lam = (l0 + c - 0.00569).to_radians();
            let eps = (23.439291 - 0.0130042 * t).to_radians();
            let ra = (eps.cos() * lam.sin()).atan2(lam.cos());
            let dec = (eps.sin() * lam.sin()).asin();
            let theta = sgp4::iau_epoch_to_sidereal_time((unix as f64 - 946_728_000.0) / 86_400.0 / 365.25);
            let h = theta + (-75.6f64).to_radians() - ra;
            let phi = 45.3f64.to_radians();
            let el = (phi.sin() * dec.sin() + phi.cos() * dec.cos() * h.cos()).asin().to_degrees();
            let az = (-h.sin()).atan2(dec.tan() * phi.cos() - phi.sin() * h.cos()).to_degrees().rem_euclid(360.0);
            let (gaz, gel) = obs.sun(unix);
            let mut da = (gaz as f64 - az).abs();
            if da > 180.0 {
                da = 360.0 - da;
            }
            worst = worst.max((gel as f64 - el).abs()).max(da * el.to_radians().cos());
        }
        assert!(worst < 0.1, "sun worst {worst}°");
    }

    /// SAT-17, SAT-18, SAT-19: the compass point, the magnetic bearing from a declination, the angle from the sun.
    #[test]
    fn compass_and_sun_aids() {
        assert_eq!(magnetic(212.0, 160), 196); // the mock-up: 212° true, 16° E
        assert_eq!(magnetic(5.0, 160), 349);
        assert_eq!(magnetic(350.0, -200), 10);
        assert_eq!(from_sun((212.0, 61.0), (222.0, 16.0)), (-10, 45)); // 10° left, 45° above
        assert_eq!(from_sun((5.0, 20.0), (355.0, 30.0)), (10, -10));
        assert_eq!(point16(212.0), "SSW");
        assert_eq!(point16(359.0), "N");
        assert_eq!(point8(225.0), "SW");
    }

    /// SAT-8: Doppler's signs on each side.
    #[test]
    fn doppler_signs() {
        let l = Look { rate: -6.85, ..Default::default() }; // coming closer at 6.85 km/s
        assert_eq!(l.down_shift(43_679_500), 998); // 436.795 heard 9.98 kHz high
        assert_eq!(l.up_shift(14_585_000), -333); // 145.850 sent 3.33 kHz low
    }
}
