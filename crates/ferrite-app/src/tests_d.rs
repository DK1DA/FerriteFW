//! Group D's rules: setup, the Extend TX warning, the PIN, the key test, the link prompts, FM broadcast, HF
//! mode, the alert and the advanced menu's groups.

use super::*;
use crate::screens::groups::Group;
use crate::screens::nav::Dest;
use crate::screens::hf::Hf;
use crate::screens::setup::Setup;
use ferrite_core::msg::BroadcastCmd;

/// MENU 0 and a path typed straight through (MENU-5).
fn menu0(t: &mut T, path: &[u8]) {
    t.hold(Key::Exit);
    t.tap(Key::Menu);
    t.tap(Key::Digit(0));
    for &d in path {
        t.tap(Key::Digit(d));
    }
}

fn toast_head(t: &T) -> std::string::String {
    t.app.toast.map(|x| std::string::String::from_utf8_lossy(x.head.as_bytes()).into_owned()).unwrap_or_default()
}

/// SETUP-1 to SETUP-5, SETUP-12: MENU 0 9 8 1 runs setup; the callsign is typed in capitals; # skips to the
/// summary; OK starts the radio; every answer is the menu's setting.
#[test]
fn setup_runs_and_writes_settings() {
    let mut t = T::new();
    menu0(&mut t, &[9, 8, 1]);
    assert!(matches!(t.app.screen(), Screen::Setup(Setup { page: 0, .. })));
    t.tap(Key::Menu); // page 2: callsign
    // V A 7 F E R with multi-tap: 8 8 8 (V), 2 (A), * 7 * (digit), 3 3 3 (F), 3 3 (E), 7 7 7 (R)
    for k in [8, 8, 8, 2] {
        t.tap(Key::Digit(k));
    }
    t.tap(Key::Star);
    t.tap(Key::Digit(7));
    t.tap(Key::Star);
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(3));
    t.wait(1200); // a pause ends the letter
    t.tap(Key::Digit(3));
    t.tap(Key::Digit(3));
    t.wait(1200);
    for _ in 0..3 {
        t.tap(Key::Digit(7));
    }
    assert_eq!(t.app.callsign(), b"VA7FER", "capitals as you type (SETUP-12)");
    t.tap(Key::Left);
    assert_eq!(t.app.callsign(), b"VA7FE", "\u{25c0} deletes");
    t.tap(Key::Digit(7));
    t.tap(Key::Digit(7));
    t.tap(Key::Digit(7));
    t.tap(Key::Menu); // page 3: region
    assert_eq!(t.app.settings().region_name(), "Not set", "nothing is chosen for you (SETUP-3)");
    t.tap(Key::Down); // Canada
    assert_eq!(t.app.settings().region_name(), "Not set", "moving doesn't choose");
    t.tap(Key::Menu); // OK chooses it and goes on
    assert_eq!(t.app.settings().region_name(), "Canada");
    t.tap(Key::Exit); // back to the region page, Canada kept
    assert_eq!(t.app.settings().region_name(), "Canada");
    t.tap(Key::Exit); // BACK: previous page keeps the answers
    assert!(matches!(t.app.screen(), Screen::Setup(Setup { page: 1, .. })));
    assert_eq!(t.app.callsign(), b"VA7FER");
    t.tap(Key::Menu);
    t.tap(Key::Hash); // SETUP-4: skip to the summary
    assert!(matches!(t.app.screen(), Screen::Setup(Setup { page: 7, .. })));
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    assert!(count_px(&fb, 0, 19, |c| c == BLUE) > 50, "setup's own top bar: progress dashes");
    t.tap(Key::Menu); // OK: start
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.app.settings().setup_done);
    // MENU 0 8 7 1 shows the same callsign (SETUP-5), and opens setup's callsign page alone.
    menu0(&mut t, &[8, 7]);
    let mut b = Buf::<24>::new();
    let dev = Device::default();
    assert_eq!(crate::screens::groups::wired(&t.app, &dev, &[8, 7, 1], &mut b), Some(true));
    assert_eq!(b.as_bytes(), b"VA7FER");
    t.tap(Key::Digit(1));
    assert!(matches!(t.app.screen(), Screen::Setup(Setup { page: 1, .. })));
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 8, sub: 7, row: 0 }), "OK returns to the list");
}

