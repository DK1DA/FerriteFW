//! Remote PTT over the link (manual chapter 29, LINK-20; Appendix G's `KEY`). A remote key-up is a PTT press: it
//! goes through [`App::key_down`] as VOX does (VOX-13), so every check a person's PTT meets is met here too (the
//! region and band plan, calibration, busy lockout, the receive-only channel, the build), and the Radio task checks
//! again when it keys. On top of what VOX already skips:
//!
//! - **Never an emergency frequency** (TX-35): refused with a toast, never the second-PTT prompt, and a remote
//!   press never answers a prompt a person opened ([`crate::screens::transmit::Emergency`]).
//! - **No keepalive, no transmit:** none for [`key::KEEPALIVE_MS`] and the radio unkeys.
//! - **The radio's own operator wins:** a local PTT pressed during a remote transmission ends it and does nothing
//!   else until it is let go (pressing again then transmits as usual); a remote key-up is refused while a local PTT
//!   is held. Ending it, rather than handing the transmission over, is the safest choice: a press is how a person
//!   on the radio stops a transmission they didn't start, and a transmission never changes hands while keyed.
//! - **It ends with its session:** `KEY` off, the session moving port or closing, and every stop of the
//!   transmission itself (the timeout, the Radio task's refusals and the sequencer's stops) end it; the PC hears why
//!   at its next keepalive.

use crate::chan::ChannelStore;
use crate::screens::transmit::{self, short};
use crate::{App, Screen};
use ferrite_core::keys::{Key, PTT_A, PTT_ACTIVE, PTT_B};
use ferrite_core::link::key;
use ferrite_core::msg::SLOTS;
use ferrite_core::txplan::{self, Refusal};

/// A remote transmission holding the key: the PTT it pressed (`Key::Ptt` code), the slot keyed, the last keepalive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remote {
    pub ptt: u8,
    pub slot: u8,
    pub last: u32,
}

/// Why a remote transmission ended, told to the PC at its next keepalive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteEnd {
    /// `KEY` off.
    Off,
    /// No keepalive for 2 s.
    Keepalive,
    /// The session closed or moved to the other port (LINK-28).
    Session,
    /// A person pressed PTT on the radio.
    LocalPtt,
    /// The transmit timeout (TX-30).
    Timeout,
    /// The Radio task refused or stopped it (calibration, battery under load, the duty limit).
    Radio(Refusal),
    /// It ended on the radio some other way.
    Stopped,
}

impl RemoteEnd {
    /// For the PC: `invalid` and this.
    pub fn text(self) -> &'static str {
        match self {
            RemoteEnd::Off => "not keyed",
            RemoteEnd::Keepalive => "ended: no keepalive for 2 s",
            RemoteEnd::Session => "ended: the link session closed",
            RemoteEnd::LocalPtt => "ended: PTT pressed on the radio",
            RemoteEnd::Timeout => "ended: transmit timeout",
            RemoteEnd::Radio(r) => short(r),
            RemoteEnd::Stopped => "ended on the radio",
        }
    }
}

/// A local PTT's bit in [`App::local_ptt`] and [`App::ptt_cut`]: A, B, the side connector.
pub(crate) fn bit(k: u8) -> u8 {
    match k {
        PTT_A => 1,
        PTT_B => 2,
        _ => 4,
    }
}

impl App {
    /// A remote transmission holds the key (LINK-20): the Transmit screen says "Remote".
    pub fn remote_keyed(&self) -> bool {
        self.remote.is_some()
    }

