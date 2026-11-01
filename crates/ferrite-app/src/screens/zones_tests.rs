//! The zones page (CHAN-27 to CHAN-30) on the sample records: the distance and direction texts, when the Nearby row
//! shows, the chips, the preview's contents and columns, the keys, and what a move redraws.

use super::*;
use crate::chan::{ChanWrite, Channel, NO_ZONE};
use crate::data::{Data, Fix};
use crate::sample::{Sample, HERE};
use crate::screens::nav::Dest;
use crate::slot::DEFAULT_CFG;
use ferrite_core::msg::{Device, GpsState, LiveAll};
use ferrite_ui::fb::FrameBuffer;
use std::string::{String, ToString};
use std::vec::Vec;

/// A panel that keeps the text drawn on it with its box (x0, y0, x1, y1 inclusive), the frame buffer under it.
struct Rec {
    fb: FrameBuffer,
    texts: Vec<(i32, i32, i32, i32, String)>,
}

impl Panel for Rec {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.fb.window(x0, y0, x1, y1);
    }
    fn push(&mut self, color: u16, count: u32) {
        self.fb.push(color, count);
    }
    fn push_buf(&mut self, px: &[u16]) {
        self.fb.push_buf(px);
    }
    fn text_hint(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, s: &[u8]) {
        self.texts.push((x0, y0, x1, y1, String::from_utf8_lossy(s).into_owned()));
    }
}

struct T {
    app: App,
    t: u32,
    s: Sample,
    lcd: Rec,
}

impl T {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        T { app, t: 1000, s: Sample::new(), lcd: Rec { fb: FrameBuffer::new(), texts: Vec::new() } }
    }

    fn tap(&mut self, k: Key) {
        let live = LiveAll::default();
        self.app.key_down(k, self.t, &self.s);
        self.t += 100;
        self.app.tick(self.t, &live, &self.s);
        self.app.key_up(k, self.t, &self.s);
        self.t += 50;
        self.app.tick(self.t, &live, &self.s);
        while let Some(w) = self.app.pop_write() {
            self.s.apply(w);
        }
        self.draw();
    }

    fn open(&mut self) {
        self.app.open(Dest::Zones);
        self.draw();
    }

    fn draw(&mut self) {
        let dev = Device { gps: GpsState::Fix, sats: 9, clock_min: Some(14 * 60 + 32), ..Default::default() };
        self.lcd.texts.clear();
        self.app.render(&mut self.lcd, &LiveAll::default(), &dev, &self.s);
    }

    /// The readback's lines (LINK-19).
    fn ui(&self) -> Vec<String> {
        let dev = Device::default();
        let mut out = Vec::new();
        let mut l = crate::term::cmd::Line::new();
        let mut i = 0;
        while crate::uistate::line(&self.app, &self.s, &dev, &LiveAll::default(), i, &mut l) {
            out.push(String::from_utf8_lossy(l.as_bytes()).into_owned());
            i += 1;
        }
        out
    }

    fn zones(&self) -> Zones {
        match self.app.screen() {
            Screen::Zones(z) => z,
            s => panic!("not on the zones page: {}", s.name()),
        }
    }

    /// A channel added to the sample, with a position or none.
    fn put(&mut self, num: u16, zone: crate::chan::ZoneId, rx: Freq10, name: &str, pos: Option<Pos>) {
        let mut c = Channel { num, zone, rx, name: [0; 16], cfg: DEFAULT_CFG, flags: 0, tones: 0 };
        c.set_name(name.as_bytes());
        self.s.apply(ChanWrite::PutPos { ch: c, pos });
    }
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn dist(m: u32, units: u8) -> String {
    let mut b = Buf::<12>::new();
    dist_short(m, units, &mut b);
    s(b.as_bytes())
}

/// A position `km` from HERE toward `deg` (flat earth, as dist_bearing measures).
fn toward(km: f64, deg: f64) -> Pos {
    let (n, e) = (km * deg.to_radians().cos(), km * deg.to_radians().sin());
    let lat = HERE.lat as f64 / 1e7 + n / 111.195;
    let lon = HERE.lon as f64 / 1e7 + e / (111.195 * (lat.to_radians()).cos());
    Pos { lat: (lat * 1e7) as i32, lon: (lon * 1e7) as i32 }
}