/// BOOT-12: the first-boot hook opens setup, where BACK on the first page goes nowhere; defaults give a radio.
#[test]
fn setup_at_first_boot() {
    let mut t = T::new();
    t.app.first_boot();
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Setup(Setup { page: 0, .. })));
    for _ in 0..8 {
        t.tap(Key::Menu); // OK, OK, OK... (SETUP-3)
    }
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.settings().region_name(), "United States", "the pre-selected region");
    assert!(!t.app.settings().extend_tx);
}

/// SETUP-8: Extend TX turns on only through its warning, shown once.
#[test]
fn extend_tx_warns_once() {
    let mut t = T::new();
    menu0(&mut t, &[8]);
    t.tap(Key::Digit(9));
    t.knob(1);
    assert!(matches!(t.app.screen(), Screen::ExtendTx(_)));
    t.tap(Key::Exit); // keep off
    assert!(!t.app.settings().extend_tx);
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 8, sub: 0, row: 8 }));
    t.knob(1);
    t.tap(Key::Menu); // turn on
    assert!(t.app.settings().extend_tx);
    t.knob(1); // off
    t.knob(1); // on again: no warning the second time
    assert!(t.app.settings().extend_tx);
    assert!(matches!(t.app.screen(), Screen::Group(_)));
}

/// PIN-1 to PIN-3, KEY-16: set a PIN; at power on nothing else works until it's right (hold BACK and PTT
/// included); three misses wait 30 s.
#[test]
fn pin_at_power_on() {
    let mut t = T::new();
    menu0(&mut t, &[9, 3]);
    t.knob(1); // PIN lock: at power on -> asks for a new PIN first, twice
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    for _ in 0..2 {
        for d in [1, 2, 3, 4] {
            t.tap(Key::Digit(d));
        }
        t.tap(Key::Menu);
    }
    assert_eq!(t.app.settings().pin_mode, 1);
    assert_eq!(t.app.settings().pin_len, 4);
    t.app.ask_pin();
    let pin = t.app.screen();
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), pin, "hold BACK doesn't skip the PIN");
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    t.app.key_up(Key::PTT, t.t + 100, &s);
    assert!(matches!(t.app.screen(), Screen::Pin(_)), "nor does PTT");
    for _ in 0..3 {
        for d in [9, 9, 9, 9] {
            t.tap(Key::Digit(d));
        }
        t.tap(Key::Menu);
    }
    assert_eq!(toast_head(&t), "Wrong PIN");
    for d in [1, 2, 3, 4] {
        t.tap(Key::Digit(d)); // ignored while waiting
    }
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Pin(_)), "waiting 30 s");
    t.wait(30_500);
    t.wake();
    for d in [1, 2, 3, 4] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// The key test shows each key and its code; short BACK and PTT are tested, not acted on.
#[test]
fn key_test() {
    let mut t = T::new();
    menu0(&mut t, &[9, 1, 3]);
    assert!(matches!(t.app.screen(), Screen::KeyTest(_)));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::KeyTest(_)), "short BACK is tested");
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    t.app.key_up(Key::PTT, t.t + 100, &s);
    assert!(matches!(t.app.screen(), Screen::KeyTest(_)), "PTT is tested, not transmitted");
    t.app.raw_key(14, true); // an unmapped matrix code
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home, "hold BACK leaves");
}

