//! The scan screen (manual chapter 14, SCAN-1 to SCAN-15; no mock-up): SIDE2 on home starts a scan on the
//! active slot and opens this screen. It shows the target, the resume rule, where the scan is, the hits (when,
//! what, how strong) and the skip count (SCAN-7). \* skips the current hit for this scan (SCAN-11, KEY-18),
//! digits add or remove zones 0-9 from the set (SCAN-3), SIDE2 stops (SCAN-1), BACK goes back (home: SIDE2
//! starts it there) with the scan still running (it shows on the card).

use super::activity::Hit;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_name, chan_label, BANDS, band_near};
use crate::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, RadioCmd};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Resume rule (MENU 0 1 5, default after 5 s) until the settings store holds it.
pub const RESUME: &str = "After 5 s";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scan {
    pub slot: u8,
}

/// SIDE2 on home (SCAN-1): start a scan on the active slot and show it, or stop the one running.
pub fn side2(app: &mut App) {
    let a = app.active;
    if app.scan_mask & 1 << a != 0 {
        app.out.push(RadioCmd::Scan { slot: a, on: false });
        return;
    }
    app.out.push(RadioCmd::Scan { slot: a, on: true });
    app.radio.clear_hits();
    app.radio.zones = 0;
    app.screen = crate::Screen::Scan(Scan { slot: a });
}

