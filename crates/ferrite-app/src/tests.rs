//! The app's rules as host tests: keys in, screens, commands and pixels out.

use super::*;
use crate::chan::{channel, Table};
use crate::screens::{advanced::Advanced, edit::Edit, quick::Quick, status::Status};
use crate::slot::{DEFAULT_CFG, Item};
use ferrite_core::msg::{Device, GpsState, SlotLive};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::gfx::{BG, BLUE, GREEN};
use std::vec::Vec;

static ZONES: [&[u8]; 3] = [b"Local", b"Empty", b"Repeaters"];
static CHANNELS: [Channel; 4] = [
    channel(0, 0, 14_652_000, "Calling", DEFAULT_CFG),
    channel(1, 0, 44_600_000, "UHF call", DEFAULT_CFG),
    channel(10, 2, 14_694_000, "North hill", DEFAULT_CFG),
    channel(11, 2, 44_347_500, "Club repeater", DEFAULT_CFG),
];

fn store() -> Table<'static> {
    Table { zones: &ZONES, channels: &CHANNELS }
}

/// Fresh settings for one test's App (the firmware keeps them in a static; a test leaks a few hundred bytes).
pub(crate) fn set() -> &'static mut crate::settings::Settings {
    std::boxed::Box::leak(std::boxed::Box::new(crate::settings::Settings::DEFAULT))
}

/// A fresh last-alert record for one test's App.
pub(crate) fn alert() -> &'static mut crate::screens::alert::AlertInfo {
    std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE))
}

/// An app, a clock and a quiet radio.
struct T {
    app: App,
    t: u32,
    live: LiveAll,
}

impl T {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        T { app, t: 1000, live: LiveAll::default() }
    }

    fn press(&mut self, k: Key, hold: u32) {
        let s = store();
        self.app.key_down(k, self.t, &s);
        let end = self.t + hold;
        while self.t < end {
            self.t += 10;
            self.app.tick(self.t, &self.live, &s);
        }
        self.app.key_up(k, self.t, &s);
        self.t += 50;
        self.app.tick(self.t, &self.live, &s);
    }

    fn tap(&mut self, k: Key) {
        self.press(k, 100);
    }

    fn hold(&mut self, k: Key) {
        self.press(k, 800);
    }

    fn knob(&mut self, d: i8) {
        self.app.knob(d, self.t, &store());
    }

    fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.app.tick(self.t, &self.live, &store());
        }
    }

    fn cmds(&mut self) -> Vec<RadioCmd> {
        let mut v = Vec::new();
        while let Some(c) = self.app.pop_cmd() {
            v.push(c);
        }
        v
    }

    fn freq(&self) -> Freq10 {
        self.app.slot(self.app.active()).freq()
    }

    fn render(&mut self, fb: &mut FrameBuffer) {
        let dev = Device { clock_min: Some(14 * 60 + 32), gps: GpsState::Fix, battery_pct: 80, ..Default::default() };
        self.app.render(fb, &self.live, &dev, &store());
    }
}

#[test]
fn ram_is_small() {
    let n = core::mem::size_of::<App>();
    std::println!("App: {n} bytes");
    assert!(n <= 1024, "App is {n} bytes");
    let s = core::mem::size_of::<crate::settings::Settings>();
    std::println!("Settings: {s} bytes");
    assert!(s <= 512, "Settings is {s} bytes");
}

#[test]
fn vfo_steps_and_bands() {
    let mut t = T::new();
    let f0 = t.freq();
    t.tap(Key::Up); // NAV-2: one step (12.5 kHz on VHF)
    assert_eq!(t.freq(), f0 + 1_250);
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: f0 + 1_250 }));
    t.knob(-2);
    assert_eq!(t.freq(), f0 - 1_250);
    // NAV-3: SIDE2 held, 1 MHz
    let s = store();
    t.app.key_down(Key::Side(2), t.t, &s);
    t.knob(1);
    t.app.key_up(Key::Side(2), t.t + 100, &s);
    assert_eq!(t.freq(), f0 - 1_250 + 100_000);
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Scan { .. })), "SIDE2 as a modifier doesn't scan");
    // ◀▶: bands, and back to where the VFO was on this band (VFO-1)
    let here = t.freq();
    t.tap(Key::Right);
    assert_eq!(slot::band_name(t.freq()), "Marine");
    t.tap(Key::Left);
    assert_eq!(t.freq(), here);
}

