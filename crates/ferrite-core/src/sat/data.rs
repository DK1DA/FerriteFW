//! What satellite mode's screens and the radio side read (SAT-6, SAT-7): the satellites, their passes soonest
//! first, and where a satellite is at a time. Behind a trait so the firmware can take the passes and tracks
//! from Console (computed there with this crate compiled to WebAssembly) without linking the propagator, while
//! the simulator and the bench builds predict on the device with [`Live`]. The per-satellite choices (trim,
//! FD/HD, transponder; SAT-8, SAT-11) live with the source too, as they belong to the satellite's record.

use super::look::{Look, Observer, Tracker};
use super::pass::{Pass, Plan};
use super::{Sat, SatSource};
use core::cell::Cell;

/// Where the source is (the pass list's line when it has no passes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Passes are current.
    Ready,
    /// Predicting (the list shows what it has so far).
    Busy,
    NoSats,
    /// SAT-5: no GPS fix and no fixed position.
    NoPosition,
    /// No clock yet (GPS time, Console or set by hand).
    NoClock,
}

/// The per-satellite choices: the downlink trim in 10 Hz (SAT-8), half duplex (SAT-11), Doppler on the 70 cm
/// side only (SAT-8), the transponder picked (1-5 on the pass page), the uplink's power (TX-9 levels: 0 Low, the
/// default, 1 Mid, 2 High; the options page, SAT-22).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Prefs {
    pub trim: i16,
    pub hd: bool,
    pub uhf_only: bool,
    pub tp: u8,
    pub power: u8,
}

impl Prefs {
    /// The starting choices from the satellite's flags (Console's, SAT-11).
    pub fn of(s: &Sat) -> Prefs {
        Prefs { trim: 0, hd: s.flags & Sat::HD != 0, uhf_only: s.flags & Sat::UHF_ONLY != 0, tp: 0, power: 0 }
    }
}

/// The most satellites (SAT-2).
pub const SATS: usize = 32;

/// Satellite mode's data, as the App sees it.
pub trait SatData {
    /// Changes whenever the passes or the satellites change (the screens redraw).
    fn version(&self) -> u32;
    fn state(&self) -> State;
    fn sats(&self) -> u8;
    fn sat(&self, i: u8) -> Option<Sat>;
    fn passes(&self) -> u8;
    fn pass(&self, i: u8) -> Option<Pass>;
    /// Where satellite `sat` is at UTC `t`, from the observer.
    fn look(&self, sat: u8, t: u32) -> Option<Look>;
    /// Time, the observer (SAT-5), whether the satellite screen is open, and a budget of work: predicting runs
    /// here, in slices, while the screen is open (SAT-6); closed, only an alarm's pass is kept up to date.
    fn tick(&mut self, now: Option<u32>, obs: Option<Observer>, open: bool, budget: u32);
    /// More of the prediction under way, after the tick (the radio predicts in time slices between keys and
    /// redraws, SAT-6): up to `budget` propagations.
    fn work(&mut self, _budget: u32) {}
    /// SAT-6: the last prediction's propagations (the speed line in the cable log).
    fn propagations(&self) -> u32 {
        0
    }
    /// SAT-12: the pass with an alarm (kept on time when the position or the clock changes), and setting it.
    fn alarm(&self) -> Option<Pass>;
    fn set_alarm(&mut self, p: Option<Pass>);
    fn prefs(&self, sat: u8) -> Prefs;
    fn set_prefs(&mut self, sat: u8, p: Prefs);
    /// SAT-6: while predicting, satellites done and in all ("Predicting 7 of 19").
    fn progress(&self) -> Option<(u8, u8)> {
        None
    }
    /// SAT-18: the magnetic declination at the radio, 0.1° east positive (Console's, with the satellites).
    fn declination(&self) -> Option<i16> {
        None
    }
    /// SAT-6, SAT-22: the lowest highest point a pass needs to be listed (whole degrees); a change predicts again.
    fn set_min_el(&mut self, _el: u8) {}
}

/// No satellites (a build without satellite data).
pub struct NoData;

impl SatData for NoData {
    fn version(&self) -> u32 {
        0
    }
    fn state(&self) -> State {
        State::NoSats
    }
    fn sats(&self) -> u8 {
        0
    }
    fn sat(&self, _i: u8) -> Option<Sat> {
        None
    }
    fn passes(&self) -> u8 {
        0
    }
    fn pass(&self, _i: u8) -> Option<Pass> {
        None
    }
    fn look(&self, _sat: u8, _t: u32) -> Option<Look> {
        None
    }
    fn tick(&mut self, _now: Option<u32>, _obs: Option<Observer>, _open: bool, _budget: u32) {}
    fn alarm(&self) -> Option<Pass> {
        None
    }
    fn set_alarm(&mut self, _p: Option<Pass>) {}
    fn prefs(&self, _sat: u8) -> Prefs {
        Prefs::default()
    }
    fn set_prefs(&mut self, _sat: u8, _p: Prefs) {}
}

