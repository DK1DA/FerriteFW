//! What the radio-activity screens (group B: transmit, scan, scope, tone search, counter, FM broadcast, HF
//! mode, weather, crossband) share, held once on the App rather than in each screen's state (the screen and
//! its BACK copies stay a few bytes): the region and transmit gates, the exclusive mode a screen started (so
//! leaving it, by any route, stops it on the Radio task), the last hits a probe screen lists, each slot's last
//! minute of activity (the watch + activity layout), the crossband settings and the last alert's text.

use crate::App;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::msg::{BroadcastCmd, Device, LiveAll, RadioCmd, XbCfg, SLOTS};
use ferrite_core::txplan::Region;

/// The exclusive mode a screen started on the Radio task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// The band scope (SCOPE-4), or home's scope strip (single + scope layout).
    Scope,
    Strip,
    Tone,
    Counter,
    Fm,
    Hf,
    Xb,
    /// Weather radio: the active slot is on a weather channel until the screen is left.
    Weather,
}

/// One remembered hit: what (a frequency, or a tone word), when (s, wrapping), how strong (dBm).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Hit {
    pub what: u32,
    pub at_s: u16,
    pub dbm: i8,
}

impl Hit {
    pub fn new(what: u32, now: u32, rssi: i16) -> Self {
        Hit { what, at_s: (now / 1000) as u16, dbm: rssi.clamp(-128, 0) as i8 }
    }

    pub fn age_ms(&self, now: u32) -> u32 {
        ((now / 1000) as u16).wrapping_sub(self.at_s) as u32 * 1000
    }

    pub fn rssi(&self) -> i16 {
        self.dbm as i16
    }
}

pub const HITS: usize = 3;

/// The last alert, for the Alert screen: its event and when it came (the full record, with areas and times,
/// goes to the event log; the toast shows the rest as it arrives).
#[derive(Clone, Copy)]
pub struct AlertNote {
    pub head: [u8; 28],
    pub at: u32,
    /// OK silenced the tone (WX-6).
    pub silenced: bool,
}

impl AlertNote {
    pub fn set(&mut self, head: &[u8], _body: &[u8], now: u32) {
        self.head.fill(0);
        let n = head.len().min(self.head.len());
        self.head[..n].copy_from_slice(&head[..n]);
        self.at = now;
        self.silenced = false;
    }

    pub fn head(&self) -> &[u8] {
        trim0(&self.head)
    }
}

pub(crate) fn trim0(b: &[u8]) -> &[u8] {
    &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]
}

/// Activity buckets per slot for the watch + activity layout: 32 of 1.875 s, the last minute.
pub const ACT_BUCKETS: u32 = 32;
pub const ACT_MS: u32 = 1875;

pub struct RadioUi {
    /// None until setup sets it: transmit then follows the ITU common plan, and weather radio stays open.
    pub region: Option<Region>,
    pub extend_tx: bool,
    /// From the last [`Device`] drawn: the build can transmit, the battery is above the cut-off.
    pub tx_built: bool,
    pub batt_cut: bool,
    /// The slot keyed (3 = HF), if any.
    pub keyed: Option<u8>,
    pub mode: Mode,
    /// When the timed screen up now began (transmit, emergency prompt, tone search), or ends.
    pub t0: u32,
    /// The scope's centre, kept between openings.
    pub centre: Freq10,
    pub hits: [Hit; HITS],
    /// Scan: zones 0-15 added with digits (SCAN-3).
    pub zones: u16,
    pub xb: XbCfg,
    /// FM broadcast frequency, 10 kHz units; the HF VFO and its mode (index into hf::MODES).
    pub fm: u16,
    pub hf: Freq10,
    pub hf_mode: u8,
    /// Home layout (MENU 0 7 2): 0 watch, 1 single + scope, 2 watch + activity.
    pub layout: u8,
    /// Bit i of act[s]: slot s heard something in bucket (now / ACT_MS - i).
    pub act: [u32; SLOTS],
    pub act_bucket: u16,
    pub callsign: [u8; 8],
    pub alert: AlertNote,
}

impl RadioUi {
    pub const fn new() -> Self {
        RadioUi {
            region: None,
            extend_tx: false,
            tx_built: false,
            batt_cut: false,
            keyed: None,
            mode: Mode::Off,
            t0: 0,
            centre: 0,
            hits: [Hit { what: 0, at_s: 0, dbm: 0 }; HITS],
            zones: 0,
            xb: XbCfg::DEFAULT,
            fm: 9_810,
            hf: mhz(7) + 15_000,
            hf_mode: 0,
            layout: 0,
            act: [0; SLOTS],
            act_bucket: 0,
            callsign: [0; 8],
            alert: AlertNote { head: [0; 28], at: 0, silenced: false },
        }
    }

    pub fn region(&self) -> Region {
        self.region.unwrap_or(Region::Other)
    }

