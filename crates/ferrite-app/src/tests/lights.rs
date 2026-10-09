//! Backlight, LEDs, auto key lock and auto power off (decision 63, KEY-20).

use super::*;

fn dev(pct: u8, low: bool) -> Device {
    Device { battery_pct: pct, battery_low: low, ..Default::default() }
}

/// The screen goes dark after the backlight time; the key that wakes it does nothing else; PTT acts at once.
#[test]
fn dark_and_wake() {
    let mut t = T::new();
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
