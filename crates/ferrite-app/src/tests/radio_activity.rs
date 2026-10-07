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
        t.app.key_down(Key::PTT, t.t, &s);
    } else {
        t.app.key_up(Key::PTT, t.t, &s);
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
    t.live.tx = ferrite_core::msg::TxLive { keyed: false, slot: 0, mic: 0, refused: Some(Refusal::Locked) };
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
    tune(&mut t, mhz(406) + 2_500); // EPIRB (243.0 is on the 1.25 m PA path, locked by TX-10 for now)
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
    assert_eq!(crate::screens::transmit::power_name(&t.app, 0), "0.25 W");
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

/// SCAN-16: ▲▼ set the direction; stopped on a hit they leave it at once.
#[test]
fn scan_direction() {
    let mut t = T::new();
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { freq: mhz(146) + 50_000, scanning: true, ..Default::default() };
    t.wait(100);
    t.cmds();
    t.tap(Key::Down);
    assert!(t.cmds().contains(&RadioCmd::ScanDir { slot: 0, up: false, resume: false }));
    t.live.slots[0] = SlotLive { freq: mhz(146) + 94_000, scanning: true, squelch_open: true, rssi: -80, ..Default::default() };
    t.wait(100);
    t.tap(Key::Up);
    assert!(t.cmds().contains(&RadioCmd::ScanDir { slot: 0, up: true, resume: true }));
    assert_eq!(t.freq(), mhz(146) + 52_000, "the arrows don't tune the slot while it scans");
}

/// SCAN-17: # on a channel scan's hit sets that channel's Skip flag for good and goes on; a band scan says why
/// it can't.
#[test]
fn scan_skip_for_good() {
    let mut t = T::new();
    let st = store();
    let z = (0..16).find(|&z| st.zone_len(z) > 1).unwrap();
    t.app.set_channel(0, z, 0, &st);
    let c = st.zone_at(z, 1).unwrap();
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { freq: c.rx, scanning: true, squelch_open: true, rssi: -80, ..Default::default() };
    t.wait(100);
    t.cmds();
    t.tap(Key::Hash);
    match t.app.pop_write() {
        Some(ChanWrite::Put { ch, .. }) => assert!(ch.num == c.num && ch.flags & crate::chan::FLAG_SKIP != 0, "{ch:?}"),
        w => panic!("{w:?}"),
    }
    assert!(t.cmds().contains(&RadioCmd::ScanSkip { slot: 0 }));
    // A band scan (VFO) keeps no list.
    let mut t = T::new();
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { freq: mhz(146) + 94_000, scanning: true, squelch_open: true, ..Default::default() };
    t.wait(100);
    t.tap(Key::Hash);
    assert!(t.app.pop_write().is_none());
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"No skip list here"));
}

/// SCAN-19: PTT while stopped on a hit ends the scan and transmits there; not on a hit, on the slot's own
/// frequency.
#[test]
fn ptt_while_scanning() {
    let mut t = ready();
    let f0 = t.freq();
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { freq: mhz(146) + 94_000, scanning: true, squelch_open: true, rssi: -80, ..Default::default() };
    t.wait(100);
    t.cmds();
    ptt(&mut t, true);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::Scan { slot: 0, on: false }), "{c:?}");
    assert!(c.iter().any(|c| matches!(c, RadioCmd::Transmit { slot: 0, on: true })), "{c:?}");
    assert_eq!(t.freq(), mhz(146) + 94_000, "on the hit");
    ptt(&mut t, false);
    let mut t = ready();
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { freq: mhz(147), scanning: true, ..Default::default() };
    t.wait(100);
    ptt(&mut t, true);
    assert_eq!(t.freq(), f0, "not on a hit: where the scan started");
}

/// SCOPE-1, SCOPE-3: hold 1; the knob moves the centre; OK tunes to the marker; leaving stops the sweep.
#[test]
fn band_scope() {
    let mut t = T::new();
    t.hold(Key::Digit(1));
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
    t.hold(Key::Digit(1));
    t.cmds();
    t.tap(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::Sweep { centre: 0, step: 0, on: false }));
}

