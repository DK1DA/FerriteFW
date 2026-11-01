//! SAT-4 and SAT-6 against the oracle: the f64 `sgp4` crate (the reference implementation's port), with f64
//! sidereal time and look angles, over 14 days for the fixture satellites (`testdata/celestrak.json`, CelesTrak's
//! OMM for six FM-era satellites, Oct 31, 2026): pointing within 0.5°, Doppler within 100 Hz at 437.8 MHz, every
//! rise within 10 s.

use super::look::{Observer, Tracker};
use super::pass::Plan;
use super::{Elements, List, Sat, F32, Y2000};
use std::vec::Vec;

const OMM: &str = include_str!("testdata/celestrak.json");

/// The fixture as the oracle's elements and ours (OMM numbers to the record's integers).
fn fixture() -> Vec<(sgp4::Elements, Elements)> {
    let list: Vec<sgp4::Elements> = serde_json::from_str(OMM).unwrap();
    list.into_iter()
        .map(|g| {
            let ts = g.datetime.and_utc();
            let unix = ts.timestamp() as u32;
            let ms = ts.timestamp_subsec_millis() as u16;
            let a = |d: f64| (d * 1e7).round() as u32;
            let e = Elements {
                epoch: unix - Y2000,
                epoch_ms: ms,
                incl: a(g.inclination),
                raan: a(g.right_ascension),
                argp: a(g.argument_of_perigee),
                ma: a(g.mean_anomaly),
                ecc: (g.eccentricity * 1e7).round() as u32,
                n: (g.mean_motion * 1e8).round() as u32,
                bstar: F32::new(g.drag_term as f32),
            };
            (g, e)
        })
        .collect()
}

/// The f64 reference: (az, el, range rate km/s) at UTC `unix`.
struct Ref {
    c: sgp4::Constants,
    epoch: f64,
    lat: f64,
    lon: f64,
    obs: [f64; 3],
}

impl Ref {
    fn new(g: &sgp4::Elements, lat: f64, lon: f64, alt_m: f64) -> Ref {
        let ts = g.datetime.and_utc();
        let epoch = ts.timestamp() as f64 + ts.timestamp_subsec_nanos() as f64 * 1e-9;
        let (a, e2) = (6378.137, 6.69437999014e-3);
        let (la, lo) = (lat.to_radians(), lon.to_radians());
        let n = a / (1.0 - e2 * la.sin().powi(2)).sqrt();
        let h = alt_m / 1000.0;
        let obs = [(n + h) * la.cos() * lo.cos(), (n + h) * la.cos() * lo.sin(), (n * (1.0 - e2) + h) * la.sin()];
        Ref { c: sgp4::Constants::from_elements(g).unwrap(), epoch, lat: la, lon: lo, obs }
    }

    fn look(&self, unix: u32) -> (f64, f64, f64) {
        let p = self.c.propagate(sgp4::MinutesSinceEpoch((unix as f64 - self.epoch) / 60.0)).unwrap();
        let th = sgp4::iau_epoch_to_sidereal_time((unix as f64 - 946_728_000.0) / 86_400.0 / 365.25);
        let (st, ct) = th.sin_cos();
        let w = 7.2921159e-5;
        let r = p.position;
        let v = p.velocity;
        let x = ct * r[0] + st * r[1];
        let y = -st * r[0] + ct * r[1];
        let vx = ct * v[0] + st * v[1] + w * y;
        let vy = -st * v[0] + ct * v[1] - w * x;
        let d = [x - self.obs[0], y - self.obs[1], r[2] - self.obs[2]];
        let range = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let rate = (d[0] * vx + d[1] * vy + d[2] * v[2]) / range;
        let (sl, cl, so, co) = (self.lat.sin(), self.lat.cos(), self.lon.sin(), self.lon.cos());
        let s = sl * co * d[0] + sl * so * d[1] - cl * d[2];
        let e = -so * d[0] + co * d[1];
        let z = cl * co * d[0] + cl * so * d[1] + sl * d[2];
        let el = z.atan2((s * s + e * e).sqrt()).to_degrees();
        let az = e.atan2(-s).to_degrees().rem_euclid(360.0);
        (az, el, rate)
    }
}

