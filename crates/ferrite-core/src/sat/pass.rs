//! Pass prediction (SAT-6): each satellite's passes over the next 24 h, found with a 30 s step, rise and set
//! refined to the second by bisection and the highest point by bisection on the slope, kept as
//! up to [`MAX`] entries sorted by rise time. It runs in slices ([`Plan::step`] does at most a given number of
//! propagations), so the UI task predicts between frames and never waits on it.

use super::look::{Observer, Tracker};
use super::SatSource;

/// Passes kept (SAT-6).
pub const MAX: usize = 64;
/// How far ahead (SAT-6) and the coarse step.
pub const SPAN_S: u32 = 86_400;
pub const STEP_S: u32 = 30;
/// The scan starts this long before now, so a pass already under way has its rise.
pub const BACK_S: u32 = 1_800;

/// One pass, 12 bytes: rise (UTC), how long and when its highest point comes (s after the rise), the
/// satellite (its index in the source), the highest elevation (whole degrees) and the rise and set azimuths
/// in 2° units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pass {
    pub aos: u32,
    pub dur: u16,
    pub tmax: u16,
    pub sat: u8,
    pub max_el: u8,
    pub aos_az: u8,
    pub los_az: u8,
}

impl Pass {
    pub fn los(&self) -> u32 {
        self.aos + self.dur as u32
    }
    /// Up at `now` (rise passed, set not yet).
    pub fn up(&self, now: u32) -> bool {
        now >= self.aos && now < self.los()
    }
}

/// The scan of one satellite in progress: its first look ahead (`probe`, to its first rise only) or the whole span.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scan {
    tr: Tracker,
    sat: u8,
    probe: bool,
    /// The next coarse time, and the elevation at the last one.
    t: u32,
    prev: f32,
    prev_t: u32,
    /// A pass under way: its rise and azimuth, the best coarse sample (time, elevation).
    aos: Option<u32>,
    aos_az: f32,
    best: (u32, f32),
}

/// A satellite's passes held back until it is done (SAT-6: each satellite's passes join the list together, so the
/// list changes once a satellite); a low orbit has at most about 8 over a day.
const FRESH: usize = 16;
/// No rise within the span.
const NONE: u32 = u32::MAX;

/// The predicted passes and the prediction in progress (about 1.3 KB).
pub struct Plan {
    pub passes: [Pass; MAX],
    pub n: u8,
    /// What the passes were made from: the start (UTC), the source's version, the observer.
    pub from: u32,
    pub version: u32,
    pub obs: Option<Observer>,
    /// How far it looks (from `from`), and the one satellite it looks at, if only one (an alarm's pass, SAT-12).
    pub span: u32,
    pub only: Option<u8>,
    /// SAT-6: passes whose highest point is under this (whole degrees) are left out (the options page's minimum
    /// elevation); not for an alarm's pass (`only`).
    pub min_el: u8,
    /// Per satellite, from a first look at each (a coarse scan to its first rise): the coarse sample just before
    /// that rise, where its full scan starts ([`NONE`]: none in the span, nothing to scan).
    start: [u32; 32],
    /// The order the satellites are scanned in (SAT-6: up now or rising soonest first, by `start`); the satellites
    /// in all, how many have had their first look, how many their full scan.
    order: [u8; 32],
    count: u8,
    probed: u8,
    next: u8,
    sorted: bool,
    scan: Option<Scan>,
    fresh: [Pass; FRESH],
    nf: u8,
    pub done: bool,
    /// The coarse step grows while the satellite is well below the horizon ([`step_for`]); off only to compare.
    pub adaptive: bool,
    /// Propagations so far in this prediction (for the speed measurement).
    pub work: u32,
}

impl Default for Plan {
    fn default() -> Self {
        Plan::new()
    }
}

const NO_PASS: Pass = Pass { aos: 0, dur: 0, tmax: 0, sat: 0, max_el: 0, aos_az: 0, los_az: 0 };

impl Plan {
    pub const fn new() -> Plan {
        Plan {
            passes: [NO_PASS; MAX],
            n: 0,
            from: 0,
            version: 0,
            obs: None,
            span: SPAN_S,
            only: None,
            min_el: 0,
            start: [NONE; 32],
            order: [0; 32],
            count: 0,
            probed: 0,
            next: 0,
            sorted: false,
            scan: None,
            fresh: [NO_PASS; FRESH],
            nf: 0,
            done: false,
            adaptive: true,
            work: 0,
        }
    }

