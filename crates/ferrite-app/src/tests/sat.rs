//! Satellite mode on the App (chapter 28a): the pass page's radio commands while working a pass (SAT-8, SAT-10,
//! SAT-11), the alarm's toast and the status bar's mark (SAT-12, SAT-20), with the sample satellites predicted on
//! the host as on the radio.

use super::*;
use crate::screens::sat::SatScr;
use ferrite_core::sat::data::{Live, SatData};
use ferrite_core::sat::look::Observer;
use ferrite_core::sat::pass::Pass;

/// Ottawa, as the simulator's fix.
const HERE: (i32, i32, i32) = (454_000_000, -757_000_000, 70);

/// A pass of satellite `sat` (the sample's index) over 30°, from the start of the sample's day.
fn a_pass(sat: u8) -> Pass {
    let mut d = Live::new(crate::sat::SimSats::new(crate::sat::sample()));
    d.tick(Some(crate::sat::SIM_T0), Some(Observer::new(HERE.0, HERE.1, HERE.2)), true, u32::MAX);
    (0..d.passes()).filter_map(|i| d.pass(i)).find(|p| p.sat == sat && p.max_el > 30).expect("a high pass")
}

/// An App with the sample satellites, its clock at `utc`, transmit built and allowed anywhere.
fn sat_app(utc: u32) -> T {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.app.set.extend_tx = true;
    t.app.radio.tx_built = true;
    t.app.sat_attach(std::boxed::Box::leak(std::boxed::Box::new(Live::new(crate::sat::SimSats::new(crate::sat::sample())))));
    t.app.sat_clock(Some(utc), true, Some(HERE));
    t
}

fn sat_cmds(t: &mut T) -> Vec<RadioCmd> {
    t.cmds().into_iter().filter(|c| matches!(c, RadioCmd::SatRx(_) | RadioCmd::SatUp(_) | RadioCmd::SatTone { .. } | RadioCmd::TxPlan { .. } | RadioCmd::Transmit { .. })).collect()
}