/// TSRCH-1 to TSRCH-5: OK sets RX, a second OK TX too; BACK leaves the tones alone.
#[test]
fn tone_search() {
    let mut t = T::new();
    t.open(Dest::ToneSearch);
    assert!(matches!(t.app.screen(), Screen::ToneSearch(_)));
    assert!(t.cmds().contains(&RadioCmd::ToneSearch { slot: 0, on: true }));
    t.tap(Key::Exit); // nothing found: BACK leaves
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::ToneSearch { slot: 0, on: false }));
    t.open(Dest::ToneSearch);
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
    t.open(Dest::ToneSearch);
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
    t.open(Dest::ToneSearch);
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

/// HOME-8: hold 0 puts the active slot on the home channel at the long press; the toast names the channel.
#[test]
fn hold_0_home_channel_toast() {
    let mut t = T::new();
    let s = store();
    let n = s.by_number(0).expect("a channel").num;
    t.app.set.home_ch = Some(n);
    t.app.key_down(Key::Digit(0), t.t, &s);
    t.wait(800);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.app.slot(0).on_channel());
    let toast = t.app.toast.expect("a toast");
    assert_eq!(toast.head.as_bytes(), b"Home channel");
    assert!(!toast.body.is_empty());
    t.app.key_up(Key::Digit(0), t.t, &s);
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
    assert!(matches!(t.app.screen(), Screen::Group(_)), "BACK: one level, to the menu");
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Hf { on: false, .. })));
}

/// BCR-1: MENU 0 3 1 6; the knob tunes; BACK turns it off.
#[test]
fn fm_broadcast() {
    let mut t = T::new();
    t.open(Dest::FmBroadcast);
    assert!(matches!(t.app.screen(), Screen::Fm(_)));
    t.knob(1);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_120))));
    t.tap(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Off)));
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
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "XB-14: local PTT talks");
    assert_eq!(t.app.radio.mode, Mode::Xb, "the repeater keeps running");
    ptt(&mut t, false);
    t.wait(5200);
    assert!(matches!(t.app.screen(), Screen::Crossband(_)), "back to the running repeater: {:?}", t.app.screen());
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
    for d in [Dest::Transmit, Dest::TxRefused, Dest::Emergency, Dest::Scan, Dest::BandScope, Dest::ToneSearch, Dest::Counter, Dest::FmBroadcast, Dest::HfMode, Dest::Alert, Dest::Crossband, Dest::Squelch, Dest::ShortcutMap] {
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

/// A transmit refused for no callsign: OK opens just the callsign page (not the whole setup), and leaving it
/// returns to where the refusal came over.
#[test]
fn no_callsign_fix_is_the_callsign_page() {
    use crate::screens::setup::Setup;
    let mut t = ready();
    crate::screens::transmit::refuse(&mut t.app, 0, Refusal::NoCallsign);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Setup(Setup::single(1)), "the callsign page alone");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home, "back where it was");
}

/// TX-10 (safety): 1.25 m never keys until its 0.25 W ceiling is measured; PTT shows why. Receive is unaffected.
#[test]
fn unmeasured_125m_refuses() {
    let mut t = ready();
    tune(&mut t, mhz(223) + 50_000);
    t.cmds();
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::Unmeasured125));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "never keyed");
    ptt(&mut t, false);
    // An automatic transmission is refused there too.
    assert!(!crate::screens::transmit::auto_tx(&mut t.app, 0, b"Beacon"));
    // 2 m still transmits.
    t.hold(Key::Exit);
    tune(&mut t, mhz(146) + 52_000);
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    ptt(&mut t, false);
}

/// CHAN-4 (safety): a channel with "RX only" on (the channel editor's cell) refuses PTT and automatic
/// transmissions with the reason; the fix opens the channel editor. Off again, it transmits.
#[test]
fn rx_only_channel_refuses() {
    use crate::screens::chedit::ChEdit;
    let mut t = ready();
    t.tap(Key::VfoMem); // channel mode: Calling, 146.520
    assert!(t.app.slot(0).on_channel());
    t.app.open(Dest::ChannelEdit);
    t.app.screen = Screen::ChEdit(ChEdit { cell: 3 + 9 }); // RX only
    t.knob(1);
    assert!(t.app.slot(0).ch.unwrap().flags & crate::chan::FLAG_RX_ONLY != 0, "the editor sets it");
    assert!(matches!(t.app.pop_write(), Some(crate::chan::ChanWrite::Put { ch, .. }) if ch.flags & crate::chan::FLAG_RX_ONLY != 0), "and saves it");
    t.hold(Key::Exit);
    t.cmds();
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::RxOnlyChannel));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "never keyed");
    ptt(&mut t, false);
    assert!(!crate::screens::transmit::auto_tx(&mut t.app, 0, b"Beacon"));
    t.tap(Key::Menu); // the fix: the channel editor
    assert!(matches!(t.app.screen(), Screen::ChEdit(_)));
    t.app.screen = Screen::ChEdit(ChEdit { cell: 3 + 9 });
    t.knob(1);
    t.hold(Key::Exit);
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)), "RX only off: transmits");
    ptt(&mut t, false);
}

