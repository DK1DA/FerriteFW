//! Group B's rules (manual chapters 6, 7, 12, 14, 15, 19-23): keys in, screens and Radio task commands out.

use super::*;
use crate::screens::activity::Mode;
use crate::screens::nav::Dest;
use ferrite_core::msg::{BroadcastCmd, ModeLive, XbCfg, XbLive};
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
    t.live.tx = ferrite_core::msg::TxLive { keyed: false, slot: 0, mic: 0, refused: Some(Refusal::Locked), dtmf_sent: 0, voice: false };
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

/// SCAN-23 (nicFW's smart scan list): a frequency scan counts each stop; OK lists the hits; OK there tunes the
/// VFO, hold V/M saves it.
#[test]
fn scan_hit_list() {
    let mut t = T::new();
    t.tap(Key::Side(2));
    let (a, b) = (mhz(146) + 94_000, mhz(147) + 30_000);
    for (n, f) in [(1, a), (2, b), (3, a), (4, a)] {
        t.live.slots[0] = SlotLive { freq: f, last_hit: f, scanning: true, squelch_open: true, rssi: -80, hits: n, ..Default::default() };
        t.wait(100);
        t.live.slots[0].squelch_open = false;
        t.wait(100);
    }
    let h = t.app.radio.hits;
    assert_eq!((h[0].what, h[0].n, h[1].what, h[1].n, h[2].what), (a, 3, b, 1, 0), "a stopped it three times");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Scan(s) if s.list));
    t.tap(Key::Down);
    t.cmds();
    t.tap(Key::Menu);
    assert_eq!((t.app.screen(), t.freq()), (Screen::Home, b), "OK: the VFO on the second hit");
    assert!(t.cmds().contains(&RadioCmd::Scan { slot: 0, on: false }));
    // hold V/M on a hit: the save form
    t.live.slots[0] = SlotLive { freq: b, ..Default::default() };
    t.wait(100);
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { freq: a, last_hit: a, scanning: true, hits: 1, ..Default::default() };
    t.wait(100);
    t.tap(Key::Menu);
    t.hold(Key::VfoMem);
    assert_eq!((t.app.screen().name(), t.freq()), ("Save as channel", a));
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
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"No skip list"));
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

/// TSRCH-6, TSRCH-7: MENU 0 3 3; OK copies to the VFO.
#[test]
fn frequency_counter() {
    let mut t = T::new();
    menu0(&mut t, &[3, 3]);
    assert!(matches!(t.app.screen(), Screen::Counter(_)));
    assert!(t.cmds().contains(&RadioCmd::Counter(true)));
    t.live.mode = ModeLive::Probe { freq: mhz(147) + 15_000, rssi: -40, tone: 0 };
    t.wait(100);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.freq(), mhz(147) + 15_000);
    assert!(t.cmds().contains(&RadioCmd::Counter(false)), "TSRCH-8: it only ever received");
}

/// TSRCH-10, TSRCH-11: only 64–1300 MHz outside 630–756 counts; # freezes the reading while the list keeps
/// filling, and OK copies the held frequency.
#[test]
fn counter_range_and_hold() {
    let mut t = T::new();
    menu0(&mut t, &[3, 3]);
    for f in [mhz(28) + 40_000, mhz(700), mhz(1_400)] {
        t.live.mode = ModeLive::Probe { freq: f, rssi: -40, tone: 0 };
        t.wait(100);
    }
    assert_eq!(t.app.radio.hits[0].what, 0, "nothing outside the range counts");
    t.live.mode = ModeLive::Probe { freq: mhz(1_296), rssi: -60, tone: 0 };
    t.wait(100);
    t.tap(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Counter(c) if c.hold && c.held == mhz(1_296)));
    t.live.mode = ModeLive::Probe { freq: mhz(446) + 6_250, rssi: -40, tone: 0 };
    t.wait(100);
    assert_eq!(t.app.radio.hits[0].what, mhz(446) + 6_250, "held, it keeps listening");
    t.tap(Key::Menu);
    assert_eq!(t.freq(), mhz(1_296), "OK copies the held reading");
}

/// HOME-8, HOME-16: hold 0 puts the active slot on the home channel when it comes up; the toast names the channel.
#[test]
fn hold_0_home_channel_toast() {
    let mut t = T::new();
    let s = store();
    let n = s.by_number(0).expect("a channel").num;
    t.app.set.home_ch = Some(n);
    t.app.key_down(Key::Digit(0), t.t, &s);
    t.wait(800);
    t.app.key_up(Key::Digit(0), t.t, &s);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.app.slot(0).on_channel());
    let toast = t.app.toast.expect("a toast");
    assert_eq!(toast.head.as_bytes(), b"Home channel");
    assert!(!toast.body.is_empty());
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
    menu0(&mut t, &[3, 4]);
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

