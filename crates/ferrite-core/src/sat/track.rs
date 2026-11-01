//! Passes and tracks computed elsewhere (Console, with this crate in WebAssembly) and only read on the radio: each
//! pass with a sample every [`STEP_S`] seconds of azimuth, elevation and range rate in 4 bytes, interpolated by a
//! cubic through the four nearest samples (Catmull-Rom). The firmware built without `sat-predict` links this in
//! place of SGP4 and the predictor ([`super::data::Live`]).

use super::data::{Prefs, SatData, State, SATS};
use super::look::{Look, Observer};
use super::pass::Pass;
use super::{Sat, SatSource};

/// Seconds between samples.
pub const STEP_S: u32 = 15;

/// One sample: azimuth 0.1° (12 bits), elevation 0.2° from -2° (9 bits), range rate 8 m/s offset (11 bits).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sample(pub u32);

impl Sample {
    pub fn new(l: &Look) -> Sample {
        let az = ((l.az * 10.0 + 0.5) as u32).min(3599);
        let el = (((l.el + 2.0) * 5.0 + 0.5).max(0.0) as u32).min(511);
        let rate = (((l.rate * 125.0) + 1024.5).max(0.0) as u32).min(2047);
        Sample(az | el << 12 | rate << 21)
    }
    fn az(self) -> f32 {
        (self.0 & 0xFFF) as f32 * 0.1
    }
    fn el(self) -> f32 {
        (self.0 >> 12 & 0x1FF) as f32 * 0.2 - 2.0
    }
    fn rate(self) -> f32 {
        ((self.0 >> 21) as f32 - 1024.0) * 0.008
    }
}

/// Where the passes and samples are kept (RAM in the simulator and tests; the SPI flash in phase 2).
pub trait TrackStore {
    fn passes(&self) -> u8;
    fn pass(&self, i: u8) -> Option<Pass>;
    /// Sample `k` of pass `i` (k = 0 at the rise; one every [`STEP_S`] past the set).
    fn sample(&self, i: u8, k: u16) -> Option<Sample>;
    fn version(&self) -> u32;
}

/// No tracks (until Console writes some).
pub struct NoTracks;

impl TrackStore for NoTracks {
    fn passes(&self) -> u8 {
        0
    }
    fn pass(&self, _i: u8) -> Option<Pass> {
        None
    }
    fn sample(&self, _i: u8, _k: u16) -> Option<Sample> {
        None
    }
    fn version(&self) -> u32 {
        0
    }
}

/// How many samples a pass takes: from its rise to one past its set.
pub fn samples(p: &Pass) -> u16 {
    (p.dur as u32 / STEP_S + 2) as u16
}

/// Catmull-Rom between `b` and `c` at `x` in [0, 1], with neighbours `a` and `d`.
fn cubic(a: f32, b: f32, c: f32, d: f32, x: f32) -> f32 {
    b + 0.5 * x * (c - a + x * (2.0 * a - 5.0 * b + 4.0 * c - d + x * (3.0 * (b - c) + d - a)))
}

/// Where a satellite is at `t` during pass `i` of a store, interpolated.
#[inline(never)]
pub fn look_at(st: &dyn TrackStore, i: u8, t: u32) -> Option<Look> {
    let p = st.pass(i)?;
    if t < p.aos.saturating_sub(STEP_S) {
        return None;
    }
    let off = t.saturating_sub(p.aos);
    let k = (off / STEP_S) as u16;
    let n = samples(&p);
    if k + 1 >= n {
        return None;
    }
    let x = (off % STEP_S) as f32 / STEP_S as f32;
    let get = |j: i32| st.sample(i, j.clamp(0, n as i32 - 1) as u16).unwrap_or_default();
    let (a, b, c, d) = (get(k as i32 - 1), get(k as i32), get(k as i32 + 1), get(k as i32 + 2));
    // azimuth unwrapped around b's
    let un = |v: f32| {
        let d = v - b.az();
        if d > 180.0 {
            v - 360.0
        } else if d < -180.0 {
            v + 360.0
        } else {
            v
        }
    };
    let mut az = cubic(un(a.az()), b.az(), un(c.az()), un(d.az()), x);
    if az < 0.0 {
        az += 360.0;
    } else if az >= 360.0 {
        az -= 360.0;
    }
    Some(Look { az, el: cubic(a.el(), b.el(), c.el(), d.el(), x), range: 0.0, rate: cubic(a.rate(), b.rate(), c.rate(), d.rate(), x) })
}