/// BCR-1, BCR-16: MENU 0 3 1 6 starts FM broadcast on the SI4732; tuning sends it; leaving (BACK or PTT) stops it.
#[test]
fn fm_broadcast() {
    let mut t = T::new();
    t.open(Dest::FmBroadcast);
    assert!(matches!(t.app.screen(), Screen::Fm(_)));
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_110))));
    t.knob(1);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_120))));
    t.hold(Key::Menu); // its options; 0 the full menu keeps the broadcast
    t.tap(Key::Digit(0));
    assert!(matches!(t.app.screen(), Screen::Advanced(_)));
    assert!(!t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Off)));
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Off)));
    t.open(Dest::FmBroadcast);
    t.cmds();
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    let c = t.cmds();
    let off = c.iter().position(|c| *c == RadioCmd::Broadcast(BroadcastCmd::Off));
    assert!(off.is_some(), "PTT stops the broadcast first (BCR-16): {c:?}");
}

/// HF-1 to HF-8, HF-13, HF-24, HF-28, SBAR-18: MENU 0 3 4 enters HF mode; the status bar says HF without
/// TW/DW; everything pauses until BACK; PTT refuses on the SI4732; a frequency held 10 s joins the recent list.
#[test]
fn hf_mode() {
    let mut t = T::new();
    t.app.set.region = 1; // US: SAME on
    menu0(&mut t, &[3, 4]);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Hf { on: true, .. })));
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    assert!(matches!(t.app.screen(), Screen::Hf(Hf { loaded: true, .. })), "no patch here: loaded at once");
    let dev = Device::default();
    let sb = t.app.status_bar(&t.live, &dev);
    assert_eq!(sb.name.as_bytes(), b"HF");
    assert_eq!(sb.watch, 0, "no TW/DW in HF mode (HF-5)");
    assert!(sb.alerts_paused, "SBAR-18");
    t.tap(Key::Slot);
    assert_eq!(toast_head(&t), "Not in HF mode");
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    t.app.key_up(Key::PTT, t.t + 50, &s);
    assert_eq!(t.app.screen().name(), "Transmit refused", "9.58 MHz AM is the SI4732's (HF-24)");
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Hf(_)));
    t.tap(Key::Up); // 9.585 (5 kHz on shortwave broadcast, HF-34)
    t.wait(10_500);
    assert_eq!(t.app.set.hf.recent[0] & 0x0FFF_FFFF, 958_500, "used 10 s: in the recent list");
    for d in [2, 8, 4, 0, 0] {
        t.tap(Key::Digit(d)); // 28400 kHz
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.set.hf.freq, 2_840_000);
    t.tap(Key::Hash);
    assert_eq!(t.app.screen().name(), "HF recent");
    t.tap(Key::Menu); // tunes the newest, back in HF mode
    assert!(matches!(t.app.screen(), Screen::Hf(_)));
    assert_eq!(t.app.set.hf.freq, 958_500);
    t.cmds();
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Group(g) if g.g == 3), "BACK: one level, to the menu it came from");
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Hf { on: false, .. })), "leaving resumes everything (HF-4)");
}

/// HF-31: hold 3 seeks up and hold 1 down with the SI4732's seek, in AM below 30 MHz; the landing is kept.
#[test]
fn hf_seek() {
    use ferrite_core::msg::ModeLive;
    let seeks = |t: &mut T| t.cmds().into_iter().filter(|c| matches!(c, RadioCmd::HfSeek { .. })).collect::<Vec<_>>();
    let mut t = T::new();
    menu0(&mut t, &[3, 4]);
    seeks(&mut t);
    let start = t.app.set.hf.freq;
    t.hold(Key::Digit(3));
    assert_eq!(seeks(&mut t), [RadioCmd::HfSeek { up: true }]);
    assert!(matches!(t.app.screen(), Screen::Hf(Hf { n: 0, .. })), "a hold types no digit");
    t.live.mode = ModeLive::Hf { freq: 1_173_500, rssi: -70, snr: 20, si4732: true, loading: 255 };
    t.wait(200);
    assert_eq!(t.app.set.hf.freq, 1_173_500, "the station it stopped on (from {start})");
    t.live.mode = ModeLive::Hf { freq: 958_000, rssi: -70, snr: 20, si4732: true, loading: 255 };
    t.wait(200);
    assert_eq!(t.app.set.hf.freq, 1_173_500, "no seek running: the screen's own frequency stands");
    t.hold(Key::Digit(1));
    assert_eq!(seeks(&mut t), [RadioCmd::HfSeek { up: false }]);
    t.wait(200);
    assert_eq!(t.app.set.hf.freq, 958_000);
    t.app.set.hf.mode = 3; // USB
    t.hold(Key::Digit(3));
    assert!(seeks(&mut t).is_empty(), "no seek in SSB");
    assert_eq!(toast_head(&t), "Seek");
}

