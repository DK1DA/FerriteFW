//! GPS and waypoints (manual chapter 28, GPS-2, GPS-10 to GPS-12): the position page (MENU 0 6 2; fix,
//! satellites, accuracy (from HDOP), fix age, position, grid square, altitude, speed, heading, UTC) and its satellites page
//! (C16, C17); the waypoint list with distance and bearing from the fix (MENU 0 6 7, C14) and going to one,
//! with the compass (C15). # marks a waypoint here on every page; hold MENU for the options.

use super::lists::*;
use super::opts::Of;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Data, Fix, Op, WpSrc};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{Device, GpsState};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Position,
    Satellites,
    Waypoints,
    Go,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Gps {
    pub page: Page,
    /// The waypoint list's position.
    pub cur: Cursor,
}

impl Gps {
    pub fn new(page: Page) -> Self {
        Gps { page, cur: Cursor::default() }
    }
}

const ALL: fn(u16) -> bool = |_| true;

impl View for Gps {
    /// KEY-19: ◀▶ switch position and satellites; elsewhere they are the knob.
    fn own_arrows(self) -> bool {
        true
    }

    fn options(self, app: &mut App) -> bool {
        app.open_screen(if matches!(self.page, Page::Waypoints | Page::Go) { Of::Waypoints } else { Of::Gps }.screen());
        true
    }

    fn name(self) -> &'static str {
        match self.page {
            Page::Position => "GPS position",
            Page::Satellites => "GPS satellites",
            Page::Waypoints => "Waypoints",
            Page::Go => "Go to waypoint",
        }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(match self.page {
            Page::Position => b"GPS".as_slice(),
            Page::Satellites => b"Sky",
            Page::Waypoints => b"Waypoint",
            Page::Go => b"Go to",
        });
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        let to = |page| Screen::Gps(Gps { page, ..self });
        match (e.key, e.press, self.page) {
            (Key::Left | Key::Right, Short, Page::Position) => app.screen = to(Page::Satellites),
            (Key::Left | Key::Right, Short, Page::Satellites) => app.screen = to(Page::Position),
            (Key::Left | Key::Right, Short | Repeat, _) => self.knob(app, if e.key == Key::Right { 1 } else { -1 }, store),
            (Key::Up | Key::Down, Short | Repeat, Page::Waypoints | Page::Go) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short, Page::Waypoints) if d.waypoints() > 0 => app.screen = to(Page::Go),
            (Key::Menu, Short, Page::Position | Page::Satellites) => app.screen = to(if self.page == Page::Position { Page::Satellites } else { Page::Position }),
            (Key::Hash, Short, _) => {
                // GPS-10: a waypoint at this fix
                if d.request(Op::MarkHere) {
                    app.show_toast(b"Waypoint marked", b"here", false);
                } else {
                    app.show_toast(b"No GPS fix", b"can't mark here", false);
                }
            }
            (Key::Exit, Short, Page::Go) => app.screen = to(Page::Waypoints),
            (Key::Exit, Short, _) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        if matches!(self.page, Page::Waypoints | Page::Go) {
            let cur = self.cur.step(d, store.data().waypoints(), ROWS2, &ALL);
            app.screen = Screen::Gps(Gps { cur, ..self });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, self.page);
        let fix = data.fix().filter(|_| cx.dev.gps == GpsState::Fix);
        let set = (app.set.units, app.set.coord_fmt, app.set.tz, app.set.clock);
        if !app.gate((self, data.version(), cx.dev.gps as u8, cx.dev.clock_min, fix, set)) {
            return;
        }
        match self.page {
            Page::Position => position(app, lcd, data, fix, cx.dev),
            Page::Satellites => satellites(app, lcd, data, fix),
            Page::Waypoints => waypoints(app, lcd, data, fix, self.cur),
            Page::Go => go(app, lcd, data, fix, self.cur.sel),
        }
    }

    fn hints(self, _: &App) -> Hints {
        match self.page {
            Page::Position => Hints::new(&[(ARROWS_LR, "Sats"), ("#", "Mark"), ("Hold MENU", "More")]),
            Page::Satellites => Hints::new(&[(ARROWS_LR, "Position"), ("#", "Mark"), ("BACK", "Back")]),
            Page::Waypoints => Hints::new(&[("OK", "Go to"), ("#", "Mark"), ("Hold MENU", "More")]),
            Page::Go => Hints::new(&[(ARROWS_UD, "Next"), ("BACK", "List")]),
        }
    }
}