/// Satellite mode's data from stored tracks.
pub struct Tracks<S: SatSource, T: TrackStore> {
    pub src: S,
    pub store: T,
    prefs: [Option<Prefs>; SATS],
    now: Option<u32>,
    alarm: Option<Pass>,
}

impl<S: SatSource, T: TrackStore> Tracks<S, T> {
    pub fn new(src: S, store: T) -> Self {
        Tracks { src, store, prefs: [None; SATS], now: None, alarm: None }
    }
}

impl<S: SatSource, T: TrackStore> SatData for Tracks<S, T> {
    fn version(&self) -> u32 {
        self.store.version()
    }
    fn state(&self) -> State {
        if self.src.count() == 0 {
            State::NoSats
        } else if self.now.is_none() {
            State::NoClock
        } else {
            State::Ready
        }
    }
    fn sats(&self) -> u8 {
        self.src.count()
    }
    fn sat(&self, i: u8) -> Option<Sat> {
        self.src.get(i)
    }
    fn passes(&self) -> u8 {
        self.store.passes()
    }
    fn pass(&self, i: u8) -> Option<Pass> {
        self.store.pass(i)
    }
    fn look(&self, sat: u8, t: u32) -> Option<Look> {
        // the pass of that satellite around t (a sample before the rise to a sample after the set)
        let i = (0..self.store.passes()).find(|&i| self.store.pass(i).is_some_and(|p| p.sat == sat && t + STEP_S >= p.aos && t < p.los() + STEP_S))?;
        look_at(&self.store, i, t)
    }
    fn tick(&mut self, now: Option<u32>, _obs: Option<Observer>, _open: bool, _budget: u32) {
        self.now = now;
        if let (Some(a), Some(t)) = (self.alarm, now) {
            if t >= a.los() {
                self.alarm = None;
            }
        }
    }
    fn alarm(&self) -> Option<Pass> {
        self.alarm
    }
    fn set_alarm(&mut self, p: Option<Pass>) {
        self.alarm = p;
    }
    fn declination(&self) -> Option<i16> {
        self.src.declination()
    }
    fn prefs(&self, sat: u8) -> Prefs {
        match self.prefs.get(sat as usize).copied().flatten() {
            Some(p) => p,
            None => self.src.get(sat).map(|s| Prefs::of(&s)).unwrap_or_default(),
        }
    }
    fn set_prefs(&mut self, sat: u8, p: Prefs) {
        if let Some(slot) = self.prefs.get_mut(sat as usize) {
            *slot = Some(p);
        }
    }
}

/// Tracks in RAM: up to `P` passes and `N` samples (the simulator, tests; what Console builds before writing).
pub struct RamTracks<const P: usize, const N: usize> {
    passes: [(Pass, u16); P],
    n: u8,
    samples: [Sample; N],
    used: usize,
    version: u32,
}

impl<const P: usize, const N: usize> RamTracks<P, N> {
    pub fn new() -> Self {
        RamTracks { passes: [(Pass::default(), 0); P], n: 0, samples: [Sample(0); N], used: 0, version: 1 }
    }

    /// Every pass of `d` that fits, with its samples (as Console would compute them).
    pub fn fill(&mut self, d: &dyn SatData) {
        (self.n, self.used) = (0, 0);
        for i in 0..d.passes() {
            let Some(p) = d.pass(i) else { continue };
            let k = samples(&p) as usize;
            if self.n as usize >= P || self.used + k > N {
                break;
            }
            for j in 0..k {
                let l = d.look(p.sat, p.aos + j as u32 * STEP_S).unwrap_or_default();
                self.samples[self.used + j] = Sample::new(&l);
            }
            self.passes[self.n as usize] = (p, self.used as u16);
            self.n += 1;
            self.used += k;
        }
        self.version = self.version.wrapping_add(1);
    }
}

