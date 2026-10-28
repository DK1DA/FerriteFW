//! The scan screen (chapter 14): a band scan and a channel scan as drawn, the hits and the hit list, its keys.

use super::*;
use crate::chan::ChannelStore;
use crate::screens::scan::{self, Scan};
use ferrite_core::freq::mhz;
use ferrite_core::msg::SlotLive;
use ferrite_ui::text::Buf;

const HIT: u32 = mhz(146) + 52_000;

/// A scan running on the active slot (SIDE2 on home), the Radio task saying so.
fn scanning(t: &mut T) -> u8 {
    t.tap(Key::Side(2));
    assert!(matches!(t.screen(), Screen::Scan(_)), "{}", t.name());
    let a = t.app.active;
    t.live.slots[a as usize] = SlotLive { freq: mhz(145) + 50_000, scanning: true, ..Default::default() };
    t.wait(100);
    a
}

/// One more stop on `f`, heard at `rssi`.
fn stop_on(t: &mut T, a: u8, f: u32, rssi: i16) {
    let l = &mut t.live.slots[a as usize];
    (l.freq, l.last_hit, l.hits, l.squelch_open, l.rssi) = (f, f, l.hits + 1, true, rssi);
    t.wait(1500);
    t.live.slots[a as usize].squelch_open = false;
    t.wait(100);
}

/// SCAN-7, SCAN-23: a band scan shows its band, the state (scanning, RX while it hears, stopped), and its
/// last hits with how long ago, how long and how strong.
#[test]
fn band_scan_as_drawn() {
    let mut t = T::ready();
    let a = scanning(&mut t);
    t.shows("SCAN \u{b7} VHF");
    t.shows("scanning");
    t.shows("None yet");
    t.shows("Band VHF");
    t.reads("hints \"^v\" \"Direction\" \"OK\" \"Hits\" \"SIDE2\" \"Stop\"");
    t.live.slots[a as usize].squelch_open = true;
    t.live.slots[a as usize].rssi = -70;
    t.wait(100);
    t.shows("RX");
    assert!(t.px(25, 117, ferrite_ui::gfx::GREEN) > 50, "the card's border green while it hears");
    t.live.slots[a as usize].squelch_open = false;
    stop_on(&mut t, a, HIT, -70);
    t.shows("146.520");
    t.shows("Last hits");
    t.live.slots[a as usize].scanning = false;
    t.wait(100);
    t.shows("stopped");
}

/// SCAN-16: on a hit, the hint bar offers skip and go on; \u{25b2}\u{25bc} leave it ("Skipped").
#[test]
fn hints_on_a_hit() {
    let mut t = T::ready();
    let a = scanning(&mut t);
    t.live.slots[a as usize].held = true; // stopped on a hit
    t.wait(100);
    t.reads("hints \"*\" \"Skip\" \"^v\" \"Go on\" \"SIDE2\" \"Stop\"");
    t.tap(Key::Up);
    assert_eq!(t.toast(), "Skipped / up");
    t.tap(Key::Hash);
    assert_eq!(t.toast(), "No skip list / * skips it once", "a band scan has no list to keep it in");
    t.live.slots[a as usize].held = false;
    t.tap(Key::Hash);
    assert_eq!(t.toast(), "Nothing to skip / ");
}

/// SCAN-23, SCAN-23.1: OK opens the hits, newest first with their count, length and age; \u{25b2}\u{25bc} move; BACK
/// returns; OK tunes the VFO to one and stops the scan.
#[test]
fn hit_list_tunes_the_vfo() {
    let mut t = T::ready();
    let a = scanning(&mut t);
    stop_on(&mut t, a, HIT, -70);
    stop_on(&mut t, a, mhz(147) + 30_000, -90);
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Scan(Scan { list: true, sel: 0, .. })));
    t.shows("Scan hits");
    t.shows("2 of 16");
    t.shows("147.300");
    t.shows("1x");
    t.shows("OK tunes the VFO there; hold 9 saves it.");
    t.reads("hints \"OK\" \"Tune\" \"Hold 9\" \"Save\" \"BACK\" \"Back\"");
    t.tap(Key::Down);
    assert!(matches!(t.screen(), Screen::Scan(Scan { sel: 1, .. })));
    t.tap(Key::Up);
    t.tap(Key::Up); // wraps
    assert!(matches!(t.screen(), Screen::Scan(Scan { sel: 1, .. })));
    t.tap(Key::Digit(5)); // nothing
    t.tap(Key::Exit);
    assert!(matches!(t.screen(), Screen::Scan(Scan { list: false, .. })));
    t.tap(Key::Menu);
    t.tap(Key::Menu); // the newest
    assert_eq!(t.screen(), Screen::Home);
    assert_eq!(t.app.slot(a).freq(), mhz(147) + 30_000);
    assert_eq!(t.toast(), "VFO set / 147.300");
}

