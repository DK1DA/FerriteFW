//! Radio-wide settings, one value wherever it is changed (setup, the advanced menu's groups, the screens that
//! keep their own: SETUP-5, MENU-7). They live outside the [`crate::App`], which holds only UI state and a
//! reference to them ([`crate::App::new`]): the firmware keeps one [`Settings`] in a static that the Storage
//! task will write to the settings struct in SPI flash (0x02D000, 2 s after the last edit: MENU-21) and HF
//! mode's memory to its own sector (HF-29); the simulator and tests keep one each. [`Settings::DEFAULT`] is
//! what a first boot writes (BOOT-12).
//!
//! The value lists ([`REGION_LIST`], [`TIME_ZONES`], ...) are the menu's tables: their names are the ones the
//! screens show (MENU-7).

use crate::screens::hf::HfMem;
use ferrite_core::msg::XbCfg;
use ferrite_core::txplan::Region;
use ferrite_ui::boot::Owner;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

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

/// Regions (0 9 4).
pub const REGION_LIST: [&str; 8] =
    ["United States", "Canada", "United Kingdom", "Europe", "Australia", "New Zealand", "Japan", "Other"];

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
/// Boot screen (0 7 3 3).
pub const BOOT_SCREENS: [&str; 2] = ["Logo", "Owner card"];
/// The cable port (0 9 2 5, TERM-2): auto picks from the first bytes; the others force one.
pub const CABLE_PORTS: [&str; 4] = ["Auto", "Link", "Terminal", "KISS"];

/// FM broadcast step (0 3 1 2) in 10 kHz units, and its names.
pub const FM_STEPS: [(u16, &str); 3] = [(5, "50 kHz"), (10, "100 kHz"), (20, "200 kHz")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// 0 8 7 1, capitals (SETUP-12).
    pub callsign: Text<10>,
    /// The owner card (SETUP-11): name, phone, e-mail. Entered on the radio only in setup page 4, then edited
    /// in the web app only; shown at boot when [`Settings::boot_card`] is on.
    pub owner: [Text<20>; 3],
    /// 0 7 3 3: the boot screen is the owner card (BOOT_SCREENS[1]), not the logo.
    pub boot_card: bool,
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
    /// 0 9 2 5: what the cable carries, an index into [`CABLE_PORTS`] (TERM-2).
    pub cable_port: u8,
    /// 0 3 3 1 (US and Canada only).
    pub same: bool,
    /// FM broadcast: frequency in 10 kHz, step index, AGC (0 auto, else gain dB + 1), radio monitor (0 3 1).
    pub fm_freq: u16,
    pub fm_step: u8,
    pub fm_agc: u8,
    pub fm_monitor: bool,
    /// Setup has finished once (the layout header exists).
    pub setup_done: bool,
    /// The one home channel's number (CHAN-17), if set.
    pub home_ch: Option<u16>,
    /// The crossband repeater's settings (chapter 19).
    pub xb: XbCfg,
    /// HF mode's VFO, options and recent list (HF-27 to HF-29): its own SPI sector.
    pub hf: HfMem,
    /// 0 6 3: how positions are written, an index into [`COORD_FORMATS`].
    pub coord_fmt: u8,
    /// 0 6 9: the fixed position (degrees x 1e7) and altitude in metres, used with no fix (GPS-3); off = none.
    pub fixed_on: bool,
    pub fixed: crate::data::Pos,
    pub fixed_alt: i16,
    /// 0 3 3 2: SAME location codes, six digits each (FIPS in the US, CLC in Canada); 0 = an empty place (WX-10).
    pub same_codes: [u32; SAME_CODES],
    /// Band scope (0 3 2): bars (an index into [`SCOPE_BAR_COUNTS`]), step (into the tuning step list), dwell
    /// per point in 0.1 ms (1-40), trigger action ([`SCOPE_TRIGGERS`]), the trigger line (0 none, 1-9: hold a
    /// digit on the scope), the waterfall row, and the scope's mode (FM, NFM, AM: \* on the scope).
    pub scope_bars: u8,
    pub scope_step: u8,
    pub scope_dwell: u8,
    pub scope_trig: u8,
    pub scope_line: u8,
    pub scope_wf: bool,
    pub scope_mode: u8,
    /// 0 9 2 6: the cable terminal in ANSI colour (TERM-18); off, reverse video and bold only.
    pub term_colour: bool,
    /// 0 5 3: APRS mode, slot C as the APRS slot (APRS-15 – APRS-22; the takeover itself comes in milestone 5).
    pub aprs_mode: bool,
    /// Message text is typed predictively (TEXT-10); hold \* switches, and the choice is kept for the next one.
    pub predictive: bool,
    /// Approval tokens of Bluetooth devices allowed to write (LINK-7), newest first; all zero is none.
    pub link_tokens: [[u8; 8]; crate::link::TOKENS],
    /// 0 7 4 1: the screen goes dark after this ([`BACKLIGHT_TIMES`] index; 0 never).
    pub backlight: u8,
    /// 0 7 4 5: the keypad light ([`KEYPAD_LIGHTS`]).
    pub keypad_light: u8,
    /// 0 7 6: what the LEDs show ([`LED_MODES`]).
    pub leds: u8,
    /// 0 7 8: auto power off ([`AUTO_OFF_TIMES`] index; 0 off).
    pub auto_off: u8,
    /// 0 9 1 1: auto key lock (KEY-20, [`AUTO_LOCK_TIMES`] index; 0 off).
    pub auto_lock: u8,
    /// 0 8 6 6: hear the DTMF digits sent, at beep volume.
    pub dtmf_sidetone: bool,
    /// 0 7 5 4: a tone on arriving at a waypoint.
    pub arrival_tone: bool,
    /// The channel lists show hidden channels too (their options).
    pub show_hidden: bool,
}

