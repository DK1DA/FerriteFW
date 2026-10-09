//! APRS mode (manual chapter 25, APRS-13 and APRS-15 to APRS-21): slot C taken for APRS. The slot's own VFO or
//! channel stays as it was in [`crate::Slot`]; while the mode is on, what goes to the Radio task for C is the
//! APRS frequency and settings instead ([`App::tuned_freq`], [`App::tuned_cfg`]), C is watched whatever Watch
//! says, and nothing on the radio can change C (APRS-19). Turning the mode off sends C's own state again.
//!
//! The APRS settings that aren't in [`crate::settings::Settings`] (the frequency, beacon mode and power) are
//! [`AprsCfg`], kept by the APRS store's owner ([`crate::data::Data::aprs_cfg`], [`crate::data::Op::AprsCfg`]);
//! the App holds a copy for the commands it sends, loaded at the first tick and written back when it changes.
//!
//! The message retry schedule (APRS-13) and the timed beacon are here too, for the APRS task and the
//! simulator's engine ([`crate::sample::Sample::aprs_tick`]) alike.

use crate::data::{Data, Fix, Pos};
use crate::screens::lists::{age_text, dist_bearing, Txt};
use crate::slot::SlotCfg;
use crate::App;
use ferrite_core::freq::{mhz, needs_relay, Freq10};
use ferrite_core::msg::RadioCmd;
use ferrite_core::txplan::Region;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// The slot APRS mode takes.
pub const APRS_SLOT: u8 = 2;

/// APRS-4: the heard list holds this many stations, in the APRS task's RAM only (never flash: it changes every
/// few minutes on a busy frequency). The oldest heard goes when a new one comes; power off empties it.
pub const HEARD_MAX: u16 = 20;

/// The APRS settings kept with the APRS store (0 5 5 1, 0 5 5 3, 0 5 5 4, 0 5 6 1, 0 5 7 1, 0 5 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AprsCfg {
    /// The frequency in 5 kHz units; 0 = the region's (APRS-15).
    pub f5k: u16,
    /// Beacon mode: 0 off, 1 manual, then every [`BEACON_MINS`] minutes, every [`BEACON_KM10`] tenths of a
    /// kilometre, and smart ([`SMART`]). Bit 7: off, the mode kept for "beacon on" (the heard list's and the APRS
    /// card's options).
    pub beacon: u8,
    /// Beacon and message power: 0 low (APRS-20's default), 1 mid, 2 high.
    pub power: u8,
    /// The path beacons and messages ask for (0 5 6 1): an index into [`PATHS`].
    pub path: u8,
    /// The digipeater (0 5 8, APRS-12, APRS-26): bit 0 on, bit 1 our callsign as an alias too, bits 4-6 the wait
    /// before relaying in seconds (0-5).
    pub digi: u8,
    /// A beacon after each voice transmission (0 5 5 3, APRS-27).
    pub after_voice: bool,
    /// The custom path (0 5 6 2, APRS-26) as typed, like `WIDE1-1,WIDE2-2`, zero-padded; used when
    /// [`AprsCfg::path`] is [`PATH_CUSTOM`]. Checked by [`path_ok`] before it is kept.
    pub custom: [u8; CUSTOM_LEN],
}

/// The custom path's room: two entries of up to 9 characters would be 19, so two short ones (WIDE1-1,WIDE2-2).
pub const CUSTOM_LEN: usize = 16;

/// The path entries a frame asks for: up to two (APRS-26).
#[derive(Clone, Copy, Debug)]
pub struct PathNames<'a> {
    names: [&'a [u8]; 2],
    n: usize,
}

impl<'a> core::ops::Deref for PathNames<'a> {
    type Target = [&'a [u8]];
    fn deref(&self) -> &Self::Target {
        &self.names[..self.n]
    }
}

impl AprsCfg {
    pub const DEFAULT: AprsCfg = AprsCfg { f5k: 0, beacon: 0, power: 0, path: PATH_DEFAULT, digi: 0, after_voice: false, custom: [0; CUSTOM_LEN] };

    /// The path's entries: one of [`PATHS`], or the custom path split at its comma.
    pub fn path(&self) -> PathNames<'_> {
        let mut p = PathNames { names: [b"", b""], n: 0 };
        if self.path == PATH_CUSTOM {
            for e in self.custom_text().split(|&c| c == b',').filter(|e| !e.is_empty()).take(2) {
                p.names[p.n] = e;
                p.n += 1;
            }
        } else {
            for &e in PATHS[self.path as usize % PATHS.len()].1 {
                p.names[p.n] = e;
                p.n += 1;
            }
        }
        p
    }