    pub fn callsign(&self) -> &[u8] {
        trim0(&self.callsign)
    }

    /// A new hit at the front (the oldest goes); a repeat of the newest only refreshes it.
    pub fn push_hit(&mut self, h: Hit) {
        if self.hits[0].what == h.what && h.what != 0 {
            self.hits[0] = h;
            return;
        }
        self.hits.copy_within(0..HITS - 1, 1);
        self.hits[0] = h;
    }

    pub fn clear_hits(&mut self) {
        self.hits = [Hit::default(); HITS];
    }
}

impl App {
    /// Region (setup, MENU 0 9 4) and Extend TX (MENU 0 8 9): until the settings pages exist, set here.
    pub fn set_region(&mut self, r: Option<Region>, extend_tx: bool) {
        self.radio.region = r;
        self.radio.extend_tx = extend_tx;
    }

    /// Put a slot on its VFO at a frequency (the simulator's samples).
    pub fn set_vfo(&mut self, slot: u8, f: Freq10) {
        let s = &mut self.slots[slot as usize % SLOTS];
        s.vfo = f;
        s.vfo_mode = true;
        self.send_slot(slot % SLOTS as u8);
    }

    /// The callsign (setup page 2): crossband ID and automatic transmit need one.
    pub fn set_callsign(&mut self, c: &[u8]) {
        let n = c.len().min(8);
        self.radio.callsign = [0; 8];
        self.radio.callsign[..n].copy_from_slice(&c[..n]);
    }

    /// Home layout (MENU 0 7 2): 0 watch, 1 single + scope, 2 watch + activity (HOME-3).
    pub fn set_layout(&mut self, l: u8) {
        self.radio.layout = l % 3;
        if self.radio.mode == Mode::Strip {
            self.stop_mode();
        }
        self.redraw_all();
    }

    /// The radio-wide state that gates transmit, remembered from what is drawn (PTT arrives without it).
    pub(crate) fn note_device(&mut self, dev: &Device) {
        self.radio.tx_built = dev.tx_built;
        self.radio.batt_cut = dev.battery_cutoff;
    }

    /// Start an exclusive mode (stopping any other first).
    pub(crate) fn start_mode(&mut self, m: Mode) {
        if self.radio.mode != m {
            self.stop_mode();
        }
        self.radio.mode = m;
    }

    /// Stop whatever exclusive mode runs: its screen was left (BACK, hold BACK, a timeout, PTT).
    pub(crate) fn stop_mode(&mut self) {
        let a = self.active;
        match core::mem::replace(&mut self.radio.mode, Mode::Off) {
            Mode::Off => {}
            Mode::Scope | Mode::Strip => self.out.push(RadioCmd::Sweep { centre: 0, step: 0, on: false }),
            Mode::Tone => self.out.push(RadioCmd::ToneSearch { slot: a, on: false }),
            Mode::Counter => self.out.push(RadioCmd::Counter(false)),
            Mode::Fm => self.out.push(RadioCmd::Broadcast(BroadcastCmd::Off)),
            Mode::Hf => self.out.push(RadioCmd::Hf { freq: self.radio.hf, mode: self.radio.hf_mode, on: false }),
            Mode::Xb => self.out.push(RadioCmd::Crossband(None)),
            Mode::Weather => self.send_slot(a), // back to the slot's own frequency
        }
    }

    /// PTT released: unkey whatever is keyed (TX-25), whichever screen is up.
    pub(crate) fn ptt_released(&mut self) {
        if let Some(slot) = self.radio.keyed.take() {
            self.out.push(RadioCmd::Transmit { slot, on: false });
        }
    }

    /// Every tick: each slot's activity for the watch + activity layout.
    pub(crate) fn radio_tick(&mut self, live: &LiveAll) {
        let b = (self.now / ACT_MS) as u16;
        let shift = b.wrapping_sub(self.radio.act_bucket).min(32) as u32;
        if shift > 0 {
            self.radio.act_bucket = b;
            for a in &mut self.radio.act {
                *a = a.checked_shl(shift).unwrap_or(0);
            }
        }
        for (i, s) in live.slots.iter().enumerate() {
            if s.squelch_open {
                self.radio.act[i] |= 1;
            }
        }
        if live.tx.keyed {
            self.radio.act[(live.tx.slot as usize).min(SLOTS - 1)] |= 1;
        }
    }
}

/// Text bytes as &str for ufmt (the UI's text is always UTF-8).
pub fn txt(b: &[u8]) -> &str {
    core::str::from_utf8(b).unwrap_or("")
}

/// "2:47" (minutes and seconds).
pub fn mmss<const N: usize>(s: u32, b: &mut ferrite_ui::text::Buf<N>) {
    let _ = ufmt::uwrite!(b, "{}:{}{}", s / 60, (s % 60) / 10, s % 10);
}