/// WX-4 to WX-7: an alert opens over any screen; OK silences, then opens the event log; BACK goes home.
#[test]
fn alert_screen() {
    let mut t = T::new();
    t.tap(Key::Menu);
    let mut a = crate::screens::alert::AlertInfo::NONE;
    a.event.set(b"Severe thunderstorm warning");
    a.areas.set(b"Ottawa");
    (a.issued, a.until, a.slot, a.freq) = (14 * 60 + 28, 15 * 60 + 15, 2, 16_255_000);
    t.app.raise_alert(&a);
    assert!(matches!(t.app.screen(), Screen::Alert(_)));
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    assert!(count_px(&fb, 24, 27, |c| c == ferrite_ui::gfx::RED) > 100, "red border");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Alert(crate::screens::alert::Alert { silenced: true })));
    t.tap(Key::Menu);
    assert_eq!(t.app.screen().name(), "Event log");
    t.app.notify(b"SAME", b"Tornado warning", true);
    t.tap(Key::Exit);
    assert_eq!(t.app.screen().name(), "Event log", "BACK: one level, to the screen it came over");
    t.app.notify(b"SAME", b"Tornado warning", true);
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home, "hold BACK: home");
}

/// MENU-12 to MENU-18 on a group: the knob changes a wired value live, # resets it, greyed items say why;
/// weather alerts exist only in the US and Canada (WX-3).
#[test]
fn groups_change_values() {
    let mut t = T::new();
    menu0(&mut t, &[7, 1]);
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 7, sub: 0, row: 0 }));
    t.knob(1);
    assert_eq!(t.app.settings().theme, 1);
    t.tap(Key::Hash);
    assert_eq!(t.app.settings().theme, 0);
    menu0(&mut t, &[4, 9]);
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 4, sub: 0, row: 8 }), "greyed: not opened");
    assert_eq!(toast_head(&t), "Weather alerts");
    t.app.set.region = 2; // Canada
    t.tap(Key::Digit(9));
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 4, sub: 9, row: 0 }));
    // every group and list draws
    let mut fb = FrameBuffer::new();
    for (g, n) in crate::screens::groups::GROUP_ITEMS.iter().enumerate() {
        t.app.open(Dest::Group(g as u8 + 1));
        t.render(&mut fb);
        assert!(count_px(&fb, 23, 23 + 22 * n.len() as i32, |c| c != BG) > 500);
    }
    for ((g, m), _) in crate::screens::groups::SUBS {
        t.app.open(Dest::Sub(g, m));
        t.render(&mut fb);
    }
}

/// D20, D21: the write approval answers the Link task; a write session ignores keys but BACK (LINK-9).
#[test]
fn link_prompts() {
    use crate::screens::link::LinkAnswer;
    let mut t = T::new();
    t.app.open(Dest::WriteApproval);
    t.tap(Key::Menu);
    assert_eq!(t.app.take_link_answer(), Some(LinkAnswer::Allowed));
    assert_eq!(t.app.screen(), Screen::Home);
    t.app.open(Dest::WriteProgress);
    t.tap(Key::Menu);
    t.hold(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Link(_)), "keys ignored");
    assert!(!t.app.locked());
    t.tap(Key::Exit);
    assert_eq!(t.app.take_link_answer(), Some(LinkAnswer::Cancelled));
}

