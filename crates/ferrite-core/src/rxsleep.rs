//! Receiver sleep: the BK4829s cycle between deep sleep (about 30 uA) and receive (49 mA), as stock's Battery Save
//! does (`analysis/battery_save.md`). It is the biggest power saving the radio has: at level 3 a chip averages
//! about 9 mA instead of 49.
//!
//! Timings are stock's. Any activity (a key, the squelch opening) holds the receiver fully on for 10 s, then a
//! gentler 300/300 ms cycle runs for 30 minutes before the level's own cycle. Anything that must not miss a
//! moment (a decoder, a scan, transmit) keeps it awake: [`RxSleep::tick`]'s `busy`.
//!
//! The cost is reaction time: a signal that starts while the receiver sleeps is heard up to one off-time late
//! (800 ms at level 3), and a burst shorter than that can be missed altogether.

/// Battery Save setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Off,
    L1,
    L2,
    L3,
}

/// Receiver on and off times, ms, per level (stock's tables at 0x0802F71D and 0x0802F719, in 10 ms units).
pub const fn timing(level: Level) -> Option<(u32, u32)> {
    match level {
        Level::Off => None,
        Level::L1 => Some((300, 500)),
        Level::L2 => Some((180, 700)),
        Level::L3 => Some((180, 800)),
    }
}

/// After activity: fully on, then the recent-activity cycle, then the level's (stock's 10 s and 30 min).
pub const HOLD_MS: u64 = 10_000;
pub const RECENT_MS: u64 = 30 * 60_000;
pub const RECENT: (u32, u32) = (300, 300);

/// What the Radio task must do to the chips now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Sleep,
    Wake,
}

#[derive(Clone, Copy, Debug)]
pub struct RxSleep {
    level: Level,
    asleep: bool,
    /// End of the current phase, or of the hold; ms.
    until: u64,
    recent_until: u64,
}

impl RxSleep {
    /// Starts awake, as after activity.
    pub fn new(level: Level, now: u64) -> Self {
        RxSleep { level, asleep: false, until: now + HOLD_MS, recent_until: now + HOLD_MS + RECENT_MS }
    }

    pub fn level(&self) -> Level {
        self.level
    }

    pub fn asleep(&self) -> bool {
        self.asleep
    }

    /// A new level counts as activity (the user just changed it).
    pub fn set_level(&mut self, level: Level, now: u64) -> Action {
        self.level = level;
        self.activity(now)
    }

    /// A key, the squelch opening, a retune: awake now and for the hold time.
    pub fn activity(&mut self, now: u64) -> Action {
        let was = self.asleep;
        self.asleep = false;
        self.until = now + HOLD_MS;
        self.recent_until = self.until + RECENT_MS;
        if was { Action::Wake } else { Action::None }
    }

    /// Call at least by [`RxSleep::deadline`]. `busy`: something needs the receiver on all the time.
    pub fn tick(&mut self, now: u64, busy: bool) -> Action {
        let Some((on, off)) = timing(self.level) else {
            return self.wake_if_asleep();
        };
        if busy {
            // Stay awake while busy, and start a fresh on-phase when it ends.
            self.until = now + on as u64;
            return self.wake_if_asleep();
        }
        if now < self.until {
            return Action::None;
        }
        let (on, off) = if now < self.recent_until { RECENT } else { (on, off) };
        self.asleep = !self.asleep;
        self.until = now + if self.asleep { off } else { on } as u64;
        if self.asleep { Action::Sleep } else { Action::Wake }
    }

    fn wake_if_asleep(&mut self) -> Action {
        if self.asleep {
            self.asleep = false;
            Action::Wake
        } else {
            Action::None
        }
    }

