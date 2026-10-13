//! Backlight, LEDs, auto key lock and auto power off (decision 63, KEY-20).

use super::*;
use crate::settings::Settings;

fn dev(pct: u8, low: bool) -> Device {
    Device { battery_pct: pct, battery_low: low, ..Default::default() }
}

/// The screen goes dark after the backlight time; the key that wakes it does nothing else; PTT acts at once.
#[test]
fn dark_and_wake() {
    let mut t = T::new();
    t.app.set.backlight = 3; // 15 s: the times below are written for it
    t.wait(16_000);
    assert!(t.app.dark(), "15 s by default");
    let f = t.freq();
    t.tap(Key::Up);
    assert!(!t.app.dark());
    assert_eq!(t.freq(), f, "the waking press only wakes");
    t.tap(Key::Up);
    assert_ne!(t.freq(), f, "the next one acts");
    t.wait(16_000);
    assert!(t.app.dark());
    t.knob(1);
    assert!(!t.app.dark());
    assert_eq!(t.freq(), f + 1_250, "the knob wakes only too");
    // Receiving keeps it lit.
    t.live.slots[0].squelch_open = true;
    t.wait(30_000);
    assert!(!t.app.dark());
    // Never.
    t.live.slots[0].squelch_open = false;
    t.app.set.backlight = 0;
    t.wait(70_000);
    assert!(!t.app.dark());
}

/// LEDs: red TX, green RX; dark: the heartbeat in the battery's colour, double while something is unread.
#[test]
fn led_heartbeat() {
    let mut t = T::new();
    t.app.set.backlight = 3; // 15 s: the times below are written for it
    t.live.tx.keyed = true;
    let l = t.app.leds(&t.live, &dev(90, false));
    assert!(l.red && !l.green);
    t.live.tx.keyed = false;
    t.live.slots[1].squelch_open = true;
    let l = t.app.leds(&t.live, &dev(90, false));
    assert!(l.green && !l.red);
    t.live.slots[1].squelch_open = false;
    assert_eq!(t.app.leds(&t.live, &dev(90, false)), crate::lights::Leds::default(), "lit screen: no heartbeat");
    // Dark: sample a whole 5 s period in 10 ms steps and count the flashes and their colour.
    t.wait(16_000);
    let count = |t: &mut T, d: Device, period: u32| {
        let (mut flashes, mut on, mut colour) = (0, false, (false, false));
        while !t.t.is_multiple_of(period) {
            t.t += 10;
            t.app.tick(t.t, &t.live, &store());
        }
        for _ in 0..period / 10 {
            let l = t.app.leds(&t.live, &d);
            t.t += 10;
            t.app.tick(t.t, &t.live, &store());
            if (l.red || l.green) && !on {
                flashes += 1;
                colour = (l.red, l.green);
            }
            on = l.red || l.green;
        }
        (flashes, colour)
    };
    assert_eq!(count(&mut t, dev(90, false), 5000), (1, (false, true)), "green above 80 %");
    assert_eq!(count(&mut t, dev(50, false), 5000), (1, (true, true)), "yellow: both");
    assert_eq!(count(&mut t, dev(10, true), 5000), (1, (true, false)), "red at the low warning");
    t.app.inbox = 1;
    assert_eq!(count(&mut t, dev(90, false), 2000).0, 2, "a double flash every 2 s while unread");
    t.app.set.leds = 1;
    assert_eq!(count(&mut t, dev(90, false), 5000).0, 0, "RX and TX only: no heartbeat");
}

/// KEY-20 (and KEY-15, which names it): the keys lock after the set time without a key; receiving doesn't
/// hold it off.
#[test]
fn auto_key_lock() {
    let mut t = T::new();
    t.app.set.auto_lock = 1; // 30 s
    t.app.set.backlight = 0;
    t.live.slots[0].squelch_open = true;
    t.tap(Key::Up);
    t.wait(29_000);
    assert!(!t.app.locked());
    t.wait(2_000);
    assert!(t.app.locked());
    t.hold(Key::Hash);
    assert!(!t.app.locked(), "hold # unlocks");
}

