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
    /// Key or unkey a slot (3 = the HF mode frequency), after the UI's checks; the Radio task checks again
    /// (TX-17) and answers in [`LiveAll::tx`].
    Transmit { slot: u8, on: bool },
    /// Scanning: skip the current hit for the rest of this scan (SCAN-11).
    ScanSkip { slot: u8 },
    /// Scanning: the zone set, zones 0-15 by bit (SCAN-3; 0 = the current zone only).
    ScanZones { slot: u8, zones: u16 },
    /// Band scope sweep around a centre at a step (10 Hz units / 25); off when `on` is false (SCOPE-4).
    Sweep { centre: Freq10, step: u16, on: bool },
    /// Tone search on a slot (TSRCH-1).
    ToneSearch { slot: u8, on: bool },
    /// The frequency counter (TSRCH-6).
    Counter(bool),
    /// HF mode: enter on a frequency and mode (as [`Param::Mode`]), retune, or leave (HF-3, HF-4). Entering
    /// switches the relay once, powers the SI4732 up with the SSB patch and pauses watch, scan, decoders, jobs
    /// and beacons; leaving undoes all of it (HF-8). The receiver follows from frequency and mode (HF-10).
    Hf { freq: Freq10, mode: u8, on: bool },
    /// The crossband repeater: start with these settings, or stop (XB-14).
    Crossband(Option<XbCfg>),
}

/// The crossband repeater's settings (manual chapter 19): small enough to ride in a [`RadioCmd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct XbCfg {
    /// The two slots, 0-2 each.
    pub side1: u8,
    pub side2: u8,
    /// 0 low, 1 mid (XB-3: never above mid).
    pub power: u8,
    /// Per-pass timeout in 10 s units (30 s - 10 min).
    pub timeout_10s: u8,
    /// Hang time in 0.1 s (0 - 3 s).
    pub hang_ds: u8,
    /// Bit 0 tone gate, bit 1 CW ID, bit 2 speaker.
    pub flags: u8,
}

impl XbCfg {
    pub const TONE_GATE: u8 = 1;
    pub const ID: u8 = 2;
    pub const SPEAKER: u8 = 4;
    pub const DEFAULT: XbCfg = XbCfg { side1: 0, side2: 1, power: 0, timeout_10s: 18, hang_ds: 10, flags: 3 };
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

/// A SAME header the decoder heard (DSP task to the UI task, which raises the alert: WX-4, WX-5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SameDecoded {
    pub header: crate::same::SameHeader,
    /// The slot and frequency it was heard on.
    pub slot: u8,
    pub freq: Freq10,
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
    /// A carrier above the squelch (the BK4829's RSSI and noise squelch), whatever tone it carries.
    pub squelch_open: bool,
    /// The slot's own RX tone (CTCSS or DCS) is decoded on that carrier (busy lockout "wrong tone", TX-21).
    pub tone_heard: bool,
    pub scanning: bool,
    /// Scan hits so far, and the last one.
    pub hits: u8,
    pub last_hit: Freq10,
    /// Not watched: it needs the other band relay state (WATCH-11).
    pub relay: bool,
    /// Nuisance skips in this scan (SCAN-11).
    pub skips: u8,
}

/// All three slots, published by the Radio task as one snapshot; [`Live`] stays the single-receiver view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveAll {
    pub slots: [SlotLive; SLOTS],
    pub rx_asleep: bool,
    pub chips: u8,
    /// Transmit: keyed, or the Radio task's refusal.
    pub tx: TxLive,
    /// The one exclusive mode running, if any: scope, tone search, counter, FM broadcast, HF, crossband.
    pub mode: ModeLive,
}

/// Transmit now (TX-22).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxLive {
    pub keyed: bool,
    /// Which slot (3 = HF mode).
    pub slot: u8,
    /// PA drive, % of the calibrated level (TX-12: never watts).
    pub drive: u8,
    /// The Radio task refused the last key-up (its own checks: calibration, read-back, battery).
    pub refused: Option<crate::txplan::Refusal>,
}

/// Bars in a band scope sweep: 4 px each across 224 px.
pub const SCOPE_BARS: usize = 56;
/// Crossband activity: 60 columns of 30 s, the last 30 minutes (XB-16).
pub const XB_COLS: usize = 60;

