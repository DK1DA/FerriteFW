//! APRS heard (MENU 0 5 1; manual chapter 25, APRS-3 to APRS-5; AprsHeard mock-up) and one station
//! (OK on a station; AprsStation mock-up): C3, C4. The heard list lives in the APRS task's RAM (APRS-4) and
//! is read a row at a time. Distances and bearings are from the GPS fix; a station without a position shows
//! "no position" and a dash (APRS-5).
//!
//! On the station: OK writes it a message, # saves it as a waypoint, ▲▼ step through the list.

use super::lists::*;
use super::nav::Dest;
use super::opts::Of;
use super::shared::*;
use super::table::{slow_table, Table, Val};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Data, Draft, Op, Station};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Aprs {
    pub cur: Cursor,
    /// The selected station's page (C4).
    pub station: bool,
}

const ALL: fn(u16) -> bool = |_| true;

impl View for Aprs {
    fn options(self, app: &mut App) -> bool {
        app.open_screen(Of::Heard.screen());
        true
    }

    fn name(self) -> &'static str {
        if self.station { "APRS station" } else { "APRS heard" }
    }

    fn mockup(self) -> Option<&'static str> {
        Some(if self.station { "AprsStation.png" } else { "AprsHeard.png" })
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.station { b"APRS" } else { b"Heard" });
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        let st = d.station(self.cur.sel);
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) if !self.station && st.is_some() => app.screen = Screen::Aprs(Aprs { station: true, ..self }),
            // OK on the station: a message to it (A16).
            (Key::Menu, Short) => {
                if let Some(s) = st {
                    let mut to = Buf::<9>::new();
                    to.set(s.call.as_bytes());
                    d.request(Op::Draft(Draft { to, ..d.draft() }));
                    app.open(Dest::Compose);
                }
            }
            (Key::Hash, Short) if self.station => match st {
                Some(s) if s.pos.is_some() && d.request(Op::SaveStation(self.cur.sel)) => app.show_toast(s.call.as_bytes(), b"saved as a waypoint", false),
                Some(s) if s.pos.is_none() => app.show_toast(s.call.as_bytes(), b"has no position", false),
                Some(s) => app.show_toast(s.call.as_bytes(), b"not saved", false),
                None => {}
            },
            (Key::Exit, Short) if self.station => app.screen = Screen::Aprs(Aprs { station: false, ..self }),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let cur = self.cur.step(d, store.data().stations(), ROWS2, &ALL);
        app.screen = Screen::Aprs(Aprs { cur, ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, self.station);
        if !app.gate((self, data.version(), cx.dev.clock_min)) {
            return;
        }
        if self.station {
            match data.station(self.cur.sel) {
                Some(s) => station(app, lcd, &s, data, cx.live, cx.dev),
                None => app.empty(lcd, b"No such station", b"The heard list changed"),
            }
            return;
        }
        let n = data.stations();
        let fix = data.fix();
        let mut l = Buf::<16>::new();
        let _ = uwrite!(l, "{} heard", n);
        let mut r = Buf::<32>::new();
        let mut f = Buf::<16>::new();
        mhz_text(app.aprs_freq(), &mut f);
        let _ = uwrite!(r, "newest first \u{b7} {}", Txt(f.as_bytes()));
        app.list_head(lcd, l.as_bytes(), r.as_bytes());
        let rows = self.cur.rows(n, ROWS2, &ALL, |row, i| {
            if let Some(s) = data.station(i) {
                let (mut dist, mut sub, mut age) = (Buf::<12>::new(), Buf::<40>::new(), Buf::<12>::new());
                age_text(s.ago_s, &mut age);
                let _ = uwrite!(sub, "{} \u{b7} ", Txt(age.as_bytes()));
                let arrow = match (s.pos, fix) {
                    (Some(p), Some(f)) => {
                        let (m, deg) = dist_bearing(f.pos, p);
                        dist_text(m, &mut dist);
                        Some(Some(deg))
                    }
                    (Some(_), None) => None,
                    (None, _) => Some(None), // APRS-5
                };
                if s.pos.is_none() {
                    sub.push(b"no position");
                } else if s.via.is_empty() {
                    sub.push(b"direct");
                } else {
                    // the first digipeater, without its used mark
                    let hop = s.via.as_bytes().split(|&c| c == b',').next().unwrap_or(b"");
                    sub.push(b"via ");
                    sub.push(hop.strip_suffix(b"*").unwrap_or(hop));
                }
                let r = Row2 {
                    tag: s.sym.name().as_bytes(),
                    title: s.call.as_bytes(),
                    big: true,
                    right: dist.as_bytes(),
                    arrow,
                    sub: sub.as_bytes(),
                    selected: i == self.cur.sel,
                    ..Row2::default()
                };
                app.row2(lcd, row, &r);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        if n == 0 {
            app.empty(lcd, b"No stations heard yet", b"APRS listens on the APRS frequency");
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.station {
            Hints::new(&[("OK", "Message"), ("#", "Waypoint"), ("BACK", "Back")])
        } else {
            Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("Hold MENU", "More")])
        }
    }
}

fn text(b: &mut Buf<40>, s: &[u8]) -> u16 {
    b.set(s);
    TEXT
}

slow_table! {
    /// C4's rows.
    ROWS: [(&str, Val<Station>); 8] = [
        (slow!("Position"), |c, b| match c.x.pos {
            Some(p) => {
                let mut pb = Buf::<32>::new();
                pos_text(p, &mut pb);
                text(b, pb.as_bytes())
            }
            None => text(b, b"none"),
        }),
        (slow!("Course"), |c, b| match (c.x.course, crate::aprs::wx(c.x.comment.as_bytes())) {
            // APRS-23: a weather station's course and speed are its wind
            (_, Some(w)) => {
                b.push(b"wind ");
                match w.dir {
                    Some(d) => {
                        let _ = uwrite!(b, "{}\u{b0}", d);
                    }
                    None => b.push(b"-"),
                }
                if let Some(v) = w.wind {
                    b.push(" \u{b7} ".as_bytes());
                    speed(c.app.set.units, v, b);
                }
                if let Some(g) = w.gust {
                    b.push(b", gust ");
                    speed(c.app.set.units, g, b);
                }
                TEXT
            }
            (Some((d, v)), None) => {
                let _ = uwrite!(b, "{}\u{b0} \u{b7} {} km/h", d, v);
                TEXT
            }
            (None, None) => text(b, b"-"),
        }),
        (slow!("Weather"), |c, b| match crate::aprs::wx(c.x.comment.as_bytes()) {
            Some(w) => {
                weather(c.app.set.units, &w, b);
                TEXT
            }
            None => text(b, b"-"),
        }),
        (slow!("Altitude"), |c, b| match c.x.alt_m {
            Some(a) => {
                let _ = uwrite!(b, "{} m", a);
                TEXT
            }
            None => text(b, b"-"),
        }),
        (slow!("Path"), |c, b| {
            let s = c.x;
            if s.via.is_empty() {
                return text(b, b"direct");
            }
            for (k, hop) in s.via.as_bytes().split(|&ch| ch == b',').enumerate() {
                if k > 0 {
                    b.push(b", ");
                }
                b.push(hop);
            }
            TEXT
        }),
        (slow!("Heard"), |c, b| {
            let s = c.x;
            let mut age = Buf::<12>::new();
            age_text(s.ago_s, &mut age);
            let _ = uwrite!(b, "{} ago \u{b7} {} {}", Txt(age.as_bytes()), s.times, if s.times == 1 { "time" } else { "times" });
            TEXT
        }),
        (slow!("Comment"), |c, b| {
            // a weather station's comment is what follows its weather fields
            let t = c.x.comment.as_bytes();
            let t = crate::aprs::wx(t).map_or(t, |w| t[w.rest..].trim_ascii());
            text(b, if t.is_empty() { b"-".as_slice() } else { t })
        }),
        (slow!("Message"), |c, b| {
            let s = c.x;
            if s.msg.is_empty() {
                return text(b, b"-");
            }
            let mut clock = Buf::<12>::new();
            if let Some(m) = s.msg_min {
                clock_text(Some(m), 0, &mut clock);
            }
            let _ = uwrite!(b, "\"{}\" {}", Txt(s.msg.as_bytes()), Txt(clock.as_bytes()));
            TEXT
        }),
    ];
}

/// A speed sent in mph, in the units of 0 6 4 (metric km/h, imperial mph, nautical knots).
fn speed(units: u8, mph: u16, b: &mut Buf<40>) {
    let mph = mph as u32;
    let _ = match units {
        1 => uwrite!(b, "{} mph", mph),
        2 => uwrite!(b, "{} kn", (mph * 869 + 500) / 1000),
        _ => uwrite!(b, "{} km/h", (mph * 1609 + 500) / 1000),
    };
}

/// APRS-23: temperature, humidity, pressure and the last hour's rain, as they are sent, in 0 6 4's units.
fn weather(units: u8, w: &crate::aprs::Wx, b: &mut Buf<40>) {
    let sep = |b: &mut Buf<40>| {
        if !b.is_empty() {
            b.push(" \u{b7} ".as_bytes());
        }
    };
    if let Some(f) = w.temp_f {
        sep(b);
        let _ = if units == 1 { uwrite!(b, "{}\u{b0}F", f) } else { uwrite!(b, "{}\u{b0}C", ((f as i32 - 32) * 5 - if f < 32 { 4 } else { -4 }) / 9) };
    }
    if let Some(h) = w.hum {
        sep(b);
        let _ = uwrite!(b, "{}%", h);
    }
    if let Some(p) = w.baro {
        sep(b);
        let _ = uwrite!(b, "{} hPa", (p + 5) / 10);
    }
    if let Some(r) = w.rain.filter(|&r| r > 0) {
        sep(b);
        let _ = if units == 1 { uwrite!(b, "rain {}.{}\"", r / 100, ferrite_ui::text::Zero(r as u32 % 100, 2)) } else { uwrite!(b, "rain {} mm", (r as u32 * 254 + 500) / 1000) };
    }
    if b.is_empty() {
        b.set(b"-");
    }
}

slow_table! {
    STATION: Table<Station> = Table { rows: &ROWS, go: &[], y: 122, h: 0, style: w::KV_DETAIL };
}

/// C4: one station.
fn station(app: &mut App, lcd: &mut impl Panel, s: &Station, data: &dyn Data, live: &LiveAll, dev: &Device) {
    let fix = data.fix();
    let where_ = match (s.pos, fix) {
        (Some(p), Some(f)) => Some(dist_bearing(f.pos, p)),
        _ => None,
    };
    let moving = s.course.is_some_and(|c| c.1 > 0);
    if app.dirty(R_TOP, hash(&(s.call, s.sym, moving, where_))) {
        lcd.fill(0, 20, 156, 102, BG);
        lcd.text4(8, 42, s.call.as_bytes(), &UI_15B, TEXT, BG);
        let mut t = Buf::<24>::new();
        t.push(s.sym.name().as_bytes());
        t.push(" \u{b7} ".as_bytes());
        t.push(if moving { b"moving".as_slice() } else if s.course.is_some() { b"stopped" } else { b"fixed" });
        lcd.text4(8, 61, t.as_bytes(), &UI_12, SUB, BG);
        match where_ {
            Some((m, deg)) => {
                // the distance big: "4.2" in the digit font, the unit beside it
                let mut d = Buf::<12>::new();
                dist_text(m, &mut d);
                let (num, unit) = d.as_bytes().split_at(d.as_bytes().iter().position(|&c| c == b' ').unwrap_or(d.len()));
                let x = 8 + lcd.text4(8, 98, num, &DIG_28, TEXT, BG);
                lcd.text4(x + 4, 98, unit.trim_ascii_start(), &UI_12B, TEXT, BG);
                let mut b = Buf::<12>::new();
                let _ = uwrite!(b, "{} {}\u{b0}", compass_name(deg), deg);
                lcd.text4(8, 117, b.as_bytes(), &UI_12B, TEXT, BG);
            }
            None => {
                lcd.text4(8, 98, b"-", &DIG_28, SUB, BG);
                lcd.text4(8, 117, if s.pos.is_none() { b"No position".as_slice() } else { b"No GPS fix here" }, &UI_12, SUB, BG);
            }
        }
    }
    if app.dirty(R_NOTE, hash(&where_.map(|w| w.1))) {
        compass(lcd, 194, 71, 33, where_.map(|w| w.1));
    }
    app.table(lcd, &STATION, live, dev, s, None);
    if app.dirty(R_HELP, 1) {
        lcd.fill(0, 122, 240, 1, LINE);
    }
}