/// CHAN-28: at most three digits, never wrapping: 2.3km, 23.4km, 123km, far; miles and nautical miles the same way.
#[test]
fn distance_texts_every_range() {
    for (m, want) in [
        (0, "0.0km"),
        (449, "0.4km"),
        (2_300, "2.3km"),
        (9_949, "9.9km"),
        (9_950, "10.0km"),
        (23_400, "23.4km"),
        (99_949, "99.9km"),
        (99_950, "100km"),
        (123_400, "123km"),
        (999_499, "999km"),
        (999_500, "far"),
        (12_000_000, "far"),
    ] {
        assert_eq!(dist(m, 0), want, "{m} m");
    }
    // Imperial (0 6 4): 1 mi = 1609.344 m
    for (m, want) in [(3_701, "2.3mi"), (37_659, "23.4mi"), (160_934, "100mi"), (1_607_734, "999mi"), (1_608_540, "far")] {
        assert_eq!(dist(m, 1), want, "{m} m");
    }
    // Nautical: 1 nm = 1852 m
    assert_eq!(dist(4_260, 2), "2.3nm");
    assert_eq!(dist(2_000_000, 2), "far");
    // Never more than three digits, and never wider than the column (CHAN-27)
    for m in (0..3_000_000).step_by(997) {
        for u in 0..3 {
            let t = dist(m, u);
            assert!(t.chars().filter(char::is_ascii_digit).count() <= 3, "{t}");
            let mut p = Buf::<12>::new();
            place_text(m, 315, u, &mut p);
            assert!(text4_width(p.as_bytes(), &UI_12) <= DIST_W, "{}", s(p.as_bytes()));
        }
    }
    let mut p = Buf::<12>::new();
    place_text(2_300, 45, 0, &mut p);
    assert_eq!(p.as_bytes(), b"NE 2.3km");
    place_text(123_000, 0, 0, &mut p);
    assert_eq!(p.as_bytes(), b"N 123km");
    place_text(5_000_000, 270, 1, &mut p);
    assert_eq!(p.as_bytes(), b"W far");
}

/// CHAN-28: the direction is one of eight compass points, absolute bearing from the fix, N wrapping across 0.
#[test]
fn bearing_letters_eight_sectors() {
    let names = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    for d in 0..360u16 {
        let want = names[((d as u32 * 10 + 225) / 450 % 8) as usize];
        assert_eq!(compass_name(d), want, "{d} degrees");
    }
    for (d, want) in [(0, "N"), (22, "N"), (23, "NE"), (337, "NW"), (338, "N"), (359, "N"), (180, "S"), (202, "S"), (203, "SW")] {
        assert_eq!(compass_name(d), want, "{d}");
    }
    // From the fix to a channel, each point of the compass (the sample's position, 20 km out).
    for (i, n) in names.iter().enumerate() {
        let (m, deg) = dist_bearing(HERE, toward(20.0, i as f64 * 45.0));
        assert_eq!(compass_name(deg), *n, "{} degrees: {deg}", i * 45);
        assert!((19_800..20_200).contains(&m), "{m}");
    }
}

fn nearby_line(t: &T) -> Option<String> {
    t.ui().into_iter().find(|l| l.contains("kind=nearby"))
}

