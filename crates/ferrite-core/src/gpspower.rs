//! The GPS power manager (manual chapter 28, GPS-26 to GPS-33; `docs/research/gps-power.md`): with GPS on (0 6 1)
//! the ATGM336H is not left running. Whoever needs a position says so ([`GpsDemand`]); this decides, from that and
//! from what the module reports, whether it runs, sleeps in CASIC standby (`$PCAS12,N`: VCC kept, it wakes itself
//! after N s, CASIC protocol 2.7.11) or is off (PA8). Pure logic: the firmware's GPS task and the simulator's fake
//! module drive it, the host tests below pin the rules.
//!
//! - **Wakes:** once a minute until a fix, then standby to the next wake (GPS-26).
//! - **Beacons:** with a timed APRS beacon the wake comes [`BEACON_LEAD_MS`] before it; the minute wakes go on
//!   between (GPS-27).
//! - **Continuous:** a GPS screen or the arrival alarm keeps it running; smart and distance beaconing do while
//!   moving, and hand back to the wakes [`STOPPED_MS`] after the last movement (GPS-28).
//! - **Ephemeris:** about every [`EPH_EVERY_MS`] a wake stays on [`EPH_HOLD_MS`] after its fix (GPS-29).
//! - **No sky:** a wake with no fix in [`NO_SKY_MS`] sleeps for 1, 2, 4, 8, then 15 minutes at most; a fix resets
//!   it (GPS-30).
//! - **Unplanned wake:** a need that comes during standby wakes the module with a byte on its UART; with no NMEA
//!   back in [`POKE_WAIT_MS`] the fallback ([`GpsManager::wake_fallback`]) cycles PA8 (GPS-31: to confirm on the
//!   radio which of the two works).
//! - **Transmit:** nothing is sent to the module while keyed (PB10 is held low then, GPS-22): a standby, a wake
//!   byte or a power cycle due then waits for the unkey (GPS-32).
//! - **Ferrite's NMEA set (GPS-34, [`NmeaSet`]):** once the module is heard after each power-up and wake, `$PCAS03`
//!   cuts its output to what the parser reads: GGA and RMC every fix, GSV and GSA every 5th, or every fix while a
//!   screen that shows satellites is open; nothing else.

/// The module's self-wake every minute while nothing else asks (GPS-26).
pub const WAKE_EVERY_MS: u32 = 60_000;
/// A timed beacon's wake comes this long before it (GPS-27).
pub const BEACON_LEAD_MS: u32 = 30_000;
/// A beacon this far overdue no longer holds the module on (the APRS task gives up waiting by then, GPS-33).
pub const BEACON_GRACE_MS: u32 = 120_000;
/// Smart or distance beaconing: still this long after the last movement, back to the wakes (GPS-28).
pub const STOPPED_MS: u32 = 180_000;
/// Moving: at least this speed (SmartBeaconing's slow speed).
pub const MOVING_KMH: u16 = 5;
/// Ephemeris refresh (GPS-29): this often, a wake stays on this long after its fix.
pub const EPH_EVERY_MS: u32 = 30 * 60_000;
pub const EPH_HOLD_MS: u32 = 60_000;
/// No fix this long after a wake: back off (GPS-30).
pub const NO_SKY_MS: u32 = 120_000;
/// The back-off waits in seconds: doubling, then the last for good.
pub const BACKOFF_S: [u16; 5] = [60, 120, 240, 480, 900];
/// The shortest standby worth sending; a nearer wake keeps the module on.
pub const MIN_STANDBY_S: u32 = 10;
/// After a standby command, sentences already on their way are not a wake.
pub const SETTLE_MS: u32 = 2_000;
/// An unplanned wake: how long the wake byte gets to bring NMEA back (GPS-31).
pub const POKE_WAIT_MS: u32 = 3_000;
/// A module that should be running and sends nothing this long gets the fallback too.
pub const SILENT_MS: u32 = 5_000;
/// The power cycle's off time (PA8).
pub const CYCLE_OFF_MS: u32 = 500;
/// The wake byte (CR LF: harmless to any NMEA reader).
pub const POKE: &[u8] = b"\r\n";

/// Who needs a position now (worked out by the UI task, `ferrite_app::gpsdata::demand`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpsDemand {
    /// GPS on (0 6 1); off: PA8 off, as stock.
    pub on: bool,
    /// A GPS screen (position, satellites, waypoints, go to) or the arrival alarm: continuous.
    pub watch: bool,
    /// Smart or distance beaconing: continuous while moving.
    pub track: bool,
    /// The next timed beacon, in the manager's milliseconds.
    pub beacon_at: Option<u32>,
    /// A screen that shows satellites (GPS position or satellites page) is open: GSV and GSA every fix (GPS-34).
    pub sky: bool,
}

/// What the module is doing, as the manager sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Module {
    /// Unpowered (GPS off, or off around a signal, GPS-4).
    #[default]
    Off,
    /// Powered and awake since then; `heard` once a sentence came this run.
    Running { since: u32, heard: bool },
    /// In standby from `at` to `until` (its own wake). `ignored`: NMEA kept coming after the command (the module
    /// didn't sleep, or woke at once): the plan holds to `until` all the same, so a module that won't sleep isn't
    /// sent a standby every fix (GPS-26, decision 143).
    Standby { at: u32, until: u32, ignored: bool },
    /// An unplanned wake: the byte went at `at`.
    Poked { at: u32 },
    /// The fallback: PA8 off since `at`, on again after [`CYCLE_OFF_MS`].
    Cycling { at: u32 },
}

/// What the GPS task does to the module on one look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpsOut {
    /// PA8 wanted on (the GPS task's GPS-4 rule may still hold it off around a signal).
    pub power: bool,
    /// Send `$PCAS12,N` ([`casic`]).
    pub standby_s: Option<u16>,
    /// Send the wake byte ([`POKE`]).
    pub poke: bool,
    /// The fallback cycled PA8 this look (for the cable log).
    pub cycled: bool,
}

/// The manager's state between looks: about 40 bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct GpsManager {
    module: Module,
    /// The first fix of this run.
    fix_at: Option<u32>,
    /// The last ephemeris refresh (a run on for [`EPH_HOLD_MS`] after its fix).
    eph_at: Option<u32>,
    /// Wakes in a row with no fix (GPS-30).
    misses: u8,
    /// The last fix at [`MOVING_KMH`] or more.
    moved_at: Option<u32>,
    /// The fallback ran with nothing heard since: not again until a sentence comes.
    cycled_silent: bool,
}

/// `now` is at or after `t` (milliseconds that wrap).
fn reached(now: u32, t: u32) -> bool {
    (now.wrapping_sub(t) as i32) >= 0
}

impl GpsManager {
    pub const fn new() -> Self {
        GpsManager { module: Module::Off, fix_at: None, eph_at: None, misses: 0, moved_at: None, cycled_silent: false }
    }