    /// The custom path as typed (empty: none set).
    pub fn custom_text(&self) -> &[u8] {
        let n = self.custom.iter().position(|&c| c == 0).unwrap_or(CUSTOM_LEN);
        &self.custom[..n]
    }

    /// 0 5 6 1's choices: the [`PATHS`], and Custom once one is set.
    pub fn path_choices(&self) -> u8 {
        PATHS.len() as u8 + !self.custom_text().is_empty() as u8
    }

    /// 0 5 6 1's name for the path in use.
    pub fn path_name(&self) -> &[u8] {
        if self.path == PATH_CUSTOM { self.custom_text() } else { PATHS[self.path as usize % PATHS.len()].0.as_bytes() }
    }

    /// The digipeater is on (0 5 8 1).
    pub fn digi_on(&self) -> bool {
        self.digi & DIGI_ON != 0
    }

    /// The digipeater also relays frames that name our callsign (0 5 8 2).
    pub fn digi_mycall(&self) -> bool {
        self.digi & DIGI_MYCALL != 0
    }

    /// Seconds the digipeater waits before relaying (0 5 8 3).
    pub fn digi_wait_s(&self) -> u8 {
        (self.digi >> 4 & 7).min(DIGI_WAIT_MAX)
    }

    /// The beacon mode, 0 when off.
    pub fn mode(&self) -> u8 {
        if self.beacon & BEACON_PAUSED != 0 { 0 } else { self.beacon & !BEACON_PAUSED }
    }

    /// Minutes between timed beacons; None in the other modes.
    pub fn every_min(&self) -> Option<u8> {
        BEACON_MINS.get((self.mode() as usize).wrapping_sub(TIMED)).copied()
    }

    /// Metres travelled between distance beacons; None in the other modes.
    pub fn every_m(&self) -> Option<u32> {
        BEACON_KM10.get((self.mode() as usize).wrapping_sub(DISTANCE)).map(|&k| k as u32 * 100)
    }

    /// Smart beaconing (APRS-24).
    pub fn smart(&self) -> bool {
        self.mode() as usize == SMART
    }
}

/// The paths offered (0 5 6 1, APRS-26): name and entries; the custom path (0 5 6 2) is [`PATH_CUSTOM`].
pub const PATHS: [(&str, &[&[u8]]); 3] = [("None", &[]), ("WIDE1-1", &[b"WIDE1-1"]), ("WIDE1-1,WIDE2-1", &[b"WIDE1-1", b"WIDE2-1"])];
/// 0 5 6 1 on the custom path (0 5 6 2).
pub const PATH_CUSTOM: u8 = PATHS.len() as u8;

/// A custom path (0 5 6 2, APRS-26) is one or two entries split by a comma, each a callsign or alias of 1-6
/// capitals and digits with an optional -SSID of 1-15 (WIDE1-1, WIDE2-2, a club's call), and fits
/// [`CUSTOM_LEN`].
pub fn path_ok(t: &[u8]) -> bool {
    let entry = |e: &[u8]| {
        let (call, ssid) = match e.iter().position(|&c| c == b'-') {
            Some(i) => (&e[..i], Some(&e[i + 1..])),
            None => (e, None),
        };
        let call_ok = (1..=6).contains(&call.len()) && call.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        let ssid_ok = ssid.is_none_or(|s| {
            !s.is_empty() && s.len() <= 2 && s.iter().all(u8::is_ascii_digit) && s[0] != b'0' && {
                let v = s.iter().fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
                (1..=15).contains(&v)
            }
        });
        call_ok && ssid_ok
    };
    !t.is_empty() && t.len() <= CUSTOM_LEN && t.split(|&c| c == b',').count() <= 2 && t.split(|&c| c == b',').all(entry)
}
/// WIDE1-1,WIDE2-1, the usual path and the default.
pub const PATH_DEFAULT: u8 = 2;
/// [`AprsCfg::digi`]'s bits.
pub const DIGI_ON: u8 = 1;
pub const DIGI_MYCALL: u8 = 2;
pub const DIGI_WAIT_MAX: u8 = 5;
/// 0 5 8 2's values.
pub const DIGI_ALIASES: [&str; 2] = ["WIDE1-1", "WIDE1-1, my call"];

