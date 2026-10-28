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
