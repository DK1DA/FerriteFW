//! Group C's rules as host tests, on the sample records: the ways in and out, the main keys, and the safety
//! rules (asking before deleting, nothing sent without a callsign).

use crate::chan::{ChannelStore, NO_ZONE};
use crate::data::{Data, MsgState};
use crate::sample::Sample;
use crate::screens::nav::Dest;
use crate::screens::Screen;
use crate::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::{Device, GpsState, LiveAll, RadioCmd};
use ferrite_ui::assets::{UI_12, UI_12B};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::gfx::text4_width;
use std::vec::Vec;

struct T {
    app: App,
    t: u32,
    s: Sample,
    fb: FrameBuffer,
}

impl T {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        T { app, t: 1000, s: Sample::new(), fb: FrameBuffer::new() }
    }

    fn press(&mut self, k: Key, hold: u32) {
        let live = LiveAll::default();
        self.app.key_down(k, self.t, &self.s);
        let end = self.t + hold;
        while self.t < end {
            self.t += 10;
            self.app.tick(self.t, &live, &self.s);
        }
        self.app.key_up(k, self.t, &self.s);
        self.t += 50;
        self.app.tick(self.t, &live, &self.s);
        self.draw();
    }

    fn tap(&mut self, k: Key) {
        self.press(k, 100);
    }

    fn hold(&mut self, k: Key) {
        self.press(k, 800);
    }

    fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.app.tick(self.t, &LiveAll::default(), &self.s);
        }
    }

    fn open(&mut self, d: Dest) {
        self.app.open(d);
        self.draw();
    }

    /// Draws, as the UI task does after every input (renders must never panic).
    fn draw(&mut self) {
        let dev = Device { gps: GpsState::Fix, sats: 9, clock_min: Some(14 * 60 + 32), ..Default::default() };
        self.app.render(&mut self.fb, &LiveAll::default(), &dev, &self.s);
    }

    fn name(&self) -> &'static str {
        self.app.screen().name()
    }

    /// In text entry: clears the text and types "AB" (multi-tap 2, pause, 22), then OK.
    fn type_ab(&mut self) {
        assert!(matches!(self.app.screen(), Screen::Text(_)), "{}", self.name());
        while !self.app.text.is_empty() {
            self.tap(Key::Exit);
        }
        self.tap(Key::Digit(2));
        self.wait(1100);
        self.tap(Key::Digit(2));
        self.tap(Key::Digit(2));
        assert_eq!(self.app.text.as_bytes(), b"AB");
        self.tap(Key::Menu);
        while let Some(w) = self.app.pop_write() {
            self.s.apply(w);
        }
    }

    fn cmds(&mut self) -> Vec<RadioCmd> {
        let mut v = Vec::new();
        while let Some(c) = self.app.pop_cmd() {
            v.push(c);
        }
        v
    }
}

#[test]
fn aprs_heard_in_and_out() {
    let mut t = T::new();
    t.hold(Key::Digit(4)); // hold 4 on home
    assert_eq!(t.name(), "APRS heard");
    t.tap(Key::Down);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "APRS station");
    assert!(matches!(t.app.screen(), Screen::Aprs(a) if a.cur.sel == 1));
    t.tap(Key::Exit);
    assert_eq!(t.name(), "APRS heard");
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Home));
}

#[test]
fn menu_paths_reach_the_lists() {
    // MENU 0 5 2 from home: advanced, group 5, then 2 (MENU-5)
    for (path, name) in [([5, 2], "APRS messages"), ([4, 1], "Event log"), ([2, 1], "Channel browser"), ([2, 3], "Zones"), ([1, 7], "Range presets"), ([1, 8], "Skip list"), ([6, 2], "GPS position"), ([6, 7], "Waypoints"), ([4, 5], "Sensor reader"), ([5, 1], "APRS heard")] {
        let mut t = T::new();
        t.hold(Key::Menu);
        t.tap(Key::Digit(path[0]));
        t.tap(Key::Digit(path[1]));
        assert_eq!(t.name(), name, "MENU 0 {path:?}");
        t.tap(Key::Exit);
        assert!(t.name().starts_with("Settings") || t.name() == "Settings group", "{path:?}: BACK returns to the group, not {}", t.name());
    }
    let mut t = T::new();
    t.hold(Key::Menu);
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(1));
    t.tap(Key::Digit(1));
    assert_eq!(t.name(), "FM presets", "MENU 0 3 1 1");
}