/// BCR-1: MENU 0 3 1; the knob tunes; BACK turns it off.
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
    assert_eq!(t.app.set.xb.flags & XbCfg::ID, 0, "ID off by default (decision 94)");
    t.app.set.xb.flags |= XbCfg::ID;
    t.app.open(Dest::Crossband);
    t.tap(Key::Hash);
    assert_ne!(t.app.radio.mode, Mode::Xb, "ID on with no callsign: refused");
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
    t.wait(600); // MODE-4: the strip waits for the frequency to rest 500 ms after the Up/Down above
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

/// TX-20: a slot in AM refuses PTT ("AM is receive only": no AM modulator); the fix is MENU 3, Mode.
#[test]
fn am_is_receive_only() {
    let mut t = ready();
    let mut cfg = DEFAULT_CFG;
    cfg.mode = 2;
    t.app.set_vfo_cfg(0, cfg);
    t.cmds();
    ptt(&mut t, true);
    assert_eq!(refused(&t), Some(Refusal::AmRxOnly));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "never keyed");
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

/// TX-21: busy lockout is on or off, as nicFW: on refuses while the squelch is open, whatever tone it carries;
/// off never refuses.
#[test]
fn busy_lockout_on_off() {
    let mut t = ready();
    let mut cfg = DEFAULT_CFG;
    cfg.rx_tone = 885;
    for (busy, open, refuses) in [(0, true, false), (1, true, true), (1, false, false)] {
        cfg.busy = busy;
        t.app.set_vfo_cfg(0, cfg);
        t.live.slots[0].squelch_open = open;
        t.wait(100);
        ptt(&mut t, true);
        assert_eq!(refused(&t) == Some(Refusal::Busy), refuses, "busy {busy}, squelch open {open}");
        ptt(&mut t, false);
        t.hold(Key::Exit);
    }
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

/// KEY-7 (decision 77): PTT A keys the active slot, as nicFW's PTT does; there is no PTT C.
#[test]
fn ptt_a_keys_the_active_slot() {
    let mut t = ready();
    t.app.slots[2] = t.app.slots[0]; // slot C's sample VFO is airband AM, which never transmits
    t.app.slots[2].vfo = 44_600_000;
    t.app.slots[2].vfo_mode = true;
    t.app.active = 2;
    let s = store();
    t.app.key_down(Key::Ptt(0), t.t, &s);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::Transmit { slot: 2, on: true }), "PTT A keys the active slot: {} {c:?}", t.app.screen().name());
    t.app.key_up(Key::Ptt(0), t.t + 100, &s);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 2, on: false }));
}

/// KEY-22: SIDE2 held alone opens the frequency counter when it comes up; SIDE2 + knob is big steps, never the
/// counter, even when the knob turns after 700 ms.
#[test]
fn side2_held_alone_is_the_counter() {
    let mut t = ready();
    let s = store();
    let t0 = t.t;
    t.app.key_down(Key::Side(2), t0, &s);
    t.app.tick(t0 + 800, &t.live, &s);
    t.app.key_up(Key::Side(2), t0 + 900, &s);
    assert!(matches!(t.app.screen(), Screen::Counter(_)), "{}", t.app.screen().name());
    t.app.key_down(Key::Exit, t0 + 1000, &s);
    t.app.key_up(Key::Exit, t0 + 1050, &s);
    assert!(matches!(t.app.screen(), Screen::Home), "{}", t.app.screen().name());
    t.app.key_down(Key::Side(2), t0 + 2000, &s);
    t.app.tick(t0 + 2800, &t.live, &s);
    t.app.knob(1, t0 + 2900, &s);
    t.app.key_up(Key::Side(2), t0 + 3000, &s);
    assert!(matches!(t.app.screen(), Screen::Home), "{}", t.app.screen().name());
}

#[test]
fn transmit_keeps_slot_order() {
    use crate::screens::transmit::{place, HF_SLOT};
    // PTT B: A stays on top as a row, B grows in the middle, C below it.
    assert_eq!(place(1, 0), (25, 23));
    assert_eq!(place(1, 1), (54, 164));
    assert_eq!(place(1, 2), (224, 23));
    // Slot A keyed: the card on top; slot C keyed: the card at the bottom.
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
    assert_eq!(t.app.toast.map(|x| x.head).as_ref().map(|b| b.as_bytes()), Some(&b"No split TX"[..]));
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
    // DTMF-11: A-D on MENU, ▲, ▼, BACK, and still keyed after.
    for (k, d) in [(Key::Menu, b'A'), (Key::Up, b'B'), (Key::Down, b'C'), (Key::Exit, b'D')] {
        t.tap(k);
        let c = t.cmds();
        assert!(c.contains(&RadioCmd::Dtmf { key: d, on: true }) && c.contains(&RadioCmd::Dtmf { key: d, on: false }), "{c:?}");
    }
    assert!(matches!(t.app.screen(), Screen::Transmit(_)), "{:?}", t.app.screen());
    ptt(&mut t, false);
    t.cmds();
    t.tap(Key::Digit(5));
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Dtmf { .. })), "not keyed: no DTMF");
}

