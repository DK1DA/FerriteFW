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
use ferrite_ui::fb::FrameBuffer;
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
    t.open(Dest::AprsHeard); // MENU 0 5 1
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
    for (path, name) in [([5, 2], "APRS messages"), ([4, 1], "Event log"), ([2, 1], "Channel browser"), ([2, 3], "Zones"), ([1, 4], "Range presets"), ([1, 5], "Skip list"), ([6, 2], "GPS position"), ([6, 7], "Waypoints"), ([5, 1], "APRS heard")] {
        let mut t = T::new();
        t.hold(Key::Menu);
        t.tap(Key::Digit(path[0]));
        t.tap(Key::Digit(path[1]));
        assert_eq!(t.name(), name, "MENU 0 {path:?}");
        t.tap(Key::Exit);
        assert!(t.name().starts_with("Settings") || t.name() == "Settings group", "{path:?}: BACK returns to the group, not {}", t.name());
    }
    // MENU 0 3 1 1: the FM screen's options on its first row, the step (BCR-3; decision 258)
    let mut t = T::new();
    t.hold(Key::Menu);
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(1));
    t.tap(Key::Digit(1));
    assert!(matches!(t.app.screen(), Screen::Fm(f) if f.menu && f.row == 0), "MENU 0 3 1 1: {}", t.name());
}

#[test]
fn event_log_reads_filters_and_goes() {
    let mut t = T::new();
    t.app.log_unread = 3;
    t.open(Dest::EventLog);
    let unread = |s: &Sample| (0..s.events()).filter(|&i| s.event(i).unwrap().unread).count();
    assert_eq!(unread(&t.s), 3);
    t.tap(Key::Down); // the pager message
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Event detail");
    assert_eq!(unread(&t.s), 2, "opening it reads it");
    assert_eq!(t.app.log_unread, 2);
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
/// Decision 63: Hide leaves a channel out of the lists; the list's options show hidden ones again.
#[test]
fn browser_hides_channels() {
    let sel = |t: &T| match t.app.screen() {
        Screen::Browse(b) => b.cur.sel,
        s => panic!("{s:?}"),
    };
    let mut t = T::new();
    t.open(Dest::Browse);
    t.tap(Key::Down); // Repeaters
    t.tap(Key::Menu);
    t.tap(Key::Down); // Club repeater
    assert_eq!(sel(&t), 1);
    t.tap(Key::Menu); // actions
    for _ in 0..6 {
        t.tap(Key::Down); // Hide
    }
    t.tap(Key::Menu);
    assert!(t.s.by_num(11).is_some_and(|(c, _)| c.flags & crate::chan::FLAG_HIDDEN != 0), "stored hidden");
    t.tap(Key::Up);
    assert_eq!(sel(&t), 0);
    t.tap(Key::Down);
    assert_eq!(sel(&t), 2, "out of the list: Down steps over it");
    t.hold(Key::Menu); // the list's options
    t.tap(Key::Menu); // Show hidden channels
    assert!(t.app.set.show_hidden);
    t.tap(Key::Up);
    assert_eq!(sel(&t), 1, "shown again");
}

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
    assert!(matches!(t.app.screen(), Screen::Confirm(c) if c.ask == crate::screens::confirm::Ask::DeleteChannel(11)), "the shared confirm (MENU-23)");
    t.tap(Key::Exit);
    assert_eq!(t.s.count(), n, "BACK keeps it");
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.mode == crate::screens::browse::Mode::Actions(7)), "back on Delete");
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
    // the picker: six zones minus none, then All, bands, Broadcast, Recent, Nearby; Up wraps to Nearby
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.view == 8));
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
    t.tap(Key::Slot); // Local's editor (decision 178)
    assert_eq!(t.name(), "Zone editor");
    t.tap(Key::Down); // Scan all
    assert!(!t.s.zone_cfg(0).scan_all);
    t.app.knob(1, t.t, &t.s);
    assert!(t.s.zone_cfg(0).scan_all, "the knob changes it at once (MENU-16)");
    t.tap(Key::Hash);
    assert!(!t.s.zone_cfg(0).scan_all, "# resets it (MENU-18)");
    for _ in 0..5 {
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
    t.app.set.predictive = false; // multi-tap (TEXT-10: hold * switches)
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

/// Two saved stations, channels in the broadcast band (BCR-31): 88.5 in zone Local, 91.5 in no zone.
fn stations(t: &T) {
    use crate::chan::{channel, ChanWrite};
    use crate::slot::DEFAULT_CFG;
    use ferrite_core::freq::mhz;
    t.s.apply(ChanWrite::Put { ch: channel(70, 0, mhz(88) + 50_000, "CBC 1", DEFAULT_CFG), from: None });
    t.s.apply(ChanWrite::Put { ch: channel(71, NO_ZONE, mhz(91) + 50_000, "CBC 2", DEFAULT_CFG), from: None });
}

/// BCR-31.2, BCR-31.3: OK on the FM screen opens the channel browser's Broadcast view, every saved station in any zone;
/// OK there plays one.
#[test]
fn fm_ok_opens_the_saved_stations_and_ok_plays_one() {
    let mut t = T::new();
    stations(&t);
    t.open(Dest::FmBroadcast);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.view == crate::screens::browse::V_BROADCAST), "{}", t.name());
    t.tap(Key::Down);
    t.cmds();
    t.tap(Key::Menu);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(ferrite_core::msg::BroadcastCmd::Fm(9150))));
    assert_eq!(t.name(), "FM broadcast");
}

