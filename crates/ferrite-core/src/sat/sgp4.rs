//! Near-Earth SGP4 in single precision (SAT-4): our trimmed port of the `sgp4` crate's near-Earth branch
//! (`src/lib.rs`, `src/near_earth.rs`), with the deep-space branch (SDP4, periods over 225 minutes) left out, as
//! every FM satellite is in low orbit. WGS84 constants, as the crate's `Constants::from_elements`. Time is minutes
//! since the element set's epoch, from whole seconds ([`Sgp4::at`]): never an f32 day count.
//!
//! Ported from <https://github.com/neuromorphicsystems/sgp4> (version 2.4.0), under its MIT licence:
//!
//! > MIT License
//! >
//! > Copyright (c) 2020 International Centre for Neuromorphic Systems
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! > documentation files (the "Software"), to deal in the Software without restriction, including without
//! > limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the
//! > Software, and to permit persons to whom the Software is furnished to do so, subject to the following
//! > conditions:
//! >
//! > The above copyright notice and this permission notice shall be included in all copies or substantial
//! > portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT
//! > LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO
//! > EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN
//! > AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE
//! > OR OTHER DEALINGS IN THE SOFTWARE.

use super::fmath::{cbrt, sin_cos, sqrt, atan2, wrap_tau};
use super::Elements;

/// WGS84 (the crate's default geopotential): equatorial radius (km), √μ in earth radii³ᐟ² per minute, the zonal
/// harmonics.
pub const AE: f32 = 6378.137;
const KE: f32 = 0.074_366_85;
const J2: f32 = 0.001_082_63;
const J3: f32 = -0.000_002_532_153;
const J4: f32 = -0.000_001_610_987_6;

/// Why an element set can't be used, or a time can't be propagated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Eccentricity outside [0, 1), or a mean motion of 0.
    Elements,
    /// A period over 225 minutes: deep space, which this port leaves out (no FM satellite is there).
    DeepSpace,
    /// The orbit decayed by then (the drag terms drove eccentricity or the semi-latus rectum out of range).
    Decayed,
}

/// Position (km) and velocity (km/s) in the TEME frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub r: [f32; 3],
    pub v: [f32; 3],
}

/// The precomputed constants for one element set (about 120 bytes): the crate's `Constants` for the near-Earth
/// method, flattened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sgp4 {
    // the orbit at epoch (Brouwer mean motion, radians per minute)
    incl: f32,
    raan0: f32,
    ecc0: f32,
    argp0: f32,
    ma0: f32,
    n0: f32,
    // secular rates and drag
    raan_dot: f32,
    argp_dot: f32,
    ma_dot: f32,
    c1: f32,
    c4: f32,
    k0: f32,
    k1: f32,
    a0: f32,
    k2: f32,
    k3: f32,
    k4: f32,
    k5: f32,
    k6: f32,
    sin_i: f32,
    cos_i: f32,
    /// The higher-order drag terms; all zero (and unused) for a perigee under 220 km (the crate's
    /// `HighAltitude::No`).
    simple: bool,
    elliptic: bool,
    c5: f32,
    d2: f32,
    d3: f32,
    d4: f32,
    eta: f32,
    k7: f32,
    k8: f32,
    k9: f32,
    k10: f32,
    k11: f32,
    k12: f32,
    k13: f32,
    /// Seconds from the element epoch's whole second to the epoch itself (its milliseconds).
    pub epoch_ms: u16,
}

#[inline]
fn sq(x: f32) -> f32 {
    x * x
}

