//! APRS heard (hold 4, MENU 0 5 1; manual chapter 25, APRS-3 to APRS-5; AprsHeard mock-up) and one station
//! (OK on a station; AprsStation mock-up): C3, C4. The heard list lives in the APRS task's RAM (APRS-4) and
//! is read a row at a time. Distances and bearings are from the GPS fix; a station without a position shows
//! "no position" and a dash (APRS-5).
//!
//! On the station: OK writes it a message, # saves it as a waypoint, ▲▼ step through the list.

use super::lists::*;
use super::nav::Dest;
use super::opts::Of;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Data, Draft, Op, Station};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Aprs {
    pub cur: Cursor,
    /// The selected station's page (C4).
    pub station: bool,
}

const ALL: fn(u16) -> bool = |_| true;

impl View for Aprs {
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
            (Key::Menu, Long) => app.open_screen(Of::Heard.screen()),
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
                Some(s) => station(app, lcd, &s, data),
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
        mhz_text(data.aprs_freq(), &mut f);
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

/// C4: one station.
fn station(app: &mut App, lcd: &mut impl Panel, s: &Station, data: &dyn Data) {
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
    let mut rows: [(&str, Buf<40>); 7] = Default::default();
    let mut b = Buf::<40>::new();
    match s.pos {
        Some(p) => {
            let mut pb = Buf::<32>::new();
            pos_text(p, &mut pb);
            rows[0] = ("Position", Buf::from(pb.as_bytes()));
        }
        None => rows[0] = ("Position", Buf::from(b"none")),
    }
    rows[1] = ("Course", Buf::from(match s.course {
        Some((c, v)) => ferrite_ui::v!(b, "{}\u{b0} \u{b7} {} km/h", c, v),
        None => b"-",
    }));
    rows[2] = ("Altitude", Buf::from(match s.alt_m {
        Some(a) => ferrite_ui::v!(b, "{} m", a),
        None => b"-",
    }));
    rows[3] = ("Path", Buf::new());
    if s.via.is_empty() {
        rows[3].1.set(b"direct");
    } else {
        for (k, hop) in s.via.as_bytes().split(|&c| c == b',').enumerate() {
            if k > 0 {
                rows[3].1.push(b", ");
            }
            rows[3].1.push(hop);
        }
    }
    let mut age = Buf::<12>::new();
    age_text(s.ago_s, &mut age);
    rows[4] = ("Heard", Buf::from(ferrite_ui::v!(b, "{} ago \u{b7} {} {}", Txt(age.as_bytes()), s.times, if s.times == 1 { "time" } else { "times" })));
    rows[5] = ("Comment", Buf::from(if s.comment.is_empty() { b"-".as_slice() } else { s.comment.as_bytes() }));
    rows[6] = ("Message", Buf::new());
    if s.msg.is_empty() {
        rows[6].1.set(b"-");
    } else {
        let mut clock = Buf::<12>::new();
        if let Some(m) = s.msg_min {
            clock_text(Some(m), 0, &mut clock);
        }
        let _ = uwrite!(rows[6].1, "\"{}\" {}", Txt(s.msg.as_bytes()), Txt(clock.as_bytes()));
    }
    let mut y = 122;
    for (i, (label, v)) in rows.iter().enumerate() {
        y = app.detail(lcd, i, y, label.as_bytes(), v.as_bytes(), TEXT);
    }
    if app.dirty(R_HELP, 1) {
        lcd.fill(0, 122, 240, 1, LINE);
    }
}
