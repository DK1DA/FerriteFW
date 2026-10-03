//! The simulator's stand-ins: a fake three-slot radio that obeys the app's commands, a few channels, and the
//! radio-wide state for the status bar.

use ferrite_app::chan::{channel, ChanWrite, Channel, ChannelStore, NO_ZONE};
use std::cell::RefCell;
use ferrite_app::slot::{band_of, BANDS, DCS_FLAG, DEFAULT_CFG};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::msg::{BroadcastCmd, Device, GpsState, LiveAll, ModeLive, RadioCmd, SlotLive, TxLive, XbLive, SCOPE_BARS, XB_COLS};
use ferrite_core::txplan::Region;

const fn cfg(tone: u16, offset: i32) -> ferrite_app::slot::SlotCfg {
    let mut c = DEFAULT_CFG;
    c.tx_tone = tone;
    c.offset = offset;
    c
}

const AIR: ferrite_app::slot::SlotCfg = {
    let mut c = DEFAULT_CFG;
    c.mode = 2;
    c
};

pub const ZONES: [&[u8]; 4] = [b"Local", b"Repeaters", b"Empty", b"Air"];
pub static CHANNELS: [Channel; 8] = [
    channel(0, 0, mhz(146) + 52_000, "Calling 2 m", DEFAULT_CFG),
    channel(1, 0, mhz(446), "Calling 70 cm", DEFAULT_CFG),
    channel(2, 0, mhz(147) + 55_500, "Simplex B", DEFAULT_CFG),
    channel(9, 1, mhz(146) + 94_000, "North hill", cfg(1000, -60_000)),
    channel(11, 1, mhz(443) + 47_500, "Club repeater", cfg(1000, 500_000)),
    channel(14, 1, mhz(444) + 20_000, "Downtown DCS", cfg(DCS_FLAG | 0o023, 500_000)),
    channel(30, 3, mhz(121) + 50_000, "Guard", AIR),
    channel(31, 3, mhz(119) + 10_000, "Tower", AIR),
];

/// The look review's sample: A a VFO with tone and shift, B on "Club repeater", C scanning airband, watch
/// A+B+C. The app's start-up commands go to the radio first (they tune C, which would stop its scan).
pub fn sample(app: &mut ferrite_app::App, store: &impl ChannelStore, radio: &mut FakeRadio) {
    sample_app(app, store);
    while let Some(c) = app.pop_cmd() {
        radio.cmd(c);
    }
    radio.cmd(RadioCmd::Scan { slot: 2, on: true });
    radio.live.slots[2].hits = 3;
}

/// The sample's app side alone (the slots, watch, region and callsign), to put back between screenshots.
pub fn sample_app(app: &mut ferrite_app::App, store: &impl ChannelStore) {
    app.set_vfo_cfg(0, cfg(1000, -60_000));
    app.set_channel(1, 1, 1, store);
    app.set_watch(2);
    app.set_region(Some(Region::Canada), false); // as the mock-ups have it, with a callsign
    app.set_callsign(b"VA3ABC");
}

pub fn store() -> RamStore {
    let mut v = CHANNELS.to_vec();
    v.sort_by_key(|c| c.num);
    RamStore { zones: &ZONES, channels: RefCell::new(v) }
}

/// The simulator's channels: the sample table, kept in RAM so the app's writes ([`ChanWrite`]) take effect, as
/// the Storage task's will in SPI flash.
pub struct RamStore {
    zones: &'static [&'static [u8]],
    channels: RefCell<Vec<Channel>>,
}

impl RamStore {
    pub fn apply(&self, w: ChanWrite) {
        let mut v = self.channels.borrow_mut();
        match w {
            ChanWrite::Put { ch, from } => {
                v.retain(|c| c.num != ch.num && Some(c.num) != from);
                v.push(ch);
                v.sort_by_key(|c| c.num);
            }
            ChanWrite::Delete(n) => v.retain(|c| c.num != n),
            ChanWrite::DeleteZone(z) => v.iter_mut().filter(|c| c.zone == z).for_each(|c| c.zone = NO_ZONE),
            ChanWrite::EraseAll => v.clear(),
        }
    }
}