/// CHAN-29: the Nearby row shows only with a fix and a channel with a position, and gives only the nearest one's
/// direction and distance.
#[test]
fn nearby_row_only_with_a_fix_and_positions() {
    let mut t = T::new();
    t.open();
    // the sample: a fix, five channels with a position; the nearest by brute force
    let (num, m, deg) = (0..1000u16)
        .filter_map(|n| t.s.position(n).map(|p| (n, dist_bearing(HERE, p))))
        .map(|(n, (m, d))| (n, m, d))
        .min_by_key(|x| (x.1, x.0))
        .unwrap();
    let mut want = Buf::<12>::new();
    place_text(m, deg, 0, &mut want);
    let line = nearby_line(&t).expect("a Nearby row");
    assert_eq!(line, std::format!("row 0 \"Nearby\" \"{}\" kind=nearby", s(want.as_bytes())), "nearest is {num}");
    // no fix: plain list
    t.s.fix.set(None);
    t.draw();
    assert_eq!(nearby_line(&t), None, "no fix");
    assert!(!t.ui().iter().any(|l| l.starts_with("pvrow") && !l.contains("\"\" \"")), "no distances without a fix: {:?}", t.ui());
    // a fix, no positions: plain list
    t.s.fix.set(Sample::sample_fix());
    t.s.hide_positions(true);
    t.draw();
    assert_eq!(nearby_line(&t), None, "no channel has a position");
    t.s.hide_positions(false);
    t.draw();
    assert!(nearby_line(&t).is_some());
    // every positioned channel hidden (CHAN-24): none listed, so no row; "show hidden" brings it back
    for n in [9, 11, 14, 16, 17] {
        assert!(t.s.request(crate::data::Op::Hide(n, true)));
    }
    t.draw();
    assert_eq!(nearby_line(&t), None, "hidden channels aren't listed");
    t.app.set.show_hidden = true;
    t.draw();
    assert!(nearby_line(&t).is_some(), "show hidden");
    // The fixed position stands in for a fix (GPS-3): the store's fix answers for both.
    t.app.set.show_hidden = false;
    t.s.fix.set(Some(Fix { pos: Pos { lat: 455_454_000, lon: -757_283_000 }, three_d: false, alt_m: 0, speed_kmh: 0, heading: 0, hdop_x10: 0, used: 0, age_s: u16::MAX, utc_s: 0 }));
    for n in [9, 11, 14, 16, 17] {
        t.s.request(crate::data::Op::Hide(n, false));
    }
    t.draw();
    assert!(nearby_line(&t).expect("a row").contains("\"N 0.0km\""), "North hill is right here: {:?}", nearby_line(&t));
}

/// CHAN-30: the active slot's zone carries "● A" (only the active slot's), SCAN a zone in scan all, HID one hidden
/// from ◀▶; an empty zone is dimmed.
#[test]
fn chips_for_the_active_slot_scan_and_hidden() {
    let mut t = T::new();
    t.app.set_channel(0, 0, 0, &t.s); // A on Simplex
    t.app.set_channel(1, 1, 1, &t.s); // B on Repeaters
    t.open();
    let rows: Vec<String> = t.ui().into_iter().filter(|l| l.contains("kind=zone")).collect();
    let row = |name: &str| rows.iter().find(|l| l.starts_with(&std::format!("row 0 \"{name}\""))).cloned().unwrap();
    assert!(row("Simplex").contains("chips=\"A\""), "{}", row("Simplex"));
    assert!(row("Repeaters").contains("chips=\"SCAN\""), "B on it says nothing: {}", row("Repeaters"));
    assert!(row("Weather").contains("chips=\"HID\""), "{}", row("Weather"));
    assert!(row("Empty").contains(" grey"), "{}", row("Empty"));
    assert!(row("Air").contains("chips=\"\""), "{}", row("Air"));
    // B active: its zone takes the chip, A's loses it
    t.app.go_home();
    t.tap(Key::Slot);
    t.open();
    let rows: Vec<String> = t.ui().into_iter().filter(|l| l.contains("kind=zone")).collect();
    assert!(rows.iter().any(|l| l.starts_with("row 0 \"Repeaters\"") && l.contains("chips=\"B SCAN\"")), "{rows:?}");
    assert!(rows.iter().any(|l| l.starts_with("row 0 \"Simplex\"") && l.contains("chips=\"\"")), "{rows:?}");
}

/// CHAN-27: the preview: the zone's name in capitals and its state, its first four channels (no numbers, no shift
/// signs), the footer.
#[test]
fn preview_of_a_zone() {
    let mut t = T::new();
    t.open();
    let ui = t.ui();
    assert!(ui.contains(&"head \"6 zones \u{b7} 21 ch\"".to_string()), "{ui:?}");
    assert!(ui.contains(&"preview \"SIMPLEX\" \"2m \u{b7} 70cm\"".to_string()), "{ui:?}");
    let rows: Vec<&String> = ui.iter().filter(|l| l.starts_with("pvrow")).collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0], "pvrow \"Calling 2 m\" \"\" \"146.520\"", "no position: no place");
    assert!(ui.contains(&"pvfoot \"scan off\"".to_string()), "{ui:?}");
    t.tap(Key::Down); // Repeaters: in scan, five channels, priority 1 Club repeater
    let ui = t.ui();
    assert!(ui.contains(&"preview \"REPEATERS\" \"scan on\"".to_string()), "{ui:?}");
    let rows: Vec<&String> = ui.iter().filter(|l| l.starts_with("pvrow")).collect();
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|r| !r.contains("\"\" \"")), "each has a place: {rows:?}");
    assert!(rows.iter().all(|r| !r.contains('+') && !r.contains("CH ")), "no shift signs, no numbers: {rows:?}");
    assert!(rows[1].starts_with("pvrow \"Club repeater\" \"SE "), "{}", rows[1]);
    assert!(ui.contains(&"pvfoot \"+1 more \u{b7} \u{2605} Club repeater\"".to_string()), "{ui:?}");
    t.tap(Key::Down); // Empty
    assert!(t.ui().contains(&"preview \"EMPTY\" \"empty\"".to_string()), "{:?}", t.ui());
    assert!(!t.ui().iter().any(|l| l.starts_with("pvrow")));
}

