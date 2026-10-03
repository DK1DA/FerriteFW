//! Group B's rules (manual chapters 6, 7, 12, 14, 15, 19-23): keys in, screens and Radio task commands out.

use super::*;
use crate::screens::activity::Mode;
use crate::screens::nav::Dest;
use ferrite_core::msg::{BroadcastCmd, ModeLive, XbLive};
use ferrite_core::txplan::{Refusal, Region};

fn ready() -> T {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(Region::Us), false);
    t
}

/// Tune the active slot's VFO.
fn tune(t: &mut T, f: Freq10) {
    let a = t.app.active as usize;
    t.app.slots[a].vfo = f;
    t.app.slots[a].vfo_mode = true;
}

fn ptt(t: &mut T, down: bool) {
    let s = store();
    if down {
        t.app.key_down(Key::Ptt, t.t, &s);
    } else {
        t.app.key_up(Key::Ptt, t.t, &s);
    }
    t.t += 50;
    t.app.tick(t.t, &t.live, &s);
}

/// MENU 0 then the digits.
fn menu0(t: &mut T, path: &[u8]) {
    t.tap(Key::Menu);
    t.tap(Key::Digit(0));
    for &d in path {
        t.tap(Key::Digit(d));
    }
}

fn refused(t: &T) -> Option<Refusal> {
    match t.app.screen() {
        Screen::TxRefused(r) => Some(r.why),
        _ => None,
    }
}

/// TX-22, TX-25: PTT keys the active slot and shows the Transmit screen; PTT up unkeys and goes home.
#[test]
fn ptt_keys_and_unkeys() {
    let mut t = ready();
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: true }));
    t.tap(Key::Menu); // TX-22: MENU does nothing while keyed
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    ptt(&mut t, false);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: false }));
}

/// Hold BACK while keyed goes home, and PTT up still unkeys (TX-25).
#[test]
fn ptt_up_unkeys_wherever_the_screen_went() {
    let mut t = ready();
    ptt(&mut t, true);
    let s = store();
    t.app.key_down(Key::Exit, t.t, &s);
    t.wait(800);
    t.app.key_up(Key::Exit, t.t, &s);
    assert_eq!(t.app.screen(), Screen::Home);
    t.cmds();
    ptt(&mut t, false);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: false }));
}

/// TX-20: out of the region's band: refused with the reason and the fix; OK goes there, BACK closes.
#[test]
fn out_of_band_is_refused_with_the_fix() {
    let mut t = ready();
    t.app.set_region(Some(Region::Canada), false);
    tune(&mut t, mhz(420) + 50_000);
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::OutOfBand));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "never keyed");
    ptt(&mut t, false);
    assert_eq!(refused(&t), Some(Refusal::OutOfBand), "stays until closed");
    t.tap(Key::Menu);
    assert_eq!(t.app.screen().name(), Dest::Group(8).name(), "OK: MENU 0 8 (Extend TX is 0 8 9)");
    t.hold(Key::Exit);
    // Extend TX on: allowed (the calibration windows are the Radio task's).
    t.app.set_region(Some(Region::Canada), true);
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    ptt(&mut t, false);
    // US: 420 is in band.
    t.app.set_region(Some(Region::Us), false);
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
}

#[test]
fn rx_only_build_and_battery_refuse() {
    let mut t = T::new();
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::RxOnlyBuild));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    t.app.radio.tx_built = true;
    t.app.radio.batt_cut = true;
    ptt(&mut t, false);
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::Battery));
}

/// The Radio task's own refusal (calibration, read-back) replaces the Transmit screen.
#[test]
fn radio_refusal_shows() {
    let mut t = ready();
    ptt(&mut t, true);
    t.live.tx = ferrite_core::msg::TxLive { keyed: false, slot: 0, drive: 0, refused: Some(Refusal::Locked) };
    t.wait(100);
    assert_eq!(refused(&t), Some(Refusal::Locked));
}