#[test]
fn arrows_repeat_while_held() {
    let mut t = T::new();
    let f0 = t.freq();
    t.press(Key::Up, 1000); // repeats from 400 ms at 10/s (KEY-4): 400..1000 is 7 steps, none on release
    assert_eq!(t.freq(), f0 + 7 * 1_250);
}

#[test]
fn channel_mode_steps_and_zones() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    let s = t.app.slot(0);
    assert!(s.on_channel());
    assert_eq!(s.ch.unwrap().name(), b"Calling");
    t.tap(Key::Down); // NAV-1: wraps within the zone
    assert_eq!(t.app.slot(0).ch.unwrap().name(), b"UHF call");
    t.tap(Key::Right); // the empty zone is skipped (CHAN-11)
    assert_eq!(t.app.slot(0).zone, 2);
    assert_eq!(t.app.slot(0).ch.unwrap().name(), b"North hill");
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: 14_694_000 }));
    t.tap(Key::VfoMem); // back to the VFO, unchanged
    assert!(!t.app.slot(0).on_channel());
    assert_eq!(t.freq(), 14_652_000);
}

#[test]
fn slot_cycles_and_edit_in_place() {
    let mut t = T::new();
    t.tap(Key::Slot); // NAV-5
    assert_eq!(t.app.active(), 1);
    assert!(t.cmds().contains(&RadioCmd::Active(1)));
    t.tap(Key::Slot);
    t.tap(Key::Slot);
    assert_eq!(t.app.active(), 0);
    t.hold(Key::Slot); // EDIT-1: first setting focused
    assert_eq!(t.app.screen(), Screen::Edit(Edit { cell: 0 }));
    t.tap(Key::Right);
    t.tap(Key::Right);
    t.tap(Key::Right); // stops at the edge
    t.tap(Key::Down);
    assert_eq!(t.app.screen(), Screen::Edit(Edit { cell: 5 })); // Step
    t.tap(Key::Left);
    t.tap(Key::Up);
    assert_eq!(t.app.screen(), Screen::Edit(Edit { cell: 1 })); // Bandwidth
    t.knob(1); // EDIT-3: the knob changes, at once (EDIT-4)
    assert!(!t.app.slot(0).cfg().narrow);
    assert!(t.cmds().contains(&RadioCmd::SlotParam { slot: 0, param: Param::Bandwidth, value: 0 }));
    t.tap(Key::Star); // undo within 5 s
    assert!(t.app.slot(0).cfg().narrow);
    t.tap(Key::Slot); // EDIT-6: SLOT leaves
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.active(), 0, "leaving edit doesn't switch slots");
    t.hold(Key::Slot);
    t.wait(EDIT_IDLE_MS + 200); // EDIT-6: 8 s idle
    assert_eq!(t.app.screen(), Screen::Home);
}

#[test]
fn step_is_vfo_only() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.hold(Key::Slot);
    for _ in 0..2 {
        t.tap(Key::Right);
    }
    t.tap(Key::Down); // Step
    let before = *t.app.slot(0).cfg();
    t.knob(1);
    assert_eq!(*t.app.slot(0).cfg(), before, "greyed on a channel (VFO-8, EDIT-7)");
    assert_eq!(t.app.greyed(Item::Step), Some("VFO only"));
}

#[test]
fn quick_settings() {
    let mut t = T::new();
    t.tap(Key::Menu); // MENU-1
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 0 }));
    t.tap(Key::Right); // MENU-16: ◀▶ change one step
    assert_eq!(t.app.slot(0).cfg().squelch, 5);
    t.knob(2);
    assert_eq!(t.app.slot(0).cfg().squelch, 7);
    t.tap(Key::Hash); // MENU-18: default
    assert_eq!(t.app.slot(0).cfg().squelch, 4);
    t.tap(Key::Up); // wraps to "0 Advanced"
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 9 }));
    t.tap(Key::Digit(7)); // MENU-5: digits pick
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 6 }));
    t.knob(1);
    assert_eq!(t.app.watch(), 2);
    assert!(t.cmds().contains(&RadioCmd::Watch(0b111)));
    t.tap(Key::Digit(0)); // advanced, then BACK returns to quick where it was (MENU-24)
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: 0, from_quick: true }));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 6 }));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
}