#[test]
fn event_log_reads_filters_and_goes() {
    let mut t = T::new();
    t.app.inbox = 3;
    t.open(Dest::EventLog);
    let unread = |s: &Sample| (0..s.events()).filter(|&i| s.event(i).unwrap().unread).count();
    assert_eq!(unread(&t.s), 3);
    t.tap(Key::Down); // the pager message
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Event detail");
    assert_eq!(unread(&t.s), 2, "opening it reads it");
    assert_eq!(t.app.inbox, 2);
    t.tap(Key::Exit);
    assert_eq!(t.name(), "Event log");
    // INBOX-9: filter by source; ▲▼ stay inside it
    for _ in 0..3 {
        t.tap(Key::Right); // APRS, Pager, SAME
    }
    let Screen::Log(l) = t.app.screen() else { panic!() };
    assert_eq!(t.s.event(l.cur.sel).unwrap().src, crate::data::Src::Same);
    t.tap(Key::Down);
    let Screen::Log(l) = t.app.screen() else { panic!() };
    assert_eq!(t.s.event(l.cur.sel).unwrap().src, crate::data::Src::Same, "only one SAME event: it stays");
    // INBOX-10: # Go tunes the active slot to it, and home
    t.cmds();
    t.tap(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Home));
    let f = t.s.event(l.cur.sel).unwrap().freq;
    assert_eq!(t.app.slot(0).freq(), f);
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: f }));
}

#[test]
fn log_options_clear_after_asking() {
    let mut t = T::new();
    t.open(Dest::EventLog);
    t.hold(Key::Menu);
    assert_eq!(t.name(), "Options");
    t.tap(Key::Down);
    t.tap(Key::Menu); // "Clear the log": asks
    assert_eq!(t.s.events(), 12);
    t.tap(Key::Exit); // keeps it
    assert_eq!(t.s.events(), 12);
    t.tap(Key::Menu);
    t.tap(Key::Menu);
    assert_eq!(t.s.events(), 0);
    assert_eq!(t.name(), "Event log", "back on the log");
}

/// CHAN-13, MENU-23: deleting a channel asks first and names it; BACK keeps it.
#[test]
fn browser_deletes_only_after_asking() {
    let mut t = T::new();
    t.open(Dest::Browse);
    t.tap(Key::Down); // Repeaters
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.view == 0 && b.zone == 1));
    t.tap(Key::Down); // Club repeater
    t.tap(Key::Menu); // actions
    t.tap(Key::Up); // Delete (the last)
    t.tap(Key::Menu);
    let n = t.s.count();
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.mode == crate::screens::browse::Mode::Confirm));
    t.tap(Key::Exit);
    assert_eq!(t.s.count(), n, "BACK keeps it");
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.mode == crate::screens::browse::Mode::Actions(6)), "back on Delete");
    t.tap(Key::Menu);
    assert_eq!(t.s.count(), n, "OK on Delete only asks");
    t.tap(Key::Menu);
    assert_eq!(t.s.count(), n - 1);
    assert!(!(0..t.s.zone_len(1)).any(|i| t.s.zone_at(1, i).unwrap().num == 11));
}

#[test]
fn browser_views_and_copy_to_slot() {
    let mut t = T::new();
    t.open(Dest::Browse);
    // the picker: six zones minus none, then All, bands, Recent, Nearby; Up wraps to Nearby
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.view == 7));
    // CHAN-15: nearest first from the fix
    let Screen::Browse(b) = t.app.screen() else { panic!() };
    assert_eq!(b.cur.sel, 0);
    // copy the nearest (Downtown DCS, number 14) to the active slot
    t.tap(Key::Menu);
    for _ in 0..4 {
        t.tap(Key::Down);
    }
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Home));
    let s = t.app.slot(0);
    assert!(s.on_channel());
    assert_eq!(s.ch.unwrap().num, 14);
    // without a fix, Nearby is empty
    t.s.fix.set(None);
    t.open(Dest::Browse);
    t.tap(Key::Up);
    t.tap(Key::Menu);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.mode == crate::screens::browse::Mode::List), "no channel, no actions");
}