/// TX-30: unkey at the timeout, home with a toast.
#[test]
fn timeout_unkeys() {
    let mut t = ready();
    ptt(&mut t, true);
    t.cmds();
    t.wait(crate::screens::transmit::TIMEOUT_S * 1000 + 200);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: false }));
}

/// TX-3: on an emergency frequency the first PTT only prompts; a second within 5 s transmits.
#[test]
fn emergency_frequency_needs_ptt_twice() {
    let mut t = ready();
    t.app.set_region(Some(Region::Us), true);
    tune(&mut t, mhz(243));
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Emergency(_)));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
    ptt(&mut t, false);
    assert!(matches!(t.app.screen(), Screen::Emergency(_)), "the first release does nothing");
    t.wait(2000);
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: true }));
    ptt(&mut t, false);
    // Unanswered: home after 5 s.
    ptt(&mut t, true);
    ptt(&mut t, false);
    t.wait(5200);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// TX-35: automatic transmissions are refused on an emergency frequency, with a toast.
#[test]
fn automatic_tx_refused_on_emergency() {
    let mut t = ready();
    t.app.set_region(Some(Region::Us), true);
    t.app.set_callsign(b"VA3ABC");
    assert!(crate::screens::transmit::auto_tx(&mut t.app, 0, b"Beacon"));
    tune(&mut t, mhz(406) + 2_500);
    assert!(!crate::screens::transmit::auto_tx(&mut t.app, 0, b"Beacon"));
    assert!(t.app.toast.is_some_and(|x| x.body.as_bytes() == b"emergency frequency"));
}

/// TX-12: the level name only; 1.25 m shows its cap.
#[test]
fn power_level_names() {
    let mut t = ready();
    assert_eq!(crate::screens::transmit::power_name(&t.app, 0), "LOW");
    tune(&mut t, mhz(223) + 50_000);
    assert_eq!(crate::screens::transmit::power_name(&t.app, 0), "0.25 W cap");
}

/// SCAN-1, SCAN-11: SIDE2 starts and shows the scan; * skips a hit; SIDE2 stops.
#[test]
fn scan_screen() {
    let mut t = T::new();
    t.tap(Key::Side(2));
    assert!(matches!(t.app.screen(), Screen::Scan(_)));
    assert!(t.cmds().contains(&RadioCmd::Scan { slot: 0, on: true }));
    t.live.slots[0] = SlotLive { freq: mhz(146) + 94_000, scanning: true, squelch_open: true, rssi: -80, hits: 1, ..Default::default() };
    t.wait(100);
    assert_eq!(t.app.radio.hits[0].what, mhz(146) + 94_000, "the hit is listed");
    t.tap(Key::Star);
    assert!(t.cmds().contains(&RadioCmd::ScanSkip { slot: 0 }));
    t.tap(Key::Side(2));
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::Scan { slot: 0, on: false }));
}

/// SCOPE-1, SCOPE-3: hold 3; the knob moves the centre; OK tunes to the marker; leaving stops the sweep.
#[test]
fn band_scope() {
    let mut t = T::new();
    t.hold(Key::Digit(3));
    assert!(matches!(t.app.screen(), Screen::Scope(_)));
    let f = t.freq();
    assert!(t.cmds().contains(&RadioCmd::Sweep { centre: f, step: 2500, on: true }));
    t.knob(2);
    assert!(t.cmds().contains(&RadioCmd::Sweep { centre: f + 5000, step: 2500, on: true }));
    t.tap(Key::Right);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.freq(), f + 5000 + 2500, "tuned to the marker");
    assert!(t.cmds().contains(&RadioCmd::Sweep { centre: 0, step: 0, on: false }));
    t.hold(Key::Digit(3));
    t.cmds();
    t.tap(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::Sweep { centre: 0, step: 0, on: false }));
}