/// WX-4, WX-5: a decoded SAME header raises the alert with its structured fields; tests only toast; outside the
/// US and Canada, or with SAME off, nothing happens (WX-3).
#[test]
fn same_header_raises_the_alert() {
    use ferrite_core::same::parse;
    use ferrite_core::txplan::Region;
    let h = parse(b"ZCZC-WXR-SVR-024031-024033+0045-2761828-KLWX/NWS-").unwrap();
    let mut t = T::new();
    t.app.settings_mut().set_region(Some(Region::Uk));
    assert!(!t.app.same_alert(&h, 2, 16_255_000), "no SAME in the UK");
    t.app.settings_mut().set_region(Some(Region::Us));
    let utc = crate::settings::TIME_ZONES.iter().position(|&q| q == 0).unwrap() as u8;
    t.app.settings_mut().tz = utc;
    t.tap(Key::Menu); // under it: quick settings
    assert!(t.app.same_alert(&h, 2, 16_255_000));
    assert!(matches!(t.app.screen(), Screen::Alert(_)));
    let a = *t.app.alert_info;
    assert_eq!(a.event.as_bytes(), b"Severe thunderstorm warning");
    assert_eq!(a.areas.as_bytes(), b"024031 +1");
    assert_eq!(a.from.as_bytes(), b"KLWX/NWS");
    assert_eq!((a.issued, a.until), (18 * 60 + 28, 19 * 60 + 13));
    assert_eq!((a.slot, a.freq), (2, 16_255_000));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Quick(_)), "BACK: to the screen it came over");
    // A weekly test: logged and toasted, no full screen.
    t.hold(Key::Exit);
    assert!(!t.app.same_alert(&parse(b"ZCZC-WXR-RWT-024031+0015-2761200-KLWX/NWS-").unwrap(), 0, 16_240_000));
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.app.toast_up());
    // SAME off (MENU 0 4 9 1): nothing.
    t.app.settings_mut().same = false;
    assert!(!t.app.same_alert(&h, 2, 16_255_000));
}

/// Crossband has its menu number: MENU 0 3 5 (Tools) opens the crossband repeater.
#[test]
fn crossband_from_the_menu() {
    let mut t = T::new();
    menu0(&mut t, &[3, 5]);
    assert!(matches!(t.app.screen(), Screen::Crossband(_)), "{}", t.app.screen().name());
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Group(g) if g.g == 3), "BACK: the Tools list");
}

/// The screens an event opens, through the App hooks the firmware calls: power off and empty battery (modal:
/// keys and hold BACK do nothing), the unknown SSB patch, a write approval then its progress, and the mailbox
/// from the Status page.
#[test]
fn event_hooks_open_their_screens() {
    use crate::screens::info::{Info, Page};
    use crate::screens::link::{LinkAnswer, LinkPage};
    let mut t = T::new();
    t.tap(Key::Menu); // quick settings under the next ones
    t.app.ssb_patch_found();
    assert_eq!(t.app.screen(), Screen::Info(Info { page: Page::SsbConfirm }));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Quick(_)), "OK uses it and returns");
    t.app.link_write_request();
    assert!(matches!(t.app.screen(), Screen::Link(l) if l.page == LinkPage::Approve));
    t.tap(Key::Menu);
    assert_eq!(t.app.take_link_answer(), Some(LinkAnswer::Allowed));
    t.app.link_write_progress();
    assert!(matches!(t.app.screen(), Screen::Link(l) if l.page == LinkPage::Progress));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Link(_)), "keys ignored while writing (LINK-9)");
    t.app.link_write_done();
    assert!(matches!(t.app.screen(), Screen::Quick(_)));
    // The mailbox: the Status page's last row.
    t.hold(Key::Exit);
    t.tap(Key::Hash);
    t.tap(Key::Up); // wraps to the last row
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Info(Info { page: Page::Mailbox }));
    t.app.battery_empty();
    assert_eq!(t.app.screen(), Screen::Info(Info { page: Page::EmptyBattery }));
    t.app.power_off();
    t.hold(Key::Exit);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Info(Info { page: Page::PowerOff }), "modal to the end");
}

