//! Radio-wide settings the entry and setup screens write (manual chapters 4, 10, 15, 34; `docs/settings.md`):
//! callsign, region, Extend TX, time, look, PIN and the home channel. Until the settings struct in SPI flash
//! exists (SETUP-5, MENU-21) they live here, a few bytes, with their defaults.

/// Regions (0 9 4) and what the setup page shows beside each: its 1.25 m transmit range, where it has one
/// (SetupRegion mock-up).
pub const REGIONS: [(&str, &str); 8] = [
    ("United States", "222-225"),
    ("Canada", "219-225"),
    ("United Kingdom", ""),
    ("Europe", ""),
    ("Australia", ""),
    ("New Zealand", ""),
    ("Japan", ""),
    ("Other", ""),
];
/// No region chosen yet.
pub const NO_REGION: u8 = 255;

pub const LAYOUTS: [&str; 3] = ["Watch", "Single + scope", "Watch + activity"];
pub const THEMES: [&str; 3] = ["Dark", "Daylight", "Night red"];
pub const UNITS: [&str; 3] = ["Metric", "Imperial", "Nautical"];
pub const PIN_MODES: [&str; 3] = ["Off", "At power on", "At power on and unlock"];

/// Time zones: UTC-12 to +14 in quarter hours (0 6 5); the 30 and 45 minute ones are quarter steps.
pub const TZ_MIN: i8 = -48;
pub const TZ_MAX: i8 = 56;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// 0 8 7 1, capitals (SETUP-12), unused bytes 0.
    pub callsign: [u8; 10],
    /// Index into [`REGIONS`], or [`NO_REGION`].
    pub region: u8,
    /// 0 8 9 (TX-2).
    pub extend_tx: bool,
    /// The Extend TX warning has been shown (SETUP-8, "shown once").
    pub extend_warned: bool,
    /// UTC offset in quarter hours (0 6 5; default UTC-8).
    pub tz: i8,
    pub clock_12h: bool,
    pub units: u8,
    pub layout: u8,
    pub theme: u8,
    /// 0 9 3 1: the PIN's digits, 4-6 of them (0 = no PIN), and when it is asked ([`PIN_MODES`]).
    pub pin: [u8; 6],
    pub pin_len: u8,
    pub pin_mode: u8,
    /// The one home channel's number (CHAN-17).
    pub home_ch: Option<u16>,
    /// The owner card (SETUP-11): entered on setup page 4 only.
    pub owner_name: [u8; 16],
    pub owner_contact: [u8; 16],
}

pub const DEFAULT_PREFS: Prefs = Prefs {
    callsign: [0; 10],
    region: NO_REGION,
    extend_tx: false,
    extend_warned: false,
    tz: -32,
    clock_12h: false,
    units: 0,
    layout: 0,
    theme: 0,
    pin: [0; 6],
    pin_len: 0,
    pin_mode: 0,
    home_ch: None,
    owner_name: [0; 16],
    owner_contact: [0; 16],
};

impl Prefs {
    pub fn callsign(&self) -> &[u8] {
        z(&self.callsign)
    }

    pub fn owner_name(&self) -> &[u8] {
        z(&self.owner_name)
    }

    pub fn owner_contact(&self) -> &[u8] {
        z(&self.owner_contact)
    }

    pub fn region_name(&self) -> &'static str {
        REGIONS.get(self.region as usize).map_or("Not set", |r| r.0)
    }

    /// A PIN is set and asked at unlock (KEY-16).
    pub fn pin_at_unlock(&self) -> bool {
        self.pin_len > 0 && self.pin_mode == 2
    }
}

/// The text in a zero-padded field.
fn z(b: &[u8]) -> &[u8] {
    &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]
}

/// "UTC-8", "UTC+5:30", "UTC" for a zone in quarter hours.
pub fn tz_text(q: i8, b: &mut ferrite_ui::text::Buf<16>) {
    b.set(b"UTC");
    if q == 0 {
        return;
    }
    b.push(if q < 0 { b"-" } else { b"+" });
    let m = q.unsigned_abs() as u32 * 15;
    let _ = ufmt::uwrite!(b, "{}", m / 60);
    if !m.is_multiple_of(60) {
        let _ = ufmt::uwrite!(b, ":{}", m % 60);
    }
}

use ferrite_core::freq::{mhz, Freq10};

/// Amateur transmit bands per region, MHz (SETUP-7), for what entry says (VFO-12). Display only: the Radio
/// task's band table decides (VFO-2, TX-2). Index = region; the last row serves "Other" and no region.
const HAM: [&[(u32, u32)]; 8] = [
    &[(28, 30), (50, 54), (144, 148), (222, 225), (420, 450)],
    &[(28, 30), (50, 54), (144, 148), (219, 225), (430, 450)],
    &[(28, 30), (50, 52), (70, 71), (144, 146), (430, 440)],
    &[(28, 30), (50, 52), (144, 146), (430, 440)],
    &[(28, 30), (50, 54), (144, 148), (420, 450)],
    &[(28, 30), (50, 54), (144, 148), (430, 440)],
    &[(28, 30), (50, 54), (144, 146), (430, 440)],
    &[(144, 146), (430, 440)],
];
/// What the hardware can transmit, MHz (Extend TX: TX-2's calibration windows, roughly).
const HW_TX: [(u32, u32); 3] = [(136, 174), (200, 260), (400, 520)];

/// Whether transmit is allowed at f, and the reason either way: Ok("Allowed") or Err(why not).
pub fn tx_check(p: &Prefs, f: Freq10, tx_built: bool) -> Result<&'static str, &'static str> {
    let within = |t: &[(u32, u32)]| t.iter().any(|&(lo, hi)| f >= mhz(lo) && f < mhz(hi));
    if !tx_built {
        return Err("RX-only build");
    }
    let r = (p.region as usize).min(HAM.len() - 1);
    if within(HAM[r]) {
        Ok(if f >= mhz(219) && f < mhz(225) { "Allowed, 0.25 W" } else { "Allowed" })
    } else if p.extend_tx && within(&HW_TX) {
        Ok("Extend TX")
    } else if p.region == NO_REGION {
        Err("Region not set")
    } else {
        Err("Not amateur")
    }
}
