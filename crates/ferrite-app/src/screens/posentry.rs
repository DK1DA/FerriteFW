//! Position entry (A8; manual chapter 28, GPS-3): the fixed position used when there is no fix, typed in the
//! coordinate format of 0 6 3. Three rows: latitude, longitude, altitude. Digits fill the selected row's
//! template from the left; * flips N/S, E/W or the altitude's sign; ▲▼ keep the row and move; OK keeps it and
//! leaves; # clears the fixed position (none); BACK deletes a digit, or leaves when nothing is typed. A typed row
//! is checked (degrees, minutes and seconds in range) before it is kept; each kept row is saved at once.
//! The fourth row is the grid square: OK on it types one (4 or 6 characters, multi-tap capitals and digits as a
//! callsign is typed), and the position becomes the square's centre (GPS-21, as nicFW takes a grid). The fifth,
//! From GPS: OK keeps the receiver's current fix as the fixed position (GPS-35), refused without one.

use super::lists::grid;
use super::shared::ARROWS_UD;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::COORD_FORMATS;
use crate::data::Fix;
use crate::gpsdata::FIXED_AGE;
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PosEntry {
    /// 0 latitude, 1 longitude, 2 altitude, 3 grid square, 4 from GPS.
    pub row: u8,
    /// The digits typed on that row, two to a byte, and how many (0: the row shows its saved value).
    buf: [u8; 5],
    n: u8,
}

const ROWS: [&str; 3] = ["Latitude", "Longitude", "Altitude"];
/// Templates per format (Degrees, Deg-min, DMS; Grid types as Deg-min): # is a digit.
const LAT: [&[u8]; 3] = [b"##.#####", b"## ##.###", b"## ## ##.#"];
const LON: [&[u8]; 3] = [b"###.#####", b"### ##.###", b"### ## ##.#"];
/// Altitude: up to 5 digits of metres.
const ALT_DIGITS: u8 = 5;

impl PosEntry {
    pub fn new() -> Self {
        PosEntry { row: 0, buf: [0; 5], n: 0 }
    }

    fn digit(self, i: u8) -> u8 {
        self.buf[i as usize / 2] >> (4 * (i % 2)) & 15
    }

    fn typed(mut self, d: u8) -> Self {
        let i = self.n as usize;
        self.buf[i / 2] = self.buf[i / 2] & !(15 << (4 * (i % 2))) | d << (4 * (i % 2));
        self.n += 1;
        self
    }

    /// The typed digits as one number.
    fn value(self) -> u32 {
        (0..self.n).fold(0, |v, i| v * 10 + self.digit(i) as u32)
    }
}

/// 0 6 3 as a template index (Grid square types as degrees and minutes).
fn fmt(app: &App) -> usize {
    match app.set.coord_fmt {
        0 => 0,
        2 => 2,
        _ => 1,
    }
}

fn template(f: usize, row: u8) -> &'static [u8] {
    if row == 0 { LAT[f] } else { LON[f] }
}

fn digits(t: &[u8]) -> u8 {
    t.iter().filter(|&&c| c == b'#').count() as u8
}

/// A template's digits (left to right, all of them) as an angle in degrees x 1e7, if each part is in range.
fn parse(f: usize, v: u32, max_deg: u32) -> Option<u32> {
    let p10 = |k: u32| 10u32.pow(k);
    let (deg, frac) = match f {
        // DD.DDDDD: 5 decimals
        0 => (v / p10(5), (v % p10(5)) as u64 * 100),
        // DD MM.MMM: minutes in thousandths
        1 => {
            let mm = v % p10(5);
            if mm >= 60_000 {
                return None;
            }
            (v / p10(5), mm as u64 * 10_000_000 / 60_000)
        }
        // DD MM SS.S: seconds in tenths
        _ => {
            let (mm, ss) = (v % p10(5) / 1000, v % 1000);
            if mm >= 60 || ss >= 600 {
                return None;
            }
            (v / p10(5), (mm as u64 * 600 + ss as u64) * 10_000_000 / 36_000)
        }
    };
    let a = deg as u64 * 10_000_000 + frac;
    (a <= max_deg as u64 * 10_000_000).then_some(a as u32)
}

/// An angle (degrees x 1e7) as the template's digits.
fn unparse(f: usize, a: u32) -> u32 {
    let (deg, rest) = (a / 10_000_000, a % 10_000_000);
    match f {
        0 => deg * 100_000 + rest / 100,
        1 => deg * 100_000 + (rest as u64 * 60_000 / 10_000_000) as u32,
        _ => {
            let t = (rest as u64 * 36_000 / 10_000_000) as u32; // tenths of a second
            deg * 100_000 + t / 600 * 1000 + t % 600
        }
    }
}

