//! The scan engine (manual chapter 14): one slot walking a [`List`] (channels in number order, or a range at
//! its step, SCAN-4), stopping on a carrier and going on by the resume rule (SCAN-5), skipping what is flagged
//! (SCAN-9, SCAN-10) or skipped for this scan (SCAN-11, SCAN-18), looking at the priority channels at their
//! interval (SCAN-12), turned by ▲▼ (SCAN-16) and ended by PTT (SCAN-19). The Radio task tunes the frequency
//! it names and reports the squelch back on the next [`Scan::tick`].
//!
//! Timings without a rule of their own are written into the manual as "to confirm on the bench" (SCAN-20).

use crate::freq::Freq10;

/// How long each step listens before going on (to confirm on the bench).
pub const STEP_MS: u32 = 50;
/// After the carrier drops on a hit, it waits this long before going on (to confirm on the bench).
pub const RESUME_WAIT_MS: u32 = 2_000;
/// How long a priority look listens on each priority channel.
pub const PRIO_DWELL_MS: u32 = 50;
/// Nuisance skips held for one scan (SCAN-11).
pub const NUISANCE: usize = 8;

/// What a scan walks: positions 0..len, each a frequency or skipped.
pub trait List {
    fn len(&self) -> u16;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The frequency at `i`; None when it is never stopped on (a channel's Skip flag, an empty channel, a
    /// preset's skipped frequency).
    fn at(&self, i: u16) -> Option<Freq10>;
    /// The `k`th priority position (SCAN-12), None past the last.
    fn priority(&self, _k: u8) -> Option<u16> {
        None
    }
}

/// A range preset: `lo` to `hi` at `step` (SCAN-4), its skipped frequencies never stopped on (SCAN-10).
#[derive(Clone, Copy, Debug)]
pub struct Range<'a> {
    pub lo: Freq10,
    pub hi: Freq10,
    pub step: Freq10,
    pub skipped: &'a [Freq10],
}

impl List for Range<'_> {
    fn len(&self) -> u16 {
        ((self.hi.saturating_sub(self.lo)) / self.step.max(1) + 1).min(u16::MAX as u32) as u16
    }

    fn at(&self, i: u16) -> Option<Freq10> {
        let f = self.lo + i as u32 * self.step;
        (f <= self.hi && !self.skipped.contains(&f)).then_some(f)
    }
}

/// SCAN-5's resume rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// After this many seconds on a hit (or [`RESUME_WAIT_MS`] after the carrier drops, if sooner).
    Time(u8),
    /// [`RESUME_WAIT_MS`] after the carrier drops.
    Drop,
    /// Stop on the hit: the scan ends there.
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    Off,
    /// Listening at `pos` since.
    Step(u32),
    /// On a hit at `pos` since; the carrier dropped at the second, if it has.
    Hit(u32, Option<u32>),
}

/// What to do now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Out {
    /// Tune here (only when it changes).
    pub tune: Option<Freq10>,
    /// A new hit at this position (count it, log it).
    pub hit: Option<u16>,
    /// The scan ended (stop on hit, or nothing left to scan).
    pub ended: bool,
}

/// The engine (60 bytes).
#[derive(Clone, Copy, Debug)]
pub struct Scan {
    st: St,
    pos: u16,
    start: u16,
    up: bool,
    pub resume: Resume,
    /// The priority look's interval (0 1 6 5; 0 = off).
    pub prio_ms: u32,
    prio_at: u32,
    /// Looking at priority `k` since, coming back to `pos` after.
    look: Option<(u8, u32)>,
    nuisance: [u16; NUISANCE],
    nn: u8,
    /// Nuisance skips this scan (SCAN-7's count).
    pub skips: u8,
    pub hits: u8,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

impl Scan {
    pub const fn new() -> Self {
        Scan {
            st: St::Off,
            pos: 0,
            start: 0,
            up: true,
            resume: Resume::Time(5),
            prio_ms: 0,
            prio_at: 0,
            look: None,
            nuisance: [0; NUISANCE],
            nn: 0,
            skips: 0,
            hits: 0,
        }
    }