/// Predicting on the device (SAT-6): SGP4 and [`Plan`] over a satellite list. The next 24 h when the screen
/// opens, again on new elements, a move of more than 10 km or a clock change while it is open; the passes are
/// dropped when it closes. Closed, only an alarm's pass is predicted again (that satellite, an hour around it)
/// when the position or clock changes, so the alarm stays on time.
pub struct Live<S: SatSource> {
    pub src: S,
    plan: Plan,
    /// The tracker last used by [`SatData::look`] (one satellite's constants, about 150 bytes).
    track: Cell<Option<(u8, Tracker)>>,
    obs: Option<Observer>,
    /// The clock's expected value at the last tick (to see a jump).
    expect: Option<u32>,
    version: u32,
    no_clock: bool,
    open: bool,
    alarm: Option<Pass>,
    /// The plan is finding the alarm's pass again, not the list.
    refining: bool,
    /// SAT-6: the minimum elevation the list was asked for (the plan's while it predicts the list).
    min_el: u8,
}

/// SAT-6: a move that counts as a new position (10 km, in radians of latitude or longitude), a clock change.
const MOVED_RAD: f32 = 1.57e-3;
const JUMP_S: u32 = 60;

impl<S: SatSource> Live<S> {
    pub fn new(src: S) -> Self {
        Live { src, plan: Plan::new(), track: Cell::new(None), obs: None, expect: None, version: 1, no_clock: true, open: false, alarm: None, refining: false, min_el: 0 }
    }

    /// The prediction's own counters (propagations, satellites done): for the speed check.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    fn tracker(&self, sat: u8) -> Option<Tracker> {
        if let Some((_, t)) = self.track.get().filter(|(s, _)| *s == sat) {
            return Some(t);
        }
        let t = Tracker::new(&self.src.get(sat)?.el, self.obs?).ok()?;
        self.track.set(Some((sat, t)));
        Some(t)
    }

    /// The alarm's pass from the passes found: the same satellite's nearest rise within half an hour.
    fn follow_alarm(&mut self) {
        if let Some(a) = self.alarm {
            if let Some(p) = self.plan.list().iter().filter(|p| p.sat == a.sat && p.aos.abs_diff(a.aos) < 1_800).min_by_key(|p| p.aos.abs_diff(a.aos)) {
                self.alarm = Some(*p);
            }
        }
    }
}

impl<S: SatSource> SatData for Live<S> {
    fn version(&self) -> u32 {
        self.version
    }
    fn state(&self) -> State {
        if self.src.count() == 0 {
            State::NoSats
        } else if self.no_clock {
            State::NoClock
        } else if self.obs.is_none() {
            State::NoPosition
        } else if self.open && !self.plan.done || self.refining {
            // closed, nothing runs but an alarm's refinement (the governor's reason, PWR-26.1, goes with it)
            State::Busy
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
        if self.refining { 0 } else { self.plan.n }
    }
    fn pass(&self, i: u8) -> Option<Pass> {
        (!self.refining).then(|| self.plan.list().get(i as usize).copied()).flatten()
    }
    fn look(&self, sat: u8, t: u32) -> Option<Look> {
        self.tracker(sat)?.look(t).ok()
    }
    #[inline(never)]
    fn tick(&mut self, now: Option<u32>, obs: Option<Observer>, open: bool, budget: u32) {
        self.no_clock = now.is_none();
        let Some(now) = now else { return };
        let moved = match (self.obs, obs) {
            (Some(a), Some(b)) => (a.lat - b.lat).abs() > MOVED_RAD || (a.lon - b.lon).abs() > MOVED_RAD,
            (a, b) => a.is_some() != b.is_some(),
        };
        let jumped = self.expect.is_some_and(|e| now.abs_diff(e) > JUMP_S);
        let changed = self.src.version() != self.plan.version;
        let redo = self.plan.min_el != self.min_el; // SAT-22's minimum elevation changed
        self.expect = Some(now);
        if moved {
            self.obs = obs;
            self.track.set(None);
        }
        if changed {
            self.track.set(None);
        }
        if self.alarm.is_some_and(|a| now >= a.los()) {
            self.alarm = None; // SAT-20: gone when the pass ends
        }
        let opened = open && !self.open;
        if !open && self.open {
            self.plan.n = 0; // closed: the list goes (SAT-6)
            self.plan.done = false;
            self.version = self.version.wrapping_add(1);
        }
        self.open = open;
        if opened || open && (moved || jumped || changed || redo) {
            self.refining = false;
            self.plan.restart(now, self.src.version(), self.obs);
            self.plan.min_el = self.min_el;
            self.version = self.version.wrapping_add(1);
        } else if !open && (moved || jumped || changed) {
            if let Some(a) = self.alarm {
                self.plan.restart(a.aos.saturating_sub(1_800).max(now), self.src.version(), self.obs);
                (self.plan.span, self.plan.only, self.refining) = (3_600, Some(a.sat), true);
            }
        }
        self.work(budget);
    }
    #[inline(never)]
    fn work(&mut self, budget: u32) {
        if !self.open && !self.refining || self.plan.done {
            return;
        }
        let was = (self.plan.n, self.plan.done);
        self.plan.step(&self.src, budget);
        if self.plan.done {
            self.follow_alarm();
            if self.refining {
                (self.refining, self.plan.n) = (false, 0);
            }
        }
        if (self.plan.n, self.plan.done) != was {
            self.version = self.version.wrapping_add(1);
        }
    }
    fn alarm(&self) -> Option<Pass> {
        self.alarm
    }
    fn set_alarm(&mut self, p: Option<Pass>) {
        self.alarm = p;
    }
    fn progress(&self) -> Option<(u8, u8)> {
        (self.state() == State::Busy && !self.refining).then(|| self.plan.progress())
    }
    fn declination(&self) -> Option<i16> {
        self.src.declination()
    }
    fn set_min_el(&mut self, el: u8) {
        self.min_el = el;
    }
    fn propagations(&self) -> u32 {
        self.plan.work
    }
    fn prefs(&self, sat: u8) -> Prefs {
        prefs_of(&self.src, sat)
    }
    fn set_prefs(&mut self, sat: u8, p: Prefs) {
        self.src.set_prefs(sat, p);
    }
}

/// A satellite's choices: as kept (the store's, SAT-2), else the starting ones from its flags.
pub fn prefs_of<S: SatSource>(src: &S, sat: u8) -> Prefs {
    src.prefs(sat).or_else(|| src.get(sat).map(|s| Prefs::of(&s))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::{tle, List, Sat};
    use super::*;

    /// A source that keeps one satellite's choices, as the store does.
    struct Kept<'a>(List<'a>, Cell<Option<Prefs>>);

    impl SatSource for Kept<'_> {
        fn count(&self) -> u8 {
            self.0.count()
        }
        fn get(&self, i: u8) -> Option<Sat> {
            self.0.get(i)
        }
        fn prefs(&self, _i: u8) -> Option<Prefs> {
            self.1.get()
        }
        fn set_prefs(&self, _i: u8, p: Prefs) {
            self.1.set(Some(p));
        }
    }