/// HF-27: each HF band keeps its own VFO (frequency, mode, step, bandwidth, fine tune) in the settings' HF
/// memory; a band never visited starts at its start.
#[test]
fn hf_bands_remember_their_vfo() {
    let mut t = T::new();
    menu0(&mut t, &[3, 4]); // 31 m, 9.580 MHz AM
    let start = t.app.set.hf.freq;
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.app.set.hf.step = 3;
    t.app.set.hf.bw = 2;
    let (f31, s31) = (t.app.set.hf.freq, t.app.set.hf.step);
    assert_ne!(f31, start);
    t.tap(Key::Right); // 25 m (AM walks the broadcast list, HF-32): first visit, its start
    assert_eq!(t.app.set.hf.freq, 1_177_000);
    t.tap(Key::Up);
    let f25 = t.app.set.hf.freq;
    t.tap(Key::Left); // back to 31 m: as it was left
    let h = t.app.set.hf;
    assert_eq!((h.freq, h.step, h.bw), (f31, s31, 2));
    t.tap(Key::Right); // and 25 m as it was left
    assert_eq!(t.app.set.hf.freq, f25);
    // In the settings: the band VFOs survive leaving HF mode.
    t.tap(Key::Exit);
    menu0(&mut t, &[3, 4]);
    assert_eq!(t.app.set.hf.freq, f25);
    t.tap(Key::Left);
    assert_eq!(t.app.set.hf.freq, f31);
}

