//! A radio's worth of sample records: channels in zones (some with positions), an event log, APRS stations
//! and messages, a GPS fix with satellites, waypoints, sensors, range presets, FM presets and HF recent, all
//! around Ottawa. The simulator and the host tests read them through the same traits the firmware's stores
//! implement ([`ChannelStore`], [`Data`]); requests ([`Op`]) change them, so deleting, skipping or marking
//! shows on screen. Host memory only: nothing here is linked into the firmware (it doesn't use it).

use crate::chan::{channel, ChanWrite, Channel, ChannelStore, Name, FLAG_SKIP, NO_ZONE};
use crate::data::*;
use crate::settings::Settings;
use crate::slot::{SlotCfg, DCS_FLAG, DEFAULT_CFG};
use core::cell::Cell;

mod aprs;
pub use aprs::AprsNews;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_ui::text::Buf;

const fn cfg(tone: u16, offset: i32) -> SlotCfg {
    let mut c = DEFAULT_CFG;
    c.tx_tone = tone;
    c.offset = offset;
    c
}

const fn mode(m: u8, narrow: bool) -> SlotCfg {
    let mut c = DEFAULT_CFG;
    c.mode = m;
    c.narrow = narrow;
    c
}

const AIR: SlotCfg = mode(2, false);
const WIDE: SlotCfg = mode(0, false);

const ZONES: [&[u8]; 8] = [b"Local", b"Repeaters", b"Empty", b"Air", b"Marine", b"Weather", b"Zone 7", b"Zone 8"];
/// Zones 0-5 exist at first; "new zone" brings the next one in.
const ZONES_AT_START: usize = 6;

/// The channels: the first eight are the look review's sample (home's mock-ups).
const CHANNELS: [Channel; 21] = [
    channel(0, 0, mhz(146) + 52_000, "Calling 2 m", DEFAULT_CFG),
    channel(1, 0, mhz(446), "Calling 70 cm", DEFAULT_CFG),
    channel(2, 0, mhz(147) + 55_500, "Simplex B", DEFAULT_CFG),
    channel(9, 1, mhz(146) + 94_000, "North hill", cfg(1000, -60_000)),
    channel(11, 1, mhz(443) + 47_500, "Club repeater", cfg(1000, 500_000)),
    channel(14, 1, mhz(444) + 20_000, "Downtown DCS", cfg(DCS_FLAG | 0o023, 500_000)),
    channel(30, 3, mhz(121) + 50_000, "Guard", AIR),
    channel(31, 3, mhz(119) + 10_000, "Tower", AIR),
    channel(16, 1, mhz(147) + 15_000, "Camp Fortune", cfg(1000, 60_000)),
    channel(17, 1, mhz(145) + 29_000, "Far east", cfg(1109, -60_000)),
    channel(32, 3, mhz(132) + 95_000, "ATIS", AIR),
    channel(40, 4, mhz(156) + 80_000, "Marine 16", WIDE),
    channel(41, 4, mhz(156) + 42_500, "Marine 68", WIDE),
    channel(42, 4, mhz(156) + 62_500, "Marine 72", WIDE),
    channel(50, 5, mhz(162) + 55_000, "WX1", WIDE),
    channel(51, 5, mhz(162) + 40_000, "WX2", WIDE),
    channel(52, 5, mhz(162) + 47_500, "WX3", WIDE),
    channel(60, NO_ZONE, mhz(145) + 80_000, "ISS downlink", DEFAULT_CFG),
    channel(61, NO_ZONE, mhz(438) + 80_000, "Hotspot", DEFAULT_CFG),
    channel(62, NO_ZONE, mhz(462) + 56_250, "FRS 1", DEFAULT_CFG),
    channel(3, 0, mhz(146) + 58_000, "Simplex C", DEFAULT_CFG),
];

/// Channels with a position (CHAN-2), by number.
const CHAN_POS: [(u16, Pos); 5] = [
    (9, Pos { lat: 455_454_922, lon: -757_283_492 }),
    (11, Pos { lat: 454_075_605, lon: -756_628_014 }),
    (14, Pos { lat: 454_178_090, lon: -757_116_483 }),
    (16, Pos { lat: 451_800_612, lon: -758_958_003 }),
    (17, Pos { lat: 453_838_771, lon: -750_845_200 }),
];

