//! The RF path state machine (SAFE-11): the one owner of the six path selects, the PA drive, the band relay
//! (PC4) and the SI4732's input (PA7/PC5). It never touches a pin: each change comes back as [`Steps`], pin
//! actions and waits in order, for the Radio task to run to the end before it asks for the next one (so the
//! unkey wait is inside a plan, never between two).
//!
//! The rules it holds:
//! - break before make: a select is cleared before another is set; an RX and a TX select are never on
//!   together (TX-27);
//! - the relay moves only for a deliberate [`Cause`] (SAFE-20), never with RF present (SAFE-21, TX-28): the
//!   receiver is muted and its select cleared first, then [`RELAY_SETTLE_MS`] before the new path and unmute;
//! - more than [`GUARD_SWITCHES`] relay switches in a minute holds the relay where it is (SAFE-22) until the
//!   minute has passed, except for [`Path::safe`];
//! - while keyed nothing but [`Path::unkey`] and [`Path::safe`] changes a pin (TX-29).

use crate::freq::{band, Band, Freq10};

/// The six path selects, in `RF_PINS` order (the PA drive is the seventh pin, [`Act::PaZero`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Sel {
    VhfRx,
    VhfTx,
    UhfRx,
    UhfTx,
    HfRx,
    HfTx,
}

impl Sel {
    pub const ALL: [Sel; 6] = [Sel::VhfRx, Sel::VhfTx, Sel::UhfRx, Sel::UhfTx, Sel::HfRx, Sel::HfTx];

    /// The receive select for a frequency. The 200-300 MHz band's own path is not known yet: it uses the VHF
    /// front end, as chip 0 does.
    pub fn rx(f: Freq10) -> Sel {
        match band(f).0 {
            Band::Hf => Sel::HfRx,
            Band::Vhf | Band::B200 => Sel::VhfRx,
            Band::B300 | Band::Uhf => Sel::UhfRx,
        }
    }

    /// The transmit select for a frequency.
    pub fn tx(f: Freq10) -> Sel {
        Sel::rx(f).to_tx()
    }

    fn to_tx(self) -> Sel {
        Sel::ALL[self as usize | 1]
    }

    pub fn is_tx(self) -> bool {
        self as u8 & 1 != 0
    }

    /// The BK4829 behind it: 0 VHF, 1 UHF and HF.
    pub fn chip(self) -> u8 {
        (self as u8 >= 2) as u8
    }

    /// The relay state it needs: energised for the HF path (WATCH-10).
    pub fn relay(self) -> bool {
        self as u8 >= 4
    }
}

/// One step of a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Set or clear one path select.
    Sel(Sel, bool),
    /// PA drive (PA5, DAC2) to 0 V.
    PaZero,
    /// PA drive up to the calibrated level for the TX frequency and power (TX-23's ramp; the firmware's).
    PaRamp,
    /// The band relay: energised (HF path) or released.
    Relay(bool),
    /// The SI4732's antenna input (PA7/PC5): to it, or away. Which pin states mean which is a bench question.
    SiInput(bool),
    /// Mute (or unmute) the receiver's audio: the chip's AF output and the speaker amplifier.
    Mute(bool),
    /// A BK4829 to transmit, then its read-back check (TX-7).
    ChipTx(u8),
    /// A BK4829 back to receive.
    ChipRx(u8),
    /// While keyed: the end tone (TONE-8), the CW ID (VOX-12), tail elimination (TONE-9).
    EndTone,
    CwId,
    TailElim,
    /// Wait this many ms before the next step.
    Wait(u8),
}

/// Relay settle after it moves (SAFE-21: G6K operate and release 3 ms; 10 ms given).
pub const RELAY_SETTLE_MS: u8 = 10;
/// The unkey wait between PA drive 0 and the paths going back to receive (TX-26: at least 100 ms).
pub const UNKEY_WAIT_MS: u8 = 100;
/// SAFE-22: more relay switches than this in [`GUARD_WINDOW_MS`] is a bug.
pub const GUARD_SWITCHES: usize = 10;
pub const GUARD_WINDOW_MS: u32 = 60_000;

