//! Hold 0 on home (HOME-16, decided Oct 12, 2026): + knob is the screen's brightness; alone, the home channel at
//! the release.

use super::*;
use crate::lights::BRIGHTNESS;
use crate::persist::{Persist, Sink};

/// 0 goes down and stays down; the App's clock moves on by `ms` (long presses fire in the ticks).
fn hold_0(t: &mut T, ms: u32) {
    t.app.key_down(Key::Digit(0), t.t, &store());
    step(t, ms);
}

fn step(t: &mut T, ms: u32) {
    let end = t.t + ms;
    while t.t < end {
        t.t += 10;
        t.app.tick(t.t, &t.live, &store());
    }
}

fn release_0(t: &mut T) {
    t.app.key_up(Key::Digit(0), t.t, &store());
    t.wait(100);
}

fn toast(t: &T) -> (Vec<u8>, Vec<u8>) {
    let x = t.app.toast.expect("a toast");
    (x.head.as_bytes().to_vec(), x.body.as_bytes().to_vec())
}

/// A slot away from the home channel: the VFO, with home set to the first channel.
fn away_from_home(t: &mut T) -> u16 {
    let n = store().by_number(0).expect("a channel").num;
    t.app.set.home_ch = Some(n);
    if t.app.slot(0).on_channel() {
        t.tap(Key::VfoMem); // V/M: to the VFO
    }
    assert!(!t.app.slot(0).on_channel());
    n
}

/// HOME-16, DISP-16, DISP-22: while 0 is held each knob step changes the brightness by one, live (the backlight's
/// and the keypad light's duty with it), clamped at 1 and 10 with no wrap; the toast says "Brightness N" and is
/// updated in place; the slot isn't tuned. Night red has its own level (DISP-23).
#[test]
fn hold_0_and_knob_change_brightness() {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.app.set.keypad_light = 1; // with the screen
    let f = t.freq();
    assert_eq!(t.app.set.brightness, 7);
    hold_0(&mut t, 200);
    t.knob(1);
    assert_eq!(t.app.set.brightness, 8);
    assert_eq!((t.app.backlight_duty(), t.app.keypad_duty()), (BRIGHTNESS[7], BRIGHTNESS[7]), "live, both lights");
    assert_eq!(toast(&t), (b"Brightness".to_vec(), b"8".to_vec()));
    for _ in 0..5 {
        t.knob(1);
    }
    assert_eq!(t.app.set.brightness, 10, "10 at most, no wrap");
    assert_eq!(toast(&t).1, b"10");
    for _ in 0..12 {
        t.knob(-1);
    }
    assert_eq!(t.app.set.brightness, 1, "1 at least, no wrap");
    assert_eq!(t.freq(), f, "the knob didn't tune");
    assert_eq!(t.app.screen(), Screen::Home);
    release_0(&mut t);
    assert_eq!(t.app.screen(), Screen::Home, "no frequency entry: the knob used the 0");
    // Night red: its own level; the day level stays.
    t.app.set.theme = crate::settings::NIGHT_RED;
    hold_0(&mut t, 100);
    t.knob(1);
    release_0(&mut t);
    assert_eq!((t.app.set.brightness_night, t.app.set.brightness), (4, 1));
    assert_eq!(t.app.backlight_duty(), BRIGHTNESS[3]);
}

/// HOME-16: the change is saved as any setting change is: the settings' bytes carry the new level.
#[test]
fn hold_0_brightness_is_saved() {
    struct V(Vec<u8>);
    impl Sink for V {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    let mut t = T::new();
    let save = |t: &T| {
        let mut v = V(Vec::new());
        t.app.settings().save(&mut v);
        v.0
    };
    let before = save(&t);
    hold_0(&mut t, 100);
    t.knob(-2);
    release_0(&mut t);
    assert_ne!(save(&t), before);
    let mut fresh = crate::settings::Settings::DEFAULT;
    struct R<'a>(&'a [u8]);
    impl crate::persist::Source for R<'_> {
        fn take(&mut self, out: &mut [u8]) -> bool {
            if self.0.len() < out.len() {
                return false;
            }
            out.copy_from_slice(&self.0[..out.len()]);
            self.0 = &self.0[out.len()..];
            true
        }
    }
    fresh.load(&mut R(&save(&t)));
    assert_eq!(fresh.brightness, 5, "loads back at the new level");
}