impl View for Scan {
    fn name(self) -> &'static str {
        "Scan"
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        let s = self.slot;
        match (e.key, e.press) {
            (Key::Side(2), Press::Short) => {
                app.out.push(RadioCmd::Scan { slot: s, on: false });
                app.go_home();
            }
            (Key::Star, Press::Short) => {
                // SCAN-11: on a hit, or in the resume wait after one.
                if app.rx_mask & 1 << s != 0 || app.radio.hits[0].what != 0 && app.radio.hits[0].age_ms(app.now) < 6000 {
                    app.out.push(RadioCmd::ScanSkip { slot: s });
                    app.show_toast(b"Skipped", b"for this scan", false);
                } else {
                    app.show_toast(b"Nothing to skip", b"", false);
                }
            }
            (Key::Digit(d), Press::Short) if app.slot(s).on_channel() => {
                // SCAN-3: zones by number join or leave the set (0-9 here).
                app.radio.zones ^= 1 << d;
                app.out.push(RadioCmd::ScanZones { slot: s, zones: app.radio.zones });
                let mut b = Buf::<28>::new();
                let _ = uwrite!(b, "Zone {} ", d);
                b.push(if app.radio.zones & 1 << d != 0 { b"added" } else { b"removed" });
                app.show_toast(b.as_bytes(), b"", false);
            }
            (Key::Exit, Press::Short) => app.back(), // the scan keeps running
            _ => {}
        }
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        let lv = &live.slots[self.slot as usize];
        if lv.scanning && lv.squelch_open {
            app.radio.push_hit(Hit::new(lv.freq, app.now, lv.rssi));
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let s = self.slot;
        let lv = cx.live.slots[s as usize];
        let sl = *app.slot(s);
        // The slot card: letter, SCAN and band, the state; the frequency it is on now; the channel's name.
        let (y, h) = (25, 92);
        let (state, sc) = if !lv.scanning {
            ("stopped", AMBER)
        } else if lv.squelch_open {
            ("RX", GREEN)
        } else {
            ("scanning", SUB)
        };
        if app.dirty(1, hash(&(state == "RX"))) {
            lcd.fill(6, y, 228, h, BG);
            lcd.fill_r(6, y, 228, h, 6, SURFACE);
            lcd.frame_r(6, y, 228, h, 6, if state == "RX" { GREEN } else { BLUE });
            app.drawn.hash[2..5].fill(0);
        }
        let mut head = Buf::<32>::new();
        head.push(b"SCAN");
        head.push(" \u{b7} ".as_bytes());
        head.push(band_name(lv.freq).as_bytes());
        if app.dirty(2, hash(&(head, state))) {
            w::badge(lcd, 14, y + 8, s, true, SURFACE);
            w::field(lcd, 34, 120, y + 7, y + 24, y + 20, Align::Left, head.as_bytes(), &UI_12, SUB, SURFACE);
            if state == "RX" {
                lcd.fill(154, y + 6, 74, 18, SURFACE);
                w::tag(lcd, 226, y + 7, b"RX", GREEN, SURFACE);
            } else {
                w::field(lcd, 154, 72, y + 7, y + 24, y + 20, Align::Right, state.as_bytes(), &UI_12, sc, SURFACE);
            }
        }
        if app.dirty(3, hash(&lv.freq)) {
            w::freq(lcd, 15, 226, y + 64, lv.freq, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
        }
        // The channel it stopped on, or the S-meter text.
        let mut name = Buf::<32>::new();
        if sl.on_channel() {
            if let Some(c) = channel_on(cx.store, sl.zone, lv.freq) {
                chan_label(c.num, &mut name);
                name.push(" \u{b7} ".as_bytes());
                name.push(c.name());
            }
        }
        let st = if lv.squelch_open { w::s_text(lv.rssi) } else { Buf::new() };
        if app.dirty(4, hash(&(name, st))) {
            w::field(lcd, 15, 170, y + 70, y + 88, y + 84, Align::Left, name.as_bytes(), &UI_12, TEXT, SURFACE);
            w::field(lcd, 185, 41, y + 70, y + 88, y + 84, Align::Right, st.as_bytes(), &UI_12B, GREEN, SURFACE);
        }
        // SCAN-7: target, resume, position, hits, skips.
        let mut target = Buf::<28>::new();
        let mut pos = Buf::<28>::new();
        if sl.on_channel() {
            target.push(b"Zone ");
            target.push(cx.store.zone_name(sl.zone).unwrap_or_else(|| crate::chan::Name::from(b"?")).as_bytes());
            let z = app.radio.zones;
            if z != 0 {
                for d in (0..10).filter(|d| z & 1 << d != 0) {
                    let _ = uwrite!(target, " +{}", d);
                }
            }
            let n = cx.store.zone_len(sl.zone);
            let i = (0..n).position(|i| cx.store.zone_at(sl.zone, i).is_some_and(|c| c.rx == lv.freq));
            match i {
                Some(i) => {
                    let _ = uwrite!(pos, "{} of {}", i + 1, n);
                }
                None => pos.set(b"-"),
            }
        } else {
            let b = &BANDS[band_near(lv.freq)];
            target.push(b"Band ");
            target.push(b.name.as_bytes());
            let _ = uwrite!(pos, "{} to {} MHz", b.lo / 100_000, b.hi / 100_000);
        }
        let mut hits = Buf::<28>::new();
        let _ = uwrite!(hits, "{}", lv.hits);
        let mut skips = Buf::<28>::new();
        let _ = uwrite!(skips, "{}", lv.skips);
        let rows: [(&[u8], &[u8]); 5] = [
            (b"Target", target.as_bytes()),
            (b"Resume", RESUME.as_bytes()),
            (b"Position", pos.as_bytes()),
            (b"Hits", hits.as_bytes()),
            (b"Skips", skips.as_bytes()),
        ];
        for (i, (l, v)) in rows.iter().enumerate() {
            app.kv(lcd, 4 + i, 122 + 18 * i as i32, 18, &w::KV_STATUS, l, v, TEXT, false);
        }
        // The last hits: how long ago, what, how strong.
        if app.dirty(10, 1) {
            w::field(lcd, 8, 224, 214, 230, 226, Align::Left, b"Last hits", &UI_12B, SUB, BG);
        }
        for i in 0..super::activity::HITS {
            let hit = app.radio.hits[i];
            let mut a = Buf::<16>::new();
            let mut what = Buf::<32>::new();
            let mut sig = Buf::<4>::new();
            if hit.what != 0 {
                ago(hit.age_ms(app.now), &mut a);
                what.push(w::freq3(hit.what).as_bytes());
                if sl.on_channel() {
                    if let Some(c) = channel_on(cx.store, sl.zone, hit.what) {
                        what.push(" \u{b7} ".as_bytes());
                        what.push(c.name());
                    }
                }
                sig = w::s_text(hit.rssi());
            } else if i == 0 {
                what.set(b"None yet");
            }
            let y = 232 + 18 * i as i32;
            if app.dirty(11 + i, hash(&(a, what, sig))) {
                w::field(lcd, 8, 52, y, y + 18, y + 13, Align::Left, a.as_bytes(), &UI_12, SUB, BG);
                w::field(lcd, 60, 140, y, y + 18, y + 13, Align::Left, what.as_bytes(), &UI_12, TEXT, BG);
                w::field(lcd, 200, 32, y, y + 18, y + 13, Align::Right, sig.as_bytes(), &UI_12B, GREEN, BG);
            }
        }
    }

    fn hints(self, app: &App) -> Hints {
        if app.slot(self.slot).on_channel() {
            Hints::new(&[("*", "Skip"), ("0-9", "Zones"), ("SIDE2", "Stop")])
        } else {
            Hints::new(&[("*", "Skip"), ("SIDE2", "Stop"), ("BACK", "Back")])
        }
    }
}

/// The zone's channel on this frequency, if any.
fn channel_on(store: &impl ChannelStore, zone: u8, f: u32) -> Option<crate::chan::Channel> {
    (0..store.zone_len(zone)).filter_map(|i| store.zone_at(zone, i)).find(|c| c.rx == f)
}

/// "12 s", "3 min", "2 h".
pub fn ago(ms: u32, b: &mut Buf<16>) {
    let s = ms / 1000;
    b.clear();
    if s < 60 {
        let _ = uwrite!(b, "{} s", s);
    } else if s < 3600 {
        let _ = uwrite!(b, "{} min", s / 60);
    } else {
        let _ = uwrite!(b, "{} h", s / 3600);
    }
}