/// CHAN-27 (decision 157): the footer marks the priority channel with a star, not the word, so "+N more · ★ NAME"
/// fits 240 px whole (the footer's 224 px from x 8) with a 12-character name, and with a full 16 and three-digit "more".
#[test]
fn preview_footer_fits_a_priority_name() {
    let mut t = T::new();
    t.put(11, 1, mhz(443) + 47_500, "Hilltop Rptr", None); // the Repeaters zone's priority 1, 12 characters
    let p = preview_zone(&t.app, &t.s, 1);
    assert_eq!(s(p.foot.as_bytes()), "+1 more \u{b7} \u{2605} Hilltop Rptr");
    assert!(UI_12.has('\u{2605}'), "the star is in the footer's font");
    assert!(text4_width(p.foot.as_bytes(), &UI_12) <= 224, "fits: {}", text4_width(p.foot.as_bytes(), &UI_12));
    t.open();
    t.tap(Key::Down);
    assert!(t.lcd.texts.iter().any(|x| x.4 == s(p.foot.as_bytes())), "drawn whole: {:?}", t.lcd.texts);
    // A full 16-character name, and the most "more" a zone can have (1000 channels, four previewed).
    t.put(11, 1, mhz(443) + 47_500, "WWWWWWWWWWWWWWWW", None);
    let p = preview_zone(&t.app, &t.s, 1);
    assert_eq!(s(p.foot.as_bytes()), "+1 more \u{b7} \u{2605} WWWWWWWWWWWWWWWW");
    let worst = "+996 more \u{b7} \u{2605} WWWWWWWWWWWWWWWW";
    assert!(text4_width(worst.as_bytes(), &UI_12) <= 224, "fits: {worst}");
}

/// CHAN-27: a preview row's name, place and frequency stay in their columns, at 240 px, whatever the name's length
/// and the frequency's: the name is clipped, never wrapped.
#[test]
fn preview_columns_never_overflow() {
    let mut t = T::new();
    let long = "WWWWWWWWWWWWWWWW";
    t.put(100, 2, 129_998_750, long, Some(toward(23.4, 315.0))); // NW 23.4km, 1299.9875
    t.put(101, 2, mhz(1296) + 12_500, "Mmmmmmmmmmmmmmmm", Some(toward(4000.0, 90.0))); // E far
    t.put(102, 2, 1_407_400, long, None); // an HF channel, no position
    t.put(103, 2, mhz(446) + 6_250, "x", Some(toward(150.0, 180.0)));
    t.open();
    t.tap(Key::Down);
    t.tap(Key::Down); // the third zone: "Empty", now these four
    let ui = t.ui();
    assert!(ui.contains(&"preview \"EMPTY\" \"HF +2\"".to_string()) || ui.iter().any(|l| l.starts_with("preview \"EMPTY\"")), "{ui:?}");
    let pv: Vec<_> = t.lcd.texts.iter().filter(|x| x.1 >= PV_Y + PV_HEAD_H && x.3 < PV_Y + PV_HEAD_H + PV_ROWS as i32 * PV_ROW_H).cloned().collect();
    assert!(pv.len() >= 10, "names, places and frequencies drawn: {pv:?}");
    for a in &pv {
        assert!(a.0 >= 8 && a.2 < 232, "inside the margins: {a:?}");
        for b in &pv {
            if a != b && a.1 == b.1 {
                assert!(a.2 < b.0 || b.2 < a.0, "overlap: {a:?} and {b:?}");
            }
        }
    }
    assert!(pv.iter().any(|x| x.4 == "NW 23.4km"), "{pv:?}");
    assert!(pv.iter().any(|x| x.4 == "E far"), "{pv:?}");
    assert!(pv.iter().all(|x| x.4.len() <= 16 && !x.4.contains('\n')));
    assert!(pv.iter().any(|x| x.4.starts_with("WWWW") && x.4.len() < long.len()), "clipped: {pv:?}");
    // Every combination of the widest texts, by the column rule itself.
    for name in ["", "x", long, "Mmmmmmmmmmmmmmmm"] {
        for f in [1_407_400u32, mhz(146) + 52_000, mhz(439) + 98_750, 129_998_750] {
            for place in ["", "N 0.0km", "NW 23.4km", "W far", "SE 999mi"] {
                let mut r = PvRow { name: crate::chan::Name::from(name.as_bytes()), place: Buf::from(place.as_bytes()), ..PvRow::default() };
                mhz_text(f, &mut r.freq);
                for dist_col in [false, true] {
                    let (n, dx, fx) = pv_columns(&r, dist_col);
                    let nw = text4_width(n, &UI_12);
                    assert!(8 + nw <= dx - GAP || n.is_empty(), "name into the place: {name} {dx}");
                    if dist_col {
                        assert!(dx + text4_width(r.place.as_bytes(), &UI_12) <= fx, "place into the frequency: {place} {f}");
                    }
                    assert!(fx >= 8 && fx + text4_width(r.freq.as_bytes(), &UI_12) <= 232);
                }
            }
        }
    }
}