impl Sgp4 {
    /// The constants for an element set (the crate's `Orbit::from_kozai_elements` and `Constants::new`).
    #[inline(never)]
    pub fn new(e: &Elements) -> Result<Sgp4, Error> {
        let incl = e.incl_rad();
        let ecc0 = e.ecc();
        let kozai = e.n_rad_min();
        if !(0.0..1.0).contains(&ecc0) || kozai <= 0.0 {
            return Err(Error::Elements);
        }
        let (sin_i, cos_i) = sin_cos(incl);
        // the Brouwer mean motion from the Kozai one
        let a1 = sq(cbrt(KE / kozai));
        let b2 = 1.0 - ecc0 * ecc0;
        let b0 = sqrt(b2);
        let p0 = 0.75 * J2 * (3.0 * cos_i * cos_i - 1.0) / (b2 * b0);
        let d1 = p0 / (a1 * a1);
        let d0 = p0 / sq(a1 * (1.0 - d1 * d1 - d1 * (1.0 / 3.0 + 134.0 * d1 * d1 / 81.0)));
        let n0 = kozai / (1.0 + d0);
        if n0 <= 0.0 {
            return Err(Error::Elements);
        }
        if n0 <= core::f32::consts::TAU / 225.0 {
            return Err(Error::DeepSpace);
        }
        let p1 = cos_i;
        let p2 = b2;
        let k6 = 3.0 * p1 * p1 - 1.0;
        let a0 = sq(cbrt(KE / n0));
        let p3 = a0 * (1.0 - ecc0);
        let p4 = AE * (p3 - 1.0);
        let p5 = if p4 < 98.0 {
            20.0
        } else if p4 < 156.0 {
            p4 - 78.0
        } else {
            78.0
        };
        let s = p5 / AE + 1.0;
        let p6 = sq(sq((120.0 - p5) / AE));
        let xi = 1.0 / (a0 - s);
        let p7 = p6 * sq(sq(xi));
        let eta = a0 * ecc0 * xi;
        let p8 = (1.0 - eta * eta).abs();
        let p9 = p7 / (p8 * p8 * p8 * sqrt(p8));
        let bstar = e.bstar.get();
        let c1 = bstar
            * (p9 * n0 * (a0 * (1.0 + 1.5 * eta * eta + ecc0 * eta * (4.0 + eta * eta)) + 0.375 * J2 * xi / p8 * k6 * (8.0 + 3.0 * eta * eta * (8.0 + eta * eta))));
        let p10 = 1.0 / sq(a0 * p2);
        let p11 = 1.5 * J2 * p10 * n0;
        let p12 = 0.5 * p11 * J2 * p10;
        let p13 = -0.468_75 * J4 * p10 * p10 * n0;
        let pp = p1 * p1;
        let p14 = -p11 * p1 + (0.5 * p12 * (4.0 - 19.0 * pp) + 2.0 * p13 * (3.0 - 7.0 * pp)) * p1;
        let k14 = -0.5 * p11 * (1.0 - 5.0 * pp) + 0.0625 * p12 * (7.0 - 114.0 * pp + 395.0 * pp * pp) + p13 * (3.0 - 36.0 * pp + 49.0 * pp * pp);
        let p15 = n0 + 0.5 * p11 * b0 * k6 + 0.0625 * p12 * b0 * (13.0 - 78.0 * pp + 137.0 * pp * pp);
        let argp0 = e.argp_rad();
        let (_, cos_2w) = sin_cos(2.0 * argp0);
        let c4 = bstar
            * (2.0
                * n0
                * p9
                * a0
                * p2
                * (eta * (2.0 + 0.5 * eta * eta) + ecc0 * (0.5 + 2.0 * eta * eta)
                    - J2 * xi / (a0 * p8)
                        * (-3.0 * k6 * (1.0 - 2.0 * ecc0 * eta + eta * eta * (1.5 - 0.5 * ecc0 * eta))
                            + 0.75 * (1.0 - pp) * (2.0 * eta * eta - ecc0 * eta * (1.0 + eta * eta)) * cos_2w)));
        let k0 = 3.5 * p2 * (-p11 * p1) * c1;
        let k1 = 1.5 * c1;
        let j3j2 = J3 / J2;
        let k2 = -0.5 * j3j2 * sin_i;
        let k3 = 1.0 - pp;
        let k4 = 7.0 * pp - 1.0;
        let k5 = -0.25 * j3j2 * sin_i * (3.0 + 5.0 * p1) / if (1.0 + p1).abs() > 1.5e-12 { 1.0 + p1 } else { 1.5e-12 };
        let ma0 = e.ma_rad();
        let mut g = Sgp4 {
            incl,
            raan0: e.raan_rad(),
            ecc0,
            argp0,
            ma0,
            n0,
            raan_dot: p14,
            argp_dot: k14,
            ma_dot: p15,
            c1,
            c4,
            k0,
            k1,
            a0,
            k2,
            k3,
            k4,
            k5,
            k6,
            sin_i,
            cos_i,
            simple: p3 < 220.0 / AE + 1.0,
            elliptic: false,
            c5: 0.0,
            d2: 0.0,
            d3: 0.0,
            d4: 0.0,
            eta,
            k7: 0.0,
            k8: 0.0,
            k9: 0.0,
            k10: 0.0,
            k11: 0.0,
            k12: 0.0,
            k13: 0.0,
            epoch_ms: e.epoch_ms,
        };
        if !g.simple {
            let d2 = 4.0 * a0 * xi * c1 * c1;
            let p16 = d2 * xi * c1 / 3.0;
            let d3 = (17.0 * a0 + s) * p16;
            let d4 = 0.5 * p16 * a0 * xi * (221.0 * a0 + 31.0 * s) * c1;
            let (sin_m, cos_m) = sin_cos(ma0);
            g.c5 = bstar * (2.0 * p9 * a0 * p2 * (1.0 + 2.75 * (eta * eta + eta * ecc0) + eta * ecc0 * eta * eta));
            (g.d2, g.d3, g.d4) = (d2, d3, d4);
            g.k7 = sin_m;
            g.k8 = d2 + 2.0 * c1 * c1;
            g.k9 = 0.25 * (3.0 * d3 + c1 * (12.0 * d2 + 10.0 * c1 * c1));
            g.k10 = 0.2 * (3.0 * d4 + 12.0 * c1 * d3 + 6.0 * d2 * d2 + 15.0 * c1 * c1 * (2.0 * d2 + c1 * c1));
            if ecc0 > 1.0e-4 {
                g.elliptic = true;
                let c = 1.0 + eta * cos_m;
                g.k11 = c * c * c;
                let (_, cos_w) = sin_cos(argp0);
                g.k12 = bstar * (-2.0 * p7 * xi * j3j2 * n0 * sin_i / ecc0) * cos_w;
                g.k13 = -2.0 / 3.0 * p7 * bstar / (ecc0 * eta);
            }
        }
        Ok(g)
    }

