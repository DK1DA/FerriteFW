//! MODE-3: the Radio task's lease on an exclusive mode (band scope or home's strip, frequency counter, FM
//! broadcast, HF mode, crossband). The UI renews it while it wants the mode ([`crate::msg::RadioCmd::Lease`], or
//! any command that starts or retunes the mode); when no renewal arrives for [`WINDOW_MS`] the Radio task stops
//! the mode by itself and goes back to normal receive, so no app bug and no starved UI can leave the radio
//! stuck out of receive.

use crate::msg::{BroadcastCmd, RadioCmd};

/// The UI renews at least this often while a mode runs.
pub const RENEW_MS: u32 = 500;
/// The Radio task gives up after this long without a renewal (three renewals missed).
pub const WINDOW_MS: u32 = 1_500;

/// The exclusive mode the Radio task holds, as far as the lease is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Scope,
    Counter,
    Broadcast,
    Hf,
    Crossband,
}

const HELD: [Held; 5] = [Held::Scope, Held::Counter, Held::Broadcast, Held::Hf, Held::Crossband];

impl Held {
    /// The command that stops it, as the UI would send it.
    pub fn stop_cmd(self) -> RadioCmd {
        match self {
            Held::Scope => RadioCmd::Sweep { centre: 0, step: 0, on: false },
            Held::Counter => RadioCmd::Counter(false),
            Held::Broadcast => RadioCmd::Broadcast(BroadcastCmd::Off),
            Held::Hf => RadioCmd::Hf { freq: 0, mode: 0, on: false },
            Held::Crossband => RadioCmd::Crossband(None),
        }
    }
}

/// The modes held, a bit per [`Held`]: home's strip can sweep beside a broadcast another slot plays (MODE-5,
/// decision 115), so two can be held at once; one renewal covers both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lease {
    held: u8,
    at: u32,
}

impl Lease {
    pub const fn new() -> Self {
        Lease { held: 0, at: 0 }
    }

    /// A mode started (or was retuned): held, renewed now.
    pub fn start(&mut self, h: Held, now: u32) {
        (self.held, self.at) = (self.held | 1 << h as u8, now);
    }

    /// The mode stopped (its stop command, or the Radio task's own stop).
    pub fn stop(&mut self, h: Held) {
        self.held &= !(1 << h as u8);
    }

    /// The UI still wants it.
    pub fn renew(&mut self, now: u32) {
        self.at = now;
    }

    /// A command from the UI: a mode's start or stop, else a renewal (any command shows the UI is running,
    /// which is what the lease is about).
    pub fn note(&mut self, c: &RadioCmd, now: u32) {
        match *c {
            RadioCmd::Sweep { on: true, .. } => self.start(Held::Scope, now),
            RadioCmd::Sweep { on: false, .. } => self.stop(Held::Scope),
            RadioCmd::Counter(true) => self.start(Held::Counter, now),
            RadioCmd::Counter(false) => self.stop(Held::Counter),
            RadioCmd::Broadcast(BroadcastCmd::Off) => self.stop(Held::Broadcast),
            RadioCmd::Broadcast(_) => self.start(Held::Broadcast, now),
            RadioCmd::Hf { on: true, .. } => self.start(Held::Hf, now),
            RadioCmd::Hf { on: false, .. } => self.stop(Held::Hf),
            RadioCmd::Crossband(Some(_)) => self.start(Held::Crossband, now),
            RadioCmd::Crossband(None) => self.stop(Held::Crossband),
            // The input task's, not the UI's: no sign the UI is running.
            RadioCmd::Accessory(_) => {}
            _ => self.renew(now),
        }
    }

    /// The first mode held, if any.
    pub fn held(&self) -> Option<Held> {
        HELD.into_iter().find(|&h| self.held & 1 << h as u8 != 0)
    }

    /// A mode to stop now, if the lease ran out; it is no longer held afterwards (a second held mode comes on the
    /// next call).
    pub fn expired(&mut self, now: u32) -> Option<Held> {
        let h = self.held()?;
        if now.wrapping_sub(self.at) < WINDOW_MS {
            return None;
        }
        self.stop(h);
        Some(h)
    }