/// WATCH-16: unkeying pauses watch for 30 s of quiet (nicFW's MultiWatch does the same); a key press ends the
/// pause; a longer # pause isn't shortened.
#[test]
fn watch_waits_after_transmit() {
    let mut t = ready();
    ptt(&mut t, true);
    t.cmds();
    ptt(&mut t, false);
    assert!(t.cmds().contains(&RadioCmd::PauseWatch(crate::TX_PAUSE_S)));
    t.tap(Key::Up);
    assert!(t.cmds().contains(&RadioCmd::PauseWatch(0)), "a key resumes watch");
    t.tap(Key::Up);
    assert!(!t.cmds().contains(&RadioCmd::PauseWatch(0)), "only once");
}

/// PIN-5: in channel-only mode, hold digits that change a value are refused; those that open a view work.
#[test]
fn channel_only_refuses_value_shortcuts() {
    let mut t = T::new();
    t.app.set.chan_only = true;
    let before = *t.app.slot(t.app.active()).cfg();
    t.hold(Key::Digit(2)); // power
    assert_eq!(*t.app.slot(t.app.active()).cfg(), before, "power unchanged");
    t.hold(Key::Digit(4)); // watch
    assert_eq!(t.app.watch, 1, "watch unchanged");
    t.hold(Key::Digit(1)); // band scope: a view
    assert!(matches!(t.app.screen(), Screen::Scope(_)));
}

/// XB-19: an alert while the repeater runs is a banner, the repeater keeps running, and the alert screen opens
/// once it stops.
#[test]
fn alert_during_crossband_is_a_banner() {
    let mut t = ready();
    xb_ready(&mut t);
    t.tap(Key::Hash);
    assert_eq!(t.app.radio.mode, Mode::Xb);
    t.app.notify(b"SAME", b"Tornado warning", true);
    assert!(matches!(t.app.screen(), Screen::Crossband(_)), "no full-screen takeover");
    assert_eq!(t.app.radio.mode, Mode::Xb, "the repeater keeps running");
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"ALERT" && x.body.as_bytes() == b"Tornado warning"));
    t.wait(5000);
    assert!(matches!(t.app.screen(), Screen::Crossband(_)), "still waiting while it runs");
    t.app.radio.tx_built = true;
    t.tap(Key::Exit);
    assert_ne!(t.app.radio.mode, Mode::Xb);
    t.wait(100);
    assert!(matches!(t.app.screen(), Screen::Alert(_)), "opens once the repeater stops: {:?}", t.app.screen());
}

/// DTMF-1, DTMF-3: the dialer's Send keys like PTT, hands the digits to the Radio task, and unkeys once the Radio
/// task says they are out; the Transmit group's settings reach the Radio task when they change.
#[test]
fn dialer_sends_then_unkeys() {
    let mut t = ready();
    menu0(&mut t, &[8, 6, 1]);
    for d in [1, 2, 3] {
        t.tap(Key::Digit(d));
    }
    t.cmds();
    t.tap(Key::Menu); // Send
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::DtmfSeq { chunk: 0, keys: [b'1', b'2', b'3', 0, 0] }), "{c:?}");
    assert!(c.contains(&RadioCmd::Transmit { slot: 0, on: true }) && c.contains(&RadioCmd::DtmfSend { len: 3 }), "{c:?}");
    assert!(matches!(t.app.screen(), Screen::Transmit(x) if x.dial));
    // Not yet out: still keyed.
    t.wait(500);
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    t.live.tx.dtmf_sent = t.live.tx.dtmf_sent.wrapping_add(1);
    t.wait(50);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: false }), "unkeyed when the digits are out");
    assert!(!matches!(t.app.screen(), Screen::Transmit(_)), "back to the dialer");
}

