//! The watch engine (manual chapter 13): which slot each BK4829 listens to now, whose audio is heard, and when a
//! watched slot takes over. Plain decisions: the Radio task tunes the chips it names and reports each chip's
//! squelch back on the next [`Watch::tick`].
//!
//! - Two chips: slots on the same chip take turns on it (WATCH-2), [`ACTIVE_DWELL_MS`] on the active slot and
//!   [`WATCHED_DWELL_MS`] on each watched one; a slot with its squelch open keeps the chip, and for
//!   [`HOLD_MS`] after it closes (the reply after the squelch tail).
//! - Audio: the active slot's first; a watched slot only while the active one is closed, unless muted (WATCH-3).
//! - The active slot never changes here: a watched slot that is heard plays and shows RX, and only the user
//!   changes the active slot (WATCH-5; Follow was dropped, decision 81).
//! - Paused (WATCH-6, WATCH-16, TX-34): while keyed and until [`Watch::pause`]'s time, only the active slot.
//! - Cancelled (WATCH-11): when the watch set needs more than one relay state, only the active slot.
//! - Off (BCR-23): slots no chip listens to at all, whatever the set says: the broadcast slot (the SI4732 has it)
//!   and, while broadcast plays with the relay released, slots that need the relay (BCR-15).
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
/// A slot that closes keeps its chip this long (the reply after the squelch tail).
pub const HOLD_MS: u32 = 2_000;

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
    /// Slots not listened to at all, the active one too (BCR-23).
    pub off: u8,
}

/// What to do now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Out {
    /// The slot each chip listens to.
    pub chip: [Option<u8>; 2],
    /// Whose audio is on the speaker.
    pub audio: Option<u8>,
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
}

fn after(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) < 1 << 31
}

impl Watch {
    pub const fn new() -> Self {
        Watch { on: [None; 2], since: [0; 2], open: 0, closed_at: [0; SLOTS], recent: 0, pause_until: None }
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
        let mut set = (c.set | 1 << active) & !c.off;
        // The relay state the set is held to: the active slot's, or released when the active slot is off (BCR-15).
        let base = set & 1 << active != 0 && relay(active as usize);
        if (0..SLOTS).any(|s| set & 1 << s != 0 && relay(s) != base) {
            (set, out.cancelled) = (1 << active & !c.off, true);
        }
        if self.pause_until.is_some_and(|t| after(now, t)) {
            self.pause_until = None;
        }
        if keyed || self.pause_until.is_some() {
            (set, out.paused) = (1 << active & !c.off, true);
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
        out
    }
}

/// BCR-24: the broadcast comes back this long after the last amateur slot heard goes quiet.
pub const BCAST_RETURN_MS: u32 = 2_000;

/// The broadcast's turn on the speaker (BCR-24): any amateur slot heard takes it at once and mutes the broadcast,
/// which comes back [`BCAST_RETURN_MS`] after that slot's audio ends (its squelch tail); a new signal meanwhile
/// starts the wait again.
#[derive(Clone, Copy, Debug, Default)]
pub struct BcastTurn {
    last: Option<u32>,
}

impl BcastTurn {
    pub const fn new() -> Self {
        BcastTurn { last: None }
    }

    /// `slot_heard`: an amateur slot's audio is on the speaker now. True when the broadcast plays.
    pub fn tick(&mut self, now: u32, slot_heard: bool) -> bool {
        if slot_heard {
            self.last = Some(now);
            return false;
        }
        match self.last {
            Some(t) if now.wrapping_sub(t) < BCAST_RETURN_MS => false,
            _ => {
                self.last = None;
                true
            }
        }
    }
}

/// The receive path select (the RF path machine's one RX select, SAFE-11) between the two chips while both have a
/// slot: one band's front end at a time, as stock's dual watch hops. The chip with the slot being heard keeps it;
/// otherwise it alternates, each chip for its slot's dwell. A chip without its path is taken as hearing nothing
/// (to confirm on the bench: if the other band's front end leaks enough, both could listen at once).
#[derive(Clone, Copy, Debug, Default)]
pub struct Hop {
    cur: Option<u8>,
    since: u32,
}

impl Hop {
    pub const fn new() -> Self {
        Hop { cur: None, since: 0 }
    }

    /// The chip whose path is selected now.
    pub fn chip(&self) -> Option<u8> {
        self.cur
    }