/// CHAN-29: ▲▼ go through the Nearby row and the zones, wrapping; OK on Nearby opens the browser's Nearby view;
/// without the row, ▲ on the first zone wraps to the last. SLOT, # and \* stay the list's (C11).
#[test]
fn keys_through_the_nearby_row() {
    let mut t = T::new();
    t.open();
    assert_eq!(t.zones().cur.sel, 0, "opens on the first zone");
    t.tap(Key::Up);
    assert!(t.zones().on_nearby());
    assert!(t.ui().iter().any(|l| l.contains("kind=nearby") && l.contains(" sel")), "{:?}", t.ui());
    assert!(t.ui().iter().any(|l| l.starts_with("preview \"NEARBY\" \"nearest first\"")), "{:?}", t.ui());
    assert_eq!(t.app.screen().hints(&t.app).as_slice()[0], ("OK", "Open"));
    t.tap(Key::Star);
    assert!(matches!(t.app.screen(), Screen::Zones(_)), "\\* does nothing on Nearby");
    t.tap(Key::Down);
    assert_eq!((t.zones().on_nearby(), t.zones().cur.sel), (false, 0));
    t.tap(Key::Up);
    t.tap(Key::Up);
    assert_eq!((t.zones().on_nearby(), t.zones().cur.sel), (false, 6), "past Nearby to the last row: Local (CHAN-34)");
    t.tap(Key::Down);
    assert!(t.zones().on_nearby(), "down from the last row: Nearby");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.view == 8), "OK: the Nearby view");
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Zones(_)), "BACK returns: {}", t.app.screen().name());
    // no fix: no row, ▲ wraps
    t.s.fix.set(None);
    t.app.open(Dest::Zones);
    t.draw();
    t.tap(Key::Up);
    assert_eq!((t.zones().on_nearby(), t.zones().cur.sel), (false, 6), "Local, there without a position too");
    t.app.set.local = 0;
    t.app.open(Dest::Zones);
    t.draw();
    t.tap(Key::Up);
    assert_eq!((t.zones().on_nearby(), t.zones().cur.sel), (false, 5), "Local off: the last zone");
    t.app.set.local = crate::local::DEFAULT_RANGE;
    // the knob does the same as ▲▼
    t.s.fix.set(Sample::sample_fix());
    t.app.open(Dest::Zones);
    t.draw();
    t.app.knob(-1, t.t, &t.s);
    assert!(t.zones().on_nearby());
    t.app.knob(2, t.t, &t.s);
    assert_eq!(t.zones().cur.sel, 1);
    // SLOT on a zone: its editor (decision 178)
    t.tap(Key::Slot);
    assert_eq!(t.app.screen().name(), "Zone editor");
    assert!(t.ui().iter().any(|l| l.starts_with("row 0 \"Scan all\" \"On\"")), "{:?}", t.ui());
}