/// BCR-31.3, BCR-25: with a slot playing broadcast, a saved station plays in that slot's VFO (one FM state).
#[test]
fn fm_station_plays_in_the_broadcast_slot() {
    let mut t = T::new();
    stations(&t);
    t.app.set_vfo(1, 10_050_000);
    t.wait(100);
    assert_eq!(t.app.bcast_slot(), Some(1));
    t.open(Dest::FmBroadcast);
    t.tap(Key::Menu);
    t.tap(Key::Down);
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(1).freq(), 9_150_000);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(ferrite_core::msg::BroadcastCmd::Fm(9150))));
}

/// BCR-31.3: a station picked from the FM screen returns to that FM screen: one BACK leaves FM broadcast.
#[test]
fn fm_station_returns_to_the_same_fm_screen() {
    let mut t = T::new();
    t.open(Dest::FmBroadcast); // BCR-1
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Browse(b) if b.view == crate::screens::browse::V_BROADCAST), "{}", t.name());
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert_eq!(t.name(), "FM broadcast", "BACK twice: the views, then FM");
    stations(&t);
    t.tap(Key::Menu);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Fm(f) if !f.menu));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Home), "one BACK leaves FM: {}", t.name());
}

#[test]
fn ptt_and_hold_back_from_every_list() {
    // Compose is the exception: PTT sends the message there (APRS-18; tests/aprs.rs).
    for d in [Dest::EventLog, Dest::AprsHeard, Dest::AprsMessages, Dest::Browse, Dest::Zones, Dest::RangePresets, Dest::SkipList, Dest::Waypoints, Dest::GpsPosition, Dest::HfRecent] {
        let mut t = T::new();
        t.open(d);
        t.hold(Key::Exit);
        assert!(matches!(t.app.screen(), Screen::Home), "{d:?}: hold BACK goes home");
        t.open(d);
        t.app.key_down(Key::PTT, t.t, &t.s);
        assert!(t.name().starts_with("Transmit"), "{d:?}: PTT transmits (or says why not) from any screen (KEY-7)");
        t.app.key_up(Key::PTT, t.t + 100, &t.s);
    }
}

/// The hint bar's pairs fit across the screen (KEY-9), on every page of every group C screen.
/// KEY-9: the hint bar of the screen up now has at most three pairs and fits the 224 px between the margins.
fn hint_fits(t: &mut T) {
    if let Some(why) = hint_overflow(t) {
        panic!("{why}");
    }
}

fn hint_overflow(t: &mut T) -> Option<std::string::String> {
    let h = t.app.screen().hints(&t.app);
    let items = h.as_slice();
    let total = ferrite_ui::widgets::hint_width(&mut t.fb, items);
    (items.len() > ferrite_ui::widgets::HINT_MAX || total > 224).then(|| std::format!("{}: {} hints, {} px: {:?}", t.name(), items.len(), total, items))
}

/// KEY-9: every screen's hint bar, rendered, fits: each destination in the manual, opened from home, and the
/// sub-pages and modes the walks below reach.
#[test]
fn every_hint_bar_fits() {
    let mut bad = std::collections::BTreeSet::new();
    let mut hint_fits = |t: &mut T| {
        if let Some(why) = hint_overflow(t) {
            bad.insert(why);
        }
    };
    for d in crate::screens::nav::ALL.iter().chain(crate::screens::nav::MENU0.iter().map(|(_, d)| d)) {
        let mut t = T::new();
        hint_fits(&mut t);
        t.open(*d);
        hint_fits(&mut t);
        for k in [Key::Down, Key::Menu] {
            if t.app.screen().modal() {
                break;
            }
            t.tap(k);
            t.wait(3100); // past any toast: the hint bar shows again
            hint_fits(&mut t);
        }
    }
    // Option pages and the lock.
    let mut t = T::new();
    for d in [Dest::FmBroadcast, Dest::HfMode, Dest::EventLog] {
        t.open(d);
        t.wait(3000);
        t.hold(Key::Menu);
        hint_fits(&mut t);
        t.hold(Key::Exit);
    }
    assert!(bad.is_empty(), "hint bars that overflow:\n{}", bad.into_iter().collect::<Vec<_>>().join("\n"));
}