/// Where the sample radio is: Parliament Hill, Ottawa.
pub const HERE: Pos = Pos { lat: 454_215_000, lon: -756_972_000 };

const fn at(lat: i32, lon: i32) -> Option<Pos> {
    Some(Pos { lat, lon })
}

// call, symbol, position, age s, via, course and speed, altitude, times heard, comment, last message, its minute
type StationRow = (&'static str, Sym, Option<Pos>, u32, &'static str, Option<(u16, u16)>, Option<i32>, u16, &'static str, &'static str, Option<u16>);
const STATIONS: [StationRow; 11] = [
    ("VE3KPW-9", Sym::Car, at(454_467_741, -756_572_082), 120, "", Some((92, 54)), Some(87), 6, "Mobile, on 146.520", "On my way", Some(14 * 60 + 10)),
    ("VA3RTX", Sym::Home, at(454_633_076, -758_249_370), 240, "VE3RSB-1*", None, Some(102), 14, "Ron, Kanata. QRV 2 m/70 cm", "Net at 8 tonight?", Some(13 * 60 + 12)),
    ("VE3RSB-1", Sym::Digi, at(456_158_696, -755_964_076), 300, "", None, Some(212), 31, "Fill-in digi, Gatineau", "", None),
    ("VE3OBW", Sym::Wx, at(452_778_355, -757_716_987), 540, "VE3RSB-1*", None, Some(95), 9, "22C 1012 hPa wind 270/12", "", None),
    ("VA3ZZB-7", Sym::Ht, at(454_243_110, -756_744_871), 720, "", Some((10, 4)), Some(71), 3, "On foot", "", None),
    ("VE2GPA-10", Sym::IGate, at(455_383_826, -752_396_720), 1260, "WIDE2*", None, None, 22, "RX-only iGate", "", None),
    ("VE3JLH", Sym::Home, at(454_662_798, -756_436_661), 2040, "", None, Some(78), 5, "", "", None),
    ("VE3XK-9", Sym::Car, None, 3120, "", None, None, 1, "Status only", "", None),
    ("VA3MAR", Sym::Boat, at(453_671_612, -756_049_365), 3600, "VE3RSB-1*,WIDE2-1", Some((245, 13)), Some(58), 4, "Rideau canal", "", None),
    ("VE3TOS-5", Sym::Truck, at(452_277_209, -764_557_333), 4500, "VE3RSB-1,WIDE2*", Some((70, 96)), Some(120), 2, "Hwy 417 east", "", None),
    ("VA3TCJ", Sym::Home, at(453_969_925, -756_844_914), 6000, "", None, None, 8, "Tim, Glebe", "", None),
];

// source, age s, frequency, slot, unread, id, x, y, position, name, text
type EventRow = (Src, u32, Freq10, u8, bool, u32, u16, u16, Option<Pos>, &'static str, &'static str);
const EVENTS: [EventRow; 12] = [
    (Src::Aprs, 120, mhz(144) + 39_000, 2, true, 0, 0, 0, None, "VE3KPW-9", "On my way"),
    (Src::Pocsag, 380, mhz(439) + 98_750, 2, true, 1_234_567, 1200, 3, None, "", "STN 4 RESPOND MVC HWY 417 EB AT KM 120"),
    (Src::Same, 840, mhz(162) + 55_000, 2, true, 0, 0, 0, None, "", "ZCZC-WXR-SVR-035001-035003+0045-2761410-KOTT/NWS-"),
    (Src::Dsc, 1500, mhz(156) + 52_500, 2, false, 316_001_234, 0, 0, at(454_692_773, -756_789_607), "", "Routine call, ch 68"),
    (Src::Ais, 1860, mhz(161) + 97_500, 2, false, 316_123_456, 245, 72, at(454_692_773, -756_789_607), "OTTAWA PRINCESS", ""),
    (Src::Dtmf, 2400, mhz(146) + 52_000, 0, false, 0, 0, 0, None, "", "*1234#"),
    (Src::Sensor, 2820, mhz(433) + 92_000, 2, false, 2741, 0, 0, None, "Acurite 5n1", "21.4 C, 45 % RH"),
    (Src::Selcall, 3120, mhz(155) + 22_000, 1, false, 1201, 0, 0, None, "MDC1200", "PTT ID"),
    (Src::Aprs, 4800, mhz(144) + 39_000, 2, false, 0, 0, 0, None, "VA3RTX", "Net at 8 tonight?"),
    (Src::Pocsag, 9000, mhz(439) + 98_750, 2, false, 1_234_570, 512, 0, None, "", "TEST PAGE 0900"),
    (Src::Dsc, 14_400, mhz(156) + 52_500, 2, false, 3_160_099, 3, 0, at(453_100_000, -755_200_000), "", "Distress: flooding"),
    (Src::Sensor, 21_600, mhz(433) + 92_000, 2, false, 58, 0, 0, None, "LaCrosse TX141", "18.9 C"),
];