#[test]
fn power_capped_on_125m() {
    let mut t = T::new();
    for _ in 0..2 {
        t.tap(Key::Right); // VHF -> Marine -> 1.25 m
    }
    assert_eq!(slot::band_name(t.freq()), "1.25 m");
    t.tap(Key::Menu);
    t.tap(Key::Down); // TX power
    t.knob(2);
    assert_eq!(t.app.slot(0).cfg().power, 0, "fixed at 0.25 W on 1.25 m");
    assert!(t.app.greyed(Item::Power).is_some());
}

#[test]
fn menu_zero_and_status() {
    let mut t = T::new();
    t.hold(Key::Menu); // MENU-2: hold MENU on home
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: 0, from_quick: false }));
    t.tap(Key::Digit(0)); // MENU 0 0 (SBAR-13)
    assert_eq!(t.app.screen(), Screen::Status(Status { row: 0, from_menu: true }));
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    t.tap(Key::Hash); // HOME-10
    assert_eq!(t.app.screen(), Screen::Status(Status { row: 0, from_menu: false }));
    t.hold(Key::Exit); // KEY-5: hold BACK goes home
    assert_eq!(t.app.screen(), Screen::Home);
    t.tap(Key::Menu);
    t.wait(MENU_TIMEOUT_MS + 200); // MENU-28
    assert_eq!(t.app.screen(), Screen::Home);
}

#[test]
fn hash_pauses_watch_while_a_watched_slot_receives() {
    let mut t = T::new();
    t.live.slots[1] = SlotLive { squelch_open: true, rssi: -90, ..Default::default() };
    t.wait(100);
    t.tap(Key::Hash); // WATCH-6
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::PauseWatch(PAUSE_S)));
}

#[test]
fn key_lock() {
    let mut t = T::new();
    t.hold(Key::Hash); // KEY-13
    assert!(t.app.locked());
    let f = t.freq();
    t.tap(Key::Up);
    t.knob(3);
    t.tap(Key::Menu);
    assert_eq!((t.freq(), t.app.screen()), (f, Screen::Home), "KEY-14: other keys do nothing");
    t.hold(Key::Hash);
    assert!(!t.app.locked());
}

#[test]
fn side1_and_knob_set_squelch() {
    let mut t = T::new();
    let (s, f) = (store(), t.freq());
    t.app.key_down(Key::Side(1), t.t, &s);
    t.knob(2);
    t.app.key_down(Key::Up, t.t, &s);
    t.app.key_up(Key::Up, t.t + 100, &s);
    t.knob(20);
    assert_eq!(t.app.slot(0).cfg().squelch, 9, "4 + 2 + 1, then stops at 9");
    assert_eq!(t.freq(), f, "no tuning while SIDE1 is held");
    t.app.key_up(Key::Side(1), t.t + 200, &s);
    assert!(t.cmds().contains(&RadioCmd::SlotParam { slot: 0, param: Param::Squelch, value: 9 }));
}

#[test]
fn side2_scans_the_active_slot() {
    let mut t = T::new();
    t.tap(Key::Side(2));
    assert!(t.cmds().contains(&RadioCmd::Scan { slot: 0, on: true }));
}

/// Chapter 7's hold-digit shortcuts, MENU 0 paths and BACK through them (MENU-24).
#[test]
fn shortcuts_and_menu_paths() {
    let mut t = T::new();
    for (d, name) in [(2, "FM broadcast"), (3, "Scope"), (4, "APRS heard"), (7, "Weather radio"), (9, "Tone search")] {
        t.hold(Key::Digit(d));
        assert_eq!(t.app.screen().name(), name, "hold {d}");
        t.tap(Key::Exit);
        assert_eq!(t.app.screen(), Screen::Home);
    }
    t.hold(Key::Digit(5)); // TX power: low -> mid
    assert_eq!(t.app.slot(0).cfg().power, 1);
    t.hold(Key::Digit(6)); // bandwidth: narrow -> wide
    assert!(!t.app.slot(0).cfg().narrow);
    t.hold(Key::Digit(8)); // Battery Save 1 -> 2
    assert!(t.cmds().contains(&RadioCmd::BatterySave(ferrite_core::rxsleep::Level::L2)));
    t.tap(Key::Digit(4)); // NAV-4: entry
    assert!(matches!(t.app.screen(), Screen::Entry(_)));
    t.hold(Key::Exit);
    // MENU 0 9 2 3: link status, typed straight through (MENU-5)
    use crate::screens::groups::Group;
    for d in [0, 9, 2, 3] {
        t.tap(if d == 0 { Key::Menu } else { Key::Digit(d) });
        if d == 0 {
            t.tap(Key::Digit(0));
        }
    }
    assert!(matches!(t.app.screen(), Screen::Link(_)));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 9, sub: 2, row: 2 }), "the list, on its row");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 9, sub: 0, row: 1 }));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: 8, from_quick: true }), "keeps the place");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 0 }));
    t.tap(Key::Digit(4)); // MENU 4, OK: tones
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Tones(_)));
    t.wait(MENU_TIMEOUT_MS + 200);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// KEY-7: PTT transmits from any screen, going home first; locked too (KEY-14).
