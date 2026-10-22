//! The GPS as the screens read it (manual chapter 28): the GPS task's latest [`GpsFix`] and [`GpsSky`] turned
//! into the [`Data`] answers the GPS pages, Nearby, waypoints and APRS use, with the fix's age (GPS-2), the
//! fixed position when there is no fix (GPS-3), the clock from GPS time (GPS-7), and the arrival alarm
//! (GPS-12). The radio's UI task fills one from the `POSITION` and `SKY` watches; it holds about 160 bytes.
//! With the power manager (GPS-26) the module sleeps between fixes, so the last fix is kept with its age and is
//! the position for [`RECENT_MS`] (GPS-25); [`demand`] says who needs the module awake.

use crate::data::{Data, Fix, Pos, Sat};
use crate::settings::TIME_ZONES;
use core::cell::Cell;
use ferrite_core::gpspower::GpsDemand;
use ferrite_core::msg::{GpsFix, GpsSky, GpsState};

/// The age a stand-in fix from the fixed position carries (GPS-3): "Mark here" refuses it, as it isn't a fix.
pub const FIXED_AGE: u16 = u16::MAX;

/// A fix older than this is not shown as current (the position page turns its age amber past 10 s, GPS-15).
pub const STALE_MS: u32 = 10_000;

/// The last fix is the position this long after the module reported it (GPS-25): the status bar's icon solid,
/// Nearby and APRS taking it, its age shown. The module wakes once a minute (GPS-26), so this spans a couple of
/// wakes that found no sky.
pub const RECENT_MS: u32 = 180_000;

/// What the GPS task is asked to do (GPS-1, GPS-4): the module on or off, and off around signals
/// (0 always on, 1 off while receiving, 2 off while transmitting, 3 both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpsCfg {
    pub on: bool,
    pub rxtx: u8,
}

impl GpsCfg {
    pub const DEFAULT: GpsCfg = GpsCfg { on: true, rxtx: 0 };

    /// Whether the module is powered now, given what the radio is doing.
    pub fn powered(&self, receiving: bool, keyed: bool) -> bool {
        self.on && !(receiving && self.rxtx & 1 != 0) && !(keyed && self.rxtx & 2 != 0)
    }
}

/// What the settings ask of the GPS task (0 6 1, 0 6 8).
pub fn cfg(set: &crate::settings::Settings) -> GpsCfg {
    GpsCfg { on: set.gps_on, rxtx: set.gps_rxtx & 3 }
}

/// Who needs the GPS module awake now (GPS-26 to GPS-28), for the GPS task's power manager: GPS on (0 6 1); a GPS
/// screen open (position, satellites, waypoints, go to) or the arrival alarm armed (`alarm`); smart or distance
/// beaconing; the next timed beacon, `now_ms` plus its seconds (APRS mode on). `sky` (GPS-34): the position or
/// satellites page is open, the two that show satellites in view (from GSV and GSA).
pub fn demand(app: &crate::App, data: &dyn Data, alarm: bool, now_ms: u32) -> GpsDemand {
    use crate::screens::gps::Page;
    let watch = alarm || matches!(app.screen(), crate::Screen::Gps(_));
    let sky = matches!(app.screen(), crate::Screen::Gps(g) if matches!(g.page, Page::Position | Page::Satellites));
    let (mut track, mut beacon_at) = (false, None);
    if app.aprs_on() {
        let c = data.aprs_cfg();
        track = c.smart() || c.every_m().is_some();
        // Due now or overdue (waiting for a fix, the relay or a callsign) holds nothing: the minute wakes bring the fix.
        beacon_at = data.beacon().next_s.filter(|&s| s > 0).map(|s| now_ms.wrapping_add(s.saturating_mul(1000)));
    }
    GpsDemand { on: app.settings().gps_on, watch, track, beacon_at, sky }
}

