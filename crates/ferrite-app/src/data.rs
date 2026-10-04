//! # Records
//!
//! What the list screens read besides channels: the event log, APRS (heard, messages, the message being
//! written), GPS (fix, satellites, waypoints), the sensor reader, the scan lists (range presets, this scan's
//! skips, zone options), FM presets and HF recent. Each comes from the task that owns it (the Storage task for
//! flash records, the APRS and GPS tasks for what lives in their RAM) and is read on demand, one record at a
//! time, newest first where that applies: the App never holds a copy (CHAN-9, APRS-4, INBOX-6). Changes go
//! back to the owner as requests ([`Op`]).
//!
//! The screens reach these through [`crate::chan::ChannelStore::data`]; a store without them returns
//! [`NoData`], and every list shows empty. [`crate::sample::Sample`] fills them for the simulator and tests.

use ferrite_core::freq::Freq10;
use ferrite_ui::text::Buf;

/// A position, degrees x 1e7 (CHAN-2, GPS-10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pos {
    pub lat: i32,
    pub lon: i32,
}

/// Where an event came from (INBOX-9: the log counts and filters by source).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    Aprs,
    Pocsag,
    Same,
    Dsc,
    Ais,
    Dtmf,
    Selcall,
    Sensor,
}

impl Src {
    pub const ALL: [Src; 8] = [Src::Aprs, Src::Pocsag, Src::Same, Src::Dsc, Src::Ais, Src::Dtmf, Src::Selcall, Src::Sensor];

    pub fn name(self) -> &'static str {
        ["APRS", "Pager", "SAME", "DSC", "AIS", "DTMF", "Selcall", "Sensor"][self as usize]
    }
}

/// One event log record (INBOX-6): time, source, frequency, type and payload, decoded. What the fields hold
/// depends on the source:
///
/// | Source | `id` | `x`, `y` | `name` | `text` |
/// | --- | --- | --- | --- | --- |
/// | APRS | | | from | the message |
/// | Pager | capcode | baud, function | | the message |
/// | SAME | | | | the header (`ZCZC-WXR-SVR-...`) |
/// | DSC | MMSI | category (0 routine, 1 safety, 2 urgency, 3 distress), | | nature or the call |
/// | AIS | MMSI | course (degrees), speed (0.1 kn) | vessel | |
/// | DTMF | | | | the sequence |
/// | Selcall | unit ID | | format | what it sent |
/// | Sensor | device ID | | model | the reading |
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Event {
    pub src: Src,
    pub ago_s: u32,
    pub freq: Freq10,
    pub slot: u8,
    pub unread: bool,
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub pos: Option<Pos>,
    pub name: Buf<20>,
    pub text: Buf<64>,
}

/// APRS symbols the heard list names (APRS-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sym {
    Car,
    Home,
    Digi,
    Wx,
    Ht,
    IGate,
    Truck,
    Boat,
}

impl Sym {
    pub fn name(self) -> &'static str {
        ["Car", "Home", "Digi", "WX", "HT", "iGate", "Truck", "Boat"][self as usize]
    }
}

/// A heard APRS station (APRS-3 to APRS-5).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Station {
    pub call: Buf<10>,
    pub sym: Sym,
    pub pos: Option<Pos>,
    pub ago_s: u32,
    /// Digipeaters it came through, the used one marked with *; empty = direct.
    pub via: Buf<24>,
    /// Course in degrees and speed in km/h, when it reports them.
    pub course: Option<(u16, u16)>,
    pub alt_m: Option<i32>,
    pub times: u16,
    pub comment: Buf<40>,
    /// The last message it sent us and the clock minute it came.
    pub msg: Buf<32>,
    pub msg_min: Option<u16>,
}

/// A message's state (APRS-6, APRS-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MsgState {
    Unread,
    Read,
    Pending,
    Acked,
    /// Retrying, with the count so far.
    Retrying(u8),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Message {
    pub inbound: bool,
    /// From (inbound) or to (sent).
    pub peer: Buf<10>,
    pub text: Buf<67>,
    pub ago_s: u32,
    pub state: MsgState,
}

/// The message being written: it lives with the APRS task's outgoing message (which holds it for retries),
/// so the App keeps no text buffer of its own.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Draft {
    pub to: Buf<9>,
    pub text: Buf<67>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WpSrc {
    Marked,
    Aprs,
    Link,
}

/// A waypoint (GPS-10).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Waypoint {
    pub name: Buf<16>,
    pub pos: Pos,
    pub src: WpSrc,
}

/// A device the sensor reader heard (DEC-11).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sensor {
    pub model: Buf<16>,
    pub id: u32,
    pub value: Buf<24>,
    pub rssi: i16,
    pub first_ago_s: u32,
    pub last_ago_s: u32,
}

/// The sensor reader's noise floor against its baseline (DEC-12).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Noise {
    pub floor: i16,
    pub baseline: i16,
    /// Jamming alarm threshold in dB, 0 = off.
    pub alarm_db: u8,
    pub jamming: bool,
}

/// The latest fix (GPS-2): consumers see its age.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fix {
    pub pos: Pos,
    pub three_d: bool,
    pub alt_m: i32,
    pub speed_kmh: u16,
    pub heading: u16,
    pub hdop_x10: u16,
    pub used: u8,
    pub age_s: u16,
    /// UTC seconds of the day.
    pub utc_s: u32,
}

