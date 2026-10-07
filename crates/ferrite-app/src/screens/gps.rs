//! GPS and waypoints (manual chapter 28, GPS-2, GPS-10 to GPS-12): the position page (MENU 0 6 2; fix,
//! satellites, accuracy (from HDOP), fix age, position, grid square, altitude, speed, heading, UTC) and its satellites page
//! (C16, C17); the waypoint list with distance and bearing from the fix (MENU 0 6 7, C14) and going to one,
//! with the compass (C15). # marks a waypoint here on every page; hold MENU for the options.

use super::lists::*;
use super::opts::Of;
use super::shared::*;
use super::table::{slow_table, Table, Val};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Data, Fix, Op, WpSrc};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{Device, GpsState, LiveAll};
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
            Page::Satellites => b"Satellites",
            Page::Waypoints => b"Waypoints",
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
        if !app.gate((self, data.version(), cx.dev.gps as u8, cx.dev.clock_min, fix)) {
            return;
        }
        match self.page {
            Page::Position => position(app, lcd, data, fix, cx.live, cx.dev),
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

/// C16: the fix.
fn position(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>, live: &LiveAll, dev: &Device) {
    let gps = dev.gps;
    let seen = data.sats();
    let mut l = Buf::<32>::new();
    let state: &[u8] = match (gps, fix) {
        (GpsState::Off, _) => b"GPS off",
        (_, None) => b"Searching",
        (_, Some(f)) => ferrite_ui::v!(l, "{} fix \u{b7} {} of {} satellites", if f.three_d { "3D" } else { "2D" }, f.used, seen),
    };
    let state = Buf::<32>::from(state);
    app.list_head(lcd, state.as_bytes(), b"");
    let Some(f) = fix else {
        app.empty(lcd, if gps == GpsState::Off { b"GPS is off" } else { b"No fix yet" }, if gps == GpsState::Off { b"MENU 0 6 1 turns it on" } else { b"Left or right: the satellites" });
        return;
    };
    app.table(lcd, &POSITION, live, dev, &f, None);
    // GPS-2: a stale fix says so
    let stale = f.age_s > 10;
    if app.dirty(R_NOTE, hash(&stale)) {
        let msg: &[u8] = if stale { b"Fix is old: position may be stale" } else { b"" };
        w::field(lcd, 8, 224, 230, 248, 243, Align::Left, msg, &UI_12, AMBER, BG);
    }
}

fn text(b: &mut Buf<40>, s: &[u8]) -> u16 {
    b.set(s);
    TEXT
}

slow_table! {
    /// C16's rows.
    ROWS: [(&str, Val<Fix>); 8] = [
        // GPS-2: estimated accuracy, HDOP x 5 m (a typical receiver's range error), not the raw HDOP.
        (slow!("Accuracy"), |c, b| {
            let m = (c.x.hdop_x10 as u32 * 5 + 5) / 10;
            let _ = uwrite!(b, "\u{b1}{} m", m.max(1));
            TEXT
        }),
        (slow!("Fix age"), |c, b| {
            let _ = uwrite!(b, "{} s", c.x.age_s);
            TEXT
        }),
        (slow!("Position"), |c, b| {
            let mut p = Buf::<32>::new();
            pos_text(c.x.pos, &mut p);
            text(b, p.as_bytes())
        }),
        (slow!("Grid"), |c, b| text(b, &grid(c.x.pos))),
        (slow!("Altitude"), |c, b| {
            let _ = uwrite!(b, "{} m", c.x.alt_m);
            TEXT
        }),
        (slow!("Speed"), |c, b| {
            let _ = uwrite!(b, "{} km/h", c.x.speed_kmh);
            TEXT
        }),
        (slow!("Heading"), |c, b| {
            let f = c.x;
            if f.speed_kmh == 0 {
                return text(b, b"- (stopped)");
            }
            let _ = uwrite!(b, "{} {}\u{b0}", compass_name(f.heading), f.heading);
            TEXT
        }),
        (slow!("UTC"), |c, b| {
            let t = c.x.utc_s;
            let _ = uwrite!(b, "{}{}:{}{}:{}{} Z", z(t / 3600), t / 3600, z(t / 60 % 60), t / 60 % 60, z(t % 60), t % 60);
            TEXT
        }),
    ];
}

slow_table! {
    POSITION: Table<Fix> = Table { rows: &ROWS, go: &[], y: (LIST_Y + 4) as i16, h: 0, style: w::KV_DETAIL };
}

/// "0" before a single digit.
fn z(v: u32) -> &'static str {
    if v < 10 { "0" } else { "" }
}