impl ChannelStore for RamStore {
    fn zones(&self) -> u8 {
        self.zones.len() as u8
    }
    fn zone_name(&self, zone: u8) -> Option<&[u8]> {
        self.zones.get(zone as usize).copied()
    }
    fn zone_len(&self, zone: u8) -> u16 {
        self.channels.borrow().iter().filter(|c| c.zone == zone).count() as u16
    }
    fn zone_at(&self, zone: u8, i: u16) -> Option<Channel> {
        self.channels.borrow().iter().filter(|c| c.zone == zone).nth(i as usize).copied()
    }
    fn count(&self) -> u16 {
        self.channels.borrow().len() as u16
    }
}

/// What the fake radio's exclusive mode is doing.
#[derive(Clone, Copy, PartialEq)]
enum Fake {
    Off,
    Scope { centre: Freq10, step: u16, t0: f32 },
    Tone { t0: f32 },
    Counter { t0: f32 },
    Fm,
    Hf { t0: f32, freq: Freq10, mode: u8 },
    Xb { t0: f32 },
}

/// Carriers the scope and counter "hear": frequency and level (dBm).
const CARRIERS: [(Freq10, f32); 5] =
    [(mhz(146) + 83_500, -60.0), (mhz(146) + 61_000, -88.0), (mhz(146) + 88_000, -92.0), (mhz(146) + 52_000, -100.0), (mhz(443) + 47_500, -80.0)];
/// FM stations for seek: frequency (10 kHz), RDS name and text.
const STATIONS: [(u16, &str, &str); 4] = [
    (8_890, "CBC R2", "Tempo with Julie Nesrallah"),
    (9_150, "CBC R1", "Ottawa Morning: traffic and weather on the hour"),
    (9_810, "JAZZ FM", "Now playing: Take Five, Dave Brubeck Quartet"),
    (10_650, "KISS", "The Morning Show"),
];

/// Three slots: A has a signal 3 s in every 10, B one for 2 s in every 25 (a watched slot opening), C scans.
pub struct FakeRadio {
    pub live: LiveAll,
    watch: u8,
    last_step: f32,
    mode: Fake,
    fm: u16,
    now: f32,
}

impl FakeRadio {
    pub fn new() -> Self {
        let mut live = LiveAll { chips: 3, ..Default::default() };
        live.slots[0] = SlotLive { freq: mhz(146) + 52_000, rssi: -121, ..Default::default() };
        live.slots[1] = SlotLive { freq: mhz(443) + 47_500, rssi: -121, ..Default::default() };
        live.slots[2] = SlotLive { freq: mhz(118) + 95_000, rssi: -121, scanning: true, hits: 3, last_hit: mhz(121) + 50_000, ..Default::default() };
        FakeRadio { live, watch: 0b111, last_step: 0.0, mode: Fake::Off, fm: 9_810, now: 0.0 }
    }

    pub fn cmd(&mut self, c: RadioCmd) {
        match c {
            RadioCmd::TuneSlot { slot, freq } => {
                let s = &mut self.live.slots[slot as usize % 3];
                s.freq = freq;
                s.scanning = false;
            }
            RadioCmd::Scan { slot, on } => {
                let s = &mut self.live.slots[slot as usize % 3];
                s.scanning = on;
                if on {
                    s.hits = 0;
                    s.skips = 0;
                }
            }
            RadioCmd::Watch(m) => self.watch = m,
            RadioCmd::Transmit { slot, on } => self.live.tx = TxLive { keyed: on, slot, drive: 0, refused: None },
            RadioCmd::ScanSkip { slot } => {
                let s = &mut self.live.slots[slot as usize % 3];
                s.skips = s.skips.saturating_add(1);
            }
            RadioCmd::Sweep { centre, step, on } => self.set(if on { Fake::Scope { centre, step, t0: self.now } } else { Fake::Off }),
            RadioCmd::ToneSearch { on, .. } => self.set(if on { Fake::Tone { t0: self.now } } else { Fake::Off }),
            RadioCmd::Counter(on) => self.set(if on { Fake::Counter { t0: self.now } } else { Fake::Off }),
            RadioCmd::Broadcast(BroadcastCmd::Fm(f)) => {
                self.fm = f;
                self.set(Fake::Fm);
            }
            RadioCmd::Broadcast(BroadcastCmd::Seek { up }) => {
                let next = if up {
                    STATIONS.iter().find(|s| s.0 > self.fm).unwrap_or(&STATIONS[0])
                } else {
                    STATIONS.iter().rev().find(|s| s.0 < self.fm).unwrap_or(&STATIONS[STATIONS.len() - 1])
                };
                self.fm = next.0;
            }
            RadioCmd::Broadcast(_) => self.set(Fake::Off),
            RadioCmd::Hf { freq, mode, on } => {
                let t0 = match self.mode {
                    Fake::Hf { t0, .. } => t0,
                    _ => self.now,
                };
                self.set(if on { Fake::Hf { t0, freq, mode } } else { Fake::Off })
            }
            RadioCmd::Crossband(c) => self.set(if c.is_some() { Fake::Xb { t0: self.now } } else { Fake::Off }),
            _ => {}
        }
    }