// ---- units and formats (GPS-16) ----

/// A distance in the units of 0 6 4: metric "850 m", "4.2 km"; imperial "300 ft", "2.6 mi"; nautical "1.84 nmi".
pub(crate) fn dist_u(m: u32, units: u8, b: &mut Buf<16>) {
    b.clear();
    let _ = match units {
        1 => {
            let ft = m as u64 * 3281 / 1000;
            let tenths = m as u64 * 10 / 1609;
            match tenths {
                0 => uwrite!(b, "{} ft", ft as u32),
                1..=99 => uwrite!(b, "{}.{} mi", (tenths / 10) as u32, (tenths % 10) as u32),
                _ => uwrite!(b, "{} mi", ((tenths + 5) / 10) as u32),
            }
        }
        2 => {
            let h = m as u64 * 100 / 1852; // hundredths of a nautical mile
            if h < 1000 { uwrite!(b, "{}.{}{} nmi", (h / 100) as u32, (h / 10 % 10) as u32, (h % 10) as u32) } else { uwrite!(b, "{} nmi", ((h + 50) / 100) as u32) }
        }
        _ => match m {
            0..=999 => uwrite!(b, "{} m", m),
            1000..=9_949 => uwrite!(b, "{}.{} km", (m + 50) / 1000, (m + 50) % 1000 / 100),
            _ => uwrite!(b, "{} km", (m + 500) / 1000),
        },
    };
}

/// A speed in the units of 0 6 4: km/h, mph or kn.
fn speed_u(kmh: u16, units: u8, b: &mut Buf<40>) {
    let _ = match units {
        1 => uwrite!(b, "{} mph", (kmh as u32 * 621 + 500) / 1000),
        2 => uwrite!(b, "{} kn", (kmh as u32 * 540 + 500) / 1000),
        _ => uwrite!(b, "{} km/h", kmh),
    };
}

/// An altitude: feet in imperial, metres otherwise.
fn alt_u(m: i32, units: u8, b: &mut Buf<40>) {
    let _ = if units == 1 { uwrite!(b, "{} ft", m * 3281 / 1000) } else { uwrite!(b, "{} m", m) };
}


/// A time of day from seconds: "14:32", or "2:32 pm" with the 12 h clock.
fn hhmm(t: u32, h12: bool, b: &mut Buf<40>) {
    let (h, m) = (t / 3600 % 24, t / 60 % 60);
    let _ = if h12 { uwrite!(b, "{}:{}{} {}", (h + 11) % 12 + 1, z(m), m, if h < 12 { "am" } else { "pm" }) } else { uwrite!(b, "{}{}:{}{}", z(h), h, z(m), m) };
}

/// GPS-17: below this, the GPS course is noise (no magnetic compass), so it reads "stopped".
const MOVING_KMH: u16 = 3;

// ---- the position page (C16, GPS-15) ----

/// Tiles: two columns of three, 112 x 41.
const TILE_W: i32 = 112;
const TILE_H: i32 = 41;
const TILE_Y: i32 = 108;
const TILE_DY: i32 = 45;
const NEAR_Y: i32 = 246;

/// A labelled value in a box: the label small, the value in 15 px bold.
fn tile(lcd: &mut impl Panel, k: i32, label: &[u8], value: &[u8], c: u16) {
    let (x, y) = (if k % 2 == 0 { 6 } else { 122 }, TILE_Y + k / 2 * TILE_DY);
    ferrite_ui::canvas::banded(lcd, x, y, TILE_W, TILE_H, BG, |lcd| {
        lcd.text4_rows(x + 8, y + 15, label, &UI_12, SUB, BG, y + 1, y + 18);
        lcd.text4_rows(x + 8, y + 35, w::fit(value, &UI_15B, TILE_W - 14), &UI_15B, c, BG, y + 18, y + TILE_H - 1);
        lcd.frame_r(x, y, TILE_W, TILE_H, 2, LINE);
    });
}

