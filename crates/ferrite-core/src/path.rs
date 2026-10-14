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
//! - while keyed nothing but [`Path::unkey`] and [`Path::safe`] changes a pin (TX-29);
//! - the one exception to "never RX and TX together" is the crossband repeater ([`Path::key_up_xb`], XB-6): the
//!   receiving chip's RX select stays on while the other chip's TX select is on, on different chips, and the relay
//!   never moves for it. Its audio switching follows stock's exactly (decision 95, `analysis/crossband_repeat.md`
//!   §4.2 and §5): the repeat audio (PA15 low) [`XB_AUDIO_DELAY_MS`] after the PA is up, back to the mic at the
//!   receive drop while still keyed, the receive select cleared before the roger and tail, the carrier held
//!   [`XB_HOLD_MS`], then the PA to 0 and the unkey wait before the chip leaves transmit.
//! - a bench test (decision 114, SAFE-24): [`Path::set_both`] holds both band receive selects on (VHF RX PB0 and
//!   UHF RX PE12, the two LNAs), so the watch hop between a VHF and a UHF slot toggles no select. Never the HF
//!   select, never with the relay energised; every key-up clears both before any TX select, the safe state and
//!   HF mode drop it.

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
    /// The crossband repeater's audio route (XB-6, XB-8): the receiving chip's audio into the transmitting chip's
    /// mic input (stock's PA15 low, with the audio stage PB8 on; to confirm on the bench), or back to the mic.
    RepeatAudio(bool),
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
/// Crossband (decision 95): stock selects the repeat audio at its PTT state 1, 10-20 ms after the TX start that
/// ramps the PA (0x080248C2-0x080248D8; its 10 ms pass runs that state every other call). The middle of that.
pub const XB_AUDIO_DELAY_MS: u8 = 15;
/// Crossband (decision 95): stock keeps the carrier up 400 ms after the roger and tail (+0x07 = 40, 0x08024DCC)
/// before the PA goes to 0; tail elimination plays inside it.
pub const XB_HOLD_MS: u16 = 400;
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
    /// [`F_ALARM`], [`F_MUTED`] and [`F_BOTH`] (one byte, so the machine stays 48 bytes).
    flags: u8,
    /// Keyed by [`Path::key_up_xb`]: 0 no, 1 keyed with the mic selected (PA15 high), 2 with the repeat audio.
    xb: u8,
}

/// The guard tripped since the last [`Path::take_alarm`].
const F_ALARM: u8 = 1;
/// The last mute asked for: a receive path selected while muted unmutes, or the safe state's mute at boot stayed
/// on and no receive audio reached the speaker (black radio, Oct 12, 2026).
const F_MUTED: u8 = 2;
/// Both band receive selects held on ([`Path::set_both`], the bench test of decision 114): RAM only, off at boot.
const F_BOTH: u8 = 4;

/// VHF RX or UHF RX: the two selects [`Path::set_both`] holds.
fn vu(s: Sel) -> bool {
    matches!(s, Sel::VhfRx | Sel::UhfRx)
}

/// The other one of the two.
fn other(s: Sel) -> Sel {
    if s == Sel::VhfRx {
        Sel::UhfRx
    } else {
        Sel::VhfRx
    }
}

impl Default for Path {
    fn default() -> Self {
        Self::new()
    }
}

impl Path {
    /// After [`Path::safe`] at start-up: everything low, relay released.
    pub const fn new() -> Self {
        Path { rx: Rx::Off, tx: None, relay: false, restore: None, times: [0; GUARD_SWITCHES], nt: 0, flags: 0, xb: 0 }
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
        let a = self.flags & F_ALARM != 0;
        self.flags &= !F_ALARM;
        a
    }

    fn flag(&mut self, f: u8, on: bool) {
        if on {
            self.flags |= f;
        } else {
            self.flags &= !f;
        }
    }

