//! A slot's working fields from the PC (manual chapter 37, TERM-35; chapter 29, LINK-33): the terminal's `slot`
//! command and the link's `SLOT` message both come here, so they share one set of rules. Each change takes the
//! keypad's own path: a frequency is the VFO entry's ([`crate::screens::entry::tune_vfo`]), mode and power the
//! quick settings' knob ([`App::change`]), a tone the Tones screen's ([`crate::screens::tones::set`]), `vfo` the V/M
//! key and `active` the SLOT key. So the checks are the keypad's, and so are the writes: a VFO's state is kept as the
//! keypad's is, and a change on a channel slot saves that channel, as a change made there on the radio does (EDIT-4).
//!
//! Refused as a whole, before anything changes: a slot on a channel given a frequency (its VFO is tuned only with
//! `vfo` in the same line, or after it: the channel is never retuned from here), a shift over 100 MHz, a split TX
//! frequency no chip tunes or above 1000 MHz, a shift or split that needs the other band relay from RX (TX-37), the
//! APRS slot (APRS-19),
//! channel-only mode (PIN-5: only `active`), a slot scanning (SIDE2 stops it), and anything while transmitting.

use crate::chan::ChannelStore;
use crate::screens::{entry, tones};
use crate::slot::Item;
use crate::App;
use ferrite_core::freq::Freq10;
use ferrite_core::msg::{RadioCmd, SLOTS};

/// What to change on a slot; None (false) leaves a field as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotChange {
    /// The VFO's frequency (10 Hz).
    pub freq: Option<Freq10>,
    /// FM, NFM, AM (0-2).
    pub mode: Option<u8>,
    /// Low, mid, high (0-2).
    pub power: Option<u8>,
    /// Both tones, sent and decoded (CHAN-3's tone word); or each alone.
    pub rx_tone: Option<u16>,
    pub tx_tone: Option<u16>,
    /// The repeater shift or split, as the slot's record keeps it: (signed shift in 10 Hz, 0 simplex; false) or
    /// (the TX frequency; true), as MENU 5 sets it ([`App::set_shift`], [`App::set_split`]).
    pub offset: Option<(i32, bool)>,
    /// To the slot's VFO, as V/M from a channel (the VFO keeps its own frequency unless `freq` is given).
    pub vfo: bool,
    /// Make it the active slot, as the SLOT key.
    pub active: bool,
}

impl SlotChange {
    /// Changes nothing: a read.
    pub fn is_empty(&self) -> bool {
        *self == SlotChange::default()
    }

    fn touches_slot(&self) -> bool {
        SlotChange { active: false, ..*self } != SlotChange::default()
    }
}

/// A slot as the PC sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotState {
    pub active: bool,
    /// The channel it is on (0-based), or None on its VFO.
    pub channel: Option<u16>,
    pub freq: Freq10,
    pub mode: u8,
    pub power: u8,
    pub rx_tone: u16,
    pub tx_tone: u16,
    /// As the slot's record: signed shift, or the split's TX frequency.
    pub offset: i32,
    pub split: bool,
    pub aprs: bool,
    pub scanning: bool,
}

impl App {
    /// Slot `s` (0-2) as it is now.
    pub fn slot_state(&self, s: u8) -> SlotState {
        let sl = &self.slots[s as usize % SLOTS];
        let c = self.tuned_cfg(s);
        SlotState {
            active: self.active == s,
            channel: sl.ch.filter(|_| sl.on_channel()).map(|c| c.num),
            freq: self.tuned_freq(s),
            mode: c.mode,
            power: c.power,
            rx_tone: c.rx_tone,
            tx_tone: c.tx_tone,
            offset: c.offset,
            split: c.split,
            aprs: self.aprs_slot(s),
            scanning: self.scan_mask & 1 << s != 0,
        }
    }

    /// Changes slot `s` as the keypad would, or refuses the whole change with the reason (nothing changed).
    pub fn slot_apply(&mut self, s: u8, ch: &SlotChange, _store: &impl ChannelStore) -> Result<(), &'static str> {
        if s as usize >= SLOTS {
            return Err("no such slot: A, B or C");
        }
        if ch.mode.is_some_and(|m| !crate::slot::slot_mode_ok(m)) {
            return Err("mode: fm, nfm, am or wfm");
        }
        if ch.power.is_some_and(|p| p > 2) {
            return Err("power: low, mid or high");
        }
        if ch.touches_slot() {
            if self.radio.keyed.is_some() {
                return Err("transmitting: nothing changes while keyed (TX-22)");
            }
            if self.set.chan_only {
                return Err("channel-only mode: only the active slot can change (PIN-5)");
            }
            if self.aprs_slot(s) {
                return Err(match self.aprs_holder() {
                    Some(crate::aprs::Holder::Kiss) => "slot C held by KISS (KISS-9)",
                    Some(crate::aprs::Holder::Beacon) => "slot C held by beacons (APRS-29)",
                    _ => "slot C held by APRS mode (APRS-19)",
                });
            }
            if self.scan_mask & 1 << s != 0 {
                return Err("the slot is scanning: SIDE2 stops it");
            }
            if let Some(f) = ch.freq {
                if self.slots[s as usize].on_channel() && !ch.vfo {
                    return Err("the slot is on a channel: add vfo to tune its VFO");
                }
                if let Some(why) = entry::refuse(f) {
                    return Err(why);
                }
            }
            if let Some((v, split)) = ch.offset {
                if split {
                    let tx = v.max(0) as Freq10;
                    if let Some(why) = entry::refuse(tx) {
                        return Err(why);
                    }
                    if tx > ferrite_core::freq::mhz(1000) {
                        return Err("split: never TX above 1000 MHz");
                    }
                } else if v.unsigned_abs() >= ferrite_core::freq::mhz(100) {
                    return Err("offset: under 100 MHz");
                }
                // TX-37 on the frequency the slot will have.
                let rx = ch.freq.unwrap_or_else(|| self.tuned_freq(s));
                let mut c = self.tuned_cfg(s);
                (c.offset, c.split) = (v, split);
                if !ferrite_core::txplan::relay_ok(c.rx_of(rx), c.tx_of(rx)) {
                    return Err("TX would need the other band relay from RX (TX-37)");
                }
            }
        }
        // Each field through the keypad's path, on `s` as the active slot (they all act on the active one).
        let was = self.active;
        self.active = s;
        if ch.vfo && !self.slots[s as usize].vfo_mode {
            self.slots[s as usize].vfo_mode = true; // V/M (the VFO keeps its frequency)
            self.send_slot(s);
        }
        if let Some(f) = ch.freq {
            entry::tune_vfo(self, s, f);
        }
        if let Some(m) = ch.mode {
            let d = m as i32 - self.slots[s as usize].cfg().mode as i32;
            self.change(Item::Mode, d);
        }
        if let Some(p) = ch.power {
            let d = p as i32 - self.slots[s as usize].cfg().power as i32;
            self.change(Item::Power, d);
        }
        match ch.offset {
            Some((tx, true)) => _ = self.set_split(tx.max(0) as Freq10),
            Some((shift, false)) => _ = self.set_shift(shift),
            None => {}
        }
        if let Some(t) = ch.tx_tone {
            tones::set(self, false, t);
        }
        if let Some(t) = ch.rx_tone {
            tones::set(self, true, t);
        }
        self.active = was;
        if ch.active && self.active != s {
            self.active = s; // NAV-5, the SLOT key
            self.out.push(RadioCmd::Active(s));
        }
        Ok(())
    }
}