    /// Start again from `now` (the passes are kept until the new ones replace them: the list doesn't empty while
    /// predicting).
    pub fn restart(&mut self, now: u32, version: u32, obs: Option<Observer>) {
        (self.from, self.version, self.obs, self.next, self.scan, self.done, self.work) = (now, version, obs, 0, None, obs.is_none(), 0);
        (self.span, self.only, self.count, self.probed, self.sorted, self.nf) = (SPAN_S, None, 0, 0, false, 0);
        if obs.is_none() {
            self.n = 0;
        }
    }

    /// SAT-6's progress: satellites done and in all (the pass list's "Predicting 7 of 19").
    pub fn progress(&self) -> (u8, u8) {
        let scanning = self.scan.is_some_and(|s| !s.probe) as u8;
        (if self.sorted { self.next.saturating_sub(scanning) } else { 0 }, self.count)
    }

    /// The passes, soonest first.
    pub fn list(&self) -> &[Pass] {
        &self.passes[..self.n as usize]
    }

    /// The next satellite's scan (its first look, or its full scan in rise order); false when there is none left.
    fn begin(&mut self, src: &dyn SatSource, obs: Observer) -> bool {
        if self.count == 0 && self.probed == 0 && !self.sorted {
            self.n = 0; // a new prediction: the old passes go
            self.count = src.count().min(32);
        }
        let probe = self.probed < self.count;
        let i = if probe {
            self.probed += 1;
            let i = self.probed - 1;
            self.start[i as usize] = NONE;
            i
        } else {
            if !self.sorted {
                self.sort();
            }
            if self.next >= self.count {
                return false;
            }
            self.next += 1;
            self.order[self.next as usize - 1]
        };
        let t = if probe { self.from.saturating_sub(BACK_S) } else { self.start[i as usize] };
        if t == NONE || self.only.is_some_and(|o| o != i) {
            return true;
        }
        let Some(sat) = src.get(i) else { return true };
        let Ok(tr) = Tracker::new(&sat.el, obs) else { return true };
        self.work += 1;
        self.scan = Some(Scan { tr, sat: i, probe, t, prev: -90.0, aos: None, aos_az: 0.0, best: (0, -90.0), prev_t: t });
        true
    }

    /// SAT-6: the scan order, soonest first rise first (up now first of all: its rise is before now); satellites
    /// with no rise in the span last (they are skipped).
    fn sort(&mut self) {
        let k = self.count as usize;
        for i in 0..k {
            let mut j = i;
            while j > 0 && self.start[self.order[j - 1] as usize] > self.start[i] {
                self.order[j] = self.order[j - 1];
                j -= 1;
            }
            self.order[j] = i as u8;
        }
        self.sorted = true;
    }

    /// Up to `budget` propagations of prediction; true when it finished (now or before).
    #[inline(never)]
    pub fn step(&mut self, src: &dyn SatSource, budget: u32) -> bool {
        let Some(obs) = self.obs else {
            self.done = true;
            return true;
        };
        let mut left = budget;
        while !self.done && left > 0 {
            let Some(mut s) = self.scan.take() else {
                if !self.begin(src, obs) {
                    self.done = true;
                }
                continue;
            };
            if self.run(&mut s, &mut left) {
                self.scan = Some(s);
            } else if !s.probe {
                self.merge();
            }
        }
        self.done
    }

    /// One satellite's coarse scan, until the budget runs out (true: more to do) or it ends.
    fn run(&mut self, s: &mut Scan, left: &mut u32) -> bool {
        let end = self.from + self.span;
        let back = self.from.saturating_sub(BACK_S);
        while *left > 0 && s.t <= end {
            // SAT-6: with the list full, nothing rising after its last pass can join
            if !s.probe && s.aos.is_none() && self.n as usize == MAX && s.prev_t >= self.passes[MAX - 1].aos {
                return false;
            }
            let el = el_at(&s.tr, s.t);
            *left = left.saturating_sub(1);
            self.work += 1;
            if s.probe {
                // the first look: the first pass not over by now (one under way counts from its rise)
                let rose = s.aos.is_none() && el >= 0.0 && s.prev < 0.0 && s.t > back;
                if rose && s.t > self.from || s.aos.is_some() && el < 0.0 && s.t > self.from {
                    self.start[s.sat as usize] = s.aos.unwrap_or(s.prev_t);
                    return false;
                }
                if rose {
                    s.aos = Some(s.prev_t); // risen before now: its full scan starts here if it is still up
                } else if el < 0.0 {
                    s.aos = None;
                }
            } else if s.aos.is_none() && el >= 0.0 && s.prev < 0.0 && s.t > back {
                let (aos, n) = edge(&s.tr, s.prev_t, s.t, true);
                *left = left.saturating_sub(n);
                self.work += n + 1;
                s.aos = Some(aos);
                s.aos_az = s.tr.look(aos).map_or(0.0, |l| l.az);
                s.best = (s.t, el);
            } else if let Some(aos) = s.aos {
                if el > s.best.1 {
                    s.best = (s.t, el);
                }
                if el < 0.0 {
                    let (los, n) = edge(&s.tr, s.prev_t, s.t, false);
                    *left = left.saturating_sub(n);
                    self.work += n + 1;
                    let los_az = s.tr.look(los).map_or(0.0, |l| l.az);
                    let (tmax, max_el, n) = top(&s.tr, s.best.0, aos, los);
                    *left = left.saturating_sub(n);
                    self.work += n;
                    let max_el = (max_el + 0.5).clamp(0.0, 90.0) as u8;
                    if los > self.from && (max_el >= self.min_el || self.only.is_some()) && (self.nf as usize) < FRESH {
                        self.fresh[self.nf as usize] = Pass {
                            aos,
                            dur: (los - aos).min(u16::MAX as u32) as u16,
                            tmax: tmax.clamp(aos, los).wrapping_sub(aos) as u16,
                            sat: s.sat,
                            max_el,
                            aos_az: (s.aos_az * 0.5 + 0.5) as u8 % 180,
                            los_az: (los_az * 0.5 + 0.5) as u8 % 180,
                        };
                        self.nf += 1;
                    }
                    s.aos = None;
                }
            }
            s.prev = el;
            s.prev_t = s.t;
            s.t += if self.adaptive { step_for(el) } else { STEP_S };
        }
        s.t <= end
    }