/// TX-21: busy lockout. Carrier refuses while the squelch is open; wrong tone refuses only while it is open
/// with another tone (the slot's RX tone not heard); off never refuses.
#[test]
fn busy_lockout_carrier_and_wrong_tone() {
    let mut t = ready();
    let mut cfg = DEFAULT_CFG;
    cfg.rx_tone = 885;
    t.live.slots[0].squelch_open = true;
    for (busy, tone_heard, refuses) in [(0, false, false), (1, true, true), (1, false, true), (2, false, true), (2, true, false)] {
        cfg.busy = busy;
        t.app.set_vfo_cfg(0, cfg);
        t.live.slots[0].tone_heard = tone_heard;
        t.wait(100);
        ptt(&mut t, true);
        assert_eq!(refused(&t) == Some(Refusal::Busy), refuses, "busy {busy}, own tone heard {tone_heard}");
        ptt(&mut t, false);
        t.hold(Key::Exit);
    }
    // Wrong tone with no RX tone set: any carrier is another group's.
    cfg.rx_tone = 0;
    cfg.busy = 2;
    t.app.set_vfo_cfg(0, cfg);
    t.live.slots[0].tone_heard = false;
    t.wait(100);
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::Busy));
    ptt(&mut t, false);
}

/// KEY-7: PTT B transmits on slot B even when A is the active slot; the connector's PTT keys the active slot.
#[test]
fn ptt_b_keys_slot_b() {
    let mut t = ready();
    t.app.slots[1].vfo = 14_652_000;
    t.app.slots[1].vfo_mode = true;
    assert_eq!(t.app.active(), 0);
    let s = store();
    t.app.key_down(Key::Ptt(1), t.t, &s);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 1, on: true }), "PTT B keys slot B");
    t.app.key_up(Key::Ptt(1), t.t + 100, &s);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 1, on: false }));
    // The side connector's PTT keys the active slot (A).
    t.app.key_down(Key::PTT, t.t + 200, &s);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: true }));
    t.app.key_up(Key::PTT, t.t + 300, &s);
}

#[test]
fn transmit_keeps_slot_order() {
    use crate::screens::transmit::{place, HF_SLOT};
    // PTT B: A stays on top as a row, B grows in the middle, C below it.
    assert_eq!(place(1, 0), (25, 23));
    assert_eq!(place(1, 1), (54, 164));
    assert_eq!(place(1, 2), (224, 23));
    // PTT A: the card on top; PTT C: the card at the bottom.
    assert_eq!(place(0, 0), (25, 164));
    assert_eq!(place(2, 2), (83, 164));
    // HF mode: its card on top, A, B, C below.
    assert_eq!(place(HF_SLOT, HF_SLOT), (25, 164));
    assert_eq!(place(HF_SLOT, 0), (195, 23));
    assert!(place(2, 2).0 + 164 <= 300 && place(HF_SLOT, 2).0 + 23 <= 300);
}

/// KEY-11: hold 7 and 8 on a channel open its editor at the Offset cell; on a VFO 7 cycles the shift.
#[test]
fn shift_and_offset_on_a_channel_open_the_editor() {
    let mut t = T::new();
    for d in [7, 8] {
        t.tap(Key::VfoMem);
        assert!(t.app.slot(0).on_channel());
        t.hold(Key::Digit(d));
        match t.app.screen() {
            Screen::ChEdit(e) => assert_eq!(e.cell, crate::screens::chedit::OFFSET_CELL, "hold {d}"),
            s => panic!("hold {d}: {}", s.name()),
        }
        t.hold(Key::Exit);
        t.tap(Key::VfoMem);
        assert!(!t.app.slot(0).on_channel());
    }
    t.hold(Key::Digit(8));
    assert!(matches!(t.app.screen(), Screen::Value(v) if v.item == Item::Offset), "VFO: the offset editor");
}