/// C16: the fix at a glance.
fn position(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>, dev: &Device) {
    let gps = dev.gps;
    let seen = data.sats();
    let (units, fmt) = (app.set.units, app.set.coord_fmt);
    // Status line (GPS-15): fix, satellites, accuracy; the fix's age at the right, amber once stale (GPS-2).
    let (l, age) = status_line(gps, fix, seen);
    let stale = fix.is_some_and(|f| f.age_s > 10);
    if app.dirty(R_TOP, Mix::new().h(&l.as_bytes()).h(&age.as_bytes()).h(&stale).get()) {
        // the age gets just its own width at the right, and the status text is fitted to what is left, so
        // neither clears part of the other ("±5 m" lost its m)
        let aw = text4_width(age.as_bytes(), &UI_12);
        head(lcd, w::fit(l.as_bytes(), &UI_12, 232 - aw - 16 - 8), b"");
        w::field(lcd, 232 - aw, aw, HEAD_Y, HEAD_Y + 20, HEAD_Y + 14, age.as_bytes(), &w::st(&UI_12, if stale { AMBER } else { SUB }, BG, Align::Right));
    }
    // Between a fix and none, the page's layout changes: clear it and redraw every part.
    let none: (&[u8], &[u8]) = if gps == GpsState::Off { (b"GPS is off", b"MENU 0 6 1 turns it on") } else { (b"No fix yet", b"Left or right: the satellites") };
    if app.dirty(R_HELP, Mix::new().h(&fix.is_some()).get()) {
        app.drawn.hash[R_ROW..R_ROW + 8].fill(0);
        app.drawn.hash[R_NOTE] = 0;
        // in bands, with the message when the fix went: no blank page between (DISP-25)
        ferrite_ui::canvas::banded(lcd, 0, HEAD_Y + 21, 240, 298 - HEAD_Y - 21, BG, |lcd| {
            if fix.is_none() {
                App::empty_now(lcd, none.0, none.1);
            }
        });
        if fix.is_none() {
            app.dirty(R_NOTE, Mix::new().h(&none).get());
        }
    }
    let Some(f) = fix else {
        app.empty(lcd, none.0, none.1);
        return;
    };
    // The position, large (GPS-15): latitude and longitude in the 0 6 3 format; as a grid square, the square
    // large and the coordinates small under it.
    if app.dirty(R_ROW, Mix::new().any(&f.pos).h(&fmt).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 42, 240, 64, BG, |lcd| {
            let (big, small) = position_lines(f, fmt);
            if fmt == 3 {
                lcd.text4(10, 72, big.as_bytes(), &NAME_26, TEXT, BG);
                lcd.text4_rows(10, 98, w::fit(small.as_bytes(), &UI_12, 222), &UI_12, SUB, BG, 84, 104);
            } else {
                lcd.text4_rows(10, 67, w::fit(big.as_bytes(), &NAME_22, 224), &NAME_22, TEXT, BG, 42, 74);
                lcd.text4_rows(10, 97, w::fit(small.as_bytes(), &NAME_22, 224), &NAME_22, TEXT, BG, 74, 106);
            }
        });
    }
    // Six tiles (GPS-15, GPS-16, GPS-17).
    let moving = f.speed_kmh >= MOVING_KMH;
    for (k, (label, what)) in TILES.into_iter().enumerate() {
        let v = tile_value(app, f, dev, what);
        if app.dirty(R_ROW + 1 + k, Mix::new().h(&v.as_bytes()).h(&k).get()) {
            tile(lcd, k as i32, label, v.as_bytes(), if what == 3 && !moving { SUB } else { TEXT });
        }
    }
    // The nearest waypoint (GPS-15): name, distance and bearing, a small arrow.
    let near = nearest(data, f);
    let key = near.map(|((m, d), wp)| (m, d, wp.name));
    if app.dirty(R_NOTE, Mix::new().h(&key).h(&units).get()) {
        let y = NEAR_Y;
        ferrite_ui::canvas::banded(lcd, 6, y, 228, 47, BG, |lcd| {
            lcd.text4_rows(14, y + 15, b"Nearest waypoint", &UI_12, SUB, BG, y + 1, y + 18);
            match near {
                Some(((m, deg), wp)) => {
                    let d = near_text(m, deg, units);
                    let dw = text4_width(d.as_bytes(), &UI_12B);
                    // the arrow keeps 6 px from the frame, the distance 8 px from the arrow
                    lcd.text4_rows(14, y + 37, w::fit(wp.name.as_bytes(), &UI_15B, 176 - dw), &UI_15B, TEXT, BG, y + 19, y + 46);
                    lcd.text4_rows(196 - dw, y + 37, d.as_bytes(), &UI_12B, TEXT, BG, y + 19, y + 46);
                    arrow(lcd, 215, y + 30, 10, deg as i32, BLUE, BG);
                }
                None => {
                    lcd.text4_rows(14, y + 37, b"None: # marks one here", &UI_12, DIM, BG, y + 19, y + 46);
                }
            }
            lcd.frame_r(6, y, 228, 47, 2, LINE);
        });
    }
}