/// The arrival alarm's radius in metres, 0 when off (0 6 7, GPS-12).
pub fn arrival_radius_m(set: &crate::settings::Settings) -> u32 {
    crate::settings::ARRIVAL_M[set.arrival as usize % crate::settings::ARRIVAL_M.len()] as u32
}

/// The GPS's latest, read by the screens through the store (the radio's [`Data`] owner).
pub struct GpsData {
    fix: Cell<GpsFix>,
    /// The last report with a position (kept while the module sleeps or searches again; gone with GPS off).
    good: Cell<Option<GpsFix>>,
    sky: Cell<GpsSky>,
    /// Milliseconds now, the same clock as [`GpsFix::at_ms`].
    now: Cell<u32>,
    /// The fixed position (0 6 9), used when there is no fix (GPS-3).
    fixed: Cell<Option<Pos>>,
    ver: Cell<u32>,
}

impl Default for GpsData {
    fn default() -> Self {
        Self::new()
    }
}

impl GpsData {
    pub const fn new() -> Self {
        GpsData {
            fix: Cell::new(GpsFix { on: false, fix: 0, used: 0, view: 0, lat: 0, lon: 0, alt_dm: 0, speed_kn10: 0, course10: 0, hdop10: 0, unix: 0, at_ms: 0 }),
            good: Cell::new(None),
            sky: Cell::new(GpsSky { n: 0, sat: [ferrite_core::msg::SkySat { prn: 0, snr: 0, elev: 0, azim_2: 0, flags: 0 }; 16] }),
            now: Cell::new(0),
            fixed: Cell::new(None),
            ver: Cell::new(0),
        }
    }

    /// A new fix from the GPS task.
    pub fn set_fix(&self, f: GpsFix) {
        if f != self.fix.get() {
            self.fix.set(f);
            if f.has_fix() {
                self.good.set(Some(f));
            } else if !f.on {
                self.good.set(None);
            }
            self.bump();
        }
    }

    pub fn set_sky(&self, s: GpsSky) {
        if s != self.sky.get() {
            self.sky.set(s);
            self.bump();
        }
    }

    /// The fixed position from the settings (None: none set).
    pub fn set_fixed(&self, p: Option<Pos>) {
        if p != self.fixed.get() {
            self.fixed.set(p);
            self.bump();
        }
    }

    /// Time passes: a fix going stale is a change the screens see.
    pub fn tick(&self, now_ms: u32) {
        let was = self.current();
        self.now.set(now_ms);
        if self.current() != was {
            self.bump();
        }
    }

    fn bump(&self) {
        self.ver.set(self.ver.get().wrapping_add(1));
    }

    /// The raw fix as the GPS task sent it.
    pub fn raw(&self) -> GpsFix {
        self.fix.get()
    }

    /// The last fix while it is recent (GPS-25): reported within [`RECENT_MS`].
    fn recent(&self) -> Option<GpsFix> {
        self.good.get().filter(|f| self.fix.get().on && self.now.get().wrapping_sub(f.at_ms) < RECENT_MS)
    }

    /// Whether there is a recent fix (what a tick watches change).
    fn current(&self) -> bool {
        self.recent().is_some()
    }

    /// The status bar's GPS state (GPS-6): solid with a fix in the last [`RECENT_MS`], not while the module
    /// merely runs; and the satellites the fix used.
    pub fn state(&self) -> (GpsState, u8) {
        let f = self.fix.get();
        match (f.on, self.recent()) {
            (false, _) => (GpsState::Off, 0),
            (true, None) => (GpsState::Searching, f.used),
            (true, Some(g)) => (GpsState::Fix, g.used),
        }
    }

    /// UTC seconds since 1970 now from GPS time (None until the module has sent a time and date): the report's
    /// time plus how long ago it came (the last fix's while the module sleeps).
    pub fn utc_now(&self) -> Option<u32> {
        let f = Some(self.fix.get()).filter(|f| f.unix != 0).or(self.good.get().filter(|g| g.unix != 0))?;
        Some(f.unix.wrapping_add(self.now.get().wrapping_sub(f.at_ms) / 1000))
    }