    /// When the next phase change is due (ms); the Radio task sleeps until then or a command. None at level Off:
    /// nothing changes, and [`RxSleep::tick`] never moves a deadline there, so a time reported then would be past
    /// 10 s after the last activity and wake the Radio task every 1 ms (decision 163). A caller that stops ticking
    /// (HF mode) must not wait on it either; the crossband repeater ticks it busy (XB-13), which keeps it ahead.
    pub fn deadline(&self) -> Option<u64> {
        timing(self.level).map(|_| self.until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `ms` of time in 10 ms ticks; returns the ms spent awake.
    fn run(s: &mut RxSleep, from: u64, ms: u64, busy: bool) -> u64 {
        let mut awake = 0;
        for t in (from..from + ms).step_by(10) {
            s.tick(t, busy);
            if !s.asleep() {
                awake += 10;
            }
        }
        awake
    }

    /// PWR-1, PWR-3: stock's timings: fully on for 10 s after activity, then the recent window's cycle, then the
    /// level's own cycle.
    #[test]
    fn stock_cycle_after_the_recent_window() {
        let mut s = RxSleep::new(Level::L3, 0);
        // 10 s hold: always on.
        assert_eq!(run(&mut s, 0, HOLD_MS, false), HOLD_MS);
        // Recent window: half on.
        let a = run(&mut s, HOLD_MS, 60_000, false);
        assert!((29_000..=31_000).contains(&a), "{a}");
        // Then level 3: 180 on, 800 off, 18 %.
        let t0 = HOLD_MS + RECENT_MS + 1_000;
        run(&mut s, HOLD_MS + 60_000, t0 - HOLD_MS - 60_000, false);
        let a = run(&mut s, t0, 98_000, false);
        assert!((17_000..=19_000).contains(&a), "{a}");
    }

    /// PWR-3, PWR-4: any key or squelch change wakes the receiver and holds it on; it never sleeps while busy.
    #[test]
    fn activity_and_busy_wake_it() {
        let mut s = RxSleep::new(Level::L1, 0);
        let t = HOLD_MS + RECENT_MS + 10;
        run(&mut s, HOLD_MS, t - HOLD_MS, false);
        while !s.asleep() {
            s.tick(s.deadline().unwrap(), false);
        }
        assert_eq!(s.activity(t + 5_000), Action::Wake);
        assert_eq!(s.tick(t + 5_010, false), Action::None, "held after activity");
        // Busy: never sleeps.
        let mut s = RxSleep::new(Level::L3, 0);
        assert_eq!(run(&mut s, 0, 5 * 60_000, true), 5 * 60_000);
        // Off: never sleeps.
        let mut s = RxSleep::new(Level::Off, 0);
        assert_eq!(run(&mut s, 0, HOLD_MS + RECENT_MS + 60_000, false), HOLD_MS + RECENT_MS + 60_000);
    }

    /// PWR-1: Battery Save off keeps the receivers on.
    #[test]
    fn turning_it_off_wakes_the_receiver() {
        let mut s = RxSleep::new(Level::L2, 0);
        while !s.asleep() {
            s.tick(s.deadline().unwrap(), false);
        }
        let now = s.deadline().unwrap() - 1;
        assert_eq!(s.set_level(Level::Off, now), Action::Wake);
        assert!(!s.asleep());
    }

    /// XB-13, decision 163: ticked busy every 10 ms (the crossband repeater's receivers awake), the deadline stays
    /// ahead of now, so the Radio task's wait is never already past (on the radio it was 10 s stale and the task
    /// woke every 1 ms, starving the UI and the link). At level Off there is no deadline at all.
    #[test]
    fn busy_keeps_the_deadline_ahead() {
        for level in [Level::L1, Level::L2, Level::L3] {
            let mut s = RxSleep::new(level, 0);
            for now in (0..3 * HOLD_MS).step_by(10) {
                assert_eq!(s.tick(now, true), Action::None);
                assert!(s.deadline().is_some_and(|d| d > now), "{level:?} at {now}");
            }
        }
        let mut s = RxSleep::new(Level::Off, 0);
        run(&mut s, 0, 3 * HOLD_MS, false);
        assert_eq!(s.deadline(), None);
    }
}