/// A plan: up to 16 steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Steps {
    a: [Act; 16],
    n: u8,
}

impl Default for Steps {
    fn default() -> Self {
        Steps { a: [Act::Wait(0); 16], n: 0 }
    }
}

impl Steps {
    pub fn push(&mut self, a: Act) {
        // Every plan here is at most 15 steps; a full list keeps its last step (the tests check the lengths).
        let i = (self.n as usize).min(15);
        self.a[i] = a;
        self.n = (i + 1) as u8;
    }

    pub fn as_slice(&self) -> &[Act] {
        &self.a[..self.n as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

/// Why the relay (or a path) may move. Only the first three are deliberate (SAFE-20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// Tuning or switching the active slot.
    ActiveSlot,
    /// HF mode changing receiver (HF-11).
    HfReceiver,
    /// PTT on a slot that needs the other relay state (WATCH-12).
    Ptt,
    /// Watch, scan, jobs, sweeps, link and the rest: they never move the relay.
    Auto,
}

/// Why a path change was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Keyed: only unkey and the safe state change pins (TX-28, TX-29).
    Keyed,
    /// It needs the relay and the cause isn't deliberate (SAFE-20): refuse or cancel, saying why.
    NeedsRelay,
    /// The guard holds the relay (SAFE-22): "Relay switching too often".
    TooOften,
}

/// What receives: a BK4829 select, or the SI4732 (relay released, HF-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rx {
    Bk(Sel),
    Si,
    /// Nothing (Battery Save's sleep phase, or before start-up).
    Off,
}

impl Rx {
    fn relay(self) -> Option<bool> {
        match self {
            Rx::Bk(s) => Some(s.relay()),
            Rx::Si => Some(false),
            Rx::Off => None,
        }
    }
}

/// The path machine (48 bytes).
#[derive(Clone, Copy, Debug)]
pub struct Path {
    rx: Rx,
    /// The TX select while keyed.
    tx: Option<Sel>,
    relay: bool,
    /// Relay state to put back after the unkey (moved at key-up, WATCH-12).
    restore: Option<bool>,
    /// The guard's last relay switch times (ms), oldest first once full.
    times: [u32; GUARD_SWITCHES],
    nt: u8,
    /// The guard tripped since the last [`Path::take_alarm`].
    alarm: bool,
}

impl Default for Path {
    fn default() -> Self {
        Self::new()
    }
}

impl Path {
    /// After [`Path::safe`] at start-up: everything low, relay released.
    pub const fn new() -> Self {
        Path { rx: Rx::Off, tx: None, relay: false, restore: None, times: [0; GUARD_SWITCHES], nt: 0, alarm: false }
    }

    pub fn rx_now(&self) -> Rx {
        self.rx
    }

    pub fn keyed(&self) -> Option<Sel> {
        self.tx
    }

    pub fn relay(&self) -> bool {
        self.relay
    }

    /// The guard tripped (once per trip): the event log's "Relay switching too often".
    pub fn take_alarm(&mut self) -> bool {
        core::mem::take(&mut self.alarm)
    }

    /// Whether `n` more relay switches fit in the guard's minute; trips the alarm if not.
    fn room(&mut self, n: usize, now: u32) -> bool {
        let used = self.times[..self.nt as usize].iter().filter(|&&t| now.wrapping_sub(t) < GUARD_WINDOW_MS).count();
        if used + n > GUARD_SWITCHES {
            self.alarm = true;
            return false;
        }
        true
    }

    fn count(&mut self, now: u32) {
        if self.nt as usize == GUARD_SWITCHES {
            self.times.copy_within(1.., 0);
            self.times[GUARD_SWITCHES - 1] = now;
        } else {
            self.times[self.nt as usize] = now;
            self.nt += 1;
        }
    }