/// HOME-16, HOME-8: hold 0 alone jumps to the home channel when it comes up, not at the long press; a turn of
/// the knob during the hold cancels the jump for that press, before or after the long-press time.
#[test]
fn hold_0_jumps_at_release_unless_the_knob_turned() {
    let mut t = T::new();
    let n = away_from_home(&mut t);
    hold_0(&mut t, 1500);
    assert!(!t.app.slot(0).on_channel(), "nothing at the long press while 0 is held");
    release_0(&mut t);
    assert_eq!(t.app.slot(0).ch.map(|c| c.num), Some(n));
    assert!(t.app.slot(0).on_channel(), "the home channel at the release");
    assert_eq!(t.app.toast.map(|x| x.head.as_bytes().to_vec()), Some(b"Home channel".to_vec()));
    // The knob after the long-press time: no jump.
    away_from_home(&mut t);
    hold_0(&mut t, 900);
    t.knob(1);
    step(&mut t, 500);
    release_0(&mut t);
    assert!(!t.app.slot(0).on_channel(), "the knob turned: no home channel");
    assert_eq!(t.app.screen(), Screen::Home);
    // The knob before it: no jump and no digit either.
    hold_0(&mut t, 200);
    t.knob(-1);
    step(&mut t, 900);
    release_0(&mut t);
    assert!(!t.app.slot(0).on_channel());
    assert_eq!(t.app.screen(), Screen::Home);
}

/// NAV-4, HOME-16: a short press of 0 is still a digit (entry), and a hold released before 700 ms is short.
#[test]
fn short_0_is_still_a_digit() {
    let mut t = T::new();
    t.press(Key::Digit(0), 300);
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "{:?}", t.app.screen());
}

/// KEY-14, HOME-16: locked, hold 0 + knob changes nothing and shows the lock toast, as the knob does alone.
#[test]
fn hold_0_and_knob_locked_do_nothing() {
    let mut t = T::new();
    t.hold(Key::Hash);
    assert!(t.app.locked());
    let f = t.freq();
    hold_0(&mut t, 100);
    t.knob(1);
    assert_eq!(t.app.set.brightness, 7, "unchanged");
    assert_eq!(t.freq(), f);
    assert_eq!(toast(&t).0, b"Locked:");
    release_0(&mut t);
    assert_eq!(t.app.set.brightness, 7);
}

/// HOME-16, decision 63: a 0 pressed on a dark screen only wakes it (its release and long press are ignored, so
/// no home channel), but the knob turned while it is still held changes the brightness.
#[test]
fn hold_0_from_dark_wakes_then_the_knob_is_brightness() {
    let mut t = T::new();
    t.app.set.backlight = 3; // 15 s
    let n = away_from_home(&mut t);
    t.wait(16_000);
    assert!(t.app.dark());
    hold_0(&mut t, 100);
    assert!(!t.app.dark(), "the 0 woke it");
    t.knob(-1);
    assert_eq!(t.app.set.brightness, 6);
    step(&mut t, 1000);
    release_0(&mut t);
    assert!(!t.app.slot(0).on_channel(), "no home channel");
    // Woken and held past the long press without the knob: the waking press does nothing at all.
    t.wait(16_000);
    hold_0(&mut t, 1000);
    release_0(&mut t);
    assert!(!t.app.slot(0).on_channel() && t.app.set.home_ch == Some(n));
    assert_eq!(t.app.screen(), Screen::Home);
}

/// HOME-16, APRS-19: brightness isn't the slot's, so it changes with the APRS slot active too; hold 0 alone there
/// is refused at the release, as the other slot shortcuts are.
#[test]
fn hold_0_on_the_aprs_slot() {
    let mut t = T::new();
    t.app.set_aprs_mode(true);
    t.tap(Key::Slot);
    t.tap(Key::Slot);
    assert_eq!(t.app.active(), 2);
    let f = t.freq();
    hold_0(&mut t, 100);
    t.knob(1);
    assert_eq!((t.app.set.brightness, t.freq()), (8, f));
    release_0(&mut t);
    t.app.set.home_ch = Some(store().by_number(0).unwrap().num);
    hold_0(&mut t, 800);
    assert_eq!(t.freq(), f, "nothing at the long press");
    release_0(&mut t);
    assert_eq!(t.freq(), f, "refused at the release");
    assert!(t.app.toast_up());
}

/// HOME-16, PIN-5: channel-only mode keeps hold 0 + knob (the screen's comfort, as volume and squelch), and still
/// refuses hold 0's home-channel jump, now at the release.
#[test]
fn hold_0_in_channel_only_mode() {
    let mut t = T::new();
    away_from_home(&mut t);
    t.tap(Key::VfoMem); // channel-only mode is for channels
    assert!(crate::screens::groups::change(&mut t.app, &[9, 3, 2], 1));
    assert!(t.app.set.chan_only);
    hold_0(&mut t, 100);
    t.knob(1);
    assert_eq!(t.app.set.brightness, 8);
    release_0(&mut t);
    hold_0(&mut t, 800);
    release_0(&mut t);
    assert_eq!(toast(&t).0, b"Channel-only");
}