#[test]
fn hints_fit() {
    let mut t = T::new();
    let check = |t: &mut T| hint_fits(t);
    let walks: &[(Dest, &[(Key, u32)])] = &[
        (Dest::EventLog, &[(Key::Menu, 100), (Key::Exit, 100), (Key::Menu, 800), (Key::Down, 100), (Key::Menu, 100)]),
        (Dest::AprsHeard, &[(Key::Menu, 100), (Key::Menu, 800)]),
        (Dest::AprsMessages, &[(Key::Menu, 100), (Key::Menu, 100), (Key::Menu, 100)]),
        (Dest::Browse, &[(Key::Menu, 100), (Key::Menu, 100), (Key::Down, 100), (Key::Down, 100), (Key::Menu, 100), (Key::Exit, 100), (Key::Down, 100), (Key::Menu, 100), (Key::Exit, 100), (Key::Up, 100), (Key::Up, 100), (Key::Up, 100), (Key::Menu, 100)]),
        (Dest::Zones, &[(Key::Star, 100), (Key::Exit, 100), (Key::Slot, 100)]),
        (Dest::RangePresets, &[(Key::Star, 100), (Key::Exit, 100), (Key::Menu, 100)]),
        (Dest::SkipList, &[]),
        (Dest::GpsPosition, &[(Key::Right, 100)]),
        (Dest::Waypoints, &[(Key::Menu, 100)]),
        (Dest::HfRecent, &[]),
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
        ("options", size_of::<opts::Options>()),
    ];
    std::println!("Screen {} bytes; {sizes:?}", size_of::<Screen>());
    for (name, n) in sizes {
        // compose keeps multi-tap timing, predictive entry's pick, case, mark and quick list (TEXT-2) and the
        // cursor (TEXT-12): 14 bytes, still inside the 16 of a Screen
        assert!(n <= if name == "compose" { 14 } else { 8 }, "{name}: {n} bytes");
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
    t.tap(Key::Slot); // its editor, Name row
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
    for (d, of) in [(Dest::EventLog, Of::Log), (Dest::AprsHeard, Of::Heard), (Dest::AprsMessages, Of::Messages), (Dest::GpsPosition, Of::Gps), (Dest::Waypoints, Of::Waypoints), (Dest::BandScope, Of::Scope), (Dest::Browse, Of::Channels)] {
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
    for d in [Dest::Zones, Dest::RangePresets, Dest::SkipList, Dest::Quick, Dest::Status] {
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

// ---- predictive text (TEXT-1 to TEXT-11) ----

/// Compose on a draft to VE3KPW-9: the cursor starts on the text.
fn compose_to(t: &mut T) {
    use crate::data::{Draft, Op};
    let mut to = ferrite_ui::text::Buf::<9>::new();
    to.set(b"VE3KPW-9");
    t.s.request(Op::Draft(Draft { to, ..Draft::default() }));
    t.open(Dest::Compose);
    assert_eq!(t.name(), "Compose message");
}

fn keys(t: &mut T, w: &str) {
    for c in w.bytes() {
        let k = crate::words::key_of(c) - b'0';
        t.tap(Key::Digit(k));
    }
}

fn text(t: &T) -> std::string::String {
    std::string::String::from_utf8(t.s.draft().text.as_bytes().to_vec()).unwrap()
}

/// TEXT-2: one press a letter; the knob moves through words with the same keys; # takes it with a space;
/// a message starts with a capital (TEXT-2's case).
#[test]
fn predictive_words() {
    let mut t = T::new();
    compose_to(&mut t);
    keys(&mut t, "home");
    t.draw();
    assert_eq!(text(&t), "", "the word in progress isn't in the draft yet");
    t.app.knob(2, t.t, &t.s); // good, GD (its short form), home
    t.tap(Key::Hash);
    assert_eq!(text(&t), "Home ");
    keys(&mut t, "my");
    t.tap(Key::Hash);
    assert_eq!(text(&t), "Home my ");
}

/// TEXT-12: ◀▶ move the cursor; words, marks and deletes go in at it; a word being predicted goes in before
/// the cursor moves; ▲▼ still pick words.
#[test]
fn cursor_edits_mid_text() {
    let mut t = T::new();
    compose_to(&mut t);
    keys(&mut t, "home");
    t.app.knob(2, t.t, &t.s);
    t.tap(Key::Hash);
    keys(&mut t, "my");
    t.tap(Key::Hash);
    assert_eq!(text(&t), "Home my ");
    for _ in 0..3 {
        t.tap(Key::Left);
    }
    // before "my ": digits (hold) and a space go in there
    t.hold(Key::Digit(7));
    t.hold(Key::Digit(3));
    t.tap(Key::Hash);
    assert_eq!(text(&t), "Home 73 my ");
    t.tap(Key::Exit); // BACK deletes before the cursor
    assert_eq!(text(&t), "Home 73my ");
    // a word in progress goes in before the cursor moves
    keys(&mut t, "my");
    t.tap(Key::Right);
    assert_eq!(text(&t), "Home 73mymy ");
    for _ in 0..20 {
        t.tap(Key::Right);
    }
    t.hold(Key::Digit(5));
    assert_eq!(text(&t), "Home 73mymy 5", "back at the end");
    // the start: BACK has nothing before the cursor and doesn't leave
    for _ in 0..30 {
        t.tap(Key::Left);
    }
    t.tap(Key::Exit);
    assert_eq!((text(&t).as_str(), t.name()), ("Home 73mymy 5", "Compose message"));
}

/// TEXT-4: a short form follows its word; Q-codes go in in capitals.
#[test]
fn predictive_short_forms() {
    let mut t = T::new();
    compose_to(&mut t);
    keys(&mut t, "received");
    t.app.knob(1, t.t, &t.s);
    t.tap(Key::Hash);
    assert_eq!(text(&t), "QSL ");
    keys(&mut t, "qth");
    t.tap(Key::Hash);
    assert_eq!(text(&t), "QSL QTH ");
}

/// TEXT-4: after the words of a phrase, its last word offers the phrase's short form, which takes their place.
#[test]
fn predictive_phrase_short_forms() {
    let mut t = T::new();
    compose_to(&mut t);
    for w in ["see", "you"] {
        keys(&mut t, w);
        t.tap(Key::Hash);
    }
    assert_eq!(text(&t), "See you ");
    keys(&mut t, "later");
    t.app.knob(1, t.t, &t.s); // later, CUL
    t.draw();
    t.tap(Key::Hash);
    assert_eq!(text(&t), "CUL ");
    keys(&mut t, "later");
    t.app.knob(1, t.t, &t.s);
    t.tap(Key::Hash);
    assert_eq!(text(&t), "CUL later ", "no phrase before it: later is the only candidate");
}

/// TEXT-3: keys that spell nothing aren't guessed; BACK takes the keys back one by one.
#[test]
fn predictive_no_word() {
    let mut t = T::new();
    compose_to(&mut t);
    for _ in 0..5 {
        t.tap(Key::Digit(9));
    }
    t.tap(Key::Hash);
    assert_eq!(text(&t), "");
    assert!(t.app.text.as_bytes() == b"99999", "the keys stay");
    for _ in 0..5 {
        t.tap(Key::Exit);
    }
    assert!(t.app.text.is_empty());
    assert_eq!(t.name(), "Compose message");
}

/// TEXT-2: 1 after a word is a full stop, again the next mark; hold a digit types it; hold * switches to
/// multi-tap (TEXT-10, kept as a setting).
#[test]
fn predictive_marks_digits_and_switch() {
    let mut t = T::new();
    compose_to(&mut t);
    keys(&mut t, "ok");
    t.tap(Key::Digit(1));
    assert_eq!(text(&t), "Ok.");
    t.tap(Key::Digit(1));
    assert_eq!(text(&t), "Ok,");
    t.tap(Key::Hash);
    t.hold(Key::Digit(7));
    t.hold(Key::Digit(3));
    assert_eq!(text(&t), "Ok, 73");
    t.hold(Key::Star);
    assert!(!t.app.set.predictive);
    t.tap(Key::Digit(4));
    assert_eq!(text(&t), "Ok, 73g", "multi-tap now");
}

/// TEXT-5: 67 characters at most; the key that would pass it beeps.
#[test]
fn predictive_limit() {
    use crate::data::Op;
    let mut t = T::new();
    compose_to(&mut t);
    let mut d = t.s.draft();
    d.text.set(&[b'x'; 65]);
    t.s.request(Op::Draft(d));
    keys(&mut t, "ok");
    // Key beeps (SND-3) are 1000 Hz; the refusal beep is the low one.
    let refused = |c: &RadioCmd| matches!(c, RadioCmd::Beep { hz, .. } if *hz != crate::KEY_BEEP_HZ);
    assert!(!t.cmds().iter().any(refused));
    t.tap(Key::Digit(6)); // a third key would pass 67
    assert_eq!(t.app.text.as_bytes(), b"65");
    assert!(t.cmds().iter().any(refused), "refused with a beep");
    t.tap(Key::Hash);
    assert_eq!(text(&t).len(), 67);
}

/// TEXT-6: an empty message offers the quick messages; OK puts one in, OK again sends.
#[test]
fn quick_messages_on_an_empty_message() {
    let mut t = T::new();
    compose_to(&mut t);
    t.app.knob(2, t.t, &t.s);
    t.tap(Key::Menu);
    assert_eq!(text(&t), "On my way");
    t.tap(Key::Menu);
    assert_ne!(t.name(), "Compose message", "sent");
    assert_eq!(t.s.message(0).unwrap().text.as_bytes(), b"On my way");
}

/// TEXT-7: the quick messages are edited from the Messages options; # puts the factory text back.
#[test]
fn quick_messages_edited() {
    let mut t = T::new();
    t.open(Dest::QuickMessages);
    assert_eq!(t.name(), "Quick messages");
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Quick message");
    t.tap(Key::Exit);
    t.tap(Key::Exit); // "73" deleted
    keys(&mut t, "home");
    t.tap(Key::Hash);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Quick messages");
    assert_eq!(t.s.quick(0).unwrap().as_bytes(), b"Good ");
    t.tap(Key::Hash);
    assert!(t.s.quick(0).is_none());
}

/// APRS-50: 0 5 5 3 edits the comment in the message editor: kept as typed, up to 40 characters, the row
/// shows it (its start when long), and an empty one is none.
#[test]
fn beacon_comment_edited() {
    let mut t = T::new();
    for k in [Key::Menu, Key::Digit(0), Key::Digit(5), Key::Digit(5), Key::Digit(3)] {
        t.tap(k);
    }
    assert_eq!(t.name(), "Comment");
    keys(&mut t, "home");
    t.tap(Key::Hash);
    t.tap(Key::Menu);
    assert_ne!(t.name(), "Comment", "OK keeps it");
    assert_eq!(t.app.aprs_cfg().comment_text(), b"Good ");
    let dev = Device::default();
    let mut b = ferrite_ui::text::Buf::<24>::new();
    assert_eq!(crate::screens::groups::wired(&t.app, &dev, &[5, 5, 3], &mut b), Some(true));
    assert_eq!(b.as_bytes(), b"Good ");
    // 39 characters: one more digit goes in, the next is refused
    t.app.set_aprs_comment(&[b'x'; 39]);
    crate::screens::groups::wired(&t.app, &dev, &[5, 5, 3], &mut b);
    assert_eq!(b.as_bytes(), b"xxxxxxxxxxxxxxx...", "its start");
    t.tap(Key::Digit(3));
    assert_eq!(t.name(), "Comment");
    t.hold(Key::Digit(7));
    t.hold(Key::Digit(3));
    assert_eq!(t.app.aprs_cfg().comment_text().len(), 40);
    assert!(t.app.aprs_cfg().comment_text().ends_with(b"x7"));
    for _ in 0..40 {
        t.tap(Key::Exit);
    }
    assert_eq!(t.name(), "Comment", "emptied, still open");
    t.tap(Key::Menu);
    assert!(t.app.aprs_cfg().comment_text().is_empty());
    crate::screens::groups::wired(&t.app, &dev, &[5, 5, 3], &mut b);
    assert_eq!(b.as_bytes(), b"Empty");
}

/// APRS-51: 0 5 5 6 steps Exact, then 1 to 4 digits, stopping at the ends; # resets it.
#[test]
fn ambiguity_steps() {
    use crate::screens::groups::{change, reset, wired};
    let mut t = T::new();
    let dev = Device::default();
    let mut b = ferrite_ui::text::Buf::<24>::new();
    assert_eq!(wired(&t.app, &dev, &[5, 5, 6], &mut b), Some(false));
    assert_eq!(b.as_bytes(), b"Exact");
    change(&mut t.app, &[5, 5, 6], 2);
    wired(&t.app, &dev, &[5, 5, 6], &mut b);
    assert_eq!(b.as_bytes(), b"2 digits");
    change(&mut t.app, &[5, 5, 6], 9);
    assert_eq!(t.app.aprs_cfg().ambiguity, 4);
    reset(&mut t.app, &[5, 5, 6]);
    assert_eq!(t.app.aprs_cfg().ambiguity, 0);
}

/// SCAN-14: the range preset editor refuses an edge that would make the span cross 64 MHz (the band relay),
/// keeping the editor open and the preset as it was (decided Oct 7, 2026).
#[test]
fn preset_span_across_64_mhz_refused() {
    let mut t = T::new();
    t.open(Dest::RangePresets);
    t.tap(Key::Menu); // the first preset, 144-148 MHz
    t.tap(Key::Down); // Start
    // 10 MHz a step: 134, 124 ... 64 are fine (64 is the edge itself), 54 would cross it
    for _ in 0..9 {
        t.app.knob(-100, t.t, &t.s);
    }
    assert_eq!(t.s.preset(0).unwrap().start, ferrite_core::freq::mhz(64), "stopped at the edge");
    assert!(t.app.toast_up(), "says why");
    assert_eq!(t.name(), "Range preset", "the editor stays open");
}

/// CHAN-25: Recent gets a channel when you tune to it, transmit on it, or a slot on it opens squelch; newest first,
/// no duplicates, at most 10 (decided Oct 7, 2026).
#[test]
fn recent_channels() {
    let mut t = T::new();
    let (a, b) = (t.s.by_number(0).unwrap(), t.s.by_number(1).unwrap());
    let put = |t: &mut T, slot: usize, c: crate::chan::Channel| {
        let s = &mut t.app.slots[slot];
        (s.ch, s.vfo_mode) = (Some(c), false);
    };
    put(&mut t, 0, a); // the active slot tuned to a channel
    t.wait(100);
    assert_eq!(t.s.recent(0), Some(a.num));
    put(&mut t, 1, b); // B on another channel opens squelch
    let mut live = LiveAll::default();
    live.slots[1].squelch_open = true;
    t.app.tick(t.t + 100, &live, &t.s);
    assert_eq!((t.s.recent(0), t.s.recent(1)), (Some(b.num), Some(a.num)), "heard: to the front");
    t.app.tick(t.t + 200, &live, &t.s);
    assert_eq!(t.s.recent(2), None, "still open: counted once, no duplicate");
    for n in 0..12u16 {
        t.s.used(100 + n);
    }
    assert_eq!(t.s.recent(0), Some(111));
    assert_eq!(t.s.recent(9), Some(102), "ten kept");
    assert_eq!(t.s.recent(10), None);
}

/// DTMF-12: # names a DTMF memory (up to 10 characters); clearing the memory clears its name.
#[test]
fn dtmf_memory_names() {
    use crate::data::Data;
    let mut t = T::new();
    t.open(Dest::DtmfMemories);
    assert_eq!(t.s.dtmf_name(0).as_bytes(), b"Link on");
    t.tap(Key::Down);
    t.tap(Key::Hash);
    assert_eq!(t.name(), "Text entry");
    t.tap(Key::Digit(4));
    t.tap(Key::Digit(4));
    t.tap(Key::Digit(4)); // I
    t.tap(Key::Menu);
    assert_eq!(t.s.dtmf_name(1).as_bytes(), b"I");
    assert_eq!(t.name(), "DTMF memories", "back to the list");
    t.tap(Key::Star);
    assert!(t.s.dtmf(1).is_none() && t.s.dtmf_name(1).is_empty(), "the name goes with the memory");
    t.tap(Key::Hash);
    assert_eq!(t.name(), "DTMF memories", "an empty memory has nothing to name");
}

/// GPS-21: the waypoints' options add one from a grid square, typed as a callsign is (capitals, * for digits).
#[test]
fn waypoint_from_grid_square() {
    let mut t = T::new();
    t.open(Dest::Waypoints);
    t.hold(Key::Menu);
    assert_eq!(t.name(), "Options");
    t.tap(Key::Digit(2));
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Text entry");
    for k in [3, 3, 3] {
        t.tap(Key::Digit(k)); // F
    }
    t.wait(1100);
    t.tap(Key::Digit(6));
    t.tap(Key::Digit(6)); // N
    t.tap(Key::Star); // digits
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(1));
    assert_eq!(t.app.text.as_bytes(), b"FN31");
    t.tap(Key::Menu);
    let w = t.s.waypoint(0).expect("a waypoint");
    assert_eq!((w.name.as_bytes(), w.src), (&b"FN31"[..], crate::data::WpSrc::Grid));
    assert_eq!((w.pos.lat, w.pos.lon), (415_000_000, -730_000_000));
}

/// APRS-23: a weather station's screen reads its weather fields: wind on the course row, the rest on Weather.
#[test]
fn aprs_weather_station() {
    let mut t = T::new();
    let n = t.s.stations();
    let i = (0..n).find(|&i| t.s.station(i).is_some_and(|s| s.call.as_bytes() == b"VE3OBW")).expect("the WX station");
    let s = t.s.station(i).unwrap();
    let w = crate::aprs::wx(s.comment.as_bytes()).expect("weather");
    assert_eq!((w.dir, w.temp_f, w.hum, w.baro), (Some(270), Some(72), Some(65), Some(10123)));
    t.open(Dest::AprsHeard);
    for _ in 0..i {
        t.tap(Key::Down);
    }
    t.tap(Key::Menu);
    t.draw(); // renders without a panic
}

/// GPS-35: the position editor's From GPS row keeps the current fix as the fixed position (position and altitude);
/// with no fix, or only the fixed position standing in, it says so and changes nothing. APRS-31: 0 5 5 5 opens the
/// same editor, and 0 5 5 4 (Fixed) makes beacons send it even with a fix.
#[test]
fn fixed_position_from_the_gps_fix() {
    let mut t = T::new();
    t.open(Dest::PositionEntry);
    for _ in 0..4 {
        t.tap(Key::Down);
    }
    assert!(matches!(t.app.screen(), Screen::PosEntry(p) if p.row == 4), "{:?}", t.app.screen());
    let f = t.s.fix().unwrap();
    t.tap(Key::Menu);
    assert!(t.app.set.fixed_on);
    assert_eq!((t.app.set.fixed, t.app.set.fixed_alt as i32), (f.pos, f.alt_m), "the fix, kept");
    // no fix, and the fixed position standing in for one: refused, nothing changes
    let was = t.app.set.fixed;
    t.s.fix.set(None);
    t.tap(Key::Menu);
    t.s.fix.set(Some(crate::data::Fix { age_s: crate::gpsdata::FIXED_AGE, pos: crate::data::Pos { lat: 1, lon: 2 }, ..f }));
    t.tap(Key::Menu);
    assert_eq!(t.app.set.fixed, was, "only a real fix is kept");
    // APRS-31: the beacon's source
    let gps = Some(crate::data::Fix { pos: crate::data::Pos { lat: 5, lon: 6 }, ..f });
    assert_eq!(t.app.beacon_fix(gps).unwrap().pos, crate::data::Pos { lat: 5, lon: 6 }, "GPS: the fix");
    t.app.set.aprs_fixed = true;
    let b = t.app.beacon_fix(gps).unwrap();
    assert_eq!((b.pos, b.age_s), (was, crate::gpsdata::FIXED_AGE), "Fixed: the fixed position, even with a fix");
    t.app.set.fixed_on = false;
    assert_eq!(t.app.beacon_fix(gps).unwrap().pos, crate::data::Pos { lat: 5, lon: 6 }, "no fixed position: the fix");
    assert_eq!(crate::screens::nav::menu0(&[5, 5, 5]), Some(Dest::PositionEntry));
}

/// APRS-38, APRS-39: an empty recipient lists saved, recent and heard callsigns and the services; ◀▶ go by
/// group (wrapping), OK takes one; a service's message shows its format and stays in multi-tap.
#[test]
fn address_book_on_an_empty_recipient() {
    let mut t = T::new();
    t.open(Dest::Compose);
    let row = |t: &T| {
        let Screen::Compose(c) = t.app.screen else { panic!("not compose") };
        let b = crate::screens::contacts::Book::new(t.s.data());
        let (g, call, _) = b.row(t.s.data(), c.quick as u16).unwrap();
        (crate::screens::contacts::GROUPS[g], std::string::String::from_utf8(call.as_bytes().to_vec()).unwrap())
    };
    assert_eq!(row(&t), ("Saved", "VA3RTX".into()), "contacts first, in callsign order");
    t.tap(Key::Right);
    assert_eq!(row(&t), ("Recent", "VE3JLH".into()), "saved ones aren't repeated as recent");
    t.tap(Key::Right);
    assert_eq!(row(&t).0, "Heard");
    t.tap(Key::Left);
    t.tap(Key::Left);
    t.tap(Key::Left);
    assert_eq!(row(&t), ("Services", "SMSGTE".into()), "wraps to the services");
    t.tap(Key::Menu);
    assert_eq!(t.s.draft().to.as_bytes(), b"SMSGTE");
    t.tap(Key::Digit(4));
    assert!(t.app.text.is_empty(), "multi-tap: no word predicted");
    assert_eq!(text(&t), "g");
    let Screen::Compose(c) = t.app.screen else { panic!() };
    assert!(c.list & 2 != 0, "a service");
    // BACK to an empty recipient brings the book back; a digit types a callsign instead.
    t.tap(Key::Exit);
    t.tap(Key::Up);
    for _ in 0..6 {
        t.tap(Key::Exit);
    }
    t.open(Dest::Compose);
    t.tap(Key::Digit(8));
    assert_eq!(t.s.draft().to.as_bytes(), b"T");
}

/// APRS-37, APRS-40: # on Contacts adds one (callsign, then name), its options rename and delete it.
#[test]
fn contacts_added_renamed_deleted() {
    use ferrite_ui::text::Buf;
    let mut t = T::new();
    t.open(Dest::Contacts);
    assert_eq!(t.name(), "Contacts");
    t.tap(Key::Hash);
    assert_eq!(t.name(), "Text entry");
    t.app.text = Buf::from(b"ve3");
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Text entry", "refused: not a callsign");
    t.app.text = Buf::from(b"VE3ABC-5");
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Text entry", "now its name");
    t.app.text = Buf::from(b"Bob");
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Contacts");
    assert_eq!(t.s.contact_of(b"VE3ABC-5").unwrap().name.as_bytes(), b"Bob");
    let k = (0..t.s.contacts()).find(|&i| t.s.contact(i).unwrap().call.as_bytes() == b"VE3ABC-5").unwrap();
    assert_eq!(k, 3, "in callsign order");
    for _ in 0..k {
        t.tap(Key::Down);
    }
    t.hold(Key::Menu);
    t.tap(Key::Digit(1)); // Edit name
    t.tap(Key::Menu);
    assert_eq!(t.app.text.as_bytes(), b"Bob", "the name to change");
    t.app.text = Buf::from(b"Robert");
    t.tap(Key::Menu);
    assert_eq!(t.s.contact_of(b"VE3ABC-5").unwrap().name.as_bytes(), b"Robert");
    t.hold(Key::Menu);
    t.tap(Key::Digit(2)); // Delete, confirmed
    t.tap(Key::Menu);
    assert!(t.s.contact_of(b"VE3ABC-5").is_some(), "asks first");
    t.tap(Key::Menu);
    assert!(t.s.contact_of(b"VE3ABC-5").is_none());
    // OK writes the selected one a message.
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Compose message");
}

/// APRS-41: a station's options save it as a contact, its name typed.
#[test]
fn station_saved_as_contact() {
    use ferrite_ui::text::Buf;
    let mut t = T::new();
    t.open(Dest::AprsHeard);
    let call = t.s.station(2).unwrap().call;
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "APRS station");
    t.hold(Key::Menu);
    t.tap(Key::Digit(2)); // Save contact
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Text entry");
    t.app.text = Buf::from(b"Digi");
    t.tap(Key::Menu);
    assert_eq!(t.name(), "APRS station");
    assert_eq!(t.s.contact_of(call.as_bytes()).unwrap().name.as_bytes(), b"Digi");
}

/// GPS-36: the waypoints' options make the selected one the fixed position.
#[test]
fn waypoint_as_fixed_position() {
    let mut t = T::new();
    t.open(Dest::Waypoints);
    t.tap(Key::Down);
    let w = t.s.waypoint(1).unwrap();
    t.hold(Key::Menu);
    t.tap(Key::Digit(4));
    t.tap(Key::Menu);
    assert!(t.app.set.fixed_on && t.app.set.fixed == w.pos);
}

/// APRS-43: a saved contact's name is found for its rows in Heard and Messages; one not saved has none.
#[test]
fn contact_names_in_rows() {
    let t = T::new();
    let d = t.s.data();
    assert_eq!(d.contact_of(b"VE3KPW-9").unwrap().name.as_bytes(), b"Mike");
    assert!(d.station(0).is_some_and(|s| s.call.as_bytes() == b"VE3KPW-9"), "the first heard station is a contact");
    assert!(d.contact_of(b"VE3JLH").is_none());
}

/// SELF-10: About names the licence and the copyright holder.
#[test]
fn about_names_the_licence() {
    let n = crate::screens::info::def(crate::screens::info::Page::About).note;
    assert!(n.contains("Copyright 2026 Alex Harvey") && n.contains("PolyForm Noncommercial 1.0.0"), "{n}");
}

/// MENU-33, MENU-6.1 (decisions 256, 259): no row says only "setting comes next": Background job, Scan target and the
/// Nearby sort are out of the menu, and the rows after them closed up (0 1 2 Resume, 0 1 6 Advanced, 0 2 4
/// Channel display); the FM presets row went too (decision 258: 0 3 1 1 is the step).
#[test]
fn stub_rows_are_out_of_the_menu() {
    use crate::screens::groups::{items, GROUP_ITEMS, SUBS};
    let all = GROUP_ITEMS.iter().flat_map(|g| g.iter()).chain(SUBS.iter().flat_map(|(_, l)| l.iter()));
    for e in all {
        assert!(!["Background job", "Scan target", "Nearby", "Presets"].contains(&e.name()), "{} is still a row", e.name());
    }
    assert_eq!(items(1, 0)[1].name(), "Resume");
    assert_eq!(items(1, 0)[5].name(), "Advanced");
    assert_eq!(items(1, 6)[0].name(), "Pause time");
    assert_eq!(items(2, 0)[3].name(), "Channel display");
    assert_eq!(items(3, 1)[0].name(), "Step");
    assert_eq!(crate::screens::nav::menu0(&[1, 4]), Some(Dest::RangePresets));
    assert_eq!(crate::screens::nav::menu0(&[1, 5]), Some(Dest::SkipList));
}