// inbound, peer, text, age s, state
type MsgRow = (bool, &'static str, &'static str, u32, MsgState);
const MESSAGES: [MsgRow; 5] = [
    (true, "VE3KPW-9", "On my way", 120, MsgState::Unread),
    (false, "VE3KPW-9", "Where are you?", 260, MsgState::Acked),
    (false, "VA3RTX", "Yes, 146.940 at 20:00", 4200, MsgState::Retrying(2)),
    (true, "VA3RTX", "Net at 8 tonight?", 4800, MsgState::Read),
    (false, "VE3JLH", "73, thanks for the QSO", 10_800, MsgState::Failed),
];

const WAYPOINTS: [(&str, Pos, WpSrc); 5] = [
    ("Home", Pos { lat: 454_164_295, lon: -756_998_294 }, WpSrc::Marked),
    ("Club shack", Pos { lat: 454_089_577, lon: -756_588_789 }, WpSrc::Link),
    ("Trailhead P2", Pos { lat: 455_076_150, lon: -758_001_499 }, WpSrc::Marked),
    ("VE3RSB-1", Pos { lat: 456_158_696, lon: -755_964_076 }, WpSrc::Aprs),
    ("Pink Lake", Pos { lat: 455_015_396, lon: -758_947_145 }, WpSrc::Marked),
];

const SENSORS: [(&str, u32, &str, i16, u32, u32); 5] = [
    ("Acurite 5n1", 2741, "21.4 C 45 %", -71, 9000, 40),
    ("LaCrosse TX141", 58, "18.9 C", -88, 21_600, 95),
    ("TPMS", 0x001F_2E33, "232 kPa", -94, 600, 300),
    ("Oregon THGR122", 0xA3, "22.1 C 38 %", -83, 7200, 610),
    ("Door contact", 0x51_7C, "closed", -79, 30_000, 2900),
];

// prn, snr, used, BeiDou
/// PRN, signal (dB-Hz), used, BeiDou, elevation, azimuth.
const SATS: [(u8, u8, bool, bool, u8, u16); 12] = [
    (2, 41, true, false, 62, 48),
    (5, 38, true, false, 41, 300),
    (6, 22, false, false, 9, 205),
    (12, 44, true, false, 78, 140),
    (13, 35, true, false, 33, 92),
    (15, 18, false, false, 6, 330),
    (19, 40, true, false, 55, 230),
    (24, 29, true, false, 21, 12),
    (25, 33, true, false, 27, 165),
    (8, 36, true, true, 47, 260),
    (11, 27, true, true, 18, 120),
    (14, 12, false, true, 4, 60),
];

const PRESETS: [(&str, Freq10, Freq10, u8, u8, bool, u8); 4] = [
    ("2 m", mhz(144), mhz(148), 10, 0, true, 3),
    ("70 cm", mhz(430), mhz(450), 10, 0, true, 0),
    ("Airband", mhz(118), mhz(137), 11, 2, false, 5),
    ("Marine", mhz(156), mhz(162) + 2_500, 11, 0, false, 1),
];

const SKIPS: [Freq10; 3] = [mhz(146) + 94_000, mhz(147) + 15_000, mhz(145) + 29_000];