/// TONE-5 to TONE-10, VOX-10, VOX-12, DTMF-3, DTMF-8: the Transmit group's settings go to the Radio task as they
/// change; CW ID needs a callsign; a channel's own end tone and tail win over the radio's.
#[test]
fn transmit_tones_reach_the_radio() {
    let mut t = ready();
    t.app.set.callsign = crate::settings::Text::EMPTY;
    menu0(&mut t, &[8, 7, 2]);
    t.cmds();
    t.app.knob(1, t.t, &store());
    assert_eq!(t.app.set.cw_id, 0, "VOX-10: no callsign, no CW ID");
    t.app.set_callsign(b"va7fer");
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::CwCall { chunk: 0, text: *b"VA7FE" }) && c.contains(&RadioCmd::CwCall { chunk: 1, text: [b'R', 0, 0, 0, 0] }), "{c:?}");
    t.app.knob(1, t.t, &store());
    assert_eq!(t.app.set.cw_id, 1);
    assert!(t.cmds().contains(&RadioCmd::TxTones { phase: 0, cw: 1, sidetone: true }));
    // 0 8 5 4: every slot's end tone follows (Beep = END_TONES[1]).
    t.app.set.end_tone = 1;
    t.app.send_slot_tones();
    let c = t.cmds();
    for s in 0..3u8 {
        assert!(c.contains(&RadioCmd::SlotParam { slot: s, param: Param::EndTone, value: 1 }), "slot {s}: {c:?}");
    }
    // A channel's own: Off (1) wins over the radio's Beep; tail On (2) is both directions.
    let mut ch = crate::chan::channel(5, 0, mhz(146) + 52_000, "Own", crate::slot::DEFAULT_CFG);
    ch.set_end_tone(1);
    ch.set_tail(2);
    t.app.set_channel_rec(0, ch);
    assert_eq!(t.app.slot_tones(0), (0, 3));
    t.app.set.tail_mode = 2; // transmit only
    assert_eq!(t.app.slot_tones(1), (1, 2), "a VFO follows the radio's");
}

/// DEC-4: 0 4 2 3 types the capcode (digits only, 21 bits at most); empty keeps every capcode.
#[test]
fn capcode_typed_at_0_4_2_3() {
    let mut t = T::new();
    menu0(&mut t, &[4, 2, 3]);
    assert!(matches!(t.app.screen(), Screen::Text(_)));
    for d in [1, 2, 3, 4, 5, 6, 7] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.set.pocsag_cap, 1_234_567);
    t.app.screen = Screen::Home;
    menu0(&mut t, &[4, 2, 3]);
    for _ in 0..7 {
        t.tap(Key::Exit); // delete the digits
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.set.pocsag_cap, 0, "empty: every capcode");
    t.app.screen = Screen::Home;
    menu0(&mut t, &[4, 2, 3]);
    for d in [9, 9, 9, 9, 9, 9, 9] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Text(_)), "over 2097151 stays to be fixed");
    assert_eq!(t.app.set.pocsag_cap, 0);
}

/// GPS-23: OK on 0 6 6 sets the clock by hand, typed in local time; the clock's owner takes it as UTC.
#[test]
fn clock_set_by_hand() {
    let mut t = T::new();
    let utc_minus5 = crate::settings::TIME_ZONES.iter().position(|&q| q == -20).expect("UTC-5") as u8;
    t.app.set.tz = utc_minus5;
    menu0(&mut t, &[6, 6]);
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Text(_)));
    for c in b"202610081430" {
        t.tap(Key::Digit(c - b'0'));
    }
    t.tap(Key::Menu);
    let local = ferrite_core::gps::unix_time(81_026, 143_000).unwrap();
    assert_eq!(t.app.take_clock_set(), Some(local + 5 * 3600), "14:30 at UTC-5 is 19:30 UTC");
    assert_eq!(t.app.take_clock_set(), None, "taken once");
    // A date that doesn't exist stays to be fixed.
    t.app.screen = Screen::Home;
    menu0(&mut t, &[6, 6]);
    t.tap(Key::Menu);
    for c in b"202602301200" {
        t.tap(Key::Digit(c - b'0'));
    }
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Text(_)));
    assert_eq!(t.app.take_clock_set(), None);
}

/// DEC-7: 0 4 8 Popups is a real setting, # puts it back on.
#[test]
fn popups_setting() {
    use crate::screens::groups::{change, reset};
    let mut t = T::new();
    assert!(t.app.set.popups, "on by default");
    change(&mut t.app, &[4, 8], 1);
    assert!(!t.app.set.popups);
    reset(&mut t.app, &[4, 8]);
    assert!(t.app.set.popups);
}

