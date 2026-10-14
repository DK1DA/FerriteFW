//! GPS (manual chapter 28, roadmap 5a): the ATGM336H on USART3 (PB11 in, 9600 baud, as stock and nicFW) and
//! its power on PA8 (low = on), which this task owns (decision 76).
//!
//! - **Power (GPS-1, GPS-4):** on or off with 0 6 1, and off around signals as 0 6 8 asks (the UI sends
//!   [`GPS_CFG`]; the Radio task's [`RF_STATE`] says what it is doing). After a signal the module comes back
//!   2 s later, so a burst of short signals doesn't cycle it. GPS off forgets the fix and publishes "off";
//!   powered off with GPS on, the last fix stays with its age (GPS-25).
//! - **PB10 while keyed (GPS-5, GPS-22):** USART3's TX pin is driven low while any chip transmits and handed
//!   back to the UART after, as stock does (0x080252D8 / 0x0802530C).
//! - **The power manager (GPS-26 to GPS-33, [`ferrite_core::gpspower`]):** with GPS on, the module sleeps between
//!   fixes: `$PCAS12,N` on USART3 TX (CASIC standby, it wakes itself N s later), a wake byte when a GPS screen
//!   or a beacon needs it sooner, a PA8 power cycle when that brings no NMEA (to confirm on the radio). The UI
//!   sends who needs a position ([`GPS_DEMAND`]); nothing goes to the module while keyed.
//! - **Parsing:** [`ferrite_core::gps::Nmea`] (about 260 B, here only). The fix goes to [`POSITION`] for the UI
//!   and the APRS task at most twice a second and whenever it changes state; the satellites go to [`SKY`] once
//!   a second for the sky plot.
//! - **The clock and the screens** are the UI task's: it reads POSITION and SKY into the store's GPS data
//!   (`ferrite_app::gpsdata`), and sets the clock from GPS time (GPS-7).
//! - **Slow flash** (memory.x, since Oct 13, 2026): the task and the parser run from the slow part. It is thread mode,
//!   the UART interrupt (fast) fills the ring, and NMEA at 9600 bd is under 1 KB/s.

use crate::board::GPS_PWR_N;
use crate::chan::{GPS_CFG, GPS_DEMAND, GPS_PHASE, POSITION, RF_BUSY_KEYED, RF_BUSY_RX, RF_STATE, SKY};
use crate::hal::gpio::{pin, Mode, GPIOB};
use crate::hal::uart;
use crate::report::UART_GPS;
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Instant, Timer};
use ferrite_app::gpsdata::GpsCfg;
use ferrite_core::gps::Nmea;
use ferrite_core::gpspower::{casic, GpsDemand, GpsManager, GpsOut, GpsPhase, CAS_STANDBY, POKE};
use ferrite_core::msg::{GpsFix, GpsSky};
use ferrite_radio::gps::GpsPower;

/// The ATGM336H's default rate.
const BAUD: u32 = 9600;
/// USART3 TX to the module.
const GPS_TX: crate::hal::gpio::Pin = pin(GPIOB, 10);
/// How often the task looks: 960 bytes/s at most, and the UART's 1 KB ring holds a second.
const POLL_MS: u64 = 50;
/// The fix is published at most this often (the module sends once a second, in several sentences).
const PUBLISH_MS: u32 = 500;

fn now_ms() -> u32 {
    Instant::now().as_millis() as u32
}