/// TSRCH-1 to TSRCH-5: OK sets RX, a second OK TX too; BACK leaves the tones alone.
#[test]
fn tone_search() {
    let mut t = T::new();
    t.hold(Key::Digit(9));
    assert!(matches!(t.app.screen(), Screen::ToneSearch(_)));
    assert!(t.cmds().contains(&RadioCmd::ToneSearch { slot: 0, on: true }));
    t.tap(Key::Exit); // nothing found: BACK leaves
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::ToneSearch { slot: 0, on: false }));
    t.hold(Key::Digit(9));
    t.live.mode = ModeLive::Probe { freq: 0, rssi: -80, tone: 1000 };
    t.wait(100);
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).cfg().rx_tone, 1000);
    assert_eq!(t.app.slot(0).cfg().tx_tone, 0, "TX only after the second confirm");
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).cfg().tx_tone, 1000);
    assert_eq!(t.app.screen(), Screen::Home);
    // BACK at the question: RX only.
    t.live.mode = ModeLive::Probe { freq: 0, rssi: -80, tone: 885 };
    t.hold(Key::Digit(9));
    t.wait(100);
    t.tap(Key::Menu);
    t.tap(Key::Exit);
    assert_eq!((t.app.slot(0).cfg().rx_tone, t.app.slot(0).cfg().tx_tone), (885, 1000));
}

/// TSRCH-9: on a channel the tone is loaded into the slot, not written to the channel.
#[test]
fn tone_search_on_a_channel_is_not_saved() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    assert!(t.app.slot(0).on_channel());
    t.live.mode = ModeLive::Probe { freq: 0, rssi: -80, tone: 1000 };
    t.hold(Key::Digit(9));
    t.wait(100);
    t.tap(Key::Menu);
    t.tap(Key::Exit);
    assert_eq!(t.app.slot(0).cfg().rx_tone, 1000, "loaded");
    assert_eq!(store().zone_at(0, 0).unwrap().cfg.rx_tone, 0, "the channel record unchanged");
    assert!(t.app.toast.is_some_and(|x| x.body.as_bytes() == b"loaded, not saved"));
}

/// TSRCH-6, TSRCH-7: MENU 0 3 4; OK copies to the VFO.
#[test]
fn frequency_counter() {
    let mut t = T::new();
    menu0(&mut t, &[3, 4]);
    assert!(matches!(t.app.screen(), Screen::Counter(_)));
    assert!(t.cmds().contains(&RadioCmd::Counter(true)));
    t.live.mode = ModeLive::Probe { freq: mhz(147) + 15_000, rssi: -40, tone: 0 };
    t.wait(100);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.freq(), mhz(147) + 15_000);
    assert!(t.cmds().contains(&RadioCmd::Counter(false)), "TSRCH-8: it only ever received");
}

/// HOME-8: the map shows while the digit is held; the shortcut runs on release.
#[test]
fn shortcut_map_while_a_digit_is_held() {
    let mut t = T::new();
    let s = store();
    t.app.key_down(Key::Digit(5), t.t, &s);
    t.wait(800);
    assert!(matches!(t.app.screen(), Screen::Shortcut(_)));
    assert_eq!(t.app.slot(0).cfg().power, 0, "not yet");
    t.app.key_up(Key::Digit(5), t.t, &s);
    t.wait(100);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.slot(0).cfg().power, 1, "TX power cycled on release");
}