/// Bit 7 of [`AprsCfg::beacon`].
const BEACON_PAUSED: u8 = 0x80;
/// Timed beacons (0 5 5 1). APRS-9's 30 s floor is well under the shortest.
pub const BEACON_MINS: [u8; 8] = [1, 2, 5, 10, 15, 20, 30, 60];
/// Distance beacons (0 5 5 1, APRS-24), in tenths of a kilometre: 0.1 to 10 km.
pub const BEACON_KM10: [u8; 7] = [1, 2, 5, 10, 20, 50, 100];
const TIMED: usize = 2;
const DISTANCE: usize = TIMED + BEACON_MINS.len();
/// The smart mode's number, after the distance ones.
pub const SMART: usize = DISTANCE + BEACON_KM10.len();
/// Beacon on from "beacon on/off" with no mode chosen yet: every 10 minutes.
const BEACON_ON: u8 = 5;
/// Beacon modes in the menu: off, manual, timed, distance, smart.
pub const BEACON_MODES: usize = SMART + 1;
pub const POWERS: [&str; 3] = ["Low", "Mid", "High"];

/// A message is tried this often (APRS-13), the waits between tries, and after the last try how long an ack
/// may still come before it is marked failed.
pub const TRIES: u8 = 5;
pub const RETRY_S: [u16; 4] = [30, 60, 120, 240];
pub const LAST_WAIT_S: u16 = 240;

/// The longest APRS information field taken or sent (decision 86): AX.25's 256 bytes, nothing valid cut. The
/// buffers that hold a frame are already this size: `uafsk`'s HDLC frame (330 bytes: 10 addresses, control,
/// PID, 256 bytes of information, the FCS) and `uaprs`'s KISS decoder (330), so it costs no RAM; a longer frame
/// is dropped as invalid.
pub const INFO_MAX: usize = 256;

/// After try `n` (1-5): seconds until the next try, or (after the fifth) until it is marked failed.
pub fn retry_wait(n: u8) -> u16 {
    RETRY_S.get((n as usize).wrapping_sub(1)).copied().unwrap_or(LAST_WAIT_S)
}

/// The last automatic beacon, for the next one's decision: when, and the heading and position then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LastBeacon {
    pub at_s: u32,
    pub heading: u16,
    pub pos: Option<Pos>,
}

impl LastBeacon {
    pub fn new(at_s: u32, fix: Option<Fix>) -> LastBeacon {
        LastBeacon { at_s, heading: fix.map_or(0, |f| f.heading), pos: fix.map(|f| f.pos) }
    }
}

/// An automatic beacon is due (APRS-24), the first as soon as the mode allows:
/// - timed: `every` minutes since the last;
/// - distance: that far from where the last went, by the GPS; never without a fix;
/// - smart: [`uaprs::beacon`]'s SmartBeaconing with its standard car values (fast 100 km/h every 3 min, slow
///   under 5 km/h every 30 min, a turn of 28 degrees plus 255 / speed after 30 s); without a fix, the slow rate.
pub fn beacon_due(c: &AprsCfg, last: Option<LastBeacon>, now_s: u32, fix: Option<Fix>) -> bool {
    if let Some(m) = c.every_min() {
        return last.is_none_or(|l| now_s.wrapping_sub(l.at_s) >= m as u32 * 60);
    }
    if let Some(m) = c.every_m() {
        let Some(f) = fix else { return false };
        return last.is_none_or(|l| l.pos.is_none_or(|p| dist_bearing(p, f.pos).0 >= m));
    }
    if c.smart() {
        let mut b = uaprs::beacon::Beaconer::new(uaprs::beacon::SmartBeacon::default());
        if let Some(l) = last {
            b.sent(l.at_s, l.heading);
        }
        let (speed, heading) = fix.map_or((0, 0), |f| (f.speed_kmh, f.heading));
        return b.due(now_s, speed, heading);
    }
    false
}

/// The region's APRS frequency (APRS-15): US, Canada and Other 144.390, UK and Europe 144.800, Australia
/// 145.175, New Zealand 144.575, Japan 144.640 MHz. No region chosen yet: 144.390.
pub fn region_freq(r: Option<Region>) -> Freq10 {
    let k = match r {
        Some(Region::Uk | Region::Europe) => 800,
        Some(Region::Australia) => 1175,
        Some(Region::NewZealand) => 575,
        Some(Region::Japan) => 640,
        _ => 390,
    };
    mhz(144) + k * 100
}

