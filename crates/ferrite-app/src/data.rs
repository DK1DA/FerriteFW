//! # Records
//!
//! What the list screens read besides channels: the event log, APRS (heard, messages, the message being
//! written), GPS (fix, satellites, waypoints), the scan lists (range presets, this scan's
//! skips, zone options) and HF recent. Each comes from the task that owns it (the Storage task for
//! flash records, the APRS and GPS tasks for what lives in their RAM) and is read on demand, one record at a
//! time, newest first where that applies: the App never holds a copy (CHAN-9, APRS-4, INBOX-6). Changes go
//! back to the owner as requests ([`Op`]).
//!
//! The screens reach these through [`crate::chan::ChannelStore::data`]; a store without them returns
//! [`NoData`], and every list shows empty. [`crate::sample::Sample`] fills them for the simulator and tests.

pub use crate::aprs::AprsCfg;
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
    /// Arriving at a waypoint (GPS-12): the waypoint's name, and "Arrived, N m" as the text.
    Gps,
}

impl Src {
    /// In the order the log stores them (a record's source byte): new sources go at the end.
    pub const ALL: [Src; 8] = [Src::Aprs, Src::Pocsag, Src::Same, Src::Dsc, Src::Ais, Src::Dtmf, Src::Selcall, Src::Gps];

    pub fn name(self) -> &'static str {
        ["APRS", "Pager", "SAME", "DSC", "AIS", "DTMF", "Selcall", "GPS"][self as usize]
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
    /// APRS-44: its last frame came as FX.25, with this many bytes corrected.
    pub fx25: Option<u8>,
}

/// A message's state (APRS-6, APRS-13, APRS-34).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MsgState {
    Unread,
    Read,
    Pending,
    Acked,
    /// Retrying, with the count so far.
    Retrying(u8),
    Failed,
    /// The recipient rejected it (APRS-34): ended at once, like an ack.
    Rejected,
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

/// The beacon, as the APRS task has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Beacon {
    /// One is due but needs the band relay in the other state (APRS-20): it waits for the active slot.
    pub waiting: bool,
    /// Seconds to the next timed beacon.
    pub next_s: Option<u32>,
    /// APRS-36: the digipeater last heard repeating one of our frames (empty: none since power-on).
    pub digi: Buf<10>,
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
    /// Typed as a grid square (GPS-21).
    Grid,
}

/// A saved station (APRS-37): its callsign and SSID as compose takes them, and a name.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Contact {
    pub call: Buf<10>,
    pub name: Buf<12>,
}

/// A waypoint (GPS-10).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Waypoint {
    pub name: Buf<16>,
    pub pos: Pos,
    pub src: WpSrc,
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
    /// Elevation in degrees (0 horizon, 90 overhead) and azimuth in degrees from north, from GSV; for the sky
    /// plot (GPS-18).
    pub elev: u8,
    pub azim: u16,
}

/// A scan range preset (MENU 0 1 4; SCAN-4, SCAN-10).
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
    /// 0 Global (0 1 2's rule), 1-10 s, 11 carrier drop, 12 stop on hit (SCAN-5, SCAN-24).
    pub resume: u8,
}

impl RangePreset {
    /// VFO-20: the mode carries the bandwidth; FM with narrow from before reads as NFM.
    pub fn fold_bw(&mut self) {
        if self.mode == 0 && self.narrow {
            self.mode = 1;
        }
        self.narrow = self.mode == 1;
    }
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
    /// Priority channels by number.
    pub pri: [Option<u16>; 2],
}


/// A DTMF memory (0 8 5 2, chapter 17): up to 16 symbols as nibbles, 0-9, A-D (10-13), * (14), # (15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Dtmf {
    pub len: u8,
    pub d: [u8; 8],
}

/// DTMF memories: 16 of up to 16 symbols (0 8 5 2).
pub const DTMF_MEMS: u8 = 16;
/// The DTMF symbols by nibble.
pub const DTMF_CHARS: &[u8; 16] = b"0123456789ABCD*#";

impl Dtmf {
    pub const MAX: u8 = 16;

    pub fn get(&self, i: u8) -> u8 {
        self.d[i as usize / 2] >> (4 * (i % 2)) & 15
    }

    /// Appends a symbol (a nibble); false when full.
    pub fn push(&mut self, n: u8) -> bool {
        if self.len >= Self::MAX {
            return false;
        }
        let i = self.len as usize;
        self.d[i / 2] = self.d[i / 2] & !(15 << (4 * (i % 2))) | (n & 15) << (4 * (i % 2));
        self.len += 1;
        true
    }

    pub fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    /// As text ("*79#").
    pub fn text(&self, b: &mut Buf<16>) {
        b.clear();
        for i in 0..self.len {
            b.push(&[DTMF_CHARS[self.get(i) as usize]]);
        }
    }

    /// From text: 0-9, A-D (either case), * and #; anything else refuses it.
    pub fn parse(s: &[u8]) -> Option<Dtmf> {
        let mut d = Dtmf::default();
        for c in s {
            let n = DTMF_CHARS.iter().position(|x| *x == c.to_ascii_uppercase())?;
            if !d.push(n as u8) {
                return None;
            }
        }
        Some(d)
    }
}