    pub fn running(&self) -> bool {
        self.st != St::Off
    }

    /// Stopped on a hit, or in the resume wait after one.
    pub fn on_hit(&self) -> bool {
        matches!(self.st, St::Hit(..))
    }

    pub fn pos(&self) -> u16 {
        self.pos
    }

    /// Start at position `from` (the slot's channel or frequency, already tuned): it listens there a step first.
    pub fn start(&mut self, from: u16, now: u32) {
        *self = Scan { resume: self.resume, prio_ms: self.prio_ms, up: self.up, ..Scan::new() };
        (self.pos, self.start, self.st, self.prio_at) = (from, from, St::Step(now), now);
    }

    pub fn stop(&mut self) {
        self.st = St::Off;
    }

    fn skipped(&self, i: u16) -> bool {
        self.nuisance[..self.nn as usize].contains(&i)
    }

    /// The next position from `pos` in the scan's direction that is stopped on; None if there is none.
    fn next(&self, l: &impl List) -> Option<u16> {
        let n = l.len();
        (1..=n).map(|k| if self.up { (self.pos as u32 + k as u32) % n as u32 } else { (self.pos as u32 + n as u32 * 2 - k as u32) % n as u32 } as u16)
            .find(|&i| l.at(i).is_some() && !self.skipped(i))
    }

    /// Off the hit and on to the next position (counted as no skip).
    fn leave(&mut self, l: &impl List, now: u32, out: &mut Out) {
        match self.next(l) {
            Some(i) => {
                self.pos = i;
                self.st = St::Step(now);
                out.tune = l.at(i);
            }
            None => {
                self.st = St::Off;
                out.ended = true;
            }
        }
    }

    /// ▲▼ (SCAN-16): the direction from here; on a hit they leave it now, without counting a skip.
    pub fn dir(&mut self, up: bool, l: &impl List, now: u32) -> Out {
        self.up = up;
        let mut out = Out::default();
        if self.on_hit() {
            self.leave(l, now, &mut out);
        }
        out
    }

    /// \* (SCAN-11, SCAN-18): skip the hit for the rest of this scan, counted.
    pub fn skip_once(&mut self, l: &impl List, now: u32) -> Out {
        let mut out = Out::default();
        if self.on_hit() {
            if (self.nn as usize) < NUISANCE {
                self.nuisance[self.nn as usize] = self.pos;
                self.nn += 1;
            }
            self.skips = self.skips.saturating_add(1);
            self.leave(l, now, &mut out);
        }
        out
    }

    /// \# (SCAN-17): the hit to skip for good, for the caller to store (the channel's Skip flag, the preset's
    /// skipped list) before the next tick; the scan leaves it.
    pub fn skip_lasting(&mut self, l: &impl List, now: u32) -> (Option<u16>, Out) {
        let mut out = Out::default();
        if !self.on_hit() {
            return (None, out);
        }
        let p = self.pos;
        self.leave(l, now, &mut out);
        (Some(p), out)
    }

    /// PTT (SCAN-19): the scan ends; the position to transmit on: the hit, or where the scan started.
    pub fn ptt(&mut self) -> u16 {
        let p = if self.on_hit() { self.pos } else { self.start };
        self.st = St::Off;
        p
    }