/// The frequency's range on 0 5 7 1: chip 0's (APRS-21).
const F_MIN: Freq10 = mhz(64);
const F_MAX: Freq10 = mhz(300);

const LOADED: u8 = 1;
const DIRTY: u8 = 2;
/// A voice transmission ended (APRS-27); the next tick asks for the beacon.
const AFTER_VOICE: u8 = 4;
const TOAST_HEAD: &[u8] = b"APRS mode:";
const TOAST_BODY: &[u8] = b"off at MENU 0 5 3";

impl App {
    /// APRS mode is on (0 5 3).
    pub fn aprs_on(&self) -> bool {
        self.set.aprs_mode
    }

    /// APRS-22: without APRS mode, the active slot is on a channel flagged APRS, so what it receives is decoded
    /// (into the heard list and messages; nothing is sent: transmitting is APRS mode's).
    pub fn aprs_flag_decode(&self) -> bool {
        let s = self.slot(self.active());
        !self.set.aprs_mode && !s.vfo_mode && s.ch.is_some_and(|c| c.flags & crate::chan::FLAG_APRS != 0)
    }

    /// Slot `s` is the APRS slot now.
    pub fn aprs_slot(&self, s: u8) -> bool {
        s == APRS_SLOT && self.set.aprs_mode
    }

    pub fn aprs_cfg(&self) -> AprsCfg {
        self.aprs
    }

    /// The APRS frequency: 0 5 7 1, or the region's.
    pub fn aprs_freq(&self) -> Freq10 {
        match self.aprs.f5k {
            0 => region_freq(self.set.region()),
            k => k as Freq10 * 500,
        }
    }

    /// What the slot is tuned to: its own frequency, or the APRS frequency on the APRS slot.
    pub fn tuned_freq(&self, s: u8) -> Freq10 {
        if self.aprs_slot(s) { self.aprs_freq() } else { self.slot(s).freq() }
    }

    /// The slot's settings as the radio has them: on the APRS slot FM, no tones, simplex, busy lockout by
    /// carrier (APRS-10) and the beacon power (APRS-20); the squelch is C's own.
    pub fn tuned_cfg(&self, s: u8) -> SlotCfg {
        let own = *self.slot(s).cfg();
        if !self.aprs_slot(s) {
            return own;
        }
        SlotCfg { mode: 0, narrow: false, power: self.aprs.power % 3, busy: 1, squelch: own.squelch, ..SlotCfg::default() }
    }

    /// APRS mode on or off (0 5 3, APRS-15, APRS-16): C tunes to APRS and is watched, or goes back as it was.
    pub(crate) fn set_aprs_mode(&mut self, on: bool) {
        self.set.aprs_mode = on;
        if self.scan_mask & 1 << APRS_SLOT != 0 {
            self.scan_mask &= !(1 << APRS_SLOT);
            self.out.push(RadioCmd::Scan { slot: APRS_SLOT, on: false }); // C stops scanning to listen to APRS
        }
        self.send_slot(APRS_SLOT);
        self.out.push(RadioCmd::Watch(self.watch_mask()));
        if on {
            let mut f = Buf::<20>::new();
            let _ = uwrite!(f, "C on {}", Txt(crate::slot::mhz3(self.aprs_freq()).as_bytes()));
            self.show_toast(b"APRS mode on", f.as_bytes(), false);
        } else {
            self.show_toast(b"APRS mode off", b"slot C restored", false);
        }
    }

    /// New APRS settings: kept, written back at the next tick, and C retuned when it is the APRS slot.
    pub(crate) fn set_aprs(&mut self, c: AprsCfg) {
        self.aprs = c;
        self.aprs_st |= DIRTY;
        if self.set.aprs_mode {
            self.send_slot(APRS_SLOT);
        }
    }

    /// The custom path as typed (0 5 6 2).
    pub fn aprs_custom_path(&self) -> &[u8] {
        self.aprs.custom_text()
    }

    /// A custom path typed at 0 5 6 2, already checked by [`path_ok`]: kept and chosen (APRS-26).
    pub(crate) fn set_aprs_custom_path(&mut self, t: &[u8]) {
        let mut c = self.aprs;
        c.custom = [0; CUSTOM_LEN];
        c.custom[..t.len().min(CUSTOM_LEN)].copy_from_slice(&t[..t.len().min(CUSTOM_LEN)]);
        c.path = PATH_CUSTOM;
        self.set_aprs(c);
    }

