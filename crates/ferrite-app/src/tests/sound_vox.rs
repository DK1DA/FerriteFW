//! Key beeps, alert tones, the beep volume, settings undo, VOX and the broadcast sleep timer (decision 96).

use super::*;
use crate::screens::nav::Dest;
use crate::{ALERT_HZ, KEY_BEEP_HZ, KEY_BEEP_MS};

fn beeps(v: &[RadioCmd], hz: u16) -> usize {
    v.iter().filter(|c| matches!(c, RadioCmd::Beep { hz: h, .. } if *h == hz)).count()
}

/// SND-3: a key beep on each key press while 0 7 5 1 is on, none while it is off; PTT never beeps. 0 7 5 2 sends
/// the volume to the Radio task.
#[test]
fn key_beep_and_volume() {
    let mut t = T::new();
    t.tap(Key::Up);
    let v = t.cmds();
    assert!(v.contains(&RadioCmd::Beep { hz: KEY_BEEP_HZ, ms: KEY_BEEP_MS }));
    t.tap(Key::PTT);
    assert_eq!(beeps(&t.cmds(), KEY_BEEP_HZ), 0, "PTT doesn't beep");
    assert!(crate::screens::groups::change(&mut t.app, &[7, 5, 1], 1));
    t.tap(Key::Up);
    assert_eq!(beeps(&t.cmds(), KEY_BEEP_HZ), 0, "key beep off");
    crate::screens::groups::change(&mut t.app, &[7, 5, 2], 1);
    assert!(t.cmds().contains(&RadioCmd::BeepVol(3)), "Mid to High");
    crate::screens::groups::change(&mut t.app, &[7, 5, 2], 9);
    assert_eq!(t.app.set.beep_vol, 3, "Low, Mid, High");
}

/// SND-6, INBOX-4: an alert sounds its tone each second until OK (MENU) silences it; with alert tones off it
/// shows silently.
#[test]
fn alert_tone_until_ok() {
    let mut t = T::new();
    t.app.notify(b"NWS", b"Tornado warning", true);
    t.wait(2500);
    assert!(beeps(&t.cmds(), ALERT_HZ) >= 2, "a beep each second");
    t.tap(Key::Menu);
    t.cmds();
    t.wait(3000);
    assert_eq!(beeps(&t.cmds(), ALERT_HZ), 0, "OK silenced it");
    t.app.go_home();
    t.app.set.alert_tones = false;
    t.app.notify(b"NWS", b"Flood warning", true);
    t.wait(3000);
    assert_eq!(t.app.screen().name(), "Alert");
    assert_eq!(beeps(&t.cmds(), ALERT_HZ), 0, "alert tones off: no tone");
}

/// MENU-22: a change in a settings list offers "* Undo" for 5 s, and * puts the radio-wide setting back, a run
/// of changes included.
#[test]
fn settings_list_undo() {
    let mut t = T::new();
    t.open(Dest::Sub(7, 5));
    t.tap(Key::Down); // Beep volume
    t.knob(-1);
    t.knob(-1);
    assert_eq!(t.app.set.beep_vol, 1);
    assert!(t.app.screen().hints(&t.app).as_slice().contains(&("*", "Undo")), "the hint bar offers * Undo");
    t.tap(Key::Star);
    assert_eq!(t.app.set.beep_vol, 2, "back to before the run");
    // A toggle, and the offer ends after 5 s.
    t.tap(Key::Down); // Alert tones
    t.knob(1);
    assert!(!t.app.set.alert_tones);
    t.wait(5200);
    t.tap(Key::Star);
    assert!(!t.app.set.alert_tones, "5 s later there is nothing to undo");
}

/// VOX-1, VOX-13, VOX-4, VOX-5: 0 8 2 is off by default; on, the level and tail go to the Radio task, and speech
/// it reports keys the active slot as PTT A does, until it stops (the tail is the Radio task's).
#[test]
fn vox_keys_the_active_slot() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    assert!(!t.app.set.vox, "off by default");
    crate::screens::groups::change(&mut t.app, &[8, 2, 1], 1);
    crate::screens::groups::change(&mut t.app, &[8, 2, 2], 2);
    assert!(t.cmds().contains(&RadioCmd::Vox { level: 7, tail: 20 }));
    t.live.tx.voice = true;
    t.wait(200);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { slot: 0, on: true })), "speech keys slot A (active)");
    t.live.tx.voice = false;
    t.wait(200);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: false, .. })), "silence unkeys");
    // Off: speech does nothing.
    crate::screens::groups::change(&mut t.app, &[8, 2, 1], 1);
    assert!(t.cmds().contains(&RadioCmd::Vox { level: 0, tail: 20 }));
    t.live.tx.voice = true;
    t.wait(200);
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
}