const FM: [(u16, &str); 7] = [(8850, "CBC 1"), (9150, "CBC 2"), (9390, "CHEZ"), (10030, "MAJIC"), (10530, "KISS"), (10610, ""), (8930, "CKCU")];

/// HF recent, newest first: frequency and mode (as `Param::Mode`: 2 AM, 3 USB, 4 LSB, 5 CW).
const HF: [(Freq10, u8); 6] = [(707_400, 3), (1_407_400, 3), (376_000, 4), (1_013_600, 3), (703_000, 5), (607_000, 2)];

/// The sample radio's settings: the look review's callsign, Canada (as the mock-ups have it), HF recent.
pub fn settings() -> Settings {
    let mut s = Settings::DEFAULT;
    s.set_callsign(b"VA7FER");
    s.set_region(Some(ferrite_core::txplan::Region::Canada));
    s.setup_done = true;
    for (r, &(f, m)) in s.hf.recent.iter_mut().zip(&HF) {
        *r = f | (m as u32) << 28;
    }
    s
}

/// The sample owner card (SETUP-11), shown at boot (0 7 3 3): the sim's boot-owner shot and tests.
pub fn owner_card(s: &mut Settings) {
    s.owner[0].set(b"Sam Fernandez");
    s.owner[1].set(b"+1 604 555 0142");
    s.owner[2].set(b"va7fer@example.org");
    s.boot_card = true;
}

/// The sample radio's records. Cells hold what requests change.
pub struct Sample {
    /// The sample channels, then room for the ones the app saves.
    chans: [Cell<Option<Channel>>; CHANNELS.len() + 16],
    zones: Cell<u8>,
    zone_cfg: [Cell<ZoneCfg>; 8],
    zone_names: [Cell<Name>; 8],
    presets: [Cell<Option<RangePreset>>; 16],
    dtmf: [Cell<Option<Dtmf>>; DTMF_MEMS as usize],
    events_read: Cell<u32>,
    events_cleared: Cell<bool>,
    msgs_read: Cell<u32>,
    /// The APRS task's live side: heard, messages with their tries, beacons.
    aprs: aprs::AprsSim,
    draft: Cell<Draft>,
    quick: Cell<[Option<Buf<67>>; crate::words::QUICK]>,
    wp_gone: Cell<u32>,
    wp_marked: Cell<u8>,
    wp_saved: Cell<Option<u16>>,
    skips_gone: Cell<u32>,
    heard_cleared: Cell<bool>,
    version: Cell<u32>,
    /// None: no fix (GPS searching).
    pub fix: Cell<Option<Fix>>,
    pub call: Cell<Option<Buf<10>>>,
}

fn txt<const N: usize>(s: &str) -> Buf<N> {
    Buf::from(s.as_bytes())
}

impl Sample {
    pub fn new() -> Self {
        let zc = |z: usize| ZoneCfg { scan_all: z == 1, shown: z != 5, resume: if z == 3 { 11 } else { 0 }, dwell: 255, pri: [if z == 1 { Some(11) } else { None }, None], ..ZoneCfg::default() };
        Sample {
            chans: core::array::from_fn(|i| Cell::new(CHANNELS.get(i).copied())),
            zones: Cell::new(ZONES_AT_START as u8),
            zone_cfg: core::array::from_fn(|z| Cell::new(zc(z))),
            zone_names: core::array::from_fn(|z| Cell::new(Name::from(ZONES[z]))),
            presets: core::array::from_fn(|i| {
                Cell::new(PRESETS.get(i).map(|&(n, start, end, step, mode, narrow, skips)| RangePreset { name: txt(n), start, end, step, mode, narrow, skips }))
            }),
            // Two DTMF memories (chapter 17): a repeater's autopatch and a link code.
            dtmf: core::array::from_fn(|i| Cell::new([Dtmf::parse(b"*79"), Dtmf::parse(b"12345#")].get(i).copied().flatten())),
            events_read: Cell::new(0),
            events_cleared: Cell::new(false),
            msgs_read: Cell::new(0),
            aprs: aprs::AprsSim::new(),
            draft: Cell::new(Draft::default()),
            quick: Cell::new([None; crate::words::QUICK]),
            wp_gone: Cell::new(0),
            wp_marked: Cell::new(0),
            wp_saved: Cell::new(None),
            skips_gone: Cell::new(0),
            heard_cleared: Cell::new(false),
            version: Cell::new(1),
            fix: Cell::new(Some(Fix { pos: HERE, three_d: true, alt_m: 74, speed_kmh: 0, heading: 0, hdop_x10: 9, used: 9, age_s: 1, utc_s: 18 * 3600 + 32 * 60 + 5 })),
            call: Cell::new(Some(txt("VA7FER-9"))),
        }
    }