    /// `KEY` on: key `slot` (`key::ACTIVE` or 0-2) as its PTT would, with every check. Ok: the slot keyed; Err: why
    /// not, for the PC (the radio shows a refusal as a PTT's: the refused screen, or the toast VOX gets).
    pub fn remote_key(&mut self, slot: u8, now: u32, store: &impl ChannelStore) -> Result<u8, &'static str> {
        self.now = now;
        // The PTT that keys it (KEY-7): PTT B keys B; PTT A and the side connector the active slot. A or C keys only
        // while it is the active slot, since the radio has no PTT for it otherwise.
        let (ptt, s) = match slot {
            key::ACTIVE => (PTT_ACTIVE, self.active),
            1 => (PTT_B, 1),
            s if s == self.active => (PTT_ACTIVE, s),
            s if (s as usize) < SLOTS => return Err("that slot isn't active: the active slot or B"),
            _ => return Err("no such slot"),
        };
        if let Some(r) = self.remote {
            if r.slot == s {
                self.remote = Some(Remote { last: now, ..r }); // a repeated key-up is a keepalive
                return Ok(s);
            }
            return Err(short(Refusal::InProgress));
        }
        if self.local_ptt | self.ptt_cut != 0 {
            return Err("PTT held on the radio");
        }
        if self.radio.keyed.is_some() {
            return Err(short(Refusal::InProgress));
        }
        // TX-35: not a person pressing PTT, so never an emergency frequency, and never the prompt's second press.
        if matches!(self.screen, Screen::Emergency(_)) || txplan::emergency(transmit::tx_freq(self, s)) {
            self.show_toast(b"No remote TX", b"emergency frequency", false);
            return Err("emergency frequency: only PTT on the radio, twice");
        }
        // APRS-18: a PTT there sends or opens a message; remote PTT is voice only.
        if matches!(self.screen, Screen::Compose(_)) || self.aprs_slot(s) {
            return Err("APRS: no voice there");
        }
        self.remote = Some(Remote { ptt, slot: s, last: now });
        self.remote_ended = None;
        self.press(Key::Ptt(ptt), true, now, store);
        if self.radio.keyed == Some(s) {
            return Ok(s);
        }
        // Refused on the way (the refused screen is up, as for PTT): let go, and say why.
        let why = match self.screen {
            Screen::TxRefused(t) => short(t.why),
            _ => "not keyed",
        };
        self.remote = None;
        self.press(Key::Ptt(ptt), false, now, store);
        Err(why)
    }

    /// `KEY` keepalive: Ok while the remote transmission holds; Err with why it ended once it has.
    pub fn remote_keepalive(&mut self, now: u32) -> Result<(), &'static str> {
        match &mut self.remote {
            Some(r) => {
                r.last = now;
                Ok(())
            }
            None => Err(self.remote_ended.unwrap_or(RemoteEnd::Off).text()),
        }
    }

    /// The remote transmission ends (`KEY` off, a missing keepalive, the session, a person's PTT, a stop on the
    /// radio): its PTT comes up, which unkeys (TX-25).
    pub fn remote_unkey(&mut self, why: RemoteEnd, now: u32, store: &impl ChannelStore) {
        let Some(r) = self.remote.take() else { return };
        self.remote_ended = Some(why);
        self.press(Key::Ptt(r.ptt), false, now, store);
        let body: &[u8] = match why {
            RemoteEnd::Keepalive => b"no keepalive",
            RemoteEnd::Session => b"link closed",
            RemoteEnd::LocalPtt => b"PTT on the radio",
            _ => return,
        };
        self.show_toast(b"Remote PTT ended", body, false);
    }

    /// Every tick: no keepalive for 2 s ends it; so does the transmission ending on the radio.
    pub(crate) fn remote_tick(&mut self, now: u32, store: &impl ChannelStore) {
        let Some(r) = self.remote else { return };
        if now.wrapping_sub(r.last) >= key::KEEPALIVE_MS {
            return self.remote_unkey(RemoteEnd::Keepalive, now, store);
        }
        if self.radio.keyed != Some(r.slot) {
            let why = match self.screen {
                Screen::TxRefused(t) => RemoteEnd::Radio(t.why),
                _ if now.wrapping_sub(self.radio.t0) >= self.set.tx_timeout_s() * 1000 => RemoteEnd::Timeout,
                _ => RemoteEnd::Stopped,
            };
            self.remote_unkey(why, now, store);
        }
    }

    /// A PTT press or release that isn't a person's: through the same path as one.
    fn press(&mut self, k: Key, down: bool, now: u32, store: &impl ChannelStore) {
        self.virtual_ptt = true;
        if down {
            self.key_down(k, now, store);
        } else {
            self.key_up(k, now, store);
        }
        self.virtual_ptt = false;
    }

    /// A person's PTT down ([`App::key_down`]): noted; during a remote transmission it ends it and is taken (true).
    pub(crate) fn local_ptt_down(&mut self, k: Key, now: u32, store: &impl ChannelStore) -> bool {
        let Key::Ptt(p) = k else { return false };
        if self.virtual_ptt {
            return false;
        }
        let b = bit(p);
        self.local_ptt |= b;
        if self.ptt_cut & b != 0 {
            return true; // still the press that ended one (a bounce)
        }
        if self.remote.is_some() {
            self.ptt_cut |= b;
            self.remote_unkey(RemoteEnd::LocalPtt, now, store);
            return true;
        }
        false
    }

    /// A person's PTT up ([`App::key_up`]): true if it was the press that ended a remote transmission (taken).
    pub(crate) fn local_ptt_up(&mut self, k: Key) -> bool {
        let Key::Ptt(p) = k else { return false };
        if self.virtual_ptt {
            return false;
        }
        let b = bit(p);
        self.local_ptt &= !b;
        let cut = self.ptt_cut & b != 0;
        self.ptt_cut &= !b;
        cut
    }
}