/// VOX-3 (stock's UI mode 0, 0x08025994): VOX keys only from home. A menu, the quick menu or any other screen
/// tells the Radio task to block it (the detector reset; it samples 1.5 s after home again, `Vox::step`), and
/// speech reported then keys nothing, so MENU 0 8 2 is reachable and VOX can be turned off there. Home again: it
/// keys; its own Transmit screen holds it, and silence still releases it.
#[test]
fn vox_only_from_home() {
    use crate::screens::nav::Dest;
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    crate::screens::groups::change(&mut t.app, &[8, 2, 1], 1);
    t.wait(100);
    assert!(t.app.vox_screen());
    assert!(t.cmds().contains(&RadioCmd::VoxScreen(true)));
    t.tap(Key::Menu);
    assert!(!matches!(t.app.screen(), crate::Screen::Home));
    t.live.tx.voice = true;
    t.wait(200);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::VoxScreen(false)), "{c:?}");
    assert!(!c.iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "a menu: speech keys nothing");
    // The VOX setting itself, reached and turned off, unkeyed throughout.
    t.app.go_home();
    t.open(Dest::Group(8));
    crate::screens::groups::change(&mut t.app, &[8, 2, 1], 1);
    t.wait(200);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::Vox { level: 0, tail: 20 }) && !c.iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
    // On again, home: speech keys, the Transmit screen holds it, silence releases it.
    crate::screens::groups::change(&mut t.app, &[8, 2, 1], 1);
    t.app.go_home();
    t.wait(200);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::VoxScreen(true)));
    assert!(c.iter().any(|c| matches!(c, RadioCmd::Transmit { slot: 0, on: true })), "home: speech keys");
    assert!(t.app.vox_screen(), "its own Transmit screen counts as home");
    t.live.tx.voice = false;
    t.wait(200);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: false, .. })));
    // The quick menu blocks it too.
    t.app.go_home();
    t.hold(Key::Star);
    assert!(t.app.qmenu.is_some());
    assert!(!t.app.vox_screen());
}

/// TX-35, VOX-4 (TX audit F11): VOX never keys an emergency frequency, nor answers its prompt: a toast says why,
/// and speech that comes and goes never acts as the second PTT.
#[test]
fn vox_never_on_an_emergency_frequency() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), true);
    crate::screens::groups::change(&mut t.app, &[8, 2, 1], 1);
    let a = t.app.active as usize;
    (t.app.slots[a].vfo, t.app.slots[a].vfo_mode) = (ferrite_core::freq::mhz(406) + 2_500, true);
    t.cmds();
    for _ in 0..3 {
        t.live.tx.voice = true;
        t.wait(200);
        t.live.tx.voice = false;
        t.wait(200);
    }
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. } | RadioCmd::TxPlan { .. })));
    assert!(!matches!(t.app.screen(), crate::Screen::Emergency(_)), "no prompt for VOX to answer");
}

/// BCR-5: the sleep timer ends the broadcast once its time is up.
#[test]
fn broadcast_sleep_timer() {
    let mut t = T::new();
    crate::screens::groups::change(&mut t.app, &[3, 1, 5], 1); // 15 min
    t.open(Dest::FmBroadcast);
    assert_eq!(t.app.screen().name(), Dest::FmBroadcast.name());
    t.cmds();
    t.wait(14 * 60_000);
    assert_eq!(t.app.screen().name(), Dest::FmBroadcast.name(), "still playing at 14 min");
    t.wait(61_000);
    assert_eq!(t.app.screen(), Screen::Home, "off at 15 min");
    assert!(t.cmds().contains(&RadioCmd::Broadcast(ferrite_core::msg::BroadcastCmd::Off)));
}