/// The power manager's look (GPS-26 to GPS-33): what it sends the module goes out here, with a line in the cable
/// log (TERM-28); its phase goes to [`GPS_PHASE`]. The power wanted (PA8) is the caller's, through GPS-4's rule.
/// True as the module comes out of standby (its own wake or the byte): the parser has started afresh, so the fix
/// from before the sleep isn't taken for this wake's.
#[inline(never)] // in .slow_text (memory.x): once a look of the GPS task, never from an interrupt
fn manage(mgr: &mut GpsManager, nmea: &mut Nmea, phase: &mut GpsPhase, now: u32, d: &GpsDemand, keyed: bool) -> (GpsOut, bool) {
    let out = mgr.step(now, d, keyed);
    let was = core::mem::replace(phase, mgr.phase(now));
    let woke = matches!(was, GpsPhase::Standby(_)) && !matches!(*phase, GpsPhase::Standby(_));
    if woke {
        nmea.reset();
        crate::bringup::line(b"gps: awake");
    }
    GPS_PHASE.store(phase.pack(), Relaxed);
    if let Some(s) = out.standby_s {
        let mut b = [0u8; 18];
        let n = casic(CAS_STANDBY, s, &mut b);
        uart::write(UART_GPS, &b[..n]);
        crate::bringup::linef(|l| {
            let _ = ufmt::uwrite!(l, "gps: standby {} s", s);
        });
    }
    if out.poke {
        uart::write(UART_GPS, POKE);
        crate::bringup::line(b"gps: wake byte (needed sooner)");
    }
    if out.cycled {
        crate::bringup::line(b"gps: no NMEA after a wake: power cycled");
    }
    (out, woke)
}

/// TERM-30: the GGA sentence just parsed, with its `$`, into [`crate::chan::GGA`] for the terminal's `gps`.
#[inline(never)] // in .slow_text (memory.x): once a second, never from an interrupt
fn keep_gga(nmea: &Nmea, now: u32) {
    crate::chan::GGA.lock(|g| {
        let mut g = g.borrow_mut();
        let s = nmea.last();
        let n = s.len().min(g.data.len() - 1);
        g.data[0] = b'$';
        g.data[1..=n].copy_from_slice(&s[..n]);
        (g.n, g.at_ms) = (n as u8 + 1, now);
    });
}

#[embassy_executor::task]
pub async fn task() {
    GPS_PWR_N.write(true); // off until the first look below decides
    uart::init(UART_GPS, BAUD);
    let mut nmea = Nmea::new();
    let (tx, sky) = (POSITION.sender(), SKY.sender());
    tx.send(GpsFix::default());
    let mut cfg = GpsCfg::DEFAULT;
    // The power and PB10 decisions (`ferrite_radio::gps`, host-tested), and when the fix and the sky were last
    // published.
    let mut power = GpsPower::default();
    let (mut mgr, mut demand) = (GpsManager::new(), GpsDemand::default());
    let mut phase = GpsPhase::Off;
    let (mut fix_at, mut sky_at) = (0u32, 0u32);
    let mut last_state = (false, 0u8);
    // The cable log (TERM-28): sentences and bytes from the module per 5 s while powered, and the fix.
    let (mut sentences, mut bytes, mut log_at) = (0u32, 0u32, 0u32);
    loop {
        if let Some(c) = GPS_CFG.try_take() {
            cfg = c;
        }
        if let Some(d) = GPS_DEMAND.try_take() {
            demand = d;
        }
        let now = now_ms();
        let rf = RF_STATE.load(Relaxed);
        let (keyed, rx) = (rf & RF_BUSY_KEYED != 0, rf & RF_BUSY_RX != 0);
        // GPS-26: the manager's look (standby, wake byte, power cycle); nothing sent while keyed (GPS-32).
        let (m, woke) = manage(&mut mgr, &mut nmea, &mut phase, now, &GpsDemand { on: cfg.on, ..demand }, keyed);
        if woke {
            last_state = (false, 0);
            (sentences, bytes, log_at) = (0, 0, now);
        }
        // GPS-22: PB10 low while keyed, back to the UART after. GPS-4: off around signals as asked, back on
        // `QUIET_MS` after the last.
        let out = power.step(now, cfg.on && m.power, cfg.powered(rx, keyed), keyed);
        match out.pb10_low {
            Some(true) => GPS_TX.output(false, Mode::OutPp),
            Some(false) => GPS_TX.output(true, Mode::AfPp),
            None => {}
        }
        let powered = power.powered;
        if let Some(want) = out.power {
            GPS_PWR_N.write(!want);
            mgr.powered(now, want);
            crate::bringup::line(if want { b"gps: power on" } else { b"gps: power off" });
            (sentences, bytes, log_at) = (0, 0, now);
            if !want {
                nmea.reset();
                // GPS on but powered off for a while (GPS-4, the power cycle): the last fix stays, with its age.
                tx.send(GpsFix { on: cfg.on, at_ms: now, ..GpsFix::default() });
                sky.send(GpsSky::default());
                last_state = (false, 0);
            } else {
                tx.send(GpsFix { on: true, at_ms: now, ..GpsFix::default() });
            }
        }
        let mut fresh = false;
        while let Some(c) = uart::getc(UART_GPS) {
            if powered {
                bytes += 1;
                let done = nmea.feed(c);
                if done && nmea.last_is_gga() {
                    keep_gga(&nmea, now);
                }
                sentences += done as u32;
                fresh |= done;
            }
        }
        let asleep = matches!(phase, GpsPhase::Standby(_));
        if powered && !asleep && now.wrapping_sub(log_at) >= 5_000 {
            let f = nmea.fix(now);
            crate::bringup::linef(|b| {
                let _ = ufmt::uwrite!(b, "gps: {} sentences, {} bytes in 5 s; fix {}D, {} used, {} in view", sentences, bytes, f.fix, f.used, f.view);
            });
            (sentences, bytes, log_at) = (0, 0, now);
        }
        if powered && fresh {
            let f = nmea.fix(now);
            if f.has_fix() {
                mgr.fix(now, ((f.speed_kn10 as u32 * 1852 + 5000) / 10_000) as u16);
            } else {
                mgr.sentence(now);
            }
            let state = (f.has_fix(), f.fix);
            if state != last_state || now.wrapping_sub(fix_at) >= PUBLISH_MS {
                tx.send(f);
                (fix_at, last_state) = (now, state);
            }
            if now.wrapping_sub(sky_at) >= 1000 {
                sky.send(nmea.sky());
                sky_at = now;
            }
        }
        Timer::after_millis(POLL_MS).await;
    }
}