#[test]
fn browser_move_skip_renumber() {
    let mut t = T::new();
    t.open(Dest::Browse);
    t.tap(Key::Menu); // Local
    t.tap(Key::Menu); // Calling 2 m: actions
    t.tap(Key::Down);
    t.tap(Key::Down); // Move
    t.tap(Key::Menu);
    t.tap(Key::Right); // Repeaters
    t.tap(Key::Menu);
    assert_eq!(t.s.zone_len(0), 3);
    assert_eq!(t.s.zone_len(1), 6);
    let c = t.s.zone_at(1, 0).unwrap();
    assert_eq!(c.num, 0);
    // skip: on, then off
    t.tap(Key::Menu); // the list now starts with Simplex B; its actions
    for _ in 0..5 {
        t.tap(Key::Down);
    }
    t.tap(Key::Menu);
    assert!(t.s.zone_at(0, 0).unwrap().skip());
    // renumber onto a taken number is refused
    t.tap(Key::Menu);
    for _ in 0..3 {
        t.tap(Key::Down);
    }
    t.tap(Key::Menu);
    t.tap(Key::Left); // 2 - 10: clamps at 0, which "Calling 2 m" has
    t.tap(Key::Menu);
    assert_eq!(t.s.zone_at(0, 0).unwrap().num, 1, "refused: number 0 is taken");
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.mode == crate::screens::browse::Mode::Renumber && b.cur.top == 0));
}

/// CHAN-12: deleting a zone asks, and its channels move to no zone and stay in All.
#[test]
fn zone_delete_keeps_channels() {
    let mut t = T::new();
    t.open(Dest::Zones);
    t.tap(Key::Down); // Repeaters
    let n = t.s.count();
    t.tap(Key::Star);
    t.tap(Key::Exit);
    assert_eq!(t.s.zone_len(1), 5, "BACK keeps it");
    t.tap(Key::Star);
    t.tap(Key::Menu);
    assert_eq!(t.s.zone_len(1), 0);
    assert_eq!(t.s.count(), n);
    assert_eq!(t.s.zone_len(NO_ZONE), 8);
    // # makes a new zone
    let z = t.s.zones();
    t.tap(Key::Hash);
    assert_eq!(t.s.zones(), z + 1);
}

#[test]
fn zone_editor_changes_live_and_resets() {
    let mut t = T::new();
    t.open(Dest::Zones);
    t.tap(Key::Menu); // Local
    assert_eq!(t.name(), "Zone editor");
    t.tap(Key::Down); // Scan all
    assert!(!t.s.zone_cfg(0).scan_all);
    t.app.knob(1, t.t, &t.s);
    assert!(t.s.zone_cfg(0).scan_all, "the knob changes it at once (MENU-16)");
    t.tap(Key::Hash);
    assert!(!t.s.zone_cfg(0).scan_all, "# resets it (MENU-18)");
    for _ in 0..6 {
        t.tap(Key::Down);
    }
    t.tap(Key::Right); // Priority 1: the zone's first channel
    assert_eq!(t.s.zone_cfg(0).pri[0], Some(0));
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Home));
}

/// APRS-8: nothing sends without a callsign; with one, the message shows pending (APRS-13).
#[test]
fn compose_needs_a_callsign() {
    let mut t = T::new();
    t.s.call.set(None);
    t.open(Dest::AprsMessages);
    t.tap(Key::Menu); // the newest: from VE3KPW-9
    assert_eq!(t.name(), "APRS message");
    t.tap(Key::Menu); // reply
    assert_eq!(t.name(), "Compose message");
    for k in [4, 4, 4] {
        t.tap(Key::Digit(k)); // g, h, i on one key
    }
    assert_eq!(t.s.draft().text.as_bytes(), b"i");
    t.wait(1100);
    t.tap(Key::Digit(4)); // a pause: a new letter
    assert_eq!(t.s.draft().text.as_bytes(), b"ig");
    t.tap(Key::Exit); // deletes
    assert_eq!(t.s.draft().text.as_bytes(), b"i");
    let n = t.s.messages();
    t.tap(Key::Menu);
    assert_eq!(t.s.messages(), n, "refused: no callsign");
    assert_eq!(t.name(), "Compose message");
    t.s.call.set(Some(ferrite_ui::text::Buf::from(b"VA7FER")));
    t.tap(Key::Menu);
    assert_eq!(t.s.messages(), n + 1);
    let m = t.s.message(0).unwrap();
    assert_eq!((m.peer.as_bytes(), m.text.as_bytes(), m.state), (&b"VE3KPW-9"[..], &b"i"[..], MsgState::Pending));
    assert_eq!(t.name(), "APRS message", "back where it was written from");
}