fn toast(t: &T) -> Option<String> {
    t.ui().into_iter().find(|l| l.starts_with("toast "))
}

/// CHAN-27, NAV-1 (decision 178): OK on a zone puts the active slot on its first listed channel in number order
/// (CHAN-10) in channel mode and goes home, where ◀▶ go on to the next zone; the hint bar says OK Use, SLOT Edit.
#[test]
fn ok_on_a_zone_puts_the_slot_on_it() {
    let mut t = T::new();
    t.open();
    assert_eq!(t.app.screen().hints(&t.app).as_slice(), [("OK", "Use"), ("SLOT", "Edit"), ("#", "New")]);
    assert!(!t.app.slot(0).on_channel(), "A starts on its VFO");
    t.tap(Key::Down); // Repeaters
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Home), "home: {}", t.app.screen().name());
    let s = *t.app.slot(0);
    assert!(s.on_channel());
    assert_eq!((s.zone, s.ch.map(|c| c.num)), (1, Some(9)), "the zone's first channel, North hill");
    assert_eq!(toast(&t).as_deref(), Some("toast \"Slot A\" \"Repeaters\""));
    t.tap(Key::Right); // NAV-1: the next zone with channels (Empty is skipped)
    assert_eq!(t.app.slot(0).zone, 3);
}

/// CHAN-27 (decision 178): the slot keeps the channel it last used in that zone (its zone and position, kept while it
/// is on its VFO); another zone starts at its first channel; hidden channels are passed (CHAN-24) unless shown.
#[test]
fn ok_keeps_the_slots_channel_in_that_zone() {
    let mut t = T::new();
    t.app.set_channel(0, 1, 2, &t.s); // Downtown DCS
    t.app.slots[0].vfo_mode = true;
    t.open();
    t.tap(Key::Down);
    t.tap(Key::Menu);
    let s = *t.app.slot(0);
    assert_eq!((s.on_channel(), s.zone, s.pos), (true, 1, 2), "back on Downtown DCS");
    // a channel hidden: OK on its zone passes it
    t.s.data().request(Op::Hide(0, true));
    t.open();
    t.tap(Key::Menu); // Simplex
    assert_eq!(t.app.slot(0).ch.map(|c| c.num), Some(1), "Calling 2 m is hidden: Calling 70 cm");
    t.app.set.show_hidden = true;
    t.app.slots[0].zone = 1;
    t.open();
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).ch.map(|c| c.num), Some(0), "shown with show hidden");
}

/// CHAN-27 (decision 178): OK on an empty zone says so and stays on the page; the slot is untouched.
#[test]
fn ok_on_an_empty_zone_says_so() {
    let mut t = T::new();
    t.open();
    t.tap(Key::Down);
    t.tap(Key::Down); // Empty
    let before = (t.app.slot(0).vfo_mode, t.app.slot(0).vfo, t.app.slot(0).ch);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen().name(), "Zones");
    assert_eq!(toast(&t).as_deref(), Some("toast \"No channels\" \"in this zone\""));
    assert_eq!((t.app.slot(0).vfo_mode, t.app.slot(0).vfo, t.app.slot(0).ch), before);
}

/// CHAN-27, EDIT-6 (decision 178): SLOT on a zone opens its editor and SLOT (or BACK) leaves it; on the Nearby row
/// SLOT does nothing.
#[test]
fn slot_key_opens_the_zone_editor() {
    let mut t = T::new();
    t.open();
    t.tap(Key::Slot);
    assert_eq!(t.app.screen().name(), "Zone editor");
    assert_eq!(t.app.active, 0, "SLOT here doesn't change the active slot");
    t.tap(Key::Slot);
    assert_eq!((t.app.screen().name(), t.zones().cur.sel), ("Zones", 0));
    t.tap(Key::Up);
    assert!(t.zones().on_nearby());
    t.tap(Key::Slot);
    assert!(t.zones().on_nearby() && t.zones().edit().is_none());
}