    /// 0 5 7 1 by the knob: 5 kHz steps from where it is, within chip 0's range.
    pub(crate) fn step_aprs_freq(&mut self, d: i32) {
        let f = (self.aprs_freq() as i64 + d as i64 * 500).clamp(F_MIN as i64, F_MAX as i64) as Freq10;
        let f5k = if f == region_freq(self.set.region()) { 0 } else { (f / 500) as u16 };
        self.set_aprs(AprsCfg { f5k, ..self.aprs });
    }

    /// 0 5 5 1 by the knob.
    pub(crate) fn step_beacon(&mut self, d: i32) {
        let m = (self.aprs.mode() as i32 + d).rem_euclid(BEACON_MODES as i32) as u8;
        self.set_aprs(AprsCfg { beacon: m, ..self.aprs });
    }

    /// "Beacon on/off" (APRS-17): off keeps the mode for turning it on again; on with none chosen is every 10 min.
    pub(crate) fn toggle_beacon(&mut self) {
        let b = self.aprs.beacon;
        let beacon = if self.aprs.mode() != 0 {
            b | BEACON_PAUSED
        } else if b & 0x0F != 0 {
            b & 0x0F
        } else {
            BEACON_ON
        };
        self.set_aprs(AprsCfg { beacon, ..self.aprs });
        let mut t = Buf::<24>::new();
        self.beacon_mode_text(&mut t);
        self.show_toast(b"Beacon", t.as_bytes(), false);
    }

    /// "Off", "Manual", "Every 10 min", "Every 0.5 km", "Smart".
    pub(crate) fn beacon_mode_text<const N: usize>(&self, b: &mut Buf<N>) {
        b.clear();
        let c = self.aprs;
        match (c.mode(), c.every_min(), c.every_m()) {
            (0, ..) => b.set(b"Off"),
            (_, Some(m), _) => {
                let _ = uwrite!(b, "Every {} min", m);
            }
            (_, _, Some(m)) => {
                let _ = uwrite!(b, "Every {} km", Km(m));
            }
            _ if c.smart() => b.set(b"Smart"),
            _ => b.set(b"Manual"),
        }
    }

    /// APRS-21: the slot that shares chip 0 (64–300 MHz) with the APRS slot, so the two take turns on it: A or B
    /// when it listens there (active or watched). None when C has the chip to itself, or APRS mode is off.
    pub fn aprs_shared(&self) -> Option<u8> {
        if !self.aprs_on() || ferrite_core::freq::band(self.aprs_freq()).1 != 0 {
            return None;
        }
        let listening = self.watch_mask() | 1 << self.active;
        (0..APRS_SLOT).find(|&s| listening & 1 << s != 0 && ferrite_core::freq::band(self.tuned_freq(s)).1 == 0)
    }

    /// APRS-19: the APRS slot can't be changed; the toast says how to get it back. True when refused.
    pub(crate) fn aprs_refuse(&mut self, s: u8) -> bool {
        if !self.aprs_slot(s) {
            return false;
        }
        self.show_toast(TOAST_HEAD, TOAST_BODY, false);
        true
    }

    /// The reset: APRS settings back to their defaults (the store's went with [`crate::data::Op::Wipe`]).
    pub(crate) fn reset_aprs(&mut self) {
        (self.aprs, self.aprs_st) = (AprsCfg::DEFAULT, LOADED);
        self.send_slot(APRS_SLOT);
    }

    /// Every tick: the APRS settings from their owner once, and written back after a change.
    pub(crate) fn aprs_tick(&mut self, data: &dyn Data) {
        if self.aprs_st & LOADED == 0 {
            self.aprs_st |= LOADED;
            let c = data.aprs_cfg();
            if c != self.aprs {
                self.aprs = c;
                if self.set.aprs_mode {
                    self.send_slot(APRS_SLOT);
                }
            }
        }
        if self.aprs_st & DIRTY != 0 {
            data.request(crate::data::Op::AprsCfg(self.aprs)); // NoData (the radio until the APRS store): kept in RAM
            self.aprs_st &= !DIRTY;
        }
        if self.aprs_st & AFTER_VOICE != 0 {
            self.aprs_st &= !AFTER_VOICE;
            if self.set.aprs_mode && self.aprs.after_voice {
                data.request(crate::data::Op::AfterVoice);
            }
        }
    }