    fn live(&self) -> impl Iterator<Item = Channel> + '_ {
        self.chans.iter().filter_map(|c| c.get())
    }

    /// The i-th smallest-numbered channel among those that pass `keep`.
    fn nth(&self, i: u16, keep: impl Fn(&Channel) -> bool) -> Option<Channel> {
        let mut last: Option<u16> = None;
        let mut found = None;
        for _ in 0..=i {
            found = self.live().filter(|c| keep(c) && last.is_none_or(|l| c.num > l)).min_by_key(|c| c.num);
            last = Some(found?.num);
        }
        found
    }

    /// A channel write from the app ([`crate::App::pop_write`]), as the Storage task does it in SPI flash.
    pub fn apply(&self, w: ChanWrite) {
        match w {
            ChanWrite::Put { ch, from } => {
                for c in &self.chans {
                    if c.get().is_some_and(|c| c.num == ch.num || Some(c.num) == from) {
                        c.set(None);
                    }
                }
                if let Some(c) = self.chans.iter().find(|c| c.get().is_none()) {
                    c.set(Some(ch));
                }
            }
            ChanWrite::Delete(n) => {
                if let Some(c) = self.slot_of(n) {
                    c.set(None);
                }
            }
            ChanWrite::DeleteZone(z) => {
                for c in &self.chans {
                    if let Some(mut ch) = c.get().filter(|ch| ch.zone == z) {
                        ch.zone = NO_ZONE;
                        c.set(Some(ch));
                    }
                }
            }
            ChanWrite::EraseAll => self.chans.iter().for_each(|c| c.set(None)),
        }
        self.bump();
    }

    fn slot_of(&self, num: u16) -> Option<&Cell<Option<Channel>>> {
        self.chans.iter().find(|c| c.get().is_some_and(|c| c.num == num))
    }

    fn bump(&self) -> bool {
        self.version.set(self.version.get().wrapping_add(1));
        true
    }

    fn marked(&self) -> u16 {
        self.wp_marked.get() as u16 + self.wp_saved.get().is_some() as u16
    }

    fn sent_n(&self) -> u16 {
        self.aprs.live_n()
    }

    /// A second of the simulated APRS task ([`aprs`]): APRS mode on or off, and whether C can transmit with the
    /// band relay where the active slot holds it. The caller notifies the arrivals and shows the transmissions.
    pub fn aprs_tick(&self, now_s: u32, on: bool, relay_ok: bool) -> AprsNews {
        let known = |c: &[u8]| STATIONS.iter().any(|s| s.0.as_bytes() == c) || self.aprs.heard(c);
        let n = self.aprs.tick(now_s, on, relay_ok, &known);
        if n.changed {
            self.bump();
        }
        n
    }
}

impl ChannelStore for Sample {
    fn zones(&self) -> u8 {
        self.zones.get()
    }
    fn zone_name(&self, zone: u8) -> Option<Name> {
        (zone < self.zones.get()).then(|| self.zone_names[zone as usize].get())
    }
    fn zone_len(&self, zone: u8) -> u16 {
        self.live().filter(|c| c.zone == zone).count() as u16
    }
    fn zone_at(&self, zone: u8, i: u16) -> Option<Channel> {
        self.nth(i, |c| c.zone == zone)
    }
    fn count(&self) -> u16 {
        self.live().count() as u16
    }
    fn by_number(&self, i: u16) -> Option<Channel> {
        self.nth(i, |_| true)
    }
    fn position(&self, num: u16) -> Option<Pos> {
        CHAN_POS.iter().find(|p| p.0 == num).map(|p| p.1)
    }
    fn data(&self) -> &dyn Data {
        self
    }
}