/// The angle between two directions given as (az, el) in degrees.
fn apart(a: (f64, f64), b: (f64, f64)) -> f64 {
    let v = |az: f64, el: f64| {
        let (az, el) = (az.to_radians(), el.to_radians());
        [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
    };
    let (p, q) = (v(a.0, a.1), v(b.0, b.1));
    (p[0] * q[0] + p[1] * q[1] + p[2] * q[2]).clamp(-1.0, 1.0).acos().to_degrees()
}

const OBSERVERS: [(f64, f64, f64); 2] = [(45.3, -75.7, 70.0), (49.25, -123.1, 30.0)];

/// SAT-4, SAT-7: pointing and Doppler against the oracle over 14 days.
#[test]
fn pointing_and_doppler_over_14_days() {
    let mut worst_deg = 0f64;
    let mut worst_hz = 0f64;
    let mut samples = 0u32;
    for (g, e) in fixture() {
        for &(lat, lon, alt) in &OBSERVERS {
            let r = Ref::new(&g, lat, lon, alt);
            let obs = Observer::new((lat * 1e7) as i32, (lon * 1e7) as i32, alt as i32);
            let tr = Tracker::new(&e, obs).unwrap();
            let t0 = e.epoch_unix();
            let mut t = t0;
            while t < t0 + 14 * 86_400 {
                let (az, el, rate) = r.look(t);
                if el > 0.0 {
                    let l = tr.look(t).unwrap();
                    worst_deg = worst_deg.max(apart((az, el), (l.az as f64, l.el as f64)));
                    worst_hz = worst_hz.max((l.rate as f64 - rate).abs() / 299_792.458 * 437.8e6);
                    samples += 1;
                }
                t += 10;
            }
        }
    }
    std::println!("SAT-4: {samples} samples above the horizon: worst pointing {worst_deg:.3}°, Doppler {worst_hz:.1} Hz at 437.8 MHz");
    assert!(samples > 10_000);
    assert!(worst_deg < 0.5, "pointing {worst_deg}°");
    assert!(worst_hz < 100.0, "Doppler {worst_hz} Hz");
}

/// SAT-4, SAT-6: every rise within 10 s of the oracle's, no pass over 2° missed.
#[test]
fn rises_within_10_s_over_14_days() {
    let fx = fixture();
    let sats: Vec<Sat> = fx.iter().map(|(_, e)| Sat { el: *e, ..Default::default() }).collect();
    let mut checked = 0;
    let mut worst = 0i64;
    let mut worst_el = 0f64;
    for &(lat, lon, alt) in &OBSERVERS {
        let obs = Observer::new((lat * 1e7) as i32, (lon * 1e7) as i32, alt as i32);
        let refs: Vec<Ref> = fx.iter().map(|(g, _)| Ref::new(g, lat, lon, alt)).collect();
        let t0 = fx.iter().map(|(_, e)| e.epoch_unix()).max().unwrap();
        let mut seen: Vec<(u8, u32)> = Vec::new();
        for day in 0..14 {
            let mut p = Plan::new();
            p.restart(t0 + day * 86_400, 1, Some(obs));
            while !p.step(&List(&sats), 500) {}
            for q in p.list() {
                if seen.iter().any(|&(s, a)| s == q.sat && a.abs_diff(q.aos) < 60) {
                    continue; // the same pass from the day before
                }
                seen.push((q.sat, q.aos));
                let r = &refs[q.sat as usize];
                // the oracle's rise: the first second at or above 0° near ours
                let mut t = q.aos - 120;
                while r.look(t).1 < 0.0 && t < q.aos + 120 {
                    t += 1;
                }
                if q.max_el >= 1 {
                    let d = t as i64 - q.aos as i64;
                    assert!(d.abs() <= 10, "sat {} rise {} off by {d} s", q.sat, q.aos);
                    worst = worst.max(d.abs());
                    let top = r.look(q.aos + q.tmax as u32).1;
                    worst_el = worst_el.max((top - q.max_el as f64).abs());
                    checked += 1;
                }
            }
        }
        // every pass above 2° the oracle finds with a 10 s scan is in the list
        for (i, r) in refs.iter().enumerate() {
            let (mut t, end) = (t0 + 1_800, t0 + 14 * 86_400 - 1_800);
            let mut start: Option<(u32, f64)> = None;
            let mut below = false; // a pass under way at the start isn't counted
            while t < end {
                let el = r.look(t).1;
                match start {
                    None if el >= 0.0 && below => start = Some((t, el)),
                    None if el < 0.0 => below = true,
                    Some((s, m)) if el < 0.0 => {
                        if m > 2.0 {
                            assert!(seen.iter().any(|&(k, a)| k as usize == i && a.abs_diff(s) <= 20), "sat {i} pass at {s} ({m:.1}°) missed");
                        }
                        start = None;
                    }
                    Some((s, m)) => start = Some((s, m.max(el))),
                    None => {}
                }
                t += 10;
            }
        }
    }
    std::println!("SAT-6: {checked} passes: worst rise {worst} s, worst highest point {worst_el:.2}°");
    assert!(checked > 100);
    assert!(worst_el <= 0.6, "highest point {worst_el}°");
}

/// SAT-6: the adaptive coarse step finds the same passes as the plain 30 s step, every day for 14 days.
#[test]
fn adaptive_step_finds_every_pass() {
    let fx = fixture();
    let sats: Vec<Sat> = fx.iter().map(|(_, e)| Sat { el: *e, ..Default::default() }).collect();
    let (mut work, mut plain_work) = (0, 0);
    for &(lat, lon, alt) in &OBSERVERS {
        let obs = Observer::new((lat * 1e7) as i32, (lon * 1e7) as i32, alt as i32);
        let t0 = fx.iter().map(|(_, e)| e.epoch_unix()).max().unwrap();
        for day in 0..14 {
            let run = |adaptive: bool| {
                let mut p = Plan::new();
                p.restart(t0 + day * 86_400, 1, Some(obs));
                p.adaptive = adaptive;
                while !p.step(&List(&sats), 1_000) {}
                p
            };
            let (a, b) = (run(true), run(false));
            // the same passes: rise, length and azimuths; the top within 15 s and 1° (its search starts from a
            // different coarse sample, and a low pass's top is flat to f32's noise)
            let key = |p: &Plan| p.list().iter().map(|q| (q.sat, q.aos, q.dur, q.aos_az, q.los_az)).collect::<Vec<_>>();
            assert_eq!(key(&a), key(&b), "day {day}");
            for (x, y) in a.list().iter().zip(b.list()) {
                assert!(x.tmax.abs_diff(y.tmax) <= 15 && x.max_el.abs_diff(y.max_el) <= 1, "day {day}: {x:?} {y:?}");
            }
            (work, plain_work) = (work + a.work, plain_work + b.work);
        }
    }
    std::println!("SAT-6: adaptive step {} propagations a day for 6 satellites, plain 30 s {}", work / 28, plain_work / 28);
    assert!(work * 2 < plain_work);
}

#[test]
fn prediction_speed() {
    let fx = fixture();
    let sats: Vec<Sat> = fx.iter().map(|(_, e)| Sat { el: *e, ..Default::default() }).collect();
    let obs = Observer::new(453_000_000, -757_000_000, 70);
    let t0 = fx[0].1.epoch_unix() + 3_600;
    let mut p = Plan::new();
    let start = std::time::Instant::now();
    let mut slices = 0;
    p.restart(t0, 1, Some(obs));
    while !p.step(&List(&sats), 300) {
        slices += 1;
    }
    let us = start.elapsed().as_micros() as f64;
    std::println!(
        "SAT-6: {} satellites, 24 h: {} propagations in {} slices, {} passes, {:.0} µs on the host ({:.2} µs each)",
        sats.len(),
        p.work,
        slices + 1,
        p.n,
        us,
        us / p.work as f64
    );
    // adaptive steps, plus about 40 per pass refined
    assert!(p.work < sats.len() as u32 * 1_700);
}