/// SAT-8, SAT-10, SAT-11: working SO-50 overhead: the downlink corrected and retuned, the uplink corrected at the
/// key-up; the first key-up of the pass sends the arming tone (74.4 Hz) for 2 s, then the access tone (67.0 Hz);
/// the next key-up the access tone only; * switches half duplex, even keyed; the trim moves the downlink.
#[test]
fn working_so50_overhead() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos + 120);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000); // predicted
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(1100);
    let c = sat_cmds(&mut t);
    let down = c.iter().rev().find_map(|c| if let RadioCmd::SatRx(f) = c { Some(*f) } else { None }).expect("the downlink tuned");
    assert!(down.abs_diff(43_679_500) < 1_200 && down != 43_679_500, "436.795 corrected by Doppler: {down}");
    let s = store();
    t.app.key_down(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    let c = sat_cmds(&mut t);
    assert!(c.contains(&RadioCmd::SatTone { tone: 744, hd: false, power: 0 }), "the arming tone first: {c:?}");
    let up = c.iter().find_map(|c| if let RadioCmd::TxPlan { tx, .. } = c { Some(*tx) } else { None }).expect("the uplink planned");
    assert!(up.abs_diff(14_585_000) < 400 && up != 14_585_000, "145.850 corrected: {up}");
    assert!(c.contains(&RadioCmd::Transmit { slot: crate::sat::SAT_SLOT, on: true }));
    assert_eq!(t.app.radio.keyed, Some(crate::sat::SAT_SLOT));
    t.wait(2500);
    let c = sat_cmds(&mut t);
    assert!(c.contains(&RadioCmd::SatTone { tone: 670, hd: false, power: 0 }), "then the access tone: {c:?} {:?} {:?} {:?}", t.app.radio.keyed, (t.app.sat.arm_until, t.app.now, t.app.sat.sent), t.app.screen());
    t.tap(Key::Star);
    assert!(sat_cmds(&mut t).contains(&RadioCmd::SatTone { tone: 670, hd: true, power: 0 }), "* while keyed: half duplex");
    t.app.key_up(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    t.wait(200);
    assert!(sat_cmds(&mut t).contains(&RadioCmd::Transmit { slot: crate::sat::SAT_SLOT, on: false }));
    t.app.key_down(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    let c = sat_cmds(&mut t);
    assert!(!c.contains(&RadioCmd::SatTone { tone: 744, hd: true, power: 0 }), "no arming tone the second time: {c:?}");
    t.app.key_up(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    t.wait(1100);
    let _ = t.cmds();
    t.tap(Key::Right);
    let c = sat_cmds(&mut t);
    let d2 = c.iter().find_map(|c| if let RadioCmd::SatRx(f) = c { Some(*f) } else { None }).expect("retuned");
    assert!((d2 as i32 - down as i32).abs() < 1_000, "the trim and a second's Doppler: {down} -> {d2}");
    assert_eq!(t.app.sat_data().prefs(1).trim, 10, "100 Hz, kept with the satellite");
    t.hold(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::SatRx(0)), "home: both chips given back (SAT-1)");
}

/// SAT-12, SAT-20: an alarm 5 minutes before the rise: the status bar's mark counts down, the toast comes at its
/// lead with the alert tone (never taking the screen), OK on it opens the pass page; the mark goes after the pass.
#[test]
fn alarm_toast_and_mark() {
    let p = a_pass(0);
    let mut t = sat_app(p.aos - 3600);
    t.app.sat_set_alarm(p, 2);
    t.wait(200);
    let m = t.app.sat_mark() & ferrite_ui::widgets::SAT_UP;
    assert!((60..=61).contains(&m), "an hour to the rise: {m}");
    t.app.sat_clock(Some(p.aos - 299), true, Some(HERE));
    t.wait(200);
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes().starts_with(b"ISS 5 min")), "the toast");
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Beep { .. })), "the alert tone (0 7 5 3 on)");
    assert_eq!(t.app.screen(), Screen::Home, "it never takes the screen");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.page == 1 && s.aos == p.aos), "OK on it: the pass page");
    t.app.sat_clock(Some(p.aos + 30), true, Some(HERE));
    t.wait(200);
    assert_eq!(t.app.sat_mark() & ferrite_ui::widgets::SAT_UP, ferrite_ui::widgets::SAT_UP, "UP while overhead");
    t.app.sat_clock(Some(p.los() + 5), true, Some(HERE));
    t.hold(Key::Exit);
    t.wait(200);
    assert_eq!(t.app.sat_mark(), 0, "gone after the pass");
}

/// SAT-22: hold MENU on a pass page opens the satellite's options: Doppler on the 70 cm side only, the uplink's
/// power (Low to start, a press each: Mid, High), duplex, the transponder, the trim back to 0; each kept with the
/// satellite and sent to the radio at once; BACK returns to the page.
#[test]
fn options_page() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos + 120);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000);
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(1100);
    t.tap(Key::Right);
    let _ = t.cmds();
    t.hold(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 1), "{:?}", t.app.screen());
    assert_eq!(t.app.sat_data().prefs(1).power, 0, "Low to start");
    t.tap(Key::Digit(2));
    t.tap(Key::Menu);
    assert_eq!(t.app.sat_data().prefs(1).power, 1);
    assert!(sat_cmds(&mut t).contains(&RadioCmd::SatTone { tone: 670, hd: false, power: 1 }), "the next key-up's power");
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert!(t.app.sat_data().prefs(1).uhf_only, "Doppler on the 70 cm side only");
    t.tap(Key::Digit(3));
    t.tap(Key::Menu);
    assert!(t.app.sat_data().prefs(1).hd);
    t.tap(Key::Digit(5));
    assert_eq!(t.app.sat_data().prefs(1).trim, 10);
    t.tap(Key::Menu);
    assert_eq!(t.app.sat_data().prefs(1).trim, 0, "OK on the trim: back to 0");
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 0 && s.page == 1), "back on the pass page");
}
