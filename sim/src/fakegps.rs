//! The simulator's GPS module (manual chapter 28, GPS-26 to GPS-33): a fake ATGM336H run by the firmware's own
//! power manager ([`ferrite_core::gpspower`]) on the demand the firmware's UI task works out
//! ([`ferrite_app::gpsdata::demand`]). Off unless asked for ([`crate::simcore::SimCore::gps_model`]): the screens
//! keep the sample's fix, and the status bar's GPS icon and the Status page follow this module only while it runs.
//!
//! The module: a fix [`HOT_MS`] into a wake from standby (VCC kept: a hot start) or [`COLD_MS`] after power-on
//! (the backup supply is unknown, so the worst case), none at all indoors; `$PCAS12,N` puts it in standby and it
//! wakes itself N s later; the wake byte wakes it (to confirm on the radio, GPS-31) unless told otherwise.

use ferrite_app::gpsdata::RECENT_MS;
use ferrite_core::gpspower::{casic, GpsDemand, GpsManager, GpsPhase, CAS_STANDBY};
use ferrite_core::msg::GpsState;

/// A fix this long into a wake from standby (the datasheet's hot start, 1 s or less, plus margin).
pub const HOT_MS: u32 = 2_000;
/// A fix this long after power-on (the datasheet's cold start, 35 s or less).
pub const COLD_MS: u32 = 30_000;

pub struct FakeGps {
    pub mgr: GpsManager,
    /// No sky: sentences but never a fix.
    pub indoor: bool,
    /// The wake byte ends standby (the bench will tell; GPS-31).
    pub poke_wakes: bool,
    powered: bool,
    /// Awake since then, and how long its fix takes from then.
    awake: Option<(u32, u32)>,
    asleep_until: Option<u32>,
    last_sentence: u32,
    last_fix: Option<u32>,
    /// What went to the module, and when (ms): the CAS12 sentences, "wake byte", "power on", "power off".
    pub log: Vec<(u32, String)>,
}

impl Default for FakeGps {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeGps {
    pub fn new() -> Self {
        FakeGps { mgr: GpsManager::new(), indoor: false, poke_wakes: true, powered: false, awake: None, asleep_until: None, last_sentence: 0, last_fix: None, log: Vec::new() }
    }

    /// One look at `now` (ms), as the firmware's GPS task makes them.
    pub fn step(&mut self, now: u32, d: &GpsDemand, keyed: bool) {
        if self.asleep_until.is_some_and(|u| now >= u) {
            (self.asleep_until, self.awake) = (None, Some((now, HOT_MS)));
        }
        let out = self.mgr.step(now, d, keyed);
        if out.power != self.powered {
            self.powered = out.power;
            self.mgr.powered(now, out.power);
            self.log.push((now, String::from(if out.power { "power on" } else { "power off" })));
            (self.awake, self.asleep_until) = (out.power.then_some((now, COLD_MS)), None);
            if !out.power && !d.on {
                self.last_fix = None;
            }
        }
        if let Some(s) = out.standby_s {
            let mut b = [0u8; ferrite_core::gpspower::CASIC_MAX];
            let n = casic(CAS_STANDBY, s, &mut b);
            self.log.push((now, String::from_utf8_lossy(&b[..n - 2]).into_owned()));
            (self.awake, self.asleep_until) = (None, Some(now + s as u32 * 1000));
        }
        if out.poke {
            self.log.push((now, String::from("wake byte")));
            if self.poke_wakes && self.asleep_until.is_some() {
                (self.asleep_until, self.awake) = (None, Some((now, HOT_MS)));
            }
        }
        // A report a second while awake.
        if let Some((at, fix_ms)) = self.awake.filter(|_| self.powered && now.wrapping_sub(self.last_sentence) >= 1000) {
            self.last_sentence = now;
            if !self.indoor && now - at >= fix_ms {
                self.mgr.fix(now, 0);
                self.last_fix = Some(now);
            } else {
                self.mgr.sentence(now);
            }
        }
    }

    /// The status bar's state (GPS-6): off, solid with a fix in the last [`RECENT_MS`], else searching.
    pub fn state(&self, now: u32, on: bool) -> (GpsState, u8) {
        match (on, self.last_fix) {
            (false, _) => (GpsState::Off, 0),
            (true, Some(f)) if now.wrapping_sub(f) < RECENT_MS => (GpsState::Fix, 9),
            _ => (GpsState::Searching, 0),
        }
    }

    pub fn phase(&self, now: u32) -> GpsPhase {
        self.mgr.phase(now)
    }

    /// The module is in standby now.
    pub fn asleep(&self) -> bool {
        self.asleep_until.is_some()
    }

    /// The CAS12 standbys sent: (ms, seconds).
    pub fn standbys(&self) -> Vec<(u32, u16)> {
        self.log.iter().filter_map(|(t, s)| s.strip_prefix("$PCAS12,").and_then(|r| r.split('*').next()).and_then(|n| n.parse().ok()).map(|n| (*t, n))).collect()
    }
}
