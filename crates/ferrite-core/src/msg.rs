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
    }
}