    /// The state `secs` whole seconds after the epoch's whole second (negative before it).
    #[inline]
    pub fn at(&self, secs: i32) -> Result<State, Error> {
        self.propagate((secs as f32 - self.epoch_ms as f32 * 1e-3) * (1.0 / 60.0))
    }

    /// The state `t` minutes after the epoch (the crate's `propagate_from_state`, near-Earth only).
    #[inline(never)]
    pub fn propagate(&self, t: f32) -> Result<State, Error> {
        let t2 = t * t;
        let raan = self.raan0 + self.raan_dot * t + self.k0 * t2;
        let p23 = self.argp0 + self.argp_dot * t;
        let p24 = self.ma0 + self.ma_dot * t;
        let (argp, ma, a, p27) = if self.simple {
            (p23, p24 + self.n0 * self.k1 * t2, self.a0 * sq(1.0 - self.c1 * t), self.ecc0 - self.c4 * t)
        } else {
            let (argp, p26) = if self.elliptic {
                let (_, cos24) = sin_cos(p24);
                let c = 1.0 + self.eta * cos24;
                let p25 = self.k13 * (c * c * c - self.k11) + self.k12 * t;
                (p23 - p25, p24 + p25)
            } else {
                (p23, p24)
            };
            let t3 = t2 * t;
            let t4 = t3 * t;
            let (sin26, _) = sin_cos(p26);
            (
                argp,
                p26 + self.n0 * (self.k1 * t2 + self.k8 * t3 + t4 * (self.k9 + t * self.k10)),
                self.a0 * sq(1.0 - self.c1 * t - self.d2 * t2 - self.d3 * t3 - self.d4 * t4),
                self.ecc0 - (self.c4 * t + self.c5 * (sin26 - self.k7)),
            )
        };
        if !(-0.001..1.0).contains(&p27) {
            return Err(Error::Decayed);
        }
        let ecc = p27.max(1.0e-6);
        let n = KE / (a * sqrt(a));
        // Kepler's equation for E + ω
        let p37 = 1.0 / (a * (1.0 - ecc * ecc));
        let (sin_w, cos_w) = sin_cos(argp);
        let axn = ecc * cos_w;
        let ayn = ecc * sin_w + p37 * self.k2;
        let p38 = wrap_tau(ma + argp + p37 * self.k5 * axn);
        let mut ew = p38;
        let (mut s_ew, mut c_ew) = sin_cos(ew);
        for _ in 0..10 {
            let delta = (p38 - ayn * c_ew + axn * s_ew - ew) / (1.0 - c_ew * axn - s_ew * ayn);
            ew += delta.clamp(-0.95, 0.95);
            (s_ew, c_ew) = sin_cos(ew);
            if delta.abs() < 1.0e-6 {
                break;
            }
        }
        let p39 = axn * axn + ayn * ayn;
        let pl = a * (1.0 - p39);
        if pl < 0.0 {
            return Err(Error::Decayed);
        }
        let p40 = axn * s_ew - ayn * c_ew;
        let r = a * (1.0 - (axn * c_ew + ayn * s_ew));
        let r_dot = sqrt(a) * p40 / r;
        let b = sqrt(1.0 - p39);
        let p41 = p40 / (1.0 + b);
        let p42 = a / r * (s_ew - ayn - axn * p41);
        let p43 = a / r * (c_ew - axn + ayn * p41);
        let u = atan2(p42, p43);
        let p44 = 2.0 * p43 * p42;
        let p45 = 1.0 - 2.0 * p42 * p42;
        let hj = 0.5 * J2 / pl;
        let p46 = hj / pl;
        let rk = r * (1.0 - 1.5 * p46 * b * self.k6) + 0.5 * hj * self.k3 * p45;
        let uk = u - 0.25 * p46 * self.k4 * p44;
        let ik = self.incl + 1.5 * p46 * self.cos_i * self.sin_i * p45;
        let raank = raan + 1.5 * p46 * self.cos_i * p44;
        let rk_dot = r_dot - n * hj * self.k3 * p44 / KE;
        let rfk_dot = sqrt(pl) / r + n * hj * (self.k3 * p45 + 1.5 * self.k6) / KE;
        let (sin_o, cos_o) = sin_cos(raank);
        let (sin_ik, cos_ik) = sin_cos(ik);
        let (sin_u, cos_u) = sin_cos(uk);
        let u0 = -sin_o * cos_ik * sin_u + cos_o * cos_u;
        let u1 = cos_o * cos_ik * sin_u + sin_o * cos_u;
        let u2 = sin_ik * sin_u;
        let v0 = -sin_o * cos_ik * cos_u - cos_o * sin_u;
        let v1 = cos_o * cos_ik * cos_u - sin_o * sin_u;
        let v2 = sin_ik * cos_u;
        let rs = rk * AE;
        let vs = AE * KE / 60.0;
        Ok(State {
            r: [rs * u0, rs * u1, rs * u2],
            v: [(rk_dot * u0 + rfk_dot * v0) * vs, (rk_dot * u1 + rfk_dot * v1) * vs, (rk_dot * u2 + rfk_dot * v2) * vs],
        })
    }
}
