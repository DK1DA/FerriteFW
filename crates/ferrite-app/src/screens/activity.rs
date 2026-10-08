//! What the radio-activity screens (group B: transmit, scan, scope, tone search, counter, FM broadcast, HF
//! mode, weather, crossband) share, held once on the App rather than in each screen's state (the screen and
//! its BACK copies stay a few bytes): the transmit gates, the exclusive mode a screen started (so leaving it,
//! by any route, stops it on the Radio task), the last hits a probe screen lists, each slot's last minute of
//! activity (the watch + activity layout) and HF mode's tuning state. Settings (region, Extend TX, callsign,
//! layout, the crossband, FM and HF memories) are in [`crate::settings`].

use crate::App;
use ferrite_core::freq::Freq10;
use ferrite_core::msg::{BroadcastCmd, Device, LiveAll, RadioCmd, SLOTS};
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
}

/// One remembered hit: what (a frequency, or a tone word), when (s, wrapping), how strong (dBm), how many
/// times (SCAN-23's count; it fits the padding).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Hit {
    pub what: u32,
    pub at_s: u16,
    pub dbm: i8,
    pub n: u8,
}

impl Hit {
    pub fn new(what: u32, now: u32, rssi: i16) -> Self {
        Hit { what, at_s: (now / 1000) as u16, dbm: rssi.clamp(-128, 0) as i8, n: 1 }
    }

    pub fn age_ms(&self, now: u32) -> u32 {
        ((now / 1000) as u16).wrapping_sub(self.at_s) as u32 * 1000
    }

    pub fn rssi(&self) -> i16 {
        self.dbm as i16
    }
}

/// The last hits the probe and scan screens show.
pub const HITS: usize = 3;
/// The hits kept: a range scan's hit list (SCAN-23, nicFW's smart scan list), the counter's and tone search's
/// last few. 8 bytes each.
pub const HIT_LIST: usize = 16;

/// Activity buckets per slot for the watch + activity layout: 32 of 1.875 s, the last minute.
pub const ACT_BUCKETS: u32 = 32;
pub const ACT_MS: u32 = 1875;

pub struct RadioUi {
    /// From the last [`Device`] drawn: the build can transmit, the battery is above the cut-off, the SSB patch
    /// is installed (PTT and HF mode's keys arrive without the Device).
    pub tx_built: bool,
    pub batt_cut: bool,
    pub ssb_patch: bool,
    /// The slot keyed (3 = HF), if any.
    pub keyed: Option<u8>,
    pub mode: Mode,
    /// When the timed screen up now began (transmit, emergency prompt, tone search), or ends.
    pub t0: u32,
    /// The scope's centre, kept between openings.
    pub centre: Freq10,
    pub hits: [Hit; HIT_LIST],
    /// The scan hit count ([`ferrite_core::msg::SlotLive::hits`]) last counted into the list.
    pub hits_seen: u8,
    /// Scan: zones 0-15 added with digits (SCAN-3).
    pub zones: u16,
    /// Bit i of act[s]: slot s heard something in bucket (now / ACT_MS - i).
    pub act: [u32; SLOTS],
    pub act_bucket: u16,
    /// HF mode: when the frequency was tuned and whether it is in the recent list yet (HF-28), and a
    /// frequency being typed, as a number (the screen state holds its length and point).
    pub hf_tuned_at: u32,
    pub hf_counted: bool,
    /// A seek (hold 3 or 1) is running: the receiver's frequency is taken as it lands.
    pub hf_seeking: bool,
    pub hf_entry: u32,
}

impl RadioUi {
    pub const fn new() -> Self {
        RadioUi {
            tx_built: false,
            batt_cut: false,
            ssb_patch: false,
            keyed: None,
            mode: Mode::Off,
            t0: 0,
            centre: 0,
            hits: [Hit { what: 0, at_s: 0, dbm: 0, n: 0 }; HIT_LIST],
            hits_seen: 0,
            zones: 0,
            act: [0; SLOTS],
            act_bucket: 0,
            hf_tuned_at: 0,
            hf_counted: false,
            hf_seeking: false,
            hf_entry: 0,
        }
    }

