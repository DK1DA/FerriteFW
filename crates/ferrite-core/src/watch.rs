//! The watch engine (manual chapter 13): which slot each BK4829 listens to now, whose audio is heard, and when a
//! watched slot takes over. Plain decisions: the Radio task tunes the chips it names and reports each chip's
//! squelch back on the next [`Watch::tick`].
//!
//! - Two chips: slots on the same chip take turns on it (WATCH-2), [`ACTIVE_DWELL_MS`] on the active slot and
//!   [`WATCHED_DWELL_MS`] on each watched one; a slot with its squelch open keeps the chip, and for
//!   [`HOLD_MS`] after it closes (the reply after the squelch tail).
//! - Audio: the active slot's first; a watched slot only while the active one is closed, unless muted (WATCH-3).
//! - Follow (WATCH-5): on, a watched slot that is heard becomes active; return, back to the first slot after
//!   it has been closed for [`HOLD_MS`].
//! - Paused (WATCH-6, WATCH-16, TX-34): while keyed and until [`Watch::pause`]'s time, only the active slot.
//! - Cancelled (WATCH-11): when the watch set needs more than one relay state, only the active slot.
//!
//! The timings marked "to confirm on the bench" in the manual (WATCH-17).

use crate::freq::{band, Freq10};
use crate::msg::SLOTS;
use crate::path::Sel;

/// How long the active slot keeps a shared chip per turn (to confirm on the bench).
pub const ACTIVE_DWELL_MS: u32 = 150;
/// How long a watched slot keeps a shared chip per turn: enough for the squelch to open on a carrier (stock
/// hops about every 70 ms; to confirm on the bench).
pub const WATCHED_DWELL_MS: u32 = 70;
/// A slot that closes keeps its chip (and a followed slot stays active) this long.
pub const HOLD_MS: u32 = 2_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Follow {
    #[default]
    Off,
    On,
    /// On, then back after the squelch tail.
    Return,
}

/// What the engine is told each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cfg {
    /// The watch set, a bit per slot (the active slot is always in).
    pub set: u8,
    pub active: u8,
    /// Each slot's frequency now (for its chip and relay state).
    pub freq: [Freq10; SLOTS],
    /// Watched slots not heard (still highlighted).
    pub muted: u8,
    pub follow: Follow,
}

/// What to do now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Out {
    /// The slot each chip listens to.
    pub chip: [Option<u8>; 2],
    /// Whose audio is on the speaker.
    pub audio: Option<u8>,
    /// Follow: make this slot active.
    pub follow: Option<u8>,
    /// WATCH-11: the set needs both relay states; only the active slot is listened to.
    pub cancelled: bool,
    /// Paused (keyed or [`Watch::pause`]).
    pub paused: bool,
}

/// The engine (36 bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct Watch {
    on: [Option<u8>; 2],
    since: [u32; 2],
    /// Squelch open, a bit per slot, as last heard.
    open: u8,
    closed_at: [u32; SLOTS],
    /// Closed within [`HOLD_MS`], a bit per slot.
    recent: u8,
    pause_until: Option<u32>,
    /// Follow return: the slot to go back to.
    home: Option<u8>,
}

fn after(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) < 1 << 31
}

impl Watch {
    pub const fn new() -> Self {
        Watch { on: [None; 2], since: [0; 2], open: 0, closed_at: [0; SLOTS], recent: 0, pause_until: None, home: None }
    }

    /// Pause for `secs` (WATCH-6, WATCH-16); 0 resumes now.
    pub fn pause(&mut self, now: u32, secs: u16) {
        self.pause_until = (secs > 0).then(|| now.wrapping_add(secs as u32 * 1000));
    }

    /// Squelch open on a slot, as last heard.
    pub fn open(&self, slot: u8) -> bool {
        self.open & 1 << slot != 0
    }

