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

/// The scan of one satellite in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scan {
    tr: Tracker,
    /// The next coarse time, and the elevation at the last one.
    t: u32,
    prev: f32,
    prev_t: u32,
    /// A pass under way: its rise and azimuth, the best coarse sample (time, elevation).
    aos: Option<u32>,
    aos_az: f32,
    best: (u32, f32),
}

/// The predicted passes and the prediction in progress (about 950 bytes).
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
    /// The order the satellites are scanned in (SAT-6: up now or rising soonest first), from one look at each;
    /// `next` counts through it, and the one being scanned.
    order: [u8; 32],
    count: u8,
    next: u8,
    scan: Option<Scan>,
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

impl Plan {
    pub const fn new() -> Plan {
        Plan { passes: [Pass { aos: 0, dur: 0, tmax: 0, sat: 0, max_el: 0, aos_az: 0, los_az: 0 }; MAX], n: 0, from: 0, version: 0, obs: None, span: SPAN_S, only: None, order: [0; 32], count: 0, next: 0, scan: None, done: false, adaptive: true, work: 0 }
    }

    /// Start again from `now` (the passes are kept until the new ones replace them: the list doesn't empty while
    /// predicting).
    pub fn restart(&mut self, now: u32, version: u32, obs: Option<Observer>) {
        (self.from, self.version, self.obs, self.next, self.scan, self.done, self.work) = (now, version, obs, 0, None, obs.is_none(), 0);
        (self.span, self.only, self.count) = (SPAN_S, None, 0);
        if obs.is_none() {
            self.n = 0;
        }
    }

    /// SAT-6's progress: satellites done and in all (the pass list's "Predicting 7 of 19").
    pub fn progress(&self) -> (u8, u8) {
        (self.next.saturating_sub(self.scan.is_some() as u8), self.count)
    }

    /// The scan order: every satellite's elevation now, highest first (up now, then rising soonest, roughly).
    /// One propagation each; returns how many.
    fn sort(&mut self, src: &dyn SatSource, obs: Observer) -> u32 {
        let n = src.count().min(32);
        let mut el = [0i16; 32];
        let mut k = 0;
        for i in 0..n {
            if self.only.is_some_and(|o| o != i) {
                continue;
            }
            let e = src.get(i).and_then(|s| Tracker::new(&s.el, obs).ok()).map_or(-90.0, |t| el_at(&t, self.from));
            // insertion by elevation, highest first
            let mut j = k;
            while j > 0 && el[j - 1] < e as i16 {
                (el[j], self.order[j]) = (el[j - 1], self.order[j - 1]);
                j -= 1;
            }
            (el[j], self.order[j]) = (e as i16, i);
            k += 1;
        }
        self.count = k as u8;
        self.work += n as u32;
        n as u32
    }

    /// The passes, soonest first.
    pub fn list(&self) -> &[Pass] {
        &self.passes[..self.n as usize]
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
                if self.next == 0 && self.count == 0 {
                    self.n = 0; // a new prediction: the old passes go
                    let n = self.sort(src, obs);
                    left = left.saturating_sub(n);
                    if self.count == 0 {
                        self.done = true;
                        break;
                    }
                }
                if self.next >= self.count {
                    self.done = true;
                    break;
                }
                let i = self.order[self.next as usize];
                self.next += 1;
                if self.only.is_some_and(|o| o != i) {
                    continue;
                }
                let Some(sat) = src.get(i) else { continue };
                let Ok(tr) = Tracker::new(&sat.el, obs) else { continue };
                let t = self.from.saturating_sub(BACK_S);
                self.scan = Some(Scan { tr, t, prev: -90.0, aos: None, aos_az: 0.0, best: (0, -90.0), prev_t: t });
                continue;
            };
            let end = self.from + self.span;
            while left > 0 && s.t <= end {
                let el = el_at(&s.tr, s.t);
                left = left.saturating_sub(1);
                self.work += 1;
                if s.aos.is_none() && el >= 0.0 && s.prev < 0.0 && s.t > self.from.saturating_sub(BACK_S) {
                    let (aos, n) = edge(&s.tr, s.prev_t, s.t, true);
                    left = left.saturating_sub(n);
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
                        left = left.saturating_sub(n);
                        self.work += n + 1;
                        let los_az = s.tr.look(los).map_or(0.0, |l| l.az);
                        let (tmax, max_el, n) = top(&s.tr, s.best.0, aos, los);
                        left = left.saturating_sub(n);
                        self.work += n;
                        let i = self.order[self.next as usize - 1];
                        if los > self.from {
                            self.insert(Pass {
                                aos,
                                dur: (los - aos).min(u16::MAX as u32) as u16,
                                tmax: tmax.clamp(aos, los).wrapping_sub(aos) as u16,
                                sat: i,
                                max_el: (max_el + 0.5).clamp(0.0, 90.0) as u8,
                                aos_az: (s.aos_az * 0.5 + 0.5) as u8 % 180,
                                los_az: (los_az * 0.5 + 0.5) as u8 % 180,
                            });
                        }
                        s.aos = None;
                    }
                }
                s.prev = el;
                s.prev_t = s.t;
                s.t += if self.adaptive { step_for(el) } else { STEP_S };
            }
            if s.t <= end {
                self.scan = Some(s);
            }
        }
        self.done
    }

    /// Into the list by rise time; when full, the latest rise goes.
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