    /// The position users take (GPS-3): a recent fix, else the fixed position, else none.
    pub fn position(&self) -> Option<Pos> {
        if let Some(f) = self.recent() {
            return Some(Pos { lat: f.lat, lon: f.lon });
        }
        self.fixed.get()
    }
}

/// Minutes since local midnight for UTC seconds `utc` in time zone `tz` (an index into [`TIME_ZONES`]), and
/// whether that is UTC itself (the bar shows `Z`, GPS-8).
pub fn local_clock(utc: u32, tz: u8) -> (u16, bool) {
    let q = TIME_ZONES[tz as usize % TIME_ZONES.len()] as i32;
    let min = (utc / 60) as i64 + q as i64 * 15;
    (min.rem_euclid(1440) as u16, q == 0)
}

/// A clock typed by hand (GPS-23): 12 digits YYYYMMDDhhmm, years 2000-2099, as seconds since 1970 in the
/// same time it was typed in (local); None if it isn't a real date and time.
pub fn parse_clock(t: &[u8]) -> Option<u32> {
    if t.len() != 12 || !t.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = |a: usize, b: usize| t[a..b].iter().fold(0u32, |v, &c| v * 10 + (c - b'0') as u32);
    let (y, m, d, hh, mm) = (n(0, 4), n(4, 6), n(6, 8), n(8, 10), n(10, 12));
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(2000..=2099).contains(&y) || !(1..=12).contains(&m) || d == 0 || d > days[m as usize - 1] {
        return None;
    }
    ferrite_core::gps::unix_time(d * 10_000 + m * 100 + y % 100, hh * 10_000 + mm * 100)
}

/// Local seconds (as [`parse_clock`] gives them) back to UTC for time zone `tz` (an index into [`TIME_ZONES`]).
pub fn local_to_utc(local: u32, tz: u8) -> u32 {
    let q = TIME_ZONES[tz as usize % TIME_ZONES.len()] as i32;
    (local as i64 - q as i64 * 900) as u32
}

/// Metres between two positions (equirectangular: plenty for the arrival radii of 50 m to 1 km).
pub fn metres(a: Pos, b: Pos) -> u32 {
    let lat = ((a.lat as f32 + b.lat as f32) * 0.5e-7).to_radians();
    let dy = (a.lat - b.lat) as f32 * 1e-7;
    let dx = (a.lon - b.lon) as f32 * 1e-7 * cos(lat);
    let deg = sqrt(dx * dx + dy * dy);
    (deg * 111_195.0) as u32
}

// no_std: the two float helpers the arrival check needs.
fn cos(x: f32) -> f32 {
    // Bhaskara-free: a short Taylor series is plenty for |x| <= 90 degrees.
    let x2 = x * x;
    1.0 - x2 / 2.0 + x2 * x2 / 24.0 - x2 * x2 * x2 / 720.0
}

fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut r = x.max(1e-12);
    for _ in 0..20 {
        r = 0.5 * (r + x / r);
    }
    r
}

/// The arrival alarm (GPS-12): fires once when a current fix comes within `radius_m` of the waypoint being
/// gone to, and is armed again by a new target or by leaving twice the radius.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arrival {
    target: Option<(u16, Pos)>,
    fired: bool,
}

impl Arrival {
    /// The waypoint being gone to (its index and position); None stops watching.
    pub fn target(&mut self, t: Option<(u16, Pos)>) {
        if t.map(|t| t.0) != self.target.map(|t| t.0) {
            *self = Arrival { target: t, fired: false };
        }
    }

    /// Watching a waypoint: the module stays awake (GPS-28) while the radius is on.
    pub fn armed(&self, radius_m: u32) -> bool {
        self.target.is_some() && radius_m > 0
    }