/// The status line (GPS-15) and the fix's age.
fn status_line(gps: GpsState, fix: Option<Fix>, seen: u8) -> (Buf<40>, Buf<12>) {
    let mut l = Buf::<40>::new();
    let mut age = Buf::<12>::new();
    match (gps, fix) {
        (GpsState::Off, _) => l.set(b"GPS off"),
        (_, None) => {
            let _ = uwrite!(l, "Searching \u{b7} {} in view", seen);
        }
        (_, Some(f)) => {
            let m = ((f.hdop_x10 as u32 * 5 + 5) / 10).max(1);
            let _ = uwrite!(l, "{} fix \u{b7} {}/{} sats \u{b7} \u{b1}{} m", if f.three_d { "3D" } else { "2D" }, f.used, seen, m);
            let _ = uwrite!(age, "{} s", f.age_s);
        }
    }
    (l, age)
}

/// The position large (GPS-15.2), in 0 6 3's format: latitude and longitude; with Grid square the square, and
/// both coordinates (degrees and minutes) small under it.
fn position_lines(f: Fix, fmt: u8) -> (Buf<48>, Buf<48>) {
    let (mut la, mut lo) = (Buf::<24>::new(), Buf::<24>::new());
    if fmt == 3 {
        coord(f.pos.lat, "N", "S", 1, &mut la);
        coord(f.pos.lon, "E", "W", 1, &mut lo);
        let mut both = Buf::<48>::from(la.as_bytes());
        both.push(b"  ");
        both.push(lo.as_bytes());
        (Buf::from(&grid(f.pos)), both)
    } else {
        coord(f.pos.lat, "N", "S", fmt, &mut la);
        coord(f.pos.lon, "E", "W", fmt, &mut lo);
        (Buf::from(la.as_bytes()), Buf::from(lo.as_bytes()))
    }
}

/// The six tiles (GPS-15.3): label and which.
const TILES: [(&[u8], u8); 6] = [(b"Grid", 0), (b"Altitude", 1), (b"Speed", 2), (b"Course", 3), (b"Local time", 4), (b"UTC", 5)];

fn tile_value(app: &App, f: Fix, dev: &Device, what: u8) -> Buf<40> {
    let (units, h12) = (app.set.units, app.set.clock == 1);
    let mut v = Buf::<40>::new();
    match what {
        0 => v.set(&grid(f.pos)),
        1 => alt_u(f.alt_m, units, &mut v),
        2 => speed_u(f.speed_kmh, units, &mut v),
        3 if f.speed_kmh < MOVING_KMH => v.set(b"stopped"),
        3 => {
            let _ = uwrite!(v, "{} {}\u{b0}", compass_name(f.heading), f.heading);
        }
        // the radio's clock, as the status bar shows it (GPS-7: local time from GPS and the time zone)
        4 => match dev.clock_min {
            Some(m) => hhmm(m as u32 * 60, h12, &mut v),
            None => v.set(b"--:--"),
        },
        _ => {
            hhmm(f.utc_s, false, &mut v);
            v.push(b" Z");
        }
    }
    v
}