/// What the running exclusive mode reports. One at a time: they all own the receivers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModeLive {
    #[default]
    Off,
    /// Band scope: one level per step (0 = blank: not swept or needs the other relay state, SCOPE-5), else
    /// dBm + 160.
    Scope { centre: Freq10, bars: [u8; SCOPE_BARS], sweeps: u8, rate: u8 },
    /// Tone search: the tone word found (0 = searching); counter: the strongest frequency (0 = none) and level.
    Probe { freq: Freq10, rssi: i16, tone: u16 },
    /// FM broadcast: frequency (10 kHz), RDS station name and text, level (dBuV) and SNR (dB).
    Fm { freq: u16, ps: [u8; 8], rt: [u8; 32], rssi: u8, snr: u8, stereo: bool },
    /// HF mode: frequency, signal (dBm), SNR on the SI4732, which receiver, the SSB patch load (0-100, 255 =
    /// loaded or none).
    Hf { freq: Freq10, rssi: i16, snr: u8, si4732: bool, loading: u8 },
    /// Crossband repeater.
    Xb(XbLive),
}

/// The crossband repeater now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XbLive {
    /// 0 ready, 1 side 1 -> side 2, 2 side 2 -> side 1, 3 hang, 4 timed out (waiting for the signal to drop).
    pub state: u8,
    /// Seconds running, and in the current (or last) pass.
    pub run_s: u32,
    pub pass_s: u16,
    /// Seconds since the last pass ended, and its direction (1 or 2).
    pub idle_s: u16,
    pub last_dir: u8,
    pub passes: u16,
    /// Seconds passed in all.
    pub busy_s: u16,
    /// Transmit duty over the last 10 minutes, %.
    pub duty: u8,
    /// Seconds to the next CW ID.
    pub next_id_s: u16,
    /// Receive level on the input side (dBm) and PA drive (%) on the output side.
    pub rssi: i16,
    pub drive: u8,
    /// Seconds passed per column, oldest first: positive side 1 -> 2, negative 2 -> 1.
    pub strip: [i8; XB_COLS],
}

impl Default for XbLive {
    fn default() -> Self {
        XbLive { state: 0, run_s: 0, pass_s: 0, idle_s: 0, last_dir: 0, passes: 0, busy_s: 0, duty: 0, next_id_s: 0, rssi: -127, drive: 0, strip: [0; XB_COLS] }
    }
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
    pub selftest: SelfTest,
    pub link: LinkLive,
}

/// Calibration status from the CAL checks (chapter 35).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalState {
    /// Not checked yet.
    #[default]
    Unknown,
    Ok,
    /// Erased (0xF0E0 = 0xFF): transmit locked (CAL-5).
    Blank,
    /// A value fails CAL-6.
    Invalid,
    /// The sector's CRC differs from the one stored at install (CAL-7).
    CrcMismatch,
}

/// What the self-test found, for About (SELF-5) and the Status page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelfTest {
    /// DEBUG_IDCODE, 0 until read.
    pub idcode: u32,
    /// The MCU is the expected part (SELF-1).
    pub mcu_ok: bool,
    /// SPI flash JEDEC ID (0 = no answer) and size from it, in MB.
    pub flash_id: u32,
    pub flash_mb: u8,
    pub cal: CalState,
    /// BK4829s that answered, a bit per chip; the SI4732 answered.
    pub chips: u8,
    pub si4732: bool,
    /// A verified SSB patch is in SPI flash (HF-20).
    pub ssb_patch: bool,
    /// Battery reading trim in 10 mV (0 9 7 3).
    pub battery_trim: i8,
}

/// The link (chapter 29): cable, and a write session's progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkLive {
    /// The web app is connected over the cable.
    pub cable: bool,
    /// The cable's rate in hundreds of baud (1152 = 115200).
    pub baud_100: u16,
    /// A write session: percent done (LINK-9).
    pub write_pct: Option<u8>,
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
        // The snapshot isn't queued (one copy in the watch, the Radio task's and the UI's); the exclusive
        // modes share one slot in it, the largest being the crossband activity strip.
        assert!(core::mem::size_of::<LiveAll>() <= 160);
    }
}