/// SQL-10: SIDE1 + knob shows the squelch screen while SIDE1 is held.
#[test]
fn squelch_screen_while_side1_is_held() {
    let mut t = T::new();
    let s = store();
    t.app.key_down(Key::Side(1), t.t, &s);
    t.knob(1);
    assert!(matches!(t.app.screen(), Screen::Squelch(_)));
    assert_eq!(t.app.slot(0).cfg().squelch, 5);
    t.app.key_up(Key::Side(1), t.t + 300, &s);
    t.wait(100);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// HF-1, HF-5, HF-24, HF-4.
#[test]
fn hf_mode() {
    let mut t = ready();
    t.app.set_region(Some(Region::Us), false);
    menu0(&mut t, &[3, 5]);
    assert!(matches!(t.app.screen(), Screen::Hf(_)));
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Hf { on: true, .. })));
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    t.app.radio.tx_built = true; // the test's Device is a receive-only build
    let sb = t.app.status_bar(&t.live, &Device::default());
    assert_eq!((sb.watch, sb.scan), (0, false), "HF-5: no TW or SCN");
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::ReceiveOnlyHere), "HF-24: SI4732 frequency");
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Hf(_)), "BACK returns to HF mode");
    ptt(&mut t, false);
    // 10 m FM on chip 1 transmits from HF mode, without going home (HF-23).
    t.app.set.hf.freq = mhz(29) + 60_000;
    t.app.set.hf.mode = crate::screens::hf::FM;
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)), "{:?}", t.app.screen());
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 3, on: true }));
    ptt(&mut t, false);
    assert!(matches!(t.app.screen(), Screen::Hf(_)), "release: back to HF mode");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Hf { on: false, .. })));
}

/// BCR-1: hold 2; the knob tunes; BACK turns it off.
#[test]
fn fm_broadcast() {
    let mut t = T::new();
    t.hold(Key::Digit(2));
    assert!(matches!(t.app.screen(), Screen::Fm(_)));
    t.knob(1);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_120))));
    t.tap(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Off)));
}

/// WX-1: hold 7 opens weather radio in the US and Canada only; leaving restores the slot.
#[test]
fn weather_radio_by_region() {
    let mut t = T::new();
    t.app.set_region(Some(Region::Uk), false);
    t.hold(Key::Digit(7));
    assert_eq!(t.app.screen(), Screen::Home, "nothing outside the US and Canada");
    t.app.set_region(Some(Region::Canada), false);
    t.hold(Key::Digit(7));
    assert!(matches!(t.app.screen(), Screen::Weather(_)));
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: mhz(162) + 55_000 }));
    t.tap(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: mhz(146) + 52_000 }), "the slot's own frequency");
}

/// WX-4, WX-6, WX-7: an alert opens over any screen; OK silences, then opens the log; BACK home.
#[test]
fn alert_screen() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.app.notify(b"Tornado warning", b"Ottawa", true);
    assert!(matches!(t.app.screen(), Screen::Alert(_)));
    assert!(matches!(t.app.screen(), Screen::Alert(a) if !a.silenced));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Alert(a) if a.silenced));
    assert!(matches!(t.app.screen(), Screen::Alert(_)));
    t.tap(Key::Menu);
    assert_eq!(t.app.screen().name(), "Event log");
}

fn xb_ready(t: &mut T) {
    t.app.set_region(Some(Region::Us), false);
    t.app.set_callsign(b"VA3ABC");
    t.app.open(Dest::Crossband);
}

/// XB-2: refuses saying why; XB-14: BACK stops, local PTT doesn't transmit; leaving stops it.
#[test]
fn crossband_repeater() {
    let mut t = ready();
    t.app.open(Dest::Crossband);
    t.tap(Key::Hash);
    assert_ne!(t.app.radio.mode, Mode::Xb, "no callsign for the ID: refused");
    assert!(t.app.toast.is_some());
    xb_ready(&mut t);
    t.knob(1); // side 1: A -> C, both on chip 0 with B? C is 118.950: VHF chip, B is UHF: fine
    t.knob(-1); // back to A
    t.tap(Key::Hash);
    assert_eq!(t.app.radio.mode, Mode::Xb, "{:?}", t.app.toast.map(|x| std::string::String::from_utf8_lossy(x.body.as_bytes()).into_owned()));
    assert!(t.cmds().contains(&RadioCmd::Crossband(Some(t.app.set.xb))));
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::Crossband), "XB-14");
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
    ptt(&mut t, false);
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Crossband(_)), "back to the running repeater");
    assert_eq!(t.app.radio.mode, Mode::Xb);
    t.live.mode = ModeLive::Xb(XbLive { state: 1, pass_s: 12, ..Default::default() });
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    t.app.radio.tx_built = true; // the test's Device is a receive-only build
    t.tap(Key::Exit);
    assert_ne!(t.app.radio.mode, Mode::Xb, "BACK stops");
    assert!(t.cmds().contains(&RadioCmd::Crossband(None)));
    // Hold BACK while running stops it too.
    t.tap(Key::Hash);
    assert_eq!(t.app.radio.mode, Mode::Xb);
    t.hold(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::Crossband(None)));
    // Same chip on both sides: refused.
    xb_ready(&mut t);
    t.tap(Key::Right);
    t.knob(1); // side 2: B -> C (118.950, chip 0 like A)
    t.tap(Key::Hash);
    assert_ne!(t.app.radio.mode, Mode::Xb, "both sides on one chip");
}