/// SCOPE-7, HOME-3: the scope screen and home's scope strip redraw their bars in place (no blank first: it
/// flickered on the radio, Oct 12, 2026) and end up exactly as a fresh full redraw, through changing sweeps.
#[test]
fn scope_bars_redraw_in_place() {
    use ferrite_ui::fb::FrameBuffer;
    let same = |t: &mut T, fb: &mut FrameBuffer, what: &str| {
        t.render(fb);
        let mut fresh = FrameBuffer::new();
        t.app.redraw_all();
        t.render(&mut fresh);
        for y in 0..320 {
            for x in 0..240 {
                assert_eq!(fb.at(x, y), fresh.at(x, y), "{what}: stale pixel at {x},{y}");
            }
        }
    };
    for layout_strip in [false, true] {
        let mut t = ready();
        let mut fb = FrameBuffer::new();
        if layout_strip {
            t.app.set.layout = 1;
        } else {
            t.hold(Key::Digit(1)); // the scope screen
        }
        let c = t.freq();
        for (k, (at, peak)) in [(10usize, 30u8), (40, 5), (25, 60), (59, 0)].into_iter().enumerate() {
            scope_live(&mut t, c, at, peak, k as u8, false, 0);
            same(&mut t, &mut fb, if layout_strip { "home strip" } else { "scope screen" });
        }
    }
}

/// HOME-3: leaving the single + scope layout stops the strip's sweep, so the slots receive again.
#[test]
fn leaving_the_scope_layout_stops_the_sweep() {
    let mut t = ready();
    t.app.set.layout = 1;
    t.wait(100);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Sweep { on: true, .. })), "the strip sweeps");
    t.app.set.layout = 0;
    t.wait(100);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Sweep { on: false, .. })), "back to Watch: the sweep stops");
}

/// An exclusive mode's stop command (the Radio task's side of [`App::stop_mode`]).
fn stops(m: Mode, c: &RadioCmd) -> bool {
    match m {
        Mode::Scope | Mode::Strip => matches!(c, RadioCmd::Sweep { on: false, .. }),
        Mode::Tone => matches!(c, RadioCmd::ToneSearch { on: false, .. }),
        Mode::Counter => matches!(c, RadioCmd::Counter(false)),
        Mode::Fm => matches!(c, RadioCmd::Broadcast(BroadcastCmd::Off)),
        Mode::Hf => matches!(c, RadioCmd::Hf { on: false, .. }),
        Mode::Xb => matches!(c, RadioCmd::Crossband(None)),
        Mode::Off => false,
    }
}

/// Start mode `m` from home, as a person would: its screen (crossband: and # Start), or for the strip the
/// single + scope layout.
fn start(t: &mut T, m: Mode) {
    match m {
        Mode::Scope => t.open(Dest::BandScope),
        Mode::Strip => {
            t.app.set.layout = 1;
            t.wait(100);
        }
        Mode::Tone => t.open(Dest::ToneSearch),
        Mode::Counter => t.open(Dest::Counter),
        Mode::Fm => t.open(Dest::FmBroadcast),
        Mode::Hf => t.open(Dest::HfMode),
        Mode::Xb => {
            t.open(Dest::Crossband);
            t.tap(Key::Hash);
        }
        Mode::Off => {}
    }
    assert_eq!(t.app.radio.mode, m, "{m:?} runs");
}

const MODES: [Mode; 7] = [Mode::Scope, Mode::Strip, Mode::Tone, Mode::Counter, Mode::Fm, Mode::Hf, Mode::Xb];

/// HOME-2, SCOPE-4 and each mode's own rules: an exclusive mode stops with its screen, however the screen is left
/// (BACK, hold BACK, the screen's timeout, PTT; the strip by leaving its layout), and only then: the Radio task
/// gets the mode's stop command once, and no mode is left running behind another screen.
#[test]
fn every_way_out_of_a_mode_stops_it() {
    #[derive(Debug, Clone, Copy)]
    enum Out {
        Back,
        HoldBack,
        Timeout,
        Ptt,
        Layout,
    }
    for m in MODES {
        let outs: &[Out] = if m == Mode::Strip { &[Out::Ptt, Out::Layout] } else { &[Out::Back, Out::HoldBack, Out::Timeout, Out::Ptt] };
        for &out in outs {
            let mut t = ready();
            start(&mut t, m);
            let shown = t.app.screen();
            t.cmds();
            match out {
                Out::Back => t.tap(Key::Exit),
                Out::HoldBack => t.hold(Key::Exit),
                Out::Timeout => t.wait(15 * 60 * 1000),
                Out::Ptt => {
                    ptt(&mut t, true);
                    // keyed on another screen (Transmit): the mode stopped before the key-up, not after; HF mode
                    // transmits (or refuses) in place (HF-23, HF-24), the repeater is held (XB-14)
                    if t.app.screen() != shown && !matches!(m, Mode::Hf | Mode::Xb) {
                        assert_eq!(t.app.radio.mode, Mode::Off, "{m:?}: PTT left its screen and the mode ran on");
                    }
                    ptt(&mut t, false);
                    t.wait(500);
                }
                Out::Layout => {
                    t.app.set.layout = 0;
                    t.wait(100);
                }
            }
            let cmds = t.cmds();
            let sent = cmds.iter().filter(|c| stops(m, c)).count();
            if let (Mode::Hf | Mode::Xb, Out::Ptt) = (m, out) {
                // HF mode transmits in place (HF-23); the repeater is held and carries on after (XB-14)
                assert_eq!((sent, t.app.radio.mode), (0, m), "{m:?}, PTT: the mode stays");
            } else if let (Mode::Strip, Out::Ptt) = (m, out) {
                // home's strip stops for the transmission and sweeps again once home is back
                assert_eq!((sent, t.app.radio.mode), (1, Mode::Strip), "strip, PTT");
                let again = cmds.iter().rposition(|c| matches!(c, RadioCmd::Sweep { on: true, .. }));
                assert!(again > cmds.iter().position(|c| stops(m, c)), "strip, PTT: sweeping again after");
            } else if t.app.radio.mode == Mode::Off {
                assert_eq!(sent, 1, "{m:?}, {out:?}: the stop command, once");
            } else {
                // still running: only on its own screen (no timeout there; the scope refuses PTT unless listening,
                // SCOPE-12), and nothing stopped
                assert_eq!((t.app.radio.mode, sent), (m, 0), "{m:?}, {out:?}");
                assert!(t.app.screen() == shown && matches!(out, Out::Timeout | Out::Ptt), "{m:?}, {out:?}: running behind another screen");
            }
        }
    }
}