    /// When the lease runs out (ms), for the Radio task's wait; None when nothing is held.
    pub fn deadline(&self) -> Option<u32> {
        (self.held != 0).then_some(self.at.wrapping_add(WINDOW_MS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MODE-3: a held mode lasts while renewed; without renewal it runs out after the window, once.
    #[test]
    fn runs_out_without_renewal() {
        let mut l = Lease::new();
        assert_eq!(l.expired(10_000), None, "nothing held");
        l.start(Held::Scope, 1_000);
        for t in (1_000..5_000).step_by(RENEW_MS as usize) {
            l.renew(t);
            assert_eq!(l.expired(t + 100), None, "renewed at {t}");
        }
        assert_eq!(l.expired(4_500 + WINDOW_MS - 1), None);
        assert_eq!(l.expired(4_500 + WINDOW_MS), Some(Held::Scope), "ran out");
        assert_eq!(l.expired(9_000), None, "only once");
        assert_eq!(l.held(), None);
    }

    /// MODE-3: a stop ends the lease; stopping another mode leaves it; the deadline follows the renewals.
    #[test]
    fn stop_and_deadline() {
        let mut l = Lease::new();
        l.start(Held::Hf, 100);
        l.stop(Held::Scope);
        assert_eq!(l.held(), Some(Held::Hf));
        assert_eq!(l.deadline(), Some(100 + WINDOW_MS));
        l.renew(700);
        assert_eq!(l.deadline(), Some(700 + WINDOW_MS));
        l.stop(Held::Hf);
        assert_eq!((l.held(), l.deadline(), l.expired(u32::MAX / 2)), (None, None, None));
    }

    /// Commands start, stop and renew; each held mode's stop command ends it again.
    #[test]
    fn commands() {
        let mut l = Lease::new();
        l.note(&RadioCmd::Lease, 50);
        assert_eq!(l.held(), None, "a renewal holds nothing");
        let starts = [
            (RadioCmd::Sweep { centre: 14_652_000, step: 2500, on: true }, Held::Scope),
            (RadioCmd::Counter(true), Held::Counter),
            (RadioCmd::Broadcast(BroadcastCmd::Fm(10_110)), Held::Broadcast),
            (RadioCmd::Hf { freq: 707_500, mode: 2, on: true }, Held::Hf),
        ];
        for (c, h) in starts {
            l.note(&c, 100);
            assert_eq!(l.held(), Some(h));
            l.note(&RadioCmd::Activity, 1_000);
            assert_eq!(l.expired(1_000 + WINDOW_MS - 1), None, "{h:?}: renewed by any command");
            l.note(&h.stop_cmd(), 1_200);
            assert_eq!(l.held(), None, "{h:?}: its stop ends it");
        }
    }

    /// MODE-5 (decision 115): home's strip beside a broadcast holds both; one renewal covers both; stopping one
    /// leaves the other; run out, each is stopped in turn.
    #[test]
    fn two_held() {
        let mut l = Lease::new();
        l.note(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_050)), 100);
        l.note(&RadioCmd::Sweep { centre: 14_652_000, step: 400, on: true }, 200);
        l.note(&RadioCmd::Lease, 600);
        assert_eq!(l.expired(600 + WINDOW_MS - 1), None);
        l.note(&RadioCmd::Sweep { centre: 0, step: 0, on: false }, 700);
        assert_eq!(l.held(), Some(Held::Broadcast), "the strip stopped, the broadcast plays on");
        l.note(&RadioCmd::Sweep { centre: 14_652_000, step: 400, on: true }, 800);
        assert_eq!(l.expired(800 + WINDOW_MS), Some(Held::Scope));
        assert_eq!(l.expired(800 + WINDOW_MS), Some(Held::Broadcast));
        assert_eq!((l.expired(u32::MAX / 2), l.deadline()), (None, None));
    }

    /// The millisecond clock wraps: renewals across the wrap still count.
    #[test]
    fn wraps() {
        let mut l = Lease::new();
        l.start(Held::Counter, u32::MAX - 200);
        l.renew(300);
        assert_eq!(l.expired(800), None);
        assert_eq!(l.expired(300 + WINDOW_MS), Some(Held::Counter));
    }
}