    /// A new position: Some(waypoint index, metres) when the alarm fires.
    pub fn check(&mut self, here: Pos, radius_m: u32) -> Option<(u16, u32)> {
        let (i, at) = self.target?;
        if radius_m == 0 {
            return None;
        }
        let d = metres(here, at);
        if !self.fired && d <= radius_m {
            self.fired = true;
            return Some((i, d));
        }
        if self.fired && d > radius_m * 2 {
            self.fired = false;
        }
        None
    }
}

impl Data for GpsData {
    fn version(&self) -> u32 {
        self.ver.get()
    }

    fn fix(&self) -> Option<Fix> {
        if let Some(f) = self.recent() {
            let age = self.now.get().wrapping_sub(f.at_ms) / 1000;
            return Some(Fix {
                pos: Pos { lat: f.lat, lon: f.lon },
                three_d: f.fix >= 3,
                alt_m: f.alt_dm / 10,
                speed_kmh: ((f.speed_kn10 as u32 * 1852 + 5000) / 10_000) as u16,
                heading: f.course10 / 10,
                hdop_x10: f.hdop10,
                used: f.used,
                age_s: age.min(u16::MAX as u32) as u16,
                utc_s: self.utc_now().map_or(0, |u| u % 86_400),
            });
        }
        // GPS-3: the fixed position stands in, as an old 2D fix with nothing heard.
        self.fixed.get().map(|pos| Fix { pos, three_d: false, alt_m: 0, speed_kmh: 0, heading: 0, hdop_x10: 0, used: 0, age_s: FIXED_AGE, utc_s: 0 })
    }

    fn sats(&self) -> u8 {
        if self.fix.get().on { self.sky.get().n } else { 0 }
    }