/// KEY-23: hold 9 on a hit opens the save form there.
#[test]
fn hit_list_hold_9_saves() {
    let mut t = T::ready();
    let a = scanning(&mut t);
    stop_on(&mut t, a, HIT, -70);
    t.tap(Key::Menu);
    t.hold(Key::Digit(9));
    assert!(matches!(t.screen(), Screen::Save(_)), "{}", t.name());
    assert_eq!(t.app.slot(a).freq(), HIT);
}

/// SCAN-3, SCAN-7, SCAN-23.2: a channel scan shows its zone (with the zones added), where it is in the zone,
/// the channel it stops on and the hits by channel name; OK on a hit goes to that channel.
#[test]
fn channel_scan_as_drawn() {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    let ch = t.s.zone_at(0, 1).expect("a second channel in zone 0");
    let mut zname = Buf::<16>::new();
    zname.set(t.s.zone_name(0).unwrap().as_bytes());
    let zname = String::from_utf8_lossy(zname.as_bytes()).into_owned();
    scanning(&mut t);
    t.reads("hints \"0-9\" \"Zones\" \"OK\" \"Hits\" \"SIDE2\" \"Stop\"");
    t.shows(&std::format!("Zone {zname}"));
    t.shows("-"); // not on one of its channels
    t.tap(Key::Digit(3));
    assert_eq!(t.toast(), "Zone 3 added / ");
    t.shows(&std::format!("Zone {zname} +3"));
    t.tap(Key::Digit(3));
    assert_eq!(t.toast(), "Zone 3 removed / ");
    stop_on(&mut t, a, ch.rx, -60);
    let name = String::from_utf8_lossy(ch.name()).into_owned();
    t.live.slots[a as usize].squelch_open = true;
    t.wait(100);
    t.shows(&name);
    t.shows(&std::format!("2 of {}", t.s.zone_len(0)));
    t.live.slots[a as usize].held = true;
    t.wait(100);
    t.reads("hints \"*\" \"Skip\" \"#\" \"Skip always\" \"^v\" \"Go on\"");
    t.tap(Key::Menu);
    t.shows(&name);
    t.shows("OK goes to that channel.");
    t.tap(Key::Menu);
    assert_eq!(t.screen(), Screen::Home);
    assert_eq!(t.toast(), std::format!("Channel / {name}"));
}

/// SCAN-6: SIDE2 on home while the slot scans stops it.
#[test]
fn side2_again_stops() {
    let mut t = T::ready();
    let a = scanning(&mut t);
    t.tap(Key::Exit); // home; the scan runs on
    assert_eq!(t.screen(), Screen::Home);
    assert!(t.app.scan_mask & 1 << a != 0);
    t.cmds();
    t.tap(Key::Side(2));
    assert!(t.cmds().contains(&RadioCmd::Scan { slot: a, on: false }));
}

/// SCAN-23.1: a stop's length and a hit's age, as short as they read.
#[test]
fn spans_and_ages() {
    let mut b = Buf::<16>::new();
    for (s, want) in [(0, "<1 s"), (14, "14 s"), (190, "3 min")] {
        scan::span(s, &mut b);
        assert_eq!(b.as_bytes(), want.as_bytes());
    }
    for (ms, want) in [(500, "now"), (12_000, "12 s"), (180_000, "3 min"), (7_200_000, "2 h")] {
        scan::ago(ms, &mut b);
        assert_eq!(b.as_bytes(), want.as_bytes());
    }
}