/// KEY-11: hold 6 (step) on a channel changes nothing and says why (VFO-8).
#[test]
fn step_on_a_channel_is_vfo_only() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    let step = t.app.slot(0).cfg().step;
    t.hold(Key::Digit(6));
    assert_eq!(t.app.slot(0).cfg().step, step);
    assert!(t.app.toast.is_some_and(|x| x.body.as_bytes() == b"VFO only"));
}

/// CHAN-14 / SQL-6: a channel's squelch is Global (the slot's level) or its own 0-9; live changes on a Global
/// channel move the slot's level and leave the channel Global.
#[test]
fn squelch_global_on_a_channel() {
    use crate::slot::SQ_GLOBAL;
    let mut t = ready();
    let s = store();
    // Slot B is on a channel; make its squelch Global and the slot's own level 6.
    t.app.slots[1].cfg.squelch = 6;
    let cfg = crate::slot::SlotCfg { squelch: SQ_GLOBAL, ..crate::slot::DEFAULT_CFG };
    t.app.slots[1].ch = Some(crate::chan::channel(11, 0, 44_347_500, "Club repeater", cfg));
    t.app.slots[1].vfo_mode = false;
    t.app.active = 1;
    assert!(t.app.slots[1].on_channel());
    assert_eq!(t.app.sq_level(1), 6, "Global uses the slot's level");
    let mut b = ferrite_ui::text::Buf::<16>::new();
    t.app.item_text(Item::Squelch, true, &mut b);
    assert_eq!(b.as_bytes(), b"Global (6)");
    t.app.step_squelch(1);
    assert_eq!(t.app.sq_level(1), 7);
    assert_eq!(t.app.slot(1).cfg().squelch, SQ_GLOBAL, "still Global");
    assert!(t.cmds().contains(&RadioCmd::SlotParam { slot: 1, param: Param::Squelch, value: 7 }));
    // The channel editor's knob: Global -> 0 (its own level), and # back to Global.
    t.app.change(Item::Squelch, 1);
    assert_eq!(t.app.slot(1).cfg().squelch, 0);
    t.app.reset(Item::Squelch);
    assert_eq!(t.app.slot(1).cfg().squelch, SQ_GLOBAL);
    // A VFO never takes Global: the knob steps past it.
    t.app.active = 0;
    t.app.slots[0].cfg.squelch = 9;
    t.app.change(Item::Squelch, 1);
    assert_eq!(t.app.slot(0).cfg().squelch, 0, "9 wraps to 0, not Global");
    let _ = &s;
}

