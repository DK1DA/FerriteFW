//! The crossband repeater (chapter 19): the start page's grid and checks, the running page in each state, and
//! the log's words.

use super::*;
use crate::screens::xb::{self, XbLog};
use ferrite_core::msg::{ModeLive, XbCfg, XbLive};
use ferrite_ui::text::Buf;

/// The start page, ready to start: a region, a callsign, A and B on two chips.
fn start_page() -> T {
    let mut t = T::ready();
    t.app.set_callsign(b"VA3ABC");
    t.open(Dest::Crossband);
    assert_eq!(t.name(), "Crossband repeater");
    t
}

/// The repeater running, its report `x`.
fn running(x: XbLive) -> T {
    let mut t = start_page();
    t.tap(Key::Hash);
    assert!(xb::running(&t.app), "{}", t.toast());
    t.live.mode = ModeLive::Xb(x);
    t.wait(100);
    t
}

/// XB-3, XB-9: the grid's timeout steps through its list (30 s to 10 min) and power stays at Low or Mid.
#[test]
fn grid_timeout_and_power() {
    let mut t = start_page();
    assert_eq!(t.screen().mockup(), Some("XbStart.png"));
    t.reads("title=\"Repeater\"");
    t.taps(&[Key::Down, Key::Right]); // the grid's timeout
    t.app.set.xb.timeout_10s = 3;
    t.knob(-1);
    t.shows("30 s");
    t.knob(1);
    t.shows("1 min");
    t.knob(5);
    t.shows("10 min");
    t.tap(Key::Left); // power
    t.knob(3);
    assert_eq!(t.app.set.xb.power, 1, "never above Mid");
    t.shows("Mid");
}

/// XB-2: a side with squelch 0 can't start; a shift is a warning only (simplex, XB-18).
#[test]
fn checks_squelch_and_shift() {
    let mut t = start_page();
    let s1 = t.app.set.xb.side1 as usize;
    t.app.slots[s1].cfg.squelch = 0;
    t.draw();
    t.shows("Squelch is 0 on a side");
    t.shows("Can't start yet");
    t.tap(Key::Hash);
    assert!(!xb::running(&t.app));
    assert_eq!(t.toast(), " / Squelch is 0 on a side");
    t.app.slots[s1].cfg.squelch = 3;
    t.app.slots[s1].cfg.offset = 60_000;
    t.draw();
    t.shows("Simplex: shifts are ignored");
    t.shows("# Start");
}

/// XB-16, XB-17: running and ready: "READY listening both", the run time in hours and minutes, what opens each
/// side (carrier, its tone, or the tone it needs), the last pass and its direction, the strip both ways.
#[test]
fn running_ready() {
    let mut strip = [0i8; ferrite_core::msg::XB_COLS];
    (strip[3], strip[7]) = (12, -20);
    let mut t = running(XbLive { state: 0, run_s: 2 * 3600 + 5 * 60, last_dir: 2, idle_s: 90, passes: 3, busy_s: 130, strip, ..Default::default() });
    assert_eq!(t.screen().mockup(), Some("XbStart.png"));
    t.shows("READY");
    t.shows("listening both");
    t.shows("2h 5m");
    t.shows("on carrier");
    t.shows("Last");
    t.shows("1 min ago");
    t.shows("3 \u{b7} 2 min");
    t.reads("hints \"Any key\" \"Stop\" \"SIDE1\" \"Listen\"");
    let f = t.app.set.xb.timeout_10s;
    t.knob(1);
    assert_eq!(t.app.set.xb.timeout_10s, f, "the knob changes nothing while it runs");
}

/// XB-17: hang and timed out name themselves; no passes yet says so.
#[test]
fn running_hang_and_timed_out() {
    let mut t = running(XbLive { state: 3, ..Default::default() });
    t.shows("hang");
    t.shows("No passes yet");
    t.live.mode = ModeLive::Xb(XbLive { state: 4, ..Default::default() });
    t.wait(100);
    t.shows("timed out");
}

/// XB-17: with the tone gate on, a side opens on its tone, or names the one it needs when it differs.
#[test]
fn running_tone_gate() {
    let mut t = start_page();
    t.app.set.xb.flags |= XbCfg::TONE_GATE;
    let (s1, s2) = (t.app.set.xb.side1 as usize, t.app.set.xb.side2 as usize);
    t.app.slots[s1].cfg.rx_tone = 1000;
    t.app.slots[s1].cfg.tx_tone = 1000;
    t.app.slots[s2].cfg.rx_tone = 1318;
    t.app.slots[s2].cfg.tx_tone = 0;
    t.tap(Key::Hash);
    assert!(xb::running(&t.app), "{}", t.toast());
    t.live.mode = ModeLive::Xb(XbLive::default());
    t.wait(100);
    t.shows("on its tone");
    t.shows("needs T 131.8");
}

/// TX-12, XB-16: passing with the output cut back says "Power reduced", in amber.
#[test]
fn running_power_reduced() {
    let mut t = running(XbLive { state: 1, pass_s: 20, reduced: true, rssi: -70, ..Default::default() });
    t.shows("PASSING");
    t.shows("Power reduced");
    t.shows("IN");
    t.shows("OUT");
    t.shows("now");
}

/// XB-20.6: the cable log's words for who started or stopped it, and why.
#[test]
fn log_words() {
    let text = |l: XbLog| {
        let mut b = Buf::<96>::new();
        l.text(&mut b);
        String::from_utf8_lossy(b.as_bytes()).into_owned()
    };
    assert_eq!(text(XbLog { on: true, by: 0x11, why: 0 }), "started by keys:bt");
    assert_eq!(text(XbLog { on: false, by: 0x12, why: xb::XB_KEY }), "stopped by keys:term: a key");
    assert_eq!(text(XbLog { on: false, by: 0x22, why: xb::XB_CMD }), "stopped by cmd:term: xb stop");
    assert_eq!(text(XbLog { on: false, by: 0, why: xb::XB_SCREEN }), "stopped by radio: its screen closed");
    assert_eq!(text(XbLog { on: false, by: 0x10 | xb::LISTEN, why: xb::XB_REFUSED | 2 }), "stopped by keys:cable: Both sides on one chip");
    assert_eq!(text(XbLog { on: false, by: 0x21, why: xb::XB_REFUSED | 9 }), "stopped by cmd:bt: refused");
    assert_eq!(xb::by_word(0x20), "cmd:cable");
}

/// XB-2: the radio's own refusal to start shows its stop reason whole in the toast, for every reason.
#[test]
fn radio_refusals_fit() {
    for c in 1..=4 {
        let why = ferrite_core::xb::Stop::from_code(c).unwrap();
        let mut t = running(XbLive { stopped: c, ..Default::default() });
        assert!(!xb::running(&t.app), "{c}");
        assert_eq!(t.toast(), std::format!(" / {}", why.text()));
        t.draw();
    }
}