    /// One look at `now`: `d` is the demand, `keyed` while any chip transmits.
    #[inline(never)] // in .slow_text (memory.x): the GPS task's, a few times a second, never from an interrupt
    pub fn step(&mut self, now: u32, d: &GpsDemand, keyed: bool) -> GpsOut {
        let mut out = GpsOut { power: d.on, ..GpsOut::default() };
        if !d.on {
            *self = GpsManager { eph_at: self.eph_at, ..GpsManager::new() };
            return out;
        }
        if let (Some(f), Module::Running { .. }) = (self.fix_at, self.module) {
            if now.wrapping_sub(f) >= EPH_HOLD_MS {
                self.eph_at = Some(now);
            }
        }
        let continuous = d.watch || d.track && self.moved_at.is_some_and(|m| now.wrapping_sub(m) < STOPPED_MS);
        // A timed beacon about to go (or just overdue, the APRS task waiting for this fix).
        let beacon_soon = d.beacon_at.is_some_and(|b| reached(now.wrapping_add(BEACON_LEAD_MS + MIN_STANDBY_S * 1000), b) && !reached(now, b.wrapping_add(BEACON_GRACE_MS)));
        match self.module {
            Module::Off => {}
            Module::Cycling { at } => out.power = now.wrapping_sub(at) >= CYCLE_OFF_MS,
            Module::Poked { at } => {
                if now.wrapping_sub(at) >= POKE_WAIT_MS && !keyed {
                    self.wake_fallback(now, &mut out);
                }
            }
            Module::Standby { until, ignored, .. } => {
                if reached(now, until) {
                    // Its own wake: NMEA should follow within a second (a hot start). One that never slept has
                    // been heard all along.
                    self.module = Module::Running { since: until, heard: ignored };
                } else if (continuous || beacon_soon) && !keyed {
                    out.poke = true;
                    self.module = Module::Poked { at: now };
                }
            }
            Module::Running { since, heard } => {
                if !heard && !self.cycled_silent && now.wrapping_sub(since) >= SILENT_MS && !keyed {
                    self.cycled_silent = true;
                    self.wake_fallback(now, &mut out);
                } else if !continuous && !keyed {
                    self.plan(now, since, d, beacon_soon, &mut out);
                }
            }
        }
        out
    }

    /// Running, nothing continuous: standby now, and for how long, or stay on.
    fn plan(&mut self, now: u32, since: u32, d: &GpsDemand, beacon_soon: bool, out: &mut GpsOut) {
        let beacon_wake = d.beacon_at.map(|b| b.wrapping_sub(BEACON_LEAD_MS)).filter(|&w| !reached(now.wrapping_add(MIN_STANDBY_S * 1000), w));
        let next = match self.fix_at {
            // GPS-30: no sky.
            None if now.wrapping_sub(since) >= NO_SKY_MS => {
                self.misses = self.misses.saturating_add(1);
                now.wrapping_add(BACKOFF_S[(self.misses as usize - 1).min(BACKOFF_S.len() - 1)] as u32 * 1000)
            }
            None => return,
            Some(_) => {
                // GPS-29: the ephemeris refresh holds it on; GPS-27: so does a beacon about to go.
                if self.eph_at.is_none_or(|e| now.wrapping_sub(e) >= EPH_EVERY_MS) || beacon_soon {
                    return;
                }
                now.wrapping_add(WAKE_EVERY_MS)
            }
        };
        let next = match beacon_wake {
            Some(w) if !reached(w, next) => w,
            _ => next,
        };
        let s = (next.wrapping_sub(now) / 1000).clamp(MIN_STANDBY_S, u16::MAX as u32);
        out.standby_s = Some(s as u16);
        self.module = Module::Standby { at: now, until: now.wrapping_add(s * 1000), ignored: false };
        self.fix_at = None;
    }

    /// GPS-31's fallback, kept apart so the bench can change it: the wake byte (or the module's own wake) brought
    /// no NMEA, so PA8 cycles the module (a cold start unless its backup supply holds the ephemeris).
    pub fn wake_fallback(&mut self, now: u32, out: &mut GpsOut) {
        self.module = Module::Cycling { at: now };
        out.power = false;
        out.cycled = true;
    }

    /// The module's power changed (PA8, by this or by GPS-4's rule).
    #[inline(never)] // in .slow_text (memory.x)
    pub fn powered(&mut self, now: u32, on: bool) {
        match (on, self.module) {
            (true, _) => {
                self.module = Module::Running { since: now, heard: false };
                self.fix_at = None;
            }
            (false, Module::Cycling { .. }) => {}
            (false, _) => {
                self.module = Module::Off;
                self.fix_at = None;
            }
        }
    }

    /// A sentence came from the module. True the first time one comes [`SETTLE_MS`] or more into a standby: the
    /// module didn't sleep (or woke at once), for the cable log. That isn't taken for a wake: the standby's plan
    /// holds to its end (decision 143; it used to read as "awake" and plan again at once).
    #[inline(never)] // in .slow_text (memory.x)
    pub fn sentence(&mut self, now: u32) -> bool {
        self.cycled_silent = false;
        match self.module {
            Module::Poked { .. } => self.module = Module::Running { since: now, heard: true },
            Module::Standby { at, until, ignored: false } if now.wrapping_sub(at) >= SETTLE_MS => {
                self.module = Module::Standby { at, until, ignored: true };
                return true;
            }
            Module::Running { since, .. } => self.module = Module::Running { since, heard: true },
            _ => {}
        }
        false
    }

    /// A sentence with a fix: `speed_kmh` over ground. Returns [`GpsManager::sentence`]'s answer.
    #[inline(never)] // in .slow_text (memory.x)
    pub fn fix(&mut self, now: u32, speed_kmh: u16) -> bool {
        let ignored = self.sentence(now);
        if let Module::Running { .. } = self.module {
            self.fix_at.get_or_insert(now);
            self.misses = 0;
            if speed_kmh >= MOVING_KMH {
                self.moved_at = Some(now);
            }
        }
        ignored
    }

    /// What it is doing, for the Status page and the terminal.
    #[inline(never)] // in .slow_text (memory.x)
    pub fn phase(&self, now: u32) -> GpsPhase {
        match self.module {
            Module::Off => GpsPhase::Off,
            Module::Running { .. } if self.fix_at.is_some() => GpsPhase::Running,
            Module::Running { .. } => GpsPhase::Searching,
            Module::Standby { until, .. } => GpsPhase::Standby(until.wrapping_sub(now).min(u16::MAX as u32 * 1000).div_ceil(1000) as u16),
            Module::Poked { .. } | Module::Cycling { .. } => GpsPhase::Waking,
        }
    }

    /// Wakes in a row with no fix (GPS-30).
    pub fn misses(&self) -> u8 {
        self.misses
    }

    /// Running and heard since its power-up or wake: the module takes commands (GPS-34's set goes then).
    pub fn awake(&self) -> bool {
        matches!(self.module, Module::Running { heard: true, .. })
    }
}