/// PIN-6, BOOT-14: MENU held at power on is the reset, before the PIN, which it clears. The
/// questions can't be skipped past the PIN: hold BACK, PTT and the menu timeout do nothing; BACK on the first
/// goes on to the PIN.
#[test]
fn power_on_reset_before_the_pin() {
    use crate::screens::confirm::{Ask, Confirm};
    let mut t = T::new();
    t.app.set.pin_len = 4;
    t.app.set.pin_mode = 1;
    t.app.power_on_reset();
    assert_eq!(t.app.screen(), Screen::Confirm(Confirm { ask: Ask::Reset { armed: false }, boot: true, since: 0 }));
    t.hold(Key::Exit);
    t.wait(31_000);
    assert!(matches!(t.app.screen(), Screen::Confirm(_)), "hold BACK and the timeout can't skip the PIN");
    t.wake();
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Pin(_)), "BACK: the PIN, as at any power on");
    // Again, and reset: the same steps as from the menu; the PIN is gone and setup runs.
    t.app.power_on_reset();
    t.tap(Key::Menu);
    assert_eq!(t.app.set.pin_len, 4, "not before hold #");
    t.press(Key::Hash, 3200);
    assert_eq!(t.app.set.pin_len, 0, "the reset clears the PIN");
    assert!(matches!(t.app.screen(), Screen::Setup(_)));
    t.app.go_home();
    // Without a PIN, BACK on the first question is home.
    t.app.power_on_reset();
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// SETUP-11: setup page 4 writes the owner card (lower case, digits, '@' by multi-tap: KEY-12); MENU 0 7 3 3
/// turns the boot screen to it (D13), and back to the logo with #.
#[test]
fn owner_card_from_setup_to_boot() {
    use ferrite_ui::boot::{self, BootInfo};
    let mut t = T::new();
    t.app.set.set_callsign(b"va7fer");
    menu0(&mut t, &[9, 8, 1]);
    for _ in 0..3 {
        t.tap(Key::Menu); // callsign, region, owner card
    }
    assert!(matches!(t.app.screen(), Screen::Setup(Setup { page: 3, .. })), "page 4: the owner card");
    t.tap(Key::Star); // abc
    t.tap(Key::Digit(2));
    t.wait(1100);
    t.tap(Key::Digit(2));
    t.tap(Key::Digit(2));
    t.tap(Key::Down);
    t.tap(Key::Star); // 123
    for d in [5, 5, 5] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Down);
    t.tap(Key::Star); // ABC
    for _ in 0..4 {
        t.tap(Key::Digit(1)); // . - / @
    }
    let o = t.app.settings().owner;
    assert_eq!((o[0].as_bytes(), o[1].as_bytes(), o[2].as_bytes()), (&b"ab"[..], &b"555"[..], &b"@"[..]));
    assert!(t.app.settings().boot_owner().is_none(), "the logo until 0 7 3 3 says otherwise");
    t.tap(Key::Hash); // skip the rest of setup
    t.tap(Key::Menu);

    // 0 7 3 3 toggles the boot screen; the boot screen follows.
    menu0(&mut t, &[7, 3]);
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.knob(1);
    assert!(t.app.settings().boot_card);
    let mut b = Buf::<24>::new();
    assert_eq!(crate::screens::groups::wired(&t.app, &Device::default(), &[7, 3, 3], &mut b), Some(true));
    assert_eq!(b.as_bytes(), b"Owner card");
    let draw = |s: &crate::settings::Settings| {
        let mut fb = FrameBuffer::new();
        boot::draw(&mut fb, &BootInfo { version: "v0.1.0", callsign: s.boot_callsign(), line: s.boot_line(), owner: s.boot_owner(), checks: &[] });
        fb
    };
    let fb = draw(t.app.settings());
    assert!(count_px(&fb, 22, 34, |c| c != BG) > 100, "the card's first line");
    assert!(count_px(&fb, 40, 72, |c| c != BG) > 500, "the callsign, large");
    t.tap(Key::Hash);
    assert!(!t.app.settings().boot_card, "# resets to the logo");
    let fb = draw(t.app.settings());
    assert_eq!(count_px(&fb, 22, 34, |c| c != BG), 0, "the logo screen: nothing at the top");
    assert!(count_px(&fb, 40, 120, |c| c != BG) > 2000, "the logo");
}

/// D13: the owner card is user data. Fields as long as Settings holds, in characters the fonts lack, stay in
/// their 12..228 column; a failed check shows the logo screen instead (BOOT-10).
#[test]
fn owner_card_clips_and_yields_to_a_failure() {
    use ferrite_ui::boot::{self, BootInfo, Check, State};
    let mut s = crate::settings::Settings::DEFAULT;
    crate::sample::owner_card(&mut s);
    s.set_callsign(b"WWWWWWWWWWWW"); // more than Text<10> holds: cut at 10
    s.owner[0].set("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}".as_bytes()); // 20 bytes: 10 '?'
    s.owner[1].set(&[0xFF; 20]);
    s.owner[2].set(b"MMMMMMMMMMMMMMMMMMMMMMMMMM");
    let ok = [Check { label: "MCU", value: "", state: State::Ok }];
    let mut fb = FrameBuffer::new();
    fb.px.fill(0x1234);
    boot::draw(&mut fb, &BootInfo { version: "v0.1.0", callsign: s.boot_callsign(), line: s.boot_line(), owner: s.boot_owner(), checks: &ok });
    for y in 0..280 {
        for x in (0..12).chain(229..240) {
            assert_eq!(fb.at(x, y), BG, "ink outside the column at {x},{y}");
        }
    }
    assert!(count_px(&fb, 80, 96, |c| c != BG) > 100, "the unknown characters drew as '?'");
    let fail = [Check { label: "MCU", value: "", state: State::Fail }];
    boot::draw(&mut fb, &BootInfo { version: "v0.1.0", callsign: s.boot_callsign(), line: s.boot_line(), owner: s.boot_owner(), checks: &fail });
    assert_eq!(count_px(&fb, 22, 34, |c| c != BG), 0, "a failure: the logo screen, not the card");
}