/// Auto power off: a warning 10 s before, any key or a slot receiving starts the time again.
#[test]
fn auto_power_off() {
    let mut t = T::new();
    t.app.set.auto_off = 1; // 30 min
    t.app.set.backlight = 0;
    t.wait(29 * 60_000 + 51_000);
    assert!(t.app.toast.is_some(), "the warning");
    assert!(!t.app.take_power_off());
    t.tap(Key::Up);
    t.wait(29 * 60_000);
    assert!(!t.app.take_power_off(), "the key started it again");
    t.wait(61_000);
    assert!(t.app.take_power_off());
}

/// The keypad light (0 7 4 5).
#[test]
fn keypad_light() {
    let mut t = T::new();
    t.app.set.backlight = 3; // 15 s: the times below are written for it
    assert!(t.app.keypad_lit(), "with the screen");
    t.wait(16_000);
    assert!(!t.app.keypad_lit());
    t.app.set.keypad_light = 3;
    assert!(t.app.keypad_lit(), "always");
    t.app.set.keypad_light = 2;
    t.tap(Key::Up);
    t.tap(Key::Up);
    assert!(t.app.keypad_lit(), "after a key");
    t.wait(6_000);
    assert!(!t.app.keypad_lit());
}

/// DISP-15: 0 7 4 4 decides whether receive and transmit wake the screen and hold it lit; keys always do. Auto
/// power off still counts them whatever it says. Broadcast FM never wakes it.
#[test]
fn wake_screen_on() {
    let mut t = T::new();
    t.app.set.backlight = 3; // 15 s
    t.app.set.wake_on = 0; // keys
    t.wait(16_000);
    assert!(t.app.dark());
    t.live.slots[1].squelch_open = true;
    t.wait(1_000);
    assert!(t.app.dark(), "keys only: receive leaves it dark");
    t.live.slots[1].squelch_open = false;
    t.app.set.wake_on = 1; // keys and receive (the default)
    assert_eq!(Settings::DEFAULT.wake_on, 1);
    t.live.slots[1].squelch_open = true;
    t.wait(200);
    assert!(!t.app.dark(), "receive wakes it");
    t.wait(30_000);
    assert!(!t.app.dark(), "and holds it lit");
    t.live.slots[1].squelch_open = false;
    t.wait(16_000);
    assert!(t.app.dark());
    t.live.tx.keyed = true; // VOX or a beacon: no key pressed
    t.wait(1_000);
    assert!(t.app.dark(), "keys and receive: transmit doesn't wake it");
    t.app.set.wake_on = 2;
    t.wait(200);
    assert!(!t.app.dark(), "keys, receive and transmit");
    t.live.tx.keyed = false;
    // Broadcast FM never wakes it, nor lights the LED.
    t.wait(16_000);
    assert!(t.app.dark());
    t.live.mode = ferrite_core::msg::ModeLive::Fm { freq: 10_050, ps: [b' '; 8], rt: [0; 64], rssi: 40, snr: 20, stereo: true, heard: true };
    t.wait(1_000);
    assert!(t.app.dark(), "broadcast plays: still dark");
    assert!(!t.app.leds(&t.live, &dev(90, false)).green, "no receive LED for broadcast");
}

/// DISP-15, PWR-17: with keys only waking the screen, a slot receiving still holds auto power off off.
#[test]
fn auto_off_counts_receive_whatever_wakes() {
    let mut t = T::new();
    t.app.set.auto_off = 1; // 30 min
    t.app.set.wake_on = 0;
    t.live.slots[0].squelch_open = true;
    t.wait(31 * 60_000);
    assert!(!t.app.take_power_off());
    assert!(t.app.dark(), "the screen went dark all the same");
}

/// DISP-26: 0 7 4 3 on (the default): a lit screen stays lit through a long transmission, past the timeout; off:
/// the backlight is dark while keyed and back after, the timeout where it was.
#[test]
fn backlight_on_transmit() {
    let mut t = T::new();
    t.app.set.backlight = 3; // 15 s
    const { assert!(Settings::DEFAULT.tx_light) };
    t.wait(5_000);
    t.live.tx.keyed = true;
    t.wait(60_000);
    assert!(!t.app.dark() && t.app.backlight_duty() > 0, "lit through the transmission");
    t.live.tx.keyed = false;
    t.wait(16_000);
    assert!(t.app.dark(), "the timeout runs again after it");
    // Off: dark while keyed, whatever 0 7 4 4 says.
    t.app.set.tx_light = false;
    t.tap(Key::Up);
    assert!(t.app.backlight_duty() > 0);
    t.live.tx.keyed = true;
    t.wait(200);
    assert_eq!(t.app.backlight_duty(), 0, "dark while keyed");
    assert!(!t.app.keypad_lit(), "the keypad light with it");
    t.live.tx.keyed = false;
    t.wait(200);
    assert!(t.app.backlight_duty() > 0, "back at unkey");
}

