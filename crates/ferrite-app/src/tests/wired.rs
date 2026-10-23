//! The settings that showed a value but did nothing until decision 243: menu timeout, pause time, lock covers,
//! busy lockout default, S-meter, channel display, scan direction and return, and the APRS symbol, TX delay and
//! TX level.

use super::*;
use crate::screens::groups::change;

/// MENU-28: 0 7 9 sets how long an idle menu stays up; Off keeps it.
#[test]
fn menu_timeout_follows_its_setting() {
    let mut t = T::new();
    t.app.set.menu_timeout = 1; // 10 s
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Quick(_)));
    t.wait(10_200);
    assert_eq!(t.app.screen(), Screen::Home, "home after 10 s");
    t.app.set.menu_timeout = 0; // Off
    t.tap(Key::Menu);
    t.wait(70_000);
    assert!(matches!(t.app.screen(), Screen::Quick(_)), "Off: the menu stays");
}

/// WATCH-6: # pauses watch for 0 1 6 1's time.
#[test]
fn pause_time_follows_its_setting() {
    let mut t = T::new();
    t.app.set.pause_time = 2; // 5 min
    t.live.slots[1] = SlotLive { squelch_open: true, rssi: -90, ..Default::default() };
    t.wait(100);
    t.tap(Key::Hash);
    assert!(t.cmds().contains(&RadioCmd::PauseWatch(300)));
}

/// KEY-14, TX-18: with Lock covers "Keys and PTT" (0 9 1 2) a locked radio doesn't key; "Keys" lets PTT through.
#[test]
fn lock_covers_ptt() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    t.hold(Key::Hash);
    assert!(t.app.locked());
    t.app.set.lock_ptt = true;
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    assert_eq!(t.app.screen(), Screen::Home, "keys and PTT: no transmit");
    assert!(t.app.toast_up());
    t.app.key_up(Key::PTT, t.t + 300, &s);
    t.app.set.lock_ptt = false;
    t.app.key_down(Key::PTT, t.t + 400, &s);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)), "keys only: PTT works locked");
    t.app.key_up(Key::PTT, t.t + 700, &s);
}

/// TX-21.1: 0 8 7 sets the VFOs' busy lockout at once and a new repeater channel's.
#[test]
fn busy_lockout_default_reaches_the_vfos() {
    let mut t = T::new();
    assert!(change(&mut t.app, &[8, 7], 1));
    assert!(t.app.set.busy_default);
    for s in 0..SLOTS as u8 {
        assert_eq!(t.app.slots[s as usize].cfg.busy, 1, "slot {s}'s VFO");
    }
}

/// SQL-12.3: the level as a number for 0 7 3 1 dBm.
#[test]
fn smeter_as_dbm() {
    assert_eq!(ferrite_ui::widgets::dbm_text(-93).as_bytes(), b"-93 dBm");
    assert_eq!(ferrite_ui::widgets::dbm_text(-200).as_bytes(), b"-140 dBm");
    let mut t = T::new();
    assert!(change(&mut t.app, &[7, 3, 1], 1));
    assert!(t.app.set.smeter_dbm);
    let mut fb = FrameBuffer::new();
    t.render(&mut fb); // draws with dBm, then with bars again
    t.app.set.smeter_dbm = false;
    t.render(&mut fb);
}

/// CHAN-33: 0 2 4 shows a channel by its name, its number or its frequency (nothing in the name's place).
#[test]
fn channel_display() {
    let mut t = T::new();
    if !t.app.slot(0).on_channel() {
        t.tap(Key::VfoMem);
    }
    assert!(t.app.slot(0).on_channel());
    let s = store();
    let name = t.app.card_name(0, &t.live, &s);
    assert!(!name.is_empty(), "a named channel");
    t.app.set.ch_display = 1;
    let n = t.app.card_name(0, &t.live, &s);
    assert!(n.as_bytes().starts_with(b"CH "), "{:?}", n.as_bytes());
    t.app.set.ch_display = 2;
    assert!(t.app.card_name(0, &t.live, &s).is_empty(), "the frequency in its place");
}

/// SCAN-26, SCAN-6: Direction "Down" (0 1 6 2) starts the scan going down; Return to "Last hit" (0 1 6 4) leaves
/// a VFO on the scan's last hit when SIDE2 stops it, "Start" where it started.
#[test]
fn scan_direction_and_return() {
    let mut t = T::new();
    if t.app.slot(0).on_channel() {
        t.tap(Key::VfoMem);
    }
    let start = t.freq();
    t.app.set.scan_down = true;
    t.cmds();
    t.tap(Key::Side(2));
    assert!(t.cmds().contains(&RadioCmd::ScanDir { slot: 0, up: false, resume: false }));
    // a hit, held, then SIDE2
    let hit = start + 2_500;
    t.live.slots[0] = SlotLive { scanning: true, held: true, freq: hit, squelch_open: true, ..Default::default() };
    t.wait(200);
    t.app.set.scan_return_hit = true;
    t.tap(Key::Side(2));
    assert_eq!(t.freq(), hit, "last hit");
    // Start: back where it started
    t.live.slots[0] = SlotLive::default();
    t.wait(200);
    t.app.set.scan_return_hit = false;
    let here = t.freq();
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { scanning: true, held: true, freq: here + 5_000, squelch_open: true, ..Default::default() };
    t.wait(200);
    t.tap(Key::Side(2));
    assert_eq!(t.freq(), here, "start");
}

/// APRS-47, APRS-48, APRS-49: the symbol, TX delay and TX level step and reset in the menu, and the levels are
/// the calibrated 3.0 kHz one's neighbours.
#[test]
fn aprs_symbol_delay_and_level() {
    use crate::aprs::{SYMBOLS, TXDELAY_DEFAULT};
    let mut t = T::new();
    assert_eq!(SYMBOLS[t.app.aprs_cfg().symbol as usize].0, "Person");
    assert_eq!(t.app.aprs_cfg().symbol_chars(), (b'/', b'['), "what beacons always sent");
    change(&mut t.app, &[5, 5, 2], 1);
    assert_eq!(t.app.aprs_cfg().symbol_chars(), (b'/', b'>'), "Car");
    change(&mut t.app, &[5, 9, 4], 2);
    assert_eq!(t.app.aprs_cfg().txdelay_10ms(), TXDELAY_DEFAULT + 10, "50 ms a step");
    change(&mut t.app, &[5, 9, 4], -100);
    assert_eq!(t.app.aprs_cfg().txdelay_10ms(), 10, "100 ms at least");
    change(&mut t.app, &[5, 9, 5], 1);
    assert_eq!(t.app.aprs_cfg().tx_level, 2);
    crate::screens::groups::reset(&mut t.app, &[5, 9, 5]);
    assert_eq!(t.app.aprs_cfg().tx_level, 1);
    let a = ferrite_core::audio::AFSK_AMPS;
    assert!(a[0] < a[1] && a[1] == ferrite_core::audio::AFSK_AMP && a[1] < a[2]);
    assert_eq!(ferrite_core::audio::afsk_tx(300).amp, ferrite_core::audio::afsk_tx_at(300, 1).amp);
}