    /// Both band receive selects held on ([`Path::set_both`]): asked for, whether or not they are on now (keyed,
    /// or nothing receiving, they aren't).
    pub fn both(&self) -> bool {
        self.flags & F_BOTH != 0
    }

    /// The two are on now: held, not keyed (a crossband key-up cleared the other already), and receiving on VHF
    /// RX or UHF RX.
    fn both_live(&self) -> bool {
        self.both() && self.tx.is_none() && self.xb == 0 && matches!(self.rx, Rx::Bk(s) if vu(s))
    }

    /// The bench test of decision 114 (SAFE-24, TERM-29): hold both band receive selects on (VHF RX PB0 and UHF
    /// RX PE12), or back to one. On: the other band's select is set beside the one receiving now (nothing if no
    /// receive path is on; the next one sets both), and from then the receive path between VHF and UHF changes
    /// no pin. Refused while keyed (TX-29) and while the relay is energised, HF receives or the SI4732 has the
    /// input (HF mode): never the HF select, never anything needing the relay. Off: the other band's select
    /// cleared, the one receiving now kept. RAM only, off at boot.
    pub fn set_both(&mut self, on: bool, out: &mut Steps) -> Result<(), Blocked> {
        if self.tx.is_some() {
            return Err(Blocked::Keyed);
        }
        if on == self.both() {
            return Ok(());
        }
        if on && (self.relay || matches!(self.rx, Rx::Si | Rx::Bk(Sel::HfRx))) {
            return Err(Blocked::NeedsRelay);
        }
        if let Rx::Bk(s) = self.rx {
            if vu(s) {
                out.push(Act::Sel(other(s), on));
            }
        }
        self.flag(F_BOTH, on);
        Ok(())
    }