    /// Clear the receive path (muted first if the relay is next).
    fn clear_rx(&mut self, out: &mut Steps) {
        match self.rx {
            Rx::Bk(s) => out.push(Act::Sel(s, false)),
            Rx::Si => out.push(Act::SiInput(false)),
            Rx::Off => {}
        }
        self.rx = Rx::Off;
    }

    fn set_rx(&mut self, rx: Rx, out: &mut Steps) {
        match rx {
            Rx::Bk(s) => out.push(Act::Sel(s, true)),
            Rx::Si => out.push(Act::SiInput(true)),
            Rx::Off => {}
        }
        self.rx = rx;
    }

    /// Receive with `rx` (tuning, Battery Save's wake, HF mode's receivers). Moving the relay needs a deliberate
    /// cause and room in the guard: mute, clear RX, relay, settle, the new path, unmute (SAFE-21).
    pub fn rx(&mut self, rx: Rx, cause: Cause, now: u32, out: &mut Steps) -> Result<(), Blocked> {
        if self.tx.is_some() {
            return Err(Blocked::Keyed);
        }
        let rx = match rx {
            Rx::Bk(s) if s.is_tx() => Rx::Bk(Sel::ALL[s as usize & !1]), // never a TX select from here
            r => r,
        };
        if rx == self.rx {
            return Ok(());
        }
        match rx.relay() {
            Some(r) if r != self.relay => {
                if cause == Cause::Auto {
                    return Err(Blocked::NeedsRelay);
                }
                if !self.room(1, now) {
                    return Err(Blocked::TooOften);
                }
                out.push(Act::Mute(true));
                self.clear_rx(out);
                self.move_relay(r, now, out);
                self.set_rx(rx, out);
                out.push(Act::Mute(false));
            }
            _ => {
                self.clear_rx(out);
                self.set_rx(rx, out);
            }
        }
        Ok(())
    }

    fn move_relay(&mut self, r: bool, now: u32, out: &mut Steps) {
        out.push(Act::Relay(r));
        out.push(Act::Wait(RELAY_SETTLE_MS));
        self.relay = r;
        self.count(now);
    }

    /// Key-up on `tx` (a TX select; RX ones are taken as their TX twin), TX-23 and TX-24: PA 0, SI4732 away, mute,
    /// RX path clear, the relay if the slot needs the other state (moved back after the unkey), the TX path, the
    /// chip to TX with its read-back, the PA ramp.
    pub fn key_up(&mut self, tx: Sel, now: u32, out: &mut Steps) -> Result<(), Blocked> {
        if self.tx.is_some() {
            return Err(Blocked::Keyed);
        }
        let tx = tx.to_tx();
        let moves = tx.relay() != self.relay;
        if moves && !self.room(2, now) {
            return Err(Blocked::TooOften); // room for the move and the one back after
        }
        out.push(Act::PaZero);
        out.push(Act::Mute(true));
        let back = self.rx;
        self.clear_rx(out);
        self.rx = back; // what the unkey goes back to
        if moves {
            self.restore = Some(self.relay);
            self.move_relay(tx.relay(), now, out);
        }
        out.push(Act::Sel(tx, true));
        out.push(Act::ChipTx(tx.chip()));
        out.push(Act::PaRamp);
        self.tx = Some(tx);
        Ok(())
    }

    /// Unkey (TX-26), after any end tone, CW ID and tail elimination the sequencer put first: PA 0, the chip back
    /// to receive, the unkey wait, the TX path clear, the relay back if key-up moved it, the RX path, unmute.
    pub fn unkey(&mut self, now: u32, out: &mut Steps) {
        let Some(tx) = self.tx.take() else { return };
        out.push(Act::PaZero);
        out.push(Act::ChipRx(tx.chip()));
        out.push(Act::Wait(UNKEY_WAIT_MS));
        out.push(Act::Sel(tx, false));
        if let Some(r) = self.restore.take() {
            self.move_relay(r, now, out); // counted, never refused: room was kept at key-up
        }
        let rx = core::mem::replace(&mut self.rx, Rx::Off);
        self.set_rx(rx, out);
        out.push(Act::Mute(false));
    }