    /// A voice transmission ended (PTT released): APRS-27's beacon, asked for at the next tick.
    pub(crate) fn voice_ended(&mut self) {
        self.aprs_st |= AFTER_VOICE;
    }

    /// The APRS card's beacon line (APRS-17, APRS-20, APRS-24): "Beacon off", "Beacon manual", "Next beacon in 4 min",
    /// "Beacon every 1 km", "Smart beacon",
    /// "Beacon waiting: HF" while the active slot holds the relay in the HF state.
    pub(crate) fn beacon_text<const N: usize>(&self, data: &dyn Data, b: &mut Buf<N>) {
        b.clear();
        let st = data.beacon();
        if st.waiting && needs_relay(self.tuned_freq(self.active)) {
            b.set(b"Beacon waiting: HF");
        } else if st.waiting {
            b.set(b"Beacon waiting");
        } else {
            match (self.aprs.mode(), st.next_s) {
                (0, _) => b.set(b"Beacon off"),
                (1, _) => b.set(b"Beacon manual"),
                _ if self.aprs.smart() => b.set(b"Smart beacon"),
                (_, _) if let Some(m) = self.aprs.every_m() => {
                    let _ = uwrite!(b, "Beacon every {} km", Km(m));
                }
                (_, Some(s)) if s < 60 => {
                    let _ = uwrite!(b, "Next beacon in {} s", s);
                }
                (_, Some(s)) => {
                    let _ = uwrite!(b, "Next beacon in {} min", s.div_ceil(60));
                }
                (_, None) => b.set(b"Beacon soon"),
            }
        }
    }
}

/// Metres as kilometres with one decimal under 1 km: "0.5", "2".
struct Km(u32);

impl ufmt::uDisplay for Km {
    fn fmt<W: ufmt::uWrite + ?Sized>(&self, f: &mut ufmt::Formatter<'_, W>) -> Result<(), W::Error> {
        if self.0 < 1000 {
            uwrite!(f, "0.{}", self.0 / 100)
        } else {
            uwrite!(f, "{}", self.0 / 1000)
        }
    }
}

/// The card's "last" line: the newest of the last station heard and the last message in (APRS-17).
pub(crate) fn last_text<const N: usize>(data: &dyn Data, b: &mut Buf<N>) {
    b.clear();
    let st = data.station(0);
    let msg = (0..data.messages().min(8)).filter_map(|i| data.message(i)).find(|m| m.inbound);
    match (st, msg) {
        (_, Some(m)) if st.is_none_or(|s| m.ago_s <= s.ago_s) => {
            let _ = uwrite!(b, "{}: {}", Txt(m.peer.as_bytes()), Txt(m.text.as_bytes()));
        }
        (Some(s), _) => {
            let mut a = Buf::<12>::new();
            age_text(s.ago_s, &mut a);
            let _ = uwrite!(b, "{} \u{b7} {} ago", Txt(s.call.as_bytes()), Txt(a.as_bytes()));
        }
        _ => b.set(b"Nothing heard yet"),
    }
}

/// A weather station's report (APRS-23), read from the start of its comment, where the APRS task leaves the
/// weather data of a positional weather report ("270/007g012t072r000p000P000h65b10123", then any text). Units as
/// sent: mph, degrees F, hundredths of an inch, tenths of a millibar. Nothing is kept: it is read when the
/// station screen is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wx {
    pub dir: Option<u16>,
    pub wind: Option<u16>,
    pub gust: Option<u16>,
    pub temp_f: Option<i16>,
    /// Rain in the last hour.
    pub rain: Option<u16>,
    pub hum: Option<u8>,
    pub baro: Option<u32>,
    /// Where the comment's own text starts.
    pub rest: usize,
}

