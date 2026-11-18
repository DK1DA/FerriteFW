//! About, System, the self-test, the calibration and mailbox pages (chapters 35, 36, 38) with a healthy radio
//! and with each lock; PTT from them.

use super::*;
use crate::screens::info::{Info, Page};
use ferrite_core::msg::CalState;

fn page(t: &mut T, page: Page) -> String {
    t.app.open(Dest::About);
    t.app.screen = Screen::Info(Info { page });
    t.draw();
    t.texts()
}

fn healthy(t: &mut T) {
    let s = &mut t.dev.selftest;
    (s.idcode, s.mcu_ok, s.flash_id, s.flash_mb, s.chips, s.si4732, s.cal) = (0x7003_0242, true, 0xEF_4017, 8, 3, true, CalState::Ok);
    t.dev.mcu_temp = Some(31);
}

/// SELF-1, BOOT-11, SBAR-20: a healthy radio: the MCU named, the flash's size and ID, transmit ready, every
/// check OK, the MCU's temperature in °C and °F.
#[test]
fn healthy_radio() {
    let mut t = T::ready();
    healthy(&mut t);
    let s = page(&mut t, Page::About);
    assert!(s.contains("AT32F403A"), "{s}");
    assert!(s.contains("8 MB \u{b7} EF4017"), "{s}");
    assert!(s.contains("Ready"), "{s}");
    let s = page(&mut t, Page::System);
    assert!(s.contains("31 \u{b0}C"), "{s}");
    t.app.set.units = 1;
    let s = page(&mut t, Page::System);
    assert!(s.contains("87 \u{b0}F"), "{s}");
    let s = page(&mut t, Page::Checks);
    assert!(s.contains("OK"), "{s}");
    let s = page(&mut t, Page::Calibration);
    assert!(s.contains("Matches"), "{s}");
}

/// BOOT-11, CAL-5: transmit locked by the calibration or a battery at cut-off; an MCU that isn't ours named by its
/// ID; a calibration changed since install.
#[test]
fn locks() {
    let mut t = T::ready();
    healthy(&mut t);
    t.dev.selftest.cal = CalState::Blank;
    assert!(page(&mut t, Page::About).contains("Locked: calibration"));
    t.dev.selftest.cal = CalState::Ok;
    t.dev.battery_cutoff = true;
    assert!(page(&mut t, Page::About).contains("Battery too low"));
    t.dev.selftest.mcu_ok = false;
    let s = page(&mut t, Page::About);
    assert!(s.contains("Unknown 70030242"), "{s}");
    t.dev.selftest.cal = CalState::CrcMismatch;
    assert!(page(&mut t, Page::Calibration).contains("Changed"));
}

/// Chapter 38: the mailbox's SSID is the callsign's, without one typed into it, -1.
#[test]
fn mailbox_ssid() {
    let mut t = T::new();
    t.app.set.set_callsign(b"VA7FER-9");
    let s = page(&mut t, Page::Mailbox);
    assert!(s.contains("VA7FER-1"), "{s}");
}

/// KEY-14, PWR-17: PTT transmits from About; never from the power-off page.
#[test]
fn ptt_from_info() {
    let mut t = T::ready();
    page(&mut t, Page::About);
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Transmit(_)), "{}", t.name());
    t.ptt(false);
    page(&mut t, Page::PowerOff);
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Info(_)), "{}", t.name());
    t.wait(500); // the bar moves
    t.ptt(false);
}

/// SELF-5: About shows the SI4732's firmware, chip and component revisions from the boot probe; "Not read" when
/// it didn't answer.
#[test]
fn si4732_revision() {
    let mut t = T::ready();
    healthy(&mut t);
    let s = page(&mut t, Page::About);
    assert!(s.contains("SI4732") && s.contains("Not read"), "{s}");
    t.dev.selftest.si_rev = *b"6000D";
    let s = page(&mut t, Page::About);
    assert!(s.contains("FW 6.0, rev D, cmp 0.0"), "{s}");
    t.dev.selftest.si_rev = [b'6', b'0', b'3', b'0', 0x01];
    let s = page(&mut t, Page::About);
    assert!(s.contains("rev ?"), "an unprintable byte: {s}");
}

/// CAL-20.1, CAL-20.2 (issue #40): a calibration changed since install shows "Changed" (not "Valid"), and About's OK asks to
/// use it as it is; OK there sends the request to the store and says it is checking. Without the change, OK on
/// About does nothing.
#[test]
fn accepting_a_changed_calibration() {
    use crate::screens::confirm::{Ask, Confirm};
    let mut t = T::ready();
    healthy(&mut t);
    page(&mut t, Page::About);
    t.reads("hints \"<>\" \"System\" \"BACK\" \"Back\"");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen, Screen::Info(_)), "nothing to accept");
    t.dev.selftest.cal = CalState::CrcMismatch;
    let s = page(&mut t, Page::About);
    assert!(s.contains("Changed: TX locked") && s.contains("Locked: calibration"), "{s}");
    t.reads("hints \"OK\" \"Calibration\"");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen, Screen::Confirm(Confirm { ask: Ask::AcceptCal, .. })));
    t.draw();
    t.shows("Use the calibration as it is?");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen, Screen::Info(Info { page: Page::About })), "back on About");
    assert_eq!(t.toast(), "Calibration / checking");
}

/// CAL-20.4: the Calibration page names the check that failed and where; a CRC that couldn't be checked reads
/// "Not checked", not "Changed", and About offers no OK for it.
#[test]
fn calibration_page_names_the_check() {
    use ferrite_core::msg::CalFault;
    let mut t = T::ready();
    healthy(&mut t);
    assert!(page(&mut t, Page::Calibration).contains("Passed"));
    t.dev.selftest.cal = CalState::Invalid;
    for (f, at, text) in [
        (CalFault::LevelOrder, [1, 5, 0], "Power order: VHF 5"),
        (CalFault::PaCurve, [3, 3, 0], "Power byte: HF 3, Low"),
        (CalFault::Windows, [0; 3], "TX windows"),
        (CalFault::Trim, [0; 3], "Crystal trim"),
        (CalFault::ToneGain, [0; 3], "Sub-tone gain"),
    ] {
        (t.dev.selftest.cal_fault, t.dev.selftest.cal_at) = (f, at);
        let s = page(&mut t, Page::Calibration);
        assert!(s.contains(text), "{f:?}: {s}");
    }
    // CAL-6.1: a band's curve fault: the sector Valid, that band locked, About says which
    (t.dev.selftest.cal, t.dev.selftest.cal_fault, t.dev.selftest.cal_at) = (CalState::Ok, CalFault::LevelOrder, [3, 1, 0]);
    let s = page(&mut t, Page::Calibration);
    assert!(s.contains("HF TX locked") && s.contains("Power order: HF 1"), "{s}");
    assert!(page(&mut t, Page::About).contains("HF locked"));
    (t.dev.selftest.cal, t.dev.selftest.cal_fault) = (CalState::CrcMismatch, CalFault::Unchecked);
    let s = page(&mut t, Page::Calibration);
    assert!(s.contains("Store not ready") && !s.contains("Changed"), "{s}");
    page(&mut t, Page::About);
    assert!(!t.app.cal_changed, "nothing to accept");
}