/// MENU 5's 4th choice, Split: choosing it asks for the TX frequency (BACK keeps the old shift); RX retunes
/// leave TX where it is; reverse swaps; the TX checks and the power cap go by the TX band.
#[test]
fn split_tx() {
    use crate::screens::entry::EntryFor;
    let mut t = ready();
    tune(&mut t, 44_600_000); // RX 446.000 (70 cm)
    // Turning onto Split (VFO-16) keeps today's TX and stays where it is; turning on leaves it again.
    t.app.change(Item::Offset, 3);
    assert_eq!(t.app.screen(), Screen::Home, "no entry from the knob");
    assert!(t.app.slot(0).cfg().split);
    assert_eq!(super::super::screens::transmit::tx_freq(&t.app, 0), 44_600_000, "today's TX: simplex, so RX");
    t.app.change(Item::Offset, 1);
    assert!(!t.app.slot(0).cfg().split, "on past Split: simplex");
    // OK on Split in the value editor: TX frequency entry, prefilled; BACK there keeps the split as it was.
    t.app.open(crate::screens::nav::Dest::Value(Item::Offset));
    t.knob(-1);
    assert!(matches!(t.app.screen(), Screen::Value(_)), "{:?}", t.app.screen());
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(e) if e.to == EntryFor::SplitTx), "{:?}", t.app.screen());
    let clear = |t: &mut T| {
        while matches!(t.app.screen(), Screen::Entry(e) if e.n > 0) {
            t.tap(Key::Exit); // BACK deletes the prefilled digits
        }
    };
    clear(&mut t);
    t.tap(Key::Exit); // and with none left, leaves: back to the value editor
    assert!(matches!(t.app.screen(), Screen::Value(_)), "{:?}", t.app.screen());
    assert!(t.app.slot(0).cfg().split, "BACK keeps the split");
    // Again, typing 146.52.
    t.tap(Key::Menu);
    clear(&mut t);
    for d in [1, 4, 6, 5, 2] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    let c = *t.app.slot(0).cfg();
    assert!(c.split);
    assert_eq!(super::super::screens::transmit::tx_freq(&t.app, 0), 14_652_000);
    assert!(t.cmds().contains(&RadioCmd::SlotParam { slot: 0, param: Param::Split, value: 14_652_000 }));
    // RX retune: TX stays.
    tune(&mut t, 44_610_000);
    assert_eq!(super::super::screens::transmit::tx_freq(&t.app, 0), 14_652_000);
    // TX goes by its own band: 2 m is allowed in the US with no 1.25 m cap.
    assert_eq!(super::super::screens::transmit::refusal(&t.app, 0), None);
    assert!(!t.app.capped(0));
    // A split TX on 1.25 m (223.5) is capped even though RX is on 2 m.
    tune(&mut t, 14_652_000);
    t.app.slots[0].cfg.offset = 22_350_000;
    assert!(t.app.capped(0), "the cap follows the TX band");
    // Reverse: transmit on RX, listen on the split TX.
    t.app.slots[0].cfg.reverse = true;
    assert_eq!(super::super::screens::transmit::tx_freq(&t.app, 0), 14_652_000);
    assert_eq!(t.app.slot(0).cfg().rx_of(14_652_000), 22_350_000);
    t.app.slots[0].cfg.reverse = false;
    // Hold 7 cycles simplex, +, - only: from split it goes to simplex, never back to split.
    t.app.slots[0].cfg.offset = 14_652_000;
    crate::screens::nav::shortcut(&mut t.app, 7, &store());
    assert!(!t.app.slot(0).cfg().split);
    for _ in 0..4 {
        crate::screens::nav::shortcut(&mut t.app, 7, &store());
        assert!(!t.app.slot(0).cfg().split, "hold 7 never picks split");
    }
}

/// A channel keeps a split through the store's record, and saving it from the slot keeps it.
#[test]
fn split_on_a_channel() {
    let mut t = ready();
    let cfg = crate::slot::SlotCfg { split: true, offset: 14_652_000, ..crate::slot::DEFAULT_CFG };
    t.app.slots[1].ch = Some(crate::chan::channel(20, 0, 44_600_000, "Cross", cfg));
    t.app.slots[1].vfo_mode = false;
    assert_eq!(super::super::screens::transmit::tx_freq(&t.app, 1), 14_652_000);
    let mut b = ferrite_ui::text::Buf::<16>::new();
    crate::slot::shift_text(t.app.slot(1).cfg(), false, &mut b);
    assert_eq!(b.as_bytes(), b"TX 146.520");
    crate::slot::shift_text(t.app.slot(1).cfg(), true, &mut b);
    assert_eq!(b.as_bytes(), b"Split 146.520");
}

