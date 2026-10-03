//! Group D's rules: setup, the Extend TX warning, the PIN, the key test, the link prompts, FM broadcast, HF
//! mode, the alert and the advanced menu's groups.

use super::*;
use crate::screens::groups::Group;
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
    t.tap(Key::Down); // Canada
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
    t.knob(1); // PIN: at power on -> asks for a new PIN first
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    for d in [1, 2, 3, 4] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.settings().pin_mode, 1);
    assert_eq!(t.app.settings().pin_len, 4);
    t.app.ask_pin();
    let pin = t.app.screen();
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), pin, "hold BACK doesn't skip the PIN");
    let s = store();
    t.app.key_down(Key::Ptt, t.t, &s);
    t.app.key_up(Key::Ptt, t.t + 100, &s);
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
    t.app.key_down(Key::Ptt, t.t, &s);
    t.app.key_up(Key::Ptt, t.t + 100, &s);
    assert!(matches!(t.app.screen(), Screen::KeyTest(_)), "PTT is tested, not transmitted");
    t.app.raw_key(14, true); // an unmapped matrix code
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home, "hold BACK leaves");
}

/// BCR-1, BCR-16: hold 2 starts FM broadcast on the SI4732; tuning sends it; leaving (BACK or PTT) stops it.
#[test]
fn fm_broadcast() {
    let mut t = T::new();
    t.hold(Key::Digit(2));
    assert!(matches!(t.app.screen(), Screen::Fm(_)));
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_110))));
    t.knob(1);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_120))));
    t.tap(Key::Menu); // its options; 0 the full menu keeps the broadcast
    t.tap(Key::Digit(0));
    assert!(matches!(t.app.screen(), Screen::Advanced(_)));
    assert!(!t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Off)));
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Off)));
    t.hold(Key::Digit(2));
    t.cmds();
    let s = store();
    t.app.key_down(Key::Ptt, t.t, &s);
    let c = t.cmds();
    let off = c.iter().position(|c| *c == RadioCmd::Broadcast(BroadcastCmd::Off));
    assert!(off.is_some(), "PTT stops the broadcast first (BCR-16): {c:?}");
}

/// HF-1 to HF-8, HF-13, HF-24, HF-28, SBAR-18: MENU 0 3 5 enters HF mode; the status bar says HF without
/// TW/DW; everything pauses until BACK; PTT refuses on the SI4732; a frequency held 10 s joins the recent list.
#[test]
fn hf_mode() {
    let mut t = T::new();
    t.app.set.region = 1; // US: SAME on
    menu0(&mut t, &[3, 5]);
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
    t.app.key_down(Key::Ptt, t.t, &s);
    t.app.key_up(Key::Ptt, t.t + 50, &s);
    assert_eq!(t.app.screen().name(), "Transmit refused", "9.58 MHz AM is the SI4732's (HF-24)");
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Hf(_)));
    t.tap(Key::Up); // 9.589
    t.wait(10_500);
    assert_eq!(t.app.set.hf.recent[0] & 0x0FFF_FFFF, 958_900, "used 10 s: in the recent list");
    for d in [2, 8, 4, 0, 0] {
        t.tap(Key::Digit(d)); // 28400 kHz
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.set.hf.freq, 2_840_000);
    t.tap(Key::Hash);
    assert_eq!(t.app.screen().name(), "HF recent");
    t.tap(Key::Menu); // tunes the newest, back in HF mode
    assert!(matches!(t.app.screen(), Screen::Hf(_)));
    assert_eq!(t.app.set.hf.freq, 958_900);
    t.cmds();
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Hf { on: false, .. })), "leaving resumes everything (HF-4)");
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
    assert_eq!(t.app.screen(), Screen::Home);
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
    menu0(&mut t, &[3, 3]);
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 3, sub: 0, row: 2 }), "greyed: not opened");
    assert_eq!(toast_head(&t), "Weather alerts");
    t.app.set.region = 2; // Canada
    t.tap(Key::Digit(3));
    assert_eq!(t.app.screen(), Screen::Group(Group { g: 3, sub: 3, row: 0 }));
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