/// What the managed module is doing (GPS-26), for the Status page and the terminal's "Radio now".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpsPhase {
    #[default]
    Off,
    /// Awake, no fix yet this wake.
    Searching,
    /// Awake with a fix.
    Running,
    /// In standby; the seconds to its own wake.
    Standby(u16),
    /// An unplanned wake (the byte, or the power cycle).
    Waking,
}

impl GpsPhase {
    /// As one word for an atomic between tasks.
    pub fn pack(self) -> u32 {
        match self {
            GpsPhase::Off => 0,
            GpsPhase::Searching => 1,
            GpsPhase::Running => 2,
            GpsPhase::Waking => 3,
            GpsPhase::Standby(s) => 4 | (s as u32) << 8,
        }
    }

    pub fn unpack(w: u32) -> GpsPhase {
        match w & 0xFF {
            1 => GpsPhase::Searching,
            2 => GpsPhase::Running,
            3 => GpsPhase::Waking,
            4 => GpsPhase::Standby((w >> 8) as u16),
            _ => GpsPhase::Off,
        }
    }
}

/// The longest CASIC sentence sent, CR LF included (the CAS03 sentences, 28 bytes).
pub const CASIC_MAX: usize = 32;

/// A CASIC command (CASIC protocol 2.7) and CR LF into `out`, the checksum the XOR of everything between `$` and `*`:
/// `$PCASnn,arg*CS` for one number, or for [`CAS_NMEA`] one of the fixed sentences (`arg` 0 [`NMEA_OFF`], 2
/// [`NMEA_SKY`], else [`NMEA_SET`]). Returns the length. Only what [`casic_refused`] passed, or GPS-34's set, is
/// ever sent.
pub fn casic(id: u8, arg: u16, out: &mut [u8; CASIC_MAX]) -> usize {
    if id == CAS_NMEA {
        let s = match arg {
            0 => NMEA_OFF,
            2 => NMEA_SKY,
            _ => NMEA_SET,
        };
        out[..s.len()].copy_from_slice(s);
        return s.len();
    }
    out[..5].copy_from_slice(b"$PCAS");
    (out[5], out[6], out[7]) = (b'0' + id / 10 % 10, b'0' + id % 10, b',');
    let mut n = 8;
    let digits = if arg >= 10_000 { 5 } else if arg >= 1000 { 4 } else if arg >= 100 { 3 } else if arg >= 10 { 2 } else { 1 };
    let mut v = arg;
    for i in (0..digits).rev() {
        out[n + i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    n += digits;
    let cs = out[1..n].iter().fold(0u8, |a, &c| a ^ c);
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out[n..n + 5].copy_from_slice(&[b'*', HEX[(cs >> 4) as usize], HEX[(cs & 15) as usize], b'\r', b'\n']);
    n + 5
}

/// The standby command (CAS12).
pub const CAS_STANDBY: u8 = 12;
/// The product information query (CAS06, 2.7.7): 0 firmware version, 1 hardware model and serial, 2 working mode,
/// 4 GNSS signals, 6 chip; the module answers with a TXT sentence.
pub const CAS_QUERY: u8 = 6;
/// The NMEA output command (CAS03, 2.7.4: `nGGA,nGLL,nGSA,nGSV,nRMC,nVTG,nZDA,nANT,...`, each 0-9 = every n-th
/// fix, 0 none, empty kept). Only fixed sentences go, all RAM only (never CAS00, so the module's flash keeps its
/// own; a power cycle restores it). Arg 0 is [`NMEA_OFF`], 1 [`NMEA_SET`], 2 [`NMEA_SKY`].
pub const CAS_NMEA: u8 = 3;
/// TERM-36: every NMEA sentence off (the eight fields stock names, GGA to ANT; the rest left as they are).
/// Other AT6558 users found the chip acts on CASIC commands reliably only with its NMEA output off.
pub const NMEA_OFF: &[u8] = b"$PCAS03,0,0,0,0,0,0,0,0*02\r\n";
/// GPS-34: Ferrite's set: GGA and RMC every fix, GSA and GSV every 5th, GLL, VTG, ZDA and the antenna TXT off (the
/// fields after ANT, off by default, left as they are). GSA stays because the parser takes 2D/3D and the satellites
/// used in the fix from it (the sky plot's filled ones). Stock V0.27 sends `$PCAS03,5,0,0,0,0,3,7,60*35` instead
/// (0x0800CDE0, at GPS power-up and on each TXT): GGA every 5th, VTG every 3rd, ZDA every 7th, RMC and GSV off.
pub const NMEA_SET: &[u8] = b"$PCAS03,1,0,5,5,1,0,0,0*02\r\n";
/// GPS-34: the set while a screen that shows satellites is open: GSA and GSV every fix too.
pub const NMEA_SKY: &[u8] = b"$PCAS03,1,0,1,1,1,0,0,0*02\r\n";
/// GPS-34: a set not taken is sent again this many times a wake at most.
pub const NMEA_TRIES: u8 = 3;

/// GPS-34: a sentence the set turns off (GLL, VTG or ZDA, any talker; `s` as [`crate::gps::Nmea::last`] keeps
/// it): coming [`SETTLE_MS`] or more after the set, the module didn't take it. TXT isn't one: a query's answer and
/// the module's start-up text are TXT too.
pub fn set_off(s: &[u8]) -> bool {
    s.len() >= 5 && matches!(&s[2..5], b"GLL" | b"VTG" | b"ZDA")
}

/// What [`NmeaSet::stray`] makes of a sentence the set turns off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stray {
    /// Nothing (no set sent yet, or still settling).
    None,
    /// Not taken: sent again at the next look.
    Again,
    /// Not taken [`NMEA_TRIES`] times: left as it is until the next wake (once a wake, for the cable log).
    GaveUp,
}

/// GPS-34: Ferrite's NMEA set, sent once the module is heard after each power-up, power cycle and wake (its CASIC
/// settings live in RAM), and again when the wanted variant changes; nothing while keyed (GPS-32), the next look
/// sends it. Pure logic: the GPS task and the simulator's fake module send what it returns.
#[derive(Clone, Copy, Debug, Default)]
pub struct NmeaSet {
    /// The variant sent this wake (true: [`NMEA_SKY`]) and when.
    sent: Option<(bool, u32)>,
    /// Sent again this wake for a sentence it turns off.
    tries: u8,
}

impl NmeaSet {
    pub const fn new() -> Self {
        NmeaSet { sent: None, tries: 0 }
    }

    /// One look at `now`: `sky` the variant wanted ([`GpsDemand::sky`]), `awake` [`GpsManager::awake`]. Returns
    /// the variant to send now (CAS03 arg 1, or 2 for sky). Anything but awake (off, standby, a wake not yet heard)
    /// forgets what was sent: the next wake gets the set again.
    #[inline(never)] // in .slow_text (memory.x)
    pub fn step(&mut self, now: u32, sky: bool, awake: bool, keyed: bool) -> Option<bool> {
        if !awake {
            *self = NmeaSet::new();
            return None;
        }
        if keyed || self.sent.is_some_and(|(s, _)| s == sky) {
            return None;
        }
        self.sent = Some((sky, now));
        Some(sky)
    }

    /// A sentence the set turns off ([`set_off`]) came at `now`.
    #[inline(never)] // in .slow_text (memory.x)
    pub fn stray(&mut self, now: u32) -> Stray {
        match self.sent {
            Some((_, at)) if now.wrapping_sub(at) >= SETTLE_MS && self.tries < NMEA_TRIES => {
                self.tries += 1;
                if self.tries < NMEA_TRIES {
                    self.sent = None;
                    Stray::Again
                } else {
                    Stray::GaveUp
                }
            }
            _ => Stray::None,
        }
    }

    /// The terminal's bench test sent `$PCAS<id>,<arg>` itself (TERM-36): NMEA off or a standby leaves no set; the
    /// set (1, 2) is the one sent.
    pub fn bench_sent(&mut self, id: u8, arg: u16, now: u32) {
        match (id, arg) {
            (CAS_NMEA, 1 | 2) => self.sent = Some((arg == 2, now)),
            (CAS_NMEA, _) | (CAS_STANDBY, _) => self.sent = None,
            _ => {}
        }
    }
}

#[cfg(feature = "bench")]
/// What may be sent to the module (TERM-36): CAS03 NMEA off or Ferrite's set (GPS-34), CAS06 queries (0, 1, 2, 4, 6) and
/// CAS12 standby (1-65535 s), nothing else. Never CAS00 (saves the configuration to the module's flash) or CAS01
/// (its baud rate: a wrong one leaves the GPS silent), nor any other setting.
pub fn casic_refused(id: u8, arg: u16) -> Option<&'static str> {
    match (id, arg) {
        (CAS_NMEA, 0..=2) => None,
        (CAS_NMEA, _) => Some("CAS03 is only NMEA off (0) or Ferrite's set (1, 2 sky)"),
        (CAS_QUERY, 0 | 1 | 2 | 4 | 6) => None,
        (CAS_QUERY, _) => Some("CAS06 takes 0, 1, 2, 4 or 6"),
        (CAS_STANDBY, 1..) => None,
        (CAS_STANDBY, _) => Some("standby needs 1 to 65535 s"),
        _ => Some("only CAS03 NMEA off/on, CAS06 queries and CAS12 standby go to the module"),
    }
}