    /// The satellite just done: its passes into the list together.
    fn merge(&mut self) {
        for k in 0..self.nf as usize {
            self.insert(self.fresh[k]);
        }
        self.nf = 0;
    }

    /// Into the list by rise time; when full, the latest rise goes (SAT-6: the earliest 64 are kept, whatever order
    /// they were found in).
    fn insert(&mut self, p: Pass) {
        let n = self.n as usize;
        let at = self.passes[..n].iter().position(|q| q.aos > p.aos).unwrap_or(n);
        if at >= MAX {
            return;
        }
        let last = n.min(MAX - 1);
        self.passes.copy_within(at..last, at + 1);
        self.passes[at] = p;
        self.n = (n + 1).min(MAX) as u8;
    }
}

/// SAT-6's adaptive step: 30 s from 15° below the horizon up; below that 10 s more per degree, to 3 minutes at
/// 30° below. A low satellite climbs from -15° to the horizon in well over 3 minutes (the 14-day test checks that
/// no pass is lost against the plain 30 s step).
pub fn step_for(el: f32) -> u32 {
    if el >= -15.0 {
        STEP_S
    } else {
        (STEP_S + ((-15.0 - el) * 10.0) as u32).min(180)
    }
}

fn el_at(tr: &Tracker, t: u32) -> f32 {
    tr.look(t).map_or(-90.0, |l| l.el)
}

/// The second the elevation crosses 0 between `lo` (below if `rising`) and `hi`: bisection to 1 s. The time
/// returned is the first second above the horizon for a rise, the first below for a set; and the propagations.
fn edge(tr: &Tracker, mut lo: u32, mut hi: u32, rising: bool) -> (u32, u32) {
    let mut n = 0;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        n += 1;
        if (el_at(tr, mid) >= 0.0) == rising {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (hi, n)
}

/// The highest point within a coarse step of the best coarse sample `t` (and inside the pass): bisection on the
/// sign of the slope, to 1 s. Its time and elevation, and the propagations.
fn top(tr: &Tracker, t: u32, aos: u32, los: u32) -> (u32, f32, u32) {
    let (mut lo, mut hi) = (t.saturating_sub(STEP_S).max(aos), (t + STEP_S).min(los));
    let mut n = 0;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        n += 2;
        if el_at(tr, mid + 1) > el_at(tr, mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    (lo, el_at(tr, lo), n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SAT-6: up to 64 passes in rise order, the latest dropped when full.
    #[test]
    fn insert_keeps_order_and_drops_the_latest() {
        let mut p = Plan::new();
        for k in 0..70u32 {
            p.insert(Pass { aos: 1000 + (k * 37 % 70) * 10, ..Default::default() });
        }
        assert_eq!(p.n as usize, MAX);
        assert!(p.list().windows(2).all(|w| w[0].aos <= w[1].aos));
        assert_eq!(p.list()[0].aos, 1000);
        assert_eq!(p.list()[MAX - 1].aos, 1000 + 63 * 10, "the six latest dropped");
    }

    #[test]
    fn no_observer_no_passes() {
        let mut p = Plan::new();
        p.n = 3;
        p.restart(1_793_500_000, 1, None);
        assert!(p.step(&super::super::NoSats, 100));
        assert_eq!(p.n, 0);
    }
}