#[test]
fn ptt_from_any_screen() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    t.tap(Key::Menu);
    t.hold(Key::Hash);
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    t.app.key_up(Key::PTT, t.t + 300, &s);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// Every destination opens and draws (its placeholder until it is built), and BACK leaves it.
#[test]
fn every_destination_draws() {
    use crate::screens::nav::{Dest, ALL, MENU0};
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    for dest in ALL.into_iter().chain(MENU0.map(|(_, d)| d)) {
        t.hold(Key::Exit);
        t.render(&mut fb);
        t.app.open(dest);
        t.render(&mut fb);
        assert!(count_px(&fb, 20, 300, |c| c != BG) > 100, "{dest:?} drew nothing");
        let mut b = ferrite_ui::text::Buf::<10>::new();
        t.app.screen().title(&t.app, &mut b);
        assert!(!b.is_empty(), "{dest:?}: no status bar name");
        if dest != Dest::Transmit {
            t.tap(Key::Exit);
            assert_ne!(t.app.screen(), Screen::Coming(crate::screens::coming::Coming { dest }), "{dest:?}: BACK");
        }
    }
    let ids: std::collections::BTreeSet<_> = ALL.iter().map(|d| d.info().2).collect();
    assert_eq!(ids.len(), 66, "one inventory ID per destination (groups and lists share D3)");
}

mod group_a;

fn count_px(fb: &FrameBuffer, y0: i32, y1: i32, f: impl Fn(u16) -> bool) -> usize {
    (y0..y1).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| f(fb.at(x, y))).count()
}

#[test]
fn screens_draw() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.live.slots[0] = SlotLive { freq: t.freq(), squelch_open: true, rssi: -85, ..Default::default() };
    t.wait(100);
    t.render(&mut fb);
    // Home: a blue-bordered big card, a green meter (receiving), a hint bar.
    assert_eq!(fb.at(6, 80), BLUE);
    assert!(count_px(&fb, 125, 133, |c| c == GREEN) > 100, "meter lit green");
    assert!(count_px(&fb, 301, 320, |c| c != BG) > 50, "hint bar text");
    for (k, hold) in [(Key::Slot, 800), (Key::Exit, 100), (Key::Menu, 100), (Key::Digit(0), 100), (Key::Exit, 100)] {
        t.press(k, hold);
        t.render(&mut fb);
        assert!(count_px(&fb, 20, 300, |c| c != BG) > 1000, "{:?} drew its content", t.app.screen());
        assert!(count_px(&fb, 0, 19, |c| c != BG) > 50, "status bar");
    }
    t.tap(Key::Exit);
    t.tap(Key::Hash);
    t.render(&mut fb);
    assert!(matches!(t.app.screen(), Screen::Status(_)));
    assert!(count_px(&fb, 20, 38, |c| c == BLUE) > 2000, "first row selected");
}

