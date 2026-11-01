//! Satellite mode's logic (manual chapter 28a, SAT-1 to SAT-20): the satellites' records as numbers
//! ([`Sat`], [`Elements`], [`Transponder`]), near-Earth SGP4 in single precision ([`sgp4`], SAT-4), where a
//! satellite is from an observer with its Doppler and the sun ([`look`], SAT-7, SAT-8, SAT-17 to SAT-19), and
//! pass prediction in slices ([`pass`], SAT-6). No storage and no screens: the App keeps the passes and draws
//! them, and the satellites come from a source the platform gives it.

pub mod data;
pub mod fmath;
pub mod look;
pub mod pass;
pub mod sgp4;
pub mod tle;
pub mod track;
pub mod wire;

use crate::freq::Freq10;

/// UTC seconds since 1970 of 2000-01-01 00:00:00 UTC: element epochs count from here (a u32 lasts to 2136).
pub const Y2000: u32 = 946_684_800;

/// One element set (the research's 48-byte record, section 6): integers keep a TLE's precision exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Elements {
    /// Epoch: seconds since 2000-01-01 00:00 UTC and the milliseconds after.
    pub epoch: u32,
    pub epoch_ms: u16,
    /// Inclination, right ascension of the ascending node, argument of perigee, mean anomaly: 1e-7 degree.
    pub incl: u32,
    pub raan: u32,
    pub argp: u32,
    pub ma: u32,
    /// Eccentricity, 1e-7.
    pub ecc: u32,
    /// Mean motion (Kozai), 1e-8 revolution per day.
    pub n: u32,
    /// The drag term B*, 1 / earth radii, as f32 bits (kept as bits so the record stays `Eq` and `Hash`).
    pub bstar: F32,
}

/// An f32 kept by its bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct F32(pub u32);

impl F32 {
    pub fn new(v: f32) -> F32 {
        F32(v.to_bits())
    }
    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

const E7_RAD: f32 = core::f32::consts::PI / 180.0 * 1e-7;

impl Elements {
    fn angle(v: u32) -> f32 {
        // split so the f32 keeps 1e-7 degree: whole degrees and the rest
        let deg = v / 10_000_000;
        fmath::rad(deg as f32) + (v % 10_000_000) as f32 * E7_RAD
    }
    pub fn incl_rad(&self) -> f32 {
        Self::angle(self.incl)
    }
    pub fn raan_rad(&self) -> f32 {
        Self::angle(self.raan)
    }
    pub fn argp_rad(&self) -> f32 {
        Self::angle(self.argp)
    }
    pub fn ma_rad(&self) -> f32 {
        Self::angle(self.ma)
    }
    pub fn ecc(&self) -> f32 {
        self.ecc as f32 * 1e-7
    }
    /// Mean motion in radians per minute.
    pub fn n_rad_min(&self) -> f32 {
        self.n as f32 * (1e-8 * core::f32::consts::TAU / 1440.0)
    }
    /// UTC seconds since 1970 of the epoch's whole second.
    pub fn epoch_unix(&self) -> u32 {
        Y2000.wrapping_add(self.epoch)
    }
    /// The elements' age in whole days at UTC `now` (0 for an epoch in the future).
    pub fn age_days(&self, now: u32) -> u32 {
        now.saturating_sub(self.epoch_unix()) / 86_400
    }
    /// Revolutions per day, rounded down (for the low-orbit check and display).
    pub fn rev_day(&self) -> u32 {
        self.n / 100_000_000
    }
}

/// A transponder's use (shown on the pass page: "Voice", "APRS", "Beacon").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    #[default]
    Voice = 0,
    Aprs = 1,
    /// Receive only: never transmits (SAT-13).
    Beacon = 2,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Voice => "Voice",
            Kind::Aprs => "APRS",
            Kind::Beacon => "Beacon",
        }
    }
}

/// One transponder (16 bytes in the record, SAT-2): frequencies in 10 Hz (0 uplink = receive only), the access
/// and arming tones in 0.1 Hz (0 none; SAT-10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Transponder {
    pub up: Freq10,
    pub down: Freq10,
    pub tone: u16,
    pub arm: u16,
    pub kind: Kind,
}