#[test]
fn recipient_takes_callsign_characters() {
    let mut t = T::new();
    t.open(Dest::AprsMessages);
    t.tap(Key::Hash); // new message: the recipient first
    assert!(matches!(t.app.screen(), Screen::Compose(c) if c.field == 0));
    t.tap(Key::Digit(8));
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(3));
    t.wait(1100);
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(1)); // "-" (the first of 1's letters a callsign takes)
    t.tap(Key::Digit(9));
    t.tap(Key::Digit(9));
    t.tap(Key::Digit(9));
    t.tap(Key::Digit(9));
    t.tap(Key::Digit(9)); // w x y z 9
    assert_eq!(t.s.draft().to.as_bytes(), b"TED-9");
}

#[test]
fn gps_pages_and_waypoints() {
    let mut t = T::new();
    t.open(Dest::GpsPosition);
    t.tap(Key::Right);
    assert_eq!(t.name(), "GPS satellites");
    let n = t.s.waypoints();
    t.tap(Key::Hash); // GPS-10: mark here
    assert_eq!(t.s.waypoints(), n + 1);
    t.open(Dest::Waypoints);
    let w = t.s.waypoint(0).unwrap();
    assert_eq!(w.pos, t.s.fix().unwrap().pos, "the newest mark first");
    t.tap(Key::Down);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Go to waypoint");
    t.tap(Key::Exit);
    assert_eq!(t.name(), "Waypoints");
    // a station saved as a waypoint (# on the station)
    let mut t = T::new();
    t.open(Dest::AprsHeard);
    t.tap(Key::Menu);
    let n = t.s.waypoints();
    t.tap(Key::Hash);
    assert_eq!(t.s.waypoints(), n + 1);
    assert_eq!(t.s.waypoint(0).unwrap().name.as_bytes(), b"VE3KPW-9");
}

#[test]
fn skips_and_presets() {
    let mut t = T::new();
    t.open(Dest::SkipList);
    t.tap(Key::Menu);
    assert_eq!(t.s.skips(), 2, "OK lets one back in");
    t.tap(Key::Hash);
    assert_eq!(t.s.skips(), 0);
    t.open(Dest::RangePresets);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Range preset");
    t.tap(Key::Down); // Start
    let s0 = t.s.preset(0).unwrap().start;
    t.tap(Key::Right);
    assert_eq!(t.s.preset(0).unwrap().start, s0 + 100_000, "right: 1 MHz");
    t.app.knob(-1, t.t, &t.s);
    assert_eq!(t.s.preset(0).unwrap().start, s0 + 90_000, "knob: 100 kHz");
    // the start never passes the end
    for _ in 0..10 {
        t.tap(Key::Right);
    }
    let p = t.s.preset(0).unwrap();
    assert!(p.start < p.end);
}

#[test]
fn fm_preset_tunes_the_broadcast_receiver() {
    let mut t = T::new();
    t.open(Dest::FmPresets);
    t.tap(Key::Down);
    t.cmds();
    t.tap(Key::Menu);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(ferrite_core::msg::BroadcastCmd::Fm(9150))));
    assert_eq!(t.name(), "FM broadcast");
}

/// Picking a preset from the FM screen returns to that FM screen: one BACK leaves FM broadcast.
#[test]
fn fm_preset_returns_to_the_same_fm_screen() {
    let mut t = T::new();
    t.hold(Key::Digit(2)); // BCR-1
    assert_eq!(t.name(), "FM broadcast");
    for options in [false, true] {
        if options {
            t.hold(Key::Menu); // the FM options: Presets is the fourth row
            t.tap(Key::Digit(4));
        }
        t.tap(Key::Menu);
        assert_eq!(t.name(), "FM presets");
        t.tap(Key::Down);
        t.tap(Key::Menu);
        assert!(matches!(t.app.screen(), Screen::Fm(f) if !f.menu));
    }
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Home), "one BACK leaves FM: {}", t.name());
}