    /// A tick: `sq` is each chip's squelch since the last one (for the slot it was on), `keyed` any slot
    /// transmitting.
    pub fn tick(&mut self, now: u32, c: &Cfg, sq: [bool; 2], keyed: bool) -> Out {
        for (n, s) in self.on.into_iter().enumerate() {
            let Some(s) = s else { continue };
            if self.open(s) && !sq[n] {
                self.closed_at[s as usize] = now;
                self.recent |= 1 << s;
            }
            self.open = self.open & !(1 << s) | (sq[n] as u8) << s;
        }
        for s in 0..SLOTS {
            if now.wrapping_sub(self.closed_at[s]) >= HOLD_MS {
                self.recent &= !(1 << s);
            }
        }
        let mut out = Out::default();
        let active = c.active % SLOTS as u8;
        let relay = |s: usize| Sel::rx(c.freq[s]).relay();
        let mut set = c.set | 1 << active;
        if (0..SLOTS).any(|s| set & 1 << s != 0 && relay(s) != relay(active as usize)) {
            (set, out.cancelled) = (1 << active, true);
        }
        if self.pause_until.is_some_and(|t| after(now, t)) {
            self.pause_until = None;
        }
        if keyed || self.pause_until.is_some() {
            (set, out.paused) = (1 << active, true);
        }
        self.open &= set;
        for n in 0..2u8 {
            let mine = |s: u8| set & 1 << s != 0 && band(c.freq[s as usize]).1 == n;
            let cur = self.on[n as usize].filter(|&s| mine(s));
            let next = match cur {
                // Open, or closed within the hold: it keeps the chip.
                Some(s) if (self.open | self.recent) & 1 << s != 0 => Some(s),
                Some(s) => {
                    let dwell = if s == active { ACTIVE_DWELL_MS } else { WATCHED_DWELL_MS };
                    if now.wrapping_sub(self.since[n as usize]) < dwell {
                        Some(s)
                    } else {
                        (1..=SLOTS as u8).map(|i| (s + i) % SLOTS as u8).find(|&x| mine(x))
                    }
                }
                None => (0..SLOTS as u8).map(|i| (active + i) % SLOTS as u8).find(|&x| mine(x)),
            };
            if next != self.on[n as usize] {
                self.since[n as usize] = now;
            }
            self.on[n as usize] = next;
        }
        out.chip = self.on;
        out.audio = if self.open(active) {
            Some(active)
        } else {
            (0..SLOTS as u8).find(|&s| s != active && self.open(s) && c.muted & 1 << s == 0)
        };
        if self.home == Some(active) {
            self.home = None; // back already (or the UI never followed): nothing to return to
        }
        match (c.follow, out.audio) {
            (Follow::Off, _) => self.home = None,
            (f, Some(s)) if s != active => {
                if f == Follow::Return && self.home.is_none() {
                    self.home = Some(active);
                }
                out.follow = Some(s);
            }
            (Follow::Return, None) => {
                if let Some(h) = self.home.filter(|&h| h != active && self.recent & 1 << active == 0) {
                    out.follow = Some(h);
                    self.home = None;
                }
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;

    /// A on 2 m and C on airband share chip 0; B on 70 cm has chip 1.
    fn cfg() -> Cfg {
        Cfg { set: 0b111, active: 0, freq: [mhz(146) + 52_000, mhz(443) + 47_500, mhz(118) + 95_000], muted: 0, follow: Follow::Off }
    }

    #[test]
    fn turns_on_a_shared_chip() {
        let mut w = Watch::new();
        let c = cfg();
        let o = w.tick(0, &c, [false; 2], false);
        assert_eq!(o.chip, [Some(0), Some(1)]);
        assert_eq!(w.tick(149, &c, [false; 2], false).chip[0], Some(0));
        assert_eq!(w.tick(150, &c, [false; 2], false).chip[0], Some(2), "active dwell over");
        assert_eq!(w.tick(219, &c, [false; 2], false).chip[0], Some(2));
        assert_eq!(w.tick(220, &c, [false; 2], false).chip[0], Some(0), "watched dwell over");
        assert_eq!(w.tick(220, &c, [false; 2], false).chip[1], Some(1), "alone on its chip");
    }

    #[test]
    fn open_slot_keeps_the_chip_and_audio_priority() {
        let mut w = Watch::new();
        let c = cfg();
        w.tick(0, &c, [false; 2], false);
        w.tick(150, &c, [false; 2], false); // C on chip 0
        let o = w.tick(200, &c, [true, true], false); // C and B open
        assert_eq!(o.chip[0], Some(2));
        assert_eq!(o.audio, Some(1), "B first among the watched");
        let o = w.tick(5_000, &c, [true, true], false);
        assert_eq!(o.chip[0], Some(2), "still open: stays");
        let o = w.tick(5_100, &c, [false, true], false);
        assert_eq!(o.chip[0], Some(2), "closed, within the hold");
        assert_eq!(w.tick(7_100, &c, [false, true], false).chip[0], Some(0), "hold over");
        // A opens: its audio wins over B's.
        assert_eq!(w.tick(7_200, &c, [true, true], false).audio, Some(0));
        let o = w.tick(7_300, &Cfg { muted: 0b010, ..c }, [false, true], false);
        assert_eq!(o.audio, None, "B muted, A closed");
    }

    #[test]
    fn paused_while_keyed_and_by_time() {
        let mut w = Watch::new();
        let c = cfg();
        let o = w.tick(0, &c, [false; 2], true);
        assert!(o.paused);
        assert_eq!(o.chip, [Some(0), None]);
        w.pause(0, 30);
        assert!(w.tick(29_999, &c, [false; 2], false).paused);
        let o = w.tick(30_000, &c, [false; 2], false);
        assert!(!o.paused && o.chip[1] == Some(1));
        w.pause(31_000, 60);
        w.pause(32_000, 0);
        assert!(!w.tick(32_000, &c, [false; 2], false).paused, "a key resumes");
    }

    #[test]
    fn relay_mismatch_cancels() {
        let mut w = Watch::new();
        let c = Cfg { freq: [mhz(146), mhz(28) + 40_000, mhz(446)], ..cfg() };
        let o = w.tick(0, &c, [false; 2], false);
        assert!(o.cancelled);
        assert_eq!(o.chip, [Some(0), None]);
        let o = w.tick(100, &Cfg { set: 0b101, ..c }, [false; 2], false);
        assert!(!o.cancelled && o.chip[1] == Some(2), "resumes once the set agrees");
    }

    #[test]
    fn follow_and_return() {
        let mut w = Watch::new();
        let c = Cfg { follow: Follow::Return, ..cfg() };
        w.tick(0, &c, [false; 2], false);
        let o = w.tick(100, &c, [false, true], false);
        assert_eq!(o.follow, Some(1));
        let c1 = Cfg { active: 1, ..c };
        assert_eq!(w.tick(200, &c1, [false, true], false).follow, None);
        assert_eq!(w.tick(300, &c1, [false, false], false).follow, None, "within the hold");
        assert_eq!(w.tick(2_299, &c1, [false, false], false).follow, None);
        assert_eq!(w.tick(2_300, &c1, [false, false], false).follow, Some(0), "back after the tail");
        // Not followed (the UI on another screen): no return left over for when B is chosen later.
        let mut w = Watch::new();
        let c = Cfg { follow: Follow::Return, ..cfg() };
        w.tick(0, &c, [false; 2], false);
        assert_eq!(w.tick(100, &c, [false, true], false).follow, Some(1));
        w.tick(200, &c, [false, false], false);
        w.tick(3_000, &c, [false, false], false);
        assert_eq!(w.tick(3_100, &Cfg { active: 1, ..c }, [false, false], false).follow, None, "B by hand stays");
        // On: follows, never returns.
        let mut w = Watch::new();
        let c = Cfg { follow: Follow::On, ..cfg() };
        w.tick(0, &c, [false; 2], false);
        assert_eq!(w.tick(100, &c, [false, true], false).follow, Some(1));
        let c1 = Cfg { active: 1, ..c };
        w.tick(200, &c1, [false, true], false);
        assert_eq!(w.tick(300, &c1, [false, false], false).follow, None);
        assert_eq!(w.tick(10_000, &c1, [false, false], false).follow, None, "stays");
        let mut w = Watch::new();
        let c = Cfg { follow: Follow::Off, ..cfg() };
        w.tick(0, &c, [false; 2], false);
        assert_eq!(w.tick(100, &c, [false, true], false).follow, None);
    }
}