/// CHAN-27, APRS-19 (decision 178): with APRS mode on and C active, OK on a zone gives APRS-19's refusal and stays;
/// channel-only mode (PIN-5, CHAN-16) still lets OK pick a zone.
#[test]
fn ok_refused_on_the_aprs_slot_and_allowed_channel_only() {
    let mut t = T::new();
    t.app.set_aprs_mode(true);
    t.app.active = 2;
    let before = (t.app.slot(2).vfo_mode, t.app.slot(2).vfo, t.app.slot(2).ch);
    t.open();
    t.tap(Key::Menu);
    assert_eq!(t.app.screen().name(), "Zones");
    assert!(toast(&t).is_some_and(|l| l.contains("APRS")), "{:?}", toast(&t));
    assert_eq!((t.app.slot(2).vfo_mode, t.app.slot(2).vfo, t.app.slot(2).ch), before, "C untouched");
    t.app.set_aprs_mode(false);
    t.app.active = 0;
    t.app.set.chan_only = true;
    t.open();
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Home));
    assert!(t.app.slot(0).on_channel());
}

/// A move redraws the two rows that changed and the preview; the header, the other rows and the hint bar stay.
#[test]
fn a_move_redraws_rows_and_preview_only() {
    let mut t = T::new();
    t.open();
    t.tap(Key::Down); // Repeaters, its preview drawn
    let before = t.app.drawn.hash;
    t.tap(Key::Down); // Empty
    let changed: Vec<usize> = (0..REGIONS).filter(|&r| t.app.drawn.hash[r] != before[r]).collect();
    // Nearby is row 0: Repeaters is row 2, Empty row 3
    let allowed = [R_ROW + 2, R_ROW + 3, R_PV, R_PV + 1, R_PV + 2, R_PV + 3, R_PV + 4, R_PV + 5, crate::screens::lists::R_GATE, R_SCROLL];
    assert!(changed.iter().all(|r| allowed.contains(r)), "{changed:?}");
    assert!(changed.contains(&(R_ROW + 2)) && changed.contains(&(R_ROW + 3)) && changed.contains(&R_PV));
    let texts: Vec<_> = t.lcd.texts.iter().map(|x| x.4.clone()).collect();
    assert!(!texts.iter().any(|x| x == "Simplex" || x == "Air" || x.contains("zones")), "only what changed is drawn: {texts:?}");
}

/// The channel browser's zone list draws its zones as the zones page does (chips, dimmed when empty).
#[test]
fn browser_zone_rows_match() {
    let mut t = T::new();
    t.app.set_channel(0, 0, 0, &t.s);
    t.app.open(Dest::Browse);
    t.draw();
    let texts: Vec<_> = t.lcd.texts.iter().map(|x| x.4.clone()).collect();
    for want in ["Simplex", "A", "Repeaters", "SCAN", "Weather", "HID", "All"] {
        assert!(texts.iter().any(|x| x == want), "{want}: {texts:?}");
    }
    let _ = NO_ZONE;
}

/// CHAN-34.1: Local is the zones page's last row (after the stored zones), named Local with its channel count and no
/// SCAN or HID chips; its preview says the range and lists its nearest four with their places, nearest first. OK
/// puts the active slot on its nearest channel and goes home ("Slot A" / "Local"); SLOT and \* do nothing on it.
#[test]
fn local_on_the_zones_page() {
    let mut t = T::new();
    t.open();
    t.tap(Key::Up);
    t.tap(Key::Up); // Nearby, then round to the last row
    let ui = t.ui();
    let row = ui.iter().find(|l| l.contains("kind=local")).cloned().unwrap_or_else(|| panic!("{ui:?}"));
    let n = t.app.local().len();
    assert!(n >= 3);
    assert_eq!(row, std::format!("row 0 \"Local\" \"{n}\" sel kind=local chips=\"\""));
    assert!(ui.iter().any(|l| l == "preview \"LOCAL\" \"within 50 km\""), "{ui:?}");
    let pv: Vec<&String> = ui.iter().filter(|l| l.starts_with("pvrow ")).collect();
    let first = t.s.get(t.app.local().num(0).unwrap()).unwrap();
    assert!(pv[0].starts_with(&std::format!("pvrow \"{}\" \"", s(first.name()))) && pv[0].contains("km\""), "{pv:?}");
    assert_eq!(pv.len(), (n as usize).min(4));
    assert!(ui.iter().any(|l| l.starts_with("pvfoot ") && l.contains("nearest first")), "{ui:?}");
    t.tap(Key::Star);
    assert_eq!(t.app.screen().name(), "Zones", "no delete");
    t.tap(Key::Slot);
    assert!(matches!(t.app.screen(), Screen::Group(g) if (g.g, g.sub, g.row) == (2, 0, 4)), "SLOT: its setting, 0 2 5");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen().name(), "Zones", "BACK: the list again");
    assert!(t.ui().iter().any(|l| l.contains("kind=local") && l.contains(" sel")), "still on Local");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Home));
    let sl = *t.app.slot(0);
    assert_eq!((sl.on_channel(), sl.zone, sl.ch.map(|c| c.num)), (true, crate::local::LOCAL, Some(first.num)));
    assert_eq!(toast(&t).as_deref(), Some("toast \"Slot A\" \"Local\""));
    let mut h = Buf::<32>::new();
    t.app.head_text(0, &LiveAll::default(), &t.s, &mut h);
    assert!(s(h.as_bytes()).ends_with("\u{b7} Local"), "the card names Local: {}", s(h.as_bytes()));
}