    /// Battery Save's sleep phase: the receive path off (refused while keyed, TX-29).
    pub fn rx_off(&mut self, out: &mut Steps) -> Result<(), Blocked> {
        if self.tx.is_some() {
            return Err(Blocked::Keyed);
        }
        self.clear_rx(out);
        Ok(())
    }

    /// The safe state, from anything (a fault, power off, a refused read-back): PA 0, both chips to receive,
    /// the unkey wait if keyed, every select and the SI4732 input clear, the relay released, muted. Always
    /// allowed, guard or not.
    pub fn safe(&mut self, now: u32, out: &mut Steps) {
        out.push(Act::PaZero);
        out.push(Act::Mute(true));
        if let Some(tx) = self.tx.take() {
            out.push(Act::ChipRx(tx.chip()));
            out.push(Act::Wait(UNKEY_WAIT_MS));
        }
        for s in Sel::ALL {
            out.push(Act::Sel(s, false));
        }
        out.push(Act::SiInput(false));
        out.push(Act::Relay(false));
        if self.relay {
            out.push(Act::Wait(RELAY_SETTLE_MS));
            self.count(now);
        }
        (self.rx, self.relay, self.restore) = (Rx::Off, false, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;

    fn plan(f: impl FnOnce(&mut Path, &mut Steps)) -> (Path, Steps) {
        let mut p = Path::new();
        let mut s = Steps::default();
        f(&mut p, &mut s);
        (p, s)
    }

    #[test]
    fn sel_from_freq() {
        assert_eq!(Sel::rx(mhz(146)), Sel::VhfRx);
        assert_eq!(Sel::rx(mhz(223)), Sel::VhfRx);
        assert_eq!(Sel::tx(mhz(446)), Sel::UhfTx);
        assert_eq!(Sel::rx(mhz(28)), Sel::HfRx);
        assert_eq!((Sel::VhfTx.chip(), Sel::UhfRx.chip(), Sel::HfTx.chip()), (0, 1, 1));
        assert!(Sel::HfRx.relay() && !Sel::UhfTx.relay());
    }

    #[test]
    fn same_relay_is_break_before_make() {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::VhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        p.rx(Rx::Bk(Sel::UhfRx), Cause::Auto, 0, &mut s).unwrap();
        assert_eq!(s.as_slice(), [Act::Sel(Sel::VhfRx, false), Act::Sel(Sel::UhfRx, true)]);
        let mut s = Steps::default();
        p.rx(Rx::Bk(Sel::UhfRx), Cause::Auto, 0, &mut s).unwrap();
        assert!(s.is_empty(), "already there");
    }

    #[test]
    fn relay_order_and_causes() {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::VhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        assert_eq!(p.rx(Rx::Bk(Sel::HfRx), Cause::Auto, 0, &mut s), Err(Blocked::NeedsRelay));
        assert!(s.is_empty());
        p.rx(Rx::Bk(Sel::HfRx), Cause::ActiveSlot, 0, &mut s).unwrap();
        assert_eq!(
            s.as_slice(),
            [Act::Mute(true), Act::Sel(Sel::VhfRx, false), Act::Relay(true), Act::Wait(10), Act::Sel(Sel::HfRx, true), Act::Mute(false)]
        );
        // HF mode: chip 1 to the SI4732 releases the relay (HF-11).
        let mut s = Steps::default();
        p.rx(Rx::Si, Cause::HfReceiver, 5, &mut s).unwrap();
        assert_eq!(s.as_slice()[2..4], [Act::Relay(false), Act::Wait(10)]);
        assert_eq!(s.as_slice()[4], Act::SiInput(true));
    }

    #[test]
    fn guard_holds_the_relay() {
        let mut p = Path::new();
        let mut t = 0;
        for i in 0..GUARD_SWITCHES {
            let sel = if i % 2 == 0 { Sel::HfRx } else { Sel::VhfRx };
            p.rx(Rx::Bk(sel), Cause::ActiveSlot, t, &mut Steps::default()).unwrap();
            t += 1000;
        }
        assert!(!p.take_alarm());
        let mut s = Steps::default();
        assert_eq!(p.rx(Rx::Bk(Sel::HfRx), Cause::ActiveSlot, t, &mut s), Err(Blocked::TooOften));
        assert!(s.is_empty() && !p.relay(), "held where it is");
        assert!(p.take_alarm() && !p.take_alarm());
        // A path on the same relay state is still fine.
        p.rx(Rx::Bk(Sel::UhfRx), Cause::ActiveSlot, t, &mut Steps::default()).unwrap();
        // The safe state always goes.
        p.safe(t, &mut Steps::default());
        // A minute after the first switch there is room again.
        p.rx(Rx::Bk(Sel::HfRx), Cause::ActiveSlot, GUARD_WINDOW_MS + 1, &mut Steps::default()).unwrap();
        assert!(p.relay());
    }

    #[test]
    fn key_up_and_unkey_orders() {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::VhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        p.key_up(Sel::VhfTx, 0, &mut s).unwrap();
        assert_eq!(s.as_slice(), [Act::PaZero, Act::Mute(true), Act::Sel(Sel::VhfRx, false), Act::Sel(Sel::VhfTx, true), Act::ChipTx(0), Act::PaRamp]);
        let mut s = Steps::default();
        assert_eq!(p.rx(Rx::Bk(Sel::UhfRx), Cause::ActiveSlot, 0, &mut s), Err(Blocked::Keyed));
        assert_eq!(p.rx_off(&mut s), Err(Blocked::Keyed));
        assert_eq!(p.key_up(Sel::VhfTx, 0, &mut s), Err(Blocked::Keyed));
        p.unkey(0, &mut s);
        assert_eq!(
            s.as_slice(),
            [Act::PaZero, Act::ChipRx(0), Act::Wait(100), Act::Sel(Sel::VhfTx, false), Act::Sel(Sel::VhfRx, true), Act::Mute(false)]
        );
        assert_eq!(p.rx_now(), Rx::Bk(Sel::VhfRx));
    }

    #[test]
    fn ptt_on_the_other_relay_state_moves_it_and_back() {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::UhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        p.key_up(Sel::HfTx, 0, &mut s).unwrap();
        let a = s.as_slice();
        let relay = a.iter().position(|x| *x == Act::Relay(true)).unwrap();
        assert_eq!(a[relay + 1], Act::Wait(10));
        assert!(a[..relay].contains(&Act::Sel(Sel::UhfRx, false)) && a[relay..].contains(&Act::Sel(Sel::HfTx, true)));
        let mut s = Steps::default();
        p.unkey(0, &mut s);
        let a = s.as_slice();
        let wait = a.iter().position(|x| *x == Act::Wait(100)).unwrap();
        let relay = a.iter().position(|x| *x == Act::Relay(false)).unwrap();
        assert!(relay > wait && a[..relay].contains(&Act::Sel(Sel::HfTx, false)), "after the unkey wait, TX path clear");
        assert_eq!(a[a.len() - 2], Act::Sel(Sel::UhfRx, true));
        assert!(!p.relay());
    }

    #[test]
    fn safe_from_keyed() {
        let (mut p, _) = plan(|p, s| p.key_up(Sel::UhfTx, 0, s).unwrap());
        let mut s = Steps::default();
        p.safe(0, &mut s);
        let a = s.as_slice();
        assert_eq!(a[0], Act::PaZero);
        assert_eq!(a[2..4], [Act::ChipRx(1), Act::Wait(100)]);
        assert_eq!(a[a.len() - 2..], [Act::SiInput(false), Act::Relay(false)]);
        assert!(p.keyed().is_none() && p.rx_now() == Rx::Off);
    }
}