/// The nearest waypoint (GPS-15.4): its distance and bearing, and the waypoint.
fn nearest(data: &dyn Data, f: Fix) -> Option<((u32, u16), crate::data::Waypoint)> {
    (0..data.waypoints()).filter_map(|i| data.waypoint(i)).map(|wp| (dist_bearing(f.pos, wp.pos), wp)).min_by_key(|((m, _), _)| *m)
}

/// "1.2 km NE".
fn near_text(m: u32, deg: u16, units: u8) -> Buf<16> {
    let mut d = Buf::<16>::new();
    dist_u(m, units, &mut d);
    let _ = uwrite!(d, " {}", compass_name(deg));
    d
}

/// The position page for the UI readback (LINK-19): the status line and age; with a fix the position's two
/// lines, the six tiles and the nearest waypoint (its name, "1.2 km NE" and the bearing), as drawn.
pub(crate) struct PosShown {
    pub status: Buf<40>,
    pub age: Buf<12>,
    pub fix: Option<(Buf<48>, Buf<48>, [Buf<40>; 6])>,
    pub near: Option<(Buf<16>, Buf<16>, u16)>,
}

pub(crate) fn position_shown(app: &App, data: &dyn Data, dev: &Device) -> PosShown {
    let fix = data.fix().filter(|_| dev.gps == GpsState::Fix);
    let (status, age) = status_line(dev.gps, fix, data.sats());
    let fix_lines = fix.map(|f| {
        let (a, b) = position_lines(f, app.set.coord_fmt);
        let mut t = [Buf::<40>::new(); 6];
        for (i, (_, what)) in TILES.iter().enumerate() {
            t[i] = tile_value(app, f, dev, *what);
        }
        (a, b, t)
    });
    let near = fix.and_then(|f| nearest(data, f)).map(|((m, deg), wp)| (Buf::from(wp.name.as_bytes()), near_text(m, deg, app.set.units), deg));
    PosShown { status, age, fix: fix_lines, near }
}

/// The tiles' labels.
pub(crate) fn tile_names() -> impl Iterator<Item = &'static [u8]> {
    TILES.iter().map(|t| t.0)
}

// ---- the satellites page (C17, GPS-18) ----

/// The sky plot's centre and radius (the horizon); rings at 30 and 60 degrees of elevation.
const SKY: (i32, i32, i32) = (120, 117, 70);