impl Transponder {
    /// SAT-13: both frequencies (where set) inside 144-148 or 430-440 MHz.
    pub fn in_band(&self) -> bool {
        let ok = |f: Freq10| (14_400_000..=14_800_000).contains(&f) || (43_000_000..=44_000_000).contains(&f);
        ok(self.down) && (self.up == 0 || ok(self.up))
    }
    /// Can transmit: an uplink, not a beacon.
    pub fn tx(&self) -> bool {
        self.up != 0 && self.kind != Kind::Beacon
    }
}

/// The most transponders a satellite keeps (SAT-2).
pub const TPS: usize = 5;

/// The status the web app sends (SAT-2): active, inactive, unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Status {
    #[default]
    Active = 0,
    Inactive = 1,
    Unknown = 2,
}

/// One satellite as the screens and the predictor read it (the record of SAT-2, phase 2's flash form).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sat {
    /// NORAD catalogue number.
    pub id: u32,
    /// Short name, NUL-padded.
    pub name: [u8; 16],
    pub status: Status,
    pub el: Elements,
    pub tps: [Transponder; TPS],
    /// How many of `tps` are used.
    pub ntp: u8,
    /// The starting choices the web app sets (SAT-8, SAT-11): [`Sat::HD`], [`Sat::UHF_ONLY`].
    pub flags: u8,
}

impl Sat {
    /// Half duplex to start with (SAT-11).
    pub const HD: u8 = 1;
    /// Doppler on the 70 cm side only (SAT-8).
    pub const UHF_ONLY: u8 = 2;

    pub fn name(&self) -> &[u8] {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(16);
        &self.name[..n]
    }
    /// The ISS (NORAD 25544): its elements age faster (reboosts, SAT-9).
    pub fn iss(&self) -> bool {
        self.id == 25_544
    }
    /// SAT-9: 0 fresh, 1 amber (past 7 days, 3 for the ISS), 2 red (past 30 days).
    pub fn age_level(&self, now: u32) -> u8 {
        let d = self.el.age_days(now);
        if d > 30 {
            2
        } else if d > if self.iss() { 3 } else { 7 } {
            1
        } else {
            0
        }
    }
    pub fn tp(&self, i: u8) -> Option<&Transponder> {
        self.tps[..self.ntp.min(TPS as u8) as usize].get(i as usize)
    }
}

/// Where the satellites come from: the store's SPI flash region on the radio (SAT-2, written over the link or by
/// the terminal's `:sat`), a list in RAM in the tests and the simulator's fixture.
pub trait SatSource {
    fn count(&self) -> u8;
    fn get(&self, i: u8) -> Option<Sat>;
    /// Bumped whenever the list or any elements change (SAT-6 predicts again).
    fn version(&self) -> u32 {
        0
    }
    /// SAT-18: the magnetic declination at the radio in 0.1° (east positive), written by Console with the
    /// satellites.
    fn declination(&self) -> Option<i16> {
        None
    }
    /// Satellite `i`'s choices as kept (None: none kept, its flags' starting ones apply).
    fn prefs(&self, _i: u8) -> Option<data::Prefs> {
        None
    }
    /// Keeps satellite `i`'s choices (the store writes them a little later: a turning knob doesn't wear the flash).
    fn set_prefs(&self, _i: u8, _p: data::Prefs) {}
}

/// No satellites (the default until something loads them).
pub struct NoSats;

impl SatSource for NoSats {
    fn count(&self) -> u8 {
        0
    }
    fn get(&self, _i: u8) -> Option<Sat> {
        None
    }
}

/// Satellites in a slice.
pub struct List<'a>(pub &'a [Sat]);

impl SatSource for List<'_> {
    fn count(&self) -> u8 {
        self.0.len().min(255) as u8
    }
    fn get(&self, i: u8) -> Option<Sat> {
        self.0.get(i as usize).copied()
    }
}

#[cfg(test)]
mod oracle;