/// Starting one exclusive mode stops the one running first: its stop command goes before the new mode's start.
#[test]
fn a_mode_stops_the_one_before_it() {
    for a in MODES {
        for b in MODES {
            if a == b || b == Mode::Strip {
                continue; // the strip starts only from home's layout
            }
            let mut t = ready();
            start(&mut t, a);
            t.cmds();
            if a != Mode::Strip {
                t.app.go_home(); // a mode screen's own keys don't open the others: home first, as a person would
            }
            start(&mut t, b);
            let cmds = t.cmds();
            let stop = cmds.iter().position(|c| stops(a, c));
            assert!(stop.is_some(), "{a:?} then {b:?}: {a:?} stopped ({cmds:?})");
            assert_eq!(t.app.radio.mode, b);
        }
    }
}

/// MODE-4 (UI backlog 5): turning the knob on the single + scope layout stops the strip at the first step, so
/// normal receive follows the knob; it sweeps again 500 ms after the last step, centred on the new frequency.
#[test]
fn strip_rests_while_the_knob_turns() {
    let mut t = ready();
    start(&mut t, Mode::Strip);
    t.cmds();
    t.knob(1);
    assert_eq!(t.app.radio.mode, Mode::Off, "the first step stops the strip");
    assert!(t.cmds().iter().any(|c| stops(Mode::Strip, c)), "its stop sent at once");
    for _ in 0..8 {
        t.wait(200);
        t.knob(1);
        assert_eq!(t.app.radio.mode, Mode::Off, "still turning");
    }
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Sweep { on: true, .. })), "no sweep while turning");
    t.wait(400);
    assert_eq!(t.app.radio.mode, Mode::Off, "400 ms after the last step: still resting");
    t.wait(200);
    assert_eq!(t.app.radio.mode, Mode::Strip, "back 500 ms after the last step");
    let f = t.freq();
    let c = t.cmds();
    assert!(c.iter().any(|c| matches!(c, RadioCmd::Sweep { on: true, centre, .. } if *centre == f)), "centred on the new frequency: {c:?}");
}

/// MODE-3: while an exclusive mode runs the app confirms it at least every RENEW_MS; with none running it sends
/// no lease. When the Radio task stopped a mode itself (its lease ran out), the app takes it as stopped and starts
/// again what its screen still wants, with no stop command of its own.
#[test]
fn modes_hold_a_lease() {
    use ferrite_core::lease::{RENEW_MS, WINDOW_MS};
    const { assert!(RENEW_MS * 3 <= WINDOW_MS, "two renewals may go missing before the Radio task gives up") };
    for m in MODES {
        let mut t = ready();
        start(&mut t, m);
        t.cmds();
        for _ in 0..6 {
            t.wait(RENEW_MS + 100);
            assert!(t.cmds().contains(&RadioCmd::Lease), "{m:?}: renewed");
        }
        // the Radio task dropped it
        t.live.lease_drops = t.live.lease_drops.wrapping_add(1);
        t.wait(100);
        let c = t.cmds();
        assert!(!c.iter().any(|c| stops(m, c)), "{m:?}: no stop for a mode already stopped: {c:?}");
        assert_eq!(t.app.radio.mode, m, "{m:?}: started again, its screen still up");
        assert_eq!(t.app.radio.lease_drops, t.live.lease_drops);
    }
    let mut t = ready();
    t.wait(5000);
    assert!(!t.cmds().contains(&RadioCmd::Lease), "no mode, no lease");
}

