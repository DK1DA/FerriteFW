//! The settings groups (chapter 11 and appendix A): rows that show their state (the power-on text, the pager's
//! frequency, the log's new count), OK on the power-on text, the calibration's names, Dry TX from the link.

use super::*;
use crate::screens::groups::{self, Group};
use ferrite_core::msg::CalState;
use ferrite_ui::text::Buf;

fn row(t: &T, path: &[u8]) -> String {
    let mut b = Buf::<24>::new();
    groups::wired(&t.app, &t.dev, path, &mut b);
    String::from_utf8_lossy(b.as_bytes()).into_owned()
}

/// BOOT-9: the power-on message row shows its text when Text is chosen; OK on it edits the text.
#[test]
fn power_on_text() {
    let mut t = T::new();
    t.app.set.pon = 2;
    t.app.set.pon_text = crate::settings::Text::EMPTY;
    for &c in b"HELLO" {
        t.app.set.pon_text.push(c);
    }
    assert_eq!(row(&t, &[7, 3, 2]), "HELLO");
    t.open(Dest::Sub(7, 3));
    t.tap(Key::Digit(2));
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Text(x) if x.to == crate::screens::text::TextFor::PowerOn), "{}", t.name());
    assert_eq!(t.app.text.as_bytes(), b"HELLO");
}

/// DEC-4, DEC-18, INBOX-6: the pager row shows its frequency while decoding; the log's row its new count.
#[test]
fn rows_with_state() {
    let mut t = T::new();
    t.app.set.pocsag_on = true;
    t.app.set.pocsag_freq = ferrite_core::freq::mhz(439) + 98_750;
    assert_eq!(row(&t, &[4, 2]), "439.987");
    t.app.log_unread = 3;
    t.open(Dest::Group(4));
    t.shows("3 new");
    assert_eq!(t.screen().mockup(), Some("MenuGroup.png"));
    assert_eq!(Screen::Group(Group { g: 4, sub: 2, row: 0 }).mockup(), None, "a third-level list has none");
}

/// CAL-5, CAL-6: the calibration state's names.
#[test]
fn calibration_names() {
    for (c, want) in [
        (CalState::Unknown, "Not checked"),
        (CalState::Ok, "Valid"),
        (CalState::Blank, "Blank: TX locked"),
        (CalState::Invalid, "Invalid: TX locked"),
        (CalState::CrcMismatch, "Changed: TX locked"),
    ] {
        assert_eq!(groups::cal_text(c), want);
    }
}

/// TX-48: Dry TX changes from the radio's own keys only, never the link's remote keys.
#[test]
fn dry_tx_not_from_the_link() {
    let mut t = T::ready();
    t.app.remote_in = true;
    assert!(!groups::change(&mut t.app, &[8, 8, 5], 1));
    assert!(!t.app.dry_tx());
    t.app.remote_in = false;
    assert!(groups::change(&mut t.app, &[8, 8, 5], 1));
    assert!(t.app.dry_tx());
}