/// HOME-7: a meter change redraws the meter, not the frequency; a still screen draws nothing.
#[test]
fn redraws_only_what_changed() {
    struct Count(FrameBuffer, Vec<(i32, i32, i32, i32)>, u64);
    impl Panel for Count {
        fn window(&mut self, a: i32, b: i32, c: i32, d: i32) {
            self.1.push((a, b, c, d));
            self.0.window(a, b, c, d)
        }
        fn push(&mut self, c: u16, n: u32) {
            self.2 += n as u64;
            self.0.push(c, n)
        }
        fn push_buf(&mut self, p: &[u16]) {
            self.2 += p.len() as u64;
            self.0.push_buf(p)
        }
    }
    let mut t = T::new();
    let mut lcd = Count(FrameBuffer::new(), Vec::new(), 0);
    let dev = Device { clock_min: Some(600), ..Default::default() };
    t.live.slots[0] = SlotLive { freq: t.freq(), squelch_open: true, rssi: -100, ..Default::default() };
    t.app.render(&mut lcd, &t.live, &dev, &store());
    lcd.1.clear();
    lcd.2 = 0;
    t.app.render(&mut lcd, &t.live, &dev, &store());
    assert_eq!(lcd.2, 0, "nothing changed, nothing drawn");
    t.live.slots[0].rssi = -90;
    t.app.render(&mut lcd, &t.live, &dev, &store());
    assert!(lcd.2 > 0 && lcd.2 < 1500, "meter only: {} px", lcd.2);
    assert!(lcd.1.iter().all(|r| r.1 >= 110), "nothing above the meter row: {:?}", lcd.1);
    lcd.1.clear();
    lcd.2 = 0;
    t.tap(Key::Up);
    t.app.render(&mut lcd, &t.live, &dev, &store());
    assert!(lcd.1.iter().all(|r| r.1 >= 50 && r.3 <= 115), "frequency only: {:?}", lcd.1);
}

#[test]
fn toasts_replace_the_hint_bar_for_3_s() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.app.notify(b"VE3KPW-9", b"On my way", false);
    t.render(&mut fb);
    assert_eq!(fb.at(120, 300), BLUE, "toast's top line");
    t.wait(TOAST_MS + 200);
    t.render(&mut fb);
    assert_ne!(fb.at(120, 300), BLUE, "hint bar back");
}

/// HOME-11: the slots stay stacked A, B, C; SLOT only moves which one is large, in its own place.
#[test]
fn slots_stay_in_order() {
    use crate::screens::home::card;
    for active in 0..3u8 {
        let (ya, _) = card(0, active);
        let (yb, _) = card(1, active);
        let (yc, hc) = card(2, active);
        assert!(ya < yb && yb < yc, "A above B above C");
        assert_eq!(ya, 25);
        assert!(yc + hc <= 296, "fits above the hint bar");
        assert_eq!(card(active, active).1, 115);
    }
    assert_eq!(card(1, 1), (93, 115));
}

/// HOME-11: after SLOT moves the large card, nothing of the old large card is left in the new gaps.
#[test]
fn slot_change_leaves_no_trace() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    t.render(&mut fb);
    // A is large (25..140). After SLOT, B is large: A shrinks to 25..87 and the gap 87..93 must be background.
    t.tap(Key::Slot);
    t.render(&mut fb);
    let (ya, ha) = crate::screens::home::card(0, 1);
    let (yb, _) = crate::screens::home::card(1, 1);
    for y in ya + ha + 1..yb - 1 {
        for x in 10..230 {
            assert_eq!(fb.at(x, y), BG, "stale pixel at {x},{y} between A and B");
        }
    }
}