    /// A tick: `open` is the squelch where the last tune put it.
    pub fn tick(&mut self, l: &impl List, now: u32, open: bool) -> Out {
        let mut out = Out::default();
        // A priority look: open there is a hit there; otherwise back where it was.
        if let Some((k, since)) = self.look {
            let p = l.priority(k);
            if open && p.is_some() {
                self.look = None;
                self.pos = p.unwrap_or(self.pos);
                self.hits = self.hits.saturating_add(1);
                (self.st, out.hit) = (St::Hit(now, None), Some(self.pos));
                return out;
            }
            if now.wrapping_sub(since) < PRIO_DWELL_MS && p.is_some() {
                return out;
            }
            match l.priority(k + 1).filter(|&q| Some(q) != Some(self.pos)) {
                Some(q) => {
                    self.look = Some((k + 1, now));
                    out.tune = l.at(q);
                }
                None => {
                    self.look = None;
                    out.tune = l.at(self.pos);
                    if let St::Step(_) = self.st {
                        self.st = St::Step(now);
                    }
                }
            }
            return out;
        }
        match self.st {
            St::Off => {}
            St::Step(since) if open && now.wrapping_sub(since) >= STEP_MS / 2 => {
                self.hits = self.hits.saturating_add(1);
                out.hit = Some(self.pos);
                if self.resume == Resume::Never {
                    self.st = St::Off;
                    out.ended = true;
                    return out;
                }
                self.st = St::Hit(now, None);
            }
            St::Step(since) => {
                if now.wrapping_sub(since) >= STEP_MS {
                    self.leave(l, now, &mut out);
                }
            }
            St::Hit(t0, dropped) => {
                let dropped = if open { None } else { dropped.or(Some(now)) };
                self.st = St::Hit(t0, dropped);
                let timed = matches!(self.resume, Resume::Time(s) if now.wrapping_sub(t0) >= s as u32 * 1000);
                if timed || dropped.is_some_and(|d| now.wrapping_sub(d) >= RESUME_WAIT_MS) {
                    self.leave(l, now, &mut out);
                }
            }
        }
        // SCAN-12: the priority channels at their interval, from a step or a hit.
        if self.running() && self.prio_ms > 0 && now.wrapping_sub(self.prio_at) >= self.prio_ms {
            self.prio_at = now;
            if let Some(q) = l.priority(0).filter(|&q| q != self.pos) {
                self.look = Some((0, now));
                out.tune = l.at(q);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;

    /// Ten channels; 3 has its Skip flag; 7 is priority.
    struct Chans;
    impl List for Chans {
        fn len(&self) -> u16 {
            10
        }
        fn at(&self, i: u16) -> Option<Freq10> {
            (i != 3 && i < 10).then(|| mhz(146) + i as u32 * 1_250)
        }
        fn priority(&self, k: u8) -> Option<u16> {
            (k == 0).then_some(7)
        }
    }

    fn walk(s: &mut Scan, l: &impl List, from: u32, to: u32, open: impl Fn(u16) -> bool) -> u32 {
        let mut t = from;
        while t < to {
            s.tick(l, t, open(s.pos));
            t += 10;
        }
        t
    }

    #[test]
    fn range_list() {
        let skipped = [mhz(146) + 2_500];
        let r = Range { lo: mhz(146), hi: mhz(146) + 5_000, step: 1_250, skipped: &skipped };
        assert_eq!(r.len(), 5);
        assert_eq!(r.at(1), Some(mhz(146) + 1_250));
        assert_eq!(r.at(2), None, "a skipped frequency");
        assert_eq!(r.at(5), None);
    }

    #[test]
    fn walks_in_order_past_flagged_channels() {
        let mut s = Scan::new();
        s.start(2, 0);
        let mut seen = std::vec::Vec::new();
        for t in (0..400).step_by(10) {
            if let Some(f) = s.tick(&Chans, t, false).tune {
                seen.push(((f - mhz(146)) / 1_250) as u16);
            }
        }
        assert_eq!(seen[..4], [4, 5, 6, 7]);
        s.dir(false, &Chans, 400);
        let mut down = std::vec::Vec::new();
        for t in (400..600).step_by(10) {
            if let Some(f) = s.tick(&Chans, t, false).tune {
                down.push(((f - mhz(146)) / 1_250) as u16);
            }
        }
        assert!(down.windows(2).all(|w| w[1] == (w[0] + 9) % 10 || w[1] == 2 && w[0] == 4), "down, past 3: {down:?}");
    }

    #[test]
    fn hit_resume_on_drop_and_time() {
        let mut s = Scan { resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        let t = walk(&mut s, &Chans, 0, 200, |p| p == 2);
        assert!(s.on_hit() && s.pos() == 2 && s.hits == 1);
        let t = walk(&mut s, &Chans, t, t + 10_000, |p| p == 2);
        assert!(s.on_hit(), "carrier still there: stays (Drop)");
        let t2 = walk(&mut s, &Chans, t, t + RESUME_WAIT_MS - 20, |_| false);
        assert!(s.on_hit(), "in the resume wait");
        walk(&mut s, &Chans, t2, t2 + 40, |_| false);
        assert!(!s.on_hit() && s.pos() == 4);

        let mut s = Scan { resume: Resume::Time(5), ..Scan::new() };
        s.start(0, 0);
        let t = walk(&mut s, &Chans, 0, 200, |p| p == 2);
        walk(&mut s, &Chans, t, t + 5_010, |p| p == 2);
        assert!(s.pos() != 2, "time's up with the carrier on");

        let mut s = Scan { resume: Resume::Never, ..Scan::new() };
        s.start(0, 0);
        walk(&mut s, &Chans, 0, 200, |p| p == 2);
        assert!(!s.running() && s.pos() == 2);
    }

    #[test]
    fn keys_on_a_hit() {
        let mut s = Scan::new();
        s.start(0, 0);
        let t = walk(&mut s, &Chans, 0, 200, |p| p == 2 || p == 5);
        assert_eq!(s.pos(), 2);
        // * : skipped for this scan, counted.
        let o = s.skip_once(&Chans, t);
        assert_eq!((s.skips, o.tune), (1, Some(mhz(146) + 4 * 1_250)));
        // Round again: 2 is passed over.
        let t = walk(&mut s, &Chans, t, t + 2_000, |p| p == 5);
        assert_eq!(s.pos(), 5);
        // # : handed back to store; not counted.
        let (p, _) = s.skip_lasting(&Chans, t);
        assert_eq!((p, s.skips), (Some(5), 1));
        // ▲▼ off a hit: no skip counted.
        let t = walk(&mut s, &Chans, t, t + 2_000, |p| p == 8);
        assert!(s.on_hit() && s.pos() == 8);
        s.dir(false, &Chans, t);
        assert_eq!((s.pos(), s.skips), (7, 1));
        // Not on a hit: the keys only turn.
        assert_eq!(s.skip_lasting(&Chans, t).0, None);
    }

    #[test]
    fn ptt_ends_on_the_hit_or_the_start() {
        let mut s = Scan::new();
        s.start(1, 0);
        walk(&mut s, &Chans, 0, 200, |p| p == 4);
        assert_eq!(s.ptt(), 4);
        assert!(!s.running());
        s.start(1, 0);
        walk(&mut s, &Chans, 0, 100, |_| false);
        assert_eq!(s.ptt(), 1);
    }

    #[test]
    fn priority_look() {
        let mut s = Scan { prio_ms: 1_000, resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        // On a hit at 1; at the interval it looks at 7, finds nothing, comes back.
        let t = walk(&mut s, &Chans, 0, 100, |p| p == 1);
        assert_eq!(s.pos(), 1);
        let mut tuned = std::vec::Vec::new();
        let mut t = t;
        while t < 1_200 {
            if let Some(f) = s.tick(&Chans, t, s.pos() == 1 && s.look.is_none()).tune {
                tuned.push(f);
            }
            t += 10;
        }
        assert_eq!(tuned, [mhz(146) + 7 * 1_250, mhz(146) + 1_250]);
        assert!(s.on_hit() && s.pos() == 1);
        // Next interval: 7 is open, it becomes the hit.
        while s.look.is_none() {
            s.tick(&Chans, t, true);
            t += 10;
        }
        s.tick(&Chans, t, true);
        assert_eq!((s.pos(), s.hits), (7, 2));
    }
}