impl Data for Sample {
    fn version(&self) -> u32 {
        self.version.get()
    }

    fn events(&self) -> u16 {
        if self.events_cleared.get() { 0 } else { EVENTS.len() as u16 }
    }
    fn event(&self, i: u16) -> Option<Event> {
        if i >= self.events() {
            return None;
        }
        let (src, ago_s, freq, slot, unread, id, x, y, pos, name, text) = EVENTS[i as usize];
        let unread = unread && self.events_read.get() & 1 << i == 0;
        Some(Event { src, ago_s, freq, slot, unread, id, x, y, pos, name: txt(name), text: txt(text) })
    }

    fn stations(&self) -> u16 {
        self.aprs.stations() + if self.heard_cleared.get() { 0 } else { STATIONS.len() as u16 }
    }
    fn station(&self, i: u16) -> Option<Station> {
        if i >= self.stations() {
            return None;
        }
        let Some(k) = i.checked_sub(self.aprs.stations()) else { return self.aprs.station(i) };
        let (call, sym, pos, ago_s, via, course, alt_m, times, comment, msg, msg_min) = STATIONS[k as usize];
        let ago_s = ago_s + self.aprs.now_s();
        Some(Station { call: txt(call), sym, pos, ago_s, via: txt(via), course, alt_m, times, comment: txt(comment), msg: txt(msg), msg_min })
    }
    fn callsign(&self) -> Option<Buf<10>> {
        self.call.get()
    }

    fn messages(&self) -> u16 {
        MESSAGES.len() as u16 + self.sent_n()
    }
    fn message(&self, i: u16) -> Option<Message> {
        if i < self.sent_n() {
            return self.aprs.message(i);
        }
        let k = i - self.sent_n();
        let &(inbound, peer, text, ago_s, state) = MESSAGES.get(k as usize)?;
        let ago_s = ago_s + self.aprs.now_s();
        let state = if state == MsgState::Unread && self.msgs_read.get() & 1 << k != 0 { MsgState::Read } else { state };
        Some(Message { inbound, peer: txt(peer), text: txt(text), ago_s, state })
    }
    fn draft(&self) -> Draft {
        self.draft.get()
    }
    fn quick(&self, i: usize) -> Option<Buf<67>> {
        self.quick.get().get(i).copied().flatten()
    }
    fn aprs_cfg(&self) -> AprsCfg {
        self.aprs.cfg.get()
    }
    fn beacon(&self) -> Beacon {
        self.aprs.beacon()
    }

    fn waypoints(&self) -> u16 {
        (0..WAYPOINTS.len() as u16 + self.marked()).filter(|&i| self.wp_gone.get() & 1 << i == 0).count() as u16
    }
    fn waypoint(&self, i: u16) -> Option<Waypoint> {
        // Marked ones first (newest first), then the stored ones; deleted ones skipped.
        let k = (0..WAYPOINTS.len() as u16 + self.marked()).filter(|&k| self.wp_gone.get() & 1 << k == 0).nth(i as usize)?;
        let fix = self.fix.get().map_or(HERE, |f| f.pos);
        let m = self.marked();
        if k < m {
            if let (0, Some(s)) = (k, self.wp_saved.get()) {
                let st = self.station(s)?;
                return Some(Waypoint { name: Buf::from(st.call.as_bytes()), pos: st.pos?, src: WpSrc::Aprs });
            }
            let mut name = Buf::<16>::new();
            let _ = ufmt::uwrite!(name, "Marked {}", m - k);
            return Some(Waypoint { name, pos: fix, src: WpSrc::Marked });
        }
        let (n, pos, src) = WAYPOINTS[(k - m) as usize];
        Some(Waypoint { name: txt(n), pos, src })
    }

    fn sensors(&self) -> u16 {
        SENSORS.len() as u16
    }
    fn sensor(&self, i: u16) -> Option<Sensor> {
        let &(m, id, v, rssi, first, last) = SENSORS.get(i as usize)?;
        Some(Sensor { model: txt(m), id, value: txt(v), rssi, first_ago_s: first, last_ago_s: last })
    }
    fn noise(&self) -> Option<Noise> {
        Some(Noise { floor: -112, baseline: -115, alarm_db: 10, jamming: false })
    }