/// TX-37: a split or shift whose TX needs the band relay in the other state from RX is refused when it is set,
/// and (a VFO retuned under a split) at PTT. A cross-band pair on the same side of 64 MHz is fine.
#[test]
fn split_across_the_relay() {
    use crate::screens::transmit::refusal;
    let mut t = ready();
    // RX 446.000 / TX 146.520: chip 1 RX, chip 0 TX, no relay: allowed.
    tune(&mut t, mhz(446));
    assert!(t.app.set_split(mhz(146) + 52_000));
    assert_eq!(refusal(&t.app, 0), None);
    // RX 146.520 / TX 50.125: refused when set, the shift is kept as it was, the toast says why.
    t.app.slots[0].cfg = crate::slot::DEFAULT_CFG;
    tune(&mut t, mhz(146) + 52_000);
    assert!(!t.app.set_split(mhz(50) + 12_500));
    assert!(!t.app.slot(0).cfg().split, "not kept");
    assert_eq!(t.app.toast.map(|x| x.head).as_ref().map(|b| b.as_bytes()), Some(&b"Split TX needs relay"[..]));
    // RX 28.400 / TX 146.520: refused too.
    tune(&mut t, mhz(28) + 40_000);
    assert!(!t.app.set_split(mhz(146) + 52_000));
    // Through the entry: OK on a refused frequency keeps the entry open to type another.
    t.app.change(Item::Offset, 3 - Item::Offset.get(t.app.slot(0).cfg()).0); // Split, TX = RX 28.400: allowed
    assert!(t.app.slot(0).cfg().split);
    t.app.open(crate::screens::nav::Dest::Value(Item::Offset));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(_)));
    while matches!(t.app.screen(), Screen::Entry(e) if e.n > 0) {
        t.tap(Key::Exit);
    }
    for d in [1, 4, 6, 5, 2] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "still open: {:?}", t.app.screen());
    assert_eq!(t.app.slot(0).cfg().offset, (mhz(28) + 40_000) as i32, "the split keeps its old TX");
    t.app.go_home();
    // A VFO retuned under an allowed split into a crossing pair: PTT refuses, with the reason.
    t.app.slots[0].cfg = crate::slot::DEFAULT_CFG;
    tune(&mut t, mhz(446));
    assert!(t.app.set_split(mhz(146) + 52_000));
    tune(&mut t, mhz(50) + 12_500);
    t.cmds();
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::RelayMismatch));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "never keyed");
    ptt(&mut t, false);
    // A + shift across 64 MHz (RX 63.900, 6 m's 500 kHz) isn't kept.
    t.app.go_home();
    t.app.slots[0].cfg = crate::slot::DEFAULT_CFG;
    tune(&mut t, mhz(63) + 90_000);
    t.app.change(Item::Offset, 1);
    assert_eq!(t.app.slot(0).cfg().offset, 0, "still simplex");
}

/// HOME-4: channel mode leads with the name; the header carries the short number ("012 · UHF"), and a VFO, a scan
/// or a channel with no name has no name line (its frequency stays large).
#[test]
fn channel_mode_leads_with_the_name() {
    let mut t = ready();
    let cfg = crate::slot::DEFAULT_CFG;
    t.app.set_channel_rec(1, crate::chan::channel(11, 0, 44_347_500, "Club repeater", cfg));
    let mut h = ferrite_ui::text::Buf::<32>::new();
    t.app.head_text(1, &t.live, &mut h);
    assert_eq!(h.as_bytes(), "012 \u{b7} UHF".as_bytes());
    assert_eq!(t.app.chan_name(1, &t.live).as_bytes(), b"Club repeater");
    t.app.set_channel_rec(1, crate::chan::channel(11, 0, 44_347_500, "", cfg));
    assert!(t.app.chan_name(1, &t.live).is_empty(), "no name: the frequency stays large");
    t.app.set_vfo(1, 14_652_000);
    assert!(t.app.chan_name(1, &t.live).is_empty());
    t.app.head_text(1, &t.live, &mut h);
    assert_eq!(h.as_bytes(), "VFO \u{b7} VHF".as_bytes());
    t.app.set_channel_rec(1, crate::chan::channel(11, 0, 44_347_500, "Club repeater", cfg));
    t.live.slots[1].scanning = true;
    assert!(t.app.chan_name(1, &t.live).is_empty(), "a scan shows its frequency");
}

/// The user's path (Oct 5, 2026): hold SLOT on a VFO, the shift cell, OK, the knob onto Split: the value
/// editor stays (no jump into frequency entry), the knob goes on to Simplex and back, and OK on Split opens
/// the TX frequency entry.
#[test]
fn split_in_the_value_editor() {
    use crate::screens::entry::EntryFor;
    let mut t = ready();
    tune(&mut t, 44_600_000);
    t.hold(Key::Slot);
    assert!(matches!(t.app.screen(), Screen::Edit(_)), "{:?}", t.app.screen());
    // The shift cell: Mode, Bandwidth, Tones on the top row, the shift first on the next.
    t.tap(Key::Down);
    assert_eq!(t.app.screen(), Screen::Edit(crate::screens::edit::Edit { cell: 3 }));
    assert_eq!(crate::slot::GRID[3], Item::Offset);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Value(_)), "{:?}", t.app.screen());
    for _ in 0..3 {
        t.knob(1); // simplex -> + -> - -> Split
    }
    assert!(t.app.slot(0).cfg().split);
    assert!(matches!(t.app.screen(), Screen::Value(_)), "the knob on Split stays in the editor: {:?}", t.app.screen());
    t.knob(1);
    assert!(!t.app.slot(0).cfg().split, "on to Simplex");
    t.knob(-1);
    assert!(t.app.slot(0).cfg().split);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(e) if e.to == EntryFor::SplitTx), "{:?}", t.app.screen());
}

