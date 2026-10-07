//! The simulator's stand-ins: a fake three-slot radio that obeys the app's commands, the sample records
//! (channels, event log, APRS, GPS: `ferrite_app::sample`), and the radio-wide state for the status bar.

use ferrite_app::chan::ChannelStore;
use ferrite_app::sample::Sample;
use ferrite_app::slot::{band_of, BANDS, DEFAULT_CFG};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::msg::{BroadcastCmd, Device, GpsState, LiveAll, ModeLive, RadioCmd, SlotLive, TxLive, XbLive, SCOPE_BARS, XB_COLS};
use ferrite_core::txplan::Region;
use ferrite_core::msg::{CalState, LinkLive, SameDecoded, SelfTest};

const fn cfg(tone: u16, offset: i32) -> ferrite_app::slot::SlotCfg {
    let mut c = DEFAULT_CFG;
    c.tx_tone = tone;
    c.offset = offset;
    c
}

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
    app.set_region(Some(Region::Canada), false); // as the mock-ups have it
}

/// An app on the sample settings ([`ferrite_app::sample::settings`]: callsign, Canada, HF recent), kept for
/// the program's life as the firmware keeps its own.
pub fn app() -> ferrite_app::App {
    let set = Box::leak(Box::new(ferrite_app::sample::settings()));
    let alert = Box::leak(Box::new(ferrite_app::screens::alert::AlertInfo::NONE));
    ferrite_app::App::new(set, alert)
}

/// Channels in zones, an event log, APRS, GPS, waypoints and the scan lists: [`Sample`].
pub fn store() -> Sample {
    Sample::new()
}

/// What the fake radio's exclusive mode is doing.
#[derive(Clone, Copy, PartialEq)]
enum Fake {
    Off,
    /// The band scope: listening on `listen` (0 = sweeping) since `tl`, the bars held still meanwhile.
    Scope { centre: Freq10, step: u16, n: u8, dwell: u8, t0: f32, listen: Freq10, tl: f32 },
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
    /// The scope's bars and dwell ([`RadioCmd::ScopeCfg`]).
    scope_cfg: (u8, u8),
    /// The two BK4829s' registers, for the terminal's `bk` (TERM-21).
    bk: [[u16; 128]; 2],
    fm: u16,
    now: f32,
    /// A slot on a NOAA / Weatheradio channel since this time (s): the fake decoder hears a SAME warning there.
    wx_since: Option<f32>,
    same: Option<SameDecoded>,
}

/// The fake decoder's warning, heard 8 s after a slot settles on a weather channel (162.400-162.550 MHz).
pub const SAME_HEADER: &[u8] = b"ZCZC-WXR-SVR-024031-024033+0045-2761828-KLWX/NWS-";