/// The template filled with `n` of the digits of `v` (which has `total` digits), the rest as '_'.
fn fill(t: &[u8], v: u32, n: u8, total: u8, b: &mut Buf<24>) {
    let mut k = 0u8;
    for &c in t {
        if c == b'#' {
            let d = if k < n { b'0' + (v / 10u32.pow((total - 1 - k) as u32) % 10) as u8 } else { b'_' };
            b.push(&[d]);
            k += 1;
        } else {
            b.push(&[c]);
        }
    }
}

/// The row as shown: what's typed so far, else the saved value, else a dash.
fn row_text(app: &App, s: PosEntry, row: u8, b: &mut Buf<24>) {
    b.clear();
    let p = app.set.fixed;
    let typing = s.row == row && s.n > 0;
    if row == 2 {
        if typing {
            let _ = uwrite!(b, "{} m", s.value());
        } else if app.set.fixed_on {
            let _ = uwrite!(b, "{} m", app.set.fixed_alt);
        } else {
            b.set(b"-");
        }
        return;
    }
    let f = fmt(app);
    let t = template(f, row);
    let total = digits(t);
    let v = if row == 0 { p.lat } else { p.lon };
    if typing {
        // the typed digits shift in from the left: pad the number on the right
        fill(t, s.value() * 10u32.pow((total - s.n) as u32), s.n, total, b);
    } else if app.set.fixed_on {
        fill(t, unparse(f, v.unsigned_abs()), total, total, b);
    } else {
        b.set(b"-");
        return;
    }
    let (pos, neg) = if row == 0 { (" N", " S") } else { (" E", " W") };
    b.push(if v < 0 { neg } else { pos }.as_bytes());
}

/// The receiver's own fix now, not the fixed position standing in for one (GPS-35).
fn gps_fix(store: &impl ChannelStore) -> Option<Fix> {
    store.data().fix().filter(|f| f.age_s != FIXED_AGE)
}

/// Keeps the typed row, if it is whole and in range; false (with a toast) if not.
fn keep(app: &mut App, s: PosEntry) -> bool {
    if s.n == 0 {
        return true;
    }
    if s.row == 2 {
        let neg = app.set.fixed_alt < 0;
        let v = s.value().min(i16::MAX as u32) as i16;
        app.set.fixed_alt = if neg { -v } else { v };
        app.set.fixed_on = true;
        return true;
    }
    let f = fmt(app);
    let t = template(f, s.row);
    if s.n < digits(t) {
        app.show_toast(ROWS[s.row as usize].as_bytes(), b"type every digit", false);
        return false;
    }
    let max = if s.row == 0 { 90 } else { 180 };
    match parse(f, s.value(), max) {
        Some(a) => {
            let p = &mut app.set.fixed;
            let v = if s.row == 0 { &mut p.lat } else { &mut p.lon };
            *v = if *v < 0 { -(a as i32) } else { a as i32 };
            app.set.fixed_on = true;
            true
        }
        None => {
            app.show_toast(ROWS[s.row as usize].as_bytes(), b"out of range", false);
            false
        }
    }
}