    /// The chip to select after a [`Watch::tick`] that gave `out`; `active` and `freq` as in [`Cfg`].
    pub fn next(&mut self, now: u32, out: &Out, active: u8) -> Option<u8> {
        let has = |n: u8| out.chip[n as usize].is_some();
        let next = match (has(0), has(1)) {
            (false, false) => None,
            (true, false) => Some(0),
            (false, true) => Some(1),
            (true, true) => {
                let heard = out.audio.and_then(|s| (0..2u8).find(|&n| out.chip[n as usize] == Some(s)));
                match (heard, self.cur) {
                    (Some(n), _) => Some(n),
                    (None, Some(c)) => {
                        let dwell = if out.chip[c as usize] == Some(active) { ACTIVE_DWELL_MS } else { WATCHED_DWELL_MS };
                        Some(if now.wrapping_sub(self.since) < dwell { c } else { c ^ 1 })
                    }
                    (None, None) => out.chip.iter().position(|&s| s == Some(active)).map_or(Some(0), |n| Some(n as u8)),
                }
            }
        };
        if next != self.cur {
            (self.cur, self.since) = (next, now);
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;

    /// A on 2 m and C on airband share chip 0; B on 70 cm has chip 1.
    fn cfg() -> Cfg {
        Cfg { set: 0b111, active: 0, freq: [mhz(146) + 52_000, mhz(443) + 47_500, mhz(118) + 95_000], muted: 0, off: 0 }
    }

    /// WATCH-2, WATCH-17: slots sharing a chip take turns on it, 150 ms on the active slot and 70 ms on each
    /// watched one; a slot alone on its chip keeps it.
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

    /// WATCH-11: a watch set needing different relay states cancels watch until the set agrees again.
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

    /// WATCH-5 (decision 81): a watched slot that is heard plays while the active one is closed; the engine
    /// never asks for another active slot (it has no such output), and B's audio stops when A opens.
    #[test]
    fn heard_slot_plays_active_stays() {
        let mut w = Watch::new();
        let c = cfg();
        w.tick(0, &c, [false; 2], false);
        let o = w.tick(100, &c, [false, true], false);
        assert_eq!((o.chip[1], o.audio), (Some(1), Some(1)));
        let o = w.tick(200, &c, [true, true], false);
        assert_eq!(o.audio, Some(0), "the active slot's audio first (WATCH-3)");
    }

    /// The path hops between the two chips at their dwells, and stays with the chip being heard.
    #[test]
    fn hop_between_chips() {
        let mut h = Hop::new();
        let both = Out { chip: [Some(0), Some(1)], ..Out::default() };
        assert_eq!(h.next(0, &both, 0), Some(0), "the active slot's chip first");
        assert_eq!(h.next(149, &both, 0), Some(0));
        assert_eq!(h.next(150, &both, 0), Some(1), "active dwell over");
        assert_eq!(h.next(219, &both, 0), Some(1));
        assert_eq!(h.next(220, &both, 0), Some(0), "watched dwell over");
        let heard = Out { audio: Some(1), ..both };
        assert_eq!(h.next(5_000, &heard, 0), Some(1), "B heard: its chip keeps the path");
        assert_eq!(h.next(9_000, &heard, 0), Some(1));
        let one = Out { chip: [None, Some(1)], ..Out::default() };
        assert_eq!(h.next(9_010, &one, 1), Some(1));
        assert_eq!(h.next(9_020, &Out::default(), 0), None);
    }

    /// BCR-23: an off slot gets no chip and no audio, even as the active slot; the rest are watched as before.
    #[test]
    fn off_slots() {
        let mut w = Watch::new();
        // A (active) is the broadcast slot on 100.5 MHz: B and C keep their chips.
        let c = Cfg { freq: [mhz(100) + 50_000, mhz(443) + 47_500, mhz(146) + 52_000], off: 0b001, ..cfg() };
        let o = w.tick(0, &c, [false, false], false);
        assert_eq!(o.chip, [Some(2), Some(1)]);
        assert!(!o.cancelled);
        let o = w.tick(100, &c, [true, false], false);
        assert_eq!(o.audio, Some(2), "C heard while A is off");
        // Keyed or paused: only the active slot, which is off: nothing.
        let o = w.tick(200, &c, [false, false], true);
        assert_eq!(o.chip, [None, None]);
        // A 6 m slot needing the relay, off while broadcast plays: no cancelling for it.
        let c = Cfg { freq: [mhz(100) + 50_000, mhz(50) + 12_500, mhz(146) + 52_000], off: 0b011, ..cfg() };
        let o = w.tick(5_000, &c, [false, false], false);
        assert!(!o.cancelled && o.chip == [Some(2), None], "{o:?}");
    }

    /// BCR-24: a slot heard mutes the broadcast at once; it returns 2 s after the slot's audio ends, and a new
    /// signal in the wait starts it again.
    #[test]
    fn broadcast_turn() {
        let mut b = BcastTurn::new();
        assert!(b.tick(0, false));
        assert!(!b.tick(100, true));
        assert!(!b.tick(1_000, true));
        assert!(!b.tick(2_900, false), "1.9 s after");
        assert!(!b.tick(2_950, true), "a new signal");
        assert!(!b.tick(4_900, false));
        assert!(b.tick(4_950, false), "2 s after the last");
        assert!(b.tick(9_000, false));
    }
}