    const L1: &[u8] = b"1 25544U 98067A   26304.75703384  .00006360  00000+0  12291-3 0  9994";
    const L2: &[u8] = b"2 25544  51.6332 344.2755 0007028 332.5119  27.5497 15.49131714592920";

    /// SAT-5, SAT-6, SAT-12: no clock or position, no passes; predicted in slices when the screen opens, again
    /// after a move, dropped on close; closed, only the alarm's pass is found again (and gone after it).
    #[test]
    fn predicts_while_open_and_keeps_only_the_alarm() {
        let sats = [Sat { id: 25_544, el: tle::parse(L1, L2).unwrap(), ..Default::default() }];
        let mut d = Live::new(Kept(List(&sats), Cell::new(None)));
        let here = Observer::new(453_000_000, -757_000_000, 70);
        let now = 1_793_480_000;
        d.tick(None, Some(here), true, 100);
        assert_eq!(d.state(), State::NoClock);
        let mut d = Live::new(Kept(List(&sats), Cell::new(None)));
        d.tick(Some(now), None, true, 100);
        assert_eq!(d.state(), State::NoPosition);
        assert_eq!(d.passes(), 0);
        let mut d = Live::new(Kept(List(&sats), Cell::new(None)));
        d.tick(Some(now), Some(here), false, 100);
        assert_eq!(d.passes(), 0, "nothing predicted while closed");
        d.tick(Some(now), Some(here), true, 100);
        assert_eq!(d.state(), State::Busy);
        let mut k = 0;
        while d.state() == State::Busy {
            d.tick(Some(now + 1), Some(here), true, 100);
            k += 1;
        }
        assert!(k > 3, "in slices");
        assert!(d.passes() >= 3, "the ISS passes a few times a day");
        let p = d.pass(1).unwrap();
        assert!(p.los() > now);
        let l = d.look(0, p.aos + p.tmax as u32).unwrap();
        assert!((l.el - p.max_el as f32).abs() < 1.0);
        // moved 50 km while open: predicted again
        let v = d.version();
        let there = Observer::new(458_000_000, -757_000_000, 70);
        d.tick(Some(now + 2), Some(there), true, 10);
        assert_eq!(d.state(), State::Busy);
        assert_ne!(d.version(), v);
        while d.state() == State::Busy {
            d.tick(Some(now + 3), Some(there), true, 500);
        }
        // an alarm, then closed: the list goes, the alarm's pass is found again after a move back
        let p = d.pass(1).unwrap();
        d.set_alarm(Some(p));
        d.tick(Some(now + 4), Some(there), false, 500);
        assert_eq!(d.passes(), 0);
        d.tick(Some(now + 5), Some(here), false, 10_000);
        let a = d.alarm().unwrap();
        assert_eq!(a.sat, p.sat);
        assert!(a.aos.abs_diff(p.aos) < 120 && a != p, "refined for the new place");
        assert_eq!(d.passes(), 0);
        d.tick(Some(a.los() + 1), Some(here), false, 10);
        assert!(d.alarm().is_none(), "gone when the pass ends");
        // prefs start from the flags and are kept
        assert!(!d.prefs(0).hd);
        d.set_prefs(0, Prefs { trim: 20, hd: true, ..Default::default() });
        assert_eq!(d.prefs(0).trim, 20);
    }
}