/// A scope report: `peak` dB over a -120 dBm floor on bar `at`, sweep number `sweeps`.
fn scope_live(t: &mut T, centre: Freq10, at: usize, peak: u8, sweeps: u8, open: bool, listen: Freq10) {
    let mut bars = [0u8; ferrite_core::msg::SCOPE_BARS];
    for b in bars[..60].iter_mut() {
        *b = 40;
    }
    bars[at] = 40 + peak;
    t.live.mode = ferrite_core::msg::ModeLive::Scope { centre, bars, n: 60, sweeps, listen, open };
    t.wait(100);
}

/// SCOPE-3, SCOPE-12: SIDE1 listens to the marker and PTT then transmits there; PTT while sweeping doesn't.
#[test]
fn scope_listen_and_talk() {
    let mut t = ready();
    t.hold(Key::Digit(1));
    let f = t.freq();
    t.cmds();
    ptt(&mut t, true);
    assert!(matches!(t.app.screen(), Screen::Scope(_)), "PTT while sweeping stays");
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
    ptt(&mut t, false);
    t.tap(Key::Right);
    t.tap(Key::Side(1));
    assert!(t.cmds().contains(&RadioCmd::ScopeListen { freq: f + 2500, mode: 0 }));
    ptt(&mut t, true);
    assert_eq!(t.freq(), f + 2500, "the slot is on the listened frequency");
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
}

/// SCOPE-10: Monitor listens to the strongest after 3 sweeps over the line, and sweeps again once it closes.
#[test]
fn scope_trigger_monitors() {
    let mut t = T::new();
    t.app.set.scope_trig = 1;
    t.hold(Key::Digit(1));
    let c = t.freq();
    t.hold(Key::Digit(3)); // the line at 3/10 of the graph: 33 px, 17 dB
    t.cmds();
    for k in 1..=2 {
        scope_live(&mut t, c, 40, 30, k, false, 0);
    }
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::ScopeListen { .. })), "not before the third sweep");
    scope_live(&mut t, c, 40, 30, 3, false, 0);
    let f = c + 10 * 2500;
    assert!(t.cmds().contains(&RadioCmd::ScopeListen { freq: f, mode: 0 }));
    scope_live(&mut t, c, 40, 30, 3, true, f);
    scope_live(&mut t, c, 40, 30, 3, false, f);
    assert!(t.cmds().contains(&RadioCmd::ScopeListen { freq: 0, mode: 0 }), "squelch closed: sweep again");
}

/// SCOPE-3: # centres on the strongest; a typed centre sweeps there.
#[test]
fn scope_centre_on_strongest_and_typed() {
    let mut t = T::new();
    t.hold(Key::Digit(1));
    let c = t.freq();
    scope_live(&mut t, c, 10, 30, 1, false, 0);
    t.cmds();
    t.tap(Key::Hash);
    let s = c - 20 * 2500;
    assert!(t.cmds().contains(&RadioCmd::Sweep { centre: s, step: 2500, on: true }));
    for k in [Key::Digit(4), Key::Digit(4), Key::Digit(6), Key::Star, Key::Digit(0), Key::Digit(0), Key::Digit(0)] {
        t.tap(k);
    }
    if matches!(t.app.screen(), Screen::Entry(_)) {
        t.tap(Key::Menu);
    }
    assert!(matches!(t.app.screen(), Screen::Scope(_)), "{:?}", t.app.screen());
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Sweep { centre: 44_600_000, on: true, .. })));
}

/// TONE-13, DTMF-7: while keyed, SIDE2 sends the tone burst as long as it is held, and the digits send DTMF live.
#[test]
fn burst_and_dtmf_while_keyed() {
    let mut t = ready();
    ptt(&mut t, true);
    t.cmds();
    t.tap(Key::Side(2));
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::ToneBurst { on: true }) && c.contains(&RadioCmd::ToneBurst { on: false }), "{c:?}");
    t.tap(Key::Digit(5));
    t.tap(Key::Hash);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::Dtmf { key: b'5', on: true }) && c.contains(&RadioCmd::Dtmf { key: b'#', on: false }), "{c:?}");
    ptt(&mut t, false);
    t.cmds();
    t.tap(Key::Digit(5));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Dtmf { .. })), "not keyed: no DTMF");
}