/// C17: where the satellites are (a sky plot, north up) and how strong (bars), used ones coloured.
fn satellites(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>) {
    let n = data.sats().min(12);
    let mut l = Buf::<32>::new();
    let used = (0..n).filter(|&i| data.sat(i).is_some_and(|s| s.used)).count();
    let _ = uwrite!(l, "{} in view \u{b7} {} used", n, used);
    let mut r = Buf::<16>::new();
    match fix {
        Some(f) => {
            let _ = uwrite!(r, "\u{b1}{} m", ((f.hdop_x10 as u32 * 5 + 5) / 10).max(1));
        }
        None => r.set(b"no fix"),
    }
    app.list_head(lcd, l.as_bytes(), r.as_bytes());
    if n == 0 {
        app.empty(lcd, b"No satellites in view", b"");
        return;
    }
    // The sky plot.
    let sky = (0..n).filter_map(|i| data.sat(i)).fold(0u32, |h, s| h.wrapping_mul(31).wrapping_add(s.prn as u32 * 7 + s.elev as u32 * 3 + s.azim as u32 + s.used as u32));
    if app.dirty(R_ROW, sky) {
        let (cx, cy, rr) = SKY;
        ferrite_ui::canvas::banded(lcd, 0, cy - rr - 4, 240, 2 * rr + 9, BG, |lcd| {
            ring(lcd, cx, cy, rr, 1, LINE);
            ring(lcd, cx, cy, rr * 2 / 3, 1, LINE);
            ring(lcd, cx, cy, rr / 3, 1, LINE);
            lcd.fill(cx - rr, cy, 2 * rr + 1, 1, LINE);
            lcd.fill(cx, cy - rr, 1, 2 * rr + 1, LINE);
            for (t, x, y) in [(b"N", cx - 3, cy - rr + 14), (b"S", cx - 3, cy + rr - 4), (b"E", cx + rr - 11, cy + 5), (b"W", cx - rr + 4, cy + 5)] {
                lcd.text4(x, y, t, &UI_12, SUB, BG);
            }
            for i in 0..n {
                let Some(s) = data.sat(i) else { continue };
                let d = rr * (90 - s.elev.min(90) as i32) / 90;
                let a = s.azim as i32 * 10;
                let (x, y) = (cx + d * sin10(a) / 4096, cy - d * cos10(a) / 4096);
                let c = match (s.used, s.beidou) {
                    (false, _) => DIM,
                    (true, false) => BLUE,
                    (true, true) => VIOLET,
                };
                if s.used {
                    lcd.fill_r(x - 4, y - 4, 9, 9, 4, c);
                } else {
                    ring(lcd, x, y, 4, 1, c);
                }
                let mut p = Buf::<4>::new();
                let _ = uwrite!(p, "{}", s.prn);
                // to the right of the dot, unless that would cross the horizon ring (or the E): then to its left
                let pw = text4_width(p.as_bytes(), &UI_12);
                let lx = if x + 6 + pw > cx + rr - 12 { x - 6 - pw } else { x + 6 };
                lcd.text4(lx, y + 4, p.as_bytes(), &UI_12, if s.used { TEXT } else { SUB }, BG);
            }
        });
    }
    // The signal bars (dB-Hz; 50 fills them).
    let (top, base) = (204, 254);
    let bw = 224 / n as i32;
    for i in 0..n {
        let Some(s) = data.sat(i) else { continue };
        if !app.dirty(R_ROW + 1 + i as usize, Mix::new().h(&s.prn).h(&s.snr).h(&s.used).h(&s.beidou).h(&bw).get()) {
            continue;
        }
        let x = 8 + i as i32 * bw;
        let h = (s.snr.min(50) as i32 * (base - top) / 50).max(2);
        ferrite_ui::canvas::banded(lcd, x, top, bw, base - top + 21, BG, |lcd| {
            lcd.fill_r(x + 2, base - h, bw - 4, h, 1, match (s.used, s.beidou) {
                (false, _) => DIM,
                (true, false) => BLUE,
                (true, true) => VIOLET,
            });
            let mut p = Buf::<4>::new();
            let _ = uwrite!(p, "{}", s.prn);
            w::field(lcd, x, bw, base + 3, base + 20, base + 16, p.as_bytes(), &w::st(&UI_12, if s.used { TEXT } else { SUB }, BG, Align::Center));
        });
    }
    if app.dirty(R_NOTE, 1) {
        w::field(lcd, 8, 224, 278, 296, 291, b"Used: blue GPS, violet BeiDou", const { &w::st(&UI_12, DIM, BG, Align::Left) });
    }
}

fn src_name(s: WpSrc) -> &'static [u8] {
    match s {
        WpSrc::Marked => b"Mark",
        WpSrc::Aprs => b"APRS",
        WpSrc::Link => b"Link",
        WpSrc::Grid => b"Grid",
    }
}

/// C14: the waypoints with distance and bearing from the fix (GPS-11).
fn waypoints(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>, cur: Cursor) {
    let n = data.waypoints();
    let mut l = Buf::<24>::new();
    let _ = uwrite!(l, "{} {}", n, if n == 1 { "waypoint" } else { "waypoints" });
    app.list_head(lcd, l.as_bytes(), if fix.is_some() { b"from here" } else { b"no GPS fix" });
    let units = app.set.units;
    let rows = cur.rows(n, ROWS2, &ALL, |row, i| {
        if let Some(wp) = data.waypoint(i) {
            let (mut d, mut p) = (Buf::<16>::new(), Buf::<32>::new());
            pos_text(wp.pos, app.set.coord_fmt, &mut p);
            let arrow = fix.map(|f| {
                let (m, deg) = dist_bearing(f.pos, wp.pos);
                dist_u(m, units, &mut d);
                Some(deg)
            });
            let r = Row2 { tag: src_name(wp.src), title: wp.name.as_bytes(), right: d.as_bytes(), arrow, sub: p.as_bytes(), selected: i == cur.sel, ..Row2::default() };
            app.row2(lcd, row, &r);
        }
    });
    app.rows_end(lcd, rows, ROWS2, ROW2_H);
    app.list_cue(lcd, cur, n, ROWS2, ROW2_H, rows, &ALL);
    if n == 0 {
        app.empty(lcd, b"No waypoints", b"# marks one here");
    }
}

