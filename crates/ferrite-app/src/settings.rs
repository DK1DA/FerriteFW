//! Radio-wide settings that setup, the advanced menu's groups and the system screens share: one value, wherever
//! it is changed (SETUP-5, MENU-7). In RAM until the Storage task keeps the settings struct (0x02D000, written
//! 2 s after the last edit: MENU-21); [`Settings::DEFAULT`] is what a first boot writes (BOOT-12).
//!
//! The value lists ([`REGION_LIST`], [`TIME_ZONES`], ...) are the menu's tables: their names are the ones the
//! screens show (MENU-7).

use crate::screens::hf::HfMem;
use ufmt::uwrite;
use ferrite_ui::text::Buf;

/// Text in a fixed array, zero padded (no length field: smaller than a [`Buf`] in the App).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Text<const N: usize>(pub [u8; N]);

impl<const N: usize> Text<N> {
    pub const EMPTY: Self = Text([0; N]);

    pub fn as_bytes(&self) -> &[u8] {
        &self.0[..self.len()]
    }

    pub fn len(&self) -> usize {
        self.0.iter().position(|&c| c == 0).unwrap_or(N)
    }

    pub fn is_empty(&self) -> bool {
        self.0[0] == 0
    }

    /// Adds a character; false when full.
    pub fn push(&mut self, c: u8) -> bool {
        let n = self.len();
        if n < N {
            self.0[n] = c;
        }
        n < N
    }

    /// The last character, replaced (multi-tap: the next letter on the same key).
    pub fn set_last(&mut self, c: u8) {
        let n = self.len();
        if n > 0 {
            self.0[n - 1] = c;
        }
    }

    pub fn pop(&mut self) {
        let n = self.len();
        if n > 0 {
            self.0[n - 1] = 0;
        }
    }

    pub fn set(&mut self, s: &[u8]) {
        self.0 = [0; N];
        let n = s.len().min(N);
        self.0[..n].copy_from_slice(&s[..n]);
    }
}

/// Regions (0 9 4): name, and the 1.25 m transmit range the setup list shows.
pub const REGION_LIST: [(&str, &str); 8] = [
    ("United States", "222-225"),
    ("Canada", "219-225"),
    ("United Kingdom", ""),
    ("Europe", ""),
    ("Australia", ""),
    ("New Zealand", ""),
    ("Japan", ""),
    ("Other", ""),
];

/// Time zones (0 6 5), in quarter hours: UTC-12 to +14 with the 30 and 45 minute offsets in use.
pub const TIME_ZONES: [i8; 38] = [
    -48, -44, -40, -38, -36, -32, -28, -24, -20, -16, -14, -12, -8, -4, 0, 4, 8, 12, 14, 16, 18, 20, 22, 23, 24,
    26, 28, 32, 35, 36, 38, 40, 42, 44, 48, 51, 52, 56,
];
/// UTC-8 (settings.md).
pub const TZ_DEFAULT: u8 = 5;

pub const CLOCKS: [&str; 2] = ["24 h", "12 h"];
pub const UNITS: [&str; 3] = ["Metric", "Imperial", "Nautical"];
pub const LAYOUTS: [&str; 3] = ["Watch", "Single + scope", "Watch + activity"];
pub const THEMES: [&str; 3] = ["Dark", "Daylight", "Night red"];
pub const PIN_MODES: [&str; 3] = ["Off", "At power on", "Power on + unlock"];
pub const ON_OFF: [&str; 2] = ["Off", "On"];
/// FM broadcast step (0 3 1 2) in 10 kHz units, and its names.
pub const FM_STEPS: [(u16, &str); 3] = [(5, "50 kHz"), (10, "100 kHz"), (20, "200 kHz")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// 0 8 7 1, capitals (SETUP-12).
    pub callsign: Text<10>,
    /// The owner card (SETUP-11): name, phone, e-mail. Entered on the radio only in setup.
    pub owner: [Text<20>; 3],
    /// 0 9 4: 0 = not chosen yet, else REGION_LIST[n - 1].
    pub region: u8,
    /// 0 8 9, and whether its warning has been shown (shown once: SETUP-8).
    pub extend_tx: bool,
    pub extend_warned: bool,
    /// 0 6 5, an index into TIME_ZONES; 0 6 6 format and from GPS; 0 6 4.
    pub tz: u8,
    pub clock: u8,
    pub clock_gps: bool,
    pub units: u8,
    /// 0 7 2, 0 7 1.
    pub layout: u8,
    pub theme: u8,
    /// 0 9 3 1: mode (PIN_MODES), the PIN and its length (0 = none).
    pub pin_mode: u8,
    pub pin: u32,
    pub pin_len: u8,
    /// Not settings: wrong PINs so far, and the wait's end in seconds of app time (0 none: PIN-3).
    pub pin_misses: u8,
    pub pin_wait: u16,
    /// 0 9 2 1.
    pub bt: bool,
    /// 0 3 3 1 (US and Canada only).
    pub same: bool,
    /// FM broadcast: frequency in 10 kHz, step index, AGC (0 auto, else gain dB + 1), radio monitor (0 3 1).
    pub fm_freq: u16,
    pub fm_step: u8,
    pub fm_agc: u8,
    pub fm_monitor: bool,
    /// Setup has finished once (the layout header exists).
    pub setup_done: bool,
    /// HF mode's VFO and recent list (HF-27 to HF-29).
    pub hf: HfMem,
    /// Not a setting: which receiver mode is running (0 none, 1 FM broadcast, 2 HF), so leaving it is noticed.
    pub listen: u8,
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        callsign: Text::EMPTY,
        owner: [Text::EMPTY; 3],
        region: 0,
        extend_tx: false,
        extend_warned: false,
        tz: TZ_DEFAULT,
        clock: 0,
        clock_gps: true,
        units: 0,
        layout: 0,
        theme: 0,
        pin_mode: 0,
        pin: 0,
        pin_len: 0,
        pin_misses: 0,
        pin_wait: 0,
        bt: false,
        same: true,
        fm_freq: 10_110,
        fm_step: 1,
        fm_agc: 0,
        fm_monitor: true,
        setup_done: false,
        hf: HfMem::DEFAULT,
        listen: 0,
    };

    /// The region's name, or "Not set".
    pub fn region_name(&self) -> &'static str {
        match self.region {
            1..=8 => REGION_LIST[self.region as usize - 1].0,
            _ => "Not set",
        }
    }

    /// Weather radio and SAME exist only in the US and Canada (WX-3).
    pub fn wx_region(&self) -> bool {
        matches!(self.region, 1 | 2)
    }

    /// SAME alerts are on: the setting, in a region that has them.
    pub fn same_on(&self) -> bool {
        self.same && self.wx_region()
    }

    /// "UTC-8", "UTC+5:30", "UTC".
    pub fn tz_text(&self, b: &mut Buf<16>) {
        tz_text(TIME_ZONES[self.tz as usize % TIME_ZONES.len()], b);
    }
}

pub fn tz_text(q: i8, b: &mut Buf<16>) {
    b.set(b"UTC");
    if q != 0 {
        let a = q.unsigned_abs();
        let _ = uwrite!(b, "{}{}", if q < 0 { "-" } else { "+" }, a / 4);
        if !a.is_multiple_of(4) {
            let _ = uwrite!(b, ":{}", (a % 4) * 15);
        }
    }
}