#[test]
fn ptt_and_hold_back_from_every_list() {
    for d in [Dest::EventLog, Dest::AprsHeard, Dest::AprsMessages, Dest::Compose, Dest::Browse, Dest::Zones, Dest::RangePresets, Dest::SkipList, Dest::Waypoints, Dest::GpsPosition, Dest::HfRecent, Dest::FmPresets, Dest::Sensors] {
        let mut t = T::new();
        t.open(d);
        t.hold(Key::Exit);
        assert!(matches!(t.app.screen(), Screen::Home), "{d:?}: hold BACK goes home");
        t.open(d);
        t.app.key_down(Key::Ptt, t.t, &t.s);
        assert!(t.name().starts_with("Transmit"), "{d:?}: PTT transmits (or says why not) from any screen (KEY-7)");
        t.app.key_up(Key::Ptt, t.t + 100, &t.s);
    }
}

/// The hint bar's pairs fit across the screen (KEY-9), on every page of every group C screen.
#[test]
fn hints_fit() {
    let width = |key: &str, label: &str| {
        let arrows = key.bytes().filter(|c| (0x18..=0x1b).contains(c)).count() as i32;
        let text: Vec<u8> = key.bytes().filter(|c| !(0x18..=0x1b).contains(c)).collect();
        text4_width(&text, &UI_12B) + 7 * arrows + if label.is_empty() { 0 } else { 4 + text4_width(label.as_bytes(), &UI_12) }
    };
    let mut t = T::new();
    let check = |t: &mut T| {
        let h = t.app.screen().hints(&t.app);
        let items = h.as_slice();
        let total: i32 = items.iter().map(|(k, l)| width(k, l)).sum::<i32>() + 8 * (items.len() as i32 - 1);
        assert!(total <= 224, "{}: hints {} px wide: {:?}", t.name(), total, items.iter().map(|i| i.1).collect::<Vec<_>>());
    };
    let walks: &[(Dest, &[(Key, u32)])] = &[
        (Dest::EventLog, &[(Key::Menu, 100), (Key::Exit, 100), (Key::Menu, 800), (Key::Down, 100), (Key::Menu, 100)]),
        (Dest::AprsHeard, &[(Key::Menu, 100), (Key::Menu, 800)]),
        (Dest::AprsMessages, &[(Key::Menu, 100), (Key::Menu, 100), (Key::Menu, 100)]),
        (Dest::Browse, &[(Key::Menu, 100), (Key::Menu, 100), (Key::Down, 100), (Key::Down, 100), (Key::Menu, 100), (Key::Exit, 100), (Key::Down, 100), (Key::Menu, 100), (Key::Exit, 100), (Key::Up, 100), (Key::Up, 100), (Key::Up, 100), (Key::Menu, 100)]),
        (Dest::Zones, &[(Key::Star, 100), (Key::Exit, 100), (Key::Menu, 100)]),
        (Dest::RangePresets, &[(Key::Star, 100), (Key::Exit, 100), (Key::Menu, 100)]),
        (Dest::SkipList, &[]),
        (Dest::GpsPosition, &[(Key::Right, 100)]),
        (Dest::Waypoints, &[(Key::Menu, 100)]),
        (Dest::HfRecent, &[]),
        (Dest::FmPresets, &[]),
        (Dest::Sensors, &[]),
    ];
    for &(d, keys) in walks {
        t.open(d);
        check(&mut t);
        for &(k, hold) in keys {
            t.press(k, hold);
            check(&mut t);
        }
        t.hold(Key::Exit);
    }
}

/// Screen state is held five times (the screen and the BACK stack): group C's states stay at eight bytes.
#[test]
fn states_are_small() {
    use core::mem::size_of;
    use crate::screens::*;
    let sizes = [
        ("log", size_of::<log::Log>()),
        ("aprs", size_of::<aprs::Aprs>()),
        ("msgs", size_of::<msgs::Msgs>()),
        ("compose", size_of::<msgs::Compose>()),
        ("gps", size_of::<gps::Gps>()),
        ("browse", size_of::<browse::Browse>()),
        ("zones", size_of::<zones::Zones>()),
        ("scanlists", size_of::<scanlists::ScanLists>()),
        ("recall", size_of::<recall::Recall>()),
        ("sensors", size_of::<sensors::Sensors>()),
        ("options", size_of::<opts::Options>()),
    ];
    std::println!("Screen {} bytes; {sizes:?}", size_of::<Screen>());
    for (name, n) in sizes {
        assert!(n <= 8, "{name}: {n} bytes");
    }
}