/// DISP-16, DISP-23: 0 7 4 2's levels 1-10 as PWM duty, rising, 10 full on, 0 while dark; night red keeps its own
/// level, and 0 7 4 2 sets the one for the theme in use.
#[test]
fn brightness_levels() {
    use crate::lights::BRIGHTNESS;
    let mut t = T::new();
    t.app.set.backlight = 3;
    assert_eq!(Settings::DEFAULT.brightness, 7);
    assert_eq!(t.app.backlight_duty(), BRIGHTNESS[6]);
    assert!(BRIGHTNESS.windows(2).all(|w| w[0] < w[1]) && BRIGHTNESS[9] == 1000 && BRIGHTNESS[0] == 45);
    // Roughly geometric: each step between 1.3 and 1.5 times the last.
    assert!(BRIGHTNESS.windows(2).all(|w| (130..=150).contains(&(w[1] as u32 * 100 / w[0] as u32))));
    assert!(crate::screens::groups::change(&mut t.app, &[7, 4, 2], 5));
    assert_eq!((t.app.set.brightness, t.app.backlight_duty()), (10, 1000), "10 at most");
    // Night red: its own level (3 by default), set apart from the others'.
    t.app.set.theme = crate::settings::NIGHT_RED;
    assert_eq!(t.app.backlight_duty(), BRIGHTNESS[2]);
    assert!(crate::screens::groups::change(&mut t.app, &[7, 4, 2], -5));
    assert_eq!((t.app.set.brightness_night, t.app.set.brightness), (1, 10), "1 at least; the day level untouched");
    t.app.set.theme = 0;
    assert_eq!(t.app.backlight_duty(), 1000);
    t.wait(16_000);
    assert_eq!(t.app.backlight_duty(), 0, "dark");
}

/// DISP-22: the keypad light, when lit, at the screen's level (night red's in night red); 0 when off, and lit at
/// that level while the screen is dark with "always".
#[test]
fn keypad_light_dims_with_the_screen() {
    use crate::lights::BRIGHTNESS;
    let mut t = T::new();
    t.app.set.backlight = 3;
    t.app.set.keypad_light = 1;
    assert_eq!(t.app.keypad_duty(), t.app.backlight_duty());
    assert!(crate::screens::groups::change(&mut t.app, &[7, 4, 2], -3));
    assert_eq!((t.app.keypad_duty(), t.app.backlight_duty()), (BRIGHTNESS[3], BRIGHTNESS[3]));
    t.app.set.theme = crate::settings::NIGHT_RED;
    assert_eq!(t.app.keypad_duty(), BRIGHTNESS[t.app.set.brightness_night as usize - 1]);
    t.app.set.theme = 0;
    t.app.set.keypad_light = 0;
    assert_eq!(t.app.keypad_duty(), 0, "off");
    t.app.set.keypad_light = 3;
    t.wait(16_000);
    assert_eq!((t.app.backlight_duty(), t.app.keypad_duty()), (0, BRIGHTNESS[3]), "always: lit while the screen is dark");
    t.app.set.keypad_light = 1;
    assert_eq!(t.app.keypad_duty(), 0, "with the screen: dark with it");
}

/// DISP-22: "on key press" lights the keypad for 5 s after any key, the one that woke the screen too.
#[test]
fn keypad_light_on_key_press() {
    let mut t = T::new();
    t.app.set.backlight = 3;
    t.app.set.keypad_light = 2;
    t.wait(16_000);
    assert!(t.app.dark() && !t.app.keypad_lit());
    t.tap(Key::Up); // only wakes the screen
    assert!(!t.app.dark());
    assert!(t.app.keypad_lit(), "the waking key lights it");
    t.wait(5_000);
    assert!(!t.app.keypad_lit(), "5 s after");
    t.knob(1);
    assert!(t.app.keypad_lit(), "the knob too");
}