    fn fix(&self) -> Option<Fix> {
        self.fix.get()
    }
    fn sats(&self) -> u8 {
        SATS.len() as u8
    }
    fn sat(&self, i: u8) -> Option<Sat> {
        let &(prn, snr, used, beidou, elev, azim) = SATS.get(i as usize)?;
        Some(Sat { prn, snr, used, beidou, elev, azim })
    }

    fn presets(&self) -> u8 {
        self.presets.iter().filter(|p| p.get().is_some()).count() as u8
    }
    fn preset(&self, i: u8) -> Option<RangePreset> {
        self.presets.iter().filter_map(|p| p.get()).nth(i as usize)
    }
    fn skips(&self) -> u16 {
        (0..SKIPS.len()).filter(|&i| self.skips_gone.get() & 1 << i == 0).count() as u16
    }
    fn skip(&self, i: u16) -> Option<Freq10> {
        (0..SKIPS.len()).filter(|&k| self.skips_gone.get() & 1 << k == 0).nth(i as usize).map(|k| SKIPS[k])
    }
    fn zone_cfg(&self, zone: u8) -> ZoneCfg {
        self.zone_cfg.get(zone as usize).map_or_else(ZoneCfg::default, |c| c.get())
    }

    fn dtmf(&self, i: u8) -> Option<Dtmf> {
        self.dtmf.get(i as usize)?.get()
    }

    fn fm_presets(&self) -> u8 {
        FM.len() as u8
    }
    fn fm_preset(&self, i: u8) -> Option<FmPreset> {
        FM.get(i as usize).map(|&(freq, n)| FmPreset { freq, name: txt(n) })
    }