    fn set(&mut self, m: Fake) {
        if !matches!((self.mode, m), (Fake::Hf { .. }, Fake::Hf { .. })) {
            self.live.mode = ModeLive::Off;
        }
        self.mode = m;
    }

    /// The exclusive mode's report at t seconds.
    fn mode_step(&mut self, t: f32) {
        let noise = |x: f32| -122.0 + (x * 12.9).sin() * 3.0 + (x * 3.3).cos() * 2.0;
        self.live.mode = match self.mode {
            Fake::Off => ModeLive::Off,
            Fake::Scope { centre, step, t0 } => {
                let mut bars = [0u8; SCOPE_BARS];
                for (i, b) in bars.iter_mut().enumerate() {
                    let f = centre as f32 + (i as f32 - SCOPE_BARS as f32 / 2.0) * step as f32;
                    let mut lv = noise(f / 1000.0 + t * 0.7);
                    for (cf, l) in CARRIERS {
                        let d = (f - cf as f32).abs() / step.max(1) as f32;
                        if d < 1.5 {
                            lv = lv.max(l - d * 14.0 + (t * 2.1 + cf as f32).sin() * 3.0);
                        }
                    }
                    *b = (lv + 160.0).clamp(1.0, 255.0) as u8;
                }
                ModeLive::Scope { centre, bars, sweeps: ((t - t0) * 3.0) as u8, rate: 3 }
            }
            Fake::Tone { t0 } => ModeLive::Probe { freq: 0, rssi: -84, tone: if t - t0 > 2.5 { 1000 } else { 0 } },
            Fake::Counter { t0 } => {
                let p = (t - t0) % 6.0;
                let (freq, rssi) = if p < 1.0 { (0, -127) } else if p < 3.5 { (mhz(147) + 15_000, -42) } else { (mhz(146) + 94_000, -58) };
                ModeLive::Probe { freq, rssi, tone: 0 }
            }
            Fake::Fm => {
                let st = STATIONS.iter().find(|s| s.0 == self.fm);
                let mut ps = [0u8; 8];
                let mut rt = [0u8; 32];
                if let Some(s) = st {
                    ps[..s.1.len()].copy_from_slice(s.1.as_bytes());
                    let n = s.2.len().min(32);
                    rt[..n].copy_from_slice(&s.2.as_bytes()[..n]);
                }
                let on = st.is_some();
                ModeLive::Fm { freq: self.fm, ps, rt, rssi: if on { 42 } else { 8 }, snr: if on { 18 } else { 2 }, stereo: on }
            }
            Fake::Hf { t0, freq, mode } => {
                let p = ((t - t0) / 2.0 * 100.0) as u32;
                let si4732 = freq < mhz(18) || matches!(mode, 1..=3);
                ModeLive::Hf { freq, rssi: -95 + ((t * 1.3).sin() * 8.0) as i16, snr: 12, si4732, loading: if p > 100 { 255 } else { p as u8 } }
            }
            Fake::Xb { t0 } => ModeLive::Xb(xb(t - t0)),
        };
        // Transmit: the PA ramps to its calibrated drive.
        if self.live.tx.keyed {
            self.live.tx.drive = (self.live.tx.drive + 3).min(45);
        }
    }