    /// Whether `n` more relay switches fit in the guard's minute; trips the alarm if not.
    fn room(&mut self, n: usize, now: u32) -> bool {
        let used = self.times[..self.nt as usize].iter().filter(|&&t| now.wrapping_sub(t) < GUARD_WINDOW_MS).count();
        if used + n > GUARD_SWITCHES {
            self.flags |= F_ALARM;
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

    /// Clear the receive path (muted first if the relay is next): both band selects while they are held.
    fn clear_rx(&mut self, out: &mut Steps) {
        let both = self.both_live();
        match self.rx {
            Rx::Bk(s) => {
                out.push(Act::Sel(s, false));
                if both {
                    out.push(Act::Sel(other(s), false));
                }
            }
            Rx::Si => out.push(Act::SiInput(false)),
            Rx::Off => {}
        }
        self.rx = Rx::Off;
    }

    /// Set the receive path: both band selects while they are held (never while keyed: callers are unkeyed).
    fn set_rx(&mut self, rx: Rx, out: &mut Steps) {
        match rx {
            Rx::Bk(s) => {
                out.push(Act::Sel(s, true));
                if self.both() && vu(s) {
                    out.push(Act::Sel(other(s), true));
                }
            }
            Rx::Si => out.push(Act::SiInput(true)),
            Rx::Off => {}
        }
        self.rx = rx;
    }

    /// Receive with `rx` (tuning, Battery Save's wake, HF mode's receivers). Moving the relay needs a deliberate
    /// cause and room in the guard: mute, clear RX, relay, settle, the new path, unmute (SAFE-21). With both band
    /// selects held, VHF RX to UHF RX (or back) changes no pin; HF RX or the SI4732 clears both and drops the hold
    /// (HF mode, SAFE-24).
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
                self.mute(true, out);
                self.clear_rx(out);
                self.drop_both(rx);
                self.move_relay(r, now, out);
                self.set_rx(rx, out);
                self.mute(false, out);
            }
            _ => {
                match (self.rx, rx) {
                    (Rx::Bk(a), Rx::Bk(b)) if self.both_live() && vu(a) && vu(b) => self.rx = rx, // both on already
                    _ => {
                        self.clear_rx(out);
                        self.drop_both(rx);
                        self.set_rx(rx, out);
                    }
                }
                if self.flags & F_MUTED != 0 && rx != Rx::Off {
                    self.mute(false, out);
                }
            }
        }
        Ok(())
    }

    /// SAFE-24: a receive path off VHF and UHF (HF RX, the SI4732: HF mode) drops the hold on both selects.
    fn drop_both(&mut self, rx: Rx) {
        if matches!(rx, Rx::Si | Rx::Bk(Sel::HfRx)) {
            self.flag(F_BOTH, false);
        }
    }

    fn mute(&mut self, m: bool, out: &mut Steps) {
        out.push(Act::Mute(m));
        self.flag(F_MUTED, m);
    }

    fn move_relay(&mut self, r: bool, now: u32, out: &mut Steps) {
        out.push(Act::Relay(r));
        out.push(Act::Wait(RELAY_SETTLE_MS));
        self.relay = r;
        self.count(now);
    }

    /// Key-up on `tx` (a TX select; RX ones are taken as their TX twin), TX-23 and TX-24: PA 0, SI4732 away, mute,
    /// RX path clear, the relay if the slot needs the other state (moved back after the unkey), the TX path, the
    /// chip to TX with its read-back, the PA ramp. Both band receive selects held (SAFE-24): both cleared here,
    /// before the TX select; the hold stays asked for and the unkey sets both again.
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
        self.mute(true, out);
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

    /// The crossband repeater's key-up (XB-6): the other chip transmits while the receiving one stays in receive,
    /// its RX select on and unmuted (its audio is what goes out). Only on a different chip from the receive select
    /// and on the relay state as it is: the relay never moves for the repeater (XB-2, SAFE-20). PA 0, the TX
    /// path, the chip to transmit with its read-back, the PA ramp, then stock's 10-20 ms (PTT state 1) and the
    /// repeat audio (decision 95). Stock writes the PA drive before the TX path; Ferrite keeps PA 0 until the
    /// path and the chip are set (TX-23), the one difference kept (a hot switch otherwise). Both band receive
    /// selects held (SAFE-24): the transmitting chip's band select is cleared first, so only the receiving one is
    /// on, as without the hold.
    pub fn key_up_xb(&mut self, tx: Sel, out: &mut Steps) -> Result<(), Blocked> {
        if self.tx.is_some() {
            return Err(Blocked::Keyed);
        }
        let tx = tx.to_tx();
        let Rx::Bk(rx) = self.rx else { return Err(Blocked::NeedsRelay) };
        if rx.chip() == tx.chip() || tx.relay() != self.relay || rx.relay() != self.relay {
            return Err(Blocked::NeedsRelay);
        }
        if self.both_live() {
            out.push(Act::Sel(other(rx), false));
        }
        out.push(Act::PaZero);
        out.push(Act::Sel(tx, true));
        out.push(Act::ChipTx(tx.chip()));
        out.push(Act::PaRamp);
        out.push(Act::Wait(XB_AUDIO_DELAY_MS));
        out.push(Act::RepeatAudio(true));
        (self.tx, self.xb) = (Some(tx), 2);
        Ok(())
    }

    /// Keyed by the crossband repeater.
    pub fn keyed_xb(&self) -> bool {
        self.xb != 0
    }

    /// Crossband (decision 95): the receive drop while still keyed, as stock's RX end (0x0801ED4C; V0.29
    /// 0x0801F534): the mic back (PA15 high). The receive select stays on through a hang time (XB-7), so a
    /// returning signal can resume the pass ([`Path::xb_reopen`]).
    pub fn xb_drop(&mut self, out: &mut Steps) {
        if self.xb == 2 {
            out.push(Act::RepeatAudio(false));
            self.xb = 1;
        }
    }

    /// Crossband: the signal back inside the hang time, the repeat audio again at once.
    pub fn xb_reopen(&mut self, out: &mut Steps) {
        if self.xb == 1 {
            out.push(Act::RepeatAudio(true));
            self.xb = 2;
        }
    }

    /// Crossband (decision 95): the start of the unkey, before the roger and tail: as stock's RX end, the mic
    /// back if not yet and every receive select cleared (`rf_path(RX class, 3)`) while the TX chip is keyed; the
    /// next tick selects the receiving side again once unkeyed, as stock's squelch state machine does.
    pub fn xb_rx_end(&mut self, out: &mut Steps) {
        self.xb_drop(out);
        if self.xb != 0 {
            self.clear_rx(out);
        }
    }

    /// Unkey (TX-26), after any end tone, CW ID and tail elimination the sequencer put first: PA 0, the chip back
    /// to receive, the unkey wait, the TX path clear, the relay back if key-up moved it, the RX path, unmute.
    /// A crossband key-up's (XB-8, decision 95), as stock's PTT state 4 (0x08024DCC on): the mic and the receive
    /// select already handled by [`Path::xb_rx_end`], then PA 0, the unkey wait (V0.29's 100 ms), the chip out of
    /// transmit, the TX path clear.
    pub fn unkey(&mut self, now: u32, out: &mut Steps) {
        let Some(tx) = self.tx.take() else { return };
        if self.xb != 0 {
            self.xb_rx_end(out); // a no-op after the sequencer's own call
            self.xb = 0;
            out.push(Act::PaZero);
            out.push(Act::Wait(UNKEY_WAIT_MS));
            out.push(Act::ChipRx(tx.chip()));
            out.push(Act::Sel(tx, false));
            return;
        }
        out.push(Act::PaZero);
        out.push(Act::ChipRx(tx.chip()));
        out.push(Act::Wait(UNKEY_WAIT_MS));
        out.push(Act::Sel(tx, false));
        if let Some(r) = self.restore.take() {
            self.move_relay(r, now, out); // counted, never refused: room was kept at key-up
        }
        let rx = core::mem::replace(&mut self.rx, Rx::Off);
        self.set_rx(rx, out);
        self.mute(false, out);
    }

    /// Battery Save's sleep phase: a BK4829's receive select off (refused while keyed, TX-29), as stock's
    /// `rf_path(band, 0)`. Only that: no mute, and the SI4732's input is left as it is, so broadcast FM plays on
    /// through the BK4829s' sleep (BCR-19). While a `broadcast` plays the select stays on too (BCR-19, decision
    /// 113): switching the band's LNA supply clicks in the shared speaker amp (bench, Oct 13, 2026), and the
    /// BK4829s' own sleep is silent. Both band selects held (SAFE-24) stay on as well.
    pub fn rx_off(&mut self, broadcast: bool, out: &mut Steps) -> Result<(), Blocked> {
        if self.tx.is_some() {
            return Err(Blocked::Keyed);
        }
        if matches!(self.rx, Rx::Bk(_)) && !broadcast && !self.both() {
            self.clear_rx(out);
        }
        Ok(())
    }

    /// The safe state, from anything (a fault, power off, a refused read-back): PA 0, both chips to receive,
    /// the unkey wait if keyed, every select and the SI4732 input clear, the relay released, muted, the hold on
    /// both band selects dropped (SAFE-24). Always allowed, guard or not.
    pub fn safe(&mut self, now: u32, out: &mut Steps) {
        out.push(Act::PaZero);
        self.mute(true, out);
        if let Some(tx) = self.tx.take() {
            out.push(Act::ChipRx(tx.chip()));
            out.push(Act::Wait(UNKEY_WAIT_MS));
        }
        if core::mem::take(&mut self.xb) == 2 {
            out.push(Act::RepeatAudio(false));
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
        self.flag(F_BOTH, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SAFE-11: the safe state at boot mutes; the first receive path selected (no relay move) unmutes, and later
    /// selects while unmuted add no mute steps.
    #[test]
    fn boot_safe_then_receive_unmutes() {
        let mut p = Path::new();
        let mut s = Steps::default();
        p.safe(0, &mut s);
        assert!(s.as_slice().contains(&Act::Mute(true)));
        let mut s = Steps::default();
        p.rx(Rx::Bk(Sel::VhfRx), Cause::ActiveSlot, 1000, &mut s).unwrap();
        assert_eq!(s.as_slice().last(), Some(&Act::Mute(false)), "receive selected: unmuted");
        let mut s = Steps::default();
        p.rx(Rx::Bk(Sel::UhfRx), Cause::Auto, 2000, &mut s).unwrap();
        assert!(!s.as_slice().iter().any(|a| matches!(a, Act::Mute(_))), "already unmuted: {:?}", s.as_slice());
    }
    use crate::freq::mhz;

    fn plan(f: impl FnOnce(&mut Path, &mut Steps)) -> (Path, Steps) {
        let mut p = Path::new();
        let mut s = Steps::default();
        f(&mut p, &mut s);
        (p, s)
    }

    /// PWR-6, BCR-19, HF-33: Battery Save's sleep step clears only a BK4829's receive select: no mute (the speaker
    /// stays as the broadcast has it) and never the SI4732's input; with the SI4732 on the path it does nothing.
    #[test]
    fn sleep_step_leaves_the_si4732_alone() {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::VhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        p.rx_off(false, &mut s).unwrap();
        assert_eq!(s.as_slice(), [Act::Sel(Sel::VhfRx, false)]);
        let (mut p, _) = plan(|p, s| p.rx(Rx::Si, Cause::HfReceiver, 0, s).unwrap());
        let mut s = Steps::default();
        p.rx_off(false, &mut s).unwrap();
        assert!(s.is_empty(), "{:?}", s.as_slice());
        assert_eq!(p.rx_now(), Rx::Si);
    }

    /// BCR-19, PWR-6 (decision 113): during a broadcast Battery Save's sleep leaves the band's receive select on
    /// (its switching clicks in the shared amp); the wake's select then changes nothing.
    #[test]
    fn sleep_during_broadcast_keeps_the_select() {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::UhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        p.rx_off(true, &mut s).unwrap();
        assert!(s.is_empty(), "{:?}", s.as_slice());
        assert_eq!(p.rx_now(), Rx::Bk(Sel::UhfRx));
        p.rx(Rx::Bk(Sel::UhfRx), Cause::Auto, 0, &mut s).unwrap();
        assert!(s.is_empty(), "the wake toggles nothing: {:?}", s.as_slice());
        assert_eq!(p.rx_off(true, &mut s), Ok(()));
    }

    fn both_on() -> Path {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::VhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        p.set_both(true, &mut s).unwrap();
        assert_eq!(s.as_slice(), [Act::Sel(Sel::UhfRx, true)]);
        assert!(p.both());
        p
    }

    /// SAFE-24, TERM-29: both band selects held: set together, a VHF-UHF hop toggles nothing, Battery Save's sleep
    /// leaves them, off clears only the other one; never the relay.
    #[test]
    fn both_held_hops_toggle_nothing() {
        let mut p = both_on();
        for rx in [Sel::UhfRx, Sel::VhfRx, Sel::UhfRx] {
            let mut s = Steps::default();
            p.rx(Rx::Bk(rx), Cause::Auto, 0, &mut s).unwrap();
            assert!(s.is_empty(), "{:?}", s.as_slice());
            assert_eq!(p.rx_now(), Rx::Bk(rx));
        }
        let mut s = Steps::default();
        p.rx_off(false, &mut s).unwrap();
        assert!(s.is_empty(), "held through the sleep: {:?}", s.as_slice());
        p.set_both(true, &mut s).unwrap();
        assert!(s.is_empty(), "already on");
        p.set_both(false, &mut s).unwrap();
        assert_eq!(s.as_slice(), [Act::Sel(Sel::VhfRx, false)], "the receiving one (UHF) stays");
        assert!(!p.both() && !p.relay());
        // Asked for with nothing receiving: the next receive path sets both.
        let mut p = Path::new();
        let mut s = Steps::default();
        p.set_both(true, &mut s).unwrap();
        assert!(s.is_empty());
        p.rx(Rx::Bk(Sel::UhfRx), Cause::ActiveSlot, 0, &mut s).unwrap();
        assert_eq!(s.as_slice(), [Act::Sel(Sel::UhfRx, true), Act::Sel(Sel::VhfRx, true)]);
    }

    /// SAFE-24: every key-up clears both before any TX select; the unkey sets both again (the hold stays asked
    /// for); crossband clears the transmitting chip's band select first and keeps the receiving one.
    #[test]
    fn both_held_cleared_before_key_up() {
        for tx in [Sel::VhfTx, Sel::UhfTx, Sel::HfTx] {
            let mut p = both_on();
            let mut s = Steps::default();
            p.key_up(tx, 0, &mut s).unwrap();
            let a = s.as_slice();
            let make = a.iter().position(|x| *x == Act::Sel(tx, true)).unwrap();
            assert!(a[..make].contains(&Act::Sel(Sel::VhfRx, false)) && a[..make].contains(&Act::Sel(Sel::UhfRx, false)), "{tx:?}: {a:?}");
            assert!(!a.iter().any(|x| matches!(x, Act::Sel(r, true) if !r.is_tx())), "{tx:?}: no RX select while keyed");
            assert_eq!(p.set_both(false, &mut Steps::default()), Err(Blocked::Keyed));
            let mut s = Steps::default();
            p.unkey(0, &mut s);
            let a = s.as_slice();
            let brk = a.iter().position(|x| *x == Act::Sel(tx, false)).unwrap();
            assert!(a[brk..].contains(&Act::Sel(Sel::VhfRx, true)) && a[brk..].contains(&Act::Sel(Sel::UhfRx, true)), "{tx:?}: {a:?}");
            assert!(p.both() && !p.relay());
        }
        let mut p = both_on();
        p.rx(Rx::Bk(Sel::UhfRx), Cause::Auto, 0, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        p.key_up_xb(Sel::VhfTx, &mut s).unwrap();
        assert_eq!(s.as_slice()[..2], [Act::Sel(Sel::VhfRx, false), Act::PaZero]);
        assert!(!s.as_slice().contains(&Act::Sel(Sel::UhfRx, false)), "the receiving select stays (XB-6)");
        let mut s = Steps::default();
        p.unkey(0, &mut s);
        assert_eq!(s.as_slice().iter().filter(|x| **x == Act::Sel(Sel::VhfRx, false)).count(), 0, "cleared once only");
        let mut s = Steps::default();
        p.rx(Rx::Bk(Sel::UhfRx), Cause::Auto, 0, &mut s).unwrap();
        assert_eq!(s.as_slice(), [Act::Sel(Sel::UhfRx, true), Act::Sel(Sel::VhfRx, true)], "the next tick: both again");
    }

    /// SAFE-24: the safe state and HF mode drop the hold; HF and the relay refuse it.
    #[test]
    fn both_held_dropped_by_safe_and_hf() {
        let mut p = both_on();
        let mut s = Steps::default();
        p.safe(0, &mut s);
        assert!(!p.both());
        assert!(s.as_slice().contains(&Act::Sel(Sel::VhfRx, false)) && s.as_slice().contains(&Act::Sel(Sel::UhfRx, false)));
        let mut p = both_on();
        let mut s = Steps::default();
        p.rx(Rx::Si, Cause::HfReceiver, 0, &mut s).unwrap();
        assert_eq!(s.as_slice()[..3], [Act::Sel(Sel::VhfRx, false), Act::Sel(Sel::UhfRx, false), Act::SiInput(true)]);
        assert!(!p.both());
        assert_eq!(p.set_both(true, &mut s), Err(Blocked::NeedsRelay), "HF mode refuses it");
        let mut p = both_on();
        let mut s = Steps::default();
        p.rx(Rx::Bk(Sel::HfRx), Cause::ActiveSlot, 0, &mut s).unwrap();
        assert_eq!(s.as_slice()[..3], [Act::Mute(true), Act::Sel(Sel::VhfRx, false), Act::Sel(Sel::UhfRx, false)]);
        assert!(!p.both() && !s.as_slice().contains(&Act::Sel(Sel::VhfRx, true)));
        let mut s = Steps::default();
        assert_eq!(p.set_both(true, &mut s), Err(Blocked::NeedsRelay), "never with the relay energised");
        assert!(s.is_empty());
        // Only the deliberate relay move was the HF slot's own: the hold never moves it.
        let mut p = both_on();
        let mut s = Steps::default();
        p.set_both(false, &mut s).unwrap();
        p.set_both(true, &mut s).unwrap();
        assert!(!s.as_slice().iter().any(|a| matches!(a, Act::Relay(_) | Act::Sel(Sel::HfRx | Sel::HfTx, _))));
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
        assert_eq!(p.rx_off(false, &mut s), Err(Blocked::Keyed));
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

    #[test]
    fn crossband_keeps_the_receiver() {
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::UhfRx), Cause::ActiveSlot, 0, s).unwrap());
        let mut s = Steps::default();
        assert_eq!(p.key_up_xb(Sel::UhfTx, &mut s), Err(Blocked::NeedsRelay), "never the receiving chip");
        assert_eq!(p.key_up_xb(Sel::HfTx, &mut s), Err(Blocked::NeedsRelay), "never the relay");
        p.key_up_xb(Sel::VhfTx, &mut s).unwrap();
        // Decision 95, stock's order: the PA up, then 10-20 ms (PTT state 1), then the repeat audio.
        assert_eq!(
            s.as_slice(),
            [Act::PaZero, Act::Sel(Sel::VhfTx, true), Act::ChipTx(0), Act::PaRamp, Act::Wait(XB_AUDIO_DELAY_MS), Act::RepeatAudio(true)]
        );
        assert_eq!(p.rx_now(), Rx::Bk(Sel::UhfRx));
        // The receive drop while keyed: the mic back (PA15 high) with the PA up; a return resumes the audio.
        let mut s = Steps::default();
        p.xb_drop(&mut s);
        p.xb_drop(&mut s);
        assert_eq!(s.as_slice(), [Act::RepeatAudio(false)]);
        let mut s = Steps::default();
        p.xb_reopen(&mut s);
        assert_eq!(s.as_slice(), [Act::RepeatAudio(true)]);
        // The unkey: stock's RX end (mic, receive select off) while keyed, then PA 0, the wait, the chip, the path.
        let mut s = Steps::default();
        p.xb_rx_end(&mut s);
        assert_eq!(s.as_slice(), [Act::RepeatAudio(false), Act::Sel(Sel::UhfRx, false)]);
        assert!(p.keyed().is_some() && p.rx_now() == Rx::Off);
        let mut s = Steps::default();
        p.unkey(0, &mut s);
        assert_eq!(s.as_slice(), [Act::PaZero, Act::Wait(100), Act::ChipRx(0), Act::Sel(Sel::VhfTx, false)]);
        assert!(p.keyed().is_none() && !p.keyed_xb());
        // Without the sequencer's call first, the unkey does the RX end itself.
        let (mut p, _) = plan(|p, s| p.rx(Rx::Bk(Sel::UhfRx), Cause::ActiveSlot, 0, s).unwrap());
        p.key_up_xb(Sel::VhfTx, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        p.unkey(0, &mut s);
        assert_eq!(s.as_slice()[..2], [Act::RepeatAudio(false), Act::Sel(Sel::UhfRx, false)]);
        // The safe state from a crossband pass puts the mic back too.
        p.rx(Rx::Bk(Sel::UhfRx), Cause::Auto, 0, &mut Steps::default()).unwrap();
        p.key_up_xb(Sel::VhfTx, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        p.safe(0, &mut s);
        assert!(s.as_slice().contains(&Act::RepeatAudio(false)));
    }
}