/// C17: each satellite's signal as a bar, used ones blue.
fn satellites(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>) {
    let n = data.sats().min(14);
    let mut l = Buf::<32>::new();
    let used = (0..n).filter(|&i| data.sat(i).is_some_and(|s| s.used)).count();
    let _ = uwrite!(l, "{} in view \u{b7} {} used", n, used);
    app.list_head(lcd, l.as_bytes(), if fix.is_some() { b"dB-Hz" } else { b"no fix" });
    if n == 0 {
        app.empty(lcd, b"No satellites in view", b"");
        return;
    }
    let (top, base) = (60, 250); // bar area; 50 dB-Hz fills it
    let bw = 224 / n as i32;
    for i in 0..n {
        let Some(s) = data.sat(i) else { continue };
        if !app.dirty(R_ROW + i as usize, hash(&(s, bw))) {
            continue;
        }
        let x = 8 + i as i32 * bw;
        let h = (s.snr.min(50) as i32 * (base - top) / 50).max(2);
        lcd.fill(x, top - 18, bw, base - top + 40, BG);
        let mut v = Buf::<4>::new();
        let _ = uwrite!(v, "{}", s.snr);
        w::field(lcd, x, bw, base - h - 17, base - h - 2, base - h - 4, Align::Center, v.as_bytes(), &UI_12, SUB, BG);
        lcd.fill_r(x + 2, base - h, bw - 4, h, 1, match (s.used, s.beidou) {
            (false, _) => DIM,
            (true, false) => BLUE,
            (true, true) => VIOLET,
        });
        let mut p = Buf::<4>::new();
        let _ = uwrite!(p, "{}", s.prn);
        w::field(lcd, x, bw, base + 4, base + 20, base + 16, Align::Center, p.as_bytes(), &UI_12, if s.used { TEXT } else { SUB }, BG);
    }
    if app.dirty(R_NOTE, 1) {
        w::field(lcd, 8, 224, 276, 294, 289, Align::Left, b"Used: blue GPS, violet BeiDou", &UI_12, DIM, BG);
    }
}

fn src_name(s: WpSrc) -> &'static [u8] {
    match s {
        WpSrc::Marked => b"Mark",
        WpSrc::Aprs => b"APRS",
        WpSrc::Link => b"Link",
    }
}

/// C14: the waypoints with distance and bearing from the fix (GPS-11).
fn waypoints(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>, cur: Cursor) {
    let n = data.waypoints();
    let mut l = Buf::<24>::new();
    let _ = uwrite!(l, "{} {}", n, if n == 1 { "waypoint" } else { "waypoints" });
    app.list_head(lcd, l.as_bytes(), if fix.is_some() { b"from here" } else { b"no GPS fix" });
    let rows = cur.rows(n, ROWS2, &ALL, |row, i| {
        if let Some(wp) = data.waypoint(i) {
            let (mut d, mut p) = (Buf::<12>::new(), Buf::<32>::new());
            pos_text(wp.pos, &mut p);
            let arrow = fix.map(|f| {
                let (m, deg) = dist_bearing(f.pos, wp.pos);
                dist_text(m, &mut d);
                Some(deg)
            });
            let r = Row2 { tag: src_name(wp.src), title: wp.name.as_bytes(), right: d.as_bytes(), arrow, sub: p.as_bytes(), selected: i == cur.sel, ..Row2::default() };
            app.row2(lcd, row, &r);
        }
    });
    app.rows_end(lcd, rows, ROWS2, ROW2_H);
    if n == 0 {
        app.empty(lcd, b"No waypoints", b"# marks one here");
    }
}

/// C15: one waypoint, its distance big and the compass (GPS-11).
fn go(app: &mut App, lcd: &mut impl Panel, data: &dyn Data, fix: Option<Fix>, i: u16) {
    let Some(wp) = data.waypoint(i) else {
        return app.empty(lcd, b"No such waypoint", b"MENU 0 6 7 lists them");
    };
    let where_ = fix.map(|f| dist_bearing(f.pos, wp.pos));
    if app.dirty(R_TOP, hash(&(wp, where_))) {
        lcd.fill(0, 20, 240, 60, BG);
        lcd.text4(8, 42, wp.name.as_bytes(), &UI_12B, TEXT, BG);
        let mut b = Buf::<32>::new();
        pos_text(wp.pos, &mut b);
        lcd.text4(8, 60, b.as_bytes(), &UI_12, SUB, BG);
    }
    if app.dirty(R_NOTE, hash(&where_)) {
        compass(lcd, 120, 150, 58, where_.map(|w| w.1));
    }
    if app.dirty(R_HELP, hash(&where_)) {
        lcd.fill(0, 214, 240, 84, BG);
        match where_ {
            Some((m, deg)) => {
                let mut d = Buf::<12>::new();
                dist_text(m, &mut d);
                let (num, unit) = d.as_bytes().split_at(d.as_bytes().iter().position(|&c| c == b' ').unwrap_or(d.len()));
                let wn = text4_width(num, &DIG_46) + 4 + text4_width(unit.trim_ascii_start(), &UI_12B);
                let x = 120 - wn / 2;
                let x2 = x + lcd.text4(x, 262, num, &DIG_46, TEXT, BG);
                lcd.text4(x2 + 4, 262, unit.trim_ascii_start(), &UI_12B, TEXT, BG);
                let mut b = Buf::<16>::new();
                let _ = uwrite!(b, "{} {}\u{b0}", compass_name(deg), deg);
                w::field(lcd, 8, 224, 270, 290, 285, Align::Center, b.as_bytes(), &UI_12B, TEXT, BG);
            }
            None => {
                w::field(lcd, 8, 224, 240, 260, 255, Align::Center, b"No GPS fix: no distance", &UI_12, SUB, BG);
            }
        }
    }
}