/// The go-to dial (GPS-19): a ring with N; course up while moving (the ring turns so the direction of travel is
/// up and N moves round it, a tick marks "ahead"), north up when stopped; the arrow points to the waypoint.
fn dial(lcd: &mut impl Panel, cx: i32, cy: i32, r: i32, course: Option<u16>, bearing: Option<u16>) {
    ferrite_ui::canvas::banded(lcd, cx - r - 7, cy - r - 9, 2 * r + 15, 2 * r + 17, BG, |lcd| {
        ring(lcd, cx, cy, r, 2, LINE);
        let up = course.unwrap_or(0) as i32;
        let a = (360 - up) * 10; // where north sits, clockwise from up
        // N sits on the ring, like a compass bezel: a gap is cleared in the ring behind it
        let (nx, ny) = (cx + r * sin10(a) / 4096, cy - r * cos10(a) / 4096);
        lcd.fill(nx - 6, ny - 7, 13, 14, BG);
        lcd.text4(nx - 3, ny + 5, b"N", &UI_12B, if course.is_some() { AMBER } else { SUB }, BG);
        if course.is_some() {
            lcd.tri(cx - 4, cy - r - 9, 9, 1, TEXT); // ahead
        }
        match bearing {
            Some(b) => {
                arrow(lcd, cx, cy, r / 2, (b as i32 - up).rem_euclid(360), BLUE, BG);
                lcd.fill_r(cx - 2, cy - 2, 5, 5, 2, TEXT);
            }
            None => lcd.fill(cx - 6, cy, 13, 2, SUB),
        }
    });
}