/// A satellite in view.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sat {
    pub prn: u8,
    /// Signal, dB-Hz.
    pub snr: u8,
    pub used: bool,
    /// GPS or BeiDou (the ATGM336H tracks both).
    pub beidou: bool,
}

/// A scan range preset (MENU 0 1 7; SCAN-4, SCAN-10).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RangePreset {
    pub name: Buf<12>,
    pub start: Freq10,
    pub end: Freq10,
    /// Index into [`crate::slot::STEPS`].
    pub step: u8,
    /// Index into [`crate::slot::MODES`].
    pub mode: u8,
    pub narrow: bool,
    /// Frequencies never stopped on (up to 48).
    pub skips: u8,
}

/// A zone's own settings (CHAN-6).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ZoneCfg {
    /// In the "scan all" set.
    pub scan_all: bool,
    /// Shown in ◀▶ on home (CHAN-11).
    pub shown: bool,
    pub by_distance: bool,
    /// 0 the radio's rule, 1-10 s, 11 carrier drop, 12 stop on hit (SCAN-5).
    pub resume: u8,
    /// Dwell before resuming, s; 255 the radio's.
    pub dwell: u8,
    pub smart: bool,
    /// Priority channels by number.
    pub pri: [Option<u16>; 2],
}

/// An FM broadcast preset.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FmPreset {
    /// 10 kHz units: 10110 is 101.1 MHz.
    pub freq: u16,
    /// RDS name, when one was heard.
    pub name: Buf<8>,
}

/// Requests to the record's owner. `request` answers false when refused (no such record, queue full).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// The event log: one event read, all read, everything gone.
    Read(u16),
    ReadAll,
    ClearLog,
    /// Channels by number (CHAN-13: deleting happens only in the browser).
    DeleteChannel(u16),
    Skip(u16, bool),
    Move(u16, u8),
    Renumber(u16, u16),
    NewZone,
    /// CHAN-12: its channels go to no zone.
    DeleteZone(u8),
    Zone(u8, ZoneCfg),
    /// A zone's new name (from text entry).
    RenameZone(u8, crate::chan::Name),
    Preset(u8, RangePreset),
    NewPreset,
    DeletePreset(u8),
    ClearSkips,
    Unskip(u16),
    /// A waypoint at the current fix (GPS-10, source "marked"), or from a heard station (source "APRS").
    MarkHere,
    SaveStation(u16),
    DeleteWaypoint(u16),
    Draft(Draft),
    /// Send the draft (APRS-7, APRS-8).
    Send,
    MessageRead(u16),
    BeaconNow,
    ClearHeard,
}

/// The records, read one at a time. Every method has a default (nothing there), so an owner that doesn't
/// exist yet costs nothing.
pub trait Data {
    /// Changes whenever anything here changes: lists redraw only when it or their own position moved.
    fn version(&self) -> u32 {
        0
    }

    /// The event log, newest first.
    fn events(&self) -> u16 {
        0
    }
    fn event(&self, _i: u16) -> Option<Event> {
        None
    }

    /// APRS heard, newest first; the station callsign and SSID this radio sends as (APRS-8: None = none set).
    fn stations(&self) -> u16 {
        0
    }
    fn station(&self, _i: u16) -> Option<Station> {
        None
    }
    fn callsign(&self) -> Option<Buf<10>> {
        None
    }
    /// The APRS frequency (0 5 7 1).
    fn aprs_freq(&self) -> Freq10 {
        14_439_000
    }

    /// APRS messages, newest first, inbox and sent together.
    fn messages(&self) -> u16 {
        0
    }
    fn message(&self, _i: u16) -> Option<Message> {
        None
    }
    fn draft(&self) -> Draft {
        Draft::default()
    }

    fn waypoints(&self) -> u16 {
        0
    }
    fn waypoint(&self, _i: u16) -> Option<Waypoint> {
        None
    }

    fn sensors(&self) -> u16 {
        0
    }
    fn sensor(&self, _i: u16) -> Option<Sensor> {
        None
    }
    fn noise(&self) -> Option<Noise> {
        None
    }

    /// The latest fix (None with GPS off or no fix yet) and the satellites in view.
    fn fix(&self) -> Option<Fix> {
        None
    }
    fn sats(&self) -> u8 {
        0
    }
    fn sat(&self, _i: u8) -> Option<Sat> {
        None
    }

    fn presets(&self) -> u8 {
        0
    }
    fn preset(&self, _i: u8) -> Option<RangePreset> {
        None
    }
    /// This scan's nuisance skips (SCAN-11), in the Radio task's RAM.
    fn skips(&self) -> u16 {
        0
    }
    fn skip(&self, _i: u16) -> Option<Freq10> {
        None
    }
    fn zone_cfg(&self, _zone: u8) -> ZoneCfg {
        ZoneCfg { shown: true, ..ZoneCfg::default() }
    }

    fn fm_presets(&self) -> u8 {
        0
    }
    fn fm_preset(&self, _i: u8) -> Option<FmPreset> {
        None
    }

    /// A change, to the record's owner.
    fn request(&self, _op: Op) -> bool {
        false
    }
}

/// Nothing stored.
pub struct NoData;

impl Data for NoData {}