/// CHAN-34.4: without a position Local is still a row, dimmed with no channels, its preview saying "no position" and
/// what it needs; OK says so and stays; a slot on it keeps its channel and SIDE2 refuses a scan, saying why.
#[test]
fn local_without_a_position() {
    let mut t = T::new();
    t.app.tick(t.t, &LiveAll::default(), &t.s);
    t.app.set_channel(0, crate::local::LOCAL, 0, &t.s);
    let ch = t.app.slot(0).ch;
    t.s.fix.set(None);
    t.open();
    t.tap(Key::Up); // no Nearby row: round to the last
    let ui = t.ui();
    assert!(ui.iter().any(|l| l == "row 0 \"Local\" \"0\" sel grey kind=local chips=\"A\""), "{ui:?}");
    assert!(ui.iter().any(|l| l == "preview \"LOCAL\" \"no position\""), "{ui:?}");
    assert!(ui.iter().any(|l| l.starts_with("pvfoot ") && l.contains("GPS fix")), "{ui:?}");
    t.tap(Key::Menu);
    assert_eq!(t.app.screen().name(), "Zones");
    assert_eq!(toast(&t).as_deref(), Some("toast \"Local\" \"no position\""));
    assert_eq!(t.app.slot(0).ch, ch, "the slot keeps its channel");
    t.app.go_home();
    while t.app.pop_cmd().is_some() {}
    crate::screens::scan::side2(&mut t.app, &t.s);
    assert!(!core::iter::from_fn(|| t.app.pop_cmd()).any(|c| matches!(c, ferrite_core::msg::RadioCmd::ScanChans { .. } | ferrite_core::msg::RadioCmd::Scan { .. })), "no scan");
    assert_eq!(toast(&t).as_deref(), Some("toast \"Local\" \"no position\""));
}

/// CHAN-34.1, NAV-1: ◀▶ on home reach Local after the last zone (and ◀ from the first), on its nearest channel; the
/// knob steps it nearest first; ▶ goes round to the first zone. With Local off, ◀▶ never stop there.
#[test]
fn home_arrows_reach_local() {
    let mut t = T::new();
    t.app.set_channel(0, 5, 0, &t.s); // Weather, the last zone
    t.app.go_home();
    t.tap(Key::Right);
    let l = *t.app.local();
    assert_eq!((t.app.slot(0).zone, t.app.slot(0).ch.map(|c| c.num)), (crate::local::LOCAL, l.num(0)));
    t.app.knob(1, t.t, &t.s);
    assert_eq!(t.app.slot(0).ch.map(|c| c.num), l.num(1), "the next nearest");
    t.tap(Key::Right);
    assert_eq!(t.app.slot(0).zone, 0, "round to the first zone");
    t.tap(Key::Left);
    assert_eq!(t.app.slot(0).zone, crate::local::LOCAL, "back from the first zone");
    t.app.set.local = 0;
    t.tap(Key::Left); // Local turned off with the slot on it: the slot went to its channel's own zone
    t.app.set_channel(0, 5, 0, &t.s);
    t.tap(Key::Right);
    assert_eq!(t.app.slot(0).zone, 0, "Local off: from the last zone round to the first");
}