/// Text entry writes to the record it was opened for, never the callsign: a channel renamed in the browser,
/// a zone in the zone editor, a range preset in its editor; each returns to the screen that asked.
#[test]
fn rename_goes_to_its_own_record() {
    let call = |t: &T| t.app.settings().callsign;
    // A channel, from the browser's actions (Rename is the second).
    let mut t = T::new();
    let before = call(&t);
    t.open(Dest::Browse);
    t.tap(Key::Menu); // Local
    let num = t.s.zone_at(0, 0).unwrap().num;
    t.tap(Key::Menu); // its actions
    t.tap(Key::Down);
    t.tap(Key::Menu); // Rename
    assert_eq!(t.app.text.as_bytes(), t.s.zone_at(0, 0).unwrap().name(), "starts from the channel's name");
    t.type_ab();
    assert_eq!(t.s.by_num(num).unwrap().0.name(), b"AB");
    assert_eq!(t.name(), "Channel browser");
    assert!(call(&t) == before, "the callsign is untouched");
    // A zone, from the zone editor's first row.
    t.open(Dest::Zones);
    t.tap(Key::Down); // Repeaters
    t.tap(Key::Menu); // its editor, Name row
    t.tap(Key::Menu);
    assert_eq!(t.app.text.as_bytes(), b"Repeaters");
    t.type_ab();
    assert_eq!(t.s.zone_name(1).unwrap().as_bytes(), b"AB");
    assert_eq!(t.s.zone_name(0).unwrap().as_bytes(), b"Local", "only that zone");
    assert_eq!(t.name(), "Zone editor");
    assert!(call(&t) == before);
    // A range preset, from its editor's Name row.
    t.open(Dest::RangePresets);
    t.tap(Key::Down); // the second preset
    t.tap(Key::Menu);
    let other = t.s.preset(0).unwrap().name;
    t.tap(Key::Menu); // Name
    t.type_ab();
    assert_eq!(t.s.preset(1).unwrap().name.as_bytes(), b"AB");
    assert!(t.s.preset(0).unwrap().name == other, "only that preset");
    assert_eq!(t.name(), "Range preset");
    assert!(call(&t) == before);
}

/// MENU-25: hold MENU is the screen's options on every screen that has them, in the one options style (0 the
/// full menu there too); a screen without options says so in a toast and stays.
#[test]
fn hold_menu_is_options_everywhere() {
    use crate::screens::opts::Of;
    let mut t = T::new();
    for (d, of) in [(Dest::EventLog, Of::Log), (Dest::AprsHeard, Of::Heard), (Dest::AprsMessages, Of::Messages), (Dest::GpsPosition, Of::Gps), (Dest::Waypoints, Of::Waypoints), (Dest::BandScope, Of::Scope)] {
        t.open(d);
        let under = t.app.screen();
        t.hold(Key::Menu);
        assert!(matches!(t.app.screen(), Screen::Options(o) if o.of == of), "{d:?}: {}", t.name());
        t.tap(Key::Digit(0)); // MENU-25: the full menu
        assert!(matches!(t.app.screen(), Screen::Advanced(_)), "{d:?}: 0 opens the full menu");
        t.tap(Key::Exit);
        assert!(matches!(t.app.screen(), Screen::Options(_)));
        t.tap(Key::Exit);
        assert_eq!(t.app.screen(), under, "BACK returns to the screen");
        t.hold(Key::Exit);
    }
    t.open(Dest::FmBroadcast); // its own page, drawn in the same style
    t.hold(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Fm(f) if f.menu));
    t.hold(Key::Exit);
    for d in [Dest::Browse, Dest::Zones, Dest::RangePresets, Dest::SkipList, Dest::Sensors, Dest::Quick, Dest::Status] {
        t.open(d);
        let s = t.app.screen();
        t.hold(Key::Menu);
        assert_eq!(t.app.screen(), s, "{d:?}: no options, stays");
        assert!(t.app.toast_up(), "{d:?}: says so");
        t.wait(3100);
        t.hold(Key::Exit);
    }
    // Home keeps its own: hold MENU is advanced settings (MENU-2).
    t.hold(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Advanced(_)));
}