#[cfg(feature = "bench")]
/// A quiet standby (TERM-36): the CAS12 goes this long after NMEA off.
pub const QUIET_GAP_MS: u32 = 100;
#[cfg(feature = "bench")]
/// A quiet standby: the NMEA set (GPS-34) goes this long after the standby's end at the latest.
pub const QUIET_RESTORE_MS: u32 = 3_000;

#[cfg(feature = "bench")]
/// What follows a bench standby's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    /// Back to the power manager.
    Idle,
    /// NMEA stays off (a standby sent after `gps nmea off`).
    Quiet,
    /// A quiet standby: the NMEA set (GPS-34) once NMEA comes back, or [`QUIET_RESTORE_MS`] after the end.
    Restore,
}

#[cfg(feature = "bench")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Bench {
    #[default]
    Idle,
    /// NMEA off (`gps nmea off`) until `gps nmea on` or a power off.
    Quiet,
    /// A quiet standby: NMEA off went at `at`, the CAS12 for `n` s goes [`QUIET_GAP_MS`] later.
    Pending { at: u32, n: u16 },
    /// A bench standby sent at `at`, its end at `until`; `heard` once a sentence came [`SETTLE_MS`] or more in.
    Standby { at: u32, until: u32, after: After, heard: bool },
}

#[cfg(feature = "bench")]
/// The terminal's GPS bench test (TERM-36, GPS-31): from `gps nmea off` or a `gps standby` until NMEA is on again
/// and the standby over, the power manager is held (nothing of its goes to the module, no power cycle) and no other
/// terminal sentence is let through during a standby. Pure logic: the GPS task sends what it returns.
#[derive(Clone, Copy, Debug, Default)]
pub struct GpsBench {
    st: Bench,
}

#[cfg(feature = "bench")]
impl GpsBench {
    pub const fn new() -> Self {
        GpsBench { st: Bench::Idle }
    }

    /// A terminal command at `now` ([`casic_refused`] passed it): the sentence to send now, or why not. `quiet`
    /// (CAS12 only): NMEA off now, the CAS12 [`QUIET_GAP_MS`] later ([`GpsBench::step`]), no NMEA on after it.
    #[inline(never)] // in .slow_text (memory.x)
    pub fn command(&mut self, id: u8, arg: u16, quiet: bool, now: u32) -> Result<(u8, u16), &'static str> {
        if let Some(why) = casic_refused(id, arg) {
            return Err(why);
        }
        if quiet && id != CAS_STANDBY {
            return Err("only a standby is quiet");
        }
        if matches!(self.st, Bench::Pending { .. } | Bench::Standby { .. }) {
            return Err("a bench standby is running: nothing else goes to the module until it ends");
        }
        match (id, arg) {
            (CAS_NMEA, 0) => self.st = Bench::Quiet,
            (CAS_NMEA, _) => self.st = Bench::Idle,
            (CAS_STANDBY, n) if quiet => {
                self.st = Bench::Pending { at: now, n };
                return Ok((CAS_NMEA, 0));
            }
            (CAS_STANDBY, n) => {
                let after = if self.st == Bench::Quiet { After::Quiet } else { After::Idle };
                self.st = Bench::Standby { at: now, until: now.wrapping_add(n as u32 * 1000), after, heard: false };
            }
            _ => {}
        }
        Ok((id, arg))
    }

    /// One look at `now` (not while keyed: PB10 is held low then): a quiet standby's CAS12, or its NMEA on at the
    /// latest [`QUIET_RESTORE_MS`] after its end.
    #[inline(never)] // in .slow_text (memory.x)
    pub fn step(&mut self, now: u32) -> Option<(u8, u16)> {
        match self.st {
            Bench::Pending { at, n } if now.wrapping_sub(at) >= QUIET_GAP_MS => {
                self.st = Bench::Standby { at: now, until: now.wrapping_add(n as u32 * 1000), after: After::Restore, heard: false };
                Some((CAS_STANDBY, n))
            }
            Bench::Standby { until, after: After::Restore, .. } if reached(now, until.wrapping_add(QUIET_RESTORE_MS)) => {
                self.st = Bench::Idle;
                Some((CAS_NMEA, 1))
            }
            Bench::Standby { until, after, .. } if after != After::Restore && reached(now, until) => {
                self.st = if after == After::Quiet { Bench::Quiet } else { Bench::Idle };
                None
            }
            _ => None,
        }
    }

    /// A sentence came from the module at `now`. The first [`SETTLE_MS`] or more into a bench standby: its seconds
    /// since the CAS12 (for the cable log), and for a quiet standby the NMEA set (GPS-34) to send now (the module woke,
    /// or never slept).
    #[inline(never)] // in .slow_text (memory.x)
    pub fn heard(&mut self, now: u32) -> (Option<u32>, Option<(u8, u16)>) {
        match self.st {
            Bench::Standby { at, until, after, heard } if now.wrapping_sub(at) >= SETTLE_MS => {
                let first = (!heard).then_some(now.wrapping_sub(at) / 1000);
                if after == After::Restore {
                    self.st = Bench::Idle;
                    return (first, Some((CAS_NMEA, 1)));
                }
                self.st = Bench::Standby { at, until, after, heard: true };
                (first, None)
            }
            _ => (None, None),
        }
    }

    /// The module lost power (GPS off, around a signal): its RAM settings are gone, the test with them.
    pub fn off(&mut self) {
        self.st = Bench::Idle;
    }

    /// The power manager is held (a bench test is running).
    pub fn holds(&self) -> bool {
        self.st != Bench::Idle
    }

    /// A bench standby (or a quiet one's gap) is running: nothing else may go to the module.
    pub fn in_standby(&self) -> bool {
        matches!(self.st, Bench::Pending { .. } | Bench::Standby { .. })
    }
}

