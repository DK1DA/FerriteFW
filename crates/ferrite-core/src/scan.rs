//! The scan engine (manual chapter 14): one slot walking a [`List`] (channels in number order, or a range at
//! its step, SCAN-4), stopping on a carrier and going on by the resume rule (SCAN-5), skipping what is flagged
//! (SCAN-9, SCAN-10) or skipped for this scan (SCAN-11, SCAN-18), looking at the priority channels at their
//! interval (SCAN-12), turned by ▲▼ (SCAN-16) and ended by PTT (SCAN-19). The Radio task tunes the frequency
//! it names and reports the squelch back on the next [`Scan::tick`].
//!
//! Range scans take a fast pass (SCAN-21): each step reads the RSSI only for [`FAST_STEP_MS`], and a step over
//! the floor is held for a full squelch check before it counts as a hit. [`Scan::check`] tells the Radio task
//! which of the two its `open` answers. [`WithTx`] adds a channel's transmit frequency after its receive one
//! (SCAN-22).
//!
//! Timings without a rule of their own are written into the manual as "to confirm on the bench" (SCAN-20).

use crate::freq::Freq10;

/// How long each step listens before going on (to confirm on the bench).
pub const STEP_MS: u32 = 50;
/// A range scan's fast pass: how long each step reads the RSSI before going on (to confirm on the bench; the
/// target is at least nicFW's 30 steps a second, SCAN-21).
pub const FAST_STEP_MS: u32 = 15;
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
    /// The transmit frequency at `i` when it differs from the receive one (a shift or split), for [`WithTx`].
    fn tx_at(&self, _i: u16) -> Option<Freq10> {
        None
    }
}

/// SCAN-22: a list with each channel's transmit frequency after its receive one, for channels with a shift or
/// split (the others' odd positions are never stopped on). Positions are doubled: channel `i` is `2 * i`.
#[derive(Clone, Copy, Debug)]
pub struct WithTx<L>(pub L);

impl<L: List> List for WithTx<L> {
    fn len(&self) -> u16 {
        self.0.len().saturating_mul(2)
    }

    fn at(&self, i: u16) -> Option<Freq10> {
        if i.is_multiple_of(2) {
            self.0.at(i / 2)
        } else {
            self.0.at(i / 2).and(self.0.tx_at(i / 2)).filter(|&t| Some(t) != self.0.at(i / 2))
        }
    }