/// HOME-3: the other two layouts draw, keep home's keys, and record activity.
#[test]
fn home_layouts() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.live.slots[0] = SlotLive { freq: t.freq(), squelch_open: true, rssi: -85, ..Default::default() };
    for l in [1, 2] {
        t.app.set_layout(l);
        t.wait(3000);
        t.render(&mut fb);
        assert!(count_px(&fb, 20, 300, |c| c != BG) > 3000, "layout {l} drew");
        t.tap(Key::Up);
        assert_eq!(t.freq(), mhz(146) + 53_250, "keys unchanged");
        t.tap(Key::Down);
    }
    assert!(t.app.radio.act[0] & 0b11 != 0, "slot A heard in the last buckets");
    assert_eq!(t.app.radio.act[1], 0);
    t.app.set_layout(1);
    t.wait(100);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Sweep { on: true, .. })), "the strip sweeps");
}

/// Every group B screen draws, with live data in.
#[test]
fn radio_screens_draw() {
    let mut t = ready();
    let mut fb = FrameBuffer::new();
    t.app.notify(b"Severe thunderstorm warning", b"Ottawa, Gatineau", true);
    for d in [Dest::Transmit, Dest::TxRefused, Dest::Emergency, Dest::Scan, Dest::BandScope, Dest::ToneSearch, Dest::Counter, Dest::FmBroadcast, Dest::HfMode, Dest::Weather, Dest::Alert, Dest::Crossband, Dest::Squelch, Dest::ShortcutMap] {
        t.hold(Key::Exit);
        t.render(&mut fb);
        t.app.open(d);
        t.render(&mut fb);
        assert!(count_px(&fb, 20, 300, |c| c != BG) > 500, "{d:?} drew little");
    }
}

/// The text fonts hold every character group B's screens draw.
#[test]
fn fonts_cover_group_b() {
    use ferrite_ui::assets::{UI_12, UI_12B};
    let sources = [
        include_str!("../screens/activity.rs"),
        include_str!("../screens/parts.rs"),
        include_str!("../screens/transmit.rs"),
        include_str!("../screens/scan.rs"),
        include_str!("../screens/scope.rs"),
        include_str!("../screens/probe.rs"),
        include_str!("../screens/fm.rs"),
        include_str!("../screens/hf.rs"),
        include_str!("../screens/weather.rs"),
        include_str!("../screens/xb.rs"),
        include_str!("../screens/squelch.rs"),
        include_str!("../screens/shortcut.rs"),
        include_str!("../screens/layouts.rs"),
        include_str!("../../../ferrite-core/src/txplan.rs"),
    ];
    for src in sources {
        for s in string_literals(src) {
            for c in s.chars().filter(|c| !('\x18'..='\x1b').contains(c) && *c != '\n') {
                assert!(UI_12.has(c) && UI_12B.has(c), "{c:?} in {s:?} is not in the text fonts");
            }
        }
    }
}