/// HOME-13 (UI backlog 6): while the knob turns, home redraws only the active slot's part; the other slots, the
/// activity rows and the strip wait, and draw once the knob rests, to the same picture a full redraw gives.
#[test]
fn knob_holds_the_heavier_redraws() {
    struct Count(FrameBuffer, Vec<(i32, i32, i32, i32)>);
    impl Panel for Count {
        fn window(&mut self, a: i32, b: i32, c: i32, d: i32) {
            self.1.push((a, b, c, d));
            self.0.window(a, b, c, d)
        }
        fn push(&mut self, c: u16, n: u32) {
            self.0.push(c, n)
        }
        fn push_buf(&mut self, p: &[u16]) {
            self.0.push_buf(p)
        }
    }
    // per layout: the active slot's rows (the frequency and what goes with it) and where the held parts start
    for (layout, live_end, held_from) in [(0u8, 150, 155), (1, 200, 200), (2, 87, 87)] {
        let mut t = ready();
        let dev = Device { clock_min: Some(600), ..Default::default() };
        let mut lcd = Count(FrameBuffer::new(), Vec::new());
        t.app.set_layout(layout);
        t.live.slots[0] = SlotLive { freq: t.freq(), ..Default::default() };
        t.wait(1000);
        t.app.render(&mut lcd, &t.live, &dev, &store());
        // something changes in the other slots (and the strip's centre with the knob)
        t.live.slots[1] = SlotLive { squelch_open: true, rssi: -70, ..Default::default() };
        t.wait(1300); // the last minute's bucket ticks over
        t.app.render(&mut lcd, &t.live, &dev, &store());
        t.live.slots[1] = SlotLive::default();
        t.live.slots[2] = SlotLive { squelch_open: true, rssi: -60, ..Default::default() };
        let f0 = t.freq();
        lcd.1.clear();
        t.knob(1);
        assert_ne!(t.freq(), f0);
        t.app.render(&mut lcd, &t.live, &dev, &store());
        let held: Vec<_> = lcd.1.iter().filter(|r| r.1 >= 21 && r.1 < 300 && !(r.1 < live_end)).collect();
        assert!(lcd.1.iter().any(|r| r.1 >= 21 && r.1 < live_end), "layout {layout}: the frequency redrew at once");
        assert!(held.is_empty(), "layout {layout}: held parts drew while the knob turned: {held:?}");
        t.wait(500);
        lcd.1.clear();
        t.app.render(&mut lcd, &t.live, &dev, &store());
        assert!(lcd.1.iter().any(|r| r.1 >= held_from && r.1 < 300), "layout {layout}: the held parts drew once it rested");
        let after = lcd.0.px.clone();
        t.app.redraw_all();
        t.app.render(&mut lcd, &t.live, &dev, &store());
        assert!(after == lcd.0.px, "layout {layout}: the same picture as a full redraw");
    }
}