impl View for PosEntry {
    fn name(self) -> &'static str {
        "Position entry"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Position");
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        if s.row == 4 {
            // GPS-35: the current fix becomes the fixed position
            match (e.key, e.press) {
                (Key::Menu, Short) => match gps_fix(store) {
                    Some(f) => {
                        let set = &mut app.set;
                        (set.fixed, set.fixed_alt, set.fixed_on) = (f.pos, f.alt_m.clamp(i16::MIN as i32, i16::MAX as i32) as i16, true);
                        app.show_toast(b"Fixed position", &grid(f.pos), false);
                    }
                    None => app.show_toast(b"No GPS fix", b"wait for a lock", false),
                },
                (Key::Up | Key::Down, Short | Repeat) => {
                    s.row = if e.key == Key::Up { 3 } else { 0 };
                    app.screen = Screen::PosEntry(s);
                }
                (Key::Exit, Short) => app.back(),
                _ => {}
            }
            return;
        }
        if s.row == 3 {
            // GPS-21: the grid row types a square in text entry
            match (e.key, e.press) {
                (Key::Menu, Short) => return super::text::open(app, super::text::TextFor::GridFixed, store),
                (Key::Up | Key::Down, Short | Repeat) => {
                    s.row = if e.key == Key::Up { 2 } else { 4 };
                    app.screen = Screen::PosEntry(s);
                }
                (Key::Hash, Short) => {
                    app.set.fixed_on = false;
                    app.show_toast(b"Fixed position", b"none", false);
                }
                (Key::Exit, Short) => app.back(),
                _ => {}
            }
            return;
        }
        let max = if s.row == 2 { ALT_DIGITS } else { digits(template(fmt(app), s.row)) };
        match (e.key, e.press) {
            (Key::Digit(d), Short) if s.n < max => s = s.typed(d),
            (Key::Star, Short) => {
                // N/S, E/W, or the altitude's sign
                let set = &mut app.set;
                match s.row {
                    0 => set.fixed.lat = -set.fixed.lat,
                    1 => set.fixed.lon = -set.fixed.lon,
                    _ => set.fixed_alt = -set.fixed_alt,
                }
            }
            (Key::Up | Key::Down, Short | Repeat) => {
                if !keep(app, s) {
                    return;
                }
                let d = if e.key == Key::Up { 4 } else { 1 };
                s = PosEntry { row: (s.row + d) % 5, buf: [0; 5], n: 0 };
            }
            (Key::Menu, Short) => {
                if keep(app, s) {
                    app.back();
                    if app.set.fixed_on {
                        app.show_toast(b"Fixed position", &grid(app.set.fixed), false);
                    }
                }
                return;
            }
            (Key::Hash, Short) => {
                app.set.fixed_on = false;
                s.n = 0;
                app.show_toast(b"Fixed position", b"none", false);
            }
            (Key::Exit, Short) if s.n > 0 => s.n -= 1,
            (Key::Exit, Short) => return app.back(),
            _ => return,
        }
        app.screen = Screen::PosEntry(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.page(lcd, 0u8);
        app.list_head(lcd, b"Fixed position", COORD_FORMATS[app.set.coord_fmt as usize % COORD_FORMATS.len()].as_bytes());
        for (i, label) in ROWS.iter().enumerate() {
            let mut v = Buf::<24>::new();
            row_text(app, self, i as u8, &mut v);
            app.row1(lcd, i, label.as_bytes(), v.as_bytes(), false, i as u8 == self.row, false);
        }
        let mut g = Buf::<24>::new();
        if app.set.fixed_on {
            g.set(&grid(app.set.fixed));
        } else {
            g.set(b"-");
        }
        app.row1(lcd, 3, b"Grid square", g.as_bytes(), false, self.row == 3, false);
        let mut f = Buf::<24>::new();
        match gps_fix(cx.store) {
            Some(x) => {
                let _ = uwrite!(f, "{} fix", if x.three_d { "3D" } else { "2D" });
            }
            None => f.set(b"No fix"),
        }
        app.row1(lcd, 4, b"From GPS", f.as_bytes(), false, self.row == 4, false);
        let note: &[u8] = if self.row == 4 {
            b"OK keeps the current GPS fix as the fixed position."
        } else if self.row == 3 {
            b"OK types a grid square: its centre is used. # clears it."
        } else {
            b"Used when there is no GPS fix (APRS, Nearby). # clears it."
        };
        app.help_box(lcd, 300 - 52, 48, note, b"MENU 0 6 9");
    }

    fn hints(self, _: &App) -> Hints {
        if self.row == 3 {
            return Hints::new(&[(ARROWS_UD, "Row"), ("OK", "Type it"), ("#", "Clear")]);
        }
        if self.row == 4 {
            return Hints::new(&[(ARROWS_UD, "Row"), ("OK", "Use fix")]);
        }
        let star = match self.row {
            0 => "N/S",
            1 => "E/W",
            _ => "+/-",
        };
        Hints::new(&[("0-9", "Type"), ("*", star), ("OK", "Save")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip() {
        // 45 25.290 N (deg-min) = 45.4215 degrees
        let a = parse(1, 4_525_290, 90).unwrap();
        assert_eq!(a, 454_215_000);
        assert_eq!(unparse(1, a), 4_525_290);
        assert_eq!(parse(0, 4_542_150, 90), Some(454_215_000));
        // DMS: 45 25 17.4
        let d = parse(2, 4_525_174, 90).unwrap();
        assert_eq!(unparse(2, d), 4_525_174);
        assert!(parse(1, 4_560_000, 90).is_none(), "60 minutes");
        assert!(parse(0, 9_100_000, 90).is_none(), "past 90");
    }
}
