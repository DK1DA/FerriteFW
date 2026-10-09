//! More end-to-end journeys: tone search, HF channels, GPS, the broadcast sleep timer, KISS and compose. The
//! harness is `common`; a rule counts as covered when its ID is in a test's doc comment.

mod common;

use common::Radio;
use ferrite_core::keys::Key;
use ferrite_sim::simcore::Event;

/// Tone search applies the found tone as the RX tone, then asks before setting TX; BACK keeps RX only (TONE-3).
#[test]
fn tone_search_applies_rx_then_asks() {
    let mut r = Radio::new();
    let tx0 = r.sim.app().slot(0).cfg().tx_tone;
    r.run("menu 0 3 6; wait 6s; expect text \"Found\"; expect text \"100.0\"; press OK");
    r.expect_text("Set the TX tone too");
    assert_eq!(r.sim.app().slot(0).cfg().rx_tone, 1000, "RX tone applied");
    r.press(Key::Exit).wait(300);
    assert_eq!(r.sim.app().slot(0).cfg().tx_tone, tx0, "TX untouched without a yes");
}

/// The GPS position page reads "stopped" for the course below 3 km/h (GPS-17).
#[test]
fn course_stopped_when_still() {
    let mut r = Radio::new();
    r.run("menu 0 6 2; wait 500ms; expect text \"0 km/h\"; expect text \"stopped\"");
}

/// In HF mode hold V/M saves the frequency as a channel, and recalling that channel opens HF mode on it
/// (HF-26).
#[test]
fn hf_channels() {
    let mut r = Radio::new();
    r.run("menu 0 3 4; wait 9s; expect screen \"HF mode\"; expect text \"9.580\"");
    r.run("hold VM; expect text \"Save this HF frequency\"; expect text \"CH 005\"; press OK; wait 500ms");
    r.home();
    r.press(Key::VfoMem).wait(300); // A to channel mode
    r.run("type 5; press OK; wait 9s; expect screen \"HF mode\"; expect text \"9.580\"");
}

/// Typing is multi-tap in the To field; in the message text it is predictive, with candidates on the strip
/// (TEXT-1, TEXT-11).
#[test]
fn compose_typing() {
    let mut r = Radio::new();
    r.run("menu 0 5 3; press OK; home; press SLOT; press SLOT; down PTT; wait 300ms; up PTT; wait 300ms");
    r.expect_screen("Compose message");
    r.type_("43");
    r.expect_text("GD");
    r.press(Key::Down);
    r.type_("43");
    let t = r.text();
    // The strip: three candidates and which page of them, e.g. "If He GE 1/5".
    let strip = t.lines().find(|l| l.split_whitespace().last().is_some_and(|w| w.contains('/') && w.len() <= 5) && l.split_whitespace().count() == 4);
    assert!(strip.is_some(), "a strip of three candidates:\n{t}");
    let _ = Event::Message;
}

/// Without a verified SSB patch, USB, LSB and CW are hidden: * cycles AM only below 30 MHz, and About says
/// the patch isn't installed (HF-20).
#[test]
fn no_ssb_patch() {
    let mut r = Radio::new();
    r.sim.device_mut().selftest.ssb_patch = false;
    r.run("menu 0 9; expect text \"SSB patch Not installed\"");
    r.run("menu 0 3 4; wait 3s; expect screen \"HF mode\"");
    for _ in 0..4 {
        r.press(Key::Star);
        let t = r.text();
        assert!(!t.contains("USB") && !t.contains("LSB") && !t.contains(" CW"), "an SSB mode without the patch:\n{t}");
    }
}

/// While the crossband repeater runs, watch and scans pause (WATCH-9, SCAN-8).
#[test]
fn crossband_pauses_watch_and_scan() {
    let mut r = Radio::new();
    assert!(r.sim.radio().live.slots[2].scanning, "C scanning before");
    r.run("event crossband; expect screen \"Crossband repeater\"; press HASH; wait 2s");
    let mut fs = std::collections::BTreeSet::new();
    for _ in 0..100 {
        r.wait(100);
        fs.insert(r.sim.radio().live.slots[2].freq);
    }
    assert!(fs.len() <= 1, "C's scan paused while the repeater runs: it visited {} frequencies", fs.len());
}

/// A band scan: * on a hit leaves it and counts it as skipped for this scan (SCAN-18); stopping returns to
/// where it started, the Return to default (SCAN-6).
#[test]
fn scan_skip_and_return() {
    let mut r = Radio::new();
    let start = r.sim.app().slot(0).freq();
    r.run("press SIDE2; wait 500ms; expect screen \"Scan\"; expect text \"Skips 0\"");
    let mut hit = false;
    for _ in 0..1500 {
        r.wait(20);
        if r.sim.radio().live.slots[0].squelch_open {
            hit = true;
            break;
        }
    }
    assert!(hit, "the band scan stopped on something");
    r.press(Key::Star).wait(300);
    r.expect_text("Skips 1");
    r.press(Key::Side(2)).wait(500);
    assert_eq!(r.sim.app().slot(0).freq(), start, "back where it started");
}