impl<const P: usize, const N: usize> Default for RamTracks<P, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const P: usize, const N: usize> TrackStore for RamTracks<P, N> {
    fn passes(&self) -> u8 {
        self.n
    }
    fn pass(&self, i: u8) -> Option<Pass> {
        self.passes[..self.n as usize].get(i as usize).map(|x| x.0)
    }
    fn sample(&self, i: u8, k: u16) -> Option<Sample> {
        let (p, at) = *self.passes[..self.n as usize].get(i as usize)?;
        (k < samples(&p)).then(|| self.samples[at as usize + k as usize])
    }
    fn version(&self) -> u32 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::super::data::Live;
    use super::super::{tle, List};
    use super::*;

    const ISS: (&[u8], &[u8]) = (
        b"1 25544U 98067A   26304.75703384  .00006360  00000+0  12291-3 0  9994",
        b"2 25544  51.6332 344.2755 0007028 332.5119  27.5497 15.49131714592920",
    );
    const SO50: (&[u8], &[u8]) = (
        b"1 27607U 02058C   26304.82911449  .00000904  00000+0  12403-3 0  9995",
        b"2 27607  64.5527  66.1378 0068979 237.1936 122.2515 14.83296838284727",
    );

    /// Tracks every 15 s, interpolated, against the propagator every second of every pass: within SAT-4's
    /// limits (pointing 0.5°, Doppler 100 Hz at 437.8 MHz).
    #[test]
    fn interpolated_tracks_match_the_propagator() {
        let sats = [ISS, SO50].map(|(a, b)| Sat { el: tle::parse(a, b).unwrap(), ..Default::default() });
        let mut live = Live::new(List(&sats));
        let here = Observer::new(453_000_000, -757_000_000, 70);
        let now = 1_793_480_000;
        live.tick(Some(now), Some(here), true, u32::MAX);
        let mut t: RamTracks<16, 2000> = RamTracks::new();
        t.fill(&live);
        assert!(t.passes() >= 6);
        let tr = Tracks::new(List(&sats), t);
        let (mut worst_deg, mut worst_hz, mut n) = (0f32, 0f32, 0);
        for i in 0..tr.passes() {
            let p = tr.pass(i).unwrap();
            for s in p.aos..p.los() {
                let a = live.look(p.sat, s).unwrap();
                let b = tr.look(p.sat, s).unwrap();
                let mut daz = (a.az - b.az).abs();
                if daz > 180.0 {
                    daz = 360.0 - daz;
                }
                let c = super::super::fmath::cos(super::super::fmath::rad(a.el));
                worst_deg = worst_deg.max(sqrt2(daz * c, a.el - b.el));
                worst_hz = worst_hz.max((a.rate - b.rate).abs() / 299_792.47 * 437.8e6);
                n += 1;
            }
        }
        std::println!("tracks: {n} s of passes: worst pointing {worst_deg:.3}°, Doppler {worst_hz:.1} Hz");
        assert!(worst_deg < 0.5 && worst_hz < 100.0);
    }

    fn sqrt2(a: f32, b: f32) -> f32 {
        super::super::fmath::sqrt(a * a + b * b)
    }

    #[test]
    fn samples_round_trip() {
        let l = Look { az: 212.34, el: 61.2, range: 0.0, rate: -6.85 };
        let s = Sample::new(&l);
        assert!((s.az() - 212.3).abs() < 0.06 && (s.el() - 61.2).abs() < 0.11 && (s.rate() + 6.85).abs() < 0.005);
        assert_eq!(Sample::new(&Look { az: 400.0, el: -9.0, range: 0.0, rate: 99.0 }).0 >> 21, 2047);
    }
}