/// APRS-23: the weather fields at the start of a comment, if it starts with them (wind direction and speed).
pub fn wx(c: &[u8]) -> Option<Wx> {
    // a number of `n` characters at i: digits, a leading '-' (temperature), or dots / spaces for "no data"
    let num = |i: usize, n: usize| -> Option<Option<i32>> {
        let f = c.get(i..i + n)?;
        if f.iter().all(|&b| b == b'.' || b == b' ') {
            return Some(None);
        }
        let (neg, d) = match f.split_first() {
            Some((b'-', d)) => (true, d),
            _ => (false, f),
        };
        if d.is_empty() || !d.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let v = d.iter().fold(0i32, |a, &b| a * 10 + (b - b'0') as i32);
        Some(Some(if neg { -v } else { v }))
    };
    let mut w = Wx::default();
    // "DDD/SSS", or the complete-weather "cDDDsSSS"
    let mut i = if c.get(3) == Some(&b'/') {
        (w.dir, w.wind) = (num(0, 3)?.map(|v| v as u16), num(4, 3)?.map(|v| v as u16));
        7
    } else if c.first() == Some(&b'c') && c.get(4) == Some(&b's') {
        (w.dir, w.wind) = (num(1, 3)?.map(|v| v as u16), num(5, 3)?.map(|v| v as u16));
        8
    } else {
        return None;
    };
    // then letter-tagged fields, each a fixed width, in any order
    while let Some(&k) = c.get(i) {
        let n = match k {
            b'g' | b't' | b'r' | b'p' | b'P' | b'L' | b'l' | b's' => 3,
            b'h' => 2,
            b'b' => 5,
            _ => break,
        };
        let Some(v) = num(i + 1, n) else { break };
        match k {
            b'g' => w.gust = v.map(|v| v as u16),
            b't' => w.temp_f = v.map(|v| v as i16),
            b'r' => w.rain = v.map(|v| v as u16),
            b'h' => w.hum = v.map(|v| if v == 0 { 100 } else { v as u8 }), // h00 is 100 %
            b'b' => w.baro = v.map(|v| v as u32),
            _ => {}
        }
        i += 1 + n;
    }
    w.rest = i;
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_reports() {
        let w = wx(b"270/007g012t072r001p000P000h65b10123 Ottawa WX").unwrap();
        assert_eq!((w.dir, w.wind, w.gust, w.temp_f, w.rain, w.hum, w.baro), (Some(270), Some(7), Some(12), Some(72), Some(1), Some(65), Some(10123)));
        assert_eq!(&b"270/007g012t072r001p000P000h65b10123 Ottawa WX"[w.rest..], b" Ottawa WX");
        let w = wx(b"c...s...g...t-05h00").unwrap();
        assert_eq!((w.dir, w.wind, w.temp_f, w.hum), (None, None, Some(-5), Some(100)));
        assert!(wx(b"Mobile, on 146.520").is_none());
        assert!(wx(b"").is_none());
    }

    #[test]
    fn region_frequencies() {
        assert_eq!(region_freq(Some(Region::Us)), 14_439_000);
        assert_eq!(region_freq(Some(Region::Canada)), 14_439_000);
        assert_eq!(region_freq(Some(Region::Other)), 14_439_000);
        assert_eq!(region_freq(None), 14_439_000);
        assert_eq!(region_freq(Some(Region::Uk)), 14_480_000);
        assert_eq!(region_freq(Some(Region::Europe)), 14_480_000);
        assert_eq!(region_freq(Some(Region::Australia)), 14_517_500);
        assert_eq!(region_freq(Some(Region::NewZealand)), 14_457_500);
        assert_eq!(region_freq(Some(Region::Japan)), 14_464_000);
    }

    #[test]
    fn retry_schedule() {
        let waits: [u16; 5] = core::array::from_fn(|i| retry_wait(i as u8 + 1));
        assert_eq!(waits, [30, 60, 120, 240, LAST_WAIT_S]);
        assert_eq!(TRIES, 5);
    }

    #[test]
    fn beacon_modes() {
        let at = |t| Some(LastBeacon { at_s: t, ..LastBeacon::default() });
        let mut c = AprsCfg::DEFAULT;
        assert_eq!((c.mode(), c.every_min()), (0, None));
        assert!(!beacon_due(&c, None, 0, None));
        c.beacon = 1;
        assert_eq!((c.mode(), c.every_min()), (1, None));
        assert!(!beacon_due(&c, None, 0, None));
        c.beacon = 2 + 3; // every 10 min
        assert_eq!(c.every_min(), Some(10));
        assert!(beacon_due(&c, None, 5, None));
        assert!(!beacon_due(&c, at(100), 699, None));
        assert!(beacon_due(&c, at(100), 700, None));
        c.beacon |= BEACON_PAUSED;
        assert_eq!(c.mode(), 0);
        assert!(!beacon_due(&c, at(0), 10_000, None));
    }
}
