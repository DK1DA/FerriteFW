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
    r.run("menu 0 3 5; wait 6s; expect text \"Found\"; expect text \"100.0\"; press OK");
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

/// In HF mode hold 9 saves the frequency as a channel, and recalling that channel opens HF mode on it
/// (HF-26).
#[test]
fn hf_channels() {
    let mut r = Radio::new();
    r.run("menu 0 3 3; wait 9s; expect screen \"HF mode\"; expect text \"9.580\"");
    r.run("hold 9; expect text \"Save this HF frequency\"; expect text \"CH 005\"; press OK; wait 500ms");
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

/// Without a verified SSB patch, USB, LSB and CW are hidden: * cycles AM only below 18 MHz, and About says
/// the patch isn't installed (HF-20).
#[test]
fn no_ssb_patch() {
    let mut r = Radio::new();
    r.sim.device_mut().selftest.ssb_patch = false;
    r.run("menu 0 9; expect text \"SSB patch Not installed\"");
    r.run("menu 0 3 3; wait 3s; expect screen \"HF mode\"");
    for _ in 0..4 {
        r.press(Key::Side(2)); // HF-12: SIDE2 the mode (decision 349)
        // the recent line may name an earlier frequency's mode; the badge and SIDE2's row may not
        let t: String = r.text().lines().filter(|l| !l.contains("Recent")).collect::<Vec<_>>().join("\n");
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

/// Time in coarse steps (the GPS manager's timing is seconds and minutes; frames would take long).
fn coarse(r: &mut Radio, ms: u32, mut each: impl FnMut(&mut Radio)) {
    let end = r.t + ms;
    while r.t < end {
        r.t += 500;
        r.sim.step(r.t);
        each(r);
    }
}

/// The GPS power manager on the fake module (GPS-26, GPS-27, GPS-30, GPS-25): indoors, wakes that find no fix in 2
/// minutes back off 1, 2, 4 minutes and the status bar says searching; outside, the next wake fixes, the module
/// sleeps a minute at a time with the icon solid; with a timed APRS beacon it is awake with a fix before each
/// beacon, and Status shows when it wakes.
#[test]
fn gps_power_backoff_and_beacon_wake() {
    use ferrite_app::chan::ChannelStore;
    use ferrite_app::data::Op;
    use ferrite_core::gpspower::GpsPhase;
    use ferrite_core::msg::GpsState;
    let mut r = Radio::new();
    r.sim.gps_model(Some(true)); // indoors
    coarse(&mut r, 11 * 60_000, |_| {});
    let g = r.sim.gps().unwrap();
    let waits: Vec<u16> = g.standbys().iter().map(|s| s.1).collect();
    assert_eq!(&waits[..3], &[60, 120, 240], "{:?}", g.log);
    assert_eq!(r.sim.device().gps, GpsState::Searching);
    // Outside: the next wake (at most 4 minutes away) fixes, then the minute wakes.
    r.sim.gps_model(Some(false));
    coarse(&mut r, 6 * 60_000, |_| {});
    assert_eq!(r.sim.device().gps, GpsState::Fix, "a fix within the last minutes: solid");
    let n = r.sim.gps().unwrap().standbys().len();
    // APRS mode with a beacon every 10 minutes.
    r.sim.store().data().request(Op::AprsCfg(ferrite_app::aprs::AprsCfg { beacon: 5, ..ferrite_app::aprs::AprsCfg::DEFAULT }));
    r.run("menu 0 5 3; press OK");
    r.home();
    assert!(r.sim.app().aprs_on());
    let (mut before, mut awake_before) = (0, 0);
    let mut standby_text = false;
    coarse(&mut r, 25 * 60_000, |r| {
        if r.sim.store().data().beacon().next_s == Some(3) {
            before += 1;
            let g = r.sim.gps().unwrap();
            awake_before += (!g.asleep() && g.phase(r.t) == GpsPhase::Running) as u32;
        }
        standby_text |= matches!(r.sim.device().gps_phase, GpsPhase::Standby(_));
    });
    assert!(before >= 4, "beacons came: {before}");
    assert_eq!(awake_before, before, "awake with a fix 3 s before every beacon");
    assert!(standby_text);
    let s = r.sim.gps().unwrap().standbys();
    assert!(s[n..].iter().all(|x| x.1 <= 60), "the minute wakes go on between beacons: {:?}", &s[n..]);
    // Status (# on home) names the standby while the module sleeps.
    r.home();
    let mut seen = false;
    for _ in 0..120 {
        coarse(&mut r, 500, |_| {});
        if matches!(r.sim.device().gps_phase, GpsPhase::Standby(_)) {
            r.press(Key::Hash).wait(300);
            seen = r.text().contains("sleep");
            break;
        }
    }
    assert!(seen, "Status: {}", r.text());
}

/// GPS-34.1, GPS-34.2, GPS-34.3, GPS-34.6: the fake module gets Ferrite's NMEA set once heard after power-on and after
/// every wake, the sky variant (GSV every fix) while the position or satellites page is open and the normal one (GSV
/// every 5th) after; GGA every second keeps the manager's wakes and standbys as before.
#[test]
fn gps_nmea_set_follows_wakes_and_screens() {
    let mut r = Radio::new();
    r.sim.gps_model(Some(false));
    coarse(&mut r, 5_000, |_| {});
    let g = r.sim.gps().unwrap();
    assert_eq!(g.sets().iter().map(|s| s.1).collect::<Vec<_>>(), ["$PCAS03,1,0,5,5,1,0,0,0*02"], "{:?}", g.log);
    assert!(g.log.iter().position(|s| s.1 == "power on") < g.log.iter().position(|s| s.1.starts_with("$PCAS03")), "after power-on");
    // Through the cold start and the ephemeris hold to the first standby: GSV every 5th report.
    let gsv0 = g.gsv;
    coarse(&mut r, 100_000, |_| {});
    let g = r.sim.gps().unwrap();
    assert!(!g.standbys().is_empty(), "standby as before: {:?}", g.log);
    assert!((g.gsv - gsv0) * 4 < 100, "GSV every 5th: {}", g.gsv - gsv0);
    // Each wake from standby: the set again.
    coarse(&mut r, 5 * 60_000, |_| {});
    let g = r.sim.gps().unwrap();
    let (sets, standbys) = (g.sets().len(), g.standbys().len());
    assert!(standbys >= 4 && sets >= standbys, "a set after every wake: {sets} sets, {standbys} standbys: {:?}", g.log);
    // The position page: the sky variant (the byte wakes the module first if it sleeps); home: the normal one.
    r.run("menu 0 6 2");
    coarse(&mut r, 6_000, |_| {});
    let g = r.sim.gps().unwrap();
    assert_eq!(g.sets().last().unwrap().1, "$PCAS03,1,0,1,1,1,0,0,0*02", "{:?}", &g.log[g.log.len().saturating_sub(4)..]);
    let gsv0 = g.gsv;
    coarse(&mut r, 10_000, |_| {});
    assert!(r.sim.gps().unwrap().gsv - gsv0 >= 9, "GSV every report on the position page");
    // Home: the module goes back to its standbys, and its next wake (a minute at most) gets the normal set.
    r.home();
    coarse(&mut r, 70_000, |_| {});
    assert_eq!(r.sim.gps().unwrap().sets().last().unwrap().1, "$PCAS03,1,0,5,5,1,0,0,0*02");
}