    fn sat(&self, i: u8) -> Option<Sat> {
        let sky = self.sky.get();
        if i >= sky.n || !self.fix.get().on {
            return None;
        }
        let s = sky.sat[i as usize];
        Some(Sat { prn: s.prn, snr: s.snr, used: s.flags & 1 != 0, beidou: s.flags & 2 != 0, elev: s.elev, azim: s.azim_2 as u16 * 2 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix_at(at_ms: u32) -> GpsFix {
        GpsFix { on: true, fix: 3, used: 9, view: 15, lat: 454_215_833, lon: -756_976_667, alt_dm: 712, speed_kn10: 126, course10: 873, hdop10: 9, unix: 1_791_383_530, at_ms }
    }

    /// GPS-25: a fix is the position for 3 minutes after the module last reported it, with its age; after that the
    /// state reads searching and no fix is offered. A report without a fix (the module waking, searching) keeps
    /// the last one; GPS off drops it.
    #[test]
    fn fix_age_and_state() {
        let g = GpsData::new();
        assert_eq!(g.state().0, GpsState::Off);
        assert!(g.fix().is_none());
        g.set_fix(fix_at(1000));
        g.tick(3000);
        let f = g.fix().unwrap();
        assert_eq!((f.age_s, f.alt_m, f.speed_kmh, f.heading, f.used, f.three_d), (2, 71, 23, 87, 9, true));
        assert_eq!(f.utc_s, (1_791_383_530 + 2) % 86_400);
        assert_eq!(g.state(), (GpsState::Fix, 9));
        g.tick(61_000); // the module asleep a minute: the fix stands, a minute old
        assert_eq!(g.fix().unwrap().age_s, 60);
        assert_eq!(g.state(), (GpsState::Fix, 9));
        g.set_fix(GpsFix { on: true, used: 2, at_ms: 62_000, ..GpsFix::default() }); // awake, no fix yet
        g.tick(62_000);
        assert_eq!(g.fix().unwrap().age_s, 61, "the last fix kept");
        assert_eq!(g.utc_now(), Some(1_791_383_530 + 61), "the clock from the last fix's time");
        let v = g.version();
        g.tick(181_500); // past RECENT_MS: no longer current, and the screens hear of it
        assert_ne!(g.version(), v);
        assert!(g.fix().is_none());
        assert_eq!(g.state(), (GpsState::Searching, 2));
        g.set_fix(fix_at(182_000));
        g.tick(182_000);
        assert!(g.fix().is_some());
        g.set_fix(GpsFix::default()); // GPS off
        assert!(g.fix().is_none());
        assert_eq!(g.state().0, GpsState::Off);
    }

    /// GPS-3: with no current fix, position users take the fixed position, or none; a current fix wins.
    #[test]
    fn fixed_position_stands_in() {
        let g = GpsData::new();
        assert_eq!(g.position(), None, "no fix and no fixed position: none");
        let p = Pos { lat: 1, lon: 2 };
        g.set_fixed(Some(p));
        assert_eq!(g.position(), Some(p));
        assert_eq!(g.fix().unwrap().pos, p);
        assert_eq!(g.fix().unwrap().age_s, u16::MAX, "marked as old");
        g.set_fix(fix_at(0));
        g.tick(0);
        assert_eq!(g.position(), Some(Pos { lat: 454_215_833, lon: -756_976_667 }), "a current fix wins");
    }

    #[test]
    fn clock_before_a_fix() {
        let g = GpsData::new();
        g.set_fix(GpsFix { on: true, unix: 1_791_383_400, at_ms: 500, ..GpsFix::default() });
        g.tick(60_500);
        assert_eq!(g.utc_now(), Some(1_791_383_460));
        assert_eq!(local_clock(1_791_383_460, crate::settings::TZ_DEFAULT), ((14 * 60 + 31), true));
        // UTC-4 (index 9 = -16 quarter hours): 10:31, not UTC
        assert_eq!(TIME_ZONES[9], -16);
        assert_eq!(local_clock(1_791_383_460, 9), ((10 * 60 + 31), false));
        // across midnight
        assert_eq!(local_clock(60, 9).0, 20 * 60 + 1);
    }

    /// GPS-4: GPS during RX/TX powers the module off for the chosen cases (receiving, transmitting, both) and
    /// on otherwise.
    #[test]
    fn powered_around_signals() {
        let c = GpsCfg { on: true, rxtx: 1 };
        assert!(c.powered(false, true));
        assert!(!c.powered(true, false));
        assert!(!GpsCfg { on: true, rxtx: 3 }.powered(false, true));
        assert!(!GpsCfg { on: false, rxtx: 0 }.powered(false, false));
        assert!(GpsCfg::DEFAULT.powered(true, true));
    }

    /// GPS-12: the arrival alarm fires once inside the radius, and again only after leaving it.
    #[test]
    fn arrival_fires_once() {
        let wp = Pos { lat: 454_215_833, lon: -756_976_667 };
        let mut a = Arrival::default();
        assert_eq!(a.check(wp, 100), None, "no target");
        a.target(Some((3, wp)));
        let far = Pos { lat: wp.lat + 50_000, lon: wp.lon }; // about 556 m north
        assert_eq!(a.check(far, 100), None);
        let near = Pos { lat: wp.lat + 5_000, lon: wp.lon }; // about 56 m
        let (i, d) = a.check(near, 100).unwrap();
        assert_eq!(i, 3);
        assert!((50..=62).contains(&d), "{d}");
        assert_eq!(a.check(near, 100), None, "once");
        assert_eq!(a.check(far, 100), None); // beyond twice the radius: armed again
        assert!(a.check(near, 100).is_some());
        assert_eq!(a.check(near, 0), None, "radius off");
        let east = Pos { lat: wp.lat, lon: wp.lon + 10_000 }; // 0.001 deg at 45.4 N is about 78 m
        assert!((74..=82).contains(&metres(wp, east)), "{}", metres(wp, east));
    }

    /// GPS-26 to GPS-28: who needs the module: GPS off, a GPS screen or the alarm, smart beaconing, the next
    /// timed beacon (APRS mode on only); GPS-34.3: the position or satellites page asks for GSV every fix.
    #[test]
    fn demand_from_the_app() {
        struct D(crate::aprs::AprsCfg, Option<u32>);
        impl Data for D {
            fn version(&self) -> u32 {
                0
            }
            fn aprs_cfg(&self) -> crate::aprs::AprsCfg {
                self.0
            }
            fn beacon(&self) -> crate::data::Beacon {
                crate::data::Beacon { waiting: false, next_s: self.1, ..Default::default() }
            }
        }
        let mut app = crate::App::new(crate::tests::set(), crate::tests::alert());
        let timed = D(crate::aprs::AprsCfg { beacon: 2 + 3, ..crate::aprs::AprsCfg::DEFAULT }, Some(400)); // 10 minutes
        let d = demand(&app, &timed, false, 1000);
        assert_eq!(d, GpsDemand { on: true, watch: false, track: false, beacon_at: None, sky: false }, "APRS mode off: no beacon");
        app.set_aprs_mode(true);
        assert_eq!(demand(&app, &timed, false, 1000).beacon_at, Some(401_000));
        assert!(demand(&app, &timed, true, 1000).watch, "the arrival alarm");
        let smart = D(crate::aprs::AprsCfg { beacon: crate::aprs::SMART as u8, ..crate::aprs::AprsCfg::DEFAULT }, None);
        assert!(demand(&app, &smart, false, 0).track);
        app.open(crate::screens::nav::Dest::GpsPosition);
        assert!(demand(&app, &smart, false, 0).watch, "the GPS screen");
        // GPS-34.3: the position and satellites pages show satellites in view: GSV every fix; the waypoints don't.
        assert!(demand(&app, &smart, false, 0).sky, "the position page");
        app.screen = crate::Screen::Gps(crate::screens::gps::Gps::new(crate::screens::gps::Page::Satellites));
        assert!(demand(&app, &smart, false, 0).sky, "the satellites page");
        app.open(crate::screens::nav::Dest::Waypoints);
        let d = demand(&app, &smart, false, 0);
        assert!(d.watch && !d.sky, "the waypoints: running, GSV every 5th");
        app.settings_mut().gps_on = false;
        assert!(!demand(&app, &smart, false, 0).on);
    }

    #[test]
    fn settings_reach_the_gps() {
        let mut set = crate::settings::Settings::DEFAULT;
        assert_eq!(cfg(&set), GpsCfg::DEFAULT, "on and always on by default (GPS-14)");
        assert_eq!(arrival_radius_m(&set), 0, "arrival alarm off by default");
        (set.gps_on, set.gps_rxtx, set.arrival) = (false, 2, 2);
        assert_eq!(cfg(&set), GpsCfg { on: false, rxtx: 2 });
        assert_eq!(arrival_radius_m(&set), 100);
    }

    /// GPS-23: a typed clock: real dates only, and back to UTC by the zone.
    #[test]
    fn clock_by_hand() {
        let t = parse_clock(b"202610081430").expect("a real date");
        assert_eq!(t, ferrite_core::gps::unix_time(81_026, 143_000).unwrap());
        assert!(parse_clock(b"202602300000").is_none(), "no 30 February");
        assert!(parse_clock(b"202402290000").is_some(), "2024 is a leap year");
        assert!(parse_clock(b"202613010000").is_none());
        assert!(parse_clock(b"202610082460").is_none());
        assert!(parse_clock(b"2026100814").is_none(), "12 digits");
        let utc0 = TIME_ZONES.iter().position(|&q| q == 0).unwrap() as u8;
        assert_eq!(local_to_utc(t, utc0), t);
        if let Some(east) = TIME_ZONES.iter().position(|&q| q == 4) {
            assert_eq!(local_to_utc(t, east as u8), t - 3600, "UTC+1: local is an hour ahead");
        }
    }
}