    /// A hit at the front, the oldest going when the list is full. The newest again only refreshes it, unless
    /// `again` (a new stop there, SCAN-23): then it counts; one further down moves to the front and counts.
    pub fn push_hit(&mut self, h: Hit, again: bool) {
        if h.what == 0 {
            return;
        }
        let n = match self.hits.iter().position(|o| o.what == h.what) {
            Some(0) if !again => self.hits[0].n,
            Some(i) => {
                let n = self.hits[i].n.saturating_add(1);
                self.hits.copy_within(0..i, 1);
                n
            }
            None => {
                self.hits.copy_within(0..HIT_LIST - 1, 1);
                1
            }
        };
        self.hits[0] = Hit { n, ..h };
    }

    pub fn clear_hits(&mut self) {
        self.hits = [Hit::default(); HIT_LIST];
    }
}

impl App {
    /// Region (setup, MENU 0 9 4) and Extend TX (MENU 0 8 9), from outside (the simulator's samples, tests).
    pub fn set_region(&mut self, r: Option<Region>, extend_tx: bool) {
        self.set.set_region(r);
        self.set.extend_tx = extend_tx;
        self.region_steps();
    }

    /// Put a slot on its VFO at a frequency (the simulator's samples).
    pub fn set_vfo(&mut self, slot: u8, f: Freq10) {
        let s = &mut self.slots[slot as usize % SLOTS];
        s.vfo = f;
        s.vfo_mode = true;
        self.send_slot(slot % SLOTS as u8);
    }

    /// Put a slot on a channel that may not be in the store (the simulator's samples: a 16-character name, no name).
    pub fn set_channel_rec(&mut self, slot: u8, c: crate::chan::Channel) {
        let s = &mut self.slots[slot as usize % SLOTS];
        (s.zone, s.ch, s.vfo_mode) = (c.zone, Some(c), false);
        self.send_slot(slot % SLOTS as u8);
    }

    /// The callsign (setup page 2): crossband ID and automatic transmit need one.
    pub fn set_callsign(&mut self, c: &[u8]) {
        self.set.set_callsign(c);
    }

    /// Home layout (MENU 0 7 2): 0 watch, 1 single + scope, 2 watch + activity (HOME-3).
    pub fn set_layout(&mut self, l: u8) {
        self.set.layout = l % 3;
        if self.radio.mode == Mode::Strip {
            self.stop_mode();
        }
        self.redraw_all();
    }

    /// The radio-wide state that gates transmit, remembered from what is drawn (PTT arrives without it).
    pub(crate) fn note_device(&mut self, dev: &Device) {
        self.radio.tx_built = dev.tx_built;
        self.radio.batt_cut = dev.battery_cutoff;
        self.radio.ssb_patch = dev.selftest.ssb_patch;
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
            Mode::Hf => self.out.push(RadioCmd::Hf { freq: self.set.hf.freq, mode: self.set.hf.mode, on: false }),
            Mode::Xb => self.out.push(RadioCmd::Crossband(None)),
        }
    }

    /// Home: a mode started by a screen (scope, tone search, counter, FM, HF, weather, crossband) stops with it;
    /// home's own scope strip stays.
    pub(crate) fn home_stops_modes(&mut self) {
        if !matches!(self.radio.mode, Mode::Off | Mode::Strip) {
            self.stop_mode();
        }
    }

    /// PTT released: unkey whatever is keyed (TX-25), whichever screen is up.
    pub(crate) fn ptt_released(&mut self) {
        if let Some(slot) = self.radio.keyed.take() {
            self.out.push(RadioCmd::Transmit { slot, on: false });
            // WATCH-16: watch waits 30 s of quiet (or a key) so the reply on this slot isn't missed; a longer #
            // pause (WATCH-6) is left as it is.
            let until = self.now.wrapping_add(crate::TX_PAUSE_S as u32 * 1000);
            if self.tx_pause || self.paused_until.is_none_or(|t| crate::after(until, t)) {
                (self.paused_until, self.tx_pause) = (Some(until), true);
                self.out.push(RadioCmd::PauseWatch(crate::TX_PAUSE_S));
            }
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