/// The GPS seen from the UI task: the watches read into the store's GPS data, the status bar's GPS and clock
/// (GPS-6, GPS-7, GPS-8), the settings sent to the GPS task, and the arrival alarm (GPS-12).
pub struct GpsUi {
    pos: embassy_sync::watch::Receiver<'static, embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, GpsFix, 3>,
    sky: embassy_sync::watch::Receiver<'static, embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, GpsSky, 1>,
    cfg: Option<GpsCfg>,
    /// UTC seconds and the milliseconds they were true at: the clock runs on from the last GPS time even with
    /// the module off (no RTC on this board, GPS-23).
    clock: Option<(u32, u32)>,
    /// When the store's clock was last set (its log and message times), in UTC seconds.
    store_time: u32,
    arrival: ferrite_app::gpsdata::Arrival,
    /// Who needs a position, as last sent to the GPS task (GPS-26).
    demand: Option<GpsDemand>,
}

impl GpsUi {
    pub fn new() -> Self {
        GpsUi {
            pos: POSITION.receiver().unwrap(),
            sky: SKY.receiver().unwrap(),
            cfg: None,
            clock: None,
            store_time: 0,
            arrival: ferrite_app::gpsdata::Arrival::default(),
            demand: None,
        }
    }

    /// Once a turn of the UI loop.
    pub fn step(&mut self, app: &mut ferrite_app::App, store: &crate::store::Store, dev: &mut ferrite_core::msg::Device) {
        use ferrite_app::chan::ChannelStore;
        use ferrite_app::data::Data;
        let now = now_ms();
        let g = &store.extra.gps;
        if let Some(f) = self.pos.try_changed() {
            g.set_fix(f);
        }
        if let Some(s) = self.sky.try_changed() {
            g.set_sky(s);
        }
        g.tick(now);
        let set = app.settings();
        g.set_fixed(set.fixed_on.then_some(set.fixed));
        let cfg = ferrite_app::gpsdata::cfg(set);
        if self.cfg != Some(cfg) {
            GPS_CFG.signal(cfg);
            self.cfg = Some(cfg);
        }
        (dev.gps, dev.sats) = g.state();
        // GPS-26 to GPS-28: who needs the module awake, sent when it changes (a beacon time that only moves with
        // the second's rounding isn't a change); and what the manager has it doing, for Status.
        let alarm = self.arrival.armed(ferrite_app::gpsdata::arrival_radius_m(app.settings()));
        let d = ferrite_app::gpsdata::demand(app, store.data(), alarm, now);
        let moved = |a: Option<u32>, b: Option<u32>| match (a, b) {
            (Some(a), Some(b)) => a.abs_diff(b) > 2000,
            (a, b) => a.is_some() != b.is_some(),
        };
        if self.demand.is_none_or(|o| GpsDemand { beacon_at: d.beacon_at, ..o } != d || moved(o.beacon_at, d.beacon_at)) {
            GPS_DEMAND.signal(d);
            self.demand = Some(d);
        }
        dev.gps_phase = GpsPhase::unpack(GPS_PHASE.load(Relaxed));
        // GPS-23: a clock set by hand runs from now; GPS time takes over when it comes, while 0 6 6 says GPS.
        if let Some(u) = app.take_clock_set() {
            self.clock = Some((u, now));
            self.store_time = 0;
        }
        let set = app.settings();
        // GPS-7: GPS time sets the clock while "from GPS" is on; the clock keeps running from it after.
        if set.clock_gps {
            if let Some(u) = g.utc_now() {
                self.clock = Some((u, now));
            }
        }
        let utc = self.clock.map(|(u, at)| u.wrapping_add(now.wrapping_sub(at) / 1000));
        match utc {
            Some(u) => {
                let (min, is_utc) = ferrite_app::gpsdata::local_clock(u, set.tz);
                (dev.clock_min, dev.utc) = (Some(min), is_utc);
                // The log's and messages' times (INBOX-6): set when first known and every 10 minutes after.
                if self.store_time == 0 || u.wrapping_sub(self.store_time) >= 600 {
                    store.set_time(u);
                    self.store_time = u;
                }
            }
            None => (dev.clock_min, dev.utc) = (None, false),
        }
        // GPS-12: the waypoint being gone to is the one the Go to page shows; the alarm keeps it after.
        if let ferrite_app::Screen::Gps(s) = app.screen() {
            if s.page == ferrite_app::screens::gps::Page::Go {
                let wp = store.data().waypoint(s.cur.sel).map(|w| (s.cur.sel, w.pos));
                self.arrival.target(wp);
            }
        }
        let radius = ferrite_app::gpsdata::arrival_radius_m(set);
        // Only a fresh fix moves the alarm (GPS-2): the last one, kept while the module sleeps, already counted.
        if let Some(here) = g.fix().filter(|f| f.age_s != ferrite_app::gpsdata::FIXED_AGE && (f.age_s as u32) * 1000 < ferrite_app::gpsdata::STALE_MS).map(|f| f.pos) {
            if let Some((i, m)) = self.arrival.check(here, radius) {
                if let Some(w) = store.data().waypoint(i) {
                    arrived(app, store, &w, m);
                }
            }
        }
    }
}

/// GPS-12: a toast with the waypoint and the distance, an event in the log, and the tone when 0 7 5 4 is on.
fn arrived(app: &mut ferrite_app::App, store: &crate::store::Store, w: &ferrite_app::data::Waypoint, m: u32) {
    use ferrite_app::data::{Event, Src};
    let mut text = ferrite_ui::text::Buf::<64>::new();
    let _ = ufmt::uwrite!(text, "Arrived, {} m", m);
    app.notify(w.name.as_bytes(), text.as_bytes(), false);
    let mut name = ferrite_ui::text::Buf::<20>::new();
    name.set(w.name.as_bytes());
    let e = Event { src: Src::Gps, ago_s: 0, freq: 0, slot: 0, unread: true, id: 0, x: 0, y: 0, pos: Some(w.pos), name, text };
    store.log(&e);
    if app.settings().arrival_tone {
        let _ = crate::chan::RADIO.try_send(ferrite_core::msg::RadioCmd::Beep { hz: 880, ms: 400 });
    }
}
