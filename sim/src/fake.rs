//! The simulator's stand-ins: a fake three-slot radio that obeys the app's commands, a few channels, and the
//! radio-wide state for the status bar.

use ferrite_app::chan::{channel, ChanWrite, Channel, ChannelStore, NO_ZONE};
use std::cell::RefCell;
use ferrite_app::slot::{band_of, BANDS, DCS_FLAG, DEFAULT_CFG};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::msg::{Device, GpsState, LiveAll, RadioCmd, SlotLive};

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
    app.set_vfo_cfg(0, cfg(1000, -60_000));
    app.set_channel(1, 1, 1, store);
    app.set_watch(2);
    while let Some(c) = app.pop_cmd() {
        radio.cmd(c);
    }
    radio.cmd(RadioCmd::Scan { slot: 2, on: true });
    radio.live.slots[2].hits = 3;
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

/// Three slots: A has a signal 3 s in every 10, B one for 2 s in every 25 (a watched slot opening), C scans.
pub struct FakeRadio {
    pub live: LiveAll,
    watch: u8,
    last_step: f32,
}

impl FakeRadio {
    pub fn new() -> Self {
        let mut live = LiveAll { chips: 3, ..Default::default() };
        live.slots[0] = SlotLive { freq: mhz(146) + 52_000, rssi: -121, ..Default::default() };
        live.slots[1] = SlotLive { freq: mhz(443) + 47_500, rssi: -121, ..Default::default() };
        live.slots[2] = SlotLive { freq: mhz(118) + 95_000, rssi: -121, scanning: true, hits: 3, last_hit: mhz(121) + 50_000, ..Default::default() };
        FakeRadio { live, watch: 0b111, last_step: 0.0 }
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
                }
            }
            RadioCmd::Watch(m) => self.watch = m,
            _ => {}
        }
    }

    /// Advance to t seconds; `active` is always heard.
    pub fn step(&mut self, t: f32, active: u8) {
        let dt = t - self.last_step;
        self.last_step = t;
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
        tx_built: false,
    }
}