impl FakeRadio {
    pub fn new() -> Self {
        let mut live = LiveAll { chips: 3, ..Default::default() };
        live.slots[0] = SlotLive { freq: mhz(146) + 52_000, rssi: -121, ..Default::default() };
        live.slots[1] = SlotLive { freq: mhz(443) + 47_500, rssi: -121, ..Default::default() };
        live.slots[2] = SlotLive { freq: mhz(118) + 95_000, rssi: -121, scanning: true, hits: 3, last_hit: mhz(121) + 50_000, ..Default::default() };
        FakeRadio { live, watch: 0b111, last_step: 0.0, mode: Fake::Off, scope_cfg: (60, 10), bk: fake_bk(), fm: 9_810, now: 0.0, wx_since: None, same: None }
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
            RadioCmd::Transmit { slot, on } => self.live.tx = TxLive { keyed: on, slot, mic: 0, refused: None },
            RadioCmd::ScanSkip { slot } => {
                let s = &mut self.live.slots[slot as usize % 3];
                s.skips = s.skips.saturating_add(1);
            }
            RadioCmd::ScopeCfg { bars, dwell } => self.scope_cfg = (bars, dwell),
            RadioCmd::Sweep { centre, step, on } => {
                let (n, dwell) = self.scope_cfg;
                self.set(if on { Fake::Scope { centre, step, n, dwell, t0: self.now, listen: 0, tl: 0.0 } } else { Fake::Off })
            }
            RadioCmd::ScopeListen { freq, .. } => {
                if let Fake::Scope { listen, tl, .. } = &mut self.mode {
                    (*listen, *tl) = (freq, self.now);
                }
            }
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

    /// A SAME header the fake decoder heard since the last call, for the app's `same_alert` (the firmware's
    /// UI task gets the same from the DSP task over a channel).
    pub fn take_same(&mut self) -> Option<SameDecoded> {
        self.same.take()
    }

    /// While a slot sits on a weather channel, a warning comes 8 s in (once per visit).
    fn same_step(&mut self, t: f32) {
        let wx = self.live.slots.iter().position(|s| (mhz(162) + 40_000..=mhz(162) + 55_000).contains(&s.freq));
        match (wx, self.wx_since) {
            (Some(_), None) => self.wx_since = Some(t),
            (Some(i), Some(t0)) if t - t0 >= 8.0 && t0 >= 0.0 => {
                if let Some(header) = ferrite_core::same::parse(SAME_HEADER) {
                    self.same = Some(SameDecoded { header, slot: i as u8, freq: self.live.slots[i].freq });
                }
                self.wx_since = Some(-1.0); // heard; not again until the slot leaves
            }
            (None, _) => self.wx_since = None,
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
            Fake::Scope { centre, step, n, dwell, t0, listen, tl } => {
                // Which chip (and relay state) a frequency needs: HF on chip 1 with the relay, VHF chip 0, UHF
                // chip 1; points needing another than the centre's are blank (SCOPE-5).
                let chip = |f: Freq10| if f < mhz(18) { 3 } else if f < mhz(64) { 2 } else if f < mhz(300) { 0 } else { 1 };
                let tt = if listen != 0 { tl } else { t };
                let n = (n as usize).clamp(1, SCOPE_BARS);
                let mut bars = [0u8; SCOPE_BARS];
                for (i, b) in bars[..n].iter_mut().enumerate() {
                    let fi = centre as i64 + (i as i64 - n as i64 / 2) * step as i64;
                    if fi <= 0 || chip(fi as Freq10) != chip(centre) {
                        continue;
                    }
                    let f = fi as f32;
                    let mut lv = noise(f / 1000.0 + tt * 0.7);
                    for (cf, l) in CARRIERS {
                        let d = (f - cf as f32).abs() / step.max(1) as f32;
                        if d < 1.5 {
                            lv = lv.max(l - d * 14.0 + (tt * 2.1 + cf as f32).sin() * 3.0);
                        }
                    }
                    *b = (lv + 160.0).clamp(1.0, 255.0) as u8;
                }
                // Sweeps a second: n points at the dwell plus about 0.3 ms each to retune.
                let rate = 1000.0 / (n as f32 * (dwell.max(1) as f32 / 10.0 + 0.3)).max(50.0);
                let open = listen != 0 && CARRIERS.iter().any(|&(cf, _)| (cf as i64 - listen as i64).abs() < 1_250);
                ModeLive::Scope { centre, bars, n: n as u8, sweeps: ((tt - t0) * rate) as u8, listen, open }
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
                let si4732 = freq < mhz(18) || matches!(mode, 3..=5); // USB, LSB, CW as Param::Mode
                ModeLive::Hf { freq, rssi: -95 + ((t * 1.3).sin() * 8.0) as i16, snr: 12, si4732, loading: if p > 100 { 255 } else { p as u8 } }
            }
            Fake::Xb { t0 } => ModeLive::Xb(xb(t - t0)),
        };
        // Transmit: speech, the mic level wandering 30-85%.
        if self.live.tx.keyed {
            self.live.tx.mic = (58.0 + (t * 5.1).sin() * 18.0 + (t * 13.7).sin() * 9.0) as u8;
        }
    }

    /// Advance to t seconds; `active` is always heard.
    pub fn step(&mut self, t: f32, active: u8) {
        let dt = t - self.last_step;
        self.last_step = t;
        self.same_step(t);
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
            s.tone_heard = s.squelch_open; // the sample's stations use the slot's own tone
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
        reduced: false,
        strip,
    }
}

/// GPS fix with 9 satellites, 7.9 V; the clock from the PC's UTC time.
#[cfg(not(target_arch = "wasm32"))]
pub fn device() -> Device {
    device_at(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

/// [`device`] with the clock at `secs` UTC (seconds since 1970).
pub fn device_at(secs: u64) -> Device {
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
        selftest: SelfTest {
            idcode: ferrite_core::chipid::EXPECTED_PID,
            mcu_ok: true,
            flash_id: 0xEF_4016,
            flash_mb: 4,
            cal: CalState::Ok,
            chips: 3,
            si4732: true,
            ssb_patch: true,
            battery_trim: 0,
        },
        link: LinkLive { cable: true, baud_100: 9216, write_pct: None },
    }
}


/// The fake BK4829s' registers: the ID in REG_00 and stock's receive-mode REG_30, the rest 0.
fn fake_bk() -> [[u16; 128]; 2] {
    let mut r = [[0u16; 128]; 2];
    for c in r.iter_mut() {
        c[0x00] = 0x4829;
        c[0x30] = 0xBFF1;
        c[0x47] = 0x6142;
    }
    r
}

impl FakeRadio {
    /// The terminal's debug requests (TERM-21), answered with made-up memory: flash and RAM read as the low byte
    /// of each address, peripherals as 0, the GPIO input registers as a pattern; BK4829 registers from the fake
    /// register file (the same refusals as the radio); the SI4732 answers its status byte 0x80 and zeros.
    pub fn debug(&mut self, r: ferrite_core::msg::DebugReq) -> ferrite_core::msg::DebugAns {
        use ferrite_core::msg::{bk_write_refused, DebugAns, DebugReq, DEBUG_MEM};
        match r {
            DebugReq::Mem { addr, n, words } => {
                let mut data = [0u8; DEBUG_MEM];
                let n = (n as usize).min(DEBUG_MEM);
                let periph = (0x4000_0000..0x4003_0000).contains(&addr) || addr >= 0xE000_0000;
                if periph && !words {
                    return DebugAns::Refused("peripheral registers: use mem32 (word reads)");
                }
                for (i, b) in data[..n].iter_mut().enumerate() {
                    let a = addr + i as u32;
                    *b = if (0x4001_0808..0x4001_1C00).contains(&(a & !3)) && (a & 0x3FF) == 0x008 { 0xA5 } else if periph { 0 } else { a as u8 };
                }
                DebugAns::Mem { addr, data, n: n as u8 }
            }
            DebugReq::BkRead { chip, reg } => DebugAns::Reg { chip, reg, val: self.bk[chip as usize % 2][reg as usize % 128] },
            DebugReq::BkWrite { chip, reg, val } => {
                if let Some(why) = bk_write_refused(reg, val) {
                    return DebugAns::Refused(why);
                }
                self.bk[chip as usize % 2][reg as usize % 128] = val;
                DebugAns::Reg { chip, reg, val }
            }
            DebugReq::Si { reply, .. } => {
                let mut data = [0u8; 16];
                data[0] = 0x80;
                DebugAns::Si { data, n: reply.clamp(1, 16) }
            }
        }
    }
}