/// Requests to the record's owner. `request` answers false when refused (no such record, queue full).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// The event log: one event read, all read, everything gone.
    Read(u16),
    ReadAll,
    ClearLog,
    /// APRS-46: every message to us read (and its log entry, INBOX-7.1); every message gone, both ways.
    MessagesReadAll,
    ClearMessages,
    /// Channels by number (CHAN-13: deleting happens only in the browser).
    DeleteChannel(u16),
    Skip(u16, bool),
    /// Hidden from the channel lists, or shown again (decision 63).
    Hide(u16, bool),
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
    /// A zone written whole, created when it is the next one (the terminal's `zone` command and `load`, TERM-13).
    PutZone(u8, crate::chan::Name, ZoneCfg),
    /// A range preset written whole, created when it is the next one (TERM-13); its skips are kept.
    PutPreset(u8, RangePreset),
    DeletePreset(u8),
    ClearSkips,
    Unskip(u16),
    /// A waypoint at the current fix (GPS-10, source "marked"), or from a heard station (source "APRS").
    MarkHere,
    SaveStation(u16),
    DeleteWaypoint(u16),
    Draft(Draft),
    /// Quick message i as the owner wrote it (TEXT-7); None puts the factory text back.
    Quick(u8, Option<Buf<67>>),
    /// Send the draft (APRS-7, APRS-8).
    Send,
    MessageRead(u16),
    BeaconNow,
    /// A voice transmission ended with beacon after voice on (APRS-27): a beacon when APRS-9's spacing allows.
    AfterVoice,
    ClearHeard,
    /// The reset (BOOT-14): everything the owner keeps goes (channels, zones, presets, DTMF memories, messages,
    /// the event log, waypoints, trims); never calibration or the SSB patch. A store with records starts over as
    /// on a first boot, so setup seeds it again.
    Wipe,
    /// The APRS settings changed (written back by the App: [`crate::App::aprs_cfg`]).
    AprsCfg(AprsCfg),
    /// DTMF memory i (0-15): set, or cleared with None (its name goes with it).
    Dtmf(u8, Option<Dtmf>),
    /// DTMF memory i's name, up to 10 characters (DTMF-12); empty clears it.
    DtmfName(u8, Buf<10>),
    /// A waypoint typed as a grid square, at its centre, named with it (GPS-21).
    AddWaypoint(Pos, Buf<16>),
    /// A contact added, or renamed by its callsign (APRS-37); refused when the book is full.
    SaveContact(Contact),
    DeleteContact(Buf<10>),
}

/// The records, read one at a time. Every method has a default (nothing there), so an owner that doesn't
/// exist yet costs nothing.
/// SETUP-13: the callsign APRS sends as fits an AX.25 address: 1-6 characters before the SSID (a longer one, kept
/// for CW ID, sends nothing, so beacons and messages are refused up front rather than queued and dropped).
pub fn ax25_call_ok(call: &[u8]) -> bool {
    (1..=crate::screens::transmit::AX25_CALL).contains(&call.split(|&c| c == b'-').next().map_or(0, <[u8]>::len))
}

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
    /// Event `i`'s sequence number (newest first, as [`Data::event`]): one more for each event ever logged, so the
    /// link's `LOG_READ` (LINK-42) can go on from where it was while new ones arrive. By default the count.
    fn event_seq(&self, i: u16) -> Option<u32> {
        (i < self.events()).then(|| (self.events() - 1 - i) as u32)
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
    /// The APRS settings kept with the APRS store (0 5 6 1, 0 5 6 3, 0 5 4).
    fn aprs_cfg(&self) -> AprsCfg {
        AprsCfg::DEFAULT
    }
    /// The beacon's state, for the APRS card (APRS-17, APRS-20).
    fn beacon(&self) -> Beacon {
        Beacon::default()
    }
    /// KISS frames since power-on (APRS-17's KISS card: "Sent 3 · Heard 14"): sent from the client, and heard
    /// (decoded and passed to it). RAM only.
    fn kiss_counts(&self) -> (u16, u16) {
        (0, 0)
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
    /// Quick message i as the owner wrote it (TEXT-7), kept with the APRS store; None: the factory text.
    fn quick(&self, _i: usize) -> Option<Buf<67>> {
        None
    }

    /// The contacts in callsign order (APRS-37), and the one with a callsign.
    fn contacts(&self) -> u16 {
        0
    }
    fn contact(&self, _i: u16) -> Option<Contact> {
        None
    }
    fn contact_of(&self, call: &[u8]) -> Option<Contact> {
        (0..self.contacts()).filter_map(|i| self.contact(i)).find(|c| c.call.as_bytes() == call)
    }

    fn waypoints(&self) -> u16 {
        0
    }
    fn waypoint(&self, _i: u16) -> Option<Waypoint> {
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

    /// DTMF memory i (0-15), None when empty.
    fn dtmf(&self, _i: u8) -> Option<Dtmf> {
        None
    }
    /// DTMF memory i's name (DTMF-12), empty when it has none.
    fn dtmf_name(&self, _i: u8) -> Buf<10> {
        Buf::new()
    }

    /// A change, to the record's owner.
    fn request(&self, _op: Op) -> bool {
        false
    }

    /// Writes are still queued or running (the Storage task): what the store answers may not include them yet.
    /// The terminal's `load` waits for this before each line (TERM-13).
    fn busy(&self) -> bool {
        false
    }
}

/// Nothing stored.
pub struct NoData;

impl Data for NoData {}
