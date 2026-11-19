//! Frequency and channel entry (A1, A2, MENU-31): the transmit row by region, the long digits, channel numbers,
//! the frequency settings and range edges, and each target's heading.

use super::*;
use crate::data::Data;
use crate::screens::entry::{self, Entry, EntryFor, FreqSet};
use ferrite_core::freq::mhz;
use ferrite_core::txplan::Region;

fn digits(t: &mut T, ds: &[u8]) {
    for &d in ds {
        t.tap(Key::Digit(d));
    }
}

fn entry(t: &mut T, to: EntryFor) {
    t.app.open_screen(Screen::Entry(Entry::new(to, None)));
    t.draw();
}

/// VFO-12, TX-1: the entry's transmit row: Allowed in the region's bands, Extend TX outside them with it on,
/// Not amateur without, Region not set with none; digits past the third decimal drawn small.
#[test]
fn transmit_row() {
    let mut t = T::ready();
    t.open(Dest::Entry(1));
    assert_eq!(t.name(), "Frequency entry");
    t.reads("title=\"Freq\"");
    digits(&mut t, &[4, 6, 5, 2]);
    t.shows("Allowed");
    t.app.screen = Screen::Entry(Entry::with_freq(EntryFor::Vfo, mhz(420) + 52_525));
    t.app.set_region(Some(Region::Canada), false);
    t.draw();
    t.shows("Not amateur");
    t.shows("420.525");
    t.shows("25");
    t.app.set_region(Some(Region::Canada), true);
    t.draw();
    t.shows("Extend TX");
    t.app.set_region(None, false);
    t.draw();
    t.shows("Region not set");
}

/// A2: channel numbers: the channel's name, zone and frequency; an empty number says so; 0 isn't one.
#[test]
fn channel_numbers() {
    let mut t = T::ready();
    entry(&mut t, EntryFor::Channel);
    assert_eq!(t.name(), "Channel entry");
    t.reads("title=\"Channel\"");
    t.shows("Type the channel number, then OK.");
    t.tap(Key::Digit(0));
    t.shows("Channels are 1 to 4000");
    t.tap(Key::Exit);
    digits(&mut t, &[9, 9, 9]);
    t.shows("empty");
}

/// MENU-31, APRS-15.1: the APRS frequency's entry: what it holds and takes; # with nothing typed puts the
/// region's back.
#[test]
fn aprs_frequency_setting() {
    let mut t = T::ready();
    entry::open_setting(&mut t.app, FreqSet::Aprs);
    t.draw();
    t.shows("APRS frequency, MHz");
    t.shows("64-300 MHz, 5k");
    t.shows("144.390");
    t.reads("hints \"#\" \"Region default\" \"OK\" \"Go\"");
    t.tap(Key::Hash);
    assert!(t.toast().starts_with("Region default / "), "{}", t.toast());
    assert_ne!(t.name(), "Frequency entry");
}

/// MENU-31, SCAN-14: a range preset's end typed: kept when the range runs upward on one side of 64 MHz; across
/// 64 MHz refused; a deleted preset has no value and can't be set.
#[test]
fn range_edges() {
    let mut t = T::ready();
    let p = t.s.preset(0).expect("a sample preset");
    entry::open_setting(&mut t.app, FreqSet::RangeEnd(0));
    t.draw();
    t.shows("Range end, MHz");
    t.shows(&String::from_utf8_lossy(ferrite_ui::widgets::freq3(p.end).as_bytes()));
    let f = p.start + 100_000;
    t.app.screen = Screen::Entry(Entry::with_freq(EntryFor::Setting(FreqSet::RangeEnd(0)), f));
    t.tap(Key::Menu);
    assert_eq!(t.s.preset(0).unwrap().end, f);
    assert_eq!(FreqSet::RangeStart(0).set_setting(&mut t.app, f), Err("Not a setting: a range edge is in the store"));
    // A start under 64 MHz with the end above: refused.
    t.app.open_screen(Screen::Entry(Entry::with_freq(EntryFor::Setting(FreqSet::RangeStart(0)), mhz(50))));
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Can't span 64 MHz / one side only", "the whole head, as the knob's (SCAN-14)");
    // No preset 15.
    entry::open_setting(&mut t.app, FreqSet::RangeEnd(15));
    t.draw();
    t.shows("Now -");
    t.app.screen = Screen::Entry(Entry::with_freq(EntryFor::Setting(FreqSet::RangeEnd(15)), mhz(150)));
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "No preset / deleted meanwhile");
}

/// A4, A10, SCOPE-3: each target's heading and tip.
#[test]
fn headings() {
    let mut t = T::ready();
    for (to, head) in [
        (EntryFor::ChannelFreq, "Channel frequency, MHz"),
        (EntryFor::Repeater, "Repeater output, MHz"),
        (EntryFor::SplitTx, "Split: TX frequency, MHz"),
        (EntryFor::ScopeCentre, "Scope centre, MHz"),
    ] {
        entry(&mut t, to);
        t.shows(head);
    }
    entry(&mut t, EntryFor::SplitTx);
    t.shows("BACK keeps the old shift.");
}

/// CHAN-26: a channel's frequency below 64 MHz is refused here: it is saved in HF mode.
#[test]
fn channel_frequency_below_64() {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    t.app.open_screen(Screen::Entry(Entry::with_freq(EntryFor::ChannelFreq, mhz(29) + 60_000)));
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Below 64 MHz / save it in HF mode");
}

/// VFO-26.5, VFO-26.6: hold 8's ▲ makes the amount +, ▼ makes it -, its Shift and TX rows following; hold 7's
/// editor shows where each choice transmits.
#[test]
fn offset_sign_and_tx_shown() {
    let mut t = T::ready();
    t.tune(14_534_000); // 145.340: the usual shift is down
    t.hold(Key::Digit(8));
    for k in [Key::Digit(5), Key::Up] {
        t.tap(k);
    }
    t.shows("Shift +");
    t.shows("TX 150.340");
    t.tap(Key::Down);
    t.shows("Shift -");
    t.shows("TX 140.340");
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Home));
    assert_eq!(t.app.slot(0).cfg().offset, 500_000, "5 MHz up, as chosen");
    // Hold 7: the choice's TX under it
    t.hold(Key::Digit(7));
    t.shows("TX 150.340");
    t.knob(-1); // to simplex
    t.shows("TX 145.340");
}