/// C15: one waypoint: the dial, its distance big, the bearing and, while moving, the time to get there.
fn go(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>, i: u16) {
    let Some(wp) = data.waypoint(i) else {
        return app.empty(lcd, b"No such waypoint", b"MENU 0 6 7 lists them");
    };
    let units = app.set.units;
    let where_ = fix.map(|f| dist_bearing(f.pos, wp.pos));
    let course = fix.filter(|f| f.speed_kmh >= MOVING_KMH).map(|f| (f.heading, f.speed_kmh));
    if app.dirty(R_TOP, Mix::new().any(&wp).h(&where_).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 20, 240, 50, BG, |lcd| {
            lcd.text4(8, 42, wp.name.as_bytes(), &UI_12B, TEXT, BG);
            // the position page's coordinate format (deg-min under a grid square), not the lists' compact one
            let fmt = if app.set.coord_fmt == 3 { 1 } else { app.set.coord_fmt };
            let (mut la, mut lo) = (Buf::<24>::new(), Buf::<24>::new());
            coord(wp.pos.lat, "N", "S", fmt, &mut la);
            coord(wp.pos.lon, "E", "W", fmt, &mut lo);
            let mut b = Buf::<48>::new();
            b.push(la.as_bytes());
            b.push(b"  ");
            b.push(lo.as_bytes());
            lcd.text4(8, 60, w::fit(b.as_bytes(), &UI_12, 224), &UI_12, SUB, BG);
        });
    }
    if app.dirty(R_NOTE, Mix::new().h(&where_).h(&course).get()) {
        dial(lcd, 120, 142, 58, course.map(|c| c.0), where_.map(|w| w.1));
        let mut t = Buf::<40>::new();
        match course {
            Some((_, kmh)) => {
                t.push("Course up \u{b7} ".as_bytes());
                speed_u(kmh, units, &mut t);
            }
            None => t.set(b"North up"),
        }
        w::field(lcd, 8, 224, 204, 221, 217, t.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Center) });
    }
    if app.dirty(R_HELP, Mix::new().h(&where_).h(&course).h(&units).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 222, 240, 76, BG, |lcd| {
            match where_ {
                Some((m, deg)) => {
                    let mut d = Buf::<16>::new();
                    dist_u(m, units, &mut d);
                    let (num, unit) = d.as_bytes().split_at(d.as_bytes().iter().position(|&c| c == b' ').unwrap_or(d.len()));
                    let wn = text4_width(num, &DIG_46) + 4 + text4_width(unit.trim_ascii_start(), &UI_12B);
                    let x = 120 - wn / 2;
                    let x2 = x + lcd.text4(x, 270, num, &DIG_46, TEXT, BG);
                    lcd.text4(x2 + 4, 270, unit.trim_ascii_start(), &UI_12B, TEXT, BG);
                    let mut b = Buf::<40>::new();
                    let _ = uwrite!(b, "{} {}\u{b0}", compass_name(deg), deg);
                    if let Some((_, kmh)) = course {
                        // ETA at the present speed
                        let s = m as u64 * 36 / (kmh as u64 * 10).max(1);
                        let _ = if s < 3600 { uwrite!(b, " \u{b7} ETA {} min", (s / 60).max(1) as u32) } else { uwrite!(b, " \u{b7} ETA {} h {}", (s / 3600) as u32, (s / 60 % 60) as u32) };
                    }
                    w::field(lcd, 8, 224, 277, 296, 292, b.as_bytes(), const { &w::st(&UI_12B, TEXT, BG, Align::Center) });
                }
                None => {
                    w::field(lcd, 8, 224, 240, 260, 255, b"No GPS fix: no distance", const { &w::st(&UI_12, SUB, BG, Align::Center) });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    fn d(m: u32, u: u8) -> std::string::String {
        let mut b = Buf::<16>::new();
        dist_u(m, u, &mut b);
        std::string::String::from_utf8(b.as_bytes().to_vec()).unwrap()
    }

    /// GPS-16: distances in the units of 0 6 4.
    #[test]
    fn distances_follow_units() {
        assert_eq!(d(599, 0), "599 m");
        assert_eq!(d(3_300, 0), "3.3 km");
        assert_eq!(d(25_400, 0), "25 km");
        assert_eq!(d(100, 1), "328 ft");
        assert_eq!(d(3_300, 1), "2.0 mi");
        assert_eq!(d(40_000, 1), "25 mi");
        assert_eq!(d(3_300, 2), "1.78 nmi");
        assert_eq!(d(40_000, 2), "22 nmi");
    }

    /// GPS-16: speed and altitude in the chosen units.
    #[test]
    fn speed_and_altitude() {
        let s = |k, u| {
            let mut b = Buf::<40>::new();
            speed_u(k, u, &mut b);
            std::string::String::from_utf8(b.as_bytes().to_vec()).unwrap()
        };
        assert_eq!(s(46, 0), "46 km/h");
        assert_eq!(s(46, 1), "29 mph");
        assert_eq!(s(46, 2), "25 kn");
        let mut b = Buf::<40>::new();
        alt_u(74, 1, &mut b);
        assert_eq!(b.as_bytes(), b"242 ft");
    }

    /// GPS-15.2: the coordinate formats of 0 6 3.
    #[test]
    fn coordinate_formats() {
        let c = |v, f| {
            let mut b = Buf::<24>::new();
            coord(v, "N", "S", f, &mut b);
            std::string::String::from_utf8(b.as_bytes().to_vec()).unwrap()
        };
        let lat = 454_215_000; // 45.4215 degrees
        assert_eq!(c(lat, 0), "45.42150\u{b0} N");
        assert_eq!(c(lat, 1), "45\u{b0}25.290' N");
        assert_eq!(c(lat, 2), "45\u{b0}25'17.4\" N");
        assert_eq!(c(-lat, 1), "45\u{b0}25.290' S");
        assert_eq!(c(10_000_000 + 500_000, 1), "1\u{b0}03.000' N");
    }
}
