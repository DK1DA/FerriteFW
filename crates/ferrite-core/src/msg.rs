//! What the six tasks say to each other (`docs/firmware-design.md`, "Messaging"). Each piece of hardware has one
//! owning task; others send it a command on its channel, and owners announce what happened as [`Event`]s on one
//! bus. Live values the UI draws every frame ([`Live`]) are published as a snapshot, never queued.
//!
//! The channels themselves are statics in the firmware; these are the types, so the logic and the simulator can
//! use them without an executor.

use crate::freq::Freq10;

/// One of the two BK4829s: 0 = VHF and 200-300 MHz, 1 = 300 MHz up and HF.
pub type Chip = u8;

/// To the Radio task: the only one that touches the BK4829s, the SI4732, the RF path and the PA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioCmd {
    /// Receive on a frequency (the Radio task picks the chip and path from the band).
    Tune(Freq10),
    /// Squelch level 0-9.
    Squelch(u8),
    /// Battery Save: the receiver sleep cycle.
    BatterySave(crate::rxsleep::Level),
    /// A key or knob: counts as activity for receiver sleep.
    Activity,
    /// Broadcast receiver: FM in 10 kHz units, AM in kHz.
    Broadcast(BroadcastCmd),
    /// Everything off but the MCU (power switch).
    Shutdown,
    /// Receive on a frequency in one slot (0 = A, 1 = B, 2 = C).
    TuneSlot { slot: u8, freq: Freq10 },
    /// One of a slot's settings (the UI sends it whenever the user changes it).
    SlotParam { slot: u8, param: Param, value: i32 },
    /// The active slot: heard first, owns the band relay (WATCH-3, WATCH-10).
    Active(u8),
    /// The watch set, a bit per slot (0 = watch off).
    Watch(u8),
    /// Start or stop scanning in a slot.
    Scan { slot: u8, on: bool },
    /// Pause watching for this many seconds (WATCH-6).
    PauseWatch(u16),
}

/// A slot setting carried by [`RadioCmd::SlotParam`]. Values are the UI's indexes, documented per variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    /// 0 FM, 1 NFM, 2 AM, 3 USB, 4 LSB, 5 CW.
    Mode,
    /// 0 wide, 1 narrow.
    Bandwidth,
    /// 0-9.
    Squelch,
    /// The tone word (CHAN-3): CTCSS in 0.1 Hz, bit 15 DCS, bit 14 inverted, 0 = none.
    RxTone,
    TxTone,
    /// Transmit offset in 10 Hz units, signed (0 = simplex).
    Offset,
    /// 0 low, 1 mid, 2 high. The Radio task caps 1.25 m at 0.25 W whatever this says.
    Power,
    /// 0 off, 1 carrier, 2 wrong tone.
    BusyLock,
    /// 0 off, 1 on.
    Reverse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastCmd {
    Off,
    Fm(u16),
    Am(u16),
    Seek { up: bool },
}

/// To the Storage task: the only one that touches the SPI flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageCmd {
    SaveSettings,
    Log(Event),
}

/// To the DSP task: which decoder runs in the shared slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DspCmd {
    Decoder(DecoderKind),
    Beep { hz: u16, ms: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoderKind {
    Off,
    Afsk,
    Pocsag,
    Dsc,
}

/// Who announced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Radio,
    Dsp,
    Link,
    Storage,
    Gps,
    Ui,
}

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Boot check result: payload[0] = check index, payload[1] = 1 passed / 0 failed.
    Check,
    SquelchOpen,
    SquelchClose,
    /// A decoder found something; the payload is the start of it, the full record goes to the event log.
    Decoded,
    GpsFix,
    LinkConnected,
    Fault,
}

/// One event: small and fixed-size, so the bus is a static array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    pub source: Source,
    pub payload: [u8; 12],
}

impl Event {
    pub const fn new(kind: Kind, source: Source) -> Self {
        Event { kind, source, payload: [0; 12] }
    }
}

/// The receiver's state now, published by the Radio task, drawn by the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Live {
    pub freq: Freq10,
    /// dBm.
    pub rssi: i16,
    pub squelch_open: bool,
    /// The receiver is in its sleep phase (Battery Save).
    pub rx_asleep: bool,
    /// Chips that answered with their ID at start-up (bit per chip).
    pub chips: u8,
}

/// Slots on the radio: A, B and C (manual chapter 6).
pub const SLOTS: usize = 3;

/// One slot's receiver now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotLive {
    /// What the receiver is on (while scanning, the current step).
    pub freq: Freq10,
    /// dBm.
    pub rssi: i16,
    pub squelch_open: bool,
    pub scanning: bool,
    /// Scan hits so far, and the last one.
    pub hits: u8,
    pub last_hit: Freq10,
    /// Not watched: it needs the other band relay state (WATCH-11).
    pub relay: bool,
}

/// All three slots, published by the Radio task as one snapshot; [`Live`] stays the single-receiver view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveAll {
    pub slots: [SlotLive; SLOTS],
    pub rx_asleep: bool,
    pub chips: u8,
}

impl LiveAll {
    /// One slot as the single-receiver [`Live`].
    pub fn live(&self, slot: usize) -> Live {
        let s = self.slots[slot % SLOTS];
        Live { freq: s.freq, rssi: s.rssi, squelch_open: s.squelch_open, rx_asleep: self.rx_asleep, chips: self.chips }
    }
}

/// GPS receiver state for the status bar (SBAR-7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpsState {
    #[default]
    Off,
    Searching,
    Fix,
}

/// Radio-wide state the status bar and Status page show, gathered from the GPS, Link and power code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// Minutes since midnight, or None with no time source (`--:--`).
    pub clock_min: Option<u16>,
    pub utc: bool,
    pub gps: GpsState,
    pub sats: u8,
    /// A Bluetooth device is connected.
    pub bt: bool,
    /// Battery in millivolts and percent.
    pub battery_mv: u16,
    pub battery_pct: u8,
    /// Below the warning level / at the transmit cut-off.
    pub battery_low: bool,
    pub battery_cutoff: bool,
    /// This build can transmit (`--features tx`).
    pub tx_built: bool,
}

/// GPS, published by the GPS task.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub fix: bool,
    /// Degrees x 1e7.
    pub lat: i32,
    pub lon: i32,
    pub sats: u8,
    /// UTC seconds of the day.
    pub time_s: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small() {
        // The bus holds a few dozen of these; keep them small.
        assert!(core::mem::size_of::<Event>() <= 16);
        assert!(core::mem::size_of::<RadioCmd>() <= 8);
        assert!(core::mem::size_of::<LiveAll>() <= 64);
    }
}