/// MODE-1, MODE-2, BCR-29: random use (keys, the knob, layouts, screens opened, timeouts, PTT, slots tuned in and
/// out of the broadcast band) never leaves the radio in
/// a mode the app doesn't want: after every step the mode is the wanted one; home on the plain layout with no
/// mode screen under it is normal receive; and the commands sent, played on a model of the Radio task, never
/// start a mode over another and end in the mode the app thinks runs.
#[test]
fn random_use_keeps_modes_in_step() {
    const KEYS: [Key; 16] = [
        Key::Menu, Key::Exit, Key::Up, Key::Down, Key::Left, Key::Right, Key::Slot, Key::VfoMem, Key::Digit(0),
        Key::Digit(1), Key::Digit(3), Key::Digit(7), Key::Star, Key::Hash, Key::Side(1), Key::Side(2),
    ];
    const DESTS: [Dest; 8] = [Dest::BandScope, Dest::ToneSearch, Dest::Counter, Dest::FmBroadcast, Dest::HfMode, Dest::Crossband, Dest::Scan, Dest::Squelch];
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Kind {
        Sweep,
        Tone,
        Counter,
        Fm,
        Hf,
        Xb,
    }
    fn kind(m: Mode) -> Option<Kind> {
        match m {
            Mode::Off => None,
            Mode::Scope | Mode::Strip => Some(Kind::Sweep),
            Mode::Tone => Some(Kind::Tone),
            Mode::Counter => Some(Kind::Counter),
            Mode::Fm => Some(Kind::Fm),
            Mode::Hf => Some(Kind::Hf),
            Mode::Xb => Some(Kind::Xb),
        }
    }
    // the Radio task's view: what a command starts (Some) or stops (None)
    fn effect(c: &RadioCmd) -> Option<(Kind, bool)> {
        Some(match c {
            RadioCmd::Sweep { on, .. } => (Kind::Sweep, *on),
            RadioCmd::ToneSearch { on, .. } => (Kind::Tone, *on),
            RadioCmd::Counter(on) => (Kind::Counter, *on),
            RadioCmd::Broadcast(b) => (Kind::Fm, *b != BroadcastCmd::Off),
            RadioCmd::Hf { on, .. } => (Kind::Hf, *on),
            RadioCmd::Crossband(x) => (Kind::Xb, x.is_some()),
            _ => return None,
        })
    }
    // play commands on the model; a start over another mode fails
    fn play(cmds: Vec<RadioCmd>, radio: &mut Option<Kind>, at: &str) {
        for c in cmds {
            let Some((k, on)) = effect(&c) else { continue };
            if on {
                assert!(radio.is_none() || *radio == Some(k), "{at}: {k:?} started over {radio:?}");
                *radio = Some(k);
            } else if *radio == Some(k) {
                *radio = None;
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for seed in 1..=32u64 {
        let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = |n: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % n
        };
        let mut t = ready();
        let mut keyed = false;
        let mut radio: Option<Kind> = None;
        for step in 0..2000 {
            let s = store();
            let what = match next(21) {
                0..=7 => {
                    let k = KEYS[next(KEYS.len() as u64) as usize];
                    if next(6) == 0 {
                        t.hold(k);
                    } else {
                        t.tap(k);
                    }
                    "key"
                }
                8..=10 => {
                    let d = if next(2) == 0 { 1 } else { -1 };
                    for _ in 0..=next(4) {
                        t.knob(d);
                        while t.app.pop_write().is_some() {} // the UI task takes each input's write
                        t.t += 30;
                        t.app.tick(t.t, &t.live, &s);
                    }
                    "knob"
                }
                11 => {
                    t.app.set_layout(next(3) as u8);
                    "layout"
                }
                12 => {
                    t.app.set.layout = next(3) as u8; // a setting changed underneath (a link write, a reset)
                    t.wait(100);
                    "setting"
                }
                13..=14 => {
                    t.open(DESTS[next(DESTS.len() as u64) as usize]);
                    "open"
                }
                15..=16 => {
                    t.wait(100 + next(3000) as u32);
                    "wait"
                }
                17 => {
                    // in steps, the commands taken as the UI task does each turn
                    for _ in 0..if next(4) == 0 { 900 } else { 30 } {
                        t.wait(1000);
                        play(t.cmds(), &mut radio, &std::format!("seed {seed} step {step} (timeout)"));
                    }
                    "timeout"
                }
                20 => {
                    // BCR-22: a slot tuned into (or out of) the broadcast band
                    let f = [mhz(100) + 50_000, mhz(99) + 10_000, mhz(146) + 52_000][next(3) as usize];
                    t.app.set_vfo(next(3) as u8, f);
                    t.wait(100);
                    "broadcast"
                }
                _ => {
                    keyed = !keyed;
                    ptt(&mut t, keyed);
                    "ptt"
                }
            };
            while t.app.pop_write().is_some() {}
            play(t.cmds(), &mut radio, &std::format!("seed {seed} step {step} ({what})"));
            let m = t.app.radio.mode;
            seen.insert(std::format!("{m:?}"));
            assert_eq!(m, t.app.wanted_mode(), "seed {seed} step {step} ({what}): reconciled");
            assert_eq!(radio, kind(m), "seed {seed} step {step} ({what}): the Radio task runs what the app thinks");
            let mode_screen = |s: &Screen| matches!(s, Screen::Scope(_) | Screen::ToneSearch(_) | Screen::Counter(_) | Screen::Fm(_) | Screen::Hf(_) | Screen::Crossband(_));
            if t.app.screen() == Screen::Home && t.app.set.layout == 0 && !t.app.back.iter().flatten().any(mode_screen) {
                let want = if t.app.bcast_slot().is_some() && t.app.radio.keyed.is_none() { Mode::Fm } else { Mode::Off };
                assert_eq!(m, want, "seed {seed} step {step} ({what}): home is normal receive, or the broadcast slot's");
            }
            if let Some(b) = t.app.bcast_slot() {
                assert!(t.app.in_bcast_band(b), "seed {seed} step {step} ({what}): the broadcast slot is in the band");
            }
            if t.app.radio.keyed.is_some() {
                assert!(matches!(m, Mode::Off | Mode::Hf | Mode::Xb), "seed {seed} step {step} ({what}): {m:?} ran while keyed");
            }
        }
        if keyed {
            ptt(&mut t, false);
        }
    }
    assert_eq!(seen.len(), MODES.len() + 1, "every mode reached: {seen:?}");
}