/// Every character in every string literal of the UI's sources is in the text fonts: a font is cut to its
/// character set (tools/mkassets.py), and a missing character would silently draw as '?'.
#[test]
fn fonts_have_every_character_the_ui_draws() {
    use ferrite_ui::assets::{UI_12, UI_12B};
    let sources = [
        ("lib.rs", include_str!("lib.rs")),
        ("screens/mod.rs", include_str!("screens/mod.rs")),
        ("screens/shared.rs", include_str!("screens/shared.rs")),
        ("screens/nav.rs", include_str!("screens/nav.rs")),
        ("screens/home.rs", include_str!("screens/home.rs")),
        ("screens/edit.rs", include_str!("screens/edit.rs")),
        ("screens/quick.rs", include_str!("screens/quick.rs")),
        ("screens/advanced.rs", include_str!("screens/advanced.rs")),
        ("screens/status.rs", include_str!("screens/status.rs")),
        ("screens/coming.rs", include_str!("screens/coming.rs")),
        // group C: lists and data
        ("screens/lists.rs", include_str!("screens/lists.rs")),
        ("screens/log.rs", include_str!("screens/log.rs")),
        ("screens/opts.rs", include_str!("screens/opts.rs")),
        ("screens/aprs.rs", include_str!("screens/aprs.rs")),
        ("screens/msgs.rs", include_str!("screens/msgs.rs")),
        ("screens/gps.rs", include_str!("screens/gps.rs")),
        ("screens/browse.rs", include_str!("screens/browse.rs")),
        ("screens/zones.rs", include_str!("screens/zones.rs")),
        ("screens/scanlists.rs", include_str!("screens/scanlists.rs")),
        ("screens/recall.rs", include_str!("screens/recall.rs")),
        ("screens/sensors.rs", include_str!("screens/sensors.rs")),
        ("data.rs", include_str!("data.rs")),
        ("sample.rs", include_str!("sample.rs")),
        ("screens/groups.rs", include_str!("screens/groups.rs")),
        ("screens/setup.rs", include_str!("screens/setup.rs")),
        ("screens/pin.rs", include_str!("screens/pin.rs")),
        ("screens/info.rs", include_str!("screens/info.rs")),
        ("screens/keytest.rs", include_str!("screens/keytest.rs")),
        ("screens/link.rs", include_str!("screens/link.rs")),
        ("screens/fm.rs", include_str!("screens/fm.rs")),
        ("screens/hf.rs", include_str!("screens/hf.rs")),
        ("screens/alert.rs", include_str!("screens/alert.rs")),
        ("settings.rs", include_str!("settings.rs")),
        ("slot.rs", include_str!("slot.rs")),
        ("chan.rs", include_str!("chan.rs")),
        ("widgets.rs", include_str!("../../ferrite-ui/src/widgets.rs")),
        ("boot.rs", include_str!("../../ferrite-ui/src/boot.rs")),
        ("home.rs", include_str!("../../ferrite-ui/src/home.rs")),
    ];
    let mut checked = 0;
    for (file, src) in sources {
        for s in string_literals(src) {
            for c in s.chars().filter(|c| !('\x18'..='\x1b').contains(c) && *c != '\n') {
                assert!(UI_12.has(c) && UI_12B.has(c), "{file}: {c:?} (U+{:04X}) in {s:?} is not in the text fonts", c as u32);
                checked += 1;
            }
        }
    }
    assert!(checked > 2000, "the scan found only {checked} characters");
    for c in "0123456789.- ".chars() {
        for f in [&ferrite_ui::assets::DIG_46, &ferrite_ui::assets::DIG_28, &ferrite_ui::assets::DIG_26, &ferrite_ui::assets::DIG_17] {
            assert!(f.has(c), "digit fonts lack {c:?}");
        }
    }
}