/// PIN-4: 0 9 3 3 Change PIN asks for the old PIN, then the new one twice (a mismatch starts the new one again);
/// turning PIN lock off asks for the PIN.
#[test]
fn pin_change_and_off() {
    let mut t = T::new();
    let typ = |t: &mut T, ds: &[u8]| {
        for &d in ds {
            t.tap(Key::Digit(d));
        }
        t.tap(Key::Menu);
    };
    menu0(&mut t, &[9, 3]);
    t.knob(1);
    typ(&mut t, &[1, 2, 3, 4]);
    typ(&mut t, &[1, 2, 3, 4]);
    assert_eq!((t.app.settings().pin, t.app.settings().pin_mode), (1234, 1));
    // Change PIN: wrong old PIN is refused.
    t.app.go_home();
    t.app.open(crate::screens::nav::menu0(&[9, 3, 3]).expect("0 9 3 3"));
    typ(&mut t, &[5, 5, 5, 5]);
    assert_eq!(toast_head(&t), "Wrong PIN");
    typ(&mut t, &[1, 2, 3, 4]);
    typ(&mut t, &[4, 3, 2, 1]);
    typ(&mut t, &[4, 3, 2, 9]); // differs
    assert_eq!(toast_head(&t), "PINs differ");
    assert_eq!(t.app.settings().pin, 1234, "unchanged so far");
    typ(&mut t, &[4, 3, 2, 1]);
    typ(&mut t, &[4, 3, 2, 1]);
    assert_eq!((t.app.settings().pin, t.app.settings().pin_mode), (4321, 1), "the lock mode stays");
    // Lock off: the PIN first.
    t.app.go_home();
    menu0(&mut t, &[9, 3]);
    t.knob(-1);
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    assert_eq!(t.app.settings().pin_mode, 1, "not off until the PIN is right");
    typ(&mut t, &[4, 3, 2, 1]);
    assert_eq!(t.app.settings().pin_mode, 0);
}

/// Decision 86: on a channel the tone picker goes on to the channel's end tone and tail; the knob sets them, the
/// channel is saved with them, \* undoes, and # is Global.
#[test]
fn channel_end_tone_and_tail() {
    let mut t = T::new();
    let st = store();
    let z = (0..16).find(|&z| st.zone_at(z, 0).is_some_and(|c| !c.is_hf())).unwrap();
    t.app.set_channel(0, z, 0, &st);
    assert!(t.app.slot(0).on_channel());
    let _ = t.cmds();
    t.open(Dest::Tones);
    assert!(matches!(t.app.screen(), Screen::Tones(_)));
    t.tap(Key::Down); // from TX (row 1) to the end tone
    t.knob(6);
    let c = t.app.slot(0).ch.unwrap();
    assert_eq!(c.end_tone(), 6, "Quindar");
    assert!(matches!(t.app.pop_write(), Some(crate::chan::ChanWrite::Put { ch, .. }) if ch.end_tone() == 6), "saved");
    t.tap(Key::Down);
    t.knob(2);
    assert_eq!(t.app.slot(0).ch.unwrap().tail(), 2, "On");
    t.tap(Key::Hash);
    assert_eq!(t.app.slot(0).ch.unwrap().tail(), 0, "# is Global");
    t.knob(1);
    t.tap(Key::Star);
    let c = t.app.slot(0).ch.unwrap();
    assert_eq!((c.end_tone(), c.tail()), (0, 0), "undo: as before the edits (MENU-19)");
    // a VFO has only the tones' page
    t.hold(Key::Exit);
    t.app.set_vfo(0, 14_652_000);
    t.open(Dest::Tones);
    t.tap(Key::Down);
    t.tap(Key::Down);
    assert!(matches!(t.app.screen(), Screen::Tones(s) if s.row < 2));
}