    fn priority(&self, k: u8) -> Option<u16> {
        self.0.priority(k).map(|p| p * 2)
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

/// The band a range scan walks when it isn't given a preset: the band `f` is in, at its usual step (the Radio
/// task's band scan, as the simulators' fake radio scans). Outside these, a window of 1 MHz each side at 12.5 kHz.
pub fn band_range(f: Freq10) -> Range<'static> {
    use crate::freq::mhz;
    // lo, hi, step (10 Hz units): HF, 6 m, airband, 2 m and VHF, marine, 1.25 m, UHF. 5 kHz on the VHF bands
    // covers both the 15 kHz and 12.5 kHz channel plans' usual frequencies (146.520, 145.500).
    const BANDS: [(Freq10, Freq10, Freq10); 7] = [
        (mhz(1) + 80_000, mhz(30), 500),
        (mhz(50), mhz(54), 500),
        (mhz(108), mhz(137), 2_500),
        (mhz(137), mhz(156), 500),
        (mhz(156), mhz(163), 2_500),
        (mhz(220), mhz(225), 500),
        (mhz(400), mhz(520), 1_250),
    ];
    let (lo, hi, step) = BANDS
        .iter()
        .copied()
        .find(|&(lo, hi, _)| (lo..hi).contains(&f))
        .unwrap_or((f.saturating_sub(mhz(1)), f + mhz(1), 1_250));
    Range { lo, hi: hi - step, step, skipped: &[] }
}

/// Where `f` is in `r` (its nearest step at or below).
pub fn range_pos(r: &Range<'_>, f: Freq10) -> u16 {
    (f.saturating_sub(r.lo) / r.step.max(1)).min(r.len().saturating_sub(1) as u32) as u16
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
    /// The fast pass found the RSSI over the floor at `pos` since: listening with the squelch to confirm.
    Confirm(u32),
    /// On a hit at `pos` since; the carrier dropped at the second, if it has.
    Hit(u32, Option<u32>),
}

/// What [`Scan::tick`]'s `open` should answer for the step tuned now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The RSSI is over the noise floor (the fast pass: no squelch wait).
    Rssi,
    /// The squelch is open (a carrier, SQL-1).
    Squelch,
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
    /// A range scan: steps take the fast pass (SCAN-21).
    pub fast: bool,
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
            fast: false,
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
        *self = Scan { resume: self.resume, prio_ms: self.prio_ms, up: self.up, fast: self.fast, ..Scan::new() };
        (self.pos, self.start, self.st, self.prio_at) = (from, from, St::Step(now), now);
    }

    /// What `open` answers on the next tick: the RSSI on a fast step, the squelch otherwise.
    pub fn check(&self) -> Check {
        if self.fast && self.look.is_none() && matches!(self.st, St::Step(_)) {
            Check::Rssi
        } else {
            Check::Squelch
        }
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
                    if let St::Step(_) | St::Confirm(_) = self.st {
                        self.st = St::Step(now);
                    }
                }
            }
            return out;
        }
        match self.st {
            St::Off => {}
            St::Step(_) if self.fast && open => self.st = St::Confirm(now),
            St::Step(since) if self.fast => {
                if now.wrapping_sub(since) >= FAST_STEP_MS {
                    self.leave(l, now, &mut out);
                }
            }
            St::Step(since) | St::Confirm(since) if open && now.wrapping_sub(since) >= STEP_MS / 2 => {
                self.hits = self.hits.saturating_add(1);
                out.hit = Some(self.pos);
                if self.resume == Resume::Never {
                    self.st = St::Off;
                    out.ended = true;
                    return out;
                }
                self.st = St::Hit(now, None);
            }
            St::Step(since) | St::Confirm(since) => {
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
    fn fast_pass_confirms_with_the_squelch() {
        let skipped = [];
        let r = Range { lo: mhz(146), hi: mhz(147), step: 1_250, skipped: &skipped };
        let mut s = Scan { fast: true, resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        // An empty band: one step per FAST_STEP_MS (about 3x the 50 ms dwell).
        let mut t = 0;
        let mut steps = 0;
        while t < 1_000 {
            assert_eq!(s.check(), Check::Rssi);
            steps += s.tick(&r, t, false).tune.is_some() as u32;
            t += 5;
        }
        assert!(steps >= 1_000 / FAST_STEP_MS - 2, "{steps} steps a second");
        // Noise over the floor at 30 but no carrier: held for the squelch check, then on.
        let p = s.pos();
        s.tick(&r, t, true);
        assert_eq!((s.check(), s.pos()), (Check::Squelch, p));
        let t2 = walk(&mut s, &r, t + 5, t + STEP_MS + 10, |_| false);
        assert!(s.pos() != p && !s.on_hit() && s.hits == 0, "a false alarm costs one full step");
        // A real carrier: the RSSI, then the squelch, then a hit.
        let target = (s.pos() + 3) % r.len();
        walk(&mut s, &r, t2, t2 + 400, |q| q == target);
        assert!(s.on_hit() && s.pos() == target && s.hits == 1);
        assert_eq!(s.check(), Check::Squelch);
    }

    /// Ten channels, each with a 600 kHz shift but 4 (simplex).
    struct Shifted;
    impl List for Shifted {
        fn len(&self) -> u16 {
            10
        }
        fn at(&self, i: u16) -> Option<Freq10> {
            Chans.at(i)
        }
        fn priority(&self, k: u8) -> Option<u16> {
            Chans.priority(k)
        }
        fn tx_at(&self, i: u16) -> Option<Freq10> {
            Chans.at(i).map(|f| if i == 4 { f } else { f + 60_000 })
        }
    }

    #[test]
    fn with_tx_adds_the_inputs() {
        let l = WithTx(Shifted);
        assert_eq!(l.len(), 20);
        assert_eq!(l.at(4), Shifted.at(2));
        assert_eq!(l.at(5), Shifted.at(2).map(|f| f + 60_000), "channel 2's input");
        assert_eq!(l.at(9), None, "4 is simplex: nothing more to hear");
        assert_eq!((l.at(6), l.at(7)), (None, None), "3 is flagged Skip: neither side");
        assert_eq!(l.priority(0), Some(14));
        // A scan stops on an input.
        let mut s = Scan::new();
        s.start(0, 0);
        walk(&mut s, &l, 0, 600, |p| p == 5);
        assert!(s.on_hit() && s.pos() == 5);
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

    #[test]
    fn band_ranges() {
        let r = band_range(mhz(146) + 52_000);
        assert_eq!((r.lo, r.step), (mhz(137), 500));
        assert_eq!(r.at(range_pos(&r, mhz(146) + 52_000)), Some(mhz(146) + 52_000));
        let r = band_range(mhz(121) + 50_000);
        assert_eq!((r.lo, r.step), (mhz(108), 2_500));
        let r = band_range(mhz(300) + 10_000);
        assert_eq!((r.lo, r.hi), (mhz(299) + 10_000, mhz(301) + 10_000 - 1_250), "no band: a window around it");
        assert_eq!(range_pos(&r, mhz(400)), r.len() - 1, "clamped to the range");
    }
}