/// 0 7 4 1's values, seconds (0 never).
pub const BACKLIGHT_TIMES: [u16; 6] = [0, 5, 10, 15, 30, 60];
/// 0 7 4 5's values.
pub const KEYPAD_LIGHTS: [&str; 4] = ["Off", "With screen", "On key press", "Always"];
/// 0 7 6's values: the heartbeat is the dark-screen flash in the battery's colour.
pub const LED_MODES: [&str; 3] = ["Off", "Receive and transmit", "Receive, transmit and heartbeat"];
/// 0 7 8's values, minutes (0 off).
pub const AUTO_OFF_TIMES: [u16; 5] = [0, 30, 60, 120, 240];
/// 0 9 1 1's values, seconds (0 off).
pub const AUTO_LOCK_TIMES: [u16; 5] = [0, 30, 60, 120, 300];


/// 0 3 2 1's values (SCOPE-9).
pub const SCOPE_BAR_COUNTS: [u8; 4] = [30, 60, 120, 240];
/// 0 3 2 4's values (SCOPE-10).
pub const SCOPE_TRIGGERS: [&str; 3] = ["Off", "Monitor", "Log"];

/// 0 6 3's values.
pub const COORD_FORMATS: [&str; 4] = ["Degrees", "Deg, min", "Deg, min, sec", "Grid square"];
/// Up to 8 SAME location codes (WX-10).
pub const SAME_CODES: usize = 8;

impl Settings {
    pub const DEFAULT: Settings = Settings {
        callsign: Text::EMPTY,
        owner: [Text::EMPTY; 3],
        boot_card: false,
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
        cable_port: 0,
        same: true,
        fm_freq: 10_110,
        fm_step: 1,
        fm_agc: 0,
        fm_monitor: true,
        setup_done: false,
        home_ch: None,
        xb: XbCfg::DEFAULT,
        hf: HfMem::DEFAULT,
        coord_fmt: 1,
        fixed_on: false,
        fixed: crate::data::Pos { lat: 0, lon: 0 },
        fixed_alt: 0,
        same_codes: [0; SAME_CODES],
        scope_bars: 1,
        scope_step: 11, // 25 kHz
        scope_dwell: 10,
        scope_trig: 0,
        scope_line: 0,
        scope_wf: false,
        scope_mode: 0,
        term_colour: false,
        aprs_mode: false,
        predictive: true,
        link_tokens: [[0; 8]; crate::link::TOKENS],
        backlight: 3,
        keypad_light: 1,
        leds: 2,
        auto_off: 0,
        auto_lock: 0,
        dtmf_sidetone: true,
        arrival_tone: true,
        show_hidden: false,
    };

    /// The region's name, or "Not set".
    pub fn region_name(&self) -> &'static str {
        match self.region {
            1..=8 => REGION_LIST[self.region as usize - 1],
            _ => "Not set",
        }
    }

    /// The region, None until setup (or 0 9 4) sets it.
    pub fn region(&self) -> Option<Region> {
        Region::ALL.get((self.region as usize).wrapping_sub(1)).copied()
    }

    /// The region for the transmit plan: until one is set, the ITU common plan ("Other").
    pub fn tx_region(&self) -> Region {
        self.region().unwrap_or(Region::Other)
    }

    pub fn set_region(&mut self, r: Option<Region>) {
        self.region = r.and_then(|r| Region::ALL.iter().position(|&x| x == r)).map_or(0, |i| i as u8 + 1);
    }

    /// Weather radio and SAME exist only in the US and Canada (WX-3).
    pub fn wx_region(&self) -> bool {
        self.region().is_some_and(|r| r.has_weather())
    }

    /// Weather radio may open: in the US and Canada, and until a region is set (WX-1).
    pub fn wx_allowed(&self) -> bool {
        self.region().is_none_or(|r| r.has_weather())
    }

    /// The callsign, capitals (SETUP-12).
    pub fn set_callsign(&mut self, c: &[u8]) {
        self.callsign = Text::EMPTY;
        for &b in c {
            self.callsign.push(b.to_ascii_uppercase());
        }
    }

    /// The boot screen's callsign (BOOT-9): None until one is set.
    pub fn boot_callsign(&self) -> Option<&[u8]> {
        (!self.callsign.is_empty()).then(|| self.callsign.as_bytes())
    }

    /// The owner card for the boot screen: Some when 0 7 3 3 is "Owner card" and the card has something on it
    /// (a callsign, name, phone or e-mail); else None, and the boot screen shows the logo.
    pub fn boot_owner(&self) -> Option<Owner<'_>> {
        let [name, phone, email] = &self.owner;
        let any = !self.callsign.is_empty() || self.owner.iter().any(|t| !t.is_empty());
        (self.boot_card && any).then(|| Owner { name: name.as_bytes(), phone: phone.as_bytes(), email: email.as_bytes() })
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