/// The string literals of a Rust source, escapes decoded; comments and char literals skipped.
fn string_literals(src: &str) -> Vec<std::string::String> {
    let b: Vec<char> = src.chars().collect();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        match b[i] {
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '\'' => {
                // a char literal ('x', '\n', '\u{b7}') or a lifetime
                if b.get(i + 1) == Some(&'\\') {
                    i += 2;
                    while i < b.len() && b[i] != '\'' {
                        i += 1;
                    }
                    i += 1;
                } else if b.get(i + 2) == Some(&'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            '"' => {
                let mut s = std::string::String::new();
                i += 1;
                while i < b.len() && b[i] != '"' {
                    if b[i] == '\\' {
                        i += 1;
                        match b[i] {
                            'n' => s.push('\n'),
                            't' | '0' | 'r' => {}
                            'x' => {
                                let v = u32::from_str_radix(&b[i + 1..i + 3].iter().collect::<std::string::String>(), 16).unwrap();
                                s.push(char::from_u32(v).unwrap());
                                i += 2;
                            }
                            'u' => {
                                let end = i + b[i..].iter().position(|&c| c == '}').unwrap();
                                let hex: std::string::String = b[i + 2..end].iter().collect();
                                s.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                                i = end;
                            }
                            '\n' => {
                                while b.get(i + 1).is_some_and(|c| c.is_whitespace()) {
                                    i += 1;
                                }
                            }
                            c => s.push(c),
                        }
                    } else {
                        s.push(b[i]);
                    }
                    i += 1;
                }
                out.push(s);
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

#[test]
fn literal_scanner() {
    let v = string_literals("let a = \"A\\u{b7}B\"; // \"not this\"\nlet c = '\"'; let d = b\"x\\x18\";");
    assert_eq!(v, ["A\u{b7}B", "x\x18"]);
}

/// Group B: transmit, scan, scope, tone search, counter, FM, HF, weather, alert, crossband, layouts.
mod radio_activity;

/// Group D's screens: setup, PIN, key test, link, FM broadcast, HF mode, alert, the menu's groups.
#[path = "tests_d.rs"]
mod d;


/// One BACK rule everywhere (KEY-5, MENU-24): short BACK goes back one level, to the screen this one was opened
/// from; hold BACK goes home from anywhere. Paths that used to jump home: the Status page's Watch row (quick
/// settings), edit in place and the channel editor, scan, an alert, the shortcut map.
#[test]
fn back_is_one_level_hold_back_is_home() {
    let mut t = T::new();
    // Status page -> Watch (quick settings) -> BACK -> Status -> BACK -> home
    t.tap(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Status(_)));
    for _ in 0..5 {
        t.tap(Key::Down); // Watch
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 6 }));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Status(s) if s.row == 5), "back to the Status page, on its row");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    // MENU 0 0 (Status from advanced) -> BACK -> advanced -> BACK -> home
    t.hold(Key::Menu);
    t.tap(Key::Digit(0));
    assert!(matches!(t.app.screen(), Screen::Status(_)));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Advanced(_)));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    // Quick -> value editor -> BACK -> quick; edit in place -> BACK -> home
    t.tap(Key::Menu);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Value(_)));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Quick(_)));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    t.hold(Key::Slot);
    assert!(matches!(t.app.screen(), Screen::Edit(_)));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    // The channel editor from quick settings (MENU 8) -> BACK -> quick
    t.tap(Key::VfoMem);
    t.tap(Key::Menu);
    t.tap(Key::Digit(8));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::ChEdit(_)));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Quick(_)), "the editor goes back to quick settings");
    // Hold BACK: home from three levels down.
    t.tap(Key::Digit(0));
    t.tap(Key::Digit(9));
    assert!(matches!(t.app.screen(), Screen::Group(_)));
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    // The key test is left with hold BACK; a short BACK is a key it shows.
    t.app.open(crate::screens::nav::Dest::KeyTest);
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::KeyTest(_)), "short BACK is tested, not obeyed");
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// CHAN-23, VFO-4: a channel's or home VFO's mode is FM, NFM or AM; USB, LSB and CW are HF mode's only.
#[test]
fn slot_modes_are_fm_nfm_am() {
    let mut c = DEFAULT_CFG;
    let mut seen = [false; 6];
    for _ in 0..7 {
        let v = Item::Mode.get(&c).0 + 1;
        Item::Mode.set(&mut c, v, 0);
        seen[c.mode as usize] = true;
    }
    assert_eq!(seen, [true, true, true, false, false, false]);
    let mut t = T::new();
    for on_channel in [false, true] {
        if on_channel {
            t.tap(Key::VfoMem);
            assert!(t.app.slot(0).on_channel());
        }
        for d in [1, 1, 1, 1, -1, -1, -1, -1] {
            t.app.change(Item::Mode, d);
            assert!(t.app.slot(0).cfg().mode < 3, "mode {} on {}", t.app.slot(0).cfg().mode, if on_channel { "a channel" } else { "the VFO" });
        }
    }
}

/// MENU-8: the Recent row on advanced settings can be selected; ◀▶ pick a recent setting and OK opens its
/// editor, BACK returns to the row.
#[test]
fn recent_row_is_selectable() {
    let mut t = T::new();
    t.hold(Key::Menu);
    t.tap(Key::Up);
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: 8, from_quick: false }), "nothing recent: wraps to the groups");
    t.hold(Key::Exit);
    t.tap(Key::Menu); // quick: squelch, then power
    t.knob(1);
    t.tap(Key::Digit(5));
    t.knob(1);
    t.hold(Key::Exit);
    t.hold(Key::Menu);
    t.tap(Key::Up);
    let recent = crate::screens::advanced::RECENT;
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: recent, from_quick: false }));
    t.tap(Key::Right);
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: recent + 1, from_quick: false }));
    t.tap(Key::Menu);
    let second = t.app.recent[1].unwrap();
    assert!(matches!(t.app.screen(), Screen::Value(v) if v.item == second), "{:?}", t.app.screen());
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: recent + 1, from_quick: false }));
    t.tap(Key::Down);
    assert_eq!(t.app.screen(), Screen::Advanced(Advanced { row: 0, from_quick: false }));
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
}