    /// Advance to t seconds; `active` is always heard.
    pub fn step(&mut self, t: f32, active: u8) {
        let dt = t - self.last_step;
        self.last_step = t;
        self.now = t;
        self.mode_step(t);
        for (i, s) in self.live.slots.iter_mut().enumerate() {
            let heard = i as u8 == active || self.watch & 1 << i != 0;
            let (open, level) = if s.scanning {
                scan(s, t, dt)
            } else {
                match i {
                    0 => ((t % 10.0) > 6.0 && (t % 10.0) < 9.0, -85.0 + (t * 1.7).sin() * 6.0),
                    1 => ((t % 25.0) > 20.0 && (t % 25.0) < 22.0, -97.0 + (t * 2.3).sin() * 4.0),
                    _ => (false, 0.0),
                }
            };
            s.squelch_open = open && heard;
            s.rssi = if s.squelch_open { level as i16 } else { -121 + ((t * 0.9 + i as f32).sin() * 2.0) as i16 };
        }
    }
}

/// Scanning: a step every 0.1 s across the slot's band, stopping 2 s on a hit every 7 s.
fn scan(s: &mut SlotLive, t: f32, dt: f32) -> (bool, f32) {
    let b = &BANDS[band_of(s.freq).unwrap_or(2)];
    let phase = t % 7.0;
    if phase < 2.0 {
        if (t - dt) % 7.0 >= 2.0 {
            s.hits = s.hits.saturating_add(1);
            s.last_hit = s.freq;
        }
        return (true, -79.0 + (t * 3.1).sin() * 5.0);
    }
    if dt > 0.0 && (t * 10.0) as u32 != ((t - dt) * 10.0) as u32 {
        let step: Freq10 = 2_500;
        s.freq = if s.freq + step >= b.hi { b.lo } else { s.freq + step };
    }
    (false, 0.0)
}

/// The crossband repeater, `s` seconds after it started (shown as 1 h 12 m in): a pass side 1 -> 2 every
/// 30 s for 12 s, one 2 -> 1 every 45 s for 8 s; the strip a fixed past plus what this run did.
fn xb(s: f32) -> XbLive {
    const PAST: [i8; XB_COLS] = [
        0, 0, 0, 0, 0, 12, 22, 25, 20, 0, -8, -14, 0, 0, 0, 0, 16, 24, 22, 18, 12, 0, -20, -26, -18, -10, 0, 0, 0, 0,
        0, 0, 14, 20, 0, 0, 0, 0, 18, 22, 25, 24, 20, 16, 0, 0, 0, -12, -22, -16, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0,
    ];
    let a = s % 30.0;
    let b = s % 45.0;
    let (state, pass) = if a < 12.0 { (1, a) } else if b > 20.0 && b < 28.0 { (2, b - 20.0) } else { (0, 0.0) };
    let mut strip = PAST;
    let cols = (s / 30.0) as usize;
    strip.rotate_left(cols.min(XB_COLS));
    if state == 1 {
        strip[XB_COLS - 1] = (pass as i8).max(3);
    } else if state == 2 {
        strip[XB_COLS - 1] = -(pass as i8).max(3);
    }
    let run = 72 * 60 + s as u32;
    XbLive {
        state,
        run_s: run,
        pass_s: if state != 0 { pass as u16 } else { 18 },
        idle_s: if state == 0 { (a - 12.0).max(0.0) as u16 + 110 } else { 0 },
        last_dir: 2,
        passes: 23 + (s / 30.0) as u16,
        busy_s: 9 * 60 + (s / 3.0) as u16,
        duty: if state != 0 { 34 } else { 8 },
        next_id_s: 240 - (s as u16 % 240),
        rssi: -79,
        drive: if state != 0 { 30 } else { 0 },
        strip,
    }
}

/// GPS fix with 9 satellites, 7.9 V; the clock from the PC's UTC time.
pub fn device() -> Device {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Device {
        clock_min: Some(((secs % 86_400) / 60) as u16),
        utc: true,
        gps: GpsState::Fix,
        sats: 9,
        bt: false,
        battery_mv: 7900,
        battery_pct: 82,
        battery_low: false,
        battery_cutoff: false,
        tx_built: true, // the fake radio transmits (into nothing)
    }
}