    fn request(&self, op: Op) -> bool {
        let edit = |num: u16, f: &dyn Fn(&mut Channel)| match self.slot_of(num) {
            Some(cell) => {
                let mut c = cell.get();
                if let Some(c) = &mut c {
                    f(c);
                }
                cell.set(c);
                true
            }
            None => false,
        };
        let ok = match op {
            Op::Read(i) => {
                self.events_read.set(self.events_read.get() | 1 << i);
                true
            }
            Op::ReadAll => {
                self.events_read.set(u32::MAX);
                true
            }
            Op::ClearLog => {
                self.events_cleared.set(true);
                true
            }
            Op::DeleteChannel(n) => self.slot_of(n).map(|c| c.set(None)).is_some(),
            Op::Skip(n, on) => edit(n, &|c| c.flags = if on { c.flags | FLAG_SKIP } else { c.flags & !FLAG_SKIP }),
            Op::Hide(n, on) => edit(n, &|c| c.flags = if on { c.flags | crate::chan::FLAG_HIDDEN } else { c.flags & !crate::chan::FLAG_HIDDEN }),
            Op::Move(n, z) => edit(n, &|c| c.zone = z),
            Op::Renumber(n, to) => self.slot_of(to).is_none() && edit(n, &|c| c.num = to),
            Op::NewZone => {
                let z = self.zones.get();
                (z < ZONES.len() as u8).then(|| self.zones.set(z + 1)).is_some()
            }
            Op::DeleteZone(z) => {
                for c in &self.chans {
                    if let Some(mut ch) = c.get().filter(|ch| ch.zone == z) {
                        ch.zone = NO_ZONE;
                        c.set(Some(ch));
                    }
                }
                if z + 1 == self.zones.get() {
                    self.zones.set(z);
                }
                true
            }
            Op::Zone(z, cfg) => self.zone_cfg.get(z as usize).map(|c| c.set(cfg)).is_some(),
            Op::RenameZone(z, name) => (z < self.zones.get()).then(|| self.zone_names[z as usize].set(name)).is_some(),
            Op::Preset(i, p) => match self.presets.iter().filter(|p| p.get().is_some()).nth(i as usize) {
                Some(c) => {
                    c.set(Some(p));
                    true
                }
                None => false,
            },
            Op::Dtmf(i, d) => self.dtmf.get(i as usize).map(|c| c.set(d)).is_some(),
            Op::NewPreset => match self.presets.iter().find(|p| p.get().is_none()) {
                Some(c) => {
                    c.set(Some(RangePreset { name: txt("New range"), start: mhz(144), end: mhz(148), step: 10, mode: 0, narrow: true, skips: 0 }));
                    true
                }
                None => false,
            },
            Op::DeletePreset(i) => self.presets.iter().filter(|p| p.get().is_some()).nth(i as usize).map(|c| c.set(None)).is_some(),
            Op::PutZone(z, name, cfg) => {
                let n = self.zones.get();
                if z > n || z as usize >= ZONES.len() {
                    return false;
                }
                if z == n {
                    self.zones.set(n + 1);
                }
                self.zone_names[z as usize].set(name);
                self.zone_cfg[z as usize].set(cfg);
                true
            }
            Op::PutPreset(i, p) => {
                let used = self.presets.iter().filter(|p| p.get().is_some()).count();
                let cell = match self.presets.iter().filter(|p| p.get().is_some()).nth(i as usize) {
                    Some(c) => Some(c),
                    None if i as usize == used => self.presets.iter().find(|p| p.get().is_none()),
                    None => None,
                };
                match cell {
                    Some(c) => {
                        let skips = c.get().map_or(0, |o| o.skips);
                        c.set(Some(RangePreset { skips, ..p }));
                        true
                    }
                    None => false,
                }
            }
            Op::ClearSkips => {
                self.skips_gone.set(u32::MAX);
                true
            }
            Op::Unskip(i) => match (0..SKIPS.len()).filter(|&k| self.skips_gone.get() & 1 << k == 0).nth(i as usize) {
                Some(k) => {
                    self.skips_gone.set(self.skips_gone.get() | 1 << k);
                    true
                }
                None => false,
            },
            Op::MarkHere => {
                // Shifts the stored ones' deleted bits along with them.
                let ok = self.fix.get().is_some() && self.wp_saved.get().is_none() && self.wp_marked.get() < 9;
                if ok {
                    self.wp_marked.set(self.wp_marked.get() + 1);
                    self.wp_gone.set(self.wp_gone.get() << 1);
                }
                ok
            }
            Op::SaveStation(i) => {
                let ok = self.station(i).is_some_and(|s| s.pos.is_some()) && self.marked() == 0;
                if ok {
                    self.wp_saved.set(Some(i));
                    self.wp_gone.set(self.wp_gone.get() << 1);
                }
                ok
            }
            Op::DeleteWaypoint(i) => {
                match (0..WAYPOINTS.len() as u16 + self.marked()).filter(|&k| self.wp_gone.get() & 1 << k == 0).nth(i as usize) {
                    Some(k) => {
                        self.wp_gone.set(self.wp_gone.get() | 1 << k);
                        true
                    }
                    None => false,
                }
            }
            Op::Draft(d) => {
                self.draft.set(d);
                true
            }
            Op::Quick(i, t) => {
                let mut q = self.quick.get();
                match q.get_mut(i as usize) {
                    Some(s) => {
                        *s = t;
                        self.quick.set(q);
                        true
                    }
                    None => false,
                }
            }
            Op::Send => {
                let d = self.draft.get();
                let ok = self.call.get().is_some() && !d.to.is_empty() && !d.text.is_empty(); // APRS-8
                if ok {
                    self.aprs.send(d.to.as_bytes(), d.text);
                    self.draft.set(Draft::default());
                }
                ok
            }
            Op::MessageRead(i) => match i.checked_sub(self.sent_n()) {
                Some(k) => {
                    self.msgs_read.set(self.msgs_read.get() | 1 << k);
                    true
                }
                None => {
                    self.aprs.read(i);
                    true
                }
            },
            Op::BeaconNow => {
                let ok = self.call.get().is_some(); // APRS-8
                if ok {
                    self.aprs.beacon_now();
                }
                ok
            }
            Op::ClearHeard => {
                self.heard_cleared.set(true);
                self.aprs.clear_heard();
                true
            }
            Op::AprsCfg(c) => {
                self.aprs.cfg.set(c);
                true
            }
        };
        ok && self.bump()
    }
}