/// A sentence as text for the cable log (TERM-28): printable bytes as they are, CR and LF as `<CR>` `<LF>`, anything
/// else as `<XX>`; cut at `out`'s length. Returns the length.
pub fn shown(b: &[u8], out: &mut [u8]) -> usize {
    let mut n = 0;
    let mut put = |s: &[u8]| {
        for &c in s {
            if n < out.len() {
                out[n] = c;
                n += 1;
            }
        }
    };
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &c in b {
        match c {
            b'\r' => put(b"<CR>"),
            b'\n' => put(b"<LF>"),
            0x20..=0x7E => put(&[c]),
            _ => put(&[b'<', HEX[(c >> 4) as usize], HEX[(c & 15) as usize], b'>']),
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(id: u8, arg: u16) -> std::string::String {
        let mut b = [0u8; CASIC_MAX];
        let n = casic(id, arg, &mut b);
        assert!(b[..n].ends_with(b"\r\n"));
        std::string::String::from_utf8(b[..n - 2].to_vec()).unwrap()
    }

    /// The encoder against the CASIC protocol specification's printed examples.
    #[test]
    fn casic_matches_the_spec() {
        assert_eq!(cmd(12, 60), "$PCAS12,60*28"); // 2.7.11: standby 60 s
        assert_eq!(cmd(10, 0), "$PCAS10,0*1C"); // 2.7.9: hot start
        assert_eq!(cmd(10, 3), "$PCAS10,3*1F");
        assert_eq!(cmd(4, 3), "$PCAS04,3*1A"); // 2.7.3: GPS + BDS
        assert_eq!(cmd(11, 0), "$PCAS11,0*1D"); // 2.7.10: portable
        assert_eq!(cmd(2, 1000), "$PCAS02,1000*2E"); // 2.7.1: 1 Hz
        assert_eq!(cmd(12, 65535).len(), 16);
    }

    const ON: GpsDemand = GpsDemand { on: true, watch: false, track: false, beacon_at: None, sky: false };

    /// A fake ATGM336H for the tests: sentences every second while awake (GGA, which GPS-34's set keeps every fix), a fix `fix_s` seconds into each wake
    /// (None: indoors), standby by CAS12 with its own wake, a wake byte that works or not.
    struct Sim {
        m: GpsManager,
        t: u32,
        powered: bool,
        awake_at: Option<u32>,
        asleep_until: Option<u32>,
        fix_s: Option<u32>,
        poke_works: bool,
        /// The module acts on CAS12 (false: it keeps sending, as on the radio Oct 13, 2026).
        standby_works: bool,
        /// Standbys the manager saw ignored.
        ignored: u32,
        speed: u16,
        /// (time, standby seconds) for each CAS12 sent; power cycles; pokes.
        standbys: std::vec::Vec<(u32, u16)>,
        cycles: u32,
        pokes: u32,
        keyed: bool,
    }

    impl Sim {
        fn new(fix_s: Option<u32>) -> Sim {
            Sim { m: GpsManager::new(), t: 0, powered: false, awake_at: None, asleep_until: None, fix_s, poke_works: true, standby_works: true, ignored: 0, speed: 0, standbys: std::vec::Vec::new(), cycles: 0, pokes: 0, keyed: false }
        }

        /// Run to `end` ms in 100 ms looks.
        fn run(&mut self, end: u32, d: &dyn Fn(u32) -> GpsDemand) {
            while self.t < end {
                self.t += 100;
                let now = self.t;
                if self.asleep_until.is_some_and(|u| now >= u) {
                    (self.asleep_until, self.awake_at) = (None, Some(now));
                }
                let out = self.m.step(now, &d(now), self.keyed);
                if out.power != self.powered {
                    self.powered = out.power;
                    self.m.powered(now, out.power);
                    (self.awake_at, self.asleep_until) = (out.power.then_some(now), None);
                }
                self.cycles += out.cycled as u32;
                if let Some(s) = out.standby_s {
                    self.standbys.push((now, s));
                    if self.standby_works {
                        (self.awake_at, self.asleep_until) = (None, Some(now + s as u32 * 1000));
                    }
                }
                if out.poke {
                    self.pokes += 1;
                    if self.poke_works && self.asleep_until.is_some() {
                        (self.awake_at, self.asleep_until) = (Some(now), None);
                    }
                }
                if let Some(a) = self.awake_at.filter(|_| self.powered && now.is_multiple_of(1000)) {
                    let ignored = match self.fix_s {
                        Some(f) if now - a >= f * 1000 => self.m.fix(now, self.speed),
                        _ => self.m.sentence(now),
                    };
                    self.ignored += ignored as u32;
                }
            }
        }
    }

    fn on(_: u32) -> GpsDemand {
        ON
    }

    /// GPS-26, GPS-29: the first fix holds the module on a minute (ephemeris), then standby to the next minute;
    /// each wake after sleeps again at its fix; 30 minutes on, a wake stays on a minute again.
    #[test]
    fn minute_wakes_and_ephemeris() {
        let mut s = Sim::new(Some(2));
        s.run(70_000, &on);
        assert_eq!(s.standbys.len(), 1);
        let (t0, n) = s.standbys[0];
        assert!((62_000..=63_000).contains(&t0), "the first fix at 2 s, held 60 s: {t0}");
        assert_eq!(n, 60);
        assert_eq!(s.m.phase(t0 + 1000), GpsPhase::Standby(59));
        s.run(20 * 60_000, &on);
        // Each wake: up 60 s later, a fix in 2 s, standby again.
        for w in s.standbys.windows(2) {
            let gap = w[1].0 - w[0].0;
            assert!((61_000..=63_000).contains(&gap), "{gap}");
            assert_eq!(w[1].1, 60);
        }
        let before = s.standbys.len();
        s.run(40 * 60_000, &on);
        let long = s.standbys.windows(2).skip(before - 1).filter(|w| w[1].0 - w[0].0 > 100_000).count();
        assert_eq!(long, 1, "one ephemeris hold in the 20 minutes after");
    }

    /// GPS-30: indoors, a wake with no fix in 2 minutes sleeps 1, 2, 4, 8, then 15 minutes; a fix resets it.
    #[test]
    fn back_off_without_sky() {
        let mut s = Sim::new(None);
        s.run(60 * 60_000, &on);
        let waits: std::vec::Vec<u16> = s.standbys.iter().map(|x| x.1).collect();
        assert_eq!(&waits[..6], &[60, 120, 240, 480, 900, 900]);
        assert!(s.m.misses() >= 5);
        // Each standby comes 2 minutes into its wake.
        let first = s.standbys[0].0;
        assert!((120_000..=120_200).contains(&first), "{first}");
        assert_eq!(s.cycles, 0, "NMEA came at every wake: no fallback");
        // Outside: the next wake fixes and the minute wakes come back.
        s.fix_s = Some(3);
        let n = s.standbys.len();
        s.run(100 * 60_000, &on);
        assert_eq!(s.m.misses(), 0);
        assert!(s.standbys[n..].iter().skip(1).all(|x| x.1 == 60), "{:?}", &s.standbys[n..]);
    }

    /// GPS-27: a timed beacon brings the wake 30 s before it, between the minute wakes; a beacon about to go keeps
    /// the module on.
    #[test]
    fn beacon_synced_wake() {
        let mut s = Sim::new(Some(1));
        // A beacon every 10 minutes at 5:00, 15:00, ... (the APRS task's next one).
        let d = |now: u32| GpsDemand { beacon_at: Some(300_000 + (now.saturating_sub(300_000) / 600_000 + (now > 300_000) as u32) * 600_000), ..ON };
        s.run(299_000, &d);
        // The ephemeris hold ends at about 61 s; standbys after it end at 4:30 at the latest.
        let wake_before = s.standbys.iter().map(|x| x.0 + x.1 as u32 * 1000).filter(|&w| w <= 300_000).max().unwrap();
        assert!((269_000..=271_000).contains(&wake_before) || s.m.phase(299_000) == GpsPhase::Running, "woken about 30 s before: {:?}", s.standbys);
        assert_eq!(s.m.phase(299_000), GpsPhase::Running, "on for the beacon");
        s.run(900_000, &d);
        // Around 15:00 too: a wake that ends 30 s before it.
        let around = s.standbys.iter().map(|x| x.0 + x.1 as u32 * 1000).filter(|&w| (860_000..=900_000).contains(&w)).count();
        assert!(around >= 1, "{:?}", s.standbys);
        assert!(s.standbys.iter().all(|x| x.1 <= 60), "the minute wakes go on: {:?}", s.standbys);
    }

    /// GPS-28: a GPS screen keeps it on; smart beaconing while moving keeps it on, and 3 minutes stopped hands
    /// back to the wakes; an unplanned need during standby pokes the module awake.
    #[test]
    fn continuous_demands() {
        let mut s = Sim::new(Some(1));
        s.run(5 * 60_000, &|_| GpsDemand { watch: true, ..ON });
        assert!(s.standbys.is_empty(), "a screen open: never standby");
        s.run(7 * 60_000, &on);
        assert!(!s.standbys.is_empty());
        // In standby, the screen opens: the byte, NMEA back, running.
        let t = s.t;
        let at = s.standbys.last().unwrap().0;
        assert!(t - at < 60_000);
        s.run(t + 2_000, &|_| GpsDemand { watch: true, ..ON });
        assert_eq!(s.pokes, 1);
        assert_eq!(s.m.phase(s.t), GpsPhase::Running);
        // Smart beaconing, moving: on; stopped: standby after 3 minutes.
        s.speed = 40;
        let track = |_| GpsDemand { track: true, ..ON };
        s.run(s.t + 2 * 60_000, &track); // the first fix that shows the movement (a minute wake at most)
        let n = s.standbys.len();
        s.run(s.t + 10 * 60_000, &track);
        assert_eq!(s.standbys.len(), n, "moving: continuous");
        s.speed = 0;
        let stop = s.t;
        s.run(s.t + 5 * 60_000, &track);
        let first = s.standbys[n].0;
        assert!(first - stop >= STOPPED_MS && first - stop <= STOPPED_MS + 2_000, "{}", first - stop);
    }

    /// GPS-31: a wake byte that brings nothing falls back to a PA8 power cycle; so does a module that doesn't wake
    /// itself.
    #[test]
    fn poke_fallback_cycles_power() {
        let mut s = Sim::new(Some(1));
        s.poke_works = false;
        s.run(65_000, &on);
        assert_eq!(s.standbys.len(), 1);
        let t = s.t;
        s.run(t + 2_000, &|_| GpsDemand { watch: true, ..ON });
        assert_eq!((s.pokes, s.cycles), (1, 0), "the byte, then 3 s to answer");
        assert_eq!(s.m.phase(s.t), GpsPhase::Waking);
        s.run(t + 6_000, &|_| GpsDemand { watch: true, ..ON });
        assert_eq!(s.cycles, 1, "no NMEA: PA8 cycled");
        s.run(t + 9_000, &|_| GpsDemand { watch: true, ..ON });
        assert_eq!(s.m.phase(s.t), GpsPhase::Running, "powered again, a fix");
    }

    /// GPS-32: nothing goes to the module while keyed; the standby waits for the unkey.
    #[test]
    fn keyed_waits() {
        let mut s = Sim::new(Some(1));
        s.keyed = true;
        s.run(90_000, &on);
        assert!(s.standbys.is_empty());
        s.keyed = false;
        s.run(91_000, &on);
        assert_eq!(s.standbys.len(), 1);
    }

    /// GPS-26 (decision 143): a module that ignores CAS12 keeps sending; that is reported once a standby and not
    /// taken for a wake, so the plan holds: outdoors one standby a minute (not one every fix), indoors the back-off
    /// as before; the phase stays the standby's to its end, and no power cycle follows.
    #[test]
    fn ignored_standby_keeps_the_plan() {
        let mut s = Sim::new(Some(2));
        s.standby_works = false;
        s.run(10 * 60_000, &on);
        let n = s.standbys.len();
        assert!((8..=10).contains(&n), "about one a minute after the ephemeris hold: {n} {:?}", s.standbys);
        assert_eq!(s.ignored as usize, n, "each reported once");
        for w in s.standbys.windows(2) {
            assert!(w[1].0 - w[0].0 >= 60_000, "{:?}", w);
        }
        let (t0, _) = *s.standbys.last().unwrap();
        assert!(matches!(s.m.phase(t0 + 5_000), GpsPhase::Standby(_)) || s.t < t0 + 5_000);
        assert_eq!(s.cycles, 0);
        let mut s = Sim::new(None);
        s.standby_works = false;
        s.run(30 * 60_000, &on);
        let waits: std::vec::Vec<u16> = s.standbys.iter().map(|x| x.1).collect();
        assert_eq!(&waits[..5], &[60, 120, 240, 480, 900]);
        assert_eq!((s.ignored as usize, s.cycles), (s.standbys.len(), 0));
    }

    #[cfg(feature = "bench")]
    /// TERM-36: only CAS06 queries and CAS12 standby are let through, and the bytes are as the spec prints them
    /// (`$PCAS06,0*1B` firmware version, 2.7.7).
    #[test]
    fn only_queries_and_standby_go_to_the_module() {
        assert_eq!(cmd(CAS_QUERY, 0), "$PCAS06,0*1B");
        assert_eq!(cmd(CAS_QUERY, 1), "$PCAS06,1*1A");
        assert_eq!(cmd(CAS_QUERY, 2), "$PCAS06,2*19");
        assert_eq!(cmd(CAS_QUERY, 4), "$PCAS06,4*1F");
        assert_eq!(cmd(CAS_QUERY, 6), "$PCAS06,6*1D");
        for a in [0, 1, 2, 4, 6] {
            assert_eq!(casic_refused(CAS_QUERY, a), None);
        }
        assert!(casic_refused(CAS_QUERY, 3).is_some());
        assert_eq!(casic_refused(CAS_STANDBY, 60), None);
        assert_eq!(casic_refused(CAS_STANDBY, 65535), None);
        assert!(casic_refused(CAS_STANDBY, 0).is_some());
        for id in [0, 1, 2, 4, 5, 10, 11, 15, 20] {
            assert!(casic_refused(id, 0).is_some() && casic_refused(id, 9600).is_some(), "CAS{id:02}");
        }
        let mut b = [0u8; CASIC_MAX];
        let n = casic(CAS_STANDBY, 60, &mut b);
        assert_eq!(&b[..n], b"$PCAS12,60*28\r\n", "the exact bytes on the wire, CR LF last");
        let mut o = [0u8; 40];
        let k = shown(&b[..n], &mut o);
        assert_eq!(&o[..k], b"$PCAS12,60*28<CR><LF>");
        let k = shown(&[b'a', 0], &mut o);
        assert_eq!(&o[..k], b"a<00>");
    }

    #[cfg(feature = "bench")]
    /// TERM-36, GPS-34: CAS03 takes only 0 (off), 1 (Ferrite's set) and 2 (its sky variant).
    #[test]
    fn nmea_off_and_ferrites_set() {
        assert_eq!(casic_refused(CAS_NMEA, 0), None);
        assert_eq!(casic_refused(CAS_NMEA, 1), None);
        assert_eq!(casic_refused(CAS_NMEA, 2), None);
        for a in [3, 9, 60, 9600] {
            assert!(casic_refused(CAS_NMEA, a).is_some(), "CAS03 {a}");
        }
        // Never a save (CAS00), a baud rate (CAS01), nor any other setting, whatever the argument.
        for id in [0, 1, 2, 4, 5, 7, 8, 9, 10, 11, 13, 20, 99] {
            for a in [0, 1, 3, 9600, 65535] {
                assert!(casic_refused(id, a).is_some(), "CAS{id:02},{a}");
            }
        }
    }

    #[cfg(feature = "bench")]
    /// TERM-36: `gps nmea off` holds the manager until `gps nmea on`; a standby sent then keeps NMEA off after its
    /// end; nothing else goes to the module during a standby.
    #[test]
    fn bench_nmea_off_standby_on() {
        let mut b = GpsBench::new();
        assert!(!b.holds());
        assert_eq!(b.command(CAS_NMEA, 0, false, 1000), Ok((CAS_NMEA, 0)));
        assert!(b.holds() && !b.in_standby());
        assert_eq!(b.command(CAS_QUERY, 1, false, 1500), Ok((CAS_QUERY, 1)), "a query while quiet");
        assert_eq!(b.command(CAS_STANDBY, 60, false, 2000), Ok((CAS_STANDBY, 60)));
        assert!(b.in_standby());
        for (id, a) in [(CAS_NMEA, 1), (CAS_QUERY, 0), (CAS_STANDBY, 10)] {
            assert!(b.command(id, a, false, 30_000).is_err(), "CAS{id:02} during the standby");
        }
        assert_eq!(b.heard(3000), (None, None), "settling");
        assert_eq!(b.heard(5000), (Some(3), None), "NMEA 3 s in: logged once");
        assert_eq!(b.heard(6000), (None, None));
        assert_eq!(b.step(61_999), None);
        assert!(b.in_standby());
        assert_eq!(b.step(62_000), None, "the end: NMEA stays off");
        assert!(b.holds() && !b.in_standby());
        assert_eq!(b.command(CAS_NMEA, 1, false, 70_000), Ok((CAS_NMEA, 1)));
        assert!(!b.holds());
        // A plain standby: held to its end, then back to the manager.
        assert_eq!(b.command(CAS_STANDBY, 10, false, 80_000), Ok((CAS_STANDBY, 10)));
        assert!(b.holds());
        assert_eq!(b.step(90_000), None);
        assert!(!b.holds());
        // Refusals: the allow-list, and quiet only with a standby.
        assert!(b.command(1, 9600, false, 0).is_err());
        assert!(b.command(0, 0, false, 0).is_err());
        assert!(b.command(CAS_QUERY, 0, true, 0).is_err());
        assert!(b.command(CAS_NMEA, 0, true, 0).is_err());
        assert!(!b.holds());
    }

    #[cfg(feature = "bench")]
    /// TERM-36: `gps standby N quiet`: NMEA off, the CAS12 100 ms later, no NMEA on after it; the NMEA set once
    /// NMEA comes back, or N + 3 s after the CAS12; a power off ends the test.
    #[test]
    fn bench_quiet_standby() {
        let mut b = GpsBench::new();
        assert_eq!(b.command(CAS_STANDBY, 40, true, 1000), Ok((CAS_NMEA, 0)), "NMEA off first");
        assert!(b.in_standby());
        assert_eq!(b.step(1050), None);
        assert!(b.command(CAS_NMEA, 1, false, 1050).is_err(), "nothing else in the gap");
        assert_eq!(b.step(1100), Some((CAS_STANDBY, 40)));
        assert_eq!(b.heard(1500), (None, None), "a sentence already on its way");
        assert_eq!(b.step(41_100), None, "its end: no NMEA on yet");
        assert_eq!(b.step(44_099), None);
        assert_eq!(b.step(44_100), Some((CAS_NMEA, 1)), "N + 3 s");
        assert!(!b.holds());
        assert_eq!(b.step(50_000), None, "once");
        // NMEA back before that (the module woke with its own settings): NMEA on at once.
        assert_eq!(b.command(CAS_STANDBY, 40, true, 0), Ok((CAS_NMEA, 0)));
        assert_eq!(b.step(100), Some((CAS_STANDBY, 40)));
        assert_eq!(b.heard(40_500), (Some(40), Some((CAS_NMEA, 1))));
        assert!(!b.holds());
        // Power off: the module forgets, so does the test.
        assert_eq!(b.command(CAS_STANDBY, 40, true, 0), Ok((CAS_NMEA, 0)));
        b.off();
        assert!(!b.holds());
        assert_eq!(b.step(100), None);
        assert!(core::mem::size_of::<GpsBench>() <= 16);
    }

    /// The checksum is the XOR of everything between `$` and `*`.
    fn xor_ok(s: &[u8]) -> bool {
        let star = s.iter().position(|&c| c == b'*').unwrap();
        let cs = s[1..star].iter().fold(0u8, |a, &c| a ^ c);
        std::format!("{cs:02X}").into_bytes() == s[star + 1..star + 3]
    }

    /// GPS-34, TERM-36: NMEA off, Ferrite's set and its sky variant are these exact bytes (CAS03 field order GGA,
    /// GLL, GSA, GSV, RMC, VTG, ZDA, ANT, CASIC 2.7.4; the same order stock's `$PCAS03,5,0,0,0,0,3,7,60*35` uses for
    /// the GGA, VTG and ZDA its own parser reads), each checksum the XOR between `$` and `*` (checked against the
    /// spec's own example, `*02`), CR LF last.
    #[test]
    fn nmea_set_bytes() {
        assert!(xor_ok(b"$PCAS03,1,1,1,1,1,1,1,1,0,1,0,0,1,0*02"), "the spec's example");
        assert!(xor_ok(b"$PCAS03,5,0,0,0,0,3,7,60*35"), "stock V0.27's (0x0800CDE0)");
        let mut b = [0u8; CASIC_MAX];
        for (arg, want) in [(0, &b"$PCAS03,0,0,0,0,0,0,0,0*02\r\n"[..]), (1, b"$PCAS03,1,0,5,5,1,0,0,0*02\r\n"), (2, b"$PCAS03,1,0,1,1,1,0,0,0*02\r\n")] {
            let n = casic(CAS_NMEA, arg, &mut b);
            assert_eq!(&b[..n], want, "CAS03 arg {arg}");
            assert!(xor_ok(&b[..n]));
        }
        // Field by field: GGA 1, GLL 0, GSA 5|1, GSV 5|1, RMC 1, VTG 0, ZDA 0, ANT 0.
        for (s, n) in [(NMEA_SET, b'5'), (NMEA_SKY, b'1')] {
            let f: std::vec::Vec<&[u8]> = s[8..s.len() - 5].split(|&c| c == b',').collect();
            assert_eq!(f, [&b"1"[..], b"0", &[n], &[n], b"1", b"0", b"0", b"0"]);
            assert!(s.len() <= CASIC_MAX);
        }
        assert!(set_off(b"GNGLL,4525.2950,N*00") && set_off(b"GNVTG,1") && set_off(b"GPZDA,1"));
        assert!(!set_off(b"GNGGA,1") && !set_off(b"GPGSV,1") && !set_off(b"GNGSA,1") && !set_off(b"GPTXT,01") && !set_off(b"GN"));
    }

    /// GPS-34: the set goes once the module is heard after a power-up and after each wake (never during standby or
    /// before it answers), again when the sky variant changes, never while keyed (the next look sends it); a module
    /// that keeps sending GLL, VTG or ZDA 2 s after it is sent it again, three times a wake at most.
    #[test]
    fn nmea_set_after_every_wake() {
        let mut m = GpsManager::new();
        let mut n = NmeaSet::new();
        let d = ON;
        m.step(0, &d, false);
        m.powered(0, true);
        assert_eq!(n.step(100, false, m.awake(), false), None, "powered, not heard yet");
        m.sentence(900);
        assert!(m.awake());
        assert_eq!(n.step(950, false, m.awake(), false), Some(false), "heard: the set");
        assert_eq!(n.step(1000, false, m.awake(), false), None, "once");
        assert_eq!(n.step(1050, true, m.awake(), true), None, "keyed: waits");
        assert_eq!(n.step(1100, true, m.awake(), false), Some(true), "a satellites screen: the sky variant");
        assert_eq!(n.step(1150, false, m.awake(), false), Some(false), "closed: back");
        // Strays: none while settling; then sent again twice, then left (once reported).
        assert_eq!(n.stray(2000), Stray::None, "a GLL on its way already");
        assert_eq!(n.stray(3200), Stray::Again);
        assert_eq!(n.step(3250, false, m.awake(), false), Some(false));
        assert_eq!(n.stray(5300), Stray::Again);
        assert_eq!(n.step(5350, false, m.awake(), false), Some(false));
        assert_eq!(n.stray(7400), Stray::GaveUp);
        assert_eq!(n.stray(9000), Stray::None, "reported once");
        assert_eq!(n.step(9050, false, m.awake(), false), None);
        // A wake: forgotten in standby, sent again once heard (tries afresh).
        m.module = Module::Standby { at: 10_000, until: 70_000, ignored: false };
        assert_eq!(n.step(10_050, false, m.awake(), false), None);
        m.step(70_000, &d, false);
        assert!(!m.awake(), "its own wake, not heard yet");
        assert_eq!(n.step(70_050, false, m.awake(), false), None);
        m.sentence(70_900);
        assert_eq!(n.step(70_950, false, m.awake(), false), Some(false));
        assert_eq!(n.stray(73_000), Stray::Again, "tries afresh this wake");
        // Power off and on (GPS-4, the fallback's cycle): again once heard.
        m.powered(80_000, false);
        assert_eq!(n.step(80_050, false, m.awake(), false), None);
        m.powered(80_500, true);
        m.sentence(81_000);
        assert_eq!(n.step(81_050, false, m.awake(), false), Some(false));
        // The bench: its NMEA off or a standby leaves no set; its set is the one sent.
        n.bench_sent(CAS_NMEA, 0, 82_000);
        assert_eq!(n.step(82_050, false, m.awake(), false), Some(false));
        n.bench_sent(CAS_NMEA, 2, 83_000);
        assert_eq!(n.step(83_050, true, m.awake(), false), None);
        n.bench_sent(CAS_QUERY, 0, 84_000);
        assert_eq!(n.step(84_050, true, m.awake(), false), None);
        assert!(core::mem::size_of::<NmeaSet>() <= 12);
    }

    /// GPS off: PA8 off, and on again it is running.
    #[test]
    fn off_is_power_off() {
        let mut s = Sim::new(Some(1));
        s.run(10_000, &on);
        assert!(s.powered);
        s.run(11_000, &|_| GpsDemand { on: false, ..ON });
        assert!(!s.powered);
        assert_eq!(s.m.phase(s.t), GpsPhase::Off);
        s.run(12_000, &on);
        assert!(s.powered);
    }

    #[test]
    fn phase_packs() {
        for p in [GpsPhase::Off, GpsPhase::Searching, GpsPhase::Running, GpsPhase::Waking, GpsPhase::Standby(900), GpsPhase::Standby(65535)] {
            assert_eq!(GpsPhase::unpack(p.pack()), p);
        }
        assert!(core::mem::size_of::<GpsManager>() <= 40);
    }
}
